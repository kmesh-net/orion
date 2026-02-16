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

use std::time::Duration;

use orion_configuration::config::network_filters::http_connection_manager::http_filters::ext_proc::{
    GrpcServiceSpecifier, HeaderMutationRules, ProcessingMode, RouteCacheAction,
};

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct ExternalProcessingWorkerConfig {
    pub grpc_service_specifier: GrpcServiceSpecifier,
    pub message_timeout: Duration,
    pub max_message_timeout: Option<Duration>,
    pub observability_mode: bool,
    pub failure_mode_allow: bool,
    pub disable_immediate_response: bool,
    pub mutation_rules: HeaderMutationRules,
    pub processing_mode: ProcessingMode,
    pub allowed_override_modes: Vec<ProcessingMode>,
    pub allow_mode_override: bool,
    pub route_cache_action: RouteCacheAction,
    pub send_body_without_waiting_for_header_response: bool,
}
