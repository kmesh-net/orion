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

use tokio::io::{AsyncRead, AsyncWrite};

pub mod bind_device;
pub mod connector;
mod grpc_channel;
mod http_channel;
mod resolver;
pub mod tcp_channel;
pub use resolver::resolve;
pub mod policy;
pub mod proxy_protocol;
pub mod tls_inspector;
pub mod tlv_listener_filter;
pub mod transport_socket;

pub use self::{
    grpc_channel::{GrpcService, SimpleRoundRobinGrpcServiceLB},
    http_channel::{HttpChannel, HttpChannelBuilder, HttpChannels},
    proxy_protocol::ProxyProtocolReader,
    tcp_channel::TcpChannelConnector,
    tlv_listener_filter::TlvListenerFilter,
    transport_socket::UpstreamTransportSocketConfigurator,
};

pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Send + Sync + Unpin {}
impl<T> AsyncReadWrite for T where T: AsyncRead + AsyncWrite + Send + Sync + Unpin {}

pub type AsyncStream = Box<dyn AsyncReadWrite>;
