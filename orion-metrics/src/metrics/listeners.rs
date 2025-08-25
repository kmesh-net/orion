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

use std::{sync::OnceLock, thread::ThreadId};

use crate::{metrics::Metric, sharded::ShardedU64};
#[cfg(feature = "metrics")]
use opentelemetry::global;
use opentelemetry::metrics::Histogram;

pub static DOWNSTREAM_CX_TOTAL: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_CX_DESTROY: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_CX_ACTIVE: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static NO_FILTER_CHAIN_MATCH: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static DOWNSTREAM_CX_LENGTH_MS: OnceLock<Histogram<u64>> = OnceLock::new();

#[cfg(feature = "metrics")]
pub(crate) fn init_listeners_metrics() {
    _ = DOWNSTREAM_CX_LENGTH_MS.set(
        global::meter("orion.listeners")
            .u64_histogram("downstream_cx_length_ms")
            .with_description("Duration of downstream connections in milliseconds")
            .build(),
    );

    init_observable_counter!(DOWNSTREAM_CX_TOTAL, "listeners", "downstream_cx_total", "Total downstream connections");
    init_observable_counter!(
        DOWNSTREAM_CX_DESTROY,
        "listeners",
        "downstream_cx_destroy",
        "Total destroyed downstream connections"
    );
    init_observable_counter!(
        NO_FILTER_CHAIN_MATCH,
        "listeners",
        "no_filter_chain_match",
        "Total connections with no filter chain match"
    );
    init_observable_gauge!(DOWNSTREAM_CX_ACTIVE, "listeners", "downstream_cx_active", "Total active connections");
}
