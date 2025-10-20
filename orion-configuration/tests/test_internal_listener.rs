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

use orion_configuration::config::{
    bootstrap::{Bootstrap, BootstrapExtension, InternalListenerBootstrap},
    cluster::{
        cluster_specifier::ClusterSpecifier, EndpointAddress, InternalEndpointAddress, InternalUpstreamTransport,
        LbEndpoint, MetadataKind, MetadataValueSource, TransportSocket,
    },
    listener::{FilterChain, FilterChainMatch, InternalListener, Listener, ListenerAddress, MainFilter},
    network_filters::tcp_proxy::TcpProxy,
    transport::BindDeviceOptions,
};
use std::num::NonZeroU32;

#[test]
fn test_internal_listener_serialization() {
    let internal_listener = InternalListener { buffer_size_kb: Some(1024) };

    let listener = Listener {
        name: "test_internal_listener".into(),
        address: ListenerAddress::Internal(internal_listener),
        filter_chains: std::collections::HashMap::from([(
            FilterChainMatch::default(),
            FilterChain {
                filter_chain_match_hash: 0,
                name: "test_filter_chain".into(),
                tls_config: None,
                rbac: Vec::new(),
                terminal_filter: MainFilter::Tcp(TcpProxy {
                    cluster_specifier: ClusterSpecifier::Cluster("test_cluster".into()),
                    access_log: Vec::new(),
                }),
            },
        )]),
        bind_device_options: BindDeviceOptions::default(),
        with_tls_inspector: false,
        proxy_protocol_config: None,
        with_tlv_listener_filter: false,
        tlv_listener_filter_config: None,
    };

    let yaml = serde_yaml::to_string(&listener).unwrap();
    assert!(yaml.contains("test_internal_listener"));

    let deserialized: Listener = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(listener, deserialized);
}

#[test]
fn test_internal_endpoint_serialization() {
    let internal_addr = InternalEndpointAddress {
        server_listener_name: "internal_listener".into(),
        endpoint_id: Some("endpoint1".into()),
    };

    let endpoint = LbEndpoint {
        address: EndpointAddress::Internal(internal_addr),
        health_status: Default::default(),
        load_balancing_weight: NonZeroU32::new(1).unwrap(),
    };

    let yaml = serde_yaml::to_string(&endpoint).unwrap();
    assert!(yaml.contains("internal_listener"));

    let deserialized: LbEndpoint = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(endpoint, deserialized);
}

#[test]
fn test_internal_upstream_transport_serialization() {
    let metadata_source =
        MetadataValueSource { kind: MetadataKind::Host, name: "envoy.filters.listener.original_dst".into() };

    let transport_socket = TransportSocket::InternalUpstream(InternalUpstreamTransport {
        passthrough_metadata: vec![metadata_source],
        transport_socket: Box::new(TransportSocket::RawBuffer),
    });

    let yaml = serde_yaml::to_string(&transport_socket).unwrap();
    assert!(yaml.contains("internal_upstream"));

    let deserialized: TransportSocket = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(transport_socket, deserialized);
}

#[test]
fn test_bootstrap_extension_serialization() {
    let internal_listener_bootstrap = InternalListenerBootstrap { buffer_size_kb: Some(2048) };

    let bootstrap_extension = BootstrapExtension::InternalListener(internal_listener_bootstrap);

    let bootstrap = Bootstrap {
        static_resources: Default::default(),
        dynamic_resources: None,
        node: None,
        admin: None,
        stats_flush_interval: None,
        stats_sinks: Vec::new(),
        bootstrap_extensions: vec![bootstrap_extension],
    };

    let yaml = serde_yaml::to_string(&bootstrap).unwrap();
    assert!(yaml.contains("internal_listener"));

    let deserialized: Bootstrap = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(bootstrap, deserialized);
}

#[test]
fn test_complete_internal_listener_config() {
    let internal_listener = InternalListener { buffer_size_kb: Some(1024) };

    let listener = Listener {
        name: "internal_listener".into(),
        address: ListenerAddress::Internal(internal_listener),
        filter_chains: std::collections::HashMap::from([(
            Default::default(),
            FilterChain {
                filter_chain_match_hash: 0,
                name: "test_filter_chain".into(),
                tls_config: None,
                rbac: Vec::new(),
                terminal_filter: MainFilter::Tcp(TcpProxy {
                    cluster_specifier: ClusterSpecifier::Cluster("internal_cluster".into()),
                    access_log: Vec::new(),
                }),
            },
        )]),
        bind_device_options: BindDeviceOptions::default(),
        with_tls_inspector: false,
        proxy_protocol_config: None,
        with_tlv_listener_filter: false,
        tlv_listener_filter_config: None,
    };

    let internal_addr = InternalEndpointAddress {
        server_listener_name: "internal_listener".into(),
        endpoint_id: Some("endpoint1".into()),
    };

    let endpoint = LbEndpoint {
        address: EndpointAddress::Internal(internal_addr),
        health_status: Default::default(),
        load_balancing_weight: NonZeroU32::new(1).unwrap(),
    };

    let transport_socket = TransportSocket::InternalUpstream(InternalUpstreamTransport {
        passthrough_metadata: vec![MetadataValueSource {
            kind: MetadataKind::Host,
            name: "envoy.filters.listener.original_dst".into(),
        }],
        transport_socket: Box::new(TransportSocket::RawBuffer),
    });

    let yaml = serde_yaml::to_string(&listener).unwrap();
    let deserialized: Listener = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(listener, deserialized);

    let yaml = serde_yaml::to_string(&endpoint).unwrap();
    let deserialized: LbEndpoint = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(endpoint, deserialized);

    let yaml = serde_yaml::to_string(&transport_socket).unwrap();
    let deserialized: TransportSocket = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(transport_socket, deserialized);
}
