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

use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    task::{self, Poll},
    time::Duration,
};

use http::uri::Authority;
use hyper::Uri;
use hyper_util::rt::TokioIo;
use orion_configuration::config::transport::BindDeviceOptions;
use orion_error::{Context, WithContext};
use orion_format::types::ResponseFlags;
use pingora_timeout::fast_timeout::fast_timeout;
use tokio::net::{TcpSocket, TcpStream};
use tower::Service;
use tracing::{debug, warn};

use crate::clusters::retry_policy::{elapsed, EventError};

use super::{resolve};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Event(#[from] EventError),
}

pub struct TcpErrorContext {
    pub upstream_addr: SocketAddr,
    pub response_flags: ResponseFlags,
    pub cluster_name: &'static str,
}

#[derive(Clone, Debug)]
pub struct LocalConnectorWithDNSResolver {
    pub addr: Authority,
    pub cluster_name: &'static str,
    pub bind_device_options: BindDeviceOptions,
    pub timeout: Option<Duration>,
}

impl LocalConnectorWithDNSResolver {
    pub fn connect(
        &self,
    ) -> impl Future<Output = std::result::Result<(TcpStream, &'static str), WithContext<ConnectError>>> + 'static {
        let addr = self.addr.clone();
        let device = self.bind_device_options.bind_device.clone();
        let bind_address = self.bind_device_options.bind_address.clone();
        let cluster_name = self.cluster_name;
        let connection_timeout = self.timeout;

        async move {
            let host = addr.host();
            let port = addr
                .port_u16()
                .ok_or(WithContext::new(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!("Port has to be set {addr:?}"),
                )))
                .map_err(|e| {
                    // this error is difficult to categorize. It happens because the port is not set in the URI.
                    // The host has not been resolved yet, so we cannot provide a specific address.
                    e.with_context_data(TcpErrorContext {
                        upstream_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
                        response_flags: ResponseFlags::UPSTREAM_CONNECTION_FAILURE,
                        cluster_name,
                    })
                    .map_into()
                })?;

            let addr = resolve(host, port).await.map_err(|e| {
                WithContext::new(e)
                    .with_context_data(TcpErrorContext {
                        upstream_addr: SocketAddr::from(([0, 0, 0, 0], port)),
                        response_flags: ResponseFlags::DNS_RESOLUTION_FAILED,
                        cluster_name,
                    })
                    .map_into()
            })?;

            let sock = match addr {
                std::net::SocketAddr::V4(_) => TcpSocket::new_v4().map_err(|e| {
                    WithContext::new(e)
                        .with_context_data(TcpErrorContext {
                            upstream_addr: addr,
                            response_flags: ResponseFlags::NO_HEALTHY_UPSTREAM
                                | ResponseFlags::UPSTREAM_CONNECTION_FAILURE,
                            cluster_name,
                        })
                        .map_into()
                })?,
                std::net::SocketAddr::V6(_) => TcpSocket::new_v4().map_err(|e| {
                    WithContext::new(e)
                        .with_context_data(TcpErrorContext {
                            upstream_addr: addr,
                            response_flags: ResponseFlags::NO_HEALTHY_UPSTREAM
                                | ResponseFlags::UPSTREAM_CONNECTION_FAILURE,
                            cluster_name,
                        })
                        .map_into()
                })?,
            };

            if let Some(device) = device {
                // binding might succeed here but still fail later
                // e.g. with an uncategorized error on connect
                debug!("Binding socket to: {:?}", device);
                super::bind_device::bind_device(&sock, &device).map_err(|e| {
                    WithContext::new(e)
                        .with_context_data(TcpErrorContext {
                            upstream_addr: addr,
                            response_flags: ResponseFlags::UPSTREAM_CONNECTION_FAILURE,
                            cluster_name,
                        })
                        .map_into()
                })?;
            }
            
            if let Some(bind_addr) = bind_address{
                match bind_addr.address(){
                    orion_configuration::config::core::envoy_conversions::Address::Socket(bind_address, _) => {
                        let maybe_socket_addr = format!("{bind_address}:0").parse::<std::net::SocketAddr>();                        
                        let bind_address = maybe_socket_addr.map_err(|e| EventError::ConnectFailure(io::Error::new(io::ErrorKind::AddrNotAvailable, e.to_string()))).map_err(|e| {
                        WithContext::new(e)
                            .with_context_data(TcpErrorContext {
                                upstream_addr: addr,
                                response_flags: ResponseFlags::LOCAL_RESET,
                                cluster_name,
                            })
                            .map_into()
                        })?;
                        
                        let maybe_error = sock.bind(bind_address);
                        debug!("LocalConnectorWithDNSResolver socket bound to {bind_address} {maybe_error:?}");
                        maybe_error.map_err(|e| EventError::ConnectFailure(io::Error::new(io::ErrorKind::AddrNotAvailable, e.to_string()))).map_err(|e| {
                        WithContext::new(e)
                            .with_context_data(TcpErrorContext {
                                upstream_addr: addr,
                                response_flags: ResponseFlags::LOCAL_RESET,
                                cluster_name,
                            })
                            .map_into()
                        })?
                    },
                        
                    orion_configuration::config::core::envoy_conversions::Address::Pipe(_, _) => (),
                }                
            }
            
            let stream = if let Some(connection_timeout) = connection_timeout {
                fast_timeout(connection_timeout, sock.connect(addr))
                    .await // Result<Result<TcpStream, io::Error>>, Elapsed>
                    .map_err(|_| EventError::ConnectTimeout(elapsed()))
                    .map_err(|e| {
                        WithContext::new(e)
                            .with_context_data(TcpErrorContext {
                                upstream_addr: addr,
                                response_flags: ResponseFlags::UPSTREAM_CONNECTION_FAILURE,
                                cluster_name,
                            })
                            .map_into()
                    })? // Result<TcpStream, io::Error>
                    .map_err(|orig| EventError::ConnectFailure(io::Error::new(orig.kind(), orig.to_string())))
                    .map_err(|e| {
                        WithContext::new(e)
                            .with_context_data(TcpErrorContext {
                                upstream_addr: addr,
                                response_flags: ResponseFlags::UPSTREAM_CONNECTION_FAILURE,
                                cluster_name,
                            })
                            .map_into()
                    })?
            } else {
                sock.connect(addr)
                    .await
                    .map_err(|orig| EventError::ConnectFailure(io::Error::new(orig.kind(), orig.to_string())))
                    .map_err(|e| {
                        WithContext::new(e)
                            .with_context_data(TcpErrorContext {
                                upstream_addr: addr,
                                response_flags: ResponseFlags::UPSTREAM_CONNECTION_FAILURE,
                                cluster_name,
                            })
                            .map_into()
                    })?
            };

            Ok((stream, cluster_name))
        }
    }
}

impl Service<Uri> for LocalConnectorWithDNSResolver {
    type Response = TokioIo<TcpStream>;
    type Error = WithContext<ConnectError>;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        // This connector is always ready, but others might not be.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: Uri) -> Self::Future {
        let f = self.connect();
        Box::pin(async move { f.await.map(|(stream, _)| stream).map(TokioIo::new) })
    }
}
