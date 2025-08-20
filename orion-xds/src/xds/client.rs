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

use super::{
    bindings,
    model::{RejectedConfig, ResourceId, ResourceVersion, TypeUrl, XdsError, XdsResourcePayload, XdsResourceUpdate},
};
use orion_configuration::config::bootstrap::Node;
use orion_data_plane_api::envoy_data_plane_api::{
    envoy::{
        config::core::v3::Node as EnvoyNode,
        service::discovery::v3::{DeltaDiscoveryRequest, DeltaDiscoveryResponse},
    },
    google::rpc::Status,
    tonic,
};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time,
};
use tracing::{debug, info, warn};

pub const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
pub const MAX_BACKOFF: Duration = Duration::from_secs(20);
pub const BACKOFF_INTERVAL: Duration = Duration::from_secs(2);
pub const RETRY_INTERVAL: Duration = Duration::from_secs(5);
pub const ACK_TIMEOUT: Duration = Duration::from_secs(5);

pub struct DiscoveryClientBuilder<C: bindings::TypedXdsBinding> {
    node: Node,
    client_binding: C,
    initial_subscriptions: HashMap<TypeUrl, HashSet<ResourceId>>,
    error: Option<String>,
}

impl<C> DiscoveryClientBuilder<C>
where
    C: bindings::TypedXdsBinding,
{
    pub fn new(node: Node, client: C) -> DiscoveryClientBuilder<C> {
        DiscoveryClientBuilder { node, client_binding: client, initial_subscriptions: HashMap::new(), error: None }
    }

    #[must_use]
    pub fn subscribe_resource_name(mut self, resource_id: ResourceId) -> Self {
        if let Some(type_url) = C::type_url() {
            self = self.subscribe_resource_name_by_typeurl(resource_id, type_url);
        } else {
            self.error = Some("subscribe only works if typed binding provides a compatible type_url".to_owned());
        }
        self
    }

    fn subscribe_resource_name_by_typeurl(mut self, resource_id: ResourceId, type_url: TypeUrl) -> Self {
        let configured_type_url = C::type_url();
        if configured_type_url.is_none() || configured_type_url.is_some_and(|type_is_set| type_is_set == type_url) {
            self.initial_subscriptions.entry(type_url).or_default().insert(resource_id);
        } else {
            self.error = Some("can only subscribe by type_url when using a compatible typed binding".to_owned());
        }
        self
    }

    pub fn build(
        self,
    ) -> Result<(DeltaClientBackgroundWorker<C>, DeltaDiscoveryClient, DeltaDiscoverySubscriptionManager), Box<XdsError>>
    {
        if let Some(err) = self.error {
            Err(Box::new(XdsError::BuilderFailed(err)))
        } else {
            let (subscription_updates_tx, subscription_updates_rx) = mpsc::channel::<SubscriptionEvent>(100);
            let (resource_updates_tx, resource_updates_rx) = mpsc::channel::<XdsUpdateEvent>(100);
            Ok((
                DeltaClientBackgroundWorker {
                    node: self.node,
                    client_binding: self.client_binding,
                    initial_subscriptions: self.initial_subscriptions,
                    subscriptions_rx: subscription_updates_rx,
                    resources_tx: resource_updates_tx,
                },
                DeltaDiscoveryClient { resources_rx: resource_updates_rx },
                DeltaDiscoverySubscriptionManager { subscriptions_tx: subscription_updates_tx },
            ))
        }
    }
}

/// Incremental Client that operates the delta version of the xDS protocol
/// use to consume xDS configuration updates asychronously, modify resource subscriptions

#[derive(Debug)]
pub struct DeltaDiscoveryClient {
    resources_rx: mpsc::Receiver<XdsUpdateEvent>,
}

#[derive(Debug)]
pub struct DeltaDiscoverySubscriptionManager {
    subscriptions_tx: mpsc::Sender<SubscriptionEvent>,
}

impl DeltaDiscoveryClient {
    pub async fn recv(&mut self) -> Option<XdsUpdateEvent> {
        self.resources_rx.recv().await
    }
}

impl DeltaDiscoverySubscriptionManager {
    pub async fn subscribe(&self, resource_id: ResourceId, type_url: TypeUrl) -> Result<(), XdsError> {
        Ok(self.subscriptions_tx.send(SubscriptionEvent::Subscribe(type_url, resource_id)).await?)
    }

    pub async fn unsubscribe(&self, resource_id: ResourceId, type_url: TypeUrl) -> Result<(), XdsError> {
        Ok(self.subscriptions_tx.send(SubscriptionEvent::Unsubscribe(type_url, resource_id)).await?)
    }
}

#[derive(Debug)]
pub struct XdsUpdateEvent {
    pub updates: Vec<XdsResourceUpdate>,
    pub ack_channel: oneshot::Sender<Vec<RejectedConfig>>,
}

#[derive(Clone, Debug)]
pub enum SubscriptionEvent {
    Subscribe(TypeUrl, ResourceId),
    Unsubscribe(TypeUrl, ResourceId),
}

/// Background worker that handles interactions with remote xDS services
#[derive(Debug)]
pub struct DeltaClientBackgroundWorker<C: bindings::TypedXdsBinding> {
    node: Node,
    client_binding: C,
    initial_subscriptions: HashMap<TypeUrl, HashSet<ResourceId>>,
    subscriptions_rx: mpsc::Receiver<SubscriptionEvent>,
    resources_tx: mpsc::Sender<XdsUpdateEvent>,
}

impl<C: bindings::TypedXdsBinding> DeltaClientBackgroundWorker<C> {
    pub async fn run(&mut self) -> Result<(), XdsError> {
        let mut connection_id = 0;

        let mut state = DiscoveryClientState {
            backoff: INITIAL_BACKOFF,
            tracked: HashMap::new(),
            subscriptions: self.initial_subscriptions.clone(),
        };
        loop {
            connection_id += 1;
            debug!(connection_id, "starting xDS (re)connect cycle {:?}", state.backoff);
            self.persistently_connect(&mut state).await;
        }
    }
}

#[derive(Debug)]
struct DiscoveryClientState {
    backoff: Duration,
    tracked: HashMap<TypeUrl, HashMap<ResourceId, ResourceVersion>>,
    subscriptions: HashMap<TypeUrl, HashSet<ResourceId>>,
}

impl DiscoveryClientState {
    fn reset_backoff(&mut self) {
        debug!("XDS client connection backoff has been reset");
        self.backoff = INITIAL_BACKOFF;
    }
}

impl<C: bindings::TypedXdsBinding> DeltaClientBackgroundWorker<C> {
    async fn persistently_connect(&mut self, state: &mut DiscoveryClientState) {
        match self.stream_resources(state).await {
            Err(ref e @ XdsError::GrpcStatus(ref status)) => {
                let backoff = std::cmp::min(MAX_BACKOFF, state.backoff * 2);
                let err_detail = e.to_string();
                warn!("xDS client error: {err_detail:?}");
                if status.code() == tonic::Code::Unknown
                    || status.code() == tonic::Code::Cancelled
                    || status.code() == tonic::Code::DeadlineExceeded
                    || status.code() == tonic::Code::Unavailable
                {
                    warn!("xDS client terminated: {}, retrying in {:?}", err_detail, backoff);
                } else {
                    warn!("xDS client interupted: {}, retrying in {:?}", err_detail, backoff);
                }
                let backoff = std::cmp::min(MAX_BACKOFF, state.backoff * 2);
                tokio::time::sleep(backoff).await;
                state.backoff = backoff;
            },
            Err(e) => {
                let backoff = std::cmp::min(MAX_BACKOFF, state.backoff * 2);
                let backoff_slowly = backoff + BACKOFF_INTERVAL;
                warn!("xDS client error: {:?}, retrying in {:?}", e, backoff_slowly);
                tokio::time::sleep(backoff_slowly).await;
                state.backoff = backoff_slowly;
            },
            Ok(()) => {
                warn!("xDS client closed");
            },
        }
    }

    async fn stream_resources(&mut self, state: &mut DiscoveryClientState) -> Result<(), XdsError> {
        let (discovery_requests_tx, mut discovery_requests_rx) = mpsc::channel::<DeltaDiscoveryRequest>(100);

        let resource_types = match C::type_url() {
            Some(type_url) => vec![type_url],
            _ => vec![
                TypeUrl::Secret,
                TypeUrl::ClusterLoadAssignment,
                TypeUrl::Cluster,
                TypeUrl::RouteConfiguration,
                TypeUrl::Listener,
            ],
        };
        let initial_requests: Vec<DeltaDiscoveryRequest> = resource_types
            .iter()
            .map(|resource_type| {
                let subscriptions = state.subscriptions.get(resource_type).cloned().unwrap_or_default();
                let already_tracked: HashMap<ResourceId, ResourceVersion> =
                    state.tracked.get(resource_type).cloned().unwrap_or_default();
                DeltaDiscoveryRequest {
                    node: Some(EnvoyNode { id: self.node.id.clone().into_string(), ..Default::default() }),
                    type_url: resource_type.to_string(),
                    initial_resource_versions: already_tracked,
                    resource_names_subscribe: subscriptions.into_iter().collect(),
                    ..Default::default()
                }
            })
            .collect();

        let outbound_requests = async_stream::stream! {
            for request in initial_requests {
                info!(
                    "sending initial discovery request {request:?}",
                );

                yield request;
            }
            while let Some(message) = discovery_requests_rx.recv().await {
                info!(
                    "sending subsequent discovery request {message:?}",
                );
                yield message
            }
            warn!("outbound discovery request stream has ended!");
        };

        let mut response_stream =
            self.client_binding.delta_request(outbound_requests).await.map_err(XdsError::GrpcStatus)?.into_inner();
        info!("xDS stream established");
        state.reset_backoff();

        loop {
            tokio::select! {
                Some(event) = self.subscriptions_rx.recv() => {
                    self.process_subscription_event(event, state, &discovery_requests_tx).await;
                }
                discovered = response_stream.message() => {
                    let payload = discovered?;
                    let discovery_response = payload.ok_or(XdsError::UnknownResourceType("empty payload received".to_owned()))?;
                    self.process_and_acknowledge(discovery_response, &discovery_requests_tx, state).await?;
                },
                else => {
                    warn!("All channels are closed...exiting");
                    return Ok(())
                }
            }
        }
    }

    async fn process_subscription_event(
        &self,
        event: SubscriptionEvent,
        state: &mut DiscoveryClientState,
        discovery_requests_tx: &tokio::sync::mpsc::Sender<DeltaDiscoveryRequest>,
    ) {
        match event {
            SubscriptionEvent::Subscribe(type_url, resource_id) => {
                debug!(type_url = type_url.to_string(), resource_id, "processing new subscription");
                let is_new = state.subscriptions.entry(type_url).or_default().insert(resource_id.clone());
                if is_new {
                    if let Err(err) = discovery_requests_tx
                        .send(DeltaDiscoveryRequest {
                            node: Some(EnvoyNode { id: self.node.id.clone().into_string(), ..Default::default() }),
                            type_url: type_url.to_string(),
                            resource_names_subscribe: vec![resource_id],
                            ..Default::default()
                        })
                        .await
                    {
                        warn!("problems updating subscription: {:?}", err);
                    }
                }
            },
            SubscriptionEvent::Unsubscribe(type_url, resource_id) => {
                debug!(type_url = type_url.to_string(), resource_id, "processing unsubscribe");
                let was_subscribed = state.subscriptions.entry(type_url).or_default().remove(resource_id.as_str());
                if was_subscribed {
                    if let Err(err) = discovery_requests_tx
                        .send(DeltaDiscoveryRequest {
                            node: Some(EnvoyNode { id: self.node.id.clone().into_string(), ..Default::default() }),
                            type_url: type_url.to_string(),
                            resource_names_unsubscribe: vec![resource_id],
                            ..Default::default()
                        })
                        .await
                    {
                        warn!("problems updating subscription: {:?}", err);
                    }
                }
            },
        }
    }
    async fn process_and_acknowledge(
        &mut self,
        response: DeltaDiscoveryResponse,
        acknowledgments_tx: &mpsc::Sender<DeltaDiscoveryRequest>,
        state: &mut DiscoveryClientState,
    ) -> Result<(), XdsError> {
        let type_url = TypeUrl::try_from(response.type_url.as_str())?;
        let nonce = response.nonce.clone();
        info!(type_url = type_url.to_string(), size = response.resources.len(), "received config resources from xDS");

        let (updates, mut pending_update_versions) = Self::map_updates(state, response, type_url);
        let (internal_ack_tx, internal_ack_rx) = oneshot::channel::<Vec<RejectedConfig>>();
        let notification = XdsUpdateEvent { updates, ack_channel: internal_ack_tx };
        self.resources_tx
            .send(notification)
            .await
            .map_err(|e: mpsc::error::SendError<XdsUpdateEvent>| XdsError::InternalProcessingError(e.to_string()))?;

        tokio::select! {
            ack = internal_ack_rx => {
                match ack {
                    Ok(rejected_configs) => {
                        let error = if rejected_configs.is_empty() {
                            debug!(
                                type_url = type_url.to_string(),
                                nonce,
                                "sending ack response after processing",
                            );
                            let tracked_resources = state.tracked.entry(type_url).or_default();
                            for (resource_id, resource_version) in pending_update_versions.drain() {
                                tracked_resources.insert(resource_id, resource_version);
                            }
                            None
                        } else {
                            let error = rejected_configs
                                    .into_iter()
                                    .map(|reject| reject.to_string())
                                    .collect::<Vec<String>>()
                                    .join("; ");
                            debug!(
                                type_url = type_url.to_string(),
                                error,
                                nonce,
                                "rejecting configs with nack response",
                            );
                            Some(Status {
                                message: error,
                                code: tonic::Code::InvalidArgument.into(),
                                ..Default::default()
                            })
                        };
                        if let Err(err) = acknowledgments_tx.send(DeltaDiscoveryRequest {
                            type_url: type_url.to_string(),
                            response_nonce: nonce,
                            error_detail: error,
                            ..Default::default()
                        })
                        .await
                        {
                            warn!("error in send xDS ack/nack upstream {:?}", err);
                        }
                    },
                    Err(err) => {
                        warn!("error in reading internal ack/nack {:?}", err);
                    },
                }
            }
            () = time::sleep(ACK_TIMEOUT) => {
                warn!("timed out while waiting to acknowledge config updates");
                let error = pending_update_versions.into_keys()
                        .collect::<Vec<String>>()
                        .join("; ");
                let error = Status {
                    message: error,
                    ..Default::default()
                };
                let _ = acknowledgments_tx.send(DeltaDiscoveryRequest {
                    type_url: type_url.to_string(),
                        response_nonce: nonce,
                        error_detail: Some(error),
                        ..Default::default()
                    })
                    .await;
            }
        }

        Ok(())
    }

    fn map_updates(
        state: &mut DiscoveryClientState,
        response: DeltaDiscoveryResponse,
        type_url: TypeUrl,
    ) -> (Vec<XdsResourceUpdate>, HashMap<String, String>) {
        let for_removal: Vec<String> = response
            .removed_resources
            .iter()
            .map(|resource_id| {
                debug!("received delete for config resource {}", resource_id);
                if let Some(resources) = state.tracked.get_mut(&type_url) {
                    resources.remove(resource_id);
                }
                resource_id.clone()
            })
            .collect();

        let mut pending_update_versions = HashMap::<ResourceId, ResourceVersion>::new();

        let updates = response
            .resources
            .into_iter()
            .filter_map(|resource| {
                let resource_id = resource.name.clone();
                let resource_version = resource.version.clone();
                let decoded = XdsResourcePayload::try_from((resource, type_url));
                if decoded.is_err() {
                    warn!("problem decoding config update for {} : error {:?}", resource_id, decoded.as_ref().err());
                } else {
                    pending_update_versions.insert(resource_id.clone(), resource_version);
                    debug!("decoded config update for resource {resource_id}");
                }
                decoded.ok().map(|value| XdsResourceUpdate::Update(resource_id.clone(), Box::new(value)))
            })
            .chain(for_removal.into_iter().map(|resource_id| XdsResourceUpdate::Remove(resource_id, type_url)))
            .collect();

        (updates, pending_update_versions)
    }
}
