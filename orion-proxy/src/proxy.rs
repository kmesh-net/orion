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

use crate::{
    admin::start_admin_server,
    core_affinity,
    runtime::{self, RuntimeId},
    signal::wait_signal,
    xds_configurator::XdsConfigurationHandler,
};
use compact_str::ToCompactString;
use futures::future::join_all;
use orion_configuration::config::{
    bootstrap::Node,
    log::AccessLogConfig,
    network_filters::tracing::{TracingConfig, TracingKey},
    runtime::Affinity,
    Bootstrap,
};
use orion_error::Context;
use orion_lib::{
    access_log::{start_access_loggers, update_configuration, Target},
    clusters::cluster::ClusterType,
    get_listeners_and_clusters, new_configuration_channel, runtime_config, ConfigurationReceivers,
    ConfigurationSenders, ListenerConfigurationChange, PartialClusterType, Result, SecretManager,
};
use orion_metrics::{metrics::init_global_metrics, wait_for_metrics_setup, Metrics, VecMetrics};
use parking_lot::RwLock;
use std::{
    collections::HashMap,
    sync::Arc,
    thread::{self, JoinHandle},
};
use tokio::{sync::mpsc::Sender, task::JoinSet};
use tracing::{debug, info, warn};

pub fn run_orion(bootstrap: Bootstrap, access_log_config: Option<AccessLogConfig>) {
    debug!("Starting on thread {:?}", std::thread::current().name());

    let ct = tokio_util::sync::CancellationToken::new();
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        // Set up signal handling and shutdown notification channel
        wait_signal().await;
        // Trigger cancellation
        ct_clone.cancel();
    });

    // launch the runtimes...
    let res = launch_runtimes(bootstrap, access_log_config, ct);
    if let Err(err) = res {
        warn!("Error running orion: {err}");
    }
}

fn calculate_num_threads_per_runtime(num_cpus: usize, num_runtimes: usize) -> Result<usize> {
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

#[derive(Debug, Clone)]
struct ProxyConfiguration {
    bootstrap: Bootstrap,
    node: Node,
    configuration_senders: Vec<ConfigurationSenders>,
    secret_manager: Arc<RwLock<SecretManager>>,
    listener_factories: Vec<orion_lib::ListenerFactory>,
    clusters: Vec<orion_lib::PartialClusterType>,
    ads_cluster_names: Vec<String>,
    access_log_config: Option<AccessLogConfig>,
    tracing: HashMap<TracingKey, TracingConfig>,
    metrics: Vec<Metrics>,
}

fn launch_runtimes(
    bootstrap: Bootstrap,
    access_log_config: Option<AccessLogConfig>,
    ct: tokio_util::sync::CancellationToken,
) -> Result<Vec<ConfigurationSenders>> {
    let rt_config = runtime_config();
    let num_runtimes = rt_config.num_runtimes();
    let num_cpus = rt_config.num_cpus();

    // build the XDS configuration channels...
    //

    let (config_senders, config_receivers): (Vec<ConfigurationSenders>, Vec<ConfigurationReceivers>) =
        (0..num_runtimes).map(|_| new_configuration_channel(100)).collect::<Vec<_>>().into_iter().unzip();

    // launch services runtime...
    //

    let metrics = VecMetrics::from(&bootstrap).0;
    let are_metrics_empty = metrics.is_empty();

    let tracing = bootstrap
        .static_resources
        .listeners
        .iter()
        .flat_map(orion_configuration::config::Listener::get_tracing_configurations)
        .collect::<HashMap<_, _>>();

    let ads_cluster_names: Vec<String> = bootstrap.get_ads_configs().iter().map(ToString::to_string).collect();
    let node = bootstrap.node.clone().unwrap_or_else(|| Node { id: "".into(), cluster_id: "".into(), metadata: None });

    let (secret_manager, listener_factories, clusters) = get_listeners_and_clusters(bootstrap.clone())?;
    let secret_manager = Arc::new(RwLock::new(secret_manager));

    if listener_factories.is_empty() && ads_cluster_names.is_empty() {
        return Err("No listeners and no ads clusters configured".into());
    }

    let config = ProxyConfiguration {
        node,
        configuration_senders: config_senders.clone(),
        secret_manager,
        listener_factories,
        clusters,
        ads_cluster_names,
        access_log_config,
        tracing,
        metrics: metrics.clone(),
        bootstrap,
    };

    let services_handle = spawn_services_runtime_from_thread(
        "services",
        rt_config.num_service_threads.get() as usize,
        None,
        config,
        ct.clone(),
    )?;

    if !are_metrics_empty {
        info!("Waiting for metrics setup to complete...");
        wait_for_metrics_setup();
    }

    //////////////////////////////////////////////////////////////////////////////////////////
    // run the proxy runtimes...
    //

    let num_threads_per_runtime = calculate_num_threads_per_runtime(num_cpus, num_runtimes)
        .with_context_msg("failed to calculate number of threads to use per runtime")?;
    info!("using {} runtimes with {num_threads_per_runtime} threads each", rt_config.num_runtimes());

    // initialize global metrics...
    init_global_metrics(&metrics, num_threads_per_runtime * num_runtimes);

    info!("Launching with {} cpus, {} runtimes", num_cpus, num_runtimes);

    let mut handlers = {
        (0..num_runtimes)
            .zip(config_receivers.into_iter())
            .map(|(id, config_receivers)| {
                spawn_proxy_runtime_from_thread(
                    "proxy",
                    num_threads_per_runtime,
                    metrics.clone(),
                    rt_config.affinity_strategy.clone().map(|affinity| (RuntimeId(id), affinity)),
                    config_receivers,
                    ct.clone(),
                )
            })
            .collect::<Result<Vec<_>>>()?
    };

    handlers.push(services_handle);

    for h in handlers {
        if let Err(err) = h.join() {
            warn!("Closing handler with error {err:?}");
        }
    }
    Ok(config_senders)
}

fn spawn_proxy_runtime_from_thread(
    thread_name: &'static str,
    num_threads: usize,
    metrics: Vec<Metrics>,
    affinity_info: Option<(RuntimeId, Affinity)>,
    configuration_receivers: ConfigurationReceivers,
    ct: tokio_util::sync::CancellationToken,
) -> Result<JoinHandle<()>> {
    let thread_name = build_thread_name(thread_name, affinity_info.as_ref());

    let handle = thread::Builder::new().name(thread_name.clone()).spawn(move || {
        let rt = runtime::build_tokio_runtime(&thread_name, num_threads, affinity_info, Some(metrics));
        rt.block_on(async {
            tokio::select! {
                _ = start_proxy(configuration_receivers, ct.clone()) => {
                    info!("Proxy Runtime terminated!");
                }
                _ = ct.cancelled() => {
                    info!("Shutdown channel closed, shutting down Proxy runtime!");
                }
            }
        });
    })?;
    Ok(handle)
}

fn spawn_services_runtime_from_thread(
    thread_name: &'static str,
    threads_num: usize,
    affinity_info: Option<(RuntimeId, Affinity)>,
    config: ProxyConfiguration,
    ct: tokio_util::sync::CancellationToken,
) -> Result<JoinHandle<()>> {
    let thread_name = build_thread_name(thread_name, affinity_info.as_ref());
    let rt_handle = thread::Builder::new().name(thread_name.clone()).spawn(move || {
        let rt = runtime::build_tokio_runtime(&thread_name, threads_num, affinity_info, None);
        rt.block_on(async {
            tokio::select! {
                result = run_services(config) => {
                    if let Err(err) = result {
                        warn!("Error in services runtime: {err:?}");
                    }
                    info!("Services Runtime terminated!");
                }
                _ = ct.cancelled() => {
                    info!("Shutdown channel closed, shutting down Services runtime!");
                }
            }
        });
    })?;
    Ok(rt_handle)
}

fn build_thread_name(thread_name: &'static str, affinity_info: Option<&(RuntimeId, Affinity)>) -> String {
    match affinity_info {
        Some((runtime_id, _)) => format!("{thread_name}_RT{runtime_id}"),
        None => format!("{thread_name}_RT"),
    }
}

async fn run_services(config: ProxyConfiguration) -> Result<()> {
    let ProxyConfiguration {
        bootstrap,
        node,
        configuration_senders,
        secret_manager,
        listener_factories,
        clusters,
        ads_cluster_names,
        access_log_config,
        metrics,
        #[allow(unused_variables)]
        tracing,
    } = config;
    let mut set: JoinSet<Result<()>> = JoinSet::new();

    // spawn XDS configuration service...
    spawn_xds_client(
        &mut set,
        bootstrap.clone(),
        node,
        configuration_senders.clone(),
        secret_manager.clone(),
        listener_factories,
        clusters,
        ads_cluster_names,
    );

    // spawn access loggers service...
    if let Some(conf) = access_log_config {
        spawn_access_loggers(&mut set, bootstrap.clone(), conf);
    }

    // spawn admin interface task
    if bootstrap.admin.is_some() {
        spawn_admin_service(&mut set, bootstrap, configuration_senders, secret_manager);
    }

    // spawn metrics exporter...
    if metrics.is_empty() {
        info!("OTEL metrics: stats_sink not configured (skipped)");
    } else {
        #[cfg(feature = "metrics")]
        orion_metrics::otel_launch_exporter(&metrics).await?;
    }

    // spawn tracing exporters...
    if tracing.is_empty() {
        info!("OTEL tracing: no tracers configured (skipped)");
    } else {
        #[cfg(feature = "tracing")]
        orion_tracing::otel_update_tracers(tracing)?;
    }

    set.join_all().await;
    Ok(())
}

fn spawn_xds_client(
    set: &mut JoinSet<Result<()>>,
    bootstrap: Bootstrap,
    node: Node,
    configuration_senders: Vec<ConfigurationSenders>,
    secret_manager: Arc<RwLock<SecretManager>>,
    listener_factories: Vec<orion_lib::ListenerFactory>,
    clusters: Vec<orion_lib::PartialClusterType>,
    ads_cluster_names: Vec<String>,
) {
    set.spawn(async move {
        let initial_clusters =
            configure_initial_resources(bootstrap, listener_factories, clusters, configuration_senders.clone()).await?;
        if !ads_cluster_names.is_empty() {
            let mut xds_handler = XdsConfigurationHandler::new(secret_manager, configuration_senders);
            _ = xds_handler.run_loop(node, initial_clusters, ads_cluster_names).await;
        }
        Ok(())
    });
}

fn spawn_access_loggers(set: &mut JoinSet<Result<()>>, bootstrap: Bootstrap, conf: AccessLogConfig) {
    let listeners = bootstrap.static_resources.listeners;
    set.spawn(async move {
        let handles = start_access_loggers(
            conf.num_instances.get(),
            conf.queue_length.get(),
            conf.log_rotation.0.clone(),
            conf.max_log_files.get(),
        );

        info!("Access loggers started with {} instances", conf.num_instances);

        let listener_configurations =
            listeners.iter().map(|l| (l.name.clone(), l.get_access_log_configurations())).collect::<Vec<_>>();

        for (listener_name, access_log_configurations) in listener_configurations {
            _ = update_configuration(Target::Listener(listener_name.to_compact_string()), access_log_configurations)
                .await;
        }

        handles.join_all().await;
        Ok(())
    });
}

fn spawn_admin_service(
    set: &mut JoinSet<Result<()>>,
    bootstrap: Bootstrap,
    configuration_senders: Vec<ConfigurationSenders>,
    secret_manager: Arc<RwLock<SecretManager>>,
) {
    set.spawn(async move {
        _ = start_admin_server(bootstrap, configuration_senders, secret_manager).await;
        Ok(())
    });
}

async fn configure_initial_resources(
    bootstrap: Bootstrap,
    listeners: Vec<orion_lib::ListenerFactory>,
    clusters: Vec<PartialClusterType>,
    configuration_senders: Vec<ConfigurationSenders>,
) -> Result<Vec<ClusterType>> {
    let listeners_tx: Vec<_> = configuration_senders
        .into_iter()
        .map(|ConfigurationSenders { listener_configuration_sender, route_configuration_sender: _ }| {
            listener_configuration_sender
        })
        .collect();

    for (listener, listener_conf) in listeners.iter().zip(bootstrap.static_resources.listeners) {
        let _ = join_all(listeners_tx.iter().map(|listener_tx: &Sender<ListenerConfigurationChange>| {
            listener_tx.send(ListenerConfigurationChange::Added(Box::new((listener.clone(), listener_conf.clone()))))
        }))
        .await
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::<orion_error::Error>::into)?;
    }

    clusters.into_iter().map(orion_lib::clusters::add_cluster).collect::<Result<_>>()
}

async fn start_proxy(
    configuration_receivers: ConfigurationReceivers,
    ct: tokio_util::sync::CancellationToken,
) -> Result<()> {
    orion_lib::start_listener_manager(configuration_receivers, ct).await?;
    Ok(())
}
