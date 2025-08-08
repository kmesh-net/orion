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

mod direct_response;
mod redirect;
mod route;
use route::MatchedRequest;

use crate::{
    body::timeout_body::TimeoutBody,
    listeners::{rate_limiter::LocalRateLimit, synthetic_http_response::SyntheticHttpResponse},
    ConversionContext, HttpBody, PolyBody, Result, RouteConfiguration,
};
use compact_str::{CompactString, ToCompactString};
use core::time::Duration;
use futures::{future::BoxFuture, FutureExt};
use hyper::{body::Incoming, service::Service, Request, Response};
use orion_configuration::config::network_filters::http_connection_manager::{
    http_filters::{http_rbac::HttpRbac, HttpFilter as HttpFilterConfig, HttpFilterType},
    route::Action,
    CodecType, ConfigSource, ConfigSourceSpecifier, HttpConnectionManager as HttpConnectionManagerConfig, RdsSpecifier,
    RouteSpecifier,
};
use std::{
    fmt,
    future::{ready, Future},
    net::SocketAddr,
    result::Result as StdResult,
    sync::Arc,
};
use tokio::sync::watch;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct HttpConnectionManagerBuilder {
    listener_name: Option<CompactString>,
    connection_manager: PartialHttpConnectionManager,
}

impl TryFrom<ConversionContext<'_, HttpConnectionManagerConfig>> for HttpConnectionManagerBuilder {
    type Error = crate::Error;
    fn try_from(ctx: ConversionContext<HttpConnectionManagerConfig>) -> Result<Self> {
        let partial = PartialHttpConnectionManager::try_from(ctx)?;
        Ok(Self { listener_name: None, connection_manager: partial })
    }
}

impl HttpConnectionManagerBuilder {
    pub fn build(self) -> Result<HttpConnectionManager> {
        let name = self.listener_name.ok_or("listener name is not set")?;

        let PartialHttpConnectionManager { router, codec_type, dynamic_route_name, http_filters, request_timeout } =
            self.connection_manager;

        let router_sender = watch::Sender::new(router.map(Arc::new));

        Ok(HttpConnectionManager {
            listener_name: name,
            router_sender,
            codec_type,
            dynamic_route_name,
            http_filters,
            request_timeout,
        })
    }

    pub fn with_listener_name(self, name: CompactString) -> Self {
        HttpConnectionManagerBuilder { listener_name: Some(name), ..self }
    }
}

#[derive(Debug, Clone)]
pub struct PartialHttpConnectionManager {
    router: Option<RouteConfiguration>,
    codec_type: CodecType,
    dynamic_route_name: Option<CompactString>,
    http_filters: Vec<HttpFilter>,
    request_timeout: Option<Duration>,
}

#[derive(Clone, Debug)]
// TODO: Implement HTTP filter chain functionality - this struct defines filters for request processing
// Used for rate limiting, RBAC, and other HTTP middleware features
pub struct HttpFilter {
    pub name: CompactString,
    pub disabled: bool,
    pub filter: HttpFilterValue,
}

#[derive(Clone, Debug)]
// TODO: Implement HTTP filter value types - defines the different types of HTTP filters
// Currently supports rate limiting and RBAC (Role-Based Access Control)
pub enum HttpFilterValue {
    RateLimit(LocalRateLimit),
    Rbac(HttpRbac),
}

impl From<HttpFilterConfig> for HttpFilter {
    fn from(value: HttpFilterConfig) -> Self {
        let HttpFilterConfig { name, disabled, filter } = value;
        let filter = match filter {
            HttpFilterType::RateLimit(r) => HttpFilterValue::RateLimit(r.into()),
            HttpFilterType::Rbac(rbac) => HttpFilterValue::Rbac(rbac),
        };
        Self { name, disabled, filter }
    }
}

impl HttpFilterValue {
    // TODO: Implement HTTP filter application logic - this method applies filters to incoming requests
    // Should return Some(Response) to short-circuit request processing, or None to continue
    pub fn apply(&self, request: &Request<Incoming>) -> Option<Response<HttpBody>> {
        match self {
            HttpFilterValue::Rbac(rbac) => apply_authorization_rules(rbac, request),
            HttpFilterValue::RateLimit(rl) => rl.run(request),
        }
    }
}

impl TryFrom<ConversionContext<'_, HttpConnectionManagerConfig>> for PartialHttpConnectionManager {
    type Error = crate::Error;
    fn try_from(ctx: ConversionContext<HttpConnectionManagerConfig>) -> Result<Self> {
        let ConversionContext { envoy_object: configuration, secret_manager: _ } = ctx;
        let codec_type = configuration.codec_type;

        let http_filters = configuration.http_filters.into_iter().map(HttpFilter::from).collect();
        let request_timeout = configuration.request_timeout;

        let (dynamic_route_name, router) = match configuration.route_specifier {
            RouteSpecifier::Rds(RdsSpecifier {
                route_config_name,
                config_source: ConfigSource { config_source_specifier },
            }) => match config_source_specifier {
                ConfigSourceSpecifier::ADS => (Some(route_config_name.to_compact_string()), None),
            },
            RouteSpecifier::RouteConfig(config) => (None, Some(config)),
        };

        Ok(PartialHttpConnectionManager { router, codec_type, dynamic_route_name, http_filters, request_timeout })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum AlpnCodecs {
    Http1,
    Http2,
}

impl AsRef<[u8]> for AlpnCodecs {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Http2 => b"h2",
            Self::Http1 => b"http/1.1",
        }
    }
}

impl AlpnCodecs {
    pub fn from_codec(codec: CodecType) -> &'static [Self] {
        match codec {
            CodecType::Auto => &[AlpnCodecs::Http2, AlpnCodecs::Http1],
            CodecType::Http2 => &[AlpnCodecs::Http2],
            CodecType::Http1 => &[AlpnCodecs::Http1],
        }
    }
}

#[derive(Debug)]
// TODO: Implement HTTP connection management - this struct manages HTTP connections and routing
// Contains filter chain, routing configuration, and connection settings
pub struct HttpConnectionManager {
    pub listener_name: CompactString,
    router_sender: watch::Sender<Option<Arc<RouteConfiguration>>>,
    pub codec_type: CodecType,
    dynamic_route_name: Option<CompactString>,
    http_filters: Vec<HttpFilter>,
    request_timeout: Option<Duration>,
}

impl fmt::Display for HttpConnectionManager {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "HttpConnectionManager {}", &self.listener_name,)
    }
}

impl HttpConnectionManager {
    pub fn get_route_id(&self) -> &Option<CompactString> {
        &self.dynamic_route_name
    }

    pub fn update_route(&self, route: Arc<RouteConfiguration>) {
        let _ = self.router_sender.send_replace(Some(route));
    }

    pub fn remove_route(&self) {
        let _ = self.router_sender.send_replace(None);
    }

    pub(crate) fn request_handler(self: &Arc<Self>) -> HttpRequestHandler {
        HttpRequestHandler { manager: Arc::clone(self), router: self.router_sender.subscribe() }
    }
}

pub(crate) struct HttpRequestHandler {
    manager: Arc<HttpConnectionManager>,
    router: watch::Receiver<Option<Arc<RouteConfiguration>>>,
}

pub struct HttpHandlerRequest {
    pub request: Request<Incoming>,
    pub source_addr: SocketAddr,
}

// has to be a trait due to foreign impl rules.
pub trait RequestHandler<R> {
    fn to_response(self, request: R) -> impl Future<Output = Result<Response<PolyBody>>> + Send;
}

impl RequestHandler<(Request<TimeoutBody<Incoming>>, SocketAddr)> for Arc<RouteConfiguration> {
    async fn to_response(
        self,
        (request, source_address): (Request<TimeoutBody<Incoming>>, SocketAddr),
    ) -> Result<Response<PolyBody>> {
        // needs some way to resolve the request first, _then_ apply modifications on both request and response from there
        // might just want to implement this whole thing as a hyper service "layer"

        let Some(chosen_vh) = self
            .virtual_hosts
            .iter()
            .max_by_key(|vh| vh.domains.iter().map(|domain| domain.eval_lpm_request(&request)).max())
        else {
            return Ok(SyntheticHttpResponse::not_found().into_response(request.version()));
        };
        let Some((chosen_route, route_match_result)) = chosen_vh
            .routes
            .iter()
            .map(|route| (route, route.route_match.match_request(&request)))
            .find(|(_, match_result)| match_result.matched())
        else {
            return Ok(SyntheticHttpResponse::not_found().into_response(request.version()));
        };

        //todo(hayley)
        // if let Some(response) = apply_filters(http_filters, &chosen_route.typed_per_filter_config, &request) {
        //     return Ok(response);
        // }
        let mut response = match &chosen_route.action {
            Action::DirectResponse(dr) => dr.to_response(request).await,
            Action::Redirect(rd) => rd.to_response((request, route_match_result)).await,
            Action::Route(route) => {
                route
                    .to_response(MatchedRequest {
                        request,
                        virtual_host: chosen_vh,
                        route_match: route_match_result,
                        source_address,
                    })
                    .await
            },
        }?;
        let resp_headers = response.headers_mut();
        if self.most_specific_header_mutations_wins {
            self.response_header_modifier.modify(resp_headers);
            chosen_vh.response_header_modifier.modify(resp_headers);
            chosen_route.response_header_modifier.modify(resp_headers);
        } else {
            chosen_route.response_header_modifier.modify(resp_headers);
            chosen_vh.response_header_modifier.modify(resp_headers);
            self.response_header_modifier.modify(resp_headers);
        }
        Ok(response)
    }
}

impl Service<HttpHandlerRequest> for HttpRequestHandler {
    type Response = Response<HttpBody>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, StdResult<Self::Response, Self::Error>>;

    fn call(&self, request: HttpHandlerRequest) -> Self::Future {
        let HttpHandlerRequest { request, source_addr } = request;

        let (parts, body) = request.into_parts();
        // optionally apply a timeout to the body.
        // envoy says this timeout is started when the request is initiated. This is relatively vague, but because at this point we will
        // already have the headers, it seems like a fair start.
        //  note that we can stil time-out a request due to e.g. the filters taking a long time to compute, or the proxy being overwhelmed
        // not just due to the downstream being slow.
        // todo(hayley): this timeout is incorrect (checks for time between frames not total time), and doesn't seem to get converted into
        //  http response
        let request = Request::from_parts(parts, TimeoutBody::new(self.manager.request_timeout, body));

        // if let Some(response) = self.apply_filters(&request) {
        //     return ready(Ok(response)).boxed();
        // }

        let Some(route_conf) = self.router.borrow().clone() else {
            return ready(Ok(SyntheticHttpResponse::not_found().into_response(request.version()))).boxed();
        };

        route_conf.to_response((request, source_addr)).boxed()
    }
}

// TODO: Implement RBAC (Role-Based Access Control) authorization logic
// This function should check permissions and return forbidden response if access is denied
fn apply_authorization_rules<B>(rbac: &HttpRbac, req: &Request<B>) -> Option<Response<HttpBody>> {
    debug!("Applying authorization rules {rbac:?} {:?}", &req.headers());
    if !rbac.is_permitted(req) {
        Some(SyntheticHttpResponse::forbidden("RBAC: access denied").into_response(req.version()))
    } else {
        None
    }
}
