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

#[cfg(test)]
mod tests;

use std::sync::Arc;

use futures::{FutureExt, TryFutureExt, future::BoxFuture};
use orion_configuration::config::cluster::health_check::{ClusterHealthCheck, TcpHealthCheck};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Notify, mpsc},
    task::JoinHandle,
};

use crate::{
    EndpointHealthUpdate, Error,
    clusters::health::{EndpointId, checkers::checker::HealthCheckerLoop, counter::HealthStatusCounter},
    transport::TcpChannelConnector,
};

use super::checker::{IntervalWaiter, ProtocolChecker, WaitInterval};

const DEFAULT_MAX_PAYLOAD_BUFFER_SIZE: usize = 0x10_0000; // 1 MB

pub fn spawn_tcp_health_checker(
    endpoint: EndpointId,
    cluster_config: ClusterHealthCheck,
    protocol_config: TcpHealthCheck,
    channel: TcpChannelConnector,
    sender: mpsc::Sender<EndpointHealthUpdate>,
    stop_signal: Arc<Notify>,
) -> JoinHandle<Result<(), Error>> {
    let interval_waiter = IntervalWaiter;
    spawn_tcp_health_checker_impl::<_, _, DEFAULT_MAX_PAYLOAD_BUFFER_SIZE>(
        endpoint,
        cluster_config,
        protocol_config,
        sender,
        stop_signal,
        (channel, interval_waiter),
    )
}

trait TcpClient
where
    Self::Stream: AsyncRead + AsyncWrite,
{
    type Stream;
    fn connect(&self) -> BoxFuture<'static, std::result::Result<Self::Stream, Error>>;
}

impl TcpClient for TcpChannelConnector {
    type Stream = crate::transport::AsyncStream;
    fn connect(&self) -> BoxFuture<'static, std::result::Result<Self::Stream, Error>> {
        self.connect(None).map(|result| result.map(|channel| channel.stream)).map_err(Error::from).boxed()
    }
}

fn spawn_tcp_health_checker_impl<T, W, const MAX_PAYLOAD_BUFFER_SIZE: usize>(
    endpoint: EndpointId,
    cluster_config: ClusterHealthCheck,
    protocol_config: TcpHealthCheck,
    sender: mpsc::Sender<EndpointHealthUpdate>,
    stop_signal: Arc<Notify>,
    dependencies: (T, W),
) -> JoinHandle<Result<(), Error>>
where
    W: WaitInterval + Send + 'static,
    T: TcpClient + Send + Sync + 'static,
    T::Stream: Unpin + Send + 'static,
{
    tracing::debug!(
        "Starting HTTP health checks of endpoint {:?} in cluster {:?}",
        endpoint.endpoint,
        endpoint.cluster
    );

    let (tcp_client, interval_waiter) = dependencies;

    let tcp_checker = TcpChecker::<_, MAX_PAYLOAD_BUFFER_SIZE> { tcp_client, config: protocol_config };
    let check_loop =
        HealthCheckerLoop::new(endpoint, cluster_config, sender, stop_signal, interval_waiter, tcp_checker);

    check_loop.spawn()
}

struct TcpChecker<T, const MAX_PAYLOAD_BUFFER_SIZE: usize> {
    tcp_client: T,
    config: TcpHealthCheck,
}

impl<T, const MAX_PAYLOAD_BUFFER_SIZE: usize> ProtocolChecker for TcpChecker<T, MAX_PAYLOAD_BUFFER_SIZE>
where
    T: TcpClient + Send,
    T::Stream: AsyncRead + AsyncWrite + Unpin + Send,
{
    type Response = ();

    async fn check(&mut self) -> Result<Self::Response, Error> {
        let mut stream = self.tcp_client.connect().await?;

        if let Some(send_payload) = &self.config.send {
            stream.write_all(send_payload).await?;
        }

        if !self.config.receive.is_empty() {
            let mut matcher = PayloadMatcher::<_, MAX_PAYLOAD_BUFFER_SIZE>::new(&mut stream, &self.config.receive);
            return matcher.try_match().await;
        }

        Ok(())
    }

    fn process_response(
        &self,
        _endpoint: &EndpointId,
        counter: &mut HealthStatusCounter,
        _response: &Self::Response,
    ) -> Option<orion_configuration::config::cluster::HealthStatus> {
        counter.add_success()
    }
}

// See the description of the pattern matcher:
// https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/core/v3/health_check.proto#envoy-v3-api-msg-config-core-v3-healthcheck-tcphealthcheck
struct PayloadMatcher<'a, T, const MAX_PAYLOAD_BUFFER_SIZE: usize>
where
    T: AsyncRead + Unpin,
{
    buffer: Vec<u8>,
    stream: &'a mut T,
    payloads: &'a [Vec<u8>],
    payload_size: usize,
}

impl<'a, T, const MAX_PAYLOAD_BUFFER_SIZE: usize> PayloadMatcher<'a, T, MAX_PAYLOAD_BUFFER_SIZE>
where
    T: AsyncRead + Unpin,
{
    fn new(stream: &'a mut T, payloads: &'a [Vec<u8>]) -> Self {
        let payload_size = payloads.iter().map(Vec::len).sum();
        Self { buffer: Vec::new(), stream, payloads, payload_size }
    }

    async fn try_match(&'a mut self) -> Result<(), Error> {
        let mut more_bytes = 1024_usize;

        if self.payload_size == 0 {
            return Ok(());
        }

        // This algorithm just keeps reading data until the payload matches
        // or there is a timeout. It could be improved by discarding the
        // buffer if the head of the payload is not found, and only reading
        // the remaining bytes if the payload partially matches.
        // However, the complexity of that code doesn't seem justified given
        // the small benefits, and would require extensive unit tests.

        self.recv_exact(self.payload_size).await?;

        loop {
            if self.matches() {
                return Ok(());
            }
            self.recv_at_most(more_bytes).await?;

            // Let's be increasingly hungry for more data until we reach the maximum
            more_bytes = more_bytes.saturating_mul(2);
        }
    }

    fn matches(&self) -> bool {
        let mut index = 0;

        for payload in self.payloads {
            if payload.is_empty() {
                continue;
            }

            let Some(buffer) = &self.buffer.get(index..) else {
                tracing::error!("Unexpected out-of-bounds error when verifying the TCP payload in health checker");
                return false;
            };

            let Some(payload_index) = buffer.windows(payload.len()).position(|window| window == payload) else {
                return false;
            };

            index += payload_index;
        }

        true
    }

    async fn recv_at_most(&mut self, bytes_to_read: usize) -> Result<(), Error> {
        if self.buffer.len() >= MAX_PAYLOAD_BUFFER_SIZE || bytes_to_read == 0 {
            return Err("payload buffer too big".into());
        }

        let prev_size = self.buffer.len();
        let new_size = MAX_PAYLOAD_BUFFER_SIZE.min(prev_size + bytes_to_read);

        self.buffer.resize(new_size, 0);

        let received = self.stream.read(&mut self.buffer[prev_size..]).await?;

        if received == 0 {
            return Err("end of stream".into());
        }

        self.buffer.resize(prev_size + received, 0);

        Ok(())
    }

    async fn recv_exact(&mut self, bytes_to_read: usize) -> Result<(), Error> {
        let prev_size = self.buffer.len();
        self.buffer.resize(prev_size + bytes_to_read, 0);

        self.stream.read_exact(&mut self.buffer[prev_size..]).await?;

        Ok(())
    }
}
