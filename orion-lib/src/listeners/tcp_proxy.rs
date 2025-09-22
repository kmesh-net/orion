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

use crate::{
    access_log::{log_access, log_access_reserve_balanced, Target},
    clusters::{clusters_manager::{self, RoutingContext}},
    listeners::{access_log::AccessLogContext, filter_state::DownstreamConnectionMetadata},
    transport::connector::TcpErrorContext,
    AsyncStream, Result,
};
use compact_str::ToCompactString;
use http::uri::Authority;
use orion_configuration::config::{
    cluster::ClusterSpecifier as ClusterSpecifierConfig,
    network_filters::{access_log::AccessLog, tcp_proxy::TcpProxy as TcpProxyConfig},
};
use orion_format::{
    context::{FinishContext, InitContext, TcpContext},
    types::ResponseFlags,
    LogFormatterLocal,
};
use std::{fmt, net::SocketAddr, sync::Arc, time::Instant};
use tracing::{debug, error, info};

#[derive(Debug, Clone)]
pub struct TcpProxy {
    pub listener_name: &'static str,
    cluster: ClusterSpecifierConfig,
    pub access_log: Vec<AccessLog>,
}

#[derive(Debug, Clone)]
pub struct TcpProxyBuilder {
    listener_name: Option<&'static str>,
    tcp_proxy_config: TcpProxyConfig,
}

impl From<TcpProxyConfig> for TcpProxyBuilder {
    fn from(tcp_proxy_config: TcpProxyConfig) -> Self {
        Self { tcp_proxy_config, listener_name: None }
    }
}

impl TcpProxyBuilder {
    pub fn with_listener_name(self, name: &'static str) -> Self {
        TcpProxyBuilder { listener_name: Some(name), ..self }
    }
    pub fn build(self) -> Result<TcpProxy> {
        let listener_name = self.listener_name.ok_or("listener name is not set")?;
        let TcpProxyConfig { cluster_specifier, access_log } = self.tcp_proxy_config;
        Ok(TcpProxy { listener_name, access_log, cluster: cluster_specifier })
    }
}

impl fmt::Display for TcpProxy {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TcpProxy").field("name", &self.listener_name).finish()
    }
}

impl TcpProxy {
    pub async fn serve_connection(
        &self,
        mut stream: AsyncStream,
        downstream_metadata: Arc<DownstreamConnectionMetadata>,
    ) -> Result<()> {
        let start_instant = Instant::now();
        let mut access_loggers = self.access_log.iter().map(|al| al.logger.local_clone()).collect::<Vec<_>>();

        access_loggers.with_context_fn(|| InitContext { start_time: std::time::SystemTime::now() });

        


        let cluster_selector = &self.cluster;
        info!("Handling request TCP for {} {:?} {:?}", self.listener_name,  cluster_selector, downstream_metadata);
        let cluster_id = clusters_manager::resolve_cluster(cluster_selector)
            .ok_or_else(|| "Failed to resolve cluster from specifier".to_owned())?;
                
        
        let routing_context = RoutingContext::Authority(Authority::try_from(downstream_metadata.local_address().to_string())?, downstream_metadata.original_destination_address());

        let maybe_connector = clusters_manager::get_tcp_connection(cluster_id, routing_context);
        info!("Handling request TCP connector {maybe_connector:?}");

        let mut bytes_received = 0;
        let mut bytes_sent = 0;
        let mut response_flags = ResponseFlags::empty();

        let cluster_name: &str;
        let maybe_upstream_local_addr: Option<SocketAddr>;
        let maybe_upstream_peer_addr: Option<SocketAddr>;

        let res = match maybe_connector {
            Ok(connector) => {
                let channel_result = connector.connect(Some(&downstream_metadata)).await;
                match channel_result {
                    Ok(mut channel) => {
                        maybe_upstream_local_addr = channel.upstream_local_addr;
                        maybe_upstream_peer_addr = channel.upstream_peer_addr;

                        let res = tokio::io::copy_bidirectional(&mut stream, &mut channel.stream).await;

                        match &res {
                            Ok((received, sent)) => {
                                bytes_received = *received;
                                bytes_sent = *sent;
                            },
                            Err(e) => {
                                response_flags.insert(ResponseFlags::UPSTREAM_CONNECTION_FAILURE);
                                debug!("Error with TCP stream: {}", e);
                            },
                        }

                        access_loggers.with_context(&TcpContext {
                            downstream_local_addr: Some(downstream_metadata.local_address()),
                            downstream_peer_addr: Some(downstream_metadata.peer_address()),
                            upstream_local_addr: maybe_upstream_local_addr,
                            upstream_peer_addr: maybe_upstream_peer_addr,
                            cluster_name: channel.cluster_name,
                        });

                        Ok(())
                    },
                    Err(e) => {
                        response_flags.insert(ResponseFlags::UPSTREAM_CONNECTION_FAILURE);

                        if let Some(tcp_error) = e.get_context_data::<TcpErrorContext>() {
                            maybe_upstream_peer_addr = Some(tcp_error.upstream_addr);
                            response_flags = tcp_error.response_flags.clone();
                            cluster_name = tcp_error.cluster_name;
                        } else {
                            // impossible case to make the compiler happy...
                            maybe_upstream_peer_addr = None;
                            cluster_name = "impossible";
                        }

                        access_loggers.with_context(&TcpContext {
                            downstream_local_addr: Some(downstream_metadata.local_address()),
                            downstream_peer_addr: Some(downstream_metadata.peer_address()),
                            upstream_local_addr: None,
                            upstream_peer_addr: maybe_upstream_peer_addr,
                            cluster_name,
                        });

                        Err(e)
                    },
                }
            },
            Err(e) => {
                error!("Failed to get TCP connection for cluster {:?}: {}", cluster_selector, e);
                response_flags.insert(ResponseFlags::NO_ROUTE_FOUND);

                access_loggers.with_context(&TcpContext {
                    downstream_local_addr: Some(downstream_metadata.local_address()),
                    downstream_peer_addr: Some(downstream_metadata.peer_address()),
                    upstream_local_addr: None,
                    upstream_peer_addr: None,
                    cluster_name: &cluster_selector.name(),
                });

                Err(e)
            },
        };

        access_loggers.with_context(&FinishContext {
            duration: start_instant.elapsed(),
            bytes_received,
            bytes_sent,
            response_flags,
        });
        let permit = log_access_reserve_balanced().await;
        let messages = access_loggers.into_iter().map(LogFormatterLocal::into_message).collect::<Vec<_>>();
        log_access(permit, Target::Listener(self.listener_name.to_compact_string()), messages);
        res
    }
}
