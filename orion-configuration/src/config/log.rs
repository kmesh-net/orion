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

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Log {
    #[serde(deserialize_with = "deserialize_log_level", serialize_with = "serialize_log_level")]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub log_level: Option<EnvFilter>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub log_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub log_file: Option<String>,
}

impl PartialEq for Log {
    fn eq(&self, other: &Self) -> bool {
        self.log_file == other.log_file
            && self.log_directory == other.log_directory
            && self.log_level.as_ref().map(EnvFilter::to_string) == other.log_level.as_ref().map(EnvFilter::to_string)
    }
}
impl Eq for Log {}

fn deserialize_log_level<'de, D>(deserializer: D) -> std::result::Result<Option<EnvFilter>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).and_then(|maybe_string| {
        maybe_string.map(|s| EnvFilter::builder().parse(s)).transpose().map_err(
            |e: tracing_subscriber::filter::ParseError| {
                serde::de::Error::custom(format!("failed to deserialize log level because of \"{e}\""))
            },
        )
    })
}

#[allow(clippy::ref_option)]
fn serialize_log_level<S: Serializer>(
    value: &Option<EnvFilter>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    value.as_ref().map(EnvFilter::to_string).serialize(serializer)
}
