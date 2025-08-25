use envoy_data_plane_api::envoy::config::bootstrap::v3::Bootstrap;
use envoy_data_plane_api::envoy::config::core::v3::socket_address::PortSpecifier;
use envoy_data_plane_api::envoy::config::core::v3::{address, Address, SocketAddress};
use envoy_data_plane_api::envoy::config::endpoint::v3::lb_endpoint::HostIdentifier;
use orion_data_plane_api::bootstrap_loader::bootstrap::{BootstrapLoader, BootstrapResolver, XdsConfig, XdsType};
use orion_data_plane_api::decode::from_yaml;
use orion_data_plane_api::xds::model::TypeUrl;
use std::collections::HashSet;
use std::path::PathBuf;

#[test]
fn read_static_resource() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("bootstrap_with_http_connection_manager.yml");

    let loader = BootstrapLoader::load(path.into_os_string().into_string().unwrap());
    let listeners = loader.get_static_listener_configs().unwrap();
    let listener = listeners.first().unwrap();
    assert_eq!(listener.name, "listener_0".to_string());

    let routes = loader.get_static_route_configs().unwrap();
    assert_eq!(routes.len(), 0);

    let mut clusters = loader.get_static_cluster_configs().unwrap();
    assert_eq!(clusters.len(), 3);

    let cluster = clusters.drain(..).next().unwrap();
    let orion_data_plane_api::envoy_data_plane_api::envoy::config::cluster::v3::Cluster { load_assignment, .. } =
        cluster;

    let endpoints = load_assignment.unwrap().endpoints.drain(..).next().unwrap().lb_endpoints;
    let endpoint = endpoints.first().unwrap();
    let Some(HostIdentifier::Endpoint(ref ept_any)) = endpoint.host_identifier else {
        panic!("None valid endpoint");
    };

    let Some(Address { address: ept_addr_any }) = ept_any.clone().address else {
        panic!("No valid address from endpoint");
    };
    let Some(address::Address::SocketAddress(ept_socket_addr)) = ept_addr_any else {
        panic!("No valid socket address from endpoint");
    };
    assert_eq!(ept_socket_addr.address, "127.0.0.1");
    let Some(PortSpecifier::PortValue(port)) = ept_socket_addr.port_specifier else {
        panic!("No valid port value from endpoint");
    };
    assert_eq!(port, 5678);
}

#[test]
fn read_dynamic_resource() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("bootstrap_with_http_connection_manager.yml");

    let loader = BootstrapLoader::load(path.into_os_string().into_string().unwrap());
    let xds_configs = loader.get_xds_configs().unwrap();
    assert_eq!(xds_configs.len(), 1);
    assert_eq!(
        xds_configs[0],
        XdsConfig(
            XdsType::Individual(TypeUrl::RouteConfiguration),
            SocketAddress {
                protocol: 0,
                address: "127.0.0.1".to_string(),
                ipv4_compat: false,
                port_specifier: Some(PortSpecifier::PortValue(5678)),
                resolver_name: String::new(),
                network_namespace_filepath: String::new(),
            }
        )
    );
}

#[test]
fn read_ads_config() {
    const ADS_BOOTSTRAP: &str = r#"
dynamic_resources:
  ads_config:
    api_type: GRPC
    transport_api_version: V3
    grpc_services:
    - envoy_grpc:
        cluster_name: ads_cluster
  lds_config:
    ads: {}
  cds_config:
    ads: {}
static_resources:
  listeners:
    - name: listener_0
      address:
        socket_address: { address: 127.0.0.1, port_value: 10000 }
  clusters:
    - name: ads_cluster
      connect_timeout: 0.25s
      type: STATIC
      lb_policy: ROUND_ROBIN
      typed_extension_protocol_options:
        envoy.extensions.upstreams.http.v3.HttpProtocolOptions:
          "@type": type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions
          explicit_http_config:
            http2_protocol_options:
              # Configure an HTTP/2 keep-alive to detect connection issues and reconnect
              # to the admin server if the connection is no longer responsive.
              connection_keepalive:
                interval: 30s
                timeout: 5s
      load_assignment:
        cluster_name: ads_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: 127.0.0.1
                      port_value: 5679
    "#;
    let bootstrap: Bootstrap = from_yaml(ADS_BOOTSTRAP).unwrap();
    let loader = BootstrapLoader::from(bootstrap);

    let xds_configs = loader.get_xds_configs().unwrap();
    assert_eq!(xds_configs.len(), 1);

    assert_eq!(
        xds_configs[0],
        XdsConfig(
            XdsType::Aggregated(HashSet::from([TypeUrl::Listener, TypeUrl::Cluster])),
            SocketAddress {
                protocol: 0,
                address: "127.0.0.1".to_string(),
                ipv4_compat: false,
                port_specifier: Some(PortSpecifier::PortValue(5679)),
                resolver_name: String::new(),
                network_namespace_filepath: String::new(),
            }
        )
    );
}

#[test]
fn read_mixture_config() {
    const BOOTSTRAP: &str = r#"
dynamic_resources:
  ads_config:
    api_type: GRPC
    transport_api_version: V3
    grpc_services:
      - envoy_grpc:
          cluster_name: ads_cluster
  lds_config:
    resource_api_version: V3
    api_config_source:
      api_type: GRPC
      transport_api_version: V3
      grpc_services:
        - envoy_grpc:
            cluster_name: lds_cluster
  cds_config:
    resource_api_version: V3
    ads: {}

static_resources:
  listeners:
    - name: listener_0
      address:
        socket_address: { address: 127.0.0.1, port_value: 10000 }
      filter_chains:
        - filters:
            - name: envoy.filters.network.http_connection_manager
              typed_config:
                "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
                stat_prefix: ingress_http
                codec_type: AUTO
                rds:
                  route_config_name: local_route
                  config_source:
                    resource_api_version: V3
                    api_config_source:
                      api_type: GRPC
                      transport_api_version: V3
                      grpc_services:
                        - envoy_grpc:
                            cluster_name: rds_cluster
  clusters:
    - name: rds_cluster
      connect_timeout: 0.25s
      type: STATIC
      lb_policy: ROUND_ROBIN
      typed_extension_protocol_options:
        envoy.extensions.upstreams.http.v3.HttpProtocolOptions:
          "@type": type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions
          explicit_http_config:
            http2_protocol_options:
              # Configure an HTTP/2 keep-alive to detect connection issues and reconnect
              # to the admin server if the connection is no longer responsive.
              connection_keepalive:
                interval: 30s
                timeout: 5s
      load_assignment:
        cluster_name: rds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: 127.0.0.1
                      port_value: 5679
    - name: lds_cluster
      connect_timeout: 0.25s
      type: STATIC
      lb_policy: ROUND_ROBIN
      typed_extension_protocol_options:
        envoy.extensions.upstreams.http.v3.HttpProtocolOptions:
          "@type": type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions
          explicit_http_config:
            http2_protocol_options:
              # Configure an HTTP/2 keep-alive to detect connection issues and reconnect
              # to the admin server if the connection is no longer responsive.
              connection_keepalive:
                interval: 30s
                timeout: 5s
      load_assignment:
        cluster_name: lds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: 127.0.0.1
                      port_value: 5677
    - name: ads_cluster
      connect_timeout: 0.25s
      type: STATIC
      lb_policy: ROUND_ROBIN
      typed_extension_protocol_options:
        envoy.extensions.upstreams.http.v3.HttpProtocolOptions:
          "@type": type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions
          explicit_http_config:
            http2_protocol_options:
              # Configure an HTTP/2 keep-alive to detect connection issues and reconnect
              # to the admin server if the connection is no longer responsive.
              connection_keepalive:
                interval: 30s
                timeout: 5s
      load_assignment:
        cluster_name: ads_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: 127.0.0.1
                      port_value: 5678

    "#;
    let bootstrap: Bootstrap = from_yaml(BOOTSTRAP).unwrap();
    let loader = BootstrapLoader::from(bootstrap);

    let xds_configs = loader.get_xds_configs().unwrap();
    assert_eq!(xds_configs.len(), 3);

    assert_eq!(
        xds_configs[0],
        XdsConfig(
            XdsType::Aggregated(HashSet::from([TypeUrl::Cluster])),
            SocketAddress {
                protocol: 0,
                address: "127.0.0.1".to_string(),
                ipv4_compat: false,
                port_specifier: Some(PortSpecifier::PortValue(5678)),
                resolver_name: String::new(),
                network_namespace_filepath: String::new(),
            }
        )
    );

    assert_eq!(
        xds_configs[1],
        XdsConfig(
            XdsType::Individual(TypeUrl::Listener),
            SocketAddress {
                protocol: 0,
                address: "127.0.0.1".to_string(),
                ipv4_compat: false,
                port_specifier: Some(PortSpecifier::PortValue(5677)),
                resolver_name: String::new(),
                network_namespace_filepath: String::new(),
            }
        )
    );

    assert_eq!(
        xds_configs[2],
        XdsConfig(
            XdsType::Individual(TypeUrl::RouteConfiguration),
            SocketAddress {
                protocol: 0,
                address: "127.0.0.1".to_string(),
                ipv4_compat: false,
                port_specifier: Some(PortSpecifier::PortValue(5679)),
                resolver_name: String::new(),
                network_namespace_filepath: String::new(),
            }
        )
    );
}
