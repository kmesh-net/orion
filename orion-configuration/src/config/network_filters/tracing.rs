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

use ::bounded_integer::BoundedU16;
use compact_str::CompactString;
use serde::{Deserialize, Serialize};

use orion_data_plane_api::envoy_data_plane_api::envoy::{
    extensions::filters::network::http_connection_manager::v3::http_connection_manager::Tracing as EnvoyTracing,
    r#type::v3::Percent,
};

use orion_data_plane_api::envoy_data_plane_api::{envoy::config::trace::v3::tracing::http, google::protobuf::Any};

use orion_data_plane_api::envoy_data_plane_api::envoy::config::trace::v3::OpenTelemetryConfig as EnvoyOpenTelemetryConfig;

use crate::config::grpc::GrpcService;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum SupportedTracingProvider {
    OpenTelemetry(OpenTelemetryConfig),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OpenTelemetryConfig {
    pub grpc_service: Option<GrpcService>,
    pub service_name: CompactString,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TracingConfig {
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub client_sampling: Option<BoundedU16<0, 100>>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub random_sampling: Option<BoundedU16<0, 100>>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub overall_sampling: Option<BoundedU16<0, 100>>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default = "Default::default")]
    pub verbose: bool,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub max_path_tag_length: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default = "Default::default")]
    pub spawn_upstream_span: bool,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub provider: Option<SupportedTracingProvider>,
}

type ListenerName = &'static str;
type FilterChainMatchHash = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TracingKey(pub ListenerName, pub FilterChainMatchHash);

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    use crate::config::{common::envoy_conversions::IsUsed, unsupported_field, GenericError};
    use compact_str::ToCompactString;
    use orion_data_plane_api::envoy_data_plane_api::prost::Message;

    use super::*;

    impl TryFrom<EnvoyTracing> for TracingConfig {
        type Error = GenericError;
        fn try_from(envoy: EnvoyTracing) -> Result<Self, Self::Error> {
            let EnvoyTracing { client_sampling, random_sampling, overall_sampling, .. } = envoy;

            let parse_percentage = |v: Option<Percent>| -> Result<Option<BoundedU16<0, 100>>, GenericError> {
                v.map(|v| -> Result<BoundedU16<0, 100>, GenericError> {
                    BoundedU16::<0, 100>::new(v.value as u16).ok_or(GenericError::from_msg(
                        "Invalid sampling percentage: expected a value between 0 and 100 (inclusive)",
                    ))
                })
                .transpose()
            };

            let client_sampling = parse_percentage(client_sampling)?;
            let random_sampling = parse_percentage(random_sampling)?;
            let overall_sampling = parse_percentage(overall_sampling)?;

            let provider = envoy
                .provider
                .as_ref()
                .and_then(|p| p.config_type.as_ref())
                .and_then(|config| {
                    let http::ConfigType::TypedConfig(config) = config;
                    Some(
                        SupportedTracingProvider::try_from(config.clone())
                            .map_err(|e| GenericError::from_msg(format!("Failed to parse tracing provider: {e}"))),
                    )
                })
                .transpose()?;

            Ok(Self {
                client_sampling,
                random_sampling,
                overall_sampling,
                verbose: envoy.verbose,
                max_path_tag_length: envoy.max_path_tag_length.map(|v| v.value),
                spawn_upstream_span: envoy.spawn_upstream_span.map(|v| v.value).unwrap_or(false),
                provider,
            })
        }
    }

    impl TryFrom<Any> for SupportedTracingProvider {
        type Error = GenericError;

        fn try_from(value: Any) -> Result<Self, Self::Error> {
            match value.type_url.as_str() {
                "type.googleapis.com/envoy.config.trace.v3.OpenTelemetryConfig" => {
                    let cfg = EnvoyOpenTelemetryConfig::decode(value.value.as_ref())
                        .map_err(|e| GenericError::from_msg(format!("Failed to decode OpenTelemetryConfig: {e}")))?;

                    cfg.try_into()
                        .map_err(|e| GenericError::from_msg(format!("Failed to convert OpenTelemetryConfig: {e}")))
                },
                _ => Err(GenericError::from_msg("Unsupported TracingProvider")),
            }
        }
    }

    impl TryFrom<EnvoyOpenTelemetryConfig> for SupportedTracingProvider {
        type Error = GenericError;

        fn try_from(value: EnvoyOpenTelemetryConfig) -> Result<Self, Self::Error> {
            let EnvoyOpenTelemetryConfig { grpc_service, http_service, service_name, resource_detectors, sampler, max_cache_size, } =
                value;
            unsupported_field!(
                // grpc_service,
                http_service,
                //service_name,
                resource_detectors,
                sampler,
                max_cache_size
            )?;

            let grpc_service = grpc_service.map(GrpcService::try_from).transpose()?;

            Ok(SupportedTracingProvider::OpenTelemetry(OpenTelemetryConfig {
                grpc_service,
                service_name: service_name.to_compact_string(),
            }))
        }
    }
}
