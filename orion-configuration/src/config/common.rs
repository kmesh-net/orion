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
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    error::Error,
    fmt::{Debug, Display},
    string::FromUtf8Error,
};

pub(crate) fn is_default<T: PartialEq + Default>(value: &T) -> bool {
    *value == T::default()
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum ProxyProtocolVersion {
    V1,
    V2,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub enum TlvType {
    Custom(u8),
    NoOp,
}

impl From<u8> for TlvType {
    fn from(value: u8) -> Self {
        match value {
            0x00 => Self::NoOp,
            other => Self::Custom(other),
        }
    }
}

impl From<TlvType> for u8 {
    fn from(tlv_type: TlvType) -> Self {
        match tlv_type {
            TlvType::NoOp => 0x00,
            TlvType::Custom(value) => value,
        }
    }
}

pub(crate) trait RegexExtension {
    fn matches_full(&self, to_match: &str) -> bool;
}

impl RegexExtension for Regex {
    fn matches_full(&self, to_match: &str) -> bool {
        Some(to_match.len()) == self.find_at(to_match, 0).map(|find_result| find_result.len())
    }
}

enum TraceNode {
    Field(Cow<'static, str>),
    Name(Cow<'static, str>),
    Index(usize),
}

impl Display for TraceNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceNode::Field(field) => f.write_str(field),
            TraceNode::Name(name) => f.write_str(&format!("[\"{name}\"]")),
            TraceNode::Index(index) => f.write_str(&format!("[{index}]")),
        }
    }
}

struct FieldTrace {
    vec: Vec<TraceNode>,
}

impl Display for FieldTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let is_named = self.vec.iter().map(|field| matches!(field, TraceNode::Name(_)));
        let inner_is_named = is_named.rev().skip(1).chain(std::iter::once(false));

        let mut iter = self.vec.iter().rev().zip(inner_is_named);
        if let Some((first, _)) = iter.next() {
            first.fmt(f)?;
        }
        for (field, inner_is_named) in iter {
            if matches!(field, TraceNode::Field(_)) {
                f.write_str(" / ")?;
                field.fmt(f)?;
            } else if matches!(field, TraceNode::Name(_)) || !inner_is_named {
                f.write_str(" ")?;
                field.fmt(f)?;
            }
        }
        Ok(())
    }
}

impl Debug for FieldTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (&self as &dyn Display).fmt(f)
    }
}

impl FieldTrace {
    fn new() -> Self {
        Self { vec: Vec::new() }
    }

    fn push(&mut self, value: TraceNode) {
        self.vec.push(value);
    }
}

impl<T: Into<Cow<'static, str>>> From<T> for FieldTrace {
    fn from(value: T) -> Self {
        Self { vec: vec![TraceNode::Field(value.into())] }
    }
}

#[derive(thiserror::Error, Debug)]
#[allow(private_interfaces)]
pub enum GenericError {
    #[error("Error parsing field {0}")]
    TracedError(FieldTrace, #[source] Box<Self>),
    #[error("{0}")]
    MessageWithCause(Cow<'static, str>, #[source] Box<dyn Error + Send + Sync + 'static>),
    #[error("{0}")]
    Message(Cow<'static, str>),
    #[error("Unsupported enum variant {0}")]
    UnsupportedVariant(Cow<'static, str>),
    #[error("Missing field: {0}")]
    MissingField(&'static str),
    #[error("Unsupported field: {0}")]
    UnsupportedField(&'static str),
    #[error("DataSource: {0}")]
    DataSource(#[from] DataSourceReadError),
    #[error("Utf8: {0}")]
    Utf8(#[from] FromUtf8Error),
}

impl GenericError {
    #[must_use]
    pub(crate) fn with_node<T: Into<Cow<'static, str>>>(self, node: T) -> Self {
        self.with_trace_node(TraceNode::Field(node.into()))
    }

    #[must_use]
    pub(crate) fn with_index(self, index: usize) -> Self {
        self.with_trace_node(TraceNode::Index(index))
    }

    #[must_use]
    pub(crate) fn with_name<T: Into<Cow<'static, str>>>(self, name: T) -> Self {
        self.with_trace_node(TraceNode::Name(name.into()))
    }

    #[must_use]
    fn with_trace_node(self, node: TraceNode) -> Self {
        match self {
            Self::TracedError(mut fields, error) => {
                fields.push(node);
                Self::TracedError(fields, error)
            },
            other => {
                let mut fields = FieldTrace::new();
                fields.push(node);
                Self::TracedError(fields, other.into())
            },
        }
    }

    pub(crate) fn unsupported_variant<T: Into<Cow<'static, str>>>(variant: T) -> Self {
        Self::UnsupportedVariant(variant.into())
    }

    pub fn from_msg<T: Into<Cow<'static, str>>>(msg: T) -> Self {
        Self::Message(msg.into())
    }

    pub fn from_msg_with_cause<T: Into<Cow<'static, str>>, E: Error + Send + Sync + 'static>(msg: T, cause: E) -> Self {
        Self::MessageWithCause(msg.into(), cause.into())
    }
}

pub(crate) trait WithNodeOnResult {
    fn with_node<T: Into<Cow<'static, str>>>(self, node: T) -> Self;
    fn with_index(self, index: usize) -> Self;
    fn with_name<T: Into<Cow<'static, str>>>(self, name: T) -> Self;
}

impl<T> WithNodeOnResult for Result<T, GenericError> {
    fn with_node<Node: Into<Cow<'static, str>>>(self, node: Node) -> Self {
        self.map_err(|e| e.with_node(node))
    }

    fn with_index(self, index: usize) -> Self {
        self.map_err(|e| e.with_index(index))
    }

    fn with_name<Node: Into<Cow<'static, str>>>(self, name: Node) -> Self {
        self.map_err(|e| e.with_name(name))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataKey {
    pub key: CompactString,
    pub path: Vec<CompactString>,
}

#[cfg(feature = "envoy-conversions")]
pub use envoy_conversions::*;

use super::core::DataSourceReadError;

#[cfg(feature = "envoy-conversions")]
pub(crate) mod envoy_conversions {
    use super::*;
    use std::{collections::HashMap, hash::BuildHasher};

    /// Trait that checks if an envoy field was explicitly set by the user.
    /// Used to check that the user isn't using unsupported fields
    ///
    /// for options this is straightforwardly "`is_some`" but for other fields it
    /// gets a bit more complicated.
    ///
    /// integers,and enums reprsented by integers, are not `Option<i32>` bur plain `i32`
    /// and will be initialized to their default value (0) if not set.
    /// Same for bools which become false.
    /// Therefore we should always support this default value, since we can't tell if the user
    /// explicitly requested this behaviour or if it was default-filled.
    ///
    /// Strings and collections  will be set to their empty equivalent.
    pub trait IsUsed {
        type Checked;
        fn is_used(&self) -> bool;
        fn checked(self, name: &'static str) -> Result<Self::Checked, GenericError>;
    }

    impl<T> IsUsed for Option<T> {
        type Checked = T;
        fn is_used(&self) -> bool {
            self.is_some()
        }

        fn checked(self, name: &'static str) -> Result<Self::Checked, GenericError> {
            self.ok_or(GenericError::MissingField(name))
        }
    }

    impl<T> IsUsed for Vec<T> {
        //non-empty vec
        type Checked = Vec<T>;
        fn is_used(&self) -> bool {
            !self.is_empty()
        }
        fn checked(self, name: &'static str) -> Result<Self::Checked, GenericError> {
            self.is_used().then_some(self).ok_or(GenericError::MissingField(name))
        }
    }

    impl IsUsed for String {
        type Checked = Self;
        fn is_used(&self) -> bool {
            !self.is_empty()
        }
        fn checked(self, name: &'static str) -> Result<Self::Checked, GenericError> {
            self.is_used().then_some(self).ok_or(GenericError::MissingField(name))
        }
    }

    impl IsUsed for bool {
        type Checked = Self;
        fn is_used(&self) -> bool {
            //protobuf default is false
            *self
        }
        fn checked(self, name: &'static str) -> Result<Self::Checked, GenericError> {
            self.is_used().then_some(self).ok_or(GenericError::MissingField(name))
        }
    }

    impl IsUsed for i32 {
        type Checked = Self;
        fn is_used(&self) -> bool {
            //protobuf default is zero
            *self != 0
        }
        fn checked(self, name: &'static str) -> Result<Self::Checked, GenericError> {
            self.is_used().then_some(self).ok_or(GenericError::MissingField(name))
        }
    }

    impl IsUsed for i64 {
        type Checked = Self;
        fn is_used(&self) -> bool {
            //protobuf default is zero
            *self != 0
        }
        fn checked(self, name: &'static str) -> Result<Self::Checked, GenericError> {
            self.is_used().then_some(self).ok_or(GenericError::MissingField(name))
        }
    }

    impl IsUsed for u32 {
        type Checked = Self;
        fn is_used(&self) -> bool {
            //protobuf default is zero
            *self != 0
        }
        fn checked(self, name: &'static str) -> Result<Self::Checked, GenericError> {
            self.is_used().then_some(self).ok_or(GenericError::MissingField(name))
        }
    }

    impl<K, V, S: BuildHasher> IsUsed for HashMap<K, V, S> {
        type Checked = Self;
        fn is_used(&self) -> bool {
            !self.is_empty()
        }
        fn checked(self, name: &'static str) -> Result<Self::Checked, GenericError> {
            self.is_used().then_some(self).ok_or(GenericError::MissingField(name))
        }
    }
    // it would be nice to allow for x = "y" syntax to overwrite the field name, since some fields are
    // named differently in the code vs config file and having the code-local name might confuse an end-user
    macro_rules! unsupported_field {
        ($field:ident) => {{
            if $field.is_used() {
                tracing::warn!(
                    "unsupported field '{}' used in configuration. This field will be ignored.",
                    stringify!($field)
                );
            }
            #[allow(dropping_copy_types, clippy::drop_non_drop)]
            drop($field);
            Result::<(), GenericError>::Ok(())
        }};
        ($field:ident, $($tail:ident),+) => {{
            if $field.is_used() {
                tracing::warn!(
                    "unsupported field '{}' used in configuration. This field will be ignored.",
                    stringify!($field)
                );
            }
            #[allow(dropping_copy_types, clippy::drop_non_drop)]
            drop($field);
            unsupported_field!($($tail),+)
        }};

    }
    pub(crate) use unsupported_field;

    macro_rules! required {
        ($field:ident) => {
            $field.checked(stringify!($field))
        };
    }
    pub(crate) use required;

    macro_rules! convert_opt {
        ($field:ident) => {
            match $field {
                None => Err(GenericError::MissingField(stringify!($field))),
                Some(envoy) => envoy.try_into().map_err(|e: GenericError| e.with_node(stringify!($field))),
            }
        };
        ($field:ident, $field_name:expr) => {
            match $field {
                None => Err(GenericError::MissingField($field_name)),
                Some(envoy) => envoy.try_into().map_err(|e: GenericError| e.with_node($field_name)),
            }
        };
    }
    pub(crate) use convert_opt;

    macro_rules! convert_non_empty_vec {
        ($field:ident) => {
            if !$field.is_used() {
                Err(GenericError::MissingField(stringify!($field)))
            } else {
                convert_vec!($field)
            }
        };
    }
    pub(crate) use convert_non_empty_vec;

    macro_rules! convert_vec {
        ($field:ident) => {
            $field
                .into_iter()
                .enumerate()
                .map(|(index, envoy)| envoy.try_into().with_index(index))
                .collect::<std::result::Result<Vec<_>, GenericError>>()
                .map_err(|e: GenericError| e.with_node(stringify!($field)))
        };
    }
    pub(crate) use convert_vec;
}
