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

use crate::config::{common::envoy_conversions::IsUsed, unsupported_field, GenericError};
use orion_data_plane_api::envoy_data_plane_api::{
    envoy::config::core::v3::{grpc_service::GoogleGrpc as EnvoyGoogleGrpc, GrpcService as EnvoyGrpcService},
    google::protobuf::Duration as EnvoyDuration,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct GrpcService {
    pub timeout: Option<std::time::Duration>,
    pub google_grpc: Option<GoogleGrpc>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct GoogleGrpc {
    pub target_uri: String,
    pub stat_prefix: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Duration(pub std::time::Duration);

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    use super::*;

    impl TryFrom<EnvoyDuration> for Duration {
        type Error = GenericError;

        fn try_from(value: EnvoyDuration) -> Result<Self, Self::Error> {
            let seconds = value.seconds;
            let nanos = value.nanos;
            if seconds < 0 || nanos < 0 {
                return Err(GenericError::from_msg("duration with negative values".to_owned()));
            }
            Ok(Duration(std::time::Duration::new(seconds as u64, nanos as u32)))
        }
    }

    impl TryFrom<EnvoyGoogleGrpc> for GoogleGrpc {
        type Error = GenericError;

        fn try_from(value: EnvoyGoogleGrpc) -> Result<Self, Self::Error> {
            let EnvoyGoogleGrpc {
                target_uri,
                channel_credentials,
                call_credentials,
                stat_prefix,
                credentials_factory_name,
                config,
                per_stream_buffer_limit_bytes,
                channel_args,
            } = value;
            unsupported_field!(
                // target_uri,
                channel_credentials,
                call_credentials,
                // stat_prefix,
                credentials_factory_name,
                config,
                per_stream_buffer_limit_bytes,
                channel_args
            )?;

            Ok(GoogleGrpc { target_uri, stat_prefix })
        }
    }

    impl TryFrom<EnvoyGrpcService> for GrpcService {
        type Error = GenericError;

        fn try_from(value: EnvoyGrpcService) -> Result<Self, Self::Error> {
            let EnvoyGrpcService { timeout, initial_metadata, retry_policy, target_specifier } = value;
            unsupported_field!(
                // timeout,
                // target_specifier
                initial_metadata,
                retry_policy
            )?;

            let google_grpc = match target_specifier {
                Some(target) => match target {
                    orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::grpc_service::TargetSpecifier::EnvoyGrpc(
                        _envoy_grpc,
                    ) => None,
                    orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::grpc_service::TargetSpecifier::GoogleGrpc(
                        google_grpc,
                    ) => Some(GoogleGrpc::try_from(google_grpc)?),
                },
                None => None,
            };

            let timeout = timeout.map(|t| Duration::try_from(t).map(|t| t.0)).transpose()?;
            Ok(Self { google_grpc, timeout })
        }
    }
}
