use crate::config::common::GenericError;
use super::ParsedTypedStruct;
use std::collections::HashMap;

pub type FilterParser = fn(&ParsedTypedStruct) -> Result<(), GenericError>;

pub struct TypedStructRegistry {
    parsers: HashMap<String, FilterParser>,
}

impl TypedStructRegistry {
    pub fn new() -> Self {
        Self {
            parsers: HashMap::new(),
        }
    }

    pub fn register(&mut self, type_url: impl Into<String>, parser: FilterParser) {
        self.parsers.insert(type_url.into(), parser);
    }

    pub fn is_supported(&self, type_url: &str) -> bool {
        self.parsers.contains_key(type_url)
    }

    pub fn parse(&self, typed_struct: &ParsedTypedStruct) -> Result<(), GenericError> {
        if let Some(parser) = self.parsers.get(&typed_struct.type_url) {
            parser(typed_struct)
        } else {
            Err(GenericError::from_msg(format!(
                "No parser registered for TypedStruct type_url: {}",
                typed_struct.type_url
            )))
        }
    }

    pub fn supported_types(&self) -> Vec<&str> {
        self.parsers.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for TypedStructRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        
        registry.register(
            "type.googleapis.com/io.istio.http.peer_metadata.Config",
            |_| Ok(()),
        );
        
        registry.register(
            "type.googleapis.com/envoy.extensions.filters.http.set_filter_state.v3.Config",
            |_| Ok(()),
        );
        
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = TypedStructRegistry::new();
        assert_eq!(registry.parsers.len(), 0);
    }

    #[test]
    fn test_default_registry() {
        let registry = TypedStructRegistry::default();
        assert!(registry.is_supported("type.googleapis.com/io.istio.http.peer_metadata.Config"));
        assert!(registry.is_supported("type.googleapis.com/envoy.extensions.filters.http.set_filter_state.v3.Config"));
        assert!(!registry.is_supported("type.googleapis.com/unknown.filter"));
    }

    #[test]
    fn test_register_custom_parser() {
        let mut registry = TypedStructRegistry::new();
        registry.register("custom.type", |_| Ok(()));
        assert!(registry.is_supported("custom.type"));
    }

    #[test]
    fn test_supported_types() {
        let registry = TypedStructRegistry::default();
        let types = registry.supported_types();
        assert!(types.contains(&"type.googleapis.com/io.istio.http.peer_metadata.Config"));
        assert!(types.contains(&"type.googleapis.com/envoy.extensions.filters.http.set_filter_state.v3.Config"));
    }
}
