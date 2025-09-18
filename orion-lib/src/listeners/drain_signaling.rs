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

use crate::{Error, Result};
use orion_configuration::config::listener::DrainType as ConfigDrainType;
use pingora_timeout::fast_timeout::fast_timeout;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainScenario {
    HealthCheckFail,
    ListenerUpdate,
    HotRestart,
}

impl DrainScenario {
    pub fn should_drain(self, drain_type: ConfigDrainType) -> bool {
        match (self, drain_type) {
            (_, ConfigDrainType::Default)
            | (DrainScenario::ListenerUpdate | DrainScenario::HotRestart, ConfigDrainType::ModifyOnly) => true,
            (DrainScenario::HealthCheckFail, ConfigDrainType::ModifyOnly) => false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DrainStrategy {
    Tcp { global_timeout: Duration },
    Http { global_timeout: Duration, drain_timeout: Duration },
    Immediate,
    Gradual,
}

#[derive(Debug, Clone)]
pub struct ListenerDrainState {
    pub started_at: Instant,
    pub strategy: super::listeners_manager::DrainStrategy,
    pub protocol_behavior: super::listeners_manager::ProtocolDrainBehavior,
    pub drain_scenario: DrainScenario,
    pub drain_type: ConfigDrainType,
}

#[derive(Debug)]
pub struct ListenerDrainContext {
    pub listener_id: String,
    pub strategy: DrainStrategy,
    pub drain_start: Instant,
    pub initial_connections: usize,
    pub active_connections: Arc<RwLock<usize>>,
    pub completed: Arc<RwLock<bool>>,
}

impl ListenerDrainContext {
    pub fn new(listener_id: String, strategy: DrainStrategy, initial_connections: usize) -> Self {
        Self {
            listener_id,
            strategy,
            drain_start: Instant::now(),
            initial_connections,
            active_connections: Arc::new(RwLock::new(initial_connections)),
            completed: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn update_connection_count(&self, count: usize) {
        let mut active = self.active_connections.write().await;
        *active = count;

        if count == 0 {
            let mut completed = self.completed.write().await;
            *completed = true;
            debug!("Listener drain completed - all connections closed for {}", self.listener_id);
        }
    }

    pub async fn is_completed(&self) -> bool {
        *self.completed.read().await
    }

    pub async fn get_active_connections(&self) -> usize {
        *self.active_connections.read().await
    }

    pub fn is_timeout_exceeded(&self) -> bool {
        let global_timeout = match &self.strategy {
            DrainStrategy::Tcp { global_timeout } | DrainStrategy::Http { global_timeout, .. } => *global_timeout,
            DrainStrategy::Immediate => Duration::from_secs(0),
            DrainStrategy::Gradual => Duration::from_secs(600),
        };

        self.drain_start.elapsed() >= global_timeout
    }

    pub fn get_http_drain_timeout(&self) -> Option<Duration> {
        match &self.strategy {
            DrainStrategy::Http { drain_timeout, .. } => Some(*drain_timeout),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct DrainSignalingManager {
    drain_contexts: Arc<RwLock<HashMap<String, Arc<ListenerDrainContext>>>>,
    global_drain_timeout: Duration,
    default_http_drain_timeout: Duration,
    listener_drain_state: Arc<RwLock<Option<ListenerDrainState>>>,
}

impl DrainSignalingManager {
    pub fn new() -> Self {
        Self {
            drain_contexts: Arc::new(RwLock::new(HashMap::new())),
            global_drain_timeout: Duration::from_secs(600),
            default_http_drain_timeout: Duration::from_millis(5000),
            listener_drain_state: Arc::new(RwLock::new(None)),
        }
    }

    pub fn with_timeouts(global_drain_timeout: Duration, default_http_drain_timeout: Duration) -> Self {
        Self {
            drain_contexts: Arc::new(RwLock::new(HashMap::new())),
            global_drain_timeout,
            default_http_drain_timeout,
            listener_drain_state: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn start_listener_draining(&self, drain_state: ListenerDrainState) {
        if !drain_state.drain_scenario.should_drain(drain_state.drain_type) {
            debug!(
                "Skipping drain for scenario {:?} with drain_type {:?}",
                drain_state.drain_scenario, drain_state.drain_type
            );
            return;
        }

        info!("Starting listener-wide draining with strategy {:?}", drain_state.strategy);
        let mut state = self.listener_drain_state.write().await;
        *state = Some(drain_state);
    }

    pub async fn stop_listener_draining(&self) {
        info!("Stopping listener-wide draining");
        let mut state = self.listener_drain_state.write().await;
        *state = None;

        let mut contexts = self.drain_contexts.write().await;
        contexts.clear();
    }

    pub async fn is_listener_draining(&self) -> bool {
        self.listener_drain_state.read().await.is_some()
    }

    pub async fn apply_http1_drain_signal<B>(&self, response: &mut hyper::Response<B>) {
        if let Some(drain_state) = &*self.listener_drain_state.read().await {
            match &drain_state.protocol_behavior {
                super::listeners_manager::ProtocolDrainBehavior::Http1 { connection_close: true }
                | super::listeners_manager::ProtocolDrainBehavior::Auto => {
                    use hyper::header::{HeaderValue, CONNECTION};
                    response.headers_mut().insert(CONNECTION, HeaderValue::from_static("close"));
                    debug!("Applied 'Connection: close' header for HTTP/1.1 drain signaling");
                },
                _ => {
                    debug!("Skipping Connection: close header for non-HTTP/1.1 protocol");
                },
            }
        }
    }

    pub fn apply_http1_drain_signal_sync<B>(response: &mut hyper::Response<B>) -> bool {
        use hyper::header::{HeaderValue, CONNECTION};
        response.headers_mut().insert(CONNECTION, HeaderValue::from_static("close"));
        debug!("Applied 'Connection: close' header for HTTP/1.1 drain signaling");
        true
    }

    pub async fn get_http2_drain_timeout(&self, listener_id: &str) -> Option<Duration> {
        let contexts = self.drain_contexts.read().await;
        if let Some(context) = contexts.get(listener_id) {
            context.get_http_drain_timeout()
        } else {
            Some(self.default_http_drain_timeout)
        }
    }

    pub async fn initiate_listener_drain(
        &self,
        listener_id: String,
        is_http: bool,
        http_drain_timeout: Option<Duration>,
        active_connections: usize,
    ) -> Result<Arc<ListenerDrainContext>> {
        let strategy = if is_http {
            DrainStrategy::Http {
                global_timeout: self.global_drain_timeout,
                drain_timeout: http_drain_timeout.unwrap_or(self.default_http_drain_timeout),
            }
        } else {
            DrainStrategy::Tcp { global_timeout: self.global_drain_timeout }
        };

        let context = Arc::new(ListenerDrainContext::new(listener_id.clone(), strategy.clone(), active_connections));

        {
            let mut contexts = self.drain_contexts.write().await;
            contexts.insert(listener_id.clone(), context.clone());
        }

        info!(
            "Initiated listener draining for {}, strategy: {:?}, active_connections: {}",
            listener_id, strategy, active_connections
        );

        let context_clone = context.clone();
        let manager_clone = self.clone();
        let listener_id_clone = listener_id.clone();
        tokio::spawn(async move {
            let () = manager_clone.monitor_drain_progress(context_clone, listener_id_clone).await;
        });

        Ok(context)
    }

    async fn monitor_drain_progress(&self, context: Arc<ListenerDrainContext>, listener_id: String) {
        let check_interval = Duration::from_secs(1);

        loop {
            sleep(check_interval).await;

            if context.is_completed().await {
                self.complete_drain(listener_id.clone()).await;
                return;
            }

            if context.is_timeout_exceeded() {
                let elapsed = context.drain_start.elapsed();
                let active_connections = context.get_active_connections().await;
                warn!(
                    "Global drain timeout exceeded for listener {}, elapsed: {:?}, active_connections: {}",
                    listener_id, elapsed, active_connections
                );
                self.force_complete_drain(listener_id.clone()).await;
                return;
            }

            let elapsed = context.drain_start.elapsed();
            let active_connections = context.get_active_connections().await;
            debug!(
                "Drain progress check for listener {}, elapsed: {:?}, active_connections: {}",
                listener_id, elapsed, active_connections
            );
        }
    }

    async fn complete_drain(&self, listener_id: String) {
        let mut contexts = self.drain_contexts.write().await;
        if let Some(context) = contexts.remove(&listener_id) {
            let duration = context.drain_start.elapsed();
            info!("Listener drain completed successfully for {}, duration: {:?}", listener_id, duration);
        }
    }

    async fn force_complete_drain(&self, listener_id: String) {
        let mut contexts = self.drain_contexts.write().await;
        if let Some(context) = contexts.remove(&listener_id) {
            let mut completed = context.completed.write().await;
            *completed = true;
            let duration = context.drain_start.elapsed();
            warn!("Listener drain force completed due to timeout for {}, duration: {:?}", listener_id, duration);
        }
    }

    pub async fn get_drain_context(&self, listener_id: &str) -> Option<Arc<ListenerDrainContext>> {
        let contexts = self.drain_contexts.read().await;
        contexts.get(listener_id).cloned()
    }

    pub async fn has_draining_listeners(&self) -> bool {
        let contexts = self.drain_contexts.read().await;
        !contexts.is_empty()
    }

    pub async fn get_draining_listeners(&self) -> Vec<String> {
        let contexts = self.drain_contexts.read().await;
        contexts.keys().cloned().collect()
    }

    pub async fn wait_for_drain_completion(&self, timeout_duration: Duration) -> Result<()> {
        let result = fast_timeout(timeout_duration, async {
            loop {
                if !self.has_draining_listeners().await {
                    break;
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await;

        if let Ok(()) = result {
            info!("All listener draining completed successfully");
            Ok(())
        } else {
            let draining = self.get_draining_listeners().await;
            warn!("Timeout waiting for drain completion, draining_listeners: {:?}", draining);
            Err(Error::new("Timeout waiting for listener drain completion"))
        }
    }
}

impl Clone for DrainSignalingManager {
    fn clone(&self) -> Self {
        Self {
            drain_contexts: self.drain_contexts.clone(),
            global_drain_timeout: self.global_drain_timeout,
            default_http_drain_timeout: self.default_http_drain_timeout,
            listener_drain_state: self.listener_drain_state.clone(),
        }
    }
}

impl Default for DrainSignalingManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct DefaultConnectionHandler {}

impl DefaultConnectionHandler {
    pub fn new() -> Self {
        Self {}
    }

    pub fn register_connection(
        _connection_id: String,
        _protocol: super::listeners_manager::ConnectionProtocol,
        _peer_addr: std::net::SocketAddr,
    ) {
    }
}

impl Default for DefaultConnectionHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_tcp_drain_context() {
        let strategy = DrainStrategy::Tcp { global_timeout: Duration::from_secs(1) };
        let context = ListenerDrainContext::new("test-tcp".to_string(), strategy, 5);

        assert_eq!(context.get_active_connections().await, 5);
        assert!(!context.is_completed().await);

        context.update_connection_count(0).await;
        assert!(context.is_completed().await);
    }

    #[tokio::test]
    async fn test_http_drain_context() {
        let strategy = DrainStrategy::Http {
            global_timeout: Duration::from_secs(600),
            drain_timeout: Duration::from_millis(5000),
        };
        let context = ListenerDrainContext::new("test-http".to_string(), strategy, 3);

        assert_eq!(context.get_active_connections().await, 3);
        assert!(!context.is_completed().await);
        assert_eq!(context.get_http_drain_timeout(), Some(Duration::from_millis(5000)));
        assert!(!context.is_timeout_exceeded());
    }

    #[tokio::test]
    async fn test_drain_manager_basic() {
        let manager = DrainSignalingManager::new();
        assert!(!manager.has_draining_listeners().await);

        let context = manager.initiate_listener_drain("test".to_string(), false, None, 1).await.unwrap();

        assert!(manager.has_draining_listeners().await);
        assert_eq!(manager.get_draining_listeners().await, vec!["test"]);

        context.update_connection_count(0).await;

        sleep(Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_timeout_behavior() {
        let manager = DrainSignalingManager::with_timeouts(Duration::from_millis(50), Duration::from_millis(25));

        let context = manager.initiate_listener_drain("timeout-test".to_string(), true, None, 5).await.unwrap();

        sleep(Duration::from_millis(10)).await;
        sleep(Duration::from_millis(60)).await;
        assert!(context.is_timeout_exceeded());

        let mut attempts = 0;
        while attempts < 20 && !context.is_completed().await {
            sleep(Duration::from_millis(100)).await;
            attempts += 1;
        }

        assert!(context.is_completed().await, "Expected context to be completed after timeout");
        assert!(
            !manager.has_draining_listeners().await,
            "Expected manager to no longer track the listener after timeout"
        );
    }
}
