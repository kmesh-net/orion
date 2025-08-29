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

use crate::{
    access_log::{log_access, log_access_reserve_balanced, Target},
    clusters::clusters_manager::{self, RoutingContext},
    event_error::{
        find_error_in_chain, ConnectionTerminationDetails, ResponseCodeDetails, UpstreamTransportEventError,
    },
    listeners::{access_log::AccessLogContext, filter_state::DownstreamMetadata},
    transport::connector::TcpErrorContext,
    AsyncStream, Result,
};
use compact_str::ToCompactString;
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
use tracing::{debug, error};

#[derive(Debug, Clone)]
pub struct TcpProxy {
    pub listener_name: String,
    cluster: ClusterSpecifierConfig,
    pub access_log: Vec<AccessLog>,
}

#[derive(Debug, Clone)]
pub struct TcpProxyBuilder {
    listener_name: Option<String>,
    tcp_proxy_config: TcpProxyConfig,
}

impl From<TcpProxyConfig> for TcpProxyBuilder {
    fn from(tcp_proxy_config: TcpProxyConfig) -> Self {
        Self { tcp_proxy_config, listener_name: None }
    }
}

impl TcpProxyBuilder {
    pub fn with_listener_name(self, name: &str) -> Self {
        TcpProxyBuilder { listener_name: Some(name.to_string()), ..self }
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
    #[allow(clippy::too_many_lines)]
    pub async fn serve_connection(
        &self,
        mut stream: AsyncStream,
        downstream_metadata: Arc<DownstreamMetadata>,
    ) -> Result<()> {
        let start_instant = Instant::now();
        let mut access_loggers = self.access_log.iter().map(|al| al.logger.local_clone()).collect::<Vec<_>>();

        access_loggers.with_context_fn(|| InitContext { start_time: std::time::SystemTime::now() });

        let cluster_selector = &self.cluster;
        let cluster_id = clusters_manager::resolve_cluster(cluster_selector)
            .ok_or_else(|| "Failed to resolve cluster from specifier".to_owned())?;
        let maybe_connector = clusters_manager::get_tcp_connection(cluster_id, RoutingContext::None);

        let mut bytes_received = 0;
        let mut bytes_sent = 0;
        let mut response_flags = ResponseFlags::empty();
        let mut maybe_upstream_transport_error: Option<UpstreamTransportEventError> = None;
        let mut maybe_response_code_details: Option<ResponseCodeDetails> = None;
        let mut maybe_connection_termination_details: Option<ConnectionTerminationDetails> = None;

        let cluster_name: &str;
        let maybe_upstream_local_addr: Option<SocketAddr>;
        let maybe_upstream_peer_addr: Option<SocketAddr>;

        let res = match maybe_connector {
            Ok(connector) => {
                let channel_result = connector.connect(Some(&downstream_metadata.connection)).await;
                match channel_result {
                    Ok(mut channel) => {
                        maybe_upstream_local_addr = channel.upstream_local_addr;
                        maybe_upstream_peer_addr = channel.upstream_peer_addr;

                        let res = tokio::io::copy_bidirectional(&mut stream, &mut channel.stream).await;
                        match res {
                            Ok((received, sent)) => {
                                bytes_received = received;
                                bytes_sent = sent;
                            },
                            Err(ref e) => {
                                debug!("Error with TCP stream: {}", e);
                                maybe_upstream_transport_error = Some(e.into());
                                maybe_response_code_details = Some(ResponseCodeDetails::from(e));
                                maybe_connection_termination_details = Some(ConnectionTerminationDetails::from(e));
                                response_flags.insert(ResponseFlags::UPSTREAM_CONNECTION_FAILURE);
                            },
                        }

                        access_loggers.with_context(&TcpContext {
                            downstream_local_addr: Some(downstream_metadata.connection.local_address()),
                            downstream_peer_addr: Some(downstream_metadata.connection.peer_address()),
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

                        let io_err = find_error_in_chain::<std::io::Error>(e.inner());
                        maybe_upstream_transport_error = io_err.map(UpstreamTransportEventError::from);
                        maybe_response_code_details = io_err.map(ResponseCodeDetails::from);
                        maybe_connection_termination_details = io_err.map(ConnectionTerminationDetails::from);

                        access_loggers.with_context(&TcpContext {
                            downstream_local_addr: Some(downstream_metadata.connection.local_address()),
                            downstream_peer_addr: Some(downstream_metadata.connection.peer_address()),
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

                let io_err = find_error_in_chain::<std::io::Error>(e.inner());
                maybe_upstream_transport_error = io_err.map(UpstreamTransportEventError::from);
                maybe_response_code_details = io_err.map(ResponseCodeDetails::from);
                maybe_connection_termination_details = io_err.map(ConnectionTerminationDetails::from);

                access_loggers.with_context(&TcpContext {
                    downstream_local_addr: Some(downstream_metadata.connection.local_address()),
                    downstream_peer_addr: Some(downstream_metadata.connection.peer_address()),
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
            upstream_failure: maybe_upstream_transport_error.map(|x| x.0),
            response_code_details: maybe_response_code_details.map(|x| x.0),
            connection_termination_details: maybe_connection_termination_details.map(|x| x.0),
        });

        let permit = log_access_reserve_balanced().await;
        let messages = access_loggers.into_iter().map(LogFormatterLocal::into_message).collect::<Vec<_>>();
        log_access(permit, Target::Listener(self.listener_name.to_compact_string()), messages);
        res
    }
}
