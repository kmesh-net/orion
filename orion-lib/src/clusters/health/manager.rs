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

use std::collections::HashMap;

use orion_configuration::config::cluster::{HealthCheck, health_check::HealthCheckProtocol};
use tokio::sync::mpsc;

use crate::clusters::{
    cluster::{ClusterOps, ClusterType},
    clusters_manager,
    health::{EndpointHealthUpdate, checkers::EndpointHealthChecker},
};

use super::EndpointId;

pub struct HealthCheckManager {
    /// This sender is kept here to clone it every time a new health checker is spawned.
    updates_from_checkers_sender: mpsc::Sender<EndpointHealthUpdate>,
    checkers: HashMap<String, Vec<EndpointHealthChecker>>,
}

impl HealthCheckManager {
    pub fn new(updates_from_checkers_sender: mpsc::Sender<EndpointHealthUpdate>) -> Self {
        HealthCheckManager { updates_from_checkers_sender, checkers: HashMap::new() }
    }

    pub async fn stop_all(&mut self) {
        for checker in self.checkers.drain().flat_map(|(_, checkers)| checkers.into_iter()) {
            checker.stop().await;
        }
    }

    pub async fn restart_cluster(&mut self, cluster_config: ClusterType) {
        let cluster_name = cluster_config.get_name();
        self.stop_cluster(cluster_name).await;
        if let Some(health_check_config) = cluster_config.into_health_check() {
            let HealthCheck { cluster: cluster_config, protocol } = health_check_config;

            let checkers = self.checkers.entry(cluster_name.to_owned()).or_default();

            match protocol {
                HealthCheckProtocol::Http(http_config) => {
                    let Ok(endpoints) = clusters_manager::all_http_connections(cluster_name) else {
                        return;
                    };

                    for (authority, channel) in endpoints {
                        let endpoint_id = EndpointId { cluster: cluster_name.to_owned(), endpoint: authority };

                        let new_checker = EndpointHealthChecker::try_new_http(
                            endpoint_id.clone(),
                            cluster_config.clone(),
                            http_config.clone(),
                            channel,
                            self.updates_from_checkers_sender.clone(),
                        );

                        match new_checker {
                            Ok(checker) => {
                                checkers.push(checker);
                            },
                            Err(err) => tracing::warn!(
                                "Could not start new HTTP health checker for endpoint {} in cluster {}: {}",
                                endpoint_id.endpoint,
                                endpoint_id.cluster,
                                err
                            ),
                        }
                    }
                },
                HealthCheckProtocol::Tcp(tcp_config) => {
                    let Ok(endpoints) = clusters_manager::all_tcp_connections(cluster_name) else {
                        return;
                    };

                    for (authority, channel) in endpoints {
                        let endpoint_id = EndpointId { cluster: cluster_name.to_owned(), endpoint: authority };

                        checkers.push(EndpointHealthChecker::new_tcp(
                            endpoint_id.clone(),
                            cluster_config.clone(),
                            tcp_config.clone(),
                            channel,
                            self.updates_from_checkers_sender.clone(),
                        ));
                    }
                },
                HealthCheckProtocol::Grpc(grpc_config) => {
                    let Ok(endpoints) = clusters_manager::all_grpc_connections(cluster_name) else {
                        return;
                    };

                    for endpoint_result in endpoints {
                        let (endpoint, channel) = match endpoint_result {
                            Ok(result) => result,
                            Err(err) => {
                                tracing::error!("Failed to obtain gRPC client in cluster {}: {}", cluster_name, err);
                                continue;
                            },
                        };

                        let endpoint_id = EndpointId { cluster: cluster_name.to_owned(), endpoint };

                        checkers.push(EndpointHealthChecker::new_grpc(
                            endpoint_id.clone(),
                            cluster_config.clone(),
                            grpc_config.clone(),
                            channel,
                            self.updates_from_checkers_sender.clone(),
                        ));
                    }
                },
            }
        }
    }

    pub async fn stop_cluster(&mut self, cluster: &str) {
        let Some(endpoints_in_the_cluster) = self.checkers.remove(cluster) else {
            return;
        };
        for checker in endpoints_in_the_cluster {
            checker.stop().await;
        }
    }
}
