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

use std::time::Duration;

use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    metrics::{MeterProvider, PeriodicReader},
    runtime,
};
use tracing::info;

use crate::Result;

pub fn init(endpoint: String, push_interval: Duration) -> Result<()> {
    info!(
        "pushing opentelemetry metrics to {endpoint} every {:.4} seconds",
        push_interval.as_secs_f32()
    );
    let export_config = opentelemetry_otlp::ExportConfig {
        endpoint,
        ..opentelemetry_otlp::ExportConfig::default()
    };
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_export_config(export_config)
        .build_metrics_exporter(
            Box::new(opentelemetry_sdk::metrics::reader::DefaultAggregationSelector::new()),
            Box::new(opentelemetry_sdk::metrics::reader::DefaultTemporalitySelector::new()),
        )?;
    let reader = PeriodicReader::builder(exporter, runtime::Tokio)
        .with_interval(Duration::from_secs(3))
        .build();
    let meter_provider = MeterProvider::builder().with_reader(reader).build();
    super::init_and_set_global_meter_provider(meter_provider)
}
