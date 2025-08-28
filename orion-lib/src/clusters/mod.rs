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

pub(crate) mod balancers;
pub(crate) mod cached_watch;
pub mod cluster;
pub mod clusters_manager;
pub(crate) mod health;
pub(crate) mod load_assignment;
pub(crate) mod retry_policy;
pub use crate::transport::{GrpcService, SimpleRoundRobinGrpcServiceLB};
pub use load_assignment::{ClusterLoadAssignmentBuilder, PartialClusterLoadAssignment};

pub use clusters_manager::{
    RoutingContext, RoutingRequirement, add_cluster, all_grpc_connections, change_cluster_load_assignment,
    get_all_clusters, get_cluster_routing_requirements, get_grpc_connection, get_http_connection, get_tcp_connection,
    remove_cluster, remove_cluster_load_assignment, resolve_cluster, update_endpoint_health, update_tls_context,
};
