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

use crate::{metrics::Metric, sharded::ShardedU64};

#[cfg(feature = "metrics")]
use opentelemetry::global;
use opentelemetry::metrics::Histogram;
use std::{sync::OnceLock, thread::ThreadId};

pub static DOWNSTREAM_CX_TOTAL: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_CX_SSL_TOTAL: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_CX_SSL_ACTIVE: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_CX_DESTROY: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_CX_ACTIVE: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_CX_LENGTH_MS: OnceLock<Histogram<u64>> = OnceLock::new();

pub static DOWNSTREAM_RQ_1XX: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_RQ_2XX: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_RQ_3XX: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_RQ_4XX: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_RQ_5XX: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();

pub static DOWNSTREAM_RQ_TOTAL: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_RQ_ACTIVE: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_CX_RX_BYTES_TOTAL: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_CX_TX_BYTES_TOTAL: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();

#[cfg(feature = "metrics")]
pub(crate) fn init_http_metrics() {
    _ = DOWNSTREAM_CX_LENGTH_MS.set(
        global::meter("orion.http")
            .u64_histogram("downstream_cx_length_ms")
            .with_description("Duration of downstream HTTP connections in milliseconds")
            .build(),
    );

    init_observable_counter!(
        DOWNSTREAM_RQ_1XX,
        "http",
        "downstream_rq_1xx",
        "1xx responses to downstream HTTP requests"
    );
    init_observable_counter!(
        DOWNSTREAM_RQ_2XX,
        "http",
        "downstream_rq_2xx",
        "2xx responses to downstream HTTP requests"
    );
    init_observable_counter!(
        DOWNSTREAM_RQ_3XX,
        "http",
        "downstream_rq_3xx",
        "3xx responses to downstream HTTP requests"
    );
    init_observable_counter!(
        DOWNSTREAM_RQ_4XX,
        "http",
        "downstream_rq_4xx",
        "4xx responses to downstream HTTP requests"
    );
    init_observable_counter!(
        DOWNSTREAM_RQ_5XX,
        "http",
        "downstream_rq_5xx",
        "5xx responses to downstream HTTP requests"
    );
    init_observable_counter!(
        DOWNSTREAM_CX_TOTAL,
        "http",
        "downstream_cx_total",
        "Total number of downstream HTTP connections"
    );
    init_observable_counter!(
        DOWNSTREAM_CX_SSL_TOTAL,
        "http",
        "downstream_cx_ssl_total",
        "Total number of downstream HTTP connections with TLS"
    );
    init_observable_gauge!(
        DOWNSTREAM_CX_SSL_ACTIVE,
        "http",
        "downstream_cx_ssl_active",
        "Active downstream HTTP connections with TLS"
    );

    init_observable_counter!(
        DOWNSTREAM_RQ_TOTAL,
        "http",
        "downstream_rq_total",
        "Total number of downstream HTTP requests"
    );
    init_observable_gauge!(DOWNSTREAM_RQ_ACTIVE, "http", "downstream_rq_active", "Active downstream HTTP requests");

    init_observable_counter!(
        DOWNSTREAM_CX_DESTROY,
        "http",
        "downstream_cx_destroy",
        "Number of destroyed downstream HTTP connections"
    );
    init_observable_gauge!(DOWNSTREAM_CX_ACTIVE, "http", "downstream_cx_active", "Active downstream HTTP connections");

    init_observable_counter!(
        DOWNSTREAM_CX_RX_BYTES_TOTAL,
        "http",
        "downstream_cx_rx_bytes_total",
        "Total number of bytes received on downstream HTTP connections"
    );
    init_observable_counter!(
        DOWNSTREAM_CX_TX_BYTES_TOTAL,
        "http",
        "downstream_cx_tx_bytes_total",
        "Total number of bytes sent on downstream HTTP connections"
    );
}
