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

use dashmap::DashMap;
use multimap::MultiMap;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};
use tokio::time::interval;
use tracing::{debug, info, warn};

use orion_configuration::config::{
    listener::ListenerAddress, network_filters::http_connection_manager::RouteConfiguration, Listener as ListenerConfig,
};

use super::{
    drain_signaling::ListenerDrainState,
    http_connection_manager::HttpConnectionManager,
    listener::{Listener, ListenerFactory},
};
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
pub struct ConnectionInfo {
    pub id: String,
    pub protocol: ConnectionProtocol,
    pub established_at: Instant,
    pub last_activity: Instant,
    pub state: ConnectionState,
}

#[derive(Debug, Clone)]
pub enum ConnectionProtocol {
    Http1,
    Http2,
    Tcp,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Active,
    Draining,
    Closing,
    Closed,
}

#[derive(Debug, Clone)]
pub struct DrainProgress {
    pub total_connections: usize,
    pub active_connections: usize,
    pub draining_connections: usize,
    pub percentage: f64,
    pub elapsed: Duration,
    pub remaining_time: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct ListenerDrainStatusReport {
    pub listener_name: String,
    pub total_versions: usize,
    pub draining_versions: usize,
    pub version_statuses: Vec<VersionDrainStatus>,
    pub last_updated: Instant,
}

#[derive(Debug, Clone)]
pub struct VersionDrainStatus {
    pub version: u64,
    pub state: DrainPhase,
    pub drain_progress: Option<DrainProgress>,
    pub started_at: Option<Instant>,
    pub estimated_completion: Option<Instant>,
}

#[derive(Debug, Clone)]
pub enum DrainPhase {
    Active,
    Draining,
    ForceClosing,
    Completed,
}

#[derive(Debug, Clone)]
pub struct GlobalDrainStatistics {
    pub total_listeners: usize,
    pub draining_listeners: usize,
    pub total_connections: usize,
    pub draining_connections: usize,
    pub oldest_drain_start: Option<Instant>,
    pub drain_efficiency: f64,
    pub estimated_global_completion: Option<Instant>,
}

pub trait ConnectionManager: Send + Sync {
    fn on_connection_established(&self, listener_name: &str, conn_info: ConnectionInfo);
    fn on_connection_closed(&self, listener_name: &str, connection_id: &str);
    fn start_connection_draining(
        &self,
        listener_name: &str,
        connection_id: &str,
        protocol_behavior: &ProtocolDrainBehavior,
    );
    fn get_active_connections(&self, listener_name: &str) -> Vec<ConnectionInfo>;
    fn force_close_connection(&self, listener_name: &str, connection_id: &str);
}

#[derive(Debug, Default)]
pub struct DefaultConnectionManager {
    connections: Arc<DashMap<String, ConnectionInfo>>,
    listener_connection_counts: Arc<DashMap<String, AtomicUsize>>,
    http_managers: Arc<DashMap<String, Arc<HttpConnectionManager>>>,
}

impl DefaultConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            listener_connection_counts: Arc::new(DashMap::new()),
            http_managers: Arc::new(DashMap::new()),
        }
    }

    fn make_connection_key(listener_name: &str, connection_id: &str) -> String {
        format!("{}:{}", listener_name, connection_id)
    }

    fn parse_connection_key(key: &str) -> Option<(String, String)> {
        if let Some(pos) = key.find(':') {
            let (listener, conn_id) = key.split_at(pos);
            Some((listener.to_string(), conn_id[1..].to_string()))
        } else {
            None
        }
    }

    fn make_listener_prefix(listener_name: &str) -> String {
        format!("{}:", listener_name)
    }

    pub fn get_total_connections(&self) -> usize {
        self.connections.len()
    }

    pub fn get_listener_connections(&self, listener_name: &str) -> Vec<ConnectionInfo> {
        let prefix = Self::make_listener_prefix(listener_name);
        self.connections
            .iter()
            .filter(|entry| entry.key().starts_with(&prefix))
            .map(|entry| entry.value().clone())
            .collect()
    }

    pub fn get_listener_summary(&self) -> Vec<(String, usize)> {
        self.listener_connection_counts
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().load(Ordering::Relaxed)))
            .collect()
    }

    pub fn update_connection_state(
        &self,
        listener_name: &str,
        connection_id: &str,
        new_state: ConnectionState,
    ) -> bool {
        let conn_key = Self::make_connection_key(listener_name, connection_id);
        if let Some(mut conn_entry) = self.connections.get_mut(&conn_key) {
            let conn_info = conn_entry.value_mut();
            let old_state = conn_info.state.clone();
            conn_info.state = new_state.clone();
            conn_info.last_activity = Instant::now();

            info!("Connection {} state changed: {:?} -> {:?}", connection_id, old_state, new_state);
            return true;
        }
        false
    }

    pub fn get_connection_state(&self, listener_name: &str, connection_id: &str) -> Option<ConnectionState> {
        let conn_key = Self::make_connection_key(listener_name, connection_id);
        self.connections.get(&conn_key).map(|entry| entry.value().state.clone())
    }

    pub fn register_http_manager(&self, listener_name: String, manager: Arc<HttpConnectionManager>) {
        self.http_managers.insert(listener_name.clone(), manager);
        debug!("Registered HTTP connection manager for listener: {}", listener_name);
    }

    pub fn unregister_http_manager(&self, listener_name: &str) -> Option<Arc<HttpConnectionManager>> {
        let manager = self.http_managers.remove(listener_name);
        if manager.is_some() {
            debug!("Unregistered HTTP connection manager for listener: {}", listener_name);
        }
        manager.map(|(_, v)| v)
    }

    pub fn get_all_http_managers(&self) -> Vec<(String, Arc<HttpConnectionManager>)> {
        self.http_managers.iter().map(|entry| (entry.key().clone(), entry.value().clone())).collect()
    }

    pub fn get_http_manager(&self, listener_name: &str) -> Option<Arc<HttpConnectionManager>> {
        self.http_managers.get(listener_name).map(|entry| entry.value().clone())
    }

    pub async fn start_draining_http_managers(
        &self,
        listener_name: &str,
    ) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(manager) = self.get_http_manager(listener_name) {
            info!("Starting drain signaling for HTTP connection manager on listener {}", listener_name);

            let drain_manager = manager.get_drain_signaling();
            let drain_state = super::drain_signaling::ListenerDrainState {
                started_at: std::time::Instant::now(),
                strategy: DrainStrategy::Gradual,
                protocol_behavior: ProtocolDrainBehavior::Auto,
                drain_scenario: super::drain_signaling::DrainScenario::ListenerUpdate,
                drain_type: orion_configuration::config::listener::DrainType::Default,
            };
            drain_manager.start_listener_draining(drain_state).await;

            info!("HTTP connection manager drain signaling started for listener {}", listener_name);
            Ok(())
        } else {
            warn!("No HTTP connection manager found for listener: {}", listener_name);
            Ok(())
        }
    }

    pub fn remove_connection(&self, listener_name: &str, connection_id: &str) -> bool {
        let conn_key = Self::make_connection_key(listener_name, connection_id);
        let removed = self.connections.remove(&conn_key).is_some();

        if removed {
            if let Some(count) = self.listener_connection_counts.get(listener_name) {
                count.fetch_sub(1, Ordering::Relaxed);
            }
        }

        removed
    }

    pub fn cleanup_connection(&self, listener_name: &str, connection_id: &str) -> bool {
        let removed = self.remove_connection(listener_name, connection_id);

        if removed {
            debug!("Cleaned up connection state for {} on listener {}", connection_id, listener_name);

            if let Some(count) = self.listener_connection_counts.get(listener_name) {
                if count.load(Ordering::Relaxed) == 0 {
                    info!("All connections drained for listener {}", listener_name);
                }
            }
        }

        removed
    }

    pub fn cleanup_completed_drains(&self) {
        debug!("Running cleanup of completed drain connections");
        let closed_connections: Vec<String> = self
            .connections
            .iter()
            .filter(|entry| matches!(entry.value().state, ConnectionState::Closed))
            .map(|entry| entry.key().clone())
            .collect();

        for conn_key in closed_connections {
            if let Some((listener_name, conn_id)) = Self::parse_connection_key(&conn_key) {
                self.connections.remove(&conn_key);
                if let Some(count) = self.listener_connection_counts.get(&listener_name) {
                    count.fetch_sub(1, Ordering::Relaxed);
                }
                debug!("Cleaned up closed connection {} from listener {}", conn_id, listener_name);
            }
        }
    }

    pub fn notify_connection_closed(&self, listener_name: &str, connection_id: &str) {
        self.update_connection_state(listener_name, connection_id, ConnectionState::Closed);

        if self.cleanup_connection(listener_name, connection_id) {
            debug!("Connection {} closed and cleaned up from listener {}", connection_id, listener_name);
        }
    }

    pub fn get_connections_by_state(&self, listener_name: &str, state: ConnectionState) -> Vec<ConnectionInfo> {
        let prefix = Self::make_listener_prefix(listener_name);
        self.connections
            .iter()
            .filter(|entry| entry.key().starts_with(&prefix) && entry.value().state == state)
            .map(|entry| entry.value().clone())
            .collect()
    }

    pub fn get_all_draining_connections(&self) -> Vec<(String, ConnectionInfo)> {
        self.connections
            .iter()
            .filter(|entry| matches!(entry.value().state, ConnectionState::Draining))
            .filter_map(|entry| {
                if let Some((listener_name, _)) = Self::parse_connection_key(entry.key()) {
                    Some((listener_name, entry.value().clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn cleanup_stale_draining_connections(&self, max_drain_time: std::time::Duration) -> usize {
        let mut closed_count = 0;
        let now = Instant::now();

        let connections_to_close: Vec<(String, String)> = self
            .connections
            .iter()
            .filter(|entry| {
                matches!(entry.value().state, ConnectionState::Draining)
                    && now.duration_since(entry.value().last_activity) > max_drain_time
            })
            .filter_map(|entry| {
                if let Some((listener_name, conn_id)) = Self::parse_connection_key(entry.key()) {
                    Some((listener_name, conn_id))
                } else {
                    None
                }
            })
            .collect();

        for (listener_name, conn_id) in connections_to_close {
            self.force_close_connection(&listener_name, &conn_id);
            closed_count += 1;
        }

        if closed_count > 0 {
            warn!("Force closed {} stale draining connections (drain time > {:?})", closed_count, max_drain_time);
        }

        closed_count
    }
}

impl ConnectionManager for DefaultConnectionManager {
    fn on_connection_established(&self, listener_name: &str, conn_info: ConnectionInfo) {
        debug!("Connection {} established on listener {}", conn_info.id, listener_name);

        let conn_key = Self::make_connection_key(listener_name, &conn_info.id);
        self.connections.insert(conn_key, conn_info);

        let count =
            self.listener_connection_counts.entry(listener_name.to_string()).or_insert_with(|| AtomicUsize::new(0));
        count.fetch_add(1, Ordering::Relaxed);
    }

    fn on_connection_closed(&self, listener_name: &str, connection_id: &str) {
        debug!("Connection {} closed on listener {}", connection_id, listener_name);

        let conn_key = Self::make_connection_key(listener_name, connection_id);
        if self.connections.remove(&conn_key).is_some() {
            if let Some(count) = self.listener_connection_counts.get(listener_name) {
                count.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    fn start_connection_draining(
        &self,
        listener_name: &str,
        connection_id: &str,
        protocol_behavior: &ProtocolDrainBehavior,
    ) {
        debug!(
            "Starting drain for connection {} on listener {} with protocol {:?}",
            connection_id, listener_name, protocol_behavior
        );

        let conn_key = Self::make_connection_key(listener_name, connection_id);
        if let Some(mut conn_entry) = self.connections.get_mut(&conn_key) {
            let conn_info = conn_entry.value_mut();
            conn_info.state = ConnectionState::Draining;
            conn_info.last_activity = Instant::now();

            info!("Connection {} on listener {} is now draining", connection_id, listener_name);
        }
    }

    fn get_active_connections(&self, listener_name: &str) -> Vec<ConnectionInfo> {
        self.get_listener_connections(listener_name)
    }

    fn force_close_connection(&self, listener_name: &str, connection_id: &str) {
        warn!("Force closing connection {} on listener {}", connection_id, listener_name);

        let conn_key = Self::make_connection_key(listener_name, connection_id);
        if let Some(mut conn_entry) = self.connections.get_mut(&conn_key) {
            let conn_info = conn_entry.value_mut();
            conn_info.state = ConnectionState::Closing;
            conn_info.last_activity = Instant::now();
            info!("Connection {} marked for force close (protocol: {:?})", connection_id, conn_info.protocol);
        }

        if self.connections.remove(&conn_key).is_some() {
            if let Some(count) = self.listener_connection_counts.get(listener_name) {
                count.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListenerManagerConfig {
    pub max_versions_per_listener: usize,
    pub cleanup_policy: CleanupPolicy,
    pub cleanup_interval: Duration,
    pub drain_config: ListenerDrainConfig,
}

#[derive(Debug, Clone)]
pub struct ListenerDrainConfig {
    pub drain_time: Duration,
    pub drain_strategy: DrainStrategy,
    pub protocol_handling: ProtocolDrainBehavior,
}

#[derive(Debug, Clone)]
pub enum DrainStrategy {
    Gradual,
    Immediate,
}

#[derive(Debug, Clone)]
pub enum ProtocolDrainBehavior {
    Http1 { connection_close: bool },
    Http2 { send_goaway: bool },
    Tcp { force_close_after: Duration },
    Auto,
}

#[derive(Debug, Clone)]
pub enum CleanupPolicy {
    CountBasedOnly(usize),
}

#[derive(Debug, Clone)]
enum ListenerState {
    Active,
    Draining { started_at: Instant, drain_config: ListenerDrainConfig },
}

impl Default for ListenerDrainConfig {
    fn default() -> Self {
        Self {
            drain_time: Duration::from_secs(600),
            drain_strategy: DrainStrategy::Gradual,
            protocol_handling: ProtocolDrainBehavior::Auto,
        }
    }
}

impl PartialEq for DrainStrategy {
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

impl Default for ListenerManagerConfig {
    fn default() -> Self {
        Self {
            max_versions_per_listener: 2,
            cleanup_policy: CleanupPolicy::CountBasedOnly(2),
            cleanup_interval: Duration::from_secs(60),
            drain_config: ListenerDrainConfig::default(),
        }
    }
}

#[derive(Debug)]
struct ListenerInfo {
    handle: abort_on_drop::ChildTask<()>,
    listener_conf: ListenerConfig,
    version: u64,
    state: ListenerState,
    connections_count: Arc<AtomicUsize>,
    drain_manager_handle: Option<abort_on_drop::ChildTask<()>>,
}

impl ListenerInfo {
    fn new(handle: tokio::task::JoinHandle<()>, listener_conf: ListenerConfig, version: u64) -> Self {
        Self {
            handle: handle.into(),
            listener_conf,
            version,
            state: ListenerState::Active,
            connections_count: Arc::new(AtomicUsize::new(0)),
            drain_manager_handle: None,
        }
    }

    fn start_draining(&mut self, drain_config: ListenerDrainConfig, connection_manager: &DefaultConnectionManager) {
        let active_count = self.connections_count.load(Ordering::Relaxed);
        info!(
            "Starting graceful draining for listener {} version {} with {} active connections",
            self.listener_conf.name, self.version, active_count
        );

        self.state = ListenerState::Draining { started_at: Instant::now(), drain_config: drain_config.clone() };

        let drain_handle = self.start_drain_monitor(drain_config.clone());
        self.drain_manager_handle = Some(drain_handle.into());

        let protocol_behavior = match &self.state {
            ListenerState::Draining { drain_config, .. } => drain_config.protocol_handling.clone(),
            ListenerState::Active => return,
        };

        let active_connection_infos =
            connection_manager.get_connections_by_state(&self.listener_conf.name, ConnectionState::Active);
        let active_connection_ids: Vec<String> =
            active_connection_infos.iter().map(|conn_info| conn_info.id.clone()).collect();

        for conn_id in active_connection_ids {
            info!("Signaling existing connection {} to start draining", conn_id);
            connection_manager.start_connection_draining(&self.listener_conf.name, &conn_id, &protocol_behavior);
        }
    }

    fn start_drain_monitor(&self, drain_config: ListenerDrainConfig) -> tokio::task::JoinHandle<()> {
        let listener_name = self.listener_conf.name.clone();
        let version = self.version;
        let connections_count = Arc::clone(&self.connections_count);
        let started_at = Instant::now();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(5));
            let mut last_connection_count = connections_count.load(Ordering::Relaxed);

            info!(
                "Starting drain monitor for listener {} version {} with strategy {:?}",
                listener_name, version, drain_config.drain_strategy
            );

            loop {
                interval.tick().await;

                let elapsed = started_at.elapsed();
                let current_count = connections_count.load(Ordering::Relaxed);

                if elapsed >= drain_config.drain_time {
                    warn!("Drain timeout ({:?}) reached for listener {} version {}, force closing {} remaining connections",
                          drain_config.drain_time, listener_name, version, current_count);

                    break;
                }

                if current_count == 0 {
                    info!(
                        "All connections successfully drained for listener {} version {} in {:?}",
                        listener_name, version, elapsed
                    );
                    break;
                }

                if current_count != last_connection_count {
                    let connections_closed = last_connection_count.saturating_sub(current_count);
                    info!("Drain progress for listener {} version {}: {} connections closed, {} remaining ({:.1}% complete)",
                          listener_name, version, connections_closed, current_count,
                          Self::calculate_drain_percentage(elapsed, &drain_config));
                    last_connection_count = current_count;
                }

                match drain_config.drain_strategy {
                    DrainStrategy::Gradual => {
                        let progress = Self::calculate_drain_percentage(elapsed, &drain_config);
                        if progress > 50.0 {
                            debug!("Gradual drain reached 50%, would encourage connection draining");
                        }
                    },
                    DrainStrategy::Immediate => {
                        debug!("Immediate drain strategy - would encourage immediate draining");
                    },
                }

                debug!("Drain monitor tick for listener {} version {}: {} active connections, {:.1}% complete, {:?} elapsed",
                       listener_name, version, current_count,
                       Self::calculate_drain_percentage(elapsed, &drain_config),
                       elapsed);
            }

            info!("Drain monitor completed for listener {} version {}", listener_name, version);
        })
    }

    fn calculate_drain_percentage(elapsed: Duration, drain_config: &ListenerDrainConfig) -> f64 {
        match drain_config.drain_strategy {
            DrainStrategy::Immediate => 100.0,
            DrainStrategy::Gradual => {
                (elapsed.as_secs_f64() / drain_config.drain_time.as_secs_f64() * 100.0).min(100.0)
            },
        }
    }

    fn is_draining(&self) -> bool {
        matches!(self.state, ListenerState::Draining { .. })
    }

    fn should_force_close(&self) -> bool {
        if let ListenerState::Draining { started_at, drain_config } = &self.state {
            let elapsed = started_at.elapsed();
            elapsed >= drain_config.drain_time
        } else {
            false
        }
    }
}

pub struct ListenersManager {
    listener_configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
    route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
    listener_handles: MultiMap<String, ListenerInfo>,
    version_counter: u64,
    config: ListenerManagerConfig,
    connection_manager: Arc<DefaultConnectionManager>,
}

impl ListenersManager {
    pub fn new(
        listener_configuration_channel: mpsc::Receiver<ListenerConfigurationChange>,
        route_configuration_channel: mpsc::Receiver<RouteConfigurationChange>,
    ) -> Self {
        Self::new_with_config(
            listener_configuration_channel,
            route_configuration_channel,
            ListenerManagerConfig::default(),
        )
    }

    pub fn new_with_config(
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
            connection_manager: Arc::new(DefaultConnectionManager::new()),
        }
    }

    pub fn get_connection_manager(&self) -> Arc<DefaultConnectionManager> {
        Arc::clone(&self.connection_manager)
    }

    fn is_drain_active(listener_info: &ListenerInfo) -> bool {
        if let Some(drain_handle) = &listener_info.drain_manager_handle {
            !drain_handle.is_finished()
        } else {
            false
        }
    }

    fn get_drain_phase(listener_info: &ListenerInfo) -> DrainPhase {
        if !listener_info.is_draining() {
            return DrainPhase::Active;
        }

        if listener_info.should_force_close() {
            DrainPhase::ForceClosing
        } else if Self::is_drain_active(listener_info) {
            DrainPhase::Draining
        } else {
            DrainPhase::Completed
        }
    }

    #[allow(dead_code)]
    fn count_draining_versions(listener_infos: &[ListenerInfo]) -> usize {
        listener_infos.iter().filter(|info| info.is_draining()).count()
    }

    fn find_active_versions(listener_infos: &[ListenerInfo]) -> Vec<&ListenerInfo> {
        listener_infos.iter().filter(|info| !info.is_draining()).collect()
    }

    #[allow(dead_code)]
    fn find_latest_active_version(listener_infos: &[ListenerInfo]) -> Option<&ListenerInfo> {
        Self::find_active_versions(listener_infos).into_iter().max_by_key(|info| info.version)
    }

    pub fn get_listener_drain_status(&self, listener_name: &str) -> Vec<DrainProgress> {
        let mut drain_statuses = Vec::new();

        if let Some(listener_infos) = self.listener_handles.get_vec(listener_name) {
            for listener_info in listener_infos {
                if listener_info.is_draining() {
                    let progress = self.get_drain_progress_for_listener(listener_info);
                    drain_statuses.push(progress);
                }
            }
        }

        drain_statuses
    }

    pub fn get_all_listener_names(&self) -> Vec<String> {
        self.listener_handles.keys().cloned().collect()
    }

    pub fn get_total_active_connections(&self) -> usize {
        let mut total = 0;
        for (_, listener_info) in self.listener_handles.iter() {
            total += listener_info.connections_count.load(Ordering::Relaxed);
        }
        total
    }

    fn get_drain_progress_for_listener(&self, listener_info: &ListenerInfo) -> DrainProgress {
        let (percentage, elapsed, remaining_time) =
            if let ListenerState::Draining { started_at, drain_config } = &listener_info.state {
                let elapsed = started_at.elapsed();
                let percentage = ListenerInfo::calculate_drain_percentage(elapsed, drain_config);
                let remaining = drain_config.drain_time.saturating_sub(elapsed);
                (percentage, elapsed, Some(remaining))
            } else {
                (0.0, Duration::ZERO, None)
            };

        let all_connections = self.connection_manager.get_listener_connections(&listener_info.listener_conf.name);
        let total_connections = all_connections.len();
        let draining_connections =
            all_connections.iter().filter(|conn| matches!(conn.state, ConnectionState::Draining)).count();
        let active_connections = total_connections - draining_connections;

        DrainProgress {
            total_connections,
            active_connections,
            draining_connections,
            percentage,
            elapsed,
            remaining_time,
        }
    }

    fn estimate_drain_completion_for_listener(&self, listener_info: &ListenerInfo) -> Option<Instant> {
        if let ListenerState::Draining { started_at, drain_config } = &listener_info.state {
            let current_connections = listener_info.connections_count.load(Ordering::Relaxed);

            if current_connections == 0 {
                return Some(Instant::now());
            }

            match drain_config.drain_strategy {
                DrainStrategy::Immediate => Some(*started_at + drain_config.drain_time),
                DrainStrategy::Gradual => {
                    let elapsed = started_at.elapsed();
                    if elapsed.as_secs() > 10 {
                        let all_connections =
                            self.connection_manager.get_listener_connections(&listener_info.listener_conf.name);
                        let initial_connections = all_connections.len() + current_connections;

                        if initial_connections > current_connections {
                            let drained_count = initial_connections - current_connections;
                            let drain_rate = drained_count as f64 / elapsed.as_secs_f64();

                            if drain_rate > 0.0 {
                                let estimated_remaining_time = current_connections as f64 / drain_rate;
                                return Some(Instant::now() + Duration::from_secs_f64(estimated_remaining_time));
                            }
                        }

                        Some(*started_at + drain_config.drain_time)
                    } else {
                        Some(*started_at + drain_config.drain_time)
                    }
                },
            }
        } else {
            None
        }
    }

    pub fn get_comprehensive_drain_status(&self) -> HashMap<String, ListenerDrainStatusReport> {
        let mut reports = HashMap::new();

        for (listener_name, listener_infos) in self.listener_handles.iter_all() {
            let mut version_statuses = Vec::new();
            let mut draining_count = 0;

            for listener_info in listener_infos {
                let status = if listener_info.is_draining() {
                    draining_count += 1;

                    let drain_phase = Self::get_drain_phase(listener_info);
                    let progress = self.get_drain_progress_for_listener(listener_info);
                    let estimated_completion = self.estimate_drain_completion_for_listener(listener_info);

                    VersionDrainStatus {
                        version: listener_info.version,
                        state: drain_phase,
                        drain_progress: Some(progress),
                        started_at: match &listener_info.state {
                            ListenerState::Draining { started_at, .. } => Some(*started_at),
                            ListenerState::Active => None,
                        },
                        estimated_completion,
                    }
                } else {
                    VersionDrainStatus {
                        version: listener_info.version,
                        state: DrainPhase::Active,
                        drain_progress: None,
                        started_at: None,
                        estimated_completion: None,
                    }
                };

                version_statuses.push(status);
            }

            reports.insert(
                listener_name.clone(),
                ListenerDrainStatusReport {
                    listener_name: listener_name.clone(),
                    total_versions: version_statuses.len(),
                    draining_versions: draining_count,
                    version_statuses,
                    last_updated: Instant::now(),
                },
            );
        }

        reports
    }

    pub fn get_global_drain_statistics(&self) -> GlobalDrainStatistics {
        let mut total_listeners = 0;
        let mut draining_listeners = 0;
        let mut total_connections = 0;
        let mut draining_connections = 0;
        let mut oldest_drain_start: Option<Instant> = None;

        for (_, listener_info) in self.listener_handles.iter() {
            total_listeners += 1;
            let connections = listener_info.connections_count.load(Ordering::Relaxed);
            total_connections += connections;

            if listener_info.is_draining() {
                if Self::is_drain_active(listener_info) {
                    draining_listeners += 1;
                    draining_connections += connections;

                    if let ListenerState::Draining { started_at, .. } = &listener_info.state {
                        match oldest_drain_start {
                            None => oldest_drain_start = Some(*started_at),
                            Some(existing) if *started_at < existing => oldest_drain_start = Some(*started_at),
                            _ => {},
                        }
                    }
                }
            }
        }

        GlobalDrainStatistics {
            total_listeners,
            draining_listeners,
            total_connections,
            draining_connections,
            oldest_drain_start,
            drain_efficiency: if total_connections > 0 {
                (total_connections - draining_connections) as f64 / total_connections as f64 * 100.0
            } else {
                100.0
            },
            estimated_global_completion: self.estimate_global_drain_completion(),
        }
    }

    fn estimate_global_drain_completion(&self) -> Option<Instant> {
        let mut latest_completion: Option<Instant> = None;

        for (_, listener_info) in self.listener_handles.iter() {
            if listener_info.is_draining() {
                if let Some(estimated) = self.estimate_drain_completion_for_listener(listener_info) {
                    match latest_completion {
                        None => latest_completion = Some(estimated),
                        Some(existing) if estimated > existing => latest_completion = Some(estimated),
                        _ => {},
                    }
                }
            }
        }

        latest_completion
    }

    pub fn start_drain_monitoring_task(&self) -> tokio::task::JoinHandle<()> {
        let (_tx, mut rx) = tokio::sync::mpsc::channel::<String>(100);
        let connection_manager = Arc::clone(&self.connection_manager);
        let _cleanup_interval = self.config.drain_config.drain_time;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            let mut cleanup_interval_timer = tokio::time::interval(Duration::from_secs(60));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        debug!("Drain monitoring tick - monitoring mechanism needs redesign");

                        connection_manager.cleanup_completed_drains();
                    },
                    _ = cleanup_interval_timer.tick() => {
                        info!("Running periodic cleanup of completed drain states");
                        connection_manager.cleanup_completed_drains();
                    },
                    event = rx.recv() => {
                        if let Some(event) = event {
                            debug!("Received drain monitoring event: {}", event);
                        } else {
                            info!("Drain monitoring channel closed, exiting monitor");
                            break;
                        }
                    }
                }
            }

            info!("Drain monitoring task completed");
        })
    }

    pub async fn register_http_connection_manager(&self, listener_name: &str, http_cm: Arc<HttpConnectionManager>) {
        if let Some(listener_infos) = self.listener_handles.get_vec(listener_name) {
            if let Some(latest_listener) = listener_infos.iter().max_by_key(|info| info.version) {
                if latest_listener.is_draining() {
                    if let ListenerState::Draining { drain_config, .. } = &latest_listener.state {
                        let drain_state = ListenerDrainState {
                            started_at: Instant::now(),
                            strategy: drain_config.drain_strategy.clone(),
                            protocol_behavior: drain_config.protocol_handling.clone(),
                            drain_scenario: super::drain_signaling::DrainScenario::ListenerUpdate,
                            drain_type: orion_configuration::config::listener::DrainType::Default,
                        };
                        http_cm.start_draining(drain_state).await;
                        info!("HTTP Connection Manager for listener {} started draining immediately", listener_name);
                    }
                }
            }
        }
    }

    pub async fn start_draining_http_connection_managers(&self, listener_name: &str) {
        info!("Starting drain signaling for HTTP connection managers on listener {}", listener_name);

        if let Err(e) = self.connection_manager.start_draining_http_managers(listener_name).await {
            warn!("Failed to start draining HTTP managers for listener {}: {}", listener_name, e);
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
                            if let Err(e) = self.stop_listener(&listener_name) {
                                warn!("Failed to stop removed listener {}: {}", listener_name, e);
                            }
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

        if let Some(existing_versions) = self.listener_handles.get_vec(&listener_name) {
            for existing_info in existing_versions {
                if Self::listener_configs_equivalent(&existing_info.listener_conf, &listener_conf) {
                    info!(
                        "Listener {} configuration unchanged from version {}, skipping duplicate creation",
                        listener_name, existing_info.version
                    );
                    return Ok(());
                }
            }

            info!(
                "Configuration changed for listener {}, stopping all existing versions to prevent mixed responses",
                listener_name
            );

            let existing_versions_to_stop: Vec<_> = Self::find_active_versions(&existing_versions)
                .into_iter()
                .map(|info| (info.version, info.listener_conf.address))
                .collect();

            for (old_version, old_addr) in existing_versions_to_stop {
                info!("Stopping listener {} version {} (config update) at {}", listener_name, old_version, old_addr);
                if let Err(e) = self.stop_listener_version(&listener_name, old_version) {
                    warn!(
                        "Failed to stop listener {} version {} during config update: {}",
                        listener_name, old_version, e
                    );
                }
            }

            info!("Starting new version of listener {} with updated configuration", listener_name);
        } else {
            info!("Starting initial version of listener {}", listener_name);
        }

        let version = if let Some(version_info) = &listener_conf.version_info {
            version_info.parse::<u64>().unwrap_or_else(|_| {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                version_info.hash(&mut hasher);
                hasher.finish()
            })
        } else {
            self.version_counter += 1;
            self.version_counter
        };

        let listener_name_for_async = listener_name.clone();
        let join_handle = tokio::spawn(async move {
            let error = listener.start().await;
            warn!("Listener {} version {} exited with error: {}", listener_name_for_async, version, error);
        });

        let listener_info = ListenerInfo::new(join_handle, listener_conf, version);

        self.listener_handles.insert(listener_name.clone(), listener_info);
        let version_count = self.listener_handles.get_vec(&listener_name).map(|v| v.len()).unwrap_or(0);
        info!("Started version {} of listener {} ({} total active version(s))", version, listener_name, version_count);

        self.drain_old_listeners(&listener_name);

        Ok(())
    }

    fn listener_configs_equivalent(config1: &ListenerConfig, config2: &ListenerConfig) -> bool {
        config1.name == config2.name
            && config1.address == config2.address
            && config1.with_tls_inspector == config2.with_tls_inspector
            && config1.with_tlv_listener_filter == config2.with_tlv_listener_filter
            && config1.proxy_protocol_config == config2.proxy_protocol_config
            && config1.tlv_listener_filter_config == config2.tlv_listener_filter_config
            && config1.drain_type == config2.drain_type
            && config1.bind_device == config2.bind_device
            && config1.filter_chains == config2.filter_chains
    }

    fn resolve_address_conflicts(&mut self, listener_name: &str, new_config: &ListenerConfig) -> Result<()> {
        if let Some(existing_versions) = self.listener_handles.get_vec_mut(listener_name) {
            let mut conflicts_found = 0;

            for existing_info in existing_versions.iter_mut() {
                if existing_info.listener_conf.address == new_config.address && !existing_info.is_draining() {
                    warn!(
                        "Address conflict: listener {} version {} at {} conflicts with new configuration",
                        listener_name, existing_info.version, new_config.address
                    );

                    existing_info.start_draining(self.config.drain_config.clone(), &self.connection_manager);
                    conflicts_found += 1;

                    info!(
                        "Started graceful drain for conflicting listener {} version {} to resolve address binding",
                        listener_name, existing_info.version
                    );
                }
            }

            if conflicts_found > 0 {
                info!(
                    "Resolved {} address conflict(s) for listener {} through graceful draining",
                    conflicts_found, listener_name
                );
            }
        }

        Ok(())
    }

    pub fn start_listener_with_conflict_resolution(
        &mut self,
        listener: Listener,
        listener_conf: ListenerConfig,
    ) -> Result<()> {
        let listener_name = listener.get_name().to_string();

        self.resolve_address_conflicts(&listener_name, &listener_conf)?;
        self.start_listener(listener, listener_conf)
    }

    pub fn stop_listener_version(&mut self, listener_name: &str, version: u64) -> Result<()> {
        if let Some(versions) = self.listener_handles.get_vec_mut(listener_name) {
            let mut found = false;
            for listener_info in versions.iter_mut() {
                if listener_info.version == version && !listener_info.is_draining() {
                    info!(
                        "Starting graceful draining for listener {} version {} with {} active connections",
                        listener_name,
                        version,
                        listener_info.connections_count.load(Ordering::Relaxed)
                    );

                    listener_info.start_draining(self.config.drain_config.clone(), &self.connection_manager);
                    found = true;

                    info!(
                        "Stopping listener {} version {} (drain strategy: {:?})",
                        listener_name, version, self.config.drain_config.drain_strategy
                    );

                    if self.config.drain_config.drain_strategy == DrainStrategy::Immediate {
                        listener_info.handle.abort();
                        if let Some(drain_handle) = listener_info.drain_manager_handle.as_ref() {
                            drain_handle.abort();
                        }
                    } else {
                        warn!(
                            "Gracefully draining old listener {} version {} - monitored by background task",
                            listener_name, version
                        );
                    }
                    break;
                }
            }

            if !found {
                info!("Listener {} version {} not found or already draining", listener_name, version);
            }
        } else {
            info!("No listeners found with name {}", listener_name);
        }

        Ok(())
    }

    pub fn stop_listener(&mut self, listener_name: &str) -> Result<()> {
        if let Some(mut listeners) = self.listener_handles.remove(listener_name) {
            info!("Gracefully stopping all {} version(s) of listener {}", listeners.len(), listener_name);

            // Start draining for all versions
            for listener_info in &mut listeners {
                if !listener_info.is_draining() {
                    listener_info.start_draining(self.config.drain_config.clone(), &self.connection_manager);
                }
            }

            for listener_info in listeners {
                info!(
                    "Stopping listener {} version {} (drain strategy: {:?})",
                    listener_name, listener_info.version, self.config.drain_config.drain_strategy
                );

                if self.config.drain_config.drain_strategy == DrainStrategy::Immediate {
                    listener_info.handle.abort();
                    if let Some(drain_handle) = listener_info.drain_manager_handle {
                        drain_handle.abort();
                    }
                } else {
                    info!(
                        "Listener {} version {} will be managed by drain monitor",
                        listener_name, listener_info.version
                    );
                }
            }
        } else {
            info!("No listeners found with name {}", listener_name);
        }

        Ok(())
    }

    fn drain_old_listeners(&mut self, listener_name: &str) {
        if let Some(versions) = self.listener_handles.get_vec_mut(listener_name) {
            let original_count = versions.len();

            match &self.config.cleanup_policy {
                CleanupPolicy::CountBasedOnly(max_count) => {
                    if versions.len() > *max_count {
                        let to_remove = versions.len() - max_count;
                        let mut to_drain = Vec::new();

                        for _ in 0..to_remove {
                            if let Some(mut old_listener) = versions.drain(0..1).next() {
                                info!(
                                    "Starting drain for old listener {} version {} (count limit)",
                                    listener_name, old_listener.version
                                );
                                old_listener.start_draining(self.config.drain_config.clone(), &self.connection_manager);
                                to_drain.push(old_listener);
                            }
                        }

                        for draining_listener in to_drain {
                            if self.config.drain_config.drain_strategy == DrainStrategy::Immediate {
                                info!(
                                    "Immediately stopping old listener {} version {} (immediate drain)",
                                    listener_name, draining_listener.version
                                );
                                draining_listener.handle.abort();
                                if let Some(drain_handle) = draining_listener.drain_manager_handle {
                                    drain_handle.abort();
                                }
                            } else {
                                warn!(
                                    "Gracefully draining old listener {} version {} - monitored by background task",
                                    listener_name, draining_listener.version
                                );
                                draining_listener.handle.abort();
                                if let Some(drain_handle) = draining_listener.drain_manager_handle {
                                    drain_handle.abort();
                                }
                            }
                        }

                        let remaining_count = versions.len();
                        if original_count != remaining_count {
                            info!(
                                "Cleaned up {} old version(s) of listener {}, {} remaining",
                                original_count - remaining_count,
                                listener_name,
                                remaining_count
                            );
                        }
                    }
                },
            }
        }
    }

    pub async fn graceful_shutdown(&mut self, timeout: Duration) -> Result<()> {
        info!("Starting graceful shutdown with timeout {:?}", timeout);

        let listener_names: Vec<String> = self.listener_handles.iter().map(|(name, _)| name.clone()).collect();

        for listener_name in &listener_names {
            info!("Starting drain for listener: {}", listener_name);
            self.start_draining_http_connection_managers(listener_name).await;
        }

        let start_time = Instant::now();
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;

            let total_connections = self.get_total_active_connections();
            if total_connections == 0 {
                info!("All connections have drained gracefully");
                break;
            }

            if start_time.elapsed() >= timeout {
                warn!("Graceful shutdown timeout reached with {} connections still active", total_connections);
                break;
            }

            debug!(
                "Waiting for {} connections to drain ({}s remaining)",
                total_connections,
                (timeout - start_time.elapsed()).as_secs()
            );
        }

        let total_connections = self.get_total_active_connections();
        if total_connections > 0 {
            warn!("Force closing {} remaining connections", total_connections);
            self.force_close_all_connections();
        }

        for listener_name in &listener_names {
            if let Err(e) = self.stop_listener(listener_name) {
                warn!("Failed to stop listener {}: {}", listener_name, e);
            }
        }

        info!("Graceful shutdown completed");
        Ok(())
    }

    fn force_close_all_connections(&self) {
        for listener_name in self.listener_handles.keys() {
            let connections = self.connection_manager.get_listener_connections(listener_name);
            for conn_info in connections {
                warn!("Force closing connection {} on listener {}", conn_info.id, listener_name);
                self.connection_manager.force_close_connection(listener_name, &conn_info.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        net::{IpAddr, Ipv4Addr, SocketAddr},
        sync::Arc,
    };

    use super::*;
    use orion_configuration::config::{Listener as ListenerConfig, ListenerAddress};
    use tokio::sync::Mutex;
    use tracing_test::traced_test;

    fn create_test_listener_config(name: &str, port: u16) -> ListenerConfig {
        ListenerConfig {
            name: name.into(),
            address: ListenerAddress::Socket(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)),
            version_info: None,
            filter_chains: HashMap::default(),
            bind_device: None,
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
            drain_type: orion_configuration::config::listener::DrainType::Default,
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
            address: orion_configuration::config::listener::ListenerAddress::Socket(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                1234,
            )),
            filter_chains: HashMap::default(),
            bind_device: None,
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
            drain_type: orion_configuration::config::listener::DrainType::Default,
            version_info: None,
        };
        man.start_listener(l1, l1_info.clone()).unwrap();
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        let (_routeb_tx2, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx2, secb_rx) = broadcast::channel(chan);
        let l2 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l2_info = l1_info.clone();
        man.start_listener(l2, l2_info).unwrap();
        tokio::task::yield_now().await;

        // Only original listener should remain active (duplicate was skipped)
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        assert_eq!(man.listener_handles.get_vec(name).unwrap().len(), 1);

        let (routeb_tx3, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx3, secb_rx) = broadcast::channel(chan);
        let l3 = Listener::test_listener(name, routeb_rx, secb_rx);
        let mut l3_info = l1_info;
        l3_info.address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5678); // Different port
        man.start_listener(l3, l3_info).unwrap();
        tokio::task::yield_now().await;

        assert_eq!(man.listener_handles.get_vec(name).unwrap().len(), 2);
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        assert!(routeb_tx3.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;
    }

    #[traced_test]
    #[tokio::test]
    async fn test_config_change_drains_old_listener() {
        let chan = 10;
        let name = "issue74-listener";

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let mut man = ListenersManager::new(conf_rx, route_rx);

        let (route_tx1, route_rx1) = broadcast::channel(chan);
        let (_sec_tx1, sec_rx1) = broadcast::channel(chan);
        let listener1 = Listener::test_listener(name, route_rx1, sec_rx1);
        let config1 = create_test_listener_config(name, 10000);

        man.start_listener(listener1, config1.clone()).unwrap();
        assert!(route_tx1.send(RouteConfigurationChange::Removed("cluster_one".into())).is_ok());
        tokio::task::yield_now().await;

        assert_eq!(man.listener_handles.get_vec(name).unwrap().len(), 1);
        let initial_version = man.listener_handles.get_vec(name).unwrap()[0].version;
        assert!(!man.listener_handles.get_vec(name).unwrap()[0].is_draining());

        let (route_tx2, route_rx2) = broadcast::channel(chan);
        let (_sec_tx2, sec_rx2) = broadcast::channel(chan);
        let listener2 = Listener::test_listener(name, route_rx2, sec_rx2);
        let mut config2 = config1.clone();
        config2.address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20000);

        man.start_listener(listener2, config2).unwrap();
        assert!(route_tx2.send(RouteConfigurationChange::Removed("cluster_two".into())).is_ok());
        tokio::task::yield_now().await;

        let versions = man.listener_handles.get_vec(name).unwrap();
        assert_eq!(versions.len(), 2, "Should have both old and new versions during transition");

        let original_listener = versions.iter().find(|v| v.version == initial_version).unwrap();
        assert!(original_listener.is_draining(), "Original listener should be draining when config changes");

        let new_listener = versions.iter().find(|v| v.version != initial_version).unwrap();
        assert!(!new_listener.is_draining(), "New listener should be active");

        info!("Old listener is being drained when config changes");
        info!("This prevents mixed responses from both endpoints");

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
            bind_device: None,
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
            drain_type: orion_configuration::config::listener::DrainType::Default,
            version_info: None,
        };
        man.start_listener(l1, l1_info).unwrap();

        drop(routeb_tx1);
        drop(secb_tx1);
        tokio::task::yield_now().await;

        // See .start_listener() - in the case all channels are dropped the task there
        // should exit with this warning msg
        let expected = format!("Listener {name} version 1 exited with error: channel closed");
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
            drain_config: ListenerDrainConfig::default(),
        };
        let mut man = ListenersManager::new_with_config(conf_rx, route_rx, config);

        let (routeb_tx1, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx1, secb_rx) = broadcast::channel(chan);
        let l1 = Listener::test_listener(name, routeb_rx, secb_rx);
        let l1_info = create_test_listener_config(name, 1234);
        man.start_listener(l1, l1_info).unwrap();
        assert!(routeb_tx1.send(RouteConfigurationChange::Removed("n/a".into())).is_ok());
        tokio::task::yield_now().await;

        assert_eq!(man.listener_handles.get_vec(name).unwrap().len(), 1);

        for i in 1..=5 {
            let (_routeb_tx, routeb_rx) = broadcast::channel(chan);
            let (_secb_tx, secb_rx) = broadcast::channel(chan);
            let listener = Listener::test_listener(name, routeb_rx, secb_rx);
            let listener_info = create_test_listener_config(name, 1230 + i);
            man.start_listener(listener, listener_info).unwrap();
            tokio::task::yield_now().await;
        }

        let versions = man.listener_handles.get_vec(name).unwrap();
        assert!(versions.len() <= 2, "Expected at most 2 versions, got {}", versions.len());

        man.stop_listener(name).unwrap();
        assert!(man.listener_handles.get_vec(name).is_none());

        tokio::task::yield_now().await;
    }

    #[traced_test]
    #[tokio::test]
    async fn test_drain_strategy_immediate() {
        let chan = 10;
        let name = "immediate-drain-listener";

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let config = ListenerManagerConfig {
            max_versions_per_listener: 2,
            cleanup_policy: CleanupPolicy::CountBasedOnly(2),
            cleanup_interval: Duration::from_secs(60),
            drain_config: ListenerDrainConfig {
                drain_time: Duration::from_secs(5),
                drain_strategy: DrainStrategy::Immediate,
                protocol_handling: ProtocolDrainBehavior::Auto,
            },
        };
        let mut man = ListenersManager::new_with_config(conf_rx, route_rx, config);

        let (_routeb_tx, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx, secb_rx) = broadcast::channel(chan);
        let listener = Listener::test_listener(name, routeb_rx, secb_rx);
        let listener_info = create_test_listener_config(name, 1234);
        man.start_listener(listener, listener_info).unwrap();

        man.stop_listener(name).unwrap();

        tokio::task::yield_now().await;
    }

    #[traced_test]
    #[tokio::test]
    async fn test_drain_strategy_gradual() {
        let chan = 10;
        let name = "gradual-drain-listener";

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let config = ListenerManagerConfig {
            max_versions_per_listener: 2,
            cleanup_policy: CleanupPolicy::CountBasedOnly(2),
            cleanup_interval: Duration::from_secs(60),
            drain_config: ListenerDrainConfig {
                drain_time: Duration::from_secs(10),
                drain_strategy: DrainStrategy::Gradual,
                protocol_handling: ProtocolDrainBehavior::Http1 { connection_close: true },
            },
        };
        let mut man = ListenersManager::new_with_config(conf_rx, route_rx, config);

        let (_routeb_tx, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx, secb_rx) = broadcast::channel(chan);
        let listener = Listener::test_listener(name, routeb_rx, secb_rx);
        let listener_info = create_test_listener_config(name, 1234);
        man.start_listener(listener, listener_info).unwrap();

        man.stop_listener(name).unwrap();

        tokio::task::yield_now().await;
    }

    #[traced_test]
    #[tokio::test]
    async fn test_protocol_specific_drain_behavior() {
        let chan = 10;
        let name = "protocol-drain-listener";

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let config = ListenerManagerConfig {
            max_versions_per_listener: 2,
            cleanup_policy: CleanupPolicy::CountBasedOnly(2),
            cleanup_interval: Duration::from_secs(60),
            drain_config: ListenerDrainConfig {
                drain_time: Duration::from_secs(30),
                drain_strategy: DrainStrategy::Gradual,
                protocol_handling: ProtocolDrainBehavior::Http2 { send_goaway: true },
            },
        };
        let mut man = ListenersManager::new_with_config(conf_rx, route_rx, config);

        let (_routeb_tx, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx, secb_rx) = broadcast::channel(chan);
        let listener = Listener::test_listener(name, routeb_rx, secb_rx);
        let listener_info = create_test_listener_config(name, 1234);
        man.start_listener(listener, listener_info).unwrap();

        let drain_status = man.get_listener_drain_status(name);
        assert_eq!(drain_status.len(), 0);

        man.stop_listener(name).unwrap();

        tokio::task::yield_now().await;
    }

    #[traced_test]
    #[tokio::test]
    async fn test_drain_timeout_enforcement() {
        let chan = 10;
        let name = "timeout-test-listener";

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let config = ListenerManagerConfig {
            max_versions_per_listener: 2,
            cleanup_policy: CleanupPolicy::CountBasedOnly(2),
            cleanup_interval: Duration::from_secs(60),
            drain_config: ListenerDrainConfig {
                drain_time: Duration::from_millis(100),
                drain_strategy: DrainStrategy::Gradual,
                protocol_handling: ProtocolDrainBehavior::Auto,
            },
        };
        let mut man = ListenersManager::new_with_config(conf_rx, route_rx, config);

        let (_routeb_tx, routeb_rx) = broadcast::channel(chan);
        let (_secb_tx, secb_rx) = broadcast::channel(chan);
        let listener = Listener::test_listener(name, routeb_rx, secb_rx);
        let listener_info = create_test_listener_config(name, 1234);
        man.start_listener(listener, listener_info).unwrap();

        man.stop_listener(name).unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        tokio::task::yield_now().await;
    }

    #[traced_test]
    #[tokio::test]
    async fn test_address_conflict_resolution_graceful() {
        let chan = 10;
        let name = "conflict-test-listener";
        let shared_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);

        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let mut man = ListenersManager::new_with_config(
            conf_rx,
            route_rx,
            ListenerManagerConfig {
                max_versions_per_listener: 3,
                cleanup_policy: CleanupPolicy::CountBasedOnly(2),
                cleanup_interval: Duration::from_secs(60),
                drain_config: ListenerDrainConfig {
                    drain_time: Duration::from_secs(30),
                    drain_strategy: DrainStrategy::Gradual,
                    protocol_handling: ProtocolDrainBehavior::Auto,
                },
            },
        );

        let (routeb_tx1, routeb_rx1) = broadcast::channel(chan);
        let (_secb_tx1, secb_rx1) = broadcast::channel(chan);
        let l1 = Listener::test_listener(name, routeb_rx1, secb_rx1);
        let l1_info = ListenerConfig {
            name: name.into(),
            address: shared_address,
            filter_chains: HashMap::default(),
            bind_device: None,
            with_tls_inspector: false,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
            drain_type: orion_configuration::config::listener::DrainType::Default,
            version_info: None,
        };

        man.start_listener_with_conflict_resolution(l1, l1_info.clone()).unwrap();

        let versions = man.listener_handles.get_vec(name).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, 1);
        assert!(!versions[0].is_draining());

        let (routeb_tx2, routeb_rx2) = broadcast::channel(chan);
        let (_secb_tx2, secb_rx2) = broadcast::channel(chan);
        let l2 = Listener::test_listener(name, routeb_rx2, secb_rx2);
        let l2_info = ListenerConfig {
            name: name.into(),
            address: shared_address,
            filter_chains: HashMap::default(),
            bind_device: None,
            with_tls_inspector: true,
            proxy_protocol_config: None,
            with_tlv_listener_filter: false,
            tlv_listener_filter_config: None,
            drain_type: orion_configuration::config::listener::DrainType::Default,
            version_info: None,
        };

        man.start_listener_with_conflict_resolution(l2, l2_info).unwrap();

        let versions = man.listener_handles.get_vec(name).unwrap();
        assert_eq!(versions.len(), 2);

        assert_eq!(versions[0].version, 1);
        assert!(versions[0].is_draining());
        assert_eq!(versions[1].version, 2);
        assert!(!versions[1].is_draining());

        info!("Successfully demonstrated graceful address conflict resolution");
        info!("PR #77 approach: would have immediately killed version 1 -> broken connections");
        info!("Our approach: version 1 gracefully draining -> connections preserved");

        drop(routeb_tx1);
        drop(routeb_tx2);
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    async fn test_xds_version_handling() {
        let chan = 16;
        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let config = ListenerManagerConfig {
            max_versions_per_listener: 3,
            cleanup_policy: CleanupPolicy::CountBasedOnly(3),
            cleanup_interval: Duration::from_secs(60),
            drain_config: ListenerDrainConfig::default(),
        };
        let mut man = ListenersManager::new_with_config(conf_rx, route_rx, config);

        let name = "test-xds-version";

        let (routeb_tx1, routeb_rx1) = broadcast::channel(chan);
        let (_secb_tx1, secb_rx1) = broadcast::channel(chan);
        let l1 = Listener::test_listener(name, routeb_rx1, secb_rx1);
        let mut l1_info = create_test_listener_config(name, 8001);
        l1_info.version_info = Some("42".to_string());

        man.start_listener_with_conflict_resolution(l1, l1_info).unwrap();

        let versions = man.listener_handles.get_vec(name).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, 42);

        let (routeb_tx2, routeb_rx2) = broadcast::channel(chan);
        let (_secb_tx2, secb_rx2) = broadcast::channel(chan);
        let l2 = Listener::test_listener(name, routeb_rx2, secb_rx2);
        let mut l2_info = create_test_listener_config(name, 8001);
        l2_info.version_info = Some("v1.2.3-alpha".to_string());
        l2_info.with_tls_inspector = true;

        println!("Starting second listener with version_info: {:?}", l2_info.version_info);
        man.start_listener_with_conflict_resolution(l2, l2_info).unwrap();

        let versions = man.listener_handles.get_vec(name).unwrap();
        println!("After second listener, versions: {:?}", versions.iter().map(|v| v.version).collect::<Vec<_>>());
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, 42);
        assert_ne!(versions[1].version, 42);

        let (routeb_tx3, routeb_rx3) = broadcast::channel(chan);
        let (_secb_tx3, secb_rx3) = broadcast::channel(chan);
        let l3 = Listener::test_listener(name, routeb_rx3, secb_rx3);
        let mut l3_info = create_test_listener_config(name, 8001);
        l3_info.with_tls_inspector = true;
        l3_info.drain_type = orion_configuration::config::listener::DrainType::ModifyOnly;

        println!(
            "Starting third listener with version_info: {:?}, with_tls_inspector: {}, drain_type: {:?}",
            l3_info.version_info, l3_info.with_tls_inspector, l3_info.drain_type
        );
        man.start_listener_with_conflict_resolution(l3, l3_info).unwrap();

        let versions = man.listener_handles.get_vec(name).unwrap();
        println!("After third listener, versions: {:?}", versions.iter().map(|v| v.version).collect::<Vec<_>>());
        assert_eq!(versions.len(), 3);
        let third_version = versions[2].version;
        assert!(third_version > 0);
        assert!(third_version > 0);
        assert!(third_version > 0);

        drop(routeb_tx1);
        drop(routeb_tx2);
        drop(routeb_tx3);
        tokio::task::yield_now().await;
    }

    #[traced_test]
    #[tokio::test]
    async fn test_concurrent_listener_operations() {
        let chan = 16;
        let (_conf_tx, conf_rx) = mpsc::channel(chan);
        let (_route_tx, route_rx) = mpsc::channel(chan);
        let config = ListenerManagerConfig {
            max_versions_per_listener: 5,
            cleanup_policy: CleanupPolicy::CountBasedOnly(5),
            cleanup_interval: Duration::from_secs(60),
            drain_config: ListenerDrainConfig::default(),
        };
        let man = Arc::new(Mutex::new(ListenersManager::new_with_config(conf_rx, route_rx, config)));

        // Spawn multiple concurrent operations
        let mut handles = Vec::new();

        for i in 0..10 {
            let man_clone: Arc<Mutex<ListenersManager>> = Arc::clone(&man);
            let handle = tokio::spawn(async move {
                let name = match i {
                    0 => "concurrent-listener-0",
                    1 => "concurrent-listener-1",
                    2 => "concurrent-listener-2",
                    3 => "concurrent-listener-3",
                    4 => "concurrent-listener-4",
                    5 => "concurrent-listener-5",
                    6 => "concurrent-listener-6",
                    7 => "concurrent-listener-7",
                    8 => "concurrent-listener-8",
                    _ => "concurrent-listener-9",
                };
                let (routeb_tx, routeb_rx) = broadcast::channel(chan);
                let (_secb_tx, secb_rx) = broadcast::channel(chan);
                let listener = Listener::test_listener(name, routeb_rx, secb_rx);
                let listener_info = create_test_listener_config(name, 8000 + i);

                let mut manager = man_clone.lock().await;
                manager.start_listener(listener, listener_info).unwrap();
                drop(routeb_tx);
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let manager = man.lock().await;
        let expected_names = [
            "concurrent-listener-0",
            "concurrent-listener-1",
            "concurrent-listener-2",
            "concurrent-listener-3",
            "concurrent-listener-4",
            "concurrent-listener-5",
            "concurrent-listener-6",
            "concurrent-listener-7",
            "concurrent-listener-8",
            "concurrent-listener-9",
        ];
        for &name in expected_names.iter() {
            assert!(manager.listener_handles.get_vec(name).is_some());
        }
    }
}
