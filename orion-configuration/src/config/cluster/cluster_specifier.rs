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

use compact_str::CompactString;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ClusterSpecifier {
    Cluster(CompactString),
    WeightedCluster(Vec<WeightedClusterSpecifier>),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct WeightedClusterSpecifier {
    pub cluster: CompactString,
    pub weight: NonZeroU32,
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{ClusterSpecifier, WeightedClusterSpecifier};
    use crate::config::common::*;
    use compact_str::CompactString;
    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        config::route::v3::{
            route_action::ClusterSpecifier as EnvoyClusterSpecifier,
            weighted_cluster::ClusterWeight as EnvoyClusterWeight, WeightedCluster as EnvoyWeightedCluster,
        },
        extensions::filters::network::tcp_proxy::v3::tcp_proxy::{
            weighted_cluster::ClusterWeight as EnvoyTcpClusterWeight, ClusterSpecifier as EnvoyTcpClusterSpecifier,
            WeightedCluster as EnvoyTcpWeightedCluster,
        },
    };

    impl TryFrom<EnvoyWeightedCluster> for ClusterSpecifier {
        type Error = GenericError;
        fn try_from(value: EnvoyWeightedCluster) -> Result<Self, Self::Error> {
            let EnvoyWeightedCluster { clusters, total_weight, runtime_key_prefix, random_value_specifier } = value;
            unsupported_field!(total_weight, runtime_key_prefix, random_value_specifier)?;
            let clusters: Vec<WeightedClusterSpecifier> = convert_non_empty_vec!(clusters)?;
            let mut sum = 0u32;
            for cluster in &clusters {
                sum = sum.checked_add(cluster.weight.into()).ok_or(
                    GenericError::from_msg("sum of cluster weights must not exceed 4_294_967_295")
                        .with_node("clusters"),
                )?
            }
            if sum == 0 {
                return Err(GenericError::from_msg("sum of cluster weights must be > 0"));
            }
            Ok(Self::WeightedCluster(clusters))
        }
    }

    impl TryFrom<EnvoyClusterSpecifier> for ClusterSpecifier {
        type Error = GenericError;
        fn try_from(value: EnvoyClusterSpecifier) -> Result<Self, Self::Error> {
            match value {
                EnvoyClusterSpecifier::Cluster(cluster) => {
                    required!(cluster).map(CompactString::from).map(Self::Cluster)
                },
                EnvoyClusterSpecifier::WeightedClusters(envoy) => envoy.try_into(),
                EnvoyClusterSpecifier::ClusterHeader(_) => Err(GenericError::unsupported_variant("ClusterHeader")),
                EnvoyClusterSpecifier::ClusterSpecifierPlugin(_) => {
                    Err(GenericError::unsupported_variant("ClusterSpecifierPlugin"))
                },
                EnvoyClusterSpecifier::InlineClusterSpecifierPlugin(_) => {
                    Err(GenericError::unsupported_variant("InlineClusterSpecifierPlugin"))
                },
            }
        }
    }

    impl TryFrom<EnvoyClusterWeight> for WeightedClusterSpecifier {
        type Error = GenericError;
        fn try_from(value: EnvoyClusterWeight) -> Result<Self, Self::Error> {
            let EnvoyClusterWeight {
                name,
                cluster_header,
                weight,
                metadata_match,
                request_headers_to_add,
                request_headers_to_remove,
                response_headers_to_add,
                response_headers_to_remove,
                typed_per_filter_config,
                host_rewrite_specifier,
            } = value;
            unsupported_field!(
                // name,
                cluster_header,
                // weight,
                metadata_match,
                request_headers_to_add,
                request_headers_to_remove,
                response_headers_to_add,
                response_headers_to_remove,
                typed_per_filter_config,
                host_rewrite_specifier
            )?;
            let cluster: CompactString = required!(name)?.into();
            (|| -> Result<_, GenericError> {
                // we could allow for default = 1 if missing in ng to allow equaly balanced clusters with shorthand notation
                let weight = weight.map(|x| x.value).ok_or(GenericError::MissingField("weight"))?;
                let weight = weight
                    .try_into()
                    .map_err(|_| GenericError::from_msg("clusterweight has to be > 0"))
                    .with_node("weight")?;
                Ok(Self { cluster: cluster.clone(), weight })
            })()
            .with_name(cluster)
        }
    }

    impl TryFrom<EnvoyTcpClusterSpecifier> for ClusterSpecifier {
        type Error = GenericError;
        fn try_from(value: EnvoyTcpClusterSpecifier) -> Result<Self, Self::Error> {
            match value {
                EnvoyTcpClusterSpecifier::Cluster(cluster) => Ok(Self::Cluster(cluster.into())),
                EnvoyTcpClusterSpecifier::WeightedClusters(wc) => wc.try_into(),
            }
        }
    }

    impl TryFrom<EnvoyTcpWeightedCluster> for ClusterSpecifier {
        type Error = GenericError;
        fn try_from(value: EnvoyTcpWeightedCluster) -> Result<Self, Self::Error> {
            let EnvoyTcpWeightedCluster { clusters } = value;
            let clusters: Vec<WeightedClusterSpecifier> = convert_non_empty_vec!(clusters)?;
            let mut sum = 0u32;
            for cluster in &clusters {
                sum = sum.checked_add(cluster.weight.into()).ok_or(
                    GenericError::from_msg("sum of cluster weights must not exceed 4_294_967_295")
                        .with_node("clusters"),
                )?;
            }
            Ok(Self::WeightedCluster(clusters))
        }
    }

    impl TryFrom<EnvoyTcpClusterWeight> for WeightedClusterSpecifier {
        type Error = GenericError;
        fn try_from(value: EnvoyTcpClusterWeight) -> Result<Self, Self::Error> {
            let EnvoyTcpClusterWeight { name, weight, metadata_match } = value;
            unsupported_field!(metadata_match)?;
            let cluster = required!(name)?.into();
            let weight = weight
                .try_into()
                .map_err(|_| GenericError::from_msg("clusterweight has to be > 0"))
                .with_node("weight")?;
            Ok(Self { cluster, weight })
        }
    }
}
