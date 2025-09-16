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

use multimap::MultiMap;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

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

#[derive(Debug, Clone)]
pub struct ListenerManagerConfig {
    pub max_versions_per_listener: usize,
    pub cleanup_policy: CleanupPolicy,
    pub cleanup_interval: Duration,
}

#[derive(Debug, Clone)]
pub enum CleanupPolicy {
    CountBasedOnly(usize),
}

impl Default for ListenerManagerConfig {
    fn default() -> Self {
        Self {
            max_versions_per_listener: 2,
            cleanup_policy: CleanupPolicy::CountBasedOnly(2),
            cleanup_interval: Duration::from_secs(60),
        }
    }
}

struct ListenerInfo {
    handle: abort_on_drop::ChildTask<()>,
    listener_conf: ListenerConfig,
    version: u64,
}
impl ListenerInfo {
    fn new(handle: tokio::task::JoinHandle<()>, listener_conf: ListenerConfig, version: u64) -> Self {
        Self { handle: handle.into(), listener_conf, version }
    }
}

pub struct ListenersManager {
    listener_configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
    route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
    listener_handles: MultiMap<String, ListenerInfo>,
    version_counter: u64,
    config: ListenerManagerConfig,
}

impl ListenersManager {
    pub fn new(
        listener_configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
        route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
    ) -> Self {
        Self::with_config(listener_configuration_channel, route_configuration_channel, ListenerManagerConfig::default())
    }

    pub fn with_config(
        listener_configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
        route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
        config: ListenerManagerConfig,
    ) -> Self {
        ListenersManager {
            listener_configuration_channel,
            route_configuration_channel,
            listener_handles: MultiMap::new(),
            version_counter: 0,
            config,
        }
    }

    pub async fn start(mut self, ct: tokio_util::sync::CancellationToken) -> Result<()> {
        let (tx_secret_updates, _) = broadcast::channel(16);
        let (tx_route_updates, _) = broadcast::channel(16);
        // TODO: create child token for each listener?
        loop {
            tokio::select! {
                Some(listener_configuration_change) = self.listener_configuration_channel.recv() => {
                    match listener_configuration_change {
                        ListenerConfigurationChange::Added(boxed) => {
                            let (factory, listener_conf) = *boxed;
                            let listener = factory.clone()
                                .make_listener(tx_route_updates.subscribe(), tx_secret_updates.subscribe())?;
                            if let Err(e) = self.start_listener(listener, listener_conf) {
                                warn!("Failed to start listener: {e}");
                            }
                        }
                        ListenerConfigurationChange::Removed(listener_name) => {
                            let _ = self.stop_listener(&listener_name);
                        },
                        ListenerConfigurationChange::TlsContextChanged((secret_id, secret)) => {
                            info!("Got tls secret update {secret_id}");
                            let res = tx_secret_updates.send(TlsContextChange::Updated((secret_id, secret)));
                            if let Err(e) = res{
                                warn!("Internal problem when updating a secret: {e}");
                            }
                        },
                        ListenerConfigurationChange::GetConfiguration(config_dump_tx) => {
                            let listeners: Vec<ListenerConfig> = self.listener_handles
                                .iter()
                                .map(|(_, info)| info.listener_conf.clone())
                                .collect();
                            config_dump_tx.send(ConfigDump { listeners: Some(listeners), ..Default::default() }).await?;
                        },
                    }
                },
                Some(route_configuration_change) = self.route_configuration_channel.recv() => {
                    // routes could be CachedWatch instead, as they are evaluated lazilly
                    let res = tx_route_updates.send(route_configuration_change);
                    if let Err(e) = res{
                        warn!("Internal problem when updating a route: {e}");
                    }
                },
                _ = ct.cancelled() => {
                    warn!("Listener manager exiting");
                    return Ok(());
                }
            }
        }
    }

    pub fn start_listener(&mut self, listener: Listener, listener_conf: ListenerConfig) -> Result<()> {
        let listener_name = listener.get_name().to_string();
        let (addr, dev) = listener.get_socket();
        info!("Listener {} at {addr} (device bind:{})", listener_name, dev.is_some());

        self.version_counter += 1;
        let version = self.version_counter;

        let listener_name_for_async = listener_name.clone();
        let join_handle = tokio::spawn(async move {
            let error = listener.start().await;
            info!("Listener {} version {} exited: {}", listener_name_for_async, version, error);
        });

        let listener_info = ListenerInfo::new(join_handle, listener_conf, version);

        self.listener_handles.insert(listener_name.clone(), listener_info);
        let version_count = self.listener_handles.get_vec(&listener_name).map(|v| v.len()).unwrap_or(0);
        info!("Started version {} of listener {} ({} total active version(s))", version, listener_name, version_count);

        self.cleanup_old_versions(&listener_name);

        Ok(())
    }

    pub fn stop_listener(&mut self, listener_name: &str) -> Result<()> {
        if let Some(listeners) = self.listener_handles.remove(listener_name) {
            info!("Stopping all {} version(s) of listener {}", listeners.len(), listener_name);
            for listener_info in listeners {
                info!("Stopping listener {} version {}", listener_name, listener_info.version);
                listener_info.handle.abort();
            }
        } else {
            info!("No listeners found with name {}", listener_name);
        }

        Ok(())
    }

    fn cleanup_old_versions(&mut self, listener_name: &str) {
        if let Some(mut versions) = self.listener_handles.remove(listener_name) {
            let original_count = versions.len();

            match &self.config.cleanup_policy {
                CleanupPolicy::CountBasedOnly(max_count) => {
                    if versions.len() > *max_count {
                        let to_remove = versions.len() - max_count;
                        let removed = versions.drain(0..to_remove).collect::<Vec<_>>();
                        for old in removed {
                            info!("Cleaning up old listener {} version {} (count limit)", listener_name, old.version);
                        }
                    }
                },
            }

            // Re-insert the remaining versions
            for version in versions {
                self.listener_handles.insert(listener_name.to_string(), version);
            }

            let remaining_count = self.listener_handles.get_vec(listener_name).map(|v| v.len()).unwrap_or(0);
            if original_count != remaining_count {
                info!(
                    "Cleaned up {} old version(s) of listener {}, {} remaining",
                    original_count - remaining_count,
                    listener_name,
                    remaining_count
                );
            }
        }
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

    fn create_test_listener_config(name: &str, port: u16) -> ListenerConfig {
        ListenerConfig {
            name: name.into(),
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            filter_chains: HashMap::default(),
            bind_device: None,
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
        }
    }

    #[traced_test]
    #[tokio::test]
    async fn start_listener_dup() {
        let chan = 10;
        let name = "testlistener";

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let mut man = ListenersManager::new(conf_rx, route_rx);

        let (routeb_tx1, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx1, secb_rx) = broadcast::channel(chan);
        let l1 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l1_info = ListenerConfig {
            name: name.into(),
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234),
            filter_chains: HashMap::default(),
            bind_device: None,
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
        };
        man.start_listener(l1, l1_info.clone()).unwrap();
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        let (routeb_tx2, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx2, secb_rx) = broadcast::channel(chan);
        let l2 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l2_info = l1_info;
        man.start_listener(l2, l2_info).unwrap();
        assert!(routeb_tx2.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        // Both listeners should still be active (multiple versions allowed)
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        assert!(routeb_tx2.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());

        assert_eq!(man.listener_handles.get_vec(name).unwrap().len(), 2);
        tokio::task::yield_now().await;
    }

    #[traced_test]
    #[tokio::test]
    async fn start_listener_shutdown() {
        let chan = 10;
        let name = "my-listener";

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let mut man = ListenersManager::new(conf_rx, route_rx);

        let (routeb_tx1, routeb_rx) = broadcast::channel(chan);
        let (secb_tx1, secb_rx) = broadcast::channel(chan);
        let l1 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l1_info = ListenerConfig {
            name: name.into(),
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234),
            filter_chains: HashMap::default(),
            bind_device: None,
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
        };
        man.start_listener(l1, l1_info).unwrap();

        drop(routeb_tx1);
        drop(secb_tx1);
        tokio::task::yield_now().await;

        // See .start_listener() - in the case all channels are dropped the task there
        // should exit with this warning msg
        let expected = format!("Listener {name} version 1 exited: channel closed");
        logs_assert(|lines: &[&str]| {
            let logs: Vec<_> = lines.iter().filter(|ln| ln.contains(&expected)).collect();
            if logs.len() == 1 {
                Ok(())
            } else {
                Err(format!("Expecting 1 log line for listener shutdown (got {})", logs.len()))
            }
        });
    }

    #[traced_test]
    #[tokio::test]
    async fn start_multiple_listener_versions() {
        let chan = 10;
        let name = "multi-version-listener";

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let config = ListenerManagerConfig {
            max_versions_per_listener: 2,
            cleanup_policy: CleanupPolicy::CountBasedOnly(2),
            cleanup_interval: Duration::from_secs(60),
        };
        let mut man = ListenersManager::with_config(conf_rx, route_rx, config);

        let (routeb_tx1, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx1, secb_rx) = broadcast::channel(chan);
        let l1 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l1_info = create_test_listener_config(name, 1234);
        man.start_listener(l1, l1_info).unwrap();
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        let (routeb_tx2, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx2, secb_rx) = broadcast::channel(chan);
        let l2 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l2_info = create_test_listener_config(name, 1235);
        man.start_listener(l2, l2_info).unwrap();
        assert!(routeb_tx2.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        let (routeb_tx3, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx3, secb_rx) = broadcast::channel(chan);
        let l3 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l3_info = create_test_listener_config(name, 1236);
        man.start_listener(l3, l3_info).unwrap();
        assert!(routeb_tx3.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        // After adding 3rd listener, first should be cleaned up due to max_versions_per_listener = 2
        // So routeb_tx1 should be closed (is_err), but routeb_tx2 and routeb_tx3 should work
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_err());
        assert!(routeb_tx2.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        assert!(routeb_tx3.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());

        // Should only have 2 versions due to cleanup policy (max_count: 2)
        assert_eq!(man.listener_handles.get_vec(name).unwrap().len(), 2);

        man.stop_listener(name).unwrap();

        assert!(man.listener_handles.get_vec(name).is_none());

        tokio::task::yield_now().await;
    }

    #[traced_test]
    #[tokio::test]
    async fn test_cleanup_policy_enforcement() {
        let chan = 10;
        let name = "cleanup-test-listener";

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let config = ListenerManagerConfig {
            max_versions_per_listener: 3,
            cleanup_policy: CleanupPolicy::CountBasedOnly(3),
            cleanup_interval: Duration::from_secs(60),
        };
        let mut man = ListenersManager::with_config(conf_rx, route_rx, config);

        // Add 5 listeners, should only keep 3 due to cleanup policy
        for i in 1..=5 {
            let (_routeb_tx, routeb_rx) = broadcast::channel(chan);
            let (_secb_tx, secb_rx) = broadcast::channel(chan);
            let listener = Listener::test_listener(name, routeb_rx, secb_rx);
            let listener_info = create_test_listener_config(name, 1230 + i);
            man.start_listener(listener, listener_info).unwrap();
            tokio::task::yield_now().await;
        }

        // Should only have 3 versions due to cleanup policy
        assert_eq!(man.listener_handles.get_vec(name).unwrap().len(), 3);

        man.stop_listener(name).unwrap();
        assert!(man.listener_handles.get_vec(name).is_none());

        tokio::task::yield_now().await;
    }
}
