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

use crate::core_affinity::{self, AffinityStrategy};
use orion_lib::runtime_config;
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

pub fn build_tokio_runtime(num_threads: usize, runtime_id: RuntimeId) -> Runtime {
    let thread_name = "proxytask";
    let config = runtime_config();
    if num_threads == 1 {
        if let Some(affinity) = &config.affinity_strategy {
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

        let mut builder = Builder::new_current_thread();
        builder.enable_all();

        config.global_queue_interval.map(|val| builder.global_queue_interval(val.into()));
        config.event_interval.map(|val| builder.event_interval(val));
        config.max_io_events_per_tick.map(|val| builder.max_io_events_per_tick(val.into()));
        #[allow(clippy::expect_used)]
        builder.thread_name(format!("{thread_name}{}", runtime_id.0)).build().expect("failed to build basic runtime")
    } else {
        if let Some(affinity) = &config.affinity_strategy {
            match affinity.run_strategy(runtime_id, num_threads) {
                Ok(aff) => {
                    if let Err(err) = core_affinity::set_cores_for_current(&aff) {
                        warn!("{thread_name}: Couldn't pin thread to core {aff:?}: {err}");
                    } else {
                        info!("{thread_name} MT-{num_threads}-runtime[{runtime_id}] pinned to cores {aff:?}");
                    }
                },
                Err(e) => {
                    warn!("{thread_name}: Strategy: {e}");
                },
            }
        }

        let mut builder = Builder::new_multi_thread();
        builder.enable_all().worker_threads(num_threads).max_blocking_threads(num_threads);

        config.global_queue_interval.map(|val| builder.global_queue_interval(val.into()));
        config.event_interval.map(|val| builder.event_interval(val));
        config.max_io_events_per_tick.map(|val| builder.max_io_events_per_tick(val.into()));

        let name = thread_name.to_owned();
        #[allow(clippy::expect_used)]
        builder
            .thread_name_fn(move || format!("{name}{}", runtime_id.0))
            .build()
            .expect("failed to build threaded runtime")
    }
}
