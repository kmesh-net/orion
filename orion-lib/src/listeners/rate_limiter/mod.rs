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

mod token_bucket;
use http::{Request, status::StatusCode};
use tracing::warn;

use token_bucket::TokenBucket;

use orion_configuration::config::network_filters::http_connection_manager::http_filters::local_rate_limit::LocalRateLimit as LocalRateLimitConfig;

use crate::{body::response_flags::ResponseFlags, event_error::EventKind};
use orion_format::types::ResponseFlags as FmtResponseFlags;

use crate::{
    listeners::{http_connection_manager::FilterDecision, synthetic_http_response::SyntheticHttpResponse},
    runtime_config,
};

#[derive(Debug, Clone)]
pub struct LocalRateLimit {
    pub status: StatusCode,
    pub token_bucket: Option<TokenBucket>,
}

impl LocalRateLimit {
    pub fn run<B>(&self, req: &Request<B>) -> FilterDecision {
        if let Some(token_bucket) = &self.token_bucket {
            if !token_bucket.consume(1) {
                let status = self.status;
                return FilterDecision::DirectResponse(
                    SyntheticHttpResponse::custom_error(
                        status,
                        EventKind::RateLimited,
                        ResponseFlags(FmtResponseFlags::RATE_LIMITED),
                    )
                    .into_response(req.version()),
                );
            }
        }
        FilterDecision::Continue
    }
}

impl From<LocalRateLimitConfig> for LocalRateLimit {
    fn from(rate_limit: LocalRateLimitConfig) -> Self {
        let status = rate_limit.status;
        if let Some(token_bucket) = rate_limit.token_bucket {
            let max_tokens = token_bucket.max_tokens;
            let tokens_per_fill = token_bucket.tokens_per_fill;
            let fill_interval = token_bucket.fill_interval;
            let adjusted_fill_interval = fill_interval.checked_mul(runtime_config().num_runtimes.into());
            let fill_interval = if let Some(value) = adjusted_fill_interval {
                value
            } else {
                warn!("failed to adjust fill interval to number of configured runtimes (overflow)");
                fill_interval
            };
            let tb = TokenBucket::new(max_tokens, tokens_per_fill, fill_interval);
            return Self { status, token_bucket: Some(tb) };
        }
        Self { status, token_bucket: None }
    }
}
