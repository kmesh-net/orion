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

use crate::PolyBody;

use orion_data_plane_api::envoy_data_plane_api::envoy::service::ext_proc::v3::HeaderMutation;

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Action<P> {
    Send(P),
    Return(ProcessingStatus),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum ProcessingStatus {
    RequestReady(ReadyStatus),
    ResponseReady(ReadyStatus),
    HaltedOnError,
    EndWithDirectResponse(http::Response<PolyBody>),
}

impl ProcessingStatus {
    pub fn with_request_ready<F>(&mut self, f: F)
    where
        F: FnOnce(&mut ReadyStatus),
    {
        if let ProcessingStatus::RequestReady(ready) = self {
            f(ready);
        }
    }

    pub fn with_response_ready<F>(&mut self, f: F)
    where
        F: FnOnce(&mut ReadyStatus),
    {
        if let ProcessingStatus::ResponseReady(ready) = self {
            f(ready);
        }
    }
}

#[derive(Debug, Default)]
pub struct ReadyStatus {
    pub headers_modifications: Option<HeaderMutation>,
    pub clear_route_cache: bool,
}
