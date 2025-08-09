// SPDX-FileCopyrightText: © 2025 Huawei Cloud Computing Technologies Co., Ltd
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 Huawei Cloud Computing Technologies Co., Ltd
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

use std::sync::Arc;

use http::status::StatusCode;
use http::Request;
use hyper::Response;

use token_bucket::TokenBucket;

use orion_configuration::config::network_filters::http_connection_manager::http_filters::local_rate_limit::LocalRateLimit as LocalRateLimitConfig;

use crate::listeners::synthetic_http_response::SyntheticHttpResponse;
use crate::{runtime_config, HttpBody};

#[derive(Debug, Clone)]
// TODO: Implement rate limiting functionality - this struct defines the interface for local rate limits
pub struct LocalRateLimit {
    pub status: StatusCode,
    pub token_bucket: Arc<TokenBucket>,
}

impl LocalRateLimit {
    // TODO: Implement rate limit enforcement - used to check and consume tokens for incoming requests
    pub fn run<B>(&self, req: &Request<B>) -> Option<Response<HttpBody>> {
        if !self.token_bucket.consume(1) {
            let status = self.status;
            return Some(SyntheticHttpResponse::custom_error(status).into_response(req.version()));
        }
        None
    }
}

impl From<LocalRateLimitConfig> for LocalRateLimit {
    fn from(rate_limit: LocalRateLimitConfig) -> Self {
        let status = rate_limit.status;
        let max_tokens = rate_limit.max_tokens;
        let tokens_per_fill = rate_limit.tokens_per_fill;
        let fill_interval = rate_limit.fill_interval;
        let token_bucket = Arc::new(TokenBucket::new(
            max_tokens,
            tokens_per_fill,
            fill_interval.checked_mul(runtime_config().num_runtimes.into()).expect("too many runtimes (overflow)"),
        ));

        Self { status, token_bucket }
    }
}
