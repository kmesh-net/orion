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

//! Filter State - Dynamic per-request metadata storage
//!
//! This module implements the filter state mechanism used by Envoy/Istio to store
//! dynamic metadata that can be:
//! - Set by filters during request processing
//! - Used for routing decisions
//! - Propagated to upstream connections
//! - Protected with read-only semantics
//!
//! # Architecture
//!
//! Filter state is stored per-request using HTTP request extensions, following
//! the same pattern as `ResponseFlags` and `EventKind` in the codebase.
//! This eliminates the need for global storage and locking.
//!
//! # Example
//! ```rust,ignore
//! use orion_lib::filter_state::{FilterStateExtension, SharedWithUpstream};
//!
//! // Access from request extensions
//! let filter_state = request.extensions_mut()
//!     .entry::<FilterStateExtension>()
//!     .or_insert_with(FilterStateExtension::default);
//!
//! filter_state.set("my.key", "value", false, SharedWithUpstream::Once)?;
//! let value = filter_state.get("my.key")?;
//! ```

use compact_str::CompactString;
use std::collections::HashMap;
use thiserror::Error;

/// Errors that can occur during filter state operations
#[derive(Error, Debug, Clone, PartialEq)]
pub enum FilterStateError {
    #[error("Filter state key '{0}' not found")]
    KeyNotFound(CompactString),

    #[error("Filter state key '{0}' is read-only and cannot be modified")]
    ReadOnly(CompactString),

    #[error("Invalid filter state value for key '{0}'")]
    InvalidValue(CompactString),
}

/// Defines how filter state is shared with upstream connections
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SharedWithUpstream {
    /// Not shared with upstream (local to this connection only)
    None,
    /// Shared with immediate upstream connection
    Once,
    /// Shared with all upstream connections in the chain
    Transitive,
}

impl Default for SharedWithUpstream {
    fn default() -> Self {
        Self::None
    }
}

/// A single filter state entry
#[derive(Debug, Clone)]
pub struct FilterStateEntry {
    /// The stored value (currently only strings are supported)
    pub value: CompactString,
    /// Whether this entry is read-only (cannot be overwritten)
    pub read_only: bool,
    /// How this entry should be shared with upstream
    pub shared_with_upstream: SharedWithUpstream,
}

/// Per-request filter state storage for dynamic metadata
///
/// This extension is stored in HTTP request extensions and contains key-value pairs
/// that can be set by filters and used for routing decisions, logging, or propagation
/// to upstream connections.
///
/// # Usage
///
/// Store in request extensions:
/// ```rust,ignore
/// request.extensions_mut().insert(FilterStateExtension::default());
/// ```
///
/// Access from request:
/// ```rust,ignore
/// if let Some(filter_state) = request.extensions().get::<FilterStateExtension>() {
///     let value = filter_state.get("my.key")?;
/// }
/// ```
///
/// # Thread Safety
///
/// No locking needed - each request has its own instance that is automatically
/// cleaned up when the request completes.
#[derive(Debug, Clone, Default)]
pub struct FilterStateExtension {
    /// Internal storage - no lock needed since this is per-request
    entries: HashMap<CompactString, FilterStateEntry>,
}

impl FilterStateExtension {
    /// Creates a new empty FilterStateExtension
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a filter state value
    ///
    /// # Arguments
    /// * `key` - The filter state key (e.g., "io.istio.connect_authority")
    /// * `value` - The value to store
    /// * `read_only` - If true, prevents future modifications to this key
    /// * `shared_with_upstream` - How to share with upstream connections
    ///
    /// # Errors
    /// Returns `FilterStateError::ReadOnly` if the key already exists and is read-only
    pub fn set(
        &mut self,
        key: impl Into<CompactString>,
        value: impl Into<CompactString>,
        read_only: bool,
        shared_with_upstream: SharedWithUpstream,
    ) -> Result<(), FilterStateError> {
        let key = key.into();

        // Check if key exists and is read-only
        if let Some(existing) = self.entries.get(&key) {
            if existing.read_only {
                return Err(FilterStateError::ReadOnly(key));
            }
        }

        self.entries.insert(key, FilterStateEntry { value: value.into(), read_only, shared_with_upstream });

        Ok(())
    }

    /// Gets a filter state value
    ///
    /// # Arguments
    /// * `key` - The filter state key to retrieve
    ///
    /// # Returns
    /// The value if the key exists
    ///
    /// # Errors
    /// Returns `FilterStateError::KeyNotFound` if the key doesn't exist
    pub fn get(&self, key: &str) -> Result<&CompactString, FilterStateError> {
        self.entries.get(key).map(|entry| &entry.value).ok_or_else(|| FilterStateError::KeyNotFound(key.into()))
    }

    /// Checks if a key exists in the filter state
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Gets a value with its metadata
    ///
    /// Returns tuple of (value, read_only, shared_with_upstream)
    pub fn get_with_metadata(&self, key: &str) -> Result<(&CompactString, bool, SharedWithUpstream), FilterStateError> {
        self.entries
            .get(key)
            .map(|entry| (&entry.value, entry.read_only, entry.shared_with_upstream))
            .ok_or_else(|| FilterStateError::KeyNotFound(key.into()))
    }

    /// Gets all keys that should be shared with upstream connections
    ///
    /// # Arguments
    /// * `mode` - The minimum sharing mode to include
    ///   - `Once`: Returns keys with Once or Transitive
    ///   - `Transitive`: Returns only Transitive keys
    ///
    /// # Returns
    /// HashMap of keys and their values that should be propagated upstream
    pub fn get_upstream_shared(&self, mode: SharedWithUpstream) -> HashMap<CompactString, CompactString> {
        self.entries
            .iter()
            .filter(|(_, entry)| match mode {
                SharedWithUpstream::None => false,
                SharedWithUpstream::Once => {
                    entry.shared_with_upstream == SharedWithUpstream::Once
                        || entry.shared_with_upstream == SharedWithUpstream::Transitive
                },
                SharedWithUpstream::Transitive => entry.shared_with_upstream == SharedWithUpstream::Transitive,
            })
            .map(|(k, v)| (k.clone(), v.value.clone()))
            .collect()
    }

    /// Clears all filter state entries
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Returns the number of entries in the filter state
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the filter state is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns an iterator over all entries
    pub fn iter(&self) -> impl Iterator<Item = (&CompactString, &FilterStateEntry)> {
        self.entries.iter()
    }
}

// Backwards compatibility alias - users should migrate to FilterStateExtension
#[deprecated(since = "0.1.0", note = "Use FilterStateExtension instead - stores per-request via HTTP extensions")]
pub type FilterState = FilterStateExtension;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_filter_state() {
        let state = FilterStateExtension::new();
        assert_eq!(state.len(), 0);
        assert!(state.is_empty());
    }

    #[test]
    fn test_set_and_get() {
        let mut state = FilterStateExtension::new();

        state.set("test.key", "test_value", false, SharedWithUpstream::None).unwrap();

        assert_eq!(state.get("test.key").unwrap().as_str(), "test_value");
        assert_eq!(state.len(), 1);
        assert!(!state.is_empty());
    }

    #[test]
    fn test_get_nonexistent_key() {
        let state = FilterStateExtension::new();

        let result = state.get("nonexistent");
        assert!(matches!(result, Err(FilterStateError::KeyNotFound(_))));
    }

    #[test]
    fn test_read_only_enforcement() {
        let mut state = FilterStateExtension::new();

        // Set initial value as read-only
        state.set("readonly.key", "initial", true, SharedWithUpstream::None).unwrap();

        // Try to overwrite - should fail
        let result = state.set("readonly.key", "modified", false, SharedWithUpstream::None);

        assert!(matches!(result, Err(FilterStateError::ReadOnly(_))));
        assert_eq!(state.get("readonly.key").unwrap().as_str(), "initial");
    }

    #[test]
    fn test_overwrite_non_readonly() {
        let mut state = FilterStateExtension::new();

        state.set("mutable.key", "first", false, SharedWithUpstream::None).unwrap();

        state.set("mutable.key", "second", false, SharedWithUpstream::None).unwrap();

        assert_eq!(state.get("mutable.key").unwrap().as_str(), "second");
    }

    #[test]
    fn test_contains_key() {
        let mut state = FilterStateExtension::new();

        assert!(!state.contains_key("test.key"));

        state.set("test.key", "value", false, SharedWithUpstream::None).unwrap();

        assert!(state.contains_key("test.key"));
    }

    #[test]
    fn test_get_with_metadata() {
        let mut state = FilterStateExtension::new();

        state.set("meta.key", "value", true, SharedWithUpstream::Once).unwrap();

        let (value, read_only, shared) = state.get_with_metadata("meta.key").unwrap();

        assert_eq!(value.as_str(), "value");
        assert!(read_only);
        assert_eq!(shared, SharedWithUpstream::Once);
    }

    #[test]
    fn test_upstream_shared_none() {
        let mut state = FilterStateExtension::new();

        state.set("local", "v1", false, SharedWithUpstream::None).unwrap();
        state.set("once", "v2", false, SharedWithUpstream::Once).unwrap();
        state.set("trans", "v3", false, SharedWithUpstream::Transitive).unwrap();

        let shared = state.get_upstream_shared(SharedWithUpstream::None);
        assert_eq!(shared.len(), 0);
    }

    #[test]
    fn test_upstream_shared_once() {
        let mut state = FilterStateExtension::new();

        state.set("local", "v1", false, SharedWithUpstream::None).unwrap();
        state.set("once", "v2", false, SharedWithUpstream::Once).unwrap();
        state.set("trans", "v3", false, SharedWithUpstream::Transitive).unwrap();

        let shared = state.get_upstream_shared(SharedWithUpstream::Once);
        assert_eq!(shared.len(), 2);
        assert_eq!(shared.get("once").unwrap().as_str(), "v2");
        assert_eq!(shared.get("trans").unwrap().as_str(), "v3");
    }

    #[test]
    fn test_upstream_shared_transitive() {
        let mut state = FilterStateExtension::new();

        state.set("local", "v1", false, SharedWithUpstream::None).unwrap();
        state.set("once", "v2", false, SharedWithUpstream::Once).unwrap();
        state.set("trans", "v3", false, SharedWithUpstream::Transitive).unwrap();

        let shared = state.get_upstream_shared(SharedWithUpstream::Transitive);
        assert_eq!(shared.len(), 1);
        assert_eq!(shared.get("trans").unwrap().as_str(), "v3");
    }

    #[test]
    fn test_clear() {
        let mut state = FilterStateExtension::new();

        state.set("key1", "v1", false, SharedWithUpstream::None).unwrap();
        state.set("key2", "v2", false, SharedWithUpstream::None).unwrap();

        assert_eq!(state.len(), 2);

        state.clear();

        assert_eq!(state.len(), 0);
        assert!(state.is_empty());
    }

    #[test]
    fn test_istio_connect_authority_use_case() {
        let mut state = FilterStateExtension::new();

        // Simulate Istio waypoint setting connect authority
        state
            .set(
                "io.istio.connect_authority",
                "my-service.default.svc.cluster.local:8080",
                true,                     // read-only to prevent tampering
                SharedWithUpstream::Once, // share with immediate upstream
            )
            .unwrap();

        // Verify the value is set
        assert_eq!(
            state.get("io.istio.connect_authority").unwrap().as_str(),
            "my-service.default.svc.cluster.local:8080"
        );

        // Verify it's included in upstream sharing
        let shared = state.get_upstream_shared(SharedWithUpstream::Once);
        assert!(shared.contains_key("io.istio.connect_authority"));

        // Verify read-only protection
        let result = state.set("io.istio.connect_authority", "tampered", false, SharedWithUpstream::None);
        assert!(matches!(result, Err(FilterStateError::ReadOnly(_))));
    }

    #[test]
    fn test_per_request_isolation() {
        // Simulate two separate requests with their own filter state
        let mut request1_state = FilterStateExtension::new();
        let mut request2_state = FilterStateExtension::new();

        // Set different values in each request's state
        request1_state.set("key", "request1_value", false, SharedWithUpstream::None).unwrap();
        request2_state.set("key", "request2_value", false, SharedWithUpstream::None).unwrap();

        // Verify isolation - each request has its own value
        assert_eq!(request1_state.get("key").unwrap().as_str(), "request1_value");
        assert_eq!(request2_state.get("key").unwrap().as_str(), "request2_value");

        // No cross-contamination
        assert_ne!(request1_state.get("key").unwrap(), request2_state.get("key").unwrap());
    }
}
