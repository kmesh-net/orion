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

//! # orion-internal
//!
//! Internal connection and internal listener infrastructure for Orion proxy.
//! This crate provides the core functionality for in-process communication between
//! different components of the proxy, particularly for waypoint proxy capabilities
//! in ambient service mesh scenarios.

pub mod connection;
pub mod connector;
pub mod filter_state;

// Re-export commonly used types
pub use connection::{
    global_internal_connection_factory, InternalConnectionFactory, InternalConnectionMetadata, InternalConnectionPair,
    InternalConnectionStats, InternalListenerHandle, InternalStream, InternalStreamWrapper,
};
pub use connector::{cluster_helpers, InternalChannel, InternalChannelConnector, InternalClusterConnector};
pub use filter_state::{is_internal_address, INTERNAL_LOCAL_ADDR, INTERNAL_PEER_ADDR};

// Re-export error types
pub type Error = orion_error::Error;
pub type Result<T> = ::core::result::Result<T, Error>;
