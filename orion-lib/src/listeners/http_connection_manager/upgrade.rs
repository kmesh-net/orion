use super::RequestHandler;
use crate::{
    body::timeout_body::TimeoutBody,
    clusters::{balancers::hash_policy::HashState, clusters_manager},
    listeners::synthetic_http_response::SyntheticHttpResponse,
    transport::request_context::RequestWithContext,
    PolyBody, Result,
};
use http::{uri::Parts as UriParts, StatusCode, Uri, Version};
use hyper::{body::Incoming, Request, Response};
use hyper_util::rt::TokioIo;
use orion_configuration::config::network_filters::http_connection_manager::route::{
    RouteMatchResult, UpgradeAction, UpgradeActionType,
};
use orion_error::Context;
use std::net::SocketAddr;
use tokio::io::copy_bidirectional;
use tracing::{debug, error};

pub struct UpgradeRequestContext {
    pub request: Request<TimeoutBody<Incoming>>,
    pub route_match: RouteMatchResult,
    pub source_address: SocketAddr,
}

impl RequestHandler<UpgradeRequestContext> for &UpgradeAction {
    async fn to_response(self, request: UpgradeRequestContext) -> Result<Response<PolyBody>> {
        let UpgradeRequestContext { request, route_match, source_address } = request;
        let hash_state = HashState::new(&[], &request, source_address);
        let maybe_channel = clusters_manager::get_http_connection(&self.cluster_specifier, Some(hash_state));

        match maybe_channel {
            Ok(svc_channel) => {
                let version = request.version();
                let mut request: Request<PolyBody> = {
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
                match (self.upgrade_type.clone(), version) {
                    (UpgradeActionType::Websocket, Version::HTTP_11) => {
                        let request_upgrade = hyper::upgrade::on(&mut request);
                        match svc_channel.to_response(RequestWithContext::new(request)).await {
                            Ok(mut upstream_response)
                                if upstream_response.status() == StatusCode::SWITCHING_PROTOCOLS =>
                            {
                                let response_upgrade = hyper::upgrade::on(&mut upstream_response);
                                tokio::spawn(async move {
                                    match (request_upgrade.await, response_upgrade.await) {
                                        (Ok(request_upgraded), Ok(response_upgraded)) => {
                                            let _ = copy_bidirectional(
                                                &mut TokioIo::new(request_upgraded),
                                                &mut TokioIo::new(response_upgraded),
                                            )
                                            .await
                                            .map_err(|err| {
                                                error!(
                                                    "HttpConnectionManager bidi copy failed for websocket {:?}",
                                                    err
                                                );
                                                err
                                            });
                                        },
                                        (req_state, resp_state) => {
                                            error!("HttpConnectionManager problem occurred during connection upgrade {:?},{:?}", req_state, resp_state);
                                        },
                                    }
                                });
                                Ok(upstream_response)
                            },
                            Ok(response) => {
                                error!("HttpConnectionManager upstream did not accept websocket upgrade, returned status code {:?}", response.status());
                                Ok(SyntheticHttpResponse::not_allowed().into_response(version))
                            },
                            Err(err) => {
                                error!(
                                    "HttpConnectionManager failed attempting to establish upstream websocket {:?}",
                                    err
                                );
                                Ok(SyntheticHttpResponse::bad_gateway().into_response(version))
                            },
                        }
                    },
                    (UpgradeActionType::Websocket, _) => {
                        Ok(SyntheticHttpResponse::bad_request().into_response(version))
                    },
                    (UpgradeActionType::Connect { allow_post: _ }, _) => {
                        Ok(SyntheticHttpResponse::upgrade_required().into_response(version))
                    },
                }
            },

            Err(err) => {
                debug!("Failed to get an HTTP connection for upgrade: {:?}", err);
                Ok(SyntheticHttpResponse::internal_error().into_response(request.version()))
            },
        }
    }
}
