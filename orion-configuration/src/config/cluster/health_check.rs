// Copyright 2025 The kmesh Authors
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
use http::{
    uri::{Authority, PathAndQuery},
    Method,
};
use serde::{ser::SerializeStruct, Deserialize, Serialize};
use smol_str::SmolStr;
use std::{ops::Range, str::FromStr, time::Duration};

pub use super::http_protocol_options::Codec;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ClusterHealthCheck {
    /// Timeout to wait for a health check response.
    #[serde(with = "humantime_serde")]
    pub timeout: Duration,
    /// The interval between health checks.
    #[serde(with = "humantime_serde")]
    pub interval: Duration,
    /// The number of unhealthy health checks required before a host is marked unhealthy.
    pub unhealthy_threshold: u16,
    /// The number of healthy health checks required before a host is marked healthy.
    pub healthy_threshold: u16,
    /// Reuse health check connection between health checks. Default is `true`.
    // pub reuse_connection: bool,
    /// If specified, Envoy will start health checking after for a random time between 0 and `initial_jitter`.
    /// This only applies to the first health check.
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub initial_jitter: Option<Duration>,
    /// If specified, during every interval Envoy will add a random time between 0 and `interval_jitter` to the wait time.
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub interval_jitter: Option<Duration>,
    /// An optional jitter amount from 0.0 to 1.0.
    /// If specified, during every interval we will add `interval` * `interval_jitter_percent` to the wait time.
    /// If `interval_jitter` and `interval_jitter_percent` are both set, both of them will be used to increase the wait time.
    #[serde(skip_serializing_if = "f32_is_zero", default = "Default::default")]
    pub interval_jitter_percent: f32,
    /// The “unhealthy interval” is a health check interval that is used for hosts that are marked as unhealthy.
    /// As soon as the host is marked as healthy, Envoy will shift back to using the standard health check
    /// interval that is defined.
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub unhealthy_interval: Option<Duration>,
    /// The “unhealthy edge interval” is a special health check interval that is used for the first health check
    /// right after a host is marked as unhealthy. For subsequent health checks Envoy will shift back to using either
    /// “unhealthy interval” if present or the standard health check interval that is defined.
    /// The default value for “unhealthy edge interval” is the same as “unhealthy interval”.
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub unhealthy_edge_interval: Option<Duration>,
    /// The “healthy edge interval” is a special health check interval that is used for the first health check
    /// right after a host is marked as healthy. For subsequent health checks Envoy will shift back to using
    /// the standard health check interval that is defined.
    /// The default value for “healthy edge interval” is the same as the default interval.
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub healthy_edge_interval: Option<Duration>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn f32_is_zero(x: &f32) -> bool {
    *x == 0.0
}

impl ClusterHealthCheck {
    pub fn new(timeout: Duration, interval: Duration, unhealthy_threshold: u16, healthy_threshold: u16) -> Self {
        ClusterHealthCheck {
            timeout,
            interval,
            unhealthy_threshold,
            healthy_threshold,
            initial_jitter: None,
            interval_jitter: None,
            interval_jitter_percent: 0.0,
            unhealthy_interval: None,
            unhealthy_edge_interval: None,
            healthy_edge_interval: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthCheckProtocol {
    Http(HttpHealthCheck),
    Tcp(TcpHealthCheck),
    Grpc(GrpcHealthCheck),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HealthCheck {
    #[serde(flatten)]
    pub cluster: ClusterHealthCheck,
    #[serde(flatten)]
    pub protocol: HealthCheckProtocol,
}

impl<'de> Deserialize<'de> for HealthCheckProtocol {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "protocol", content = "protocol_settings", rename_all = "UPPERCASE")]
        enum ProtocolSerde {
            Http(Option<HttpHealthCheck>),
            Tcp(Option<TcpHealthCheck>),
            Grpc(Option<GrpcHealthCheck>),
        }

        ProtocolSerde::deserialize(deserializer).map(|protocol| match protocol {
            ProtocolSerde::Http(http) => HealthCheckProtocol::Http(http.unwrap_or_default()),
            ProtocolSerde::Tcp(tcp) => HealthCheckProtocol::Tcp(tcp.unwrap_or_default()),
            ProtocolSerde::Grpc(grpc) => HealthCheckProtocol::Grpc(grpc.unwrap_or_default()),
        })
    }
}

impl Serialize for HealthCheckProtocol {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        const TAG: &str = "protocol";
        const CONTENT: &str = "protocol_settings";
        let mut s = serializer.serialize_struct("Protocol", 2)?;
        match self {
            Self::Http(_) => s.serialize_field(TAG, "HTTP")?,
            Self::Tcp(_) => s.serialize_field(TAG, "TCP")?,
            Self::Grpc(_) => s.serialize_field(TAG, "GRPC")?,
        }
        match self {
            Self::Http(http) => {
                if is_default(http) {
                    s.skip_field(CONTENT)?
                } else {
                    s.serialize_field(CONTENT, http)?
                }
            },
            Self::Tcp(tcp) => {
                if is_default(tcp) {
                    s.skip_field(CONTENT)?
                } else {
                    s.serialize_field(CONTENT, tcp)?
                }
            },
            Self::Grpc(grpc) => {
                if is_default(grpc) {
                    s.skip_field(CONTENT)?
                } else {
                    s.serialize_field(CONTENT, grpc)?
                }
            },
        }
        s.end()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HttpHealthCheck {
    #[serde(skip_serializing_if = "is_default", default)]
    pub http_version: Codec,
    #[serde(with = "http_serde_ext::method", skip_serializing_if = "is_default", default)]
    pub method: Method,
    #[serde(with = "http_serde_ext::authority::option", skip_serializing_if = "is_default", default)]
    pub host: Option<Authority>,
    //todo(hayley): should be Range<StatusCode> perhaps but would require some custom serde shenanigans
    // see for example https://stackoverflow.com/a/72484080
    // n.b. for the inner StatusCode we can use http_serde_ext
    // since Vec<Range> is a very specific type we might want to fully wrap it in another type even that's essentially
    // a HashSet
    #[serde(skip_serializing_if = "is_default_expected_statuses", default = "default_expected_statuses")]
    pub expected_statuses: Vec<Range<u16>>,
    #[serde(skip_serializing_if = "is_default", default)]
    pub retriable_statuses: Vec<Range<u16>>,
    #[serde(with = "http_serde_ext::path_and_query::option", skip_serializing_if = "is_default", default)]
    pub path: Option<PathAndQuery>,
}

#[allow(clippy::single_range_in_vec_init)]
fn default_expected_statuses() -> Vec<Range<u16>> {
    vec![200..201]
}

fn is_default_expected_statuses(value: &Vec<Range<u16>>) -> bool {
    *value == default_expected_statuses()
}

impl Default for HttpHealthCheck {
    #[allow(clippy::single_range_in_vec_init)]
    fn default() -> Self {
        Self {
            http_version: Codec::Http1,
            host: None,
            method: Method::GET,
            expected_statuses: default_expected_statuses(),
            retriable_statuses: Vec::new(),
            path: None,
        }
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Cluster name is not a valid host name")]
pub struct ClusterHostnameError;

impl HttpHealthCheck {
    pub fn host(&self, cluster_name: &str) -> Result<Authority, ClusterHostnameError> {
        // todo(hayley): validate that this order is correct.
        //  looking at the envoy docs for the http health check it says the following about the host field
        // > The value of the host header in the HTTP health check request. If
        // > left empty (default value), the name of the cluster this health check is associated
        // > with will be used. The host header can be customized for a specific endpoint by setting the
        // > :ref:`hostname <envoy_v3_api_field_config.endpoint.v3.Endpoint.HealthCheckConfig.hostname>` field.
        //  which could imply it's 2->1->3 instead

        // The `host` field of the HTTP request comes from:
        // 1. The HttpHealthCheck.host field, or
        // 2. The HealthCheckConfig.hostname field, or
        //NOTE(hayley): this^^^ is not implemented
        // 3. The cluster's name.
        if let Some(host) = &self.host {
            //ideally all of the options here would be a headervalue and we wouldn't need to error out
            Ok(host.to_owned())
        } else {
            Authority::from_str(cluster_name).map_err(|_| ClusterHostnameError)
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct TcpHealthCheck {
    pub send: Option<Vec<u8>>,
    pub receive: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct GrpcHealthCheck {
    pub service_name: SmolStr,
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{
        default_expected_statuses, Codec, GrpcHealthCheck, HealthCheck, HealthCheckProtocol, HttpHealthCheck,
        TcpHealthCheck,
    };
    use crate::config::{common::*, util::duration_from_envoy};
    use http::{
        uri::{Authority, PathAndQuery},
        Method,
    };
    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        config::core::v3::{
            health_check::{
                payload::Payload as EnvoyPayload, GrpcHealthCheck as EnvoyGrpcHealthCheck,
                HealthChecker as EnvoyHealthChecker, HttpHealthCheck as EnvoyHttpHealthCheck,
                Payload as EnvoyPayloadOption, TcpHealthCheck as EnvoyTcpHealthCheck,
            },
            HealthCheck as EnvoyHealthCheck, RequestMethod,
        },
        r#type::v3::{CodecClientType, Int64Range},
    };
    use std::{ops::Range, str::FromStr};

    impl TryFrom<EnvoyHealthCheck> for HealthCheck {
        type Error = GenericError;
        fn try_from(value: EnvoyHealthCheck) -> Result<Self, Self::Error> {
            let EnvoyHealthCheck {
                timeout,
                interval,
                initial_jitter,
                interval_jitter,
                interval_jitter_percent,
                unhealthy_threshold,
                healthy_threshold,
                alt_port,
                reuse_connection,
                no_traffic_interval,
                no_traffic_healthy_interval,
                unhealthy_interval,
                unhealthy_edge_interval,
                healthy_edge_interval,
                event_log_path,
                event_logger,
                event_service,
                always_log_health_check_failures,
                tls_options,
                transport_socket_match_criteria,
                health_checker,
                always_log_health_check_success,
            } = value;
            unsupported_field!(
                // timeout,
                // interval,
                // initial_jitter,
                // interval_jitter,
                // interval_jitter_percent,
                // unhealthy_threshold,
                // healthy_threshold,
                alt_port,
                reuse_connection,
                no_traffic_interval,
                no_traffic_healthy_interval,
                // unhealthy_interval,
                // unhealthy_edge_interval,
                // healthy_edge_interval,
                event_log_path,
                event_logger,
                event_service,
                always_log_health_check_failures,
                tls_options,
                transport_socket_match_criteria,
                always_log_health_check_success // health_checker
            )?;
            let timeout = duration_from_envoy(required!(timeout)?).map_err(|e| {
                GenericError::from_msg_with_cause("failed to convert {timeout} to std::time::Duration", e)
                    .with_node("timeout")
            })?;
            let interval = duration_from_envoy(required!(interval)?).map_err(|e| {
                GenericError::from_msg_with_cause("failed to convert {interval} to std::time::Duration", e)
                    .with_node("interval")
            })?;
            let initial_jitter = initial_jitter.map(duration_from_envoy).transpose().map_err(|e| {
                GenericError::from_msg_with_cause("failed to convert {initial_jitter} to std::time::Duration", e)
                    .with_node("initial_jitter")
            })?;
            let interval_jitter = interval_jitter.map(duration_from_envoy).transpose().map_err(|e| {
                GenericError::from_msg_with_cause("failed to convert {interval_jitter} to std::time::Duration", e)
                    .with_node("interval_jitter")
            })?;
            let interval_jitter_percent = {
                #[allow(clippy::cast_precision_loss)]
                let as_float = interval_jitter_percent as f32;
                if !as_float.is_finite() || as_float > 1.0 || as_float.is_sign_negative() {
                    Err(GenericError::from_msg(format!(
                        "Invalid jitter percentage {as_float}. Jitter percentage has to be within the range [0.0, 1.0]"
                    )))
                } else {
                    Ok(as_float)
                }
            }
            .with_node("interval_jitter_percent")?;
            let unhealthy_threshold = required!(unhealthy_threshold)?.value;
            let unhealthy_threshold = unhealthy_threshold
                .try_into()
                .map_err(|_| {
                    GenericError::from_msg(format!("invalid value {unhealthy_threshold}. Must be less than 65536."))
                })
                .with_node("unhealthy_threshold")?;
            let healthy_threshold = required!(healthy_threshold)?.value;
            let healthy_threshold = healthy_threshold
                .try_into()
                .map_err(|_| {
                    GenericError::from_msg(format!("invalid value {healthy_threshold}. Must be less than 65536."))
                })
                .with_node("healthy_threshold")?;
            let unhealthy_interval = unhealthy_interval.map(duration_from_envoy).transpose().map_err(|e| {
                GenericError::from_msg_with_cause("failed to convert {unhealthy_interval} to std::time::Duration", e)
                    .with_node("unhealthy_interval")
            })?;
            let unhealthy_edge_interval =
                unhealthy_edge_interval.map(duration_from_envoy).transpose().map_err(|e| {
                    GenericError::from_msg_with_cause(
                        "failed to convert {unhealthy_edge_interval} to std::time::Duration",
                        e,
                    )
                    .with_node("unhealthy_edge_interval")
                })?;
            let healthy_edge_interval = healthy_edge_interval.map(duration_from_envoy).transpose().map_err(|e| {
                GenericError::from_msg_with_cause("failed to convert {healthy_edge_interval} to std::time::Duration", e)
                    .with_node("healthy_edge_interval")
            })?;
            let protocol = convert_opt!(health_checker)?;
            Ok(Self {
                cluster: super::ClusterHealthCheck {
                    timeout,
                    interval,
                    initial_jitter,
                    interval_jitter,
                    interval_jitter_percent,
                    unhealthy_threshold,
                    healthy_threshold,
                    unhealthy_interval,
                    unhealthy_edge_interval,
                    healthy_edge_interval,
                },
                protocol,
            })
        }
    }

    impl TryFrom<EnvoyHealthChecker> for HealthCheckProtocol {
        type Error = GenericError;
        fn try_from(value: EnvoyHealthChecker) -> Result<Self, Self::Error> {
            match value {
                EnvoyHealthChecker::HttpHealthCheck(envoy) => envoy.try_into().map(Self::Http),
                EnvoyHealthChecker::TcpHealthCheck(envoy) => Ok(Self::Tcp(envoy.try_into()?)),
                EnvoyHealthChecker::CustomHealthCheck(_) => Err(GenericError::unsupported_variant("CustomHealthCheck")),
                EnvoyHealthChecker::GrpcHealthCheck(_) => Err(GenericError::unsupported_variant("GrpcHealthCheck")),
            }
        }
    }

    fn status_range_vec_from_i64_rang_vec(statuses: Vec<Int64Range>) -> Result<Vec<Range<u16>>, GenericError> {
        statuses
            .into_iter()
            .map(|Int64Range { start, end }| {
                if start >= end {
                    Err(GenericError::from_msg(format!(
                        "invalid range [{start},{end}). End has to be greater than end"
                    )))
                } else if start < 100 || end >= 600 {
                    Err(GenericError::from_msg("invalid range [{start},{end}). Range has to be within [100,600)."))
                } else {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    Ok((start as u16)..(end as u16))
                }
            })
            .collect()
    }

    impl TryFrom<EnvoyHttpHealthCheck> for HttpHealthCheck {
        type Error = GenericError;
        fn try_from(value: EnvoyHttpHealthCheck) -> Result<Self, Self::Error> {
            let EnvoyHttpHealthCheck {
                host,
                path,
                send,
                receive,
                response_buffer_size,
                request_headers_to_add,
                request_headers_to_remove,
                expected_statuses,
                retriable_statuses,
                codec_client_type,
                service_name_matcher,
                method,
            } = value;
            unsupported_field!(
                //  host,
                // path,
                send,
                receive,
                response_buffer_size,
                request_headers_to_add,
                request_headers_to_remove,
                // expected_statuses,
                // retriable_statuses,
                // codec_client_type,
                service_name_matcher // method
            )?;
            let method = RequestMethod::from_i32(method)
                .ok_or_else(|| GenericError::unsupported_variant(format!("[unknown RequestMethod value {method}")))
                .with_node("method")?;
            let method = match method {
                RequestMethod::Get | RequestMethod::MethodUnspecified => Ok(Method::GET),
                x => Err(GenericError::unsupported_variant(format!("{x:?}"))),
            }
            .with_node("method")?;
            let http_version = CodecClientType::from_i32(codec_client_type)
                .ok_or_else(|| GenericError::unsupported_variant(format!("[unknown RequestMethod value {method}")))
                .with_node("codec_client_type")?;
            let http_version = match http_version {
                CodecClientType::Http1 => Ok(Codec::Http1),
                CodecClientType::Http2 => Ok(Codec::Http2),
                CodecClientType::Http3 => Err(GenericError::unsupported_variant("Http3")),
            }
            .with_node("codec_client_type")?;
            let host = host
                .is_used()
                .then(|| Authority::from_str(&host))
                .transpose()
                .map_err(|e| {
                    GenericError::from_msg_with_cause(format!("Failed to convert \"{host}\" to a HeaderValue"), e)
                })
                .with_node("host")?;

            let expected_statuses = if expected_statuses.is_empty() {
                default_expected_statuses()
            } else {
                status_range_vec_from_i64_rang_vec(expected_statuses).with_node("expected_statuses")?
            };
            let retriable_statuses =
                status_range_vec_from_i64_rang_vec(retriable_statuses).with_node("retriable_statuses")?;
            let path = path
                .is_used()
                .then(|| {
                    let path_and_query = PathAndQuery::from_str(&path).map_err(|e| {
                        GenericError::from_msg_with_cause(format!("Failed to parse \"{path}\" as Path"), e)
                    })?;
                    if path_and_query.query().is_some() {
                        Err(GenericError::from_msg("path can't contain query"))
                    } else {
                        Ok(path_and_query)
                    }
                })
                .transpose()
                .with_node("path")?;
            Ok(Self { path, method, http_version, host, expected_statuses, retriable_statuses })
        }
    }

    fn try_convert_payload(payload: EnvoyPayloadOption) -> Option<Result<Vec<u8>, GenericError>> {
        let EnvoyPayloadOption { payload } = payload;

        payload.map(|payload| match payload {
            EnvoyPayload::Text(text) => try_convert_text_payload(&text),
            EnvoyPayload::Binary(binary) => Ok(binary),
        })
    }

    // The documentation doesn't specify the details of this conversion, so this is based on:
    // https://github.com/envoyproxy/envoy/blob/v1.32.1/source/common/common/hex.cc
    fn try_convert_text_payload(text_payload: &str) -> Result<Vec<u8>, GenericError> {
        // This check guarantees that the match below doesn't panic
        if text_payload.len() % 2 != 0 {
            return Err(GenericError::from_msg("invalid text payload with odd number of characters"));
        }

        let mut bytes = Vec::with_capacity(text_payload.len() / 2);
        let mut chars = text_payload.chars();
        loop {
            match (chars.next(), chars.next()) {
                (Some(msb), Some(lsb)) => bytes.push(parse_hex_chars(msb, lsb)?),
                (Some(_), None) | (None, Some(_)) => {
                    // This case should not happen because we check the length at the beginning
                    unreachable!("unexpected number of characters in text payload")
                },
                (None, None) => break,
            }
        }

        Ok(bytes)
    }

    fn parse_hex_chars(msb: char, lsb: char) -> Result<u8, GenericError> {
        // char::to_digit(16) is better than u8::from_str_radix(s, 16) because the latter accepts an initial '+'
        match (msb.to_digit(16), lsb.to_digit(16)) {
            (Some(msb), Some(lsb)) if msb <= 0xf && lsb <= 0xf => {
                // this cast is valid because the match checks the upper bound
                #[allow(clippy::cast_possible_truncation)]
                let byte = ((msb << 4) + lsb) as u8;
                Ok(byte)
            },
            _ => Err(GenericError::from_msg("invalid text payload")),
        }
    }

    impl TryFrom<EnvoyTcpHealthCheck> for TcpHealthCheck {
        type Error = GenericError;

        fn try_from(value: EnvoyTcpHealthCheck) -> Result<Self, Self::Error> {
            let EnvoyTcpHealthCheck { send, receive, proxy_protocol_config } = value;
            unsupported_field!(proxy_protocol_config)?;
            Ok(TcpHealthCheck {
                send: send.and_then(try_convert_payload).transpose()?,
                receive: receive.into_iter().filter_map(try_convert_payload).collect::<Result<Vec<_>, _>>()?,
            })
        }
    }

    impl TryFrom<EnvoyGrpcHealthCheck> for GrpcHealthCheck {
        type Error = GenericError;
        fn try_from(value: EnvoyGrpcHealthCheck) -> Result<Self, Self::Error> {
            let EnvoyGrpcHealthCheck { service_name, authority, initial_metadata } = value;
            unsupported_field!(authority, initial_metadata)?;

            Ok(GrpcHealthCheck { service_name: service_name.into() })
        }
    }

    #[cfg(test)]
    mod health_check_tests {
        use crate::config::cluster::health_check::envoy_conversions::try_convert_text_payload;

        fn assert_parsed(payload: &str, expected: &[u8]) {
            assert_eq!(try_convert_text_payload(payload).expect("payload parsing failed"), expected);
        }

        fn assert_err(payload: &str) {
            assert!(try_convert_text_payload(payload).is_err(), "payload should be invalid");
        }

        #[test]
        fn text_payload_parsing() {
            assert_parsed("", &[]);

            for byte in 0..u8::MAX {
                assert_parsed(&format!("{byte:02x}"), &[byte]);
            }

            assert_parsed("0000", &[0x0, 0x0]);
            assert_parsed("0001", &[0x0, 0x1]);
            assert_parsed("abba", &[0xab, 0xba]);

            assert_err("0");
            assert_err("000");
            assert_err("00000");
            assert_err("+000");
            assert_err("-000");
            assert_err("zz");
        }
    }
}
