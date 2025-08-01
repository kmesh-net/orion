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

use super::RequestHandler;
use crate::{body::timeout_body::TimeoutBody, PolyBody, Result};

use http_body_util::Full;
use hyper::{body::Incoming, Request, Response};
use orion_configuration::config::network_filters::http_connection_manager::route::DirectResponseAction;

impl RequestHandler<Request<TimeoutBody<Incoming>>> for &DirectResponseAction {
    async fn to_response(self, request: Request<TimeoutBody<Incoming>>) -> Result<Response<PolyBody>> {
        let body = Full::new(self.body.as_ref().map(|b| bytes::Bytes::copy_from_slice(b.data())).unwrap_or_default());
        let mut resp = Response::new(body.into());
        *resp.status_mut() = self.status;
        *resp.version_mut() = request.version();
        Ok(resp)
    }
}
