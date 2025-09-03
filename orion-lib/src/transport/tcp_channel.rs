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

use std::{net::SocketAddr, sync::Arc, time::Duration};

use super::{
    bind_device::BindDevice, connector::LocalConnectorWithDNSResolver, AsyncStream, UpstreamTransportSocketConfigurator,
};
use crate::{
    listeners::filter_state::DownstreamConnectionMetadata,
    secrets::{TlsConfigurator, WantsToBuildClient},
};
use futures::future::BoxFuture;
use http::uri::Authority;
use rustls::ClientConfig;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use webpki::types::ServerName;

#[derive(Debug, Clone)]
pub struct TcpChannelConnector {
    connector: LocalConnectorWithDNSResolver,
    transport_socket: UpstreamTransportSocketConfigurator,
}

pub struct TcpChannel {
    pub stream: AsyncStream,
    pub cluster_name: &'static str,
    pub upstream_local_addr: Option<SocketAddr>,
    pub upstream_peer_addr: Option<SocketAddr>,
}

impl TcpChannelConnector {
    pub fn new(
        authority: &Authority,
        cluster_name: &'static str,
        bind_device: Option<BindDevice>,
        timeout: Option<Duration>,
        transport_socket: UpstreamTransportSocketConfigurator,
    ) -> Self {
        Self {
            connector: LocalConnectorWithDNSResolver { addr: authority.clone(), cluster_name, bind_device, timeout },
            transport_socket,
        }
    }

    pub fn connect(
        &self,
        downstream_metadata: Option<&DownstreamConnectionMetadata>,
    ) -> BoxFuture<'static, crate::Result<TcpChannel>> {
        let connector = self.connector.clone();
        let transport_socket = self.transport_socket.clone();
        let downstream_metadata = downstream_metadata.cloned();

        Box::pin(async move {
            let (mut stream, cluster_name) = connector
                .connect()
                .await
                .map_err(|e| -> crate::Error { format!("TCP connection failed: {e}").into() })?;

            let upstream_local_addr = stream.local_addr().ok();
            let upstream_peer_addr = stream.peer_addr().ok();

            let stream: AsyncStream = match &transport_socket {
                UpstreamTransportSocketConfigurator::Tls(tls_configurator) => {
                    configure_tls(tls_configurator, stream).await?
                },
                UpstreamTransportSocketConfigurator::ProxyProtocol(proxy_configurator) => {
                    if let Some(metadata) = &downstream_metadata {
                        proxy_configurator.write_proxy_header(&mut stream, metadata).await.map_err(
                            |e| -> crate::Error { format!("Failed to write proxy protocol header: {e}").into() },
                        )?;
                    }
                    if let Some(inner_tls) = &proxy_configurator.inner_tls_configurator {
                        configure_tls(inner_tls, stream).await?
                    } else {
                        Box::new(stream)
                    }
                },
                UpstreamTransportSocketConfigurator::None => Box::new(stream),
            };

            Ok(TcpChannel { stream, cluster_name, upstream_local_addr, upstream_peer_addr })
        })
    }
}

async fn configure_tls(
    tls_config: &TlsConfigurator<ClientConfig, WantsToBuildClient>,
    stream: TcpStream,
) -> crate::Result<AsyncStream> {
    let client_config = tls_config.clone().into_inner();
    let server_name = ServerName::try_from(tls_config.sni())
        .map_err(|e| -> crate::Error { format!("Invalid server name: {e}").into() })?;
    let tls_connector = TlsConnector::from(Arc::new(client_config));
    let tls_stream = tls_connector
        .connect(server_name, stream)
        .await
        .map_err(|e| -> crate::Error { format!("TLS connection failed: {e}").into() })?;
    Ok(Box::new(tls_stream))
}
