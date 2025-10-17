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

pub mod health_check;
pub use health_check::HealthCheck;
pub mod http_protocol_options;
pub use http_protocol_options::HttpProtocolOptions;
pub mod cluster_specifier;
pub use cluster_specifier::ClusterSpecifier;

use crate::config::core::Address;

use super::{
    common::{is_default, MetadataKey},
    secret::TlsCertificate,
    transport::{BindDevice, CommonTlsValidationContext, TlsParameters, UpstreamTransportSocketConfig},
};

use compact_str::CompactString;
use http::HeaderName;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt::Display, net::SocketAddr, num::NonZeroU32, time::Duration};
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Cluster {
    pub name: CompactString,
    #[serde(flatten)]
    pub discovery_settings: ClusterDiscoveryType,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub transport_socket: Option<UpstreamTransportSocketConfig>,
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
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub cleanup_interval: Option<Duration>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub internal_transport_socket: Option<TransportSocket>,
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
        // Only simplify if all addresses are socket addresses
        let socket_addresses: Vec<SocketAddr> = value
            .iter()
            .filter_map(|endpoint| match &endpoint.address {
                EndpointAddress::Socket(addr) => Some(*addr),
                EndpointAddress::Internal(_) => None,
            })
            .collect();

        if socket_addresses.len() == value.len() {
            socket_addresses.serialize(serializer)
        } else {
            value.serialize(serializer)
        }
    } else {
        value.serialize(serializer)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum LbEndpointVecDeser {
    LbEndpoints(Vec<LbEndpoint>),
    SocketAddr(Vec<SocketAddr>),
    Address(Vec<Address>),
}

impl From<LbEndpointVecDeser> for Vec<LbEndpoint> {
    fn from(value: LbEndpointVecDeser) -> Self {
        match value {
            LbEndpointVecDeser::SocketAddr(socket_addrs) => socket_addrs
                .into_iter()
                .map(|socket_addr| LbEndpoint {
                    address: EndpointAddress::Socket(socket_addr),
                    health_status: HealthStatus::default(),
                    load_balancing_weight: NonZeroU32::MIN,
                })
                .collect(),
            LbEndpointVecDeser::Address(address) => address
                .into_iter()
                .filter_map(|address| match address {
                    Address::Socket(socket_addr) => Some(LbEndpoint {
                        address: EndpointAddress::Socket(socket_addr),
                        health_status: HealthStatus::default(),
                        load_balancing_weight: NonZeroU32::MIN,
                    }),
                    Address::Internal(internal_addr) => Some(LbEndpoint {
                        address: EndpointAddress::Internal(InternalEndpointAddress {
                            server_listener_name: internal_addr.server_listener_name.into(),
                            endpoint_id: internal_addr.endpoint_id.map(std::convert::Into::into),
                        }),
                        health_status: HealthStatus::default(),
                        load_balancing_weight: NonZeroU32::MIN,
                    }),
                    Address::Pipe(_, _) => None, // Skip pipe addresses
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
    pub address: EndpointAddress,
    #[serde(skip_serializing_if = "is_default", default)]
    pub health_status: HealthStatus,
    pub load_balancing_weight: NonZeroU32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum EndpointAddress {
    Socket(SocketAddr),
    Internal(InternalEndpointAddress),
}

impl EndpointAddress {
    pub fn into_addr(self) -> Result<SocketAddr, String> {
        match self {
            EndpointAddress::Socket(addr) => Ok(addr),
            EndpointAddress::Internal(_) => Err("Cannot convert internal address to socket address".to_owned()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct InternalEndpointAddress {
    pub server_listener_name: CompactString,
    pub endpoint_id: Option<CompactString>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "name")]
pub enum TransportSocket {
    #[serde(rename = "internal_upstream")]
    InternalUpstream(InternalUpstreamTransport),
    #[serde(rename = "raw_buffer")]
    RawBuffer,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InternalUpstreamTransport {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub passthrough_metadata: Vec<MetadataValueSource>,
    pub transport_socket: Box<TransportSocket>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataValueSource {
    pub kind: MetadataKind,
    pub name: CompactString,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum MetadataKind {
    #[serde(rename = "host")]
    Host,
    #[serde(rename = "route")]
    Route,
    #[serde(rename = "cluster")]
    Cluster,
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
    #[serde(rename = "stict_dns")]
    StrictDns(ClusterLoadAssignment),
    // The ClusterLoadAssignment is optional for EDS clusters since it cannot be
    // configured statically in the bootstrap, but we need to assign it to the
    // serializable type when returning the EDS cluster running configuration
    // through admin config_dump API
    #[serde(rename = "EDS")]
    Eds(Option<ClusterLoadAssignment>),
    #[serde(rename = "ORIGINAL_DST")]
    OriginalDst(OriginalDstConfig),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OriginalDstRoutingMethod {
    #[serde(rename = "use_http_header")]
    HttpHeader {
        #[serde(with = "http_serde_ext::header_name::option", skip_serializing_if = "Option::is_none", default)]
        http_header_name: Option<HeaderName>,
    },
    #[serde(rename = "metadata_key")]
    MetadataKey(MetadataKey),
    #[default]
    Default,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OriginalDstConfig {
    #[serde(flatten)]
    pub routing_method: OriginalDstRoutingMethod,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub upstream_port_override: Option<u16>,
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
    ClusterProvided,
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{
        health_check::{ClusterHostnameError, HealthCheck, HealthCheckProtocol},
        Cluster, ClusterDiscoveryType, ClusterLoadAssignment, EndpointAddress, HealthStatus, HttpProtocolOptions,
        InternalEndpointAddress, InternalUpstreamTransport, LbEndpoint, LbPolicy, LocalityLbEndpoints, MetadataKind,
        MetadataValueSource, OriginalDstConfig, OriginalDstRoutingMethod, TlsConfig, TlsSecret, TransportSocket,
    };
    use crate::config::{
        common::*,
        core::Address,
        transport::{
            BindDevice, CommonTlsContext, Secrets, SupportedEnvoyTransportSocket, UpstreamTransportSocketConfig,
        },
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
            extensions::transport_sockets::{
                internal_upstream::v3::{
                    internal_upstream_transport::MetadataValueSource as EnvoyMetadataValueSource,
                    InternalUpstreamTransport as EnvoyInternalUpstreamTransport,
                },
                tls::v3::UpstreamTlsContext,
            },
            r#type::metadata::v3::{
                metadata_key::path_segment::Segment, metadata_kind::Kind as EnvoyMetadataKindType,
                MetadataKind as EnvoyMetadataKind,
            },
        },
        google::protobuf::Any,
        prost::Message,
    };

    use http::HeaderName;
    use std::{collections::BTreeSet, num::NonZeroU32};

    impl TryFrom<EnvoyCluster> for Cluster {
        type Error = GenericError;
        fn try_from(envoy: EnvoyCluster) -> Result<Self, Self::Error> {
            let EnvoyCluster {
                transport_socket_matches,
                name,
                alt_stat_name,
                eds_cluster_config,
                connect_timeout,
                per_connection_buffer_limit_bytes,
                lb_policy,
                load_assignment,
                health_checks,
                max_requests_per_connection,
                circuit_breakers,
                upstream_http_protocol_options,
                common_http_protocol_options,
                http_protocol_options,
                http2_protocol_options,
                typed_extension_protocol_options,
                dns_refresh_rate,
                dns_failure_refresh_rate,
                respect_dns_ttl,
                dns_lookup_family,
                dns_resolvers,
                use_tcp_for_dns_lookups,
                dns_resolution_config,
                typed_dns_resolver_config,
                wait_for_warm_on_init,
                outlier_detection,
                cleanup_interval,
                upstream_bind_config,
                lb_subset_config,
                common_lb_config,
                transport_socket,
                metadata,
                protocol_selection,
                upstream_connection_options,
                close_connections_on_host_health_failure,
                ignore_health_on_host_removal,
                filters,
                load_balancing_policy,
                lrs_server,
                track_timeout_budgets,
                upstream_config,
                track_cluster_stats,
                preconnect_policy,
                connection_pool_per_downstream_connection,
                cluster_discovery_type,
                lb_config,
                dns_jitter,
                lrs_report_endpoint_metrics,
            } = envoy;
            let name = required!(name)?;
            (|| -> Result<Self, GenericError> {
                unsupported_field!(
                    transport_socket_matches,
                    // name,
                    alt_stat_name,
                    eds_cluster_config,
                    // connect_timeout,
                    per_connection_buffer_limit_bytes,
                    // lb_policy,
                    // load_assignment,
                    // health_checks,
                    max_requests_per_connection,
                    circuit_breakers,
                    upstream_http_protocol_options,
                    common_http_protocol_options,
                    http_protocol_options,
                    http2_protocol_options,
                    // typed_extension_protocol_options,
                    dns_refresh_rate,
                    dns_failure_refresh_rate,
                    respect_dns_ttl,
                    dns_lookup_family,
                    dns_resolvers,
                    use_tcp_for_dns_lookups,
                    dns_resolution_config,
                    typed_dns_resolver_config,
                    wait_for_warm_on_init,
                    outlier_detection,
                    // cleanup_interval,
                    // upstream_bind_config,
                    lb_subset_config,
                    common_lb_config,
                    // transport_socket,
                    metadata,
                    protocol_selection,
                    upstream_connection_options,
                    close_connections_on_host_health_failure,
                    ignore_health_on_host_removal,
                    filters,
                    load_balancing_policy,
                    lrs_server,
                    track_timeout_budgets,
                    upstream_config,
                    track_cluster_stats,
                    preconnect_policy,
                    connection_pool_per_downstream_connection,
                    dns_jitter, // cluster_discovery_type,
                    lrs_report_endpoint_metrics
                    // lb_config

                )?;

                let original_dst_config = if let Some(lb_config_type) = &lb_config {
                    // `lb_config` is a synthetic enum created when parsing the configuration,
                    // we can't report it as the actual offending field
                    match lb_config_type {
                        EnvoyLbConfig::RingHashLbConfig(_) => Err(GenericError::UnsupportedField("ring_hash_lb_config")),
                        EnvoyLbConfig::MaglevLbConfig(_) => Err(GenericError::UnsupportedField("maglev_lb_config")),
                        EnvoyLbConfig::OriginalDstLbConfig(config) => {
                            let routing_method = if config.use_http_header {
                                if config.metadata_key.is_some() {
                                    return Err(GenericError::from_msg(
                                        "use_http_header and metadata_key cannot both be specified - they are mutually exclusive"
                                    ).with_node("original_dst_lb_config"));
                                }
                                OriginalDstRoutingMethod::HttpHeader {
                                    http_header_name: if config.http_header_name.is_empty() {
                                        None
                                    } else {
                                        Some(HeaderName::try_from(&config.http_header_name)
                                            .map_err(|e| GenericError::from_msg_with_cause(
                                                format!("Invalid header name: '{}'", config.http_header_name),
                                                e
                                            ).with_node("http_header_name"))?)
                                    },
                                }
                            } else if let Some(metadata_key) = &config.metadata_key {
                                let key = CompactString::from(&metadata_key.key);
                                let path = metadata_key.path.iter()
                                    .filter_map(|path_segment| {
                                        if let Some(segment) = &path_segment.segment {
                                            match segment {
                                                Segment::Key(key_str) => {
                                                    Some(CompactString::from(key_str))
                                                }
                                            }
                                        } else {
                                            None
                                        }
                                    })
                                    .collect::<Vec<CompactString>>();
                                OriginalDstRoutingMethod::MetadataKey(MetadataKey { key, path })
                            } else {
                                OriginalDstRoutingMethod::Default
                            };
                            let upstream_port_override = if let Some(port_value) = &config.upstream_port_override {
                                let port = u16::try_from(port_value.value).map_err(|_| {
                                    GenericError::from_msg(format!("failed to convert {} to a port number", port_value.value))
                                        .with_node("upstream_port_override")
                                })?;
                                Some(port)
                            } else {
                                None
                            };
                            Ok(Some(OriginalDstConfig {
                                routing_method,
                                upstream_port_override,
                            }))
                        },
                        EnvoyLbConfig::LeastRequestLbConfig(_) => {
                            Err(GenericError::UnsupportedField("least_request_lb_config"))
                        },
                        EnvoyLbConfig::RoundRobinLbConfig(_) => Err(GenericError::UnsupportedField("round_robin_lb_config")),
                    }
                } else {
                    Ok(None)
                }?;
                let name = CompactString::from(&name);
                let discovery_type = extract_discovery_type(&required!(cluster_discovery_type)?)
                    .with_node("cluster_discovery_type")?;
                if discovery_type == EnvoyDiscoveryType::OriginalDst {
                    let envoy_lb_policy = EnvoyLbPolicy::from_i32(lb_policy)
                        .ok_or_else(|| GenericError::unsupported_variant(format!("[unknown LbPolicy {lb_policy}]")))
                        .with_node("lb_policy")?;
                    if envoy_lb_policy != EnvoyLbPolicy::ClusterProvided {
                        return Err(GenericError::from_msg("ORIGINAL_DST clusters must use CLUSTER_PROVIDED load balancing policy")
                            .with_node("lb_policy"));
                    }
                }

                let discovery_settings = ClusterDiscoveryType::try_from((
                    discovery_type,
                    load_assignment.map(ClusterLoadAssignment::try_from).transpose().with_node("load_assignment")?,
                    original_dst_config,
                ))
                .with_node("cluster_discovery_type")?;
                //fixme(hayley): the envoy protobuf documentation says:
                // > If the address and port are empty, no bind will be performed.
                // but its unclear what adress this is refering to. For now we will always bind.
                let bind_device = upstream_bind_config
                    .map(bind_device_from_bind_config)
                    .transpose()
                    .with_node("upstream_bind_config")?
                    .flatten();
                // Parse transport socket: try internal transport first, then fallback to upstream transport
                let (upstream_transport_socket_config, transport_socket_config) = if let Some(ts) = transport_socket {
                    if let Ok(internal_socket) = TransportSocket::try_from(ts.clone()) {
                        // It's an internal transport socket
                        (None, Some(internal_socket))
                    } else {
                        // It's not an internal transport socket, so try to parse it as a regular upstream transport socket (e.g., TLS)
                        let upstream_socket = UpstreamTransportSocketConfig::try_from(ts).with_node("transport_socket")?;
                        (Some(upstream_socket), None)
                    }
                } else {
                    (None, None)
                };
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
                    return Err(GenericError::from_msg("only one healthcheck per cluster is supported")
                        .with_node("health_check"));
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
                                return Err(GenericError::from_msg(
                                    "health check and cluster HTTP versions don't match",
                                )
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
                let cleanup_interval = cleanup_interval
                    .map(duration_from_envoy)
                    .transpose()
                    .map_err(|_| GenericError::from_msg("Failed to convert cleanup_interval into Duration"))
                    .with_node("cleanup_interval")?;
                Ok(Self {
                    name,
                    discovery_settings,
                    bind_device,
                    transport_socket: upstream_transport_socket_config,
                    load_balancing_policy,
                    http_protocol_options,
                    health_check,
                    connect_timeout,
                    cleanup_interval,
                    internal_transport_socket: transport_socket_config,
                })
            })()
            .with_name(name)
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
                return ret.with_name(cluster_name);
            }
            ret
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
                metadata,
            } = value;
            unsupported_field!(locality, load_balancing_weight, proximity, lb_config, metadata)?;
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
                }) => (|| -> Result<EndpointAddress, GenericError> {
                    unsupported_field!(health_check_config, hostname, additional_addresses)?;
                    let address: Address = convert_opt!(address)?;
                    match address {
                        Address::Socket(socket_addr) => Ok(EndpointAddress::Socket(socket_addr)),
                        Address::Internal(internal_addr) => Ok(EndpointAddress::Internal(InternalEndpointAddress {
                            server_listener_name: internal_addr.server_listener_name.into(),
                            endpoint_id: internal_addr.endpoint_id.map(std::convert::Into::into),
                        })),
                        Address::Pipe(_, _) => {
                            Err(GenericError::unsupported_variant("Pipe addresses are not supported for endpoints"))
                        },
                    }
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

    impl TryFrom<(EnvoyDiscoveryType, Option<ClusterLoadAssignment>, Option<OriginalDstConfig>)> for ClusterDiscoveryType {
        type Error = GenericError;
        fn try_from(
            (discovery, cla, odc): (EnvoyDiscoveryType, Option<ClusterLoadAssignment>, Option<OriginalDstConfig>),
        ) -> Result<Self, Self::Error> {
            match (discovery, cla) {
                (EnvoyDiscoveryType::Static, Some(cla)) => {
                    if cla
                        .endpoints
                        .iter()
                        .flat_map(|e| e.lb_endpoints.iter().map(|e| e.address.clone().into_addr()).collect::<Vec<_>>())
                        .filter(std::result::Result::is_err)
                        .collect::<Vec<_>>()
                        .is_empty()
                    {
                        Ok(ClusterDiscoveryType::Static(cla))
                    } else {
                        Err(GenericError::from_msg(
                            "Static clusters are required to have a cluster load assignment configured and all endpoints must be valid IP addresses",
                        ))
                    }
                },
                (EnvoyDiscoveryType::Static, None) => Err(GenericError::from_msg(
                    "Static clusters are required to have a cluster load assignment configured",
                )),
                (EnvoyDiscoveryType::Eds, None) => Ok(Self::Eds(None)),
                (EnvoyDiscoveryType::Eds, Some(_)) => {
                    Err(GenericError::from_msg("EDS clusters can't have a static cluster load assignment configured"))
                },
                (EnvoyDiscoveryType::LogicalDns, _) => Err(GenericError::unsupported_variant("LogicalDns")),
                (EnvoyDiscoveryType::StrictDns, Some(cla)) => Ok(ClusterDiscoveryType::StrictDns(cla)),
                (EnvoyDiscoveryType::StrictDns, None) => Err(GenericError::from_msg(
                    "Strict DNS clusters are required to have a cluster load assignment configured",
                )),
                (EnvoyDiscoveryType::OriginalDst, _) => Ok(Self::OriginalDst(odc.unwrap_or_default())),
            }
        }
    }

    fn extract_discovery_type(discovery: &EnvoyClusterDiscoveryType) -> Result<EnvoyDiscoveryType, GenericError> {
        match discovery {
            EnvoyClusterDiscoveryType::ClusterType(_) => Err(GenericError::unsupported_variant("ClusterType")),
            EnvoyClusterDiscoveryType::Type(x) => EnvoyDiscoveryType::from_i32(*x)
                .ok_or_else(|| GenericError::unsupported_variant(format!("[unknown DiscoveryType {x}]"))),
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

    impl TryFrom<Any> for UpstreamTransportSocketConfig {
        type Error = GenericError;
        fn try_from(envoy: Any) -> Result<Self, Self::Error> {
            SupportedEnvoyTransportSocket::try_from(envoy)?.try_into()
        }
    }

    impl TryFrom<EnvoyTransportSocket> for UpstreamTransportSocketConfig {
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

    impl TryFrom<SupportedEnvoyTransportSocket> for UpstreamTransportSocketConfig {
        type Error = GenericError;
        fn try_from(value: SupportedEnvoyTransportSocket) -> Result<Self, Self::Error> {
            match value {
                SupportedEnvoyTransportSocket::DownstreamTlsContext(_) => Err(GenericError::unsupported_variant(
                    "DownstreamTlsContext is not supported in TransportSocket configured on a cluster",
                )),
                SupportedEnvoyTransportSocket::UpstreamTlsContext(x) => {
                    Ok(UpstreamTransportSocketConfig::Tls(TlsConfig::try_from(x)?))
                },
                SupportedEnvoyTransportSocket::ProxyProtocolUpstreamTransport(x) => {
                    Ok(UpstreamTransportSocketConfig::ProxyProtocol(x.try_into()?))
                },
                SupportedEnvoyTransportSocket::RawBuffer(_) => Ok(UpstreamTransportSocketConfig::RawBuffer),
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
                auto_host_sni,
                auto_sni_san_validation,
            } = value;
            unsupported_field!(
                // common_tls_context,
                // sni,
                allow_renegotiation,
                max_session_keys,
                enforce_rsa_key_usage,
                auto_host_sni,
                auto_sni_san_validation
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
                EnvoyLbPolicy::ClusterProvided => Self::ClusterProvided,
                EnvoyLbPolicy::LoadBalancingPolicyConfig => {
                    return Err(GenericError::unsupported_variant("LoadBalancingPolicyConfig"));
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

    impl TryFrom<EnvoyTransportSocket> for TransportSocket {
        type Error = GenericError;
        fn try_from(value: EnvoyTransportSocket) -> Result<Self, Self::Error> {
            let EnvoyTransportSocket { name, config_type } = value;
            let name = required!(name)?;
            let config = required!(config_type)?;

            match name.as_str() {
                "internal_upstream" => {
                    match config {
                        orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::transport_socket::ConfigType::TypedConfig(any) => {
                            if any.type_url == "type.googleapis.com/envoy.extensions.transport_sockets.internal_upstream.v3.InternalUpstreamTransport" {
                                let internal_transport = EnvoyInternalUpstreamTransport::decode(any.value.as_slice())
                                    .map_err(|e| GenericError::from_msg_with_cause("Failed to decode InternalUpstreamTransport", e))?;
                                Ok(TransportSocket::InternalUpstream(internal_transport.try_into()?))
                            } else {
                                Err(GenericError::from_msg(format!("Unsupported transport socket type: {}", any.type_url)))
                            }
                        }
                    }
                }
                "raw_buffer" => Ok(TransportSocket::RawBuffer),
                _ => Err(GenericError::unsupported_variant(name)),
            }
        }
    }

    impl TryFrom<EnvoyInternalUpstreamTransport> for InternalUpstreamTransport {
        type Error = GenericError;
        fn try_from(value: EnvoyInternalUpstreamTransport) -> Result<Self, Self::Error> {
            let EnvoyInternalUpstreamTransport { passthrough_metadata, transport_socket } = value;
            let passthrough_metadata =
                passthrough_metadata.into_iter().map(MetadataValueSource::try_from).collect::<Result<Vec<_>, _>>()?;
            let transport_socket = required!(transport_socket)?;
            let transport_socket = Box::new(TransportSocket::try_from(transport_socket)?);
            Ok(Self { passthrough_metadata, transport_socket })
        }
    }

    impl TryFrom<EnvoyMetadataValueSource> for MetadataValueSource {
        type Error = GenericError;
        fn try_from(value: EnvoyMetadataValueSource) -> Result<Self, Self::Error> {
            let EnvoyMetadataValueSource { kind, name } = value;
            let kind = required!(kind)?;
            let name = required!(name)?.into();
            let kind = MetadataKind::try_from(kind)?;
            Ok(Self { kind, name })
        }
    }

    impl TryFrom<EnvoyMetadataKind> for MetadataKind {
        type Error = GenericError;
        fn try_from(value: EnvoyMetadataKind) -> Result<Self, Self::Error> {
            let EnvoyMetadataKind { kind } = value;
            match required!(kind)? {
                EnvoyMetadataKindType::Host(_) => Ok(MetadataKind::Host),
                EnvoyMetadataKindType::Route(_) => Ok(MetadataKind::Route),
                EnvoyMetadataKindType::Cluster(_) => Ok(MetadataKind::Cluster),
                EnvoyMetadataKindType::Request(_) => Ok(MetadataKind::Route), // Map Request to Route for now
            }
        }
    }
}
