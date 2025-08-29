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

use orion_configuration::config::network_filters::http_connection_manager::RetryPolicy;
use std::time::Duration;

#[derive(Clone, Debug, Default)]
pub struct RequestContext<'a> {
    pub route_timeout: Option<Duration>,
    pub retry_policy: Option<&'a RetryPolicy>,
}

pub struct RequestExt<'a, R> {
    pub req: R,
    pub ctx: RequestContext<'a>,
}

impl<'a, R> RequestExt<'a, R> {
    pub fn new(req: R) -> Self {
        RequestExt { req, ctx: RequestContext::default() }
    }

    pub fn with_context(ctx: RequestContext<'a>, req: R) -> Self {
        RequestExt { req, ctx }
    }
}
