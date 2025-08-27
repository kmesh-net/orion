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

use crate::core_affinity::{self, AffinityStrategy};
use orion_configuration::config::runtime::Affinity;
use orion_lib::runtime_config;
use orion_metrics::{metrics::init_per_thread_metrics, Metrics};
use std::{fmt::Display, ops::Deref};
use tokio::runtime::{Builder, Runtime};
use tracing::{info, warn};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeId(pub usize);

impl Display for RuntimeId {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for RuntimeId {
    type Target = usize;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub fn build_tokio_runtime(
    thread_name: &str,
    num_threads: usize,
    affinity_info: Option<(RuntimeId, Affinity)>,
    metrics: Option<Vec<Metrics>>,
) -> Runtime {
    let config = runtime_config();

    let thread_name: String = match affinity_info {
        Some((runtime_id, _)) => format!("{thread_name}_{runtime_id}"),
        None => thread_name.to_owned(),
    };

    if let Some((runtime_id, affinity)) = affinity_info {
        match affinity.run_strategy(runtime_id, num_threads) {
            Ok(aff) => {
                if let Err(err) = core_affinity::set_cores_for_current(&aff) {
                    warn!("{thread_name}: Couldn't pin thread to core {aff:?}: {err}");
                } else {
                    info!("{thread_name}: ST-runtime[{runtime_id}] pinned to core {aff:?}");
                }
            },
            Err(e) => {
                warn!("{thread_name}: Strategy: {e}");
            },
        }
    }

    let (mut builder, current_thread) = if num_threads <= 1 {
        let mut b = Builder::new_current_thread();
        b.enable_all();
        (b, true)
    } else {
        let mut b = Builder::new_multi_thread();
        b.worker_threads(num_threads).max_blocking_threads(num_threads).enable_all();
        (b, false)
    };

    config.global_queue_interval.map(|val| builder.global_queue_interval(val.into()));
    config.event_interval.map(|val| builder.event_interval(val));
    config.max_io_events_per_tick.map(|val| builder.max_io_events_per_tick(val.into()));

    // initialize per-thread metrics...
    //
    if let Some(metrics) = metrics {
        if current_thread {
            init_per_thread_metrics(&metrics);
        } else {
            builder.on_thread_start(move || init_per_thread_metrics(&metrics));
        }
    }

    #[allow(clippy::expect_used)]
    builder.thread_name(thread_name).build().expect("failed to build basic runtime")
}
