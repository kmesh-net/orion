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

use std::net::SocketAddr;

use axum::{extract::State, response::IntoResponse, routing::get, Router};
use opentelemetry_sdk::metrics::MeterProvider;
use prometheus::{Encoder, Registry, TextEncoder};
use tracing::{trace, warn};

use crate::Result;

pub fn init(addr: SocketAddr) -> Result<()> {
    let registry = prometheus::Registry::new();
    let reader = opentelemetry_prometheus::exporter()
        .with_registry(registry.clone())
        .build()?;
    let meter_provider = MeterProvider::builder().with_reader(reader).build();
    super::init_and_set_global_meter_provider(meter_provider)?;
    let fut = start_prometheus_server(addr, registry)?;
    tokio::spawn(async move {
        if let Err(e) = fut.await {
            tracing::error!("Prometheus dashboard failed with error: {e}");
        }
    });
    Ok(())
}

fn start_prometheus_server(
    addr: SocketAddr,
    registry: Registry,
) -> Result<impl std::future::Future<Output = Result<()>> + Send> {
    async fn get_handler(State(registry): State<prometheus::Registry>) -> axum::response::Response {
        trace!("prometheum got pinged");
        let encoder = TextEncoder::new();
        let metric_families = registry.gather();
        let mut result = Vec::new();
        if let Err(e) = encoder.encode(&metric_families, &mut result) {
            warn!("failed to encode metrics for prometheus because of error: \"{e}\"");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{e}"),
            )
                .into_response()
        } else {
            result.into_response()
        }
    }
    let app = Router::new().route("/metrics", get(get_handler).with_state(registry));
    Ok(async move {
        axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await?;
        Ok(())
    })
}
