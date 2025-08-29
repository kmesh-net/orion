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

pub mod attributes;
pub mod http_tracer;
pub mod request_id;
pub mod span_state;
pub mod trace_context;
pub mod trace_info;

#[cfg(feature = "tracing")]
use {
    opentelemetry::trace::TracerProvider,
    opentelemetry::KeyValue,
    opentelemetry_otlp::{SpanExporter, WithExportConfig},
    opentelemetry_sdk::{trace as sdktrace, Resource},
    std::sync::LazyLock,
    std::time::Duration,
    tracing::info,
};

use std::{collections::HashMap, sync::Arc};

use arc_swap::ArcSwap;
use compact_str::CompactString;
use opentelemetry::global::BoxedTracer;
use orion_configuration::config::network_filters::tracing::{SupportedTracingProvider, TracingConfig, TracingKey};
use parking_lot::Mutex;
use std::result::Result as StdResult;
use thiserror::Error;
#[cfg(feature = "tracing")]
use {compact_str::ToCompactString, orion_error::Result};

#[allow(dead_code)]
struct OtelConfig {
    service_name: CompactString,
    target_uri: String,
}

#[derive(Error, Debug)]
pub enum OtelConfigError {
    #[error("No tracing provider configured")]
    NoProvider,
    #[error("Tracing provider is not OpenTelemetry")]
    UnsupportedProvider,
    #[error("OpenTelemetry configuration is missing required GRPC service details")]
    MissingGrpcService,
}

type TracerMap = HashMap<TracingKey, Arc<BoxedTracer>>;

#[allow(dead_code)]
struct GlobalTracers {
    write_lock: Mutex<()>,
    tracers: ArcSwap<TracerMap>,
}

#[cfg(feature = "tracing")]
static GLOBAL_TRACERS: LazyLock<GlobalTracers> =
    LazyLock::new(|| GlobalTracers { write_lock: Mutex::new(()), tracers: ArcSwap::new(Arc::new(HashMap::new())) });

impl TryFrom<&TracingConfig> for OtelConfig {
    type Error = OtelConfigError;

    fn try_from(config: &TracingConfig) -> StdResult<Self, Self::Error> {
        // Extract the tracing provider, returning early if none is configured
        let provider = config.provider.as_ref().ok_or(OtelConfigError::NoProvider)?;

        // Match on the specific provider type

        let SupportedTracingProvider::OpenTelemetry(opentelemetry_config) = provider;
        // NOTE: This is an irrefutable pattern match, so no need for an else branch.
        // else {
        //     return Err(OtelConfigError::UnsupportedProvider);
        // };

        // Extract gRPC service configuration
        let grpc_service = opentelemetry_config.grpc_service.as_ref().ok_or(OtelConfigError::MissingGrpcService)?;

        // Extract Google gRPC configuration from the service
        let google_grpc = grpc_service.google_grpc.as_ref().ok_or(OtelConfigError::MissingGrpcService)?;

        // Build the final configuration
        Ok(OtelConfig {
            service_name: opentelemetry_config.service_name.clone(),
            target_uri: google_grpc.target_uri.clone(),
        })
    }
}

#[cfg(feature = "tracing")]
pub fn otel_remove_tracers_by_listeners(listeners: &[CompactString]) -> Result<()> {
    if !listeners.is_empty() {
        info!("OTEL Tracers: removing listeners configuration...");
        let _lock = GLOBAL_TRACERS.write_lock.lock();
        let map_arc = GLOBAL_TRACERS.tracers.load_full();
        let mut cur_map = (*map_arc).clone();
        info!("Removing tracer for listeners: {listeners:?}");
        cur_map.retain(|&TracingKey(name, _), _| !listeners.contains(&name.to_compact_string()));
        GLOBAL_TRACERS.tracers.store(Arc::new(cur_map));
        info!("OTEL Tracers: removal done.");
    }
    Ok(())
}

#[cfg(feature = "tracing")]
pub fn otel_update_tracers(tracers: HashMap<TracingKey, TracingConfig>) -> Result<()> {
    info!("OTEL Tracers: update listeners configuration...");
    let _lock = GLOBAL_TRACERS.write_lock.lock();
    let map_arc = GLOBAL_TRACERS.tracers.load_full();
    let mut cur_map = (*map_arc).clone();

    let listeners = tracers.keys().cloned().map(|TracingKey(name, _)| name).collect::<Vec<_>>();
    cur_map.retain(|TracingKey(name, _), _| !listeners.contains(name));

    // insert new tracers...
    for (key, ref config) in tracers {
        info!("Updating tracer with {key:?}...");
        insert_tracer(key.clone(), config, &mut cur_map)?;
    }

    GLOBAL_TRACERS.tracers.store(Arc::new(cur_map));
    info!("OTEL Tracers: update done.");
    Ok(())
}

/// Retrieves a tracer by name from the global TRACERS registry.
///
/// Returns an Arc reference to the BoxedTracer if found, None otherwise.
#[cfg(feature = "tracing")]
pub fn get_otel_tracer(key: &TracingKey) -> Option<Arc<BoxedTracer>> {
    let tracers_guard = GLOBAL_TRACERS.tracers.load();
    tracers_guard.get(key).cloned()
}

#[cfg(not(feature = "tracing"))]
#[inline]
pub fn get_otel_tracer(_key: &TracingKey) -> Option<Arc<BoxedTracer>> {
    None
}

#[cfg(feature = "tracing")]
fn insert_tracer(
    key: TracingKey,
    config: &TracingConfig,
    hm: &mut HashMap<TracingKey, Arc<BoxedTracer>>,
) -> Result<()> {
    let Some(config): Option<OtelConfig> = config.try_into().map_err(|e| info!("could not start tracer: {e}")).ok()
    else {
        return Ok(());
    };

    // NOTE: Envoy configuration contains two distinct service names that serve different purposes:
    //
    // 1. Node service name (control plane): Defined in the bootstrap node configuration,
    //    used by the control plane (xDS) for service discovery, cluster identification,
    //    and management operations.
    //
    // 2. Tracing service name (data plane): Defined in the tracing configuration,
    //    used by the data plane for telemetry and observability, appearing in trace
    //    spans and metrics to identify this service in monitoring systems.
    //

    let resource = Resource::builder()
        .with_attribute(KeyValue::new("service.name", config.service_name.to_string()))
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build();

    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(config.target_uri.clone())
        .with_timeout(Duration::from_secs(3))
        .build()?;

    let provider =
        sdktrace::SdkTracerProvider::builder().with_batch_exporter(exporter).with_resource(resource.clone()).build();

    let tracer = provider.tracer("orion-tracing");

    info!("OTEL tracer initialized -> {}", config.target_uri);
    hm.insert(key, Arc::new(BoxedTracer::new(Box::new(tracer))));

    Ok(())
}
