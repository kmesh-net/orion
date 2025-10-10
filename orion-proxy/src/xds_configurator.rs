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

#[cfg(feature = "tracing")]
use compact_str::ToCompactString;
use futures::future::join_all;
use orion_configuration::config::{Listener, bootstrap::Node, cluster::ClusterSpecifier};
use orion_lib::{
    ConfigurationSenders, ConversionContext, EndpointHealthUpdate, HealthCheckManager, ListenerConfigurationChange,
    ListenerFactory, PartialClusterLoadAssignment, PartialClusterType, Result, RouteConfigurationChange, SecretManager,
    SessionManager,
    access_log::{Target, update_configuration},
    clusters::cluster::ClusterType,
};
use orion_xds::{
    start_aggregate_client_no_retry_loop,
    xds::{
        bindings::AggregatedDiscoveryType,
        client::{
            DeltaClientBackgroundWorker, DeltaDiscoveryClient, DeltaDiscoverySubscriptionManager, XdsUpdateEvent,
        },
        model::{RejectedConfig, TypeUrl, XdsResourcePayload, XdsResourceUpdate},
    },
};
use parking_lot::RwLock;
use std::{sync::Arc, time::Duration};
use tokio::{
    select,
    sync::mpsc::{self, Receiver, Sender},
};
use tracing::{debug, info, warn};

const RETRY_INTERVAL: Duration = Duration::from_secs(10);

pub struct XdsConfigurationHandler {
    secret_manager: Arc<RwLock<SecretManager>>,
    health_manager: HealthCheckManager,
    listeners_senders: Vec<Sender<ListenerConfigurationChange>>,
    route_senders: Vec<Sender<RouteConfigurationChange>>,
    health_updates_receiver: Receiver<EndpointHealthUpdate>,
    mcp_session_manager: Arc<SessionManager>,
}

impl XdsConfigurationHandler {
    pub fn new(
        secret_manager: Arc<RwLock<SecretManager>>,
        configuration_senders: Vec<ConfigurationSenders>,
        mcp_session_manager: Arc<SessionManager>,
    ) -> Self {
        let mut listeners_senders = Vec::with_capacity(configuration_senders.len());
        let mut route_senders = Vec::with_capacity(configuration_senders.len());
        for ConfigurationSenders { listener_configuration_sender, route_configuration_sender } in configuration_senders
        {
            listeners_senders.push(listener_configuration_sender);
            route_senders.push(route_configuration_sender);
        }
        let (health_updates_sender, health_updates_receiver) = mpsc::channel(1000);
        let health_manager = HealthCheckManager::new(health_updates_sender);
        Self {
            secret_manager,
            health_manager,
            listeners_senders,
            route_senders,
            health_updates_receiver,
            mcp_session_manager,
        }
    }

    // Resolve cluster name into working endpoint(s), return working client
    fn resolve_endpoints(
        cluster_name: &str,
        node: &Node,
    ) -> Result<(
        DeltaClientBackgroundWorker<AggregatedDiscoveryType<orion_lib::clusters::SimpleRoundRobinGrpcServiceLB>>,
        DeltaDiscoveryClient,
        DeltaDiscoverySubscriptionManager,
    )> {
        let selector = ClusterSpecifier::Cluster(cluster_name.into());
        let cluster_id = orion_lib::clusters::resolve_cluster(&selector)
            .ok_or_else(|| format!("Failed to resolve cluster {cluster_name} from specifier"))?;
        let grpc_connections = match orion_lib::clusters::all_grpc_connections(cluster_id) {
            Ok(connections) => connections,
            Err(err) => {
                let msg = format!("Failed to get gRPC connections from cluster ({cluster_name}): {err}");
                warn!(msg);
                return Err(msg.into());
            },
        };
        let grpc_services: Vec<orion_lib::clusters::GrpcService> = grpc_connections
            .into_iter()
            .filter_map(|result| match result {
                Ok((_, grpc_service)) => Some(grpc_service),
                Err(err) => {
                    let msg = format!("Skipping (failed) gRPC endpoint for cluster ({cluster_name}): {err}");
                    warn!(msg);
                    None
                },
            })
            .collect();

        if grpc_services.is_empty() {
            let msg = format!("Failed to locate any gRPC connections for cluster ({cluster_name})");
            warn!(msg);
            Err(msg.into())
        } else {
            let grpc_service_lb = orion_lib::clusters::SimpleRoundRobinGrpcServiceLB::new(grpc_services);
            start_aggregate_client_no_retry_loop(node.clone(), grpc_service_lb)
                .inspect_err(|e| warn!("Failed to connect to xDS server ({cluster_name}): {e}"))
                .map_err(Into::into)
        }
    }

    pub async fn run_loop(
        &mut self,
        node: Node,
        initial_clusters: Vec<ClusterType>,
        ads_cluster_names: Vec<String>,
    ) -> Result<()> {
        for cluster in initial_clusters {
            self.health_manager.restart_cluster(cluster).await;
        }

        let mut cluster_names = ads_cluster_names.into_iter().cycle();

        let (mut worker, mut client, _subscription_manager) = loop {
            let Some(cluster_name) = cluster_names.next() else {
                info!("No xDS clusters configured");
                return Ok(());
            };

            if let Ok(val) = Self::resolve_endpoints(&cluster_name, &node) {
                break val;
            }

            info!("Retrying XDS connection in {} seconds", RETRY_INTERVAL.as_secs());
            tokio::time::sleep(RETRY_INTERVAL).await;
        };

        tokio::spawn(async move {
            let subscribe = worker.run().await;
            info!("Worker exited {subscribe:?}");
        });

        loop {
            select! {
                Some(xds_update) = client.recv() => {
                    info!("Got notification {xds_update:?}");
                    let XdsUpdateEvent { ack_channel, updates } = xds_update;
                    // Box::pin because the future from self.process_updates() is very large
                    let rejected_updates = Box::pin(self.process_updates(updates)).await;
                    let _ = ack_channel.send(rejected_updates);
                },
                Some(health_update) = self.health_updates_receiver.recv() => Self::process_health_event(&health_update),
                else => break,
            }
        }

        self.health_manager.stop_all().await;
        Ok(())
    }

    async fn process_updates(&mut self, updates: Vec<XdsResourceUpdate>) -> Vec<RejectedConfig> {
        let mut rejected_updates = Vec::new();
        for update in updates {
            match update {
                XdsResourceUpdate::Update(id, resource, _) => {
                    if let Err(e) = self.process_update_event(&id, resource).await {
                        rejected_updates.push(RejectedConfig::from((id, e)));
                    }
                },
                XdsResourceUpdate::Remove(id, resource) => {
                    if let Err(e) = self.process_remove_event(&id, resource).await {
                        rejected_updates.push(RejectedConfig::from((id, e)));
                    }
                },
            }
        }
        rejected_updates
    }

    async fn process_remove_event(&mut self, id: &str, resource: TypeUrl) -> Result<()> {
        match resource {
            orion_xds::xds::model::TypeUrl::Cluster => {
                orion_lib::clusters::remove_cluster(id)?;
                self.health_manager.stop_cluster(id).await;
                Ok(())
            },
            orion_xds::xds::model::TypeUrl::Listener => {
                let change = ListenerConfigurationChange::Removed(id.to_owned());
                let _ = send_change_to_runtimes(&self.listeners_senders, change).await;
                // remove access logs configuration...
                self.access_log_listener_remove(id).await;
                // remove tracer configuration...
                #[cfg(feature = "tracing")]
                self.tracer_listener_remove(id);
                Ok(())
            },
            orion_xds::xds::model::TypeUrl::ClusterLoadAssignment => {
                orion_lib::clusters::remove_cluster_load_assignment(id)?;
                self.health_manager.stop_cluster(id).await;
                Ok(())
            },
            orion_xds::xds::model::TypeUrl::RouteConfiguration => {
                let change = RouteConfigurationChange::Removed(id.to_owned());
                let _ = send_change_to_runtimes(&self.route_senders, change).await;
                Ok(())
            },
            orion_xds::xds::model::TypeUrl::Secret => {
                let msg = "Secret removal is not supported";
                warn!("{msg}");
                Err(msg.into())
            },
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn process_update_event(&mut self, _: &str, resource: XdsResourcePayload) -> Result<()> {
        match resource {
            XdsResourcePayload::Listener(id, listener) => {
                debug!("Got update for listener {id} {:?}", listener);
                let factory = ListenerFactory::try_from(ConversionContext::new((
                    listener.clone(),
                    &*self.secret_manager.read(),
                    Arc::clone(&self.mcp_session_manager),
                )));

                match factory {
                    Ok(factory) => {
                        let change = ListenerConfigurationChange::Added(Box::new((factory, listener.clone())));
                        let _ = send_change_to_runtimes(&self.listeners_senders, change).await;
                        // update access logs configuration...
                        self.access_log_listener_update(&id, &listener).await;

                        // update tracer configuration...
                        #[cfg(feature = "tracing")]
                        self.tracer_listener_update(&id, &listener);
                        Ok(())
                    },
                    Err(err) => {
                        warn!("Got invalid update for listener {id}");
                        Err(err)
                    },
                }
            },
            XdsResourcePayload::Cluster(id, cluster) => {
                debug!("Got update for cluster: {id}: {:#?}", cluster);
                let cluster_builder = PartialClusterType::try_from((cluster, &*self.secret_manager.read()));
                match cluster_builder {
                    Ok(cluster) => self.add_cluster(cluster).await,
                    Err(err) => {
                        warn!("Got invalid update for cluster {id}");
                        Err(err)
                    },
                }
            },
            XdsResourcePayload::RouteConfiguration(id, route) => {
                debug!("Got update for route configuration {id}: {:#?}", route);
                let change = RouteConfigurationChange::Added((id.clone(), route));
                let _ = send_change_to_runtimes(&self.route_senders, change).await;
                Ok(())
            },
            XdsResourcePayload::Endpoints(id, cla) => {
                debug!("Got update for cluster load assignment {id}: {:#?}", cla);
                let cla = PartialClusterLoadAssignment::try_from(cla);

                match cla {
                    Ok(cla) => {
                        let cluster_name = id.clone();
                        let cluster_config = orion_lib::clusters::change_cluster_load_assignment(&cluster_name, &cla)?;
                        self.health_manager.restart_cluster(cluster_config).await;
                        Ok(())
                    },
                    Err(err) => {
                        warn!("Got invalid update for cluster load assignment {id}");
                        Err(err)
                    },
                }
            },
            XdsResourcePayload::Secret(id, secret) => {
                debug!("Got update for secret {id}: {:#?}", secret);
                let res = self.secret_manager.write().add(&secret);

                match res {
                    Ok(secret) => {
                        let cluster_configs = orion_lib::clusters::update_tls_context(&id, &secret)?;
                        for cluster_config in cluster_configs {
                            self.health_manager.restart_cluster(cluster_config).await;
                        }
                        let change = ListenerConfigurationChange::TlsContextChanged((id.clone(), secret));
                        let _ = send_change_to_runtimes(&self.listeners_senders, change).await;
                        Ok(())
                    },
                    Err(err) => {
                        warn!("Got invalid update for cluster load assignment {id}");
                        Err(err)
                    },
                }
            },
        }
    }

    #[cfg(feature = "tracing")]
    fn tracer_listener_update(&self, id: &str, listener: &Listener) {
        orion_tracing::otel_update_tracers(listener.get_tracing_configurations())
            .unwrap_or_else(|err| warn!("Failed to update tracer for listener {id}: {err}"));
    }

    #[cfg(feature = "tracing")]
    fn tracer_listener_remove(&self, id: &str) {
        orion_tracing::otel_remove_tracers_by_listeners(&[id.to_compact_string()])
            .unwrap_or_else(|err| warn!("Failed to remove tracer for listener {id}: {err}"));
    }

    async fn access_log_listener_update(&mut self, id: &str, listener: &Listener) {
        let access_logs = listener.get_access_log_configurations();
        if let Err(err) = update_configuration(Target::Listener(id.into()), access_logs).await {
            warn!("Failed to update access log configuration for listener {id}: {err}");
        }
    }

    async fn access_log_listener_remove(&mut self, id: &str) {
        if let Err(err) = update_configuration(Target::Listener(id.into()), vec![]).await {
            warn!("Failed to remove access log configuration for listener {id}: {err}");
        }
    }

    async fn add_cluster(&mut self, cluster: PartialClusterType) -> Result<()> {
        let cluster_config = orion_lib::clusters::add_cluster(cluster)?;
        self.health_manager.restart_cluster(cluster_config).await;
        Ok(())
    }

    fn process_health_event(health_update: &EndpointHealthUpdate) {
        orion_lib::clusters::update_endpoint_health(
            &health_update.endpoint.cluster,
            &health_update.endpoint.endpoint,
            health_update.health,
        );
    }
}

pub async fn send_change_to_runtimes<Change: Clone>(channels: &[Sender<Change>], change: Change) -> Result<()> {
    let futures: Vec<_> = channels
        .iter()
        .map(|f| {
            let change = change.clone();
            f.send(change)
        })
        .collect();
    let _ = join_all(futures).await;
    Ok(())
}
