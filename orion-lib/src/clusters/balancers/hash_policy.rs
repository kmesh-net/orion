#![allow(clippy::too_many_lines)]

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

use std::hash::Hasher;
use std::net::SocketAddr;
use std::ops::ControlFlow;

use http::Request;
use hyper::body::Incoming;
use orion_configuration::config::network_filters::http_connection_manager::route::{HashPolicy, HashPolicyResult};
use twox_hash::XxHash64;

use crate::body::timeout_body::TimeoutBody;

#[derive(Clone, Debug)]
pub struct HashState<'a, B = TimeoutBody<Incoming>> {
    policies: &'a [HashPolicy],
    req: &'a Request<B>,
    src_addr: SocketAddr,
}

impl<'a, B> HashState<'a, B> {
    pub fn new(policies: &'a [HashPolicy], req: &'a Request<B>, src_addr: SocketAddr) -> Self {
        Self { policies, req, src_addr }
    }
    pub fn compute(self) -> Option<u64> {
        if self.policies.is_empty() {
            return None;
        }
        let mut hasher = DeterministicBuildHasher::build_hasher();
        match self.policies.iter().try_fold(false, |prev, policy| {
            match policy.apply(&mut hasher, self.req, self.src_addr) {
                HashPolicyResult::Applied => ControlFlow::Continue(true),
                HashPolicyResult::Skipped => ControlFlow::Continue(prev),
                HashPolicyResult::Terminal => ControlFlow::Break(()),
            }
        }) {
            ControlFlow::Continue(applied) => applied.then_some(hasher.finish()),
            ControlFlow::Break(()) => Some(hasher.finish()),
        }
    }
}

/// Similar to [std::hash::BuildHasher] but with a deterministic seed.
#[derive(Default)]
pub(crate) struct DeterministicBuildHasher;

impl DeterministicBuildHasher {
    const SEED: u64 = 0;

    pub fn build_hasher() -> XxHash64 {
        XxHash64::with_seed(Self::SEED)
    }

    // FIXME(oriol): for some reason Clippy 1.78 was failing in CI and couldn't find `Hash`:
    //   error[E0404]: expected trait, found derive macro `Hash`
    // ...despite `Hash` being imported. This could not be reproduced locally, so leaving this
    // fully qualified name to pass CI.
    pub fn hash_one_with_seed<T: std::hash::Hash>(x: T, seed: u64) -> u64 {
        let mut hasher = XxHash64::with_seed(seed);
        x.hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
mod test {
    use super::{DeterministicBuildHasher, HashPolicy, HashState};
    use http::{request::Builder, HeaderName, HeaderValue, Request};
    use orion_configuration::config::network_filters::http_connection_manager::route::PolicySpecifier;
    use std::{
        hash::{Hash, Hasher},
        net::SocketAddr,
    };
    use twox_hash::XxHash64;

    pub struct TestHasher(XxHash64);

    impl TestHasher {
        fn new() -> Self {
            Self(DeterministicBuildHasher::build_hasher())
        }

        fn hash<T: Hash>(mut self, value: T) -> Self {
            value.hash(&mut self.0);
            self
        }

        fn finish(self) -> u64 {
            self.0.finish()
        }
    }

    fn build_request<'a>(uri: &str, headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Request<()> {
        let mut builder = Builder::new().uri(uri);

        builder = headers.into_iter().fold(builder, |builder, (key, value)| builder.header(key, value));

        builder.body(()).unwrap()
    }

    fn hasher_from_policies<'a>(policies: impl IntoIterator<Item = (&'a PolicySpecifier, bool)>) -> Vec<HashPolicy> {
        policies
            .into_iter()
            .map(|(policy_specifier, terminal)| HashPolicy { policy_specifier: policy_specifier.clone(), terminal })
            .collect()
    }

    #[test]
    fn hash_policy() {
        let source_ip = SocketAddr::from(([192, 168, 0, 1], 8000));

        let policy_header = PolicySpecifier::Header(HeaderName::from_static("lb-header"));
        let policy_query = PolicySpecifier::QueryParameter("lb-param".into());
        let policy_addr = PolicySpecifier::SourceIp(true);

        // Check header hashing
        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_header, false)]),
                &build_request("https://example.com", [("Lb-Header", "foo")]),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash(HeaderValue::from_static("foo")).finish()
        );

        // Case insensitive
        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_header, false)]),
                &build_request("https://example.com", [("lb-header", "foo")]),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash(HeaderValue::from_static("foo")).finish()
        );

        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_header, false)]),
                &build_request(
                    "https://example.com",
                    [("First-Header", "bar"), ("Lb-Header", "foo"), ("Last-Header", "zarp")]
                ),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash(HeaderValue::from_static("foo")).finish()
        );

        assert!(HashState::new(
            &hasher_from_policies([(&policy_header, false)]),
            &build_request("https://example.com", [("Different-Header", "foo")]),
            source_ip
        )
        .compute()
        .is_none());

        assert!(HashState::new(
            &hasher_from_policies([(&policy_header, false)]),
            &build_request("https://example.com", None),
            source_ip
        )
        .compute()
        .is_none());

        // Check query parameter hashing
        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_query, false)]),
                &build_request("https://example.com/?lb-param=bar", None),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash("bar").finish()
        );

        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_query, false)]),
                &build_request("https://example.com/?first=foo&lb-param=bar&last=zarp", None),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash("bar").finish()
        );

        // Case sensitive
        assert!(HashState::new(
            &hasher_from_policies([(&policy_query, false)]),
            &build_request("https://example.com/?Lb-Param=bar", None),
            source_ip
        )
        .compute()
        .is_none());

        assert!(HashState::new(
            &hasher_from_policies([(&policy_query, false)]),
            &build_request("https://example.com/?different-param=bar", None),
            source_ip
        )
        .compute()
        .is_none());

        // Check IP address hashing
        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_addr, false)]),
                &build_request("https://example.com/", None),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash(source_ip).finish()
        );

        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_addr, false)]),
                &build_request("https://example.com/?lb-param=bar", [("Lb-Header", "foo")]),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash(source_ip).finish()
        );

        // Check chains of policies
        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_header, false), (&policy_query, false), (&policy_addr, false)]),
                &build_request("https://example.com/?lb-param=bar", [("Lb-Header", "foo")]),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash(HeaderValue::from_static("foo")).hash("bar").hash(source_ip).finish()
        );

        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_header, false), (&policy_query, false), (&policy_addr, false)]),
                &build_request("https://example.com/", [("Lb-Header", "foo")]),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash(HeaderValue::from_static("foo")).hash(source_ip).finish()
        );

        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_header, false), (&policy_query, false), (&policy_addr, false)]),
                &build_request("https://example.com/?lb-param=bar", None),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash("bar").hash(source_ip).finish()
        );

        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_header, false), (&policy_query, false)]),
                &build_request("https://example.com/?lb-param=bar", [("Lb-Header", "foo")]),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash(HeaderValue::from_static("foo")).hash("bar").finish()
        );

        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_header, false), (&policy_query, true), (&policy_addr, false)]),
                &build_request("https://example.com/?lb-param=bar", [("Lb-Header", "foo")]),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash(HeaderValue::from_static("foo")).hash("bar").finish()
        );

        assert_eq!(
            HashState::new(
                &hasher_from_policies([(&policy_header, false), (&policy_query, true), (&policy_addr, false)]),
                &build_request("https://example.com/", [("Lb-Header", "foo")]),
                source_ip
            )
            .compute()
            .unwrap(),
            TestHasher::new().hash(HeaderValue::from_static("foo")).hash(source_ip).finish()
        );
    }
}
