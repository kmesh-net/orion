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

use std::{future::Future, sync::Arc, time::Duration};

use orion_configuration::config::cluster::{health_check::ClusterHealthCheck, HealthStatus};
use pingora_timeout::fast_timeout::fast_timeout;
use rand::{distributions::Uniform, thread_rng, Rng};
use tokio::{
    select,
    sync::{mpsc, Notify},
    task::JoinHandle,
};

use crate::{
    clusters::health::{counter::HealthStatusCounter, EndpointHealthUpdate, EndpointId},
    Error,
};

use super::CurrentHealthStatus;

pub struct HealthCheckerLoop<W, P> {
    endpoint: EndpointId,
    cluster_config: ClusterHealthCheck,
    sender: mpsc::Sender<EndpointHealthUpdate>,
    stop_signal: Arc<Notify>,
    interval_waiter: W,
    checker: P,
}

impl<W, P> HealthCheckerLoop<W, P>
where
    W: WaitInterval + Send + 'static,
    P: ProtocolChecker + Send + 'static,
    P::Response: Send,
{
    pub fn new(
        endpoint: EndpointId,
        cluster_config: ClusterHealthCheck,
        sender: mpsc::Sender<EndpointHealthUpdate>,
        stop_signal: Arc<Notify>,
        interval_waiter: W,
        checker: P,
    ) -> Self {
        Self { endpoint, cluster_config, sender, stop_signal, interval_waiter, checker }
    }

    pub fn spawn(self) -> JoinHandle<Result<(), Error>> {
        tokio::spawn(self.run())
    }

    async fn run(mut self) -> Result<(), Error> {
        let mut health_status =
            HealthStatusCounter::new(self.cluster_config.healthy_threshold, self.cluster_config.unhealthy_threshold);

        // Initial jitter
        if wait_was_cancelled_opt(self.cluster_config.initial_jitter.map(get_random_duration), &self.stop_signal).await
        {
            return Ok(());
        }

        loop {
            tracing::debug!("Sending health check to {:?}", self.endpoint.endpoint);

            // Wait for the response or cancellation

            let check_result = select! {
                () = self.stop_signal.notified() => HealthCheckResult::Cancelled,
                result = fast_timeout(self.cluster_config.timeout, self.checker.check()) => {
                    if let Ok(response) = result {
                        HealthCheckResult::Response(response)
                    } else {
                        HealthCheckResult::Timeout
                    }
                }
            };

            let health_status_change = match check_result {
                HealthCheckResult::Timeout => {
                    tracing::debug!(
                        "Response for {:?} in cluster {}: timeout",
                        self.endpoint.endpoint,
                        self.endpoint.cluster
                    );
                    health_status.add_failure()
                },
                HealthCheckResult::Response(Err(err)) => {
                    tracing::debug!(
                        "Response for {:?} in cluster {}: error {}",
                        self.endpoint.endpoint,
                        self.endpoint.cluster,
                        err
                    );
                    health_status.add_failure()
                },
                HealthCheckResult::Response(Ok(response)) => {
                    self.checker.process_response(&self.endpoint, &mut health_status, &response)
                },
                HealthCheckResult::Cancelled => {
                    tracing::debug!(
                        "Stopping checks of endpoint {:?} in cluster {:?}",
                        self.endpoint.endpoint,
                        self.endpoint.cluster
                    );
                    return Ok(());
                },
            };

            let _ = self
                .sender
                .send(EndpointHealthUpdate {
                    endpoint: self.endpoint.clone(),
                    health: health_status.status().unwrap_or_default(),
                    changed: health_status_change.is_some(),
                })
                .await;

            if self
                .interval_waiter
                .wait_interval_was_cancelled(
                    &self.cluster_config,
                    match health_status_change {
                        Some(new_status) => CurrentHealthStatus::Edge(new_status),
                        None => CurrentHealthStatus::Unchanged(health_status.status()),
                    },
                    &self.stop_signal,
                )
                .await
            {
                return Ok(());
            }
        }
    }
}

pub trait ProtocolChecker {
    type Response;
    fn check(&mut self) -> impl Future<Output = Result<Self::Response, Error>> + Send;
    fn process_response(
        &self,
        endpoint: &EndpointId,
        counter: &mut HealthStatusCounter,
        response: &Self::Response,
    ) -> Option<HealthStatus>;
}

pub enum HealthCheckResult<Response> {
    Response(Result<Response, Error>),
    Timeout,
    Cancelled,
}

pub trait WaitInterval {
    /// Wait for the next interval of this checker, based on the configutation and the
    /// current health status. Returns `true` if it was cancelled during the wait.
    fn wait_interval_was_cancelled(
        &self,
        config: &ClusterHealthCheck,
        health_status: CurrentHealthStatus,
        stop_signal: &Notify,
    ) -> impl Future<Output = bool> + Send;
}

pub struct IntervalWaiter;

impl WaitInterval for IntervalWaiter {
    async fn wait_interval_was_cancelled(
        &self,
        config: &ClusterHealthCheck,
        health_status: CurrentHealthStatus,
        stop_signal: &Notify,
    ) -> bool {
        // 1. Base interval
        let mut interval = match health_status {
            CurrentHealthStatus::Edge(new_health_status) => {
                // a) Edge interval (transition from one status to another)
                match new_health_status {
                    HealthStatus::Healthy => config.healthy_edge_interval,
                    HealthStatus::Unhealthy => config.unhealthy_edge_interval,
                }
                .unwrap_or(config.interval)
            },
            CurrentHealthStatus::Unchanged(Some(HealthStatus::Unhealthy)) => {
                // b) Same status interval
                config.unhealthy_interval.unwrap_or(config.interval)
            },
            CurrentHealthStatus::Unchanged(_) => config.interval,
        };

        // 2. Add interval jitter
        if let Some(interval_jitter) = config.interval_jitter {
            interval += interval_jitter;
        }

        // 3. Add interval jitter percent
        interval += interval.mul_f32(config.interval_jitter_percent);

        wait_was_cancelled(interval, stop_signal).await
    }
}

/// Wait for the `interval`. Returns `true` if it was cancelled.
async fn wait_was_cancelled(interval: Duration, stop_signal: &Notify) -> bool {
    fast_timeout(interval, stop_signal.notified()).await.is_err()
}

/// If the option has a value, wait for the `interval`. If the option is empty, return immediately.
/// Returns `true` if it was cancelled during the wait.
async fn wait_was_cancelled_opt(interval: Option<Duration>, stop_signal: &Notify) -> bool {
    if let Some(interval) = interval {
        wait_was_cancelled(interval, stop_signal).await
    } else {
        false
    }
}

fn get_random_duration(max: Duration) -> Duration {
    thread_rng().sample(Uniform::new_inclusive(Duration::from_secs(0), max))
}
