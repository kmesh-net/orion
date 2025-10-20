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
    fmt,
    fmt::{Display, Formatter},
};

use anyhow::Result;
use envoy_data_plane_api::{
    envoy::{
        config::{
            cluster::v3::Cluster, endpoint::v3::ClusterLoadAssignment, listener::v3::Listener,
            route::v3::RouteConfiguration,
        },
        extensions::transport_sockets::tls::v3::Secret,
        service::discovery::v3::{DeltaDiscoveryRequest, Resource},
    },
    prost,
    prost::Message,
    tonic,
};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::mpsc;

pub type ResourceId = String;
pub type ResourceVersion = String;

#[derive(Clone, Debug)]
pub enum XdsResourceUpdate {
    Update(ResourceId, Box<XdsResourcePayload>),
    Remove(ResourceId, TypeUrl),
}

impl XdsResourceUpdate {
    pub fn id(&self) -> ResourceId {
        match self {
            XdsResourceUpdate::Update(id, _) => id.to_string(),
            XdsResourceUpdate::Remove(id, _) => id.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum XdsResourcePayload {
    Listener(ResourceId, Box<Listener>),
    Cluster(ResourceId, Box<Cluster>),
    Endpoints(ResourceId, Box<ClusterLoadAssignment>),
    RouteConfiguration(ResourceId, Box<RouteConfiguration>),
    Secret(ResourceId, Box<Secret>),
}

impl TryFrom<(Resource, TypeUrl)> for XdsResourcePayload {
    type Error = XdsError;

    fn try_from((resource, type_url): (Resource, TypeUrl)) -> Result<XdsResourcePayload, XdsError> {
        let resource_id = resource.name;
        resource.resource.ok_or(XdsError::MissingResource()).and_then(|res| match type_url {
            TypeUrl::Listener => {
                let decoded = Listener::decode(res.value.as_slice()).map_err(XdsError::Decode);
                decoded.map(|value| XdsResourcePayload::Listener(resource_id, Box::new(value)))
            },
            TypeUrl::Cluster => {
                let decoded = Cluster::decode(res.value.as_slice()).map_err(XdsError::Decode);
                decoded.map(|value| XdsResourcePayload::Cluster(resource_id, Box::new(value)))
            },
            TypeUrl::RouteConfiguration => {
                let decoded = RouteConfiguration::decode(res.value.as_slice()).map_err(XdsError::Decode);
                decoded.map(|value| XdsResourcePayload::RouteConfiguration(resource_id, Box::new(value)))
            },
            TypeUrl::ClusterLoadAssignment => {
                let decoded = ClusterLoadAssignment::decode(res.value.as_slice()).map_err(XdsError::Decode);
                decoded.map(|value| XdsResourcePayload::Endpoints(resource_id, Box::new(value)))
            },
            TypeUrl::Secret => {
                let decoded = Secret::decode(res.value.as_slice()).map_err(XdsError::Decode);
                decoded.map(|value| XdsResourcePayload::Secret(resource_id, Box::new(value)))
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
            value => Err(XdsError::UnknownResourceType(format!("did not recognise type_url {value}"))),
        }
    }
}

#[derive(Debug)]
pub struct RejectedConfig {
    name: ResourceId,
    reason: anyhow::Error,
}
impl From<(ResourceId, anyhow::Error)> for RejectedConfig {
    fn from(context: (ResourceId, anyhow::Error)) -> RejectedConfig {
        RejectedConfig { name: context.0, reason: context.1 }
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
    GrpcStatus(Box<tonic::Status>),
    #[error(transparent)]
    RequestFailure(#[from] Box<mpsc::error::SendError<DeltaDiscoveryRequest>>),
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
}

impl From<tonic::Status> for XdsError {
    fn from(status: tonic::Status) -> Self {
        XdsError::GrpcStatus(Box::new(status))
    }
}
