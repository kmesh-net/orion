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

use std::collections::{HashMap, HashSet};

//use ::http::{header, StatusCode};
use axum::extract::{Query, State};
use orion_metrics::{
    metrics::{clusters, http, listeners, server, server::update_server_metrics, tcp, tls},
    sharded::ShardedU64,
};
use prometheus::{Encoder, IntCounterVec, IntGaugeVec, Opts, Registry, TextEncoder};
use serde::Deserialize;

use crate::admin::AdminState;
use ::http::{header::HeaderMap, StatusCode};
use opentelemetry::KeyValue;
use std::hash::Hash;
use tracing::{debug, warn};

/// Populates a Prometheus `IntCounterVec` by reading from a `ShardedU64`.
fn populate_counter_vec<S: Eq + Hash>(
    metric_source: &ShardedU64<S>,
    prom_metric: &IntCounterVec,
    label_keys: &[String],
) {
    let data = metric_source.load_all();
    for (key_values, value) in data {
        let key_map: HashMap<_, _> =
            key_values.into_iter().map(|kv| (kv.key.to_string(), kv.value.to_string())).collect();

        let label_values: Vec<&str> =
            label_keys.iter().map(|k| key_map.get(k).map(String::as_str).unwrap_or("")).collect();

        prom_metric.with_label_values(&label_values).inc_by(value);
    }
}

/// Populates a Prometheus `IntGaugeVec` by reading from a `ShardedU64`.
fn populate_gauge_vec<S: Eq + Hash>(metric_source: &ShardedU64<S>, prom_metric: &IntGaugeVec, label_keys: &[String]) {
    let data = metric_source.load_all();
    for (key_values, value) in data {
        let key_map: HashMap<_, _> =
            key_values.into_iter().map(|kv| (kv.key.to_string(), kv.value.to_string())).collect();

        let label_values: Vec<&str> =
            label_keys.iter().map(|k| key_map.get(k).map(String::as_str).unwrap_or("")).collect();

        prom_metric.with_label_values(&label_values).set(value as i64);
    }
}

/// Extracts all unique, sorted label keys from a given counter's data.
fn get_label_keys(data: &HashMap<Vec<KeyValue>, u64>) -> Vec<String> {
    let mut keys = HashSet::new();
    for kvs in data.keys() {
        for kv in kvs {
            keys.insert(kv.key.to_string());
        }
    }
    let mut sorted_keys: Vec<String> = keys.into_iter().collect();
    sorted_keys.sort();
    sorted_keys
}

/// A macro to process and register a metric if its source is initialized.
macro_rules! process_metric {
    ($registry:expr, $source:expr, $metric_type:ty, $populate_fn:ident) => {
        if let Some(counter) = $source.get() {
            let data = counter.value.load_all();
            if !data.is_empty() {
                let label_keys = get_label_keys(&data);
                let label_keys_str: Vec<&str> = label_keys.iter().map(|s| s.as_str()).collect();
                let prom_metric = <$metric_type>::new(
                    Opts::new(format!("{}_{}", counter.prefix, counter.name), counter.descr),
                    &label_keys_str,
                )
                .unwrap();
                $registry.register(Box::new(prom_metric.clone())).unwrap();
                $populate_fn(&counter.value, &prom_metric, &label_keys);
            } else {
                warn!("[prometheus] No data for metric: {}.{}", counter.prefix, counter.name);
            }
        }
    };
}

pub(crate) async fn prometheus_handler(
    State(_): State<AdminState>,
) -> Result<(HeaderMap, String), (StatusCode, String)> {
    let registry = Registry::new();

    update_server_metrics();

    // listeners metrics
    process_metric!(registry, &listeners::DOWNSTREAM_CX_TOTAL, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &listeners::DOWNSTREAM_CX_DESTROY, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &listeners::DOWNSTREAM_CX_ACTIVE, IntGaugeVec, populate_gauge_vec);
    process_metric!(registry, &listeners::NO_FILTER_CHAIN_MATCH, IntCounterVec, populate_counter_vec);

    // clusters metrics
    process_metric!(registry, &clusters::UPSTREAM_RQ_TOTAL, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &clusters::UPSTREAM_RQ_ACTIVE, IntGaugeVec, populate_gauge_vec);
    process_metric!(registry, &clusters::UPSTREAM_RQ_TIMEOUT, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &clusters::UPSTREAM_RQ_PER_TRY_TIMEOUT, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &clusters::UPSTREAM_RQ_RETRY, IntCounterVec, populate_counter_vec);

    process_metric!(registry, &clusters::UPSTREAM_CX_TOTAL, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &clusters::UPSTREAM_CX_IDLE_TIMEOUT, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &clusters::UPSTREAM_CX_CONNECT_FAIL, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &clusters::UPSTREAM_CX_CONNECT_TIMEOUT, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &clusters::UPSTREAM_CX_DESTROY, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &clusters::UPSTREAM_CX_ACTIVE, IntGaugeVec, populate_gauge_vec);

    // http metrics
    process_metric!(registry, &http::DOWNSTREAM_CX_TOTAL, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &http::DOWNSTREAM_CX_SSL_TOTAL, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &http::DOWNSTREAM_CX_SSL_ACTIVE, IntGaugeVec, populate_gauge_vec);
    process_metric!(registry, &http::DOWNSTREAM_CX_DESTROY, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &http::DOWNSTREAM_CX_ACTIVE, IntGaugeVec, populate_gauge_vec);
    process_metric!(registry, &http::DOWNSTREAM_RQ_1XX, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &http::DOWNSTREAM_RQ_2XX, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &http::DOWNSTREAM_RQ_3XX, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &http::DOWNSTREAM_RQ_4XX, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &http::DOWNSTREAM_RQ_5XX, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &http::DOWNSTREAM_RQ_TOTAL, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &http::DOWNSTREAM_RQ_ACTIVE, IntGaugeVec, populate_gauge_vec);
    process_metric!(registry, &http::DOWNSTREAM_CX_RX_BYTES_TOTAL, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &http::DOWNSTREAM_CX_TX_BYTES_TOTAL, IntCounterVec, populate_counter_vec);

    // server metrics
    process_metric!(registry, &server::UPTIME, IntGaugeVec, populate_gauge_vec);
    process_metric!(registry, &server::CONCURRENCY, IntGaugeVec, populate_gauge_vec);
    process_metric!(registry, &server::MEMORY_HEAP_SIZE, IntGaugeVec, populate_gauge_vec);
    process_metric!(registry, &server::MEMORY_PHYSICAL_SIZE, IntGaugeVec, populate_gauge_vec);
    process_metric!(registry, &server::MEMORY_ALLOCATED, IntGaugeVec, populate_gauge_vec);

    // tcp metrics
    process_metric!(registry, &tcp::DOWNSTREAM_CX_TOTAL, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &tcp::DOWNSTREAM_CX_DESTROY, IntCounterVec, populate_counter_vec);
    process_metric!(registry, &tcp::DOWNSTREAM_CX_ACTIVE, IntGaugeVec, populate_gauge_vec);

    // tls
    process_metric!(registry, &tls::HANDSHAKES, IntCounterVec, populate_counter_vec);

    // Encode and return

    let encoder = TextEncoder::new();
    let mut buffer = vec![];

    encoder
        .encode(&registry.gather(), &mut buffer)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to encode metrics: {e}")))?;

    let body = String::from_utf8(buffer)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Metrics not valid UTF-8: {e}")))?;

    let mut headers = HeaderMap::new();
    let content_type = encoder
        .format_type()
        .parse()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to parse content type: {e}")))?;
    headers.insert(::http::header::CONTENT_TYPE, content_type);

    Ok((headers, body))
}


#[derive(Debug, Deserialize)]
pub(crate) struct QueryParams{
    usedonly: Option<String>
}

pub(crate) async fn stats_handler(Query(params): Query<QueryParams>,
    State(state): State<AdminState>,
) -> Result<(HeaderMap, String), (StatusCode, String)> {
    let mut response = String::new();
    debug!("Query params {params:?}");
    if params.usedonly.is_some(){
        response+= &format!("server.uptime: {}\n",orion_metrics::metrics::server::util::server_uptime());
        response+= &format!("server.state: {}\n",2);
        prometheus_handler(State(state)).await
    }else{
        Err((StatusCode::NOT_FOUND,"Something wrong".to_owned()))
    }
    
}
