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

use orion_configuration::typed_struct::{TypedStructError, TypedStructRegistry};
use serde_json::json;

#[test]
fn test_registry_with_istio_filters() {
    let registry = TypedStructRegistry::default();

    assert!(registry.is_supported("type.googleapis.com/io.istio.http.peer_metadata.Config"));
    assert!(registry.is_supported("type.googleapis.com/envoy.extensions.filters.http.set_filter_state.v3.Config"));

    let supported = registry.supported_types();
    assert_eq!(supported.len(), 2);
}

#[test]
fn test_registry_custom_parser() {
    let registry = TypedStructRegistry::new();

    let _ = registry.register("custom.filter", |parsed| {
        assert_eq!(parsed.type_url, "custom.filter");
        Ok(())
    });

    assert!(registry.is_supported("custom.filter"));
    assert!(!registry.is_supported("unknown.filter"));
}

#[test]
fn test_error_formatting() {
    let err =
        TypedStructError::InvalidTypeUrl { expected: "expected.type".to_owned(), actual: "actual.type".to_owned() };

    let msg = format!("{err}");
    assert!(msg.contains("expected.type"));
    assert!(msg.contains("actual.type"));
    assert!(msg.contains("mismatch"));
}

#[test]
fn test_unsupported_filter_error() {
    let err = TypedStructError::UnsupportedFilter {
        type_url: "unknown.filter".to_string(),
        available: vec!["filter.a".to_string(), "filter.b".to_string()],
    };

    let msg = format!("{}", err);
    assert!(msg.contains("unknown.filter"));
    assert!(msg.contains("filter.a"));
    assert!(msg.contains("filter.b"));
}

#[test]
fn test_multiple_filter_registration() {
    let registry = TypedStructRegistry::new();

    let _ = registry.register("filter.a", |_| Ok(()));
    let _ = registry.register("filter.b", |_| Ok(()));
    let _ = registry.register("filter.c", |_| Ok(()));

    assert_eq!(registry.supported_types().len(), 3);
    assert!(registry.is_supported("filter.a"));
    assert!(registry.is_supported("filter.b"));
    assert!(registry.is_supported("filter.c"));
}

#[test]
fn test_registry_parser_execution() {
    use orion_configuration::typed_struct::ParsedTypedStruct;

    let registry = TypedStructRegistry::new();

    let _ = registry.register("test.filter", |parsed| {
        assert_eq!(parsed.type_url, "test.filter");
        Ok(())
    });

    let parsed = ParsedTypedStruct { type_url: "test.filter".to_string(), value: json!({"key": "value"}) };

    let result = registry.parse(&parsed);
    assert!(result.is_ok());
}
