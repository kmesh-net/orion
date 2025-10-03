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

use orion_lib::{global_internal_connection_factory, InternalChannelConnector};

#[tokio::test]
async fn test_complete_internal_connection_flow() {
    let factory = global_internal_connection_factory();

    let listener_name = "test_integration_listener_global";
    let (_handle, mut connection_receiver, _listener_ref) =
        factory.register_listener(listener_name.to_string()).await.expect("Failed to register listener");

    assert!(factory.is_listener_active(listener_name).await);
    let listeners = factory.list_listeners().await;
    assert!(listeners.contains(&listener_name.to_string()));

    let cluster_connector =
        InternalChannelConnector::new(listener_name.to_string(), "test_cluster", Some("endpoint1".to_string()));

    assert!(cluster_connector.is_available().await);

    let connection_future = cluster_connector.connect();
    let listener_future = async { connection_receiver.recv().await };

    let (cluster_connection, listener_connection) = tokio::join!(connection_future, listener_future);

    assert!(cluster_connection.is_ok(), "Cluster connection failed: {:?}", cluster_connection.err());
    assert!(listener_connection.is_some(), "Listener didn't receive connection");

    let cluster_channel = cluster_connection.unwrap();
    let listener_pair = listener_connection.unwrap();

    assert_eq!(cluster_channel.cluster_name(), "test_cluster");
    assert_eq!(cluster_channel.listener_name(), listener_name);
    assert_eq!(cluster_channel.endpoint_id(), Some("endpoint1"));

    let metadata = listener_pair.downstream.metadata();
    assert_eq!(metadata.listener_name, listener_name);
    assert_eq!(metadata.buffer_size_kb, None);
    assert_eq!(metadata.endpoint_id, Some("endpoint1".to_string()));

    assert!(listener_pair.upstream.is_active());
    assert!(listener_pair.downstream.is_active());

    factory.unregister_listener(listener_name).await.expect("Failed to unregister listener");
    assert!(!factory.is_listener_active(listener_name).await);
}

#[tokio::test]
async fn test_connection_pooling() {
    let factory = global_internal_connection_factory();

    let listener_name = "test_pooling_listener_global";
    let (_handle, mut connection_receiver, _listener_ref) =
        factory.register_listener(listener_name.to_string()).await.expect("Failed to register listener");

    let connector = InternalChannelConnector::new(listener_name.to_string(), "test_cluster", None);

    let connection_future = connector.connect();
    let listener_future = connection_receiver.recv();
    let (_cluster_conn, listener_pair) = tokio::join!(connection_future, listener_future);

    assert!(listener_pair.is_some());

    let stats = factory.get_stats().await;
    assert!(stats.active_listeners >= 1);

    factory.unregister_listener(listener_name).await.expect("Failed to unregister listener");
}

#[tokio::test]
async fn test_error_scenarios() {
    let factory = global_internal_connection_factory();

    let connector = InternalChannelConnector::new("non_existent_listener".to_string(), "test_cluster", None);

    assert!(!connector.is_available().await);
    let result = connector.connect().await;
    assert!(result.is_err());

    let listener_name = "test_error_listener_global";
    let result1 = factory.register_listener(listener_name.to_string()).await;
    assert!(result1.is_ok());
    let (_handle1, _rx1, _listener_ref1) = result1.unwrap();

    let result2 = factory.register_listener(listener_name.to_string()).await;
    assert!(result2.is_err());

    let result = factory.unregister_listener("non_existent").await;
    assert!(result.is_err());

    factory.unregister_listener(listener_name).await.expect("Failed to unregister listener");
}

#[tokio::test]
async fn test_connection_to_unregistered_listener() {
    let _factory = global_internal_connection_factory();

    let connector = InternalChannelConnector::new("non_existent_timeout_listener".to_string(), "test_cluster", None);

    let result = connector.connect().await;
    assert!(result.is_err());

    assert!(!connector.is_available().await);
}

#[tokio::test]
async fn test_global_factory() {
    let factory = global_internal_connection_factory();

    let listener_name = "test_global_listener";
    let (_handle, _rx, _listener_ref) = factory.register_listener(listener_name.to_string()).await.unwrap();

    assert!(factory.is_listener_active(listener_name).await);

    let stats = factory.get_stats().await;
    assert!(stats.active_listeners > 0);

    factory.unregister_listener(listener_name).await.expect("Failed to unregister listener");
}

#[tokio::test]
async fn test_statistics_and_monitoring() {
    let factory = global_internal_connection_factory();

    let listener1 = "test_stats_listener1_global";

    let (_handle1, _rx1, _listener_ref1) =
        factory.register_listener(listener1.to_string()).await.expect("Failed to register listener1");

    let stats = factory.get_stats().await;
    assert!(stats.active_listeners >= 1);

    let listeners = factory.list_listeners().await;
    assert!(listeners.contains(&listener1.to_string()));

    factory.unregister_listener(listener1).await.expect("Failed to unregister listener1");
}

#[tokio::test]
async fn test_cluster_helpers() {
    use orion_configuration::config::cluster::InternalEndpointAddress;
    use orion_lib::cluster_helpers::*;

    let internal_addr = InternalEndpointAddress {
        server_listener_name: "test_listener".to_string().into(),
        endpoint_id: Some("endpoint1".to_string().into()),
    };

    let connector = create_internal_connector(&internal_addr, "test_cluster");
    assert_eq!(connector.cluster_name(), "test_cluster");
    assert_eq!(connector.listener_name(), "test_listener");

    assert!(!is_internal_listener_available("non_existent").await);

    let stats = get_internal_connection_stats().await;
    assert_eq!(stats.max_pooled_connections, 0);

    let listeners = list_internal_listeners().await;
    assert!(listeners.is_empty());
}
