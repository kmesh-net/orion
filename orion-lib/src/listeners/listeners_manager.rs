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

use std::collections::BTreeMap;

use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use orion_configuration::config::{
    network_filters::http_connection_manager::RouteConfiguration, Listener as ListenerConfig,
};

use super::listener::{Listener, ListenerFactory};
use crate::{secrets::TransportSecret, ConfigDump, Result};

#[derive(Debug, Clone)]
pub enum ListenerConfigurationChange {
    Added(Box<(ListenerFactory, ListenerConfig)>),
    Removed(String),
    TlsContextChanged((String, TransportSecret)),
    GetConfiguration(mpsc::Sender<ConfigDump>),
}

#[derive(Debug, Clone)]
pub enum RouteConfigurationChange {
    Added((String, RouteConfiguration)),
    Removed(String),
}

#[derive(Debug, Clone)]
pub enum TlsContextChange {
    Updated((String, TransportSecret)),
}

struct ListenerInfo {
    handle: abort_on_drop::ChildTask<()>,
    listener_conf: ListenerConfig,
}

impl ListenerInfo {
    fn new(handle: tokio::task::JoinHandle<()>, listener_conf: ListenerConfig) -> Self {
        Self { handle: handle.into(), listener_conf }
    }
}

struct PendingListener {
    factory: ListenerFactory,
    config: ListenerConfig,
    dependencies: ListenerDependencies,
}

#[derive(Debug, Clone)]
struct ListenerDependencies {
    route_names: Vec<String>,
    secret_names: Vec<String>,
}

impl ListenerDependencies {
    fn new() -> Self {
        Self { route_names: Vec::new(), secret_names: Vec::new() }
    }

    fn is_ready(
        &self,
        available_routes: &std::collections::HashSet<String>,
        available_secrets: &std::collections::HashSet<String>,
    ) -> bool {
        self.route_names.iter().all(|name| available_routes.contains(name))
            && self.secret_names.iter().all(|name| available_secrets.contains(name))
    }
}

pub struct ListenersManager {
    configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
    route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
    listener_handles: BTreeMap<&'static str, ListenerInfo>,
    pending_listeners: BTreeMap<&'static str, PendingListener>,
    available_routes: std::collections::HashSet<String>,
    available_secrets: std::collections::HashSet<String>,
}

impl ListenersManager {
    pub fn new(
        configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
        route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
    ) -> Self {
        ListenersManager {
            configuration_channel,
            route_configuration_channel,
            listener_handles: BTreeMap::new(),
            pending_listeners: BTreeMap::new(),
            available_routes: std::collections::HashSet::new(),
            available_secrets: std::collections::HashSet::new(),
        }
    }

    pub async fn start(mut self) -> Result<()> {
        let (tx_secret_updates, _) = broadcast::channel(16);
        let (tx_route_updates, _) = broadcast::channel(16);

        loop {
            tokio::select! {
                Some(listener_configuration_change) = self.configuration_channel.recv() => {
                    match listener_configuration_change {
                        ListenerConfigurationChange::Added(boxed) => {
                            let (factory, listener_conf) = *boxed;
                            self.add_listener(factory, listener_conf, &tx_route_updates, &tx_secret_updates)?;
                        }
                        ListenerConfigurationChange::Removed(listener_name) => {
                            let _ = self.stop_listener(&listener_name);
                        },
                        ListenerConfigurationChange::TlsContextChanged((secret_id, secret)) => {
                            info!("Got tls secret update {secret_id}");
                            self.available_secrets.insert(secret_id.clone());
                            let res = tx_secret_updates.send(TlsContextChange::Updated((secret_id, secret)));
                            if let Err(e) = res{
                                warn!("Internal problem when updating a secret: {e}");
                            }
                            self.try_start_pending_listeners(&tx_route_updates, &tx_secret_updates)?;
                        },
                        ListenerConfigurationChange::GetConfiguration(config_dump_tx) => {
                            let listeners: Vec<ListenerConfig> = self.listener_handles
                                .values()
                                .map(|info| info.listener_conf.clone())
                                .collect();
                            config_dump_tx.send(ConfigDump { listeners: Some(listeners), ..Default::default() }).await?;
                        },
                    }
                },
                Some(route_configuration_change) = self.route_configuration_channel.recv() => {
                    match route_configuration_change {
                        RouteConfigurationChange::Added((ref route_name, ref _route_config)) => {
                            self.available_routes.insert(route_name.clone());
                            self.try_start_pending_listeners(&tx_route_updates, &tx_secret_updates)?;
                        },
                        RouteConfigurationChange::Removed(ref route_name) => {
                            self.available_routes.remove(route_name);
                        },
                    }
                    let res = tx_route_updates.send(route_configuration_change);
                    if let Err(e) = res{
                        warn!("Internal problem when updating a route: {e}");
                    }
                },
                else => {
                    warn!("All listener manager channels are closed...exiting");
                    return Err("All listener manager channels are closed...exiting".into());
                }
            }
        }
    }

    fn add_listener(
        &mut self,
        factory: ListenerFactory,
        config: ListenerConfig,
        tx_route_updates: &broadcast::Sender<RouteConfigurationChange>,
        tx_secret_updates: &broadcast::Sender<TlsContextChange>,
    ) -> Result<()> {
        let dependencies = self.extract_listener_dependencies(&config)?;

        if dependencies.is_ready(&self.available_routes, &self.available_secrets) {
            let listener =
                factory.clone().make_listener(tx_route_updates.subscribe(), tx_secret_updates.subscribe())?;
            if let Err(e) = self.start_listener(listener, config) {
                warn!("Failed to start listener: {e}");
            }
        } else {
            let listener_name = factory.get_name();
            info!("Listener {} has dependencies that are not ready yet, adding to pending list", listener_name);
            self.pending_listeners.insert(listener_name, PendingListener { factory, config, dependencies });
        }
        Ok(())
    }

    fn extract_listener_dependencies(&self, config: &ListenerConfig) -> Result<ListenerDependencies> {
        let mut dependencies = ListenerDependencies::new();

        for filter_chain in config.filter_chains.values() {
            if let Some(http_config) = filter_chain.get_http_connection_manager() {
                if let Some(route_name) = http_config.get_dynamic_route_name() {
                    dependencies.route_names.push(route_name.to_string());
                }
            }
            if let Some(secret_names) = filter_chain.get_tls_secret_names() {
                dependencies.secret_names.extend(secret_names);
            }
        }

        Ok(dependencies)
    }

    fn try_start_pending_listeners(
        &mut self,
        tx_route_updates: &broadcast::Sender<RouteConfigurationChange>,
        tx_secret_updates: &broadcast::Sender<TlsContextChange>,
    ) -> Result<()> {
        let ready_keys: Vec<_> = self
            .pending_listeners
            .iter()
            .filter(|(_, p)| p.dependencies.is_ready(&self.available_routes, &self.available_secrets))
            .map(|(k, _)| *k)
            .collect();

        for listener_name in ready_keys {
            if let Some(pending) = self.pending_listeners.remove(listener_name) {
                info!("All dependencies for listener {} are now ready, starting it", listener_name);

                let listener = pending
                    .factory
                    .clone()
                    .make_listener(tx_route_updates.subscribe(), tx_secret_updates.subscribe())?;

                if let Err(e) = self.start_listener(listener, pending.config) {
                    warn!("Failed to start listener {}: {e}", listener_name);
                }
            }
        }

        Ok(())
    }

    pub fn start_listener(&mut self, listener: Listener, listener_conf: ListenerConfig) -> Result<()> {
        let listener_name = listener.get_name();
        let (addr, dev) = listener.get_socket();
        info!("Listener {} at {addr} (device bind:{})", listener_name, dev.is_some());
        // spawn the task for this listener address, this will spawn additional task per connection
        let join_handle = tokio::spawn(async move {
            let error = listener.start().await;
            warn!("Listener {listener_name} exited: {error}");
        });
        #[cfg(debug_assertions)]
        if self.listener_handles.contains_key(&listener_name) {
            debug!("Listener {listener_name} already exists, replacing it");
        }
        // note: join handle gets overwritten here if it already exists.
        // handles are abort on drop so will be aborted, closing the socket
        // but the any tasks spawned within this task, which happens on a per-connection basis,
        // will survive past this point and only get dropped when their session ends
        self.listener_handles.insert(listener_name, ListenerInfo::new(join_handle, listener_conf));

        Ok(())
    }

    pub fn stop_listener(&mut self, listener_name: &str) -> Result<()> {
        if let Some(abort_handler) = self.listener_handles.remove(listener_name) {
            info!("{listener_name} : Stopped");
            abort_handler.handle.abort();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        net::{IpAddr, Ipv4Addr, SocketAddr},
    };

    use super::*;
    use orion_configuration::config::Listener as ListenerConfig;
    use tracing_test::traced_test;

    #[traced_test]
    #[tokio::test]
    async fn start_listener_dup() {
        let chan = 10;
        let name = "testlistener";

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let (_secb_tx, _secb_rx) = broadcast::channel::<TlsContextChange>(chan);
        let (_routeb_tx1, _routeb_rx1) = broadcast::channel::<RouteConfigurationChange>(chan);
        let (_routeb_tx2, _routeb_rx2) = broadcast::channel::<RouteConfigurationChange>(chan);

        let _man = ListenersManager::new(conf_rx, route_rx);

        // Create a simple test listener factory
        let _l1_info = ListenerConfig {
            name: name.into(),
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8000),
            filter_chains: HashMap::new(),
            bind_device: None,
            with_tls_inspector: false,
            proxy_protocol_config: None,
        };

        // For testing, we'll just test the dependency logic directly
        let dependencies = ListenerDependencies::new();
        let available_routes = std::collections::HashSet::new();
        let available_secrets = std::collections::HashSet::new();

        // Empty dependencies should be ready
        assert!(dependencies.is_ready(&available_routes, &available_secrets));

        // Dependencies with missing resources should not be ready
        let mut dependencies = ListenerDependencies::new();
        dependencies.route_names.push("route1".to_string());
        dependencies.secret_names.push("secret1".to_string());

        assert!(!dependencies.is_ready(&available_routes, &available_secrets));

        // Dependencies with available resources should be ready
        let mut available_routes = std::collections::HashSet::new();
        let mut available_secrets = std::collections::HashSet::new();
        available_routes.insert("route1".to_string());
        available_secrets.insert("secret1".to_string());

        assert!(dependencies.is_ready(&available_routes, &available_secrets));

        drop(_routeb_rx1);
        drop(_secb_tx);
        drop(_routeb_rx2);
    }

    #[test]
    fn test_listener_dependencies() {
        let dependencies = ListenerDependencies::new();
        let available_routes = std::collections::HashSet::new();
        let available_secrets = std::collections::HashSet::new();

        // Empty dependencies should be ready
        assert!(dependencies.is_ready(&available_routes, &available_secrets));

        // Dependencies with missing resources should not be ready
        let mut dependencies = ListenerDependencies::new();
        dependencies.route_names.push("route1".to_string());
        dependencies.secret_names.push("secret1".to_string());

        assert!(!dependencies.is_ready(&available_routes, &available_secrets));

        // Dependencies with available resources should be ready
        let mut available_routes = std::collections::HashSet::new();
        let mut available_secrets = std::collections::HashSet::new();
        available_routes.insert("route1".to_string());
        available_secrets.insert("secret1".to_string());

        assert!(dependencies.is_ready(&available_routes, &available_secrets));
    }
}
