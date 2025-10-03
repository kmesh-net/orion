#![allow(clippy::all)]

pub use prost;
pub use prost_reflect;
pub use tonic;
pub use tonic_health;

// This needs to match the file descriptor path from build.rs
/// The binary contents of the protobuf descriptor
pub const FILE_DESCRIPTOR_SET_BYTES: &'static [u8] = include_bytes!(concat!(env!("OUT_DIR"), "/proto_descriptor.bin"));

include!(concat!(env!("OUT_DIR"), "/mod.rs"));

#[test]
fn test_json_any_decode() {
    use crate::envoy::{
        config::{
            bootstrap::v3::Bootstrap,
            core::v3::{data_source::Specifier, transport_socket::ConfigType},
        },
        extensions::transport_sockets::tls::v3::DownstreamTlsContext,
    };
    use prost::{Message, Name};

    const BOOTSTRAP_JSON: &str = r#"{
  "staticResources": {
    "listeners": [
      {
        "name": "server-1",
        "address": {
          "socketAddress": {
            "address": "127.0.0.1",
            "portValue": 9000
          }
        },
        "filterChains": [
          {
            "filters": [],
            "transportSocket": {
              "name": "envoy.transport_sockets.tls",
              "typedConfig": {
                "@type": "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.DownstreamTlsContext",
                "requireClientCertificate": true,
                "commonTlsContext": {
                  "tlsParams": {
                    "tlsMinimumProtocolVersion": "TLSv1_3",
                    "tlsMaximumProtocolVersion": "TLSv1_3"
                  },
                  "validationContext": {
                    "trustedCa": {
                      "filename": "./certs/ca.crt"
                    },
                    "matchTypedSubjectAltNames": [
                      {
                        "sanType": "DNS",
                        "matcher": {
                          "exact": "client.test"
                        }
                      }
                    ]
                  },
                  "tlsCertificates": [
                    {
                      "certificateChain": {
                        "filename": "./certs/server.test.ecdsa-p256.crt"
                      },
                      "privateKey": {
                        "filename": "./certs/server.test.ecdsa-p256.key"
                      }
                    }
                  ]
                }
              }
            }
          }
        ]
      }
    ]
  }
}
    "#;

    let pool = prost_reflect::DescriptorPool::decode(crate::FILE_DESCRIPTOR_SET_BYTES).unwrap();
    let message_descriptor = pool.get_message_by_name(&Bootstrap::full_name()).unwrap();
    let mut deserializer = serde_json::de::Deserializer::from_str(BOOTSTRAP_JSON);
    let bootstrap_dyn = prost_reflect::DynamicMessage::deserialize(message_descriptor, &mut deserializer).unwrap();
    deserializer.end().unwrap();
    let bootstrap: Bootstrap = bootstrap_dyn.transcode_to().unwrap();

    let bootstrap_tls_config = bootstrap.static_resources.as_ref().unwrap().listeners[0].filter_chains[0]
        .transport_socket
        .as_ref()
        .unwrap()
        .config_type
        .as_ref()
        .unwrap();
    let ConfigType::TypedConfig(tls_any) = bootstrap_tls_config;
    if tls_any.type_url == "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.DownstreamTlsContext" {
        let tls_ctx_map = DownstreamTlsContext::decode(tls_any.value.as_slice()).unwrap();
        let ctx = tls_ctx_map.common_tls_context.unwrap();
        assert_eq!(ctx.tls_params.unwrap().tls_maximum_protocol_version, 4);
        assert_eq!(
            *ctx.tls_certificates[0].private_key.as_ref().unwrap().specifier.as_ref().unwrap(),
            Specifier::Filename("./certs/server.test.ecdsa-p256.key".to_owned())
        );
    }
}

#[test]
fn test_yaml_any_decode() {
    use crate::envoy::{
        config::{bootstrap::v3::Bootstrap, listener::v3::filter::ConfigType, route::v3::route_match::PathSpecifier},
        extensions::filters::network::http_connection_manager::v3::{
            HttpConnectionManager, http_connection_manager::RouteSpecifier,
        },
    };
    use prost::{Message, Name};

    const BOOTSTRAP_YAML: &str = r#"
---
admin:
  address:
    socketAddress:
      address: "127.0.0.1"
      portValue: 9901
node:
  id: envoy-test-1
  cluster: envoy-test-cluster-1
staticResources:
  listeners:
    - name: server-1
      address:
        socketAddress:
          address: 127.0.0.1
          portValue: 9000
      filterChains:
        - filters:
          - name: envoy.filters.network.http_connection_manager
            typedConfig:
              "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
              statPrefix: ingress_http
              httpFilters:
                - name: envoy.filters.http.router
              routeConfig:
                name: local_route
                virtualHosts:
                  - name: local_service
                    domains: ["*"]
                    routes:
                      - match:
                          prefix: "/"
                        route:
                          cluster: local-srv
          transportSocket:
            name: envoy.transport_sockets.tls
            typedConfig:
              '@type': type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.DownstreamTlsContext
              requireClientCertificate: true
              commonTlsContext:
                tlsParams:
                  tlsMinimumProtocolVersion: TLSv1_3
                  tlsMaximumProtocolVersion: TLSv1_3
                validationContext:
                  trustedCa:
                    filename: ./certs/ca.crt
                  matchTypedSubjectAltNames:
                    - sanType: DNS
                      matcher:
                        exact: client.test
                tlsCertificates:
                  - certificateChain:
                      filename: ./certs/server.test.ecdsa-p256.crt
                    privateKey:
                      filename: ./certs/server.test.ecdsa-p256.key
  clusters:
    - name: local-srv
      type: STATIC
      lbPolicy: ROUND_ROBIN
      loadAssignment:
        clusterName: local-srv
        endpoints:
          - lbEndpoints:
            - endpoint:
                address:
                  socketAddress:
                    address: "127.0.0.1"
                    portValue: 9110
    "#;

    let pool = prost_reflect::DescriptorPool::decode(crate::FILE_DESCRIPTOR_SET_BYTES).unwrap();
    let message_descriptor = pool.get_message_by_name(&Bootstrap::full_name()).unwrap();
    let deserializer = serde_yaml::Deserializer::from_str(BOOTSTRAP_YAML);
    let bootstrap_dyn = prost_reflect::DynamicMessage::deserialize(message_descriptor, deserializer).unwrap();
    let bootstrap: Bootstrap = bootstrap_dyn.transcode_to().unwrap();

    let config_type = bootstrap.static_resources.as_ref().unwrap().listeners[0].filter_chains[0].filters[0]
        .config_type
        .as_ref()
        .unwrap();

    if let ConfigType::TypedConfig(http_manager_any) = config_type {
        if http_manager_any.type_url
            == "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager"
        {
            let http_manager_map = HttpConnectionManager::decode(http_manager_any.value.as_slice()).unwrap();

            let Some(RouteSpecifier::RouteConfig(route_config)) = http_manager_map.route_specifier else {
                panic!("Expecting route specifier to hold route configuration: {:?}", http_manager_map.route_specifier);
            };

            assert_eq!(route_config.name, "local_route");
            let route_match = route_config.virtual_hosts[0].routes[0].r#match.as_ref().unwrap();
            assert_eq!(*route_match.path_specifier.as_ref().unwrap(), PathSpecifier::Prefix("/".to_owned()));
        }
    }
}
