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

use crate::{
    core_affinity,
    runtime::{self, RuntimeId},
    xds_configurator::XdsConfigurationHandler,
};
use futures::future::join_all;
use orion_configuration::config::{bootstrap::Node, runtime::Affinity, Bootstrap};
use orion_error::Context;
use orion_lib::{
    get_listeners_and_clusters, new_configuration_channel, runtime_config, ConfigurationReceivers,
    ConfigurationSenders, ListenerConfigurationChange, Result, SecretManager,
};
use std::thread::{self, JoinHandle};
use tokio::{sync::mpsc::Sender, task::JoinSet};
use tracing::{debug, info, warn};

pub fn run_proxy(bootstrap: Bootstrap) -> Result<()> {
    debug!("Starting on thread {:?}", std::thread::current().name());
    launch_runtimes(bootstrap).context("failed to launch runtimes")
}

fn calculate_threads_per_runtime(num_cpus: usize, num_runtimes: usize) -> Result<usize> {
    let avail_cpus = core_affinity::get_avail_core_num()?;
    if num_cpus > avail_cpus {
        return Err(
            format!("The number of CPUs ({num_cpus}) exceeds those available for this process ({avail_cpus})").into()
        );
    }

    let threads = num_cpus / num_runtimes;
    if threads == 0 {
        return Err(
            format!("The number of runtimes greater than the number of cpus ({num_cpus} < {num_runtimes})").into()
        );
    }

    if num_cpus % num_runtimes != 0 {
        return Err(format!(
            "The number of CPUs ({num_cpus}) is not a multiple of the number of runtimes ({num_runtimes})",
        )
        .into());
    }

    Ok(threads)
}

struct ServiceInfo {
    bootstrap: Bootstrap,
    node: Node,
    configuration_senders: Vec<ConfigurationSenders>,
    secret_manager: SecretManager,
    listener_factories: Vec<orion_lib::ListenerFactory>,
    clusters: Vec<orion_lib::PartialClusterType>,
    ads_cluster_names: Vec<String>,
}

fn launch_runtimes(bootstrap: Bootstrap) -> Result<()> {
    let rt_config = runtime_config();

    let num_runtimes = rt_config.num_runtimes();
    let num_cpus = rt_config.num_cpus();
    info!("Launching with {} cpus, {} runtimes", num_cpus, num_runtimes);

    let handles = {
        let num_threads_per_runtime = calculate_threads_per_runtime(num_cpus, num_runtimes)
            .context("failed to calculate number of threads to use per runtime")?;
        info!("using {} runtimes with {num_threads_per_runtime} threads each", rt_config.num_runtimes());

        (0..num_runtimes)
            .map(|id| {
                spawn_proxy_runtime_from_thread(
                    "proxy",
                    num_threads_per_runtime,
                    rt_config.affinity_strategy.clone().map(|affinity| (RuntimeId(id), affinity)),
                )
            })
            .collect::<Result<Vec<_>>>()?
    };

    let (handles, configuration_senders): (Vec<_>, Vec<_>) = handles.into_iter().unzip();

    // The xDS runtime always runs - this is necessary for initialization even if we do not
    // use dynamic updates from remote xDS servers. The decision on whether dynamic updates
    // are used is based on:
    // - The bootstrap loader from orion-data-plane-api gets the list of cluster names used
    //   in dynamic_resources/ads_config (for grpc_services)
    // - resolve ads clusters into endpoints, to be used as xDS address
    // TODO: the xDS client could receive updates for endpoints too i.e. dynamic clusters. We
    // should replace this with passing a configuration receiver. For now endpoints from
    // static clusters.

    let ads_cluster_names: Vec<String> = bootstrap.get_ads_configs().iter().map(ToString::to_string).collect();
    let node = bootstrap.node.clone().unwrap_or_else(|| Node { id: "".into() });

    let (secret_manager, listener_factories, clusters) =
        get_listeners_and_clusters(bootstrap.clone()).context("Failed to get listeners and clusters")?;

    if listener_factories.is_empty() && ads_cluster_names.is_empty() {
        return Err("No listeners and no ads clusters configured".into());
    }

    let service_info = ServiceInfo {
        node,
        configuration_senders: configuration_senders.clone(),
        secret_manager,
        listener_factories,
        bootstrap,
        clusters,
        ads_cluster_names,
    };

    spawn_service_runtime_from_thread("services", rt_config.num_service_threads.get() as usize, None, service_info)?;

    for handle in handles {
        if let Err(err) = handle.join() {
            warn!("Closing handler with error {err:?}");
        }
    }
    Ok(())
}

type RuntimeHandle = JoinHandle<Result<()>>;

fn spawn_proxy_runtime_from_thread(
    thread_name: &'static str,
    num_threads: usize,
    affinity_info: Option<(RuntimeId, Affinity)>,
) -> Result<(RuntimeHandle, ConfigurationSenders)> {
    let (configuration_senders, configuration_receivers) = new_configuration_channel(100);

    let thread_name = match &affinity_info {
        Some((runtime_id, _affinity)) => format!("{thread_name}_RT{runtime_id}"),
        None => format!("{thread_name}_rt"),
    };

    let handle: JoinHandle<Result<()>> = thread::Builder::new().name(thread_name.clone()).spawn(move || {
        let rt = runtime::build_tokio_runtime(&thread_name, num_threads, affinity_info);
        rt.block_on(async {
            tokio::select! {
                _ = start_proxy(configuration_receivers) => {
                    info!("Proxy Runtime terminated!");
                    Ok(())
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("CTRL+C (Proxy runtime)!");
                    Ok(())
                }
            }
        })
    })?;
    Ok((handle, configuration_senders))
}

fn spawn_service_runtime_from_thread(
    thread_name: &'static str,
    num_threads: usize,
    affinity_info: Option<(RuntimeId, Affinity)>,
    service_info: ServiceInfo,
) -> Result<RuntimeHandle> {
    let thread_name = match &affinity_info {
        Some((runtime_id, _affinity)) => format!("{thread_name}_RT{runtime_id}"),
        None => format!("{thread_name}_rt"),
    };

    let rt_handle = thread::Builder::new().name(thread_name.clone()).spawn(move || {
        let rt = runtime::build_tokio_runtime(&thread_name, num_threads, affinity_info);
        rt.block_on(async {
            tokio::select! {
                () = run_services(service_info) => {
                    info!("Service Runtime terminated!");
                    Ok(())
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("CTRL+C (service runtime)!");
                    Ok(())
                }
            }
        })
    })?;

    Ok(rt_handle)
}

#[allow(clippy::too_many_arguments)]
async fn run_services(info: ServiceInfo) {
    let ServiceInfo {
        bootstrap: _,
        node,
        configuration_senders,
        secret_manager,
        listener_factories,
        clusters,
        ads_cluster_names,
    } = info;
    let mut set: JoinSet<Result<()>> = JoinSet::new();

    // spawn XSD configuration service...

    set.spawn(async move {
        let secret_manager =
            configure_initial_resources(secret_manager, listener_factories, configuration_senders.clone()).await?;
        let xds_handler = XdsConfigurationHandler::new(secret_manager, configuration_senders);
        _ = xds_handler.xds_run(node, clusters, ads_cluster_names).await;
        Ok(())
    });

    set.join_all().await;
}

async fn configure_initial_resources(
    secret_manager: SecretManager,
    listeners: Vec<orion_lib::ListenerFactory>,
    configuration_senders: Vec<ConfigurationSenders>,
) -> Result<SecretManager> {
    let listeners_tx: Vec<_> = configuration_senders
        .into_iter()
        .map(|ConfigurationSenders { listener_configuration_sender, route_configuration_sender: _ }| {
            listener_configuration_sender
        })
        .collect();

    for listener in listeners {
        let _ = join_all(listeners_tx.iter().map(|listener_tx: &Sender<ListenerConfigurationChange>| {
            listener_tx.send(ListenerConfigurationChange::Added(listener.clone()))
        }))
        .await;
    }

    Ok(secret_manager)
}

async fn start_proxy(configuration_receivers: ConfigurationReceivers) -> Result<()> {
    orion_lib::start_listener_manager(configuration_receivers).await?;
    Ok(())
}
