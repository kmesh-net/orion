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

use http::uri::Authority;

use orion_configuration::config::{
    cluster::{
        ClusterLoadAssignment as ClusterLoadAssignmentConfig, HealthCheck, HealthStatus,
        LbEndpoint as LbEndpointConfig, LbPolicy, LocalityLbEndpoints as LocalityLbEndpointsConfig,
    },
    transport::BindDeviceOptions,
};

use crate::{
    clusters::{
        clusters_manager::{RoutingContext, RoutingRequirement},
        load_assignment::ClusterLoadAssignment,
        GrpcService,
    },
    secrets::TransportSecret,
    transport::{HttpChannel, TcpChannelConnector, UpstreamTransportSocketConfigurator},
    Result,
};

use super::{ClusterOps, ClusterType};

#[derive(Debug, Clone)]
pub struct DynamicClusterBuilder {
    pub name: &'static str,
    pub bind_device_options: BindDeviceOptions,
    pub transport_socket: UpstreamTransportSocketConfigurator,
    pub health_check: Option<HealthCheck>,
    pub load_balancing_policy: LbPolicy,
    pub config: orion_configuration::config::cluster::Cluster,
}

impl DynamicClusterBuilder {
    pub fn build(self) -> ClusterType {
        let DynamicClusterBuilder {
            name,
            transport_socket,
            health_check,
            load_balancing_policy,
            bind_device_options,
            config,
        } = self;
        ClusterType::Dynamic(DynamicCluster {
            name,
            load_assignment: None,
            transport_socket,
            health_check,
            load_balancing_policy,
            bind_device_options,
            config,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DynamicCluster {
    pub name: &'static str,
    pub bind_device_options: BindDeviceOptions,
    pub(super) load_assignment: Option<ClusterLoadAssignment>,
    pub transport_socket: UpstreamTransportSocketConfigurator,
    pub health_check: Option<HealthCheck>,
    pub load_balancing_policy: LbPolicy,
    pub config: orion_configuration::config::cluster::Cluster,
}

impl DynamicCluster {
    pub fn change_load_assignment(&mut self, cluster_load_assignment: Option<ClusterLoadAssignment>) {
        self.load_assignment = cluster_load_assignment;
    }
}

impl ClusterOps for DynamicCluster {
    fn get_name(&self) -> &'static str {
        self.name
    }

    fn into_health_check(self) -> Option<HealthCheck> {
        self.health_check
    }

    fn all_http_channels(&mut self) -> Vec<(Authority, HttpChannel)> {
        self.load_assignment.as_ref().map_or(Vec::new(), ClusterLoadAssignment::all_http_channels)
    }

    fn all_tcp_channels(&mut self) -> Vec<(Authority, TcpChannelConnector)> {
        self.load_assignment.as_ref().map_or(Vec::new(), ClusterLoadAssignment::all_tcp_channels)
    }

    fn all_grpc_channels(&mut self) -> Vec<Result<(Authority, GrpcService)>> {
        self.load_assignment.as_ref().map_or(Vec::new(), ClusterLoadAssignment::try_all_grpc_channels)
    }

    fn change_tls_context(&mut self, secret_id: &str, secret: TransportSecret) -> Result<()> {
        self.transport_socket.update_secret(secret_id, secret)?;
        if let Some(mut load_assignment) = self.load_assignment.take() {
            load_assignment.transport_socket = self.transport_socket.clone();
            let load_assignment = load_assignment.rebuild()?;
            self.load_assignment = Some(load_assignment);
        }
        Ok(())
    }

    fn update_health(&mut self, endpoint: &http::uri::Authority, health: HealthStatus) {
        if let Some(load_assignment) = self.load_assignment.as_mut() {
            load_assignment.update_endpoint_health(endpoint, health);
        }
    }

    fn get_http_connection(&mut self, context: RoutingContext) -> Result<HttpChannel> {
        if let Some(cla) = self.load_assignment.as_mut() {
            match context {
                RoutingContext::Hash(hash_state) => cla.get_http_channel(Some(hash_state)),
                _ => cla.get_http_channel(None),
            }
        } else {
            Err(format!("{} No channels available", self.name).into())
        }
    }

    fn get_tcp_connection(&mut self, _context: RoutingContext) -> Result<TcpChannelConnector> {
        if let Some(cla) = self.load_assignment.as_mut() {
            cla.get_tcp_channel()
        } else {
            Err(format!("{} No channels available", self.name).into())
        }
    }

    fn get_grpc_connection(&mut self, _context: RoutingContext) -> Result<GrpcService> {
        if let Some(cla) = self.load_assignment.as_mut() {
            cla.get_grpc_channel()
        } else {
            Err(format!("{} No channels available", self.name).into())
        }
    }

    fn get_routing_requirements(&self) -> RoutingRequirement {
        match self.load_balancing_policy {
            LbPolicy::RingHash | LbPolicy::Maglev => RoutingRequirement::Hash,
            _ => RoutingRequirement::None,
        }
    }
}

impl TryFrom<&DynamicCluster> for ClusterLoadAssignmentConfig {
    type Error = crate::Error;
    fn try_from(cluster: &DynamicCluster) -> crate::Result<Self> {
        let endpoints = cluster
            .load_assignment
            .as_ref()
            .ok_or_else(|| "No load assignment found".to_owned())?
            .endpoints
            .iter()
            .map(|lep| {
                let lb_endpoints = lep
                    .endpoints
                    .iter()
                    .map(|ep| {
                        let load_balancing_weight = std::num::NonZeroU32::new(ep.weight)
                            .ok_or_else(|| format!("Invalid load balancing weight: {}", ep.weight))?;
                        Ok(LbEndpointConfig {
                            address: ep.address.to_address(),
                            health_status: ep.health_status,
                            load_balancing_weight,
                        })
                    })
                    .collect::<crate::Result<Vec<_>>>()?;
                Ok(LocalityLbEndpointsConfig { priority: lep.priority, lb_endpoints })
            })
            .collect::<crate::Result<Vec<_>>>()?;
        Ok(ClusterLoadAssignmentConfig { cluster_name: cluster.name.to_owned(), endpoints })
    }
}
