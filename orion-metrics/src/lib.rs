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

#[macro_use]
pub mod macros;
pub mod metrics;
pub mod sharded;

use orion_configuration::config::{Bootstrap, metrics::StatsSink};
use serde::{Deserialize, Serialize};

#[cfg(feature = "metrics")]
use {
    opentelemetry::global,
    opentelemetry_otlp::{Protocol, WithExportConfig},
    opentelemetry_sdk::metrics::PeriodicReader,
    parking_lot::{Condvar, Mutex},
    std::sync::LazyLock,
    std::time::Duration,
    tracing::info,
};

#[cfg(feature = "metrics")]
const DEFAULT_EXPORT_PERIOD: std::time::Duration = std::time::Duration::from_secs(5);
#[cfg(feature = "metrics")]
static SETUP_BARRIER: LazyLock<(Mutex<bool>, Condvar)> = LazyLock::new(|| (Mutex::new(false), Condvar::new()));

pub struct VecMetrics(pub Vec<Metrics>);

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Metrics {
    pub prefix: String,
    pub endpoint: String,
    pub stat_prefix: Option<String>,
    pub export_period: Option<std::time::Duration>,
}

impl From<&Bootstrap> for VecMetrics {
    fn from(bootstrap: &Bootstrap) -> Self {
        let metrics: Vec<_> = bootstrap
            .stats_sinks
            .iter()
            .filter_map(|sink| match sink {
                StatsSink::OpenTelemetry(config) => {
                    let endpoint = config.grpc_service.google_grpc.as_ref().map(|g| g.target_uri.clone());
                    let stat_prefix = config.grpc_service.google_grpc.as_ref().map(|g| g.stat_prefix.clone());
                    let prefix = config.prefix.clone();
                    let export_period = bootstrap.stats_flush_interval;
                    endpoint.map(|endpoint| Metrics { prefix, endpoint, stat_prefix, export_period })
                },
            })
            .collect();

        Self(metrics)
    }
}

// This function launches the metrics exporter based on the provided configuration.
// It's designed to be called during the application startup to initialize the OpenTelemetry metrics exporter.
//
#[cfg(feature = "metrics")]
pub async fn launch_metrics_exporter(
    multi_metrics: &[Metrics],
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    info!("Launching OpenTelemetry exporter...");
    let mut provider_builder = opentelemetry_sdk::metrics::SdkMeterProvider::builder();

    for metrics in multi_metrics {
        info!("Building gRPC exporter {}, with endpoint {}...", metrics.prefix, metrics.endpoint);
        let exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(&metrics.endpoint)
            .with_protocol(Protocol::Grpc)
            .with_timeout(Duration::from_secs(5))
            .build()?;

        let exporter_period = metrics.export_period.unwrap_or(DEFAULT_EXPORT_PERIOD);

        info!("Building periodic reader, with period {:?}...", exporter_period);
        let periodic_reader = PeriodicReader::builder(exporter).with_interval(exporter_period).build();

        info!("Updating provider with the periodic_reader...");
        provider_builder = provider_builder.with_reader(periodic_reader);
    }

    info!("Building and setting global Meter provider...");
    let provider = provider_builder.build();
    global::set_meter_provider(provider);

    signal_setup_complete();
    info!("OpenTelemetry exporter launched successfully.");

    Ok(())
}

#[cfg(not(feature = "metrics"))]
pub async fn launch_metrics_exporter(
    _multi_metrics: &[Metrics],
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    Ok(())
}

#[cfg(feature = "metrics")]
fn signal_setup_complete() {
    let (lock, cvar) = &*SETUP_BARRIER;
    let mut setup_complete = lock.lock();
    *setup_complete = true;
    cvar.notify_all();
}

#[cfg(feature = "metrics")]
pub fn wait_for_metrics_setup() {
    let (lock, cvar) = &*SETUP_BARRIER;
    let mut setup_complete = lock.lock();
    while !*setup_complete {
        cvar.wait(&mut setup_complete);
    }
}
#[cfg(not(feature = "metrics"))]
pub fn wait_for_metrics_setup() {}
