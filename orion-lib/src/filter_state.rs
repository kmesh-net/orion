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
//! # Example
//! ```rust,ignore
//! use orion_lib::filter_state::{FilterState, SharedWithUpstream};
//!
//! let state = FilterState::new();
//! state.set("my.key", "value".into(), false, SharedWithUpstream::Once)?;
//! let value = state.get("my.key")?;
//! ```

use compact_str::CompactString;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use thiserror::Error;

/// Errors that can occur during filter state operations
#[derive(Error, Debug, Clone, PartialEq)]
pub enum FilterStateError {
    #[error("Filter state key '{0}' not found")]
    KeyNotFound(CompactString),

    #[error("Filter state key '{0}' is read-only and cannot be modified")]
    ReadOnly(CompactString),

    #[error("Filter state lock poisoned")]
    LockPoisoned,

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
struct FilterStateEntry {
    /// The stored value (currently only strings are supported)
    value: CompactString,
    /// Whether this entry is read-only (cannot be overwritten)
    read_only: bool,
    /// How this entry should be shared with upstream
    shared_with_upstream: SharedWithUpstream,
}

/// Thread-safe filter state storage for dynamic per-request metadata
///
/// FilterState stores key-value pairs that can be set by filters and used
/// for routing decisions, logging, or propagation to upstream connections.
///
/// # Thread Safety
/// Uses RwLock for interior mutability, allowing concurrent reads and
/// exclusive writes.
#[derive(Debug, Clone)]
pub struct FilterState {
    /// Internal storage protected by RwLock
    storage: Arc<RwLock<HashMap<CompactString, FilterStateEntry>>>,
}

impl FilterState {
    /// Creates a new empty FilterState
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
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
        &self,
        key: impl Into<CompactString>,
        value: CompactString,
        read_only: bool,
        shared_with_upstream: SharedWithUpstream,
    ) -> Result<(), FilterStateError> {
        let key = key.into();
        let mut storage = self
            .storage
            .write()
            .map_err(|_| FilterStateError::LockPoisoned)?;

        // Check if key exists and is read-only
        if let Some(existing) = storage.get(&key) {
            if existing.read_only {
                return Err(FilterStateError::ReadOnly(key));
            }
        }

        storage.insert(
            key,
            FilterStateEntry {
                value,
                read_only,
                shared_with_upstream,
            },
        );

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
    pub fn get(&self, key: &str) -> Result<CompactString, FilterStateError> {
        let storage = self
            .storage
            .read()
            .map_err(|_| FilterStateError::LockPoisoned)?;

        storage
            .get(key)
            .map(|entry| entry.value.clone())
            .ok_or_else(|| FilterStateError::KeyNotFound(key.into()))
    }

    /// Checks if a key exists in the filter state
    pub fn contains_key(&self, key: &str) -> bool {
        self.storage
            .read()
            .map(|storage| storage.contains_key(key))
            .unwrap_or(false)
    }

    /// Gets a value with its metadata
    ///
    /// Returns tuple of (value, read_only, shared_with_upstream)
    pub fn get_with_metadata(
        &self,
        key: &str,
    ) -> Result<(CompactString, bool, SharedWithUpstream), FilterStateError> {
        let storage = self
            .storage
            .read()
            .map_err(|_| FilterStateError::LockPoisoned)?;

        storage
            .get(key)
            .map(|entry| (entry.value.clone(), entry.read_only, entry.shared_with_upstream))
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
    pub fn get_upstream_shared(
        &self,
        mode: SharedWithUpstream,
    ) -> HashMap<CompactString, CompactString> {
        self.storage
            .read()
            .ok()
            .map(|storage| {
                storage
                    .iter()
                    .filter(|(_, entry)| match mode {
                        SharedWithUpstream::None => false,
                        SharedWithUpstream::Once => {
                            entry.shared_with_upstream == SharedWithUpstream::Once
                                || entry.shared_with_upstream == SharedWithUpstream::Transitive
                        }
                        SharedWithUpstream::Transitive => {
                            entry.shared_with_upstream == SharedWithUpstream::Transitive
                        }
                    })
                    .map(|(k, v)| (k.clone(), v.value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Clears all filter state (useful for connection cleanup)
    pub fn clear(&self) {
        if let Ok(mut storage) = self.storage.write() {
            storage.clear();
        }
    }

    /// Returns the number of entries in the filter state
    pub fn len(&self) -> usize {
        self.storage
            .read()
            .map(|storage| storage.len())
            .unwrap_or(0)
    }

    /// Returns true if the filter state is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for FilterState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_filter_state() {
        let state = FilterState::new();
        assert_eq!(state.len(), 0);
        assert!(state.is_empty());
    }

    #[test]
    fn test_set_and_get() {
        let state = FilterState::new();
        
        state
            .set("test.key", "test_value".into(), false, SharedWithUpstream::None)
            .unwrap();

        assert_eq!(state.get("test.key").unwrap(), "test_value");
        assert_eq!(state.len(), 1);
        assert!(!state.is_empty());
    }

    #[test]
    fn test_get_nonexistent_key() {
        let state = FilterState::new();
        
        let result = state.get("nonexistent");
        assert!(matches!(result, Err(FilterStateError::KeyNotFound(_))));
    }

    #[test]
    fn test_read_only_enforcement() {
        let state = FilterState::new();
        
        // Set initial value as read-only
        state
            .set("readonly.key", "initial".into(), true, SharedWithUpstream::None)
            .unwrap();

        // Try to overwrite - should fail
        let result = state.set(
            "readonly.key",
            "modified".into(),
            false,
            SharedWithUpstream::None,
        );
        
        assert!(matches!(result, Err(FilterStateError::ReadOnly(_))));
        assert_eq!(state.get("readonly.key").unwrap(), "initial");
    }

    #[test]
    fn test_overwrite_non_readonly() {
        let state = FilterState::new();
        
        state
            .set("mutable.key", "first".into(), false, SharedWithUpstream::None)
            .unwrap();

        state
            .set("mutable.key", "second".into(), false, SharedWithUpstream::None)
            .unwrap();

        assert_eq!(state.get("mutable.key").unwrap(), "second");
    }

    #[test]
    fn test_contains_key() {
        let state = FilterState::new();
        
        assert!(!state.contains_key("test.key"));
        
        state
            .set("test.key", "value".into(), false, SharedWithUpstream::None)
            .unwrap();
        
        assert!(state.contains_key("test.key"));
    }

    #[test]
    fn test_get_with_metadata() {
        let state = FilterState::new();
        
        state
            .set("meta.key", "value".into(), true, SharedWithUpstream::Once)
            .unwrap();

        let (value, read_only, shared) = state.get_with_metadata("meta.key").unwrap();
        
        assert_eq!(value, "value");
        assert!(read_only);
        assert_eq!(shared, SharedWithUpstream::Once);
    }

    #[test]
    fn test_upstream_shared_none() {
        let state = FilterState::new();
        
        state.set("local", "v1".into(), false, SharedWithUpstream::None).unwrap();
        state.set("once", "v2".into(), false, SharedWithUpstream::Once).unwrap();
        state.set("trans", "v3".into(), false, SharedWithUpstream::Transitive).unwrap();

        let shared = state.get_upstream_shared(SharedWithUpstream::None);
        assert_eq!(shared.len(), 0);
    }

    #[test]
    fn test_upstream_shared_once() {
        let state = FilterState::new();
        
        state.set("local", "v1".into(), false, SharedWithUpstream::None).unwrap();
        state.set("once", "v2".into(), false, SharedWithUpstream::Once).unwrap();
        state.set("trans", "v3".into(), false, SharedWithUpstream::Transitive).unwrap();

        let shared = state.get_upstream_shared(SharedWithUpstream::Once);
        assert_eq!(shared.len(), 2);
        assert_eq!(shared.get("once").unwrap(), "v2");
        assert_eq!(shared.get("trans").unwrap(), "v3");
    }

    #[test]
    fn test_upstream_shared_transitive() {
        let state = FilterState::new();
        
        state.set("local", "v1".into(), false, SharedWithUpstream::None).unwrap();
        state.set("once", "v2".into(), false, SharedWithUpstream::Once).unwrap();
        state.set("trans", "v3".into(), false, SharedWithUpstream::Transitive).unwrap();

        let shared = state.get_upstream_shared(SharedWithUpstream::Transitive);
        assert_eq!(shared.len(), 1);
        assert_eq!(shared.get("trans").unwrap(), "v3");
    }

    #[test]
    fn test_clear() {
        let state = FilterState::new();
        
        state.set("key1", "v1".into(), false, SharedWithUpstream::None).unwrap();
        state.set("key2", "v2".into(), false, SharedWithUpstream::None).unwrap();
        
        assert_eq!(state.len(), 2);
        
        state.clear();
        
        assert_eq!(state.len(), 0);
        assert!(state.is_empty());
    }

    #[test]
    fn test_istio_connect_authority_use_case() {
        let state = FilterState::new();
        
        // Simulate Istio waypoint setting connect authority
        state
            .set(
                "io.istio.connect_authority",
                "my-service.default.svc.cluster.local:8080".into(),
                true, // read-only to prevent tampering
                SharedWithUpstream::Once, // share with immediate upstream
            )
            .unwrap();

        // Verify the value is set
        assert_eq!(
            state.get("io.istio.connect_authority").unwrap(),
            "my-service.default.svc.cluster.local:8080"
        );

        // Verify it's included in upstream sharing
        let shared = state.get_upstream_shared(SharedWithUpstream::Once);
        assert!(shared.contains_key("io.istio.connect_authority"));

        // Verify read-only protection
        let result = state.set(
            "io.istio.connect_authority",
            "tampered".into(),
            false,
            SharedWithUpstream::None,
        );
        assert!(matches!(result, Err(FilterStateError::ReadOnly(_))));
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let state = Arc::new(FilterState::new());
        let mut handles = vec![];

        // Spawn multiple threads writing different keys
        for i in 0..10 {
            let state_clone = Arc::clone(&state);
            let handle = thread::spawn(move || {
                let key = format!("key{}", i);
                let value = format!("value{}", i);
                state_clone
                    .set(&key, value.into(), false, SharedWithUpstream::None)
                    .unwrap();
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all keys were set
        assert_eq!(state.len(), 10);
        for i in 0..10 {
            let key = format!("key{}", i);
            assert!(state.contains_key(&key));
        }
    }
}
