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
 * The test fixture constructs a mock HTTP stack, which inspects the requests and
 * reports them to the test cases. A queue of responses is used to reply to all
 * the requests that arrive from the health checker. No actual HTTP or TCP
 * connections are done. It's a bit more code, but worth in the long run.
 */

use std::{sync::Arc, time::Duration};

use http::{Request, Response, Version};
use parking_lot::Mutex;
use tokio::sync::mpsc;

use super::*;
use crate::{
    PolyBody, Result,
    clusters::health::{
        HealthStatus,
        checkers::tests::{TestFixture, deref},
    },
    listeners::http_connection_manager::TransactionContext,
};

/// Channels to report every time an HTTP request is made, `requests`,
/// and will respond with the items in `responses`.
struct HttpActionTrace {
    requests: mpsc::UnboundedSender<http::Request<BodyWithMetrics<PolyBody>>>,
    responses: mpsc::UnboundedReceiver<http::Response<PolyBody>>,
}

#[derive(Clone)]
struct MockHttpStack(Arc<Mutex<HttpActionTrace>>);

impl MockHttpStack {
    pub fn new(
        requests: mpsc::UnboundedSender<http::Request<BodyWithMetrics<PolyBody>>>,
        responses: mpsc::UnboundedReceiver<http::Response<PolyBody>>,
    ) -> Self {
        MockHttpStack(Arc::new(Mutex::new(HttpActionTrace { requests, responses })))
    }
}

impl<'a> RequestHandler<RequestExt<'a, Request<BodyWithMetrics<PolyBody>>>> for &MockHttpStack {
    async fn to_response(
        self,
        _trans_ctx: &TransactionContext,
        request: RequestExt<'a, Request<BodyWithMetrics<PolyBody>>>,
    ) -> Result<Response<PolyBody>> {
        let state = &mut self.0.lock();
        // Log this request
        state.requests.send(request.req).unwrap();
        // Return the predefined response, if any
        let response = state.responses.try_recv()?;
        Ok(response)
    }
}

struct HttpTestFixture {
    inner:
        TestFixture<HttpHealthCheck, MockHttpStack, http::Request<BodyWithMetrics<PolyBody>>, http::Response<PolyBody>>,
}

#[allow(clippy::panic)]
impl HttpTestFixture {
    pub fn new(http_version: http::Version, healthy_threshold: u16, unhealthy_threshold: u16) -> Self {
        let protocol_config = HttpHealthCheck {
            http_version: match http_version {
                http::Version::HTTP_11 => Codec::Http1,
                http::Version::HTTP_2 => Codec::Http2,
                _ => panic!("Unsupported HTTP version"),
            },
            ..Default::default()
        };
        let channel_builder =
            |_, request_sender, response_receiver| MockHttpStack::new(request_sender, response_receiver);
        Self { inner: TestFixture::new(protocol_config, healthy_threshold, unhealthy_threshold, channel_builder) }
    }

    pub fn enqueue_response(&self, code: http::StatusCode) {
        let response = hyper::Response::builder()
            .version(match self.inner.protocol_config.http_version {
                Codec::Http1 => Version::HTTP_11,
                Codec::Http2 => Version::HTTP_2,
            })
            .status(code)
            .body(PolyBody::default())
            .unwrap();
        self.inner.enqueue_response(response);
    }

    pub async fn request_expected(&mut self, timeout_value: Duration) -> http::Request<BodyWithMetrics<PolyBody>> {
        let req = self.inner.request_expected(timeout_value).await;

        assert_eq!(
            req.version(),
            match self.inner.protocol_config.http_version {
                Codec::Http1 => Version::HTTP_11,
                Codec::Http2 => Version::HTTP_2,
            }
        );
        assert_eq!(req.method(), self.inner.protocol_config.method, "Wrong HTTP method");
        assert_eq!(req.uri().host(), Some(self.inner.endpoint.cluster.as_str()), "Wrong HTTP host");

        req
    }

    pub fn start(&mut self) {
        self.inner.start(|endpoint, cluster_config, protocol_config, channel, sender, dependencies| {
            try_spawn_http_health_checker_impl(
                endpoint,
                cluster_config,
                protocol_config,
                false,
                channel,
                sender,
                dependencies,
            )
            .unwrap()
        });
    }
}

deref!(HttpTestFixture => inner as TestFixture<HttpHealthCheck, MockHttpStack, http::Request<BodyWithMetrics<PolyBody>>, http::Response<PolyBody>>);

const HEALTHY_THRESHOLD: u16 = 5;
const UNHEALTHY_THRESHOLD: u16 = 10;

#[tokio::test]
async fn http1() {
    let mut test = HttpTestFixture::new(http::Version::HTTP_11, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);
    test.enqueue_response(http::StatusCode::OK);
    test.start();
    let _req = test.request_expected(Duration::from_millis(100)).await;
    let _update = test.health_update_expected(HealthStatus::Healthy, Duration::from_millis(100)).await;
    test.stop().await;
}

#[tokio::test]
async fn http2() {
    let mut test = HttpTestFixture::new(http::Version::HTTP_2, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);
    test.enqueue_response(http::StatusCode::OK);
    test.start();
    let _req = test.request_expected(Duration::from_millis(100)).await;
    let _update = test.health_update_expected(HealthStatus::Healthy, Duration::from_millis(100)).await;
    test.stop().await;
}

#[tokio::test]
async fn failure() {
    let mut test = HttpTestFixture::new(http::Version::HTTP_11, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);
    test.enqueue_response(http::StatusCode::NOT_FOUND);
    test.start();
    let _req = test.request_expected(Duration::from_millis(100)).await;
    let _update = test.health_update_expected(HealthStatus::Unhealthy, Duration::from_millis(100)).await;
    test.stop().await;
}

#[tokio::test]
async fn transition_to_healthy() {
    let mut test = HttpTestFixture::new(http::Version::HTTP_11, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);

    // Start with a failure, then transition to healthy
    test.enqueue_response(http::StatusCode::NOT_FOUND);
    for _ in 0..HEALTHY_THRESHOLD {
        test.enqueue_response(http::StatusCode::OK);
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
    let mut test = HttpTestFixture::new(http::Version::HTTP_11, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);

    // Start with a failure, then transition to unhealthy
    test.enqueue_response(http::StatusCode::OK);
    for _ in 0..UNHEALTHY_THRESHOLD {
        test.enqueue_response(http::StatusCode::NOT_FOUND);
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

#[tokio::test]
#[allow(clippy::single_range_in_vec_init)]
async fn expected_and_retriable_statuses() {
    use http::StatusCode;

    // The initial expected status (2XX) should cause a healthy update,
    // then the retriable status (3XX) should cause a transition if the unhealthy threshold is surpassed.
    for (expected_status, retriable_status) in [
        (StatusCode::OK, StatusCode::MULTIPLE_CHOICES),
        (StatusCode::CREATED, StatusCode::MOVED_PERMANENTLY),
        (StatusCode::ACCEPTED, StatusCode::FOUND),
        (StatusCode::NON_AUTHORITATIVE_INFORMATION, StatusCode::SEE_OTHER),
        (StatusCode::NO_CONTENT, StatusCode::NOT_MODIFIED),
        (StatusCode::RESET_CONTENT, StatusCode::USE_PROXY),
        (StatusCode::PARTIAL_CONTENT, StatusCode::TEMPORARY_REDIRECT),
        (StatusCode::MULTI_STATUS, StatusCode::PERMANENT_REDIRECT),
    ] {
        let mut test = HttpTestFixture::new(http::Version::HTTP_11, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);

        // This is what it is being tested:
        test.protocol_config.expected_statuses = vec![200..300];
        test.protocol_config.retriable_statuses = vec![300..400];

        test.enqueue_response(expected_status);
        for _ in 0..UNHEALTHY_THRESHOLD {
            test.enqueue_response(retriable_status);
            test.tick(); // allow the checker to advance one interval
        }

        test.start();
        let _req = test.request_expected(Duration::from_millis(100)).await;
        let _update = test.health_update_expected(HealthStatus::Healthy, Duration::from_millis(100)).await;
        for _ in 0..UNHEALTHY_THRESHOLD {
            let _req = test.request_expected(Duration::from_millis(100)).await;
        }
        let _update = test.health_update_expected(HealthStatus::Unhealthy, Duration::from_millis(100)).await;

        test.stop().await;
    }
}

#[tokio::test]
#[allow(clippy::single_range_in_vec_init)]
async fn unexpected_status() {
    let mut test = HttpTestFixture::new(http::Version::HTTP_11, HEALTHY_THRESHOLD, UNHEALTHY_THRESHOLD);

    // Expect any 2XX except 200 OK
    test.protocol_config.expected_statuses = vec![201..300];

    test.enqueue_response(http::StatusCode::ACCEPTED); // 202 expected
    test.enqueue_response(http::StatusCode::OK); // 200 unexpected
    test.tick();

    test.start();
    let _req = test.request_expected(Duration::from_millis(100)).await;
    let _update = test.health_update_expected(HealthStatus::Healthy, Duration::from_millis(100)).await;
    let _req = test.request_expected(Duration::from_millis(100)).await;
    let _update = test.health_update_expected(HealthStatus::Unhealthy, Duration::from_millis(100)).await;
    test.stop().await;
}
