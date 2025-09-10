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

use std::str::FromStr;

use orion_data_plane_api::envoy_data_plane_api::envoy::{
    config::accesslog::v3::{access_log::ConfigType, AccessLog as EnvoyAccessLog},
    extensions::access_loggers::{
        file::v3::FileAccessLog as EnvoyFileAccessLog,
        stream::v3::{StderrAccessLog as EnvoyStderrAccessLog, StdoutAccessLog as EnvoyStdoutAccessLog},
    },
};

use orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::SubstitutionFormatString as EnvoySubstitutionFormatString;

use orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::substitution_format_string::Format;

use orion_data_plane_api::envoy_data_plane_api::{
    envoy::extensions::access_loggers::{
        file::v3::file_access_log::AccessLogFormat as FileAccessLogFormat,
        stream::v3::{
            stderr_access_log::AccessLogFormat as StderrAccessLogFormat,
            stdout_access_log::AccessLogFormat as StdoutAccessLogFormat,
        },
    },
    google::protobuf::Any,
    prost::Message,
};

use orion_format::{LogFormatter, DEFAULT_ACCESS_LOG_FORMAT};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::config::{common::*, core::DataSource};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum AccessLogType {
    File,
    Stderr,
    Stdout,
    // Fluentd, // not implemented yet
    // HttpGrpc,
    // OpenTelemetry,
    // TcpGrpc,
    // Wasm,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum AccessLogConf {
    File(String),
    Stderr,
    Stdout,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AccessLog {
    pub config: AccessLogConf,
    pub logger: LogFormatter,
}

impl AccessLog {
    pub fn new(config: AccessLogConf, logger: LogFormatter) -> Self {
        AccessLog { config, logger }
    }

    pub fn get_config(&self) -> &AccessLogConf {
        &self.config
    }

    pub fn get_logger(&self) -> &LogFormatter {
        &self.logger
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SupportedAccessLog {
    File(EnvoyFileAccessLog),
    Stdout(EnvoyStdoutAccessLog),
    Stderr(EnvoyStderrAccessLog),
}

impl TryFrom<Any> for SupportedAccessLog {
    type Error = GenericError;

    fn try_from(typed_config: Any) -> Result<Self, Self::Error> {
        match typed_config.type_url.as_str() {
            "type.googleapis.com/envoy.extensions.access_loggers.file.v3.FileAccessLog" => {
                EnvoyFileAccessLog::decode(typed_config.value.as_slice()).map(Self::File)
            },
            "type.googleapis.com/envoy.extensions.access_loggers.stream.v3.StdoutAccessLog" => {
                EnvoyStdoutAccessLog::decode(typed_config.value.as_slice()).map(Self::Stdout)
            },
            "type.googleapis.com/envoy.extensions.access_loggers.stream.v3.StderrAccessLog" => {
                EnvoyStderrAccessLog::decode(typed_config.value.as_slice()).map(Self::Stderr)
            },
            _ => {
                return Err(GenericError::unsupported_variant(typed_config.type_url));
            },
        }
        .map_err(|e| {
            GenericError::from_msg_with_cause(format!("failed to parse protobuf for \"{}\"", typed_config.type_url), e)
        })
    }
}

impl FromStr for AccessLogType {
    type Err = GenericError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "envoy.access_loggers.file" => Ok(Self::File),
            "envoy.access_loggers.stderr" => Ok(Self::Stderr),
            "envoy.access_loggers.stdout" => Ok(Self::Stdout),
            // "envoy.access_loggers.tcp_grpc" => Ok(Self::TcpGrpc),
            // "envoy.access_loggers.wasm" => Ok(Self::Wasm),
            // "envoy.access_loggers.fluentd" => Ok(Self::Fluentd),
            // "envoy.access_loggers.http_grpc" => Ok(Self::HttpGrpc),
            // "envoy.access_loggers.open_telemetry" => Ok(Self::OpenTelemetry),
            _ => Err(GenericError::from_msg(format!("Error: unsupported access log type: {s}"))),
        }
    }
}

impl TryFrom<EnvoyAccessLog> for AccessLog {
    type Error = GenericError;

    fn try_from(value: EnvoyAccessLog) -> Result<Self, Self::Error> {
        let EnvoyAccessLog { name, filter, config_type } = value;
        unsupported_field!(filter)?;

        let log_type = name
            .parse::<AccessLogType>()
            .map_err(|e| GenericError::from_msg_with_cause("failed to parse access_log name", e))?;

        let (path, fmt) = match &log_type {
            AccessLogType::File => config_type
                .map(|x| {
                    let ConfigType::TypedConfig(typed_config) = x;
                    match SupportedAccessLog::try_from(typed_config)? {
                        SupportedAccessLog::File(file_access_log) => {
                            let fmt = file_access_log
                                .access_log_format
                                .map(|fmt| -> Result<SubstitutionFormatString, GenericError> {
                                    let FileAccessLogFormat::LogFormat(substitution_format_string) = fmt else {
                                        return Err(GenericError::unsupported_variant("only LogFormat is supported"));
                                    };

                                    SubstitutionFormatString::try_from(substitution_format_string)
                                })
                                .transpose()?;

                            Ok((Some(file_access_log.path), fmt))
                        },
                        _ => Err(GenericError::from_msg("Error: AccessLog configuration mismatch")),
                    }
                })
                .transpose(),
            AccessLogType::Stdout => config_type
                .map(|x| {
                    let ConfigType::TypedConfig(typed_config) = x;
                    match SupportedAccessLog::try_from(typed_config)? {
                        SupportedAccessLog::Stdout(stdout_access_log) => {
                            let fmt = stdout_access_log
                                .access_log_format
                                .map(|fmt| -> Result<SubstitutionFormatString, GenericError> {
                                    let StdoutAccessLogFormat::LogFormat(substitution_format_string) = fmt; // irrefutable pattern

                                    SubstitutionFormatString::try_from(substitution_format_string)
                                })
                                .transpose()?;
                            Ok((None, fmt))
                        },
                        _ => Err(GenericError::from_msg("Error: AccessLog configuration mismatch")),
                    }
                })
                .transpose(),
            AccessLogType::Stderr => config_type
                .map(|x| {
                    let ConfigType::TypedConfig(typed_config) = x;
                    match SupportedAccessLog::try_from(typed_config)? {
                        SupportedAccessLog::Stderr(stderr_access_log) => {
                            let fmt = stderr_access_log
                                .access_log_format
                                .map(|fmt| -> Result<SubstitutionFormatString, GenericError> {
                                    let StderrAccessLogFormat::LogFormat(substitution_format_string) = fmt; // irrefutable pattern

                                    SubstitutionFormatString::try_from(substitution_format_string)
                                })
                                .transpose()?;

                            Ok((None, fmt))
                        },
                        _ => Err(GenericError::from_msg("Error: AccessLog configuration mismatch")),
                    }
                })
                .transpose(),
        }?
        .unzip();

        let config = match (log_type, path.flatten()) {
            (AccessLogType::File, None) => Err(GenericError::from_msg("Error: path unspecified for file logger")),
            (AccessLogType::File, Some(f)) if f.is_empty() => {
                Err(GenericError::from_msg("Error: empty path for file logger"))
            },
            (AccessLogType::File, Some(f)) => Ok(AccessLogConf::File(f)),
            (AccessLogType::Stderr, _) => Ok(AccessLogConf::Stderr),
            (AccessLogType::Stdout, _) => Ok(AccessLogConf::Stdout),
        }?;

        let logger = match fmt.flatten() {
            Some(SubstitutionFormatString { format, omit_empty_values }) => {
                let format = format.as_ref().map(AsRef::as_ref).unwrap_or(DEFAULT_ACCESS_LOG_FORMAT);
                debug!("Received formatter {format}");
                let format = DEFAULT_ACCESS_LOG_FORMAT;
                orion_format::LogFormatter::try_new(format, omit_empty_values)
            },
            None => orion_format::LogFormatter::try_new(DEFAULT_ACCESS_LOG_FORMAT, false),
        }
        .map_err(|e| GenericError::from_msg(format!("Error: failed to create log formatter: {e}")))?;

        Ok(AccessLog { config, logger })
    }
}

struct SubstitutionFormatString {
    omit_empty_values: bool,
    format: Option<String>,
}

impl TryFrom<EnvoySubstitutionFormatString> for SubstitutionFormatString {
    type Error = GenericError;
    fn try_from(value: EnvoySubstitutionFormatString) -> Result<Self, Self::Error> {
        let EnvoySubstitutionFormatString { omit_empty_values, content_type, formatters, json_format_options, format } =
            value;
        unsupported_field!(
            // omit_empty_values,
            // format,
            content_type,
            formatters,
            json_format_options
        )?;

        let format = format
            .map(|f| match f {
                Format::TextFormat(text) => Ok(text),
                Format::JsonFormat(_) => Err(GenericError::unsupported_variant("JsonFormat")),
                Format::TextFormatSource(data_source) => {
                    let data_source = DataSource::try_from(data_source)?;
                    let bytes = data_source.to_bytes_blocking()?;
                    Ok(String::from_utf8(bytes)?)
                },
            })
            .transpose()?;

        Ok(SubstitutionFormatString { omit_empty_values, format })
    }
}
