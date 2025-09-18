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

use ::http::HeaderValue;
use arc_swap::ArcSwap;
use compact_str::{CompactString, ToCompactString};
use core::time::Duration;
use futures::future::BoxFuture;
use hyper::{body::Incoming, service::Service, Request, Response};
use opentelemetry::global::BoxedSpan;
use opentelemetry::trace::{Span, Status};
use opentelemetry::KeyValue;
use orion_configuration::config::GenericError;
use orion_tracing::span_state::SpanState;
use orion_tracing::{attributes::HTTP_RESPONSE_STATUS_CODE, with_client_span, with_server_span};
use std::collections::HashMap;
use tracing::{debug, info};

use orion_configuration::config::network_filters::http_connection_manager::http_filters::{
    FilterConfigOverride, FilterOverride,
};
use orion_configuration::config::network_filters::http_connection_manager::route::RouteMatch;
use orion_configuration::config::network_filters::http_connection_manager::{Route, VirtualHost, XffSettings};
use orion_configuration::config::network_filters::tracing::{TracingConfig, TracingKey};
use orion_configuration::config::network_filters::{
    access_log::AccessLog,
    http_connection_manager::{
        http_filters::{http_rbac::HttpRbac, HttpFilter as HttpFilterConfig, HttpFilterType},
        route::{Action, RouteMatchResult},
        CodecType, ConfigSource, ConfigSourceSpecifier, HttpConnectionManager as HttpConnectionManagerConfig,
        RdsSpecifier, RouteSpecifier, UpgradeType,
    },
};
use orion_configuration::config::TlvType;
use orion_format::context::{
    DownstreamResponse, FinishContext, HttpRequestDuration, HttpResponseDuration, InitHttpContext,
};

use orion_format::LogFormatterLocal;
use orion_metrics::{metrics::http, with_metric};
use parking_lot::Mutex;
use route::MatchedRequest;
use scopeguard::defer;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::thread::ThreadId;
use std::time::Instant;
use std::{fmt, future::Future, result::Result as StdResult, sync::Arc};
use tokio::sync::mpsc::Permit;
use tokio::sync::watch;
use upgrades as upgrade_utils;

use crate::{
    access_log::{is_access_log_enabled, log_access, log_access_reserve_balanced, AccessLogMessage, Target},
    body::{
        body_with_metrics::BodyWithMetrics,
        response_flags::{BodyKind, ResponseFlags},
    },
};

use crate::{
    body::body_with_timeout::BodyWithTimeout,
    listeners::{
        access_log::AccessLogContext, drain_signaling::DrainSignalingManager,
        filter_state::DownstreamConnectionMetadata, listeners_manager::ConnectionManager, rate_limiter::LocalRateLimit,
        synthetic_http_response::SyntheticHttpResponse,
    },
    utils::http::{request_head_size, response_head_size},
    ConversionContext, PolyBody, Result, RouteConfiguration,
};
use orion_tracing::http_tracer::{HttpTracer, SpanKind, SpanName};
use orion_tracing::request_id::{RequestId, RequestIdManager};
use orion_tracing::trace_context::TraceContext;

#[derive(Debug, Clone)]
pub struct HttpConnectionManagerBuilder {
    listener_name: Option<&'static str>,
    filter_chain_match_hash: Option<u64>,
    connection_manager: PartialHttpConnectionManager,
}

impl TryFrom<ConversionContext<'_, HttpConnectionManagerConfig>> for HttpConnectionManagerBuilder {
    type Error = crate::Error;
    fn try_from(ctx: ConversionContext<HttpConnectionManagerConfig>) -> Result<Self> {
        let partial = PartialHttpConnectionManager::try_from(ctx)?;
        Ok(Self { listener_name: None, filter_chain_match_hash: None, connection_manager: partial })
    }
}

impl HttpConnectionManagerBuilder {
    pub fn build(self) -> Result<HttpConnectionManager> {
        let listener_name = self.listener_name.ok_or("listener name is not set")?;
        let filter_chain_match_hash = self.filter_chain_match_hash.unwrap_or(0);
        let partial = self.connection_manager;
        let router_sender = watch::Sender::new(partial.router.map(Arc::new));

        Ok(HttpConnectionManager {
            listener_name,
            filter_chain_match_hash,
            router_sender,
            codec_type: partial.codec_type,
            dynamic_route_name: partial.dynamic_route_name,
            http_filters_hcm: partial.http_filters_hcm,
            http_filters_per_route: ArcSwap::new(Arc::new(partial.http_filters_per_route)),
            enabled_upgrades: partial.enabled_upgrades,
            request_timeout: partial.request_timeout,
            drain_timeout: partial.drain_timeout,
            access_log: partial.access_log,
            xff_settings: partial.xff_settings,
            request_id_handler: RequestIdManager::new(
                partial.generate_request_id,
                partial.preserve_external_request_id,
                partial.always_set_request_id_in_response,
            ),
            http_tracer: match partial.tracing {
                Some(tracing) => HttpTracer::new().with_config(tracing),
                None => HttpTracer::new(),
            },
            drain_signaling: Arc::new(DrainSignalingManager::new()),
            connection_manager: None, // Will be set during listener startup
        })
    }

    pub fn with_listener_name(self, name: &'static str) -> Self {
        HttpConnectionManagerBuilder { listener_name: Some(name), ..self }
    }

    pub fn with_filter_chain_match_hash(self, value: u64) -> Self {
        HttpConnectionManagerBuilder { filter_chain_match_hash: Some(value), ..self }
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
    drain_timeout: Option<Duration>,
    access_log: Vec<AccessLog>,
    xff_settings: XffSettings,
    generate_request_id: bool,
    preserve_external_request_id: bool,
    always_set_request_id_in_response: bool,
    tracing: Option<TracingConfig>,
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
        let drain_timeout = configuration.drain_timeout;
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
            drain_timeout,
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

pub struct HttpConnectionManager {
    pub listener_name: &'static str,
    pub filter_chain_match_hash: u64,
    router_sender: watch::Sender<Option<Arc<RouteConfiguration>>>,
    pub codec_type: CodecType,
    dynamic_route_name: Option<CompactString>,
    http_filters_hcm: Vec<Arc<HttpFilter>>,
    http_filters_per_route: ArcSwap<HashMap<RouteMatch, Vec<Arc<HttpFilter>>>>,
    enabled_upgrades: Vec<UpgradeType>,
    request_timeout: Option<Duration>,
    drain_timeout: Option<Duration>,
    access_log: Vec<AccessLog>,
    xff_settings: XffSettings,
    request_id_handler: RequestIdManager,
    pub http_tracer: HttpTracer,
    drain_signaling: Arc<DrainSignalingManager>,
    connection_manager: Option<Arc<dyn ConnectionManager>>,
}

impl Clone for HttpConnectionManager {
    fn clone(&self) -> Self {
        Self {
            listener_name: self.listener_name,
            filter_chain_match_hash: self.filter_chain_match_hash,
            router_sender: self.router_sender.clone(),
            codec_type: self.codec_type,
            dynamic_route_name: self.dynamic_route_name.clone(),
            http_filters_hcm: self.http_filters_hcm.clone(),
            http_filters_per_route: ArcSwap::new(self.http_filters_per_route.load_full()),
            enabled_upgrades: self.enabled_upgrades.clone(),
            request_timeout: self.request_timeout,
            drain_timeout: self.drain_timeout,
            access_log: self.access_log.clone(),
            xff_settings: self.xff_settings.clone(),
            request_id_handler: self.request_id_handler.clone(),
            http_tracer: self.http_tracer.clone(),
            drain_signaling: self.drain_signaling.clone(),
            connection_manager: self.connection_manager.clone(),
        }
    }
}

impl std::fmt::Debug for HttpConnectionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpConnectionManager")
            .field("listener_name", &self.listener_name)
            .field("filter_chain_match_hash", &self.filter_chain_match_hash)
            .field("codec_type", &self.codec_type)
            .field("connection_manager", &"<ConnectionManager>")
            .finish()
    }
}

impl fmt::Display for HttpConnectionManager {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "HttpConnectionManager {}", &self.listener_name,)
    }
}

impl HttpConnectionManager {
    #[inline]
    pub fn get_tracing_key(&self) -> TracingKey {
        TracingKey(self.listener_name, self.filter_chain_match_hash)
    }

    #[inline]
    pub fn get_route_id(&self) -> Option<&CompactString> {
        self.dynamic_route_name.as_ref()
    }

    pub fn update_route(&self, route: RouteConfiguration) {
        self.http_filters_per_route.swap(Arc::new(per_route_http_filters(&route, &self.http_filters_hcm)));
        let _ = self.router_sender.send_replace(Some(Arc::new(route)));
    }

    pub fn remove_route(&self) {
        self.http_filters_per_route.swap(Arc::new(HashMap::new()));
        let _ = self.router_sender.send_replace(None);
    }

    pub fn get_drain_signaling(&self) -> Arc<DrainSignalingManager> {
        Arc::clone(&self.drain_signaling)
    }

    /// Get the configured drain timeout for HTTP/2 GOAWAY grace period
    /// Returns None if not configured (uses Envoy default of 5s)
    pub fn get_drain_timeout(&self) -> Option<Duration> {
        self.drain_timeout
    }

    pub async fn start_draining(&self, drain_state: crate::listeners::drain_signaling::ListenerDrainState) {
        // Set up drain context with this HTTP connection manager's configured drain timeout
        if let Some(drain_timeout) = self.drain_timeout {
            let listener_id = format!("{}-{}", self.listener_name, self.filter_chain_match_hash);
            let _ = self
                .drain_signaling
                .initiate_listener_drain(
                    listener_id,
                    true, // is_http = true
                    Some(drain_timeout),
                    0, // active_connections (will be updated by actual connection tracking)
                )
                .await;
        }

        self.drain_signaling.start_listener_draining(drain_state).await;
    }

    pub async fn stop_draining(&self) {
        self.drain_signaling.stop_listener_draining().await;
    }

    /// Set the connection manager for this HTTP connection manager
    pub fn set_connection_manager(&mut self, connection_manager: Arc<dyn ConnectionManager>) {
        self.connection_manager = Some(connection_manager);
    }

    /// Track a new HTTP connection establishment
    pub fn on_connection_established(
        &self,
        connection_id: String,
        protocol: crate::listeners::listeners_manager::ConnectionProtocol,
    ) {
        use crate::listeners::listeners_manager::{ConnectionInfo, ConnectionState};
        use std::time::Instant;

        let conn_info = ConnectionInfo {
            id: connection_id.clone(),
            protocol: protocol.clone(),
            established_at: Instant::now(),
            last_activity: Instant::now(),
            state: ConnectionState::Active,
        };

        info!("HTTP connection {} established with protocol {:?}", connection_id, protocol);

        // Notify the connection manager if available
        if let Some(ref connection_manager) = self.connection_manager {
            connection_manager.on_connection_established(self.listener_name, conn_info);
        }
    }

    /// Track HTTP connection closure
    pub fn on_connection_closed(&self, connection_id: &str) {
        info!("HTTP connection {} closed", connection_id);

        // Notify the connection manager if available
        if let Some(ref connection_manager) = self.connection_manager {
            connection_manager.on_connection_closed(self.listener_name, connection_id);
        }
    }

    /// Apply drain signaling to HTTP response if draining is active
    pub async fn apply_response_drain_signaling<B>(&self, response: &mut hyper::Response<B>) {
        if self.drain_signaling.is_listener_draining().await {
            self.drain_signaling.apply_http1_drain_signal(response).await;
        }
    }

    /// Apply drain signaling to HTTP response if draining is active (synchronous version)
    pub fn apply_response_drain_signaling_sync<B>(&self, response: &mut hyper::Response<B>) -> bool {
        self.drain_signaling.apply_http1_drain_signal_sync(response)
    }

    /// Extract connection ID from downstream metadata
    pub fn extract_connection_id(downstream_metadata: &DownstreamConnectionMetadata) -> String {
        // Generate a connection ID based on local and peer addresses
        format!("{}:{}", downstream_metadata.local_address(), downstream_metadata.peer_address())
    }

    /// Get the protocol from downstream metadata with enhanced detection
    pub fn extract_connection_protocol(
        downstream_metadata: &DownstreamConnectionMetadata,
    ) -> crate::listeners::listeners_manager::ConnectionProtocol {
        // Enhanced protocol detection logic
        match downstream_metadata {
            DownstreamConnectionMetadata::FromProxyProtocol { protocol, tlv_data, .. } => {
                // Check proxy protocol information for TLS/ALPN hints
                Self::detect_from_proxy_protocol(protocol, tlv_data)
            },
            DownstreamConnectionMetadata::FromTlv { tlv_data, .. } => {
                // Check TLV data for protocol hints
                Self::detect_from_tlv_data(tlv_data)
            },
            DownstreamConnectionMetadata::FromSocket { local_address, .. } => {
                // Make educated guess based on port
                Self::detect_from_port(local_address.port())
            },
        }
    }

    /// Detect protocol from proxy protocol information
    fn detect_from_proxy_protocol(
        protocol: &ppp::v2::Protocol,
        tlv_data: &HashMap<TlvType, Vec<u8>>,
    ) -> crate::listeners::listeners_manager::ConnectionProtocol {
        use crate::listeners::listeners_manager::ConnectionProtocol;

        // Check for ALPN TLV data (PP2_TYPE_ALPN = 0x01)
        if let Some(alpn_data) = tlv_data.get(&TlvType::Custom(0x01)) {
            return Self::parse_alpn_protocol(alpn_data);
        }

        // Check for SSL TLV indicating TLS connection (PP2_TYPE_SSL = 0x20)
        if tlv_data.contains_key(&TlvType::Custom(0x20)) {
            // TLS connection detected, likely HTTP/2 capable
            return ConnectionProtocol::Http2;
        }

        // Fall back to basic protocol detection
        match protocol {
            ppp::v2::Protocol::Stream => ConnectionProtocol::Http1, // TCP streams default to HTTP/1.1
            _ => ConnectionProtocol::Unknown,
        }
    }

    /// Detect protocol from TLV data
    fn detect_from_tlv_data(
        tlv_data: &HashMap<u8, Vec<u8>>,
    ) -> crate::listeners::listeners_manager::ConnectionProtocol {
        use crate::listeners::listeners_manager::ConnectionProtocol;

        // Check for ALPN information in TLV data
        if let Some(alpn_data) = tlv_data.get(&0x01) {
            // ALPN TLV type
            return Self::parse_alpn_protocol(alpn_data);
        }

        // Check for TLS/SSL indicators
        if tlv_data.contains_key(&0x20) {
            // SSL TLV type
            return ConnectionProtocol::Http2;
        }

        ConnectionProtocol::Http1 // Default for TLV connections
    }

    /// Parse ALPN protocol from TLV data
    fn parse_alpn_protocol(alpn_data: &[u8]) -> crate::listeners::listeners_manager::ConnectionProtocol {
        use crate::listeners::listeners_manager::ConnectionProtocol;

        if alpn_data.is_empty() {
            return ConnectionProtocol::Http1;
        }

        // ALPN format: length-prefixed protocol strings
        let mut offset = 0;
        while offset < alpn_data.len() {
            if offset + 1 > alpn_data.len() {
                break;
            }

            let proto_len = alpn_data[offset] as usize;
            offset += 1;

            if offset + proto_len > alpn_data.len() {
                break;
            }

            let protocol = &alpn_data[offset..offset + proto_len];
            match protocol {
                b"h2" => return ConnectionProtocol::Http2,
                b"http/1.1" => return ConnectionProtocol::Http1,
                b"http/1.0" => return ConnectionProtocol::Http1,
                _ => {
                    debug!("Unknown ALPN protocol: {:?}", String::from_utf8_lossy(protocol));
                },
            }

            offset += proto_len;
        }

        ConnectionProtocol::Http1 // Default if no recognized protocols
    }

    /// Detect protocol from listening port (heuristic)
    fn detect_from_port(port: u16) -> crate::listeners::listeners_manager::ConnectionProtocol {
        use crate::listeners::listeners_manager::ConnectionProtocol;

        match port {
            443 | 8443 => {
                // HTTPS ports - likely HTTP/2 capable
                ConnectionProtocol::Http2
            },
            80 | 8080 | 8000 => {
                // HTTP ports - likely HTTP/1.1
                ConnectionProtocol::Http1
            },
            _ => {
                // Unknown port - default to HTTP/1.1 for safety
                ConnectionProtocol::Http1
            },
        }
    }

    /// Check if incoming request should trigger drain signaling
    pub async fn should_apply_drain_signaling<B>(&self, _request: &hyper::Request<B>) -> bool {
        self.drain_signaling.is_listener_draining().await
    }

    pub async fn is_draining(&self) -> bool {
        self.drain_signaling.is_listener_draining().await
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
#[allow(dead_code)]
pub enum FilterDecision {
    Continue,
    Reroute,
    DirectResponse(Response<PolyBody>),
}

pub struct CachedRoute<'a> {
    route: &'a Route,
    route_match: RouteMatchResult,
    vh: &'a VirtualHost,
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
}

impl AccessLoggersContext {
    pub fn new(access_log: &[AccessLog]) -> Self {
        AccessLoggersContext {
            access_loggers: Mutex::new(access_log.iter().map(|al| al.logger.local_clone()).collect::<Vec<_>>()),
            bytes: AtomicU64::new(0),
        }
    }
}

#[derive(Debug)]
pub struct TransactionHandler {
    start_instant: std::time::Instant,
    access_log_ctx: Option<AccessLoggersContext>,
    trace_ctx: Option<TraceContext>,
    request_id: RequestId,
    completed_phases: AtomicU8,
    span_state: Option<Arc<SpanState>>,
    thread_id: ThreadId,
}

impl Default for TransactionHandler {
    fn default() -> Self {
        TransactionHandler {
            start_instant: std::time::Instant::now(),
            access_log_ctx: None,
            trace_ctx: None,
            request_id: RequestId::Internal(HeaderValue::from_static("")),
            completed_phases: AtomicU8::new(0),
            span_state: None,
            thread_id: std::thread::current().id(),
        }
    }
}

impl TransactionHandler {
    pub fn new(
        access_log: &[AccessLog],
        trace_ctx: Option<TraceContext>,
        request_id: RequestId,
        server_span: Option<BoxedSpan>,
        thread_id: ThreadId,
    ) -> Self {
        TransactionHandler {
            start_instant: std::time::Instant::now(),
            access_log_ctx: is_access_log_enabled().then(|| AccessLoggersContext::new(access_log)),
            trace_ctx,
            request_id,
            completed_phases: AtomicU8::new(0),
            span_state: server_span.map(|span| Arc::new(SpanState::new(Some(span)))),
            thread_id,
        }
    }

    #[inline]
    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }

    async fn handle_transaction<RC>(
        self: Arc<Self>,
        route_conf: RC,
        manager: Arc<HttpConnectionManager>,
        permit: Arc<Mutex<Option<Permit<'static, AccessLogMessage>>>>,
        mut request: Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>,
        downstream_metadata: Arc<DownstreamConnectionMetadata>,
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
        http_modifiers::apply_prerouting_functions(
            &mut request,
            downstream_metadata.peer_address(),
            manager.xff_settings,
        );

        // process request, get the response and calcuate the first byte time
        let result = route_conf.to_response(&self, (request, manager.clone(), downstream_metadata.clone())).await;
        let first_byte_instant = Instant::now();

        result.map(|mut response| {
            // Apply drain signaling to response if draining is active
            manager.drain_signaling.apply_http1_drain_signal_sync(&mut response);

            // set the request id on the response...
            manager.request_id_handler.apply_to(&mut response, self.request_id.propagate_ref());

            let initial_flags = response.extensions().get::<ResponseFlags>().cloned().unwrap_or_default();

            if let Some(ctx) = self.access_log_ctx.as_ref() {
                let response_head_size = response_head_size(&response);
                ctx.access_loggers.lock().with_context(&DownstreamResponse { response: &response, response_head_size })
            }

            let resp_head_size = response_head_size(&response);

            response.map(move |body| {
                BodyWithMetrics::new(BodyKind::Response, body, move |nbytes, flags| {
                    let is_transaction_complete = self.completed_phases.fetch_add(1, Ordering::Relaxed) > 0;

                    with_metric!(
                        http::DOWNSTREAM_CX_TX_BYTES_TOTAL,
                        add,
                        nbytes + resp_head_size as u64,
                        self.thread_id(),
                        &[KeyValue::new("listener", listener_name)]
                    );

                    if let Some(ctx) = self.access_log_ctx.as_ref() {
                        let mut access_loggers = ctx.access_loggers.lock();
                        let duration = first_byte_instant.saturating_duration_since(self.start_instant);
                        let tx_duration = Instant::now().saturating_duration_since(first_byte_instant);
                        access_loggers.with_context(&HttpResponseDuration { duration, tx_duration });

                        if is_transaction_complete {
                            eval_http_finish_context(
                                access_loggers.as_mut(),
                                self.start_instant,
                                ctx.bytes.load(Ordering::Relaxed), // bytes received
                                nbytes,                            // bytes sent
                                listener_name,
                                initial_flags | flags,
                                permit,
                            );
                        } else {
                            ctx.bytes.store(nbytes, Ordering::Relaxed);
                        }
                    }

                    if is_transaction_complete {
                        if let Some(span) = self.span_state.as_ref() {
                            span.end();
                        }
                    }
                })
            })
        })
    }

    fn trace_status_code(
        self: Arc<Self>,
        res: Result<Response<BodyWithMetrics<PolyBody>>>,
        listener_name: &'static str,
    ) -> Result<Response<BodyWithMetrics<PolyBody>>> {
        if let Ok(response) = &res {
            let status_code = response.status().as_u16();

            with_server_span!(self.span_state, |srv_span: &mut BoxedSpan| srv_span
                .set_attribute(KeyValue::new(HTTP_RESPONSE_STATUS_CODE, i64::from(status_code))));

            match status_code {
                100..200 => {
                    with_metric!(
                        http::DOWNSTREAM_RQ_1XX,
                        add,
                        1,
                        self.thread_id(),
                        &[KeyValue::new("listener", listener_name)]
                    );
                },
                200..300 => {
                    with_metric!(
                        http::DOWNSTREAM_RQ_2XX,
                        add,
                        1,
                        self.thread_id(),
                        &[KeyValue::new("listener", listener_name)]
                    );
                },
                300..400 => {
                    with_metric!(
                        http::DOWNSTREAM_RQ_3XX,
                        add,
                        1,
                        self.thread_id(),
                        &[KeyValue::new("listener", listener_name)]
                    );
                },
                400..500 => {
                    with_metric!(
                        http::DOWNSTREAM_RQ_4XX,
                        add,
                        1,
                        self.thread_id(),
                        &[KeyValue::new("listener", listener_name)]
                    );
                },
                500..600 => {
                    with_metric!(
                        http::DOWNSTREAM_RQ_5XX,
                        add,
                        1,
                        self.thread_id(),
                        &[KeyValue::new("listener", listener_name)]
                    );

                    with_server_span!(self.span_state, |srv_span: &mut BoxedSpan| {
                        srv_span.set_status(Status::error("5xx"));
                    });

                    with_client_span!(self.span_state, |clt_span: &mut BoxedSpan| {
                        clt_span.set_status(Status::error("5xx"));
                    });
                },
                _ => {},
            }
        } else {
            with_metric!(
                http::DOWNSTREAM_RQ_5XX,
                add,
                1,
                self.thread_id(),
                &[KeyValue::new("listener", listener_name)]
            );

            with_server_span!(self.span_state, |srv_span: &mut BoxedSpan| {
                srv_span.set_attribute(KeyValue::new(HTTP_RESPONSE_STATUS_CODE, 500));
                srv_span.set_status(Status::error("5xx"));
            });

            with_client_span!(self.span_state, |clt_span: &mut BoxedSpan| {
                clt_span.set_status(Status::error("5xx"));
            });
        }
        res
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
        trans_handler: &TransactionHandler,
        request: R,
    ) -> impl Future<Output = Result<Response<PolyBody>>> + Send;
}

#[inline]
fn match_request_route<'a, B>(request: &Request<B>, route_config: &'a RouteConfiguration) -> Option<CachedRoute<'a>> {
    let chosen_vh = select_virtual_host(request, &route_config.virtual_hosts)?;
    let (chosen_route, route_match_result) = chosen_vh
        .routes
        .iter()
        .map(|route| (route, route.route_match.match_request(request)))
        .find(|(_, match_result)| match_result.matched())?;
    Some(CachedRoute { route: chosen_route, route_match: route_match_result, vh: chosen_vh })
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
        trans_handler: &TransactionHandler,
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
                Action::DirectResponse(dr) => dr.to_response(trans_handler, request).await,
                Action::Redirect(rd) => rd.to_response(trans_handler, (request, chosen_route.route_match)).await,
                Action::Route(route) => {
                    route
                        .to_response(
                            trans_handler,
                            (
                                MatchedRequest {
                                    request,
                                    retry_policy: chosen_route.vh.retry_policy.as_ref(),
                                    route_match: chosen_route.route_match,
                                    remote_address: downstream_metadata.peer_address(),
                                    websocket_enabled_by_default,
                                },
                                &connection_manager,
                            ),
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

            match connection_manager.codec_type {
                CodecType::Http1 => {
                    connection_manager.drain_signaling.apply_http1_drain_signal(&mut response).await;
                },
                CodecType::Http2 => {
                    // HTTP/2 drain signaling is handled at the connection level via GOAWAY
                    // This would be implemented in the HTTP/2 connection handling
                },
                CodecType::Auto => {
                    // For auto-detection, default to HTTP/1.1 behavior
                    connection_manager.drain_signaling.apply_http1_drain_signal(&mut response).await;
                },
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

        // 2. create a trace context and SERVER span, if enabled...
        let trace_context = self
            .manager
            .http_tracer
            .try_build_trace_context(&updated_request, incoming_request_id.or(Some(request_id.clone())));

        let mut server_span = self.manager.http_tracer.try_create_span(
            trace_context.as_ref(),
            &self.manager.get_tracing_key(),
            SpanKind::Server,
            SpanName::Host(&updated_request),
        );

        // set default attributes to span, using downstream request information...
        if let Some(span) = server_span.as_mut() {
            self.manager.http_tracer.set_attributes_from_request(span, &updated_request);
        }

        // 3. create the transaction context
        let trans_handler = Arc::new(TransactionHandler::new(
            &self.manager.access_log,
            trace_context,
            request_id,
            server_span,
            std::thread::current().id(),
        ));

        // 4. update tracing headers...
        if let Some(trace_ctx) = trans_handler.trace_ctx.as_ref() {
            self.manager.http_tracer.update_tracing_headers(trace_ctx, &mut updated_request);
        }

        // 5. update the incoming request...
        let req = ExtendedRequest { request: updated_request, downstream_metadata };

        let req_timeout = self.manager.request_timeout;
        let listener_name = self.manager.listener_name;
        let route_conf = self.router.borrow().clone();
        let manager = Arc::clone(&self.manager);

        with_metric!(
            http::DOWNSTREAM_RQ_TOTAL,
            add,
            1,
            trans_handler.thread_id(),
            &[KeyValue::new("listener", listener_name)]
        );
        with_metric!(
            http::DOWNSTREAM_RQ_ACTIVE,
            add,
            1,
            trans_handler.thread_id(),
            &[KeyValue::new("listener", listener_name)]
        );
        defer! {
            with_metric!(http::DOWNSTREAM_RQ_ACTIVE, sub, 1, trans_handler.thread_id(), &[KeyValue::new("listener", listener_name)]);
        }

        let trans_handler = trans_handler.clone();
        Box::pin(async move {
            let ExtendedRequest { request, downstream_metadata } = req;

            // Track this connection if not already tracked
            let connection_id = HttpConnectionManager::extract_connection_id(&downstream_metadata);
            let protocol = HttpConnectionManager::extract_connection_protocol(&downstream_metadata);

            // Notify connection establishment (this may be called multiple times for the same connection,
            // but the connection manager should handle deduplication)
            manager.on_connection_established(connection_id.clone(), protocol);

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
            eval_http_init_context(&request, &trans_handler);

            //
            // 2. create the MetricsBody, which will track the size of the request body

            let permit_clone = Arc::clone(&permit);
            let init_flags = request.extensions().get::<ResponseFlags>().cloned().unwrap_or_default();

            let req_head_size = request_head_size(&request);
            let request = request.map(|body| {
                let trans_handler = Arc::clone(&trans_handler);
                BodyWithMetrics::new(BodyKind::Request, body, move |nbytes, flags| {
                    let is_transaction_complete = trans_handler.completed_phases.fetch_add(1, Ordering::Relaxed) > 0;

                    with_metric!(
                        http::DOWNSTREAM_CX_RX_BYTES_TOTAL,
                        add,
                        nbytes + req_head_size as u64,
                        trans_handler.thread_id(),
                        &[KeyValue::new("listener", listener_name)]
                    );

                    // emit the access log, if the request is completed..
                    if let Some(ctx) = trans_handler.access_log_ctx.as_ref() {
                        let mut access_loggers = ctx.access_loggers.lock();
                        let duration = trans_handler.start_instant.elapsed();
                        access_loggers.with_context(&HttpRequestDuration { duration, tx_duration: duration });

                        if is_transaction_complete {
                            // if this happens is because the stream of body response finished before the request one!
                            eval_http_finish_context(
                                access_loggers.as_mut(),
                                trans_handler.start_instant,
                                nbytes,                            // bytes received
                                ctx.bytes.load(Ordering::Relaxed), // bytes sent
                                listener_name,
                                init_flags | flags,
                                permit_clone,
                            );
                        } else {
                            ctx.bytes.store(nbytes, Ordering::Relaxed);
                        }
                    }

                    if is_transaction_complete {
                        if let Some(span) = trans_handler.span_state.as_ref() {
                            span.end();
                        }
                    }
                })
            });

            let Some(route_conf) = route_conf else {
                // immediately return a SyntheticHttpResponse, and calcuate the first byte instant
                let resp = SyntheticHttpResponse::not_found().into_response(request.version());
                let first_byte_instant = Instant::now();

                with_metric!(
                    http::DOWNSTREAM_RQ_4XX,
                    add,
                    1,
                    trans_handler.thread_id(),
                    &[KeyValue::new("listener", listener_name)]
                );

                if let Some(state) = trans_handler.span_state.as_ref() {
                    if let Some(ref mut span) = *state.server_span.lock() {
                        span.set_attribute(KeyValue::new(HTTP_RESPONSE_STATUS_CODE, 400));
                    }
                }

                if let Some(ctx) = trans_handler.access_log_ctx.as_ref() {
                    let response_head_size = response_head_size(&resp);
                    ctx.access_loggers.lock().with_context(&DownstreamResponse { response: &resp, response_head_size })
                }

                let init_flags = resp.extensions().get::<ResponseFlags>().cloned().unwrap_or_default();

                let resp_head_size = response_head_size(&resp);

                let response = resp.map(|body| {
                    BodyWithMetrics::new(BodyKind::Response, body, move |nbytes, flags| {
                        let is_transaction_complete =
                            trans_handler.completed_phases.fetch_add(1, Ordering::Relaxed) > 0;

                        with_metric!(
                            http::DOWNSTREAM_CX_TX_BYTES_TOTAL,
                            add,
                            nbytes + resp_head_size as u64,
                            trans_handler.thread_id(),
                            &[KeyValue::new("listener", listener_name)]
                        );

                        if let Some(ctx) = trans_handler.access_log_ctx.as_ref() {
                            let mut access_loggers = ctx.access_loggers.lock();
                            let duration = first_byte_instant.saturating_duration_since(trans_handler.start_instant);
                            let tx_duration = Instant::now().saturating_duration_since(first_byte_instant);
                            access_loggers.with_context(&HttpResponseDuration { duration, tx_duration });

                            if is_transaction_complete {
                                eval_http_finish_context(
                                    access_loggers.as_mut(),
                                    trans_handler.start_instant,
                                    ctx.bytes.load(Ordering::Relaxed), // bytes received
                                    nbytes,                            // bytes sent
                                    listener_name,
                                    init_flags | flags,
                                    permit,
                                );
                            } else {
                                ctx.bytes.store(nbytes, Ordering::Relaxed);
                            }
                        }

                        if is_transaction_complete {
                            if let Some(span) = trans_handler.span_state.as_ref() {
                                span.end();
                            }
                        }
                    })
                });
                return Ok(response);
            };

            let response = trans_handler
                .clone()
                .handle_transaction(route_conf, manager, permit, request, downstream_metadata)
                .await;

            trans_handler.trace_status_code(response, listener_name)
        })
    }
}

fn eval_http_init_context<R>(request: &Request<R>, trans_handler: &TransactionHandler) {
    if let Some(ctx) = trans_handler.access_log_ctx.as_ref() {
        let trace_id =
            trans_handler.trace_ctx.as_ref().and_then(|t| t.map_child(orion_tracing::trace_info::TraceInfo::trace_id));
        let request_head_size = request_head_size(request);
        ctx.access_loggers.lock().with_context_fn(|| InitHttpContext {
            start_time: std::time::SystemTime::now(),
            downstream_request: request,
            request_head_size,
            trace_id,
        })
    }
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
    use crate::listeners::{
        drain_signaling::{DrainScenario, DrainSignalingManager, ListenerDrainState},
        listeners_manager::{DrainStrategy, ProtocolDrainBehavior},
    };
    use orion_configuration::config::network_filters::http_connection_manager::MatchHost;
    use std::time::Instant;
    use tracing_test::traced_test;

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

    #[traced_test]
    #[tokio::test]
    async fn test_drain_signaling_integration() {
        let drain_signaling = Arc::new(DrainSignalingManager::new());

        // Initially not draining
        assert!(!drain_signaling.is_listener_draining().await);

        // Start draining
        let drain_state = ListenerDrainState {
            started_at: Instant::now(),
            strategy: DrainStrategy::Immediate,
            protocol_behavior: ProtocolDrainBehavior::Http1 { connection_close: true },
            drain_scenario: DrainScenario::ListenerUpdate,
            drain_type: orion_configuration::config::listener::DrainType::Default,
        };
        drain_signaling.start_listener_draining(drain_state).await;
        assert!(drain_signaling.is_listener_draining().await);

        // Test HTTP/1.1 response modification
        let mut response = Response::builder().status(200).body("response body").unwrap();

        drain_signaling.apply_http1_drain_signal(&mut response).await;
        assert_eq!(response.headers().get("connection").unwrap(), "close");
    }

    #[traced_test]
    #[tokio::test]
    async fn test_http1_drain_signal_application() {
        let drain_signaling = Arc::new(DrainSignalingManager::new());

        // Start HTTP/1.1 draining
        let drain_state = ListenerDrainState {
            started_at: Instant::now(),
            strategy: DrainStrategy::Gradual,
            protocol_behavior: ProtocolDrainBehavior::Http1 { connection_close: true },
            drain_scenario: DrainScenario::ListenerUpdate,
            drain_type: orion_configuration::config::listener::DrainType::Default,
        };
        drain_signaling.start_listener_draining(drain_state).await;

        let mut response = Response::builder().status(200).body("response body").unwrap();

        drain_signaling.apply_http1_drain_signal(&mut response).await;
        assert_eq!(response.headers().get("connection").unwrap(), "close");
    }

    #[traced_test]
    #[tokio::test]
    async fn test_auto_drain_behavior() {
        let drain_signaling = Arc::new(DrainSignalingManager::new());

        // Start auto draining
        let drain_state = ListenerDrainState {
            started_at: Instant::now(),
            strategy: DrainStrategy::Gradual,
            protocol_behavior: ProtocolDrainBehavior::Auto,
            drain_scenario: DrainScenario::ListenerUpdate,
            drain_type: orion_configuration::config::listener::DrainType::Default,
        };
        drain_signaling.start_listener_draining(drain_state).await;

        let mut response = Response::builder().status(200).body("response body").unwrap();

        drain_signaling.apply_http1_drain_signal(&mut response).await;
        // Auto defaults to HTTP/1.1 behavior
        assert_eq!(response.headers().get("connection").unwrap(), "close");
    }

    #[traced_test]
    #[tokio::test]
    async fn test_no_drain_signal_when_not_draining() {
        let drain_signaling = Arc::new(DrainSignalingManager::new());

        let mut response = Response::builder().status(200).body("response body").unwrap();

        drain_signaling.apply_http1_drain_signal(&mut response).await;
        // Should not have Connection: close header when not draining
        assert!(!response.headers().contains_key("connection"));
    }

    #[traced_test]
    #[tokio::test]
    async fn test_http2_drain_behavior() {
        let drain_signaling = Arc::new(DrainSignalingManager::new());

        // Start HTTP/2 draining
        let drain_state = ListenerDrainState {
            started_at: Instant::now(),
            strategy: DrainStrategy::Gradual,
            protocol_behavior: ProtocolDrainBehavior::Http2 { send_goaway: true },
            drain_scenario: DrainScenario::ListenerUpdate,
            drain_type: orion_configuration::config::listener::DrainType::Default,
        };
        drain_signaling.start_listener_draining(drain_state).await;

        let mut response = Response::builder().status(200).body("response body").unwrap();

        // HTTP/2 doesn't use Connection: close headers, it uses GOAWAY frames
        drain_signaling.apply_http1_drain_signal(&mut response).await;
        assert!(!response.headers().contains_key("connection"));
    }
}
