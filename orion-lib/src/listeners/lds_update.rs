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

use std::time::Duration;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

use orion_configuration::config::Listener as ListenerConfig;
use crate::listeners::{Listener, ListenerInfo};

#[derive(Debug, Clone, PartialEq)]
pub enum ListenerState {
    Active,
    Draining { started_at: std::time::Instant },
}

#[derive(Debug)]
pub struct LdsListenerInfo {
    pub handle: tokio::task::JoinHandle<String>,
    pub config: ListenerConfig,
    pub state: ListenerState,
    pub created_at: std::time::Instant,
}

impl LdsListenerInfo {
    pub fn new(handle: tokio::task::JoinHandle<String>, config: ListenerConfig) -> Self {
        Self {
            handle,
            config,
            state: ListenerState::Active,
            created_at: std::time::Instant::now(),
        }
    }

    pub fn start_draining(&mut self) {
        self.state = ListenerState::Draining {
            started_at: std::time::Instant::now(),
        };
    }

    pub fn is_draining(&self) -> bool {
        matches!(self.state, ListenerState::Draining { .. })
    }
}

pub struct LdsManager {
    listeners: Arc<RwLock<HashMap<String, LdsListenerInfo>>>,
    drain_timeout: Duration,
}

impl LdsManager {
    pub fn new() -> Self {
        Self {
            listeners: Arc::new(RwLock::new(HashMap::new())),
            drain_timeout: Duration::from_secs(600),
        }
    }

    pub async fn handle_lds_update(
        &self,
        listener: Listener,
        config: ListenerConfig,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener_name = config.name.clone();
        let mut listeners = self.listeners.write().await;

        if let Some(existing) = listeners.get_mut(&listener_name) {
            info!("LDS: Updating existing listener '{}'", listener_name);
            
            existing.start_draining();
            info!("LDS: Old listener '{}' placed in draining state", listener_name);

            self.start_drain_timeout(&listener_name).await;
        } else {
            info!("LDS: Adding new listener '{}'", listener_name);
        }

        let handle = tokio::spawn(async move {
            listener.start().await
        });

        let new_listener_info = LdsListenerInfo::new(handle, config);
        listeners.insert(listener_name.clone(), new_listener_info);

        info!("LDS: Listener '{}' successfully updated", listener_name);
        Ok(())
    }

    pub async fn remove_listener(&self, listener_name: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut listeners = self.listeners.write().await;
        
        if let Some(mut listener_info) = listeners.remove(listener_name) {
            info!("LDS: Removing listener '{}'", listener_name);
            
            listener_info.start_draining();
            info!("LDS: Listener '{}' placed in draining state for removal", listener_name);

            self.start_drain_timeout(listener_name).await;
            
            Ok(())
        } else {
            warn!("LDS: Attempted to remove non-existent listener '{}'", listener_name);
            Ok(())
        }
    }

    async fn start_drain_timeout(&self, listener_name: &str) {
        let listeners = self.listeners.clone();
        let timeout = self.drain_timeout;
        let name = listener_name.to_string();

        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            
            let mut listeners_guard = listeners.write().await;
            if let Some(listener_info) = listeners_guard.get(&name) {
                if listener_info.is_draining() {
                    info!("LDS: Drain timeout reached, forcibly closing listener '{}'", name);
                    
                    if let Some(mut listener_info) = listeners_guard.remove(&name) {
                        listener_info.handle.abort();
                        info!("LDS: Listener '{}' forcibly closed after drain timeout", name);
                    }
                }
            }
        });
    }

    pub async fn get_listener_info(&self, name: &str) -> Option<ListenerState> {
        let listeners = self.listeners.read().await;
        listeners.get(name).map(|info| info.state.clone())
    }

    pub async fn list_listeners(&self) -> HashMap<String, ListenerState> {
        let listeners = self.listeners.read().await;
        listeners
            .iter()
            .map(|(name, info)| (name.clone(), info.state.clone()))
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
            name: "test-listener".to_string(),
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
