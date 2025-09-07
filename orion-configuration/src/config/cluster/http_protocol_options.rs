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

use crate::config::common::is_default;
use serde::{Deserialize, Serialize};
use std::{num::NonZeroU32, time::Duration};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum Codec {
    #[default]
    Http1,
    Http2,
}

impl Codec {
    pub fn is_http1(&self) -> bool {
        matches!(self, Self::Http1)
    }
    pub fn is_http2(&self) -> bool {
        matches!(self, Self::Http2)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct HttpProtocolOptions {
    #[serde(skip_serializing_if = "is_default", default)]
    pub codec: Codec,
    #[serde(skip_serializing_if = "is_default", default, flatten)]
    pub common: CommonHttpOptions,
    #[serde(skip_serializing_if = "is_default", default)]
    pub http2_options: Http2ProtocolOptions,
    #[serde(skip_serializing_if = "is_default", default)]
    pub http1_options: Http1ProtocolOptions,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct CommonHttpOptions {
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    #[serde(with = "humantime_serde")]
    pub idle_timeout: Option<Duration>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
enum UpstreamHttpProtocolOptions {
    Explicit(ExplicitProtocolOptions),
    UseDownstream(UseDownstreamProtocolOptions),
}

impl Default for UpstreamHttpProtocolOptions {
    fn default() -> Self {
        Self::Explicit(ExplicitProtocolOptions::default())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "http_version", rename_all = "UPPERCASE")]
enum ExplicitProtocolOptions {
    Http1(Http1ProtocolOptions),
    Http2(Http2ProtocolOptions),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "http_version", rename_all = "UPPERCASE")]
enum UseDownstreamProtocolOptions {
    Http1(Http1ProtocolOptions),
    Http2(Http2ProtocolOptions),
}

impl Default for ExplicitProtocolOptions {
    fn default() -> Self {
        Self::Http1(Http1ProtocolOptions)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct Http1ProtocolOptions;

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Http2ProtocolOptions {
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub keep_alive_settings: Option<Http2KeepAliveSettings>,
    // Envoy limits this to 2^31-1, h2 says 0 is valid
    // envoy accepts up from 1.
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub max_concurrent_streams: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub initial_stream_window_size: Option<NonZeroU32>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub initial_connection_window_size: Option<NonZeroU32>,
}

impl Http2ProtocolOptions {
    pub fn max_concurrent_streams(&self) -> Option<usize> {
        self.max_concurrent_streams
    }
    pub fn initial_stream_window_size(&self) -> Option<u32> {
        self.initial_stream_window_size.map(u32::from)
    }
    pub fn initial_connection_window_size(&self) -> Option<u32> {
        self.initial_connection_window_size.map(u32::from)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Http2KeepAliveSettings {
    #[serde(with = "humantime_serde")]
    pub keep_alive_interval: Duration,
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub keep_alive_timeout: Option<Duration>,
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{
        Codec, CommonHttpOptions, ExplicitProtocolOptions, Http1ProtocolOptions, Http2KeepAliveSettings,
        Http2ProtocolOptions, HttpProtocolOptions, UpstreamHttpProtocolOptions,
    };
    use crate::config::{
        cluster::http_protocol_options::UseDownstreamProtocolOptions, common::*, util::duration_from_envoy,
    };
    use orion_data_plane_api::envoy_data_plane_api::{
        envoy::{
            config::core::v3::{
                Http1ProtocolOptions as EnvoyHttp1ProtocolOptions, Http2ProtocolOptions as EnvoyHttp2ProtocolOptions,
                HttpProtocolOptions as EnvoyCommonHttpProtocolOptions, KeepaliveSettings,
            },
            extensions::upstreams::http::v3::{
                http_protocol_options::{
                    explicit_http_config::ProtocolConfig as EnvoyProtocolConfig,
                    ExplicitHttpConfig as EnvoyExplicitHttpConfig,
                    UpstreamProtocolOptions as EnvoyUpstreamProtocolOptions, UseDownstreamHttpConfig,
                },
                HttpProtocolOptions as EnvoyHttpProtocolOptions,
            },
        },
        google::protobuf::Any,
        prost::Message,
    };

    pub(crate) enum SupportedEnvoyProtocolOptions {
        HttpProtocolOptions(EnvoyHttpProtocolOptions),
    }

    impl TryFrom<Any> for SupportedEnvoyProtocolOptions {
        type Error = GenericError;
        fn try_from(typed_config: Any) -> Result<Self, Self::Error> {
            match typed_config.type_url.as_str() {
                "type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions" => {
                    EnvoyHttpProtocolOptions::decode(typed_config.value.as_slice())
                        .map(Self::HttpProtocolOptions)
                        .map_err(|e| {
                            GenericError::from_msg_with_cause(
                                format!("failed to parse protobuf for \"{}\"", typed_config.type_url),
                                e,
                            )
                        })
                },
                s => Err(GenericError::unsupported_variant(s.to_owned())),
            }
        }
    }

    impl TryFrom<Any> for HttpProtocolOptions {
        type Error = GenericError;
        fn try_from(envoy: Any) -> Result<Self, GenericError> {
            SupportedEnvoyProtocolOptions::try_from(envoy)?.try_into()
        }
    }

    impl TryFrom<SupportedEnvoyProtocolOptions> for HttpProtocolOptions {
        type Error = GenericError;
        fn try_from(value: SupportedEnvoyProtocolOptions) -> Result<Self, Self::Error> {
            match value {
                SupportedEnvoyProtocolOptions::HttpProtocolOptions(x) => x.try_into(),
            }
        }
    }

    impl TryFrom<EnvoyCommonHttpProtocolOptions> for CommonHttpOptions {
        type Error = GenericError;
        fn try_from(value: EnvoyCommonHttpProtocolOptions) -> Result<Self, Self::Error> {
            let EnvoyCommonHttpProtocolOptions {
                idle_timeout,
                max_connection_duration,
                max_headers_count,
                max_stream_duration: _,
                headers_with_underscores_action,
                max_requests_per_connection: _,
                max_response_headers_kb,
            } = value;
            unsupported_field!(
                // idle_timeout,
                max_connection_duration,
                max_headers_count,
                //max_stream_duration,
                headers_with_underscores_action,
                //max_requests_per_connection,
                max_response_headers_kb
            )?;
            let idle_timeout = idle_timeout
                .map(duration_from_envoy)
                .transpose()
                .map_err(|_| GenericError::from_msg("Failed to convert to duration"))?;
            Ok(Self { idle_timeout })
        }
    }

    impl TryFrom<EnvoyHttpProtocolOptions> for HttpProtocolOptions {
        type Error = GenericError;
        fn try_from(value: EnvoyHttpProtocolOptions) -> Result<Self, Self::Error> {
            let EnvoyHttpProtocolOptions {
                common_http_protocol_options,
                upstream_http_protocol_options,
                http_filters,
                header_validation_config,
                upstream_protocol_options,
            } = value;
            unsupported_field!(
                // common_http_protocol_options,
                upstream_http_protocol_options,
                http_filters,
                header_validation_config // upstream_protocol_options
            )?;
            let upstream_protocol_options = upstream_protocol_options
                .map(UpstreamHttpProtocolOptions::try_from)
                .transpose()
                .with_node("upstream_protocol_options")?
                .unwrap_or_default();
            let common = common_http_protocol_options.map(CommonHttpOptions::try_from).transpose()?.unwrap_or_default();
            let (codec, http1_options, http2_options) = match upstream_protocol_options {
                UpstreamHttpProtocolOptions::Explicit(ExplicitProtocolOptions::Http1(http1))
                | UpstreamHttpProtocolOptions::UseDownstream(UseDownstreamProtocolOptions::Http1(http1)) => {
                    (Codec::Http1, http1, Http2ProtocolOptions::default())
                },
                UpstreamHttpProtocolOptions::Explicit(ExplicitProtocolOptions::Http2(http2))
                | UpstreamHttpProtocolOptions::UseDownstream(UseDownstreamProtocolOptions::Http2(http2)) => {
                    (Codec::Http2, Http1ProtocolOptions, http2)
                },
            };

            Ok(Self { common, codec, http1_options, http2_options })
        }
    }

    impl TryFrom<EnvoyUpstreamProtocolOptions> for UpstreamHttpProtocolOptions {
        type Error = GenericError;
        fn try_from(value: EnvoyUpstreamProtocolOptions) -> Result<Self, Self::Error> {
            match value {
                EnvoyUpstreamProtocolOptions::ExplicitHttpConfig(envoy) => envoy.try_into().map(Self::Explicit),
                EnvoyUpstreamProtocolOptions::AutoConfig(_) => Err(GenericError::unsupported_variant("AutoConfig")),
                EnvoyUpstreamProtocolOptions::UseDownstreamProtocolConfig(envoy) => {
                    envoy.try_into().map(Self::UseDownstream)
                },
            }
        }
    }

    impl TryFrom<EnvoyExplicitHttpConfig> for ExplicitProtocolOptions {
        type Error = GenericError;
        fn try_from(value: EnvoyExplicitHttpConfig) -> Result<Self, Self::Error> {
            let EnvoyExplicitHttpConfig { protocol_config } = value;
            convert_opt!(protocol_config)
        }
    }

    impl TryFrom<UseDownstreamHttpConfig> for UseDownstreamProtocolOptions {
        type Error = GenericError;
        fn try_from(value: UseDownstreamHttpConfig) -> Result<Self, Self::Error> {
            let UseDownstreamHttpConfig { http_protocol_options, http2_protocol_options, http3_protocol_options: _ } =
                value;

            match (http_protocol_options, http2_protocol_options) {
                (None, None) => Err(GenericError::MissingField("No http options provided")),
                (None | Some(_), Some(http2_config)) => http2_config.try_into().map(Self::Http2),
                (Some(http1_config), None) => http1_config.try_into().map(Self::Http1),
            }
        }
    }

    impl TryFrom<EnvoyProtocolConfig> for ExplicitProtocolOptions {
        type Error = GenericError;
        fn try_from(value: EnvoyProtocolConfig) -> Result<Self, Self::Error> {
            match value {
                EnvoyProtocolConfig::HttpProtocolOptions(envoy) => envoy.try_into().map(Self::Http1),
                EnvoyProtocolConfig::Http2ProtocolOptions(envoy) => envoy.try_into().map(Self::Http2),
                EnvoyProtocolConfig::Http3ProtocolOptions(_) => {
                    Err(GenericError::unsupported_variant("Http3ProtocolOptions"))
                },
            }
        }
    }

    impl TryFrom<EnvoyHttp1ProtocolOptions> for Http1ProtocolOptions {
        type Error = GenericError;
        fn try_from(value: EnvoyHttp1ProtocolOptions) -> Result<Self, Self::Error> {
            let EnvoyHttp1ProtocolOptions {
                allow_absolute_url,
                accept_http_10,
                default_host_for_http_10,
                header_key_format,
                enable_trailers,
                allow_chunked_length,
                override_stream_error_on_invalid_http_message,
                send_fully_qualified_url,
                use_balsa_parser,
                allow_custom_methods,
                ignore_http_11_upgrade,
            } = value;
            unsupported_field!(
                allow_absolute_url,
                accept_http_10,
                default_host_for_http_10,
                header_key_format,
                enable_trailers,
                allow_chunked_length,
                override_stream_error_on_invalid_http_message,
                send_fully_qualified_url,
                use_balsa_parser,
                allow_custom_methods,
                ignore_http_11_upgrade
            )?;

            Ok(Self {})
        }
    }

    impl TryFrom<EnvoyHttp2ProtocolOptions> for Http2ProtocolOptions {
        type Error = GenericError;
        fn try_from(value: EnvoyHttp2ProtocolOptions) -> Result<Self, Self::Error> {
            let EnvoyHttp2ProtocolOptions {
                hpack_table_size,
                max_concurrent_streams,
                initial_stream_window_size,
                initial_connection_window_size,
                allow_connect,
                allow_metadata,
                max_outbound_frames,
                max_outbound_control_frames,
                max_consecutive_inbound_frames_with_empty_payload,
                max_inbound_priority_frames_per_stream,
                max_inbound_window_update_frames_per_data_frame_sent,
                stream_error_on_invalid_http_messaging,
                override_stream_error_on_invalid_http_message,
                custom_settings_parameters,
                connection_keepalive,
                use_oghttp2_codec,
                max_metadata_size,
            } = value;
            unsupported_field!(
                hpack_table_size,
                // max_concurrent_streams,
                // initial_stream_window_size,
                // initial_connection_window_size,
                allow_connect,
                allow_metadata,
                max_outbound_frames,
                max_outbound_control_frames,
                max_consecutive_inbound_frames_with_empty_payload,
                max_inbound_priority_frames_per_stream,
                max_inbound_window_update_frames_per_data_frame_sent,
                stream_error_on_invalid_http_messaging,
                override_stream_error_on_invalid_http_message,
                custom_settings_parameters,
                // connection_keepalive,
                use_oghttp2_codec,
                max_metadata_size
            )?;
            let max_concurrent_streams = max_concurrent_streams.map(|v| v.value as usize);
            let initial_stream_window_size = initial_stream_window_size
                .map(|v| v.value.try_into())
                .transpose()
                .map_err(|_| GenericError::from_msg("value can't be 0"))
                .with_node("initial_stream_window_size")?;
            let initial_connection_window_size = initial_connection_window_size
                .map(|v| v.value.try_into())
                .transpose()
                .map_err(|_| GenericError::from_msg("value can't be 0"))
                .with_node("initial_connection_window_size")?;
            let keep_alive_settings = connection_keepalive
                .map(|KeepaliveSettings { interval, timeout, interval_jitter, connection_idle_interval }| {
                    unsupported_field!(interval_jitter, connection_idle_interval)?;
                    Ok(Http2KeepAliveSettings {
                        keep_alive_interval: duration_from_envoy(required!(interval)?)
                            .map_err(|_| GenericError::from_msg("failed to convert into Duration"))
                            .with_node("keep_alive_interval")?,
                        keep_alive_timeout: timeout
                            .map(duration_from_envoy)
                            .transpose()
                            .map_err(|_| GenericError::from_msg("failed to convert into Duration"))
                            .with_node("keep_alive_timeout")?,
                    })
                })
                .transpose()
                .with_node("keep_alive_settings")?;

            Ok(Self {
                keep_alive_settings,
                max_concurrent_streams,
                initial_stream_window_size,
                initial_connection_window_size,
            })
        }
    }
}
