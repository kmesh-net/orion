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

use super::{
    http_connection_manager::{AlpnCodecs, HttpConnectionManager, HttpConnectionManagerBuilder},
    tcp_proxy::{TcpProxy, TcpProxyBuilder},
};
use crate::{
    AsyncStream, ConversionContext, Error, Result,
    listeners::{filter_state::DownstreamConnectionMetadata, http_connection_manager::ExtendedRequest},
    secrets::{TlsConfigurator, WantsToBuildServer},
    transport::AsyncReadWrite,
    utils::tokio::TokioExecutor,
};
use compact_str::CompactString;
use futures::TryFutureExt;
use hyper::{Request, service::Service};
use opentelemetry::KeyValue;
use orion_client::{rt::TokioIo, server::conn::auto::Builder as HyperServerBuilder};
use orion_configuration::config::{
    listener::{FilterChain as FilterChainConfig, MainFilter},
    network_filters::{
        http_connection_manager::CodecType,
        network_rbac::{NetworkContext, NetworkRbac},
    },
};
use orion_metrics::{
    metrics::{http, tcp, tls},
    with_histogram, with_metric,
};
use rustls::{ServerConfig, server::Acceptor};
use scopeguard::defer;
use std::{sync::Arc, thread::ThreadId};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct FilterchainType {
    pub config: Filterchain,
    pub handler: ConnectionHandler,
}

#[derive(Debug, Clone)]
pub enum ConnectionHandler {
    Http(Arc<HttpConnectionManager>),
    Tcp(TcpProxy),
}

#[derive(Debug, Clone)]
pub struct Filterchain {
    pub name: CompactString,
    pub rbac_filters: Vec<NetworkRbac>,
    pub tls_configurator: Option<TlsConfigurator<ServerConfig, WantsToBuildServer>>,
}

#[derive(Debug, Clone)]
pub enum MainFilterBuilder {
    Http(HttpConnectionManagerBuilder),
    Tcp(TcpProxyBuilder),
}

impl TryFrom<ConversionContext<'_, MainFilter>> for MainFilterBuilder {
    type Error = crate::Error;
    fn try_from(ctx: ConversionContext<MainFilter>) -> Result<Self> {
        let ConversionContext { envoy_object: main_filter, secret_manager } = ctx;
        match main_filter {
            MainFilter::Http(http) => {
                Ok(Self::Http(HttpConnectionManagerBuilder::try_from(ConversionContext::new((http, secret_manager)))?))
            },
            MainFilter::Tcp(tcp) => Ok(Self::Tcp(tcp.into())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FilterchainBuilder {
    name: CompactString,
    listener_name: Option<&'static str>,
    main_filter: MainFilterBuilder,
    rbac_filters: Vec<NetworkRbac>,
    tls_configurator: Option<TlsConfigurator<ServerConfig, WantsToBuildServer>>,
}

impl FilterchainBuilder {
    pub fn with_listener_name(self, name: &'static str) -> Self {
        FilterchainBuilder { listener_name: Some(name), ..self }
    }

    pub fn build(self) -> Result<FilterchainType> {
        let listener_name = self.listener_name.ok_or("listener name is not set")?;
        let filterchain_name = self.name;
        let config = Filterchain {
            name: filterchain_name,
            tls_configurator: self.tls_configurator,
            rbac_filters: self.rbac_filters,
        };
        let handler = match self.main_filter {
            MainFilterBuilder::Http(http_connection_manager) => {
                ConnectionHandler::Http(Arc::new(http_connection_manager.with_listener_name(listener_name).build()?))
            },
            MainFilterBuilder::Tcp(tcp_proxy) => {
                ConnectionHandler::Tcp(tcp_proxy.with_listener_name(listener_name).build()?)
            },
        };
        Ok(FilterchainType { config, handler })
    }
}

impl TryFrom<ConversionContext<'_, FilterChainConfig>> for FilterchainBuilder {
    type Error = Error;
    fn try_from(ctx: ConversionContext<FilterChainConfig>) -> std::result::Result<Self, Self::Error> {
        let ConversionContext { envoy_object: filter_chain, secret_manager } = ctx;
        let main_filter = ConversionContext::new((filter_chain.terminal_filter, secret_manager)).try_into()?;
        let tls_config = filter_chain.tls_config;
        let rbac_filters = filter_chain.rbac;
        let tls_configurator =
            tls_config.map(|tls_config| TlsConfigurator::try_from((tls_config, secret_manager))).transpose()?;
        Ok(FilterchainBuilder {
            name: filter_chain.name,
            listener_name: None,
            main_filter,
            rbac_filters,
            tls_configurator,
        })
    }
}

impl FilterchainType {
    pub fn filter_chain(&self) -> &Filterchain {
        &self.config
    }

    pub fn apply_rbac(
        &self,
        stream: AsyncStream,
        downstream_metadata: &DownstreamConnectionMetadata,
        server_name: Option<&str>,
    ) -> Option<AsyncStream> {
        let rbac_filters = &self.filter_chain().rbac_filters;
        let network_context =
            NetworkContext::new(downstream_metadata.local_address(), downstream_metadata.peer_address(), server_name);
        for rbac in rbac_filters {
            if !rbac.is_permitted(network_context) {
                return None;
            }
        }
        Some(stream)
    }

    pub async fn start_filterchain(
        &self,
        stream: AsyncStream,
        downstream_metadata: Arc<DownstreamConnectionMetadata>,
        shard_id: ThreadId,
        listener_name: &'static str,
        start_instant: std::time::Instant,
    ) -> Result<()> {
        let Self { config, handler } = self;
        match handler {
            ConnectionHandler::Http(http_connection_manager) => {
                with_metric!(http::DOWNSTREAM_CX_TOTAL, add, 1, shard_id, &[KeyValue::new("listener", listener_name)]);
                with_metric!(http::DOWNSTREAM_CX_ACTIVE, add, 1, shard_id, &[KeyValue::new("listener", listener_name)]);
                defer! {
                    with_metric!(http::DOWNSTREAM_CX_DESTROY, add, 1, shard_id, &[KeyValue::new("listener", listener_name)]);
                    with_metric!(http::DOWNSTREAM_CX_ACTIVE, sub, 1, shard_id, &[KeyValue::new("listener", listener_name)]);
                    let ms = u64::try_from(start_instant.elapsed().as_millis()).unwrap_or(u64::MAX);
                    with_histogram!(http::DOWNSTREAM_CX_LENGTH_MS, record, ms, &[KeyValue::new("listener", listener_name)]);
                }

                let req_handler = http_connection_manager.request_handler();
                // codec type as given in the listener, not alpn
                let codec_type = http_connection_manager.codec_type;
                let tls_configurator = config
                    .tls_configurator
                    .clone()
                    .map(TlsConfigurator::<ServerConfig, WantsToBuildServer>::into_inner);

                let (stream, selected_codec) = if let Some(tls_configurator) = tls_configurator {
                    let (stream, negotiated) =
                        start_tls(http_connection_manager.listener_name, stream, tls_configurator, Some(codec_type))
                            .await?;
                    with_metric!(tls::HANDSHAKES, add, 1, shard_id, &[KeyValue::new("listener", listener_name)]);

                    // if we negotiated a protocol over ALPN, use that instead of the configured CodecType.
                    // since we use codec_type to determine our alpn response, we will never negotiate a protocol not covered by codec_type
                    // if we change our config to support setting the alpn protocols from the TlsContext, we should
                    // update this code to make sure it doesn't do anything _too_ weird.
                    let selected_codec = match negotiated {
                        Some(AlpnCodecs::Http2) => CodecType::Http2,
                        Some(AlpnCodecs::Http1) => CodecType::Http1,
                        None => codec_type,
                    };
                    (stream, selected_codec)
                } else {
                    // without TLS no negotiation possible at this point
                    // perhaps we want to preserve auto here and do an upgrade handshake, but that's pretty messy in the code
                    // and only useful in the cases where the listener is not using TLS.
                    // any deployment that does not want to do TLS to downstream, is probably already in the private network
                    // and would prefer prior-knowledge http2
                    let stream: Box<dyn AsyncReadWrite> = Box::new(stream);
                    (stream, codec_type)
                };

                debug!("{listener_name} tried to negotiate {codec_type:?}, got {selected_codec:?}");
                let mut hyper_server = HyperServerBuilder::new(TokioExecutor);
                let stream = TokioIo::new(stream);
                //todo(hayley): we should be applying listener http settings here
                hyper_server = match selected_codec {
                    CodecType::Http1 => hyper_server.http1_only(),
                    CodecType::Http2 => hyper_server.http2_only(),
                    CodecType::Auto => hyper_server,
                };
                hyper_server
                    .serve_connection_with_upgrades(
                        stream,
                        hyper::service::service_fn(|req: Request<hyper::body::Incoming>| {
                            let handler_req =
                                ExtendedRequest { request: req, downstream_metadata: downstream_metadata.clone() };
                            req_handler.call(handler_req).map_err(orion_error::Error::into_inner)
                        }),
                    )
                    .await
                    .inspect_err(|err| debug!("{listener_name} : HTTP connection error: {err}"))
                    .map_err(Error::from)
            },
            ConnectionHandler::Tcp(tcp_proxy) => {
                with_metric!(tcp::DOWNSTREAM_CX_TOTAL, add, 1, shard_id, &[KeyValue::new("listener", listener_name)]);
                with_metric!(tcp::DOWNSTREAM_CX_ACTIVE, add, 1, shard_id, &[KeyValue::new("listener", listener_name)]);
                defer! {
                    with_metric!(tcp::DOWNSTREAM_CX_DESTROY, add, 1, shard_id, &[KeyValue::new("listener", listener_name)]);
                    with_metric!(tcp::DOWNSTREAM_CX_ACTIVE, sub, 1, shard_id, &[KeyValue::new("listener", listener_name)]);
                    let ms = u64::try_from(start_instant.elapsed().as_millis()).unwrap_or(u64::MAX);
                    with_histogram!(tcp::DOWNSTREAM_CX_LENGTH_MS, record, ms, &[KeyValue::new("listener", listener_name)]);
                }

                let tcp_proxy = tcp_proxy.clone();
                let listener_name = tcp_proxy.listener_name;
                let server_config = config
                    .tls_configurator
                    .clone()
                    .map(TlsConfigurator::<ServerConfig, WantsToBuildServer>::into_inner);

                let (stream, _alpns): (Box<dyn AsyncReadWrite>, Option<AlpnCodecs>) =
                    if let Some(server_config) = server_config {
                        start_tls(listener_name, stream, server_config, None).await?
                    } else {
                        (Box::new(stream), None)
                    };

                debug!("Starting tcp proxy");
                let res = tcp_proxy.serve_connection(stream, downstream_metadata.clone()).await;
                debug!("TcpProxy closed {res:?}");
                res
            },
        }
    }
}

fn negotiate_codec_type<'a>(codec_type: CodecType, client_alpns: impl Iterator<Item = &'a [u8]>) -> Option<AlpnCodecs> {
    let client_alpns = client_alpns.collect::<Vec<_>>();
    AlpnCodecs::from_codec(codec_type)
        .iter()
        .find(|&&desired_proto| client_alpns.contains(&desired_proto.as_ref()))
        .copied()
}

async fn start_tls(
    listener_name: &'static str,
    stream: AsyncStream,
    mut config: ServerConfig,
    codec_type: Option<CodecType>,
) -> Result<(AsyncStream, Option<AlpnCodecs>)> {
    let acceptor = tokio_rustls::LazyConfigAcceptor::new(Acceptor::default(), stream);
    tokio::pin!(acceptor);
    match acceptor.as_mut().await {
        Ok(accepted) => {
            let client_hello = accepted.client_hello();
            let server_name = client_hello.server_name().unwrap_or("No Address").to_owned();
            debug!(
                "{listener_name} server_name {server_name} {codec_type:?} {:?}",
                client_hello
                    .alpn()
                    .map(|iter| iter.map(|i| String::from_utf8_lossy(i).into_owned()).collect::<Vec<String>>())
            );

            //note(hayley): here we use the CodecType (Http1, H2, Auto) to determine what alpn
            // we should offer. however, envoy also has a field commonTlsContext:  alpn_protocols: [h2,http/1.1]
            // in the TLS config. That one should probably take precedence.
            let negotiated_codec_type = match (codec_type, client_hello.alpn()) {
                (Some(desired), Some(offered)) => {
                    if let Some(negotiated_codec_type) = negotiate_codec_type(desired, offered) {
                        debug!("{listener_name} Negotiated codec type {negotiated_codec_type:?}");
                        // note(hayley): do we need to dynamically set this? inspecting the offer vs. desired is useful to log and configure the hyper server
                        //  but maybe we should set this at the listener level and let rustls handle it the handshake.
                        //  since the spec says that rustls has to send a specific error if the client offers only unsupported alpn
                        config.alpn_protocols = vec![negotiated_codec_type.as_ref().to_owned()];
                        Some(negotiated_codec_type)
                    } else {
                        // this error message could be better but is a bit of a refactor to get the names
                        warn!("Couldn't agree on a common codec");
                        // set our alpn reply to all the protocols we tried so Rustls can gracefully reject the hello.
                        config.alpn_protocols = AlpnCodecs::from_codec(desired)
                            .iter()
                            .map(|alpn| alpn.as_ref().to_owned())
                            .collect::<Vec<_>>();
                        None
                    }
                },
                (Some(desired), None) => {
                    warn!("Wanted to negotiate codec {desired:?} but client didn't offer any alpns");
                    // note(hayley):
                    //  the envoy docs state that ALPN is preferred when it is available, but if it is not
                    // protocol inference is used if the codec is set to auto
                    // since we pass in the Codec from the listener here, not the alpn config, we should accept this.
                    None
                },
                //nothing requested (i.e. tcp proxy), nothing offered
                //nothing requested, client offered alpn.
                (None, None | Some(_)) => None,
            };
            let stream = accepted.into_stream(Arc::new(config)).await.map_err(|e| format!("Can't accept {e:?}"))?;
            Ok((Box::new(stream), negotiated_codec_type))
        },
        Err(err) => Err(format!("{listener_name} Can't start tls {err:?}").into()),
    }
}

#[cfg(test)]
mod tests {
    use orion_configuration::config::listener::{FilterChainMatch, MatchResult};
    use orion_data_plane_api::{
        decode::from_yaml, envoy_data_plane_api::envoy::config::listener::v3::FilterChainMatch as EnvoyFilterChainMatch,
    };
    use std::net::Ipv4Addr;
    use tracing_test::traced_test;

    #[traced_test]
    #[test]
    fn filter_chain_match_empty_sni() {
        let m: EnvoyFilterChainMatch = from_yaml(
            r"
        server_names: []
        ",
        )
        .unwrap();
        let m: FilterChainMatch = m.try_into().unwrap();
        let dstport = 443;
        let sourceip = Ipv4Addr::new(127, 0, 0, 1).into();
        let srcport = 33000;
        assert_eq!(m.matches_destination_ip(sourceip), MatchResult::NoRule);
        assert_eq!(m.matches_source_ip(sourceip), MatchResult::NoRule);
        assert_eq!(m.matches_destination_port(dstport), MatchResult::NoRule);
        assert_eq!(m.matches_source_port(srcport), MatchResult::NoRule);
        assert_eq!(m.matches_server_name("host.test"), MatchResult::NoRule);
    }

    #[traced_test]
    #[test]
    fn filter_chain_match_ip_prefix() {
        let m: EnvoyFilterChainMatch =
            from_yaml("prefix_ranges: [{address_prefix: 192.168.0.0, prefix_len: 24}]").unwrap();
        let m: FilterChainMatch = m.try_into().unwrap();
        assert_eq!(m.matches_destination_ip(Ipv4Addr::new(192, 168, 0, 1).into()), MatchResult::Matched(8));
        assert_eq!(m.matches_destination_ip(Ipv4Addr::new(192, 168, 0, 255).into()), MatchResult::Matched(8));
        assert_eq!(m.matches_destination_ip(Ipv4Addr::new(192, 168, 1, 1).into()), MatchResult::FailedMatch);
        assert_eq!(m.matches_destination_ip(Ipv4Addr::new(172, 168, 0, 1).into()), MatchResult::FailedMatch);
        assert_eq!(m.matches_source_ip(Ipv4Addr::new(192, 168, 0, 1).into()), MatchResult::NoRule);
    }

    #[traced_test]
    #[test]
    fn filter_chain_wildcards() {
        let m: EnvoyFilterChainMatch = from_yaml(
            "
        server_names: [host.test, \"*.wildcard\"]
        destination_port: 443
        source_ports: [3300]
        prefix_ranges: [{address_prefix: 127.0.0.1, prefix_len: 32}]
        ",
        )
        .unwrap();
        let m: FilterChainMatch = m.try_into().unwrap();

        assert_eq!(m.matches_server_name("host.test"), MatchResult::Matched(0));
        assert_eq!(m.matches_server_name(""), MatchResult::FailedMatch);

        assert_eq!(m.matches_server_name("wildcard"), MatchResult::FailedMatch);
        assert_eq!(m.matches_server_name("shost.test"), MatchResult::FailedMatch);
        assert_eq!(m.matches_server_name("s.host.test"), MatchResult::FailedMatch);
        assert_eq!(m.matches_server_name("notawildcard"), MatchResult::FailedMatch);

        assert_eq!(m.matches_server_name("a.wildcard"), MatchResult::Matched(1));
        assert_eq!(m.matches_server_name("1.a.wildcard"), MatchResult::Matched(2));
        assert_eq!(m.matches_server_name("*.wildcard"), MatchResult::Matched(1));
    }
}
