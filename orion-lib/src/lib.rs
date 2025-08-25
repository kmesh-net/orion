#![recursion_limit = "128"]
// SPDX-FileCopyrightText: © 2025 kmesh authors
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 kmesh authors
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

pub mod configuration;

pub mod clusters;
mod listeners;
//mod observability;
mod body;
mod secrets;
pub(crate) mod thread_local;
mod transport;
pub(crate) mod utils;

use std::sync::OnceLock;

use listeners::listeners_manager;
use orion_configuration::config::Runtime;
use tokio::sync::mpsc;

pub use crate::configuration::get_listeners_and_clusters;

pub use clusters::health::{EndpointHealthUpdate, HealthCheckManager};
pub use clusters::load_assignment::{LbEndpoint, PartialClusterLoadAssignment};
pub use clusters::{cluster::PartialClusterType, ClusterLoadAssignmentBuilder};
pub use listeners::listener::ListenerFactory;
pub use listeners_manager::{ListenerConfigurationChange, ListenersManager, RouteConfigurationChange};
pub use orion_configuration::config::network_filters::http_connection_manager::RouteConfiguration;
pub use secrets::SecretManager;
pub(crate) use transport::AsyncStream;

pub type Error = orion_error::Error;
pub type Result<T> = ::core::result::Result<T, Error>;

pub use crate::body::poly_body::PolyBody;

pub type HttpBody = PolyBody;

pub static RUNTIME_CONFIG: OnceLock<Runtime> = OnceLock::new();

pub fn runtime_config() -> &'static Runtime {
    RUNTIME_CONFIG.get().expect("Called runtime_config without setting RUNTIME_CONFIG first")
}

pub struct ConversionContext<'a, T> {
    envoy_object: T,
    secret_manager: &'a SecretManager,
}
impl<'a, T> ConversionContext<'a, T> {
    pub fn new(ctx: (T, &'a SecretManager)) -> Self {
        Self { envoy_object: ctx.0, secret_manager: ctx.1 }
    }
}

pub struct ConfigurationReceivers {
    listener_configuration_receiver: mpsc::Receiver<ListenerConfigurationChange>,
    route_configuration_receiver: mpsc::Receiver<RouteConfigurationChange>,
}

#[derive(Clone)]
pub struct ConfigurationSenders {
    pub listener_configuration_sender: mpsc::Sender<ListenerConfigurationChange>,
    pub route_configuration_sender: mpsc::Sender<RouteConfigurationChange>,
}

impl ConfigurationReceivers {
    pub fn new(
        listener_configuration_receiver: mpsc::Receiver<ListenerConfigurationChange>,
        route_configuration_receiver: mpsc::Receiver<RouteConfigurationChange>,
    ) -> Self {
        Self { listener_configuration_receiver, route_configuration_receiver }
    }
}

impl ConfigurationSenders {
    pub fn new(
        listener_configuration_sender: mpsc::Sender<ListenerConfigurationChange>,
        route_configuration_sender: mpsc::Sender<RouteConfigurationChange>,
    ) -> Self {
        Self { listener_configuration_sender, route_configuration_sender }
    }
}

pub fn new_configuration_channel(capacity: usize) -> (ConfigurationSenders, ConfigurationReceivers) {
    let (listener_tx, listener_rx) = mpsc::channel::<ListenerConfigurationChange>(capacity);
    let (route_tx, route_rx) = mpsc::channel::<RouteConfigurationChange>(capacity);
    (ConfigurationSenders::new(listener_tx, route_tx), ConfigurationReceivers::new(listener_rx, route_rx))
}

/// Start the listeners manager directly without spawning a background task.
/// Caller must be inside a Tokio runtime and await this async function.
pub async fn start_listener_manager(configuration_receivers: ConfigurationReceivers) -> Result<()> {
    let ConfigurationReceivers { listener_configuration_receiver, route_configuration_receiver } =
        configuration_receivers;
    tracing::debug!("listeners manager starting");
    let mgr = ListenersManager::new(listener_configuration_receiver, route_configuration_receiver);
    if let Err(err) = mgr.start().await {
        tracing::warn!(error = %err, "listeners manager exited with error");
        return Err(err);
    }
    tracing::debug!("listeners manager finished cleanly");
    Ok(())
}
