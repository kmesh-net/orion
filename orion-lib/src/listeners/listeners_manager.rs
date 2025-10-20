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
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

use orion_configuration::config::{
    listener::ListenerAddress, network_filters::http_connection_manager::RouteConfiguration, Listener as ListenerConfig,
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
}

impl ListenersManager {
    pub fn new(
        listener_configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
        route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
    ) -> Self {
        ListenersManager {
            listener_configuration_channel,
            route_configuration_channel,
            listener_handles: MultiMap::new(),
            version_counter: 0,
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
        if let Some((addr, dev)) = listener.get_socket() {
            info!("Listener {} at {addr} (device bind:{})", listener_name, dev.is_some());
        } else {
            info!("Internal listener {}", listener_name);
        }

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

        Ok(())
    }

    pub fn stop_listener(&mut self, listener_name: &str) -> Result<()> {
        if let Some(listeners) = self.listener_handles.get_vec_mut(listener_name) {
            info!("Stopping all {} version(s) of listener {}", listeners.len(), listener_name);
            for listener_info in listeners.drain(..) {
                info!("Stopping listener {} version {}", listener_name, listener_info.version);
                listener_info.handle.abort();
            }
            self.listener_handles.remove(listener_name);
        } else {
            info!("No listeners found with name {}", listener_name);
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
    use orion_configuration::config::{transport::BindDeviceOptions, Listener as ListenerConfig};
    use tracing_test::traced_test;

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
            address: orion_configuration::config::listener::ListenerAddress::Socket(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1234,
            )),
            filter_chains: HashMap::default(),
            bind_device_options: BindDeviceOptions::default(),
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
            address: orion_configuration::config::listener::ListenerAddress::Socket(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1234,
            )),
            filter_chains: HashMap::default(),
            bind_device_options: BindDeviceOptions::default(),
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
        let mut man = ListenersManager::new(conf_rx, route_rx);

        let (routeb_tx1, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx1, secb_rx) = broadcast::channel(chan);
        let l1 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l1_info = ListenerConfig {
            name: name.into(),
            address: ListenerAddress::Socket(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234)),
            filter_chains: HashMap::default(),
            bind_device_options: BindDeviceOptions::default(),
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
        };
        man.start_listener(l1, l1_info).unwrap();
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        let (routeb_tx2, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx2, secb_rx) = broadcast::channel(chan);
        let l2 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l2_info = ListenerConfig {
            name: name.into(),
            address: ListenerAddress::Socket(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1235)), // Different port
            filter_chains: HashMap::default(),
            bind_device_options: BindDeviceOptions::default(),
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
        };
        man.start_listener(l2, l2_info).unwrap();
        assert!(routeb_tx2.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        let (routeb_tx3, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx3, secb_rx) = broadcast::channel(chan);
        let l3 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l3_info = ListenerConfig {
            name: name.into(),
            address: ListenerAddress::Socket(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1236)), // Different port
            filter_chains: HashMap::default(),
            bind_device_options: BindDeviceOptions::default(),
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
        };
        man.start_listener(l3, l3_info).unwrap();
        assert!(routeb_tx3.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        assert!(routeb_tx2.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        assert!(routeb_tx3.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());

        assert_eq!(man.listener_handles.get_vec(name).unwrap().len(), 3);

        man.stop_listener(name).unwrap();

        assert!(man.listener_handles.get_vec(name).is_none());

        tokio::task::yield_now().await;
    }
}
