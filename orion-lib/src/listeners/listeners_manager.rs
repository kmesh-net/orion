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

use std::collections::BTreeMap;

use compact_str::CompactString;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use orion_configuration::config::network_filters::http_connection_manager::RouteConfiguration;

use super::listener::{Listener, ListenerFactory};
use crate::{secrets::TransportSecret, Result};
#[derive(Debug, Clone)]
pub enum ListenerConfigurationChange {
    Added(ListenerFactory),
    Removed(String),
    TlsContextChanged((String, TransportSecret)),
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
}
impl ListenerInfo {
    fn new(handle: tokio::task::JoinHandle<()>) -> Self {
        Self { handle: handle.into() }
    }
}

pub struct ListenersManager {
    configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
    route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
    listener_handles: BTreeMap<CompactString, ListenerInfo>,
}

impl ListenersManager {
    pub fn new(
        configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
        route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
    ) -> Self {
        ListenersManager { configuration_channel, route_configuration_channel, listener_handles: BTreeMap::new() }
    }

    pub async fn start(mut self) -> Result<()> {
        let (tx_secret_updates, _) = broadcast::channel(16);
        let (tx_route_updates, _) = broadcast::channel(16);

        loop {
            tokio::select! {
                Some(listener_configuration_change) = self.configuration_channel.recv() => {
                    match listener_configuration_change {
                        ListenerConfigurationChange::Added(factory) => {
                            let listener = factory
                                .make_listener(tx_route_updates.subscribe(), tx_secret_updates.subscribe())?;
                            if let Err(e) = self.start_listener(listener) {
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
                    }
                },
                Some(route_configuration_change) = self.route_configuration_channel.recv() => {
                    // routes could be CachedWatch instead, as they are evaluated lazilly
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

    pub fn start_listener(&mut self, listener: Listener) -> Result<()> {
        let listener_name = listener.get_name().clone();
        let (addr, dev) = listener.get_socket();
        info!("Listener {} at {addr} (device bind:{})", listener_name, dev.is_some());
        let listener_name_co = listener_name.clone();
        // spawn the task for this listener address, this will spawn additional task per connection
        let join_handle = tokio::spawn(async move {
            let error = listener.start().await;
            warn!("Listener {listener_name_co} exited: {error}");
        });
        #[cfg(debug_assertions)]
        if self.listener_handles.contains_key(&listener_name) {
            debug!("Listener {listener_name} already exists, replacing it");
        }
        // note: join handle gets overwritten here if it already exists.
        // handles are abort on drop so will be aborted, closing the socket
        // but the any tasks spawned within this task, which happens on a per-connection basis,
        // will survive past this point and only get dropped when their session ends
        self.listener_handles.insert(listener_name, ListenerInfo::new(join_handle));

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
    use super::*;
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
        man.start_listener(l1).unwrap();
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        let (routeb_tx2, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx2, secb_rx) = broadcast::channel(chan);
        let l2 = Listener::test_listener(name, routeb_rx, secb_rx);
        man.start_listener(l2).unwrap();
        assert!(routeb_tx2.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        // This should fail because the old listener exited already dropping the rx
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_err());
        // Yield once more just in case more logs can be seen
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
        man.start_listener(l1).unwrap();

        drop(routeb_tx1);
        drop(secb_tx1);
        tokio::task::yield_now().await;

        // See .start_listener() - in the case all channels are dropped the task there
        // should exit with this warning msg
        let expected = format!("Listener {name} exited: channel closed");
        logs_assert(|lines: &[&str]| {
            let logs: Vec<_> = lines.iter().filter(|ln| ln.contains(&expected)).collect();
            if logs.len() == 1 {
                Ok(())
            } else {
                Err(format!("Expecting 1 log line for listener shutdown (got {})", logs.len()))
            }
        });
    }
}
