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

use std::{future::Future, pin::Pin};

use super::model;
use envoy_data_plane_api::envoy::service::{
    cluster::v3::cluster_discovery_service_client::ClusterDiscoveryServiceClient,
    discovery::v3::{
        aggregated_discovery_service_client::AggregatedDiscoveryServiceClient, DeltaDiscoveryRequest,
        DeltaDiscoveryResponse,
    },
    endpoint::v3::endpoint_discovery_service_client::EndpointDiscoveryServiceClient,
    listener::v3::listener_discovery_service_client::ListenerDiscoveryServiceClient,
    route::v3::route_discovery_service_client::RouteDiscoveryServiceClient,
    secret::v3::secret_discovery_service_client::SecretDiscoveryServiceClient,
};
use model::TypeUrl;

use envoy_data_plane_api::tonic;
use tokio_stream::Stream;
use tonic::transport::Channel;

pub type DeltaFuture<'a> = Pin<
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

/// Abstracts over the variation in generated xDS clients
pub trait TypedXdsBinding {
    fn type_url() -> Option<TypeUrl>;
    fn delta_request(&mut self, request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static)
        -> DeltaFuture<'_>;
}

/// Handle to ADS client
pub struct AggregatedDiscoveryType {
    pub underlying_client: AggregatedDiscoveryServiceClient<Channel>,
}

impl TypedXdsBinding for AggregatedDiscoveryType {
    fn type_url() -> Option<TypeUrl> {
        None
    }
    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaFuture<'_> {
        Box::pin(self.underlying_client.delta_aggregated_resources(request))
    }
}

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
    ) -> DeltaFuture<'_> {
        Box::pin(self.underlying_client.delta_clusters(request))
    }
}

/// Handle to LDS Client
pub struct ListenerDiscoveryType {
    pub underlying_client: ListenerDiscoveryServiceClient<Channel>,
}

impl TypedXdsBinding for ListenerDiscoveryType {
    fn type_url() -> Option<TypeUrl> {
        Some(TypeUrl::Listener)
    }
    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaFuture<'_> {
        Box::pin(self.underlying_client.delta_listeners(request))
    }
}

/// Handle to RDS Client
pub struct RouteDiscoveryType {
    pub underlying_client: RouteDiscoveryServiceClient<Channel>,
}

impl TypedXdsBinding for RouteDiscoveryType {
    fn type_url() -> Option<TypeUrl> {
        Some(TypeUrl::RouteConfiguration)
    }
    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaFuture<'_> {
        Box::pin(self.underlying_client.delta_routes(request))
    }
}

/// Handle to EDS Client
pub struct EndpointDiscoveryType {
    pub underlying_client: EndpointDiscoveryServiceClient<Channel>,
}

impl TypedXdsBinding for EndpointDiscoveryType {
    fn type_url() -> Option<TypeUrl> {
        Some(TypeUrl::ClusterLoadAssignment)
    }
    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaFuture<'_> {
        Box::pin(self.underlying_client.delta_endpoints(request))
    }
}

/// Handle to SDS Client
pub struct SecretsDiscoveryType {
    pub underlying_client: SecretDiscoveryServiceClient<Channel>,
}

impl TypedXdsBinding for SecretsDiscoveryType {
    fn type_url() -> Option<TypeUrl> {
        Some(TypeUrl::Secret)
    }
    fn delta_request(
        &mut self,
        request: impl Stream<Item = DeltaDiscoveryRequest> + Send + 'static,
    ) -> DeltaFuture<'_> {
        Box::pin(self.underlying_client.delta_secrets(request))
    }
}
