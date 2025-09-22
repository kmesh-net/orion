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

use std::{collections::BTreeSet, net::{IpAddr, SocketAddr}, str::FromStr, time::{Duration, Instant}};

use rustls::ClientConfig;
use if_addrs;
use orion_configuration::config::{
    cluster::{ClusterDiscoveryType, HealthCheck, OriginalDstRoutingMethod}, transport::{BindDeviceOptions}
};
use tracing::{debug, warn};
use webpki::types::ServerName;

use crate::{
    clusters::{
        clusters_manager::{RoutingContext, RoutingRequirement},
        health::HealthStatus,
    },
    secrets::{TlsConfigurator, TransportSecret, WantsToBuildClient},
    transport::{
        GrpcService, HttpChannel, HttpChannelBuilder, TcpChannelConnector, UpstreamTransportSocketConfigurator,
    },
    Result,
};
use http::{uri::Authority, HeaderName, HeaderValue};
use orion_configuration::config::cluster::HttpProtocolOptions;

use super::{ClusterOps, ClusterType};

/// Envoy doesn't seem to have any kind of limitation of the number of endpoints
/// in the cluster, but seems like a sensible thing to have. Let's have at least
/// a big number. Each endpoint has a connection pool, so we are not limiting
/// the number of connections.
const MAXIMUM_ENDPOINTS: usize = 10_000;

/// Default cleanup interval for `ORIGINAL_DST` clusters.
/// See <https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/cluster/v3/cluster.proto#envoy-v3-api-field-config-cluster-v3-cluster-cleanup-interval>
const DEFAULT_CLEANUP_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct OriginalDstClusterBuilder {
    pub name: &'static str,
    pub bind_device_options: BindDeviceOptions,
    pub transport_socket: UpstreamTransportSocketConfigurator,
    pub connect_timeout: Option<Duration>,
    pub server_name: Option<ServerName<'static>>,
    pub config: orion_configuration::config::cluster::Cluster,
}

impl OriginalDstClusterBuilder {
    pub fn build(self) -> ClusterType {
        let OriginalDstClusterBuilder { name, bind_device_options, transport_socket, connect_timeout, server_name, config } =
            self;
        let (routing_requirements, upstream_port_override) =
            if let ClusterDiscoveryType::OriginalDst(ref original_dst_config) = config.discovery_settings {
                let routing_req = match &original_dst_config.routing_method {
                    OriginalDstRoutingMethod::HttpHeader { http_header_name } => {
                        let header_name = http_header_name
                            .to_owned()
                            .unwrap_or_else(|| HeaderName::from_static("x-envoy-original-dst-host"));
                        RoutingRequirement::Header(header_name)
                    },
                    OriginalDstRoutingMethod::MetadataKey(_) => {
                        warn!("Routing by metadata is not supported yet for ORIGINAL_DST cluster");
                        RoutingRequirement::Authority
                    },
                    OriginalDstRoutingMethod::Default => RoutingRequirement::Authority,
                };
                (routing_req, original_dst_config.upstream_port_override)
            } else {
                (RoutingRequirement::Authority, None)
            };
        let http_config = HttpChannelConfig {
            tls_configurator: transport_socket.tls_configurator().cloned(),
            connect_timeout,
            server_name,
            http_protocol_options: config.http_protocol_options.clone(),
        };
        ClusterType::OnDemand(OriginalDstCluster {
            name,
            http_config,
            transport_socket,
            bind_device_options,
            cleanup_interval: config.cleanup_interval.unwrap_or(DEFAULT_CLEANUP_INTERVAL),
            endpoints: lrumap::LruMap::new(),
            routing_requirements,
            upstream_port_override,
            last_cleanup_time: Instant::now(),
            config,
        })
    }
}

#[derive(Debug, Clone)]
struct HttpChannelConfig {
    tls_configurator: Option<TlsConfigurator<ClientConfig, WantsToBuildClient>>,
    connect_timeout: Option<Duration>,
    server_name: Option<ServerName<'static>>,
    http_protocol_options: HttpProtocolOptions,
}

#[derive(Debug, Clone)]
pub struct OriginalDstCluster {
    pub name: &'static str,
    http_config: HttpChannelConfig,
    pub transport_socket: UpstreamTransportSocketConfigurator,
    bind_device_options: BindDeviceOptions,
    cleanup_interval: Duration,
    endpoints: lrumap::LruMap<EndpointAddress, Endpoint>,
    routing_requirements: RoutingRequirement,
    upstream_port_override: Option<u16>,
    last_cleanup_time: Instant,
    pub config: orion_configuration::config::cluster::Cluster,
}



impl ClusterOps for OriginalDstCluster {
    fn get_name(&self) -> &'static str {
        self.name
    }

    fn into_health_check(self) -> Option<HealthCheck> {
        None
    }

    fn all_http_channels(&self) -> Vec<(Authority, HttpChannel)> {
        self.endpoints
            .iter()
            .filter_map(|endpoint_addr| {
                self.endpoints
                    .get(endpoint_addr)
                    .map(|endpoint| (endpoint_addr.0.clone(), endpoint.http_channel.clone()))
            })
            .collect()
    }

    fn all_tcp_channels(&self) -> Vec<(Authority, TcpChannelConnector)> {
        self.endpoints
            .iter()
            .filter_map(|endpoint_addr| {
                self.endpoints
                    .get(endpoint_addr)
                    .map(|endpoint| (endpoint_addr.0.clone(), endpoint.tcp_channel.clone()))
            })
            .collect()
    }

    fn all_grpc_channels(&self) -> Vec<Result<(Authority, GrpcService)>> {
        self.endpoints
            .iter()
            .filter_map(|endpoint_addr| {
                self.endpoints
                    .get(endpoint_addr)
                    .map(|endpoint| endpoint.grpc_service().map(|service| (endpoint_addr.0.clone(), service)))
            })
            .collect()
    }

    fn change_tls_context(&mut self, secret_id: &str, secret: TransportSecret) -> Result<()> {
        if let Some(tls_configurator) = self.http_config.tls_configurator.clone() {
            let tls_configurator =
                TlsConfigurator::<ClientConfig, WantsToBuildClient>::update(tls_configurator, secret_id, secret)?;
            self.http_config.tls_configurator = Some(tls_configurator);
        }
        Ok(())
    }

    fn update_health(&mut self, _endpoint: &http::uri::Authority, _health: HealthStatus) {
        // ORIGINAL_DST clusters do not support health checks
    }

    fn get_http_connection(&mut self, context: RoutingContext) -> Result<HttpChannel> {
        warn!("OriginalDstCluster get HTTP connection for {:?}", context);
        match context {
            RoutingContext::Authority(authority, original_dst_address) => self.get_http_connection_by_authority(authority,Some(original_dst_address)),
            RoutingContext::Header(header_value) => self.get_http_connection_by_header(header_value),
            _ => Err(format!("ORIGINAL_DST cluster {} requires authority or header routing context", self.name).into()),
        }
    }

    fn get_tcp_connection(&mut self, context: RoutingContext) -> Result<TcpChannelConnector> {
        warn!("OriginalDstCluster get TCP connection for {:?}", context);
        match context {
            RoutingContext::Authority(authority, original_dst_address) => self.get_tcp_connection_by_authority(authority),
            _ => Err(format!("ORIGINAL_DST cluster {} requires authority routing context", self.name).into()),
        }
    }

    fn get_grpc_connection(&mut self, context: RoutingContext) -> Result<GrpcService> {
        match context {
            RoutingContext::Authority(authority, original_dst_address) => self.get_grpc_connection_by_authority(authority),
            _ => Err(format!("ORIGINAL_DST cluster {} requires authority routing context", self.name).into()),
        }
    }

    fn get_routing_requirements(&self) -> RoutingRequirement {
        self.routing_requirements.clone()
    }
}

impl OriginalDstCluster {
    fn apply_port_override(&self, authority: Authority) -> Result<Authority> {
        match self.upstream_port_override {
            Some(port_override) => {
                let host = authority.host();
                format!("{host}:{port_override}").parse::<Authority>().map_err(|e| {
                    format!("Failed to apply port override {} for cluster {}: {}", port_override, self.name, e).into()
                })
            },
            None => Ok(authority),
        }
    }

    pub fn get_grpc_connection_by_authority(&mut self, authority: Authority) -> Result<GrpcService> {
        let authority = self.apply_port_override(authority)?;

        let endpoint_addr = EndpointAddress(authority.clone());
        if let Some(endpoint) = self.endpoints.touch(&endpoint_addr) {
            return endpoint.grpc_service();
        }

        self.cleanup_if_needed();

        let endpoint =
            Endpoint::try_new(&authority, &self.http_config, self.bind_device_options.clone(), self.transport_socket.clone())?;
        let grpc_service = endpoint.grpc_service()?;
        self.endpoints.insert(&endpoint_addr, endpoint);
        Ok(grpc_service)
    }

    pub fn get_tcp_connection_by_authority(&mut self, authority: Authority) -> Result<TcpChannelConnector> {
        let authority = self.apply_port_override(authority)?;

        let endpoint_addr = EndpointAddress(authority.clone());
        if let Some(endpoint) = self.endpoints.touch(&endpoint_addr) {
            return Ok(endpoint.tcp_channel.clone());
        }

        self.cleanup_if_needed();

        let endpoint =
            Endpoint::try_new(&authority, &self.http_config, self.bind_device_options.clone(), self.transport_socket.clone())?;
        let tcp_connector = endpoint.tcp_channel.clone();
        self.endpoints.insert(&endpoint_addr, endpoint);
        Ok(tcp_connector)
    }

    pub fn get_http_connection_by_header(&mut self, header_value: &HeaderValue) -> Result<HttpChannel> {
        let authority = Authority::try_from(header_value.as_bytes())
            .map_err(|_| format!("Invalid authority in header for ORIGINAL_DST cluster {}", self.name))?;
        self.get_http_connection_by_authority(authority, None)
    }

    pub fn get_http_connection_by_authority(&mut self, authority: Authority, original_dst_address: Option<SocketAddr>) -> Result<HttpChannel> {
        debug!("Original Dst Cluster original authority {authority} original dst address {original_dst_address:?} ");
        
        let authority = if let Some(dst_address) = original_dst_address{
            Authority::from_str(&dst_address.to_string())?                
        }else{
            authority
        };
        
        let authority = self.apply_port_override(authority)?;

        let endpoint_addr = EndpointAddress(authority.clone());
        if let Some(endpoint) = self.endpoints.touch(&endpoint_addr) {
            return Ok(endpoint.http_channel.clone());
        }

        self.cleanup_if_needed();

        warn!("Original Dst Cluster using authority {authority}");
        let endpoint =
            Endpoint::try_new(&authority, &self.http_config, self.bind_device_options.clone(), self.transport_socket.clone())?;
        let http_channel = endpoint.http_channel.clone();
        self.endpoints.insert(&endpoint_addr, endpoint);
        Ok(http_channel)
    }

    fn cleanup_if_needed(&mut self) {
        let now = Instant::now();

        let time_since_last_cleanup = now.duration_since(self.last_cleanup_time);
        let should_cleanup =
            self.endpoints.len() >= MAXIMUM_ENDPOINTS || time_since_last_cleanup >= self.cleanup_interval;

        if should_cleanup {
            self.cleanup();

            if self.endpoints.len() >= MAXIMUM_ENDPOINTS {
                warn!(


                    "ORIGINAL_DST cluster {} is running over its connection (pool) limit with {} dynamic endpoints",
                    self.name,
                    self.endpoints.len()
                );
            }
        }
    }

    pub fn cleanup(&mut self) {
        self.last_cleanup_time = Instant::now();

        let Some(cleanup_deadline) = self.last_cleanup_time.checked_sub(self.cleanup_interval) else {
            warn!("Invalid ORIGINAL_DST cluster cleanup interval");
            return;
        };

        let expired_endpoints: Vec<EndpointAddress> = self
            .endpoints
            .iter()
            .take_while(|endpoint| {
                let Some(time) = self.endpoints.last_update(endpoint) else {
                    return false;
                };
                time < cleanup_deadline
            })
            .cloned()
            .collect();

        debug!("Removing {} stale ORIGINAL_DST endpoints", expired_endpoints.len());

        for expired_endpoint in expired_endpoints {
            self.endpoints.remove(&expired_endpoint);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EndpointAddress(Authority);

#[derive(Debug, Clone)]
struct Endpoint {
    http_channel: HttpChannel,
    tcp_channel: TcpChannelConnector,
}

impl Endpoint {
    fn try_new(        
        authority: &Authority,
        http_config: &HttpChannelConfig,
        bind_device_options: BindDeviceOptions,
        transport_socket: UpstreamTransportSocketConfigurator,
    ) -> Result<Self> {
        let builder = HttpChannelBuilder::new(bind_device_options.clone())            
            .with_authority(authority.clone())
            .with_timeout(http_config.connect_timeout);
        let builder = if let Some(tls_conf) = &http_config.tls_configurator {
            if let Some(server_name) = &http_config.server_name {
                builder.with_tls(Some(tls_conf.clone())).with_server_name(server_name.clone())
            } else {
                builder.with_tls(Some(tls_conf.clone()))
            }
        } else {
            builder
        };
        let http_channel = builder.with_http_protocol_options(http_config.http_protocol_options.clone()).build_with_no_address()?;
        let tcp_channel = TcpChannelConnector::new(
            authority,
            "original_dst_cluster",
            bind_device_options,
            http_config.connect_timeout,
            transport_socket,
        );

        Ok(Endpoint { http_channel, tcp_channel })
    }

    fn grpc_service(&self) -> Result<GrpcService> {
        GrpcService::try_new(self.http_channel.clone(), self.http_channel.upstream_authority.clone())
    }
}

impl PartialOrd for EndpointAddress {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EndpointAddress {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.as_str().cmp(other.0.as_str())
    }
}

mod lrumap {
    use std::{
        cmp::Ordering,
        collections::{BTreeSet, HashMap},
        hash::Hash,
        time::Instant,
    };

    /// A container that can access items by key in O(1) time, and orders them by access time.
    /// Accessing an item will update its access time.
    /// Insertion and removal is O(log n).
    #[derive(Debug, Clone)]
    pub struct LruMap<K, V>
    where
        K: Clone + Eq + Hash,
    {
        by_key: HashMap<K, LruValue<V>>,
        by_time: BTreeSet<LruKey<K>>,
    }

    impl<K: Clone + Eq + Hash, V> Default for LruMap<K, V> {
        fn default() -> Self {
            LruMap { by_key: HashMap::default(), by_time: BTreeSet::default() }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LruValue<V> {
        value: V,
        time: Instant,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LruKey<K: Clone> {
        key: K,
        time: Instant,
    }

    impl<K: Clone + PartialEq + PartialOrd> PartialOrd for LruKey<K> {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            self.time.partial_cmp(&other.time).or_else(|| self.key.partial_cmp(&other.key))
        }
    }

    impl<K: Clone + Ord> Ord for LruKey<K> {
        fn cmp(&self, other: &Self) -> Ordering {
            match self.time.cmp(&other.time) {
                Ordering::Equal => self.key.cmp(&other.key),
                unequal => unequal,
            }
        }
    }

    impl<K: Clone + Ord + Hash, V> LruMap<K, V> {
        pub fn new() -> Self {
            LruMap::default()
        }

        pub fn len(&self) -> usize {
            self.by_key.len()
        }

        pub fn iter(&'_ self) -> LruMapIter<'_, K> {
            self.into_iter()
        }

        pub fn last_update(&self, key: &K) -> Option<Instant> {
            self.by_key.get(key).map(|lru_value| lru_value.time)
        }

        /// Retrieves the value corresponding to `key` without updating its access time.
        /// This is used for introspection without affecting LRU ordering.
        pub fn get(&self, key: &K) -> Option<&V> {
            self.by_key.get(key).map(|lru_value| &lru_value.value)
        }

        /// Retrieves the value corresponding to `key`, and if it exists updates its access time.
        /// The cost of this method is O(log n).
        pub fn touch(&mut self, key: &K) -> Option<&V> {
            let now = Instant::now();
            self.touch_with_time(key, now)
        }

        fn touch_with_time(&mut self, key: &K, time: Instant) -> Option<&V> {
            let entry = self.by_key.get_mut(key)?;
            let old_lru_key = LruKey { key: key.clone(), time: entry.time };

            let new_lru_key = LruKey { key: key.clone(), time };

            debug_assert!(self.by_time.remove(&old_lru_key), "LruMap corrupted while LruMap::touch()");
            self.by_time.insert(new_lru_key);

            entry.time = time;

            Some(&entry.value)
        }

        /// Inserts an element into the map. If it already existed, overwrites it and returns true.
        pub fn insert(&mut self, key: &K, value: V) -> bool {
            let now = Instant::now();
            self.insert_with_time(key, value, now)
        }

        fn insert_with_time(&mut self, key: &K, value: V, time: Instant) -> bool {
            let lru_value = LruValue { value, time };
            let lru_key = LruKey { key: key.clone(), time };

            let is_new = if let Some(entry) = self.by_key.insert(key.clone(), lru_value) {
                let old_lru_value = LruKey { key: key.clone(), time: entry.time };
                self.by_time.remove(&old_lru_value);
                false
            } else {
                true
            };
            self.by_time.insert(lru_key);

            is_new
        }

        #[allow(dead_code)]
        pub fn remove(&mut self, key: &K) -> bool {
            let Some(entry) = self.by_key.remove(key) else {
                return false;
            };

            let lru_key = LruKey { key: key.clone(), time: entry.time };
            debug_assert!(self.by_time.remove(&lru_key), "LruMap corrupted while LruMap::remove()");

            true
        }
    }

    pub struct LruMapIter<'a, K: Clone + Eq + Ord + Hash> {
        iter: std::collections::btree_set::Iter<'a, LruKey<K>>,
    }

    impl<'a, K: Clone + Eq + Ord + Hash> Iterator for LruMapIter<'a, K> {
        type Item = &'a K;

        fn next(&mut self) -> Option<Self::Item> {
            self.iter.next().map(|lru_key| &lru_key.key)
        }
    }

    impl<'a, K: Clone + Eq + Ord + Hash, V> IntoIterator for &'a LruMap<K, V> {
        type Item = &'a K;
        type IntoIter = LruMapIter<'a, K>;

        fn into_iter(self) -> Self::IntoIter {
            LruMapIter { iter: self.by_time.iter() }
        }
    }

    // TODO: understand why tests are failing in CI (github)
    #[cfg(any())]
    mod lrumap_tests {
        use std::{
            collections::HashMap,
            time::{Duration, Instant},
        };

        use super::LruMap;

        struct LruMapFixture {
            map: LruMap<usize, &'static str>,
            control_map: HashMap<usize, &'static str>,
        }

        impl LruMapFixture {
            fn new() -> Self {
                LruMapFixture { map: LruMap::new(), control_map: HashMap::new() }
            }

            fn insert(&mut self, key: usize, value: &'static str, time: Instant) -> bool {
                let insert_expectation = self.control_map.insert(key, value).is_none();
                let insert_result = self.map.insert_with_time(&key, value, time);

                assert_eq!(insert_result, insert_expectation, "control map expectation unfulfilled");
                assert_eq!(self.map.len(), self.control_map.len(), "control map expectation unfulfilled");

                insert_result
            }

            fn remove(&mut self, key: usize) -> bool {
                let removal_expectation = self.control_map.remove(&key).is_some();
                let removal_result = self.map.remove(&key);
                assert_eq!(removal_result, removal_expectation, "control map expectation unfulfilled");
                assert_eq!(self.map.len(), self.control_map.len(), "control map expectation unfulfilled");

                removal_result
            }

            fn touch(&mut self, key: usize, time: Instant) -> Option<&str> {
                let value_expectation = self.control_map.get(&key);
                let value = self.map.touch_with_time(&key, time);
                assert_eq!(value, value_expectation, "control map expectation unfulfilled");
                let value = value.copied(); // &&str -> &str
                assert_eq!(self.map.len(), self.control_map.len(), "control map expectation unfulfilled");

                value
            }

            fn assert_ordered_values(&self, keys: &[usize], error_message: &str) {
                assert_eq!(self.map.len(), keys.len(), "expected same map size");

                let mut values_iter = self.map.into_iter();
                let mut expected_iter = keys.iter();
                loop {
                    match (values_iter.next(), expected_iter.next()) {
                        (Some(value), Some(expected)) => {
                            assert_eq!(value, expected, "expected same map order: {}", error_message)
                        },
                        (None, None) => break,
                        _ => panic!("expected same map values: {}", error_message),
                    }
                }
            }
        }

        #[test]
        fn crud() {
            let mut map = LruMapFixture::new();

            // Map is empty
            map.assert_ordered_values(&[], "initially empty");

            // Insert one element
            let t0 = Instant::now();

            assert!(map.insert(0, "0", t0));
            map.assert_ordered_values(&[0], "after inserting first element");
            assert!(!map.insert(0, "0", t0));
            map.assert_ordered_values(&[0], "after reinserting first element");

            // Touch one element
            let t1 = t0 + Duration::from_secs(2);

            assert_eq!(map.touch(0, t1), Some("0"));
            map.assert_ordered_values(&[0], "after touching the only element");

            // Remove the only element
            assert!(map.remove(0));
            map.assert_ordered_values(&[], "after removing the only element");
            assert!(!map.remove(0));

            // Insert again
            assert!(map.insert(0, "0", t0));
            map.assert_ordered_values(&[0], "after reinserting first element");

            // Insert another
            assert!(map.insert(1, "1", t1));
            map.assert_ordered_values(&[0, 1], "after inserting second element");

            // Touch oldest element
            let t2 = t1 + Duration::from_secs(2);

            assert_eq!(map.touch(0, t2), Some("0"));
            map.assert_ordered_values(&[1, 0], "after touching oldest element");

            // Add another
            let t3 = t2 + Duration::from_secs(2);
            assert!(map.insert(2, "2", t3));
            map.assert_ordered_values(&[1, 0, 2], "after inserting third element");

            // Touch oldest element
            let t4 = t3 + Duration::from_secs(2);

            assert_eq!(map.touch(1, t4), Some("1"));
            map.assert_ordered_values(&[0, 2, 1], "after touching oldest element");

            // Remove elements
            assert!(map.remove(2));
            map.assert_ordered_values(&[0, 1], "after removing middle element");

            assert!(map.remove(1));
            map.assert_ordered_values(&[0], "after removing another element");

            assert!(map.remove(0));
            map.assert_ordered_values(&[], "after removing last element");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::SecretManager;
    use orion_configuration::config::cluster::{
        http_protocol_options::Codec, Cluster as ClusterConfig, ClusterDiscoveryType, LbPolicy, OriginalDstConfig,
    };
    use std::str::FromStr;

    fn create_test_cluster_config(
        name: &str,
        routing_method: OriginalDstRoutingMethod,
        port_override: Option<u16>,
        cleanup_interval: Option<Duration>,
    ) -> ClusterConfig {
        ClusterConfig {
            name: name.into(),
            discovery_settings: ClusterDiscoveryType::OriginalDst(OriginalDstConfig {
                routing_method,
                upstream_port_override: port_override,
            }),
            cleanup_interval,
            transport_socket: None,
            bind_device_options: BindDeviceOptions::default(),
            load_balancing_policy: LbPolicy::ClusterProvided,
            http_protocol_options: HttpProtocolOptions::default(),
            health_check: None,
            connect_timeout: None,
        }
    }

    fn build_original_dst_cluster(config: ClusterConfig) -> OriginalDstCluster {
        let secrets_man = SecretManager::new();
        let partial = super::super::PartialClusterType::try_from((config, &secrets_man)).unwrap();
        match partial.build().unwrap() {
            ClusterType::OnDemand(cluster) => cluster,
            _ => unreachable!("expected OriginalDstCluster config"),
        }
    }

    #[test]
    fn test_http_connection_by_authority() {
        let config = create_test_cluster_config("test-cluster", OriginalDstRoutingMethod::Default, None, None);
        let mut cluster = build_original_dst_cluster(config);

        let authority = Authority::from_str("localhost:52000").unwrap();
        let channel1 = cluster.get_http_connection(RoutingContext::Authority(authority.clone(), "127.0.0.1:9000".parse().expect("Do expect this to work"))).unwrap();
        let channel2 = cluster.get_http_connection(RoutingContext::Authority(authority, "127.0.0.1:9000".parse().expect("Do expect this to work"))).unwrap();
        assert_eq!(channel1.upstream_authority, channel2.upstream_authority);
        assert_eq!(cluster.endpoints.len(), 1);

        let config = create_test_cluster_config(
            "test-cluster",
            OriginalDstRoutingMethod::HttpHeader { http_header_name: None },
            Some(50001),
            None,
        );
        let mut cluster = build_original_dst_cluster(config);
        assert_eq!(
            cluster.get_routing_requirements(),
            RoutingRequirement::Header(HeaderName::from_static("x-envoy-original-dst-host"))
        );

        let header_value = HeaderValue::from_str("localhost:52000").unwrap();
        let channel = cluster.get_http_connection(RoutingContext::Header(&header_value)).unwrap();
        assert_eq!(channel.upstream_authority.as_str(), "localhost:50001");

        let http_no_dest = cluster.get_http_connection(RoutingContext::None);
        assert!(http_no_dest.is_err());
    }

    #[test]
    fn test_get_tcp_connection() {
        let config = create_test_cluster_config("test-cluster", OriginalDstRoutingMethod::Default, Some(50002), None);
        let mut cluster = build_original_dst_cluster(config);

        let authority = Authority::from_str("localhost:52000").unwrap();
        let _tcp_future = cluster.get_tcp_connection(RoutingContext::Authority(authority, "127.0.0.1:9000".parse().expect("Do expect this to work"))).unwrap();

        let endpoints = cluster.all_tcp_channels();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].0.as_str(), "localhost:50002");

        let tcp_no_dest = cluster.get_tcp_connection(RoutingContext::None);
        assert!(tcp_no_dest.is_err());
    }

    #[test]
    fn test_grpc_connection() {
        let mut config = create_test_cluster_config("test-cluster", OriginalDstRoutingMethod::Default, None, None);
        config.http_protocol_options.codec = Codec::Http2;
        let mut cluster = build_original_dst_cluster(config);

        let auth1 = Authority::from_str("localhost:5100").unwrap();
        let auth1_context = RoutingContext::Authority(auth1, "127.0.0.1:9000".parse().expect("Do expect this to work"));
        let auth2 = Authority::from_str("localhost:5101").unwrap();
        let auth2_context = RoutingContext::Authority(auth2, "127.0.0.1:9000".parse().expect("Do expect this to work"));

        let _grpc1 = cluster.get_grpc_connection(auth1_context).unwrap();
        let _grpc2 = cluster.get_grpc_connection(auth2_context).unwrap();

        let grpc_channels = cluster.all_grpc_channels();
        assert_eq!(grpc_channels.len(), 2);
        for result in &grpc_channels {
            assert!(result.is_ok());
        }

        let grpc_no_dest = cluster.get_grpc_connection(RoutingContext::None);
        assert!(grpc_no_dest.is_err());
    }
}
