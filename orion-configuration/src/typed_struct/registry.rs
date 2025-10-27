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

use super::ParsedTypedStruct;
use crate::config::common::GenericError;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub trait DynamicTypedStructParser: Send + Sync {
    fn type_url(&self) -> &str;
    fn parse_json(&self, value: JsonValue) -> Result<Box<dyn std::any::Any + Send>, GenericError>;
}

pub type FilterParser = fn(&ParsedTypedStruct) -> Result<(), GenericError>;

pub struct TypedStructRegistry {
    parsers: RwLock<HashMap<String, FilterParser>>,
    dynamic_parsers: RwLock<HashMap<String, Arc<dyn DynamicTypedStructParser>>>,
}

impl TypedStructRegistry {
    pub fn new() -> Self {
        Self { parsers: RwLock::new(HashMap::new()), dynamic_parsers: RwLock::new(HashMap::new()) }
    }

    pub fn register(&self, type_url: impl Into<String>, parser: FilterParser) -> Result<(), GenericError> {
        let type_url = type_url.into();
        let mut parsers =
            self.parsers.write().map_err(|e| GenericError::from_msg(format!("Failed to acquire write lock: {}", e)))?;

        if parsers.contains_key(&type_url) {
            return Err(GenericError::from_msg(format!("Filter with type URL '{}' is already registered", type_url)));
        }

        parsers.insert(type_url, parser);
        Ok(())
    }

    pub fn register_dynamic<P>(&self, parser: P) -> Result<(), GenericError>
    where
        P: DynamicTypedStructParser + 'static,
    {
        let type_url = parser.type_url().to_string();
        let mut dynamic_parsers = self
            .dynamic_parsers
            .write()
            .map_err(|e| GenericError::from_msg(format!("Failed to acquire write lock: {}", e)))?;

        if dynamic_parsers.contains_key(&type_url) {
            return Err(GenericError::from_msg(format!(
                "Dynamic filter with type URL '{}' is already registered",
                type_url
            )));
        }

        dynamic_parsers.insert(type_url, Arc::new(parser));
        Ok(())
    }

    pub fn is_supported(&self, type_url: &str) -> bool {
        self.parsers.read().map(|p| p.contains_key(type_url)).unwrap_or(false)
            || self.dynamic_parsers.read().map(|p| p.contains_key(type_url)).unwrap_or(false)
    }

    pub fn parse(&self, typed_struct: &ParsedTypedStruct) -> Result<(), GenericError> {
        if let Ok(parsers) = self.parsers.read() {
            if let Some(parser) = parsers.get(&typed_struct.type_url) {
                return parser(typed_struct);
            }
        }

        Err(GenericError::from_msg(format!("No parser registered for TypedStruct type_url: {}", typed_struct.type_url)))
    }

    pub fn parse_dynamic(
        &self,
        typed_struct: &ParsedTypedStruct,
    ) -> Result<Box<dyn std::any::Any + Send>, GenericError> {
        let dynamic_parsers = self
            .dynamic_parsers
            .read()
            .map_err(|e| GenericError::from_msg(format!("Failed to acquire read lock: {}", e)))?;

        let parser = dynamic_parsers.get(&typed_struct.type_url).ok_or_else(|| {
            GenericError::from_msg(format!("No dynamic parser registered for type URL: {}", typed_struct.type_url))
        })?;

        parser.parse_json(typed_struct.value.clone())
    }

    pub fn supported_types(&self) -> Vec<String> {
        let mut types = Vec::new();

        if let Ok(parsers) = self.parsers.read() {
            types.extend(parsers.keys().cloned());
        }

        if let Ok(dynamic_parsers) = self.dynamic_parsers.read() {
            types.extend(dynamic_parsers.keys().cloned());
        }

        types.sort();
        types.dedup();
        types
    }

    pub fn unregister(&self, type_url: &str) -> Result<(), GenericError> {
        let mut parsers =
            self.parsers.write().map_err(|e| GenericError::from_msg(format!("Failed to acquire write lock: {}", e)))?;

        let mut dynamic_parsers = self
            .dynamic_parsers
            .write()
            .map_err(|e| GenericError::from_msg(format!("Failed to acquire write lock: {}", e)))?;

        let removed = parsers.remove(type_url).is_some() || dynamic_parsers.remove(type_url).is_some();

        if !removed {
            return Err(GenericError::from_msg(format!("No filter registered with type URL: {}", type_url)));
        }

        Ok(())
    }
}

impl Default for TypedStructRegistry {
    fn default() -> Self {
        let registry = Self::new();

        let _ = registry.register("type.googleapis.com/io.istio.http.peer_metadata.Config", |_| Ok(()));

        let _ = registry
            .register("type.googleapis.com/envoy.extensions.filters.http.set_filter_state.v3.Config", |_| Ok(()));

        registry
    }
}

pub struct GenericFilterParser<T> {
    type_url: String,
    _phantom: std::marker::PhantomData<T>,
}

impl<T> GenericFilterParser<T> {
    pub fn new(type_url: impl Into<String>) -> Self {
        Self { type_url: type_url.into(), _phantom: std::marker::PhantomData }
    }
}

impl<T> DynamicTypedStructParser for GenericFilterParser<T>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    fn type_url(&self) -> &str {
        &self.type_url
    }

    fn parse_json(&self, value: JsonValue) -> Result<Box<dyn std::any::Any + Send>, GenericError> {
        let config: T = serde_json::from_value(value)
            .map_err(|e| GenericError::from_msg(format!("Failed to deserialize filter config: {}", e)))?;

        Ok(Box::new(config))
    }
}

#[macro_export]
macro_rules! register_typed_struct_filter {
    ($registry:expr, $filter_type:ty) => {{
        use $crate::typed_struct::registry::GenericFilterParser;

        let parser = GenericFilterParser::<$filter_type>::new(<$filter_type>::TYPE_URL);
        $registry.register_dynamic(parser)
    }};
}

static GLOBAL_REGISTRY: std::sync::OnceLock<TypedStructRegistry> = std::sync::OnceLock::new();

pub fn global_registry() -> &'static TypedStructRegistry {
    GLOBAL_REGISTRY.get_or_init(TypedStructRegistry::default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[test]
    fn test_registry_creation() {
        let registry = TypedStructRegistry::new();
        assert_eq!(registry.supported_types().len(), 0);
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
        let registry = TypedStructRegistry::new();
        registry.register("custom.type", |_| Ok(())).unwrap();
        assert!(registry.is_supported("custom.type"));
    }

    #[test]
    fn test_supported_types() {
        let registry = TypedStructRegistry::default();
        let types = registry.supported_types();
        assert!(types.contains(&"type.googleapis.com/io.istio.http.peer_metadata.Config".to_string()));
        assert!(
            types.contains(&"type.googleapis.com/envoy.extensions.filters.http.set_filter_state.v3.Config".to_string())
        );
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestFilter {
        name: String,
        value: i32,
    }

    impl TestFilter {
        const TYPE_URL: &'static str = "type.googleapis.com/test.TestFilter";
    }

    #[test]
    fn test_dynamic_register_and_parse() {
        let registry = TypedStructRegistry::new();

        let parser = GenericFilterParser::<TestFilter>::new(TestFilter::TYPE_URL);
        registry.register_dynamic(parser).unwrap();

        assert!(registry.is_supported(TestFilter::TYPE_URL));

        let json = serde_json::json!({
            "name": "test_filter",
            "value": 42
        });

        let typed_struct = ParsedTypedStruct { type_url: TestFilter::TYPE_URL.to_string(), value: json };

        let result = registry.parse_dynamic(&typed_struct).unwrap();
        let filter = result.downcast_ref::<TestFilter>().unwrap();

        assert_eq!(filter.name, "test_filter");
        assert_eq!(filter.value, 42);
    }

    #[test]
    fn test_register_duplicate() {
        let registry = TypedStructRegistry::new();

        registry.register("duplicate.type", |_| Ok(())).unwrap();
        let result = registry.register("duplicate.type", |_| Ok(()));

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already registered"));
    }

    #[test]
    fn test_register_duplicate_dynamic() {
        let registry = TypedStructRegistry::new();

        let parser1 = GenericFilterParser::<TestFilter>::new(TestFilter::TYPE_URL);
        registry.register_dynamic(parser1).unwrap();

        let parser2 = GenericFilterParser::<TestFilter>::new(TestFilter::TYPE_URL);
        let result = registry.register_dynamic(parser2);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already registered"));
    }

    #[test]
    fn test_parse_unregistered() {
        let registry = TypedStructRegistry::new();

        let json = serde_json::json!({
            "name": "test_filter",
            "value": 42
        });

        let typed_struct =
            ParsedTypedStruct { type_url: "type.googleapis.com/unknown.Filter".to_string(), value: json };

        let result = registry.parse_dynamic(&typed_struct);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No dynamic parser registered"));
    }

    #[test]
    fn test_unregister() {
        let registry = TypedStructRegistry::new();

        registry.register("test.type", |_| Ok(())).unwrap();
        assert!(registry.is_supported("test.type"));

        registry.unregister("test.type").unwrap();
        assert!(!registry.is_supported("test.type"));
    }

    #[test]
    fn test_unregister_dynamic() {
        let registry = TypedStructRegistry::new();

        let parser = GenericFilterParser::<TestFilter>::new(TestFilter::TYPE_URL);
        registry.register_dynamic(parser).unwrap();

        assert!(registry.is_supported(TestFilter::TYPE_URL));

        registry.unregister(TestFilter::TYPE_URL).unwrap();

        assert!(!registry.is_supported(TestFilter::TYPE_URL));
    }

    #[test]
    fn test_global_registry() {
        let registry = global_registry();
        assert!(registry.is_supported("type.googleapis.com/io.istio.http.peer_metadata.Config"));
    }

    #[test]
    fn test_thread_safety() {
        use std::thread;

        let registry = Arc::new(TypedStructRegistry::new());
        let mut handles = vec![];

        for i in 0..10 {
            let registry_clone = Arc::clone(&registry);
            let handle = thread::spawn(move || {
                let type_url = format!("test.type.{}", i);
                registry_clone.register(&type_url, |_| Ok(())).unwrap();
                assert!(registry_clone.is_supported(&type_url));
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(registry.supported_types().len(), 10);
    }
}
