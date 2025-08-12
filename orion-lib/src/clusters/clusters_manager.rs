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
    cluster::{ClusterType, TcpService},
    health::HealthStatus,
    load_assignment::{ClusterLoadAssignmentBuilder, PartialClusterLoadAssignment},
};
use crate::clusters::cluster::ClusterOps;
use crate::transport::{GrpcService, TcpChannel};
use crate::PartialClusterType;
use crate::Result;
use crate::{secrets::TransportSecret, transport::HttpChannel};
use compact_str::CompactString;
use http::uri::Authority;
use orion_configuration::config::cluster::ClusterSpecifier as ClusterSpecifierConfig;
use rand::prelude::SliceRandom;
use rand::thread_rng;
use std::cell::RefCell;
use std::collections::{btree_map::Entry as BTreeEntry, BTreeMap};
use tracing::{debug, warn};

type ClustersMap = BTreeMap<CompactString, ClusterType>;

static CLUSTERS_MAP: CachedWatch<ClustersMap> = CachedWatch::new(ClustersMap::new());

thread_local! {
    static CLUSTERS_MAP_CACHE : RefCell<CachedWatcher<'static, ClustersMap>> = RefCell::new(CLUSTERS_MAP.watcher());
}

pub fn change_cluster_load_assignment(name: &str, cla: &PartialClusterLoadAssignment) -> Result<ClusterType> {
    CLUSTERS_MAP.update(|current| {
        if let Some(cluster) = current.get_mut(name) {
            match cluster {
                ClusterType::Dynamic(dynamyc_cluster) => {
                    let cla = ClusterLoadAssignmentBuilder::builder()
                        .with_cla(cla.clone())
                        .with_tls_configurator(dynamyc_cluster.tls_configurator.clone())
                        .with_cluster_name(dynamyc_cluster.name.clone())
                        .with_bind_device(dynamyc_cluster.bind_device.clone())
                        .with_lb_policy(dynamyc_cluster.load_balancing_policy)
                        .prepare();
                    cla.build().map(|cla| dynamyc_cluster.change_load_assignment(Some(cla)))?;
                    Ok(cluster.clone())
                },
                ClusterType::Static(_) => {
                    let msg = format!("{name} Attempt to change CLA for static cluster");
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
    let cluster = partial_cluster.build()?;
    let cluster_name = cluster.get_name().clone();

    CLUSTERS_MAP.update(|current| match current.entry(cluster_name) {
        BTreeEntry::Vacant(entry) => {
            entry.insert(cluster.clone());
            Ok(cluster)
        },
        BTreeEntry::Occupied(entry) => {
            let cluster_name = entry.key();
            Err(format!("Cluster {cluster_name} already exists... need to remove it first").into())
        },
    })
}

pub fn remove_cluster(cluster_name: &str) -> Result<()> {
    CLUSTERS_MAP.update(|current| current.remove(cluster_name).map(|_| ()).ok_or("No such cluster".into()))
}

pub fn get_http_connection(selector: &ClusterSpecifierConfig, lb_hash: HashState) -> Result<HttpChannel> {
    debug!("Http connection for {selector:?}");
    with_cluster_selector(selector, |cluster| cluster.get_http_connection(lb_hash))
}

pub fn get_tcp_connection(selector: &ClusterSpecifierConfig) -> Result<TcpService> {
    with_cluster_selector(selector, ClusterOps::get_tcp_connection)
}

pub fn get_grpc_connection(selector: &ClusterSpecifierConfig) -> Result<GrpcService> {
    with_cluster_selector(selector, ClusterOps::get_grpc_connection)
}

pub fn all_http_connections(cluster_name: &str) -> Result<Vec<(Authority, HttpChannel)>> {
    with_cluster(cluster_name, |cluster| Ok(cluster.all_http_channels()))
}

pub fn all_tcp_connections(cluster_name: &str) -> Result<Vec<(Authority, TcpChannel)>> {
    with_cluster(cluster_name, |cluster| Ok(cluster.all_tcp_channels()))
}

pub fn all_grpc_connections(cluster_name: &str) -> Result<Vec<Result<(Authority, GrpcService)>>> {
    with_cluster(cluster_name, |cluster| Ok(cluster.all_grpc_channels()))
}

fn with_cluster_selector<F, R>(selector: &ClusterSpecifierConfig, f: F) -> Result<R>
where
    F: FnOnce(&mut ClusterType) -> Result<R>,
{
    let cluster_name = match selector {
        ClusterSpecifierConfig::Cluster(cluster_name) => cluster_name,
        ClusterSpecifierConfig::WeightedCluster(weighted_clusters) => {
            &weighted_clusters.choose_weighted(&mut thread_rng(), |cluster| u32::from(cluster.weight))?.cluster
        },
    };

    with_cluster(cluster_name, f)
}

fn with_cluster<F, R>(cluster_name: &str, f: F) -> Result<R>
where
    F: FnOnce(&mut ClusterType) -> Result<R>,
{
    CLUSTERS_MAP_CACHE.with_borrow_mut(|watcher| {
        if let Some(cluster) = watcher.cached_or_latest().get_mut(cluster_name) {
            f(cluster)
        } else {
            Err(format!("Cluster {cluster_name} not found").into())
        }
    })
}
