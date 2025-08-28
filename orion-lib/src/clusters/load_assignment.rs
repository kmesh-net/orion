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

use std::{sync::Arc, time::Duration};

use http::uri::Authority;
use orion_configuration::config::cluster::{
    ClusterLoadAssignment as ClusterLoadAssignmentConfig, HealthStatus, HttpProtocolOptions,
    LbEndpoint as LbEndpointConfig, LbPolicy, LocalityLbEndpoints as LocalityLbEndpointsConfig,
};
use tracing::debug;
use typed_builder::TypedBuilder;
use webpki::types::ServerName;

use super::{
    balancers::{
        Balancer, DefaultBalancer, EndpointWithAuthority, EndpointWithLoad, WeightedEndpoint, hash_policy::HashState,
        least::WeightedLeastRequestBalancer, maglev::MaglevBalancer, random::RandomBalancer, ring::RingHashBalancer,
        wrr::WeightedRoundRobinBalancer,
    },
    // cluster::HyperService,
    health::{EndpointHealth, ValueUpdated},
};
use crate::{
    Result,
    transport::{
        GrpcService, HttpChannel, HttpChannelBuilder, TcpChannelConnector, UpstreamTransportSocketConfigurator,
        bind_device::BindDevice,
    },
};

#[derive(Debug, Clone)]
pub struct LbEndpoint {
    pub name: &'static str,
    pub authority: http::uri::Authority,
    pub bind_device: Option<BindDevice>,
    pub weight: u32,
    pub health_status: HealthStatus,
    http_channel: HttpChannel,
    tcp_channel: TcpChannelConnector,
}

impl PartialEq for LbEndpoint {
    fn eq(&self, other: &Self) -> bool {
        self.authority == other.authority
    }
}

impl WeightedEndpoint for LbEndpoint {
    fn weight(&self) -> u32 {
        self.weight
    }
}

impl EndpointWithAuthority for LbEndpoint {
    fn authority(&self) -> &Authority {
        &self.authority
    }
}

impl Eq for LbEndpoint {}

impl PartialOrd for LbEndpoint {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for LbEndpoint {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.authority.as_str().cmp(other.authority.as_str())
    }
}

impl EndpointHealth for LbEndpoint {
    fn health(&self) -> HealthStatus {
        self.health_status
    }

    fn update_health(&mut self, health: HealthStatus) -> ValueUpdated {
        self.health_status.update_health(health)
    }
}

impl LbEndpoint {
    pub fn grpc_service(&self) -> Result<GrpcService> {
        GrpcService::try_new(self.http_channel.clone(), self.authority.clone())
    }
}

#[derive(Debug, Clone)]
pub struct PartialLbEndpoint {
    pub authority: http::uri::Authority,
    pub bind_device: Option<BindDevice>,
    pub weight: u32,
    pub health_status: HealthStatus,
}

impl PartialLbEndpoint {
    fn new(value: &LbEndpoint) -> Self {
        PartialLbEndpoint {
            authority: value.authority.clone(),
            bind_device: value.bind_device.clone(),
            weight: value.weight,
            health_status: value.health_status,
        }
    }
}

impl EndpointWithLoad for LbEndpoint {
    fn http_load(&self) -> u32 {
        self.http_channel.load()
    }
}

#[derive(Debug, Clone, TypedBuilder)]
#[builder(build_method(vis="", name=prepare), field_defaults(setter(prefix = "with_")))]
struct LbEndpointBuilder {
    cluster_name: &'static str,
    endpoint: PartialLbEndpoint,
    http_protocol_options: HttpProtocolOptions,
    transport_socket: UpstreamTransportSocketConfigurator,
    #[builder(default)]
    server_name: Option<ServerName<'static>>,
    connect_timeout: Option<Duration>,
}

impl LbEndpointBuilder {
    #[must_use]
    fn replace_bind_device(mut self, bind_device: Option<BindDevice>) -> Self {
        self.endpoint.bind_device = bind_device;
        self
    }

    pub fn build(self) -> Result<Arc<LbEndpoint>> {
        let cluster_name = self.cluster_name;
        let PartialLbEndpoint { authority, bind_device, weight, health_status } = self.endpoint;

        let builder = HttpChannelBuilder::new(bind_device.clone())
            .with_authority(authority.clone())
            .with_timeout(self.connect_timeout)
            .with_cluster_name(cluster_name);

        let maybe_tls_conf = self.transport_socket.tls_configurator();
        let builder = if let Some(server_name) = self.server_name {
            builder.with_tls(maybe_tls_conf.cloned()).with_server_name(server_name)
        } else {
            builder.with_tls(maybe_tls_conf.cloned())
        };
        let http_channel = builder.with_http_protocol_options(self.http_protocol_options).build()?;
        let tcp_channel = TcpChannelConnector::new(
            &authority,
            cluster_name,
            bind_device.clone(),
            self.connect_timeout,
            self.transport_socket.clone(),
        );

        Ok(Arc::new(LbEndpoint {
            name: cluster_name,
            authority,
            bind_device,
            weight,
            health_status,
            http_channel,
            tcp_channel,
        }))
    }
}

impl TryFrom<LbEndpointConfig> for PartialLbEndpoint {
    type Error = crate::Error;

    fn try_from(lb_endpoint: LbEndpointConfig) -> Result<Self> {
        let health_status = lb_endpoint.health_status;
        let address = lb_endpoint.address;
        let authority = http::uri::Authority::try_from(format!("{address}"))?;
        let weight = lb_endpoint.load_balancing_weight.into();
        Ok(PartialLbEndpoint { authority, bind_device: None, weight, health_status })
    }
}

#[derive(Debug, Clone, Default)]
pub struct LocalityLbEndpoints {
    pub name: &'static str,
    pub endpoints: Vec<Arc<LbEndpoint>>,
    pub priority: u32,
    pub healthy_endpoints: u32,
    pub total_endpoints: u32,
    pub transport_socket: UpstreamTransportSocketConfigurator,
    pub http_protocol_options: HttpProtocolOptions,
    pub connection_timeout: Option<Duration>,
}
impl LocalityLbEndpoints {
    fn rebuild(self) -> Result<Self> {
        let endpoints = self
            .endpoints
            .into_iter()
            .map(|e| {
                LbEndpointBuilder::builder()
                    .with_cluster_name(self.name)
                    .with_http_protocol_options(self.http_protocol_options.clone())
                    .with_connect_timeout(self.connection_timeout)
                    .with_transport_socket(self.transport_socket.clone())
                    .with_endpoint(PartialLbEndpoint::new(&e))
                    .prepare()
                    .build()
            })
            .collect::<Result<_>>()?;

        Ok(Self { endpoints, ..self })
    }
}

#[derive(Debug, Clone, Default)]
pub struct PartialLocalityLbEndpoints {
    endpoints: Vec<PartialLbEndpoint>,
    pub priority: u32,
}
#[derive(Debug, Clone, Default, TypedBuilder)]
#[builder(build_method(vis="", name=prepare), field_defaults(setter(prefix = "with_")))]
pub struct LocalityLbEndpointsBuilder {
    cluster_name: &'static str,
    bind_device: Option<BindDevice>,
    endpoints: PartialLocalityLbEndpoints,
    http_protocol_options: HttpProtocolOptions,
    transport_socket: UpstreamTransportSocketConfigurator,
    server_name: Option<ServerName<'static>>,
    connection_timeout: Option<Duration>,
}

impl LocalityLbEndpointsBuilder {
    pub fn build(self) -> Result<LocalityLbEndpoints> {
        let cluster_name = self.cluster_name;
        let PartialLocalityLbEndpoints { endpoints, priority } = self.endpoints;

        let endpoints: Vec<Arc<LbEndpoint>> = endpoints
            .into_iter()
            .map(|e| {
                let server_name = self.transport_socket.tls_configurator().and(self.server_name.clone());

                LbEndpointBuilder::builder()
                    .with_endpoint(e)
                    .with_cluster_name(cluster_name)
                    .with_connect_timeout(self.connection_timeout)
                    .with_transport_socket(self.transport_socket.clone())
                    .with_server_name(server_name)
                    .with_http_protocol_options(self.http_protocol_options.clone())
                    .prepare()
                    .replace_bind_device(self.bind_device.clone())
                    .build()
            })
            .collect::<Result<_>>()?;

        let total_endpoints_usize = endpoints.len();
        let healthy_endpoints_usize = endpoints.iter().filter(|e| e.health_status.is_healthy()).count();

        let (Ok(total_endpoints), Ok(healthy_endpoints)) =
            (u32::try_from(total_endpoints_usize), u32::try_from(healthy_endpoints_usize))
        else {
            return Err("Too many endpoints".into());
        };

        // we divide by 100 because we multiply by 100 later to calculate a percentage
        if healthy_endpoints > u32::MAX / 100 {
            return Err("Too many endpoints".into());
        }

        Ok(LocalityLbEndpoints {
            name: cluster_name,
            endpoints,
            priority,
            healthy_endpoints,
            total_endpoints,
            transport_socket: self.transport_socket,
            http_protocol_options: self.http_protocol_options,
            connection_timeout: self.connection_timeout,
        })
    }
}

impl TryFrom<LocalityLbEndpointsConfig> for PartialLocalityLbEndpoints {
    type Error = crate::Error;

    fn try_from(value: LocalityLbEndpointsConfig) -> Result<Self> {
        let endpoints = value.lb_endpoints.into_iter().map(PartialLbEndpoint::try_from).collect::<Result<_>>()?;
        let priority = value.priority;
        Ok(PartialLocalityLbEndpoints { priority, endpoints })
    }
}

#[derive(Debug, Clone)]
pub enum BalancerType {
    RoundRobin(DefaultBalancer<WeightedRoundRobinBalancer<LbEndpoint>, LbEndpoint>),
    Random(DefaultBalancer<RandomBalancer<LbEndpoint>, LbEndpoint>),
    LeastRequests(DefaultBalancer<WeightedLeastRequestBalancer<LbEndpoint>, LbEndpoint>),
    RingHash(DefaultBalancer<RingHashBalancer<LbEndpoint>, LbEndpoint>),
    Maglev(DefaultBalancer<MaglevBalancer<LbEndpoint>, LbEndpoint>),
}

impl BalancerType {
    pub fn update_health(&mut self, endpoint: &LbEndpoint, health: HealthStatus) -> Result<ValueUpdated> {
        match self {
            BalancerType::RoundRobin(balancer) => balancer.update_health(endpoint, health),
            BalancerType::Random(balancer) => balancer.update_health(endpoint, health),
            BalancerType::LeastRequests(balancer) => balancer.update_health(endpoint, health),
            BalancerType::RingHash(balancer) => balancer.update_health(endpoint, health),
            BalancerType::Maglev(balancer) => balancer.update_health(endpoint, health),
        }
    }
    fn next_item(&mut self, maybe_hash: Option<HashState>) -> Option<Arc<LbEndpoint>> {
        match self {
            BalancerType::RoundRobin(balancer) => balancer.next_item(None),
            BalancerType::Random(balancer) => balancer.next_item(None),
            BalancerType::LeastRequests(balancer) => balancer.next_item(None),
            BalancerType::RingHash(balancer) => balancer.next_item(maybe_hash.and_then(HashState::compute)),
            BalancerType::Maglev(balancer) => balancer.next_item(maybe_hash.and_then(HashState::compute)),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ClusterLoadAssignment {
    cluster_name: &'static str,
    pub transport_socket: UpstreamTransportSocketConfigurator,
    protocol_options: HttpProtocolOptions,
    balancer: BalancerType,
    pub endpoints: Vec<LocalityLbEndpoints>,
}

#[derive(Debug, Clone)]
pub struct PartialClusterLoadAssignment {
    endpoints: Vec<PartialLocalityLbEndpoints>,
}

impl ClusterLoadAssignment {
    pub fn get_http_channel(&mut self, hash: Option<HashState>) -> Result<HttpChannel> {
        let endpoint = self.balancer.next_item(hash).ok_or("No active endpoint")?;
        Ok(endpoint.http_channel.clone())
    }

    pub fn get_tcp_channel(&mut self) -> Result<TcpChannelConnector> {
        let endpoint = self.balancer.next_item(None).ok_or("No active endpoint")?;
        Ok(endpoint.tcp_channel.clone())
    }

    pub fn get_grpc_channel(&mut self) -> Result<GrpcService> {
        let endpoint = self.balancer.next_item(None).ok_or("No active endpoint")?;
        endpoint.grpc_service()
    }

    pub fn all_http_channels(&self) -> Vec<(Authority, HttpChannel)> {
        self.all_endpoints_iter().map(|endpoint| (endpoint.authority.clone(), endpoint.http_channel.clone())).collect()
    }

    pub fn all_tcp_channels(&self) -> Vec<(Authority, TcpChannelConnector)> {
        self.all_endpoints_iter().map(|endpoint| (endpoint.authority.clone(), endpoint.tcp_channel.clone())).collect()
    }

    pub fn try_all_grpc_channels(&self) -> Vec<Result<(Authority, GrpcService)>> {
        self.all_endpoints_iter()
            .map(|endpoint| endpoint.grpc_service().map(|channel| (endpoint.authority.clone(), channel)))
            .collect()
    }

    pub fn update_endpoint_health(&mut self, authority: &http::uri::Authority, health: HealthStatus) {
        for locality in &self.endpoints {
            locality.endpoints.iter().filter(|endpoint| &endpoint.authority == authority).for_each(|endpoint| {
                if let Err(err) = self.balancer.update_health(endpoint, health) {
                    debug!("Could not update endpoint health: {}", err);
                }
            });
        }
    }

    pub fn rebuild(self) -> Result<Self> {
        let endpoints = self
            .endpoints
            .into_iter()
            .map(|mut e| {
                e.transport_socket = self.transport_socket.clone();
                e.rebuild()
            })
            .collect::<Result<Vec<_>>>()?;
        let balancer = self.balancer;
        Ok(Self { endpoints, balancer, ..self })
    }

    fn all_endpoints_iter(&self) -> impl Iterator<Item = &LbEndpoint> {
        self.endpoints.iter().flat_map(|locality_endpoints| &locality_endpoints.endpoints).map(Arc::as_ref)
    }
}

#[derive(Debug, Clone, TypedBuilder)]
#[builder(build_method(vis="pub(crate)", name=prepare), field_defaults(setter(prefix = "with_")))]
pub struct ClusterLoadAssignmentBuilder {
    cluster_name: &'static str,
    cla: PartialClusterLoadAssignment,
    bind_device: Option<BindDevice>,
    #[builder(default)]
    protocol_options: Option<HttpProtocolOptions>,
    lb_policy: LbPolicy,
    transport_socket: UpstreamTransportSocketConfigurator,
    #[builder(default)]
    server_name: Option<ServerName<'static>>,
    #[builder(default)]
    connection_timeout: Option<Duration>,
}

impl ClusterLoadAssignmentBuilder {
    pub fn build(self) -> Result<ClusterLoadAssignment> {
        let cluster_name = self.cluster_name;
        let protocol_options = self.protocol_options.unwrap_or_default();

        let PartialClusterLoadAssignment { endpoints } = self.cla;

        let endpoints = endpoints
            .into_iter()
            .map(|e| {
                let server_name = self.transport_socket.tls_configurator().and(self.server_name.clone());

                LocalityLbEndpointsBuilder::builder()
                    .with_cluster_name(cluster_name)
                    .with_endpoints(e)
                    .with_bind_device(self.bind_device.clone())
                    .with_connection_timeout(self.connection_timeout)
                    .with_transport_socket(self.transport_socket.clone())
                    .with_server_name(server_name)
                    .with_http_protocol_options(protocol_options.clone())
                    .prepare()
                    .build()
            })
            .collect::<Result<Vec<_>>>()?;

        let balancer = match self.lb_policy {
            LbPolicy::Random | LbPolicy::ClusterProvided => {
                BalancerType::Random(DefaultBalancer::from_slice(&endpoints))
            },
            LbPolicy::RoundRobin => BalancerType::RoundRobin(DefaultBalancer::from_slice(&endpoints)),
            LbPolicy::LeastRequest => BalancerType::LeastRequests(DefaultBalancer::from_slice(&endpoints)),
            LbPolicy::RingHash => BalancerType::RingHash(DefaultBalancer::from_slice(&endpoints)),
            LbPolicy::Maglev => BalancerType::Maglev(DefaultBalancer::from_slice(&endpoints)),
        };

        Ok(ClusterLoadAssignment {
            cluster_name,
            protocol_options,
            balancer,
            transport_socket: self.transport_socket,
            endpoints,
        })
    }
}

impl TryFrom<ClusterLoadAssignmentConfig> for PartialClusterLoadAssignment {
    type Error = crate::Error;
    fn try_from(cla: ClusterLoadAssignmentConfig) -> Result<Self> {
        let endpoints: Vec<_> =
            cla.endpoints.into_iter().map(PartialLocalityLbEndpoints::try_from).collect::<Result<_>>()?;

        if endpoints.is_empty() {
            return Err("At least one locality must be specified".into());
        }

        Ok(Self { endpoints })
    }
}

#[cfg(test)]
mod test {
    use http::uri::Authority;

    use super::LbEndpoint;
    use crate::{
        clusters::health::HealthStatus,
        transport::{
            HttpChannelBuilder, TcpChannelConnector, UpstreamTransportSocketConfigurator, bind_device::BindDevice,
        },
    };

    impl LbEndpoint {
        /// This function is used by unit tests in other modules
        pub fn new(
            authority: Authority,
            cluster_name: &'static str,
            bind_device: Option<BindDevice>,
            weight: u32,
            health_status: HealthStatus,
        ) -> Self {
            let http_channel = HttpChannelBuilder::new(bind_device.clone())
                .with_authority(authority.clone())
                .with_cluster_name(cluster_name)
                .build()
                .unwrap();
            let tcp_channel = TcpChannelConnector::new(
                &authority,
                "test_cluster",
                bind_device.clone(),
                None,
                UpstreamTransportSocketConfigurator::default(),
            );

            Self { name: "Cluster", authority, bind_device, weight, health_status, http_channel, tcp_channel }
        }
    }
}
