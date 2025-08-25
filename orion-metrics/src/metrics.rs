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

use tracing::info;

#[cfg(feature = "metrics")]
use crate::metrics::{
    clusters::init_clusters_metrics, http::init_http_metrics, listeners::init_listeners_metrics,
    server::init_server_metrics, tcp::init_tcp_metrics, tls::init_tls_metrics,
};

use crate::Metrics;
pub mod clusters;
pub mod http;
pub mod listeners;
pub mod server;
pub mod tcp;
pub mod tls;

pub struct Metric<T> {
    pub prefix: &'static str,
    pub name: &'static str,
    pub descr: &'static str,
    pub value: T,
}

impl<T> Metric<T> {
    #[allow(dead_code)]
    fn new(prefix: &'static str, name: &'static str, descr: &'static str, value: T) -> Self {
        Metric { prefix, name, descr, value }
    }
}

// This function initializes per-thread metrics based on the provided configuration.
// It must be called in the context of each thread that needs to collect metrics (e.g. Tokio threads)
#[cfg(feature = "metrics")]
pub fn init_per_thread_metrics(_metrics: &[Metrics]) {
    info!("Initializing per-thread metrics...");
}
#[cfg(not(feature = "metrics"))]
pub fn init_per_thread_metrics(_metrics: &[Metrics]) {
    info!("Metrics feature is disabled, skipping per-thread metrics initialization.");
}

// This function initializes global metrics based on the provided configuration. Must be called once at application startup.
//
#[cfg(feature = "metrics")]
pub fn init_global_metrics(_metrics: &[Metrics], number_of_threads: usize) {
    info!("Initializing global metrics...");
    init_tcp_metrics();
    init_tls_metrics();
    init_http_metrics();
    init_listeners_metrics();
    init_clusters_metrics();
    init_server_metrics(number_of_threads);
}

#[cfg(not(feature = "metrics"))]
pub fn init_global_metrics(_metrics: &[Metrics], _number_of_threads: usize) {
    info!("Metrics feature is disabled, skipping global metrics initialization.");
}
