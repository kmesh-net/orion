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

#[cfg(test)]
mod tests;

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use orion_configuration::config::cluster::health_check::{ClusterHealthCheck, GrpcHealthCheck};
use orion_xds::grpc_deps::tonic_health::pb::health_client::HealthClient;
use orion_xds::grpc_deps::tonic_health::pb::{
    health_check_response::ServingStatus, HealthCheckRequest, HealthCheckResponse,
};
use orion_xds::grpc_deps::{Response as TonicResponse, Status as TonicStatus};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;

use super::checker::{IntervalWaiter, ProtocolChecker, WaitInterval};
use crate::clusters::health::checkers::checker::HealthCheckerLoop;
use crate::clusters::health::counter::HealthStatusCounter;
use crate::clusters::health::{EndpointHealthUpdate, EndpointId};
use crate::transport::GrpcService;
use crate::Error;

/// Spawns an HTTP health checker and returns its handle. Must be called from a Tokio runtime context.
pub fn spawn_grpc_health_checker(
    endpoint: EndpointId,
    cluster_config: ClusterHealthCheck,
    protocol_config: GrpcHealthCheck,
    channel: GrpcService,
    sender: mpsc::Sender<EndpointHealthUpdate>,
    stop_signal: Arc<Notify>,
) -> JoinHandle<Result<(), Error>> {
    let interval_waiter = IntervalWaiter;
    spawn_grpc_health_checker_impl(
        endpoint,
        cluster_config,
        protocol_config,
        sender,
        stop_signal,
        (HealthClient::new(channel), interval_waiter),
    )
}

trait GrpcHealthChannel {
    fn check(
        &mut self,
        request: HealthCheckRequest,
    ) -> BoxFuture<'_, Result<TonicResponse<HealthCheckResponse>, TonicStatus>>;
}

impl GrpcHealthChannel for HealthClient<GrpcService> {
    fn check(
        &mut self,
        request: HealthCheckRequest,
    ) -> BoxFuture<'_, Result<TonicResponse<HealthCheckResponse>, TonicStatus>> {
        HealthClient::check(self, request).boxed()
    }
}

/// Actual implementation of `spawn_grpc_health_checker()`, with `dependencies` containing the
/// injected gRPC stack builder and interval waiter.
fn spawn_grpc_health_checker_impl<G, W>(
    endpoint: EndpointId,
    cluster_config: ClusterHealthCheck,
    protocol_config: GrpcHealthCheck,
    sender: mpsc::Sender<EndpointHealthUpdate>,
    stop_signal: Arc<Notify>,
    dependencies: (G, W),
) -> JoinHandle<Result<(), Error>>
where
    G: GrpcHealthChannel + Send + 'static,
    W: WaitInterval + Send + 'static,
{
    tracing::debug!(
        "Starting gRPC health checks of endpoint {:?} in cluster {:?}",
        endpoint.endpoint,
        endpoint.cluster
    );

    let (grpc_client, interval_waiter) = dependencies;

    let grpc_checker = GrpcChecker { channel: grpc_client, config: protocol_config };

    let check_loop =
        HealthCheckerLoop::new(endpoint, cluster_config, sender, stop_signal, interval_waiter, grpc_checker);

    check_loop.spawn()
}

struct GrpcChecker<G> {
    channel: G,
    config: GrpcHealthCheck,
}

impl<G> ProtocolChecker for GrpcChecker<G>
where
    G: GrpcHealthChannel + Send,
{
    type Response = HealthCheckResponse;

    fn check(
        &mut self,
    ) -> impl futures::Future<Output = std::result::Result<<Self as ProtocolChecker>::Response, orion_error::Error>>
           + std::marker::Send {
        async move {
            let request = HealthCheckRequest { service: self.config.service_name.clone().into() };
            Ok(self.channel.check(request).await.map(TonicResponse::into_inner)?)
        }
        .boxed()
    }

    fn process_response(
        &self,
        endpoint: &EndpointId,
        counter: &mut HealthStatusCounter,
        response: &Self::Response,
    ) -> Option<orion_configuration::config::cluster::HealthStatus> {
        match response.status() {
            status @ (ServingStatus::Unknown | ServingStatus::NotServing | ServingStatus::ServiceUnknown) => {
                tracing::debug!(
                    "Failed health check of {:?} in cluster {}: {}",
                    endpoint.endpoint,
                    endpoint.cluster,
                    status.as_str_name(),
                );
                counter.add_failure()
            },
            ServingStatus::Serving => counter.add_success(),
        }
    }
}
