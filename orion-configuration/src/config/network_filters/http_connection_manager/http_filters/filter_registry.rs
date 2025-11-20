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

use super::peer_metadata::PeerMetadataConfig;
use super::set_filter_state::SetFilterState;
use crate::config::common::GenericError;
use crate::typed_struct::registry::{global_registry, GenericFilterParser};
use crate::typed_struct::TypedStructFilter;

pub fn register_all_filters() -> Result<(), GenericError> {
    let registry = global_registry();

    let peer_metadata_parser = GenericFilterParser::<PeerMetadataConfig>::new(PeerMetadataConfig::TYPE_URL);
    registry.register_dynamic(peer_metadata_parser)?;

    let set_filter_state_parser = GenericFilterParser::<SetFilterState>::new(SetFilterState::TYPE_URL);
    registry.register_dynamic(set_filter_state_parser)?;

    Ok(())
}

pub fn ensure_filters_registered() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        if let Err(e) = register_all_filters() {
            tracing::warn!("Failed to register TypedStruct filters: {}", e);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typed_struct::TypedStructFilter;

    #[test]
    fn test_register_all_filters() {
        ensure_filters_registered();

        let registry = global_registry();
        assert!(registry.is_supported(PeerMetadataConfig::TYPE_URL));
        assert!(registry.is_supported(SetFilterState::TYPE_URL));
    }

    #[test]
    fn test_ensure_filters_registered_idempotent() {
        ensure_filters_registered();
        ensure_filters_registered();

        let registry = global_registry();
        assert!(registry.is_supported(PeerMetadataConfig::TYPE_URL));
        assert!(registry.is_supported(SetFilterState::TYPE_URL));
    }
}
