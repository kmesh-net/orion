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

use super::{
    filterchain::{ConnectionHandler, FilterchainBuilder, FilterchainType},
    listeners_manager::TlsContextChange,
};
use crate::{
    secrets::{TlsConfigurator, WantsToBuildServer},
    transport::{bind_device::BindDevice, tls_inspector::TlsInspector},
    ConversionContext, Error, Result, RouteConfigurationChange,
};
use compact_str::{CompactString, ToCompactString};
use orion_configuration::config::listener::{FilterChainMatch, Listener as ListenerConfig, MatchResult};
use rustls::ServerConfig;
use std::{collections::HashMap, fmt::Debug, net::SocketAddr, sync::Arc};
use tokio::{
    net::{TcpListener, TcpSocket},
    sync::broadcast::{self},
};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
struct PartialListener {
    name: CompactString,
    socket_address: std::net::SocketAddr,
    bind_device: Option<BindDevice>,
    filter_chains: HashMap<FilterChainMatch, FilterchainBuilder>,
    with_tls_inspector: bool,
}
#[derive(Debug, Clone)]
pub struct ListenerFactory {
    listener: PartialListener,
}

impl TryFrom<ConversionContext<'_, ListenerConfig>> for PartialListener {
    type Error = Error;
    fn try_from(ctx: ConversionContext<'_, ListenerConfig>) -> std::result::Result<Self, Self::Error> {
        let ConversionContext { envoy_object: listener, secret_manager } = ctx;
        let name = listener.name.to_compact_string();
        let addr = listener.address;
        let with_tls_inspector = listener.with_tls_inspector;
        debug!("Listener {name} :TLS Inspector is {with_tls_inspector}");
        let filter_chains: HashMap<FilterChainMatch, _> = listener
            .filter_chains
            .into_iter()
            .map(|f| FilterchainBuilder::try_from(ConversionContext::new((f.1, secret_manager))).map(|x| (f.0, x)))
            .collect::<Result<_>>()?;
        let bind_device = listener.bind_device;

        if !with_tls_inspector {
            let has_server_names = filter_chains.keys().any(|m| !m.server_names.is_empty());
            if has_server_names {
                return Err((format!(
                    "Listener '{name}' has server_names in filter_chain_match, but no TLS inspector so matches would always fail"
                )).into());
            }
        }

        Ok(PartialListener { name, socket_address: addr, bind_device, filter_chains, with_tls_inspector })
    }
}

impl ListenerFactory {
    pub fn make_listener(
        self,
        route_updates_receiver: broadcast::Receiver<RouteConfigurationChange>,
        secret_updates_receiver: broadcast::Receiver<TlsContextChange>,
    ) -> Result<Listener> {
        let PartialListener { name, socket_address, bind_device, filter_chains, with_tls_inspector } = self.listener;

        let filter_chains = filter_chains
            .into_iter()
            .map(|fc| fc.1.with_listener_name(name.clone()).build().map(|x| (fc.0, x)))
            .collect::<Result<HashMap<_, _>>>()?;

        Ok(Listener {
            name,
            socket_address,
            bind_device,
            filter_chains,
            with_tls_inspector,
            route_updates_receiver,
            secret_updates_receiver,
        })
    }
}

impl TryFrom<ConversionContext<'_, ListenerConfig>> for ListenerFactory {
    type Error = Error;
    fn try_from(ctx: ConversionContext<'_, ListenerConfig>) -> std::result::Result<Self, Self::Error> {
        let listener = PartialListener::try_from(ctx)?;
        Ok(Self { listener })
    }
}

#[derive(Debug)]
pub struct Listener {
    name: CompactString,
    socket_address: std::net::SocketAddr,
    bind_device: Option<BindDevice>,
    pub filter_chains: HashMap<FilterChainMatch, FilterchainType>,
    with_tls_inspector: bool,
    route_updates_receiver: broadcast::Receiver<RouteConfigurationChange>,
    secret_updates_receiver: broadcast::Receiver<TlsContextChange>,
}

impl Listener {
    #[cfg(test)]
    pub(crate) fn test_listener(
        name: &str,
        route_rx: broadcast::Receiver<RouteConfigurationChange>,
        secret_rx: broadcast::Receiver<TlsContextChange>,
    ) -> Self {
        use std::net::{IpAddr, Ipv4Addr};
        Listener {
            name: name.into(),
            socket_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0),
            bind_device: None,
            filter_chains: HashMap::new(),
            with_tls_inspector: false,
            route_updates_receiver: route_rx,
            secret_updates_receiver: secret_rx,
        }
    }

    pub fn get_name(&self) -> &CompactString {
        &self.name
    }
    pub fn get_socket(&self) -> (&std::net::SocketAddr, Option<&BindDevice>) {
        (&self.socket_address, self.bind_device.as_ref())
    }

    pub async fn start(self) -> Error {
        let Self {
            name,
            socket_address: local_address,
            bind_device,
            filter_chains,
            with_tls_inspector,
            mut route_updates_receiver,
            mut secret_updates_receiver,
        } = self;
        let listener = match configure_and_start_tcp_listener(local_address, bind_device.as_ref()) {
            Ok(x) => x,
            Err(e) => return e,
        };
        info!("listener '{name}' started: {local_address}");
        let mut filter_chains = Arc::new(filter_chains);
        loop {
            tokio::select! {
                // here we accept a connection, and then start proccesing it.
                //  we spawn early so that we don't block other connections from being accepted due to a slow client
                maybe_stream = listener.accept() => {
                    match maybe_stream {
                        Ok((stream, peer_addr)) => {
                            let filter_chains = Arc::clone(&filter_chains);
                            let name = name.clone();
                            // spawn a seperate task for handling this client<->proxy connection
                            // we spawn before we know if we want to process this route because we might need to run the tls_inspector which could
                            // stall if the client is slow to send the ClientHello and end up blocking the acceptance of new connections
                            //
                            //  we could optimize a little here by either splitting up the filter_chain selection and rbac into the parts that can run
                            // before we have the ClientHello and the ones after. since we might already have enough info to decide to drop the connection
                            // or pick a specific filter_chain to run, or we could simply if-else on the with_tls_inspector variable.
                            tokio::spawn(Self::process_listener_update(name, filter_chains, with_tls_inspector, local_address, peer_addr, stream));
                        },
                        Err(e) => {warn!("failed to accept tcp connection: {e}");}
                    }
                },
                maybe_route_update = route_updates_receiver.recv() => {
                    //todo: add context to the error here once orion-error lands
                    match maybe_route_update {
                        Ok(route_update) => {Self::process_route_update(&name, &filter_chains, route_update);}
                        Err(e) => {return e.into();}
                    }
                },
                maybe_secret_update = secret_updates_receiver.recv() => {
                    match maybe_secret_update {
                        Ok(secret_update) => {
                            // todo: possibly expensive clone - may need to rethink this structure
                            let mut filter_chains_clone = filter_chains.as_ref().clone();
                            Self::process_secret_update(&name, &mut filter_chains_clone, secret_update);
                            filter_chains = Arc::new(filter_chains_clone);
                        }
                        Err(e) => {return e.into();}
                    }
                }
            }
        }
    }

    fn select_filterchain<'a, T>(
        filter_chains: &'a HashMap<FilterChainMatch, T>,
        source_addr: SocketAddr,
        destination_addr: SocketAddr,
        server_name: Option<&str>,
    ) -> Result<Option<&'a T>> {
        //todo: smallvec? other optimization?
        let mut possible_filters = vec![true; filter_chains.len()];
        let mut scratchpad = vec![MatchResult::NoRule; filter_chains.len()];

        fn match_subitem<'a, F: Fn(&FilterChainMatch, T) -> MatchResult, T: Copy>(
            function: F,
            comparand: T,
            iter: impl Iterator<Item = &'a FilterChainMatch>,
            scratchpad: &mut [MatchResult],
            possible_filters: &mut [bool],
        ) {
            let mut best_match = MatchResult::FailedMatch;
            // check all filters still in the running, skipping over those already eliminated
            for (i, match_config) in iter.enumerate().filter(|(i, _)| possible_filters[*i]) {
                let match_result = function(match_config, comparand);
                //mark the outcome of this iteration, and keep track of the best result
                scratchpad[i] = match_result;
                if match_result > best_match {
                    best_match = match_result;
                }
            }
            // now trim all the results that failed to match, or were less specific than the best match
            for i in 0..scratchpad.len() {
                if scratchpad[i] != best_match || scratchpad[i] == MatchResult::FailedMatch {
                    possible_filters[i] = false;
                }
            }
        }

        match_subitem(
            FilterChainMatch::matches_destination_port,
            destination_addr.port(),
            filter_chains.keys(),
            &mut scratchpad,
            &mut possible_filters,
        );

        match_subitem(
            FilterChainMatch::matches_destination_ip,
            destination_addr.ip(),
            filter_chains.keys(),
            &mut scratchpad,
            &mut possible_filters,
        );

        match_subitem(
            FilterChainMatch::matches_server_name,
            server_name.unwrap_or_default(),
            filter_chains.keys(),
            &mut scratchpad,
            &mut possible_filters,
        );

        match_subitem(
            FilterChainMatch::matches_source_ip,
            source_addr.ip(),
            filter_chains.keys(),
            &mut scratchpad,
            &mut possible_filters,
        );

        match_subitem(
            FilterChainMatch::matches_source_port,
            source_addr.port(),
            filter_chains.keys(),
            &mut scratchpad,
            &mut possible_filters,
        );

        let mut possible_filters = possible_filters
            .into_iter()
            .zip(filter_chains.iter())
            .filter_map(|(include, item)| include.then_some(item.1));

        let first_match = possible_filters.next();
        if possible_filters.next().is_some() {
            Err("multiple filterchains matched a single connection. This is a bug in orion!".into())
        } else {
            Ok(first_match)
        }
    }

    async fn process_listener_update(
        listener_name: CompactString,
        filter_chains: Arc<HashMap<FilterChainMatch, FilterchainType>>,
        with_tls_inspector: bool,
        local_address: SocketAddr,
        peer_addr: SocketAddr,
        mut stream: tokio::net::TcpStream,
    ) -> Result<()> {
        let server_name = if with_tls_inspector {
            let sni = TlsInspector::peek_sni(&mut stream).await;
            if let Some(sni) = sni.as_ref() {
                debug!("{listener_name} : Detected TLS server name: {sni}");
            } else {
                debug!("{listener_name} : No TLS server name detected");
            }
            sni
        } else {
            None
        };

        let selected_filterchain =
            Self::select_filterchain(&filter_chains, peer_addr, local_address, server_name.as_deref())?;
        if let Some(filterchain) = selected_filterchain {
            debug!(
                "{listener_name} : mapping connection from {peer_addr} to filter chain {}",
                filterchain.filter_chain().name
            );
            if let Some(stream) = filterchain.apply_rbac(stream, local_address, peer_addr, server_name.as_deref()) {
                return filterchain.start_filterchain(stream).await;
            } else {
                debug!("{listener_name} : dropped connection from {peer_addr} due to rbac");
            }
        } else {
            warn!("{listener_name} : No match for {peer_addr} {local_address}");
        }
        Ok(())
    }

    //could secrets and routes also be updated through a CachedWatch?
    // they only need to be updated when they're read after all and could work with
    fn process_secret_update(
        listener_name: &str,
        filter_chains: &mut HashMap<FilterChainMatch, FilterchainType>,
        secret_update: TlsContextChange,
    ) {
        match secret_update {
            TlsContextChange::Updated((secret_id, secret)) => {
                for chain in filter_chains.values_mut() {
                    let filterchain = &mut chain.config;
                    if let Some(tls_configurator) = filterchain.tls_configurator.clone() {
                        let maybe_configurator = TlsConfigurator::<ServerConfig, WantsToBuildServer>::update(
                            tls_configurator,
                            &secret_id,
                            secret.clone(),
                        );
                        if let Ok(new_tls_configurator) = maybe_configurator {
                            filterchain.tls_configurator = Some(new_tls_configurator);
                        } else {
                            let msg = format!(
                                "{listener_name} Couldn't update a secret for filterchain {} {:?}",
                                filterchain.name,
                                maybe_configurator.err()
                            );
                            warn!("{msg}");
                        }
                    }
                }
            },
        }
    }

    fn process_route_update(
        listener_name: &str,
        filter_chains: &HashMap<FilterChainMatch, FilterchainType>,
        route_update: RouteConfigurationChange,
    ) {
        match route_update {
            RouteConfigurationChange::Added((id, route)) => {
                for chain in filter_chains.values() {
                    if let ConnectionHandler::Http(http_manager) = &chain.handler {
                        let route_id = http_manager.get_route_id();
                        if let Some(route_id) = route_id {
                            if route_id == id {
                                debug!("{listener_name} Route updated {id} {route:?}");
                                http_manager.update_route(Arc::new(route.clone()));
                            }
                        } else {
                            debug!("{listener_name} Got route update but id doesn't match {route_id:?} {id}");
                        }
                    }
                }
            },
            RouteConfigurationChange::Removed(id) => {
                for chain in filter_chains.values() {
                    if let ConnectionHandler::Http(http_manager) = &chain.handler {
                        if let Some(route_id) = http_manager.get_route_id() {
                            if route_id == id {
                                http_manager.remove_route();
                            }
                        }
                    }
                }
            },
        }
    }
}

fn configure_and_start_tcp_listener(addr: SocketAddr, device: Option<&BindDevice>) -> Result<TcpListener> {
    let socket = if addr.is_ipv4() { TcpSocket::new_v4()? } else { TcpSocket::new_v6()? };
    socket.set_reuseaddr(true)?;
    socket.set_keepalive(true)?;

    if let Some(device) = device {
        crate::transport::bind_device::bind_device(&socket, device)?;
    }

    #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
    socket.set_reuseport(true)?;
    socket.bind(addr)?;

    Ok(socket.listen(128)?)
}

#[cfg(test)]
mod tests {
    use orion_configuration::config::listener::{FilterChainMatch as FilterChainMatchConfig, ServerNameMatch};
    use orion_data_plane_api::decode::from_yaml;
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::listener::v3::FilterChainMatch as EnvoyFilterChainMatch;

    use crate::SecretManager;

    use super::*;
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::listener::v3::Listener as EnvoyListener;

    use std::net::Ipv4Addr;
    use std::str::FromStr;
    use tracing_test::traced_test;

    #[test]
    fn listener_bind_device() {
        const LISTENER: &str = r#"
name: listener_https
address:
  socket_address: { address: 0.0.0.0, port_value: 8443 }
filter_chains:
  - name: filter_chain
    filters:
      - name: https_gateway
        typedConfig:
          "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
          codec_type: HTTP1
          stat_prefix: http
          httpFilters:
          - name: envoy.filters.http.router
            typed_config:
              "@type": type.googleapis.com/envoy.extensions.filters.http.router.v3.Router
              start_child_span: false
          route_config:
            name: basic_https_route
            virtual_hosts:
              - name: backend_https
                domains: ["*"]
socket_options:
  - description: "bind to interface virt1"
    level: 1
    name: 25
    # utf8 string 'virt1' bytes encoded as base64
    buf_value: dmlydDE=
"#;

        let envoy_listener: EnvoyListener = from_yaml(LISTENER).unwrap();
        let listener = envoy_listener.try_into().unwrap();
        let secrets_manager = SecretManager::new();
        let ctx = ConversionContext::new((listener, &secrets_manager));
        let l = PartialListener::try_from(ctx).unwrap();
        let expected_bind_device = Some(BindDevice::from_str("virt1").unwrap());

        assert_eq!(&l.bind_device, &expected_bind_device);
    }

    #[test]
    fn match_fallback_sni() {
        let fcm = [
            (
                FilterChainMatch {
                    destination_port: None,
                    destination_prefix_ranges: Vec::new(),
                    server_names: vec![
                        ServerNameMatch::from_str("host1.test").unwrap(),
                        ServerNameMatch::from_str("host2.test").unwrap(),
                    ],
                    source_prefix_ranges: Vec::new(),
                    source_ports: Vec::new(),
                },
                0,
            ),
            (FilterChainMatch::default(), 1),
        ];
        let hashmap: HashMap<_, _> = fcm.iter().cloned().collect();
        let srcaddr = (Ipv4Addr::new(127, 0, 0, 1), 33000).into();
        let selected =
            Listener::select_filterchain(&hashmap, srcaddr, (Ipv4Addr::LOCALHOST, 8443).into(), None).unwrap();
        assert_eq!(selected.copied(), Some(1));
    }

    #[traced_test]
    #[test]
    fn sni_match_without_inspector_fails() {
        const LISTENER: &str = r#"
name: listener_https
address:
  socket_address: { address: 0.0.0.0, port_value: 8443 }
filter_chains:
  - name: filter_chain_https1
    filter_chain_match:
      server_names: [hostname.example]
    filters:
      - name: https_gateway
        typedConfig:
          "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
          codec_type: HTTP1
          stat_prefix: http
          httpFilters:
            - name: envoy.filters.http.router
              typed_config:
                "@type": type.googleapis.com/envoy.extensions.filters.http.router.v3.Router
                start_child_span: false
          route_config:
            name: basic_https_route
            virtual_hosts:
              - name: backend_https
                domains: ["*"]
"#;

        let envoy_listener: EnvoyListener = from_yaml(LISTENER).unwrap();
        let listener = envoy_listener.try_into().unwrap();
        let secrets_man = SecretManager::new();

        let conv = ConversionContext { envoy_object: listener, secret_manager: &secrets_man };
        let r = PartialListener::try_from(conv);
        let err = r.unwrap_err();
        assert!(err
            .to_string()
            .contains("has server_names in filter_chain_match, but no TLS inspector so matches would always fail"));
    }

    #[traced_test]
    #[test]
    fn filter_chain_multiple() {
        let m: EnvoyFilterChainMatch = from_yaml(
            "
        server_names: [host.test, \"*.wildcard\"]
        destination_port: 443
        source_ports: [3300]
        prefix_ranges: [{address_prefix: 127.0.0.1, prefix_len: 32}]
        ",
        )
        .unwrap();
        let m: HashMap<FilterChainMatch, _> = std::iter::once((m.try_into().unwrap(), ())).collect();
        let good_source = (Ipv4Addr::LOCALHOST, 3300).into();
        let good_destination = (Ipv4Addr::LOCALHOST, 443).into();
        let good_host = Some("host.test");
        assert!(matches!(Listener::select_filterchain(&m, good_source, good_destination, good_host), Ok(Some(_))));
        assert!(matches!(
            Listener::select_filterchain(&m, good_source, good_destination, Some("a.wildcard")),
            Ok(Some(_))
        ));
        assert!(matches!(Listener::select_filterchain(&m, good_source, good_destination, None), Ok(None)));
        assert!(matches!(
            Listener::select_filterchain(&m, good_source, (Ipv4Addr::LOCALHOST, 444).into(), good_host),
            Ok(None)
        ));
    }

    #[test]
    fn most_specific_wins() {
        let l: EnvoyListener = from_yaml(
            "
        name: listener
        filter_chains:
        - filter_chain_match:
            server_names: [this.is.more.specific]
        - filter_chain_match:
            server_names: [\"*.more.specific\"]
        - filter_chain_match:
            server_names: [\"*.specific\"]
        - filter_chain_match:
            server_names: []
        ",
        )
        .unwrap();
        //     let listener : Listener = l.try_into().unwrap();
        let m = l
            .filter_chains
            .into_iter()
            .enumerate()
            .map(|(i, fc)| {
                fc.filter_chain_match
                    .map(FilterChainMatchConfig::try_from)
                    .transpose()
                    .map(|x| (x.unwrap_or_default(), i))
            })
            .collect::<std::result::Result<HashMap<_, _>, _>>()
            .unwrap();
        let srcaddr = (Ipv4Addr::new(127, 0, 0, 1), 33000).into();
        let dst = (Ipv4Addr::LOCALHOST, 8443).into();
        assert_eq!(Listener::select_filterchain(&m, srcaddr, dst, None).unwrap().copied(), Some(3));
        assert_eq!(
            Listener::select_filterchain(&m, srcaddr, dst, Some("this.is.more.specific")).unwrap().copied(),
            Some(0)
        );
        assert_eq!(
            Listener::select_filterchain(&m, srcaddr, dst, Some("not.this.is.more.specific")).unwrap().copied(),
            Some(1)
        );
        assert_eq!(Listener::select_filterchain(&m, srcaddr, dst, Some("is.more.specific")).unwrap().copied(), Some(1));

        assert_eq!(Listener::select_filterchain(&m, srcaddr, dst, Some("more.specific")).unwrap().copied(), Some(2));
        assert_eq!(
            Listener::select_filterchain(&m, srcaddr, dst, Some("this.is.less.specific")).unwrap().copied(),
            Some(2)
        );

        assert_eq!(Listener::select_filterchain(&m, srcaddr, dst, Some("hello.world")).unwrap().copied(), Some(3));
    }
}
