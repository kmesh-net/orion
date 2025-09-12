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

use orion_configuration::config::common::TlvType;
use std::{collections::HashMap, net::SocketAddr};

#[derive(Debug, Clone)]
pub enum DownstreamConnectionMetadata {
    FromSocket {
        peer_address: SocketAddr,
        local_address: SocketAddr,
    },
    FromProxyProtocol {
        original_peer_address: SocketAddr,
        original_destination_address: SocketAddr,
        protocol: ppp::v2::Protocol,
        tlv_data: HashMap<TlvType, Vec<u8>>,
        proxy_peer_address: SocketAddr,
        proxy_local_address: SocketAddr,
    },
    FromTlv {
        original_destination_address: SocketAddr,
        tlv_data: HashMap<u8, Vec<u8>>,
        proxy_peer_address: SocketAddr,
        proxy_local_address: SocketAddr,
    },
}

impl DownstreamConnectionMetadata {
    pub fn peer_address(&self) -> SocketAddr {
        match self {
            Self::FromSocket { peer_address, .. } => *peer_address,
            Self::FromProxyProtocol { original_peer_address, .. } => *original_peer_address,
            Self::FromTlv { proxy_peer_address, .. } => *proxy_peer_address,
        }
    }
    pub fn local_address(&self) -> SocketAddr {
        match self {
            Self::FromSocket { local_address, .. } => *local_address,
            Self::FromProxyProtocol { original_destination_address, .. } => *original_destination_address,
            Self::FromTlv { original_destination_address, .. } => *original_destination_address,
        }
    }
}
