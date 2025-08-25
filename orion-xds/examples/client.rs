use futures::future::select;
use orion_configuration::config::bootstrap::Node;
use orion_xds::{
    start_aggregate_client,
    xds::model::{XdsResourcePayload, XdsResourceUpdate},
};
use std::future::IntoFuture;
use tracing::{debug, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,orion_xds=debug".into()),
        )
        .init();

    let (mut worker, mut client, _subscription_manager) =
        start_aggregate_client(Node { id: "node1".into() }, "http://127.0.0.1:50051".parse()?).await?;
    let xds_worker = tokio::spawn(async move {
        let subscribe = worker.run().await;
        info!("Worker exited {subscribe:?}");
    });

    let xds_client = tokio::spawn(async move {
        while let Some(notification) = client.recv().await {
            debug!("Got notification {notification:?}");
            let _ = notification.ack_channel.send(vec![]);

            for update in notification.updates {
                match update {
                    XdsResourceUpdate::Update(_id, resource) => match resource {
                        XdsResourcePayload::Listener(_id, resource) => {
                            info!("Got update for listener {resource:#?}");
                        },
                        XdsResourcePayload::Cluster(_id, resource) => {
                            info!("Got update for cluster {resource:#?}");
                        },
                        _ => {},
                    },
                    XdsResourceUpdate::Remove(_id, _resource) => {},
                }
            }
        }
    });

    let _ = select(xds_client.into_future(), xds_worker.into_future()).await;
    Ok(())
}
