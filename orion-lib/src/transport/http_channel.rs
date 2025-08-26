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

use super::{bind_device::BindDevice, request_context::RequestWithContext};
use super::{connector::LocalConnectorWithDNSResolver, request_context::RequestContext};
use crate::listeners::http_connection_manager::RequestHandler;
use crate::HttpBody;
use crate::{
    body::timeout_body::TimeoutBody,
    clusters::retry_policy::{should_retry, EventError, FailureKind},
    listeners::synthetic_http_response::SyntheticHttpResponse,
    thread_local::{LocalBuilder, LocalObject},
};
use crate::{
    secrets::{TlsConfigurator, WantsToBuildClient},
    utils::TokioExecutor,
    Error, Result,
};
use http::Version;
use http::{
    uri::{Authority, Parts},
    HeaderValue, Response,
};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Request, Uri};
use hyper_rustls::{FixedServerNameResolver, HttpsConnector};
use orion_configuration::config::{
    cluster::http_protocol_options::{Codec, HttpProtocolOptions},
    network_filters::http_connection_manager::RetryPolicy,
};
use orion_hyper_util::client::legacy::connect::Connect;
use orion_hyper_util::client::legacy::{Builder, Client};
use orion_hyper_util::rt::tokio::TokioTimer;
use pingora_timeout::fast_timeout::fast_timeout;
use pretty_duration::pretty_duration;
use rustls::ClientConfig;
use std::{
    io::ErrorKind,
    mem,
    result::Result as StdResult,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::debug;
use webpki::types::ServerName;

type IncomingResult = (std::result::Result<Response<Incoming>, Error>, Duration);

type HttpClient = Client<LocalConnectorWithDNSResolver, HttpBody>;
type HttpsClient = Client<HttpsConnector<LocalConnectorWithDNSResolver>, HttpBody>;

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
    client: HttpChannelClient,
    http_version: Codec,
}

#[derive(Clone, Debug)]
pub enum HttpChannelClient {
    Plain(Arc<LocalObject<Arc<HttpClient>, Builder, LocalConnectorWithDNSResolver>>),
    Tls(ClientContext),
}

pub struct HttpChannelBuilder {
    tls: Option<TlsConfigurator<ClientConfig, WantsToBuildClient>>,
    authority: Option<Authority>,
    bind_device: Option<BindDevice>,
    server_name: Option<ServerName<'static>>,
    http_protocol_options: HttpProtocolOptions,
    connection_timeout: Option<Duration>,
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
        Self {
            tls: None,
            authority: None,
            bind_device,
            http_protocol_options: HttpProtocolOptions::default(),
            server_name: None,
            connection_timeout: None,
        }
    }

    pub fn with_tls(self, tls_configurator: TlsConfigurator<ClientConfig, WantsToBuildClient>) -> Self {
        Self { tls: Some(tls_configurator), ..self }
    }

    pub fn with_timeout(self, timeout: Option<Duration>) -> Self {
        Self { connection_timeout: timeout, ..self }
    }

    pub fn with_authority(self, authority: Authority) -> Self {
        Self { authority: Some(authority), ..self }
    }

    pub fn with_server_name(self, server_name: ServerName<'static>) -> Self {
        Self { server_name: Some(server_name), ..self }
    }

    pub fn with_http_protocol_options(self, http_protocol_options: HttpProtocolOptions) -> Self {
        Self { http_protocol_options, ..self }
    }

    #[allow(clippy::cast_sign_loss)]
    pub fn build(self) -> crate::Result<HttpChannel> {
        let authority = self.authority.clone().ok_or("Authority is mandatory")?;
        let mut client_builder = Client::builder(TokioExecutor);
        client_builder.timer(TokioTimer::new());
        // note: legacy client builder is not persistent struct (&mut Self -> &mut Self)
        client_builder
            // Set an optional timeout for idle sockets being kept-alive. A Timer is required for this to take effect.
            .pool_idle_timeout(
                self.http_protocol_options.common.idle_timeout.unwrap_or(std::time::Duration::from_secs(30)),
            )
            // Pass a timer for the timeout...
            .pool_timer(TokioTimer::new())
            .pool_max_idle_per_host(usize::MAX)
            .set_host(false);

        let configured_upstream_http_version = self.http_protocol_options.codec;

        if matches!(configured_upstream_http_version, Codec::Http2) {
            client_builder.http2_only(true);
            let http2_options = self.http_protocol_options.http2_options;
            if let Some(settings) = &http2_options.keep_alive_settings {
                client_builder.http2_keep_alive_interval(settings.keep_alive_interval);
                if let Some(timeout) = settings.keep_alive_timeout {
                    client_builder.http2_keep_alive_timeout(timeout);
                }
                client_builder.http2_keep_alive_while_idle(true);
            }
            client_builder.http2_initial_connection_window_size(http2_options.initial_connection_window_size());
            client_builder.http2_initial_stream_window_size(http2_options.initial_stream_window_size());
            //fixme(hayley): this is not max_concurrent_streams! this is reset streams
            if let Some(max) = http2_options.max_concurrent_streams() {
                client_builder.http2_max_concurrent_reset_streams(max);
            }
        }

        if let Some(tls_context) = self.tls {
            let builder = hyper_rustls::HttpsConnectorBuilder::new();
            let builder = builder.with_tls_config(tls_context.into_inner());
            let builder = builder.https_or_http();
            let builder = if let Some(server_name) = self.server_name {
                builder.with_server_name_resolver(FixedServerNameResolver::new(server_name))
            } else {
                let server_name = ServerName::try_from(authority.host().to_owned())?;
                debug!("Server name is not configured in boostrap.. using endpoint authority {:?}", server_name);
                builder.with_server_name_resolver(FixedServerNameResolver::new(server_name))
            };
            let tls_connector = match self.http_protocol_options.codec {
                Codec::Http2 => builder.enable_http2().wrap_connector(LocalConnectorWithDNSResolver {
                    addr: authority,
                    bind_device: self.bind_device,
                    timeout: self.connection_timeout,
                }),

                Codec::Http1 => builder.enable_http1().wrap_connector(LocalConnectorWithDNSResolver {
                    addr: authority,
                    bind_device: self.bind_device,
                    timeout: self.connection_timeout,
                }),
            };
            Ok(HttpChannel {
                client: HttpChannelClient::Tls(ClientContext::new(
                    configured_upstream_http_version,
                    Arc::new(LocalObject::new(client_builder, tls_connector)),
                )),
                http_version: configured_upstream_http_version,
            })
        } else {
            let arg = LocalConnectorWithDNSResolver {
                addr: authority,
                bind_device: self.bind_device,
                timeout: self.connection_timeout,
            };

            Ok(HttpChannel {
                client: HttpChannelClient::Plain(Arc::new(LocalObject::new(client_builder, arg))),
                http_version: configured_upstream_http_version,
            })
        }
    }
}

impl<'a> RequestHandler<RequestWithContext<'a, HttpBody>> for &HttpChannel {
    async fn to_response(self, request: RequestWithContext<'a, HttpBody>) -> Result<Response<crate::PolyBody>> {
        let version = request.req.version();

        match &self.client {
            HttpChannelClient::Plain(sender) => {
                let RequestContext { route_timeout, retry_policy } = request.ctx.clone();
                let sender = Arc::clone(sender.get());
                let req = maybe_normalize_uri(request.req, false)?;

                let result = if let Some(t) = route_timeout {
                    match fast_timeout(t, HttpChannel::send_request(retry_policy, sender, req)).await {
                        Ok(result) => result,
                        Err(_) => (Err(EventError::RouteTimeout.into()), t),
                    }
                } else {
                    HttpChannel::send_request(retry_policy, sender, req).await
                };
                HttpChannel::handle_response(result, route_timeout, version)
            },
            HttpChannelClient::Tls(context) => {
                let ClientContext { configured_upstream_http_version, client: sender } = context;
                let RequestContext { route_timeout, retry_policy } = request.ctx.clone();
                let configured_version = *configured_upstream_http_version;
                let sender = Arc::clone(sender.get());

                //FIXME(hayley): apply http protocol translation for plaintext too
                debug!("Using TLS incoming http {version:?} configured {configured_version:?}");
                let req = maybe_normalize_uri(request.req, true)?;
                let req = maybe_change_http_protocol_version(req, configured_version)?;
                let result = if let Some(t) = route_timeout {
                    match fast_timeout(t, HttpChannel::send_request(retry_policy, sender, req)).await {
                        Ok(result) => result,
                        Err(_) => (Err(EventError::RouteTimeout.into()), t),
                    }
                } else {
                    HttpChannel::send_request(retry_policy, sender, req).await
                };

                HttpChannel::handle_response(result, route_timeout, version)
            },
        }
    }
}

impl HttpChannel {
    /// Send the request and return the Result, either the Response or an Error,
    /// along with the time spent for possible retransmissions. Note: the returned
    /// duration does not include the time spent receiving the Body of the Response.
    async fn send_request<C>(
        retry_policy: Option<&RetryPolicy>,
        sender: Arc<Client<C, HttpBody>>,
        req: Request<HttpBody>,
    ) -> (StdResult<Response<Incoming>, Error>, Duration)
    where
        C: Connect + Clone + Send + Sync + 'static,
    {
        if let Some(policy) = retry_policy {
            if policy.is_retriable(&req) {
                Self::send_with_retry(policy, sender, req).await
            } else {
                (sender.request(req).await.map_err(Error::from), Duration::default())
            }
        } else {
            (sender.request(req).await.map_err(Error::from), Duration::default())
        }
    }

    async fn send_with_retry<C>(
        retry_policy: &RetryPolicy,
        sender: Arc<Client<C, HttpBody>>,
        req: Request<HttpBody>,
    ) -> (StdResult<Response<Incoming>, Error>, Duration)
    where
        C: Connect + Clone + Send + Sync + 'static,
    {
        let (parts, body) = req.into_parts();
        let body = match body.collect().await {
            Ok(body) => body,
            Err(e) => {
                return (Err(e.into()), Duration::default());
            },
        };

        let body = http_body_util::Full::new(body.to_bytes());
        let start_time = Instant::now();

        for (index, back_off) in retry_policy.exponential_back_off().iter().enumerate() {
            let cloned_body = body.clone().into();

            let cloned_req: Request<HttpBody> = Request::from_parts(parts.clone(), cloned_body);

            // actually send the request and wait for the response...

            let result: StdResult<Response<Incoming>, Error> = if let Some(t) = retry_policy.per_try_timeout() {
                match fast_timeout(t, sender.request(cloned_req)).await.map_err(|_| EventError::PerTryTimeout) {
                    Ok(result) => result.map_err(Into::into),
                    Err(err) => Err(err.into()),
                }
            } else {
                sender.request(cloned_req).await.map_err(Into::into)
            };

            // generate a possible failure event...

            let Some(failure) = FailureKind::try_infer(&result) else {
                return (result, start_time.elapsed());
            };

            // check for a possible retry...

            if !should_retry(retry_policy, &failure) {
                return (result, start_time.elapsed());
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
        (result, start_time.elapsed())
    }

    fn handle_response(
        result: IncomingResult,
        route_timeout: Option<Duration>,
        version: http::Version,
    ) -> StdResult<hyper::Response<HttpBody>, Error> {
        match result {
            (Ok(response), elapsed) => {
                // calculate the remaining timeout (relative to the route timeout) for receiving
                // the body of the incoming response...
                if let Some(residual_timeout) = route_timeout.map(|dur| dur.checked_sub(elapsed).unwrap_or_default()) {
                    // set the residual_timeout on the body of the Response
                    let (parts, body) = response.into_parts();
                    Ok(Response::from_parts(parts, TimeoutBody::new(Some(residual_timeout), body).into()))
                } else {
                    let (parts, body) = response.into_parts();
                    Ok(Response::from_parts(parts, body.into()))
                }
            },
            (Err(err), dur) => {
                if let Some(FailureKind::Event(
                    event @ (EventError::ConnectTimeout | EventError::PerTryTimeout | EventError::RouteTimeout),
                )) = FailureKind::<Incoming>::try_infer_from_error(err.as_ref())
                {
                    debug!("Route: timeout ({event}) occurred after {:?}", pretty_duration(&dur, None));
                    Ok(SyntheticHttpResponse::gateway_timeout().into_response(version))
                } else {
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
            HttpChannelClient::Plain(sender) => Arc::strong_count(sender.get()),
            HttpChannelClient::Tls(sender) => Arc::strong_count(sender.client.get()),
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

fn maybe_change_http_protocol_version(request: Request<HttpBody>, version: Codec) -> Result<Request<HttpBody>> {
    let request = maybe_update_host(request, version)?;
    Ok(maybe_rewrite_version(request, version))
}

fn maybe_rewrite_version(mut request: Request<HttpBody>, version: Codec) -> Request<HttpBody> {
    *request.version_mut() = match version {
        Codec::Http1 => Version::HTTP_11,
        Codec::Http2 => Version::HTTP_2,
    };
    request
}

fn maybe_update_host(mut request: Request<HttpBody>, version: Codec) -> Result<Request<HttpBody>> {
    let request_version = request.version();
    match (request_version, version) {
        (Version::HTTP_11, Codec::Http2) => {
            let headers = request.headers_mut();
            headers.remove(http::header::HOST);
        },
        (Version::HTTP_2, Codec::Http1) => {
            if let Some(authority) = request.uri().authority().cloned() {
                debug!("Swaping authority/host (http2 -> http1)");
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

fn maybe_normalize_uri(mut request: Request<HttpBody>, is_tls: bool) -> crate::Result<Request<HttpBody>> {
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
