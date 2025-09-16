// SPDX-FileCopyrightText: © 2025 kmesh authors
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 kmesh authors
//
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
//

use crate::listeners::filter_state::DownstreamConnectionMetadata;
use crate::transport::AsyncStream;
use crate::{Error, Result};
use std::io::{Error as IoError, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::io::AsyncReadExt;
use tracing::{debug, error, trace};

const TLV_TYPE_LEN: usize = 1;
const TLV_LENGTH_LEN: usize = 4;

const TLV_TYPE_SERVICE_ADDRESS: u8 = 0x1;
const TLV_TYPE_ENDING: u8 = 0xfe;

const TLV_TYPE_SERVICE_ADDRESS_IPV4_LEN: u32 = 6;
const TLV_TYPE_SERVICE_ADDRESS_IPV6_LEN: u32 = 18;

const MAX_TLV_LENGTH: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TlvParseState {
    TypeAndLength,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadOrParseState {
    Done,
    Error,
    SkipFilter,
}

pub struct TlvListenerFilter {
    state: TlvParseState,
    expected_length: usize,
    index: usize,
    content_length: u32,
    max_tlv_length: usize,
    buffer: Vec<u8>,
}

impl Default for TlvListenerFilter {
    fn default() -> Self {
        Self::new(MAX_TLV_LENGTH)
    }
}

impl TlvListenerFilter {
    pub fn new(max_tlv_length: usize) -> Self {
        Self {
            state: TlvParseState::TypeAndLength,
            expected_length: TLV_TYPE_LEN + TLV_LENGTH_LEN,
            index: 0,
            content_length: 0,
            max_tlv_length,
            buffer: Vec::new(),
        }
    }

    fn read_u32_be(&self, offset: usize) -> Result<u32> {
        if offset + 4 > self.buffer.len() {
            return Err(Error::from(IoError::new(ErrorKind::InvalidData, "Not enough data for u32")));
        }
        Ok(u32::from_be_bytes([
            self.buffer[offset],
            self.buffer[offset + 1],
            self.buffer[offset + 2],
            self.buffer[offset + 3],
        ]))
    }

    fn read_u16_be(&self, offset: usize) -> Result<u16> {
        if offset + 2 > self.buffer.len() {
            return Err(Error::from(IoError::new(ErrorKind::InvalidData, "Not enough data for u16")));
        }
        Ok(u16::from_be_bytes([self.buffer[offset], self.buffer[offset + 1]]))
    }

    fn validate_service_address_length(content_length: u32) -> bool {
        content_length == TLV_TYPE_SERVICE_ADDRESS_IPV4_LEN || content_length == TLV_TYPE_SERVICE_ADDRESS_IPV6_LEN
    }

    fn parse_ipv4_address(&self, content_start: usize) -> Result<SocketAddr> {
        if content_start + 6 > self.buffer.len() {
            return Err(Error::from(IoError::new(ErrorKind::InvalidData, "Not enough data for IPv4 address")));
        }

        let ip_bytes: [u8; 4] = self.buffer[content_start..content_start + 4]
            .try_into()
            .map_err(|_| IoError::new(ErrorKind::InvalidData, "Invalid IPv4 bytes"))?;
        let port = self.read_u16_be(content_start + 4)?;

        let ip = IpAddr::V4(Ipv4Addr::from(ip_bytes));
        Ok(SocketAddr::new(ip, port))
    }

    fn parse_ipv6_address(&self, content_start: usize) -> Result<SocketAddr> {
        if content_start + 18 > self.buffer.len() {
            return Err(Error::from(IoError::new(ErrorKind::InvalidData, "Not enough data for IPv6 address")));
        }

        let mut ip_bytes = [0u8; 16];
        ip_bytes.copy_from_slice(&self.buffer[content_start..content_start + 16]);
        let port = self.read_u16_be(content_start + 16)?;

        let ip = IpAddr::V6(Ipv6Addr::from(ip_bytes));
        Ok(SocketAddr::new(ip, port))
    }

    fn process_service_address_tlv(&mut self) -> ReadOrParseState {
        if self.expected_length < self.index + TLV_TYPE_LEN + TLV_LENGTH_LEN {
            self.expected_length = self.index + TLV_TYPE_LEN + TLV_LENGTH_LEN;
            return ReadOrParseState::Done;
        }

        trace!("Processing TLV_TYPE_SERVICE_ADDRESS");

        let content_length = match self.read_u32_be(self.index + 1) {
            Ok(len) => len,
            Err(_) => return ReadOrParseState::Error,
        };

        trace!("TLV content length: {}", content_length);

        if !Self::validate_service_address_length(content_length) {
            error!(
                "Invalid TLV service address content length: {} (expected {} or {})",
                content_length, TLV_TYPE_SERVICE_ADDRESS_IPV4_LEN, TLV_TYPE_SERVICE_ADDRESS_IPV6_LEN
            );
            return ReadOrParseState::Error;
        }

        self.expected_length = self.index + TLV_TYPE_LEN + TLV_LENGTH_LEN + content_length as usize;
        if self.expected_length > self.max_tlv_length {
            error!("TLV data exceeds maximum length: {} > {}", self.expected_length, self.max_tlv_length);
            return ReadOrParseState::Error;
        }

        self.content_length = content_length;
        self.index += TLV_TYPE_LEN + TLV_LENGTH_LEN;
        self.state = TlvParseState::Content;
        ReadOrParseState::Done
    }

    fn process_ending_tlv(&mut self) -> ReadOrParseState {
        trace!("Processing TLV_TYPE_ENDING");

        if self.expected_length < self.index + TLV_TYPE_LEN + TLV_LENGTH_LEN {
            self.expected_length = self.index + TLV_TYPE_LEN + TLV_LENGTH_LEN;
            return ReadOrParseState::Done;
        }

        let end_length = match self.read_u32_be(self.index + 1) {
            Ok(len) => len,
            Err(_) => return ReadOrParseState::Error,
        };

        if end_length != 0 {
            error!("Invalid TLV end marker length: {} (expected 0)", end_length);
            return ReadOrParseState::Error;
        }

        ReadOrParseState::Done
    }

    fn process_tlv_content(&mut self) -> ReadOrParseState {
        trace!("Parsing TLV content");
        self.expected_length += TLV_TYPE_LEN;
        self.index += self.content_length as usize;
        self.state = TlvParseState::TypeAndLength;
        ReadOrParseState::Done
    }

    async fn read_required_data(&mut self, stream: &mut AsyncStream) -> ReadOrParseState {
        let mut temp_buf = vec![0u8; 512];

        while self.buffer.len() < self.expected_length {
            let bytes_needed = self.expected_length - self.buffer.len();
            let read_size = bytes_needed.min(temp_buf.len());

            match stream.read(&mut temp_buf[..read_size]).await {
                Ok(0) => {
                    if self.buffer.len() >= self.expected_length {
                        break;
                    }
                    trace!("Connection closed while reading TLV data, skipping filter");
                    return ReadOrParseState::SkipFilter;
                },
                Ok(n) => {
                    self.buffer.extend_from_slice(&temp_buf[..n]);
                },
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    trace!("No data available for TLV, skipping filter");
                    return ReadOrParseState::SkipFilter;
                },
                Err(e) => {
                    error!("Error reading TLV data: {}", e);
                    return ReadOrParseState::Error;
                },
            }
        }

        ReadOrParseState::Done
    }

    pub async fn process_stream(
        &mut self,
        mut stream: AsyncStream,
        local_address: SocketAddr,
        peer_address: SocketAddr,
    ) -> Result<(AsyncStream, DownstreamConnectionMetadata)> {
        trace!("Starting TLV processing");

        match self.read_and_parse_tlv(&mut stream).await {
            ReadOrParseState::Done => {
                trace!("TLV parsing completed successfully");

                if let Some(original_dest) = self.extract_original_destination() {
                    debug!("Extracted original destination: {}", original_dest);
                    let metadata = DownstreamConnectionMetadata::FromTlv {
                        original_destination_address: original_dest,
                        tlv_data: std::collections::HashMap::new(),
                        proxy_peer_address: peer_address,
                        proxy_local_address: local_address,
                    };
                    Ok((stream, metadata))
                } else {
                    let metadata = DownstreamConnectionMetadata::FromSocket { peer_address, local_address };
                    Ok((stream, metadata))
                }
            },
            ReadOrParseState::SkipFilter => {
                trace!("Skipping TLV filter");
                let metadata = DownstreamConnectionMetadata::FromSocket { peer_address, local_address };
                Ok((stream, metadata))
            },
            ReadOrParseState::Error => {
                error!("Error parsing TLV data");
                Err(Error::from(IoError::new(ErrorKind::InvalidData, "Failed to parse TLV data")))
            },
        }
    }

    async fn read_and_parse_tlv(&mut self, stream: &mut AsyncStream) -> ReadOrParseState {
        loop {
            let read_result = self.read_required_data(stream).await;
            match read_result {
                ReadOrParseState::Done => {},
                other => return other,
            }

            trace!("Buffer has {} bytes, expected {}", self.buffer.len(), self.expected_length);

            match self.state {
                TlvParseState::TypeAndLength => {
                    trace!("Parsing TLV type and length");

                    let tlv_type = self.buffer[self.index];

                    let parse_result = match tlv_type {
                        TLV_TYPE_SERVICE_ADDRESS => self.process_service_address_tlv(),
                        TLV_TYPE_ENDING => return self.process_ending_tlv(),
                        _ => {
                            error!("Invalid TLV type: {:#x}", tlv_type);
                            ReadOrParseState::Error
                        },
                    };

                    match parse_result {
                        ReadOrParseState::Done => {},
                        other => return other,
                    }
                },
                TlvParseState::Content => {
                    let parse_result = self.process_tlv_content();
                    match parse_result {
                        ReadOrParseState::Done => {},
                        other => return other,
                    }
                },
            }
        }
    }

    fn extract_original_destination(&self) -> Option<SocketAddr> {
        let mut current_index = 0;

        while current_index + TLV_TYPE_LEN + TLV_LENGTH_LEN <= self.buffer.len() {
            let tlv_type = self.buffer[current_index];

            if tlv_type == TLV_TYPE_ENDING {
                break;
            }

            let content_length = match self.read_u32_be(current_index + TLV_TYPE_LEN) {
                Ok(len) => len,
                Err(_) => return None,
            };

            let content_start = current_index + TLV_TYPE_LEN + TLV_LENGTH_LEN;
            if content_start + content_length as usize > self.buffer.len() {
                // Not enough data for content
                return None;
            }

            if tlv_type == TLV_TYPE_SERVICE_ADDRESS {
                let socket_addr = match content_length {
                    TLV_TYPE_SERVICE_ADDRESS_IPV4_LEN => self.parse_ipv4_address(content_start).ok(),
                    TLV_TYPE_SERVICE_ADDRESS_IPV6_LEN => self.parse_ipv6_address(content_start).ok(),
                    _ => None,
                };

                if let Some(addr) = socket_addr {
                    return Some(addr);
                }
            }

            current_index += TLV_TYPE_LEN + TLV_LENGTH_LEN + content_length as usize;
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::AsyncRead;

    struct CursorAdapter {
        cursor: Cursor<Vec<u8>>,
    }

    impl AsyncRead for CursorAdapter {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let n = match std::io::Read::read(&mut self.cursor, buf.initialize_unfilled()) {
                Ok(n) => n,
                Err(e) => return std::task::Poll::Ready(Err(e)),
            };
            buf.advance(n);
            std::task::Poll::Ready(Ok(()))
        }
    }

    impl tokio::io::AsyncWrite for CursorAdapter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "write not supported")))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn test_ipv4_tlv_parsing() {
        let mut filter = TlvListenerFilter::new(256);

        let mut tlv_data = Vec::new();
        tlv_data.push(TLV_TYPE_SERVICE_ADDRESS);
        tlv_data.extend_from_slice(&TLV_TYPE_SERVICE_ADDRESS_IPV4_LEN.to_be_bytes());
        tlv_data.extend_from_slice(&[192, 168, 1, 100]);
        tlv_data.extend_from_slice(&8080u16.to_be_bytes());
        tlv_data.push(TLV_TYPE_ENDING);
        tlv_data.extend_from_slice(&0u32.to_be_bytes());

        let cursor_adapter = CursorAdapter { cursor: Cursor::new(tlv_data) };
        let stream = Box::new(cursor_adapter) as AsyncStream;

        let local_addr = "0.0.0.0:80".parse().unwrap();
        let peer_addr = "10.0.0.1:12345".parse().unwrap();

        let (_stream, metadata) = filter.process_stream(stream, local_addr, peer_addr).await.unwrap();

        match metadata {
            DownstreamConnectionMetadata::FromTlv { original_destination_address, .. } => {
                assert_eq!(original_destination_address, "192.168.1.100:8080".parse::<SocketAddr>().unwrap());
            },
            other => {
                unreachable!("Expected FromTlv metadata, got: {:?}", other);
            },
        }
    }

    #[tokio::test]
    async fn test_ipv6_tlv_parsing() {
        let mut filter = TlvListenerFilter::new(256);

        let mut tlv_data = Vec::new();
        tlv_data.push(TLV_TYPE_SERVICE_ADDRESS);
        tlv_data.extend_from_slice(&TLV_TYPE_SERVICE_ADDRESS_IPV6_LEN.to_be_bytes());

        tlv_data.extend_from_slice(&[
            0x20, 0x01, 0x0d, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        ]);
        tlv_data.extend_from_slice(&8080u16.to_be_bytes());
        tlv_data.push(TLV_TYPE_ENDING);
        tlv_data.extend_from_slice(&0u32.to_be_bytes());

        let cursor_adapter = CursorAdapter { cursor: Cursor::new(tlv_data) };
        let stream = Box::new(cursor_adapter) as AsyncStream;

        let local_addr = "[::]:80".parse().unwrap();
        let peer_addr = "[2001:db8::2]:12345".parse().unwrap();

        let (_stream, metadata) = filter.process_stream(stream, local_addr, peer_addr).await.unwrap();

        match metadata {
            DownstreamConnectionMetadata::FromTlv { original_destination_address, .. } => {
                assert_eq!(original_destination_address, "[2001:db8::1]:8080".parse::<SocketAddr>().unwrap());
            },
            other => {
                unreachable!("Expected FromTlv metadata, got: {:?}", other);
            },
        }
    }
    #[tokio::test]
    async fn test_invalid_tlv_type() {
        let mut filter = TlvListenerFilter::new(256);

        let tlv_data = vec![0x99, 0x00, 0x00, 0x00, 0x04, 0x01, 0x02, 0x03, 0x04];

        let cursor_adapter = CursorAdapter { cursor: Cursor::new(tlv_data) };
        let stream = Box::new(cursor_adapter) as AsyncStream;

        let local_addr = "0.0.0.0:80".parse().unwrap();
        let peer_addr = "10.0.0.1:12345".parse().unwrap();

        let result = filter.process_stream(stream, local_addr, peer_addr).await;
        assert!(result.is_err());
    }
}
