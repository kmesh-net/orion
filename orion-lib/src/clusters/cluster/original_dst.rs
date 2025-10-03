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

use std::time::Duration;

use lru_time_cache::LruCache;
use rustls::ClientConfig;

use orion_configuration::config::{
    cluster::{ClusterDiscoveryType, HealthCheck, OriginalDstRoutingMethod},
    transport::BindDevice,
};
use tracing::warn;
use webpki::types::ServerName;

use crate::{
    Result,
    clusters::{
        clusters_manager::{RoutingContext, RoutingRequirement},
        health::HealthStatus,
    },
    secrets::{TlsConfigurator, TransportSecret, WantsToBuildClient},
    transport::{
        GrpcService, HttpChannel, HttpChannelBuilder, TcpChannelConnector, UpstreamTransportSocketConfigurator,
    },
};
use http::{HeaderName, HeaderValue, uri::Authority};
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
    pub bind_device: Option<BindDevice>,
    pub transport_socket: UpstreamTransportSocketConfigurator,
    pub connect_timeout: Option<Duration>,
    pub server_name: Option<ServerName<'static>>,
    pub config: orion_configuration::config::cluster::Cluster,
}

impl OriginalDstClusterBuilder {
    pub fn build(self) -> ClusterType {
        let OriginalDstClusterBuilder { name, bind_device, transport_socket, connect_timeout, server_name, config } =
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
            bind_device,
            endpoints: LruCache::with_expiry_duration_and_capacity(
                config.cleanup_interval.unwrap_or(DEFAULT_CLEANUP_INTERVAL),
                MAXIMUM_ENDPOINTS,
            ),
            routing_requirements,
            upstream_port_override,
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

#[derive(Clone)]
pub struct OriginalDstCluster {
    pub name: &'static str,
    http_config: HttpChannelConfig,
    transport_socket: UpstreamTransportSocketConfigurator,
    bind_device: Option<BindDevice>,
    endpoints: LruCache<EndpointAddress, Endpoint>,
    routing_requirements: RoutingRequirement,
    upstream_port_override: Option<u16>,
    pub config: orion_configuration::config::cluster::Cluster,
}

impl ClusterOps for OriginalDstCluster {
    fn get_name(&self) -> &'static str {
        self.name
    }

    fn into_health_check(self) -> Option<HealthCheck> {
        None
    }

    fn all_http_channels(&mut self) -> Vec<(Authority, HttpChannel)> {
        self.endpoints.iter().map(|(addr, endpoint)| (addr.0.clone(), endpoint.http_channel.clone())).collect()
    }

    fn all_tcp_channels(&mut self) -> Vec<(Authority, TcpChannelConnector)> {
        self.endpoints.iter().map(|(addr, endpoint)| (addr.0.clone(), endpoint.tcp_channel.clone())).collect()
    }

    fn all_grpc_channels(&mut self) -> Vec<Result<(Authority, GrpcService)>> {
        self.endpoints
            .iter()
            .map(|(addr, endpoint)| endpoint.grpc_service().map(|service| (addr.0.clone(), service)))
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
        match context {
            RoutingContext::Authority(authority) => self.get_http_connection_by_authority(authority),
            RoutingContext::Header(header_value) => self.get_http_connection_by_header(header_value),
            _ => Err(format!("ORIGINAL_DST cluster {} requires authority or header routing context", self.name).into()),
        }
    }

    fn get_tcp_connection(&mut self, context: RoutingContext) -> Result<TcpChannelConnector> {
        match context {
            RoutingContext::Authority(authority) => self.get_tcp_connection_by_authority(authority),
            _ => Err(format!("ORIGINAL_DST cluster {} requires authority routing context", self.name).into()),
        }
    }

    fn get_grpc_connection(&mut self, context: RoutingContext) -> Result<GrpcService> {
        match context {
            RoutingContext::Authority(authority) => self.get_grpc_connection_by_authority(authority),
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
        if let Some(endpoint) = self.endpoints.get(&endpoint_addr) {
            return endpoint.grpc_service();
        }

        let endpoint =
            Endpoint::try_new(&authority, &self.http_config, self.bind_device.clone(), self.transport_socket.clone())?;

        let grpc_service = endpoint.grpc_service()?;
        self.endpoints.insert(endpoint_addr, endpoint);
        Ok(grpc_service)
    }

    pub fn get_tcp_connection_by_authority(&mut self, authority: Authority) -> Result<TcpChannelConnector> {
        let authority = self.apply_port_override(authority)?;

        let endpoint_addr = EndpointAddress(authority.clone());
        if let Some(endpoint) = self.endpoints.get(&endpoint_addr) {
            return Ok(endpoint.tcp_channel.clone());
        }

        let endpoint =
            Endpoint::try_new(&authority, &self.http_config, self.bind_device.clone(), self.transport_socket.clone())?;
        let tcp_connector = endpoint.tcp_channel.clone();
        self.endpoints.insert(endpoint_addr, endpoint);
        Ok(tcp_connector)
    }

    pub fn get_http_connection_by_header(&mut self, header_value: &HeaderValue) -> Result<HttpChannel> {
        let authority = Authority::try_from(header_value.as_bytes())
            .map_err(|_| format!("Invalid authority in header for ORIGINAL_DST cluster {}", self.name))?;
        self.get_http_connection_by_authority(authority)
    }

    pub fn get_http_connection_by_authority(&mut self, authority: Authority) -> Result<HttpChannel> {
        let authority = self.apply_port_override(authority)?;

        let endpoint_addr = EndpointAddress(authority.clone());
        if let Some(endpoint) = self.endpoints.get(&endpoint_addr) {
            return Ok(endpoint.http_channel.clone());
        }

        let endpoint =
            Endpoint::try_new(&authority, &self.http_config, self.bind_device.clone(), self.transport_socket.clone())?;
        let http_channel = endpoint.http_channel.clone();
        self.endpoints.insert(endpoint_addr, endpoint);
        Ok(http_channel)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EndpointAddress(Authority);

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

#[derive(Debug, Clone)]
struct Endpoint {
    http_channel: HttpChannel,
    tcp_channel: TcpChannelConnector,
}

impl Endpoint {
    fn try_new(
        authority: &Authority,
        http_config: &HttpChannelConfig,
        bind_device: Option<BindDevice>,
        transport_socket: UpstreamTransportSocketConfigurator,
    ) -> Result<Self> {
        let builder = HttpChannelBuilder::new(bind_device.clone())
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
        let http_channel = builder.with_http_protocol_options(http_config.http_protocol_options.clone()).build()?;
        let tcp_channel = TcpChannelConnector::new(
            authority,
            "original_dst_cluster",
            bind_device,
            http_config.connect_timeout,
            transport_socket,
        );

        Ok(Endpoint { http_channel, tcp_channel })
    }

    fn grpc_service(&self) -> Result<GrpcService> {
        GrpcService::try_new(self.http_channel.clone(), self.http_channel.upstream_authority.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{collections::HashMap, time::Duration};

    use super::LruCache;

    struct LruMapFixture {
        map: LruCache<usize, &'static str>,
        control_map: HashMap<usize, &'static str>,
    }

    impl LruMapFixture {
        fn new() -> Self {
            LruMapFixture { map: LruCache::with_capacity(100), control_map: HashMap::new() }
        }

        fn insert(&mut self, key: usize, value: &'static str) -> bool {
            let insert_expectation = self.control_map.insert(key, value);
            let insert_result = self.map.insert(key, value);

            assert_eq!(insert_result, insert_expectation, "control map expectation unfulfilled");
            assert_eq!(self.map.len(), self.control_map.len(), "control map expectation unfulfilled");

            insert_result.is_none()
        }

        fn remove(&mut self, key: usize) -> bool {
            let removal_expectation = self.control_map.remove(&key);
            let removal_result = self.map.remove(&key);
            assert_eq!(removal_result, removal_expectation, "control map expectation unfulfilled");
            assert_eq!(self.map.len(), self.control_map.len(), "control map expectation unfulfilled");

            removal_result.is_some()
        }

        fn touch(&mut self, key: usize) -> Option<&str> {
            let value_expectation = self.control_map.get(&key);
            let value = self.map.get(&key);
            assert_eq!(value, value_expectation, "control map expectation unfulfilled");
            let value = value.copied(); // &&str -> &str
            assert_eq!(self.map.len(), self.control_map.len(), "control map expectation unfulfilled");

            value
        }

        fn assert_ordered_values(&mut self, keys: &[usize], error_message: &str) {
            assert_eq!(self.map.len(), keys.len(), "expected same map size");

            let mut values_iter = self.map.peek_iter();
            let mut expected_iter = keys.iter();
            let mut i = 0;
            loop {
                match (values_iter.next(), expected_iter.next()) {
                    (Some((value, _)), Some(expected)) => {
                        assert_eq!(
                            value,
                            expected,
                            "expected same map order: {} (index:{i}, keys:{})",
                            error_message,
                            keys.len(),
                        )
                    },
                    (None, None) => break,
                    pair => {
                        panic!("expected same map values: {} (index:{i}, keys:{}, {pair:?})", error_message, keys.len())
                    },
                }
                i += 1;
            }
        }
    }

    #[test]
    fn crud() {
        let mut map = LruMapFixture::new();

        // Map is empty
        map.assert_ordered_values(&[], "initially empty");

        assert!(map.insert(0, "0"));
        map.assert_ordered_values(&[0], "after inserting first element");
        assert!(!map.insert(0, "0"));
        map.assert_ordered_values(&[0], "after reinserting first element");

        assert_eq!(map.touch(0), Some("0"));
        map.assert_ordered_values(&[0], "after touching the only element");

        // Remove the only element
        assert!(map.remove(0));
        map.assert_ordered_values(&[], "after removing the only element");
        assert!(!map.remove(0));

        // Insert again
        assert!(map.insert(0, "0"));
        map.assert_ordered_values(&[0], "after reinserting first element");

        // Insert another
        assert!(map.insert(1, "1"));
        map.assert_ordered_values(&[1, 0], "after inserting second element");

        // Touch oldest element
        assert_eq!(map.touch(0), Some("0"));
        map.assert_ordered_values(&[0, 1], "after touching oldest element");

        // Add another
        assert!(map.insert(2, "2"));
        map.assert_ordered_values(&[2, 0, 1], "after inserting third element");

        // Touch oldest element
        assert_eq!(map.touch(1), Some("1"));
        map.assert_ordered_values(&[1, 2, 0], "after touching oldest element");

        // Remove elements
        assert!(map.remove(2));
        map.assert_ordered_values(&[1, 0], "after removing middle element");

        assert!(map.remove(1));
        map.assert_ordered_values(&[0], "after removing another element");

        assert!(map.remove(0));
        map.assert_ordered_values(&[], "after removing last element");
    }

    use crate::secrets::SecretManager;
    use orion_configuration::config::cluster::{
        Cluster as ClusterConfig, ClusterDiscoveryType, LbPolicy, OriginalDstConfig, http_protocol_options::Codec,
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
            internal_transport_socket: None,
            bind_device: None,
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
        let channel1 = cluster.get_http_connection(RoutingContext::Authority(authority.clone())).unwrap();
        let channel2 = cluster.get_http_connection(RoutingContext::Authority(authority)).unwrap();
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
        let _tcp_future = cluster.get_tcp_connection(RoutingContext::Authority(authority)).unwrap();

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
        let auth1_context = RoutingContext::Authority(auth1);
        let auth2 = Authority::from_str("localhost:5101").unwrap();
        let auth2_context = RoutingContext::Authority(auth2);

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
