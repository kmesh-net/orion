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

use super::model::{ResourceId, ResourceVersion, TypeUrl};

use orion_data_plane_api::envoy_data_plane_api::{
    envoy::{config::core::v3::Node as EnvoyNode, service::discovery::v3::DeltaDiscoveryRequest},
    google::rpc::Status,
    tonic,
};
use orion_configuration::config::bootstrap::Node;
use std::collections::HashMap;

pub struct StatusBuilder {
    code: i32,
    message: String,
}

#[allow(dead_code)]
impl StatusBuilder {
    pub fn unspecified_error() -> Self {
        StatusBuilder { code: tonic::Code::Unknown.into(), message: String::new() }
    }

    pub fn invalid_argument() -> Self {
        StatusBuilder { code: tonic::Code::InvalidArgument.into(), message: String::new() }
    }

    pub fn with_message(mut self, message: String) -> Self {
        self.message = message;
        self
    }

    pub fn build(self) -> Status {
        Status { message: self.message, code: self.code, ..Default::default() }
    }
}

pub struct DeltaDiscoveryRequestBuilder {
    node: Option<Node>,
    nounce: Option<String>,
    type_url: TypeUrl,
    error_detail: Option<Status>,
    resource_names_subscribe: Vec<ResourceId>,
    resource_names_unsubscribe: Vec<ResourceId>,
    initial_resource_versions: HashMap<ResourceId, ResourceVersion>,
}

#[allow(dead_code)]
impl DeltaDiscoveryRequestBuilder {
    pub fn for_resource(type_url: TypeUrl) -> Self {
        DeltaDiscoveryRequestBuilder {
            node: None,
            nounce: None,
            type_url,
            error_detail: None,
            resource_names_subscribe: Vec::new(),
            resource_names_unsubscribe: Vec::new(),
            initial_resource_versions: HashMap::new(),
        }
    }

    pub fn with_node_id(mut self, node: Node) -> Self {
        self.node = Some(node);
        self
    }

    pub fn with_nounce(mut self, nounce: String) -> Self {
        self.nounce = Some(nounce);
        self
    }

    pub fn with_resource_names_subscribe(mut self, resource_names: Vec<ResourceId>) -> Self {
        self.resource_names_subscribe = resource_names;
        self
    }

    pub fn with_resource_names_unsubscribe(mut self, resource_names: Vec<ResourceId>) -> Self {
        self.resource_names_unsubscribe = resource_names;
        self
    }

    pub fn with_initial_resource_versions(
        mut self,
        initial_resource_versions: HashMap<ResourceId, ResourceVersion>,
    ) -> Self {
        self.initial_resource_versions = initial_resource_versions;
        self
    }

    pub fn with_error_detail(mut self, error_detail: Option<Status>) -> Self {
        self.error_detail = error_detail;
        self
    }

    pub fn build(self) -> DeltaDiscoveryRequest {
        let Node { id, cluster_id } = self.node.unwrap_or_default();
        let nounce = self.nounce.unwrap_or_default();
        DeltaDiscoveryRequest {
            node: Some(EnvoyNode { id: id.into(), cluster: cluster_id.into(), ..Default::default() }),
            response_nonce: nounce,
            type_url: self.type_url.to_string(),
            resource_names_subscribe: self.resource_names_subscribe,
            resource_names_unsubscribe: self.resource_names_unsubscribe,
            initial_resource_versions: self.initial_resource_versions,
            error_detail: self.error_detail,
            ..Default::default()
        }
    }
}
