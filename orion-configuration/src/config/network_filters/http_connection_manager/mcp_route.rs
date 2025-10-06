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

//todo: impl serialize, deserialize on DirectResponsebody to prepare the bytes at deserialization

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::cluster::ClusterSpecifier;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MCPRouteAction {
    pub cluster_specifier: ClusterSpecifier,

    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "is_default_timeout", default = "default_timeout_deser")]
    pub timeout: Option<Duration>,

    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub upgrade_config: Option<UpgradeConfig>,
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
#[allow(clippy::unnecessary_wraps)]
const fn default_timeout_deser() -> Option<Duration> {
    Some(DEFAULT_TIMEOUT)
}
#[allow(clippy::ref_option)]
fn is_default_timeout(timeout: &Option<Duration>) -> bool {
    *timeout == default_timeout_deser()
}

#[derive(Clone, Debug, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum Websocket {
    Enabled,
    Disabled,
}

#[derive(Clone, Debug, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum Connect {
    Enabled { allow_post: bool /* , proxy_protocol_config: Option<...> */ },
    Disabled,
}

#[derive(Clone, Debug, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct UpgradeConfig {
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub websocket: Option<Websocket>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub connect: Option<Connect>,
}

impl UpgradeConfig {
    pub fn is_websocket_enabled(&self, enabled_via_hcm: bool) -> bool {
        let locally_enabled = matches!(self.websocket, Some(Websocket::Enabled));
        let locally_disabled = matches!(self.websocket, Some(Websocket::Disabled));
        locally_enabled || (enabled_via_hcm && !locally_disabled)
    }
}
