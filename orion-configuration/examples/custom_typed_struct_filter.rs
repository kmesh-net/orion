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

use orion_configuration::config::common::GenericError;
use orion_configuration::typed_struct::{
    registry::{global_registry, GenericFilterParser},
    ParsedTypedStruct, TypedStructFilter,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomMetricsFilter {
    pub enabled: bool,
    pub metrics_endpoint: Option<String>,
    pub sample_rate: Option<f64>,
    pub labels: Option<Vec<MetricLabel>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricLabel {
    pub key: String,
    pub value: String,
}

impl TypedStructFilter for CustomMetricsFilter {
    const TYPE_URL: &'static str = "type.googleapis.com/custom.filters.http.metrics.v1.Config";

    fn from_json_value(value: JsonValue) -> Result<Self, GenericError> {
        serde_json::from_value(value)
            .map_err(|e| GenericError::from_msg(format!("Failed to deserialize CustomMetricsFilter: {e}")))
    }
}

fn main() -> Result<(), GenericError> {
    eprintln!("🔌 Custom TypedStruct Filter Plugin Example\n");

    eprintln!("1. Creating custom filter parser...");
    let parser = GenericFilterParser::<CustomMetricsFilter>::new(CustomMetricsFilter::TYPE_URL);

    eprintln!("2. Registering filter with global registry...");
    let registry = global_registry();
    registry.register_dynamic(parser)?;

    eprintln!("3. Verifying registration...");
    assert!(registry.is_supported(CustomMetricsFilter::TYPE_URL));
    eprintln!("   ✓ Filter registered successfully");

    eprintln!("\n4. Testing filter parsing...");
    let config_json = serde_json::json!({
        "enabled": true,
        "metrics_endpoint": "/metrics",
        "sample_rate": 0.1,
        "labels": [
            {"key": "env", "value": "production"},
            {"key": "service", "value": "api-gateway"}
        ]
    });

    let typed_struct = ParsedTypedStruct { type_url: CustomMetricsFilter::TYPE_URL.to_owned(), value: config_json };

    let result = registry.parse_dynamic(&typed_struct)?;
    let filter = result
        .downcast_ref::<CustomMetricsFilter>()
        .ok_or_else(|| GenericError::from_msg("Failed to downcast to CustomMetricsFilter"))?;

    eprintln!("   ✓ Filter parsed successfully:");
    eprintln!("     - Enabled: {}", filter.enabled);
    if let Some(endpoint) = &filter.metrics_endpoint {
        eprintln!("     - Endpoint: {endpoint}");
    }
    if let Some(rate) = filter.sample_rate {
        eprintln!("     - Sample Rate: {rate}");
    }
    if let Some(labels) = &filter.labels {
        eprintln!("     - Labels: {} entries", labels.len());
    }

    eprintln!("\n5. Listing all registered filters:");
    let registered = registry.supported_types();
    eprintln!("   Total registered filters: {}", registered.len());
    for (i, type_url) in registered.iter().enumerate() {
        eprintln!("   {}. {type_url}", i + 1);
    }

    eprintln!("\n✅ Plugin example completed successfully!");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_filter_registration() {
        let parser = GenericFilterParser::<CustomMetricsFilter>::new(CustomMetricsFilter::TYPE_URL);

        let registry = global_registry();
        let _ = registry.register_dynamic(parser);

        assert!(registry.is_supported(CustomMetricsFilter::TYPE_URL));
    }

    #[test]
    fn test_custom_filter_parsing() {
        let config_json = serde_json::json!({
            "enabled": true,
            "metrics_endpoint": "/stats",
            "sample_rate": 0.5,
        });

        let typed_struct =
            ParsedTypedStruct { type_url: CustomMetricsFilter::TYPE_URL.to_string(), value: config_json };

        let filter = CustomMetricsFilter::from_typed_struct(&typed_struct).unwrap();
        assert_eq!(filter.enabled, true);
        assert_eq!(filter.metrics_endpoint, Some("/stats".to_string()));
        assert_eq!(filter.sample_rate, Some(0.5));
    }
}
