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

use crate::config::core::StringMatcher;
use compact_str::CompactString;
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr};
use tracing::debug;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct NetworkRbac {
    pub action: Action,
    pub policies: BTreeMap<CompactString, Policy>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Policy {
    pub permissions: Vec<Permission>,
    pub principals: Vec<Principal>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Any,
    DestinationIp(IpNet),
    DestinationPort(u16),
    DestinationPortRange(std::ops::Range<u16>),
    ServerName(StringMatcher),
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Principal {
    Any,
    DownstreamRemoteIp(IpNet),
}

#[derive(Debug, Clone, Copy)]
pub struct NetworkContext<'a> {
    destination: SocketAddr,
    downstream: SocketAddr,
    server_name: Option<&'a str>,
}

impl<'a> NetworkContext<'a> {
    pub fn new(destination: SocketAddr, downstream: SocketAddr, server_name: Option<&'a str>) -> Self {
        Self { destination, downstream, server_name }
    }
}

impl Permission {
    fn is_applicable(&self, ctx: &NetworkContext) -> bool {
        match self {
            Self::Any => true,
            Self::DestinationIp(ip) => ip.contains(&ctx.destination.ip()),
            Self::DestinationPort(port) => ctx.destination.port() == *port,
            Self::DestinationPortRange(range) => range.contains(&ctx.destination.port()),
            Self::ServerName(matcher) => match ctx.server_name.map(|sn| matcher.matches(sn)) {
                Some(true) => true,
                Some(false) | None => false,
            },
        }
    }
}

impl Principal {
    fn has_principal(&self, ctx: &NetworkContext) -> bool {
        match self {
            Principal::Any => true,
            Principal::DownstreamRemoteIp(ip) => ip.contains(&ctx.downstream.ip()),
        }
    }
}

impl Policy {
    fn enforce(&self, ctx: &NetworkContext) -> bool {
        let has_permission = self.permissions.iter().any(|p| p.is_applicable(ctx));
        let has_principal = self.principals.iter().any(|p| p.has_principal(ctx));
        debug!("Enforcing policy permissions {has_permission} principals {has_principal}");
        has_permission && has_principal
    }
}

impl NetworkRbac {
    pub fn is_permitted(&self, ctx: &NetworkContext) -> (bool, Option<CompactString>) {
        let enforced_policy = self.policies.iter().find(|(_, p)| p.enforce(ctx)).map(|(id, _)| id.clone());
        let permitted = match self.action {
            Action::Allow => enforced_policy.is_some(),
            Action::Deny => enforced_policy.is_none(),
        };

        debug!(
            "NetworkRbac: rule is enforced by {enforced_policy:?} with action: {:?} -> permitted {permitted}",
            self.action
        );
        (permitted, enforced_policy)
    }
}

#[cfg(test)]
mod tests {

    use super::{Action, NetworkContext, NetworkRbac, Permission, Policy, Principal};

    fn create_network_context<'a>(
        destination_ip: &str,
        destination_port: u16,
        downstream_ip: &str,
        downstream_port: u16,
        server_name: Option<&'a str>,
    ) -> NetworkContext<'a> {
        NetworkContext {
            destination: format!("{destination_ip}:{destination_port}").parse().unwrap(),
            downstream: format!("{downstream_ip}:{downstream_port}").parse().unwrap(),
            server_name,
        }
    }

    #[test]
    fn rule_test_allow_any() {
        let permission = Permission::Any;
        let principal = Principal::Any;
        let policy = Policy { permissions: vec![permission], principals: vec![principal] };
        let rbac_rule =
            NetworkRbac { action: Action::Allow, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) =
            rbac_rule.is_permitted(&create_network_context("127.0.0.1", 8000, "127.0.0.1", 9000, None));
        assert!(permitted);
        assert_eq!(rule, Some("my-id".into()));
    }
    #[test]
    fn rule_test_allow_dest_ip_permission_any_principal() {
        let permission = Permission::DestinationIp("127.0.0.0/24".parse().unwrap());
        let principal = Principal::Any;
        let policy = Policy { permissions: vec![permission], principals: vec![principal] };
        let rbac_rule =
            NetworkRbac { action: Action::Allow, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) =
            rbac_rule.is_permitted(&create_network_context("127.0.0.1", 8000, "127.0.0.1", 9000, None));
        assert!(permitted);
        assert_eq!(rule, Some("my-id".into()));
    }

    #[test]
    fn rule_test_allow_dest_ip_host_and_any_permission_any_principal() {
        let permission2 = Permission::DestinationIp("127.0.0.0/24".parse().unwrap());
        let permission1 = Permission::Any;
        let principal = Principal::Any;
        let policy = Policy { permissions: vec![permission1, permission2], principals: vec![principal] };
        let rbac_rule =
            NetworkRbac { action: Action::Allow, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) =
            rbac_rule.is_permitted(&create_network_context("127.0.0.1", 8000, "127.0.0.1", 9000, None));
        assert!(permitted);
        assert_eq!(rule, Some("my-id".into()));
    }

    #[test]
    fn rule_test_allow_dest_ip_and_any_permission_any_principal_negative() {
        let permission2 = Permission::DestinationIp("192.168.1.0/24".parse().unwrap());
        let permission1 = Permission::Any;
        let principal = Principal::Any;
        let policy = Policy { permissions: vec![permission1, permission2], principals: vec![principal] };
        let rbac_rule =
            NetworkRbac { action: Action::Allow, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) =
            rbac_rule.is_permitted(&create_network_context("127.0.0.1", 8000, "127.0.0.1", 9000, None));
        assert!(permitted);
        assert_eq!(rule, Some("my-id".into()));
    }

    #[test]
    fn rule_test_allow_dest_ip_permission_src_ip_principal() {
        let permission = Permission::DestinationIp("127.0.0.0/24".parse().unwrap());
        let principal = Principal::DownstreamRemoteIp("127.0.0.0/24".parse().unwrap());
        let policy = Policy { permissions: vec![permission], principals: vec![principal] };
        let rbac_rule =
            NetworkRbac { action: Action::Allow, policies: vec![("my-id".into(), policy)].into_iter().collect() };
        let (permitted, rule) =
            rbac_rule.is_permitted(&create_network_context("127.0.0.1", 8000, "127.0.0.1", 9000, None));
        assert!(permitted);
        assert_eq!(rule, Some("my-id".into()));
    }
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{Action, NetworkRbac, Permission, Policy, Principal};
    use crate::config::{common::*, core::CidrRange, util::u32_to_u16};
    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        config::rbac::v3::{
            permission::Rule as EnvoyPermissionRule, principal::Identifier as EnvoyPrincipalIdentifier,
            rbac::Action as EnvoyAction, Permission as EnvoyPermission, Policy as EnvoyPolicy,
            Principal as EnvoyPrincipal, Rbac as EnvoyRbac,
        },
        extensions::filters::network::rbac::v3::Rbac as EnvoyNetworkRbac,
        r#type::v3::Int32Range,
    };

    impl TryFrom<EnvoyRbac> for NetworkRbac {
        type Error = GenericError;

        fn try_from(envoy: EnvoyRbac) -> Result<Self, Self::Error> {
            let EnvoyRbac { action, policies, audit_logging_options } = envoy;
            unsupported_field!(audit_logging_options)?;
            let action = Action::try_from(action).with_node("action")?;
            let policies = required!(policies)?
                .into_iter()
                .map(|(id, pol)| Policy::try_from(pol).map(|value| (id.into(), value)))
                .collect::<Result<_, _>>()?;
            Ok(NetworkRbac { action, policies })
        }
    }
    impl TryFrom<EnvoyNetworkRbac> for NetworkRbac {
        type Error = GenericError;
        fn try_from(envoy: EnvoyNetworkRbac) -> Result<Self, Self::Error> {
            let EnvoyNetworkRbac {
                rules,
                matcher,
                shadow_rules,
                shadow_matcher,
                shadow_rules_stat_prefix,
                stat_prefix,
                enforcement_type,
                delay_deny,
            } = envoy;
            unsupported_field!(
                // rules,
                matcher,
                shadow_rules,
                shadow_matcher,
                shadow_rules_stat_prefix,
                // stat_prefix,
                enforcement_type,
                delay_deny
            )?;
            if stat_prefix.is_used() {
                tracing::warn!(
                    "unsupported field stat_prefix used in network rbac filter. This field will be ignored."
                );
            }
            convert_opt!(rules)
        }
    }

    impl TryFrom<i32> for Action {
        type Error = GenericError;
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            EnvoyAction::try_from(value)
                .map_err(|_| GenericError::unsupported_variant(format!("[unknown action {value}]")))?
                .try_into()
        }
    }

    impl TryFrom<EnvoyAction> for Action {
        type Error = GenericError;
        fn try_from(value: EnvoyAction) -> Result<Self, Self::Error> {
            Ok(match value {
                EnvoyAction::Allow => Self::Allow,
                EnvoyAction::Deny => Self::Deny,
                EnvoyAction::Log => return Err(GenericError::unsupported_variant("Log")),
            })
        }
    }
    impl TryFrom<EnvoyPolicy> for Policy {
        type Error = GenericError;
        fn try_from(envoy: EnvoyPolicy) -> Result<Self, Self::Error> {
            let EnvoyPolicy { permissions, principals, condition, checked_condition } = envoy;
            unsupported_field!(condition, checked_condition)?;
            let permissions = convert_non_empty_vec!(permissions)?;
            let principals = convert_non_empty_vec!(principals)?;
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
                EnvoyPermissionRule::DestinationIp(destination_ip) => {
                    CidrRange::try_from(destination_ip).map(CidrRange::into_ipnet).map(Self::DestinationIp)
                },
                EnvoyPermissionRule::DestinationPort(port) => u32_to_u16(port).map(Self::DestinationPort),
                EnvoyPermissionRule::DestinationPortRange(Int32Range { start, end }) => {
                    match (start.try_into(), end.try_into()) {
                        (Ok(start), Ok(end)) if start < end => Ok(Self::DestinationPortRange(start..end)),
                        (Ok(_), Ok(_)) => {
                            Err(GenericError::from_msg(format!("lower port range {start} is >= upper range {end}")))
                        },
                        (Err(_), _) | (_, Err(_)) => {
                            Err(GenericError::from_msg(format!("Invalid port range {start}..{end}")))
                        },
                    }
                },
                EnvoyPermissionRule::RequestedServerName(matcher) => matcher.try_into().map(Self::ServerName),
                _ => Err(GenericError::unsupported_variant("[Unsupported Permission Rule]")),
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
                EnvoyPrincipalIdentifier::DirectRemoteIp(cidr) => {
                    CidrRange::try_from(cidr).map(CidrRange::into_ipnet).map(Self::DownstreamRemoteIp)
                },
                _ => return Err(GenericError::unsupported_variant("[Unsupported Principal Identifier]")),
            }
            .with_node("identifier")
        }
    }
}
