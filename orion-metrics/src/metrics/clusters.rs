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
use std::{sync::OnceLock, thread::ThreadId};

pub static UPSTREAM_RQ_TOTAL: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static UPSTREAM_RQ_ACTIVE: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();

pub static UPSTREAM_RQ_TIMEOUT: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static UPSTREAM_RQ_PER_TRY_TIMEOUT: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static UPSTREAM_RQ_RETRY: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();

// Currently, we use the `http::uri::Authority` type to identify endpoints.
// Each thread runs a separate Hyper client instance for each endpoint,
// so the shard ID is derived from a combination of the thread ID and the authority.
// Metrics are aggregated using the `ShardedU64` type.

pub static UPSTREAM_CX_TOTAL: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static UPSTREAM_CX_IDLE_TIMEOUT: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static UPSTREAM_CX_DESTROY: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static UPSTREAM_CX_ACTIVE: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static UPSTREAM_CX_CONNECT_FAIL: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static UPSTREAM_CX_CONNECT_TIMEOUT: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();

#[cfg(feature = "metrics")]
pub(crate) fn init_clusters_metrics() {
    init_observable_counter!(UPSTREAM_RQ_TOTAL, "cluster", "upstream_rq_total", "Total number of upstream requests");
    init_observable_gauge!(UPSTREAM_RQ_ACTIVE, "cluster", "upstream_rq_active", "Number of active upstream requests");
    init_observable_counter!(UPSTREAM_RQ_TIMEOUT, "cluster", "upstream_rq_timeout", "Total upstream request timeouts");
    init_observable_counter!(
        UPSTREAM_RQ_PER_TRY_TIMEOUT,
        "cluster",
        "upstream_rq_per_try_timeout",
        "Total upstream request timeouts per try"
    );
    init_observable_counter!(UPSTREAM_RQ_RETRY, "cluster", "upstream_rq_retry", "Total upstream request retries");
    init_observable_counter!(UPSTREAM_CX_TOTAL, "cluster", "upstream_cx_total", "Total upstream connections");
    init_observable_counter!(
        UPSTREAM_CX_CONNECT_FAIL,
        "cluster",
        "upstream_cx_connect_fail",
        "Total upstream connection failures"
    );
    init_observable_counter!(
        UPSTREAM_CX_CONNECT_TIMEOUT,
        "cluster",
        "upstream_cx_connect_timeout",
        "Total upstream connection timeouts"
    );
    init_observable_counter!(
        UPSTREAM_CX_IDLE_TIMEOUT,
        "cluster",
        "upstream_cx_idle_timeout",
        "Total upstream connections idle timeout"
    );
    init_observable_counter!(
        UPSTREAM_CX_DESTROY,
        "cluster",
        "upstream_cx_destroy",
        "Total upstream connections destroyed"
    );
    init_observable_gauge!(UPSTREAM_CX_ACTIVE, "cluster", "upstream_cx_active", "Number of active connections");
}
