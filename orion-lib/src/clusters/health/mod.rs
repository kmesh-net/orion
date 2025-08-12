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

mod checkers;
mod counter;
mod manager;

use http::uri::Authority;

pub use manager::HealthCheckManager;
pub use orion_configuration::config::cluster::HealthStatus;

#[derive(Clone, Copy, PartialEq)]
pub enum ValueUpdated {
    Updated,
    NotUpdated,
}

pub trait EndpointHealth {
    fn health(&self) -> HealthStatus;
    fn update_health(&mut self, health: HealthStatus) -> ValueUpdated;
    fn is_healthy(&self) -> bool {
        self.health() == HealthStatus::Healthy
    }
}

impl EndpointHealth for HealthStatus {
    fn health(&self) -> HealthStatus {
        *self
    }

    fn update_health(&mut self, health_status: Self) -> ValueUpdated {
        if *self == health_status {
            ValueUpdated::NotUpdated
        } else {
            *self = health_status;
            ValueUpdated::Updated
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EndpointId {
    pub cluster: String,
    pub endpoint: Authority,
}

#[derive(Clone, Debug)]
pub struct EndpointHealthUpdate {
    /// The endpoint whose health has been checked.
    pub endpoint: EndpointId,
    /// The health status.
    pub health: HealthStatus,
    /// This is `true` if the health status has changed since the last update
    /// and `false` if it is repeated.
    pub changed: bool,
}
