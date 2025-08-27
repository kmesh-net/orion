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

use super::{ClusterOps, ClusterType};
use crate::{
    clusters::{
        clusters_manager::{RoutingContext, RoutingRequirement},
        load_assignment::{ClusterLoadAssignment, ClusterLoadAssignmentBuilder},
        GrpcService,
    },
    secrets::TransportSecret,
    transport::{HttpChannel, TcpChannelConnector, UpstreamTransportSocketConfigurator},
    Result,
};
use http::uri::Authority;
use orion_configuration::config::cluster::{HealthCheck, HealthStatus, LbPolicy};
use tracing::debug;

#[derive(Debug, Clone)]
pub struct StaticClusterBuilder {
    pub name: &'static str,
    pub load_assignment: ClusterLoadAssignmentBuilder,
    pub transport_socket: UpstreamTransportSocketConfigurator,
    pub health_check: Option<HealthCheck>,
    pub config: orion_configuration::config::cluster::Cluster,
}

impl StaticClusterBuilder {
    pub fn build(self) -> Result<ClusterType> {
        let StaticClusterBuilder { name, load_assignment, transport_socket, health_check, config } = self;
        let load_assignment = load_assignment.build()?;
        Ok(ClusterType::Static(StaticCluster { name, load_assignment, transport_socket, health_check, config }))
    }
}

#[derive(Debug, Clone)]
pub struct StaticCluster {
    pub name: &'static str,
    pub load_assignment: ClusterLoadAssignment,
    pub(super) transport_socket: UpstreamTransportSocketConfigurator,
    pub health_check: Option<HealthCheck>,
    pub config: orion_configuration::config::cluster::Cluster,
}

impl ClusterOps for StaticCluster {
    fn get_name(&self) -> &'static str {
        self.name
    }

    fn into_health_check(self) -> Option<HealthCheck> {
        self.health_check
    }

    fn all_http_channels(&self) -> Vec<(Authority, HttpChannel)> {
        self.load_assignment.all_http_channels()
    }

    fn all_tcp_channels(&self) -> Vec<(Authority, TcpChannelConnector)> {
        self.load_assignment.all_tcp_channels()
    }

    fn all_grpc_channels(&self) -> Vec<Result<(Authority, GrpcService)>> {
        self.load_assignment.try_all_grpc_channels()
    }

    fn change_tls_context(&mut self, secret_id: &str, secret: TransportSecret) -> Result<()> {
        self.transport_socket.update_secret(secret_id, secret)?;
        let mut load_assignment = self.load_assignment.clone();
        load_assignment.transport_socket = self.transport_socket.clone();
        let load_assignment = load_assignment.rebuild()?;
        self.load_assignment = load_assignment;
        Ok(())
    }

    fn update_health(&mut self, endpoint: &http::uri::Authority, health: HealthStatus) {
        self.load_assignment.update_endpoint_health(endpoint, health);
    }

    fn get_http_connection(&mut self, context: RoutingContext) -> Result<HttpChannel> {
        debug!("{} : Getting connection", self.name);
        match context {
            RoutingContext::Hash(hash_state) => self.load_assignment.get_http_channel(Some(hash_state)),
            _ => self.load_assignment.get_http_channel(None),
        }
    }

    fn get_tcp_connection(&mut self, _context: RoutingContext) -> Result<TcpChannelConnector> {
        self.load_assignment.get_tcp_channel()
    }

    fn get_grpc_connection(&mut self, _context: RoutingContext) -> Result<GrpcService> {
        self.load_assignment.get_grpc_channel()
    }

    fn get_routing_requirements(&self) -> RoutingRequirement {
        match self.config.load_balancing_policy {
            LbPolicy::RingHash | LbPolicy::Maglev => RoutingRequirement::Hash,
            _ => RoutingRequirement::None,
        }
    }
}
