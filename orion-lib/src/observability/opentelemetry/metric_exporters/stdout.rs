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

use opentelemetry_sdk::{
    metrics::{MeterProvider, PeriodicReader},
    runtime,
};

use crate::Result;

pub fn init() -> Result<()> {
    let exporter = opentelemetry_stdout::MetricsExporterBuilder::default().build();
    let reader = PeriodicReader::builder(exporter, runtime::Tokio).build();
    let meter_provider = MeterProvider::builder().with_reader(reader).build();
    super::init_and_set_global_meter_provider(meter_provider)
}
