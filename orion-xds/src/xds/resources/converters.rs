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

use std::net::SocketAddr;

use orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::{
    address, socket_address::PortSpecifier, SocketAddress,
};

pub struct SocketConverter;

impl SocketConverter {
    pub fn from(value: SocketAddr) -> address::Address {
        let (protocol, ip, port) = match value {
            SocketAddr::V4(s) => (0, s.ip().to_string(), u32::from(s.port())),
            SocketAddr::V6(s) => (0, s.ip().to_string(), u32::from(s.port())),
        };
        let address = SocketAddress {
            protocol,
            address: ip,
            port_specifier: Some(PortSpecifier::PortValue(port)),
            resolver_name: String::new(),
            ipv4_compat: false,
            network_namespace_filepath: String::new(),
        };
        address::Address::SocketAddress(address)
    }
}
