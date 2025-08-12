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

pub mod bootstrap;
pub use bootstrap::Bootstrap;
pub mod cluster;
pub use cluster::Cluster;
pub mod core;
pub mod listener;
pub use listener::Listener;
pub mod listener_filters;
pub mod log;
pub use log::Log;
pub mod network_filters;
pub mod runtime;
pub use runtime::Runtime;
pub mod common;
pub mod secret;
pub mod transport;

pub(crate) mod util;

pub use crate::config::common::*;
use crate::{options::Options, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{fs::File, path::Path};

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct Config {
    #[serde(skip_serializing_if = "is_default", default)]
    pub runtime: Runtime,
    #[serde(skip_serializing_if = "is_default", default)]
    pub logging: Log,
    #[serde(skip_serializing_if = "is_default", default)]
    pub bootstrap: Bootstrap,
}

impl Config {
    fn apply_options(self, opt: &Options) -> Self {
        let runtime = self.runtime.update_from_env_and_options(opt);
        let max_cpus = num_cpus::get();
        if runtime.num_cpus() > max_cpus {
            tracing::warn!(
                max_cpus,
                ORION_GATEWAY_CORES = runtime.num_cpus(),
                "Requested more cores than available CPUs"
            );
        }
        if runtime.num_runtimes() > runtime.num_cpus() {
            tracing::warn!(
                runtime.num_cpus,
                ORION_GATEWAY_RUNTIMES = runtime.num_runtimes(),
                "Requested more runtimes than cores"
            );
        }
        Self { runtime, ..self }
    }

    #[cfg(not(feature = "envoy-conversions"))]
    pub fn new(opt: &Options) -> Result<Self> {
        deserialize_yaml(&opt.config).map(|conf| conf.apply_options(opt))
    }
}

pub fn deserialize_yaml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path)?;
    serde_path_to_error::deserialize(serde_yaml::Deserializer::from_reader(&file)).map_err(crate::Error::from)
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    use std::path::Path;

    use super::{deserialize_yaml, Bootstrap, Config};
    use crate::{
        config::{log::Log, runtime::Runtime},
        options::Options,
        Result,
    };
    use orion_data_plane_api::decode::from_serde_deserializer;
    pub use orion_data_plane_api::envoy_data_plane_api::envoy::config::bootstrap::v3::Bootstrap as EnvoyBootstrap;
    use orion_error::ResultExtension;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Wrapper(#[serde(deserialize_with = "orion_data_plane_api::decode::from_serde_deserializer")] EnvoyBootstrap);

    #[derive(Deserialize)]
    struct ShimConfig {
        #[serde(default)]
        pub runtime: Runtime,
        #[serde(default)]
        pub logging: Log,
        #[serde(default)]
        pub bootstrap: Option<Bootstrap>,

        pub envoy_bootstrap: Option<Wrapper>,
    }

    fn bootstrap_from_path_to_envoy_bootstrap(envoy_path: impl AsRef<Path>) -> Result<Bootstrap> {
        (|| -> Result<_> {
            let envoy_file = std::fs::File::open(&envoy_path).context("failed to open file")?;
            let mut track = serde_path_to_error::Track::new();
            let envoy: EnvoyBootstrap = from_serde_deserializer(serde_path_to_error::Deserializer::new(
                serde_yaml::Deserializer::from_reader(&envoy_file),
                &mut track,
            ))
            .with_context(|| format!("failed to deserialize {}", track.path().to_string()))?;
            Bootstrap::try_from(envoy).context("failed to convert into orion bootstrap")
        })()
        .with_context(|| format!("failed to read config from \"{}\"", envoy_path.as_ref().display()))
    }

    impl Config {
        pub fn new(opt: &Options) -> Result<Self> {
            let config = match (&opt.config_files.config, &opt.config_files.bootstrap_override) {
                (None, None) => return Err("no config file specified".into()),
                (None, Some(envoy_path)) => {
                    let bootstrap = bootstrap_from_path_to_envoy_bootstrap(envoy_path)?;
                    Self { runtime: Runtime::default(), logging: Log::default(), bootstrap }
                },
                (Some(config), maybe_override) => {
                    let ShimConfig { runtime, logging, bootstrap, envoy_bootstrap } = deserialize_yaml(&config)
                        .with_context(|| format!("failed to deserialize \"{}\"", config.display()))?;
                    let mut bootstrap = match (bootstrap, envoy_bootstrap) {
                        (None, None) => Bootstrap::default(),
                        (Some(b), None) => b,
                        (None, Some(envoy)) => Bootstrap::try_from(envoy.0)
                            .context("failed to convert envoy bootstrap to orion bootstrap")?,
                        (Some(_), Some(_)) => {
                            return Err("only one of `bootstrap` and `envoy_bootstrap` may be set".into())
                        },
                    };
                    if let Some(bootstrap_override) = maybe_override {
                        bootstrap = bootstrap_from_path_to_envoy_bootstrap(&bootstrap_override)?;
                    }
                    Self { runtime, logging, bootstrap }
                },
            };
            Ok(config.apply_options(opt))
        }
    }
    #[cfg(test)]
    mod tests {
        use crate::{config::Config, options::Options, Result};
        use tracing_test::traced_test;
        #[test]
        #[traced_test]
        fn roundtrip_configs() -> Result<()> {
            let paths = std::fs::read_dir("../orion-proxy/conf")?;

            for path in paths {
                let path = path?.path();

                if Some("yaml") == path.extension().map(|os| os.to_str().unwrap())
                    && path.file_name().is_some_and(|os| {
                        let as_str = os.to_str().unwrap();
                        as_str.starts_with("orion-") || as_str.starts_with("envoy-")
                    })
                {
                    tracing::info!("parsing {}", path.display());
                    let new_conf = Config::new(&if path
                        .file_name()
                        .is_some_and(|os| os.to_str().unwrap().starts_with("orion-"))
                    {
                        Options::from_path(path.clone())
                    } else {
                        Options::from_path_to_envoy(path.clone())
                    })?;
                    let serialized = serde_yaml::to_string(&new_conf)?;
                    tracing::info!("\n{serialized}\n");
                    // if !path.ends_with("new.yaml") {
                    //     let new_path = format!(
                    //         "../orion-proxy/conf/{}-new.yaml",
                    //         path.file_name()
                    //             .unwrap()
                    //             .to_str()
                    //             .unwrap()
                    //             .trim_end_matches(".yaml")
                    //             .replace("envoy-", "orion-")
                    //     );
                    //     std::fs::write(new_path, serialized.as_bytes())?;
                    // }
                    let deserialized: Config = serde_yaml::from_str(&serialized)?;
                    if new_conf != deserialized {
                        tracing::info!("\n{}\n", serde_yaml::to_string(&deserialized)?);
                        panic!("failed to roundtrip config transcoding")
                    }
                } else {
                    tracing::info!("skipping {}", path.display())
                }
            }

            Ok(())
        }
    }
}
