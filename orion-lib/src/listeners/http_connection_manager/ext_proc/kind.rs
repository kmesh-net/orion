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

#[derive(Debug, Clone, Default)]
pub struct Observability {}

#[derive(Debug, Copy, Clone, Default)]
pub struct Processing {}

pub trait Mode {
    const OBSERVABILITY: bool;
}

impl Mode for Observability {
    const OBSERVABILITY: bool = true;
}

impl Mode for Processing {
    const OBSERVABILITY: bool = false;
}

#[derive(Debug, Clone, Default)]
pub struct RequestMsg {}
#[derive(Debug, Clone, Default)]
pub struct ResponseMsg {}

pub trait MsgKind {
    const IS_REQUEST: bool;
    #[allow(dead_code)]
    const IS_RESPONSE: bool = !Self::IS_REQUEST;
    #[allow(dead_code)]
    const NAME: &'static str = if Self::IS_REQUEST { "request" } else { "response" };
}

impl MsgKind for RequestMsg {
    const IS_REQUEST: bool = true;
}

impl MsgKind for ResponseMsg {
    const IS_REQUEST: bool = false;
}
