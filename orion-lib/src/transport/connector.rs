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
    pin::Pin,
    task::{self, Poll},
    time::Duration,
};

use http::uri::Authority;
use hyper::Uri;
use hyper_util::rt::TokioIo;
use pingora_timeout::fast_timeout::fast_timeout;
use tokio::net::{TcpSocket, TcpStream};
use tower::Service;
use tracing::debug;

use crate::clusters::retry_policy::EventError;

use super::{bind_device::BindDevice, resolve};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Event(#[from] EventError),
}

#[derive(Clone, Debug)]
pub struct LocalConnectorWithDNSResolver {
    pub addr: Authority,
    pub bind_device: Option<BindDevice>,
    pub timeout: Option<Duration>,
}

impl LocalConnectorWithDNSResolver {
    pub fn connect(&self) -> impl Future<Output = std::result::Result<TcpStream, ConnectError>> + 'static {
        let addr = self.addr.clone();
        let device = self.bind_device.clone();
        let connection_timeout = self.timeout;

        async move {
            let host = addr.host();
            let port = addr
                .port_u16()
                .ok_or(io::Error::new(io::ErrorKind::AddrNotAvailable, format!("Port has to be set {addr:?}")))?;

            let addr = resolve(host, port).await?;

            let sock = match addr {
                std::net::SocketAddr::V4(_) => TcpSocket::new_v4()?,
                std::net::SocketAddr::V6(_) => TcpSocket::new_v6()?,
            };

            if let Some(device) = device {
                // binding might succeed here but still fail later
                // e.g. with an uncategorized error on connect
                debug!("Binding socket to: {:?}", device);
                super::bind_device::bind_device(&sock, &device)?;
            }

            let stream = if let Some(connection_timeout) = connection_timeout {
                fast_timeout(connection_timeout, sock.connect(addr))
                    .await
                    .map_err(|_| EventError::ConnectTimeout)?
                    .map_err(|_| EventError::ConnectFailure)?
            } else {
                sock.connect(addr).await.map_err(|_| EventError::ConnectFailure)?
            };

            Ok(stream)
        }
    }
}

impl Service<Uri> for LocalConnectorWithDNSResolver {
    type Response = TokioIo<TcpStream>;
    type Error = ConnectError;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        // This connector is always ready, but others might not be.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: Uri) -> Self::Future {
        let f = self.connect();
        Box::pin(async move { f.await.map(TokioIo::new) })
    }
}
