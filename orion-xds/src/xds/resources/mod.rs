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

use std::net::SocketAddr;

use futures::Stream;
use orion_data_plane_api::envoy_data_plane_api::{
    envoy::{
        self,
        config::{
            cluster::v3::{
                cluster::{ClusterDiscoveryType, DiscoveryType, LbPolicy},
                Cluster,
            },
            core::v3::{
                socket_address::PortSpecifier, Address, Http1ProtocolOptions, Http2ProtocolOptions,
                HttpProtocolOptions as ConfigHttpProtocolOptions, Node,
            },
            endpoint::v3::{
                lb_endpoint::HostIdentifier, ClusterLoadAssignment, Endpoint, LbEndpoint, LocalityLbEndpoints,
            },
            listener::v3::{filter::ConfigType, Filter, FilterChain, Listener},
            route::v3::{
                header_matcher::HeaderMatchSpecifier, route::Action, route_action::ClusterSpecifier,
                route_match::PathSpecifier, weighted_cluster::ClusterWeight, HeaderMatcher, Route, RouteAction,
                RouteConfiguration, RouteMatch, VirtualHost, WeightedCluster,
            },
        },
        extensions::{
            filters::network::http_connection_manager::v3::{
                http_connection_manager::{CodecType, RouteSpecifier},
                HttpConnectionManager,
            },
            transport_sockets::tls::v3::{secret, Secret},
            upstreams::http::v3::{
                http_protocol_options::{
                    explicit_http_config::ProtocolConfig, ExplicitHttpConfig, UpstreamProtocolOptions,
                },
                HttpProtocolOptions,
            },
        },
        service::discovery::v3::{DeltaDiscoveryRequest, DiscoveryRequest, Resource},
    },
    google::protobuf::{Any, UInt32Value},
    prost::Message,
};
use tokio_stream::StreamExt;

use self::converters::SocketConverter;
use crate::xds::model::TypeUrl;

pub mod converters;

#[allow(deprecated)]
pub fn create_node<S1, S2>(id: S1, cluster_name: S2) -> Node
where
    S1: Into<String>,
    S2: Into<String>,
{
    Node {
        id: id.into(),
        cluster: cluster_name.into(),
        metadata: None,
        locality: None,
        user_agent_name: String::new(),
        extensions: vec![],
        client_features: vec![],
        listening_addresses: vec![],
        user_agent_version_type: None,
        dynamic_parameters: std::collections::HashMap::new(),
    }
}

pub fn create_endpoints(endpoint_addrs: Vec<Address>) -> LocalityLbEndpoints {
    let lb_endpoints = endpoint_addrs.into_iter().map(create_endpoint).collect();
    LocalityLbEndpoints {
        priority: 0,
        // load_balancing_weight: Some(UInt32Value { value: 2 }),
        lb_endpoints,
        ..Default::default()
    }
}

pub fn create_endpoint(addr: Address) -> LbEndpoint {
    // let health_check_config =
    //     HealthCheckConfig { address: Some(addr.clone()), disable_active_health_check: true, ..Default::default() };

    LbEndpoint {
        host_identifier: Some(HostIdentifier::Endpoint(Endpoint {
            address: Some(addr),
            // health_check_config: Some(health_check_config),
            ..Default::default()
        })),
        load_balancing_weight: Some(UInt32Value { value: 1 }),
        ..Default::default()
    }
}

pub fn create_addresses(addr: SocketAddr, count: u32) -> Vec<Address> {
    let socket = SocketConverter::from(addr);
    (0..count)
        .map(|i| {
            let mut socket = socket.clone();
            if let orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::address::Address::SocketAddress(ref mut socket) =
                socket
            {
                if let Some(PortSpecifier::PortValue(port)) = socket.port_specifier {
                    socket.port_specifier = Some(PortSpecifier::PortValue(i + port));
                }
            }
            Address { address: Some(socket) }
        })
        .collect()
}

pub fn create_cluster_with_endpoints(
    name: &str,
    endpoint_addr: SocketAddr,
    endpoints: u32,
    enable_http2: bool,
) -> Cluster {
    let addresses = create_addresses(endpoint_addr, endpoints);
    create_cluster(name, vec![create_endpoints(addresses)], enable_http2)
}

pub fn create_cluster_load_assignment(name: &str, endpoint_addr: SocketAddr, endpoints: u32) -> ClusterLoadAssignment {
    let addresses = create_addresses(endpoint_addr, endpoints);
    let endpoints = vec![create_endpoints(addresses)];
    ClusterLoadAssignment { cluster_name: name.to_owned(), endpoints, ..Default::default() }
}
pub fn create_get_header_matcher(name: &str) -> HeaderMatcher {
    create_header_matcher(name, "GET".to_owned())
}
pub fn create_post_header_matcher(name: &str) -> HeaderMatcher {
    create_header_matcher(name, "POST".to_owned())
}

pub fn create_header_matcher(name: &str, method: String) -> HeaderMatcher {
    HeaderMatcher {
        name: format!("{name}-header_matcher"),
        header_match_specifier: Some(HeaderMatchSpecifier::ExactMatch(method)),
        ..Default::default()
    }
}

pub fn create_route_match(name: &str, with_prefix: String) -> RouteMatch {
    let name = format!("{name}-route_match");
    RouteMatch {
        headers: vec![create_get_header_matcher(&name), create_post_header_matcher(&name)],
        path_specifier: Some(PathSpecifier::Prefix(with_prefix)),
        ..Default::default()
    }
}

pub fn create_route(name: &str, with_prefix: String, with_cluster: String) -> Route {
    let name = format!("{name}-route");
    Route {
        r#match: Some(create_route_match(&name, with_prefix)),
        name,
        action: Some(Action::Route(RouteAction {
            cluster_specifier: Some(ClusterSpecifier::Cluster(with_cluster)),
            ..Default::default()
        })),
        ..Default::default()
    }
}
pub fn create_virtual_host(name: &str, domains: Vec<String>, with_prefix: String, with_cluster: String) -> VirtualHost {
    let name = format!("{name}-vc");
    VirtualHost { routes: vec![create_route(&name, with_prefix, with_cluster)], name, domains, ..Default::default() }
}

pub fn create_route_resource(
    name: &str,
    domains: Vec<String>,
    with_prefix: String,
    with_cluster: String,
) -> RouteConfiguration {
    RouteConfiguration {
        name: name.to_owned(),
        virtual_hosts: vec![create_virtual_host(name, domains, with_prefix, with_cluster)],
        ..Default::default()
    }
}

pub fn create_secret(name: &str, secret: secret::Type) -> Secret {
    Secret { name: name.to_owned(), r#type: Some(secret) }
}

pub fn create_cluster(name: &str, endpoints: Vec<LocalityLbEndpoints>, enable_http2: bool) -> Cluster {
    let load_assignment = ClusterLoadAssignment { cluster_name: name.to_owned(), endpoints, ..Default::default() };

    let config = if enable_http2 {
        let http2_protocol_options =
            Http2ProtocolOptions { max_concurrent_streams: Some(UInt32Value { value: 10 }), ..Default::default() };
        ExplicitHttpConfig { protocol_config: Some(ProtocolConfig::Http2ProtocolOptions(http2_protocol_options)) }
    } else {
        let http1_protocol_options = Http1ProtocolOptions::default();
        ExplicitHttpConfig { protocol_config: Some(ProtocolConfig::HttpProtocolOptions(http1_protocol_options)) }
    };

    let options = HttpProtocolOptions {
        upstream_protocol_options: Some(UpstreamProtocolOptions::ExplicitHttpConfig(config)),
        common_http_protocol_options: Some(ConfigHttpProtocolOptions::default()),
        ..Default::default()
    };

    let extension = Any {
        type_url: "type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions".to_owned(),
        value: options.encode_to_vec(),
    };

    let mut extensions = std::collections::HashMap::new();
    extensions.insert("envoy.extensions.upstreams.http.v3.HttpProtocolOptions".to_owned(), extension);

    Cluster {
        name: name.to_owned(),
        cluster_discovery_type: Some(ClusterDiscoveryType::Type(DiscoveryType::Static.into())),
        lb_policy: LbPolicy::RoundRobin.into(),
        load_assignment: Some(load_assignment),
        typed_extension_protocol_options: extensions,
        ..Default::default()
    }
}

pub fn create_cluster_resource(cluster: &Cluster) -> Resource {
    let value = cluster.encode_to_vec();
    let any = Any { type_url: TypeUrl::Cluster.to_string(), value };

    let mut cluster_resource = Resource { ..Default::default() };
    cluster_resource.name.clone_from(&cluster.name);
    cluster_resource.resource = Some(any);
    cluster_resource
}

pub fn create_load_assignment_resource(name: &str, load_assignment: &ClusterLoadAssignment) -> Resource {
    let value = load_assignment.encode_to_vec();
    let any = Any { type_url: TypeUrl::ClusterLoadAssignment.to_string(), value };

    let mut cla_resource = Resource { ..Default::default() };
    cla_resource.name.clone_from(&name.to_owned());
    cla_resource.resource = Some(any);
    cla_resource
}

pub fn create_route_configuration_resource(name: &str, load_assignment: &RouteConfiguration) -> Resource {
    let value = load_assignment.encode_to_vec();
    let any = Any { type_url: TypeUrl::RouteConfiguration.to_string(), value };

    let mut route_resource = Resource { ..Default::default() };
    route_resource.name.clone_from(&name.to_owned());
    route_resource.resource = Some(any);
    route_resource
}

pub fn create_secret_resource(name: &str, secret: &Secret) -> Resource {
    let value = secret.encode_to_vec();
    let any = Any { type_url: TypeUrl::Secret.to_string(), value };

    let mut route_resource = Resource { ..Default::default() };
    route_resource.name.clone_from(&name.to_owned());
    route_resource.resource = Some(any);
    route_resource
}

pub fn create_listener(
    name: &str,
    addr: SocketAddr,
    codec_type: CodecType,
    domains: Vec<String>,
    mut cluster_names: Vec<(String, u32)>,
) -> Listener {
    let route_match = RouteMatch { path_specifier: Some(PathSpecifier::Path("/".to_owned())), ..Default::default() };

    let cluster_action = if cluster_names.len() == 1 {
        let cluster_name = cluster_names.remove(0).0;
        RouteAction { cluster_specifier: Some(ClusterSpecifier::Cluster(cluster_name)), ..Default::default() }
    } else {
        let clusters: Vec<_> = cluster_names
            .into_iter()
            .map(|(name, weight)| ClusterWeight {
                name,
                weight: Some(UInt32Value { value: weight }),
                ..Default::default()
            })
            .collect();

        RouteAction {
            cluster_specifier: Some(ClusterSpecifier::WeightedClusters(WeightedCluster {
                clusters,
                ..Default::default()
            })),
            ..Default::default()
        }
    };

    let vc_name = format!("{name}-vc");
    let virtual_host_route = Route {
        name: format!("{vc_name}-route"),
        r#match: Some(route_match),
        action: Some(Action::Route(cluster_action)),
        ..Default::default()
    };

    let virtual_host = VirtualHost { name: vc_name, domains, routes: vec![virtual_host_route], ..Default::default() };

    let http_connection_manager = HttpConnectionManager {
        codec_type: codec_type.into(),
        route_specifier: Some(RouteSpecifier::RouteConfig(RouteConfiguration {
            name: format!("{name}-route-conf"),
            virtual_hosts: vec![virtual_host],
            ..Default::default()
        })),
        http_filters: vec![envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter {
            name: "envoy.filters.http.router".to_string(),
            config_type: Some(
                envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType::TypedConfig(
                    Any {
                        type_url: "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router".to_string(),
                        value: vec![],
                    },
                ),
            ),
            is_optional: false,
            disabled: false,
        }],
        ..Default::default()
    };

    let http_connection_manager_any = Any {
        type_url:
            "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager"
                .to_owned(),
        value: http_connection_manager.encode_to_vec().into(),
    };

    let http_connection_manager_filter = Filter {
        name: "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager"
            .to_owned(),
        config_type: Some(ConfigType::TypedConfig(http_connection_manager_any)),
    };
    let filter_chain =
        FilterChain { name: format!("{name}-fc"), filters: vec![http_connection_manager_filter], ..Default::default() };

    Listener {
        name: name.to_owned(),
        address: Some(Address { address: Some(SocketConverter::from(addr)) }),
        filter_chains: vec![filter_chain],
        ..Default::default()
    }
}

pub fn create_listener_resource(listener: &Listener) -> Resource {
    let value = listener.encode_to_vec();
    let any = Any { type_url: TypeUrl::Listener.to_string(), value };

    let mut listener_resource = Resource { ..Default::default() };
    listener_resource.name.clone_from(&listener.name);
    listener_resource.resource = Some(any);
    listener_resource
}

pub fn dicovery_request_stream() -> impl Stream<Item = DiscoveryRequest> {
    tokio_stream::iter(1..usize::MAX).throttle(std::time::Duration::from_secs(5)).map(|i| DiscoveryRequest {
        version_info: "v3".to_owned(),
        node: None,
        resource_names: vec![],
        resource_locators: vec![],
        type_url: TypeUrl::Cluster.to_string(),
        response_nonce: format!("nonce {i}"),
        error_detail: None,
    })
}

pub fn delta_dicovery_request_stream() -> impl Stream<Item = DeltaDiscoveryRequest> {
    tokio_stream::iter(1..usize::MAX).throttle(std::time::Duration::from_secs(5)).map(|i| DeltaDiscoveryRequest {
        node: None,
        type_url: TypeUrl::Cluster.to_string(),
        response_nonce: format!("nonce {i}"),
        error_detail: None,
        resource_names_subscribe: vec![],
        resource_names_unsubscribe: vec![],
        resource_locators_subscribe: vec![],
        resource_locators_unsubscribe: vec![],
        initial_resource_versions: std::collections::HashMap::new(),
    })
}

#[cfg(test)]
mod test {
    use orion_data_plane_api::decode::from_yaml;
    use orion_data_plane_api::envoy_data_plane_api::{
        envoy::{config::cluster::v3::Cluster, service::discovery::v3::Resource},
        google::protobuf::Any,
        prost::{self, Message},
    };
    use tracing::info;

    use crate::xds::model::{TypeUrl, XdsResourcePayload};

    #[test]
    fn test_cluster_conversion() {
        const CLUSTER: &str = r#"
name: cluster1
type: STATIC
load_assignment:
  endpoints:
    - lb_endpoints:
        - endpoint:
            address:
              socket_address:
                address: 192.168.2.10
                port_value: 80
"#;
        let cluster: Cluster = from_yaml(CLUSTER).unwrap();

        let mut value: Vec<u8> = vec![];
        cluster.encode(&mut value).unwrap();
        let any = Any { type_url: TypeUrl::Cluster.to_string(), value };

        let resource = Resource { name: "cluster__111".to_owned(), resource: Some(any), ..Default::default() };
        let mut buf: Vec<u8> = vec![];
        cluster.encode(&mut buf).unwrap();
        let prost_buf = prost::bytes::Bytes::from(buf);
        let decoded = Cluster::decode(prost_buf).unwrap();
        info!("decoded {decoded:?}");
        XdsResourcePayload::try_from((resource, TypeUrl::Cluster)).unwrap();
    }
}
