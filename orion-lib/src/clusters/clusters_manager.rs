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

use super::{
    balancers::hash_policy::HashState,
    cached_watch::{CachedWatch, CachedWatcher},
    cluster::ClusterType,
    health::HealthStatus,
    load_assignment::{ClusterLoadAssignmentBuilder, PartialClusterLoadAssignment},
};
use crate::{
    body::{body_with_metrics::BodyWithMetrics, body_with_timeout::BodyWithTimeout},
    clusters::cluster::{ClusterOps, PartialClusterType},
    secrets::TransportSecret,
    transport::{GrpcService, HttpChannel, TcpChannelConnector},
    Result,
};
use http::{uri::Authority, HeaderName, HeaderValue, Request};
use hyper::body::Incoming;
use orion_configuration::config::cluster::{Cluster as ClusterConfig, ClusterSpecifier as ClusterSpecifierConfig};
use orion_interner::StringInterner;
use rand::{prelude::SliceRandom, thread_rng};
use std::{
    cell::RefCell,
    collections::{btree_map::Entry as BTreeEntry, BTreeMap},
};
use tracing::warn;

type ClusterID = &'static str;
type ClustersMap = BTreeMap<ClusterID, ClusterType>;

#[derive(Debug, Clone, PartialEq)]
pub enum RoutingRequirement {
    None,
    Header(HeaderName),
    Authority,
    Hash,
}

pub enum RoutingContext<'a> {
    None,
    Header(&'a HeaderValue),
    Authority(Authority),
    Hash(HashState<'a>),
}

impl<'a> TryFrom<(&'a RoutingRequirement, &'a Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>, HashState<'a>)>
    for RoutingContext<'a>
{
    type Error = String;

    fn try_from(
        value: (&'a RoutingRequirement, &'a Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>, HashState<'a>),
    ) -> std::result::Result<Self, Self::Error> {
        let (routing_requirement, request, hash_state) = value;
        match routing_requirement {
            RoutingRequirement::Header(header_name) => {
                let header_value = request
                    .headers()
                    .get(header_name)
                    .ok_or_else(|| format!("Missing required header '{header_name}' for ORIGINAL_DST cluster"))?;
                Ok(RoutingContext::Header(header_value))
            },
            RoutingRequirement::Authority => {
                let msg = "Routing by Authority is not currently supported, coming soon".to_owned();
                warn!(msg);
                Err(msg)
            },
            RoutingRequirement::Hash => Ok(RoutingContext::Hash(hash_state)),
            RoutingRequirement::None => Ok(RoutingContext::None),
        }
    }
}

static CLUSTERS_MAP: CachedWatch<ClustersMap> = CachedWatch::new(ClustersMap::new());

thread_local! {
    static CLUSTERS_MAP_CACHE : RefCell<CachedWatcher<'static, ClustersMap>> = RefCell::new(CLUSTERS_MAP.watcher());
}

pub fn resolve_cluster(selector: &ClusterSpecifierConfig) -> Option<ClusterID> {
    match selector {
        ClusterSpecifierConfig::Cluster(cluster_name) => Some(cluster_name.to_static_str()),
        ClusterSpecifierConfig::WeightedCluster(weighted_clusters) => weighted_clusters
            .choose_weighted(&mut thread_rng(), |cluster| u32::from(cluster.weight))
            .ok()
            .map(|cluster| cluster.cluster.to_static_str()),
    }
}

pub fn get_cluster_routing_requirements(cluster_id: ClusterID) -> RoutingRequirement {
    with_cluster(cluster_id, |cluster| Ok(cluster.get_routing_requirements())).unwrap_or(RoutingRequirement::None)
}

pub fn change_cluster_load_assignment(name: &str, cla: &PartialClusterLoadAssignment) -> Result<ClusterType> {
    CLUSTERS_MAP.update(|current| {
        if let Some(cluster) = current.get_mut(name) {
            match cluster {
                ClusterType::Dynamic(dynamic_cluster) => {
                    let cla = ClusterLoadAssignmentBuilder::builder()
                        .with_cla(cla.clone())
                        .with_transport_socket(dynamic_cluster.transport_socket.clone())
                        .with_cluster_name(dynamic_cluster.name)
                        .with_bind_device(dynamic_cluster.bind_device.clone())
                        .with_lb_policy(dynamic_cluster.load_balancing_policy)
                        .prepare();
                    cla.build().map(|cla| dynamic_cluster.change_load_assignment(Some(cla)))?;
                    Ok(cluster.clone())
                },
                ClusterType::Static(_) => {
                    let msg = format!("{name} Attempt to change CLA for static cluster");
                    warn!(msg);
                    Err(msg.into())
                },
                ClusterType::OnDemand(_) => {
                    let msg = format!("{name} Attempt to change CLA for ORIGINAL_DST cluster");
                    warn!(msg);
                    Err(msg.into())
                },
            }
        } else {
            let msg = format!("{name} No cluster found");
            warn!(msg);
            Err(msg.into())
        }
    })
}

pub fn remove_cluster_load_assignment(name: &str) -> Result<()> {
    CLUSTERS_MAP.update(|current| {
        let maybe_cluster = current.get_mut(name);
        if let Some(cluster) = maybe_cluster {
            match cluster {
                ClusterType::Dynamic(cluster) => {
                    cluster.change_load_assignment(None);
                    Ok(())
                },
                ClusterType::Static(_) => {
                    let msg = format!("{name} Attempt to change CLA for static cluster");
                    warn!(msg);
                    Err(msg.into())
                },
                ClusterType::OnDemand(_) => {
                    let msg = format!("{name} Attempt to change CLA for ORIGINAL_DST cluster");
                    warn!(msg);
                    Err(msg.into())
                },
            }
        } else {
            let msg = format!("{name} No cluster found");
            warn!(msg);
            Err(msg.into())
        }
    })
}

pub fn update_endpoint_health(cluster: &str, endpoint: &Authority, health: HealthStatus) {
    CLUSTERS_MAP.update(|current| {
        if let Some(cluster) = current.get_mut(cluster) {
            cluster.update_health(endpoint, health);
        }
    });
}

pub fn update_tls_context(secret_id: &str, secret: &TransportSecret) -> Result<Vec<ClusterType>> {
    CLUSTERS_MAP.update(|current| {
        let mut cluster_configs = Vec::with_capacity(current.len());
        for cluster in current.values_mut() {
            cluster.change_tls_context(secret_id, secret.clone())?;
            cluster_configs.push(cluster.clone());
        }
        Ok(cluster_configs)
    })
}

pub fn add_cluster(partial_cluster: PartialClusterType) -> Result<ClusterType> {
    let cluster = partial_cluster.build();
    dbg!(&cluster);
    let cluster = cluster?;

    let cluster_name = cluster.get_name();

    CLUSTERS_MAP.update(|current| match current.entry(cluster_name) {
        BTreeEntry::Vacant(entry) => {
            entry.insert(cluster.clone());
            Ok(cluster)
        },
        BTreeEntry::Occupied(mut entry) => {
            *(entry.get_mut()) = cluster.clone();
            Ok(cluster)
        },
    })
}

pub fn remove_cluster(cluster_name: &str) -> Result<()> {
    CLUSTERS_MAP.update(|current| current.remove(cluster_name).map(|_| ()).ok_or("No such cluster".into()))
}

pub fn get_all_clusters() -> Vec<ClusterConfig> {
    CLUSTERS_MAP.get_clone().0.values().by_ref().filter_map(|cluster| ClusterConfig::try_from(cluster).ok()).collect()
}

pub fn get_http_connection(cluster_id: ClusterID, context: RoutingContext) -> Result<HttpChannel> {
    with_cluster(cluster_id, |cluster| cluster.get_http_connection(context))
}

pub fn get_tcp_connection(cluster_id: ClusterID, context: RoutingContext) -> Result<TcpChannelConnector> {
    with_cluster(cluster_id, |cluster| cluster.get_tcp_connection(context))
}

pub fn get_grpc_connection(cluster_id: ClusterID, context: RoutingContext) -> Result<GrpcService> {
    with_cluster(cluster_id, |cluster| cluster.get_grpc_connection(context))
}

pub fn all_http_connections(cluster_id: ClusterID) -> Result<Vec<(Authority, HttpChannel)>> {
    with_cluster(cluster_id, |cluster| Ok(cluster.all_http_channels()))
}

pub fn all_tcp_connections(cluster_id: ClusterID) -> Result<Vec<(Authority, TcpChannelConnector)>> {
    with_cluster(cluster_id, |cluster| Ok(cluster.all_tcp_channels()))
}

pub fn all_grpc_connections(cluster_id: ClusterID) -> Result<Vec<Result<(Authority, GrpcService)>>> {
    with_cluster(cluster_id, |cluster| Ok(cluster.all_grpc_channels()))
}

fn with_cluster<F, R>(cluster_id: &str, f: F) -> Result<R>
where
    F: FnOnce(&mut ClusterType) -> Result<R>,
{
    CLUSTERS_MAP_CACHE.with_borrow_mut(|watcher| {
        if let Some(cluster) = watcher.cached_or_latest().get_mut(cluster_id) {
            f(cluster)
        } else {
            Err(format!("Cluster {cluster_id} not found").into())
        }
    })
}
