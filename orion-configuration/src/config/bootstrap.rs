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

use std::time::Duration;

use crate::config::{
    cluster::Cluster, common::is_default, core::Address, listener::Listener, metrics::StatsSink, secret::Secret,
};
use compact_str::CompactString;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Bootstrap {
    #[serde(with = "serde_yaml::with::singleton_map_recursive", skip_serializing_if = "is_default", default)]
    pub static_resources: StaticResources,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub dynamic_resources: Option<DynamicResources>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub node: Option<Node>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub admin: Option<Admin>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub stats_flush_interval: Option<Duration>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub stats_sinks: Vec<StatsSink>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub bootstrap_extensions: Vec<BootstrapExtension>,
}

impl Bootstrap {
    pub fn get_ads_configs(&self) -> &[CompactString] {
        self.dynamic_resources.as_ref().map(|dr| dr.grpc_cluster_specifiers.as_slice()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Node {
    pub id: CompactString,
    pub cluster_id: CompactString,
    #[serde(skip_serializing, skip_deserializing)]
    pub metadata: Option<orion_data_plane_api::envoy_data_plane_api::google::protobuf::Struct>,
}

impl Eq for Node {}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.cluster_id == other.cluster_id
    }
}

impl Node {
    pub fn service_name(&self) -> &str {
        if !self.id.is_empty() {
            &self.id
        } else if !self.cluster_id.is_empty() {
            &self.cluster_id
        } else {
            "unknown_service:orion"
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DynamicResources {
    pub grpc_cluster_specifiers: Vec<CompactString>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Admin {
    pub address: Address,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]

pub enum BootstrapExtension {
    InternalListener(InternalListenerBootstrap),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InternalListenerBootstrap {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub buffer_size_kb: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct StaticResources {
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub listeners: Vec<Listener>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub clusters: Vec<Cluster>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub secrets: Vec<Secret>,
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{
        Admin, Bootstrap, BootstrapExtension, DynamicResources, InternalListenerBootstrap, Node, StaticResources,
    };
    use crate::config::{common::*, core::envoy_conversions::SocketAddressWrapper, grpc::Duration, metrics::StatsSink};
    use compact_str::CompactString;
    use orion_data_plane_api::envoy_data_plane_api::{
        envoy::{
            config::{
                bootstrap::v3::{
                    bootstrap::{DynamicResources as EnvoyDynamicResources, StaticResources as EnvoyStaticResources},
                    Admin as EnvoyAdmin, Bootstrap as EnvoyBootstrap,
                },
                core::v3::{
                    address,
                    grpc_service::{EnvoyGrpc, TargetSpecifier as EnvoyGrpcTargetSpecifier},
                    ApiConfigSource as EnvoyApiConfigSource, GrpcService as EnvoyGrpcService, Node as EnvoyNode,
                    TypedExtensionConfig as EnvoyTypedExtensionConfig,
                },
                metrics::v3::stats_sink::ConfigType,
            },
            extensions::bootstrap::internal_listener::v3::InternalListener as EnvoyInternalListener,
        },
        google::protobuf::value::Kind,
        prost::Message,
        udpa::r#type::v1::TypedStruct,
    };

    use tracing::debug;

    impl Bootstrap {
        pub fn deserialize_from_envoy<R: std::io::Read>(rdr: R) -> Result<Self, GenericError> {
            let envoy: EnvoyBootstrap =
                orion_data_plane_api::decode::from_serde_deserializer(serde_yaml::Deserializer::from_reader(rdr))
                    .map_err(|e| GenericError::from_msg_with_cause("failed to deserialize envoy bootstrap", e))?;
            envoy.try_into()
        }
    }

    impl TryFrom<EnvoyBootstrap> for Bootstrap {
        type Error = GenericError;
        fn try_from(envoy: EnvoyBootstrap) -> Result<Self, Self::Error> {
            let EnvoyBootstrap {
                node,
                node_context_params,
                static_resources,
                dynamic_resources,
                cluster_manager,
                hds_config,
                flags_path,
                stats_sinks,
                deferred_stat_options: _,
                stats_config: _,
                stats_flush_interval,
                watchdog,
                watchdogs,
                tracing,
                layered_runtime: _,
                admin,
                overload_manager,
                enable_dispatcher_stats,
                header_prefix,
                stats_server_version_override,
                use_tcp_for_dns_lookups,
                dns_resolution_config,
                typed_dns_resolver_config,
                bootstrap_extensions,
                fatal_actions,
                config_sources,
                default_config_source,
                default_socket_interface,
                certificate_provider_instances,
                inline_headers,
                perf_tracing_file_path,
                default_regex_engine,
                xds_delegate_extension,
                xds_config_tracker_extension,
                listener_manager,
                application_log_config: _,
                grpc_async_client_manager_config,
                stats_flush,
                memory_allocator_manager,
                ..
            } = envoy;
            unsupported_field!(
                // node,
                node_context_params,
                // static_resources,
                // dynamic_resources,
                cluster_manager,
                hds_config,
                flags_path,
                // stats_sinks,
                //deferred_stat_options,
                //stats_config,
                // stats_flush_interval,
                watchdog,
                watchdogs,
                tracing,
                //layered_runtime,
                //admin,
                overload_manager,
                enable_dispatcher_stats,
                header_prefix,
                stats_server_version_override,
                use_tcp_for_dns_lookups,
                dns_resolution_config,
                typed_dns_resolver_config,
                //bootstrap_extensions,
                fatal_actions,
                config_sources,
                default_config_source,
                default_socket_interface,
                certificate_provider_instances,
                inline_headers,
                perf_tracing_file_path,
                default_regex_engine,
                xds_delegate_extension,
                xds_config_tracker_extension,
                listener_manager,
                //application_log_config,
                grpc_async_client_manager_config,
                stats_flush,
                memory_allocator_manager
            )?;
            let static_resources = convert_opt!(static_resources)?;
            let dynamic_resources =
                dynamic_resources.map(DynamicResources::try_from).transpose().with_node("dynamic_resources")?;
            let node = node.map(Node::try_from).transpose().with_node("node")?;
            let admin = admin.map(Admin::try_from).transpose().with_node("admin")?;
            let stats_flush_interval = stats_flush_interval
                .map(|d| Duration::try_from(d).map(|d| d.0))
                .transpose()
                .with_node("stats_flush_interval")?;

            let stats_sinks = stats_sinks
                .into_iter()
                .filter_map(|s| s.config_type)
                .map(|c| {
                    let ConfigType::TypedConfig(any_config) = c;
                    StatsSink::try_from(any_config)
                })
                .collect::<Result<Vec<_>, _>>()
                .with_node("stats_sinks")?;

            let bootstrap_extensions = bootstrap_extensions
                .into_iter()
                .map(BootstrapExtension::try_from)
                .collect::<Result<Vec<_>, _>>()
                .with_node("bootstrap_extensions")?;

            Ok(Self {
                static_resources,
                node,
                dynamic_resources,
                admin,
                stats_flush_interval,
                stats_sinks,
                bootstrap_extensions,
            })
        }
    }
    impl TryFrom<EnvoyNode> for Node {
        type Error = GenericError;
        fn try_from(value: EnvoyNode) -> Result<Self, Self::Error> {
            let EnvoyNode {
                id,
                cluster,
                metadata,
                dynamic_parameters,
                locality: _,
                user_agent_name,
                extensions: _,
                client_features,
                listening_addresses,
                user_agent_version_type,
            } = value;
            unsupported_field!(
                // id,
                //cluster,
                //metadata,
                dynamic_parameters,
                //locality,
                user_agent_name,
                //extensions,
                client_features,
                listening_addresses,
                user_agent_version_type
            )?;

            let id = required!(id)?.into();
            let cluster = required!(cluster)?.into();
            Ok(Self { id, cluster_id: cluster, metadata })
        }
    }
    impl TryFrom<EnvoyDynamicResources> for DynamicResources {
        type Error = GenericError;
        fn try_from(value: EnvoyDynamicResources) -> Result<Self, Self::Error> {
            let EnvoyDynamicResources {
                lds_config: _,
                lds_resources_locator,
                cds_config: _,
                cds_resources_locator,
                ads_config,
            } = value;
            unsupported_field!(lds_resources_locator, cds_resources_locator)?;
            let EnvoyApiConfigSource {
                api_type: _,
                transport_api_version: _,
                cluster_names: _,
                grpc_services,
                refresh_delay,
                request_timeout,
                rate_limit_settings,
                set_node_on_first_message_only: _,
                config_validators,
            } = required!(ads_config)?;
            let grpc_cluster_specifiers = (|| -> Result<_, GenericError> {
                unsupported_field!(
                    //todo(hayley): are these required to be set?
                    // api_type,
                    // transport_api_version,
                    // cluster_names,
                    // grpc_services,
                    refresh_delay,
                    request_timeout,
                    rate_limit_settings,
                    //set_node_on_first_message_only,
                    config_validators
                )?;
                (|| -> Result<_, GenericError> {
                    let mut cluster_specifiers = Vec::new();

                    for EnvoyGrpcService { timeout, initial_metadata, target_specifier, retry_policy } in
                        required!(grpc_services)?
                    {
                        unsupported_field!(timeout, initial_metadata, retry_policy)?;
                        match required!(target_specifier)? {
                            EnvoyGrpcTargetSpecifier::EnvoyGrpc(EnvoyGrpc {
                                cluster_name,
                                authority,
                                retry_policy,
                                max_receive_message_length,
                                skip_envoy_headers,
                            }) => {
                                unsupported_field!(
                                    authority,
                                    retry_policy,
                                    max_receive_message_length,
                                    skip_envoy_headers
                                )
                                .with_node("target_specifier")?;
                                let cluster_name = required!(cluster_name).with_node("target_specifier")?;
                                cluster_specifiers.push(CompactString::from(cluster_name))
                            },
                            EnvoyGrpcTargetSpecifier::GoogleGrpc(_) => {
                                return Err(GenericError::unsupported_variant("GoogleGrpc"))
                                    .with_node("target_specifier");
                            },
                        }
                    }
                    Ok(cluster_specifiers)
                })()
                .with_node("grpc_services")
            })()
            .with_node("ads_config")?;
            Ok(DynamicResources { grpc_cluster_specifiers })
        }
    }
    impl TryFrom<EnvoyStaticResources> for StaticResources {
        type Error = GenericError;
        fn try_from(envoy: EnvoyStaticResources) -> Result<Self, Self::Error> {
            let EnvoyStaticResources { listeners, clusters, secrets } = envoy;
            let listeners = convert_vec!(listeners)?;
            let secrets = convert_vec!(secrets)?;
            let clusters = convert_vec!(clusters)?;
            Ok(Self { listeners, clusters, secrets })
        }
    }

    impl TryFrom<EnvoyAdmin> for Admin {
        type Error = GenericError;
        fn try_from(envoy: EnvoyAdmin) -> Result<Self, Self::Error> {
            let EnvoyAdmin {
                access_log: _,
                access_log_path,
                profile_path: _,
                address,
                socket_options,
                ignore_global_conn_limit: _,
            } = envoy;
            unsupported_field!(access_log_path, socket_options)?;
            let address = match required!(address)?
                .address
                .ok_or(GenericError::MissingField("address is mandatory to setup admin interface"))?
            {
                address::Address::SocketAddress(sa) => {
                    let wrapper: SocketAddressWrapper = sa.try_into()?;
                    wrapper.0
                },
                _ => {
                    return Err(GenericError::UnsupportedVariant(std::borrow::Cow::Borrowed(
                        "Only SocketAddress is supported",
                    )));
                },
            };
            Ok(Self { address: crate::config::core::Address::Socket(address) })
        }
    }

    use num_traits::cast::ToPrimitive;
    impl TryFrom<EnvoyTypedExtensionConfig> for BootstrapExtension {
        type Error = GenericError;
        fn try_from(value: EnvoyTypedExtensionConfig) -> Result<Self, Self::Error> {
            let EnvoyTypedExtensionConfig { name, typed_config } = value;
            let name = required!(name)?;
            let typed_config = required!(typed_config)?;

            match name.as_str() {
                "internal_listener" | "envoy.bootstrap.internal_listener" => {
                    if typed_config.type_url
                        == "type.googleapis.com/envoy.extensions.bootstrap.internal_listener.v3.InternalListener"
                    {
                        let internal_listener = EnvoyInternalListener::decode(typed_config.value.as_slice())
                            .map_err(|e| GenericError::from_msg_with_cause("Failed to decode InternalListener", e))?;
                        Ok(BootstrapExtension::InternalListener(internal_listener.try_into()?))
                    } else if typed_config.type_url == "type.googleapis.com/udpa.type.v1.TypedStruct" {
                        let typed_struct = TypedStruct::decode(typed_config.value.as_slice())
                            .map_err(|e| GenericError::from_msg_with_cause("Failed to decode InternalListener", e))?;
                        debug!("Internal listener value {:?}", typed_struct);
                        if let Some(internl_listener_struct) = typed_struct.value {
                            if let Some(maybe_buffer_size) = internl_listener_struct.fields.get("buffer_size_kb") {
                                match maybe_buffer_size.kind {
                                    Some(Kind::NumberValue(number)) => {
                                        Ok(BootstrapExtension::InternalListener(InternalListenerBootstrap {
                                            buffer_size_kb: number.to_u32(),
                                        }))
                                    },
                                    _ => Err(GenericError::from_msg(
                                        "Can't parse internal listener value from typed struct ",
                                    )),
                                }
                            } else {
                                Err(GenericError::from_msg("Can't parse internal listener value from typed struct "))
                            }
                        } else {
                            Err(GenericError::from_msg("Can't parse internal listener value from typed struct "))
                        }
                    } else {
                        Err(GenericError::from_msg(format!(
                            "Unsupported bootstrap extension type: {}",
                            typed_config.type_url
                        )))
                    }
                },
                _ => Err(GenericError::unsupported_variant(name)),
            }
        }
    }

    impl TryFrom<EnvoyInternalListener> for InternalListenerBootstrap {
        type Error = GenericError;
        fn try_from(value: EnvoyInternalListener) -> Result<Self, Self::Error> {
            let EnvoyInternalListener { buffer_size_kb } = value;
            let buffer_size_kb = buffer_size_kb.map(|v| v.value);
            Ok(Self { buffer_size_kb })
        }
    }
}
