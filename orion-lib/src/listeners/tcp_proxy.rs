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

use crate::{clusters::clusters_manager, AsyncStream, Result};
use compact_str::CompactString;
use orion_configuration::config::{
    cluster::ClusterSpecifier as ClusterSpecifierConfig, network_filters::tcp_proxy::TcpProxy as TcpProxyConfig,
};
use std::fmt;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct TcpProxy {
    pub listener_name: CompactString,
    cluster: ClusterSpecifierConfig,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TcpProxyBuilder {
    listener_name: Option<CompactString>,
    tcp_proxy_config: TcpProxyConfig,
}

impl From<TcpProxyConfig> for TcpProxyBuilder {
    fn from(tcp_proxy_config: TcpProxyConfig) -> Self {
        Self { tcp_proxy_config, listener_name: None }
    }
}

#[allow(dead_code)]
impl TcpProxyBuilder {
    pub fn with_listener_name(self, name: CompactString) -> Self {
        TcpProxyBuilder { listener_name: Some(name), ..self }
    }
    pub fn build(self) -> Result<TcpProxy> {
        let listener_name = self.listener_name.ok_or("listener name is not set")?;
        let TcpProxyConfig { cluster_specifier } = self.tcp_proxy_config;
        Ok(TcpProxy { listener_name, cluster: cluster_specifier })
    }
}

impl fmt::Display for TcpProxy {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TcpProxy").field("name", &self.listener_name).finish()
    }
}

impl TcpProxy {
    pub async fn serve_connection(&self, mut stream: AsyncStream) -> Result<()> {
        let cluster_selector = &self.cluster;
        let maybe_channel = clusters_manager::get_tcp_connection(cluster_selector);
        if let Ok(channel) = maybe_channel {
            let mut channel = channel.await?;
            let res = tokio::io::copy_bidirectional(&mut stream, &mut channel).await;
            debug!("TCP Connection closed {res:?}");
            Ok(())
        } else {
            Err("No route for domain".into())
        }
    }
}
