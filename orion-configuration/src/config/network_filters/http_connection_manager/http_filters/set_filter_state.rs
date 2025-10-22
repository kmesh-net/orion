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

use crate::config::common::GenericError;
use crate::typed_struct::TypedStructFilter;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SetFilterStateConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_request_headers: Option<Vec<FilterStateAction>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_response_headers: Option<Vec<FilterStateAction>>,

    #[serde(flatten)]
    pub additional_fields: Option<serde_json::Map<String, JsonValue>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FilterStateAction {
    pub object_key: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_string: Option<FormatString>,

    #[serde(flatten)]
    pub additional_fields: Option<serde_json::Map<String, JsonValue>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FormatString {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_format_source: Option<TextFormatSource>,

    #[serde(flatten)]
    pub additional_fields: Option<serde_json::Map<String, JsonValue>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TextFormatSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_string: Option<String>,

    #[serde(flatten)]
    pub additional_fields: Option<serde_json::Map<String, JsonValue>>,
}

impl Default for SetFilterStateConfig {
    fn default() -> Self {
        Self { on_request_headers: None, on_response_headers: None, additional_fields: None }
    }
}

impl TypedStructFilter for SetFilterStateConfig {
    const TYPE_URL: &'static str = "type.googleapis.com/envoy.extensions.filters.http.set_filter_state.v3.Config";

    fn from_json_value(value: JsonValue) -> Result<Self, GenericError> {
        serde_json::from_value(value)
            .map_err(|e| GenericError::from_msg_with_cause("Failed to deserialize SetFilterStateConfig from JSON", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typed_struct::{ParsedTypedStruct, TypedStruct, TypedStructParser};
    use prost::Message;
    use prost_types::{value::Kind, Struct, Value};
    use std::collections::BTreeMap;

    #[test]
    fn test_deserialize_empty_config() {
        let json = serde_json::json!({});
        let config = SetFilterStateConfig::from_json_value(json).unwrap();
        assert_eq!(config.on_request_headers, None);
        assert_eq!(config.on_response_headers, None);
    }

    #[test]
    fn test_deserialize_connect_authority_config() {
        let json = serde_json::json!({
            "on_request_headers": [
                {
                    "object_key": "envoy.filters.listener.original_dst.local_ip",
                    "format_string": {
                        "text_format_source": {
                            "inline_string": "%REQ(:authority)%"
                        }
                    }
                }
            ]
        });

        let config = SetFilterStateConfig::from_json_value(json).unwrap();
        assert!(config.on_request_headers.is_some());

        let actions = config.on_request_headers.unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].object_key, "envoy.filters.listener.original_dst.local_ip");

        let format_string = actions[0].format_string.as_ref().unwrap();
        let text_source = format_string.text_format_source.as_ref().unwrap();
        assert_eq!(text_source.inline_string, Some("%REQ(:authority)%".to_string()));
    }

    #[test]
    fn test_from_typed_struct() {
        let mut action_fields = BTreeMap::new();
        action_fields.insert("object_key".to_string(), Value { kind: Some(Kind::StringValue("test_key".to_string())) });

        let mut fields = BTreeMap::new();
        fields.insert(
            "on_request_headers".to_string(),
            Value {
                kind: Some(Kind::ListValue(prost_types::ListValue {
                    values: vec![Value { kind: Some(Kind::StructValue(Struct { fields: action_fields })) }],
                })),
            },
        );

        let typed_struct =
            TypedStruct { type_url: SetFilterStateConfig::TYPE_URL.to_string(), value: Some(Struct { fields }) };

        let mut buf = Vec::new();
        typed_struct.encode(&mut buf).unwrap();

        let parsed = TypedStructParser::parse(&buf).unwrap();
        let config = SetFilterStateConfig::from_typed_struct(&parsed).unwrap();

        assert!(config.on_request_headers.is_some());
        let actions = config.on_request_headers.unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].object_key, "test_key");
    }

    #[test]
    fn test_type_url_validation() {
        let json = serde_json::json!({});
        let parsed = ParsedTypedStruct { type_url: "type.googleapis.com/wrong.type.Config".to_string(), value: json };

        let result = SetFilterStateConfig::from_typed_struct(&parsed);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("type URL mismatch"));
    }

    #[test]
    fn test_multiple_actions() {
        let json = serde_json::json!({
            "on_request_headers": [
                {"object_key": "key1"},
                {"object_key": "key2"}
            ],
            "on_response_headers": [
                {"object_key": "key3"}
            ]
        });

        let config = SetFilterStateConfig::from_json_value(json).unwrap();
        assert_eq!(config.on_request_headers.as_ref().unwrap().len(), 2);
        assert_eq!(config.on_response_headers.as_ref().unwrap().len(), 1);
    }
}
