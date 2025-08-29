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

use super::{is_default, GenericError};
use http::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Default, Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderModifier {
    #[serde(with = "http_serde_ext::header_name::vec", default, skip_serializing_if = "is_default")]
    remove: Vec<HeaderName>,
    #[serde(default, skip_serializing_if = "is_default")]
    add: Vec<HeaderValueOption>,
}

impl HeaderModifier {
    pub fn new(remove: Vec<HeaderName>, add: Vec<HeaderValueOption>) -> Self {
        Self { remove, add }
    }
    pub fn modify(&self, header_map: &mut HeaderMap) {
        for name in &self.remove {
            header_map.remove(name);
        }
        for value in &self.add {
            value.apply(header_map);
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderValueOption {
    pub header: HeaderKeyValue,
    pub append_action: HeaderAppendAction,
    pub keep_empty_value: bool,
}

impl HeaderValueOption {
    pub fn apply(&self, header_map: &mut HeaderMap) -> bool {
        if self.header.value.is_empty() && !self.keep_empty_value {
            header_map.remove(&self.header.key).is_some()
        } else {
            match self.append_action {
                HeaderAppendAction::AppendIfExistsOrAdd => {
                    header_map.append(&self.header.key, self.header.value.clone());
                    true
                },
                HeaderAppendAction::AppendIfAbsent => {
                    if header_map.get(&self.header.key).is_none() {
                        header_map.append(&self.header.key, self.header.value.clone());
                        true
                    } else {
                        false
                    }
                },
                HeaderAppendAction::OverwriteIfExistsOrAdd => {
                    header_map.insert(&self.header.key, self.header.value.clone());
                    true
                },
                HeaderAppendAction::OverwriteIfExists => {
                    if header_map.get(&self.header.key).is_some() {
                        header_map.insert(&self.header.key, self.header.value.clone());
                        true
                    } else {
                        false
                    }
                },
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Deserialize, Serialize)]
pub enum HeaderAppendAction {
    AppendIfExistsOrAdd,
    AppendIfAbsent,
    OverwriteIfExistsOrAdd,
    OverwriteIfExists,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Deserialize, Serialize)]
pub struct HeaderKeyValue {
    #[serde(with = "http_serde_ext::header_name")]
    pub key: HeaderName,
    //todo(hayley): this macro is too restricive. It only accepts headervalues that have printable ascii characters but
    // the struct accepts opaque bytes too.
    #[serde(with = "http_serde_ext::header_value")]
    pub value: HeaderValue,
}

impl TryFrom<(String, Vec<u8>)> for HeaderKeyValue {
    type Error = GenericError;
    fn try_from(value: (String, Vec<u8>)) -> Result<Self, Self::Error> {
        let key = HeaderName::from_str(&value.0).map_err(|e| {
            GenericError::from_msg_with_cause(format!("failed to parse \"{}\" as a HeaderName", value.0), e)
        })?;
        let value = HeaderValue::try_from(value.1)
            .map_err(|e| GenericError::from_msg_with_cause("failed to parse bytes as HeaderValue", e))?;
        Ok(Self { key, value })
    }
}
impl TryFrom<(String, String)> for HeaderKeyValue {
    type Error = GenericError;
    fn try_from(value: (String, String)) -> Result<Self, Self::Error> {
        let key = HeaderName::from_str(&value.0).map_err(|e| {
            GenericError::from_msg_with_cause(format!("failed to parse \"{}\" as a HeaderName", value.0), e)
        })?;
        let value = HeaderValue::from_str(&value.1).map_err(|e| {
            GenericError::from_msg_with_cause(format!("failed to parse \"{}\" as a HeaderValue", value.1), e)
        })?;
        Ok(Self { key, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::network_filters::http_connection_manager::header_modifer::HeaderKeyValue;
    use http::{
        header::{COOKIE, LOCATION, USER_AGENT},
        HeaderMap, HeaderValue,
    };

    #[test]
    fn test_header_mutation_append_if_exists_or_add() {
        let header_map = &mut HeaderMap::<HeaderValue>::default();

        let hello = HeaderValue::from_str("hello").unwrap();
        let world = HeaderValue::from_str("world").unwrap();

        HeaderValueOption {
            header: HeaderKeyValue { key: LOCATION, value: hello.clone() },
            append_action: HeaderAppendAction::AppendIfExistsOrAdd,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.get(LOCATION), Some(&hello));
        assert_eq!(header_map.len(), 1);

        HeaderValueOption {
            header: HeaderKeyValue { key: LOCATION, value: world.clone() },
            append_action: HeaderAppendAction::AppendIfExistsOrAdd,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.len(), 2);

        let mut iter = header_map.get_all(LOCATION).iter();
        assert_eq!(&hello, iter.next().unwrap());
        assert_eq!(&world, iter.next().unwrap());
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_header_mutation_inline_append() {
        let header_map = &mut HeaderMap::<HeaderValue>::default();

        let hello = HeaderValue::from_str("hello").unwrap();
        let world = HeaderValue::from_str("world").unwrap();

        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: hello.clone() },
            append_action: HeaderAppendAction::AppendIfExistsOrAdd,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.get(USER_AGENT), Some(&hello));
        assert_eq!(header_map.len(), 1);

        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: world.clone() },
            append_action: HeaderAppendAction::AppendIfExistsOrAdd,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.len(), 2);
    }

    #[test]
    fn test_header_mutation_cookie_append() {
        let header_map = &mut HeaderMap::<HeaderValue>::default();

        let hello = HeaderValue::from_str("hello").unwrap();
        let world = HeaderValue::from_str("world").unwrap();

        HeaderValueOption {
            header: HeaderKeyValue { key: COOKIE, value: hello.clone() },
            append_action: HeaderAppendAction::AppendIfExistsOrAdd,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.get(COOKIE), Some(&hello));
        assert_eq!(header_map.len(), 1);

        HeaderValueOption {
            header: HeaderKeyValue { key: COOKIE, value: world.clone() },
            append_action: HeaderAppendAction::AppendIfExistsOrAdd,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.len(), 2);
    }

    #[test]
    fn test_header_mutation_append_if_absent() {
        let header_map = &mut HeaderMap::<HeaderValue>::default();

        let hello = HeaderValue::from_str("hello").unwrap();

        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: hello.clone() },
            append_action: HeaderAppendAction::AppendIfAbsent,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.get(USER_AGENT), Some(&hello));
        assert_eq!(header_map.len(), 1);

        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: hello.clone() },
            append_action: HeaderAppendAction::AppendIfAbsent,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.get(USER_AGENT), Some(&hello));
        assert_eq!(header_map.len(), 1);
    }

    #[test]
    fn test_header_mutation_overwrite_if_exists_or_add() {
        let header_map = &mut HeaderMap::<HeaderValue>::default();

        let hello = HeaderValue::from_str("hello").unwrap();
        let world = HeaderValue::from_str("world").unwrap();

        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: hello.clone() },
            append_action: HeaderAppendAction::OverwriteIfExistsOrAdd,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.get(USER_AGENT), Some(&hello));
        assert_eq!(header_map.len(), 1);

        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: world.clone() },
            append_action: HeaderAppendAction::OverwriteIfExistsOrAdd,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.get(USER_AGENT), Some(&world));
        assert_eq!(header_map.len(), 1);
    }

    #[test]
    fn test_header_mutation_overwrite_if_exists() {
        let header_map = &mut HeaderMap::<HeaderValue>::default();

        let hello = HeaderValue::from_str("hello").unwrap();
        let world = HeaderValue::from_str("world").unwrap();

        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: hello.clone() },
            append_action: HeaderAppendAction::OverwriteIfExists,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.get(USER_AGENT), None);
        assert!(header_map.is_empty());
        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: hello.clone() },
            append_action: HeaderAppendAction::AppendIfAbsent,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.get(USER_AGENT), Some(&hello));
        assert_eq!(header_map.len(), 1);

        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: world.clone() },
            append_action: HeaderAppendAction::OverwriteIfExists,
            keep_empty_value: false,
        }
        .apply(header_map);

        assert_eq!(header_map.get(USER_AGENT), Some(&world));
        assert_eq!(header_map.len(), 1);
    }

    #[test]
    fn test_header_mutation_append_empty_value() {
        let header_map = &mut HeaderMap::<HeaderValue>::default();

        let empty = HeaderValue::from_str("").unwrap();
        let test = HeaderValue::from_str("test").unwrap();

        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: test.clone() },
            append_action: HeaderAppendAction::AppendIfExistsOrAdd,
            keep_empty_value: true,
        }
        .apply(header_map);

        assert_eq!(header_map.get(USER_AGENT), Some(&test));
        assert_eq!(header_map.len(), 1);

        HeaderValueOption {
            header: HeaderKeyValue { key: USER_AGENT, value: empty.clone() },
            append_action: HeaderAppendAction::AppendIfExistsOrAdd,
            keep_empty_value: true,
        }
        .apply(header_map);

        assert_eq!(header_map.get(USER_AGENT), Some(&test));
        assert_eq!(header_map.len(), 2);
    }
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{HeaderAppendAction, HeaderKeyValue, HeaderValueOption};
    use crate::config::common::*;
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::{
        header_value_option::HeaderAppendAction as EnvoyHeaderAppendAction, HeaderValue as EnvoyHeaderValue,
        HeaderValueOption as EnvoyHeaderValueOption,
    };

    impl TryFrom<EnvoyHeaderValueOption> for HeaderValueOption {
        type Error = GenericError;
        fn try_from(value: EnvoyHeaderValueOption) -> Result<Self, Self::Error> {
            let EnvoyHeaderValueOption { header, append, append_action, keep_empty_value } = value;
            unsupported_field!(append)?;
            let header = convert_opt!(header)?;
            let append_action = HeaderAppendAction::try_from(append_action).with_node("append_action")?;
            Ok(Self { header, append_action, keep_empty_value })
        }
    }

    impl From<EnvoyHeaderAppendAction> for HeaderAppendAction {
        fn from(value: EnvoyHeaderAppendAction) -> Self {
            match value {
                EnvoyHeaderAppendAction::AppendIfExistsOrAdd => Self::AppendIfExistsOrAdd,
                EnvoyHeaderAppendAction::AddIfAbsent => Self::AppendIfAbsent,
                EnvoyHeaderAppendAction::OverwriteIfExists => Self::OverwriteIfExists,
                EnvoyHeaderAppendAction::OverwriteIfExistsOrAdd => Self::OverwriteIfExistsOrAdd,
            }
        }
    }

    impl TryFrom<i32> for HeaderAppendAction {
        type Error = GenericError;
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            EnvoyHeaderAppendAction::from_i32(value)
                .ok_or(GenericError::unsupported_variant("[unknown header append action]"))
                .map(Self::from)
        }
    }

    impl TryFrom<EnvoyHeaderValue> for HeaderKeyValue {
        type Error = GenericError;
        fn try_from(value: EnvoyHeaderValue) -> Result<Self, Self::Error> {
            let EnvoyHeaderValue { key, value, raw_value } = value;
            match (value.is_used(), raw_value.is_used()) {
                (true, true) => {
                    Err(GenericError::from_msg(format!("both value ({value}) and raw_value ({raw_value:?}) were set"))
                        .with_node("value"))
                },
                (true, false) => Self::try_from((key, value)),
                (false, true) => Self::try_from((key, raw_value)),
                (false, false) => Err(GenericError::MissingField("value OR raw_value")),
            }
        }
    }
}
