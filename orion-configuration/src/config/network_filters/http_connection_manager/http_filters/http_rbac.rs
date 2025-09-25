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

use std::collections::BTreeMap;

use crate::config::network_filters::{http_connection_manager::header_matcher::HeaderMatcher, network_rbac::Action};
use compact_str::CompactString;
use http::Request;
use serde::{Deserialize, Serialize};
use tracing::debug;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HttpRbac {
    pub action: Action,
    pub policies: BTreeMap<CompactString, Policy>,
}

//since we support different permission and principals for http vs network rbac, this struct is different from the network rbac
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Policy {
    pub permissions: Vec<Permission>,
    pub principals: Vec<Principal>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Any,
    Header(HeaderMatcher),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Principal {
    Any,
    Header(HeaderMatcher),
}

impl Permission {
    fn is_applicable<B>(&self, req: &Request<B>) -> bool {
        match self {
            Self::Any => true,
            Self::Header(h) => h.request_matches(req),
        }
    }
}

impl Principal {
    fn has_principal<B>(&self, req: &Request<B>) -> bool {
        match self {
            Principal::Any => true,
            Principal::Header(h) => h.request_matches(req),
        }
    }
}

impl Policy {
    fn enforce<B>(&self, req: &Request<B>) -> bool {
        let has_permission = self.permissions.iter().any(|p| p.is_applicable(req));
        let has_principal = self.principals.iter().any(|p| p.has_principal(req));
        debug!("Enforcing policy permissions {has_permission} principals {has_principal}");
        has_permission && has_principal
    }
}

impl HttpRbac {
    pub fn is_permitted<B>(&self, req: &Request<B>) -> (bool, Option<CompactString>) {
        // policies are applied in the lexicographical order of their keys
        let enforced_policy = self.policies.iter().find(|(_, p)| p.enforce(req)).map(|(name, _)| name.clone());
        let permitted = match self.action {
            Action::Allow => enforced_policy.is_some(),
            Action::Deny => enforced_policy.is_none(),
        };

        debug!("HttpRbac: rule is enforced by {enforced_policy:?} with action: {:?} -> permitted {permitted}", self.action);
        (permitted, enforced_policy)
    }
}

#[cfg(test)]
mod rbac_tests {
    use super::*;
    use crate::config::core::{StringMatcher, StringMatcherPattern};
    use http::{header::HOST, HeaderMap, HeaderValue, Request};
    fn create_host_request(host: &str) -> Request<()> {
        let mut hm = HeaderMap::new();
        hm.insert(HOST, HeaderValue::from_str(host).unwrap());
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm;
        builder.body(()).unwrap()
    }

    fn create_host_matcher(host: &str) -> HeaderMatcher {
        HeaderMatcher {
            header_name: HOST.into(),
            invert_match: false,
            treat_missing_header_as_empty: false,
            header_matcher: StringMatcher { ignore_case: true, pattern: StringMatcherPattern::Exact(host.into()) },
        }
    }

    #[test]
    fn rule_test_allow_any() {
        let permission = Permission::Any;
        let principal = Principal::Any;
        let policy = Policy { permissions: vec![permission], principals: vec![principal] };
        let rbac_rule = HttpRbac { action: Action::Allow, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) = rbac_rule.is_permitted(&create_host_request("blah.com"));
        assert!(permitted);
        assert_eq!(rule, Some("my-id".into()));
    }
    #[test]
    fn rule_test_allow_host_permission_any_principal() {
        let host = "blah.com";
        let permission = Permission::Header(create_host_matcher(host));
        let principal = Principal::Any;
        let policy = Policy { permissions: vec![permission], principals: vec![principal] };
        let rbac_rule = HttpRbac { action: Action::Allow, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) = rbac_rule.is_permitted(&create_host_request(host));
        assert!(permitted);
        assert_eq!(rule, Some("my-id".into()));
    }

    #[test]
    fn rule_test_allow_host_and_any_permission_any_principal() {
        let host = "blah2.com";
        let permission1 = Permission::Header(create_host_matcher(host));
        let permission2 = Permission::Any;
        let principal = Principal::Any;
        let policy = Policy { permissions: vec![permission1, permission2], principals: vec![principal] };
        let rbac_rule = HttpRbac { action: Action::Allow, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) = rbac_rule.is_permitted(&create_host_request(host));
        assert!(permitted);
        assert_eq!(rule, Some("my-id".into()));
    }

    #[test]
    fn rule_test_allow_host_permission_host_principal() {
        let host = "blah.com";
        let permission = Permission::Header(create_host_matcher(host));
        let principal = Principal::Header(create_host_matcher(host));
        let policy = Policy { permissions: vec![permission], principals: vec![principal] };
        let rbac_rule = HttpRbac { action: Action::Allow, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) = rbac_rule.is_permitted(&create_host_request(host));
        assert!(permitted);
        assert_eq!(rule, Some("my-id".into()));
    }

    #[test]
    fn rule_test_deny_any() {
        let host = "blah.com";
        let permission = Permission::Any;
        let principal = Principal::Any;
        let policy = Policy { permissions: vec![permission], principals: vec![principal] };
        let rbac_rule = HttpRbac { action: Action::Deny, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) = rbac_rule.is_permitted(&create_host_request(host));
        assert!(!permitted);
        assert_eq!(rule, Some("my-id".into()));
    }

    #[test]
    fn rule_test_deny_host_permission_any_principal() {
        let host = "blah.com";
        let permission = Permission::Header(create_host_matcher(host));
        let principal = Principal::Any;
        let policy = Policy { permissions: vec![permission], principals: vec![principal] };
        let rbac_rule = HttpRbac { action: Action::Deny, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) = rbac_rule.is_permitted(&create_host_request(host));
        assert!(!permitted);
        assert_eq!(rule, Some("my-id".into()));
    }

    #[test]
    fn rule_test_deny_host_permission_host_principal() {
        let host = "blah.com";
        let permission = Permission::Header(create_host_matcher(host));
        let principal = Principal::Header(create_host_matcher(host));
        let policy = Policy { permissions: vec![permission], principals: vec![principal] };
        let rbac_rule = HttpRbac { action: Action::Deny, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) = rbac_rule.is_permitted(&create_host_request(host));
        assert!(!permitted);
        assert_eq!(rule, Some("my-id".into()));
    }
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{HttpRbac, Permission, Policy, Principal};
    use crate::config::{common::*, network_filters::network_rbac::Action};
    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        config::rbac::v3::{
            permission::Rule as EnvoyPermissionRule, principal::Identifier as EnvoyPrincipalIdentifier,
            Permission as EnvoyPermission, Policy as EnvoyPolicy, Principal as EnvoyPrincipal, Rbac as EnvoyHttpRbac,
        },
        extensions::filters::http::rbac::v3::Rbac as EnvoyRbac,
    };

    impl TryFrom<EnvoyRbac> for HttpRbac {
        type Error = GenericError;
        fn try_from(envoy: EnvoyRbac) -> Result<Self, Self::Error> {
            let EnvoyRbac {
                rules,
                matcher,
                shadow_rules,
                shadow_matcher,
                shadow_rules_stat_prefix,
                rules_stat_prefix,
                track_per_rule_stats,
            } = envoy;
            unsupported_field!(
                // rules,
                matcher,
                shadow_rules,
                shadow_matcher,
                shadow_rules_stat_prefix,
                rules_stat_prefix,
                track_per_rule_stats
            )?;
            convert_opt!(rules)
        }
    }

    impl TryFrom<EnvoyHttpRbac> for HttpRbac {
        type Error = GenericError;

        fn try_from(envoy: EnvoyHttpRbac) -> Result<Self, Self::Error> {
            let EnvoyHttpRbac { action, policies, audit_logging_options } = envoy;
            unsupported_field!(audit_logging_options)?;
            let action = Action::try_from(action).with_node("action")?;
            let policies = required!(policies)?
                .into_iter()
                .map(|(id, pol)| Policy::try_from(pol).map(|value| (id.into(), value)))
                .collect::<Result<_, _>>()?;
            Ok(HttpRbac { action, policies })
        }
    }

    impl TryFrom<EnvoyPolicy> for Policy {
        type Error = GenericError;
        fn try_from(envoy: EnvoyPolicy) -> Result<Self, Self::Error> {
            let EnvoyPolicy { permissions, principals, condition, checked_condition } = envoy;
            unsupported_field!(condition, checked_condition)?;
            let permissions = convert_vec!(permissions)?;
            let principals = convert_vec!(principals)?;
            Ok(Self { permissions, principals })
        }
    }

    impl TryFrom<EnvoyPermission> for Permission {
        type Error = GenericError;
        fn try_from(envoy: EnvoyPermission) -> Result<Self, Self::Error> {
            let EnvoyPermission { rule } = envoy;
            match required!(rule)? {
                EnvoyPermissionRule::Any(true) => Ok(Self::Any),
                EnvoyPermissionRule::Any(false) => Err(GenericError::from_msg("Any has to be true")),
                EnvoyPermissionRule::Header(header) => header.try_into().map(Self::Header),
                //todo(hayley): write all these out
                _ => return Err(GenericError::unsupported_variant("[Unsupported Permission Rule]")),
            }
            .with_node("rule")
        }
    }

    impl TryFrom<EnvoyPrincipal> for Principal {
        type Error = GenericError;
        fn try_from(value: EnvoyPrincipal) -> Result<Self, Self::Error> {
            let EnvoyPrincipal { identifier } = value;
            match required!(identifier)? {
                EnvoyPrincipalIdentifier::Any(true) => Ok(Self::Any),
                EnvoyPrincipalIdentifier::Any(false) => Err(GenericError::from_msg("Any has to be true")),
                EnvoyPrincipalIdentifier::Header(header) => header.try_into().map(Self::Header),
                _ => return Err(GenericError::unsupported_variant("[Unsupported Principal Identifier]")),
            }
            .with_node("identifier")
        }
    }
}
