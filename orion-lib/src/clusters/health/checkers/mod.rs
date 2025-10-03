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

mod checker;
mod grpc;
mod http;
mod tcp;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use orion_configuration::config::cluster::health_check::{
    ClusterHealthCheck, GrpcHealthCheck, HttpHealthCheck, TcpHealthCheck,
};
use tokio::{
    sync::{Notify, mpsc},
    task::JoinHandle,
};

use self::{grpc::spawn_grpc_health_checker, http::try_spawn_http_health_checker, tcp::spawn_tcp_health_checker};
use super::{EndpointHealthUpdate, EndpointId, HealthStatus};
use crate::{
    Error,
    clusters::GrpcService,
    transport::{HttpChannel, TcpChannelConnector},
};

#[derive(Debug)]
pub struct EndpointHealthChecker {
    health_check_task: Option<JoinHandle<Result<(), Error>>>,
    stop_signal: Arc<Notify>,
}

impl EndpointHealthChecker {
    pub fn try_new_http(
        endpoint: EndpointId,
        cluster_config: ClusterHealthCheck,
        protocol_config: HttpHealthCheck,
        channel: HttpChannel,
        sender: mpsc::Sender<EndpointHealthUpdate>,
    ) -> Result<Self, Error> {
        let stop_signal = Arc::new(Notify::new());
        Ok(EndpointHealthChecker {
            health_check_task: {
                Some(try_spawn_http_health_checker(
                    endpoint,
                    cluster_config,
                    protocol_config,
                    channel,
                    sender,
                    Arc::clone(&stop_signal),
                )?)
            },
            stop_signal,
        })
    }

    pub fn new_tcp(
        endpoint: EndpointId,
        cluster_config: ClusterHealthCheck,
        protocol_config: TcpHealthCheck,
        channel: TcpChannelConnector,
        sender: mpsc::Sender<EndpointHealthUpdate>,
    ) -> Self {
        let stop_signal = Arc::new(Notify::new());
        EndpointHealthChecker {
            health_check_task: {
                Some(spawn_tcp_health_checker(
                    endpoint,
                    cluster_config,
                    protocol_config,
                    channel,
                    sender,
                    Arc::clone(&stop_signal),
                ))
            },
            stop_signal,
        }
    }

    pub fn new_grpc(
        endpoint: EndpointId,
        cluster_config: ClusterHealthCheck,
        protocol_config: GrpcHealthCheck,
        channel: GrpcService,
        sender: mpsc::Sender<EndpointHealthUpdate>,
    ) -> Self {
        let stop_signal = Arc::new(Notify::new());
        EndpointHealthChecker {
            health_check_task: {
                Some(spawn_grpc_health_checker(
                    endpoint,
                    cluster_config,
                    protocol_config,
                    channel,
                    sender,
                    Arc::clone(&stop_signal),
                ))
            },
            stop_signal,
        }
    }

    pub async fn stop(mut self) {
        self.stop_signal.notify_waiters();
        if let Some(handle) = self.health_check_task.take() {
            match handle.await {
                Ok(Err(err)) => tracing::warn!("Health checker failed: {}", err),
                Err(join_err) => tracing::warn!("Error joining health checker task: {}", join_err),
                _ => (),
            }
        }
    }
}

enum CurrentHealthStatus {
    Unchanged(Option<HealthStatus>),
    Edge(HealthStatus),
}
