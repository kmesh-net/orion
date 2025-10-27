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

mod converter;
mod error;
mod parser;
pub mod registry;

pub use converter::JsonConverter;
pub use error::TypedStructError;
pub use parser::{TypedStruct, TypedStructParser};
pub use registry::{global_registry, DynamicTypedStructParser, GenericFilterParser, TypedStructRegistry};

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
