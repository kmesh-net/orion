use crate::{
    body::{body_with_timeout::BodyWithTimeout, response_flags::BodyKind},
    mcp,
    transport::HttpChannel,
};
use ahash::{HashMap, HashMapExt};
use http::{Request, Response, Uri, request::Parts, uri::Parts as UriParts};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use opentelemetry::{KeyValue, trace::Span};
use orion_configuration::config::network_filters::http_connection_manager::{
    RetryPolicy, mcp_route::MCPRouteAction, route::RouteMatchResult,
};
use orion_error::Context;
use orion_format::{
    context::{UpstreamContext, UpstreamRequest},
    types::{ResponseFlagsLong, ResponseFlagsShort},
};
use orion_tracing::{
    attributes::{UPSTREAM_ADDRESS, UPSTREAM_CLUSTER_NAME},
    http_tracer::{SpanKind, SpanName},
};
use smol_str::ToSmolStr;
use tracing::{debug, warn};

use crate::{
    PolyBody, Result,
    body::{body_with_metrics::BodyWithMetrics, response_flags::ResponseFlags},
    clusters::{RoutingContext, balancers::hash_policy::HashState, clusters_manager},
    event_error::{EventError, EventKind, TryInferFrom},
    listeners::{
        access_log::AccessLogContext,
        http_connection_manager::{
            HttpConnectionManager, RequestHandler, TransactionHandler, http_modifiers, route::MatchedRequest,
        },
        synthetic_http_response::SyntheticHttpResponse,
    },
    transport::policy::{RequestContext, RequestExt},
};

impl<'a> RequestHandler<(MatchedRequest<'a>, &HttpConnectionManager)> for &MCPRouteAction {
    #[allow(clippy::too_many_lines)]
    async fn to_response(
        self,
        trans_handler: &TransactionHandler,
        (request, connection_manager): (MatchedRequest<'a>, &HttpConnectionManager),
    ) -> Result<Response<PolyBody>> {
        let MatchedRequest {
            request: downstream_request,
            route_name,
            retry_policy: _,
            remote_address,
            route_match,
            websocket_enabled_by_default: _,
        } = request;

        let channels = self
            .cluster_mappings
            .iter()
            .map(|(name, cluster_specifier)| {
                if let Some(authority) = downstream_request.uri().authority()
                    && cluster_specifier.name() == authority.host()
                {
                    if let Some(cluster_id) = clusters_manager::resolve_cluster(cluster_specifier) {
                        let routing_requirement = clusters_manager::get_cluster_routing_requirements(cluster_id);
                        let hash_state = HashState::new(&[], &downstream_request, remote_address);
                        if let Ok(routing_context) =
                            RoutingContext::try_from((&routing_requirement, &downstream_request, hash_state))
                        {
                            (name, clusters_manager::get_http_connection(cluster_id, routing_context))
                        } else {
                            (name, Err(orion_error::Error::from("Failed to create routing context".to_owned())))
                        }
                    } else {
                        (name, Err(orion_error::Error::from("Failed to resolve cluster from specifier".to_owned())))
                    }
                } else {
                    (
                        name,
                        Err(orion_error::Error::from(format!(
                            "Failed to find cluster for {:?}",
                            downstream_request.uri().authority()
                        ))
                        .into()),
                    )
                }
            })
            .collect::<HashMap<_, _>>();

        channels.iter().for_each(|(k, v)| {
            if v.is_err() {
                warn!("Problem when processing {k} {v:?} ")
            }
        });

        if channels.iter().any(|(_, v)| v.is_err()) {
            return Err(orion_error::Error::from("Problem when processing channels".to_owned()));
        }

        let (p, b) = downstream_request.into_parts();

        let data = b.inner.collect().await?.to_bytes();

        let mut channel_request_map = HashMap::new();
        for (k, v) in channels.into_iter() {
            let channel = v.unwrap();
            match process_channel(
                self,
                (p.clone(), BodyWithMetrics::new(BodyKind::Request, PolyBody::from(data.clone()), |_, _, _| {})),
                &channel,
                route_name,
                &route_match,
                trans_handler,
            ) {
                Ok(new_request) => {
                    channel_request_map.insert(k.clone(), (channel, new_request.uri().clone()));
                },
                Err(e) => return Err(e),
            }
        }
        let original_request = Request::from_parts(
            p.clone(),
            BodyWithMetrics::new(BodyKind::Request, PolyBody::from(data.clone()), |_, _, _| {}),
        );
        let app = mcp::App::new();
        Ok(app.serve(original_request, channel_request_map).await)
    }
}

fn process_channel<'a, B>(
    mcp_route: &MCPRouteAction,
    downstream_request: (Parts, B),
    svc_channel: &HttpChannel,
    route_name: &str,
    route_match: &RouteMatchResult,
    trans_handler: &TransactionHandler,
) -> Result<Request<B>> {
    if let Some(ctx) = trans_handler.access_log_ctx.as_ref() {
        ctx.lock().loggers.with_context(&UpstreamContext {
            authority: Some(&svc_channel.upstream_authority),
            cluster_name: Some(svc_channel.cluster_name),
            route_name,
        })
    }

    let (mut parts, body) = downstream_request;
    let ver = parts.version;

    let upstream_request = {
        let path_and_query_replacement = if let Some(rewrite) = &mcp_route.rewrite {
            rewrite.apply(parts.uri.path_and_query(), &route_match)?
        } else {
            None
        };

        let authority_replacement = if let Some(authority_rewrite) = &mcp_route.authority_rewrite {
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
            let header_value = http::HeaderValue::from_str(new_authority.as_str())?;
            parts.headers.insert(http::header::HOST, header_value);
        }

        Request::from_parts(parts, body)
    };
    Ok(upstream_request)
}
