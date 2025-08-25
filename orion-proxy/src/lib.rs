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

use orion_configuration::{
    config::{Config, Log as LogConf},
    options::Options,
};
use orion_lib::{Result, RUNTIME_CONFIG};
use tracing_appender::non_blocking::WorkerGuard;

#[macro_use]
mod core_affinity;
mod proxy;
mod runtime;
mod xds_configurator;

pub fn run() -> Result<()> {
    let options = Options::parse_options();
    let Config { runtime, logging, bootstrap } = Config::new(&options)?;
    RUNTIME_CONFIG.set(runtime).map_err(|_| "runtime config was somehow set before we had a chance to set it")?;
    let _guard = init_tracing(logging);
    #[cfg(target_os = "linux")]
    if !(caps::has_cap(None, caps::CapSet::Permitted, caps::Capability::CAP_NET_RAW)?) {
        tracing::warn!("CAP_NET_RAW is NOT available, SO_BINDTODEVICE will not work");
    }
    proxy::run_proxy(bootstrap)
}

#[cfg(feature = "console")]
fn init_tracing(_conf: LogConf) -> WorkerGuard {
    let (_non_blocking, guard) = tracing_appender::non_blocking(std::io::stdout());
    console_subscriber::init();
    guard
}

#[cfg(not(feature = "console"))]
fn init_tracing(log_conf: LogConf) -> WorkerGuard {
    use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, EnvFilter, Registry};
    let env_filter = EnvFilter::try_from_default_env().ok().or(log_conf.log_level).unwrap_or_else(|| {
        EnvFilter::builder()
            .with_default_directive(tracing_subscriber::filter::LevelFilter::ERROR.into())
            .parse_lossy("")
    });

    match log_conf.log_file.as_ref() {
        None => {
            let out = std::io::stdout();
            let is_terminal = std::io::IsTerminal::is_terminal(&out);
            let (non_blocking, guard) = tracing_appender::non_blocking(out);
            let mut std_layer = fmt::layer().with_writer(non_blocking).with_thread_names(true);

            if !is_terminal {
                std_layer = std_layer.with_ansi(false);
            }

            Registry::default().with(env_filter).with(std_layer).init();
            guard
        },
        Some(filename) => {
            let file_appender =
                tracing_appender::rolling::hourly(log_conf.log_directory.as_ref().unwrap_or(&".".into()), filename);
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
            let file_layer = fmt::layer().with_ansi(false).with_writer(non_blocking).with_thread_names(true);
            Registry::default().with(env_filter).with(file_layer).init();
            guard
        },
    }
}
