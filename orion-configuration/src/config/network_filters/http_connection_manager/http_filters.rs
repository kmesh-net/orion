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
pub mod router;

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
}

#[cfg(feature = "envoy-conversions")]
pub(crate) use envoy_conversions::*;

use super::is_default;

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
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
                    HttpFilter as EnvoyHttpFilter, http_filter::ConfigType as EnvoyConfigType,
                },
            },
        },
        google::protobuf::Any,
        prost::Message,
    };

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
            }
        }
    }

    #[allow(clippy::large_enum_variant)]
    #[derive(Debug, Clone)]
    pub(crate) enum SupportedEnvoyFilter {
        LocalRateLimit(EnvoyLocalRateLimit),
        Rbac(EnvoyRbac),
        Router(EnvoyRouter),
    }

    impl TryFrom<Any> for SupportedEnvoyFilter {
        type Error = GenericError;
        fn try_from(typed_config: Any) -> Result<Self, Self::Error> {
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
                _ => return Err(GenericError::unsupported_variant(typed_config.type_url)),
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
                _ => return Err(GenericError::unsupported_variant(typed_config.type_url)),
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
