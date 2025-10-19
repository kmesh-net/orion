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

use orion_configuration::{config::Config, options::Options};
use orion_lib::{Result, RUNTIME_CONFIG};
use tracing::warn;

#[macro_use]
mod admin;
mod core_affinity;
mod proxy;
mod runtime;
mod signal;
mod xds_configurator;

pub fn run() -> Result<()> {
    let mut tracing_manager = proxy_tracing::TracingManager::new();

    let options = Options::parse_options();
    let Config { runtime, logging, access_logging, bootstrap } = Config::new(&options)?;

    RUNTIME_CONFIG.set(runtime).map_err(|_| "runtime config was somehow set before we had a chance to set it")?;

    tracing_manager.update(logging)?;

    #[cfg(target_os = "linux")]
    if !(caps::has_cap(None, caps::CapSet::Permitted, caps::Capability::CAP_NET_RAW)?) {
        tracing::warn!("CAP_NET_RAW is NOT available, SO_BINDTODEVICE will not work");
    }

<<<<<<< HEAD
    if let Err(e) = proxy::run_orion(bootstrap, access_logging) {
        warn!("Can't start orion {e:?}");
        Err(e)
    } else {
        Ok(())
    }
=======
    proxy::run_orion(bootstrap, access_logging);
    Ok(())
>>>>>>> origin/main
}

mod proxy_tracing {
    use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
    use tracing_subscriber::{
        fmt,
        fmt::format::{DefaultFields, Format},
        layer::Layered,
        reload,
        reload::Handle,
        EnvFilter, Registry,
    };

    use orion_configuration::config::LogConfig as LogConf;
    use orion_lib::Result;

    type RegistryLayer =
        fmt::Layer<Layered<reload::Layer<EnvFilter, Registry>, Registry>, DefaultFields, Format, NonBlocking>;
    type FilterReloadHandle = Handle<EnvFilter, Registry>;
    type LayerReloadHandle = Handle<
        fmt::Layer<Layered<reload::Layer<EnvFilter, Registry>, Registry>, DefaultFields, Format, NonBlocking>,
        Layered<reload::Layer<EnvFilter, Registry>, Registry>,
    >;

    pub struct TracingManager {
        guard: WorkerGuard,
        layer_reload_handle: LayerReloadHandle,
        filter_reload_handle: FilterReloadHandle,
    }

    impl TracingManager {
        pub fn new() -> Self {
            let level = EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .parse_lossy("");
            let (guard, layer_reload_handle, filter_reload_handle) = Self::init_tracing(Registry::default(), level);
            TracingManager { guard, filter_reload_handle, layer_reload_handle }
        }

        pub fn update(&mut self, log_conf: LogConf) -> Result<()> {
            // Update log level
            self.filter_reload_handle.modify(|filter| {
                *filter = EnvFilter::try_from_default_env().ok().or(log_conf.log_level).unwrap_or_else(|| {
                    EnvFilter::builder()
                        .with_default_directive(tracing_subscriber::filter::LevelFilter::ERROR.into())
                        .parse_lossy("")
                });
            })?;

            // Update tracing layer if necessary (stdout -> file)
            if let Some(log_file) = log_conf.log_file {
                self.layer_reload_handle.modify(|layer| {
                    let (new_guard, new_layer) = Self::file_layer(&log_file, log_conf.log_directory.as_ref());
                    *layer = new_layer;
                    self.guard = new_guard;
                })?;
            }

            Ok(())
        }

        fn init_tracing(
            registry: Registry,
            log_level: EnvFilter,
        ) -> (WorkerGuard, LayerReloadHandle, FilterReloadHandle) {
            use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

            let env_filter = EnvFilter::try_from_default_env().ok().or(Some(log_level)).unwrap_or_else(|| {
                EnvFilter::builder()
                    .with_default_directive(tracing_subscriber::filter::LevelFilter::ERROR.into())
                    .parse_lossy("")
            });

            // Start as an stdout layer by default, after reading the configuration this can be upgraded to a file layer
            let (guard, layer) = Self::stdout_layer();
            let (layer, layer_reload_handle) = reload::Layer::new(layer);

            let (env_filter, filter_reload_handle) = reload::Layer::new(env_filter);

            registry.with(env_filter).with(layer).init();
            (guard, layer_reload_handle, filter_reload_handle)
        }

        fn stdout_layer() -> (WorkerGuard, RegistryLayer) {
            let out = std::io::stdout();
            let is_terminal = std::io::IsTerminal::is_terminal(&out);
            let (non_blocking, guard) = tracing_appender::non_blocking(out);
            let mut std_layer = fmt::layer().with_writer(non_blocking).with_thread_names(true);

            if !is_terminal {
                std_layer = std_layer.with_ansi(false);
            }

            (guard, std_layer)
        }

        fn file_layer(filename: &str, log_directory: Option<&String>) -> (WorkerGuard, RegistryLayer) {
            let file_appender = tracing_appender::rolling::hourly(log_directory.unwrap_or(&".".into()), filename);
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
            let file_layer = fmt::layer().with_ansi(false).with_writer(non_blocking).with_thread_names(true);

            (guard, file_layer)
        }
    }
}
