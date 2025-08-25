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
use std::{sync::OnceLock, thread::ThreadId};

#[cfg(feature = "metrics")]
use {opentelemetry::global, std::time::Instant};

#[cfg(feature = "metrics")]
static STARTUP_TIME: OnceLock<Instant> = OnceLock::new();

pub static UPTIME: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static CONCURRENCY: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static MEMORY_HEAP_SIZE: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static MEMORY_PHYSICAL_SIZE: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();
pub static MEMORY_ALLOCATED: OnceLock<Metric<ShardedU64<ThreadId>>> = OnceLock::new();

#[cfg(feature = "metrics")]
const SERVER_PREFIX: &str = "orion.server";

#[cfg(feature = "metrics")]
pub fn update_server_metrics() {
    let shard_id = std::thread::current().id();
    let uptime = util::server_uptime();
    let physical_memory = util::get_memory_physical_size().unwrap_or(0);
    let memory_heap_size = util::get_memory_heap_size().unwrap_or(physical_memory) as u64;
    let memory_allocated = util::get_memory_allocated().unwrap_or(physical_memory) as u64;

    UPTIME
        .get_or_init(|| Metric::new("server", "uptime", "Current server uptime in seconds", ShardedU64::new()))
        .value
        .store(uptime, shard_id, &[]);

    MEMORY_HEAP_SIZE
        .get_or_init(|| {
            Metric::new("server", "memory_heap_size", "Current memory heap size in bytes", ShardedU64::new())
        })
        .value
        .store(memory_heap_size as u64, shard_id, &[]);

    MEMORY_PHYSICAL_SIZE
        .get_or_init(|| Metric::new("server", "memory_physical_size", "Cu", ShardedU64::new()))
        .value
        .store(memory_heap_size as u64, shard_id, &[]);

    MEMORY_ALLOCATED
        .get_or_init(|| {
            Metric::new("server", "memory_allocated", "Current memory allocated in bytes", ShardedU64::new())
        })
        .value
        .store(memory_allocated, shard_id, &[]);
}

#[cfg(not(feature = "metrics"))]
pub fn update_server_metrics() {}

#[cfg(feature = "metrics")]
pub(crate) fn init_server_metrics(number_of_threads: usize) {
    _ = STARTUP_TIME.set(Instant::now());
    let shard_id = std::thread::current().id();

    CONCURRENCY
        .get_or_init(|| Metric::new("server", "concurrency", "Number of worker threads", ShardedU64::new()))
        .value
        .store(number_of_threads as u64, shard_id, &[]);

    update_server_metrics();

    global::meter(SERVER_PREFIX)
        .u64_observable_counter(UPTIME.wait().name)
        .with_description(UPTIME.wait().descr)
        .with_callback(move |observer| observer.observe(util::server_uptime(), &[]))
        .build();

    global::meter(SERVER_PREFIX)
        .u64_observable_counter(CONCURRENCY.wait().name)
        .with_description(CONCURRENCY.wait().descr)
        .with_callback(move |observer| observer.observe(number_of_threads as u64, &[]))
        .build();

    let physical_memory = util::get_memory_physical_size().unwrap_or(0);

    global::meter(SERVER_PREFIX)
        .u64_observable_gauge(MEMORY_HEAP_SIZE.wait().name)
        .with_description(MEMORY_HEAP_SIZE.wait().descr)
        .with_callback(move |observer| {
            observer.observe(util::get_memory_heap_size().unwrap_or(physical_memory) as u64, &[])
        })
        .build();

    global::meter(SERVER_PREFIX)
        .u64_observable_gauge(MEMORY_PHYSICAL_SIZE.wait().name)
        .with_description(MEMORY_PHYSICAL_SIZE.wait().descr)
        .with_callback(move |observer| observer.observe(physical_memory as u64, &[]))
        .build();

    global::meter(SERVER_PREFIX)
        .u64_observable_gauge(MEMORY_ALLOCATED.wait().name)
        .with_description(MEMORY_ALLOCATED.wait().descr)
        .with_callback(move |observer| {
            observer.observe(util::get_memory_allocated().unwrap_or(physical_memory) as u64, &[])
        })
        .build();
}

#[cfg(feature = "metrics")]
mod util {
    /// Return the physical memory allocated by the process.
    ///
    pub(crate) fn get_memory_physical_size() -> Option<usize> {
        memory_stats::memory_stats().map(|s| s.physical_mem)
    }

    /// Return the precise memory allocated on the heap.
    ///
    pub(crate) fn get_memory_allocated() -> Option<usize> {
        #[cfg(all(feature = "jemalloc", not(target_env = "msvc")))]
        {
            use tikv_jemalloc_ctl::{epoch, stats};
            epoch::advance().ok()?;
            let allocated_bytes = stats::allocated::mib().ok()?;
            allocated_bytes.read().ok()
        }
        #[cfg(not(all(feature = "jemalloc", not(target_env = "msvc"))))]
        {
            None
        }
    }

    /// Return the total size of the heap (active pages) managed by the allocator.
    ///
    pub(crate) fn get_memory_heap_size() -> Option<usize> {
        #[cfg(all(feature = "jemalloc", not(target_env = "msvc")))]
        {
            use tikv_jemalloc_ctl::{epoch, stats};
            epoch::advance().ok()?;
            let active_bytes = stats::active::mib().ok()?;
            active_bytes.read().ok()
        }
        #[cfg(not(all(feature = "jemalloc", not(target_env = "msvc"))))]
        {
            None
        }
    }

    pub(crate) fn server_uptime() -> u64 {
        use std::time::Instant;
        let start_up_time = super::STARTUP_TIME.get().copied().unwrap_or_else(Instant::now);
        start_up_time.elapsed().as_secs()
    }
}
