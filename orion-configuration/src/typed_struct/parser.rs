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

//! TypedStruct parser - extracts type_url and JSON value from protobuf

use crate::config::common::GenericError;
use super::{ParsedTypedStruct, JsonConverter};
use prost::Message;

/// TypedStruct protobuf message definition
/// 
/// From udpa/type/v1/typed_struct.proto:
/// ```protobuf
/// message TypedStruct {
///   string type_url = 1;
///   google.protobuf.Struct value = 2;
/// }
/// ```
#[derive(Clone, PartialEq, Message)]
pub struct TypedStruct {
    #[prost(string, tag = "1")]
    pub type_url: String,
    
    #[prost(message, optional, tag = "2")]
    pub value: Option<prost_types::Struct>,
}

/// Parser for TypedStruct wrappers
pub struct TypedStructParser;

impl TypedStructParser {
    /// Parse a TypedStruct from raw bytes
    pub fn parse(bytes: &[u8]) -> Result<ParsedTypedStruct, GenericError> {
        // Decode the TypedStruct protobuf
        let typed_struct = TypedStruct::decode(bytes)
            .map_err(|e| GenericError::from_msg(format!("Failed to decode TypedStruct: {}", e)))?;
        
        // Extract type URL
        let type_url = typed_struct.type_url;
        
        // Convert protobuf Struct to JSON
        let value = match typed_struct.value {
            Some(struct_value) => JsonConverter::struct_to_json(&struct_value)?,
            None => serde_json::json!({}), // Empty config
        };
        
        Ok(ParsedTypedStruct { type_url, value })
    }
    
    /// Check if a type URL indicates a TypedStruct wrapper
    pub fn is_typed_struct_url(type_url: &str) -> bool {
        type_url == "type.googleapis.com/udpa.type.v1.TypedStruct"
            || type_url == "type.googleapis.com/xds.type.v3.TypedStruct"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::{Struct, Value, value::Kind};
    use std::collections::BTreeMap;

    #[test]
    fn test_is_typed_struct_url() {
        assert!(TypedStructParser::is_typed_struct_url("type.googleapis.com/udpa.type.v1.TypedStruct"));
        assert!(TypedStructParser::is_typed_struct_url("type.googleapis.com/xds.type.v3.TypedStruct"));
        assert!(!TypedStructParser::is_typed_struct_url("type.googleapis.com/envoy.extensions.filters.http.router.v3.Router"));
    }

    #[test]
    fn test_parse_empty_typed_struct() {
        let typed_struct = TypedStruct {
            type_url: "type.googleapis.com/test.Config".to_string(),
            value: None,
        };
        
        let mut buf = Vec::new();
        typed_struct.encode(&mut buf).unwrap();
        
        let parsed = TypedStructParser::parse(&buf).unwrap();
        assert_eq!(parsed.type_url, "type.googleapis.com/test.Config");
        assert_eq!(parsed.value, serde_json::json!({}));
    }

    #[test]
    fn test_parse_typed_struct_with_value() {
        let mut fields = BTreeMap::new();
        fields.insert("enabled".to_string(), Value {
            kind: Some(Kind::BoolValue(true)),
        });
        
        let typed_struct = TypedStruct {
            type_url: "type.googleapis.com/test.Config".to_string(),
            value: Some(Struct { fields }),
        };
        
        let mut buf = Vec::new();
        typed_struct.encode(&mut buf).unwrap();
        
        let parsed = TypedStructParser::parse(&buf).unwrap();
        assert_eq!(parsed.type_url, "type.googleapis.com/test.Config");
        assert_eq!(parsed.value["enabled"], serde_json::json!(true));
    }
}
