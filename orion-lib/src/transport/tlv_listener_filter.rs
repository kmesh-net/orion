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

// TLV field lengths
const TLV_TYPE_LEN: usize = 1;
const TLV_LENGTH_LEN: usize = 4;

// TLV type constants
const TLV_TYPE_SERVICE_ADDRESS: u8 = 0x1;
const TLV_TYPE_ENDING: u8 = 0xfe;

// Content lengths for service address
const TLV_TYPE_SERVICE_ADDRESS_IPV4_LEN: u32 = 6; // 4 bytes IP + 2 bytes port
const TLV_TYPE_SERVICE_ADDRESS_IPV6_LEN: u32 = 18; // 16 bytes IP + 2 bytes port

// Maximum TLV buffer length to prevent attacks
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

/// TLV listener filter implementation
///
/// This filter processes TLV (Type-Length-Value) encoded data to extract
/// original destination information from Kmesh waypoint connections.
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
    /// Create a new `TlvListenerFilter` with the specified maximum TLV length
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

    /// Process an incoming stream to extract TLV data and original destination info
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
        let mut temp_buf = vec![0u8; 512]; // Reusable buffer for reading

        loop {
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

                        if n < bytes_needed && self.buffer.len() < self.expected_length {
                            // Continue reading more data
                        }
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

            trace!("Buffer has {} bytes, expected {}", self.buffer.len(), self.expected_length);

            match self.state {
                TlvParseState::TypeAndLength => {
                    trace!("Parsing TLV type and length");

                    let tlv_type = self.buffer[self.index];

                    if tlv_type == TLV_TYPE_SERVICE_ADDRESS {
                        if self.expected_length < self.index + TLV_TYPE_LEN + TLV_LENGTH_LEN {
                            self.expected_length = self.index + TLV_TYPE_LEN + TLV_LENGTH_LEN;
                            continue;
                        }

                        trace!("Processing TLV_TYPE_SERVICE_ADDRESS");

                        let content_len = u32::from_be_bytes([
                            self.buffer[self.index + 1],
                            self.buffer[self.index + 2],
                            self.buffer[self.index + 3],
                            self.buffer[self.index + 4],
                        ]);

                        trace!("TLV content length: {}", content_len);

                        if content_len != TLV_TYPE_SERVICE_ADDRESS_IPV4_LEN
                            && content_len != TLV_TYPE_SERVICE_ADDRESS_IPV6_LEN
                        {
                            error!(
                                "Invalid TLV service address content length: {} (expected {} or {})",
                                content_len, TLV_TYPE_SERVICE_ADDRESS_IPV4_LEN, TLV_TYPE_SERVICE_ADDRESS_IPV6_LEN
                            );
                            return ReadOrParseState::Error;
                        }

                        self.expected_length = self.index + TLV_TYPE_LEN + TLV_LENGTH_LEN + content_len as usize;
                        if self.expected_length > self.max_tlv_length {
                            error!(
                                "TLV data exceeds maximum length: {} > {}",
                                self.expected_length, self.max_tlv_length
                            );
                            return ReadOrParseState::Error;
                        }

                        self.content_length = content_len;
                        self.index += TLV_TYPE_LEN + TLV_LENGTH_LEN;
                        self.state = TlvParseState::Content;
                    } else if tlv_type == TLV_TYPE_ENDING {
                        trace!("Processing TLV_TYPE_ENDING");

                        if self.expected_length < self.index + TLV_TYPE_LEN + TLV_LENGTH_LEN {
                            self.expected_length = self.index + TLV_TYPE_LEN + TLV_LENGTH_LEN;
                            continue;
                        }

                        let end_length = u32::from_be_bytes([
                            self.buffer[self.index + 1],
                            self.buffer[self.index + 2],
                            self.buffer[self.index + 3],
                            self.buffer[self.index + 4],
                        ]);

                        if end_length != 0 {
                            error!("Invalid TLV end marker length: {} (expected 0)", end_length);
                            return ReadOrParseState::Error;
                        }

                        return ReadOrParseState::Done;
                    } else {
                        error!("Invalid TLV type: {:#x}", tlv_type);
                        return ReadOrParseState::Error;
                    }
                },

                TlvParseState::Content => {
                    trace!("Parsing TLV content");

                    self.expected_length += TLV_TYPE_LEN;
                    self.index += self.content_length as usize;
                    self.state = TlvParseState::TypeAndLength;
                },
            }
        }
    }

    fn extract_original_destination(&self) -> Option<SocketAddr> {
        let mut idx = 0;
        while idx + TLV_TYPE_LEN + TLV_LENGTH_LEN <= self.buffer.len() {
            let tlv_type = self.buffer[idx];

            if tlv_type == TLV_TYPE_ENDING {
                break;
            }

            let content_len_bytes: [u8; 4] =
                self.buffer[idx + TLV_TYPE_LEN..idx + TLV_TYPE_LEN + TLV_LENGTH_LEN].try_into().ok()?;
            let content_len = u32::from_be_bytes(content_len_bytes);

            let content_start = idx + TLV_TYPE_LEN + TLV_LENGTH_LEN;
            if content_start + content_len as usize > self.buffer.len() {
                // Not enough data for content
                return None;
            }

            if tlv_type == TLV_TYPE_SERVICE_ADDRESS {
                if content_len == TLV_TYPE_SERVICE_ADDRESS_IPV4_LEN {
                    let ip_bytes: [u8; 4] = self.buffer[content_start..content_start + 4].try_into().ok()?;
                    let port_bytes: [u8; 2] = self.buffer[content_start + 4..content_start + 6].try_into().ok()?;

                    let ip = IpAddr::V4(Ipv4Addr::from(ip_bytes));
                    let port = u16::from_be_bytes(port_bytes);

                    return Some(SocketAddr::new(ip, port));
                } else if content_len == TLV_TYPE_SERVICE_ADDRESS_IPV6_LEN {
                    let mut ip_bytes = [0u8; 16];
                    ip_bytes.copy_from_slice(&self.buffer[content_start..content_start + 16]);
                    let port_bytes: [u8; 2] = self.buffer[content_start + 16..content_start + 18].try_into().ok()?;

                    let ip = IpAddr::V6(Ipv6Addr::from(ip_bytes));
                    let port = u16::from_be_bytes(port_bytes);

                    return Some(SocketAddr::new(ip, port));
                }
            }

            idx += TLV_TYPE_LEN + TLV_LENGTH_LEN + content_len as usize;
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
        tlv_data.push(TLV_TYPE_SERVICE_ADDRESS); // Type
        tlv_data.extend_from_slice(&TLV_TYPE_SERVICE_ADDRESS_IPV6_LEN.to_be_bytes()); // Length (18)

        tlv_data.extend_from_slice(&[
            0x20, 0x01, 0x0d, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        ]);
        tlv_data.extend_from_slice(&8080u16.to_be_bytes()); // Port
        tlv_data.push(TLV_TYPE_ENDING); // End marker type
        tlv_data.extend_from_slice(&0u32.to_be_bytes()); // End marker length (0)

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
