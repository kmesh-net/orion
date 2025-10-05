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

use crate::{connection::global_internal_connection_factory, Error, InternalStreamWrapper, Result};

#[derive(Debug, Clone)]
pub struct InternalClusterConnector {
    listener_name: String,
    endpoint_id: Option<String>,
}

impl InternalClusterConnector {
    pub fn new(listener_name: String, endpoint_id: Option<String>) -> Self {
        Self { listener_name, endpoint_id }
    }

    pub fn listener_name(&self) -> &str {
        &self.listener_name
    }

    pub fn endpoint_id(&self) -> Option<&str> {
        self.endpoint_id.as_deref()
    }

    pub async fn connect(&self) -> Result<Box<InternalStreamWrapper>> {
        let factory = global_internal_connection_factory();

        if !factory.is_listener_active(&self.listener_name).await {
            return Err(Error::new(format!(
                "Internal listener '{}' is not active or not registered",
                self.listener_name
            )));
        }

        factory.connect_to_listener(&self.listener_name, self.endpoint_id.clone()).await
    }

    pub async fn is_available(&self) -> bool {
        let factory = global_internal_connection_factory();
        factory.is_listener_active(&self.listener_name).await
    }
}

#[derive(Debug, Clone)]
pub struct InternalChannelConnector {
    connector: InternalClusterConnector,
    cluster_name: &'static str,
}

impl InternalChannelConnector {
    pub fn new(listener_name: String, cluster_name: &'static str, endpoint_id: Option<String>) -> Self {
        let connector = InternalClusterConnector::new(listener_name, endpoint_id);

        Self { connector, cluster_name }
    }

    pub fn cluster_name(&self) -> &'static str {
        self.cluster_name
    }

    pub fn listener_name(&self) -> &str {
        self.connector.listener_name()
    }

    pub async fn connect(&self) -> Result<InternalChannel> {
        let stream = self.connector.connect().await?;

        Ok(InternalChannel {
            stream,
            cluster_name: self.cluster_name,
            listener_name: self.connector.listener_name().to_owned(),
            endpoint_id: self.connector.endpoint_id().map(std::borrow::ToOwned::to_owned),
        })
    }

    pub async fn is_available(&self) -> bool {
        self.connector.is_available().await
    }
}

pub struct InternalChannel {
    pub stream: Box<InternalStreamWrapper>,
    pub cluster_name: &'static str,
    pub listener_name: String,
    pub endpoint_id: Option<String>,
}

impl InternalChannel {
    pub fn cluster_name(&self) -> &'static str {
        self.cluster_name
    }

    pub fn listener_name(&self) -> &str {
        &self.listener_name
    }

    pub fn endpoint_id(&self) -> Option<&str> {
        self.endpoint_id.as_deref()
    }
}

pub mod cluster_helpers {
    use super::{global_internal_connection_factory, InternalChannelConnector};
    use crate::InternalConnectionStats;
    use orion_configuration::config::cluster::InternalEndpointAddress;

    pub fn create_internal_connector(
        internal_addr: &InternalEndpointAddress,
        cluster_name: &'static str,
    ) -> InternalChannelConnector {
        InternalChannelConnector::new(
            internal_addr.server_listener_name.to_string(),
            cluster_name,
            internal_addr.endpoint_id.as_ref().map(std::string::ToString::to_string),
        )
    }

    pub async fn is_internal_listener_available(listener_name: &str) -> bool {
        let factory = global_internal_connection_factory();
        factory.is_listener_active(listener_name).await
    }

    pub async fn get_internal_connection_stats() -> InternalConnectionStats {
        let factory = global_internal_connection_factory();
        factory.get_stats().await
    }

    pub async fn list_internal_listeners() -> Vec<String> {
        let factory = global_internal_connection_factory();
        factory.list_listeners().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_internal_connector_creation() {
        let connector = InternalClusterConnector::new(String::from("test_listener"), Some(String::from("endpoint1")));
        assert_eq!(connector.listener_name(), "test_listener");
        assert_eq!(connector.endpoint_id(), Some("endpoint1"));
    }

    #[tokio::test]
    async fn test_connection_to_non_existent_listener() {
        let connector = InternalClusterConnector::new(String::from("non_existent_listener"), None);
        let result = connector.connect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_availability_check() {
        let connector = InternalClusterConnector::new(String::from("non_existent_listener"), None);
        assert!(!connector.is_available().await);
    }

    #[tokio::test]
    async fn test_internal_channel_connector() {
        let channel_connector = InternalChannelConnector::new(
            String::from("test_listener"),
            "test_cluster",
            Some(String::from("endpoint1")),
        );

        assert_eq!(channel_connector.cluster_name(), "test_cluster");
        assert_eq!(channel_connector.listener_name(), "test_listener");
        assert!(!channel_connector.is_available().await);
    }

    #[tokio::test]
    async fn test_cluster_helpers() {
        use cluster_helpers::*;
        use orion_configuration::config::cluster::InternalEndpointAddress;

        let internal_addr = InternalEndpointAddress {
            server_listener_name: String::from("test_listener").into(),
            endpoint_id: Some(String::from("endpoint1").into()),
        };

        let connector = create_internal_connector(&internal_addr, "test_cluster");
        assert_eq!(connector.cluster_name(), "test_cluster");
        assert_eq!(connector.listener_name(), "test_listener");

        assert!(!is_internal_listener_available("non_existent").await);

        let stats = get_internal_connection_stats().await;
        assert_eq!(stats.active_listeners, 0);

        let listeners = list_internal_listeners().await;
        assert!(listeners.is_empty());
    }
}
