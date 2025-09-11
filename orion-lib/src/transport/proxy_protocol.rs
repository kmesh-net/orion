// Copyright 2025 The kmesh Authors
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

use crate::{
    listeners::filter_state::DownstreamConnectionMetadata,
    secrets::{TlsConfigurator, WantsToBuildClient},
    transport::AsyncReadWrite,
    utils::rewindable_stream::RewindableHeadAsyncStream,
    Error, Result, SecretManager,
};
use orion_configuration::config::{
    common::{ProxyProtocolVersion, TlvType},
    listener_filters::DownstreamProxyProtocolConfig,
    transport::{PassTlvMatchType, ProxyProtocolPassThroughTlvs, TlvEntry, UpstreamProxyProtocolConfig},
};
use orion_error::Context;
use ppp::{v1, v2, HeaderResult};
use rustls::ClientConfig;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const V1_PREFIX_LEN: usize = 5;
const V1_MAX_LENGTH: usize = 107;
const V1_TERMINATOR: &[u8] = b"\r\n";

const V2_PREFIX_LEN: usize = 12;
const V2_MINIMUM_LEN: usize = 16;
const V2_LENGTH_INDEX: usize = 14;

const READ_BUFFER_LEN: usize = 512;

enum DetectedHeader {
    V1,
    V2 { extra_buffer: Option<Vec<u8>> },
    None,
}

enum PolicyAction {
    Proceed,
    Reject(String),
    TransparentPassthrough,
}

pub struct ProxyProtocolReader {
    config: Arc<DownstreamProxyProtocolConfig>,
}

impl ProxyProtocolReader {
    pub fn new(config: Arc<DownstreamProxyProtocolConfig>) -> Self {
        Self { config }
    }

    pub async fn try_read_proxy_header(
        &self,
        stream: Box<dyn AsyncReadWrite>,
        local_address: SocketAddr,
        peer_address: SocketAddr,
    ) -> Result<(DownstreamConnectionMetadata, Box<dyn AsyncReadWrite>)> {
        let mut stream = RewindableHeadAsyncStream::new(stream);
        let mut buffer = [0; READ_BUFFER_LEN];

        let maybe_header = Self::read_header_bytes(&mut stream, &mut buffer, peer_address).await?;
        let buffered_data = match self.apply_policy(&maybe_header, peer_address) {
            PolicyAction::Reject(error_msg) => {
                return Err(Error::new(error_msg));
            },
            PolicyAction::TransparentPassthrough => {
                return Ok((
                    DownstreamConnectionMetadata::FromSocket { peer_address, local_address },
                    Box::new(stream.into_rewound_stream()),
                ));
            },
            PolicyAction::Proceed => match &maybe_header {
                DetectedHeader::V2 { extra_buffer } => extra_buffer.as_deref().unwrap_or(&buffer[..]),
                _ => &buffer[..],
            },
        };

        let parsed_header = HeaderResult::parse(buffered_data);
        let metadata = self.extract_metadata(parsed_header, peer_address, local_address)?;
        Ok((metadata, stream.into_stream()))
    }

    fn should_reject_v1(&self) -> bool {
        self.config.disallowed_versions.contains(&ProxyProtocolVersion::V1)
            && !self.config.allow_requests_without_proxy_protocol
    }

    fn should_allow_v1_transparent_passthrough(&self) -> bool {
        self.config.disallowed_versions.contains(&ProxyProtocolVersion::V1)
            && self.config.allow_requests_without_proxy_protocol
    }

    fn should_reject_v2(&self) -> bool {
        self.config.disallowed_versions.contains(&ProxyProtocolVersion::V2)
            && !self.config.allow_requests_without_proxy_protocol
    }

    fn should_allow_v2_transparent_passthrough(&self) -> bool {
        self.config.disallowed_versions.contains(&ProxyProtocolVersion::V2)
            && self.config.allow_requests_without_proxy_protocol
    }

    fn apply_policy(&self, detected_header: &DetectedHeader, peer_address: SocketAddr) -> PolicyAction {
        match detected_header {
            DetectedHeader::V1 => {
                if self.should_reject_v1() {
                    PolicyAction::Reject(format!("Proxy protocol V1 is disallowed for peer {peer_address}"))
                } else if self.should_allow_v1_transparent_passthrough() {
                    PolicyAction::TransparentPassthrough
                } else {
                    PolicyAction::Proceed
                }
            },
            DetectedHeader::V2 { .. } => {
                if self.should_reject_v2() {
                    PolicyAction::Reject(format!("Proxy protocol V2 is disallowed for peer {peer_address}"))
                } else if self.should_allow_v2_transparent_passthrough() {
                    PolicyAction::TransparentPassthrough
                } else {
                    PolicyAction::Proceed
                }
            },
            DetectedHeader::None => {
                if self.config.allow_requests_without_proxy_protocol {
                    PolicyAction::TransparentPassthrough
                } else {
                    PolicyAction::Reject(format!(
                        "Connection has been configured with proxy protocol enabled, but no valid proxy protocol header detected from peer {peer_address}"
                    ))
                }
            },
        }
    }

    async fn read_header_bytes<I>(
        stream: &mut I,
        buffer: &mut [u8; READ_BUFFER_LEN],
        peer_address: SocketAddr,
    ) -> Result<DetectedHeader>
    where
        I: AsyncRead + Unpin,
    {
        stream
            .read_exact(&mut buffer[..V1_PREFIX_LEN])
            .await
            .with_context_msg(format!("Failed to read initial bytes from peer {peer_address}"))?;

        if &buffer[..V1_PREFIX_LEN] == v1::PROTOCOL_PREFIX.as_bytes() {
            let mut end_found = false;
            for i in V1_PREFIX_LEN..V1_MAX_LENGTH {
                buffer[i] =
                    stream.read_u8().await.with_context_msg("Problem reading V1 proxy protocol header bytes")?;
                if [buffer[i - 1], buffer[i]] == V1_TERMINATOR {
                    end_found = true;
                    break;
                }
            }
            if !end_found {
                return Err(Error::new("Invalid proxy protocol V1 header: terminator not found"));
            }
            Ok(DetectedHeader::V1)
        } else {
            stream
                .read_exact(&mut buffer[V1_PREFIX_LEN..V2_MINIMUM_LEN])
                .await
                .with_context_msg(format!("Problem reading additional header bytes from peer {peer_address}"))?;

            if &buffer[..V2_PREFIX_LEN] == v2::PROTOCOL_PREFIX {
                let length = u16::from_be_bytes([buffer[V2_LENGTH_INDEX], buffer[V2_LENGTH_INDEX + 1]]) as usize;
                let full_length = V2_MINIMUM_LEN + length;
                let extra_buffer = if full_length > READ_BUFFER_LEN {
                    let mut dynamic_buffer = Vec::with_capacity(full_length);
                    dynamic_buffer.extend_from_slice(&buffer[..V2_MINIMUM_LEN]);
                    stream
                        .read_exact(&mut dynamic_buffer[V2_MINIMUM_LEN..full_length])
                        .await
                        .with_context_msg("Problem reading V2 proxy protocol header into extended buffer")?;
                    Some(dynamic_buffer)
                } else {
                    stream
                        .read_exact(&mut buffer[V2_MINIMUM_LEN..full_length])
                        .await
                        .with_context_msg("Problem reading V2 proxy protocol header into buffer")?;
                    None
                };
                Ok(DetectedHeader::V2 { extra_buffer })
            } else {
                Ok(DetectedHeader::None)
            }
        }
    }

    fn extract_metadata(
        &self,
        parsed_header: HeaderResult,
        peer_address: SocketAddr,
        local_address: SocketAddr,
    ) -> Result<DownstreamConnectionMetadata> {
        match parsed_header {
            HeaderResult::V1(Ok(header)) => {
                let (original_peer_address, original_destination_address) = match header.addresses {
                    v1::Addresses::Tcp4(ip) => (
                        SocketAddr::new(IpAddr::V4(ip.source_address), ip.source_port),
                        SocketAddr::new(IpAddr::V4(ip.destination_address), ip.destination_port),
                    ),
                    v1::Addresses::Tcp6(ip) => (
                        SocketAddr::new(IpAddr::V6(ip.source_address), ip.source_port),
                        SocketAddr::new(IpAddr::V6(ip.destination_address), ip.destination_port),
                    ),
                    v1::Addresses::Unknown => {
                        return Ok(DownstreamConnectionMetadata::FromSocket { peer_address, local_address });
                    },
                };
                Ok(DownstreamConnectionMetadata::FromProxyProtocol {
                    original_peer_address,
                    original_destination_address,
                    protocol: ppp::v2::Protocol::Stream,
                    tlv_data: HashMap::new(),
                    proxy_peer_address: peer_address,
                    proxy_local_address: local_address,
                })
            },
            HeaderResult::V1(Err(error)) => {
                Err(Error::new(format!("Detected, but failed to parse proxy protocol V1 header: {error}")))
            },
            HeaderResult::V2(Ok(header)) => {
                let (original_peer_address, original_destination_address) = match header.addresses {
                    v2::Addresses::IPv4(ip) => (
                        SocketAddr::new(IpAddr::V4(ip.source_address), ip.source_port),
                        SocketAddr::new(IpAddr::V4(ip.destination_address), ip.destination_port),
                    ),
                    v2::Addresses::IPv6(ip) => (
                        SocketAddr::new(IpAddr::V6(ip.source_address), ip.source_port),
                        SocketAddr::new(IpAddr::V6(ip.destination_address), ip.destination_port),
                    ),
                    v2::Addresses::Unix(unix) => {
                        return Err(Error::new(format!("Unix socket addresses are not supported: {unix:?}")));
                    },
                    v2::Addresses::Unspecified => {
                        return Ok(DownstreamConnectionMetadata::FromSocket { peer_address, local_address });
                    },
                };
                let mut tlv_data = HashMap::new();
                if let Some(pass_through_config) = &self.config.pass_through_tlvs {
                    for tlv in header.tlvs().flatten() {
                        let tlv_type = match tlv.kind {
                            0x00 => TlvType::NoOp,
                            other => TlvType::Custom(other),
                        };
                        let should_pass_through = match pass_through_config.match_type {
                            PassTlvMatchType::IncludeAll => true,
                            PassTlvMatchType::Include => pass_through_config.tlv_types.contains(&tlv_type),
                        };
                        if should_pass_through {
                            tlv_data.insert(tlv_type, tlv.value.to_vec());
                        }
                    }
                }
                Ok(DownstreamConnectionMetadata::FromProxyProtocol {
                    original_peer_address,
                    original_destination_address,
                    protocol: header.protocol,
                    tlv_data,
                    proxy_peer_address: peer_address,
                    proxy_local_address: local_address,
                })
            },
            HeaderResult::V2(Err(error)) => {
                Err(Error::new(format!("Detected, but failed to parse proxy protocol V2 header: {error}")))
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProxyProtocolConfigurator {
    pub version: ProxyProtocolVersion,
    pub pass_through_tlvs: Option<ProxyProtocolPassThroughTlvs>,
    pub added_tlvs: Vec<TlvEntry>,
    pub inner_tls_configurator: Option<TlsConfigurator<ClientConfig, WantsToBuildClient>>,
}

impl ProxyProtocolConfigurator {
    pub fn update_secret(&mut self, secret_id: &str, secret: crate::secrets::TransportSecret) -> Result<()> {
        if let Some(inner_tls_configurator) = &self.inner_tls_configurator {
            let updated_tls = TlsConfigurator::<ClientConfig, WantsToBuildClient>::update(
                inner_tls_configurator.clone(),
                secret_id,
                secret,
            )?;
            self.inner_tls_configurator = Some(updated_tls);
        }
        Ok(())
    }

    pub async fn write_proxy_header<S>(
        &self,
        stream: &mut S,
        downstream_metadata: &DownstreamConnectionMetadata,
    ) -> Result<()>
    where
        S: AsyncWrite + Unpin,
    {
        let header = self.build_proxy_header(downstream_metadata)?;
        stream.write_all(&header).await.with_context_msg("Failed to write proxy protocol header")?;
        Ok(())
    }

    fn build_proxy_header(&self, downstream_metadata: &DownstreamConnectionMetadata) -> Result<Vec<u8>> {
        match self.version {
            ProxyProtocolVersion::V1 => Self::build_v1_header(downstream_metadata),
            ProxyProtocolVersion::V2 => self.build_v2_header(downstream_metadata),
        }
    }

    fn build_v1_header(downstream_metadata: &DownstreamConnectionMetadata) -> Result<Vec<u8>> {
        let peer_address = downstream_metadata.peer_address();
        let local_address = downstream_metadata.local_address();
        let header = match (peer_address, local_address) {
            (SocketAddr::V4(src), SocketAddr::V4(dst)) => {
                format!("PROXY TCP4 {} {} {} {}\r\n", src.ip(), dst.ip(), src.port(), dst.port())
            },
            (SocketAddr::V6(src), SocketAddr::V6(dst)) => {
                format!("PROXY TCP6 {} {} {} {}\r\n", src.ip(), dst.ip(), src.port(), dst.port())
            },
            _ => {
                return Err(Error::new("Mixed IPv4/IPv6 addresses not supported in proxy protocol v1"));
            },
        };
        Ok(header.into_bytes())
    }

    fn build_v2_header(&self, downstream_metadata: &DownstreamConnectionMetadata) -> Result<Vec<u8>> {
        let peer_address = downstream_metadata.peer_address();
        let local_address = downstream_metadata.local_address();
        let mut builder = ppp::v2::Builder::with_addresses(
            ppp::v2::Version::Two | ppp::v2::Command::Proxy,
            ppp::v2::Protocol::Stream,
            (peer_address, local_address),
        );
        for tlv_entry in &self.added_tlvs {
            let tlv_type: u8 = tlv_entry.tlv_type.clone().into();
            builder = builder
                .write_tlv(tlv_type, &tlv_entry.value)
                .map_err(|e| Error::new(format!("Failed to add configured TLV: {e}")))?;
        }
        if let DownstreamConnectionMetadata::FromProxyProtocol { tlv_data, .. } = downstream_metadata {
            if let Some(pass_through_config) = &self.pass_through_tlvs {
                for (tlv_type, value) in tlv_data {
                    let should_pass_through = match pass_through_config.match_type {
                        PassTlvMatchType::IncludeAll => true,
                        PassTlvMatchType::Include => pass_through_config.tlv_types.contains(tlv_type),
                    };
                    if should_pass_through {
                        let tlv_type: u8 = tlv_type.clone().into();
                        builder = builder
                            .write_tlv(tlv_type, value)
                            .map_err(|e| Error::new(format!("Failed to add pass-through TLV: {e}")))?;
                    }
                }
            }
        }
        builder.build().map_err(|e| Error::new(format!("Failed to build proxy protocol v2 header: {e}")))
    }
}

impl TryFrom<(UpstreamProxyProtocolConfig, &SecretManager)> for ProxyProtocolConfigurator {
    type Error = crate::Error;
    fn try_from((config, secrets): (UpstreamProxyProtocolConfig, &SecretManager)) -> Result<Self> {
        let UpstreamProxyProtocolConfig { version, pass_through_tlvs, added_tlvs, inner_tls_config } = config;
        let inner_tls_configurator = if let Some(tls_config) = inner_tls_config {
            let tls_configurator =
                TlsConfigurator::<ClientConfig, WantsToBuildClient>::try_from((tls_config, secrets))?;
            Some(tls_configurator)
        } else {
            None
        };
        Ok(Self { version, pass_through_tlvs, added_tlvs, inner_tls_configurator })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_configuration::config::transport::{PassTlvMatchType, ProxyProtocolPassThroughTlvs, TlvEntry};
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    #[allow(clippy::ref_option)]
    async fn test_proxy_protocol_v1_detection() {
        let config = DownstreamProxyProtocolConfig {
            allow_requests_without_proxy_protocol: true,
            stat_prefix: None,
            disallowed_versions: Vec::new(),
            pass_through_tlvs: None,
        };
        let local_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 54321);
        let reader = ProxyProtocolReader::new(Arc::new(config));

        let v1_header = b"PROXY TCP4 192.168.1.100 10.0.0.1 54321 80\r\n";
        let test_data = b"HTTP/1.1 GET /\r\n\r\n";
        let (mut write_side, read_side) = tokio::io::duplex(1024);
        write_side.write_all(v1_header).await.unwrap();
        write_side.write_all(test_data).await.unwrap();

        let result = reader.try_read_proxy_header(Box::new(read_side), local_addr, peer_addr).await;
        assert!(result.is_ok());

        let (metadata, _stream) = result.unwrap();
        match metadata {
            DownstreamConnectionMetadata::FromProxyProtocol {
                original_peer_address,
                original_destination_address,
                proxy_peer_address,
                proxy_local_address,
                protocol,
                ..
            } => {
                assert_eq!(original_peer_address, "192.168.1.100:54321".parse::<SocketAddr>().unwrap());
                assert_eq!(original_destination_address, "10.0.0.1:80".parse::<SocketAddr>().unwrap());
                assert_eq!(proxy_peer_address, peer_addr);
                assert_eq!(proxy_local_address, local_addr);
                assert_eq!(protocol, ppp::v2::Protocol::Stream);
            },
            _ => unreachable!("Expected FromProxyProtocol metadata"),
        }
    }

    #[tokio::test]
    #[allow(clippy::ref_option)]
    async fn test_proxy_protocol_v2_detection() {
        let config = DownstreamProxyProtocolConfig {
            allow_requests_without_proxy_protocol: true,
            stat_prefix: None,
            disallowed_versions: Vec::new(),
            pass_through_tlvs: None,
        };
        let local_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        let peer_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 54321);
        let reader = ProxyProtocolReader::new(Arc::new(config));

        let source_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 100)), 12345);
        let dest_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 50)), 443);
        let v2_header = ppp::v2::Builder::with_addresses(
            ppp::v2::Version::Two | ppp::v2::Command::Proxy,
            ppp::v2::Protocol::Stream,
            (source_addr, dest_addr),
        )
        .build()
        .unwrap();
        let test_data = b"HTTP/1.1 GET /\r\n\r\n";

        let (mut write_side, read_side) = tokio::io::duplex(1024);
        write_side.write_all(&v2_header).await.unwrap();
        write_side.write_all(test_data).await.unwrap();

        let result = reader.try_read_proxy_header(Box::new(read_side), local_addr, peer_addr).await;
        assert!(result.is_ok());

        let (metadata, _stream) = result.unwrap();
        match metadata {
            DownstreamConnectionMetadata::FromProxyProtocol {
                original_peer_address,
                original_destination_address,
                proxy_peer_address,
                proxy_local_address,
                protocol,
                ..
            } => {
                assert_eq!(original_peer_address, "172.16.0.100:12345".parse::<SocketAddr>().unwrap());
                assert_eq!(original_destination_address, "192.168.0.50:443".parse::<SocketAddr>().unwrap());
                assert_eq!(proxy_peer_address, peer_addr);
                assert_eq!(proxy_local_address, local_addr);
                assert_eq!(protocol, ppp::v2::Protocol::Stream);
            },
            _ => unreachable!("Expected FromProxyProtocol metadata"),
        }
    }

    #[tokio::test]
    #[allow(clippy::ref_option)]
    async fn test_proxy_protocol_v2_round_trip_with_tlv() {
        let original_peer = "192.168.1.100:54321".parse::<SocketAddr>().unwrap();
        let original_dest = "10.0.0.1:80".parse::<SocketAddr>().unwrap();
        let proxy_peer = "127.0.0.1:12345".parse::<SocketAddr>().unwrap();
        let proxy_local = "127.0.0.1:8080".parse::<SocketAddr>().unwrap();

        let mut incoming_tlv_data = HashMap::new();
        incoming_tlv_data.insert(TlvType::NoOp, b"noop_data".to_vec());
        incoming_tlv_data.insert(TlvType::Custom(0x01), b"custom_type_1".to_vec());
        incoming_tlv_data.insert(TlvType::Custom(0x02), b"custom_type_2".to_vec());
        incoming_tlv_data.insert(TlvType::Custom(0x03), b"should_be_filtered".to_vec());

        let initial_metadata = DownstreamConnectionMetadata::FromProxyProtocol {
            original_peer_address: original_peer,
            original_destination_address: original_dest,
            protocol: ppp::v2::Protocol::Stream,
            tlv_data: incoming_tlv_data,
            proxy_peer_address: proxy_peer,
            proxy_local_address: proxy_local,
        };

        let writer_configurator = ProxyProtocolConfigurator {
            version: ProxyProtocolVersion::V2,
            added_tlvs: vec![
                TlvEntry { tlv_type: TlvType::Custom(0x10), value: b"added_config_tlv".to_vec() },
                TlvEntry { tlv_type: TlvType::Custom(0x11), value: b"another_added_tlv".to_vec() },
            ],
            pass_through_tlvs: Some(ProxyProtocolPassThroughTlvs {
                match_type: PassTlvMatchType::Include,
                tlv_types: vec![TlvType::NoOp, TlvType::Custom(0x01), TlvType::Custom(0x02)],
            }),
            inner_tls_configurator: None,
        };

        let reader_config = DownstreamProxyProtocolConfig {
            allow_requests_without_proxy_protocol: false,
            stat_prefix: None,
            disallowed_versions: vec![],
            pass_through_tlvs: Some(ProxyProtocolPassThroughTlvs {
                match_type: PassTlvMatchType::Include,
                tlv_types: vec![
                    TlvType::NoOp,
                    TlvType::Custom(0x01),
                    TlvType::Custom(0x02),
                    TlvType::Custom(0x10),
                    TlvType::Custom(0x11),
                ],
            }),
        };

        let header_bytes = writer_configurator.build_v2_header(&initial_metadata).unwrap();

        let reader = ProxyProtocolReader::new(Arc::new(reader_config));
        let (mut write_side, read_side) = tokio::io::duplex(1024);
        write_side.write_all(&header_bytes).await.unwrap();

        let (parsed_metadata, _) =
            reader.try_read_proxy_header(Box::new(read_side), proxy_local, proxy_peer).await.unwrap();

        match parsed_metadata {
            DownstreamConnectionMetadata::FromProxyProtocol {
                original_peer_address,
                original_destination_address,
                protocol,
                tlv_data,
                proxy_peer_address,
                proxy_local_address,
            } => {
                assert_eq!(original_peer_address, original_peer);
                assert_eq!(original_destination_address, original_dest);
                assert_eq!(proxy_peer_address, proxy_peer);
                assert_eq!(proxy_local_address, proxy_local);
                assert_eq!(protocol, ppp::v2::Protocol::Stream);

                assert_eq!(tlv_data.len(), 5);

                assert_eq!(tlv_data.get(&TlvType::Custom(0x10)), Some(&b"added_config_tlv".to_vec()));
                assert_eq!(tlv_data.get(&TlvType::Custom(0x11)), Some(&b"another_added_tlv".to_vec()));

                assert_eq!(tlv_data.get(&TlvType::NoOp), Some(&b"noop_data".to_vec()));
                assert_eq!(tlv_data.get(&TlvType::Custom(0x01)), Some(&b"custom_type_1".to_vec()));
                assert_eq!(tlv_data.get(&TlvType::Custom(0x02)), Some(&b"custom_type_2".to_vec()));

                assert_eq!(tlv_data.get(&TlvType::Custom(0x03)), None);
            },
            _ => unreachable!("Expected FromProxyProtocol metadata"),
        }
    }
}
