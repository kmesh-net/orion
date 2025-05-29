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

use super::{header_matcher::HeaderMatcher, RetryPolicy, UpgradeType};
use crate::config::{
    cluster::ClusterSpecifier,
    common::*,
    core::{CaseSensitive, DataSource},
};
use bytes::Bytes;
use compact_str::CompactString;
use http::{
    uri::{Authority, InvalidUri, PathAndQuery, Scheme},
    HeaderName, Request, StatusCode,
};
use regex::Regex;
use serde::{de::Error, Deserialize, Serialize};
use std::{
    borrow::Cow,
    hash::{Hash, Hasher},
    net::SocketAddr,
    num::NonZeroU16,
    ops::Range,
    str::FromStr,
    time::Duration,
};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Route(RouteAction),
    DirectResponse(DirectResponseAction),
    Redirect(RedirectAction),
    Upgrade(UpgradeAction),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityRedirect {
    /// only redirect the port, leave the host as-is
    PortRedirect(NonZeroU16),
    /// only redirect the host, leave the port as-is
    HostRedirect(#[serde(with = "http_serde_ext::authority")] Authority),
    /// Redirect the whole authority (host:port)
    AuthorityRedirect(#[serde(with = "http_serde_ext::authority")] Authority),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename = "regex")]
pub struct RegexMatchAndSubstitute {
    #[serde(with = "serde_regex")]
    pub pattern: Regex,
    pub substitution: CompactString,
}

impl PartialEq for RegexMatchAndSubstitute {
    fn eq(&self, other: &Self) -> bool {
        self.substitution == other.substitution && self.pattern.as_str() == other.pattern.as_str()
    }
}

impl Eq for RegexMatchAndSubstitute {}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PathRewriteSpecifier {
    Path(#[serde(with = "http_serde_ext::path_and_query")] PathAndQuery),
    Prefix(CompactString),
    Regex(RegexMatchAndSubstitute),
}

impl PathRewriteSpecifier {
    /// will preserve the query part of the input if the replacement does not contain one
    #[allow(clippy::redundant_else)]
    pub fn apply(
        &self,
        path_and_query: Option<&PathAndQuery>,
        route_match_result: &RouteMatchResult,
    ) -> Result<Option<PathAndQuery>, InvalidUri> {
        let old_path = path_and_query.map(PathAndQuery::path).unwrap_or_default();
        let old_query = path_and_query.map(PathAndQuery::query).unwrap_or_default();
        let new_path = match self {
            //full overwrite, doesn't care what original was
            PathRewriteSpecifier::Path(p) => p.path().into(),
            // apply a regex tot the original
            PathRewriteSpecifier::Regex(regex) => {
                //we need to run the regex even if the original is empty because it could match against '^$' (^ = start-of-string, $ = end-of-string)
                let replacement = regex.pattern.replace_all(old_path, regex.substitution.as_str());
                if let Cow::Borrowed(_) = replacement {
                    return Ok(None);
                } else {
                    replacement
                }
            },
            PathRewriteSpecifier::Prefix(prefix) => {
                if let Some(matched_range) = route_match_result.matched_range() {
                    let orig_without_prefix = &old_path[matched_range.end..];
                    format!("{prefix}{orig_without_prefix}").into()
                } else {
                    return Ok(None);
                }
            },
        };
        if let Some(old_query) = old_query {
            if !new_path.contains('?') {
                return Some(PathAndQuery::from_str(&format!("{new_path}?{old_query}"))).transpose();
            }
        }
        Some(PathAndQuery::from_str(&new_path)).transpose()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RedirectAction {
    pub response_code: RedirectResponseCode,
    pub strip_query: bool,
    #[serde(flatten)]
    pub authority_redirect: Option<AuthorityRedirect>,
    #[serde(with = "http_serde_ext::scheme::option")]
    pub scheme_rewrite_specifier: Option<Scheme>,
    #[serde(flatten)]
    pub path_rewrite_specifier: Option<PathRewriteSpecifier>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RedirectResponseCode {
    Found,
    MovedPermanently,
    PermanentRedirect,
    SeeOther, //🌊🦦
    TemporaryRedirect,
}

impl From<RedirectResponseCode> for StatusCode {
    fn from(value: RedirectResponseCode) -> Self {
        match value {
            RedirectResponseCode::Found => StatusCode::FOUND,
            RedirectResponseCode::MovedPermanently => StatusCode::MOVED_PERMANENTLY,
            RedirectResponseCode::PermanentRedirect => StatusCode::PERMANENT_REDIRECT,
            RedirectResponseCode::SeeOther => StatusCode::SEE_OTHER,
            RedirectResponseCode::TemporaryRedirect => StatusCode::TEMPORARY_REDIRECT,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DirectResponseAction {
    #[serde(with = "http_serde_ext::status_code")]
    pub status: StatusCode,
    #[serde(flatten)]
    pub body: Option<DirectResponseBody>,
}

impl DirectResponseAction {
    pub fn body(&self) -> &[u8] {
        self.body.as_ref().map(DirectResponseBody::data).unwrap_or_default()
    }
}

//hayley:
// we immidiatly load the value of the DataSource and cache it here, allowing us to
//  use this type directly in orion-lib without having to worry about reading or caching the result in
//  the datapath. A file being loaded once at startup and not reloaded when the file changes matches Envoy
//  perhaps we want to reconsider this behaviour in the future.
// we also might want to use this representation for all DataSource, for consistency?
#[derive(Debug, Clone, Serialize)]
pub struct DirectResponseBody {
    #[serde(flatten)]
    source: DataSource,
    #[serde(skip, default)]
    pub data: Bytes,
}

impl<'de> Deserialize<'de> for DirectResponseBody {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        pub struct Inner {
            #[serde(flatten)]
            source: DataSource,
        }
        let source = Inner::deserialize(deserializer)?.source;
        let data =
            source.to_bytes_blocking().map_err(|e| D::Error::custom(format!("failed to read datasource: {e}")))?.into();
        Ok(Self { source, data })
    }
}

impl DirectResponseBody {
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

impl PartialEq for DirectResponseBody {
    fn eq(&self, other: &Self) -> bool {
        self.source.eq(&other.source)
    }
}
impl Eq for DirectResponseBody {}

//todo: impl serialize, deserialize on DirectResponsebody to prepare the bytes at deserialization

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum UpgradeActionType {
    Websocket,
    Connect { allow_post: bool /* , proxy_protocol_config: Option<...> */ },
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct UpgradeAction {
    pub cluster_specifier: ClusterSpecifier,
    #[serde(
        with = "http_serde_ext::status_code",
        skip_serializing_if = "is_default_statuscode",
        default = "default_statuscode_deser"
    )]
    pub cluster_not_found_response_code: StatusCode,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub rewrite: Option<PathRewriteSpecifier>,
    pub upgrade_type: UpgradeActionType,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RouteAction {
    pub cluster_specifier: ClusterSpecifier,
    #[serde(
        with = "http_serde_ext::status_code",
        skip_serializing_if = "is_default_statuscode",
        default = "default_statuscode_deser"
    )]
    pub cluster_not_found_response_code: StatusCode,
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "is_default_timeout", default = "default_timeout_deser")]
    pub timeout: Option<Duration>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub rewrite: Option<PathRewriteSpecifier>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub retry_policy: Option<RetryPolicy>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub upgrade_config: Option<UpgradeConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub hash_policy: Vec<HashPolicy>,
}

const DEFAULT_CLUSTER_NOT_FOUND_STATUSCODE: StatusCode = StatusCode::SERVICE_UNAVAILABLE;
const fn default_statuscode_deser() -> StatusCode {
    DEFAULT_CLUSTER_NOT_FOUND_STATUSCODE
}
fn is_default_statuscode(code: &StatusCode) -> bool {
    *code == DEFAULT_CLUSTER_NOT_FOUND_STATUSCODE
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
#[allow(clippy::unnecessary_wraps)]
const fn default_timeout_deser() -> Option<Duration> {
    Some(DEFAULT_TIMEOUT)
}
#[allow(clippy::ref_option)]
fn is_default_timeout(timeout: &Option<Duration>) -> bool {
    *timeout == default_timeout_deser()
}

#[derive(Clone, Debug, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum Websocket {
    Enabled,
    Disabled,
}

#[derive(Clone, Debug, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum Connect {
    Enabled { allow_post: bool /* , proxy_protocol_config: Option<...> */ },
    Disabled,
}

#[derive(Clone, Debug, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct UpgradeConfig {
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub websocket: Option<Websocket>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub connect: Option<Connect>,
}

impl UpgradeConfig {
    pub fn is_websocket_enabled(&self, inherited: &[UpgradeType]) -> bool {
        let enabled_via_hcm = inherited.iter().any(|upgrade| matches!(upgrade, UpgradeType::Websocket));
        let locally_enabled = matches!(self.websocket, Some(Websocket::Enabled));
        let locally_disabled = matches!(self.websocket, Some(Websocket::Disabled));
        locally_enabled || (enabled_via_hcm && !locally_disabled)
    }
    pub fn is_connect_enabled(&self, inherited: &[UpgradeType]) -> bool {
        let enabled_via_hcm = inherited.iter().any(|upgrade| matches!(upgrade, UpgradeType::Connect));
        let locally_enabled = matches!(self.connect, Some(Connect::Enabled { allow_post: _ }));
        let locally_disabled = matches!(self.connect, Some(Connect::Disabled));
        locally_enabled || (enabled_via_hcm && !locally_disabled)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HashPolicy {
    pub policy_specifier: PolicySpecifier,
    #[serde(skip_serializing_if = "std::ops::Not::not", default = "Default::default")]
    pub terminal: bool,
}

#[derive(Clone, Debug, Copy)]
pub enum HashPolicyResult {
    Applied,
    Skipped,
    Terminal,
}

impl HashPolicy {
    pub fn apply<B>(&self, hasher: &mut impl Hasher, req: &Request<B>, src_addr: SocketAddr) -> HashPolicyResult {
        let applied = match &self.policy_specifier {
            PolicySpecifier::SourceIp(true) => {
                src_addr.hash(hasher);
                true
            },
            PolicySpecifier::SourceIp(false) => false,
            PolicySpecifier::Header(name) => req.headers().get(name).inspect(|value| value.hash(hasher)).is_some(),
            PolicySpecifier::QueryParameter(name) => req
                .uri()
                .query()
                .and_then(|query| {
                    // Hash the value of the first query key that matches (case-sensitive)
                    //note(hayley): we might slightly improve performance here by urlencoding the name parameter
                    // instead of decoding the query
                    url::form_urlencoded::parse(query.as_bytes()).find(|(key, _value)| key == name)
                })
                .inspect(|(_key, value)| value.hash(hasher))
                .is_some(),
        };

        match (applied, self.terminal) {
            (true, true) => HashPolicyResult::Terminal,
            (true, false) => HashPolicyResult::Applied,
            (false, _) => HashPolicyResult::Skipped,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicySpecifier {
    SourceIp(bool),
    Header(#[serde(with = "http_serde_ext::header_name")] HeaderName),
    QueryParameter(CompactString),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RouteMatch {
    // todo(hayley): can't be none?
    #[serde(flatten)]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub path_matcher: Option<PathMatcher>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub headers: Vec<HeaderMatcher>,
}

impl Default for RouteMatch {
    fn default() -> Self {
        Self {
            path_matcher: Some(PathMatcher { specifier: PathSpecifier::Prefix("".into()), ignore_case: false }),
            headers: Vec::new(),
        }
    }
}

pub struct RouteMatchResult {
    path_match: Option<PathMatcherResult>,
    headers_matched: bool,
}

impl RouteMatchResult {
    pub fn matched(&self) -> bool {
        self.headers_matched && self.path_match.as_ref().map(PathMatcherResult::matched).unwrap_or(true)
    }

    pub fn matched_range(&self) -> Option<Range<usize>> {
        if self.headers_matched {
            if let Some(pmr) = &self.path_match {
                return pmr.matched_range();
            }
        }
        None
    }
}

impl RouteMatch {
    pub fn match_request<B>(&self, request: &Request<B>) -> RouteMatchResult {
        let path_match = self.path_matcher.as_ref().map(|path_matcher| {
            //todo(hayley): how do we treat empty paths here?
            path_matcher.matches(request.uri().path_and_query().unwrap_or(&PathAndQuery::from_static("")))
        });
        //short circuit if path match fails
        let headers_matched = if path_match.is_some() {
            self.headers.iter().all(|matcher| matcher.request_matches(request))
        } else {
            false
        };
        RouteMatchResult { path_match, headers_matched }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PathMatcher {
    #[serde(flatten)]
    pub specifier: PathSpecifier,
    #[serde(skip_serializing_if = "std::ops::Not::not", default = "Default::default")]
    pub ignore_case: bool,
}

pub struct PathMatcherResult {
    inner: Option<usize>,
}

impl PathMatcherResult {
    pub fn matched(&self) -> bool {
        self.inner.is_some()
    }

    pub fn matched_range(&self) -> Option<Range<usize>> {
        self.inner.map(|up_to| 0..up_to)
    }
}

impl PathMatcher {
    pub fn matches(&self, path: &PathAndQuery) -> PathMatcherResult {
        let path = path.path();
        let case_matcher = CaseSensitive(!self.ignore_case, path);
        let inner = match &self.specifier {
            PathSpecifier::Exact(s) => case_matcher.equals(s).then_some(s.len()),
            PathSpecifier::Prefix(p) => case_matcher.starts_with(p).then_some(p.len()),
            PathSpecifier::PathSeparatedPrefix(psp) => {
                if case_matcher.equals(&psp[..psp.len() - 1]) {
                    Some(psp.len() - 1)
                } else if case_matcher.starts_with(psp) {
                    Some(psp.len())
                } else {
                    None
                }
            },
            PathSpecifier::Regex(r) => r.matches_full(path).then_some(path.len()),
        };
        PathMatcherResult { inner }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathSpecifier {
    Prefix(CompactString),
    Exact(CompactString),
    Regex(#[serde(with = "serde_regex")] Regex),
    PathSeparatedPrefix(CompactString),
}

impl PartialEq for PathSpecifier {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Regex(r1), Self::Regex(r2)) => r1.as_str().eq(r2.as_str()),
            (Self::Prefix(s1), Self::Prefix(s2))
            | (Self::Exact(s1), Self::Exact(s2))
            | (Self::PathSeparatedPrefix(s1), Self::PathSeparatedPrefix(s2)) => s1.eq(s2),
            _ => false,
        }
    }
}

impl Eq for PathSpecifier {}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_rewrite_uri_by_path_match_range() {
        let uri = PathAndQuery::from_str("/test/123").unwrap();
        let expected = Some(PathAndQuery::from_str("/hello/123").unwrap());
        let result = PathRewriteSpecifier::Prefix("/hello".into())
            .apply(
                Some(&uri),
                &RouteMatchResult { path_match: Some(PathMatcherResult { inner: Some(5) }), headers_matched: true },
            )
            .unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_rewrite_uri_by_regex() {
        let uri = PathAndQuery::from_str("/test/123").unwrap();
        let expected = Some(PathAndQuery::from_str("/hello/123").unwrap());
        let result = PathRewriteSpecifier::Regex(RegexMatchAndSubstitute {
            pattern: Regex::new("/test(.*)").unwrap(),
            substitution: "/hello$1".into(),
        })
        .apply(
            Some(&uri),
            &RouteMatchResult { path_match: Some(PathMatcherResult { inner: None }), headers_matched: true },
        )
        .unwrap();
        assert_eq!(result, expected);
    }
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{
        Action, AuthorityRedirect, Connect, DirectResponseAction, DirectResponseBody, HashPolicy, PathMatcher,
        PathRewriteSpecifier, PathSpecifier, PolicySpecifier, RedirectAction, RedirectResponseCode,
        RegexMatchAndSubstitute, RouteAction, RouteMatch, UpgradeConfig, Websocket, DEFAULT_TIMEOUT,
    };
    use crate::config::network_filters::http_connection_manager::RetryPolicy;
    use crate::config::{
        common::*,
        core::{regex_from_envoy, DataSource},
        util::{duration_from_envoy, http_status_from},
    };
    use http::{
        uri::{Authority, PathAndQuery, Scheme},
        HeaderName,
    };
    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        config::route::v3::{
            redirect_action::{
                PathRewriteSpecifier as EnvoyPathRewriteSpecifier, RedirectResponseCode as EnvoyRedirectResponseCode,
                SchemeRewriteSpecifier as EnvoySchemeRewriteSpecifier,
            },
            route::Action as EnvoyAction,
            route_action::{
                hash_policy::{
                    ConnectionProperties as EnvoyConnectionProperties, Header as EnvoyHeader,
                    PolicySpecifier as EnvoyPolicySpecifier, QueryParameter as EnvoyQueryParameter,
                },
                HashPolicy as EnvoyHashPolicy, UpgradeConfig as EnvoyUpgradeConfig,
            },
            route_match::PathSpecifier as EnvoyPathSpecifier,
            DirectResponseAction as EnvoyDirectResponseAction, RedirectAction as EnvoyRedirectAction,
            RouteAction as EnvoyRouteAction, RouteMatch as EnvoyRouteMatch,
        },
        r#type::matcher::v3::RegexMatchAndSubstitute as EnvoyRegexMatchAndSubstitute,
    };
    use orion_data_plane_api::envoy_data_plane_api::google::protobuf::BoolValue;
    use std::{num::NonZeroU16, str::FromStr};

    impl TryFrom<EnvoyHashPolicy> for HashPolicy {
        type Error = GenericError;
        fn try_from(value: EnvoyHashPolicy) -> Result<Self, Self::Error> {
            let EnvoyHashPolicy { terminal, policy_specifier } = value;
            let policy_specifier = convert_opt!(policy_specifier)?;
            Ok(Self { policy_specifier, terminal })
        }
    }
    impl TryFrom<EnvoyAction> for Action {
        type Error = GenericError;
        fn try_from(value: EnvoyAction) -> Result<Self, Self::Error> {
            Ok(match value {
                EnvoyAction::DirectResponse(dr) => Self::DirectResponse(dr.try_into()?),
                EnvoyAction::Redirect(rd) => Self::Redirect(rd.try_into()?),
                EnvoyAction::Route(r) => Self::Route(r.try_into()?),
                EnvoyAction::FilterAction(_) => return Err(GenericError::unsupported_variant("FilterAction")),
                EnvoyAction::NonForwardingAction(_) => {
                    return Err(GenericError::unsupported_variant("NonForwardingAction"))
                },
            })
        }
    }

    impl TryFrom<EnvoyRegexMatchAndSubstitute> for RegexMatchAndSubstitute {
        type Error = GenericError;
        fn try_from(value: EnvoyRegexMatchAndSubstitute) -> Result<Self, Self::Error> {
            let EnvoyRegexMatchAndSubstitute { pattern, substitution } = value;
            let pattern = regex_from_envoy(required!(pattern)?)?;
            let substitution = substitution.into();
            Ok(Self { pattern, substitution })
        }
    }

    impl From<EnvoyRedirectResponseCode> for RedirectResponseCode {
        fn from(value: EnvoyRedirectResponseCode) -> Self {
            match value {
                EnvoyRedirectResponseCode::Found => Self::Found,
                EnvoyRedirectResponseCode::MovedPermanently => Self::MovedPermanently,
                EnvoyRedirectResponseCode::PermanentRedirect => Self::PermanentRedirect,
                EnvoyRedirectResponseCode::SeeOther => Self::SeeOther,
                EnvoyRedirectResponseCode::TemporaryRedirect => Self::TemporaryRedirect,
            }
        }
    }

    impl TryFrom<EnvoyRedirectAction> for RedirectAction {
        type Error = GenericError;
        fn try_from(value: EnvoyRedirectAction) -> Result<Self, Self::Error> {
            let EnvoyRedirectAction {
                host_redirect,
                port_redirect,
                response_code,
                strip_query,
                scheme_rewrite_specifier,
                path_rewrite_specifier,
            } = value;

            let response_code = EnvoyRedirectResponseCode::from_i32(response_code)
                .ok_or_else(|| {
                    GenericError::from_msg(format!("[unknown response_code {response_code}]"))
                        .with_node("response_code")
                })?
                .into();

            let port_redirect = u16::try_from(port_redirect).map(NonZeroU16::new).map_err(|_| {
                GenericError::from_msg("{port_redirect} is not a valid port").with_node("port_redirect")
            })?;
            let host_redirect = host_redirect.is_used().then_some(host_redirect);
            let authority_redirect = match (host_redirect, port_redirect) {
                (None, None) => None,
                //can contain a port number in the host section too
                (Some(host), None) => {
                    Some(AuthorityRedirect::HostRedirect(Authority::from_str(&host).map_err(|e| {
                        GenericError::from_msg_with_cause(format!("failed to parse {host} as authority"), e)
                            .with_node("host_redirect")
                    })?))
                },
                (None, Some(port)) => Some(AuthorityRedirect::PortRedirect(port)),
                (Some(host), Some(port)) => {
                    let authority_string = format!("{host}:{port}");
                    match Authority::from_str(&authority_string) {
                        Ok(authority) => Some(AuthorityRedirect::AuthorityRedirect(authority)),
                        Err(e) => {
                            // Envoy lets you set both host_redirect and port redirect
                            // but if you specify both a host_redirect and a port redirect it just appends them together
                            // and creates an invalid authority.
                            return Err(GenericError::from_msg_with_cause(
                                format!("failed to parse {authority_string} as authority"),
                                e,
                            )
                            .with_node("host_redirect"));
                        },
                    }
                },
            };
            let scheme_rewrite_specifier = scheme_rewrite_specifier
                .and_then(|s| match s {
                    EnvoySchemeRewriteSpecifier::HttpsRedirect(true) => Some(Ok(Scheme::HTTPS)),
                    EnvoySchemeRewriteSpecifier::SchemeRedirect(s) if s.eq_ignore_ascii_case("https") => {
                        Some(Ok(Scheme::HTTPS))
                    },

                    EnvoySchemeRewriteSpecifier::SchemeRedirect(s) if s.eq_ignore_ascii_case("http") => {
                        Some(Ok(Scheme::HTTP))
                    },

                    EnvoySchemeRewriteSpecifier::SchemeRedirect(s) => {
                        Some(Scheme::from_str(&s.to_lowercase()).map_err(|e| {
                            GenericError::from_msg_with_cause(format!("failed to parse {s} as scheme"), e)
                        }))
                    },
                    // nothing happens?
                    EnvoySchemeRewriteSpecifier::HttpsRedirect(false) => None,
                })
                .transpose()
                .with_node("scheme_rewrite")?;

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
            Ok(Self {
                response_code,
                strip_query,
                authority_redirect,
                scheme_rewrite_specifier,
                path_rewrite_specifier,
            })
        }
    }
    impl TryFrom<EnvoyDirectResponseAction> for DirectResponseAction {
        type Error = GenericError;
        fn try_from(value: EnvoyDirectResponseAction) -> Result<Self, Self::Error> {
            let EnvoyDirectResponseAction { status, body } = value;
            let status = http_status_from(required!(status)?).with_node("status")?;
            let body = if let Some(source) = body.map(DataSource::try_from).transpose().with_node("body")? {
                let data = source
                    .to_bytes_blocking()
                    .map_err(|e| GenericError::from_msg_with_cause("failed to read datasource", e))
                    .with_node("body")?
                    .into();
                Some(DirectResponseBody { source, data })
            } else {
                None
            };
            Ok(Self { status, body })
        }
    }

    impl TryFrom<EnvoyRouteAction> for RouteAction {
        type Error = GenericError;
        fn try_from(value: EnvoyRouteAction) -> Result<Self, Self::Error> {
            let EnvoyRouteAction {
                cluster_not_found_response_code,
                metadata_match,
                prefix_rewrite,
                regex_rewrite,
                path_rewrite_policy,
                append_x_forwarded_host,
                timeout,
                idle_timeout,
                early_data_policy,
                retry_policy,
                retry_policy_typed_config,
                request_mirror_policies,
                priority,
                rate_limits,
                include_vh_rate_limits,
                hash_policy,
                cors,
                max_grpc_timeout,
                grpc_timeout_offset,
                upgrade_configs,
                internal_redirect_policy,
                internal_redirect_action,
                max_internal_redirects,
                hedge_policy,
                max_stream_duration,
                cluster_specifier,
                host_rewrite_specifier,
            } = value;
            unsupported_field!(
                // cluster_not_found_response_code,
                metadata_match,
                // prefix_rewrite,
                // regex_rewrite,
                path_rewrite_policy,
                append_x_forwarded_host,
                // timeout,
                idle_timeout,
                early_data_policy,
                // retry_policy,
                retry_policy_typed_config,
                request_mirror_policies,
                priority,
                rate_limits,
                include_vh_rate_limits,
                // hash_policy,
                cors,
                max_grpc_timeout,
                grpc_timeout_offset,
                // upgrade_configs,
                internal_redirect_policy,
                internal_redirect_action,
                max_internal_redirects,
                hedge_policy,
                max_stream_duration,
                // cluster_specifier,
                host_rewrite_specifier
            )?;
            let cluster_not_found_response_code = if cluster_not_found_response_code.is_used() {
                http_status_from(cluster_not_found_response_code)
            } else {
                Ok(super::DEFAULT_CLUSTER_NOT_FOUND_STATUSCODE)
            }
            .with_node("cluster_not_found_response_code")?;
            let timeout = timeout.map(duration_from_envoy).unwrap_or(Ok(DEFAULT_TIMEOUT)).with_node("timeout")?;
            // in envoy, the default value for the timeout (if not set) is 15s, but setting the timeout disables it.
            // in order to better match the rest of the code/rust, we map disabled to None and the default to Some(15s)
            let timeout = if timeout.is_zero() { None } else { Some(timeout) };
            let cluster_specifier = convert_opt!(cluster_specifier)?;
            let rewrite = match (prefix_rewrite.is_used().then_some(prefix_rewrite), regex_rewrite) {
                (None, None) => None,
                (Some(s), None) => Some(PathRewriteSpecifier::Prefix(s.into())),
                (None, Some(regex)) => {
                    Some(regex.try_into().map(PathRewriteSpecifier::Regex).with_node("regex_rewrite")?)
                },
                (Some(_), Some(_)) => {
                    return Err(GenericError::from_msg(
                        "only one of `prefix_rewrite` and `regex_rewrite` may be specified",
                    ))
                },
            };
            let retry_policy = retry_policy.map(RetryPolicy::try_from).transpose().with_node("retry_policy")?;
            let upgrade_config = upgrade_configs.try_into().with_node("upgrade_configs").ok();
            let hash_policy = convert_vec!(hash_policy)?;
            Ok(Self {
                cluster_not_found_response_code,
                timeout,
                cluster_specifier,
                rewrite,
                retry_policy,
                upgrade_config,
                hash_policy,
            })
        }
    }

    impl TryFrom<Vec<EnvoyUpgradeConfig>> for UpgradeConfig {
        type Error = GenericError;
        fn try_from(value: Vec<EnvoyUpgradeConfig>) -> Result<Self, Self::Error> {
            let mut upgrade_config = UpgradeConfig::default();
            for envoy_upgrade_config in value {
                match (envoy_upgrade_config.upgrade_type.to_lowercase().as_str(), envoy_upgrade_config.enabled) {
                    ("websocket", Some(BoolValue { value: true })) => {
                        upgrade_config.websocket = Some(Websocket::Enabled)
                    },
                    ("websocket", Some(BoolValue { value: false })) => {
                        upgrade_config.websocket = Some(Websocket::Disabled)
                    },
                    ("connect", Some(BoolValue { value: true })) => {
                        return Err(GenericError::from_msg("Http CONNECT upgrades are not currently supported"))
                    },
                    ("connect", Some(BoolValue { value: false })) => upgrade_config.connect = Some(Connect::Disabled),
                    (unknown, _) => {
                        return Err(GenericError::from_msg(format!("Unsupported upgrade type [{unknown}]")))
                    },
                }
            }
            Ok(upgrade_config)
        }
    }

    impl TryFrom<EnvoyPolicySpecifier> for PolicySpecifier {
        type Error = GenericError;
        fn try_from(value: EnvoyPolicySpecifier) -> Result<Self, Self::Error> {
            Ok(match value {
                EnvoyPolicySpecifier::ConnectionProperties(EnvoyConnectionProperties { source_ip }) => {
                    Self::SourceIp(source_ip)
                },
                EnvoyPolicySpecifier::Header(EnvoyHeader { header_name, regex_rewrite }) => {
                    unsupported_field!(regex_rewrite)?;
                    PolicySpecifier::Header(HeaderName::from_str(&header_name).map_err(|e| {
                        GenericError::from_msg_with_cause(
                            format!("Couldn't convert \"{header_name}\" to a header name"),
                            e,
                        )
                    })?)
                },
                EnvoyPolicySpecifier::QueryParameter(EnvoyQueryParameter { name }) => {
                    Self::QueryParameter(required!(name)?.into())
                },
                EnvoyPolicySpecifier::Cookie(_) => return Err(GenericError::unsupported_variant("Cookie")),
                EnvoyPolicySpecifier::FilterState(_) => return Err(GenericError::unsupported_variant("FilterState")),
            })
        }
    }

    impl TryFrom<EnvoyRouteMatch> for RouteMatch {
        type Error = GenericError;
        fn try_from(value: EnvoyRouteMatch) -> Result<Self, Self::Error> {
            let EnvoyRouteMatch {
                case_sensitive,
                runtime_fraction,
                headers,
                query_parameters,
                grpc,
                tls_context,
                dynamic_metadata,
                path_specifier,
                filter_state,
            } = value;
            unsupported_field!(
                // case_sensitive,
                runtime_fraction,
                // headers,
                query_parameters,
                grpc,
                tls_context,
                dynamic_metadata, // path_specifier,
                filter_state
            )?;
            let ignore_case = !case_sensitive.map(|v| v.value).unwrap_or(true);
            let path_specifier = path_specifier.map(PathSpecifier::try_from).transpose().with_node("path_specifier")?;
            let headers = convert_vec!(headers)?;
            let path_matcher = path_specifier.map(|specifier| PathMatcher { specifier, ignore_case });
            Ok(Self { path_matcher, headers })
        }
    }

    impl TryFrom<EnvoyPathSpecifier> for PathSpecifier {
        type Error = GenericError;
        fn try_from(value: EnvoyPathSpecifier) -> Result<Self, Self::Error> {
            match value {
                EnvoyPathSpecifier::Prefix(s) => Ok(Self::Prefix(s.into())),
                EnvoyPathSpecifier::Path(s) => Ok(Self::Exact(s.into())),
                EnvoyPathSpecifier::SafeRegex(r) => regex_from_envoy(r).map(Self::Regex),
                EnvoyPathSpecifier::PathSeparatedPrefix(mut s) => {
                    if s.ends_with('/') || s.contains(['?', '/']) {
                        Err(GenericError::from_msg(format!(
                            "PathSeperatedPrefix \"{s}\" contains invalid characters ('?' or '/') or ends with '/'"
                        )))
                    } else {
                        s.push('/');
                        Ok(Self::PathSeparatedPrefix(s.into()))
                    }
                },
                EnvoyPathSpecifier::ConnectMatcher(_) => Err(GenericError::unsupported_variant("ConnectMatcher")),
                EnvoyPathSpecifier::PathMatchPolicy(_) => Err(GenericError::unsupported_variant("PathMatchPolicy")),
            }
        }
    }
}
