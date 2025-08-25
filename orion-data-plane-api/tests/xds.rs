use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use hyper_util::rt::tokio::TokioIo;
use orion_data_plane_api::envoy_data_plane_api::envoy::config::cluster::v3::Cluster;
use orion_data_plane_api::envoy_data_plane_api::envoy::service::cluster::v3::cluster_discovery_service_client::ClusterDiscoveryServiceClient;
use orion_data_plane_api::envoy_data_plane_api::envoy::service::cluster::v3::cluster_discovery_service_server::{
    ClusterDiscoveryService, ClusterDiscoveryServiceServer,
};
use orion_data_plane_api::envoy_data_plane_api::envoy::service::discovery::v3::aggregated_discovery_service_client::AggregatedDiscoveryServiceClient;
use orion_data_plane_api::envoy_data_plane_api::envoy::service::discovery::v3::aggregated_discovery_service_server::{
    AggregatedDiscoveryService, AggregatedDiscoveryServiceServer,
};
use orion_data_plane_api::envoy_data_plane_api::tonic;
use orion_data_plane_api::xds::client::DiscoveryClientBuilder;
use tonic::transport::Server;

use orion_data_plane_api::xds::bindings;
use orion_data_plane_api::xds::model::{TypeUrl, XdsResourceUpdate};

use futures::Stream;
use orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::Node;
use orion_data_plane_api::envoy_data_plane_api::envoy::service::discovery::v3::{
    DeltaDiscoveryResponse, DiscoveryResponse, Resource,
};
use orion_data_plane_api::envoy_data_plane_api::google::protobuf::Any;
use orion_data_plane_api::envoy_data_plane_api::prost::Message;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{self, sleep};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Uri;
use tonic::{Response, Status};
use tower::service_fn;
pub struct MockDiscoveryService {
    relay: Arc<Mutex<mpsc::Receiver<Result<DeltaDiscoveryResponse, tonic::Status>>>>,
}

#[tonic::async_trait]
impl AggregatedDiscoveryService for MockDiscoveryService {
    type StreamAggregatedResourcesStream = Pin<Box<dyn Stream<Item = Result<DiscoveryResponse, Status>> + Send>>;
    async fn stream_aggregated_resources(
        &self,
        _request: tonic::Request<
            tonic::Streaming<
                orion_data_plane_api::envoy_data_plane_api::envoy::service::discovery::v3::DiscoveryRequest,
            >,
        >,
    ) -> std::result::Result<tonic::Response<Self::StreamAggregatedResourcesStream>, tonic::Status> {
        unimplemented!("not used by proxy");
    }

    type DeltaAggregatedResourcesStream = Pin<Box<dyn Stream<Item = Result<DeltaDiscoveryResponse, Status>> + Send>>;
    async fn delta_aggregated_resources(
        &self,
        request: tonic::Request<
            tonic::Streaming<
                orion_data_plane_api::envoy_data_plane_api::envoy::service::discovery::v3::DeltaDiscoveryRequest,
            >,
        >,
    ) -> std::result::Result<tonic::Response<Self::DeltaAggregatedResourcesStream>, tonic::Status> {
        let mut in_stream = request.into_inner();
        let (tx, rx) = mpsc::channel::<Result<DeltaDiscoveryResponse, tonic::Status>>(100);
        let shared_receiver = self.relay.clone();
        tokio::spawn(async move {
            let mut receiver = shared_receiver.lock().await;
            'outer: while let Ok(result) = in_stream.message().await {
                match result {
                    Some(_) => {
                        while let Some(wrapped_response) = receiver.recv().await {
                            match tx.send(wrapped_response.clone()).await {
                                Ok(_) => {
                                    if wrapped_response.is_err() {
                                        break 'outer;
                                    }
                                },
                                _ => {
                                    break 'outer;
                                },
                            }
                        }
                    },
                    _ => {
                        break;
                    },
                }
            }
        });
        let output_stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(output_stream) as Self::DeltaAggregatedResourcesStream))
    }
}

#[tonic::async_trait]
impl ClusterDiscoveryService for MockDiscoveryService {
    type StreamClustersStream = Pin<Box<dyn Stream<Item = Result<DiscoveryResponse, Status>> + Send>>;
    async fn stream_clusters(
        &self,
        _request: tonic::Request<
            tonic::Streaming<
                orion_data_plane_api::envoy_data_plane_api::envoy::service::discovery::v3::DiscoveryRequest,
            >,
        >,
    ) -> std::result::Result<tonic::Response<Self::StreamClustersStream>, tonic::Status> {
        unimplemented!("not used by proxy");
    }

    type DeltaClustersStream = Pin<Box<dyn Stream<Item = Result<DeltaDiscoveryResponse, Status>> + Send>>;
    async fn delta_clusters(
        &self,
        request: tonic::Request<
            tonic::Streaming<
                orion_data_plane_api::envoy_data_plane_api::envoy::service::discovery::v3::DeltaDiscoveryRequest,
            >,
        >,
    ) -> std::result::Result<tonic::Response<Self::DeltaClustersStream>, tonic::Status> {
        let mut in_stream = request.into_inner();
        let (tx, rx) = mpsc::channel::<Result<DeltaDiscoveryResponse, tonic::Status>>(100);
        let shared_receiver = self.relay.clone();
        tokio::spawn(async move {
            let mut receiver = shared_receiver.lock().await;
            'outer: while let Ok(result) = in_stream.message().await {
                match result {
                    Some(_) => {
                        while let Some(wrapped_response) = receiver.recv().await {
                            match tx.send(wrapped_response.clone()).await {
                                Ok(_) => {
                                    if wrapped_response.is_err() {
                                        break 'outer;
                                    }
                                },
                                _ => {
                                    break 'outer;
                                },
                            }
                        }
                    },
                    _ => {
                        break;
                    },
                }
            }
        });
        let output_stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(output_stream) as Self::DeltaClustersStream))
    }

    async fn fetch_clusters(
        &self,
        _request: tonic::Request<
            orion_data_plane_api::envoy_data_plane_api::envoy::service::discovery::v3::DiscoveryRequest,
        >,
    ) -> std::result::Result<
        tonic::Response<orion_data_plane_api::envoy_data_plane_api::envoy::service::discovery::v3::DiscoveryResponse>,
        tonic::Status,
    > {
        unimplemented!("not used by proxy");
    }
}

#[tokio::test]
async fn test_client_operations() {
    let node = Node { id: "node-id".to_string(), cluster: "gw-cluster".to_string(), ..Default::default() };
    let cluster = Cluster { name: "cluster-a".to_string(), ..Default::default() };
    let cluster_resource = Resource {
        name: cluster.name.clone(),
        version: "0.1".to_string(),
        resource: Some(Any {
            type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
            value: cluster.encode_to_vec().into(),
        }),
        ..Default::default()
    };
    let resources = vec![cluster_resource];

    let initial_response: Result<DeltaDiscoveryResponse, tonic::Status> = Ok(DeltaDiscoveryResponse {
        resources,
        nonce: "abcd".to_string(),
        type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
        ..Default::default()
    });

    let (server_side_response_tx, server_side_response_rx) =
        mpsc::channel::<Result<DeltaDiscoveryResponse, tonic::Status>>(100);

    let (client, server) = tokio::io::duplex(1024);
    let cds_server = MockDiscoveryService { relay: Arc::new(Mutex::new(server_side_response_rx)) };
    tokio::spawn(async move {
        Server::builder()
            .add_service(ClusterDiscoveryServiceServer::new(cds_server))
            .serve_with_incoming(tokio_stream::once(Ok::<_, std::io::Error>(server)))
            .await
    });

    let mut client = Some(client);
    let channel = tonic::transport::Endpoint::try_from("http://[::]:50051")
        .expect("failed to init Endpoint")
        .connect_with_connector(service_fn(move |_: Uri| {
            let client = client.take();
            async move {
                if let Some(client) = client {
                    Ok(TokioIo::new(client))
                } else {
                    Err(std::io::Error::new(std::io::ErrorKind::Other, "client is already taken"))
                }
            }
        }))
        .await;

    let cds_client = ClusterDiscoveryServiceClient::new(channel.unwrap());
    let typed_binding = bindings::ClusterDiscoveryType { underlying_client: cds_client };

    let (mut worker, mut client) =
        DiscoveryClientBuilder::<bindings::ClusterDiscoveryType>::new(node, typed_binding).build().unwrap();

    tokio::spawn(async move {
        let _status = worker.run().await;
    });

    let _status = server_side_response_tx.send(initial_response).await;

    let _ = client.subscribe("".to_string(), TypeUrl::Cluster).await;

    tokio::select! {
        Some(captured_response) = client.recv() => {
            match captured_response.updates.first() {
                Some(XdsResourceUpdate::Update(name, _payload)) => {
                    assert_eq!(name, "cluster-a");
                    let ack_result = captured_response.ack_channel.send(vec![]);
                    assert!(ack_result.is_ok(), "failed to acknowledge response");
                }
                _ => panic!("failed to receive config update from xDS")
            }
        }
        _ = time::sleep(Duration::from_secs(5)) =>
            panic!("timed out waiting for xds resource over update channel")
    }
}

#[tokio::test]
async fn test_client_resilience() {
    let node = Node { id: "node-id".to_string(), cluster: "gw-cluster".to_string(), ..Default::default() };
    let cluster = Cluster { name: "cluster-a".to_string(), ..Default::default() };
    let cluster_resource = Resource {
        name: cluster.name.clone(),
        version: "0.1".to_string(),
        resource: Some(Any {
            type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
            value: cluster.encode_to_vec().into(),
        }),
        ..Default::default()
    };
    let resources = vec![cluster_resource];

    let initial_response: Result<DeltaDiscoveryResponse, tonic::Status> = Ok(DeltaDiscoveryResponse {
        resources,
        nonce: "abcd".to_string(),
        type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
        ..Default::default()
    });

    let (server_side_response_tx, server_side_response_rx) =
        mpsc::channel::<Result<DeltaDiscoveryResponse, tonic::Status>>(100);

    let (client, server) = tokio::io::duplex(1024);
    let cds_server = MockDiscoveryService { relay: Arc::new(Mutex::new(server_side_response_rx)) };

    tokio::spawn(async move {
        Server::builder()
            .add_service(ClusterDiscoveryServiceServer::new(cds_server))
            .serve_with_incoming(tokio_stream::once(Ok::<_, std::io::Error>(server)))
            .await
    });

    let mut client = Some(client);
    let channel = tonic::transport::Endpoint::try_from("http://[::]:50051")
        .expect("failed to init Endpoint")
        .connect_with_connector_lazy(service_fn(move |_: Uri| {
            let client = client.take();
            async move {
                if let Some(client) = client {
                    Ok(TokioIo::new(client))
                } else {
                    Err(std::io::Error::new(std::io::ErrorKind::Other, "client is already taken"))
                }
            }
        }));

    let cds_client = ClusterDiscoveryServiceClient::new(channel);
    let typed_binding = bindings::ClusterDiscoveryType { underlying_client: cds_client };

    let (mut worker, mut client) = DiscoveryClientBuilder::<bindings::ClusterDiscoveryType>::new(node, typed_binding)
        .subscribe_resource_name("cluster-a".to_string())
        .subscribe_resource_name("cluster-b".to_string())
        .build()
        .unwrap();

    tokio::spawn(async move {
        let _status = worker.run().await;
    });
    let captured_count = AtomicUsize::new(0);

    let _status = server_side_response_tx.send(initial_response.clone()).await;

    tokio::select! {
        Some(captured_response) = client.recv() => {
            match captured_response.updates.first() {
                Some(XdsResourceUpdate::Update(name, _payload)) => {
                    assert_eq!(name, "cluster-a");
                    let _cnt = captured_count.fetch_add(1, Ordering::Relaxed);
                    assert_eq!(
                        captured_count.load(Ordering::Relaxed),
                        1,
                        "cluster-a should be captured just once after some time"
                    );
                }
                _ => panic!("failed to receive config update from xDS")
            }
        }
        _ = time::sleep(Duration::from_secs(3)) =>
                panic!("timed out waiting for xds resource over update channel")
    }

    let abort_response: Result<DeltaDiscoveryResponse, tonic::Status> =
        Err(tonic::Status::aborted("kill the stream for testing purposes"));
    let _status = server_side_response_tx.send(abort_response).await;
    sleep(Duration::from_millis(300)).await;

    let _status = server_side_response_tx.send(initial_response.clone()).await;
    sleep(Duration::from_millis(300)).await;

    tokio::select! {
        Some(captured_response) = client.recv() => {
            match captured_response.updates.first() {
                Some(XdsResourceUpdate::Update(name, _payload)) => {
                    assert_eq!(name, "cluster-a");
                    let _cnt = captured_count.fetch_add(1, Ordering::Relaxed);
                    assert_eq!(
                        captured_count.load(Ordering::Relaxed),
                        2,
                        "cluster-a should be captured again after reconnect"
                    );
                }
                _ => panic!("failed to receive config update from xDS")
            }
        }
        _ = time::sleep(Duration::from_secs(3)) =>
                panic!("timed out waiting for xds resource over update channel")
    }
}

#[tokio::test]
async fn test_aggregated_discovery() {
    let node = Node { id: "node-id".to_string(), cluster: "gw-cluster".to_string(), ..Default::default() };
    let cluster = Cluster { name: "cluster-a".to_string(), ..Default::default() };
    let cluster_resource = Resource {
        name: cluster.name.clone(),
        version: "0.1".to_string(),
        resource: Some(Any {
            type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
            value: cluster.encode_to_vec().into(),
        }),
        ..Default::default()
    };
    let resources = vec![cluster_resource];

    let initial_response: Result<DeltaDiscoveryResponse, tonic::Status> = Ok(DeltaDiscoveryResponse {
        resources,
        nonce: "abcd".to_string(),
        type_url: "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_string(),
        ..Default::default()
    });

    let (server_side_response_tx, server_side_response_rx) =
        mpsc::channel::<Result<DeltaDiscoveryResponse, tonic::Status>>(100);

    let (client, server) = tokio::io::duplex(1024);
    let ads_server = MockDiscoveryService { relay: Arc::new(Mutex::new(server_side_response_rx)) };
    tokio::spawn(async move {
        Server::builder()
            .add_service(AggregatedDiscoveryServiceServer::new(ads_server))
            .serve_with_incoming(tokio_stream::once(Ok::<_, std::io::Error>(server)))
            .await
    });

    let mut client = Some(client);
    let channel = tonic::transport::Endpoint::try_from("http://[::]:50051")
        .expect("failed to init Endpoint")
        .connect_with_connector(service_fn(move |_: Uri| {
            let client = client.take();
            async move {
                if let Some(client) = client {
                    Ok(TokioIo::new(client))
                } else {
                    Err(std::io::Error::new(std::io::ErrorKind::Other, "client is already taken"))
                }
            }
        }))
        .await
        .unwrap();

    let ads_client = AggregatedDiscoveryServiceClient::new(channel.clone());
    let typed_binding = bindings::AggregatedDiscoveryType { underlying_client: ads_client };

    let client = DiscoveryClientBuilder::<bindings::AggregatedDiscoveryType>::new(node.clone(), typed_binding)
        .subscribe_resource_name("my-cluster".to_string())
        .build();
    assert!(client.is_err(), "cannot subscribe to resources without a type_url for ADS");

    let ads_client = AggregatedDiscoveryServiceClient::new(channel);
    let typed_binding = bindings::AggregatedDiscoveryType { underlying_client: ads_client };

    let (mut worker, mut client) =
        DiscoveryClientBuilder::<bindings::AggregatedDiscoveryType>::new(node, typed_binding)
            .subscribe_resource_name_by_typeurl("cluster-a".to_string(), TypeUrl::Cluster)
            .subscribe_resource_name_by_typeurl("cluster-z".to_string(), TypeUrl::Cluster)
            .subscribe_resource_name_by_typeurl("endpoints-a".to_string(), TypeUrl::ClusterLoadAssignment)
            .subscribe_resource_name_by_typeurl("secret-config-a".to_string(), TypeUrl::Secret)
            .build()
            .unwrap();

    tokio::spawn(async move {
        let _status = worker.run().await;
    });

    let _status = server_side_response_tx.send(initial_response).await;

    let _ = client.subscribe("".to_string(), TypeUrl::Cluster).await;

    tokio::select! {
        Some(captured_response) = client.recv() => {
            match captured_response.updates.first() {
                Some(XdsResourceUpdate::Update(name, _payload)) => {
                    assert_eq!(name, "cluster-a");
                }
                _ => panic!("failed to receive config update from xDS")
            }
        }
        _ = time::sleep(Duration::from_secs(5)) =>
            panic!("timed out waiting for xds resource over update channel")
    }
}
