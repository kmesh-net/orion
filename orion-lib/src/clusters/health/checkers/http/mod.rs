// SPDX-FileCopyrightText: © 2025 Huawei Cloud Computing Technologies Co., Ltd
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 Huawei Cloud Computing Technologies Co., Ltd
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

use std::ops::Range;
use std::sync::Arc;

use http::uri::{Authority, PathAndQuery, Scheme};
use http::{Response, Version};
use orion_configuration::config::cluster::health_check::{ClusterHealthCheck, Codec, HttpHealthCheck};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;

use super::checker::{IntervalWaiter, ProtocolChecker, WaitInterval};
// use crate::clusters::cluster::HyperService;
use crate::clusters::health::checkers::checker::HealthCheckerLoop;
use crate::clusters::health::counter::HealthStatusCounter;
use crate::clusters::health::{EndpointHealthUpdate, EndpointId, HealthStatus};
use crate::listeners::http_connection_manager::RequestHandler;
use crate::transport::request_context::RequestWithContext;
use crate::transport::HttpChannel;
use crate::{Error, HttpBody};

#[derive(Debug, thiserror::Error)]
#[error("invalid HTTP status range")]
// TODO: This error type is defined for future HTTP status range validation functionality
pub struct InvalidHttpStatusRange;

/// Spawns an HTTP health checker and returns its handle. Must be called from a Tokio runtime context.
pub fn try_spawn_http_health_checker(
    endpoint: EndpointId,
    cluster_config: ClusterHealthCheck,
    protocol_config: HttpHealthCheck,
    channel: HttpChannel,
    sender: mpsc::Sender<EndpointHealthUpdate>,
    stop_signal: Arc<Notify>,
) -> Result<JoinHandle<Result<(), Error>>, Error> {
    // This is a dumb structure that only creates the HTTP client.
    let interval_waiter = IntervalWaiter;
    try_spawn_http_health_checker_impl(
        endpoint,
        cluster_config,
        protocol_config,
        channel.is_https(),
        sender,
        stop_signal,
        (channel, interval_waiter),
    )
}

/// Actual implementation of `spawn_http_health_checker()`, with `dependencies` containing the
/// injected HTTP stack builder and interval waiter.
fn try_spawn_http_health_checker_impl<H, W>(
    endpoint: EndpointId,
    cluster_config: ClusterHealthCheck,
    protocol_config: HttpHealthCheck,
    is_https: bool,
    sender: mpsc::Sender<EndpointHealthUpdate>,
    stop_signal: Arc<Notify>,
    dependencies: (H, W),
) -> Result<JoinHandle<Result<(), Error>>, Error>
where
    W: WaitInterval + Send + 'static,
    H: Send + 'static,
    for<'a> &'a H: RequestHandler<RequestWithContext<'static, HttpBody>>,
{
    tracing::debug!(
        "Starting HTTP health checks of endpoint {:?} in cluster {:?}",
        endpoint.endpoint,
        endpoint.cluster
    );

    let (http_client, interval_waiter) = dependencies;

    let http_version = match protocol_config.http_version {
        Codec::Http1 => Version::HTTP_11,
        Codec::Http2 => Version::HTTP_2,
    };

    let scheme = if is_https { Scheme::HTTPS } else { Scheme::HTTP };

    let host = protocol_config.host(&endpoint.cluster)?;
    let host_name = host.to_string();
    let uri = build_uri(scheme, host, protocol_config.path.unwrap_or(PathAndQuery::from_static("")))?;

    let checker = HttpChecker {
        expected_statuses: protocol_config.expected_statuses,
        retriable_statuses: protocol_config.retriable_statuses,
        http_version,
        method: protocol_config.method,
        host: host_name,
        uri,
        client: http_client,
    };

    let check_loop = HealthCheckerLoop::new(endpoint, cluster_config, sender, stop_signal, interval_waiter, checker);

    Ok(check_loop.spawn())
}

struct HttpChecker<H = HttpChannel> {
    expected_statuses: Vec<Range<u16>>,
    retriable_statuses: Vec<Range<u16>>,
    http_version: http::Version,
    method: http::Method,
    host: String,
    uri: http::Uri,
    client: H,
}

impl<H> ProtocolChecker for HttpChecker<H>
where
    H: Send,
    for<'a> &'a H: RequestHandler<RequestWithContext<'static, HttpBody>>,
{
    type Response = Response<HttpBody>;

    async fn check(&mut self) -> Result<Self::Response, Error> {
        let request = create_request(self.http_version, &self.method, &self.host, &self.uri)?;
        self.client.to_response(request).await
    }

    fn process_response(
        &self,
        endpoint: &EndpointId,
        counter: &mut HealthStatusCounter,
        response: &Self::Response,
    ) -> Option<HealthStatus> {
        let status_code = response.status();
        let default_expectation = self.expected_statuses.is_empty() && status_code == http::StatusCode::OK;

        if default_expectation || status_in_ranges(&self.expected_statuses, status_code) {
            // Expected statuses count as a success, and take priority
            tracing::debug!("Response for cluster {:?}: success {}", endpoint.endpoint, status_code);
            counter.add_success()
        } else if status_in_ranges(&self.retriable_statuses, status_code) {
            // Retriable statuses count as a failure, but don't change the status immediately
            tracing::debug!("Response for cluster {:?}: retriable failure {}", endpoint.endpoint, status_code);
            counter.add_failure()
        } else {
            // Unexpected statuses immediately cause the endpoint to be unhealthy
            tracing::debug!("Response for cluster {:?}: failure {}", endpoint.endpoint, status_code);
            counter.add_failure_ignore_threshold()
        }
    }
}

fn status_in_ranges(ranges: &[Range<u16>], status: http::StatusCode) -> bool {
    ranges.iter().any(|range| range.contains(&status.as_u16()))
}

fn build_uri(scheme: Scheme, host: Authority, path: PathAndQuery) -> Result<http::Uri, http::Error> {
    hyper::Uri::builder().scheme(scheme).authority(host).path_and_query(path).build()
}

fn create_request(
    http_version: http::Version,
    method: &http::Method,
    host: &str,
    uri: &http::Uri,
) -> Result<RequestWithContext<'static, HttpBody>, http::Error> {
    let req = http::Request::builder().version(http_version).method(method).uri(uri);
    let req = if http_version < http::Version::HTTP_2 { req.header("Host", host) } else { req };
    let req = req.header("User-Agent", "orion/health-checks");
    Ok(RequestWithContext::new(req.body(HttpBody::default())?))
}
