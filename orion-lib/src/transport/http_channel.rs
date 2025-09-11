// Copyright 2025 The kmesh Authors
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

use super::{
    bind_device::BindDevice,
    connector::LocalConnectorWithDNSResolver,
    policy::{RequestContext, RequestExt},
};
use crate::{
    body::{body_with_metrics::BodyWithMetrics, body_with_timeout::BodyWithTimeout, response_flags::ResponseFlags},
    clusters::retry_policy::{EventError, RetryCondition, TryInferFrom},
    listeners::{
        http_connection_manager::{RequestHandler, TransactionHandler},
        synthetic_http_response::SyntheticHttpResponse,
    },
    secrets::{TlsConfigurator, WantsToBuildClient},
    thread_local::{LocalBuilder, LocalObject},
    Error, PolyBody, Result,
};
use http::{
    uri::{Authority, Parts},
    HeaderValue, Response, Version,
};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Request, Uri};
use hyper_rustls::{FixedServerNameResolver, HttpsConnector};
use hyper_util::{
    client::legacy::{connect::Connect, Builder, Client},
    rt::tokio::{TokioExecutor, TokioTimer},
};
use opentelemetry::KeyValue;
use orion_configuration::config::{
    cluster::http_protocol_options::{Codec, HttpProtocolOptions},
    network_filters::http_connection_manager::RetryPolicy,
};
use orion_format::types::{ResponseFlagsLong, ResponseFlagsShort};
use orion_metrics::{metrics::clusters, with_metric};
use pingora_timeout::fast_timeout::fast_timeout;
use pretty_duration::pretty_duration;
use rustls::ClientConfig;
use scopeguard::defer;
use smol_str::ToSmolStr;
use std::{
    io::ErrorKind,
    mem,
    result::Result as StdResult,
    sync::Arc,
    thread::ThreadId,
    time::{Duration, Instant},
};
use tracing::debug;
use webpki::types::ServerName;

#[cfg(feature = "metrics")]
use {
    hyper_util::client::legacy::pool::{ConnectionEvent, EventHandler, Tag},
    hyper_util::client::legacy::PoolKey,
    std::any::Any,
};
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

type IncomingResult = (std::result::Result<Response<Incoming>, Error>, Duration);

type HttpClient = Client<LocalConnectorWithDNSResolver, BodyWithMetrics<PolyBody>>;
type HttpsClient = Client<HttpsConnector<LocalConnectorWithDNSResolver>, BodyWithMetrics<PolyBody>>;

// Rationale: The outer Arc is necessary to avoid building a new Client when cloning the HttpChannel.
// The inner Arc, instead, is used to pass the client to async code, so it's already wrapped by the Arc.

#[derive(Clone, Debug)]
pub struct ClientContext {
    configured_upstream_http_version: Codec,
    client: Arc<LocalObject<Arc<HttpsClient>, Builder, HttpsConnector<LocalConnectorWithDNSResolver>>>,
}
impl ClientContext {
    fn new(
        configured_upstream_http_version: Codec,
        client: Arc<LocalObject<Arc<HttpsClient>, Builder, HttpsConnector<LocalConnectorWithDNSResolver>>>,
    ) -> Self {
        Self { configured_upstream_http_version, client }
    }
}

#[derive(Clone, Debug)]
pub struct HttpChannel {
    pub client: HttpChannelClient,
    pub http_version: Codec,
    pub upstream_authority: Authority, // upstream authority
    pub cluster_name: &'static str,
}

#[derive(Clone, Debug)]
pub enum HttpChannelClient {
    Plain(Arc<LocalObject<Arc<HttpClient>, Builder, LocalConnectorWithDNSResolver>>),
    Tls(ClientContext),
}

impl HttpChannelClient {
    pub fn is_tls(&self) -> bool {
        matches!(self, HttpChannelClient::Tls(_))
    }
}

#[derive(Default)]
pub struct HttpChannelBuilder {
    tls: Option<TlsConfigurator<ClientConfig, WantsToBuildClient>>,
    authority: Option<Authority>,
    bind_device: Option<BindDevice>,
    server_name: Option<ServerName<'static>>,
    http_protocol_options: HttpProtocolOptions,
    connection_timeout: Option<Duration>,
    cluster_name: Option<&'static str>,
}

impl LocalBuilder<LocalConnectorWithDNSResolver, Arc<HttpClient>> for Builder {
    fn build(&self, arg: LocalConnectorWithDNSResolver) -> Arc<HttpClient> {
        Arc::new(self.build(arg))
    }
}

impl LocalBuilder<HttpsConnector<LocalConnectorWithDNSResolver>, Arc<HttpsClient>> for Builder {
    fn build(&self, arg: HttpsConnector<LocalConnectorWithDNSResolver>) -> Arc<HttpsClient> {
        Arc::new(self.build(arg))
    }
}

impl HttpChannelBuilder {
    pub fn new(bind_device: Option<BindDevice>) -> Self {
        Self { bind_device, ..Default::default() }
    }

    pub fn with_tls(self, tls_configurator: Option<TlsConfigurator<ClientConfig, WantsToBuildClient>>) -> Self {
        Self { tls: tls_configurator, ..self }
    }

    pub fn with_timeout(self, timeout: Option<Duration>) -> Self {
        Self { connection_timeout: timeout, ..self }
    }

    pub fn with_authority(self, authority: Authority) -> Self {
        Self { authority: Some(authority), ..self }
    }

    pub fn with_cluster_name(self, cluster_name: &'static str) -> Self {
        Self { cluster_name: Some(cluster_name), ..self }
    }

    pub fn with_server_name(self, server_name: ServerName<'static>) -> Self {
        Self { server_name: Some(server_name), ..self }
    }

    pub fn with_http_protocol_options(self, http_protocol_options: HttpProtocolOptions) -> Self {
        Self { http_protocol_options, ..self }
    }

    #[allow(clippy::cast_sign_loss)]
    pub fn build(self) -> crate::Result<HttpChannel> {
        let authority = self.authority.clone().ok_or_else(|| Error::from("Authority is mandatory"))?;
        let client_builder = self.configure_hyper_client();

        if let Some(tls_context) = self.tls {
            // Build TLS client inline to avoid ownership issues
            let mut builder =
                hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls_context.into_inner()).https_or_http();

            builder = if let Some(server_name) = self.server_name {
                builder.with_server_name_resolver(FixedServerNameResolver::new(server_name))
            } else {
                let server_name = ServerName::try_from(authority.host().to_owned())?;
                debug!("Server name is not configured in bootstrap.. using endpoint authority {:?}", server_name);
                builder.with_server_name_resolver(FixedServerNameResolver::new(server_name))
            };

            let connector = LocalConnectorWithDNSResolver {
                addr: authority.clone(),
                cluster_name: self.cluster_name.unwrap_or_default(),
                bind_device: self.bind_device,
                timeout: self.connection_timeout,
            };

            let tls_connector = match self.http_protocol_options.codec {
                Codec::Http2 => builder.enable_http2().wrap_connector(connector),
                Codec::Http1 => builder.enable_http1().wrap_connector(connector),
            };

            Ok(HttpChannel {
                client: HttpChannelClient::Tls(ClientContext::new(
                    self.http_protocol_options.codec,
                    Arc::new(LocalObject::new(client_builder, tls_connector)),
                )),
                http_version: self.http_protocol_options.codec,
                upstream_authority: authority,
                cluster_name: self.cluster_name.unwrap_or_default(),
            })
        } else {
            // Build plain client inline
            let connector = LocalConnectorWithDNSResolver {
                addr: authority.clone(),
                bind_device: self.bind_device,
                timeout: self.connection_timeout,
                cluster_name: self.cluster_name.unwrap_or_default(),
            };

            Ok(HttpChannel {
                client: HttpChannelClient::Plain(Arc::new(LocalObject::new(client_builder, connector))),
                http_version: self.http_protocol_options.codec,
                upstream_authority: authority,
                cluster_name: self.cluster_name.unwrap_or_default(),
            })
        }
    }

    fn configure_hyper_client(&self) -> Builder {
        let mut client_builder = Client::builder(TokioExecutor::new());
        client_builder
            .timer(TokioTimer::new())
            .pool_idle_timeout(self.http_protocol_options.common.idle_timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT))
            .pool_timer(TokioTimer::new())
            .pool_max_idle_per_host(usize::MAX)
            .set_host(false);

        let configured_upstream_http_version = self.http_protocol_options.codec;

        self.configure_http2_if_needed(&mut client_builder, configured_upstream_http_version);

        #[cfg(feature = "metrics")]
        {
            let cluster_name = self.cluster_name.unwrap_or_default();
            client_builder.event_handler(EventHandler::new(update_upstream_stats, cluster_name));
        }

        client_builder
    }

    fn configure_http2_if_needed(&self, client_builder: &mut Builder, version: Codec) {
        if matches!(version, Codec::Http2) {
            client_builder.http2_only(true);
            let http2_options = &self.http_protocol_options.http2_options;

            if let Some(settings) = &http2_options.keep_alive_settings {
                client_builder.http2_keep_alive_interval(settings.keep_alive_interval);
                if let Some(timeout) = settings.keep_alive_timeout {
                    client_builder.http2_keep_alive_timeout(timeout);
                }
                client_builder.http2_keep_alive_while_idle(true);
            }

            client_builder.http2_initial_connection_window_size(http2_options.initial_connection_window_size());
            client_builder.http2_initial_stream_window_size(http2_options.initial_stream_window_size());

            if let Some(max) = http2_options.max_concurrent_streams() {
                client_builder.http2_max_concurrent_reset_streams(max);
            }
        }
    }
}

#[cfg(feature = "metrics")]
#[allow(clippy::needless_pass_by_value)]
fn update_upstream_stats(event: ConnectionEvent, key: &dyn Any, tag: &dyn Tag) {
    use tracing::debug;
    let cluster_name = *(tag.as_any().downcast_ref::<&str>().unwrap_or(&""));
    let shard_id = std::thread::current().id();
    if let Some(pk) = key.downcast_ref::<PoolKey>() {
        debug!("HttpClient: {:?} for cluster {:?} (pool_key: {:?})", event, cluster_name, pk);
    }

    match event {
        ConnectionEvent::NewConnection => {
            with_metric!(clusters::UPSTREAM_CX_TOTAL, add, 1, shard_id, &[KeyValue::new("cluster", cluster_name)]);
            with_metric!(clusters::UPSTREAM_CX_ACTIVE, add, 1, shard_id, &[KeyValue::new("cluster", cluster_name)]);
        },
        ConnectionEvent::IdleConnectionClosed => {
            with_metric!(clusters::UPSTREAM_CX_DESTROY, add, 1, shard_id, &[KeyValue::new("cluster", cluster_name)]);
            with_metric!(
                clusters::UPSTREAM_CX_IDLE_TIMEOUT,
                add,
                1,
                shard_id,
                &[KeyValue::new("cluster", cluster_name)]
            );
            with_metric!(clusters::UPSTREAM_CX_ACTIVE, sub, 1, shard_id, &[KeyValue::new("cluster", cluster_name)]);
        },
        ConnectionEvent::ConnectionError => {
            with_metric!(
                clusters::UPSTREAM_CX_CONNECT_FAIL,
                add,
                1,
                shard_id,
                &[KeyValue::new("cluster", cluster_name)]
            );
        },
        ConnectionEvent::ConnectionTimeout => {
            with_metric!(
                clusters::UPSTREAM_CX_CONNECT_TIMEOUT,
                add,
                1,
                shard_id,
                &[KeyValue::new("cluster", cluster_name)]
            );
        },
        ConnectionEvent::ConnectionClosed => {
            with_metric!(clusters::UPSTREAM_CX_DESTROY, add, 1, shard_id, &[KeyValue::new("cluster", cluster_name)]);
            with_metric!(clusters::UPSTREAM_CX_ACTIVE, sub, 1, shard_id, &[KeyValue::new("cluster", cluster_name)]);
        },
    }
}

impl<'a> RequestHandler<RequestExt<'a, Request<BodyWithMetrics<PolyBody>>>> for &HttpChannel {
    async fn to_response(
        self,
        _trans_handler: &TransactionHandler,
        request: RequestExt<'a, Request<BodyWithMetrics<PolyBody>>>,
    ) -> Result<Response<crate::PolyBody>> {
        let version = request.req.version();
        let cluster_name = self.cluster_name;
        match &self.client {
            HttpChannelClient::Plain(sender) => {
                let RequestContext { route_timeout, retry_policy } = request.ctx.clone();
                let client = sender.get_or_build();
                let req = maybe_normalize_uri(request.req, false)?;

                let result = if let Some(t) = route_timeout {
                    match fast_timeout(t, self.send_request(retry_policy, client, req, cluster_name)).await {
                        Ok(result) => result,
                        Err(_) => (Err(EventError::RouteTimeout.into()), t),
                    }
                } else {
                    self.send_request(retry_policy, client, req, cluster_name).await
                };
                HttpChannel::handle_response(result, route_timeout, version)
            },
            HttpChannelClient::Tls(context) => {
                let ClientContext { configured_upstream_http_version, client: sender } = context;
                let RequestContext { route_timeout, retry_policy } = request.ctx.clone();
                let configured_version = *configured_upstream_http_version;
                let client = sender.get_or_build();

                //FIXME(hayley): apply http protocol translation for plaintext too
                debug!("Using TLS incoming http {version:?} configured {configured_version:?}");
                let req = maybe_normalize_uri(request.req, true)?;
                let req = maybe_change_http_protocol_version(req, configured_version)?;
                let result = if let Some(t) = route_timeout {
                    match fast_timeout(t, self.send_request(retry_policy, client, req, cluster_name)).await {
                        Ok(result) => result,
                        Err(_) => (Err(EventError::RouteTimeout.into()), t),
                    }
                } else {
                    self.send_request(retry_policy, client, req, cluster_name).await
                };

                HttpChannel::handle_response(result, route_timeout, version)
            },
        }
    }
}

// fn update_upstream_stats(
//     client_stats: &ClientEndpointStats,
//     cluster_name: &'static str,
//     shard_id: (ThreadId, Authority),
//     is_tls: bool,
// ) {
//     let total = client_stats.total_cx.load(std::sync::atomic::Ordering::Relaxed) as u64;
//     let destroy = client_stats.destroy_cx.load(std::sync::atomic::Ordering::Relaxed) as u64;
//     let active = total.saturating_sub(destroy);
//
//     with_metric!(
//         clusters::UPSTREAM_CX_TOTAL,
//         store,
//         total,
//         shard_id.clone(),
//         &[KeyValue::new("clusters", cluster_name)]
//     );
//     with_metric!(
//         clusters::UPSTREAM_CX_DESTROY,
//         store,
//         destroy,
//         shard_id.clone(),
//         &[KeyValue::new("clusters", cluster_name)]
//     );
//
//     with_metric!(clusters::UPSTREAM_CX_ACTIVE, store, active, shard_id, &[KeyValue::new("clusters", cluster_name)]);
// }

impl HttpChannel {
    /// Send the request and return the Result, either the Response or an Error,
    /// along with the time spent for possible retransmissions. Note: the returned
    /// duration does not include the time spent receiving the Body of the Response.
    async fn send_request<C>(
        &self,
        retry_policy: Option<&RetryPolicy>,
        sender: &Client<C, BodyWithMetrics<PolyBody>>,
        req: Request<BodyWithMetrics<PolyBody>>,
        cluster_name: &'static str,
    ) -> (StdResult<Response<Incoming>, Error>, Duration)
    where
        C: Connect + Clone + Send + Sync + 'static,
    {
        let thread_id = std::thread::current().id();

        with_metric!(clusters::UPSTREAM_RQ_ACTIVE, add, 1, thread_id, &[KeyValue::new("cluster", cluster_name)]);
        defer! {
            with_metric!(clusters::UPSTREAM_RQ_ACTIVE, sub, 1, thread_id, &[KeyValue::new("cluster", cluster_name)]);
        }

        match retry_policy {
            Some(policy) if policy.is_retriable(&req) => {
                let (resp, dur, total_request) =
                    self.send_with_retry(policy, sender, req, thread_id, cluster_name).await;
                with_metric!(
                    clusters::UPSTREAM_RQ_TOTAL,
                    add,
                    total_request as u64,
                    thread_id,
                    &[KeyValue::new("cluster", cluster_name)]
                );
                with_metric!(
                    clusters::UPSTREAM_RQ_RETRY,
                    add,
                    (total_request - 1) as u64,
                    thread_id,
                    &[KeyValue::new("cluster", cluster_name)]
                );
                (resp, dur)
            },
            _ => {
                with_metric!(clusters::UPSTREAM_RQ_TOTAL, add, 1, thread_id, &[KeyValue::new("cluster", cluster_name)]);
                let start_time = Instant::now();
                let resp = sender.request(req).await.map_err(Error::from);
                (resp, start_time.elapsed())
            },
        }
    }

    async fn send_with_retry<C>(
        &self,
        retry_policy: &RetryPolicy,
        sender: &Client<C, BodyWithMetrics<PolyBody>>,
        req: Request<BodyWithMetrics<PolyBody>>,
        thread_id: ThreadId,
        cluster_name: &'static str,
    ) -> (StdResult<Response<Incoming>, Error>, Duration, usize)
    where
        C: Connect + Clone + Send + Sync + 'static,
    {
        let (parts, body) = req.into_parts();
        let BodyWithMetrics { inner, guard, state } = body;

        let body = match inner.collect().await {
            Ok(body) => body,
            Err(e) => {
                return (Err(e.into()), Duration::default(), 0);
            },
        };

        let body = http_body_util::Full::new(body.to_bytes());
        let start_time = Instant::now();

        let mut total_requests = 0;
        for (index, back_off) in retry_policy.exponential_back_off().iter().enumerate() {
            total_requests += 1;
            let cloned_body =
                BodyWithMetrics { inner: body.clone().into(), guard: guard.clone(), state: state.clone() };

            let cloned_req: Request<BodyWithMetrics<PolyBody>> = Request::from_parts(parts.clone(), cloned_body);

            // actually send the request and wait for the response...
            let result: StdResult<Response<Incoming>, Error> = if let Some(t) = retry_policy.per_try_timeout() {
                match fast_timeout(t, sender.request(cloned_req)).await.map_err(|_| EventError::PerTryTimeout) {
                    Ok(result) => result.map_err(Into::into),
                    Err(err) => Err(err.into()),
                }
            } else {
                sender.request(cloned_req).await.map_err(Into::into)
            };

            // generate a possible retry condition...
            let Some(condition) = RetryCondition::try_infer_from(&result) else {
                return (result, start_time.elapsed(), total_requests);
            };

            if condition.is_per_try_timeout() {
                with_metric!(
                    clusters::UPSTREAM_RQ_PER_TRY_TIMEOUT,
                    add,
                    1,
                    thread_id,
                    &[KeyValue::new("cluster", cluster_name)]
                );
            }

            // check for a possible retry...
            if !condition.should_retry(retry_policy) {
                return (result, start_time.elapsed(), total_requests);
            }

            // take an exponential back off break and retry...
            if index < retry_policy.num_retries() as usize {
                debug!(
                    "retry_policy: retrying request #{}/{} in {}...",
                    index + 1,
                    retry_policy.num_retries(),
                    pretty_duration(&back_off, None)
                );

                tokio::time::sleep(back_off).await;
            }
        }

        let result = Err(std::io::Error::new(ErrorKind::InvalidData, "invalid retry_policy configuration").into());
        (result, start_time.elapsed(), total_requests)
    }

    fn handle_response(
        result: IncomingResult,
        route_timeout: Option<Duration>,
        version: http::Version,
    ) -> StdResult<hyper::Response<PolyBody>, Error> {
        match result {
            (Ok(response), elapsed) => {
                // calculate the remaining timeout (relative to the route timeout) for receiving
                // the body of the incoming response...
                if let Some(residual_timeout) = route_timeout.map(|dur| dur.checked_sub(elapsed).unwrap_or_default()) {
                    // set the residual_timeout on the body of the Response
                    let (parts, body) = response.into_parts();
                    Ok(Response::from_parts(parts, BodyWithTimeout::new(Some(residual_timeout), body).into()))
                } else {
                    let (parts, body) = response.into_parts();
                    Ok(Response::from_parts(parts, body.into()))
                }
            },
            (Err(err), dur) => {
                if let Some(event_error) = EventError::try_infer_from(err.as_ref()) {
                    let response_flags: ResponseFlags = event_error.clone().into();
                    debug!(
                        "Event ({event_error}) occurred after {:?}: {} ({})",
                        pretty_duration(&dur, None),
                        ResponseFlagsLong(&response_flags.0).to_smolstr(),
                        ResponseFlagsShort(&response_flags.0).to_smolstr()
                    );
                    match event_error {
                        EventError::RefusedStream | EventError::ConnectFailure(_) | EventError::ConnectTimeout(_) => {
                            Ok(SyntheticHttpResponse::service_unavailable(response_flags).into_response(version))
                        },
                        EventError::PerTryTimeout | EventError::RouteTimeout => {
                            Ok(SyntheticHttpResponse::gateway_timeout(response_flags).into_response(version))
                        },
                        EventError::Reset | EventError::Http3PostConnectFailure => {
                            Ok(SyntheticHttpResponse::bad_gateway(response_flags).into_response(version))
                        },
                    }
                } else {
                    debug!("Route: error occurred after {:?}: {err}", pretty_duration(&dur, None));
                    Err(err)
                }
            },
        }
    }

    pub fn is_https(&self) -> bool {
        match &self.client {
            HttpChannelClient::Plain(_) => false,
            HttpChannelClient::Tls(_) => true,
        }
    }

    pub fn http_version(&self) -> Codec {
        self.http_version
    }

    pub fn load(&self) -> u32 {
        let load = match &self.client {
            HttpChannelClient::Plain(sender) => Arc::strong_count(sender.get_or_build()),
            HttpChannelClient::Tls(sender) => Arc::strong_count(sender.client.get_or_build()),
        };
        u32::try_from(load).unwrap_or(u32::MAX)
    }
}

#[inline]
fn is_absolute(uri: &Uri) -> bool {
    uri.authority().is_some() && uri.scheme().is_some()
}

fn select_scheme(version: http::Version, is_tls: bool) -> Option<http::uri::Scheme> {
    match (version, is_tls) {
        (http::Version::HTTP_09 | http::Version::HTTP_10 | http::Version::HTTP_11, false) => {
            Some(http::uri::Scheme::HTTP)
        },
        (http::Version::HTTP_09 | http::Version::HTTP_10 | http::Version::HTTP_11, true) => {
            Some(http::uri::Scheme::HTTPS)
        },
        (http::Version::HTTP_2, _) => Some(http::uri::Scheme::HTTPS),
        _ => None,
    }
}

fn maybe_change_http_protocol_version(
    request: Request<BodyWithMetrics<PolyBody>>,
    version: Codec,
) -> Result<Request<BodyWithMetrics<PolyBody>>> {
    let request = maybe_update_host(request, version)?;
    Ok(maybe_rewrite_version(request, version))
}

fn maybe_rewrite_version(
    mut request: Request<BodyWithMetrics<PolyBody>>,
    version: Codec,
) -> Request<BodyWithMetrics<PolyBody>> {
    *request.version_mut() = match version {
        Codec::Http1 => Version::HTTP_11,
        Codec::Http2 => Version::HTTP_2,
    };
    request
}

fn maybe_update_host(
    mut request: Request<BodyWithMetrics<PolyBody>>,
    version: Codec,
) -> Result<Request<BodyWithMetrics<PolyBody>>> {
    let request_version = request.version();
    match (request_version, version) {
        (Version::HTTP_11, Codec::Http2) => {
            let headers = request.headers_mut();
            headers.remove(http::header::HOST);
        },
        (Version::HTTP_2, Codec::Http1) => {
            if let Some(authority) = request.uri().authority().cloned() {
                debug!("Swapping authority/host (http2 -> http1)");
                request.headers_mut().append(http::header::HOST, HeaderValue::from_str(authority.as_str())?);
            }
        },
        (Version::HTTP_11, Codec::Http1) | (Version::HTTP_2, Codec::Http2) => {},
        (v, _) => {
            return Err(format!("Unsupported http version {v:?}").into());
        },
    }
    Ok(request)
}

fn maybe_normalize_uri(
    mut request: Request<BodyWithMetrics<PolyBody>>,
    is_tls: bool,
) -> crate::Result<Request<BodyWithMetrics<PolyBody>>> {
    let uri = request.uri();
    if !is_absolute(uri) {
        if let Some(host_header) = request.headers().get("host") {
            let authority = host_header.to_str().map_err(|e| format!("Can't parse Host header {e:?}"))?;
            let authority = authority.parse::<Authority>().map_err(|e| format!("Can't parse uri {e:?}"))?;

            let version = request.version();
            let uri = request.uri_mut();
            let mut parts = Parts::from(mem::take(uri));
            if parts.scheme.is_none() {
                parts.scheme = select_scheme(version, is_tls);
            }
            parts.authority = Some(authority);
            let new = Uri::from_parts(parts).map_err(|_| format!("Can't normalize uri: {uri}"))?;
            *uri = new;
        }
    }
    Ok(request)
}
