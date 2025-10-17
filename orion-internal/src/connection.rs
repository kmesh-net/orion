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

use std::{
    collections::HashMap,
    ops::Deref,
    sync::{Arc, Weak},
};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, Mutex, RwLock},
    time::Instant,
};

pub use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct InternalConnectionMetadata {
    pub listener_name: String,
    pub buffer_size_kb: Option<u32>,
    pub created_at: Instant,
    pub endpoint_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InternalListenerHandle {
    pub name: String,
    pub connection_sender: mpsc::UnboundedSender<InternalConnectionPair>,
    listener_ref: Weak<()>,
    buffer_size_kb: Option<u32>,
}

impl InternalListenerHandle {
    pub fn new(
        name: String,
        connection_sender: mpsc::UnboundedSender<InternalConnectionPair>,
        listener_ref: Weak<()>,
        buffer_size_kb: Option<u32>,
    ) -> Self {
        Self { name, connection_sender, listener_ref, buffer_size_kb }
    }

    pub fn is_alive(&self) -> bool {
        self.listener_ref.strong_count() > 0
    }

    pub fn create_connection(&self, endpoint_id: Option<String>) -> Result<InternalConnectionPair> {
        if !self.is_alive() {
            return Err(Error::new(format!("Internal listener '{}' is no longer active", self.name)));
        }

        let metadata = InternalConnectionMetadata {
            listener_name: self.name.clone(),
            buffer_size_kb: self.buffer_size_kb,
            created_at: Instant::now(),
            endpoint_id,
        };

        let (upstream, downstream) = create_internal_connection_pair(metadata);

        let connection_pair = InternalConnectionPair { upstream: upstream.clone(), downstream: downstream.clone() };

        if self.connection_sender.send(connection_pair.clone()).is_err() {
            return Err(Error::new(format!("Failed to send connection to internal listener '{}'", self.name)));
        }

        Ok(connection_pair)
    }
}

#[derive(Debug, Clone)]
pub struct InternalConnectionPair {
    pub upstream: Arc<InternalStream>,
    pub downstream: Arc<InternalStream>,
}

#[derive(Debug)]
pub struct InternalStream {
    metadata: InternalConnectionMetadata,
    stream: Mutex<tokio::io::DuplexStream>,
    is_closed: Arc<RwLock<bool>>,
}

impl InternalStream {
    fn new(metadata: InternalConnectionMetadata, stream: tokio::io::DuplexStream) -> Self {
        Self { metadata, stream: Mutex::new(stream), is_closed: Arc::new(RwLock::new(false)) }
    }
}

impl InternalStream {
    pub fn metadata(&self) -> &InternalConnectionMetadata {
        &self.metadata
    }

    pub fn is_active(&self) -> bool {
        if let Ok(is_closed) = self.is_closed.try_read() {
            !*is_closed
        } else {
            false
        }
    }
}

impl AsyncRead for InternalStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.stream.try_lock() {
            Ok(mut stream) => tokio::io::AsyncRead::poll_read(std::pin::Pin::new(&mut *stream), cx, buf),
            Err(_) => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            },
        }
    }
}

impl AsyncWrite for InternalStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.stream.try_lock() {
            Ok(mut stream) => tokio::io::AsyncWrite::poll_write(std::pin::Pin::new(&mut *stream), cx, buf),
            Err(_) => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            },
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.stream.try_lock() {
            Ok(mut stream) => tokio::io::AsyncWrite::poll_flush(std::pin::Pin::new(&mut *stream), cx),
            Err(_) => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            },
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.stream.try_lock() {
            Ok(mut stream) => tokio::io::AsyncWrite::poll_shutdown(std::pin::Pin::new(&mut *stream), cx),
            Err(_) => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            },
        }
    }
}

#[derive(Debug)]
pub struct InternalConnectionFactory {
    listeners: Arc<RwLock<HashMap<String, InternalListenerHandle>>>,
}

impl InternalConnectionFactory {
    pub fn new() -> Self {
        Self { listeners: Arc::new(RwLock::new(HashMap::new())) }
    }

    pub async fn register_listener(
        &self,
        name: String,
        buffer_size_kb: Option<u32>,
    ) -> Result<(InternalListenerHandle, mpsc::UnboundedReceiver<InternalConnectionPair>, Arc<()>)> {
        let (connection_tx, connection_rx) = mpsc::unbounded_channel();
        let listener_ref = Arc::new(());
        let weak_ref = Arc::downgrade(&listener_ref);

        let handle = InternalListenerHandle::new(name.clone(), connection_tx, weak_ref, buffer_size_kb);

        let mut listeners = self.listeners.write().await;

        if listeners.contains_key(&name) {
            return Err(Error::new(format!("Internal listener '{name}' is already registered")));
        }

        listeners.insert(name, handle.clone());
        Ok((handle, connection_rx, listener_ref))
    }

    pub async fn unregister_listener(&self, name: &str) -> Result<()> {
        let mut listeners = self.listeners.write().await;

        if listeners.remove(name).is_none() {
            return Err(Error::new(format!("Internal listener '{name}' was not registered")));
        }

        Ok(())
    }

    pub async fn connect_to_listener(
        &self,
        name: &str,
        endpoint_id: Option<String>,
    ) -> Result<Box<InternalStreamWrapper>> {
        let listeners = self.listeners.read().await;
        let handle = listeners.get(name).ok_or_else(|| Error::new(format!("Internal listener '{name}' not found")))?;

        let connection_pair = handle.create_connection(endpoint_id)?;
        Ok(Box::new(InternalStreamWrapper { inner: connection_pair.upstream }))
    }

    pub async fn list_listeners(&self) -> Vec<String> {
        let listeners = self.listeners.read().await;
        listeners.keys().map(String::clone).collect()
    }

    pub async fn is_listener_active(&self, name: &str) -> bool {
        let listeners = self.listeners.read().await;
        listeners.get(name).is_some_and(InternalListenerHandle::is_alive)
    }

    pub async fn get_stats(&self) -> InternalConnectionStats {
        let listeners = self.listeners.read().await;
        let active_listeners = listeners.len();

        InternalConnectionStats { active_listeners, total_pooled_connections: 0, max_pooled_connections: 0 }
    }
}

impl Default for InternalConnectionFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct InternalConnectionStats {
    pub active_listeners: usize,
    pub total_pooled_connections: usize,
    pub max_pooled_connections: usize,
}

pub struct InternalStreamWrapper {
    inner: Arc<InternalStream>,
}

impl Deref for InternalStreamWrapper {
    type Target = InternalStream;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl AsyncRead for InternalStreamWrapper {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.inner.stream.try_lock() {
            Ok(mut stream) => {
                let pinned_stream = std::pin::Pin::new(&mut *stream);
                tokio::io::AsyncRead::poll_read(pinned_stream, cx, buf)
            },
            Err(_) => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            },
        }
    }
}

impl AsyncWrite for InternalStreamWrapper {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.inner.stream.try_lock() {
            Ok(mut stream) => {
                let pinned_stream = std::pin::Pin::new(&mut *stream);
                tokio::io::AsyncWrite::poll_write(pinned_stream, cx, buf)
            },
            Err(_) => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            },
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.inner.stream.try_lock() {
            Ok(mut stream) => {
                let pinned_stream = std::pin::Pin::new(&mut *stream);
                tokio::io::AsyncWrite::poll_flush(pinned_stream, cx)
            },
            Err(_) => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            },
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.inner.stream.try_lock() {
            Ok(mut stream) => {
                let pinned_stream = std::pin::Pin::new(&mut *stream);
                tokio::io::AsyncWrite::poll_shutdown(pinned_stream, cx)
            },
            Err(_) => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            },
        }
    }
}

const DEFAULT_BUFFER_SIZE: usize = 8192;

fn create_internal_connection_pair(metadata: InternalConnectionMetadata) -> (Arc<InternalStream>, Arc<InternalStream>) {
    let buffer_size = metadata.buffer_size_kb.map(|kb| (kb as usize) * 1024).unwrap_or(DEFAULT_BUFFER_SIZE);

    let (upstream_io, downstream_io) = tokio::io::duplex(buffer_size);

    let upstream = Arc::new(InternalStream::new(metadata.clone(), upstream_io));
    let downstream = Arc::new(InternalStream::new(metadata, downstream_io));

    (upstream, downstream)
}

static GLOBAL_FACTORY: std::sync::OnceLock<InternalConnectionFactory> = std::sync::OnceLock::new();

pub fn global_internal_connection_factory() -> &'static InternalConnectionFactory {
    GLOBAL_FACTORY.get_or_init(InternalConnectionFactory::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_factory_creation() {
        let factory = InternalConnectionFactory::new();
        let stats = factory.get_stats().await;
        assert_eq!(stats.active_listeners, 0);
        assert_eq!(stats.total_pooled_connections, 0);
    }

    #[tokio::test]
    async fn test_listener_registration() {
        let factory = InternalConnectionFactory::new();

        let result = factory.register_listener("test_listener".to_string(), None).await;
        assert!(result.is_ok());
        let (_handle, _rx, _listener_ref) = result.unwrap();

        let stats = factory.get_stats().await;
        assert_eq!(stats.active_listeners, 1);

        let result = factory.register_listener("test_listener".to_string(), None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_listener_unregistration() {
        let factory = InternalConnectionFactory::new();

        let (_handle, _rx, _listener_ref) = factory.register_listener("test_listener".to_string(), None).await.unwrap();
        let result = factory.unregister_listener("test_listener").await;
        assert!(result.is_ok());

        let stats = factory.get_stats().await;
        assert_eq!(stats.active_listeners, 0);

        let result = factory.unregister_listener("non_existent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connection_to_non_existent_listener() {
        let factory = InternalConnectionFactory::new();

        let result = factory.connect_to_listener("non_existent", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_listener_lifecycle() {
        let factory = InternalConnectionFactory::new();

        let (handle, _rx, _listener_ref) = factory.register_listener("test_listener".to_string(), None).await.unwrap();

        assert!(factory.is_listener_active("test_listener").await);
        assert!(handle.is_alive());

        factory.unregister_listener("test_listener").await.unwrap();

        assert!(!factory.is_listener_active("test_listener").await);
    }

    #[tokio::test]
    async fn test_list_listeners() {
        let factory = InternalConnectionFactory::new();

        let listeners = factory.list_listeners().await;
        assert!(listeners.is_empty());

        let (_handle1, _rx1, _listener_ref1) = factory.register_listener("listener1".to_string(), None).await.unwrap();
        let (_handle2, _rx2, _listener_ref2) = factory.register_listener("listener2".to_string(), None).await.unwrap();

        let listeners = factory.list_listeners().await;
        assert_eq!(listeners.len(), 2);
        assert!(listeners.contains(&String::from("listener1")));
        assert!(listeners.contains(&String::from("listener2")));
    }

    #[tokio::test]
    async fn test_global_factory() {
        let factory = global_internal_connection_factory();
        let stats = factory.get_stats().await;
        assert_eq!(stats.max_pooled_connections, 0);
    }

    #[tokio::test]
    async fn test_buffer_size_configuration() {
        let metadata_default = InternalConnectionMetadata {
            listener_name: "test".to_string(),
            buffer_size_kb: None,
            created_at: Instant::now(),
            endpoint_id: None,
        };
        let (upstream, downstream) = create_internal_connection_pair(metadata_default);
        assert!(upstream.is_active());
        assert!(downstream.is_active());

        let metadata_custom = InternalConnectionMetadata {
            listener_name: "test".to_string(),
            buffer_size_kb: Some(4),
            created_at: Instant::now(),
            endpoint_id: None,
        };
        let (upstream_custom, downstream_custom) = create_internal_connection_pair(metadata_custom);
        assert!(upstream_custom.is_active());
        assert!(downstream_custom.is_active());
    }
}
