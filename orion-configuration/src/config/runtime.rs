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

use serde::{Deserialize, Serialize};
use std::{
    env::var,
    fmt::Display,
    num::{NonZeroU32, NonZeroUsize},
    ops::Deref,
};

use crate::options::Options;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
pub struct Runtime {
    #[serde(default = "non_zero_num_cpus")]
    pub num_cpus: NonZeroUsize,
    #[serde(default = "one")]
    pub num_runtimes: NonZeroU32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub global_queue_interval: Option<NonZeroU32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub affinity_strategy: Option<Affinity>,
    //may be zero?
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub event_interval: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_io_events_per_tick: Option<NonZeroUsize>,
}

fn one() -> NonZeroU32 {
    NonZeroU32::MIN
}

impl Runtime {
    #[must_use]
    pub fn update_from_env_and_options(self, opt: &Options) -> Self {
        Runtime {
            num_cpus: var("ORION_GATEWAY_CORES")
                .ok()
                .and_then(|v| v.parse::<NonZeroUsize>().ok())
                .or(opt.num_cpus)
                .unwrap_or(self.num_cpus),

            num_runtimes: var("ORION_GATEWAY_RUNTIMES")
                .ok()
                .and_then(|v| v.parse::<NonZeroU32>().ok())
                .or(opt.num_runtimes)
                .unwrap_or(self.num_runtimes),

            global_queue_interval: var("ORION_RT_GLOBAL_QUEUE_INTERVAL")
                .ok()
                .and_then(|v| v.parse::<NonZeroU32>().ok())
                .or(opt.global_queue_interval)
                .or(self.global_queue_interval),

            event_interval: var("ORION_RT_EVENT_INTERVAL")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .or(opt.event_interval)
                .or(self.event_interval),

            max_io_events_per_tick: var("ORION_RT_MAX_IO_EVENT_PER_TICK")
                .ok()
                .and_then(|v| v.parse::<NonZeroUsize>().ok())
                .or(opt.max_io_events_per_tick)
                .or(self.max_io_events_per_tick),

            affinity_strategy: self.affinity_strategy,
        }
    }

    //upcasts to usize to make it easier to do math with it and num_cpus
    pub fn num_runtimes(&self) -> usize {
        self.num_runtimes.get() as usize
    }
    pub fn num_cpus(&self) -> usize {
        self.num_cpus.get()
    }
}

pub(crate) fn non_zero_num_cpus() -> NonZeroUsize {
    NonZeroUsize::try_from(num_cpus::get()).expect("found zero cpus")
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            num_cpus: non_zero_num_cpus(),
            num_runtimes: one(),
            global_queue_interval: None,
            event_interval: None,
            max_io_events_per_tick: None,
            affinity_strategy: None,
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Deserialize, Serialize)]
pub struct CoreId(usize);

impl CoreId {
    pub fn new(id: usize) -> CoreId {
        CoreId(id)
    }
}

impl Display for CoreId {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for CoreId {
    type Target = usize;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Deserialize, Serialize, PartialEq, Eq, Debug, Clone)]
#[serde(tag = "type", content = "map")]
pub enum Affinity {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "nodes")]
    Nodes(Vec<Vec<CoreId>>),
    #[serde(rename = "runtimes")]
    Runtimes(Vec<Vec<CoreId>>),
}
