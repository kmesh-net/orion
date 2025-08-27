#![allow(clippy::expect_used)]
use std::{future::IntoFuture, time::Duration};

use orion_xds::xds::{
    resources,
    server::{ServerAction, start_aggregate_server},
};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info, orion_xds=debug".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let (delta_resource_tx, delta_resources_rx) = tokio::sync::mpsc::channel(100);
    let (_stream_resource_tx, stream_resources_rx) = tokio::sync::mpsc::channel(100);
    let addr = "127.0.0.1:50051".parse()?;

    let grpc_server = tokio::spawn(async move {
        info!("Server started");
        let res = start_aggregate_server(addr, delta_resources_rx, stream_resources_rx).await;
        info!("Server stopped {res:?}");
    });
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    let _xds_resource_producer = tokio::spawn(async move {
        // needs to match ../orion-proxy/conf/orion-bootstap-minimal.yaml
        let cluster_id = "cluster_http".to_owned();
        let route_id = "rds_route".to_owned();

        let cla = resources::create_cluster_load_assignment(
            &cluster_id,
            "127.0.0.1:4001".parse().expect("We should panic here alright"),
            5,
        );
        info!("Adding Cluster Load Assignment for cluster {cluster_id}");
        let load_assigment_resource = resources::create_load_assignment_resource(&cluster_id, &cla);

        if delta_resource_tx.send(ServerAction::Add(load_assigment_resource.clone())).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;

        info!("Adding Route configuration  {route_id}");
        let route_configuration =
            resources::create_route_resource(&route_id, vec!["*".to_owned()], "/".to_owned(), cluster_id.clone());
        let route_configuration_resource =
            resources::create_route_configuration_resource(&route_id, &route_configuration);

        if delta_resource_tx.send(ServerAction::Add(route_configuration_resource.clone())).await.is_err() {
            return;
        }

        tokio::time::sleep(Duration::from_secs(15)).await;

        info!("Removing cluster load assignment {cluster_id}");
        if delta_resource_tx.send(ServerAction::Remove(load_assigment_resource)).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;

        info!("Removing route configuration {route_id}");
        if delta_resource_tx.send(ServerAction::Remove(route_configuration_resource)).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let _ = grpc_server.into_future().await;
    Ok(())
}
