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

use super::AdminState;
use axum::{extract::State, response::Json};
use orion_configuration::config::{
    cluster::{ClusterDiscoveryType, LocalityLbEndpoints as LocalityLbEndpointsConfig},
    core::DataSource,
    listener::MainFilter,
    network_filters::http_connection_manager::RouteSpecifier,
    secret::{Secret, Type},
};
use orion_lib::{
    clusters::clusters_manager::get_all_clusters, ConfigDump, ConfigurationSenders, ListenerConfigurationChange,
};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::xds_configurator::send_change_to_runtimes;

pub fn redact_secrets(secrets: Vec<Secret>) -> Vec<Secret> {
    secrets
        .into_iter()
        .map(|mut secret| {
            match secret.kind_mut() {
                Type::TlsCertificate(tls_certificate) => {
                    *tls_certificate.private_key_mut() = DataSource::InlineString("[redacted]".into());
                },
                Type::ValidationContext(_) => {},
            }
            secret
        })
        .collect()
}

pub async fn get_config_dump(State(admin_state): State<AdminState>) -> Json<Value> {
    // Unwrap listeners and routes configuration channels
    let mut listeners_senders = Vec::with_capacity(admin_state.configuration_senders.len());
    for ConfigurationSenders { listener_configuration_sender, .. } in admin_state.configuration_senders {
        listeners_senders.push(listener_configuration_sender);
    }

    // Create config_dump channels to send to components so they can send back their config
    let (config_dump_sender, mut config_dump_receiver) = mpsc::channel::<ConfigDump>(100);
    let mut config = ConfigDump { bootstrap: Some(admin_state.bootstrap.clone()), ..Default::default() };

    // Retrieve active listers configuration
    let change = ListenerConfigurationChange::GetConfiguration(config_dump_sender.clone());
    let _ = send_change_to_runtimes(&listeners_senders, change).await;
    if let Some(listeners_config) = config_dump_receiver.recv().await {
        config.listeners = listeners_config.listeners;
        // Extract and flatten routes from listeners
        let routes_flattened: Vec<RouteSpecifier> = config.listeners.as_ref().map_or_else(Vec::new, |listeners| {
            listeners
                .iter()
                .flat_map(|listener| listener.filter_chains.values())
                .filter_map(|filter_chain| match &filter_chain.terminal_filter {
                    MainFilter::Http(hcm) => Some(hcm.route_specifier.clone()),
                    MainFilter::Tcp(_) => None,
                })
                .collect()
        });

        if !routes_flattened.is_empty() {
            config.routes = Some(routes_flattened);
        }
    }

    let clusters = get_all_clusters();
    config.clusters = (!clusters.is_empty()).then_some(clusters.clone());

    let endpoints: Vec<LocalityLbEndpointsConfig> = clusters
        .iter()
        .flat_map(|cluster| match &cluster.discovery_settings {
            ClusterDiscoveryType::Static(load_assignment)
            | ClusterDiscoveryType::Eds(Some(load_assignment))
            | ClusterDiscoveryType::StrictDns(load_assignment) => load_assignment.endpoints.clone(),
            ClusterDiscoveryType::Eds(None) | ClusterDiscoveryType::OriginalDst(_) => vec![],
        })
        .collect();
    config.endpoints = (!endpoints.is_empty()).then_some(endpoints);

    let secrets: Vec<Secret> = redact_secrets(admin_state.secret_manager.read().get_all_secrets());
    config.secrets = (!secrets.is_empty()).then_some(secrets);

    Json(json!(config))
}

#[cfg(test)]
mod config_dump_tests {
    use super::{super::*, *};
    use axum_test::TestServer;
    use compact_str::CompactString;
    use orion_configuration::config::{
        core::DataSource,
        network_filters::http_connection_manager::header_modifer::HeaderModifier,
        secret::{Secret, TlsCertificate, Type, ValidationContext},
        Bootstrap, Listener,
    };
    use orion_lib::{ConfigDump, ListenerConfigurationChange};
    use parking_lot::RwLock;
    use serde_json::json;
    use std::{sync::Arc, time::Instant};
    use tokio::sync::mpsc;

    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        config::core::v3::{data_source::Specifier::InlineString, DataSource as EnvoyDataSource},
        extensions::transport_sockets::tls::v3::{
            CertificateValidationContext as EnvoyCertificateValidationContext, TlsCertificate as EnvoyTlsCertificate,
        },
    };
    use tokio::task::JoinHandle;

    fn spawn_mock_listener_manager(mock_listeners: Option<Vec<Listener>>) -> (ConfigurationSenders, JoinHandle<()>) {
        let (list_tx, mut list_rx) = mpsc::channel(10);
        let (route_tx, _route_rx) = mpsc::channel(10);
        let handle = tokio::spawn(async move {
            while let Some(message) = list_rx.recv().await {
                if let ListenerConfigurationChange::GetConfiguration(response_sender) = message {
                    let config = ConfigDump { listeners: mock_listeners.clone(), ..Default::default() };
                    let _ = response_sender.send(config).await;
                }
            }
        });
        (ConfigurationSenders { listener_configuration_sender: list_tx, route_configuration_sender: route_tx }, handle)
    }

    #[test]
    fn test_redact_secrets_tls_certificate() {
        let secret = Secret {
            name: CompactString::from("test_tls"),
            kind: Type::TlsCertificate(
                TlsCertificate::try_from(EnvoyTlsCertificate {
                    certificate_chain: Some(EnvoyDataSource {
                        specifier: Some(InlineString("cert_data".into())),
                        ..Default::default()
                    }),
                    private_key: Some(EnvoyDataSource {
                        specifier: Some(InlineString("private_data".into())),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .unwrap(),
            ),
        };
        let redacted = redact_secrets(vec![secret.clone()]);
        match &redacted[0].kind {
            Type::TlsCertificate(tls) => {
                assert_eq!(tls.private_key(), &DataSource::InlineString("[redacted]".into()));
                assert_eq!(tls.certificate_chain(), &DataSource::InlineString("cert_data".into()));
            },
            Type::ValidationContext(_) => unreachable!(),
        }
    }

    #[test]
    fn test_redact_secrets_validation_context() {
        let secret = Secret {
            name: CompactString::from("test_validation"),
            kind: Type::ValidationContext(
                ValidationContext::try_from(EnvoyCertificateValidationContext {
                    trusted_ca: Some(EnvoyDataSource {
                        specifier: Some(InlineString("ca_data".into())),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .unwrap(),
            ),
        };
        let redacted = redact_secrets(vec![secret.clone()]);
        match &redacted[0].kind {
            Type::ValidationContext(vc) => {
                assert_eq!(vc.trusted_ca(), &DataSource::InlineString("ca_data".into()));
            },
            Type::TlsCertificate(_) => unreachable!(),
        }
    }

    #[tokio::test]
    async fn config_dump_bootstrap() {
        use orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::{
            address::Address as EnvoyAddress, socket_address::PortSpecifier, Address as EnvoyOuterAddress,
            SocketAddress as EnvoySocketAddress,
        };
        use serde_json::json;
        let envoy_sock_addr = EnvoySocketAddress {
            address: "127.0.0.1".to_owned(),
            port_specifier: Some(PortSpecifier::PortValue(12345)),
            ..Default::default()
        };
        let envoy_addr = EnvoyAddress::SocketAddress(envoy_sock_addr);
        let envoy_outer_addr = EnvoyOuterAddress { address: Some(envoy_addr) };
        let bootstrap = Bootstrap {
            admin: Some(orion_configuration::config::bootstrap::Admin {
                address: envoy_outer_addr.try_into().unwrap(),
            }),
            ..Default::default()
        };
        let (configuration_senders, handle) = spawn_mock_listener_manager(None);
        let admin_state = AdminState {
            bootstrap: bootstrap.clone(),
            configuration_senders: vec![configuration_senders],
            secret_manager: Arc::new(RwLock::new(orion_lib::SecretManager::default())),
            server_info: ServerInfo::default(),
            server_startup: Instant::now(),
        };
        let app = build_admin_router(admin_state);
        let server = TestServer::new(app).unwrap();
        let response = server.get("/config_dump").await;
        response.assert_status_ok();
        let value: serde_json::Value = response.json();
        assert_eq!(value["bootstrap"], json!(bootstrap));
        handle.abort();
    }

    #[tokio::test]
    async fn config_dump_listeners_and_routes() {
        use compact_str::CompactString;
        use orion_configuration::config::{
            listener::{FilterChain, FilterChainMatch, Listener, MainFilter},
            network_filters::http_connection_manager::{
                route::{Action, RouteMatch},
                CodecType, HttpConnectionManager, Route, RouteConfiguration, RouteSpecifier, VirtualHost, XffSettings,
            },
        };
        use std::{
            collections::HashMap,
            net::{IpAddr, Ipv4Addr, SocketAddr},
            time::Duration,
        };
        let listener = Listener {
            name: CompactString::from("listener1"),
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080),
            filter_chains: {
                let mut map = HashMap::new();
                map.insert(
                    FilterChainMatch::default(),
                    FilterChain {
                        name: CompactString::from("fc1"),
                        filter_chain_match_hash: 0,
                        tls_config: None,
                        rbac: vec![],
                        terminal_filter: MainFilter::Http(HttpConnectionManager {
                            codec_type: CodecType::Http1,
                            request_timeout: Some(Duration::from_secs(10)),
                            http_filters: vec![],
                            enabled_upgrades: vec![],
                            route_specifier: RouteSpecifier::RouteConfig(RouteConfiguration {
                                name: CompactString::from("route_config1"),
                                most_specific_header_mutations_wins: false,
                                response_header_modifier: HeaderModifier::default(),
                                request_headers_to_add: vec![],
                                request_headers_to_remove: vec![],
                                virtual_hosts: vec![VirtualHost {
                                    name: CompactString::from("vh1"),
                                    domains: vec![],
                                    routes: vec![Route {
                                        name: "test_route".to_owned(),
                                        response_header_modifier: HeaderModifier::default(),
                                        request_headers_to_add: vec![],
                                        request_headers_to_remove: vec![],
                                        route_match: RouteMatch::default(),
                                        typed_per_filter_config: HashMap::new(),
                                        action: Action::DirectResponse(
                                            orion_configuration::config::network_filters::http_connection_manager::route::DirectResponseAction {
                                                status: http::StatusCode::OK,
                                                body: None,
                                            }
                                        ),
                                    }],
                                    response_header_modifier: HeaderModifier::default(),
                                    request_headers_to_add: vec![],
                                    request_headers_to_remove: vec![],
                                    retry_policy: None,
                                }],
                            }),
                            access_log: vec![],
                            xff_settings: XffSettings { use_remote_address: true, skip_xff_append: false, xff_num_trusted_hops: 0 },
                            generate_request_id: false,
                            preserve_external_request_id: false,
                            always_set_request_id_in_response: false,
                            tracing: None,
                        }),
                    },
                );
                map
            },
            bind_device: None,
            proxy_protocol_config: None,
            with_tls_inspector: false,
        };
        let (configuration_senders, handle) = spawn_mock_listener_manager(Some(vec![listener]));
        let admin_state = AdminState {
            bootstrap: Bootstrap::default(),
            configuration_senders: vec![configuration_senders],
            secret_manager: Arc::new(RwLock::new(orion_lib::SecretManager::default())),
            server_info: ServerInfo::default(),
            server_startup: Instant::now(),
        };
        let app = build_admin_router(admin_state);
        let server = TestServer::new(app).unwrap();
        let response = server.get("/config_dump").await;
        response.assert_status_ok();
        let value: serde_json::Value = response.json();
        assert_eq!(value["listeners"][0]["name"], "listener1");
        assert_eq!(value["routes"][0]["name"], "route_config1");
        handle.abort();
    }

    #[tokio::test]
    async fn config_dump_clusters() {
        use compact_str::CompactString;
        use orion_configuration::config::{
            cluster::{
                Cluster, ClusterDiscoveryType, ClusterLoadAssignment, HealthStatus, HttpProtocolOptions, LbEndpoint,
                LbPolicy, LocalityLbEndpoints,
            },
            core::envoy_conversions::Address,
        };
        use std::{num::NonZeroU32, time::Duration};
        let endpoint_addr = Address::Socket("127.0.0.1".to_owned(), 9000);
        let cluster = Cluster {
            name: CompactString::from("cluster1"),
            discovery_settings: ClusterDiscoveryType::Static(ClusterLoadAssignment {
                endpoints: vec![LocalityLbEndpoints {
                    priority: 0,
                    lb_endpoints: vec![LbEndpoint {
                        address: endpoint_addr,
                        health_status: HealthStatus::default(),
                        load_balancing_weight: NonZeroU32::new(1).unwrap(),
                    }],
                }],
            }),
            transport_socket: None,
            bind_device: None,
            load_balancing_policy: LbPolicy::default(),
            http_protocol_options: HttpProtocolOptions::default(),
            health_check: None,
            connect_timeout: Some(Duration::from_secs(5)),
            cleanup_interval: None,
        };
        let secret_manager = orion_lib::SecretManager::default();
        let partial_cluster =
            orion_lib::clusters::cluster::PartialClusterType::try_from((cluster.clone(), &secret_manager)).unwrap();
        let _ = orion_lib::clusters::clusters_manager::add_cluster(partial_cluster);
        let (configuration_senders, handle) = spawn_mock_listener_manager(None);
        let admin_state = AdminState {
            bootstrap: Bootstrap::default(),
            configuration_senders: vec![configuration_senders],
            secret_manager: Arc::new(RwLock::new(secret_manager)),
            server_info: ServerInfo::default(),
            server_startup: Instant::now(),
        };
        let app = build_admin_router(admin_state);
        let server = TestServer::new(app).unwrap();
        let response = server.get("/config_dump").await;
        response.assert_status_ok();
        let value: serde_json::Value = response.json();
        assert_eq!(value["clusters"][0]["name"], "cluster1");
        handle.abort();
    }

    #[tokio::test]
    async fn config_dump_endpoints() {
        use compact_str::CompactString;
        use orion_configuration::config::{
            cluster::{
                Cluster, ClusterDiscoveryType, ClusterLoadAssignment, HealthStatus, HttpProtocolOptions, LbEndpoint,
                LbPolicy, LocalityLbEndpoints,
            },
            core::envoy_conversions::Address,
        };
        use std::{num::NonZeroU32, time::Duration};
        let endpoint_addr = Address::Socket("127.0.0.1".to_owned(), 9000);
        let cluster = Cluster {
            name: CompactString::from("cluster1"),
            discovery_settings: ClusterDiscoveryType::Static(ClusterLoadAssignment {
                endpoints: vec![LocalityLbEndpoints {
                    priority: 0,
                    lb_endpoints: vec![LbEndpoint {
                        address: endpoint_addr,
                        health_status: HealthStatus::default(),
                        load_balancing_weight: NonZeroU32::new(1).unwrap(),
                    }],
                }],
            }),
            transport_socket: None,
            bind_device: None,
            load_balancing_policy: LbPolicy::default(),
            http_protocol_options: HttpProtocolOptions::default(),
            health_check: None,
            connect_timeout: Some(Duration::from_secs(5)),
            cleanup_interval: None,
        };
        let secret_manager = orion_lib::SecretManager::default();
        let partial_cluster =
            orion_lib::clusters::cluster::PartialClusterType::try_from((cluster.clone(), &secret_manager)).unwrap();
        let _ = orion_lib::clusters::clusters_manager::add_cluster(partial_cluster);
        let (configuration_senders, handle) = spawn_mock_listener_manager(None);
        let admin_state = AdminState {
            bootstrap: Bootstrap::default(),
            configuration_senders: vec![configuration_senders],
            secret_manager: Arc::new(RwLock::new(secret_manager)),
            server_info: ServerInfo::default(),
            server_startup: Instant::now(),
        };
        let app = build_admin_router(admin_state);
        let server = TestServer::new(app).unwrap();
        let response = server.get("/config_dump").await;
        response.assert_status_ok();
        let value: serde_json::Value = response.json();
        let address = value["endpoints"][0]["lb_endpoints"][0]["Socket"].to_string();
        assert_eq!(address, "[\"127.0.0.1\",9000]");
        handle.abort();
    }

    #[tokio::test]
    async fn config_dump_secrets() {
        use compact_str::CompactString;
        use orion_configuration::config::secret::{Secret, TlsCertificate, Type, ValidationContext};
        use std::fs;
        let mut secret_manager = orion_lib::SecretManager::new();

        // Find the project root by looking for Cargo.toml
        let mut project_root = std::env::current_dir().unwrap();
        while !project_root.join("Cargo.lock").exists() {
            project_root = project_root.parent().unwrap().to_path_buf();
        }

        // Read certificate files from test_certs directory
        let cert_pem =
            fs::read_to_string(project_root.join("test_certs/beefcakeCA-gathered/beefcake-dublin.cert.pem")).unwrap();
        let key_pem =
            fs::read_to_string(project_root.join("test_certs/beefcakeCA-gathered/beefcake-dublin.key.pem")).unwrap();
        let ca_pem = fs::read_to_string(
            project_root.join("test_certs/beefcakeCA-gathered/beefcake.intermediate.ca-chain.cert.pem"),
        )
        .unwrap();

        let tls_secret = Secret {
            name: CompactString::from("beefcake_dublin"),
            kind: Type::TlsCertificate(
                TlsCertificate::try_from(
                    orion_data_plane_api::envoy_data_plane_api::envoy::extensions::transport_sockets::tls::v3::TlsCertificate {
                        certificate_chain: Some(orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::DataSource {
                            specifier: Some(
                                orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::data_source::Specifier::InlineString(
                                    cert_pem,
                                ),
                            ),
                            ..Default::default()
                        }),
                        private_key: Some(orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::DataSource {
                            specifier: Some(
                                orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::data_source::Specifier::InlineString(
                                    key_pem,
                                ),
                            ),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .unwrap(),
            ),
        };
        let validation_secret = Secret {
            name: CompactString::from("beefcake_ca"),
            kind: Type::ValidationContext(
                ValidationContext::try_from(
                    orion_data_plane_api::envoy_data_plane_api::envoy::extensions::transport_sockets::tls::v3::CertificateValidationContext {
                        trusted_ca: Some(orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::DataSource {
                            specifier: Some(
                                orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::data_source::Specifier::InlineString(
                                    ca_pem.clone(),
                                ),
                            ),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .unwrap(),
            ),
        };
        let _ = secret_manager.add(&tls_secret).unwrap();
        let _ = secret_manager.add(&validation_secret).unwrap();
        let (configuration_senders, handle) = spawn_mock_listener_manager(None);
        let admin_state = AdminState {
            bootstrap: Bootstrap::default(),
            configuration_senders: vec![configuration_senders],
            secret_manager: Arc::new(RwLock::new(secret_manager)),
            server_info: ServerInfo::default(),
            server_startup: Instant::now(),
        };
        let app = build_admin_router(admin_state);
        let server = TestServer::new(app).unwrap();
        let response = server.get("/config_dump").await;
        response.assert_status_ok();
        let value: serde_json::Value = response.json();
        assert_eq!(value["secrets"][0]["name"], "beefcake_dublin");
        assert_eq!(value["secrets"][1]["name"], "beefcake_ca");
        // Check redaction
        assert_eq!(value["secrets"][0]["tls_certificate"]["private_key"]["inline_string"], "[redacted]");
        assert_eq!(value["secrets"][1]["validation_context"]["trusted_ca"]["inline_string"], ca_pem);
        handle.abort();
    }
}
