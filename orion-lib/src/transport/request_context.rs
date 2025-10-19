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

use http::Request;
use http_body::Body;
use orion_configuration::config::network_filters::http_connection_manager::RetryPolicy;
use std::time::Duration;

#[derive(Clone, Debug, Default)]
pub struct RequestContext<'a> {
    pub route_timeout: Option<Duration>,
    pub retry_policy: Option<&'a RetryPolicy>,
}

pub struct RequestWithContext<'a, B: Body> {
    pub req: Request<B>,
    pub ctx: RequestContext<'a>,
}

impl<'a, B: Body> RequestWithContext<'a, B> {
    pub fn new(req: Request<B>) -> Self {
        RequestWithContext { req, ctx: RequestContext::default() }
    }

    pub fn with_context(req: Request<B>, ctx: RequestContext<'a>) -> Self {
        RequestWithContext { req, ctx }
    }
}
