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

/*
 * The test fixture constructs a mock TCP stack, which inspects the requests and
 * reports them to the test cases. A queue of responses is used to reply to all
 * the requests that arrive from the health checker. No actual TCP
 * connections are done. It's a bit more code, but worth in the long run.
 */

use std::{collections::VecDeque, sync::Arc, task::Poll, time::Duration};

use futures::future::BoxFuture;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::clusters::health::{
    checkers::tests::{deref, TestFixture},
    HealthStatus,
};

use super::*;

/// Channels to report every time a TCP request is made, `requests`,
/// and will respond with the items in `responses`.
struct TcpActionTrace {
    connections_allowed: bool,
    connections: mpsc::UnboundedSender<bool>,
    requests: mpsc::UnboundedSender<Vec<u8>>,
    responses: mpsc::UnboundedReceiver<Vec<Vec<u8>>>,
}

#[derive(Clone)]
struct MockTcpClient(Arc<Mutex<TcpActionTrace>>);

impl MockTcpClient {
    pub fn new(
        connections_allowed: bool,
        connections: mpsc::UnboundedSender<bool>,
        requests: mpsc::UnboundedSender<Vec<u8>>,
        responses: mpsc::UnboundedReceiver<Vec<Vec<u8>>>,
    ) -> Self {
        MockTcpClient(Arc::new(Mutex::new(TcpActionTrace { connections_allowed, connections, requests, responses })))
    }
}

#[derive(Clone)]
struct MockTcpStream {
    actions: Arc<Mutex<TcpActionTrace>>,
    responses: VecDeque<Vec<u8>>,
    buffer: Vec<u8>,
}

impl MockTcpStream {
    pub fn new(actions: Arc<Mutex<TcpActionTrace>>) -> Self {
        let responses = {
            let state = &mut actions.lock();
            if let Ok(responses) = state.responses.try_recv() {
                responses.into()
            } else {
                VecDeque::new()
            }
        };
        MockTcpStream { actions, responses, buffer: Vec::new() }
    }

    pub fn read_into_buffer(&mut self, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        if self.buffer.is_empty() {
            let data = {
                let Some(data) = self.responses.pop_front() else {
                    return Poll::Ready(Ok(()));
                };
                data
            };
            self.buffer = data;
        }

        self.read_pending(buf);
        Poll::Ready(Ok(()))
    }

    fn read_pending(&mut self, buf: &mut tokio::io::ReadBuf<'_>) {
        let consumed = buf.remaining().min(self.buffer.len());
        buf.put_slice(&self.buffer[..consumed]);
        self.buffer.drain(0..consumed);
    }
}

impl AsyncRead for MockTcpStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.read_into_buffer(buf)
    }
}

impl AsyncWrite for MockTcpStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let state = &mut self.actions.lock();
        state.requests.send(buf.into()).unwrap();
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl TcpClient for MockTcpClient {
    type Stream = MockTcpStream;

    fn connect(&self) -> BoxFuture<'static, std::result::Result<Self::Stream, Error>> {
        {
            let state = self.0.lock();
            state.connections.send(state.connections_allowed).unwrap();
            if !state.connections_allowed {
                return Box::pin(futures::future::err("connections not allowed in this test".into()));
            }
        }
        Box::pin(futures::future::ready(Ok(MockTcpStream::new(Arc::clone(&self.0)))))
    }
}

struct TcpTestFixture {
    inner: TestFixture<TcpHealthCheck, MockTcpClient, Vec<u8>, Vec<Vec<u8>>>,
}

impl TcpTestFixture {
    const MAX_PAYLOAD_BUFFER_SIZE: usize = 1024;

    pub fn new(allow_connections: bool, healthy_threshold: u16, unhealthy_threshold: u16) -> Self {
        let protocol_config = TcpHealthCheck::default();
        let channel_builder = |connections_sender, request_sender, response_receiver| {
            MockTcpClient::new(allow_connections, connections_sender, request_sender, response_receiver)
        };
        Self { inner: TestFixture::new(protocol_config, healthy_threshold, unhealthy_threshold, channel_builder) }
    }

    pub fn start(&mut self) {
        self.inner.start(|endpoint, cluster_config, protocol_config, channel, sender, dependencies| {
            spawn_tcp_health_checker_impl::<_, _, { Self::MAX_PAYLOAD_BUFFER_SIZE }>(
                endpoint,
                cluster_config,
                protocol_config,
                channel,
                sender,
                dependencies,
            )
        });
    }

    pub fn set_send_text_payload(&mut self, text: &str) {
        self.inner.protocol_config.send = Some(text.as_bytes().to_vec());
    }

    pub fn add_receive_text_payload(&mut self, text: &str) {
        self.inner.protocol_config.receive.push(text.as_bytes().to_vec());
    }

    pub fn enqueue_response(&self, payloads: &[&[u8]]) {
        self.inner.enqueue_response(payloads.iter().copied().map(Vec::from).collect());
    }
}

deref!(TcpTestFixture => inner as TestFixture<TcpHealthCheck, MockTcpClient, Vec<u8>, Vec<Vec<u8>>>);

const HEALTHY_THRESHOLD: u16 = 5;
const UNHEALTHY_THRESHOLD: u16 = 10;

#[tokio::test]
async fn connect() {
    let mut test = TcpTestFixture::new(true, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);
    test.start();
    assert!(test.connection_expected(Duration::from_millis(100)).await);
    let _update = test.health_update_expected(HealthStatus::Healthy, Duration::from_millis(100)).await;
    test.stop().await;
}

#[tokio::test]
async fn failure() {
    let mut test = TcpTestFixture::new(false, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);
    test.start();
    assert!(!test.connection_expected(Duration::from_millis(100)).await);
    let _update = test.health_update_expected(HealthStatus::Unhealthy, Duration::from_millis(100)).await;
    test.stop().await;
}

#[tokio::test]
async fn matching() {
    fn test_with_payloads(payloads: &[&[u8]]) -> TcpTestFixture {
        let mut test = TcpTestFixture::new(true, 1, 1);

        test.set_send_text_payload("Charles Dickens");
        test.add_receive_text_payload("It was the best of times, ");
        test.add_receive_text_payload("it was the worst of times");

        test.enqueue_response(payloads);
        test.tick();

        test
    }

    const CHECKS: &[(&[&[u8]], HealthStatus)] = &[
        (
            &[b"foo", b"It was the best of times, ", b"bar", b"it was the worst of times", b"zarb"],
            HealthStatus::Healthy,
        ),
        (&[b"Call me Ishmael"], HealthStatus::Unhealthy),
        (
            &[
                b"it was the worst of times",
                b"It was the best of times, ",
                b"It was the best of times, ",
                b"bar",
                b"it was the worst of times",
                b"zarb",
            ],
            HealthStatus::Healthy,
        ),
        (
            &[
                b"it was the worst of times",
                b"it was the worst of times",
                b"It was the best of times, ",
                b"It was the best of times, ",
            ],
            HealthStatus::Unhealthy,
        ),
    ];

    let very_long_response = vec![b' '; TcpTestFixture::MAX_PAYLOAD_BUFFER_SIZE];
    let very_long_payload =
        vec![b"It was the best of times, ", very_long_response.as_slice(), b"it was the worst of times"];

    for (payloads, status) in CHECKS.iter().chain(&[(very_long_payload.as_slice(), HealthStatus::Unhealthy)]) {
        let mut test = test_with_payloads(payloads);

        test.start();

        assert!(test.connection_expected(Duration::from_millis(100)).await);
        assert_eq!(test.request_expected(Duration::from_millis(100)).await, b"Charles Dickens");
        let _update = test.health_update_expected(*status, Duration::from_millis(100)).await;

        test.stop().await;
    }
}

#[tokio::test]
async fn transition_to_healthy() {
    let mut test = TcpTestFixture::new(true, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);

    test.set_send_text_payload("Charles Dickens");
    test.add_receive_text_payload("It was the best of times, ");
    test.add_receive_text_payload("it was the worst of times");

    // Start with a failure, then transition to healthy
    test.enqueue_response(&[b"Call me Ishmael"]);

    for _ in 0..HEALTHY_THRESHOLD {
        test.enqueue_response(&[b"foo", b"It was the best of times, ", b"bar", b"it was the worst of times", b"zarb"]);
        test.tick(); // allow the checker to advance one interval
    }
    test.start();

    assert!(test.connection_expected(Duration::from_millis(100)).await);
    assert_eq!(test.request_expected(Duration::from_millis(100)).await, b"Charles Dickens");
    let _update = test.health_update_expected(HealthStatus::Unhealthy, Duration::from_millis(100)).await;
    for _ in 0..HEALTHY_THRESHOLD {
        assert!(test.connection_expected(Duration::from_millis(100)).await);
        assert_eq!(test.request_expected(Duration::from_millis(100)).await, b"Charles Dickens");
    }
    let _update = test.health_update_expected(HealthStatus::Healthy, Duration::from_millis(100)).await;
    test.stop().await;
}

#[tokio::test]
async fn transition_to_unhealthy() {
    let mut test = TcpTestFixture::new(true, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);

    test.set_send_text_payload("Charles Dickens");
    test.add_receive_text_payload("It was the best of times, it was the worst of times");

    // Start with a success, then transition to unhealthy
    test.enqueue_response(&["It was the best of times, it was the worst of times".as_bytes()]);
    for _ in 0..UNHEALTHY_THRESHOLD {
        test.enqueue_response(&[b"Call me Ishmael"]);
        test.tick(); // allow the checker to advance one interval
    }
    test.start();
    assert!(test.connection_expected(Duration::from_millis(100)).await);
    let _req = test.request_expected(Duration::from_millis(100)).await;
    let _update = test.health_update_expected(HealthStatus::Healthy, Duration::from_millis(100)).await;
    for _ in 0..HEALTHY_THRESHOLD {
        assert!(test.connection_expected(Duration::from_millis(100)).await);
        assert_eq!(test.request_expected(Duration::from_millis(100)).await, b"Charles Dickens");
    }
    let _update = test.health_update_expected(HealthStatus::Unhealthy, Duration::from_millis(100)).await;
    test.stop().await;
}
