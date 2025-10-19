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
use super::{RequestHandler, TransactionHandler};
use crate::{
    body::{body_with_metrics::BodyWithMetrics, body_with_timeout::BodyWithTimeout},
    PolyBody, Result,
};
use http_body_util::Full;
use hyper::{body::Incoming, Request, Response};
use orion_configuration::config::network_filters::http_connection_manager::route::DirectResponseAction;

use orion_format::context::UpstreamContext;

use crate::listeners::access_log::AccessLogContext;

impl<'a> RequestHandler<(Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>, &'a str)> for &DirectResponseAction {
    async fn to_response(
        self,
        trans_handler: &TransactionHandler,
        (request, route_name): (Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>, &'a str),
    ) -> Result<Response<PolyBody>> {
        if let Some(ctx) = trans_handler.access_log_ctx.as_ref() {
            ctx.lock().loggers.with_context(&UpstreamContext { authority: None, cluster_name: None, route_name })
        }
        let body = Full::new(self.body.as_ref().map(|b| bytes::Bytes::copy_from_slice(b.data())).unwrap_or_default());
        let mut resp = Response::new(body.into());
        *resp.status_mut() = self.status;
        *resp.version_mut() = request.version();
        Ok(resp)
    }
}
