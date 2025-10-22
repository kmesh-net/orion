use orion_configuration::typed_struct::{
    TypedStructRegistry, TypedStructError,
};
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
    let mut registry = TypedStructRegistry::new();
    
    registry.register("custom.filter", |parsed| {
        assert_eq!(parsed.type_url, "custom.filter");
        Ok(())
    });
    
    assert!(registry.is_supported("custom.filter"));
    assert!(!registry.is_supported("unknown.filter"));
}

#[test]
fn test_error_formatting() {
    let err = TypedStructError::InvalidTypeUrl {
        expected: "expected.type".to_string(),
        actual: "actual.type".to_string(),
    };
    
    let msg = format!("{}", err);
    assert!(msg.contains("expected.type"));
    assert!(msg.contains("actual.type"));
    assert!(msg.contains("mismatch"));
}

#[test]
fn test_unsupported_filter_error() {
    let err = TypedStructError::UnsupportedFilter {
        type_url: "unknown.filter".to_string(),
        available: vec![
            "filter.a".to_string(),
            "filter.b".to_string(),
        ],
    };
    
    let msg = format!("{}", err);
    assert!(msg.contains("unknown.filter"));
    assert!(msg.contains("filter.a"));
    assert!(msg.contains("filter.b"));
}

#[test]
fn test_multiple_filter_registration() {
    let mut registry = TypedStructRegistry::new();
    
    registry.register("filter.a", |_| Ok(()));
    registry.register("filter.b", |_| Ok(()));
    registry.register("filter.c", |_| Ok(()));
    
    assert_eq!(registry.supported_types().len(), 3);
    assert!(registry.is_supported("filter.a"));
    assert!(registry.is_supported("filter.b"));
    assert!(registry.is_supported("filter.c"));
}

#[test]
fn test_registry_parser_execution() {
    use orion_configuration::typed_struct::ParsedTypedStruct;
    
    let mut registry = TypedStructRegistry::new();
    
    registry.register("test.filter", |parsed| {
        assert_eq!(parsed.type_url, "test.filter");
        Ok(())
    });
    
    let parsed = ParsedTypedStruct {
        type_url: "test.filter".to_string(),
        value: json!({"key": "value"}),
    };
    
    let result = registry.parse(&parsed);
    assert!(result.is_ok());
}
