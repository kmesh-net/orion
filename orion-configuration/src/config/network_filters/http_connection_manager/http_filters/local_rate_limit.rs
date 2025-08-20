// SPDX-FileCopyrightText: © 2025 kmesh authors
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 kmesh authors
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

use http::StatusCode;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalRateLimit {
    #[serde(
        with = "http_serde_ext::status_code",
        skip_serializing_if = "is_default_statuscode",
        default = "default_statuscode_deser"
    )]
    pub status: StatusCode,
    pub max_tokens: u32,
    pub tokens_per_fill: u32,
    #[serde(with = "humantime_serde")]
    pub fill_interval: Duration,
}

const DEFAULT_RATE_LIMIT_STATUSCODE: StatusCode = StatusCode::TOO_MANY_REQUESTS;
const fn default_statuscode_deser() -> StatusCode {
    DEFAULT_RATE_LIMIT_STATUSCODE
}
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_default_statuscode(code: &StatusCode) -> bool {
    *code == DEFAULT_RATE_LIMIT_STATUSCODE
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::LocalRateLimit;
    use crate::config::{
        common::*,
        util::{duration_from_envoy, http_status_from_envoy},
    };
    use http::StatusCode;
    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        extensions::filters::http::local_ratelimit::v3::LocalRateLimit as EnvoyLocalRateLimit,
        r#type::v3::TokenBucket as EnvoyTokenBucket,
    };
    impl TryFrom<EnvoyLocalRateLimit> for LocalRateLimit {
        type Error = GenericError;
        fn try_from(value: EnvoyLocalRateLimit) -> Result<Self, Self::Error> {
            let EnvoyLocalRateLimit {
                stat_prefix,
                status,
                token_bucket,
                filter_enabled,
                filter_enforced,
                request_headers_to_add_when_not_enforced,
                response_headers_to_add,
                descriptors,
                stage,
                local_rate_limit_per_downstream_connection,
                enable_x_ratelimit_headers,
                vh_rate_limits,
                always_consume_default_token_bucket,
                rate_limited_as_resource_exhausted,
                local_cluster_rate_limit,
                rate_limits,
                max_dynamic_descriptors,
            } = value;
            unsupported_field!(
                // stat_prefix,
                // status,
                // token_bucket,
                filter_enabled,
                filter_enforced,
                request_headers_to_add_when_not_enforced,
                response_headers_to_add,
                descriptors,
                stage,
                local_rate_limit_per_downstream_connection,
                enable_x_ratelimit_headers,
                vh_rate_limits,
                always_consume_default_token_bucket,
                rate_limited_as_resource_exhausted,
                local_cluster_rate_limit,
                rate_limits,
                max_dynamic_descriptors
            )?;
            if stat_prefix.is_used() {
                tracing::warn!("stat_prefix used in local_rate_limit, this field will be ignored.");
            }
            //note(hayley): envoy sets status codes <400 to 429 here.
            // we might want to do some validation too
            let status = status
                .map(http_status_from_envoy)
                .transpose()
                .with_node("status")?
                .unwrap_or(StatusCode::TOO_MANY_REQUESTS);
            let EnvoyTokenBucket { max_tokens, tokens_per_fill, fill_interval } = required!(token_bucket)?;
            let max_tokens = required!(max_tokens).with_node("token_bucket")?;
            let tokens_per_fill = tokens_per_fill.map(|t| t.value).unwrap_or(1);
            if tokens_per_fill == 0 {
                return Err(GenericError::from_msg("tokens per fill can't be zero")
                    .with_node("tokens_per_fill")
                    .with_node("token_bucket"));
            }
            let fill_interval =
                duration_from_envoy(required!(fill_interval)?).with_node("fill_interval").with_node("token_bucket")?;
            Ok(Self { status, max_tokens, tokens_per_fill, fill_interval })
        }
    }
}
