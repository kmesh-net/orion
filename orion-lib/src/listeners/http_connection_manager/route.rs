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

use super::RequestHandler;
use crate::{
    body::timeout_body::TimeoutBody,
    clusters::{balancers::hash_policy::HashState, clusters_manager},
    listeners::synthetic_http_response::SyntheticHttpResponse,
    transport::request_context::{RequestContext, RequestWithContext},
    PolyBody, Result,
};
use http::{uri::Parts as UriParts, Uri};
use hyper::{body::Incoming, Request, Response};
use orion_configuration::config::network_filters::http_connection_manager::{
    route::{RouteAction, RouteMatchResult},
    VirtualHost,
};
use orion_error::Context;
use std::net::SocketAddr;
use tracing::debug;

pub struct MatchedRequest<'a> {
    pub request: Request<TimeoutBody<Incoming>>,
    pub virtual_host: &'a VirtualHost,
    pub route_match: RouteMatchResult,
    pub source_address: SocketAddr,
}

impl<'a> RequestHandler<MatchedRequest<'a>> for &RouteAction {
    async fn to_response(self, request: MatchedRequest<'a>) -> Result<Response<PolyBody>> {
        let MatchedRequest { request, virtual_host, route_match, source_address } = request;
        let retry_policy = self.retry_policy.as_ref().or(virtual_host.retry_policy.as_ref());
        //todo(hayley): the envoy docs say
        // > The router filter will place the original path before rewrite into the x-envoy-original-path header.

        let hash_state = HashState::new(&self.hash_policy, &request, source_address);
        let maybe_channel = clusters_manager::get_http_connection(&self.cluster_specifier, hash_state);
        match maybe_channel {
            Ok(svc_channel) => {
                let ver = request.version();
                let request: Request<PolyBody> = {
                    let (mut parts, body) = request.into_parts();
                    let path_and_query_replacement = if let Some(rewrite) = &self.rewrite {
                        rewrite.apply(parts.uri.path_and_query(), &route_match).context("invalid path after rewrite")?
                    } else {
                        None
                    };
                    if path_and_query_replacement.is_some() {
                        parts.uri = {
                            let UriParts { scheme, authority, path_and_query: _, .. } = parts.uri.into_parts();
                            let mut new_parts = UriParts::default();
                            new_parts.scheme = scheme;
                            new_parts.authority = authority;
                            new_parts.path_and_query = path_and_query_replacement;
                            Uri::from_parts(new_parts).context("failed to replace request path_and_query")?
                        }
                    }
                    Request::from_parts(parts, body.into())
                };
                match svc_channel
                    .to_response(RequestWithContext::with_context(
                        request,
                        RequestContext { route_timeout: self.timeout, retry_policy },
                    ))
                    .await
                {
                    Err(err) => {
                        debug!("HttpConnectionManager Error processing response {:?}", err);
                        Ok(SyntheticHttpResponse::bad_gateway().into_response(ver))
                    },
                    Ok(resp) => Ok(resp),
                }
            },
            Err(err) => {
                debug!("Failed to get an HTTP connection: {:?}", err);
                Ok(SyntheticHttpResponse::internal_error().into_response(request.version()))
            },
        }
    }
}
