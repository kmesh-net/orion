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

//! SetFilterState filter implementation
//!
//! This module implements the set_filter_state HTTP filter, which allows setting
//! filter state values based on format strings evaluated from request properties.
//!
//! Follows Envoy's pattern where filters apply in decodeHeaders() with access to stream context.

use crate::format_string::{FormatStringEvaluator, RequestContext};
use crate::listeners::filter_state::DownstreamMetadata;
use crate::{FilterStateExtension, SharedWithUpstream as RuntimeSharedWithUpstream};
use http::Request;
use orion_configuration::config::network_filters::http_connection_manager::http_filters::set_filter_state::{
    FormatString, SetFilterState, SharedWithUpstream as ConfigSharedWithUpstream,
};

use super::FilterDecision;

/// Applies set_filter_state configuration to a request
///
/// This function evaluates format strings and stores the resulting values in the request's
/// FilterStateExtension (via HTTP extensions) for use in routing decisions and upstream propagation.
///
/// # Arguments
///
/// * `config` - The SetFilterState configuration
/// * `request` - The HTTP request to modify
/// * `downstream_metadata` - Metadata about the downstream connection
/// * `format_evaluator` - Evaluator for format strings
///
/// # Returns
///
/// Always returns `FilterDecision::Continue` as SetFilterState never interrupts the pipeline
pub(super) fn apply_set_filter_state<B>(
    config: &SetFilterState,
    request: &mut Request<B>,
    downstream_metadata: &DownstreamMetadata,
    format_evaluator: &FormatStringEvaluator,
) -> FilterDecision {
    tracing::trace!("Applying SetFilterState filter: method={}, uri={}", request.method(), request.uri());

    let mut ctx = RequestContext::new();

    for (name, value) in request.headers() {
        match value.to_str() {
            Ok(value_str) => ctx = ctx.with_header(name.as_str(), value_str),
            Err(_) => tracing::warn!(
                header_name = %name.as_str(),
                "Skipping non-UTF8 header value in format string context"
            ),
        }
    }

    if ctx.get_header(":authority").is_none() {
        if let Some(host) = ctx.get_header("host") {
            let host = host.clone();
            ctx = ctx.with_header(":authority", host);
        }
    }

    ctx = ctx.with_method(request.method().as_str());

    ctx = ctx.with_header(":method", request.method().as_str());

    ctx = ctx.with_path(request.uri().path());

    ctx = ctx.with_protocol(format!("{:?}", request.version()));

    let remote_addr = downstream_metadata.connection.peer_address();
    ctx = ctx.with_downstream_remote_address(remote_addr);

    let local_addr = downstream_metadata.connection.local_address();
    ctx = ctx.with_downstream_local_address(local_addr);

    for value_config in &config.on_request_headers {
        let evaluated_value = match &value_config.format_string {
            FormatString::Text(template) => format_evaluator.evaluate(&template, &ctx),
            FormatString::Structured { .. } => {
                tracing::warn!(
                    "Structured format strings not yet supported for filter state key '{}'",
                    value_config.object_key
                );
                continue;
            },
        };

        if value_config.skip_if_empty && (evaluated_value.is_empty() || evaluated_value == "-") {
            tracing::debug!("Skipping filter state key '{}' due to empty value", value_config.object_key);
            continue;
        }

        let shared_with_upstream = match value_config.shared_with_upstream {
            ConfigSharedWithUpstream::None => RuntimeSharedWithUpstream::None,
            ConfigSharedWithUpstream::Once => RuntimeSharedWithUpstream::Once,
            ConfigSharedWithUpstream::Transitive => RuntimeSharedWithUpstream::Transitive,
        };

        let filter_state = if let Some(fs) = request.extensions_mut().get_mut::<FilterStateExtension>() {
            fs
        } else {
            request.extensions_mut().insert(FilterStateExtension::default());
            request.extensions_mut().get_mut::<FilterStateExtension>().expect("FilterStateExtension was just inserted")
        };

        if let Err(e) = filter_state.set(
            value_config.object_key.as_str(),
            evaluated_value.as_str(),
            value_config.read_only,
            shared_with_upstream,
        ) {
            tracing::warn!("Failed to set filter state key '{}': {}", value_config.object_key, e);
        } else {
            tracing::debug!(
                "Set filter state: {} = {} (read_only={}, shared={:?})",
                value_config.object_key,
                evaluated_value,
                value_config.read_only,
                shared_with_upstream
            );
        }
    }

    FilterDecision::Continue
}
