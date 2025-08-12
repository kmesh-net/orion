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

use std::time::Duration;

use super::bind_device::BindDevice;
use super::connector::{ConnectError, LocalConnectorWithDNSResolver};
use futures::future::BoxFuture;
use http::uri::Authority;
use tokio::net::TcpStream;

#[derive(Debug, Clone)]
pub struct TcpChannel {
    connector: LocalConnectorWithDNSResolver,
}

impl TcpChannel {
    pub fn new(authority: &Authority, bind_device: Option<BindDevice>, timeout: Option<Duration>) -> Self {
        Self { connector: LocalConnectorWithDNSResolver { addr: authority.clone(), bind_device, timeout } }
    }

    pub fn connect(&self) -> BoxFuture<'static, std::result::Result<TcpStream, ConnectError>> {
        Box::pin(self.connector.connect())
    }
}
