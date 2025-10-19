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

use std::{sync::Arc, time::Duration};

use http::uri::Authority;
use orion_configuration::config::cluster::{health_check::ClusterHealthCheck, HealthStatus};
use pingora_timeout::fast_timeout::fast_timeout;
use tokio::{
    sync::{
        mpsc::{self, Sender, UnboundedReceiver, UnboundedSender},
        Notify,
    },
    task::JoinHandle,
};

use super::{checker::WaitInterval, CurrentHealthStatus};
use crate::{clusters::health::EndpointId, EndpointHealthUpdate, Error};

macro_rules! deref {
    ($subclass:ty => $field:ident as $base:ty) => {
        impl ::std::ops::Deref for $subclass {
            type Target = $base;

            fn deref(&self) -> &Self::Target {
                &self.$field
            }
        }

        impl ::std::ops::DerefMut for $subclass {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.$field
            }
        }
    };
}
pub(crate) use deref;

#[derive(Clone)]
pub struct MockIntervalWaiter(Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<()>>>);

impl MockIntervalWaiter {
    pub fn new() -> (Self, mpsc::UnboundedSender<()>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (MockIntervalWaiter(Arc::new(tokio::sync::Mutex::new(receiver))), sender)
    }
}

impl WaitInterval for MockIntervalWaiter {
    async fn wait_interval_was_cancelled(
        &self,
        _config: &ClusterHealthCheck,
        _health_status: CurrentHealthStatus,
        stop_signal: &Notify,
    ) -> bool {
        let intervals = Arc::clone(&self.0);
        let mut interval_receiver = intervals.lock().await;
        tokio::select! {
            () = stop_signal.notified() => true,
            _ = interval_receiver.recv() => false,
        }
    }
}

pub enum CheckerTask<MockChannel> {
    Ready((MockChannel, MockIntervalWaiter, mpsc::Sender<EndpointHealthUpdate>)),
    Started(JoinHandle<Result<(), Error>>),
    Finished,
}

pub struct TestFixture<ProtocolConfig, MockChannel, Req, Resp> {
    pub endpoint: EndpointId,
    pub cluster_config: ClusterHealthCheck,
    pub protocol_config: ProtocolConfig,
    pub stop_signal: Arc<Notify>,
    pub health_event_receiver: mpsc::Receiver<EndpointHealthUpdate>,
    pub connections_receiver: mpsc::UnboundedReceiver<bool>,
    pub request_receiver: mpsc::UnboundedReceiver<Req>,
    pub response_sender: mpsc::UnboundedSender<Resp>,
    pub interval_sender: mpsc::UnboundedSender<()>,
    pub checker_task: CheckerTask<MockChannel>,
}

#[allow(clippy::panic)]
impl<ProtocolConfig, MockChannel, Req, Resp> TestFixture<ProtocolConfig, MockChannel, Req, Resp>
where
    ProtocolConfig: Clone,
{
    pub fn new<F>(
        protocol_config: ProtocolConfig,
        healthy_threshold: u16,
        unhealthy_threshold: u16,
        channel_builder: F,
    ) -> Self
    where
        F: FnOnce(UnboundedSender<bool>, UnboundedSender<Req>, UnboundedReceiver<Resp>) -> MockChannel,
    {
        // Prepare all the configuration
        let endpoint = EndpointId { cluster: "test_cluster".into(), endpoint: Authority::from_static("10.0.0.1:8080") };
        let cluster_config = ClusterHealthCheck::new(
            Duration::from_secs(1),
            Duration::from_secs(0),
            unhealthy_threshold,
            healthy_threshold,
        );

        // Prepare the channels
        let (health_event_sender, health_event_receiver) = mpsc::channel::<EndpointHealthUpdate>(1000);
        let stop_signal = Arc::new(Notify::new());

        // Prepare mocks
        let (connections_sender, connections_receiver) = mpsc::unbounded_channel::<bool>();
        let (request_sender, request_receiver) = mpsc::unbounded_channel::<Req>();
        let (response_sender, response_receiver) = mpsc::unbounded_channel::<Resp>();
        let mock_grpc_stack = channel_builder(connections_sender, request_sender, response_receiver);

        let (mock_interval_waiter, interval_sender) = MockIntervalWaiter::new();

        TestFixture {
            endpoint,
            cluster_config,
            protocol_config,
            stop_signal,
            health_event_receiver,
            connections_receiver,
            request_receiver,
            response_sender,
            interval_sender,
            checker_task: CheckerTask::Ready((mock_grpc_stack, mock_interval_waiter, health_event_sender)),
        }
    }

    pub fn start<F>(&mut self, spawner: F)
    where
        F: FnOnce(
            EndpointId,
            ClusterHealthCheck,
            ProtocolConfig,
            Sender<EndpointHealthUpdate>,
            Arc<Notify>,
            (MockChannel, MockIntervalWaiter),
        ) -> JoinHandle<Result<(), Error>>,
    {
        // Hack to extract the values from the enum :(
        let CheckerTask::Ready((stack_builder, interval_waiter, sender)) =
            std::mem::replace(&mut self.checker_task, CheckerTask::Finished)
        else {
            panic!("Checker task not ready to start");
        };
        self.checker_task = CheckerTask::Started(spawner(
            self.endpoint.clone(),
            self.cluster_config.clone(),
            self.protocol_config.clone(),
            sender.clone(),
            Arc::clone(&self.stop_signal),
            (stack_builder, interval_waiter),
        ));
    }

    pub async fn stop(&mut self) {
        let CheckerTask::Started(task_handle) = &mut self.checker_task else {
            panic!("Checker task not running");
        };
        self.stop_signal.notify_waiters();
        task_handle.await.unwrap().unwrap();
        self.checker_task = CheckerTask::Finished;
    }

    pub fn enqueue_response(&self, response: Resp) {
        self.response_sender.send(response).unwrap();
    }

    pub async fn connection_expected(&mut self, timeout_value: Duration) -> bool {
        let Ok(Some(req)) = fast_timeout(timeout_value, self.connections_receiver.recv()).await else {
            panic!("Connection not received");
        };
        req
    }

    pub async fn request_expected(&mut self, timeout_value: Duration) -> Req {
        let Ok(Some(req)) = fast_timeout(timeout_value, self.request_receiver.recv()).await else {
            panic!("Health check not received");
        };
        req
    }

    pub async fn health_update_expected(
        &mut self,
        expected_status: HealthStatus,
        timeout_value: Duration,
    ) -> EndpointHealthUpdate {
        loop {
            let Ok(Some(health_status)) = fast_timeout(timeout_value, self.health_event_receiver.recv()).await else {
                panic!("Health check not received");
            };
            if !health_status.changed {
                // ignore repeated status updates
                continue;
            }
            assert_eq!(health_status.endpoint, self.endpoint, "Unexpected endpoint in health update");
            assert_eq!(health_status.health, expected_status, "Unexpected status in health update");
            return health_status;
        }
    }

    /// Advances the clock in the test, just one interval.
    pub fn tick(&self) {
        self.interval_sender.send(()).unwrap();
    }
}
