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

pub mod xds;

pub use crate::xds::model::XdsError;
use crate::xds::{
    bindings::AggregatedDiscoveryType,
    client::{DeltaDiscoveryClient, DiscoveryClientBuilder, RETRY_INTERVAL},
};
use http::{Request, Response};
use orion_configuration::config::bootstrap::Node;
use orion_data_plane_api::envoy_data_plane_api::{
    envoy::service::discovery::v3::aggregated_discovery_service_client::AggregatedDiscoveryServiceClient, tonic,
};
use tonic::{
    body::Body,
    codegen::StdError as TonicError,
    transport::{Channel, Endpoint},
};
use tower::Service;
use tracing::info;
use xds::client::{DeltaClientBackgroundWorker, DeltaDiscoverySubscriptionManager};

pub mod grpc_deps {
    pub use orion_data_plane_api::envoy_data_plane_api::{
        tonic::{body::Body as GrpcBody, codegen::StdError as Error, Response, Status},
        tonic_health,
    };
}
pub const DECODED_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

pub async fn start_aggregate_client(
    node: Node,
    configuration_service_address: tonic::transport::Uri,
) -> Result<
    (
        DeltaClientBackgroundWorker<AggregatedDiscoveryType<Channel>>,
        DeltaDiscoveryClient,
        DeltaDiscoverySubscriptionManager,
    ),
    XdsError,
> {
    info!("Starting xDS client: {:?}", configuration_service_address);
    let endpoint = Endpoint::from(configuration_service_address);
    let disovery_client = loop {
        let endpoint = endpoint.clone();
        if let Ok(client) = AggregatedDiscoveryServiceClient::connect(endpoint).await {
            break client;
        }
        info!("Server doesn't exist yet... retrying in {RETRY_INTERVAL:?}");
        tokio::time::sleep(RETRY_INTERVAL).await;
    };

    let aggregated_discovery_service_client = AggregatedDiscoveryType { underlying_client: disovery_client };

    DiscoveryClientBuilder::new(node, aggregated_discovery_service_client).build()
}

pub fn start_aggregate_client_no_retry_loop<C>(
    node: Node,
    channel: C,
) -> Result<
    (DeltaClientBackgroundWorker<AggregatedDiscoveryType<C>>, DeltaDiscoveryClient, DeltaDiscoverySubscriptionManager),
    XdsError,
>
where
    C: Service<Request<Body>, Response = Response<Body>, Error = TonicError> + Send,
    C::Future: Send,
{
    let underlying_client =
        AggregatedDiscoveryServiceClient::new(channel).max_decoding_message_size(DECODED_MESSAGE_SIZE);
    let aggregated_discovery_service_client = AggregatedDiscoveryType { underlying_client };
    DiscoveryClientBuilder::new(node, aggregated_discovery_service_client).build()
}
