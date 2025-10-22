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

//! Converter between google.protobuf.Struct and serde_json::Value

use crate::config::common::GenericError;
use prost_types::{Struct, Value, ListValue, value::Kind};
use serde_json::{Value as JsonValue, Map as JsonMap};

/// Converter for google.protobuf.Struct ↔ serde_json::Value
pub struct JsonConverter;

impl JsonConverter {
    /// Convert google.protobuf.Struct to serde_json::Value
    pub fn struct_to_json(proto_struct: &Struct) -> Result<JsonValue, GenericError> {
        let mut map = JsonMap::new();
        
        for (key, value) in &proto_struct.fields {
            map.insert(key.clone(), Self::value_to_json(value)?);
        }
        
        Ok(JsonValue::Object(map))
    }
    
    /// Convert google.protobuf.Value to serde_json::Value
    pub fn value_to_json(proto_value: &Value) -> Result<JsonValue, GenericError> {
        match &proto_value.kind {
            None => Ok(JsonValue::Null),
            Some(Kind::NullValue(_)) => Ok(JsonValue::Null),
            Some(Kind::NumberValue(n)) => Ok(JsonValue::Number(
                serde_json::Number::from_f64(*n)
                    .ok_or_else(|| GenericError::from_msg(format!("Invalid number: {}", n)))?
            )),
            Some(Kind::StringValue(s)) => Ok(JsonValue::String(s.clone())),
            Some(Kind::BoolValue(b)) => Ok(JsonValue::Bool(*b)),
            Some(Kind::StructValue(s)) => Self::struct_to_json(s),
            Some(Kind::ListValue(l)) => Self::list_to_json(l),
        }
    }
    
    /// Convert google.protobuf.ListValue to serde_json::Value
    pub fn list_to_json(proto_list: &ListValue) -> Result<JsonValue, GenericError> {
        let mut arr = Vec::new();
        
        for value in &proto_list.values {
            arr.push(Self::value_to_json(value)?);
        }
        
        Ok(JsonValue::Array(arr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_null_value() {
        let value = Value { kind: None };
        assert_eq!(JsonConverter::value_to_json(&value).unwrap(), JsonValue::Null);
    }

    #[test]
    fn test_number_value() {
        let value = Value { kind: Some(Kind::NumberValue(42.5)) };
        assert_eq!(JsonConverter::value_to_json(&value).unwrap(), serde_json::json!(42.5));
    }

    #[test]
    fn test_string_value() {
        let value = Value { kind: Some(Kind::StringValue("hello".to_string())) };
        assert_eq!(JsonConverter::value_to_json(&value).unwrap(), serde_json::json!("hello"));
    }

    #[test]
    fn test_bool_value() {
        let value = Value { kind: Some(Kind::BoolValue(true)) };
        assert_eq!(JsonConverter::value_to_json(&value).unwrap(), serde_json::json!(true));
    }

    #[test]
    fn test_list_value() {
        let list = ListValue {
            values: vec![
                Value { kind: Some(Kind::NumberValue(1.0)) },
                Value { kind: Some(Kind::NumberValue(2.0)) },
                Value { kind: Some(Kind::NumberValue(3.0)) },
            ],
        };
        let value = Value { kind: Some(Kind::ListValue(list)) };
        assert_eq!(JsonConverter::value_to_json(&value).unwrap(), serde_json::json!([1.0, 2.0, 3.0]));
    }

    #[test]
    fn test_struct_value() {
        let mut fields = BTreeMap::new();
        fields.insert("name".to_string(), Value {
            kind: Some(Kind::StringValue("test".to_string())),
        });
        fields.insert("count".to_string(), Value {
            kind: Some(Kind::NumberValue(5.0)),
        });
        
        let proto_struct = Struct { fields };
        let json = JsonConverter::struct_to_json(&proto_struct).unwrap();
        
        assert_eq!(json["name"], serde_json::json!("test"));
        assert_eq!(json["count"], serde_json::json!(5.0));
    }

    #[test]
    fn test_nested_struct() {
        let mut inner_fields = BTreeMap::new();
        inner_fields.insert("enabled".to_string(), Value {
            kind: Some(Kind::BoolValue(true)),
        });
        
        let mut outer_fields = BTreeMap::new();
        outer_fields.insert("config".to_string(), Value {
            kind: Some(Kind::StructValue(Struct { fields: inner_fields })),
        });
        
        let proto_struct = Struct { fields: outer_fields };
        let json = JsonConverter::struct_to_json(&proto_struct).unwrap();
        
        assert_eq!(json["config"]["enabled"], serde_json::json!(true));
    }
}
