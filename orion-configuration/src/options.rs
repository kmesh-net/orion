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

use std::{
    num::{NonZeroU32, NonZeroUsize},
    path::PathBuf,
};

use clap::Parser;

#[derive(Debug, Clone, clap::Args)]
#[group(required = true, multiple = true)]
pub struct ConfigFiles {
    #[arg(help = "Configuration file", short = 'c', long = "config")]
    pub config: Option<PathBuf>,
    #[cfg(feature = "envoy-conversions")]
    #[arg(help = "Override bootstrap with Envoy bootstrap from a file ", long = "with-envoy-bootstrap")]
    pub bootstrap_override: Option<PathBuf>,
}

#[derive(Parser, Debug, Clone)]
pub struct Options {
    #[clap(flatten)]
    pub config_files: ConfigFiles,
    #[arg(help = "Number of CPU to use", short = 'C', long = "num-cpus")]
    pub num_cpus: Option<NonZeroUsize>,
    #[arg(help = "Number of Tokio runtimes to use", short = 'R', long = "num-runtimes")]
    pub num_runtimes: Option<NonZeroU32>,
    #[arg(help = "Number of Service threads to use", short = 'S', long = "num-service-threads")]
    pub num_service_threads: Option<NonZeroU32>,
    #[arg(help = "Tokio global queue interval (ticks)", long = "global-queue-interval")]
    pub global_queue_interval: Option<NonZeroU32>,
    #[arg(help = "Tokio event interval (ticks)", long = "event-interval")]
    pub event_interval: Option<u32>,
    #[arg(help = "Tokio max. io events per ticks", long = "max-io-events-per-tick")]
    pub max_io_events_per_tick: Option<NonZeroUsize>,
    #[arg(
        help = "Specify the queue length (channel) toward the clusters manager",
        long = "clusters-manager-queue-length"
    )]
    pub clusters_manager_queue_length: Option<NonZeroUsize>,
    #[arg(
        help = "Comma delimited non-empty list of core ids to use for worker nodes",
        long = "core-ids",
        num_args = 1..,
        value_delimiter = ',',
    )]
    pub core_ids: Option<Vec<usize>>,
}

impl Options {
    pub fn parse_options() -> Self {
        Options::parse()
    }

    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self {
            config_files: ConfigFiles { config: Some(path.into()), bootstrap_override: None },
            num_cpus: None,
            num_runtimes: None,
            num_service_threads: None,
            global_queue_interval: None,
            event_interval: None,
            max_io_events_per_tick: None,
            clusters_manager_queue_length: None,
            core_ids: None,
        }
    }
    pub fn from_path_to_envoy(path: impl Into<PathBuf>) -> Self {
        Self {
            config_files: ConfigFiles { config: None, bootstrap_override: Some(path.into()) },
            num_cpus: None,
            num_runtimes: None,
            num_service_threads: None,
            global_queue_interval: None,
            event_interval: None,
            max_io_events_per_tick: None,
            clusters_manager_queue_length: None,
            core_ids: None,
        }
    }
}
