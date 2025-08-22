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

use crate::config::{cluster::Cluster, common::is_default, listener::Listener, secret::Secret};
use compact_str::CompactString;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct Bootstrap {
    #[serde(with = "serde_yaml::with::singleton_map_recursive", skip_serializing_if = "is_default", default)]
    pub static_resources: StaticResources,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    dynamic_resources: Option<DynamicResources>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub node: Option<Node>,
}

impl Bootstrap {
    pub fn get_ads_configs(&self) -> &[CompactString] {
        self.dynamic_resources.as_ref().map(|dr| dr.grpc_cluster_specifiers.as_slice()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub id: CompactString,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DynamicResources {
    pub grpc_cluster_specifiers: Vec<CompactString>,
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq)]
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
    use super::{Bootstrap, DynamicResources, Node, StaticResources};
    use crate::config::common::*;
    use compact_str::CompactString;
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::{
        bootstrap::v3::{
            bootstrap::{DynamicResources as EnvoyDynamicResources, StaticResources as EnvoyStaticResources},
            Bootstrap as EnvoyBootstrap,
        },
        core::v3::{
            grpc_service::{EnvoyGrpc, TargetSpecifier as EnvoyGrpcTargetSpecifier},
            ApiConfigSource as EnvoyApiConfigSource, GrpcService as EnvoyGrpcService, Node as EnvoyNode,
        },
    };

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
            let EnvoyBootstrap { node, static_resources, dynamic_resources, .. } = envoy;
            let static_resources = convert_opt!(static_resources)?;
            let dynamic_resources =
                dynamic_resources.map(DynamicResources::try_from).transpose().with_node("dynamic_resources")?;
            let node = node.map(Node::try_from).transpose().with_node("node")?;
            Ok(Self { static_resources, node, dynamic_resources })
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
                locality,
                user_agent_name,
                extensions,
                client_features,
                listening_addresses,
                user_agent_version_type,
            } = value;
            unsupported_field!(
                // id,
                cluster,
                metadata,
                dynamic_parameters,
                locality,
                user_agent_name,
                extensions,
                client_features,
                listening_addresses,
                user_agent_version_type
            )?;
            let id = required!(id)?.into();
            Ok(Self { id })
        }
    }
    impl TryFrom<EnvoyDynamicResources> for DynamicResources {
        type Error = GenericError;
        fn try_from(value: EnvoyDynamicResources) -> Result<Self, Self::Error> {
            let EnvoyDynamicResources {
                lds_config,
                lds_resources_locator,
                cds_config,
                cds_resources_locator,
                ads_config,
                ..
            } = value;
            unsupported_field!(lds_config, lds_resources_locator, cds_config, cds_resources_locator)?;
            let grpc_cluster_specifiers: Vec<compact_str::CompactString> = ads_config
                .map(|config| {
                    config
                        .grpc_services
                        .into_iter()
                        .filter_map(|service| match service.target_specifier {
                            Some(EnvoyGrpcTargetSpecifier::EnvoyGrpc(grpc)) if !grpc.cluster_name.is_empty() => {
                                Some(grpc.cluster_name.into())
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
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
}
