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

use crate::decode::decode_any_type;
use envoy_data_plane_api::envoy::{
    config::{
        cluster::v3::Cluster,
        core::v3::transport_socket,
        listener::v3::{Filter, FilterChain},
        route::v3::Route,
    },
    extensions::{
        filters::{
            http::local_ratelimit::v3::LocalRateLimit, network::http_connection_manager::v3::HttpConnectionManager,
        },
        transport_sockets::tls::v3::DownstreamTlsContext,
        upstreams::http::v3::HttpProtocolOptions,
    },
};

type Result<T> = std::result::Result<T, crate::decode::DecodeAnyError>;

pub trait FilterChainValidation {
    /// Decode TLS context settings from filter chain transport socket.
    /// May fail due to decoding errors
    fn get_downstream_tls_context(&self) -> Result<Option<DownstreamTlsContext>>;
}

const T_EXT_TLS_DOWNSTREAM_CONTEXT: &str =
    "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.DownstreamTlsContext";

impl FilterChainValidation for FilterChain {
    fn get_downstream_tls_context(&self) -> Result<Option<DownstreamTlsContext>> {
        self.transport_socket
            .as_ref()
            .and_then(|s| s.config_type.as_ref())
            .map(|cfg| {
                let transport_socket::ConfigType::TypedConfig(envoy_data_plane_api::google::protobuf::Any {
                    type_url: _,
                    value: _,
                }) = cfg;
                todo!()
            })
            .filter(|envoy_data_plane_api::google::protobuf::Any { type_url, value: _ }| {
                *type_url == T_EXT_TLS_DOWNSTREAM_CONTEXT
            })
            .map(|any| decode_any_type(&any, T_EXT_TLS_DOWNSTREAM_CONTEXT))
            .transpose()
    }
}

const T_EXT_HTTP_CONN_MANAGER: &str =
    "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager";

pub trait FilterValidation {
    /// Get the HTTP connection manager extension associated with this filter if any.
    /// Fails on decoding errors.
    fn get_http_connection_manager(&self) -> Result<Option<HttpConnectionManager>>;
}

impl FilterValidation for Filter {
    fn get_http_connection_manager(&self) -> Result<Option<HttpConnectionManager>> {
        use envoy_data_plane_api::envoy::config::listener::v3::filter::ConfigType;
        self.config_type
            .as_ref()
            .and_then(|cfg| match cfg {
                ConfigType::TypedConfig(envoy_data_plane_api::google::protobuf::Any { type_url: _, value: _ }) => {
                    Some(todo!())
                },
                ConfigType::ConfigDiscovery(_) => None,
            })
            .filter(|envoy_data_plane_api::google::protobuf::Any { type_url, value: _ }| {
                *type_url == T_EXT_HTTP_CONN_MANAGER
            })
            .map(|any| decode_any_type(&any, T_EXT_HTTP_CONN_MANAGER))
            .transpose()
    }
}

const T_EXT_HTTP_PROTO_OPTIONS: &str = "type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions";

pub trait ClusterValidation {
    fn get_http_protocol_options(&self) -> Result<Option<HttpProtocolOptions>>;
}

impl ClusterValidation for Cluster {
    fn get_http_protocol_options(&self) -> Result<Option<HttpProtocolOptions>> {
        self.typed_extension_protocol_options
            // Same as T_EXT_HTTP_PROTO_OPTIONS sans prefix
            .get("envoy.extensions.upstreams.http.v3.HttpProtocolOptions")
            .as_ref()
            .filter(|any| any.type_url == T_EXT_HTTP_PROTO_OPTIONS)
            .map(|any| decode_any_type(any, T_EXT_HTTP_PROTO_OPTIONS))
            .transpose()
    }
}

const T_EXT_HTTP_LOCAL_RATELIMIT: &str =
    "type.googleapis.com/envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit";

pub trait LocalRateLimitValidation {
    fn get_local_ratelimit(&self) -> Result<Option<LocalRateLimit>>;
}

impl LocalRateLimitValidation for Route {
    fn get_local_ratelimit(&self) -> Result<Option<LocalRateLimit>> {
        self.typed_per_filter_config
            // Same as T_EXT_HTTP_LOCAL_RATELIMIT sans prefix
            .get("envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit")
            .as_ref()
            .filter(|any| any.type_url == T_EXT_HTTP_LOCAL_RATELIMIT)
            .map(|any| decode_any_type(any, T_EXT_HTTP_LOCAL_RATELIMIT))
            .transpose()
    }
}
