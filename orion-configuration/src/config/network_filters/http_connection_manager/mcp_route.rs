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

//todo: impl serialize, deserialize on DirectResponsebody to prepare the bytes at deserialization

use std::{collections::HashMap, time::Duration};

use serde::{Deserialize, Serialize};

use crate::config::cluster::ClusterSpecifier;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct McpRouteAction {
    pub cluster_mappings: HashMap<String, ClusterSpecifier>,

    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "is_default_timeout", default = "default_timeout_deser")]
    pub timeout: Option<Duration>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub rewrite: Option<super::route::PathRewriteSpecifier>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub authority_rewrite: Option<super::route::AuthorityRewriteSpecifier>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub retry_policy: Option<super::RetryPolicy>,
}

#[allow(clippy::unnecessary_wraps)]
const fn default_timeout_deser() -> Option<Duration> {
    Some(DEFAULT_TIMEOUT)
}
#[allow(clippy::ref_option)]
fn is_default_timeout(timeout: &Option<Duration>) -> bool {
    *timeout == default_timeout_deser()
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::super::{route::AuthorityRewriteSpecifier, route::PathRewriteSpecifier};
    use crate::config::{
        cluster::ClusterSpecifier,
        common::*,
        network_filters::http_connection_manager::{RetryPolicy, mcp_route::McpRouteAction},
        util::duration_from_envoy,
    };
    use http::{
        HeaderName,
        uri::{Authority, PathAndQuery},
    };
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::route::v3::{
        McpRouteAction as EnvoyMcpRouteAction, mcp_route_action::HostRewriteSpecifier as EnvoyHostRewriteSpecifier,
        mcp_route_action::PathRewriteSpecifier as EnvoyPathRewriteSpecifier,
    };

    use std::{collections::HashMap, str::FromStr};

    impl TryFrom<EnvoyMcpRouteAction> for McpRouteAction {
        type Error = GenericError;
        fn try_from(value: EnvoyMcpRouteAction) -> Result<Self, Self::Error> {
            let EnvoyMcpRouteAction {
                cluster_mappings,
                retry_policy,
                timeout,
                path_rewrite_specifier,
                host_rewrite_specifier,
            } = value;

            let timeout =
                timeout.map(duration_from_envoy).unwrap_or(Ok(super::DEFAULT_TIMEOUT)).with_node("timeout")?;
            let timeout = if timeout.is_zero() { None } else { Some(timeout) };

            let retry_policy = retry_policy.map(RetryPolicy::try_from).transpose().with_node("retry_policy")?;

            let cluster_mappings = cluster_mappings
                .into_iter()
                .map(|(k, v)| (k, ClusterSpecifier::Cluster(v.into())))
                .collect::<HashMap<_, _>>();

            let path_rewrite_specifier = path_rewrite_specifier
                .map(|path_rewrite_specifier| match path_rewrite_specifier {
                    EnvoyPathRewriteSpecifier::PathRedirect(pr) => PathAndQuery::from_str(&pr)
                        .map_err(|e| {
                            GenericError::from_msg_with_cause(format!("failed to parse {pr} as a path and query"), e)
                        })
                        .map(PathRewriteSpecifier::Path),
                    EnvoyPathRewriteSpecifier::PrefixRewrite(prefix) => Ok(PathRewriteSpecifier::Prefix(prefix.into())),
                    EnvoyPathRewriteSpecifier::RegexRewrite(regex) => regex.try_into().map(PathRewriteSpecifier::Regex),
                })
                .transpose()
                .with_node("path_rewrite_specifier")?;

            let authority_rewrite = match host_rewrite_specifier {
                Some(EnvoyHostRewriteSpecifier::AutoHostRewrite(bv)) if bv.value => {
                    Ok(Some(AuthorityRewriteSpecifier::AutoHostRewrite))
                },
                Some(EnvoyHostRewriteSpecifier::AutoHostRewrite(bv)) => Ok(None),
                Some(spec) => match spec {
                    EnvoyHostRewriteSpecifier::HostRewriteLiteral(literal) => {
                        Authority::from_str(&literal).map(AuthorityRewriteSpecifier::Authority).map_err(|e| {
                            GenericError::from_msg_with_cause(
                                format!("failed to parse host rewrite literal '{literal}' as authority"),
                                e,
                            )
                        })
                    },
                    EnvoyHostRewriteSpecifier::HostRewriteHeader(header) => match HeaderName::from_str(&header) {
                        Ok(_) => Ok(AuthorityRewriteSpecifier::Header(header.into())),
                        Err(e) => Err(GenericError::from_msg_with_cause(
                            format!("failed to parse host rewrite header '{header}' as header name"),
                            e,
                        )),
                    },
                    EnvoyHostRewriteSpecifier::HostRewritePathRegex(regex) => {
                        regex.try_into().map(AuthorityRewriteSpecifier::Regex)
                    },
                    EnvoyHostRewriteSpecifier::AutoHostRewrite(_) => unreachable!(),
                }
                .map(Some),
                None => Ok(None),
            }
            .with_node("host_rewrite_specifier")?;
            Ok(Self { timeout, cluster_mappings, rewrite: path_rewrite_specifier, authority_rewrite, retry_policy })
        }
    }
}
