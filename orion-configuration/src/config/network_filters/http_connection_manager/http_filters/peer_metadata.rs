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

//! Istio Peer Metadata HTTP Filter
//!
//! This filter is used in Istio waypoint proxies to extract and propagate
//! peer metadata information. It's typically used for telemetry and policy
//! decisions based on workload identity.
//!
//! Type URL: `type.googleapis.com/io.istio.http.peer_metadata.Config`

use crate::config::common::GenericError;
use crate::typed_struct::TypedStructFilter;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Configuration for Istio's peer metadata filter
///
/// This filter extracts metadata about the peer workload from various sources
/// (typically from mTLS certificates and xDS metadata) and makes it available
/// for other filters and for telemetry.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PeerMetadataConfig {
    /// List of discovery configurations for downstream connections
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downstream_discovery: Option<Vec<JsonValue>>,

    /// List of discovery configurations for upstream connections
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_discovery: Option<Vec<JsonValue>>,

    /// Whether to share discovered metadata with upstream proxies
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_with_upstream: Option<bool>,

    /// Additional configuration fields (catch-all for forward compatibility)
    #[serde(flatten)]
    pub additional_fields: Option<serde_json::Map<String, JsonValue>>,
}

impl Default for PeerMetadataConfig {
    fn default() -> Self {
        Self {
            downstream_discovery: None,
            upstream_discovery: None,
            shared_with_upstream: Some(false),
            additional_fields: None,
        }
    }
}

impl TypedStructFilter for PeerMetadataConfig {
    const TYPE_URL: &'static str = "type.googleapis.com/io.istio.http.peer_metadata.Config";

    fn from_json_value(value: JsonValue) -> Result<Self, GenericError> {
        serde_json::from_value(value).map_err(|e| {
            GenericError::from_msg_with_cause(
                "Failed to deserialize PeerMetadataConfig from JSON",
                e,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typed_struct::{ParsedTypedStruct, TypedStructParser, TypedStruct};
    use prost::Message;
    use prost_types::{value::Kind, Struct, Value};
    use std::collections::BTreeMap;

    #[test]
    fn test_deserialize_empty_config() {
        let json = serde_json::json!({});
        let config = PeerMetadataConfig::from_json_value(json).unwrap();
        assert_eq!(config.downstream_discovery, None);
        assert_eq!(config.upstream_discovery, None);
    }

    #[test]
    fn test_deserialize_full_config() {
        let json = serde_json::json!({
            "downstream_discovery": [{"workload_discovery": {}}],
            "upstream_discovery": [{"service_discovery": {}}],
            "shared_with_upstream": true
        });
        let config = PeerMetadataConfig::from_json_value(json).unwrap();
        assert_eq!(config.downstream_discovery.as_ref().unwrap().len(), 1);
        assert_eq!(config.upstream_discovery.as_ref().unwrap().len(), 1);
        assert_eq!(config.shared_with_upstream, Some(true));
    }

    #[test]
    fn test_from_typed_struct() {
        // Create a protobuf Struct with peer metadata config
        let mut fields = BTreeMap::new();
        
        // downstream_discovery is a list with one discovery config object
        let mut discovery_obj = BTreeMap::new();
        discovery_obj.insert(
            "workload_discovery".to_string(),
            Value {
                kind: Some(Kind::StructValue(Struct { fields: BTreeMap::new() })),
            },
        );
        
        fields.insert(
            "downstream_discovery".to_string(),
            Value {
                kind: Some(Kind::ListValue(prost_types::ListValue {
                    values: vec![Value {
                        kind: Some(Kind::StructValue(Struct { fields: discovery_obj })),
                    }],
                })),
            },
        );
        fields.insert(
            "shared_with_upstream".to_string(),
            Value {
                kind: Some(Kind::BoolValue(true)),
            },
        );

        // Encode as TypedStruct
        let typed_struct = TypedStruct {
            type_url: PeerMetadataConfig::TYPE_URL.to_string(),
            value: Some(Struct { fields }),
        };

        let mut buf = Vec::new();
        typed_struct.encode(&mut buf).unwrap();

        // Parse and convert
        let parsed = TypedStructParser::parse(&buf).unwrap();
        let config = PeerMetadataConfig::from_typed_struct(&parsed).unwrap();

        assert_eq!(config.downstream_discovery.as_ref().unwrap().len(), 1);
        assert_eq!(config.shared_with_upstream, Some(true));
    }

    #[test]
    fn test_type_url_validation() {
        let json = serde_json::json!({});
        let parsed = ParsedTypedStruct {
            type_url: "type.googleapis.com/wrong.type.Config".to_string(),
            value: json,
        };

        let result = PeerMetadataConfig::from_typed_struct(&parsed);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("type URL mismatch"));
    }
}
