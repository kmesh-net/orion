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

use crate::config::{common::ProxyProtocolVersion, transport::ProxyProtocolPassThroughTlvs};
use compact_str::CompactString;
use serde::{Deserialize, Serialize};

pub struct ListenerFilter {
    pub name: CompactString,
    pub config: ListenerFilterConfig,
}

pub enum ListenerFilterConfig {
    TlsInspector,
    ProxyProtocol(DownstreamProxyProtocolConfig),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct DownstreamProxyProtocolConfig {
    #[serde(default)]
    pub allow_requests_without_proxy_protocol: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stat_prefix: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub disallowed_versions: Vec<ProxyProtocolVersion>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub pass_through_tlvs: Option<ProxyProtocolPassThroughTlvs>,
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{DownstreamProxyProtocolConfig, ListenerFilter, ListenerFilterConfig};
    use crate::config::{
        common::{ProxyProtocolVersion, *},
        transport::ProxyProtocolPassThroughTlvs,
    };
    use compact_str::CompactString;
    use orion_data_plane_api::envoy_data_plane_api::{
        envoy::{
            config::listener::v3::{
                listener_filter::ConfigType as EnvoyListenerFilterConfigType, ListenerFilter as EnvoyListenerFilter,
            },
            extensions::filters::listener::{
                proxy_protocol::v3::ProxyProtocol as EnvoyProxyProtocol,
                tls_inspector::v3::TlsInspector as EnvoyTlsInspector,
            },
        },
        google::protobuf::Any,
        prost::Message,
    };
    #[derive(Debug, Clone)]
    enum SupportedEnvoyListenerFilter {
        TlsInspector(EnvoyTlsInspector),
        ProxyProtocol(EnvoyProxyProtocol),
    }

    impl TryFrom<Any> for SupportedEnvoyListenerFilter {
        type Error = GenericError;
        fn try_from(typed_config: Any) -> Result<Self, Self::Error> {
            match typed_config.type_url.as_str() {
                "type.googleapis.com/envoy.extensions.filters.listener.tls_inspector.v3.TlsInspector" => {
                    EnvoyTlsInspector::decode(typed_config.value.as_slice()).map(Self::TlsInspector)
                },
                "type.googleapis.com/envoy.extensions.filters.listener.proxy_protocol.v3.ProxyProtocol" => {
                    EnvoyProxyProtocol::decode(typed_config.value.as_slice()).map(Self::ProxyProtocol)
                },
                _ => {
                    return Err(GenericError::unsupported_variant(typed_config.type_url));
                },
            }
            .map_err(|e| {
                GenericError::from_msg_with_cause(
                    format!("failed to parse protobuf for \"{}\"", typed_config.type_url),
                    e,
                )
            })
        }
    }

    impl TryFrom<Any> for ListenerFilterConfig {
        type Error = GenericError;
        fn try_from(typed_config: Any) -> Result<Self, Self::Error> {
            SupportedEnvoyListenerFilter::try_from(typed_config)?.try_into()
        }
    }
    impl TryFrom<EnvoyListenerFilter> for ListenerFilter {
        type Error = GenericError;
        fn try_from(envoy: EnvoyListenerFilter) -> Result<Self, Self::Error> {
            let EnvoyListenerFilter { name, filter_disabled, config_type } = envoy;
            unsupported_field!(filter_disabled)?;
            let name: CompactString = required!(name)?.into();
            (|| -> Result<_, GenericError> {
                let config = match required!(config_type) {
                    Ok(EnvoyListenerFilterConfigType::ConfigDiscovery(_)) => {
                        Err(GenericError::unsupported_variant("ConfigDiscovery"))
                    },
                    Ok(EnvoyListenerFilterConfigType::TypedConfig(typed_config)) => {
                        ListenerFilterConfig::try_from(typed_config)
                    },
                    Err(e) => Err(e),
                }?;
                Ok(Self { name: name.clone(), config })
            })()
            .with_node("config_type")
            .with_name(name)
        }
    }

    impl TryFrom<SupportedEnvoyListenerFilter> for ListenerFilterConfig {
        type Error = GenericError;
        fn try_from(value: SupportedEnvoyListenerFilter) -> Result<Self, Self::Error> {
            match value {
                SupportedEnvoyListenerFilter::TlsInspector(EnvoyTlsInspector {
                    enable_ja3_fingerprinting,
                    initial_read_buffer_size,
                    enable_ja4_fingerprinting,
                }) => {
                    // both fields are optional, and unsupported, but serde_yaml requires that at least one field is populated
                    // so allow for enable_ja3_fingerprinting: false
                    unsupported_field!(initial_read_buffer_size, enable_ja4_fingerprinting)?;
                    if enable_ja3_fingerprinting.is_some_and(|b| b.value) {
                        return Err(GenericError::UnsupportedField("enable_ja3_fingerprinting"));
                    }
                    Ok(Self::TlsInspector)
                },
                SupportedEnvoyListenerFilter::ProxyProtocol(envoy_proxy_protocol) => {
                    let config = DownstreamProxyProtocolConfig::try_from(envoy_proxy_protocol)?;
                    Ok(Self::ProxyProtocol(config))
                },
            }
        }
    }

    impl TryFrom<EnvoyProxyProtocol> for DownstreamProxyProtocolConfig {
        type Error = GenericError;
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        fn try_from(value: EnvoyProxyProtocol) -> Result<Self, Self::Error> {
            let EnvoyProxyProtocol {
                rules,
                allow_requests_without_proxy_protocol,
                pass_through_tlvs,
                disallowed_versions,
                stat_prefix,
            } = value;
            unsupported_field!(rules)?;
            let stat_prefix = if stat_prefix.is_empty() { None } else { Some(stat_prefix) };
            let disallowed_versions = disallowed_versions
                .into_iter()
                .map(|v| match v {
                    0 => Ok(ProxyProtocolVersion::V1),
                    1 => Ok(ProxyProtocolVersion::V2),
                    other => Err(GenericError::from_msg(format!("Unsupported proxy protocol version: {other}"))),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let pass_through_tlvs = pass_through_tlvs.map(ProxyProtocolPassThroughTlvs::try_from).transpose()?;
            Ok(Self { allow_requests_without_proxy_protocol, stat_prefix, disallowed_versions, pass_through_tlvs })
        }
    }
}
