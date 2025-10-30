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

#![allow(dead_code)]

use std::{future::Future, pin::Pin};

use model::TypeUrl;
use orion_data_plane_api::envoy_data_plane_api::{
    envoy::service::{
        cluster::v3::cluster_discovery_service_client::ClusterDiscoveryServiceClient,
        discovery::v3::{
            aggregated_discovery_service_client::AggregatedDiscoveryServiceClient, DeltaDiscoveryRequest,
            DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
        },
        endpoint::v3::endpoint_discovery_service_client::EndpointDiscoveryServiceClient,
        listener::v3::listener_discovery_service_client::ListenerDiscoveryServiceClient,
        route::v3::route_discovery_service_client::RouteDiscoveryServiceClient,
        secret::v3::secret_discovery_service_client::SecretDiscoveryServiceClient,
    },
    tonic,
};
use tokio_stream::Stream;
use tonic::{codegen::StdError, transport::Channel};

use super::model;

pub type DeltaDiscoveryResponseFuture<'a> = Pin<
    Box<
        dyn Future<
                Output = std::result::Result<
                    tonic::Response<tonic::codec::Streaming<DeltaDiscoveryResponse>>,
                    tonic::Status,
                >,
            > + Send
            + 'a,
    >,
>;

pub type DiscoveryResponseFuture<'a> = Pin<
    Box<
        dyn Future<
                Output = std::result::Result<
                    tonic::Response<tonic::codec::Streaming<DiscoveryResponse>>,
                    tonic::Status,
                >,
            > + Send
            + 'a,
    >,
>;

/// Abstracts over the variation in generated xDS clients
pub trait TypedXdsBinding {
    fn type_url() -> Option<TypeUrl>;
    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaDiscoveryResponseFuture<'_>;
    fn stream_request(
        &mut self,
        request: impl Stream<Item = DiscoveryRequest> + Send + 'static,
    ) -> DiscoveryResponseFuture<'_>;
}

/// Handle to ADS client
#[derive(Debug)]
pub struct AggregatedDiscoveryType<C> {
    pub underlying_client: AggregatedDiscoveryServiceClient<C>,
}

impl<C> TypedXdsBinding for AggregatedDiscoveryType<C>
where
    C: tower::Service<http::Request<tonic::body::Body>, Response = http::Response<tonic::body::Body>> + Send,
    C::Error: Into<StdError>,
    C::Future: Send,
{
    fn type_url() -> Option<TypeUrl> {
        None
    }

    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaDiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.delta_aggregated_resources(request))
    }

    fn stream_request(
        &mut self,
        request: impl Stream<Item = DiscoveryRequest> + Send + 'static,
    ) -> DiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.stream_aggregated_resources(request))
    }
}

#[derive(Debug)]
/// Handle to CDS client
pub struct ClusterDiscoveryType {
    pub underlying_client: ClusterDiscoveryServiceClient<Channel>,
}

impl TypedXdsBinding for ClusterDiscoveryType {
    fn type_url() -> Option<TypeUrl> {
        Some(TypeUrl::Cluster)
    }

    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaDiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.delta_clusters(request))
    }

    fn stream_request(
        &mut self,
        request: impl Stream<Item = DiscoveryRequest> + Send + 'static,
    ) -> DiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.stream_clusters(request))
    }
}

/// Handle to LDS Client
#[derive(Debug)]
pub struct ListenerDiscoveryType {
    underlying_client: ListenerDiscoveryServiceClient<Channel>,
}

impl TypedXdsBinding for ListenerDiscoveryType {
    fn type_url() -> Option<TypeUrl> {
        Some(TypeUrl::Listener)
    }
    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaDiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.delta_listeners(request))
    }

    fn stream_request(
        &mut self,
        request: impl Stream<Item = DiscoveryRequest> + Send + 'static,
    ) -> DiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.stream_listeners(request))
    }
}

/// Handle to RDS Client
#[derive(Debug)]
pub struct RouteDiscoveryType {
    underlying_client: RouteDiscoveryServiceClient<Channel>,
}

impl TypedXdsBinding for RouteDiscoveryType {
    fn type_url() -> Option<TypeUrl> {
        Some(TypeUrl::RouteConfiguration)
    }

    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaDiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.delta_routes(request))
    }

    fn stream_request(
        &mut self,
        request: impl Stream<Item = DiscoveryRequest> + Send + 'static,
    ) -> DiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.stream_routes(request))
    }
}

/// Handle to EDS Client
#[derive(Debug)]
pub struct EndpointDiscoveryType {
    underlying_client: EndpointDiscoveryServiceClient<Channel>,
}

impl TypedXdsBinding for EndpointDiscoveryType {
    fn type_url() -> Option<TypeUrl> {
        Some(TypeUrl::ClusterLoadAssignment)
    }

    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaDiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.delta_endpoints(request))
    }

    fn stream_request(
        &mut self,
        request: impl Stream<Item = DiscoveryRequest> + Send + 'static,
    ) -> DiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.stream_endpoints(request))
    }
}

/// Handle to SDS Client
#[derive(Debug)]
pub struct SecretsDiscoveryType {
    underlying_client: SecretDiscoveryServiceClient<Channel>,
}

impl TypedXdsBinding for SecretsDiscoveryType {
    fn type_url() -> Option<TypeUrl> {
        Some(TypeUrl::Secret)
    }

    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaDiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.delta_secrets(request))
    }

    fn stream_request(
        &mut self,
        request: impl Stream<Item = DiscoveryRequest> + Send + 'static,
    ) -> DiscoveryResponseFuture<'_> {
        Box::pin(self.underlying_client.stream_secrets(request))
    }
}
