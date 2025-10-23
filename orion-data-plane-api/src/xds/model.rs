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

use std::fmt;

use anyhow::Result;
use envoy_data_plane_api::{envoy::service::discovery::v3::DeltaDiscoveryRequest, prost, tonic};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::mpsc;

pub type ResourceId = String;
pub type ResourceVersion = String;

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
