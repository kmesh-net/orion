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

pub mod opentelemetry;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

static STATS: StatisticsHolder = StatisticsHolder::new();

// a ZST that exposes access to the static StatisticsHolder
// makes the exposed API a bit more rust-y and lets us swap the implementation out or extend it easier later on
// e.g. when we add a struct that requires obtaining a lock, or we wish to hand out handles which only
// update the atomics on drop to reduce the amount of inter-core syncs.
pub struct Statistics;
impl Statistics {
    pub fn listener() -> &'static ListenerStatistics {
        &STATS.listener
    }

    pub fn http() -> &'static HttpStatistics {
        &STATS.http
    }
}

pub struct StatisticsHolder {
    listener: ListenerStatistics,
    http: HttpStatistics,
}

impl StatisticsHolder {
    const fn new() -> Self {
        Self {
            listener: ListenerStatistics::new(),
            http: HttpStatistics::new(),
        }
    }
}

#[derive(Default, Debug)]
pub struct ListenerStatistics {
    total_connections: AtomicU64,
    active_connections: AtomicI64,
}

impl ListenerStatistics {
    const fn new() -> Self {
        Self {
            total_connections: AtomicU64::new(0),
            active_connections: AtomicI64::new(0),
        }
    }
    pub fn _add_connection(&self) {
        self.total_connections.fetch_add(1, Ordering::Relaxed);
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }
    pub fn remove_connection(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn total_connections(&self) -> u64 {
        self.total_connections.load(Ordering::Relaxed)
    }

    pub fn active_connections(&self) -> i64 {
        self.active_connections.load(Ordering::Relaxed)
    }
}
pub struct HttpStatistics {
    gateway_errors: AtomicU64,
}

impl HttpStatistics {
    const fn new() -> Self {
        Self {
            gateway_errors: AtomicU64::new(0),
        }
    }

    pub fn _increment_error(&self) {
        self.gateway_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn gateway_errors(&self) -> u64 {
        self.gateway_errors.load(Ordering::Relaxed)
    }
}
