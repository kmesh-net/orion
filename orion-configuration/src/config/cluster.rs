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

pub mod health_check;
pub use health_check::HealthCheck;
pub mod http_protocol_options;
pub use http_protocol_options::HttpProtocolOptions;
pub mod cluster_specifier;
pub use cluster_specifier::ClusterSpecifier;

use super::{
    common::is_default,
    secret::TlsCertificate,
    transport::{BindDevice, CommonTlsValidationContext, TlsParameters},
};

use compact_str::CompactString;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt::Display, net::SocketAddr, num::NonZeroU32, time::Duration};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Cluster {
    pub name: CompactString,
    #[serde(flatten)]
    pub discovery_settings: ClusterDiscoveryType,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub tls_config: Option<TlsConfig>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub bind_device: Option<BindDevice>,
    #[serde(skip_serializing_if = "is_default", default)]
    pub load_balancing_policy: LbPolicy,
    #[serde(skip_serializing_if = "is_default", default)]
    pub http_protocol_options: HttpProtocolOptions,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub health_check: Option<HealthCheck>,
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub connect_timeout: Option<Duration>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClusterLoadAssignment {
    #[serde(
        serialize_with = "simplify_locality_lb_endpoints",
        deserialize_with = "deser_through::<LocalityLbEndpointsDeser,_,_>"
    )]
    pub endpoints: Vec<LocalityLbEndpoints>,
}

fn simplify_locality_lb_endpoints<S: Serializer>(
    value: &Vec<LocalityLbEndpoints>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if value.len() == 1 && value[0].priority == 0 {
        simplify_lb_endpoints(&value[0].lb_endpoints, serializer)
    } else {
        value.serialize(serializer)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum LocalityLbEndpointsDeser {
    LocalityLbEndpoints(Vec<LocalityLbEndpoints>),
    Simplified(LbEndpointVecDeser),
}

impl From<LocalityLbEndpointsDeser> for Vec<LocalityLbEndpoints> {
    fn from(value: LocalityLbEndpointsDeser) -> Self {
        match value {
            LocalityLbEndpointsDeser::Simplified(simple) => {
                vec![LocalityLbEndpoints { priority: 0, lb_endpoints: simple.into() }]
            },
            LocalityLbEndpointsDeser::LocalityLbEndpoints(vec) => vec,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalityLbEndpoints {
    pub priority: u32,
    #[serde(serialize_with = "simplify_lb_endpoints", deserialize_with = "deser_through::<LbEndpointVecDeser,_,_>")]
    pub lb_endpoints: Vec<LbEndpoint>,
}

fn simplify_lb_endpoints<S: Serializer>(value: &Vec<LbEndpoint>, serializer: S) -> Result<S::Ok, S::Error> {
    if value.iter().all(|s| is_default(&s.health_status) && s.load_balancing_weight == NonZeroU32::MIN) {
        value.iter().map(|endpoint| endpoint.address).collect::<Vec<_>>().serialize(serializer)
    } else {
        value.serialize(serializer)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum LbEndpointVecDeser {
    LbEndpoints(Vec<LbEndpoint>),
    Address(Vec<SocketAddr>),
}

impl From<LbEndpointVecDeser> for Vec<LbEndpoint> {
    fn from(value: LbEndpointVecDeser) -> Self {
        match value {
            LbEndpointVecDeser::Address(address) => address
                .into_iter()
                .map(|address| LbEndpoint {
                    address,
                    health_status: HealthStatus::default(),
                    load_balancing_weight: NonZeroU32::MIN,
                })
                .collect(),
            LbEndpointVecDeser::LbEndpoints(vec) => vec,
        }
    }
}

fn deser_through<'de, In: Deserialize<'de>, Out: From<In>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Out, D::Error> {
    In::deserialize(deserializer).map(Out::from)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LbEndpoint {
    pub address: SocketAddr,
    #[serde(skip_serializing_if = "is_default", default)]
    pub health_status: HealthStatus,
    pub load_balancing_weight: NonZeroU32,
}

#[derive(Clone, Debug, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HealthStatus {
    #[default]
    Healthy,
    Unhealthy,
}

impl Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                HealthStatus::Healthy => "Healthy",
                HealthStatus::Unhealthy => "Unhealthy",
            }
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "discovery", content = "discovery_settings")]
pub enum ClusterDiscoveryType {
    #[serde(rename = "static")]
    Static(ClusterLoadAssignment),
    #[serde(rename = "EDS")]
    Eds,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TlsConfig {
    //todo(hayley): This field is not marked as required by envoy
    // but sni is required in our client TLS stack.
    //  We could technically fall back to using the endpoint adress/name for the sni
    // where no sni is configured here but that would require a major refactor.
    // previous behaviour was to set sni to the empty string if missing.
    pub sni: CompactString,
    #[serde(skip_serializing_if = "is_default", default)]
    pub parameters: TlsParameters,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default", flatten)]
    pub secret: Option<TlsSecret>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default", flatten)]
    pub validation_context: Option<CommonTlsValidationContext>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TlsSecret {
    #[serde(rename = "tls_certificate_sds")]
    SdsConfig(CompactString),
    #[serde(rename = "tls_certificate")]
    Certificate(TlsCertificate),
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LbPolicy {
    #[default]
    RoundRobin,
    Random,
    LeastRequest,
    RingHash,
    Maglev,
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::health_check::{ClusterHostnameError, HealthCheck, HealthCheckProtocol};
    use super::{
        Cluster, ClusterDiscoveryType, ClusterLoadAssignment, HealthStatus, HttpProtocolOptions, LbEndpoint, LbPolicy,
        LocalityLbEndpoints, TlsConfig, TlsSecret,
    };
    use crate::config::common::*;
    use crate::config::{
        core::Address,
        transport::{BindDevice, CommonTlsContext, Secrets, SupportedEnvoyTransportSocket},
        util::duration_from_envoy,
    };
    use compact_str::CompactString;
    use orion_data_plane_api::envoy_data_plane_api::{
        envoy::{
            config::{
                cluster::v3::{
                    cluster::{
                        ClusterDiscoveryType as EnvoyClusterDiscoveryType, DiscoveryType as EnvoyDiscoveryType,
                        LbConfig as EnvoyLbConfig, LbPolicy as EnvoyLbPolicy,
                    },
                    Cluster as EnvoyCluster,
                },
                core::v3::{
                    BindConfig as EnvoyBindConfig, HealthStatus as EnvoyHealthStatus,
                    TransportSocket as EnvoyTransportSocket,
                },
                endpoint::v3::{
                    lb_endpoint::HostIdentifier as EnvoyHostIdentifier,
                    ClusterLoadAssignment as EnvoyClusterLoadAssignment, Endpoint as EnvoyEndpoint,
                    LbEndpoint as EnvoyLbEndpoint, LocalityLbEndpoints as EnvoyLocalityLbEndpoints,
                },
            },
            extensions::transport_sockets::tls::v3::UpstreamTlsContext,
        },
        google::protobuf::Any,
    };
    use std::{collections::BTreeSet, net::SocketAddr, num::NonZeroU32};

    impl TryFrom<EnvoyCluster> for Cluster {
        type Error = GenericError;
        fn try_from(envoy: EnvoyCluster) -> Result<Self, Self::Error> {
            let EnvoyCluster {
                name,
                connect_timeout,
                lb_policy,
                load_assignment,
                health_checks,
                upstream_bind_config,
                transport_socket,
                typed_extension_protocol_options,
                cluster_discovery_type,
                lb_config,
                ..
            } = envoy;

            if let Some(lb_config_type) = &lb_config {
                // `lb_config` is a synthetic enum created when parsing the configuration,
                // we can't report it as the actual offending field
                let err = match lb_config_type {
                    EnvoyLbConfig::RingHashLbConfig(_) => GenericError::UnsupportedField("ring_hash_lb_config"),
                    EnvoyLbConfig::MaglevLbConfig(_) => GenericError::UnsupportedField("maglev_lb_config"),
                    EnvoyLbConfig::OriginalDstLbConfig(_) => GenericError::UnsupportedField("original_dst_lb_config"),
                    EnvoyLbConfig::LeastRequestLbConfig(_) => GenericError::UnsupportedField("least_request_lb_config"),
                    EnvoyLbConfig::RoundRobinLbConfig(_) => GenericError::UnsupportedField("round_robin_lb_config"),
                };
                Err(err)?
            }

            let name = CompactString::from(&name);
            if name.is_empty() {
                return Err(GenericError::MissingField("name"));
            }
            let name_for_with_name = name.clone();
            // let cluster_discovery_type = convert_opt!(cluster_discovery_type)?;
            let discovery_settings = (
                required!(cluster_discovery_type)?,
                load_assignment.map(ClusterLoadAssignment::try_from).transpose().with_node("load_assignment")?,
            )
                .try_into()
                .with_node("cluster_discovery_type")?;
            //fixme(hayley): the envoy protobuf documentation says:
            // > If the address and port are empty, no bind will be performed.
            // but its unclear what adress this is refering to. For now we will always bind.
            let bind_device = upstream_bind_config
                .map(bind_device_from_bind_config)
                .transpose()
                .with_node("upstream_bind_config")?
                .flatten();
            let tls_config = transport_socket.map(TlsConfig::try_from).transpose().with_node("transport_socket")?;
            let load_balancing_policy = lb_policy.try_into().with_node("lb_policy")?;
            let http_protocol_options = typed_extension_protocol_options
                .into_values()
                .map(HttpProtocolOptions::try_from)
                .collect::<Result<Vec<_>, GenericError>>()
                .with_node("typed_extension_protocol_options")?;
            if http_protocol_options.len() > 1 {
                return Err(GenericError::from_msg(
                    "Only one set of http protocol options can be specified per upstream",
                ))
                .with_node("typed_extension_protocol_options");
            }
            let http_protocol_options = http_protocol_options.into_iter().next().unwrap_or_default();
            if health_checks.len() > 1 {
                return Err(
                    GenericError::from_msg("only one healthcheck per cluster is supported").with_node("health_check")
                );
            }
            let health_check = health_checks
                .into_iter()
                .next()
                .map(HealthCheck::try_from)
                .transpose()
                .with_index(0)
                .with_node("health_checks")?;

            // These are soft validations related to the health checkers that are hard to encode in the type system,
            // so we'll try to detect as many of them here and fail now. These validations are done again in the
            // actual health checking code, but since we validated the data here, they should always come clean.
            if let Some(health_check_value) = &health_check {
                match &health_check_value.protocol {
                    HealthCheckProtocol::Http(http_check) => {
                        // Validate the host name for the HTTP request
                        match http_check.host(&name) {
                            Ok(_) => (),
                            Err(err @ ClusterHostnameError) => {
                                return Err(GenericError::from_msg_with_cause(
                                    "tried to use the cluster name as the HTTP health check host name (since http_health_check.host was not specified) but failed",
                                    err,
                                )
                                .with_node("name"))
                            },
                        }

                        // Validate the HTTP version of the health checker is supported by the HTTP options
                        if http_check.http_version != http_protocol_options.codec {
                            return Err(GenericError::from_msg("health check and cluster HTTP versions don't match")
                                .with_node("codec_client_type")
                                .with_node("http_health_check")
                                .with_index(0)
                                .with_node("health_checks"));
                        }
                    },
                    HealthCheckProtocol::Grpc(_) => {
                        if !http_protocol_options.codec.is_http2() {
                            return Err(GenericError::from_msg("gRPC health checker requires HTTP 2")
                                .with_node("grpc_health_check")
                                .with_index(0)
                                .with_node("health_checks"));
                        }
                    },
                    HealthCheckProtocol::Tcp(_) => (),
                }
            }

            let connect_timeout = connect_timeout
                .map(duration_from_envoy)
                .transpose()
                .map_err(|_| GenericError::from_msg("Failed to convert connect_timeout into Duration"))
                .with_node("connect_timeout")?;
            Ok(Self {
                name,
                discovery_settings,
                bind_device,
                tls_config,
                load_balancing_policy,
                http_protocol_options,
                health_check,
                connect_timeout,
            })
            .with_name(name_for_with_name)
        }
    }

    impl TryFrom<EnvoyClusterLoadAssignment> for ClusterLoadAssignment {
        type Error = GenericError;
        fn try_from(value: EnvoyClusterLoadAssignment) -> Result<Self, Self::Error> {
            let EnvoyClusterLoadAssignment { cluster_name, endpoints, named_endpoints, policy } = value;
            unsupported_field!(named_endpoints, policy)?;
            let ret = (|| -> Result<_, _> {
                let endpoints: Vec<LocalityLbEndpoints> = convert_non_empty_vec!(endpoints)?;
                if !endpoints.is_empty() {
                    let set_of_priorities = endpoints.iter().map(|e| e.priority).collect::<BTreeSet<u32>>();
                    let n_entries = set_of_priorities.len();
                    let first = set_of_priorities.first().copied().unwrap_or_default();
                    let last = set_of_priorities.last().copied().unwrap_or_default() as usize;
                    if (first, last) != (0, n_entries - 1) {
                        return Err(GenericError::from_msg(
                            "Priorities should range from 0 (highest) to N (lowest) without skipping.",
                        ))
                        .with_node("endpoints");
                    }
                }
                Ok(Self { endpoints })
            })();
            if !cluster_name.is_empty() {
                ret.with_name(cluster_name)
            } else {
                ret
            }
        }
    }

    impl TryFrom<EnvoyLocalityLbEndpoints> for LocalityLbEndpoints {
        type Error = GenericError;
        fn try_from(value: EnvoyLocalityLbEndpoints) -> Result<Self, Self::Error> {
            let EnvoyLocalityLbEndpoints {
                locality,
                lb_endpoints,
                load_balancing_weight,
                priority,
                proximity,
                lb_config,
                metadata: _,
            } = value;
            unsupported_field!(locality, load_balancing_weight, proximity, lb_config)?;
            let lb_endpoints: Vec<LbEndpoint> = convert_non_empty_vec!(lb_endpoints)?;
            let mut sum = 0u32;
            for lb_endpoint in &lb_endpoints {
                sum = if let Some(x) = sum.checked_add(lb_endpoint.load_balancing_weight.into()) {
                    x
                } else {
                    return Err(GenericError::from_msg("Sum of weights has to be less than 4_294_967_295"))
                        .with_node("lb_endpoints");
                }
            }
            Ok(Self { lb_endpoints, priority })
        }
    }

    impl From<EnvoyHealthStatus> for HealthStatus {
        fn from(value: EnvoyHealthStatus) -> Self {
            match value {
                EnvoyHealthStatus::Healthy | EnvoyHealthStatus::Unknown => HealthStatus::Healthy,
                _ => HealthStatus::Unhealthy,
            }
        }
    }

    impl TryFrom<i32> for HealthStatus {
        type Error = GenericError;
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            EnvoyHealthStatus::from_i32(value)
                .ok_or_else(|| GenericError::from_msg(format!("[unknown HealthStatus {value}]")))
                .map(Self::from)
        }
    }

    impl TryFrom<EnvoyLbEndpoint> for LbEndpoint {
        type Error = GenericError;
        fn try_from(value: EnvoyLbEndpoint) -> Result<Self, Self::Error> {
            let EnvoyLbEndpoint { health_status, metadata, load_balancing_weight, host_identifier } = value;
            unsupported_field!(metadata)?;
            let address = match required!(host_identifier)? {
                EnvoyHostIdentifier::Endpoint(EnvoyEndpoint {
                    address,
                    health_check_config,
                    hostname,
                    additional_addresses,
                }) => (|| -> Result<SocketAddr, GenericError> {
                    unsupported_field!(health_check_config, hostname, additional_addresses)?;
                    let address: Address = convert_opt!(address)?;
                    Ok(address.into_socket_addr())
                })(),
                EnvoyHostIdentifier::EndpointName(_) => Err(GenericError::unsupported_variant("EndpointName")),
            }
            .with_node("host")?;
            let load_balancing_weight = load_balancing_weight.map(|v| v.value).unwrap_or(1);
            let load_balancing_weight = NonZeroU32::try_from(load_balancing_weight)
                .map_err(|_| GenericError::from_msg("load_balancing_weight can't be zero"))
                .with_node("load_balancing_weight")?;
            let health_status = health_status.try_into().with_node("health_status")?;
            Ok(Self { address, health_status, load_balancing_weight })
        }
    }

    impl TryFrom<(EnvoyClusterDiscoveryType, Option<ClusterLoadAssignment>)> for ClusterDiscoveryType {
        type Error = GenericError;
        fn try_from(
            (discovery, cla): (EnvoyClusterDiscoveryType, Option<ClusterLoadAssignment>),
        ) -> Result<Self, Self::Error> {
            match discovery {
                EnvoyClusterDiscoveryType::ClusterType(_) => Err(GenericError::unsupported_variant("ClusterType")),
                EnvoyClusterDiscoveryType::Type(x) => (x, cla).try_into(),
            }
        }
    }

    impl TryFrom<(i32, Option<ClusterLoadAssignment>)> for ClusterDiscoveryType {
        type Error = GenericError;
        fn try_from((discovery, cla): (i32, Option<ClusterLoadAssignment>)) -> Result<Self, Self::Error> {
            let discovery = EnvoyDiscoveryType::from_i32(discovery)
                .ok_or_else(|| GenericError::unsupported_variant(format!("[unknown DiscoveryType {discovery}]")))?;
            (discovery, cla).try_into()
        }
    }

    impl TryFrom<(EnvoyDiscoveryType, Option<ClusterLoadAssignment>)> for ClusterDiscoveryType {
        type Error = GenericError;
        fn try_from(
            (discovery, cla): (EnvoyDiscoveryType, Option<ClusterLoadAssignment>),
        ) -> Result<Self, Self::Error> {
            match (discovery, cla) {
                (EnvoyDiscoveryType::Static, Some(cla)) => Ok(Self::Static(cla)),
                (EnvoyDiscoveryType::Static, None) => Err(GenericError::from_msg(
                    "Static clusters are required to have a cluster load assignment configured",
                )),
                (EnvoyDiscoveryType::Eds, None) => Ok(Self::Eds),
                (EnvoyDiscoveryType::Eds, Some(_)) => {
                    Err(GenericError::from_msg("EDS clusters can't have a static cluster load assignment configured"))
                },
                (EnvoyDiscoveryType::LogicalDns, _) => Err(GenericError::unsupported_variant("LogicalDns")),
                (EnvoyDiscoveryType::StrictDns, _) => Err(GenericError::unsupported_variant("StrictDns")),
                (EnvoyDiscoveryType::OriginalDst, _) => Err(GenericError::unsupported_variant("OriginalDst")),
            }
        }
    }

    //todo(hayley): refactor this to a trait impl when splitting the envoy conversions out of this crate
    fn bind_device_from_bind_config(value: EnvoyBindConfig) -> Result<Option<BindDevice>, GenericError> {
        let EnvoyBindConfig {
            source_address,
            freebind,
            socket_options,
            extra_source_addresses,
            additional_source_addresses,
            local_address_selector,
        } = value;
        unsupported_field!(
            source_address,
            freebind,
            // socket_options,
            extra_source_addresses,
            additional_source_addresses,
            local_address_selector
        )?;
        let bind_device = convert_vec!(socket_options)?;
        if bind_device.len() > 1 {
            return Err(GenericError::from_msg("at most one bind device is supported")).with_node("socket_options");
        }
        Ok(bind_device.into_iter().next())
    }

    impl TryFrom<Any> for TlsConfig {
        type Error = GenericError;
        fn try_from(envoy: Any) -> Result<Self, Self::Error> {
            SupportedEnvoyTransportSocket::try_from(envoy)?.try_into()
        }
    }

    impl TryFrom<EnvoyTransportSocket> for TlsConfig {
        type Error = GenericError;
        fn try_from(envoy: EnvoyTransportSocket) -> Result<Self, Self::Error> {
            let EnvoyTransportSocket { name, config_type } = envoy;
            // the envoy docs say that name has to be envoy.transport_sockets.tls or tls (deprecated)
            // but it doesn't actually have to be, it just works with any string but it _is_ required to be
            // non-empty.
            //  so in order to maximize compat with Envoys actual behaviour we check that it's not empty and leave it at that
            let name = required!(name)?;
            (|| -> Result<_, GenericError> {
                match required!(config_type)? {
                    orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::transport_socket::ConfigType::TypedConfig(any) => {
                        Self::try_from(any)
                    }
                }
            })().with_node("config_type").with_name(name)
        }
    }

    impl TryFrom<SupportedEnvoyTransportSocket> for TlsConfig {
        type Error = GenericError;
        fn try_from(value: SupportedEnvoyTransportSocket) -> Result<Self, Self::Error> {
            match value {
                SupportedEnvoyTransportSocket::DownstreamTlsContext(_) => {
                    Err(GenericError::unsupported_variant("DownstreamTlsContext"))
                },
                SupportedEnvoyTransportSocket::UpstreamTlsContext(x) => x.try_into(),
            }
        }
    }

    impl TryFrom<UpstreamTlsContext> for TlsConfig {
        type Error = GenericError;
        fn try_from(value: UpstreamTlsContext) -> Result<Self, Self::Error> {
            let UpstreamTlsContext {
                common_tls_context,
                sni,
                allow_renegotiation,
                max_session_keys,
                enforce_rsa_key_usage,
                auto_host_sni: _,
                auto_sni_san_validation: _,
            } = value;
            unsupported_field!(
                // common_tls_context,
                // sni,
                allow_renegotiation,
                max_session_keys,
                enforce_rsa_key_usage
            )?;
            let CommonTlsContext { parameters, secrets, validation_context } = convert_opt!(common_tls_context)?;
            let secret = match secrets {
                Secrets::Certificates(certs) => {
                    if certs.len() > 1 {
                        Err(GenericError::from_msg("at most one certificate is supported for upstream tls context"))
                    } else {
                        Ok(certs.into_iter().next().map(TlsSecret::Certificate))
                    }
                },
                Secrets::SdsConfig(sds) => {
                    if sds.len() > 1 {
                        Err(GenericError::from_msg("at most one certificate is supported for upstream tls context"))
                    } else {
                        Ok(sds.into_iter().next().map(TlsSecret::SdsConfig))
                    }
                },
            }
            .with_node("common_tls_context")
            .with_node("secrets")?;
            let sni = required!(sni)?.into();
            Ok(Self { sni, parameters, secret, validation_context })
        }
    }

    impl TryFrom<EnvoyLbPolicy> for LbPolicy {
        type Error = GenericError;
        fn try_from(value: EnvoyLbPolicy) -> Result<Self, Self::Error> {
            Ok(match value {
                EnvoyLbPolicy::RoundRobin => Self::RoundRobin,
                EnvoyLbPolicy::Random => Self::Random,
                EnvoyLbPolicy::LeastRequest => Self::LeastRequest,
                EnvoyLbPolicy::RingHash => Self::RingHash,
                EnvoyLbPolicy::Maglev => Self::Maglev,
                EnvoyLbPolicy::ClusterProvided => return Err(GenericError::unsupported_variant("ClusterProvided")),
                EnvoyLbPolicy::LoadBalancingPolicyConfig => {
                    return Err(GenericError::unsupported_variant("LoadBalancingPolicyConfig"))
                },
            })
        }
    }

    impl TryFrom<i32> for LbPolicy {
        type Error = GenericError;
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            EnvoyLbPolicy::from_i32(value)
                .ok_or_else(|| GenericError::unsupported_variant(format!("[unknown LbPolicy {value}]")))?
                .try_into()
        }
    }
}
