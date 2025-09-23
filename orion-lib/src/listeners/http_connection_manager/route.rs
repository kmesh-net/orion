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
use super::{http_modifiers, upgrades as upgrade_utils, RequestHandler, TransactionHandler};
use crate::event_error::{EventError, EventKind, TryInferFrom};
use crate::{
    body::{body_with_metrics::BodyWithMetrics, body_with_timeout::BodyWithTimeout, response_flags::ResponseFlags},
    clusters::{
        balancers::hash_policy::HashState,
        clusters_manager::{self, RoutingContext},
    },
    listeners::{
        access_log::AccessLogContext, http_connection_manager::HttpConnectionManager,
        synthetic_http_response::SyntheticHttpResponse,
    },
    transport::policy::{RequestContext, RequestExt},
    PolyBody, Result,
};

use http::{uri::Parts as UriParts, Uri};
use hyper::{body::Incoming, Request, Response};
use opentelemetry::trace::Span;
use opentelemetry::KeyValue;
use orion_configuration::config::network_filters::http_connection_manager::{
    route::{RouteAction, RouteMatchResult},
    RetryPolicy,
};
use orion_error::Context;
use orion_format::{
    context::{UpstreamContext, UpstreamRequest},
    types::{ResponseFlagsLong, ResponseFlagsShort},
};
use orion_tracing::attributes::{UPSTREAM_ADDRESS, UPSTREAM_CLUSTER_NAME};
use orion_tracing::http_tracer::{SpanKind, SpanName};
use smol_str::ToSmolStr;
use std::net::SocketAddr;
use tracing::debug;

pub struct MatchedRequest<'a> {
    pub request: Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>,
    pub retry_policy: Option<&'a RetryPolicy>,
    pub route_name: &'a str,
    pub remote_address: SocketAddr,
    pub route_match: RouteMatchResult,
    pub websocket_enabled_by_default: bool,
}

impl<'a> RequestHandler<(MatchedRequest<'a>, &HttpConnectionManager)> for &RouteAction {
    #[allow(clippy::too_many_lines)]
    async fn to_response(
        self,
        trans_handler: &TransactionHandler,
        (request, connection_manager): (MatchedRequest<'a>, &HttpConnectionManager),
    ) -> Result<Response<PolyBody>> {
        let MatchedRequest {
            request: downstream_request,
            route_name,
            retry_policy,
            remote_address,
            route_match,
            websocket_enabled_by_default,
        } = request;
        let cluster_id = clusters_manager::resolve_cluster(&self.cluster_specifier)
            .ok_or_else(|| "Failed to resolve cluster from specifier".to_owned())?;
        let routing_requirement = clusters_manager::get_cluster_routing_requirements(cluster_id);
        let hash_state = HashState::new(self.hash_policy.as_slice(), &downstream_request, remote_address);
        let routing_context = RoutingContext::try_from((&routing_requirement, &downstream_request, hash_state))?;
        let maybe_channel = clusters_manager::get_http_connection(cluster_id, routing_context);

        match maybe_channel {
            Ok(svc_channel) => {
                if let Some(ctx) = trans_handler.access_log_ctx.as_ref() {
                    ctx.lock().loggers.with_context(&UpstreamContext {
                        authority: Some(&svc_channel.upstream_authority),
                        cluster_name: Some(svc_channel.cluster_name),
                        route_name,
                    })
                }

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

                    let authority_replacement = if let Some(authority_rewrite) = &self.authority_rewrite {
                        authority_rewrite.apply(&parts.uri, &parts.headers, &svc_channel.upstream_authority)
                    } else {
                        None
                    };

                    if path_and_query_replacement.is_some() || authority_replacement.is_some() {
                        parts.uri = {
                            let UriParts { scheme, authority, path_and_query, .. } = parts.uri.into_parts();
                            let mut new_parts = UriParts::default();
                            new_parts.scheme = scheme;
                            new_parts.authority = authority_replacement.clone().or(authority);
                            new_parts.path_and_query = path_and_query_replacement.or(path_and_query);
                            Uri::from_parts(new_parts).with_context_msg("failed to replace request URI")?
                        }
                    }

                    if let Some(new_authority) = &authority_replacement {
                        let header_value = http::HeaderValue::from_str(new_authority.as_str())
                            .with_context_msg("failed to create Host header value")?;
                        parts.headers.insert(http::header::HOST, header_value);
                    }

                    Request::from_parts(parts, body.map_into())
                };

                let mut client_span = connection_manager.http_tracer.try_create_span(
                    trans_handler.trace_ctx.as_ref(),
                    &connection_manager.get_tracing_key(),
                    SpanKind::Client,
                    SpanName::Str::<()>(svc_channel.upstream_authority.as_str()),
                );

                if let Some(ref mut client_span) = client_span {
                    // set default attributes to span, using upstream request information...
                    connection_manager.http_tracer.set_attributes_from_request(client_span, &upstream_request);

                    // set additional attributes for client span...
                    client_span.set_attributes([
                        KeyValue::new(UPSTREAM_CLUSTER_NAME, svc_channel.cluster_name),
                        KeyValue::new(UPSTREAM_ADDRESS, svc_channel.upstream_authority.to_string()),
                    ]);
                }

                // ... store the span in the span_state
                if let Some(ref span_state) = trans_handler.span_state {
                    *span_state.client_span.lock() = client_span;
                }

                if let Some(ctx) = trans_handler.access_log_ctx.as_ref() {
                    ctx.lock().loggers.with_context(&UpstreamRequest(&upstream_request));
                }

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
                            return Ok(SyntheticHttpResponse::bad_request(EventKind::UpgradeFailed).into_response(ver));
                        },
                    }
                } else {
                    false
                };
                if should_upgrade_websocket {
                    return upgrade_utils::handle_websocket_upgrade(trans_handler, upstream_request, &svc_channel)
                        .await;
                }
                if let Some(direct_response) = http_modifiers::apply_preflight_functions(&mut upstream_request) {
                    return Ok(direct_response);
                }

                // send the request to the upstream service channel and wait for the response...
                let resp = svc_channel
                    .to_response(
                        trans_handler,
                        RequestExt::with_context(
                            RequestContext { route_timeout: self.timeout, retry_policy },
                            upstream_request,
                        ),
                    )
                    .await;
                match resp {
                    Err(err) => {
                        let err = err.into_inner();
                        let event_error = EventError::try_infer_from(&err);
                        let flags = event_error.clone().map(ResponseFlags::from).unwrap_or_default();
                        let event_kind = event_error.map_or(EventKind::ViaUpstream, |e| EventKind::Error(e));
                        debug!(
                            "HttpConnectionManager Error processing response {:?}: {}({})",
                            err,
                            ResponseFlagsLong(&flags.0).to_smolstr(),
                            ResponseFlagsShort(&flags.0).to_smolstr()
                        );
                        Ok(SyntheticHttpResponse::bad_gateway(event_kind, flags).into_response(ver))
                    },
                    Ok(resp) => Ok(resp),
                }
            },
            // http connection not avaiable from cluster...
            Err(err) => {
                let err = err.into_inner();
                let event_error = EventError::try_infer_from(&err);
                let flags = event_error.clone().map(ResponseFlags::from).unwrap_or_default();
                let event_kind = event_error.map_or(EventKind::ViaUpstream, |e| EventKind::Error(e));
                debug!(
                    "Failed to get an HTTP connection: {:?}: {}({})",
                    err,
                    ResponseFlagsLong(&flags.0).to_smolstr(),
                    ResponseFlagsShort(&flags.0).to_smolstr()
                );
                Ok(SyntheticHttpResponse::internal_error(event_kind, flags).into_response(downstream_request.version()))
            },
        }
    }
}
