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

pub mod http_rbac;
use compact_str::CompactString;
use http_rbac::HttpRbac;
pub mod local_rate_limit;
use local_rate_limit::LocalRateLimit;
pub mod filter_registry;
pub mod peer_metadata;
pub mod router;
pub mod set_filter_state;

use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FilterOverride {
    // this field can be optional iff config is Some(_)
    pub disabled: bool,
    #[serde(skip_serializing_if = "is_default", default)]
    pub filter_settings: Option<FilterConfigOverride>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged, rename_all = "snake_case")]
pub enum FilterConfigOverride {
    LocalRateLimit(LocalRateLimit),
    // in Envoy this is a seperate type, RbacPerRoute, but it only has one field named rbac with the full config.
    // so we replace it with an option to be more rusty
    Rbac(Option<HttpRbac>),
}

impl From<FilterConfigOverride> for FilterOverride {
    fn from(value: FilterConfigOverride) -> Self {
        Self { disabled: false, filter_settings: Some(value) }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HttpFilter {
    pub name: CompactString,
    #[serde(skip_serializing_if = "is_default", default)]
    pub disabled: bool,
    #[serde(flatten)]
    pub filter: HttpFilterType,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "filter_type", content = "filter_settings")]
pub enum HttpFilterType {
    Rbac(HttpRbac),
    RateLimit(LocalRateLimit),
    Ingored,
    /// Istio peer metadata filter (parsed but may not be executed)
    PeerMetadata(peer_metadata::PeerMetadataConfig),
    /// Envoy set filter state filter (parsed but may not be executed)
    SetFilterState(set_filter_state::SetFilterState),
}

#[cfg(feature = "envoy-conversions")]
pub(crate) use envoy_conversions::*;

use super::is_default;

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::filter_registry::ensure_filters_registered;
    use super::{FilterConfigOverride, FilterOverride, HttpFilter, HttpFilterType, HttpRbac};
    use crate::config::common::*;
    use compact_str::CompactString;
    use orion_data_plane_api::envoy_data_plane_api::{
        envoy::{
            config::route::v3::FilterConfig as EnvoyFilterConfig,
            extensions::filters::{
                http::{
                    local_ratelimit::v3::LocalRateLimit as EnvoyLocalRateLimit,
                    rbac::v3::{Rbac as EnvoyRbac, RbacPerRoute as EnvoyRbacPerRoute},
                    router::v3::Router as EnvoyRouter,
                },
                network::http_connection_manager::v3::{
                    http_filter::ConfigType as EnvoyConfigType, HttpFilter as EnvoyHttpFilter,
                },
            },
        },
        google::protobuf::Any,
        prost::Message,
    };
    use tracing::info;

    #[derive(Debug, Clone)]
    pub(crate) struct SupportedEnvoyHttpFilter {
        pub name: CompactString,
        pub disabled: bool,
        pub filter: SupportedEnvoyFilter,
    }

    impl TryFrom<EnvoyHttpFilter> for SupportedEnvoyHttpFilter {
        type Error = GenericError;
        fn try_from(envoy: EnvoyHttpFilter) -> Result<Self, Self::Error> {
            let EnvoyHttpFilter { name, is_optional, disabled, config_type } = envoy;
            unsupported_field!(is_optional)?;
            let name: CompactString = required!(name)?.into();
            match required!(config_type).map(|x| match x {
                EnvoyConfigType::ConfigDiscovery(_) => {
                    Err(GenericError::unsupported_variant("ConfigDiscovery")).with_node(name.clone())
                },
                EnvoyConfigType::TypedConfig(typed_config) => SupportedEnvoyFilter::try_from(typed_config),
            }) {
                Ok(Ok(filter)) => Ok(Self { name, filter, disabled }),
                Err(e) | Ok(Err(e)) => Err(e.with_name(name)),
            }
        }
    }

    impl TryFrom<SupportedEnvoyHttpFilter> for HttpFilter {
        type Error = GenericError;
        fn try_from(value: SupportedEnvoyHttpFilter) -> Result<Self, Self::Error> {
            let SupportedEnvoyHttpFilter { name, disabled, filter } = value;
            Ok(Self { name, disabled, filter: filter.try_into()? })
        }
    }

    impl TryFrom<SupportedEnvoyFilter> for HttpFilterType {
        type Error = GenericError;
        fn try_from(value: SupportedEnvoyFilter) -> Result<Self, Self::Error> {
            match value {
                SupportedEnvoyFilter::LocalRateLimit(lr) => lr.try_into().map(Self::RateLimit),
                SupportedEnvoyFilter::Rbac(rbac) => rbac.try_into().map(Self::Rbac),
                SupportedEnvoyFilter::Router(_) => {
                    Err(GenericError::from_msg("router filter has to be the last filter in the chain"))
                },
                SupportedEnvoyFilter::Ignored => Ok(Self::Ingored),
                SupportedEnvoyFilter::PeerMetadata(config) => Ok(Self::PeerMetadata(config)),
                SupportedEnvoyFilter::SetFilterState(config) => Ok(Self::SetFilterState(config)),
            }
        }
    }

    #[allow(clippy::large_enum_variant)]
    #[derive(Debug, Clone)]
    pub(crate) enum SupportedEnvoyFilter {
        LocalRateLimit(EnvoyLocalRateLimit),
        Rbac(EnvoyRbac),
        Router(EnvoyRouter),
        Ignored,
        PeerMetadata(super::peer_metadata::PeerMetadataConfig),
        SetFilterState(super::set_filter_state::SetFilterState),
    }

    impl TryFrom<Any> for SupportedEnvoyFilter {
        type Error = GenericError;
        fn try_from(typed_config: Any) -> Result<Self, Self::Error> {
            use crate::typed_struct::{global_registry, TypedStructFilter, TypedStructParser};

            ensure_filters_registered();

            if TypedStructParser::is_typed_struct_url(&typed_config.type_url) {
                let parsed = TypedStructParser::parse(&typed_config.value)
                    .map_err(|e| GenericError::from_msg_with_cause("Failed to parse TypedStruct wrapper", e))?;

                let registry = global_registry();
                if registry.is_supported(&parsed.type_url) {
                    match parsed.type_url.as_str() {
                        url if url == super::peer_metadata::PeerMetadataConfig::TYPE_URL => {
                            super::peer_metadata::PeerMetadataConfig::from_typed_struct(&parsed).map(Self::PeerMetadata)
                        },
                        url if url == super::set_filter_state::SetFilterState::TYPE_URL => {
                            super::set_filter_state::SetFilterState::from_typed_struct(&parsed)
                                .map(Self::SetFilterState)
                        },
                        _ => Err(GenericError::unsupported_variant(format!(
                            "TypedStruct with inner type: {}",
                            parsed.type_url
                        ))),
                    }
                } else {
                    Err(GenericError::unsupported_variant(format!(
                        "Unregistered TypedStruct filter: {}. Use global_registry().register_dynamic() to add support.",
                        parsed.type_url
                    )))
                }
            } else {
                match typed_config.type_url.as_str() {
                    "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit" => {
                        EnvoyLocalRateLimit::decode(typed_config.value.as_slice()).map(Self::LocalRateLimit)
                    },
                    "type.googleapis.com/envoy.extensions.filters.http.rbac.v3.RBAC" => {
                        EnvoyRbac::decode(typed_config.value.as_slice()).map(Self::Rbac)
                    },
                    "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router" => {
                        EnvoyRouter::decode(typed_config.value.as_slice()).map(Self::Router)
                    },
                    url if url == super::set_filter_state::SetFilterState::TYPE_URL => {
                        return super::set_filter_state::SetFilterState::try_from_raw_protobuf(&typed_config.value)
                            .map(Self::SetFilterState)
                    },
                    url if url == super::peer_metadata::PeerMetadataConfig::TYPE_URL => {
                        return super::peer_metadata::PeerMetadataConfig::try_from_raw_protobuf(&typed_config.value)
                            .map(Self::PeerMetadata)
                    },
                    "type.googleapis.com/udpa.type.v1.TypedStruct"
                    | "type.googleapis.com/stats.PluginConfig"
                    | "type.googleapis.com/envoy.extensions.filters.http.grpc_stats.v3.FilterConfig"
                    | "type.googleapis.com/istio.envoy.config.filter.http.alpn.v2alpha1.FilterConfig"
                    | "type.googleapis.com/envoy.extensions.filters.http.fault.v3.HTTPFault"
                    | "type.googleapis.com/envoy.extensions.filters.http.cors.v3.Cors" => {
                        info!("Ignored Istio type {}", typed_config.type_url);
                        Ok(SupportedEnvoyFilter::Ignored)
                    },
                    _ => {
                        return Err(GenericError::unsupported_variant(format!(
                            "HTTP filter unsupported variant {}",
                            typed_config.type_url
                        )))
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
    }

    #[derive(Debug, Clone)]
    #[allow(clippy::large_enum_variant)]
    pub enum SupportedEnvoyFilterOverride {
        LocalRateLimit(EnvoyLocalRateLimit),
        Rbac(EnvoyRbacPerRoute),
    }

    impl TryFrom<Any> for SupportedEnvoyFilterOverride {
        type Error = GenericError;
        fn try_from(typed_config: Any) -> Result<Self, Self::Error> {
            match typed_config.type_url.as_str() {
                "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit" => {
                    EnvoyLocalRateLimit::decode(typed_config.value.as_slice()).map(Self::LocalRateLimit)
                },
                "type.googleapis.com/envoy.extensions.filters.http.rbac.v3.RBACPerRoute" => {
                    EnvoyRbacPerRoute::decode(typed_config.value.as_slice()).map(Self::Rbac)
                },
                _ => {
                    return Err(GenericError::unsupported_variant(format!(
                        "HTTP Filter override unsupported variant {}",
                        typed_config.type_url
                    )))
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

    #[derive(Debug, Clone)]
    pub enum MaybeWrappedEnvoyFilter {
        Wrapped(EnvoyFilterConfig),
        Direct(SupportedEnvoyFilterOverride),
    }

    impl TryFrom<Any> for MaybeWrappedEnvoyFilter {
        type Error = GenericError;
        fn try_from(typed_config: Any) -> Result<Self, Self::Error> {
            match typed_config.type_url.as_str() {
                "type.googleapis.com/envoy.config.route.v3.FilterConfig" => {
                    EnvoyFilterConfig::decode(typed_config.value.as_slice()).map(Self::Wrapped).map_err(|e| {
                        GenericError::from_msg_with_cause(
                            format!("failed to parse protobuf for \"{}\"", typed_config.type_url),
                            e,
                        )
                    })
                },
                _ => SupportedEnvoyFilterOverride::try_from(typed_config).map(Self::Direct),
            }
        }
    }

    impl TryFrom<SupportedEnvoyFilterOverride> for FilterConfigOverride {
        type Error = GenericError;
        fn try_from(value: SupportedEnvoyFilterOverride) -> Result<Self, Self::Error> {
            match value {
                SupportedEnvoyFilterOverride::LocalRateLimit(envoy) => envoy.try_into().map(Self::LocalRateLimit),
                SupportedEnvoyFilterOverride::Rbac(EnvoyRbacPerRoute { rbac }) => {
                    rbac.map(HttpRbac::try_from).transpose().map(Self::Rbac)
                },
            }
        }
    }

    impl TryFrom<Any> for FilterConfigOverride {
        type Error = GenericError;
        fn try_from(envoy: Any) -> Result<Self, Self::Error> {
            let supported = SupportedEnvoyFilterOverride::try_from(envoy)?;
            supported.try_into()
        }
    }
    impl TryFrom<EnvoyFilterConfig> for FilterOverride {
        type Error = GenericError;
        fn try_from(envoy: EnvoyFilterConfig) -> Result<Self, Self::Error> {
            let EnvoyFilterConfig { config, is_optional, disabled } = envoy;
            unsupported_field!(is_optional)?;
            let filter_settings = config.map(FilterConfigOverride::try_from).transpose().with_node("config")?;
            Ok(Self { disabled, filter_settings })
        }
    }

    impl TryFrom<MaybeWrappedEnvoyFilter> for FilterOverride {
        type Error = GenericError;
        fn try_from(envoy: MaybeWrappedEnvoyFilter) -> Result<Self, Self::Error> {
            match envoy {
                MaybeWrappedEnvoyFilter::Direct(envoy) => FilterConfigOverride::try_from(envoy).map(Self::from),
                MaybeWrappedEnvoyFilter::Wrapped(envoy) => envoy.try_into(),
            }
        }
    }

    impl TryFrom<Any> for FilterOverride {
        type Error = GenericError;
        fn try_from(envoy: Any) -> Result<Self, Self::Error> {
            MaybeWrappedEnvoyFilter::try_from(envoy)?.try_into()
        }
    }
}

#[cfg(test)]
mod typed_struct_integration_tests {
    use super::*;
    use crate::typed_struct::{TypedStruct, TypedStructFilter};
    use orion_data_plane_api::envoy_data_plane_api::google::protobuf::Any;
    use prost::Message;
    use prost_types::{value::Kind, ListValue, Struct, Value};
    use std::collections::BTreeMap;

    #[test]
    fn test_try_from_any_typed_struct_peer_metadata() {
        // Build inner Struct for PeerMetadataConfig with discovery object
        let mut discovery_obj = BTreeMap::new();
        discovery_obj.insert(
            "workload_discovery".to_string(),
            Value { kind: Some(Kind::StructValue(Struct { fields: BTreeMap::new() })) },
        );

        let mut fields = BTreeMap::new();
        fields.insert(
            "downstream_discovery".to_string(),
            Value {
                kind: Some(Kind::ListValue(ListValue {
                    values: vec![Value { kind: Some(Kind::StructValue(Struct { fields: discovery_obj })) }],
                })),
            },
        );
        fields.insert("shared_with_upstream".to_string(), Value { kind: Some(Kind::BoolValue(true)) });

        let typed_struct = TypedStruct {
            type_url: super::peer_metadata::PeerMetadataConfig::TYPE_URL.to_string(),
            value: Some(Struct { fields }),
        };

        let mut buf = Vec::new();
        typed_struct.encode(&mut buf).unwrap();

        // Wrap in Any as TypedStruct wrapper
        let any = Any { type_url: "type.googleapis.com/udpa.type.v1.TypedStruct".to_string(), value: buf };

        let parsed = SupportedEnvoyFilter::try_from(any).expect("should parse typed struct");
        match parsed {
            SupportedEnvoyFilter::PeerMetadata(cfg) => {
                assert_eq!(cfg.shared_with_upstream, Some(true));
                assert!(cfg.downstream_discovery.is_some());
            },
            _ => panic!("expected PeerMetadata variant"),
        }
    }

    #[test]
    fn test_try_from_any_typed_struct_set_filter_state() {
        // Build inner Struct for SetFilterState with one action including format_string
        let mut struct_fields = BTreeMap::new();
        struct_fields.insert("object_key".to_string(), Value { kind: Some(Kind::StringValue("test_key".to_string())) });
        struct_fields.insert(
            "format_string".to_string(),
            Value { kind: Some(Kind::StringValue("%REQ(:authority)%".to_string())) },
        );

        let action_struct = Value { kind: Some(Kind::StructValue(Struct { fields: struct_fields })) };

        let mut list_values = Vec::new();
        list_values.push(action_struct);

        let mut fields = BTreeMap::new();
        fields.insert(
            "on_request_headers".to_string(),
            Value { kind: Some(Kind::ListValue(ListValue { values: list_values })) },
        );

        let typed_struct = TypedStruct {
            type_url: super::set_filter_state::SetFilterState::TYPE_URL.to_string(),
            value: Some(Struct { fields }),
        };

        let mut buf = Vec::new();
        typed_struct.encode(&mut buf).unwrap();

        let any = Any { type_url: "type.googleapis.com/udpa.type.v1.TypedStruct".to_string(), value: buf };

        let parsed = SupportedEnvoyFilter::try_from(any).expect("should parse typed struct");
        match parsed {
            SupportedEnvoyFilter::SetFilterState(cfg) => {
                assert!(!cfg.on_request_headers.is_empty());
                let actions = &cfg.on_request_headers;
                assert_eq!(actions.len(), 1);
                assert_eq!(actions[0].object_key, "test_key");
            },
            _ => panic!("expected SetFilterState variant"),
        }
    }
}
