// Copyright 2025 The kmesh Authors
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

use std::net::SocketAddr;

use opentelemetry::{global, metrics::MeterProvider as _};
use opentelemetry_sdk::metrics::MeterProvider;
use serde::Deserialize;

use crate::{observability::Statistics, Result};

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum Config {
    StdOut,
    Prometheus {
        socket_address: SocketAddr,
    },
    OtelPusher {
        endpoint: String,
        #[serde(default = "default_push_interval_seconds")]
        push_interval_seconds: f32,
    }, //endpoint takes a string, but should be URI?
}

//need to use a function for this because of https://github.com/serde-rs/serde/issues/368
// grumble grumble
fn default_push_interval_seconds() -> f32 {
    5.0
}

pub fn register_observable_metrics(provider: &MeterProvider) -> Result<()> {
    let meter = provider.meter("AtomicObservables");
    meter
        .u64_observable_counter("non_2xx")
        .with_description(
            "total amount of replies with a status code other than 2xx send by this proxy",
        )
        .with_callback(|i| i.observe(Statistics::http().gateway_errors(), &[]))
        .try_init()?;
    meter
        .u64_observable_counter("n_connections")
        .with_description("total amount of connections seen by this proxy")
        .with_callback(|i| i.observe(Statistics::listener().total_connections(), &[]))
        .try_init()?;
    meter
        .i64_observable_gauge("active_connections")
        .with_description("current number of inbound connections")
        .with_callback(|i| i.observe(Statistics::listener().active_connections(), &[]))
        .try_init()?;
    Ok(())
}

pub fn init_and_set_global_meter_provider(meter_provider: MeterProvider) -> Result<()> {
    crate::observability::opentelemetry::register_observable_metrics(&meter_provider)?;
    global::set_meter_provider(meter_provider);
    Ok(())
}

pub mod otel;

#[cfg(not(feature = "metrics-stdout"))]
pub mod stdout {
    use crate::Result;
    pub fn init() -> Result<()> {
        Err("Tried to use stdout logger for metrics, but this version is not compiled with --features metric-stdout".into())
    }
}

#[cfg(feature = "metrics-stdout")]
pub mod stdout;
#[cfg(not(feature = "metrics-prometheus"))]
pub mod prometheus {
    use std::net::SocketAddr;

    use crate::Result;

    pub fn init(addr: SocketAddr) -> Result<()> {
        Err(format!("Tried to use prometheus logger for metrics on {addr:?}, but this version is not compiled with --features metric-prometheus").into())
    }
}

#[cfg(feature = "metrics-prometheus")]
pub mod prometheus;
