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

//! TypedStruct parsing and conversion utilities
//! 
//! This module provides infrastructure for handling TypedStruct-wrapped
//! configurations in Envoy/Istio xDS. TypedStruct is used when protobuf
//! descriptors are not available at compile time, wrapping arbitrary
//! JSON-serialized protocol buffer messages.
//!
//! # Architecture
//!
//! ```text
//! xDS Config (Any)
//!     ↓
//! TypedStruct Wrapper (type_url = "udpa.type.v1.TypedStruct")
//!     ↓
//! Inner Config (type_url = actual filter type, value = JSON)
//!     ↓
//! Decode JSON → Rust struct → Apply configuration
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use orion_configuration::typed_struct::{TypedStructParser, TypedStructFilter};
//!
//! // Parse a TypedStruct from xDS
//! let typed_struct = TypedStructParser::parse(&any_value)?;
//! 
//! // Check the inner type
//! match typed_struct.type_url.as_str() {
//!     "type.googleapis.com/io.istio.http.peer_metadata.Config" => {
//!         let config = PeerMetadataConfig::from_typed_struct(&typed_struct)?;
//!         // Use config...
//!     },
//!     _ => {
//!         // Unknown filter type - log warning
//!     }
//! }
//! ```

mod parser;
mod converter;
mod registry;
mod error;

pub use parser::{TypedStructParser, TypedStruct};
pub use converter::JsonConverter;
pub use registry::TypedStructRegistry;
pub use error::TypedStructError;

use crate::config::common::GenericError;
use serde_json::Value as JsonValue;

/// Parsed TypedStruct with type URL and JSON value
#[derive(Debug, Clone)]
pub struct ParsedTypedStruct {
    /// Type URL identifying the inner message type
    pub type_url: String,
    
    /// JSON representation of the configuration
    pub value: JsonValue,
}





/// Trait for filters that can be constructed from TypedStruct
pub trait TypedStructFilter: Sized {
    /// The type URL this filter expects (e.g., "type.googleapis.com/io.istio.http.peer_metadata.Config")
    const TYPE_URL: &'static str;
    
    /// Construct from a ParsedTypedStruct
    fn from_typed_struct(typed_struct: &ParsedTypedStruct) -> Result<Self, GenericError> {
        // Validate type URL matches
        if typed_struct.type_url != Self::TYPE_URL {
            return Err(GenericError::from_msg(format!(
                "TypedStruct type URL mismatch: expected {}, got {}",
                Self::TYPE_URL,
                typed_struct.type_url
            )));
        }
        
        // Deserialize from JSON value
        Self::from_json_value(typed_struct.value.clone())
    }
    
    /// Construct from JSON value
    fn from_json_value(value: JsonValue) -> Result<Self, GenericError>;
}
