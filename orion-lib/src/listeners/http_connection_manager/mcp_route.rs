use std::net::SocketAddr;

use crate::{
    body::{body_with_timeout::BodyWithTimeout, response_flags::BodyKind},
    listeners::http_connection_manager::{self, mcp_route},
    mcp::{self, router::McpBackend},
    transport::HttpChannel,
};
use ahash::{HashMap, HashMapExt};
use http::{
    Request, Response, Uri,
    request::Parts,
    uri::{Authority, Parts as UriParts},
};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use orion_configuration::config::network_filters::http_connection_manager::{
    mcp_route::{McpBackendType, McpRouteAction, McpStreamableHttpParams},
    route::RouteMatchResult,
};
use orion_data_plane_api::envoy_data_plane_api::envoy::{
    extensions::{
        filters::http::adaptive_concurrency::v3::gradient_controller_config::MinimumRttCalculationParams,
        router::cluster_specifiers,
    },
    service::ext_proc::v3::BodyMutation,
};
use orion_error::Context;
use orion_format::context::UpstreamContext;

use tracing::{debug, warn};

use crate::{
    PolyBody, Result,
    body::body_with_metrics::BodyWithMetrics,
    clusters::{RoutingContext, balancers::hash_policy::HashState, clusters_manager},
    listeners::{
        access_log::AccessLogContext,
        http_connection_manager::{HttpConnectionManager, RequestHandler, TransactionHandler, route::MatchedRequest},
    },
};

impl<'a> RequestHandler<(MatchedRequest<'a>, &HttpConnectionManager)> for &McpRouteAction {
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

        // let downstream_authority = if let Some(authority) = downstream_request.uri().clone().authority() {
        //     authority.clone()
        // } else if let Some(host_header) = downstream_request.headers().get(http::header::HOST)
        //     && let Ok(host) = host_header.to_str()
        // {
        //     host.parse::<Authority>()?
        // } else {
        //     return Err(orion_error::Error::from(
        //         "Problem when processing channels can't find authority from uri nor headers".to_owned(),
        //     ));
        // };

        let (parts, b) = downstream_request.into_parts();
        let data = b.inner.collect().await?.to_bytes();
        let mut backends = self.backend_mappings.iter().map(|(name, backend_type)| {
            (
                name,
                match backend_type {
                    McpBackendType::StdioBackend(params) => Ok(McpBackend::Stdio {
                        cmd: params.cmd.clone(),
                        envs: params.env.clone(),
                        args: params.args.clone(),
                    }),
                    McpBackendType::StreamableHttpBackend(item) => process_streamable_http_channel(
                        item,
                        &parts,
                        route_name,
                        &route_match,
                        remote_address,
                        &self,
                        trans_handler,
                    ),
                },
            )
        });

        if backends.find(|(_, backend_type)| backend_type.is_err()).is_some() {
            return Err(orion_error::Error::from(
                "Problem when processing channels can't find authority from uri nor headers".to_owned(),
            ));
        }
        let backends = backends
            .into_iter()
            .filter_map(
                |(name, backend_type)| {
                    if let Ok(backend_type) = backend_type { Some((name.clone(), backend_type)) } else { None }
                },
            )
            .collect::<HashMap<_, _>>();

        let original_request = Request::from_parts(
            parts,
            BodyWithMetrics::new(BodyKind::Request, PolyBody::from(data.clone()), |_, _, _| {}),
        );
        let app = mcp::App::new_with_session_manager(connection_manager.mcp_session_manager.clone());
        Ok(app.serve(original_request, backends).await)
    }
}

fn process_streamable_http_channel(
    params: &McpStreamableHttpParams,
    downstream_request_parts: &Parts,
    route_name: &str,
    route_match: &RouteMatchResult,
    remote_address: SocketAddr,
    route_action: &McpRouteAction,
    trans_handler: &TransactionHandler,
) -> Result<McpBackend> {
    let McpStreamableHttpParams { cluster_specifier } = params;
    let cluster_id = clusters_manager::resolve_cluster(cluster_specifier)
        .ok_or(orion_error::Error::from("Can't find cluster".to_owned()))?;
    let routing_requirement = clusters_manager::get_cluster_routing_requirements(cluster_id);
    let hash_state = HashState::new(&[], &downstream_request_parts, remote_address);
    let routing_context = RoutingContext::try_from((&routing_requirement, downstream_request_parts, hash_state))?;
    let http_channel = clusters_manager::get_http_connection(cluster_id, routing_context)?;

    let parts = update_uri(
        route_action,
        downstream_request_parts.clone(),
        &http_channel,
        route_name,
        route_match,
        trans_handler,
    )?;

    Ok(McpBackend::StreamableHttp { http_channel, uri: parts.uri })
}

fn update_uri<'a>(
    mcp_route: &McpRouteAction,
    mut request_parts: Parts,
    svc_channel: &HttpChannel,
    route_name: &str,
    route_match: &RouteMatchResult,
    trans_handler: &TransactionHandler,
) -> Result<Parts> {
    if let Some(ctx) = trans_handler.access_log_ctx.as_ref() {
        ctx.lock().loggers.with_context(&UpstreamContext {
            authority: Some(&svc_channel.upstream_authority),
            cluster_name: Some(svc_channel.cluster_name),
            route_name,
        })
    }

    let path_and_query_replacement = if let Some(rewrite) = &mcp_route.rewrite {
        rewrite.apply(request_parts.uri.path_and_query(), route_match)?
    } else {
        None
    };

    let authority_replacement = if let Some(authority_rewrite) = &mcp_route.authority_rewrite {
        authority_rewrite.apply(&request_parts.uri, &request_parts.headers, &svc_channel.upstream_authority)
    } else {
        None
    };

    if path_and_query_replacement.is_some() || authority_replacement.is_some() {
        let UriParts { scheme, authority, path_and_query, .. } = request_parts.uri.into_parts();
        let mut new_parts = UriParts::default();
        new_parts.scheme = scheme;
        new_parts.authority = authority_replacement.clone().or(authority);
        new_parts.path_and_query = path_and_query_replacement.or(path_and_query);
        request_parts.uri = Uri::from_parts(new_parts).with_context_msg("failed to replace request URI")?;
    }

    if let Some(new_authority) = &authority_replacement {
        let header_value = http::HeaderValue::from_str(new_authority.as_str())?;
        request_parts.headers.insert(http::header::HOST, header_value);
    }

    Ok(request_parts)
}
