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

use std::{sync::Arc, time::Duration};

use compact_str::CompactString;
use http::uri::Authority;
use orion_configuration::config::cluster::{
    ClusterLoadAssignment as ClusterLoadAssignmentConfig, EndpointAddress, HealthStatus, HttpProtocolOptions,
    InternalEndpointAddress, LbEndpoint as LbEndpointConfig, LbPolicy,
    LocalityLbEndpoints as LocalityLbEndpointsConfig,
};
use tracing::debug;
use typed_builder::TypedBuilder;
use webpki::types::ServerName;

use super::{
    balancers::{
        hash_policy::HashState, least::WeightedLeastRequestBalancer, maglev::MaglevBalancer, random::RandomBalancer,
        ring::RingHashBalancer, wrr::WeightedRoundRobinBalancer, Balancer, DefaultBalancer, EndpointWithAuthority,
        EndpointWithLoad, WeightedEndpoint,
    },
    // cluster::HyperService,
    health::{EndpointHealth, ValueUpdated},
};
use crate::{
    transport::{
        bind_device::BindDevice, GrpcService, HttpChannel, HttpChannelBuilder, TcpChannelConnector,
        UpstreamTransportSocketConfigurator,
    },
    Result,
};

#[derive(Debug, Clone)]
pub struct LbEndpoint {
    pub name: CompactString,
    pub address: EndpointAddressType,
    pub bind_device: Option<BindDevice>,
    pub weight: u32,
    pub health_status: HealthStatus,
}

#[derive(Debug, Clone)]
pub enum EndpointAddressType {
    Socket(http::uri::Authority, HttpChannel, TcpChannelConnector),
    Internal(InternalEndpointAddress, InternalConnection),
}

impl PartialEq for EndpointAddressType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Socket(auth1, _, _), Self::Socket(auth2, _, _)) => auth1 == auth2,
            (Self::Internal(addr1, _), Self::Internal(addr2, _)) => addr1 == addr2,
            _ => false,
        }
    }
}

impl Eq for EndpointAddressType {}

#[derive(Debug, Clone)]
pub struct InternalConnection {
    pub server_listener_name: CompactString,
    pub endpoint_id: Option<CompactString>,
}

impl PartialEq for LbEndpoint {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
    }
}

impl LbEndpoint {
    pub fn authority(&self) -> &Authority {
        match &self.address {
            EndpointAddressType::Socket(authority, _, _) => authority,
            EndpointAddressType::Internal(_, _) => panic!("Internal endpoints don't have authorities"),
        }
    }
}

impl WeightedEndpoint for LbEndpoint {
    fn weight(&self) -> u32 {
        self.weight
    }
}

impl EndpointWithAuthority for LbEndpoint {
    fn authority(&self) -> &Authority {
        match &self.address {
            EndpointAddressType::Socket(authority, _, _) => authority,
            EndpointAddressType::Internal(_, _) => {
                // For internal endpoints, we'll use a placeholder authority
                // This might need to be handled differently based on the balancer requirements
                panic!("Internal endpoints don't have authorities")
            },
        }
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
        match (&self.address, &other.address) {
            (EndpointAddressType::Socket(a, _, _), EndpointAddressType::Socket(b, _, _)) => a.as_str().cmp(b.as_str()),
            (EndpointAddressType::Internal(a, _), EndpointAddressType::Internal(b, _)) => {
                a.server_listener_name.cmp(&b.server_listener_name)
            },
            (EndpointAddressType::Socket(_, _, _), EndpointAddressType::Internal(_, _)) => std::cmp::Ordering::Less,
            (EndpointAddressType::Internal(_, _), EndpointAddressType::Socket(_, _, _)) => std::cmp::Ordering::Greater,
        }
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
        match &self.address {
            EndpointAddressType::Socket(authority, http_channel, _) => {
                GrpcService::try_new(http_channel.clone(), authority.clone())
            },
            EndpointAddressType::Internal(_, _) => Err("Internal endpoints don't support gRPC service yet".into()),
        }
    }

    pub fn http_channel(&self) -> Option<&HttpChannel> {
        match &self.address {
            EndpointAddressType::Socket(_, http_channel, _) => Some(http_channel),
            EndpointAddressType::Internal(_, _) => None,
        }
    }

    pub fn tcp_channel(&self) -> Option<&TcpChannelConnector> {
        match &self.address {
            EndpointAddressType::Socket(_, _, tcp_channel) => Some(tcp_channel),
            EndpointAddressType::Internal(_, _) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PartialLbEndpoint {
    pub address: EndpointAddressType,
    pub bind_device: Option<BindDevice>,
    pub weight: u32,
    pub health_status: HealthStatus,
}

impl PartialLbEndpoint {
    fn new(value: &LbEndpoint) -> Self {
        PartialLbEndpoint {
            address: value.address.clone(),
            bind_device: value.bind_device.clone(),
            weight: value.weight,
            health_status: value.health_status,
        }
    }
}

impl EndpointWithLoad for LbEndpoint {
    fn http_load(&self) -> u32 {
        match &self.address {
            EndpointAddressType::Socket(_, http_channel, _) => http_channel.load(),
            EndpointAddressType::Internal(_, _) => 0, // Internal endpoints don't have HTTP load tracking yet
        }
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
        let PartialLbEndpoint { ref address, bind_device, weight, health_status } = self.endpoint;

        let address = match address {
            EndpointAddressType::Socket(authority, _, _) => {
                let mut builder = HttpChannelBuilder::new(bind_device.clone())
                    .with_timeout(self.connect_timeout)
                    .with_authority(authority.clone());

                // Configure TLS if needed
                if let UpstreamTransportSocketConfigurator::Tls(tls_configurator) = &self.transport_socket {
                    builder = builder.with_tls(Some(tls_configurator.clone()));
                }

                let builder = if let Some(_bind_device) = &bind_device { builder } else { builder };
                let http_channel = builder.with_http_protocol_options(self.http_protocol_options).build()?;
                let tcp_channel = TcpChannelConnector::new(
                    &authority,
                    cluster_name,
                    bind_device.clone(),
                    self.connect_timeout,
                    self.transport_socket.clone(),
                );
                EndpointAddressType::Socket(authority.clone(), http_channel, tcp_channel)
            },
            EndpointAddressType::Internal(internal_addr, _) => EndpointAddressType::Internal(
                internal_addr.clone(),
                InternalConnection {
                    server_listener_name: internal_addr.server_listener_name.clone(),
                    endpoint_id: internal_addr.endpoint_id.clone(),
                },
            ),
        };

        Ok(Arc::new(LbEndpoint { name: cluster_name.into(), address, bind_device, weight, health_status }))
    }
}

impl TryFrom<LbEndpointConfig> for PartialLbEndpoint {
    type Error = crate::Error;

    fn try_from(lb_endpoint: LbEndpointConfig) -> Result<Self> {
        let health_status = lb_endpoint.health_status;
        let address = match lb_endpoint.address {
            EndpointAddress::Socket(socket_addr) => {
                let authority = http::uri::Authority::try_from(format!("{socket_addr}"))?;
                // Note: We'll create placeholder channels here; they'll be properly initialized in the builder
                let http_channel = HttpChannelBuilder::new(None).with_authority(authority.clone()).build()?;
                let tcp_channel = TcpChannelConnector::new(
                    &authority,
                    "placeholder",
                    None,
                    Some(Duration::from_secs(5)),
                    UpstreamTransportSocketConfigurator::default(),
                );
                EndpointAddressType::Socket(authority, http_channel, tcp_channel)
            },
            EndpointAddress::Internal(internal_addr) => EndpointAddressType::Internal(
                internal_addr.clone(),
                InternalConnection {
                    server_listener_name: internal_addr.server_listener_name.clone(),
                    endpoint_id: internal_addr.endpoint_id.clone(),
                },
            ),
        };
        let weight = lb_endpoint.load_balancing_weight.into();
        Ok(PartialLbEndpoint { address, bind_device: None, weight, health_status })
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
        Ok(endpoint.http_channel().ok_or("No HTTP channel available for this endpoint")?.clone())
    }

    pub fn get_tcp_channel(&mut self) -> Result<TcpChannelConnector> {
        let endpoint = self.balancer.next_item(None).ok_or("No active endpoint")?;
        Ok(endpoint.tcp_channel().ok_or("No TCP channel available for this endpoint")?.clone())
    }

    pub fn get_grpc_channel(&mut self) -> Result<GrpcService> {
        let endpoint = self.balancer.next_item(None).ok_or("No active endpoint")?;
        endpoint.grpc_service()
    }

    pub fn all_http_channels(&self) -> Vec<(Authority, HttpChannel)> {
        self.all_endpoints_iter()
            .filter_map(|endpoint| {
                endpoint.http_channel().map(|channel| (endpoint.authority().clone(), channel.clone()))
            })
            .collect()
    }

    pub fn all_tcp_channels(&self) -> Vec<(Authority, TcpChannelConnector)> {
        self.all_endpoints_iter()
            .filter_map(|endpoint| {
                endpoint.tcp_channel().map(|channel| (endpoint.authority().clone(), channel.clone()))
            })
            .collect()
    }

    pub fn try_all_grpc_channels(&self) -> Vec<Result<(Authority, GrpcService)>> {
        self.all_endpoints_iter()
            .map(|endpoint| endpoint.grpc_service().map(|channel| (endpoint.authority().clone(), channel)))
            .collect()
    }

    pub fn update_endpoint_health(&mut self, authority: &http::uri::Authority, health: HealthStatus) {
        for locality in &self.endpoints {
            locality.endpoints.iter().filter(|endpoint| endpoint.authority() == authority).for_each(|endpoint| {
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

    use super::{EndpointAddressType, LbEndpoint};
    use crate::{
        clusters::health::HealthStatus,
        transport::{
            bind_device::BindDevice, HttpChannelBuilder, TcpChannelConnector, UpstreamTransportSocketConfigurator,
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
                cluster_name,
                bind_device.clone(),
                None,
                UpstreamTransportSocketConfigurator::None,
            );

            Self {
                name: "Cluster".into(),
                address: EndpointAddressType::Socket(authority, http_channel, tcp_channel),
                bind_device,
                weight,
                health_status,
            }
        }
    }
}
