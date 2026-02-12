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

mod dynamic;
mod original_dst;
mod r#static;

use enum_dispatch::enum_dispatch;
use http::uri::Authority;
use rustls::pki_types::ServerName;

use crate::clusters::clusters_manager::{RoutingContext, RoutingRequirement};
use orion_configuration::config::cluster::{
    Cluster as ClusterConfig, ClusterDiscoveryType, ClusterLoadAssignment as ClusterLoadAssignmentConfig, HealthCheck,
};
use tracing::{debug, warn};

use super::health::HealthStatus;
use crate::{
    clusters::load_assignment::{ClusterLoadAssignmentBuilder, PartialClusterLoadAssignment},
    secrets::TransportSecret,
    transport::{GrpcService, HttpChannel, HttpChannels, TcpChannelConnector, UpstreamTransportSocketConfigurator},
    Error, Result, SecretManager,
};

use dynamic::{DynamicCluster, DynamicClusterBuilder};
use original_dst::{OriginalDstCluster, OriginalDstClusterBuilder};
use orion_interner::StringInterner;
use r#static::{StaticCluster, StaticClusterBuilder};

impl TryFrom<(ClusterConfig, &SecretManager)> for PartialClusterType {
    type Error = Error;
    fn try_from(value: (ClusterConfig, &SecretManager)) -> std::result::Result<Self, Self::Error> {
        let (cluster, secrets) = value;
        let config = cluster.clone();
        let transport_socket_config = cluster.transport_socket;
        let bind_device_options = cluster.bind_device_options;
        let load_balancing_policy = cluster.load_balancing_policy;
        let protocol_options = cluster.http_protocol_options;

        let transport_socket = UpstreamTransportSocketConfigurator::try_from((transport_socket_config, secrets))?;

        let health_check = cluster.health_check;
        debug!("Cluster {} type {:?} ", cluster.name, cluster.discovery_settings);

        let server_name = if let Ok(Some(server_name)) = transport_socket
            .tls_configurator()
            .map(|tls_configurator| ServerName::try_from(tls_configurator.sni()))
            .transpose()
        {
            Some(server_name)
        } else {
            None
        };

        match cluster.discovery_settings {
            ClusterDiscoveryType::Static(cla) => {
                let cluster_name = cla.cluster_name.clone();
                let pcla = PartialClusterLoadAssignment::try_from(cla)
                    .map_err(|e| Error::from(format!("Unable to create cluster load assignment {cluster_name} {e}")))?;

                let cla = ClusterLoadAssignmentBuilder::builder()
                    .with_cla(pcla)
                    .with_cluster_name(cluster.name.to_static_str())
                    .with_bind_device_options(bind_device_options)
                    .with_lb_policy(load_balancing_policy)
                    .with_connection_timeout(cluster.connect_timeout)
                    .with_transport_socket(transport_socket.clone())
                    .with_server_name(server_name)
                    .with_protocol_options(Some(protocol_options))
                    .prepare();

                Ok(PartialClusterType::Static(StaticClusterBuilder {
                    name: cluster.name.to_static_str(),
                    load_assignment: cla,
                    transport_socket,
                    health_check,
                    config,
                }))
            },

            // at the moment there is no difference for us since both cluster types are using the same resolver
            ClusterDiscoveryType::StrictDns(cla) => {
                let cla = ClusterLoadAssignmentBuilder::builder()
                    .with_cla(PartialClusterLoadAssignment::try_from(cla)?)
                    .with_cluster_name(cluster.name.to_static_str())
                    .with_bind_device_options(bind_device_options)
                    .with_lb_policy(load_balancing_policy)
                    .with_connection_timeout(cluster.connect_timeout)
                    .with_transport_socket(transport_socket.clone())
                    .with_server_name(server_name)
                    .with_protocol_options(Some(protocol_options))
                    .prepare();

                Ok(PartialClusterType::Static(StaticClusterBuilder {
                    name: cluster.name.to_static_str(),
                    load_assignment: cla,
                    transport_socket,
                    health_check,
                    config,
                }))
            },

            // ClusterDiscoveryType::Eds(None, None) => Ok(PartialClusterType::Dynamic(DynamicClusterBuilder {
            //     name: cluster.name.to_static_str(),
            //     bind_device,
            //     transport_socket,
            //     health_check,
            //     load_balancing_policy,
            //     config,
            // })),
            ClusterDiscoveryType::Eds(cla, eds) => {
                warn!("Creating EDS cluster and skippint static endpoints {cla:?} {eds:?}");
                Ok(PartialClusterType::Dynamic(DynamicClusterBuilder {
                    name: cluster.name.to_static_str(),
                    bind_device_options,
                    transport_socket,
                    health_check,
                    load_balancing_policy,
                    config,
                }))
            },
            ClusterDiscoveryType::OriginalDst(_) => Ok(PartialClusterType::OnDemand(OriginalDstClusterBuilder {
                name: cluster.name.to_static_str(),
                bind_device_options,
                transport_socket,
                connect_timeout: cluster.connect_timeout,
                server_name,
                config,
            })),
        }
    }
}

#[enum_dispatch]
pub trait ClusterOps {
    fn get_name(&self) -> &'static str;
    fn into_health_check(self) -> Option<HealthCheck>;
    fn all_http_channels(&mut self) -> Vec<(Authority, HttpChannel)>;
    fn all_tcp_channels(&mut self) -> Vec<(Authority, TcpChannelConnector)>;
    fn all_grpc_channels(&mut self) -> Vec<Result<(Authority, GrpcService)>>;
    fn change_tls_context(&mut self, secret_id: &str, secret: TransportSecret) -> Result<()>;
    fn update_health(&mut self, endpoint: &http::uri::Authority, health: HealthStatus);
    fn get_http_connection(&mut self, context: RoutingContext) -> Result<HttpChannels>;
    fn get_tcp_connection(&mut self, context: RoutingContext) -> Result<TcpChannelConnector>;
    fn get_grpc_connection(&mut self, context: RoutingContext) -> Result<GrpcService>;
    fn get_routing_requirements(&self) -> RoutingRequirement;
}

#[derive(Debug, Clone)]
#[enum_dispatch(ClusterOps)]
pub enum ClusterType {
    Static(StaticCluster),
    Dynamic(DynamicCluster),
    OnDemand(OriginalDstCluster),
}

impl TryFrom<&ClusterType> for ClusterConfig {
    type Error = Error;
    fn try_from(cluster: &ClusterType) -> Result<Self> {
        match cluster {
            ClusterType::Static(static_cluster) => Ok(static_cluster.config.clone()),
            ClusterType::Dynamic(dynamic_cluster) => {
                let cla: ClusterLoadAssignmentConfig = dynamic_cluster.try_into()?;
                let mut config = dynamic_cluster.config.clone();
                config.discovery_settings = ClusterDiscoveryType::Eds(Some(cla), None);
                Ok(config)
            },
            ClusterType::OnDemand(original_dst_cluster) => Ok(original_dst_cluster.config.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum PartialClusterType {
    Static(StaticClusterBuilder),
    Dynamic(DynamicClusterBuilder),
    OnDemand(OriginalDstClusterBuilder),
}

impl PartialClusterType {
    pub fn build(self) -> Result<ClusterType> {
        match self {
            PartialClusterType::Static(cluster_builder) => cluster_builder.build(),
            PartialClusterType::Dynamic(cluster_builder) => Ok(cluster_builder.build()),
            PartialClusterType::OnDemand(cluster_builder) => Ok(cluster_builder.build()),
        }
    }

    pub fn get_name(&self) -> &'static str {
        match &self {
            PartialClusterType::Static(cluster) => cluster.name,
            PartialClusterType::Dynamic(cluster) => cluster.name,
            PartialClusterType::OnDemand(cluster) => cluster.name,
        }
    }

    pub fn into_health_check(self) -> Option<HealthCheck> {
        match self {
            PartialClusterType::Static(cluster) => cluster.health_check,
            PartialClusterType::Dynamic(cluster) => cluster.health_check,
            PartialClusterType::OnDemand(_cluster) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use decode::from_yaml;
    use orion_configuration::config::transport::BindDevice;
    use orion_data_plane_api::decode;
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::cluster::v3::Cluster as EnvoyCluster;
    use std::str::FromStr;

    use super::*;

    fn check_bind_device(c: &ClusterType, device_name: &str) {
        let expected_bind_device = Some(BindDevice::from_str(device_name).unwrap());

        let cla = match c {
            ClusterType::Static(s) => Some(&s.load_assignment),
            ClusterType::Dynamic(d) => {
                assert_eq!(d.bind_device_options.bind_device, expected_bind_device);
                d.load_assignment.as_ref()
            },
            ClusterType::OnDemand(_) => unreachable!("OnDemand cluster has no load assignment"),
        };

        if let Some(load_assignment) = cla {
            for lep in &load_assignment.endpoints {
                for ep in &lep.endpoints {
                    assert_eq!(ep.bind_device_options.bind_device, expected_bind_device);
                }
            }
        }
    }

    #[test]
    fn static_cluster_upstream_bind_device() {
        const CLUSTER: &str = r#"
name: cluster1
type: STATIC
upstream_bind_config:
  socket_options:
  - description: "bind to interface virt1"
    level: 1
    name: 25
    # utf8 string 'virt1' bytes encoded as base64
    buf_value: dmlydDE=
load_assignment:
  endpoints:
    - lb_endpoints:
        - endpoint:
            address:
              socket_address:
                address: 192.168.2.10
                port_value: 80
"#;

        let secrets_man = SecretManager::new();
        let envoy_cluster: EnvoyCluster = from_yaml(CLUSTER).unwrap();
        let cluster = ClusterConfig::try_from(envoy_cluster).unwrap();
        let c = PartialClusterType::try_from((cluster, &secrets_man)).unwrap();
        let c = c.build().unwrap();

        check_bind_device(&c, "virt1");
    }

    #[test]
    fn eds_cluster_upstream_bind_device() {
        const CLUSTER: &str = r#"
name: cluster1
type: EDS
upstream_bind_config:
  socket_options:
  - description: "bind to interface virt1"
    level: 1
    name: 25
    # utf8 string 'virt1' bytes encoded as base64
    buf_value: dmlydDE=
"#;

        let secrets_man = SecretManager::new();
        let envoy_cluster: EnvoyCluster = from_yaml(CLUSTER).unwrap();
        let cluster = ClusterConfig::try_from(envoy_cluster).unwrap();
        let c = PartialClusterType::try_from((cluster, &secrets_man)).unwrap();
        println!("{c:#?}");
        let c = c.build().unwrap();
        check_bind_device(&c, "virt1");
    }

    #[test]
    fn cluster_2_health_check_not_supported() {
        const CLUSTER: &str = r#"
name: cluster1
type: EDS
health_checks:
- timeout: 0.1s
  interval: 5s
  healthy_threshold: "3"
  unhealthy_threshold: "2"
  http_health_check:
    path: /health
- timeout: 0.1s
  interval: 5s
  healthy_threshold: "3"
  unhealthy_threshold: "2"
  http_health_check:
    path: /health
load_assignment:
  endpoints:
    - lb_endpoints:
        - endpoint:
            address:
              socket_address:
                address: 192.168.2.10
                port_value: 80
"#;

        let envoy_cluster: EnvoyCluster = from_yaml(CLUSTER).unwrap();
        assert_eq!(envoy_cluster.health_checks.len(), 2);
        let _ = ClusterConfig::try_from(envoy_cluster).unwrap_err();
    }
}
