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

/*
 * The test fixture constructs a mock gRPC stack, which inspects the requests and
 * reports them to the test cases. A queue of responses is used to reply to all
 * the requests that arrive from the health checker. No actual HTTP or TCP
 * connections are done. It's a bit more code, but worth in the long run.
 */

use std::{sync::Arc, time::Duration};

use futures::future::BoxFuture;
use orion_xds::grpc_deps::Response;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::clusters::health::{
    HealthStatus,
    checkers::tests::{TestFixture, deref},
};

use super::*;

/// Channels to report every time a gRPC request is made, `requests`,
/// and will respond with the items in `responses`.
struct GrpcActionTrace {
    requests: mpsc::UnboundedSender<HealthCheckRequest>,
    responses: mpsc::UnboundedReceiver<Response<HealthCheckResponse>>,
}

#[derive(Clone)]
struct MockGrpcChannel(Arc<Mutex<GrpcActionTrace>>);

impl MockGrpcChannel {
    pub fn new(
        requests: mpsc::UnboundedSender<HealthCheckRequest>,
        responses: mpsc::UnboundedReceiver<Response<HealthCheckResponse>>,
    ) -> Self {
        MockGrpcChannel(Arc::new(Mutex::new(GrpcActionTrace { requests, responses })))
    }
}

impl GrpcHealthChannel for MockGrpcChannel {
    fn check(
        &'_ mut self,
        request: HealthCheckRequest,
    ) -> BoxFuture<'_, Result<Response<HealthCheckResponse>, TonicStatus>> {
        let state = Arc::clone(&self.0);
        Box::pin(async move {
            let state = &mut state.lock();
            // Log this request
            state.requests.send(request).unwrap();
            // Return the predefined response, if any
            let response = state.responses.try_recv().unwrap();
            Ok(response)
        })
    }
}

struct GrpcTestFixture {
    inner: TestFixture<GrpcHealthCheck, MockGrpcChannel, HealthCheckRequest, Response<HealthCheckResponse>>,
}

#[allow(clippy::panic)]
impl GrpcTestFixture {
    pub fn new(healthy_threshold: u16, unhealthy_threshold: u16) -> Self {
        let protocol_config = GrpcHealthCheck::default();
        let channel_builder =
            |_, request_sender, response_receiver| MockGrpcChannel::new(request_sender, response_receiver);
        Self { inner: TestFixture::new(protocol_config, healthy_threshold, unhealthy_threshold, channel_builder) }
    }

    pub fn start(&mut self) {
        self.inner.start(|endpoint, cluster_config, protocol_config, channel, sender, dependencies| {
            spawn_grpc_health_checker_impl(endpoint, cluster_config, protocol_config, channel, sender, dependencies)
        });
    }

    pub fn enqueue_response(&self, status: ServingStatus) {
        let mut response = HealthCheckResponse::default();
        response.set_status(status);
        self.inner.enqueue_response(Response::new(response));
    }
}

deref!(GrpcTestFixture => inner as TestFixture<GrpcHealthCheck, MockGrpcChannel, HealthCheckRequest, Response<HealthCheckResponse>>);

const HEALTHY_THRESHOLD: u16 = 5;
const UNHEALTHY_THRESHOLD: u16 = 10;

#[tokio::test]
async fn success() {
    let mut test = GrpcTestFixture::new(HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);
    test.enqueue_response(ServingStatus::Serving);
    test.start();
    let _req = test.request_expected(Duration::from_millis(100)).await;
    let _update = test.health_update_expected(HealthStatus::Healthy, Duration::from_millis(100)).await;
    test.stop().await;
}

#[tokio::test]
async fn failure() {
    for failure_code in [ServingStatus::NotServing, ServingStatus::ServiceUnknown, ServingStatus::Unknown] {
        let mut test = GrpcTestFixture::new(HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);
        test.enqueue_response(failure_code);
        test.start();
        let _req = test.request_expected(Duration::from_millis(100)).await;
        let _update = test.health_update_expected(HealthStatus::Unhealthy, Duration::from_millis(100)).await;
        test.stop().await;
    }
}

#[tokio::test]
async fn transition_to_healthy() {
    let mut test = GrpcTestFixture::new(HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);

    // Start with a failure, then transition to healthy
    test.enqueue_response(ServingStatus::NotServing);
    for _ in 0..HEALTHY_THRESHOLD {
        test.enqueue_response(ServingStatus::Serving);
        test.tick(); // allow the checker to advance one interval
    }
    test.start();
    let _req = test.request_expected(Duration::from_millis(100)).await;
    let _update = test.health_update_expected(HealthStatus::Unhealthy, Duration::from_millis(100)).await;
    for _ in 0..HEALTHY_THRESHOLD {
        let _req = test.request_expected(Duration::from_millis(100)).await;
    }
    let _update = test.health_update_expected(HealthStatus::Healthy, Duration::from_millis(100)).await;
    test.stop().await;
}

#[tokio::test]
async fn transition_to_unhealthy() {
    let mut test = GrpcTestFixture::new(HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);

    // Start with a failure, then transition to unhealthy
    test.enqueue_response(ServingStatus::Serving);
    for _ in 0..UNHEALTHY_THRESHOLD {
        test.enqueue_response(ServingStatus::NotServing);
        test.tick(); // allow the checker to advance one interval
    }
    test.start();
    let _req = test.request_expected(Duration::from_millis(100)).await;
    let _update = test.health_update_expected(HealthStatus::Healthy, Duration::from_millis(100)).await;
    for _ in 0..HEALTHY_THRESHOLD {
        let _req = test.request_expected(Duration::from_millis(100)).await;
    }
    let _update = test.health_update_expected(HealthStatus::Unhealthy, Duration::from_millis(100)).await;
    test.stop().await;
}
