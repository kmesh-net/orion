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

#![allow(dead_code)]

pub mod metric_exporters;

use std::time::Duration;

use metric_exporters::*;
use opentelemetry::{global, KeyValue};

use crate::Result;

pub fn init_metrics_exporter(config: Config) -> Result<()> {
    use Config::*;
    match config {
        StdOut => stdout::init(),
        Prometheus { socket_address } => prometheus::init(socket_address),
        OtelPusher { endpoint, push_interval_seconds } => {
            otel::init(endpoint, Duration::from_secs_f32(push_interval_seconds))
        },
    }
}

const ENVOY_LISTENER_NAME_KEY: &str = "envoy-listener-name";

//todo: these binds us to the opentelemetry crate which is alpha/beta and not great quality.
// we probably want to replace these with a more loose wrapper.
// but the question is whether or not tagging is even something that is supported or wanted
// by HQ. If not, atomic counters work. If so, a lockfree hashmap both paired with an opentelemetry observer.

pub fn add_listener_connection(listener_name: String) {
    let meter = global::meter("listeners");
    let counter = meter.u64_counter("connections").init();
    counter.add(1, &[KeyValue::new(ENVOY_LISTENER_NAME_KEY, listener_name)]);
}

pub fn add_proxy_error(listener_name: String) {
    let meter = global::meter("http");
    let counter = meter.u64_counter("proxy-errors").init();
    counter.add(1, &[KeyValue::new(ENVOY_LISTENER_NAME_KEY, listener_name)]);
}
