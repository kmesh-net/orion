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

use compact_str::CompactString;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::core::{StringMatcher, StringMatcherPattern};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct ExternalProcessor {
    pub grpc_service: GrpcService,
    #[serde(default)]
    pub failure_mode_allow: bool,
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none", default)]
    pub message_timeout: Option<Duration>,
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none", default)]
    pub max_message_timeout: Option<Duration>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub processing_mode: Option<ProcessingMode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mutation_rules: Option<HeaderMutationRules>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub forward_rules: Option<HeaderForwardingRules>,
    #[serde(default)]
    pub allow_mode_override: bool,
    #[serde(default)]
    pub route_cache_action: RouteCacheAction,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub allowed_override_modes: Vec<ProcessingMode>,
    #[serde(default)]
    pub send_body_without_waiting_for_header_response: bool,
    #[serde(default)]
    pub observability_mode: bool,
    #[serde(default)]
    pub disable_immediate_response: bool,
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none", default)]
    pub deferred_close_timeout: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtProcPerRoute {
    #[serde(default)]
    pub disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub overrides: Option<ExtProcOverrides>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtProcOverrides {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub processing_mode: Option<ProcessingMode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub grpc_service: Option<GrpcService>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub failure_mode_allow: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GrpcService {
    #[serde(flatten)]
    pub service_specifier: GrpcServiceSpecifier,
    #[serde(with = "humantime_serde", skip_serializing_if = "Option::is_none", default)]
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GrpcServiceSpecifier {
    Cluster(CompactString),
    GoogleGrpc(GoogleGrpc),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoogleGrpc {
    pub target_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProcessingMode {
    #[serde(default)]
    pub request_header_mode: HeaderProcessingMode,
    #[serde(default)]
    pub response_header_mode: HeaderProcessingMode,
    #[serde(default)]
    pub request_body_mode: BodyProcessingMode,
    #[serde(default)]
    pub response_body_mode: BodyProcessingMode,
    #[serde(default)]
    pub request_trailer_mode: TrailerProcessingMode,
    #[serde(default)]
    pub response_trailer_mode: TrailerProcessingMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HeaderProcessingMode {
    #[default]
    Default,
    Send,
    Skip,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BodyProcessingMode {
    #[default]
    None,
    Streamed,
    Buffered,
    BufferedPartial,
    FullDuplexStreamed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TrailerProcessingMode {
    #[default]
    Default,
    Skip,
    Send,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MutationPolicy {
    #[serde(rename = "allow_all")]
    AllowAll,

    #[serde(rename = "standard")]
    Standard {
        #[serde(default)]
        allow_routing: bool,
        #[serde(default)]
        allow_envoy: bool,
        #[serde(default = "default_true")]
        allow_system: bool,
    },

    #[serde(rename = "deny_all")]
    DenyAll,
}

impl Default for MutationPolicy {
    fn default() -> Self {
        MutationPolicy::Standard { allow_routing: false, allow_envoy: false, allow_system: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PatternOverrides {
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub allow: Vec<StringMatcher>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub deny: Vec<StringMatcher>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HeaderMutationRules {
    #[serde(default)]
    pub policy: MutationPolicy,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub overrides: Option<PatternOverrides>,
    #[serde(default)]
    pub disallow_is_error: bool,
}

impl HeaderMutationRules {
    pub fn is_modification_permitted(&self, header: &str) -> bool {
        if let Some(overrides) = &self.overrides {
            if overrides.deny.iter().any(|m| m.matches(header)) {
                return false;
            }
            if overrides.allow.iter().any(|m| m.matches(header)) {
                return true;
            }
        }
        match &self.policy {
            MutationPolicy::DenyAll => false,
            MutationPolicy::AllowAll => true,
            MutationPolicy::Standard { allow_routing, allow_envoy, allow_system } => {
                if header == "host" {
                    return *allow_routing;
                }
                if header.starts_with(':') {
                    if matches!(header, ":authority" | ":scheme" | ":method") {
                        return *allow_routing;
                    }
                    if !allow_system {
                        return false;
                    }
                }
                if header.starts_with("x-envoy-") {
                    return *allow_envoy;
                }
                true
            },
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeaderForwardingRules {
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub allowed_headers: Vec<StringMatcher>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub disallowed_headers: Vec<StringMatcher>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RouteCacheAction {
    #[default]
    Default,
    Clear,
    Retain,
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::*;
    use crate::config::{common::*, core::regex_from_envoy, util::duration_from_envoy};
    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        config::{
            common::mutation_rules::v3::HeaderMutationRules as EnvoyHeaderMutationRules,
            core::v3::{
                grpc_service::{GoogleGrpc as EnvoyGoogleGrpc, TargetSpecifier},
                GrpcService as EnvoyGrpcService,
            },
        },
        extensions::filters::http::ext_proc::v3::{
            processing_mode::{BodySendMode as EnvoyBodySendMode, HeaderSendMode as EnvoyHeaderSendMode},
            ExtProcOverrides as EnvoyExtProcOverrides, ExtProcPerRoute as EnvoyExtProcPerRoute,
            ExternalProcessor as EnvoyExternalProcessor, HeaderForwardingRules as EnvoyHeaderForwardingRules,
            ProcessingMode as EnvoyProcessingMode,
        },
    };

    impl TryFrom<EnvoyExternalProcessor> for ExternalProcessor {
        type Error = GenericError;
        fn try_from(value: EnvoyExternalProcessor) -> Result<Self, Self::Error> {
            let EnvoyExternalProcessor {
                grpc_service,
                http_service,
                failure_mode_allow,
                processing_mode,
                message_timeout,
                max_message_timeout,
                stat_prefix: _,
                forward_rules,
                mutation_rules,
                allow_mode_override,
                disable_clear_route_cache,
                filter_metadata,
                disable_immediate_response,
                metadata_options,
                observability_mode,
                route_cache_action,
                deferred_close_timeout,
                send_body_without_waiting_for_header_response,
                request_attributes,
                response_attributes,
                allowed_override_modes,
                on_processing_response,
                processing_request_modifier: _,
                status_on_error: _,
            } = value;

            unsupported_field!(
                // grpc_service,
                http_service,
                // failure_mode_allow,
                // processing_mode,
                // message_timeout,
                // max_message_timeout,
                // stat_prefix,
                // forward_rules,
                // mutation_rules,
                // allow_mode_override,
                disable_clear_route_cache,
                filter_metadata,
                // disable_immediate_response,
                metadata_options,
                // observability_mode,
                // route_cache_action,
                // deferred_close_timeout,
                // send_body_without_waiting_for_header_response,
                request_attributes,
                response_attributes,
                // allowed_override_modes,
                on_processing_response
            )?;

            let grpc_service: GrpcService =
                GrpcService::try_from(required!(grpc_service)?).with_node("grpc_service")?;
            let message_timeout = message_timeout.map(duration_from_envoy).transpose().with_node("message_timeout")?;
            let max_message_timeout =
                max_message_timeout.map(duration_from_envoy).transpose().with_node("max_message_timeout")?;
            let processing_mode = processing_mode
                .map(ProcessingMode::try_from)
                .transpose()
                .with_node("processing_mode")?
                .or(Some(ProcessingMode::default()));
            let mutation_rules = mutation_rules
                .map(HeaderMutationRules::try_from)
                .transpose()
                .with_node("mutation_rules")?
                .or(Some(HeaderMutationRules::default()));
            let forward_rules =
                forward_rules.map(HeaderForwardingRules::try_from).transpose().with_node("forward_rules")?;
            let deferred_close_timeout =
                deferred_close_timeout.map(duration_from_envoy).transpose().with_node("deferred_close_timeout")?;
            let allowed_override_modes = convert_vec!(allowed_override_modes)?;

            let route_cache_action = RouteCacheAction::try_from(route_cache_action).with_node("route_cache_action")?;

            Ok(Self {
                grpc_service,
                failure_mode_allow,
                message_timeout,
                max_message_timeout,
                processing_mode,
                mutation_rules,
                forward_rules,
                allow_mode_override,
                route_cache_action,
                allowed_override_modes,
                send_body_without_waiting_for_header_response,
                observability_mode,
                disable_immediate_response,
                deferred_close_timeout,
            })
        }
    }

    impl TryFrom<EnvoyGrpcService> for GrpcService {
        type Error = GenericError;
        fn try_from(value: EnvoyGrpcService) -> Result<Self, Self::Error> {
            let EnvoyGrpcService { target_specifier, timeout, initial_metadata, retry_policy } = value;

            unsupported_field!(
                // target_specifier,
                // timeout,
                initial_metadata,
                retry_policy
            )?;

            let service_specifier = match required!(target_specifier)? {
                TargetSpecifier::EnvoyGrpc(eg) => GrpcServiceSpecifier::Cluster(eg.cluster_name.into()),
                TargetSpecifier::GoogleGrpc(gg) => {
                    let google_grpc: GoogleGrpc = gg.try_into().with_node("google_grpc")?;
                    GrpcServiceSpecifier::GoogleGrpc(google_grpc)
                },
            };

            let timeout = timeout.map(duration_from_envoy).transpose().with_node("timeout")?;

            Ok(Self { service_specifier, timeout })
        }
    }

    impl TryFrom<EnvoyGoogleGrpc> for GoogleGrpc {
        type Error = GenericError;
        fn try_from(value: EnvoyGoogleGrpc) -> Result<Self, Self::Error> {
            let EnvoyGoogleGrpc {
                target_uri,
                channel_credentials,
                call_credentials,
                stat_prefix: _,
                credentials_factory_name,
                config,
                per_stream_buffer_limit_bytes,
                channel_args,
                channel_credentials_plugin: _,
                call_credentials_plugin: _,
            } = value;

            unsupported_field!(
                // target_uri,
                channel_credentials,
                call_credentials,
                //stat_prefix,
                credentials_factory_name,
                config,
                per_stream_buffer_limit_bytes,
                channel_args
            )?;

            Ok(Self { target_uri })
        }
    }

    impl TryFrom<EnvoyProcessingMode> for ProcessingMode {
        type Error = GenericError;
        fn try_from(value: EnvoyProcessingMode) -> Result<Self, Self::Error> {
            let EnvoyProcessingMode {
                request_header_mode,
                response_header_mode,
                request_body_mode,
                response_body_mode,
                request_trailer_mode,
                response_trailer_mode,
            } = value;

            let request_header_mode =
                HeaderProcessingMode::try_from(request_header_mode).with_node("request_header_mode")?;
            let response_header_mode =
                HeaderProcessingMode::try_from(response_header_mode).with_node("response_header_mode")?;
            let request_body_mode = BodyProcessingMode::try_from(request_body_mode).with_node("request_body_mode")?;
            let response_body_mode =
                BodyProcessingMode::try_from(response_body_mode).with_node("response_body_mode")?;
            let request_trailer_mode =
                TrailerProcessingMode::try_from(request_trailer_mode).with_node("request_trailer_mode")?;
            let response_trailer_mode =
                TrailerProcessingMode::try_from(response_trailer_mode).with_node("response_trailer_mode")?;

            Ok(Self {
                request_header_mode,
                response_header_mode,
                request_body_mode,
                response_body_mode,
                request_trailer_mode,
                response_trailer_mode,
            })
        }
    }

    impl TryFrom<i32> for HeaderProcessingMode {
        type Error = GenericError;
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            match EnvoyHeaderSendMode::try_from(value) {
                Ok(EnvoyHeaderSendMode::Default) => Ok(Self::Default),
                Ok(EnvoyHeaderSendMode::Send) => Ok(Self::Send),
                Ok(EnvoyHeaderSendMode::Skip) => Ok(Self::Skip),
                Err(_) => Err(GenericError::from_msg(format!("unknown header send mode: {value}"))),
            }
        }
    }

    impl TryFrom<i32> for BodyProcessingMode {
        type Error = GenericError;
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            match EnvoyBodySendMode::try_from(value) {
                Ok(EnvoyBodySendMode::None) => Ok(Self::None),
                Ok(EnvoyBodySendMode::Streamed) => Ok(Self::Streamed),
                Ok(EnvoyBodySendMode::Buffered) => Ok(Self::Buffered),
                Ok(EnvoyBodySendMode::BufferedPartial) => Ok(Self::BufferedPartial),
                Ok(EnvoyBodySendMode::FullDuplexStreamed) => Ok(Self::FullDuplexStreamed),
                Ok(EnvoyBodySendMode::Grpc) => Ok(Self::FullDuplexStreamed),
                Err(_) => Err(GenericError::from_msg(format!("unknown body send mode: {value}"))),
            }
        }
    }

    impl TryFrom<i32> for TrailerProcessingMode {
        type Error = GenericError;
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            match EnvoyHeaderSendMode::try_from(value) {
                Ok(EnvoyHeaderSendMode::Send) => Ok(Self::Send),
                Ok(EnvoyHeaderSendMode::Default) => Ok(Self::Default),
                Ok(EnvoyHeaderSendMode::Skip) => Ok(Self::Skip),
                Err(_) => Err(GenericError::from_msg(format!("unknown trailer send mode: {value}"))),
            }
        }
    }

    impl TryFrom<EnvoyHeaderMutationRules> for HeaderMutationRules {
        type Error = GenericError;
        fn try_from(value: EnvoyHeaderMutationRules) -> Result<Self, Self::Error> {
            let EnvoyHeaderMutationRules {
                allow_all_routing,
                allow_envoy,
                disallow_system,
                disallow_all,
                allow_expression,
                disallow_expression,
                disallow_is_error,
            } = value;

            let allow_expression = allow_expression
                .map(|envoy_regex| {
                    regex_from_envoy(envoy_regex)
                        .map(|regex| StringMatcher { ignore_case: false, pattern: StringMatcherPattern::Regex(regex) })
                })
                .transpose()
                .with_node("allow_expression")?;
            let disallow_expression = disallow_expression
                .map(|envoy_regex| {
                    regex_from_envoy(envoy_regex)
                        .map(|regex| StringMatcher { ignore_case: false, pattern: StringMatcherPattern::Regex(regex) })
                })
                .transpose()
                .with_node("disallow_expression")?;

            let allow_all_routing = allow_all_routing.map(|v| v.value).unwrap_or(false);
            let allow_envoy = allow_envoy.map(|v| v.value).unwrap_or(false);
            let disallow_system = disallow_system.map(|v| v.value).unwrap_or(false);
            let disallow_all = disallow_all.map(|v| v.value).unwrap_or(false);
            let disallow_is_error = disallow_is_error.map(|v| v.value).unwrap_or(false);

            let policy = if disallow_all {
                MutationPolicy::DenyAll
            } else {
                MutationPolicy::Standard {
                    allow_routing: allow_all_routing,
                    allow_envoy,
                    allow_system: !disallow_system,
                }
            };
            let overrides = (allow_expression.is_some() || disallow_expression.is_some()).then(|| PatternOverrides {
                allow: allow_expression.into_iter().collect(),
                deny: disallow_expression.into_iter().collect(),
            });

            Ok(Self { policy, overrides, disallow_is_error })
        }
    }

    impl TryFrom<i32> for RouteCacheAction {
        type Error = GenericError;
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            match value {
                0 => Ok(Self::Default),
                1 => Ok(Self::Clear),
                2 => Ok(Self::Retain),
                _ => Err(GenericError::from_msg(format!("unknown route cache action: {value}"))),
            }
        }
    }

    impl TryFrom<EnvoyHeaderForwardingRules> for HeaderForwardingRules {
        type Error = GenericError;
        fn try_from(value: EnvoyHeaderForwardingRules) -> Result<Self, Self::Error> {
            let EnvoyHeaderForwardingRules { allowed_headers, disallowed_headers } = value;

            let allowed_headers = allowed_headers
                .map(|list| list.patterns.into_iter().map(TryInto::try_into).collect::<Result<Vec<_>, _>>())
                .transpose()
                .with_node("allowed_headers")?
                .unwrap_or_default();

            let disallowed_headers = disallowed_headers
                .map(|list| list.patterns.into_iter().map(TryInto::try_into).collect::<Result<Vec<_>, _>>())
                .transpose()
                .with_node("disallowed_headers")?
                .unwrap_or_default();

            Ok(Self { allowed_headers, disallowed_headers })
        }
    }

    impl TryFrom<EnvoyExtProcPerRoute> for ExtProcPerRoute {
        type Error = GenericError;
        fn try_from(value: EnvoyExtProcPerRoute) -> Result<Self, Self::Error> {
            use orion_data_plane_api::envoy_data_plane_api::envoy::extensions::filters::http::ext_proc::v3::ext_proc_per_route::Override;

            match value.r#override {
                Some(Override::Disabled(disabled)) => Ok(Self { disabled, overrides: None }),
                Some(Override::Overrides(envoy_overrides)) => Ok(Self {
                    disabled: false,
                    overrides: Some(ExtProcOverrides::try_from(envoy_overrides).with_node("overrides")?),
                }),
                None => Ok(Self { disabled: false, overrides: None }),
            }
        }
    }

    impl TryFrom<EnvoyExtProcOverrides> for ExtProcOverrides {
        type Error = GenericError;
        fn try_from(value: EnvoyExtProcOverrides) -> Result<Self, Self::Error> {
            let EnvoyExtProcOverrides {
                processing_mode,
                grpc_service,
                grpc_initial_metadata,
                metadata_options,
                async_mode,
                request_attributes,
                response_attributes,
                failure_mode_allow,
                processing_request_modifier: _,
            } = value;

            unsupported_field!(
                grpc_initial_metadata,
                metadata_options,
                async_mode,
                request_attributes,
                response_attributes
            )?;

            let processing_mode = processing_mode.map(TryInto::try_into).transpose().with_node("processing_mode")?;
            let grpc_service = grpc_service.map(TryInto::try_into).transpose().with_node("grpc_service")?;

            Ok(Self { processing_mode, grpc_service, failure_mode_allow: failure_mode_allow.map(|v| v.value) })
        }
    }
}
