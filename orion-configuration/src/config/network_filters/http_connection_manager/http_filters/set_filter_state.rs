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

use compact_str::CompactString;
use serde::{Deserialize, Serialize};

use super::super::is_default;

/// Set Filter State HTTP filter configuration
/// 
/// This filter dynamically sets filter state objects based on request data.
/// Filter state can be used for routing decisions, metadata propagation,
/// and internal connection handling.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SetFilterState {
    /// Values to set when request headers are received
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub on_request_headers: Vec<FilterStateValue>,
}

/// A filter state key-value pair configuration
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FilterStateValue {
    /// Filter state object key (required)
    /// 
    /// Examples:
    /// - "io.istio.connect_authority" (Istio HBONE)
    /// - "envoy.filters.listener.original_dst.local_ip"
    /// - "envoy.tcp_proxy.cluster"
    pub object_key: CompactString,
    
    /// Optional factory key for object creation
    /// If not specified, object_key is used for factory lookup
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub factory_key: Option<CompactString>,
    
    /// Format string to generate the value
    /// Supports Envoy substitution format strings like:
    /// - %REQ(:authority)% - Request header
    /// - %DOWNSTREAM_REMOTE_ADDRESS% - Client IP
    /// - %UPSTREAM_HOST% - Selected upstream
    pub format_string: FormatString,
    
    /// Make this value read-only (cannot be overridden by other filters)
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub read_only: bool,
    
    /// Share with upstream internal connections
    #[serde(skip_serializing_if = "is_default", default)]
    pub shared_with_upstream: SharedWithUpstream,
    
    /// Skip setting the value if it evaluates to empty string
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub skip_if_empty: bool,
}

/// Upstream sharing mode for filter state values
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SharedWithUpstream {
    /// Not shared with upstream connections (default)
    #[default]
    None,
    /// Shared with immediate upstream internal connection
    Once,
    /// Shared transitively through the entire internal connection chain
    Transitive,
}

/// Format string for generating filter state values
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum FormatString {
    /// Plain text format string with command operators
    /// Example: "%REQ(:authority)%"
    Text(CompactString),
    
    /// Structured format (JSON, etc.) - future extension
    Structured {
        format: CompactString,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        json_format: Option<serde_json::Value>,
    },
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    use super::*;
    use crate::config::common::*;
    use orion_data_plane_api::envoy_data_plane_api::{
        envoy::{
            config::core::v3::SubstitutionFormatString as EnvoySubstitutionFormatString,
            extensions::filters::{
                common::set_filter_state::v3::{
                    FilterStateValue as EnvoyFilterStateValue,
                    filter_state_value::{
                        Key as EnvoyKey, 
                        Value as EnvoyValue,
                        SharedWithUpstream as EnvoySharedWithUpstream
                    },
                },
                http::set_filter_state::v3::Config as EnvoySetFilterStateConfig,
            },
        },
    };
    
    impl TryFrom<EnvoySetFilterStateConfig> for SetFilterState {
        type Error = GenericError;
        
        fn try_from(envoy: EnvoySetFilterStateConfig) -> Result<Self, Self::Error> {
            let on_request_headers = envoy.on_request_headers
                .into_iter()
                .map(FilterStateValue::try_from)
                .collect::<Result<Vec<_>, _>>()
                .with_node("on_request_headers")?;
            
            Ok(Self { on_request_headers })
        }
    }
    
    impl TryFrom<EnvoyFilterStateValue> for FilterStateValue {
        type Error = GenericError;
        
        fn try_from(envoy: EnvoyFilterStateValue) -> Result<Self, Self::Error> {
            let object_key = match envoy.key {
                Some(EnvoyKey::ObjectKey(key)) => CompactString::from(key),
                None => return Err(GenericError::from_msg("missing object_key in FilterStateValue")),
            };
            
            let factory_key = if envoy.factory_key.is_empty() {
                None
            } else {
                Some(envoy.factory_key.into())
            };
            
            let format_string = match envoy.value {
                Some(EnvoyValue::FormatString(fs)) => FormatString::try_from(fs).with_node("format_string")?,
                None => return Err(GenericError::from_msg("missing format_string in FilterStateValue")),
            };
            
            let shared_with_upstream = SharedWithUpstream::try_from(envoy.shared_with_upstream)
                .with_node("shared_with_upstream")?;
            
            Ok(Self {
                object_key,
                factory_key,
                format_string,
                read_only: envoy.read_only,
                shared_with_upstream,
                skip_if_empty: envoy.skip_if_empty,
            })
        }
    }
    
    impl TryFrom<EnvoySubstitutionFormatString> for FormatString {
        type Error = GenericError;
        
        fn try_from(envoy: EnvoySubstitutionFormatString) -> Result<Self, Self::Error> {
            use orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::{
                substitution_format_string::Format,
                data_source::Specifier,
            };
            
            match envoy.format {
                Some(Format::TextFormat(text)) => Ok(FormatString::Text(text.into())),
                Some(Format::TextFormatSource(source)) => {
                    match source.specifier {
                        Some(Specifier::InlineString(s)) => Ok(FormatString::Text(s.into())),
                        Some(Specifier::InlineBytes(b)) => {
                            let s = String::from_utf8(b)
                                .map_err(|e| GenericError::from_msg(format!("Invalid UTF-8 in format string: {}", e)))?;
                            Ok(FormatString::Text(s.into()))
                        },
                        Some(Specifier::Filename(_)) => {
                            Err(GenericError::unsupported_variant("filename format strings not supported"))
                        },
                        Some(Specifier::EnvironmentVariable(_)) => {
                            Err(GenericError::unsupported_variant("environment variable format strings not supported"))
                        },
                        None => Err(GenericError::from_msg("missing format string specifier")),
                    }
                },
                Some(Format::JsonFormat(_)) => {
                    // JSON format not yet supported - would need structured logging
                    Err(GenericError::unsupported_variant("json_format not yet supported"))
                },
                None => Err(GenericError::from_msg("missing format in SubstitutionFormatString")),
            }
        }
    }
    impl TryFrom<i32> for SharedWithUpstream {
        type Error = GenericError;
        
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            match EnvoySharedWithUpstream::try_from(value) {
                Ok(EnvoySharedWithUpstream::None) => Ok(Self::None),
                Ok(EnvoySharedWithUpstream::Once) => Ok(Self::Once),
                Ok(EnvoySharedWithUpstream::Transitive) => Ok(Self::Transitive),
                Err(_) => Err(GenericError::from_msg(format!("Invalid SharedWithUpstream value: {}", value))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_with_upstream_default() {
        assert_eq!(SharedWithUpstream::default(), SharedWithUpstream::None);
    }

    #[test]
    fn test_format_string_text() {
        let format = FormatString::Text("%REQ(:authority)%".into());
        match format {
            FormatString::Text(s) => assert_eq!(s.as_str(), "%REQ(:authority)%"),
            _ => panic!("Expected Text variant"),
        }
    }
}
