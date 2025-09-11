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

use std::sync::Arc;

mod default_balancer;
pub(crate) mod hash_policy;
pub(crate) mod healthy;
pub(crate) mod least;
pub(crate) mod maglev;
pub(crate) mod priority;
pub(crate) mod random;
pub(crate) mod ring;
pub(crate) mod wrr;

pub use default_balancer::{DefaultBalancer, EndpointWithAuthority, EndpointWithLoad, WeightedEndpoint};

pub trait Balancer<E> {
    fn next_item(&mut self, hash: Option<u64>) -> Option<Arc<E>>;
}
