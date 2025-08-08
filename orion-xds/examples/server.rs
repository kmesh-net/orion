use orion_data_plane_api::envoy_data_plane_api::envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager::CodecType;
use orion_xds::xds::{resources, server::{start_aggregate_server, ServerAction}};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use std::{future::IntoFuture, time::Duration};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,orion_xds=debug".into()),
        )
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
        loop {
            let id = uuid::Uuid::new_v4().to_string();
            let listener_id = format!("Listener-{id}");
            let cluster_id = format!("Cluster-{id}");

            let cluster = resources::create_cluster_with_endpoints(
                &cluster_id,
                "192.168.1.10:4000".parse().expect("we really should panic here if this is wrong"),
                2,
                true,
            );
            info!("Adding cluster {cluster_id}");
            let cluster_resource = resources::create_cluster_resource(&cluster);

            if delta_resource_tx.send(ServerAction::Add(cluster_resource.clone())).await.is_err() {
                break;
            };
            tokio::time::sleep(Duration::from_secs(5)).await;
            let listener = resources::create_listener(
                &listener_id,
                "192.168.1.10:8000".parse().expect("we really should panic here if this is wrong"),
                CodecType::Http1,
                vec!["*".to_owned(), "example.com".to_owned()],
                vec![(cluster_id.clone(), 1)],
            );
            let listener_resource = resources::create_listener_resource(&listener);
            info!("Adding listener {listener_resource:?}");
            if delta_resource_tx.send(ServerAction::Add(listener_resource)).await.is_err() {
                break;
            };
            tokio::time::sleep(Duration::from_secs(15)).await;

            info!("Removing cluster {cluster_id}");
            if delta_resource_tx.send(ServerAction::Remove(cluster_resource)).await.is_err() {
                break;
            };
            tokio::time::sleep(Duration::from_secs(5)).await;
            let listener = resources::create_listener(
                &listener_id,
                "192.168.1.10:8000".parse().expect("we really should panic here if this is wrong"),
                CodecType::Http1,
                vec!["*".to_owned(), "example.com".to_owned()],
                vec![(cluster_id, 1)],
            );
            let listener_resource = resources::create_listener_resource(&listener);
            info!("Removing listener {listener_resource:?}");
            if delta_resource_tx.send(ServerAction::Remove(listener_resource)).await.is_err() {
                break;
            };
        }
    });

    let _ = grpc_server.into_future().await;
    Ok(())
}
