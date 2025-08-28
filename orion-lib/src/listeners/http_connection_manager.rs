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

// This is to remove linter warning on HashMap<RouteMatch, Vec<HttpFilter>>
// which is a false positive since the Hasher of RouteMatch does not use mutable
// keys for the string/pattern matchers Regex field.
//
// This false positive is a known issue in the clippy linter:
// https://rust-lang.github.io/rust-clippy/master/index.html#mutable_key_type
#![allow(clippy::mutable_key_type)]

mod direct_response;
mod http_modifiers;
mod redirect;
mod route;
mod upgrades;

use arc_swap::ArcSwap;
use compact_str::{CompactString, ToCompactString};
use core::time::Duration;
use futures::future::BoxFuture;
use hyper::{Request, Response, body::Incoming, service::Service};
use opentelemetry::KeyValue;

use orion_configuration::config::{
    GenericError,
    network_filters::{
        access_log::AccessLog,
        http_connection_manager::{
            CodecType, ConfigSource, ConfigSourceSpecifier, HttpConnectionManager as HttpConnectionManagerConfig,
            RdsSpecifier, Route, RouteSpecifier, UpgradeType, VirtualHost, XffSettings,
            http_filters::{
                FilterConfigOverride, FilterOverride, HttpFilter as HttpFilterConfig, HttpFilterType,
                http_rbac::HttpRbac,
            },
            route::{Action, RouteMatch, RouteMatchResult},
        },
        tracing::Tracing,
    },
};
use orion_format::context::{
    DownstreamResponse, FinishContext, HttpRequestDuration, HttpResponseDuration, InitHttpContext,
};

use orion_format::LogFormatterLocal;
use orion_metrics::{metrics::http, with_metric};
use parking_lot::Mutex;
use route::MatchedRequest;
use scopeguard::defer;
use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
    result::Result as StdResult,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::ThreadId,
    time::Instant,
};
use tokio::sync::{mpsc::Permit, watch};
use tracing::debug;
use upgrades as upgrade_utils;

use crate::{
    access_log::{AccessLogMessage, Target, is_access_log_enabled, log_access, log_access_reserve_balanced},
    body::{
        body_with_metrics::BodyWithMetrics,
        response_flags::{BodyKind, ResponseFlags},
    },
};

use crate::{
    ConversionContext, PolyBody, Result, RouteConfiguration,
    body::body_with_timeout::BodyWithTimeout,
    listeners::{
        access_log::AccessLogContext, filter_state::DownstreamConnectionMetadata, rate_limiter::LocalRateLimit,
        synthetic_http_response::SyntheticHttpResponse,
    },
    trace::{
        request_id::{RequestId, RequestIdManager},
        tracer::{TraceContext, Tracer},
    },
    utils::http::{request_head_size, response_head_size},
};

#[derive(Debug, Clone)]
pub struct HttpConnectionManagerBuilder {
    listener_name: Option<&'static str>,
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

        let PartialHttpConnectionManager {
            router,
            codec_type,
            dynamic_route_name,
            http_filters_hcm,
            http_filters_per_route,
            request_timeout,
            enabled_upgrades,
            access_log,
            xff_settings,
            generate_request_id,
            preserve_external_request_id,
            always_set_request_id_in_response,
            tracing,
        } = self.connection_manager;

        let router_sender = watch::Sender::new(router.map(Arc::new));

        Ok(HttpConnectionManager {
            listener_name: name,
            router_sender,
            codec_type,
            dynamic_route_name,
            http_filters_hcm,
            http_filters_per_route: ArcSwap::new(Arc::new(http_filters_per_route)),
            enabled_upgrades,
            request_timeout,
            access_log,
            xff_settings,
            request_id_handler: RequestIdManager::new(
                generate_request_id,
                preserve_external_request_id,
                always_set_request_id_in_response,
            ),
            tracer: tracing.map(Tracer::new),
        })
    }

    pub fn with_listener_name(self, name: &'static str) -> Self {
        HttpConnectionManagerBuilder { listener_name: Some(name), ..self }
    }
}

#[derive(Debug, Clone)]
pub struct PartialHttpConnectionManager {
    router: Option<RouteConfiguration>,
    codec_type: CodecType,
    dynamic_route_name: Option<CompactString>,
    http_filters_hcm: Vec<Arc<HttpFilter>>,
    http_filters_per_route: HashMap<RouteMatch, Vec<Arc<HttpFilter>>>,
    enabled_upgrades: Vec<UpgradeType>,
    request_timeout: Option<Duration>,
    access_log: Vec<AccessLog>,
    xff_settings: XffSettings,
    generate_request_id: bool,
    preserve_external_request_id: bool,
    always_set_request_id_in_response: bool,
    tracing: Option<Tracing>,
}

#[derive(Debug, Clone)]
pub struct HttpFilter {
    pub name: CompactString,
    pub disabled: bool,
    pub filter: Option<HttpFilterValue>,
}

#[derive(Debug, Clone)]
pub enum HttpFilterValue {
    // todo(francesco): In this enum the RateLimit variant uses a runtime type
    // while Rbac uses a configuration type - we might want to revisit this
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
        Self { name, disabled, filter: Some(filter) }
    }
}

impl HttpFilterValue {
    pub fn apply_request<B>(&self, request: &Request<B>) -> FilterDecision {
        match self {
            HttpFilterValue::Rbac(rbac) => apply_authorization_rules(rbac, request),
            HttpFilterValue::RateLimit(rl) => rl.run(request),
        }
    }
    pub fn apply_response(&self, _response: &mut Response<PolyBody>) -> FilterDecision {
        match self {
            // RBAC and RateLimit do not apply on the response path
            HttpFilterValue::Rbac(_) | HttpFilterValue::RateLimit(_) => FilterDecision::Continue,
        }
    }
    fn from_filter_override(value: &FilterOverride) -> Option<Self> {
        match &value.filter_settings {
            Some(filter_settings) => match filter_settings {
                FilterConfigOverride::LocalRateLimit(rl) => Some(HttpFilterValue::RateLimit((*rl).into())),
                FilterConfigOverride::Rbac(Some(rbac)) => Some(HttpFilterValue::Rbac(rbac.clone())),
                FilterConfigOverride::Rbac(None) => None,
            },
            None => None,
        }
    }
}

fn per_route_http_filters(
    route_config: &RouteConfiguration,
    hcm_filters: &[Arc<HttpFilter>],
) -> HashMap<RouteMatch, Vec<Arc<HttpFilter>>> {
    let mut per_route_filters: HashMap<RouteMatch, Vec<Arc<HttpFilter>>> = HashMap::new();
    for vh in &route_config.virtual_hosts {
        for route in &vh.routes {
            for hcm_filter in hcm_filters {
                let effective_filter = match route.typed_per_filter_config.get(&hcm_filter.name) {
                    Some(override_config) => Arc::new(HttpFilter {
                        name: hcm_filter.name.clone(),
                        disabled: override_config.disabled,
                        filter: HttpFilterValue::from_filter_override(override_config),
                    }),
                    None => Arc::clone(hcm_filter),
                };
                per_route_filters.entry(route.route_match.clone()).or_default().push(effective_filter);
            }
        }
    }
    per_route_filters
}

impl TryFrom<ConversionContext<'_, HttpConnectionManagerConfig>> for PartialHttpConnectionManager {
    type Error = crate::Error;
    fn try_from(ctx: ConversionContext<HttpConnectionManagerConfig>) -> Result<Self> {
        let ConversionContext { envoy_object: configuration, secret_manager: _ } = ctx;
        let codec_type = configuration.codec_type;
        let enabled_upgrades = configuration.enabled_upgrades;
        let http_filters_hcm = configuration
            .http_filters
            .into_iter()
            .map(|f| Arc::new(HttpFilter::from(f)))
            .collect::<Vec<Arc<HttpFilter>>>();
        let request_timeout = configuration.request_timeout;
        let access_log = configuration.access_log;
        let xff_settings = configuration.xff_settings;
        let generate_request_id = configuration.generate_request_id;
        let preserve_external_request_id = configuration.preserve_external_request_id;
        let always_set_request_id_in_response = configuration.always_set_request_id_in_response;

        let mut http_filters_per_route = HashMap::new();
        let (dynamic_route_name, router) = match configuration.route_specifier {
            RouteSpecifier::Rds(RdsSpecifier {
                route_config_name,
                config_source: ConfigSource { config_source_specifier },
            }) => match config_source_specifier {
                ConfigSourceSpecifier::ADS => (Some(route_config_name.to_compact_string()), None),
            },
            RouteSpecifier::RouteConfig(config) => {
                http_filters_per_route = per_route_http_filters(&config, &http_filters_hcm);
                (None, Some(config))
            },
        };

        Ok(PartialHttpConnectionManager {
            router,
            codec_type,
            dynamic_route_name,
            http_filters_hcm,
            http_filters_per_route,
            enabled_upgrades,
            request_timeout,
            access_log,
            xff_settings,
            generate_request_id,
            preserve_external_request_id,
            always_set_request_id_in_response,
            tracing: configuration.tracing,
        })
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
pub struct HttpConnectionManager {
    pub listener_name: &'static str,
    router_sender: watch::Sender<Option<Arc<RouteConfiguration>>>,
    pub codec_type: CodecType,
    dynamic_route_name: Option<CompactString>,
    http_filters_hcm: Vec<Arc<HttpFilter>>,
    http_filters_per_route: ArcSwap<HashMap<RouteMatch, Vec<Arc<HttpFilter>>>>,
    enabled_upgrades: Vec<UpgradeType>,
    request_timeout: Option<Duration>,
    access_log: Vec<AccessLog>,
    xff_settings: XffSettings,
    request_id_handler: RequestIdManager,
    tracer: Option<Tracer>,
}

impl fmt::Display for HttpConnectionManager {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "HttpConnectionManager {}", &self.listener_name,)
    }
}

impl HttpConnectionManager {
    pub fn get_route_id(&self) -> Option<&CompactString> {
        self.dynamic_route_name.as_ref()
    }

    pub fn update_route(&self, route: RouteConfiguration) {
        self.http_filters_per_route.swap(Arc::new(per_route_http_filters(&route, &self.http_filters_hcm)));
        let _ = self.router_sender.send_replace(Some(Arc::new(route)));
    }

    pub fn remove_route(&self) {
        let _ = self.router_sender.send_replace(None);
    }

    pub(crate) fn request_handler(
        self: &Arc<Self>,
    ) -> Box<
        dyn Service<
                ExtendedRequest<Incoming>,
                Response = Response<BodyWithMetrics<PolyBody>>,
                Error = crate::Error,
                Future = BoxFuture<'static, StdResult<Response<BodyWithMetrics<PolyBody>>, crate::Error>>,
            > + Send
            + Sync,
    > {
        Box::new(HttpRequestHandler { manager: Arc::clone(self), router: self.router_sender.subscribe() })
            as Box<
                dyn Service<
                        ExtendedRequest<Incoming>,
                        Response = Response<BodyWithMetrics<PolyBody>>,
                        Error = crate::Error,
                        Future = BoxFuture<'static, StdResult<Response<BodyWithMetrics<PolyBody>>, crate::Error>>,
                    > + Send
                    + Sync,
            >
    }
}

#[derive(Debug)]
pub enum FilterDecision {
    Continue,
    Reroute,
    DirectResponse(Response<PolyBody>),
}

pub struct CachedRoute {
    route: Route,
    route_match: RouteMatchResult,
    vh: VirtualHost,
}

pub(crate) struct HttpRequestHandler {
    manager: Arc<HttpConnectionManager>,
    router: watch::Receiver<Option<Arc<RouteConfiguration>>>,
}

pub struct ExtendedRequest<B> {
    pub request: Request<B>,
    pub downstream_metadata: Arc<DownstreamConnectionMetadata>,
}

#[derive(Debug)]
pub struct AccessLoggersContext {
    access_loggers: Mutex<Vec<LogFormatterLocal>>,
    bytes: AtomicU64, // either the request or response body size, depending which one has completed first
    half_completed: AtomicBool,
}

impl AccessLoggersContext {
    pub fn new(access_log: &[AccessLog]) -> Self {
        AccessLoggersContext {
            access_loggers: Mutex::new(access_log.iter().map(|al| al.logger.local_clone()).collect::<Vec<_>>()),
            bytes: AtomicU64::new(0),
            half_completed: AtomicBool::new(false),
        }
    }
}

#[derive(Debug)]
pub struct TransactionContext {
    start_instant: std::time::Instant,
    access_log_ctx: Option<AccessLoggersContext>,
    trace_ctx: Option<TraceContext>,
    request_id: Option<RequestId>,
}

impl Default for TransactionContext {
    fn default() -> Self {
        TransactionContext {
            start_instant: std::time::Instant::now(),
            access_log_ctx: None,
            trace_ctx: None,
            request_id: None,
        }
    }
}

impl TransactionContext {
    pub fn new(access_log: &[AccessLog], trace_ctx: Option<TraceContext>, request_id: Option<RequestId>) -> Self {
        TransactionContext {
            start_instant: std::time::Instant::now(),
            access_log_ctx: if is_access_log_enabled() { Some(AccessLoggersContext::new(access_log)) } else { None },
            trace_ctx,
            request_id,
        }
    }
}

fn select_virtual_host<'a, T>(request: &Request<T>, virtual_hosts: &'a [VirtualHost]) -> Option<&'a VirtualHost> {
    let mapped_vhs = virtual_hosts.iter().filter_map(|vh| {
        let maybe_score = vh.domains.iter().map(|domain| domain.eval_lpm_request(request)).max().flatten();
        maybe_score.map(|score| (vh, score))
    });

    let virtual_host_with_max_score = mapped_vhs.max_by_key(|(_, score)| score.clone());
    virtual_host_with_max_score.map(|(vh, _)| vh)
}

// has to be a trait due to foreign impl rules.
pub trait RequestHandler<R>: Sized {
    fn to_response(
        self,
        trans_ctx: &TransactionContext,
        request: R,
    ) -> impl Future<Output = Result<Response<PolyBody>>> + Send;
}

fn match_request_route<B>(request: &Request<B>, route_config: &RouteConfiguration) -> Option<CachedRoute> {
    let chosen_vh = select_virtual_host(request, &route_config.virtual_hosts)?;
    let (chosen_route, route_match_result) = chosen_vh
        .routes
        .iter()
        .map(|route| (route, route.route_match.match_request(request)))
        .find(|(_, match_result)| match_result.matched())?;
    Some(CachedRoute { route: chosen_route.clone(), route_match: route_match_result, vh: chosen_vh.clone() })
}

impl
    RequestHandler<(
        Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>,
        Arc<HttpConnectionManager>,
        Arc<DownstreamConnectionMetadata>,
    )> for Arc<RouteConfiguration>
{
    async fn to_response(
        self,
        trans_ctx: &TransactionContext,
        (request, connection_manager, downstream_metadata): (
            Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>,
            Arc<HttpConnectionManager>,
            Arc<DownstreamConnectionMetadata>,
        ),
    ) -> Result<Response<PolyBody>> {
        let mut processed_routes: HashSet<RouteMatch> = HashSet::new();
        let mut cached_route = match_request_route(&request, &self);

        loop {
            if let Some(ref chosen_route) = cached_route {
                if processed_routes.contains(&chosen_route.route.route_match) {
                    // we are in routing loop, processing the same route twice is not permitted
                    return Err(GenericError::from_msg("Routing loop detected").into());
                }

                let guard = connection_manager.http_filters_per_route.load();
                let route_filters = guard.get(&chosen_route.route.route_match);
                if let Some(route_filters) = route_filters {
                    let mut is_reroute = false;
                    for filter in route_filters {
                        if filter.disabled {
                            continue;
                        }
                        if let Some(filter_value) = &filter.filter {
                            let filter_res = filter_value.apply_request(&request);
                            if matches!(filter_res, FilterDecision::Reroute) {
                                // stop processing filters and re-evaluate the route
                                is_reroute = true;
                                break;
                            }
                            if let FilterDecision::DirectResponse(response) = filter_res {
                                return Ok(response);
                            }
                        }
                    }
                    if !is_reroute {
                        break;
                    }
                    processed_routes.insert(chosen_route.route.route_match.clone());
                    cached_route = match_request_route(&request, &self);
                } else {
                    // there are no filters to process
                    break;
                }
            } else {
                return Ok(SyntheticHttpResponse::not_found().into_response(request.version()));
            }
        }

        if let Some(chosen_route) = cached_route {
            let websocket_enabled_by_default =
                upgrade_utils::is_websocket_enabled_by_hcm(&connection_manager.enabled_upgrades);

            let mut response = match &chosen_route.route.action {
                Action::DirectResponse(dr) => dr.to_response(trans_ctx, request).await,
                Action::Redirect(rd) => rd.to_response(trans_ctx, (request, chosen_route.route_match)).await,
                Action::Route(route) => {
                    route
                        .to_response(
                            trans_ctx,
                            MatchedRequest {
                                request,
                                retry_policy: chosen_route.vh.retry_policy.as_ref(),
                                route_match: chosen_route.route_match,
                                remote_address: downstream_metadata.peer_address(),
                                websocket_enabled_by_default,
                            },
                        )
                        .await
                },
            }?;

            let guard = connection_manager.http_filters_per_route.load();
            let route_filters = guard.get(&chosen_route.route.route_match);
            if let Some(route_filters) = route_filters {
                for filter in route_filters.iter().rev() {
                    if filter.disabled {
                        continue;
                    }
                    if let Some(filter_value) = &filter.filter {
                        // we do not evaluate filter decision on the response
                        // path since it cannot be a reroute
                        filter_value.apply_response(&mut response);
                    }
                }
            }

            let resp_headers = response.headers_mut();
            if self.most_specific_header_mutations_wins {
                self.response_header_modifier.modify(resp_headers);
                chosen_route.vh.response_header_modifier.modify(resp_headers);
                chosen_route.route.response_header_modifier.modify(resp_headers);
            } else {
                chosen_route.route.response_header_modifier.modify(resp_headers);
                chosen_route.vh.response_header_modifier.modify(resp_headers);
                self.response_header_modifier.modify(resp_headers);
            }

            Ok(response)
        } else {
            // We should not be here
            Ok(SyntheticHttpResponse::not_found().into_response(request.version()))
        }
    }
}

impl Service<ExtendedRequest<Incoming>> for HttpRequestHandler {
    type Response = Response<BodyWithMetrics<PolyBody>>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, StdResult<Self::Response, Self::Error>>;

    fn call(&self, req: ExtendedRequest<Incoming>) -> Self::Future {
        // 0. destructure the ExtendedRequest to get the request and addresses
        let ExtendedRequest { request, downstream_metadata } = req;
        let incoming_request_id = RequestId::from_request(&request);

        // 1. apply x_request_id policy first...
        let (mut updated_request, request_id) = self.manager.request_id_handler.apply_policy(request);

        // 2. create a trace context, if enabled...
        let trace_context = self
            .manager
            .tracer
            .as_ref()
            .map(|t| t.build_trace_context(&updated_request, incoming_request_id.or(Some(request_id.clone()))));

        // 3. create the transaction context
        let trans_ctx = Arc::new(TransactionContext::new(&self.manager.access_log, trace_context, Some(request_id)));

        // 4. update tracing headers...
        self.manager.tracer.as_ref().inspect(|tracer| {
            if let Some(trace_ctx) = &trans_ctx.trace_ctx {
                tracer.update_tracing_headers(trace_ctx, &mut updated_request);
            }
        });

        // 5. update the incoming request...
        let req = ExtendedRequest { request: updated_request, downstream_metadata };

        let req_timeout = self.manager.request_timeout;
        let listener_name = self.manager.listener_name;
        let route_conf = self.router.borrow().clone();
        let manager = Arc::clone(&self.manager);

        let thread_id = std::thread::current().id();

        with_metric!(http::DOWNSTREAM_RQ_TOTAL, add, 1, thread_id, &[KeyValue::new("listener", listener_name)]);
        with_metric!(http::DOWNSTREAM_RQ_ACTIVE, add, 1, thread_id, &[KeyValue::new("listener", listener_name)]);
        defer! {
            with_metric!(http::DOWNSTREAM_RQ_ACTIVE, sub, 1, thread_id, &[KeyValue::new("listener", listener_name)]);
        }

        Box::pin(async move {
            let ExtendedRequest { request, downstream_metadata } = req;

            let (parts, body) = request.into_parts();
            let request = Request::from_parts(parts, BodyWithTimeout::new(req_timeout, body));
            let permit = log_access_reserve_balanced().await;

            // optionally apply a timeout to the body.
            // envoy says this timeout is started when the request is initiated. This is relatively vague, but because at this point we will
            // already have the headers, it seems like a fair start.
            //  note that we can still time-out a request due to e.g. the filters taking a long time to compute, or the proxy being overwhelmed
            // not just due to the downstream being slow.
            // todo(hayley): this timeout is incorrect (checks for time between frames not total time), and doesn't seem to get converted into
            //  http response

            //
            // 1. evaluate InitHttpContext, if logging is enabled
            eval_http_init_context(&request, &trans_ctx);

            //
            // 2. create the MetricsBody, which will track the size of the request body

            let permit_clone = Arc::clone(&permit);
            let init_flags = request.extensions().get::<ResponseFlags>().cloned().unwrap_or_default();

            let req_head_size = request_head_size(&request);
            let request = request.map(|body| {
                let trans_ctx = Arc::clone(&trans_ctx);
                BodyWithMetrics::new(BodyKind::Request, body, move |nbytes, flags| {
                    with_metric!(
                        http::DOWNSTREAM_CX_RX_BYTES_TOTAL,
                        add,
                        nbytes + req_head_size as u64,
                        thread_id,
                        &[KeyValue::new("listener", listener_name)]
                    );
                    trans_ctx.access_log_ctx.as_ref().inspect(|ctx| {
                        let mut access_loggers = ctx.access_loggers.lock();
                        let duration = trans_ctx.start_instant.elapsed();
                        access_loggers.with_context(&HttpRequestDuration { duration, tx_duration: duration });

                        if ctx.half_completed.load(Ordering::Relaxed) {
                            // if this happens is because the stream of body response finished before the request one!
                            eval_http_finish_context(
                                access_loggers.as_mut(),
                                trans_ctx.start_instant,
                                nbytes,                            // bytes received
                                ctx.bytes.load(Ordering::Relaxed), // bytes sent
                                listener_name,
                                init_flags | flags,
                                permit_clone,
                            );
                        } else {
                            ctx.half_completed.store(true, Ordering::Relaxed);
                            ctx.bytes.store(nbytes, Ordering::Relaxed);
                        }
                    });
                })
            });

            let Some(route_conf) = route_conf else {
                // immediately return a SyntheticHttpResponse, and calcuate the first byte instant
                let resp = SyntheticHttpResponse::not_found().into_response(request.version());
                let first_byte_instant = Instant::now();

                with_metric!(http::DOWNSTREAM_RQ_4XX, add, 1, thread_id, &[KeyValue::new("listener", listener_name)]);

                trans_ctx.access_log_ctx.as_ref().inspect(|ctx| {
                    let response_head_size = response_head_size(&resp);
                    ctx.access_loggers.lock().with_context(&DownstreamResponse { response: &resp, response_head_size })
                });

                let init_flags = resp.extensions().get::<ResponseFlags>().cloned().unwrap_or_default();

                let resp_head_size = response_head_size(&resp);
                let response = resp.map(|body| {
                    BodyWithMetrics::new(BodyKind::Response, body, move |nbytes, flags| {
                        with_metric!(
                            http::DOWNSTREAM_CX_TX_BYTES_TOTAL,
                            add,
                            nbytes + resp_head_size as u64,
                            thread_id,
                            &[KeyValue::new("listener", listener_name)]
                        );
                        trans_ctx.access_log_ctx.as_ref().inspect(|ctx| {
                            let mut access_loggers = ctx.access_loggers.lock();
                            let duration = first_byte_instant.saturating_duration_since(trans_ctx.start_instant);
                            let tx_duration = Instant::now().saturating_duration_since(first_byte_instant);
                            access_loggers.with_context(&HttpResponseDuration { duration, tx_duration });

                            if ctx.half_completed.load(Ordering::Relaxed) {
                                eval_http_finish_context(
                                    access_loggers.as_mut(),
                                    trans_ctx.start_instant,
                                    ctx.bytes.load(Ordering::Relaxed), // bytes received
                                    nbytes,                            // bytes sent
                                    listener_name,
                                    init_flags | flags,
                                    permit,
                                );
                            } else {
                                // notify the request body that response body is completed
                                ctx.half_completed.store(true, Ordering::Relaxed);
                                ctx.bytes.store(nbytes, Ordering::Relaxed);
                            }
                        });
                    })
                });
                return Ok(response);
            };

            let response = handle_http_transaction(
                route_conf,
                trans_ctx,
                permit,
                request,
                manager,
                downstream_metadata,
                thread_id,
            )
            .await;

            update_http_status_code_stats(response, thread_id, listener_name)
        })
    }
}

fn update_http_status_code_stats(
    res: Result<Response<BodyWithMetrics<PolyBody>>>,
    shard_id: ThreadId,
    listener_name: &'static str,
) -> Result<Response<BodyWithMetrics<PolyBody>>> {
    match &res {
        Ok(response) => {
            let status_code = response.status().as_u16();
            match status_code {
                100..200 => {
                    with_metric!(
                        http::DOWNSTREAM_RQ_1XX,
                        add,
                        1,
                        shard_id,
                        &[KeyValue::new("listener", listener_name)]
                    );
                },
                200..300 => {
                    with_metric!(
                        http::DOWNSTREAM_RQ_2XX,
                        add,
                        1,
                        shard_id,
                        &[KeyValue::new("listener", listener_name)]
                    );
                },
                300..400 => {
                    with_metric!(
                        http::DOWNSTREAM_RQ_3XX,
                        add,
                        1,
                        shard_id,
                        &[KeyValue::new("listener", listener_name)]
                    );
                },
                400..500 => {
                    with_metric!(
                        http::DOWNSTREAM_RQ_4XX,
                        add,
                        1,
                        shard_id,
                        &[KeyValue::new("listener", listener_name)]
                    );
                },
                500..600 => {
                    with_metric!(
                        http::DOWNSTREAM_RQ_5XX,
                        add,
                        1,
                        shard_id,
                        &[KeyValue::new("listener", listener_name)]
                    );
                },
                _ => {},
            }
        },
        Err(_) => {
            with_metric!(http::DOWNSTREAM_RQ_5XX, add, 1, shard_id, &[KeyValue::new("listener", listener_name)]);
        },
    }
    res
}

fn eval_http_init_context<R>(request: &Request<R>, ctx: &TransactionContext) {
    ctx.access_log_ctx.as_ref().inspect(|ctx| {
        let request_head_size = request_head_size(request);
        ctx.access_loggers.lock().with_context_fn(|| InitHttpContext {
            start_time: std::time::SystemTime::now(),
            downstream_request: request,
            request_head_size,
            trace_id: None, // todo(nicola): specify the trace ID
        })
    });
}

fn eval_http_finish_context(
    access_loggers: &mut Vec<LogFormatterLocal>,
    trans_start_time: Instant,
    bytes_received: u64,
    bytes_sent: u64,
    listener_name: &'static str,
    flags: ResponseFlags,
    permit: Arc<Mutex<Option<Permit<'static, AccessLogMessage>>>>,
) {
    access_loggers.with_context(&FinishContext {
        duration: trans_start_time.elapsed(),
        bytes_received,
        bytes_sent,
        response_flags: flags.0,
    });

    let loggers: Vec<LogFormatterLocal> = std::mem::take(access_loggers);
    let messages = loggers.into_iter().map(LogFormatterLocal::into_message).collect::<Vec<_>>();
    log_access(permit, Target::Listener(listener_name.to_compact_string()), messages);
}

async fn handle_http_transaction<RC>(
    route_conf: RC,
    trans_ctx: Arc<TransactionContext>,
    permit: Arc<Mutex<Option<Permit<'static, AccessLogMessage>>>>,
    mut request: Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>,
    manager: Arc<HttpConnectionManager>,
    downstream_metadata: Arc<DownstreamConnectionMetadata>,
    shard_id: ThreadId,
) -> Result<Response<BodyWithMetrics<PolyBody>>>
where
    RC: RequestHandler<(
            Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>,
            Arc<HttpConnectionManager>,
            Arc<DownstreamConnectionMetadata>,
        )> + Clone,
{
    let listener_name = manager.listener_name;

    // apply the request header modifiers
    http_modifiers::apply_prerouting_functions(&mut request, downstream_metadata.peer_address(), manager.xff_settings);

    // process request, get the response and calcuate the first byte time
    let result = route_conf.to_response(&trans_ctx, (request, manager.clone(), downstream_metadata.clone())).await;
    let first_byte_instant = Instant::now();

    result.map(|mut response| {
        // set the request id on the response, if necessary...
        if let Some(request_id) = trans_ctx.request_id.as_ref() {
            manager.request_id_handler.apply_to(&mut response, request_id.propagate_ref());
        }

        let initial_flags = response.extensions().get::<ResponseFlags>().cloned().unwrap_or_default();
        trans_ctx.access_log_ctx.as_ref().inspect(|ctx| {
            let response_head_size = response_head_size(&response);
            ctx.access_loggers.lock().with_context(&DownstreamResponse { response: &response, response_head_size })
        });

        let resp_head_size = response_head_size(&response);
        response.map(move |body| {
            BodyWithMetrics::new(BodyKind::Response, body, move |nbytes, flags| {
                with_metric!(
                    http::DOWNSTREAM_CX_TX_BYTES_TOTAL,
                    add,
                    nbytes + resp_head_size as u64,
                    shard_id,
                    &[KeyValue::new("listener", listener_name)]
                );
                trans_ctx.access_log_ctx.as_ref().inspect(|ctx| {
                    let mut access_loggers = ctx.access_loggers.lock();
                    let duration = first_byte_instant.saturating_duration_since(trans_ctx.start_instant);
                    let tx_duration = Instant::now().saturating_duration_since(first_byte_instant);
                    access_loggers.with_context(&HttpResponseDuration { duration, tx_duration });

                    if ctx.half_completed.load(Ordering::Relaxed) {
                        eval_http_finish_context(
                            access_loggers.as_mut(),
                            trans_ctx.start_instant,
                            ctx.bytes.load(Ordering::Relaxed), // bytes received
                            nbytes,                            // bytes sent
                            listener_name,
                            initial_flags | flags,
                            permit,
                        );
                    } else {
                        // notify the request body that response body is completed
                        ctx.half_completed.store(true, Ordering::Relaxed);
                        ctx.bytes.store(nbytes, Ordering::Relaxed);
                    }
                });
            })
        })
    })
}

fn apply_authorization_rules<B>(rbac: &HttpRbac, req: &Request<B>) -> FilterDecision {
    debug!("Applying authorization rules {rbac:?} {:?}", &req.headers());
    if rbac.is_permitted(req) {
        FilterDecision::Continue
    } else {
        FilterDecision::DirectResponse(
            SyntheticHttpResponse::forbidden("RBAC: access denied").into_response(req.version()),
        )
    }
}

#[cfg(test)]
mod tests {
    use orion_configuration::config::network_filters::http_connection_manager::MatchHost;

    use super::*;

    #[test]
    fn test_select_virtual_hosts() {
        let domains1 = vec!["domain1.com:8000", "domain1.com"].into_iter().flat_map(MatchHost::try_from).collect();
        let domains2 = vec!["domain2.com"].into_iter().flat_map(MatchHost::try_from).collect();
        let domains3 = vec!["*.domain3.com", "domain3.com"].into_iter().flat_map(MatchHost::try_from).collect();
        let vh1 = VirtualHost { domains: domains1, ..Default::default() };
        let vh2 = VirtualHost { domains: domains2, ..Default::default() };
        let vh3 = VirtualHost { domains: domains3, ..Default::default() };

        let request = Request::builder().header("host", "127.0.0.1:8000").body(()).unwrap();
        assert_eq!(select_virtual_host(&request, &[vh1.clone(), vh2.clone(), vh3.clone()]), None);

        let request = Request::builder().header("host", "domain1.com:8000").body(()).unwrap();
        assert_eq!(select_virtual_host(&request, &[vh1.clone(), vh2.clone(), vh3.clone()]), Some(&vh1));

        let request = Request::builder().header("host", "domain1.com").body(()).unwrap();
        assert_eq!(select_virtual_host(&request, &[vh1.clone(), vh2.clone(), vh3.clone()]), Some(&vh1));

        let request = Request::builder().header("host", "domain3.com").body(()).unwrap();
        assert_eq!(select_virtual_host(&request, &[vh1.clone(), vh2.clone(), vh3.clone()]), Some(&vh3));

        let request = Request::builder().header("host", "blah.domain3.com").body(()).unwrap();
        assert_eq!(select_virtual_host(&request, &[vh1.clone(), vh2.clone(), vh3.clone()]), Some(&vh3));

        let request = Request::builder().header("host", "blah.domain3.com:8000").body(()).unwrap();
        assert_eq!(select_virtual_host(&request, &[vh1.clone(), vh2.clone(), vh3.clone()]), None);

        let request = Request::builder().header("host", "domain2.com:8000").body(()).unwrap();
        assert_eq!(select_virtual_host(&request, &[vh1.clone(), vh2.clone(), vh3.clone()]), None);

        let domains2 = vec!["domain2.com:8000"].into_iter().flat_map(MatchHost::try_from).collect();
        let vh2 = VirtualHost { domains: domains2, ..Default::default() };
        let request = Request::builder().header("host", "domain2.com").body(()).unwrap();
        assert_eq!(select_virtual_host(&request, &[vh1.clone(), vh2.clone(), vh3.clone()]), None);
    }
}
