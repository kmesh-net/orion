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

use super::{
    GenericError,
    network_filters::{
        HttpConnectionManager, NetworkRbac, TcpProxy,
        access_log::{AccessLog, AccessLogConf},
    },
    transport::{BindDevice, CommonTlsContext},
};
use crate::config::listener;
use crate::config::network_filters::tracing::{TracingConfig, TracingKey};
use compact_str::CompactString;
use ipnet::IpNet;
use orion_interner::StringInterner;
use serde::{Deserialize, Serialize, Serializer};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

// Removed unused import

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Listener {
    pub name: CompactString,
    pub address: ListenerAddress,
    #[serde(with = "serde_filterchains")]
    pub filter_chains: HashMap<FilterChainMatch, FilterChain>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub bind_device: Option<BindDevice>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub with_tls_inspector: bool,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub proxy_protocol_config: Option<super::listener_filters::DownstreamProxyProtocolConfig>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub with_tlv_listener_filter: bool,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub tlv_listener_filter_config: Option<super::listener_filters::TlvListenerFilterConfig>,
}

impl Listener {
    pub fn get_access_log_configurations(&self) -> Vec<AccessLogConf> {
        self.filter_chains
            .iter()
            .flat_map(|(_, filter_chain)| match &filter_chain.terminal_filter {
                MainFilter::Http(http_connection_manager) => {
                    http_connection_manager.access_log.iter().map(AccessLog::get_config).cloned().collect::<Vec<_>>()
                },
                MainFilter::Tcp(tcp_proxy) => {
                    tcp_proxy.access_log.iter().map(AccessLog::get_config).cloned().collect::<Vec<_>>()
                },
            })
            .collect::<Vec<_>>()
    }

    pub fn get_tracing_configurations(&self) -> HashMap<TracingKey, TracingConfig> {
        self.filter_chains
            .iter()
            .filter_map(|(filter_chain_match, filter_chain)| {
                let mut filter_chain_match_hash = DefaultHasher::new();
                filter_chain_match.hash(&mut filter_chain_match_hash);

                match &filter_chain.terminal_filter {
                    MainFilter::Http(http_connection_manager) => {
                        http_connection_manager.tracing.as_ref().map(|tracing| {
                            (TracingKey(self.name.to_static_str(), filter_chain_match_hash.finish()), tracing.clone())
                        })
                    },
                    MainFilter::Tcp(_) => None,
                }
            })
            .collect::<HashMap<_, _>>()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ListenerAddress {
    Socket(SocketAddr),
    Internal(InternalListener),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InternalListener {
    pub buffer_size_kb: Option<u32>,
}

mod serde_filterchains {
    use serde::Deserializer;

    use crate::config::is_default;

    use super::*;
    pub fn serialize<S: Serializer>(
        value: &HashMap<FilterChainMatch, FilterChain>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        #[allow(clippy::trivially_copy_pass_by_ref)]
        fn is_default_ref(fcm: &&FilterChainMatch) -> bool {
            is_default(*fcm)
        }
        #[derive(Serialize)]
        struct SerializeAs<'a> {
            #[serde(rename = "filterchain_match", skip_serializing_if = "is_default_ref")]
            key: &'a FilterChainMatch,
            #[serde(flatten)]
            value: &'a FilterChain,
        }
        serializer.collect_seq(value.iter().map(|(key, value)| SerializeAs { key, value }))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<HashMap<FilterChainMatch, FilterChain>, D::Error> {
        #[derive(Deserialize)]
        struct DeserializeAs {
            #[serde(rename = "filterchain_match", default)]
            key: FilterChainMatch,
            #[serde(flatten)]
            value: FilterChain,
        }
        let kvp = Vec::<DeserializeAs>::deserialize(deserializer)?;
        let vec_len = kvp.len();
        let hashmap = kvp.into_iter().map(|DeserializeAs { key, value }| (key, value)).collect::<HashMap<_, _>>();
        match hashmap.len() {
            0 => Err(serde::de::Error::custom("Listener needs atleast one filter_chain")),
            x if x == vec_len => Ok(hashmap),
            _ => Err(serde::de::Error::custom("all match statements in a filterchain have to be unique")),
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FilterChain {
    pub filter_chain_match_hash: u64,
    pub name: CompactString,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub tls_config: Option<listener::TlsConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub rbac: Vec<NetworkRbac>,
    pub terminal_filter: MainFilter,
}

//todo(hayley): neater serialize/deserialize
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct ServerNameMatch {
    //eg example.com
    name: CompactString,
    // should we also match on anything.example.com? (but not anythingexample.com)
    match_subdomains: bool,
}

impl FromStr for ServerNameMatch {
    type Err = GenericError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // we don't check if the label is a valid hostname here, we only check for wildcards
        let (match_subdomains, s) = if s.starts_with("*.") { (true, &s[1..]) } else { (false, s) };
        if s.contains('*') {
            return Err(GenericError::from_msg(
                "internal wildcards are not supported (Hostnames may only start with '*.')",
            ));
        }
        // we convert the hostname to lowercase since hostnames should be matched case-insensitively
        Ok(Self { name: s.to_lowercase().into(), match_subdomains })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize, Default)]
pub struct FilterChainMatch {
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub destination_port: Option<u16>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub destination_prefix_ranges: Vec<IpNet>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub server_names: Vec<ServerNameMatch>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub source_prefix_ranges: Vec<IpNet>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub source_ports: Vec<u16>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MatchResult {
    FailedMatch,
    NoRule,
    Matched(u32), //todo, invert
}

impl PartialOrd for MatchResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MatchResult {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Matched(x), Self::Matched(y)) => x.cmp(y).reverse(), //inverted, lower-score means more specific match
            (Self::FailedMatch, Self::FailedMatch) | (Self::NoRule, Self::NoRule) => std::cmp::Ordering::Equal,
            // anything matched is better than not matched, NoRule is better than failing
            (Self::Matched(_), _) | (Self::NoRule, Self::FailedMatch) => std::cmp::Ordering::Greater,
            (_, Self::Matched(_)) | (Self::FailedMatch, Self::NoRule) => std::cmp::Ordering::Less,
        }
    }
}

impl FilterChainMatch {
    pub fn matches_destination_port(&self, port: u16) -> MatchResult {
        match self.destination_port {
            Some(destination) if port == destination => MatchResult::Matched(0),
            Some(_) => MatchResult::FailedMatch,
            None => MatchResult::NoRule,
        }
    }

    ///For criteria that allow ranges or wildcards, the most specific value in any of the configured filter chains that matches the incoming connection is going to be used (e.g. for SNI www.example.com the most specific match would be www.example.com, then *.example.com, then *.com, then any filter chain without `server_names` requirements).
    pub fn matches_destination_ip(&self, ip: IpAddr) -> MatchResult {
        self.destination_prefix_ranges
            .iter()
            .map(|range| {
                if range.contains(&ip) {
                    let bits_matched = match ip {
                        IpAddr::V4(_) => 32,
                        IpAddr::V6(_) => 128,
                    } - u32::from(range.prefix_len());
                    MatchResult::Matched(bits_matched)
                } else {
                    MatchResult::FailedMatch
                }
            })
            .max()
            .unwrap_or(MatchResult::NoRule)
    }

    pub fn matches_server_name(&self, server_name: &str) -> MatchResult {
        self.server_names
            .iter()
            .map(|name_match| {
                if name_match.match_subdomains {
                    //something.example.com matching *.example.com
                    // trim the '*' in the matcher
                    if server_name.ends_with(name_match.name.as_str()) {
                        // the score is the amount of labels in server_name that matched on the '*' (lower is more specific)
                        MatchResult::Matched(
                            // -1 so we include and extra dot and ".bad.domain" matching "*.bad.domain" won't score equal to an exact match
                            server_name[0..server_name.len() - (name_match.name.len() - 1)]
                                .chars()
                                .filter(|c| *c == '.')
                                .count()
                                .try_into()
                                .unwrap_or(u32::MAX),
                        )
                    } else {
                        MatchResult::FailedMatch
                    }
                } else if server_name == name_match.name {
                    MatchResult::Matched(0)
                } else {
                    MatchResult::FailedMatch
                }
            })
            .max()
            .unwrap_or(MatchResult::NoRule)
    }

    pub fn matches_source_port(&self, source_port: u16) -> MatchResult {
        if self.source_ports.is_empty() {
            MatchResult::NoRule
        } else if self.source_ports.contains(&source_port) {
            MatchResult::Matched(0)
        } else {
            MatchResult::FailedMatch
        }
    }

    ///For criteria that allow ranges or wildcards, the most specific value in any of the configured filter chains that matches the incoming connection is going to be used (e.g. for SNI www.example.com the most specific match would be www.example.com, then *.example.com, then *.com, then any filter chain without `server_names` requirements).
    pub fn matches_source_ip(&self, ip: IpAddr) -> MatchResult {
        self.source_prefix_ranges
            .iter()
            .map(|range| {
                if range.contains(&ip) {
                    let bits_matched = match ip {
                        IpAddr::V4(_) => 32,
                        IpAddr::V6(_) => 128,
                    } - u32::from(range.prefix_len());
                    MatchResult::Matched(bits_matched)
                } else {
                    MatchResult::FailedMatch
                }
            })
            .max()
            .unwrap_or(MatchResult::NoRule)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type")]
#[serde(rename_all = "UPPERCASE")]
pub enum MainFilter {
    Http(HttpConnectionManager),
    Tcp(TcpProxy),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TlsConfig {
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub require_client_certificate: bool,
    #[serde(flatten)]
    pub common_tls_context: CommonTlsContext,
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use std::collections::HashMap;
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::str::FromStr;

    use super::{FilterChain, FilterChainMatch, Listener, MainFilter, ServerNameMatch, TlsConfig};
    use crate::config::{
        common::*,
        core::{Address, CidrRange},
        listener_filters::{ListenerFilter, ListenerFilterConfig},
        transport::SupportedEnvoyTransportSocket,
        util::{envoy_u32_to_u16, u32_to_u16},
    };
    use compact_str::CompactString;
    use orion_data_plane_api::envoy_data_plane_api::{
        envoy::{
            config::{
                core::v3::TransportSocket as EnvoyTransportSocket,
                listener::v3::{
                    Filter as EnvoyFilter, FilterChain as EnvoyFilterChain, FilterChainMatch as EnvoyFilterChainMatch,
                    Listener as EnvoyListener, filter::ConfigType as EnvoyConfigType,
                },
            },
            extensions::{
                filters::network::{
                    http_connection_manager::v3::HttpConnectionManager as EnvoyHttpConnectionManager,
                    rbac::v3::Rbac as EnvoyNetworkRbac, tcp_proxy::v3::TcpProxy as EnvoyTcpProxy,
                },
                transport_sockets::tls::v3::DownstreamTlsContext as EnvoyDownstreamTlsContext,
            },
        },
        google::protobuf::Any,
        prost::Message,
    };

    impl TryFrom<EnvoyListener> for Listener {
        type Error = GenericError;
        fn try_from(envoy: EnvoyListener) -> Result<Self, Self::Error> {
            let EnvoyListener {
                name,
                address,
                additional_addresses,
                stat_prefix,
                filter_chains,
                filter_chain_matcher,
                use_original_dst,
                default_filter_chain,
                per_connection_buffer_limit_bytes,
                metadata,
                deprecated_v1,
                drain_type,
                listener_filters,
                listener_filters_timeout,
                continue_on_listener_filters_timeout,
                transparent,
                freebind,
                socket_options,
                tcp_fast_open_queue_length,
                traffic_direction,
                udp_listener_config,
                api_listener,
                connection_balance_config,
                reuse_port,
                enable_reuse_port,
                access_log,
                tcp_backlog_size,
                max_connections_to_accept_per_socket_event,
                bind_to_port,
                enable_mptcp,
                ignore_global_conn_limit,
                listener_specifier,
                bypass_overload_manager,
                fcds_config,
            } = envoy;
            unsupported_field!(
                // name,
                // address,
                additional_addresses,
                stat_prefix,
                // filter_chains,
                filter_chain_matcher,
                use_original_dst,
                default_filter_chain,
                per_connection_buffer_limit_bytes,
                metadata,
                deprecated_v1,
                drain_type,
                // listener_filters,
                listener_filters_timeout,
                continue_on_listener_filters_timeout,
                transparent,
                freebind,
                // socket_options,
                tcp_fast_open_queue_length,
                traffic_direction,
                udp_listener_config,
                api_listener,
                connection_balance_config,
                reuse_port,
                enable_reuse_port,
                access_log,
                tcp_backlog_size,
                max_connections_to_accept_per_socket_event,
                bind_to_port,
                enable_mptcp,
                ignore_global_conn_limit,
                listener_specifier,
                bypass_overload_manager,
                fcds_config
            )?;
            let name: CompactString = required!(name)?.into();
            (|| -> Result<_, GenericError> {
                let name = name.clone();
                let address_result = convert_opt!(address)?;
                let address = match address_result {
                    Address::Socket(socket_addr) => crate::config::listener::ListenerAddress::Socket(socket_addr),
                    Address::Internal(_internal_addr) => {
                        crate::config::listener::ListenerAddress::Internal(crate::config::listener::InternalListener {
                            buffer_size_kb: None, // Default buffer size, can be configured via bootstrap extension
                        })
                    },
                    Address::Pipe(_, _) => {
                        return Err(GenericError::unsupported_variant("Pipe addresses are not supported for listeners"));
                    },
                };
                let filter_chains: Vec<FilterChainWrapper> = convert_non_empty_vec!(filter_chains)?;
                let n_filter_chains = filter_chains.len();
                let filter_chains: HashMap<_, _> = filter_chains.into_iter().map(|x| x.0).collect();

                // This is a hard requirement from Envoy as otherwise it can't pick which filterchain to use.
                if filter_chains.len() != n_filter_chains {
                    return Err(GenericError::from_msg("filter chain contains duplicate filter_chain_match entries")
                        .with_node("filter_chains"));
                }
                let listener_filters: Vec<ListenerFilter> = convert_vec!(listener_filters)?;
                let mut with_tls_inspector = false;
                let mut proxy_protocol_config = None;
                let mut with_tlv_listener_filter = false;
                let mut tlv_listener_filter_config = None;

                for filter in listener_filters {
                    match filter.config {
                        ListenerFilterConfig::TlsInspector => {
                            if with_tls_inspector {
                                return Err(GenericError::from_msg("duplicate TLS inspector listener filter"))
                                    .with_node("listener_filters");
                            }
                            with_tls_inspector = true;
                        },
                        ListenerFilterConfig::ProxyProtocol(config) => {
                            if proxy_protocol_config.is_some() {
                                return Err(GenericError::from_msg("duplicate proxy protocol listener filter"))
                                    .with_node("listener_filters");
                            }
                            proxy_protocol_config = Some(config);
                        },
                        ListenerFilterConfig::TlvListenerFilter(config) => {
                            if with_tlv_listener_filter {
                                return Err(GenericError::from_msg("duplicate TLV listener filter"))
                                    .with_node("listener_filters");
                            }
                            with_tlv_listener_filter = true;
                            tlv_listener_filter_config = Some(config);
                        },
                    }
                }
                let bind_device = convert_vec!(socket_options)?;
                if bind_device.len() > 1 {
                    return Err(GenericError::from_msg("at most one bind device is supported"))
                        .with_node("socket_options");
                }
                let bind_device = bind_device.into_iter().next();
                Ok(Self {
                    name,
                    address,
                    filter_chains,
                    bind_device,
                    with_tls_inspector,
                    proxy_protocol_config,
                    with_tlv_listener_filter,
                    tlv_listener_filter_config,
                })
            }())
            .with_name(name)
        }
    }

    struct FilterChainWrapper((FilterChainMatch, FilterChain));

    impl TryFrom<EnvoyFilterChain> for FilterChainWrapper {
        type Error = GenericError;
        fn try_from(envoy: EnvoyFilterChain) -> Result<Self, Self::Error> {
            let EnvoyFilterChain {
                filter_chain_match,
                filters,
                use_proxy_proto,
                metadata,
                transport_socket,
                transport_socket_connect_timeout,
                name,
            } = envoy;
            unsupported_field!(
                // filter_chain_match,
                // filters,
                use_proxy_proto,
                metadata,
                // transport_socket,
                transport_socket_connect_timeout // name,
            )?;
            let name: CompactString = if name.is_empty() { "".into() } else { name.into() };
            (|| -> Result<_, GenericError> {
                let name = name.clone();
                let filter_chain_match = filter_chain_match
                    .map(FilterChainMatch::try_from)
                    .transpose()
                    .with_node("filter_chain_match")?
                    .unwrap_or_default();
                let filters = required!(filters)?;
                let mut rbac = Vec::new();
                let mut main_filter = None;
                for (idx, filter) in filters.into_iter().enumerate() {
                    let filter_name = filter.name.clone().is_used().then_some(filter.name.clone());
                    match Filter::try_from(filter) {
                        Ok(f) => match f.filter {
                            SupportedEnvoyFilter::NetworkRbac(rbac_filter) => {
                                if main_filter.is_some() {
                                    Err(GenericError::from_msg(
                            "rbac filter found after a http connection manager or tcp proxy in the same filterchain",
                        ))
                                } else {
                                    match rbac_filter.try_into() {
                                        Ok(rbac_filter) => {
                                            rbac.push(rbac_filter);
                                            Ok(())
                                        },
                                        Result::<_, GenericError>::Err(e) => Err(e),
                                    }
                                }
                            },

                            SupportedEnvoyFilter::HttpConnectionManager(http) => {
                                if main_filter.is_some() {
                                    Err(GenericError::from_msg(
                                        "multiple http connection managers or tcp proxies defined in filterchain",
                                    ))
                                } else {
                                    match http.try_into() {
                                        Err(e) => Err(e),
                                        Ok(http) => {
                                            main_filter = Some(MainFilter::Http(http));
                                            Ok(())
                                        },
                                    }
                                }
                            },
                            SupportedEnvoyFilter::TcpProxy(tcp) => {
                                if main_filter.is_some() {
                                    Err(GenericError::from_msg(
                                        "multiple http connection managers or tcp proxies defined in filterchain",
                                    ))
                                } else {
                                    match tcp.try_into() {
                                        Err(e) => Err(e),
                                        Ok(tcp) => {
                                            main_filter = Some(MainFilter::Tcp(tcp));
                                            Ok(())
                                        },
                                    }
                                }
                            },
                        },
                        Err(e) => Err(e),
                    }
                    .map_err(|err| if let Some(name) = filter_name { err.with_name(name) } else { err })
                    .with_index(idx)
                    .with_node("filters")?;
                }

                let Some(terminal_filter) = main_filter else {
                    return Err(GenericError::from_msg("no tcp proxy or http connection manager specified for chain")
                        .with_node("filters"));
                };
                let tls_config = transport_socket
                    .map(try_tls_config_from_envoy)
                    .transpose()
                    .with_node("transport_socket")?
                    .flatten();

                let mut s = DefaultHasher::new();
                filter_chain_match.hash(&mut s);
                Ok(FilterChainWrapper((
                    filter_chain_match,
                    FilterChain { filter_chain_match_hash: s.finish(), name, rbac, terminal_filter, tls_config },
                )))
            }())
            .with_name(name)
        }
    }

    impl TryFrom<EnvoyFilterChainMatch> for FilterChainMatch {
        type Error = GenericError;
        fn try_from(envoy: EnvoyFilterChainMatch) -> Result<Self, Self::Error> {
            let EnvoyFilterChainMatch {
                destination_port,
                prefix_ranges,
                address_suffix,
                suffix_len,
                direct_source_prefix_ranges,
                source_type,
                source_prefix_ranges,
                source_ports,
                server_names,
                transport_protocol,
                application_protocols,
            } = envoy;
            unsupported_field!(
                // destination_port,
                // prefix_ranges,
                address_suffix,
                suffix_len,
                direct_source_prefix_ranges,
                source_type,
                // source_prefix_ranges,
                // source_ports,
                // server_names,
                transport_protocol,
                application_protocols
            )?;
            let server_names = server_names
                .into_iter()
                .map(|s| ServerNameMatch::from_str(&s))
                .collect::<Result<Vec<_>, _>>()
                .with_node("server_names")?;
            if server_names.iter().any(|sn| sn.name == "*") {
                return Err(
                    GenericError::from_msg("full wildcard entries ('*') are not supported").with_node("server_names")
                );
            }
            let destination_port = destination_port.map(envoy_u32_to_u16).transpose().with_node("destination_port")?;
            let source_ports =
                source_ports.into_iter().map(u32_to_u16).collect::<Result<_, _>>().with_node("source_ports")?;
            let destination_prefix_ranges = prefix_ranges
                .into_iter()
                .map(|envoy| CidrRange::try_from(envoy).map(CidrRange::into_ipnet))
                .collect::<Result<_, _>>()
                .with_node("prefix_ranges")?;
            let source_prefix_ranges = source_prefix_ranges
                .into_iter()
                .map(|envoy| CidrRange::try_from(envoy).map(CidrRange::into_ipnet))
                .collect::<Result<_, _>>()
                .with_node("source_prefix_ranges")?;
            Ok(Self { server_names, destination_port, source_ports, destination_prefix_ranges, source_prefix_ranges })
        }
    }

    #[derive(Debug, Clone)]
    struct Filter {
        #[allow(unused)]
        pub name: Option<CompactString>,
        pub filter: SupportedEnvoyFilter,
    }

    impl TryFrom<EnvoyFilter> for Filter {
        type Error = GenericError;
        fn try_from(envoy: EnvoyFilter) -> Result<Self, Self::Error> {
            let EnvoyFilter { name, config_type } = envoy;
            let name = name.is_used().then_some(CompactString::from(name));

            let result = (|| -> Result<_, GenericError> {
                let filter: SupportedEnvoyFilter = match required!(config_type)? {
                    EnvoyConfigType::ConfigDiscovery(_) => Err(GenericError::unsupported_variant("ConfigDiscovery")),
                    EnvoyConfigType::TypedConfig(typed_config) => SupportedEnvoyFilter::try_from(typed_config),
                }
                .with_node("config_type")?;
                Ok(Self { name: name.clone(), filter })
            })();

            if let Some(name) = name {
                return result.with_name(name);
            }
            result
        }
    }

    #[allow(clippy::large_enum_variant)]
    #[derive(Debug, Clone)]
    enum SupportedEnvoyFilter {
        HttpConnectionManager(EnvoyHttpConnectionManager),
        NetworkRbac(EnvoyNetworkRbac),
        TcpProxy(EnvoyTcpProxy),
    }

    impl TryFrom<Any> for SupportedEnvoyFilter {
        type Error = GenericError;
        fn try_from(typed_config: Any) -> Result<Self, Self::Error> {
            match typed_config.type_url.as_str() {
            "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager" => {
                EnvoyHttpConnectionManager::decode(typed_config.value.as_slice()).map(Self::HttpConnectionManager)
            },
            "type.googleapis.com/envoy.extensions.filters.network.rbac.v3.RBAC" => {
                EnvoyNetworkRbac::decode(typed_config.value.as_slice()).map(Self::NetworkRbac)
            },
            "type.googleapis.com/envoy.extensions.filters.network.tcp_proxy.v3.TcpProxy" => {
                EnvoyTcpProxy::decode(typed_config.value.as_slice()).map(Self::TcpProxy)
            },
            _ => {
                return Err(GenericError::unsupported_variant(typed_config.type_url));
            },
        }
        .map_err(|e| {
            GenericError::from_msg_with_cause(format!("failed to parse protobuf for \"{}\"", typed_config.type_url), e)
        })
        }
    }

    fn try_tls_config_from_envoy(transport_socket: EnvoyTransportSocket) -> Result<Option<TlsConfig>, GenericError> {
        let EnvoyTransportSocket { name, config_type } = transport_socket;
        let name = required!(name)?;
        let maybe_tls_config = match required!(config_type)? {
            orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::transport_socket::ConfigType::TypedConfig(any) => {
                let transport_socket = SupportedEnvoyTransportSocket::try_from(any)?;
                match transport_socket {
                    SupportedEnvoyTransportSocket::DownstreamTlsContext(x) => Some(x.try_into()).transpose(),
                    SupportedEnvoyTransportSocket::RawBuffer(_) => Ok(None),
                    SupportedEnvoyTransportSocket::UpstreamTlsContext(_)
                    | SupportedEnvoyTransportSocket::ProxyProtocolUpstreamTransport(_) => {
                        Err(GenericError::unsupported_variant(
                            "Only DownstreamTlsContext or RawBuffer transport sockets are supported on listeners",
                        ))
                    },
                }
            },
        };
        maybe_tls_config.with_node("config_type").with_name(name)
    }

    impl TryFrom<EnvoyDownstreamTlsContext> for TlsConfig {
        type Error = GenericError;
        fn try_from(value: EnvoyDownstreamTlsContext) -> Result<Self, Self::Error> {
            let EnvoyDownstreamTlsContext {
                common_tls_context,
                require_client_certificate,
                require_sni,
                disable_stateful_session_resumption,
                session_timeout,
                ocsp_staple_policy,
                full_scan_certs_on_sni_mismatch,
                session_ticket_keys_type,
                prefer_client_ciphers,
            } = value;
            unsupported_field!(
                // common_tls_context,
                // require_client_certificate,
                require_sni,
                disable_stateful_session_resumption,
                session_timeout,
                ocsp_staple_policy,
                full_scan_certs_on_sni_mismatch,
                session_ticket_keys_type,
                prefer_client_ciphers
            )?;
            let require_client_certificate = require_client_certificate.is_some_and(|v| v.value);
            let common_tls_context = convert_opt!(common_tls_context)?;
            Ok(Self { require_client_certificate, common_tls_context })
        }
    }
}
