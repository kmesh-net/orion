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

use orion_configuration::config::network_filters::access_log::AccessLogConf;
use tracing_appender::{
    non_blocking::{NonBlocking, WorkerGuard},
    rolling::{RollingFileAppender, Rotation},
};

use super::{LoggerError, deferred_init};
use deferred_init::DeferredInit;

pub(crate) struct LogWriter {
    conf: AccessLogConf,
    handle: DeferredInit<Result<(NonBlocking, WorkerGuard), LoggerError>>,
}

impl LogWriter {
    pub(crate) fn config(&self) -> &AccessLogConf {
        &self.conf
    }

    pub(crate) fn new(index: usize, conf: AccessLogConf, rotation: Rotation, max_log_files: usize) -> Self {
        let handle = match conf {
            AccessLogConf::Stdout => DeferredInit::new(|| Ok(tracing_appender::non_blocking(std::io::stdout()))),
            AccessLogConf::Stderr => DeferredInit::new(|| Ok(tracing_appender::non_blocking(std::io::stderr()))),
            AccessLogConf::File(ref path) => {
                let path = path.clone();
                DeferredInit::new(move || {
                    let filename = if index == 0 { path.clone() } else { format!("{path}-{index}") };
                    match RollingFileAppender::builder()
                        .rotation(rotation)
                        .max_log_files(max_log_files)
                        .filename_prefix(filename)
                        .build("")
                    {
                        Ok(app) => Ok(tracing_appender::non_blocking(app)),
                        Err(e) => Err(LoggerError::InitializationError(format!(
                            "Failed to create RollingFileAppender for path: {path}. Error: {e}"
                        ))),
                    }
                })
            },
        };

        LogWriter { conf, handle }
    }

    #[inline]
    pub(crate) fn get_mut(&mut self) -> Option<&mut NonBlocking> {
        self.handle.get_mut().as_mut().ok().map(|(nb, _)| nb)
    }
}
