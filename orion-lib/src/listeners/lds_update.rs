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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::listeners::listener::Listener;
use orion_configuration::config::Listener as ListenerConfig;

#[derive(Debug, Clone, PartialEq)]
pub enum ListenerState {
    Active,
    Draining { started_at: std::time::Instant },
}

#[derive(Debug)]
pub struct LdsListenerInfo {
    pub handle: tokio::task::JoinHandle<crate::Error>,
    pub config: ListenerConfig,
    pub state: ListenerState,
    pub created_at: std::time::Instant,
}

impl LdsListenerInfo {
    pub fn new(handle: tokio::task::JoinHandle<crate::Error>, config: ListenerConfig) -> Self {
        Self { handle, config, state: ListenerState::Active, created_at: std::time::Instant::now() }
    }

    pub fn start_draining(&mut self) {
        self.state = ListenerState::Draining { started_at: std::time::Instant::now() };
    }

    pub fn is_draining(&self) -> bool {
        matches!(self.state, ListenerState::Draining { .. })
    }
}

pub struct LdsManager {
    listeners: Arc<RwLock<MultiMap<String, LdsListenerInfo>>>,
    drain_timeout: Duration,
}

impl LdsManager {
    pub fn new() -> Self {
        Self { listeners: Arc::new(RwLock::new(MultiMap::new())), drain_timeout: Duration::from_secs(600) }
    }

    pub async fn handle_lds_update(
        &self,
        listener: Listener,
        config: ListenerConfig,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener_name = config.name.to_string();
        let mut listeners = self.listeners.write().await;

        if let Some(existing_versions) = listeners.get_vec_mut(&listener_name) {
            info!("LDS: Updating existing listener '{}' with {} versions", listener_name, existing_versions.len());

            for existing in existing_versions {
                if !existing.is_draining() {
                    existing.start_draining();
                    info!("LDS: Old version of listener '{}' placed in draining state", listener_name);
                }
            }

            self.start_drain_timeout_for_existing(&listener_name);
        } else {
            info!("LDS: Adding new listener '{}'", listener_name);
        }

        let handle = tokio::spawn(async move { listener.start().await });

        let new_listener_info = LdsListenerInfo::new(handle, config);
        listeners.insert(listener_name.clone(), new_listener_info);

        info!("LDS: Listener '{}' successfully updated", listener_name);
        Ok(())
    }

    pub async fn remove_listener(&self, listener_name: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut listeners = self.listeners.write().await;

        if let Some(versions) = listeners.get_vec_mut(listener_name) {
            info!("LDS: Removing listener '{}' with {} versions", listener_name, versions.len());

            for listener_info in versions {
                if !listener_info.is_draining() {
                    listener_info.start_draining();
                    info!("LDS: Version of listener '{}' placed in draining state for removal", listener_name);
                }
            }

            self.start_drain_timeout_for_removal(listener_name);

            Ok(())
        } else {
            warn!("LDS: Attempted to remove non-existent listener '{}'", listener_name);
            Ok(())
        }
    }

    fn start_drain_timeout_for_existing(&self, listener_name: &str) {
        let listeners = self.listeners.clone();
        let timeout = self.drain_timeout;
        let name = listener_name.to_owned();

        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;

            let mut listeners_guard = listeners.write().await;
            if let Some(versions) = listeners_guard.get_vec_mut(&name) {
                versions.iter_mut().filter(|listener_info| listener_info.is_draining()).for_each(|listener_info| {
                    listener_info.handle.abort();
                    info!("LDS: Draining version of listener '{}' forcibly closed after timeout", name);
                });
                versions.retain(|listener_info| !listener_info.is_draining());
                if versions.is_empty() {
                    listeners_guard.remove(&name);
                }
            }
        });
    }

    fn start_drain_timeout_for_removal(&self, listener_name: &str) {
        let listeners = self.listeners.clone();
        let timeout = self.drain_timeout;
        let name = listener_name.to_owned();

        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;

            let mut listeners_guard = listeners.write().await;
            if let Some(versions) = listeners_guard.remove(&name) {
                for listener_info in versions {
                    listener_info.handle.abort();
                    info!("LDS: Listener '{}' forcibly closed after drain timeout during removal", name);
                }
            }
        });
    }

    pub async fn get_listener_info(&self, name: &str) -> Option<ListenerState> {
        let listeners = self.listeners.read().await;
        listeners.get_vec(name)?.last().map(|info| info.state.clone())
    }

    pub async fn list_listeners(&self) -> HashMap<String, Vec<ListenerState>> {
        let listeners = self.listeners.read().await;
        listeners
            .iter_all()
            .map(|(name, versions)| {
                let states = versions.iter().map(|info| info.state.clone()).collect();
                (name.clone(), states)
            })
            .collect()
    }
}

impl Default for LdsManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn test_lds_listener_update() {
        let manager = LdsManager::new();

        let config = ListenerConfig {
            name: "test-listener".to_string().into(),
            address: "127.0.0.1:8080".parse().unwrap(),
            filter_chains: Default::default(),
            bind_device: None,
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
            drain_type: orion_configuration::config::listener::DrainType::Default,
            version_info: None,
        };

        let (route_tx, route_rx) = broadcast::channel(10);
        let (sec_tx, sec_rx) = broadcast::channel(10);
        let listener = Listener::test_listener("test-listener", route_rx, sec_rx);

        manager.handle_lds_update(listener, config.clone()).await.unwrap();

        let state = manager.get_listener_info("test-listener").await.unwrap();
        assert_eq!(state, ListenerState::Active);

        let (route_tx2, route_rx2) = broadcast::channel(10);
        let (sec_tx2, sec_rx2) = broadcast::channel(10);
        let listener2 = Listener::test_listener("test-listener", route_rx2, sec_rx2);

        let mut config2 = config;
        config2.address = "127.0.0.1:8081".parse().unwrap();

        manager.handle_lds_update(listener2, config2).await.unwrap();

        let state = manager.get_listener_info("test-listener").await.unwrap();
        assert_eq!(state, ListenerState::Active);

        drop(route_tx);
        drop(sec_tx);
        drop(route_tx2);
        drop(sec_tx2);
    }
}
