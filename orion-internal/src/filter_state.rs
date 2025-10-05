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

use std::net::SocketAddr;

/// Reserved internal address used as the peer address for in-process (internal) connections.
/// Chosen from the loopback range (127.255.255.254:65534) to avoid conflicts with real network traffic.
/// This address clearly identifies internal connections in logs and debugging.
pub const INTERNAL_PEER_ADDR: SocketAddr =
    SocketAddr::V4(std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(127, 255, 255, 254), 65534));

/// Reserved internal address used as the local address for in-process (internal) connections.
/// Chosen from the loopback range (127.255.255.255:65535) to avoid conflicts with real network traffic.
/// This address clearly identifies internal connections in logs and debugging.
pub const INTERNAL_LOCAL_ADDR: SocketAddr =
    SocketAddr::V4(std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(127, 255, 255, 255), 65535));

/// Check if a socket address is an internal connection address
pub fn is_internal_address(addr: &SocketAddr) -> bool {
    *addr == INTERNAL_PEER_ADDR || *addr == INTERNAL_LOCAL_ADDR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_internal_addresses() {
        assert!(is_internal_address(&INTERNAL_PEER_ADDR));
        assert!(is_internal_address(&INTERNAL_LOCAL_ADDR));

        let normal_addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        assert!(!is_internal_address(&normal_addr));
    }
}
