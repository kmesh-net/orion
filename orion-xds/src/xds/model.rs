// Copyright 2025 The kmesh Authors
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

use core::result::Result::Err;

use orion_configuration::config::{
    cluster::ClusterLoadAssignment, network_filters::http_connection_manager::RouteConfiguration, secret::Secret,
    Cluster, GenericError, Listener,
};
use orion_data_plane_api::envoy_data_plane_api::{
    envoy::{
        config::{
            cluster::v3::Cluster as EnvoyCluster, endpoint::v3::ClusterLoadAssignment as EnvoyClusterLoadAssignment,
            listener::v3::Listener as EnvoyListener, route::v3::RouteConfiguration as EnvoyRouteConfiguration,
        },
        extensions::transport_sockets::tls::v3::Secret as EnvoySecret,
        service::discovery::v3::Resource,
    },
    prost,
    prost::Message,
    tonic,
};
use serde::Deserialize;
use std::{
    fmt,
    fmt::{Display, Formatter},
};
use thiserror::Error;
use tokio::sync::mpsc;

pub type ResourceId = String;
pub type ResourceVersion = String;

#[derive(Debug)]
pub enum XdsResourceUpdate {
    Update(ResourceId, XdsResourcePayload, ResourceVersion),
    Remove(ResourceId, TypeUrl),
}

impl XdsResourceUpdate {
    pub fn id(&self) -> ResourceId {
        match self {
            XdsResourceUpdate::Update(id, _, _) | XdsResourceUpdate::Remove(id, _) => id.clone(),
        }
    }
}

#[derive(Debug)]
pub enum XdsResourcePayload {
    Listener(ResourceId, Listener),
    Cluster(ResourceId, Cluster),
    Endpoints(ResourceId, ClusterLoadAssignment),
    RouteConfiguration(ResourceId, RouteConfiguration),
    Secret(ResourceId, Secret),
}

impl TryFrom<(Resource, TypeUrl)> for XdsResourcePayload {
    type Error = XdsError;

    fn try_from((resource, type_url): (Resource, TypeUrl)) -> Result<XdsResourcePayload, XdsError> {
        let resource_id = resource.name;
        resource.resource.ok_or(XdsError::MissingResource()).and_then(|res| match type_url {
            TypeUrl::Listener => {
                let decoded = EnvoyListener::decode(res.value.as_slice())?.try_into()?;
                Ok(XdsResourcePayload::Listener(resource_id, decoded))
            },
            TypeUrl::Cluster => {
                let decoded = EnvoyCluster::decode(res.value.as_slice())?.try_into()?;
                Ok(XdsResourcePayload::Cluster(resource_id, decoded))
            },
            TypeUrl::RouteConfiguration => {
                let decoded = EnvoyRouteConfiguration::decode(res.value.as_slice())?.try_into()?;
                Ok(XdsResourcePayload::RouteConfiguration(resource_id, decoded))
            },
            TypeUrl::ClusterLoadAssignment => {
                let decoded = EnvoyClusterLoadAssignment::decode(res.value.as_slice())?.try_into()?;
                Ok(XdsResourcePayload::Endpoints(resource_id, decoded))
            },
            TypeUrl::Secret => {
                let decoded = EnvoySecret::decode(res.value.as_slice())?.try_into()?;
                Ok(XdsResourcePayload::Secret(resource_id, decoded))
            },
        })
    }
}

#[derive(Eq, Hash, PartialEq, Debug, Copy, Clone, Deserialize)]
pub enum TypeUrl {
    Listener,
    Cluster,
    RouteConfiguration,
    ClusterLoadAssignment,
    Secret,
}

impl fmt::Display for TypeUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                TypeUrl::Listener => "type.googleapis.com/envoy.config.listener.v3.Listener".to_owned(),
                TypeUrl::Cluster => "type.googleapis.com/envoy.config.cluster.v3.Cluster".to_owned(),
                TypeUrl::RouteConfiguration =>
                    "type.googleapis.com/envoy.config.route.v3.RouteConfiguration".to_owned(),
                TypeUrl::ClusterLoadAssignment =>
                    "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment".to_owned(),
                TypeUrl::Secret => "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.Secret".to_owned(),
            }
        )
    }
}

impl TryFrom<&str> for TypeUrl {
    type Error = XdsError;

    fn try_from(type_url_string: &str) -> Result<TypeUrl, XdsError> {
        match type_url_string {
            "type.googleapis.com/envoy.config.listener.v3.Listener" => Ok(TypeUrl::Listener),
            "type.googleapis.com/envoy.config.cluster.v3.Cluster" => Ok(TypeUrl::Cluster),
            "type.googleapis.com/envoy.config.route.v3.RouteConfiguration" => Ok(TypeUrl::RouteConfiguration),
            "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment" => Ok(TypeUrl::ClusterLoadAssignment),
            "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.Secret" => Ok(TypeUrl::Secret),
            value => Err(XdsError::UnknownResourceType(value.to_owned())),
        }
    }
}

#[derive(Debug)]
pub struct RejectedConfig {
    name: ResourceId,
    reason: orion_error::Error,
}
impl<E: Into<orion_error::Error>> From<(ResourceId, E)> for RejectedConfig {
    fn from(context: (ResourceId, E)) -> RejectedConfig {
        RejectedConfig { name: context.0, reason: context.1.into() }
    }
}
impl Display for RejectedConfig {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.reason)
    }
}

#[derive(Error, Debug)]
pub enum XdsError {
    #[error("gRPC error ({}): {}", .0.code(), .0.message())]
    GrpcStatus(#[from] tonic::Status),
    #[error("unknown resource type: {0}")]
    UnknownResourceType(String),
    #[error("error decoding xDS payload: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("malformed xDS payload, missing resource")]
    MissingResource(),
    #[error("problem occured during processing")]
    InternalProcessingError(String),
    #[error("cannot construct client: {0}")]
    BuilderFailed(String),
    #[error("failed to convert envoy type")]
    ConversionError(#[from] GenericError),
    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),
    #[error("Failed to push xDS subscription event to channel")]
    SubscriptionFailure(#[from] mpsc::error::SendError<crate::xds::client::SubscriptionEvent>),
}
