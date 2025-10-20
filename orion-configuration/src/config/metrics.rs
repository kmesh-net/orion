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
use crate::config::{common::envoy_conversions::IsUsed, grpc::GrpcService, unsupported_field, GenericError};
use orion_data_plane_api::envoy_data_plane_api::{
    envoy::extensions::stat_sinks::open_telemetry::v3::SinkConfig as EnvoySinkConfig, google::protobuf::Any,
    prost::Message,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StatsSink {
    OpenTelemetry(SinkConfig),
}

impl TryFrom<Any> for StatsSink {
    type Error = GenericError;

    fn try_from(typed_config: Any) -> Result<Self, Self::Error> {
        match typed_config.type_url.as_str() {
            "type.googleapis.com/envoy.extensions.stat_sinks.open_telemetry.v3.SinkConfig" => {
                let sink_config = EnvoySinkConfig::decode(typed_config.value.as_slice()).map_err(|e| {
                    GenericError::from_msg_with_cause(
                        format!("failed to parse protobuf for \"{}\"", typed_config.type_url),
                        e,
                    )
                })?;
                SinkConfig::try_from(sink_config).map(Self::OpenTelemetry)
            },
            _ => Err(GenericError::unsupported_variant(typed_config.type_url)),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SinkConfig {
    pub grpc_service: GrpcService,
    pub prefix: String,
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    use super::*;

    impl TryFrom<EnvoySinkConfig> for SinkConfig {
        type Error = GenericError;
        fn try_from(value: EnvoySinkConfig) -> Result<Self, Self::Error> {
            let EnvoySinkConfig {
                report_counters_as_deltas,
                report_histograms_as_deltas,
                emit_tags_as_attributes,
                use_tag_extracted_name,
                prefix,
                protocol_specifier,
                ..
            } = value;
            unsupported_field!(
                // prefix,
                // protocol_specifier
                report_counters_as_deltas,
                report_histograms_as_deltas,
                emit_tags_as_attributes,
                use_tag_extracted_name
            )?;

            let orion_data_plane_api::envoy_data_plane_api::envoy::extensions::stat_sinks::open_telemetry::v3::sink_config::ProtocolSpecifier::GrpcService(grpc_srv) = protocol_specifier.ok_or_else(|| GenericError::from_msg("ProtocolSpecifier unspecified"))?;
            let grpc_service = GrpcService::try_from(grpc_srv)?;
            Ok(Self { grpc_service, prefix })
        }
    }
}
