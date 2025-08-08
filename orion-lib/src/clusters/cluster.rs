// SPDX-FileCopyrightText: © 2025 Huawei Cloud Computing Technologies Co., Ltd
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 Huawei Cloud Computing Technologies Co., Ltd
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

use compact_str::{CompactString, ToCompactString};
use enum_dispatch::enum_dispatch;
use futures::future::BoxFuture;
use http::uri::Authority;

use orion_configuration::config::cluster::ClusterDiscoveryType;
use orion_configuration::config::cluster::{Cluster as ClusterConfig, HealthCheck, LbPolicy};
use rustls::ClientConfig;
use tokio::net::TcpStream;
use tracing::debug;
use webpki::types::ServerName;

use super::balancers::hash_policy::HashState;
use super::{health::HealthStatus, load_assignment::ClusterLoadAssignment};
use crate::clusters::load_assignment::PartialClusterLoadAssignment;
use crate::transport::{GrpcService, HttpChannel};
use crate::{
    clusters::load_assignment::ClusterLoadAssignmentBuilder,
    secrets::{TlsConfigurator, TransportSecret, WantsToBuildClient},
    transport::{bind_device::BindDevice, connector::ConnectError, TcpChannel},
    Error, Result, SecretManager,
};

pub type TcpService = BoxFuture<'static, std::result::Result<TcpStream, ConnectError>>;

#[derive(Debug, Clone)]
pub struct StaticCluster {
    pub name: CompactString,
    pub load_assignment: ClusterLoadAssignment,
    pub tls_configurator: Option<TlsConfigurator<ClientConfig, WantsToBuildClient>>,
    pub health_check: Option<HealthCheck>,
}

#[derive(Debug, Clone)]
pub struct DynamicCluster {
    pub name: CompactString,
    pub bind_device: Option<BindDevice>,
    load_assignment: Option<ClusterLoadAssignment>,
    pub tls_configurator: Option<TlsConfigurator<ClientConfig, WantsToBuildClient>>,
    pub health_check: Option<HealthCheck>,
    pub load_balancing_policy: LbPolicy,
}

#[derive(Debug, Clone)]
pub struct StaticClusterBuilder {
    pub name: CompactString,
    pub load_assignment: ClusterLoadAssignmentBuilder,
    pub tls_configurator: Option<TlsConfigurator<ClientConfig, WantsToBuildClient>>,
    pub health_check: Option<HealthCheck>,
}

#[derive(Debug, Clone)]
// TODO: Implement dynamic cluster configuration builder
// Used for clusters that are configured at runtime via xDS or other control plane mechanisms
pub struct DynamicClusterBuilder {
    pub name: CompactString,
    pub bind_device: Option<BindDevice>,
    pub tls_configurator: Option<TlsConfigurator<ClientConfig, WantsToBuildClient>>,
    pub health_check: Option<HealthCheck>,
    pub load_balancing_policy: LbPolicy,
}

impl StaticClusterBuilder {
    fn build(self) -> Result<ClusterType> {
        let StaticClusterBuilder { name, load_assignment, tls_configurator, health_check } = self;
        let load_assignment = load_assignment.build()?;
        Ok(ClusterType::Static(StaticCluster { name, load_assignment, tls_configurator, health_check }))
    }
}

impl DynamicClusterBuilder {
    fn build(self) -> ClusterType {
        let DynamicClusterBuilder { name, tls_configurator, health_check, load_balancing_policy, bind_device } = self;
        ClusterType::Dynamic(DynamicCluster {
            name,
            load_assignment: None,
            tls_configurator,
            health_check,
            load_balancing_policy,
            bind_device,
        })
    }
}

impl TryFrom<(ClusterConfig, &SecretManager)> for PartialClusterType {
    type Error = Error;
    fn try_from(value: (ClusterConfig, &SecretManager)) -> std::result::Result<Self, Self::Error> {
        let (cluster, secrets) = value;
        let upstream_tls_context = cluster.tls_config;
        let bind_device = cluster.bind_device;
        let load_balancing_policy = cluster.load_balancing_policy;
        let protocol_options = cluster.http_protocol_options;

        let cluster_tls_configurator = if let Some(upstream_tls_context) = upstream_tls_context {
            Some(TlsConfigurator::<ClientConfig, WantsToBuildClient>::try_from((upstream_tls_context, secrets))?)
        } else {
            None
        };

        let health_check = cluster.health_check;
        debug!("Cluster {} type {:?} ", cluster.name, cluster.discovery_settings);
        match cluster.discovery_settings {
            ClusterDiscoveryType::Static(cla) => {
                let server_name = cluster_tls_configurator
                    .as_ref()
                    .map(|tls_configurator| ServerName::try_from(tls_configurator.sni()))
                    .transpose()?;

                let cla = ClusterLoadAssignmentBuilder::builder()
                    .with_cla(PartialClusterLoadAssignment::try_from(cla)?)
                    .with_cluster_name(cluster.name.clone())
                    .with_bind_device(bind_device)
                    .with_lb_policy(load_balancing_policy)
                    .with_connection_timeout(cluster.connect_timeout)
                    .with_tls_configurator(cluster_tls_configurator.clone())
                    .with_server_name(server_name)
                    .with_protocol_options(Some(protocol_options))
                    .prepare();

                Ok(PartialClusterType::Static(StaticClusterBuilder {
                    name: cluster.name.to_compact_string(),
                    load_assignment: cla,
                    tls_configurator: cluster_tls_configurator,
                    health_check,
                }))
            },
            ClusterDiscoveryType::Eds => Ok(PartialClusterType::Dynamic(DynamicClusterBuilder {
                name: cluster.name.to_compact_string(),
                bind_device,
                tls_configurator: cluster_tls_configurator,
                health_check,
                load_balancing_policy,
            })),
        }
    }
}

#[enum_dispatch]
pub trait ClusterOps {
    fn get_name(&self) -> &CompactString;
    fn into_health_check(self) -> Option<HealthCheck>;
    fn all_http_channels(&self) -> Vec<(Authority, HttpChannel)>;
    fn all_tcp_channels(&self) -> Vec<(Authority, TcpChannel)>;
    fn all_grpc_channels(&self) -> Vec<Result<(Authority, GrpcService)>>;
    fn change_tls_context(&mut self, secret_id: &str, secret: TransportSecret) -> Result<()>;
    fn update_health(&mut self, endpoint: &http::uri::Authority, health: HealthStatus);
    fn get_http_connection(&mut self, lb_hash: HashState) -> Result<HttpChannel>;
    fn get_tcp_connection(&mut self) -> Result<BoxFuture<'static, std::result::Result<TcpStream, ConnectError>>>;
    fn get_grpc_connection(&mut self) -> Result<GrpcService>;
}

#[derive(Debug, Clone)]
#[enum_dispatch(ClusterOps)]
pub enum ClusterType {
    Static(StaticCluster),
    Dynamic(DynamicCluster),
}

#[derive(Debug, Clone)]
pub enum PartialClusterType {
    Static(StaticClusterBuilder),
    Dynamic(DynamicClusterBuilder),
}

impl PartialClusterType {
    pub fn build(self) -> Result<ClusterType> {
        match self {
            Self::Static(cluster_builder) => cluster_builder.build(),
            Self::Dynamic(cluster_builder) => Ok(cluster_builder.build()),
        }
    }

    pub fn get_name(&self) -> &CompactString {
        match &self {
            PartialClusterType::Static(cluster) => &cluster.name,
            PartialClusterType::Dynamic(cluster) => &cluster.name,
        }
    }

    pub fn into_health_check(self) -> Option<HealthCheck> {
        match self {
            PartialClusterType::Static(cluster) => cluster.health_check,
            PartialClusterType::Dynamic(cluster) => cluster.health_check,
        }
    }
}

impl DynamicCluster {
    pub fn change_load_assignment(&mut self, cluster_load_assignment: Option<ClusterLoadAssignment>) {
        self.load_assignment = cluster_load_assignment;
    }
}

impl ClusterOps for DynamicCluster {
    fn get_name(&self) -> &CompactString {
        &self.name
    }

    fn into_health_check(self) -> Option<HealthCheck> {
        self.health_check
    }

    fn all_http_channels(&self) -> Vec<(Authority, HttpChannel)> {
        self.load_assignment.as_ref().map_or(Vec::new(), ClusterLoadAssignment::all_http_channels)
    }

    fn all_tcp_channels(&self) -> Vec<(Authority, TcpChannel)> {
        self.load_assignment.as_ref().map_or(Vec::new(), ClusterLoadAssignment::all_tcp_channels)
    }

    fn all_grpc_channels(&self) -> Vec<Result<(Authority, GrpcService)>> {
        self.load_assignment.as_ref().map_or(Vec::new(), ClusterLoadAssignment::try_all_grpc_channels)
    }

    fn change_tls_context(&mut self, secret_id: &str, secret: TransportSecret) -> Result<()> {
        if let Some(tls_configurator) = self.tls_configurator.clone() {
            let tls_configurator =
                TlsConfigurator::<ClientConfig, WantsToBuildClient>::update(tls_configurator, secret_id, secret)?;
            if let Some(mut load_assignment) = self.load_assignment.take() {
                load_assignment.tls_configurator = Some(tls_configurator.clone());
                let load_assignment = load_assignment.rebuild()?;
                self.load_assignment = Some(load_assignment);
            };
            self.tls_configurator = Some(tls_configurator);
        }
        Ok(())
    }

    fn update_health(&mut self, endpoint: &http::uri::Authority, health: HealthStatus) {
        if let Some(load_assignment) = self.load_assignment.as_mut() {
            load_assignment.update_endpoint_health(endpoint, health);
        }
    }

    fn get_http_connection(&mut self, lb_hash: HashState) -> Result<HttpChannel> {
        if let Some(cla) = self.load_assignment.as_mut() {
            cla.get_http_channel(lb_hash)
        } else {
            Err(format!("{} No channels available", self.name).into())
        }
    }

    fn get_tcp_connection(&mut self) -> Result<BoxFuture<'static, std::result::Result<TcpStream, ConnectError>>> {
        if let Some(cla) = self.load_assignment.as_mut() {
            cla.get_tcp_channel()
        } else {
            Err(format!("{} No channels available", self.name).into())
        }
    }

    fn get_grpc_connection(&mut self) -> Result<GrpcService> {
        if let Some(cla) = self.load_assignment.as_mut() {
            cla.get_grpc_channel()
        } else {
            Err(format!("{} No channels available", self.name).into())
        }
    }
}

impl ClusterOps for StaticCluster {
    fn get_name(&self) -> &CompactString {
        &self.name
    }

    fn into_health_check(self) -> Option<HealthCheck> {
        self.health_check
    }

    fn all_http_channels(&self) -> Vec<(Authority, HttpChannel)> {
        self.load_assignment.all_http_channels()
    }

    fn all_tcp_channels(&self) -> Vec<(Authority, TcpChannel)> {
        self.load_assignment.all_tcp_channels()
    }

    fn all_grpc_channels(&self) -> Vec<Result<(Authority, GrpcService)>> {
        self.load_assignment.try_all_grpc_channels()
    }

    fn change_tls_context(&mut self, secret_id: &str, secret: TransportSecret) -> Result<()> {
        if let Some(tls_configurator) = self.tls_configurator.clone() {
            let tls_configurator =
                TlsConfigurator::<ClientConfig, WantsToBuildClient>::update(tls_configurator, secret_id, secret)?;
            let mut load_assignment = self.load_assignment.clone();
            load_assignment.tls_configurator = Some(tls_configurator.clone());
            let load_assignment = load_assignment.rebuild()?;
            self.load_assignment = load_assignment;
            self.tls_configurator = Some(tls_configurator);
        }
        Ok(())
    }

    fn update_health(&mut self, endpoint: &http::uri::Authority, health: HealthStatus) {
        self.load_assignment.update_endpoint_health(endpoint, health);
    }

    fn get_http_connection(&mut self, lb_hash: HashState) -> Result<HttpChannel> {
        debug!("{} : Getting connection", self.name);
        self.load_assignment.get_http_channel(lb_hash)
    }

    fn get_tcp_connection(&mut self) -> Result<BoxFuture<'static, std::result::Result<TcpStream, ConnectError>>> {
        self.load_assignment.get_tcp_channel()
    }

    fn get_grpc_connection(&mut self) -> Result<GrpcService> {
        self.load_assignment.get_grpc_channel()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use orion_data_plane_api::decode::from_yaml;
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::cluster::v3::Cluster as EnvoyCluster;

    use super::*;

    fn check_bind_device(c: &ClusterType, device_name: &str) {
        let expected_bind_device = Some(BindDevice::from_str(device_name).unwrap());

        let cla = match c {
            ClusterType::Static(s) => Some(&s.load_assignment),
            ClusterType::Dynamic(d) => {
                assert_eq!(&d.bind_device, &expected_bind_device);
                d.load_assignment.as_ref()
            },
        };

        if let Some(load_assignment) = cla {
            for lep in &load_assignment.endpoints {
                for ep in &lep.endpoints {
                    assert_eq!(ep.bind_device, expected_bind_device);
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
