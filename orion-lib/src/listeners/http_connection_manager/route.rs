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

use super::{RequestHandler, TransactionContext, http_modifiers, upgrades as upgrade_utils};
use crate::{
    PolyBody, Result,
    body::{body_with_metrics::BodyWithMetrics, body_with_timeout::BodyWithTimeout, response_flags::ResponseFlags},
    clusters::{
        balancers::hash_policy::HashState,
        clusters_manager::{self, RoutingContext},
        retry_policy::{EventError, TryInferFrom},
    },
    listeners::{access_log::AccessLogContext, synthetic_http_response::SyntheticHttpResponse},
    transport::policy::{RequestContext, RequestExt},
};
use http::{Uri, uri::Parts as UriParts};
use hyper::{Request, Response, body::Incoming};
use orion_configuration::config::network_filters::http_connection_manager::{
    RetryPolicy,
    route::{RouteAction, RouteMatchResult},
};
use orion_error::Context;
use orion_format::{
    context::{UpstreamContext, UpstreamRequest},
    types::{ResponseFlagsLong, ResponseFlagsShort},
};
use smol_str::ToSmolStr;
use std::net::SocketAddr;
use tracing::debug;

pub struct MatchedRequest<'a> {
    pub request: Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>,
    pub retry_policy: Option<&'a RetryPolicy>,
    pub remote_address: SocketAddr,
    pub route_match: RouteMatchResult,
    pub websocket_enabled_by_default: bool,
}

impl<'a> RequestHandler<MatchedRequest<'a>> for &RouteAction {
    #[allow(clippy::too_many_lines)]
    async fn to_response(
        self,
        trans_ctx: &TransactionContext,
        request: MatchedRequest<'a>,
    ) -> Result<Response<PolyBody>> {
        let MatchedRequest {
            request: downstream_request,
            retry_policy,
            remote_address,
            route_match,
            websocket_enabled_by_default,
        } = request;
        let cluster_id = clusters_manager::resolve_cluster(&self.cluster_specifier)
            .ok_or_else(|| "Failed to resolve cluster from specifier".to_string())?;
        let routing_requirement = clusters_manager::get_cluster_routing_requirements(cluster_id);
        let hash_state = HashState::new(self.hash_policy.as_slice(), &downstream_request, remote_address);
        let routing_context = RoutingContext::try_from((&routing_requirement, &downstream_request, hash_state))?;
        let maybe_channel = clusters_manager::get_http_connection(cluster_id, routing_context);

        match maybe_channel {
            Ok(svc_channel) => {
                trans_ctx.access_log_ctx.as_ref().inspect(|ctx| {
                    ctx.access_loggers.lock().with_context(&UpstreamContext {
                        authority: &svc_channel.upstream_authority,
                        cluster_name: svc_channel.cluster_name,
                    })
                });

                let ver = downstream_request.version();
                let mut upstream_request: Request<BodyWithMetrics<PolyBody>> = {
                    let (mut parts, body) = downstream_request.into_parts();
                    let path_and_query_replacement = if let Some(rewrite) = &self.rewrite {
                        rewrite
                            .apply(parts.uri.path_and_query(), &route_match)
                            .with_context_msg("invalid path after rewrite")?
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
                            Uri::from_parts(new_parts).with_context_msg("failed to replace request path_and_query")?
                        }
                    }
                    Request::from_parts(parts, body.map_into())
                };

                trans_ctx
                    .access_log_ctx
                    .as_ref()
                    .inspect(|ctx| ctx.access_loggers.lock().with_context(&UpstreamRequest(&upstream_request)));

                let websocket_enabled = if let Some(upgrade_config) = self.upgrade_config {
                    upgrade_config.is_websocket_enabled(websocket_enabled_by_default)
                } else {
                    websocket_enabled_by_default
                };
                let should_upgrade_websocket = if websocket_enabled {
                    match upgrade_utils::is_valid_websocket_upgrade_request(upstream_request.headers()) {
                        Ok(maybe_upgrade) => maybe_upgrade,
                        Err(upgrade_error) => {
                            debug!("Failed to upgrade to websockets {upgrade_error}");
                            return Ok(SyntheticHttpResponse::bad_request().into_response(ver));
                        },
                    }
                } else {
                    false
                };
                if should_upgrade_websocket {
                    return upgrade_utils::handle_websocket_upgrade(trans_ctx, upstream_request, &svc_channel).await;
                }
                if let Some(direct_response) = http_modifiers::apply_preflight_functions(&mut upstream_request) {
                    return Ok(direct_response);
                }

                // send the request to the upstream service channel and wait for the response...
                let resp = svc_channel
                    .to_response(
                        trans_ctx,
                        RequestExt::with_context(
                            RequestContext { route_timeout: self.timeout, retry_policy },
                            upstream_request,
                        ),
                    )
                    .await;
                match resp {
                    Err(err) => {
                        let err = err.into_inner();
                        let flags = EventError::try_infer_from(&err).map(ResponseFlags::from).unwrap_or_default();
                        debug!(
                            "HttpConnectionManager Error processing response {:?}: {}({})",
                            err,
                            ResponseFlagsLong(&flags.0).to_smolstr(),
                            ResponseFlagsShort(&flags.0).to_smolstr()
                        );
                        Ok(SyntheticHttpResponse::bad_gateway(flags).into_response(ver))
                    },
                    Ok(resp) => Ok(resp),
                }
            },
            Err(err) => {
                let err = err.into_inner();
                let flags = EventError::try_infer_from(&err).map(ResponseFlags::from).unwrap_or_default();
                debug!(
                    "Failed to get an HTTP connection: {:?}: {}({})",
                    err,
                    ResponseFlagsLong(&flags.0).to_smolstr(),
                    ResponseFlagsShort(&flags.0).to_smolstr()
                );
                Ok(SyntheticHttpResponse::internal_error(flags).into_response(downstream_request.version()))
            },
        }
    }
}
