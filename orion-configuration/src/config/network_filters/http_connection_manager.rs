// SPDX-FileCopyrightText: © 2025 Huawei Cloud Computing Technologies Co., Ltd
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 Huawei Cloud Computing Technologies Co., Ltd
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

pub mod header_modifer;
use header_modifer::{HeaderModifier, HeaderValueOption};
pub mod header_matcher;
use header_matcher::HeaderMatcher;
pub mod route;
use route::{Action, RouteMatch};
pub mod http_filters;
use http_filters::{FilterOverride, HttpFilter};

use crate::config::common::*;
use compact_str::CompactString;
use exponential_backoff::Backoff;
use http::{HeaderName, HeaderValue, StatusCode};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, str::FromStr, time::Duration};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HttpConnectionManager {
    pub codec_type: CodecType,
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request_timeout: Option<Duration>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub http_filters: Vec<HttpFilter>,
    pub route_specifier: RouteSpecifier,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum CodecType {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "HTTP1")]
    Http1,
    #[serde(rename = "HTTP2")]
    Http2,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum RouteSpecifier {
    Rds(RdsSpecifier),
    RouteConfig(RouteConfiguration),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RouteConfiguration {
    pub name: CompactString,
    #[serde(skip_serializing_if = "std::ops::Not::not", default = "Default::default")]
    pub most_specific_header_mutations_wins: bool,
    #[serde(skip_serializing_if = "is_default", default)]
    pub response_header_modifier: HeaderModifier,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub request_headers_to_add: Vec<HeaderValueOption>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    #[serde(with = "http_serde_ext::header_name::vec")]
    pub request_headers_to_remove: Vec<HeaderName>,
    pub virtual_hosts: Vec<VirtualHost>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchHost {
    Wildcard,
    Prefix(CompactString),
    Suffix(CompactString),
    Exact(CompactString),
}

impl Serialize for MatchHost {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Wildcard => serializer.serialize_str("*"),
            Self::Exact(cs) => serializer.serialize_str(cs.as_str()),
            Self::Prefix(cs) => serializer.serialize_str(&format!("{cs}*")),
            Self::Suffix(cs) => serializer.serialize_str(&format!("*{cs}")),
        }
    }
}

impl<'de> Deserialize<'de> for MatchHost {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let cs = CompactString::deserialize(deserializer)?;
        Self::try_from_compact_str(cs).map_err(|e| serde::de::Error::custom(format!("{e}")))
    }
}

impl MatchHost {
    pub fn try_from_compact_str(value: CompactString) -> Result<Self, GenericError> {
        let _ = HeaderValue::from_str(&value)
            .map_err(|_| GenericError::from_msg(format!("failed to parse \"{value}\" as a headervalue")))?;

        if value == "*" {
            return Ok(Self::Wildcard);
        }

        if value.chars().filter(|c| *c == '*').count() > 1 {
            return Err(GenericError::from_msg("only one wildcard supported at the beginning or at the end"));
        }

        if let Some(host) = value.strip_prefix('*') {
            return Ok(Self::Suffix(host.into()));
        }

        if let Some(host) = value.strip_suffix('*') {
            return Ok(Self::Prefix(host.into()));
        }

        if value.contains('*') {
            return Err(GenericError::from_msg("only one wildcard supported at the beginning or at the end"));
        }

        Ok(Self::Exact(value))
    }
}

impl TryFrom<String> for MatchHost {
    type Error = GenericError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from_compact_str(value.into())
    }
}

impl TryFrom<&str> for MatchHost {
    type Error = GenericError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from_compact_str(value.into())
    }
}

impl TryFrom<CompactString> for MatchHost {
    type Error = GenericError;
    fn try_from(value: CompactString) -> Result<Self, Self::Error> {
        Self::try_from_compact_str(value)
    }
}

impl FromStr for MatchHost {
    type Err = GenericError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

#[repr(u32)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
/// Score given to the request host matching to a given rule defined in the config
///
/// Exact match always get prioritizied over Suffix matches. The integer content
/// represents how many character match for the request string in the uri.authority.
/// Hosts/domain matching order is derived implicitly from the enum lexicographic order
/// enum values are manually overwritten to avoid unwanted reordering
pub enum MatchHostScoreLPM {
    Wildcard = 0,
    Prefix(usize) = 1,
    Suffix(usize) = 2,
    Exact(usize) = 3,
}

impl MatchHost {
    pub fn eval_lpm_request<B>(&self, req: &http::Request<B>) -> Option<MatchHostScoreLPM> {
        if let Some(header_value) = req.headers().get(http::header::HOST) {
            let host = header_value.to_str().ok()?;
            self.eval_lpm_host(host)
        } else {
            self.eval_lpm_host(req.uri().host()?)
        }
    }

    pub fn eval_lpm_host(&self, mut host: &str) -> Option<MatchHostScoreLPM> {
        match self {
            Self::Exact(h) => {
                host = host.strip_suffix('.').unwrap_or(host);
                (h == host).then_some(MatchHostScoreLPM::Exact(h.len()))
            },

            // Wildcard in Suffix and Prefix will not match empty strings, see
            // https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/route/v3/route_components.proto#config-route-v3-virtualhost
            Self::Suffix(suffix) => {
                host = host.strip_suffix('.').unwrap_or(host);
                (host.len() > suffix.len() && host.ends_with(suffix.as_str()))
                    .then_some(MatchHostScoreLPM::Suffix(suffix.len()))
            },

            Self::Prefix(prefix) => (host.len() > prefix.len() && host.starts_with(prefix.as_str()))
                .then_some(MatchHostScoreLPM::Prefix(prefix.len())),

            Self::Wildcard => Some(MatchHostScoreLPM::Wildcard),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct VirtualHost {
    pub name: CompactString,
    pub domains: Vec<MatchHost>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub routes: Vec<Route>,
    #[serde(skip_serializing_if = "is_default", default)]
    pub response_header_modifier: HeaderModifier,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub request_headers_to_add: Vec<HeaderValueOption>,
    #[serde(with = "http_serde_ext::header_name::vec")]
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub request_headers_to_remove: Vec<HeaderName>,
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub retry_policy: Option<RetryPolicy>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetryPolicy {
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub retry_on: Vec<RetryOn>,
    pub num_retries: u32,
    #[serde(with = "humantime_serde")]
    #[serde(skip_serializing_if = "Option::is_none", default = "Default::default")]
    pub per_try_timeout: Option<Duration>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    #[serde(with = "http_serde_ext::status_code::vec")]
    pub retriable_status_codes: Vec<StatusCode>,
    //envoy uses back_off but that's the verb
    // the noun is backoff.
    pub retry_backoff: RetryBackoff,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub retriable_headers: Vec<HeaderMatcher>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub retriable_request_headers: Vec<HeaderMatcher>,
}

impl RetryPolicy {
    #[inline]
    pub fn is_retriable<B>(&self, req: &http::Request<B>) -> bool {
        // todo(haylyey):
        //  the docs say this field contains
        // > HTTP headers which must be present in the request for retries to be attempted.
        // so is the behaviour to ignore this when its empty or must the headers be present for it to retry?
        self.retriable_request_headers.is_empty()
            || self.retriable_request_headers.iter().any(|hm| hm.request_matches(req))
    }

    #[inline]
    pub fn exponential_back_off(&self) -> Backoff {
        Backoff::new(self.num_retries, self.retry_backoff.base_interval, self.retry_backoff.max_interval)
    }

    #[inline]
    pub fn per_try_timeout(&self) -> Option<Duration> {
        self.per_try_timeout
    }

    #[inline]
    pub fn num_retries(&self) -> u32 {
        self.num_retries
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryBackoff {
    #[serde(with = "humantime_serde")]
    pub base_interval: Duration,
    #[serde(with = "humantime_serde")]
    pub max_interval: Duration,
}

impl Default for RetryBackoff {
    fn default() -> Self {
        Self { base_interval: Duration::from_millis(25), max_interval: Duration::from_millis(250) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetryOn {
    Err5xx,
    GatewayError,
    Reset,
    ConnectFailure,
    EnvoyRateLimited,
    Retriable4xx,
    RefusedStream,
    RetriableStatusCodes,
    RetriableHeaders,
    Http3PostConnectFailure,
}

impl FromStr for RetryOn {
    type Err = GenericError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // see https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/router_filter#x-envoy-retry-on
        // for the list of fields and their meaning
        match s {
            "5xx" => Ok(RetryOn::Err5xx),
            "gateway-error" => Ok(RetryOn::GatewayError),
            "reset" => Ok(RetryOn::Reset),
            "connect-failure" => Ok(RetryOn::ConnectFailure),
            "envoy-ratelimited" => Ok(RetryOn::EnvoyRateLimited),
            "retriable-4xx" => Ok(RetryOn::Retriable4xx),
            "refused-stream" => Ok(RetryOn::RefusedStream),
            "retriable-status-codes" => Ok(RetryOn::RetriableStatusCodes),
            "retriable-headers" => Ok(RetryOn::RetriableHeaders),
            "http3-post-connect-failure" => Ok(RetryOn::Http3PostConnectFailure),
            s => Err(GenericError::from_msg(format!("Invalid RetryOn value \"{s}\""))),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Route {
    #[serde(skip_serializing_if = "is_default", default)]
    pub name: String,
    #[serde(skip_serializing_if = "is_default", default)]
    pub response_header_modifier: HeaderModifier,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    pub request_headers_to_add: Vec<HeaderValueOption>,
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Default::default")]
    #[serde(with = "http_serde_ext::header_name::vec")]
    pub request_headers_to_remove: Vec<HeaderName>,
    #[serde(rename = "match")]
    pub route_match: RouteMatch,
    //todo(hayley): fix this field. Is it used correctly? key is name in higher level filter. value is overwrite (append-overwrite?)
    #[serde(skip_serializing_if = "HashMap::is_empty", default = "Default::default")]
    pub typed_per_filter_config: std::collections::HashMap<CompactString, FilterOverride>,
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RdsSpecifier {
    pub route_config_name: CompactString,
    pub config_source: ConfigSource,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ConfigSource {
    pub config_source_specifier: ConfigSourceSpecifier,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum ConfigSourceSpecifier {
    ADS,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[inline]
    fn request_uri(uri: &str) -> http::Request<()> {
        http::Request::builder().uri(uri).body(()).unwrap()
    }

    #[test]
    fn match_host_exact() -> Result<(), GenericError> {
        assert_eq!(MatchHost::from_str("www.example.com")?, MatchHost::Exact("www.example.com".into()));

        assert_eq!(
            MatchHost::from_str("www.example.com")?.eval_lpm_host("www.example.com"),
            Some(MatchHostScoreLPM::Exact("www.example.com".len()))
        );

        assert_eq!(MatchHost::from_str("another.example.com")?.eval_lpm_host("www.example.com"), None);

        assert_eq!(
            MatchHost::from_str("localhost")?.eval_lpm_host("localhost"),
            Some(MatchHostScoreLPM::Exact("localhost".len()))
        );
        assert_eq!(MatchHost::from_str("localhost")?.eval_lpm_host("another"), None);
        Ok(())
    }

    #[test]
    fn match_host_suffix() -> Result<(), GenericError> {
        assert_eq!(MatchHost::from_str("*.example.com")?, MatchHost::Suffix(".example.com".into()));

        assert_eq!(
            MatchHost::from_str("*.example.com")?.eval_lpm_host("www.example.com"),
            Some(MatchHostScoreLPM::Suffix(12))
        );

        assert_eq!(MatchHost::from_str("*.example.com")?.eval_lpm_host("example.com"), None);

        Ok(())
    }

    #[test]
    fn test_host_exact() {
        let e: MatchHost = "test.com".parse().expect("test.com error parsing");
        assert_eq!(e, MatchHost::Exact("test.com".into()));
        assert_eq!(
            e.eval_lpm_request(&request_uri("http://test.com/foo/bar")),
            Some(MatchHostScoreLPM::Exact("test.com".len()))
        );
        assert_eq!(
            e.eval_lpm_request(&request_uri("http://test.com./foo/bar")),
            Some(MatchHostScoreLPM::Exact("test.com".len()))
        );
        assert_eq!(e.eval_lpm_request(&request_uri("http://foo.test.com/bar")), None);
    }

    #[test]
    fn test_host_suffix() {
        let rule: MatchHost = "*.test.com".parse().expect("*.test.com error parsing");
        assert_eq!(rule, MatchHost::Suffix(".test.com".into()));
        assert_eq!(
            rule.eval_lpm_request(&request_uri("http://foo.test.com/bar")),
            Some(MatchHostScoreLPM::Suffix(".test.com".len()))
        );
        assert_eq!(
            rule.eval_lpm_request(&request_uri("http://foo.bar.test.com/foo/bar")),
            Some(MatchHostScoreLPM::Suffix(".test.com".len()))
        );
        assert_eq!(
            rule.eval_lpm_request(&request_uri("http://foo.test.com./foo/bar")),
            Some(MatchHostScoreLPM::Suffix(".test.com".len()))
        );
        assert_eq!(rule.eval_lpm_request(&request_uri("http://foo.bar.test2.com/foo/bar")), None);
        assert_eq!(rule.eval_lpm_request(&request_uri("http://*test.com/foo/bar")), None);

        let rule: MatchHost = "*-bar.foo.com".parse().expect("*-bar.foo.com error parsing");
        assert_eq!(rule, MatchHost::Suffix("-bar.foo.com".into()));

        assert_eq!(
            rule.eval_lpm_request(&request_uri("http://baz-bar.foo.com/foo/bar")),
            Some(MatchHostScoreLPM::Suffix("-bar.foo.com".len()))
        );

        assert_eq!(rule.eval_lpm_request(&request_uri("http://-bar.foo.com/foo/bar")), None);
    }

    #[test]
    fn test_host_prefix() {
        let rule: MatchHost = "www.test.*".parse().expect("www.test.* error parsing");
        assert_eq!(rule, MatchHost::Prefix("www.test.".into()));
        assert_eq!(
            rule.eval_lpm_request(&request_uri("http://www.test.com/bar")),
            Some(MatchHostScoreLPM::Prefix("www.test.".len()))
        );
        assert_eq!(
            rule.eval_lpm_request(&request_uri("http://www.test.it/bar")),
            Some(MatchHostScoreLPM::Prefix("www.test.".len()))
        );
        assert_eq!(
            rule.eval_lpm_request(&request_uri("http://www.test.com./bar")),
            Some(MatchHostScoreLPM::Prefix("www.test.".len()))
        );

        assert_eq!(rule.eval_lpm_request(&request_uri("http://www.test2.com/bar")), None);
        assert_eq!(rule.eval_lpm_request(&request_uri("http://www.test./bar")), None);
        assert_eq!(rule.eval_lpm_request(&request_uri("http://www.test/bar")), None);
    }

    #[test]
    fn test_host_wildcard() {
        let rule: MatchHost = "*".parse().expect("* error parsing");
        assert_eq!(rule, MatchHost::Wildcard);

        assert_eq!(rule.eval_lpm_request(&request_uri("http://www.test.com/bar")), Some(MatchHostScoreLPM::Wildcard));
        assert_eq!(rule.eval_lpm_request(&request_uri("http://www.test.it/bar")), Some(MatchHostScoreLPM::Wildcard));
        assert_eq!(rule.eval_lpm_request(&request_uri("http://www.test.com./bar")), Some(MatchHostScoreLPM::Wildcard));

        assert_eq!(rule.eval_lpm_request(&request_uri("http://www.test2.com/bar")), Some(MatchHostScoreLPM::Wildcard));
        assert_eq!(rule.eval_lpm_request(&request_uri("http://www.test./bar")), Some(MatchHostScoreLPM::Wildcard));
        assert_eq!(rule.eval_lpm_request(&request_uri("http://www.test/bar")), Some(MatchHostScoreLPM::Wildcard));
    }

    #[test]
    fn test_bad_rules() {
        assert!("*asdf*".parse::<MatchHost>().is_err());
        assert!("*.example.*.com".parse::<MatchHost>().is_err());
        assert!("**".parse::<MatchHost>().is_err());
        assert!("asdf*asdf".parse::<MatchHost>().is_err());
        assert!("*asdf*".parse::<MatchHost>().is_err());
        assert!("*asdf*asdf".parse::<MatchHost>().is_err());
        assert!("asdf*asdf*".parse::<MatchHost>().is_err());
    }

    #[test]
    fn test_host_cmp() {
        assert!(MatchHostScoreLPM::Exact("test.com".len()) < MatchHostScoreLPM::Exact("foo.bar.test.com".len()));
        assert!(MatchHostScoreLPM::Suffix("test.com".len()) < MatchHostScoreLPM::Suffix("foo.bar.test.com".len()));
        assert!(MatchHostScoreLPM::Suffix("test.com".len()) < MatchHostScoreLPM::Suffix(".test.com".len()));
        assert!(MatchHostScoreLPM::Exact("test.com".len()) > MatchHostScoreLPM::Suffix("foo.bar.test.com".len()));
        assert_eq!(MatchHostScoreLPM::Suffix("foo.test.com".len()), MatchHostScoreLPM::Suffix("bar.test.com".len()));

        assert!(MatchHostScoreLPM::Exact("test.com".len()) < MatchHostScoreLPM::Exact("foo.bar.test".len()));
        assert!(MatchHostScoreLPM::Exact("test.com".len()) > MatchHostScoreLPM::Prefix("foo.bar.test".len()));
        assert!(MatchHostScoreLPM::Exact("test.com".len()) > MatchHostScoreLPM::Suffix("foo.bar.test.com".len()));
        assert!(MatchHostScoreLPM::Exact("test.com".len()) > MatchHostScoreLPM::Wildcard);

        assert!(MatchHostScoreLPM::Suffix(".test.com".len()) < MatchHostScoreLPM::Exact("test.com".len()));
        assert!(MatchHostScoreLPM::Suffix(".test.com".len()) > MatchHostScoreLPM::Prefix("foo.bar.test".len()));
        assert!(MatchHostScoreLPM::Suffix(".test.com".len()) < MatchHostScoreLPM::Suffix("foo.bar.test.com".len()));
        assert!(MatchHostScoreLPM::Suffix(".test.com".len()) > MatchHostScoreLPM::Wildcard);

        assert!(MatchHostScoreLPM::Prefix("www.test.".len()) < MatchHostScoreLPM::Exact("test.com".len()));
        assert!(MatchHostScoreLPM::Prefix("www.test.".len()) < MatchHostScoreLPM::Prefix("foo.bar.test.".len()));
        assert!(MatchHostScoreLPM::Prefix("www.test.".len()) < MatchHostScoreLPM::Suffix("foo.bar.test.com".len()));
        assert!(MatchHostScoreLPM::Prefix("www.test.".len()) > MatchHostScoreLPM::Wildcard);

        assert!(MatchHostScoreLPM::Wildcard < MatchHostScoreLPM::Exact("test.com".len()));
        assert!(MatchHostScoreLPM::Wildcard < MatchHostScoreLPM::Prefix("foo.bar.test.".len()));
        assert!(MatchHostScoreLPM::Wildcard < MatchHostScoreLPM::Suffix("foo.bar.test.com".len()));
        assert!(MatchHostScoreLPM::Wildcard == MatchHostScoreLPM::Wildcard);
    }
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{
        header_modifer::HeaderModifier,
        http_filters::{
            router::Router, FilterConfigOverride, FilterOverride, HttpFilter, HttpFilterType, SupportedEnvoyFilter,
            SupportedEnvoyHttpFilter,
        },
        CodecType, ConfigSource, ConfigSourceSpecifier, HttpConnectionManager, RdsSpecifier, RetryBackoff, RetryOn,
        RetryPolicy, Route, RouteConfiguration, RouteSpecifier, VirtualHost,
    };
    use crate::config::{
        common::*,
        util::{duration_from_envoy, http_status_from},
    };
    use compact_str::CompactString;
    use http::HeaderName;
    use orion_data_plane_api::envoy_data_plane_api::envoy::{
        config::{
            core::v3::{
                config_source::ConfigSourceSpecifier as EnvoyConfigSourceSpecifier, AggregatedConfigSource,
                ConfigSource as EnvoyConfigSource,
            },
            route::v3::{
                retry_policy::RetryBackOff as EnvoyRetryBackoff, RetryPolicy as EnvoyRetryPolicy, Route as EnvoyRoute,
                RouteConfiguration as EnvoyRouteConfiguration, VirtualHost as EnvoyVirtualHost,
            },
        },
        extensions::filters::network::http_connection_manager::v3::{
            http_connection_manager::{CodecType as EnvoyCodecType, RouteSpecifier as EnvoyRouteSpecifier},
            HttpConnectionManager as EnvoyHttpConnectionManager, Rds as EnvoyRds,
        },
    };
    use std::{collections::HashMap, str::FromStr, time::Duration};

    impl HttpConnectionManager {
        fn ensure_corresponding_filter_exists(
            filter_override: (&CompactString, &FilterOverride),
            http_filters: &[HttpFilter],
        ) -> Result<(), GenericError> {
            let (name, config) = filter_override;
            match http_filters.iter().find(|filter| filter.name == name) {
                None => Err(GenericError::from_msg(format!("http filter \"{name}\" does not exist"))),
                Some(matching_filter) => match &config.filter_settings {
                    None => Ok(()),
                    Some(x) => match (x, &matching_filter.filter) {
                        (FilterConfigOverride::LocalRateLimit(_), HttpFilterType::RateLimit(_))
                        | (FilterConfigOverride::Rbac(_), HttpFilterType::Rbac(_)) => Ok(()),
                        (_, _) => Err(GenericError::from_msg(format!(
                            "can't override http filter \"{name}\" with a different filter type"
                        ))),
                    },
                },
            }
        }
    }

    impl TryFrom<EnvoyHttpConnectionManager> for HttpConnectionManager {
        type Error = GenericError;
        #[allow(clippy::too_many_lines)]
        fn try_from(envoy: EnvoyHttpConnectionManager) -> Result<Self, Self::Error> {
            let EnvoyHttpConnectionManager {
                codec_type,
                stat_prefix,
                http_filters,
                add_user_agent,
                tracing,
                common_http_protocol_options,
                http_protocol_options,
                http2_protocol_options,
                http3_protocol_options,
                server_name,
                server_header_transformation,
                scheme_header_transformation,
                max_request_headers_kb,
                stream_idle_timeout,
                request_timeout,
                request_headers_timeout,
                drain_timeout,
                delayed_close_timeout,
                access_log,
                access_log_flush_interval,
                flush_access_log_on_new_request,
                access_log_options,
                use_remote_address,
                xff_num_trusted_hops,
                original_ip_detection_extensions,
                early_header_mutation_extensions,
                internal_address_config,
                skip_xff_append,
                via,
                generate_request_id,
                preserve_external_request_id,
                always_set_request_id_in_response,
                forward_client_cert_details,
                set_current_client_cert_details,
                proxy_100_continue,
                represent_ipv4_remote_address_as_ipv4_mapped_ipv6,
                upgrade_configs,
                normalize_path,
                merge_slashes,
                path_with_escaped_slashes_action,
                request_id_extension,
                local_reply_config,
                strip_matching_host_port,
                stream_error_on_invalid_http_message,
                path_normalization_options,
                strip_trailing_host_dot,
                proxy_status_config,
                typed_header_validation_config,
                append_x_forwarded_port,
                add_proxy_protocol_connection_state,
                route_specifier,
                strip_port_mode,
                http1_safe_max_connection_duration,
                append_local_overload,
            } = envoy;
            unsupported_field!(
                // codec_type,
                // stat_prefix,
                // http_filters,
                add_user_agent,
                tracing,
                common_http_protocol_options,
                http_protocol_options,
                http2_protocol_options,
                http3_protocol_options,
                server_name,
                server_header_transformation,
                scheme_header_transformation,
                max_request_headers_kb,
                stream_idle_timeout,
                // request_timeout,
                request_headers_timeout,
                drain_timeout,
                delayed_close_timeout,
                access_log,
                access_log_flush_interval,
                flush_access_log_on_new_request,
                access_log_options,
                use_remote_address,
                xff_num_trusted_hops,
                original_ip_detection_extensions,
                early_header_mutation_extensions,
                internal_address_config,
                skip_xff_append,
                via,
                generate_request_id,
                preserve_external_request_id,
                always_set_request_id_in_response,
                forward_client_cert_details,
                set_current_client_cert_details,
                proxy_100_continue,
                represent_ipv4_remote_address_as_ipv4_mapped_ipv6,
                upgrade_configs,
                normalize_path,
                merge_slashes,
                path_with_escaped_slashes_action,
                request_id_extension,
                local_reply_config,
                strip_matching_host_port,
                stream_error_on_invalid_http_message,
                path_normalization_options,
                strip_trailing_host_dot,
                proxy_status_config,
                typed_header_validation_config,
                append_x_forwarded_port,
                add_proxy_protocol_connection_state,
                // route_specifier,
                strip_port_mode,
                http1_safe_max_connection_duration,
                append_local_overload
            )?;
            if stat_prefix.is_used() {
                tracing::warn!(
                    "unsupported field stat_prefix used in http_connection_manager. This field will be ignored."
                );
            }
            let codec_type = codec_type.try_into().with_node("codec")?;
            let route_specifier = route_specifier.try_into()?;
            let request_timeout = request_timeout
                .map(duration_from_envoy)
                .transpose()
                .map_err(|_| GenericError::from_msg("failed to convert into Duration"))
                .with_node("request_timeout")?;
            let mut http_filters: Vec<SupportedEnvoyHttpFilter> = convert_non_empty_vec!(http_filters)?;
            if http_filters.is_empty() {
                return Err(GenericError::from_msg("http_filters cannot be empty").with_node("http_filters"));
            };
            match http_filters.pop() {
                Some(SupportedEnvoyHttpFilter { filter: SupportedEnvoyFilter::Router(rtr), name, disabled: false }) => {
                    Router::try_from(rtr).with_node(name)
                },
                Some(SupportedEnvoyHttpFilter { filter: SupportedEnvoyFilter::Router(_), name, disabled: true }) => {
                    Err(GenericError::from_msg("router cannot be disabled").with_node(name))
                },
                _ => Err(GenericError::from_msg("final filter of the chain has to be a router")),
            }
            .with_node("http_filters")?;

            let http_filters = convert_vec!(http_filters).with_node("http_filters")?;

            // and now we make sure to validate that any overrides specified in the routes, actually match the name and type of these filters
            //todo(hayley): this check only happens when converting from envoy to our config types.
            // we should make sure this check always happens when constructing, so it also happens when deserializing this struct directly.
            // or maybe want to specify over-rides per filter type in the ng config struct so that the equality gets encoded in the type system
            // or diverge from envoy by letting the user override filters with a different type. Doesn't seem like it's too terrible an idea
            if let RouteSpecifier::RouteConfig(route_config) = &route_specifier {
                for vh in &route_config.virtual_hosts {
                    let result = vh
                        .routes
                        .iter()
                        .flat_map(|r| {
                            r.typed_per_filter_config.iter().map(|filter_override| {
                                Self::ensure_corresponding_filter_exists(filter_override, &http_filters)
                            })
                        })
                        .collect::<Result<(), _>>();
                    if let Err(e) = result {
                        return Err(e
                            .with_node("typed_per_filter_config")
                            .with_node("route")
                            .with_node(vh.name.clone())
                            .with_node("virtual_hosts")
                            .with_node("route_specifier"));
                    }
                }
            }
            Ok(Self { codec_type, http_filters, route_specifier, request_timeout })
        }
    }

    impl TryFrom<EnvoyCodecType> for CodecType {
        type Error = GenericError;
        fn try_from(envoy: EnvoyCodecType) -> Result<Self, Self::Error> {
            match envoy {
                EnvoyCodecType::Auto => Ok(Self::Auto),
                EnvoyCodecType::Http1 => Ok(Self::Http1),
                EnvoyCodecType::Http2 => Ok(Self::Http2),
                EnvoyCodecType::Http3 => Err(GenericError::unsupported_variant("Http3")),
            }
        }
    }

    impl TryFrom<i32> for CodecType {
        type Error = GenericError;
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            EnvoyCodecType::from_i32(value).ok_or(GenericError::unsupported_variant("[unknown codec type]"))?.try_into()
        }
    }

    impl TryFrom<Option<EnvoyRouteSpecifier>> for RouteSpecifier {
        type Error = GenericError;
        fn try_from(envoy: Option<EnvoyRouteSpecifier>) -> Result<Self, Self::Error> {
            Ok(match envoy {
                Some(EnvoyRouteSpecifier::Rds(rds)) => {
                    Self::Rds(RdsSpecifier::try_from(rds).map_err(|e| e.with_node("rds"))?)
                },
                Some(EnvoyRouteSpecifier::RouteConfig(envoy)) => {
                    Self::RouteConfig(envoy.try_into().with_node("route_config")?)
                },
                Some(EnvoyRouteSpecifier::ScopedRoutes(_)) => {
                    return Err(GenericError::unsupported_variant("ScopedRoutes"))
                },
                None => return Err(GenericError::MissingField("rds or route_config")),
            })
        }
    }

    impl TryFrom<EnvoyRouteConfiguration> for RouteConfiguration {
        type Error = GenericError;
        fn try_from(envoy: EnvoyRouteConfiguration) -> Result<Self, Self::Error> {
            let EnvoyRouteConfiguration {
                name,
                virtual_hosts,
                vhds,
                internal_only_headers,
                response_headers_to_add,
                response_headers_to_remove,
                request_headers_to_add,
                request_headers_to_remove,
                most_specific_header_mutations_wins,
                validate_clusters,
                max_direct_response_body_size_bytes,
                cluster_specifier_plugins,
                request_mirror_policies,
                ignore_port_in_host_matching,
                ignore_path_parameters_in_path_matching,
                typed_per_filter_config,
                metadata,
            } = envoy;
            unsupported_field!(
                // name,
                // virtual_hosts,
                vhds,
                internal_only_headers,
                // response_headers_to_add,
                // response_headers_to_remove,
                // request_headers_to_add,
                // request_headers_to_remove,
                // most_specific_header_mutations_wins,
                validate_clusters,
                max_direct_response_body_size_bytes,
                cluster_specifier_plugins,
                request_mirror_policies,
                ignore_port_in_host_matching,
                ignore_path_parameters_in_path_matching,
                typed_per_filter_config,
                metadata
            )?;
            let name: CompactString = required!(name)?.into();
            (|| -> Result<_, GenericError> {
                let response_headers_to_add = convert_vec!(response_headers_to_add)?;
                let request_headers_to_add = convert_vec!(request_headers_to_add)?;
                let response_headers_to_remove = response_headers_to_remove
                    .into_iter()
                    .map(|s| {
                        HeaderName::from_str(s.as_str()).map_err(|e| {
                            GenericError::from_msg_with_cause(format!("failed to convert \"{s}\" into HeaderName"), e)
                                .with_node("response_headers_to_remove")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let request_headers_to_remove = request_headers_to_remove
                    .into_iter()
                    .map(|s| {
                        HeaderName::from_str(s.as_str()).map_err(|e| {
                            GenericError::from_msg_with_cause(format!("failed to convert \"{s}\" into HeaderName"), e)
                                .with_node("request_headers_to_remove")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let virtual_hosts = convert_non_empty_vec!(virtual_hosts)?;
                let response_header_modifier = HeaderModifier::new(response_headers_to_remove, response_headers_to_add);
                Ok(Self {
                    name: name.clone(),
                    virtual_hosts,
                    most_specific_header_mutations_wins,
                    response_header_modifier,
                    request_headers_to_add,
                    request_headers_to_remove,
                })
            })()
            .with_name(name)
        }
    }

    impl TryFrom<EnvoyVirtualHost> for VirtualHost {
        type Error = GenericError;
        fn try_from(envoy: EnvoyVirtualHost) -> Result<Self, Self::Error> {
            let EnvoyVirtualHost {
                name,
                domains,
                routes,
                matcher,
                require_tls,
                virtual_clusters,
                rate_limits,
                request_headers_to_add,
                request_headers_to_remove,
                response_headers_to_add,
                response_headers_to_remove,
                cors,
                typed_per_filter_config,
                include_request_attempt_count,
                include_attempt_count_in_response,
                retry_policy,
                retry_policy_typed_config,
                hedge_policy,
                include_is_timeout_retry_header,
                per_request_buffer_limit_bytes,
                request_mirror_policies,
                metadata,
            } = envoy;
            unsupported_field!(
                // name,
                // domains,
                // routes,
                matcher,
                require_tls,
                virtual_clusters,
                rate_limits,
                // request_headers_to_add,
                // request_headers_to_remove,
                // response_headers_to_add,
                // response_headers_to_remove,
                cors,
                typed_per_filter_config,
                include_request_attempt_count,
                include_attempt_count_in_response,
                // retry_policy,
                retry_policy_typed_config,
                hedge_policy,
                include_is_timeout_retry_header,
                per_request_buffer_limit_bytes,
                request_mirror_policies,
                metadata
            )?;
            let name: CompactString = required!(name)?.into();
            (|| -> Result<_, GenericError> {
                let response_headers_to_add = convert_vec!(response_headers_to_add)?;
                let request_headers_to_add = convert_vec!(request_headers_to_add)?;
                let response_headers_to_remove = response_headers_to_remove
                    .into_iter()
                    .map(|s| {
                        HeaderName::from_str(s.as_str()).map_err(|e| {
                            GenericError::from_msg_with_cause(format!("failed to convert \"{s}\" into HeaderName"), e)
                                .with_node("response_headers_to_remove")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let request_headers_to_remove = request_headers_to_remove
                    .into_iter()
                    .map(|s| {
                        HeaderName::from_str(s.as_str()).map_err(|e| {
                            GenericError::from_msg_with_cause(format!("failed to convert \"{s}\" into HeaderName"), e)
                                .with_node("request_headers_to_remove")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let domains = convert_non_empty_vec!(domains)?;
                let routes = convert_vec!(routes)?;

                let retry_policy = retry_policy.map(RetryPolicy::try_from).transpose().with_node("retry_policy")?;
                let response_header_modifier = HeaderModifier::new(response_headers_to_remove, response_headers_to_add);
                Ok(Self {
                    name: name.clone(),
                    routes,
                    domains,
                    request_headers_to_add,
                    request_headers_to_remove,
                    retry_policy,
                    response_header_modifier,
                })
            })()
            .with_name(name)
        }
    }

    impl TryFrom<EnvoyRetryPolicy> for RetryPolicy {
        type Error = GenericError;
        fn try_from(value: EnvoyRetryPolicy) -> Result<Self, Self::Error> {
            let EnvoyRetryPolicy {
                retry_on,
                num_retries,
                per_try_timeout,
                per_try_idle_timeout,
                retry_priority,
                retry_host_predicate,
                retry_options_predicates,
                host_selection_retry_max_attempts,
                retriable_status_codes,
                retry_back_off,
                rate_limited_retry_back_off,
                retriable_headers,
                retriable_request_headers,
            } = value;
            unsupported_field!(
                // retry_on,
                // num_retries,
                // per_try_timeout,
                per_try_idle_timeout,
                retry_priority,
                retry_host_predicate,
                retry_options_predicates,
                host_selection_retry_max_attempts,
                // retriable_status_codes,
                // retry_back_off,
                // retriable_headers,
                // retriable_request_headers
                rate_limited_retry_back_off
            )?;
            let retry_on =
                retry_on.split(',').map(RetryOn::from_str).collect::<Result<Vec<_>, _>>().with_node("retry_on")?;
            let num_retries = num_retries.map(|v| v.value).unwrap_or(1);
            // from the docs,
            // > If left unspecified, Envoy will use the global
            // > :ref:`route timeout <envoy_v3_api_field_config.route.v3.RouteAction.timeout>` for the request.
            // do we do that? if not we should require this field first.
            // and, if we do use this field, do/should we ignore the route action timeout?
            let per_try_timeout = per_try_timeout
                .map(duration_from_envoy)
                .transpose()
                .map_err(|_| GenericError::from_msg("failed to convert into Duration").with_node("per_try_timeout"))?;
            let retriable_status_codes = retriable_status_codes
                .into_iter()
                .map(http_status_from)
                .collect::<Result<Vec<_>, _>>()
                .with_node("retriable_status_codes")?;
            let retry_backoff =
                retry_back_off.map(RetryBackoff::try_from).transpose().with_node("retry_backoff")?.unwrap_or_default();
            let retriable_headers = convert_vec!(retriable_headers)?;
            let retriable_request_headers = convert_vec!(retriable_request_headers)?;
            Ok(Self {
                retry_on,
                num_retries,
                per_try_timeout,
                retriable_status_codes,
                retry_backoff,
                retriable_request_headers,
                retriable_headers,
            })
        }
    }

    impl TryFrom<EnvoyRetryBackoff> for RetryBackoff {
        type Error = GenericError;
        fn try_from(value: EnvoyRetryBackoff) -> Result<Self, Self::Error> {
            let EnvoyRetryBackoff { base_interval, max_interval } = value;
            //note: envoy docs says this can't be zero, but also that less than 1ms gets rounded up
            // so for simplicity we just round up zero too.
            let base_interval = duration_from_envoy(required!(base_interval)?)
                .with_node("base_interval")?
                .max(Duration::from_millis(1));
            let max_interval = max_interval
                .map(duration_from_envoy)
                .transpose()
                .map_err(|_| GenericError::from_msg("failed to convert into Duration"))
                .with_node("max_interval")?
                .unwrap_or(base_interval * 10);
            if max_interval < base_interval {
                return Err(GenericError::from_msg(format!(
                    "max_interval ({}ms) is less than base_interval ({}ms)",
                    max_interval.as_millis(),
                    base_interval.as_millis()
                )));
            }
            Ok(Self { base_interval, max_interval })
        }
    }

    impl TryFrom<EnvoyRoute> for Route {
        type Error = GenericError;
        fn try_from(envoy: EnvoyRoute) -> Result<Self, Self::Error> {
            let EnvoyRoute {
                name,
                r#match,
                metadata,
                decorator,
                typed_per_filter_config,
                request_headers_to_add,
                request_headers_to_remove,
                response_headers_to_add,
                response_headers_to_remove,
                tracing,
                per_request_buffer_limit_bytes,
                stat_prefix,
                action,
            } = envoy;
            unsupported_field!(
                // r#match,
                metadata,
                decorator,
                // typed_per_filter_config,
                // request_headers_to_add,
                // request_headers_to_remove,
                // response_headers_to_add,
                // response_headers_to_remove,
                tracing,
                per_request_buffer_limit_bytes,
                stat_prefix // action
            )?;
            let response_headers_to_add = convert_vec!(response_headers_to_add)?;
            let request_headers_to_add = convert_vec!(request_headers_to_add)?;
            let response_headers_to_remove = response_headers_to_remove
                .into_iter()
                .map(|s| {
                    HeaderName::from_str(s.as_str()).map_err(|e| {
                        GenericError::from_msg_with_cause(format!("failed to convert \"{s}\" into HeaderName"), e)
                            .with_node("response_headers_to_remove")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let request_headers_to_remove = request_headers_to_remove
                .into_iter()
                .map(|s| {
                    HeaderName::from_str(s.as_str()).map_err(|e| {
                        GenericError::from_msg_with_cause(format!("failed to convert \"{s}\" into HeaderName"), e)
                            .with_node("request_headers_to_remove")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let action = convert_opt!(action)?;
            let route_match = convert_opt!(r#match, "match")?;
            let typed_per_filter_config = {
                typed_per_filter_config
                    .into_iter()
                    .map(|(name, typed_config)| {
                        FilterOverride::try_from(typed_config).map(|x| (CompactString::new(&name), x)).with_node(name)
                    })
                    .collect::<Result<HashMap<_, _>, GenericError>>()
            }
            .with_node("typed_per_filter_config")?;
            let response_header_modifier = HeaderModifier::new(response_headers_to_remove, response_headers_to_add);
            Ok(Self {
                name: name.clone(),
                route_match,
                action,
                typed_per_filter_config,
                request_headers_to_add,
                request_headers_to_remove,
                response_header_modifier,
            })
        }
    }

    impl TryFrom<EnvoyRds> for RdsSpecifier {
        type Error = GenericError;
        fn try_from(value: EnvoyRds) -> Result<Self, Self::Error> {
            let EnvoyRds { config_source, route_config_name } = value;
            let route_config_name = required!(route_config_name)?.into();
            let config_source = convert_opt!(config_source)?;
            Ok(Self { route_config_name, config_source })
        }
    }
    impl TryFrom<EnvoyConfigSource> for ConfigSource {
        type Error = GenericError;
        fn try_from(value: EnvoyConfigSource) -> Result<Self, Self::Error> {
            let EnvoyConfigSource { authorities, initial_fetch_timeout, resource_api_version, config_source_specifier } =
                value;
            unsupported_field!(authorities, initial_fetch_timeout, resource_api_version)?;
            let config_source_specifier = convert_opt!(config_source_specifier)?;
            Ok(Self { config_source_specifier })
        }
    }

    impl TryFrom<EnvoyConfigSourceSpecifier> for ConfigSourceSpecifier {
        type Error = GenericError;
        fn try_from(value: EnvoyConfigSourceSpecifier) -> Result<Self, Self::Error> {
            match value {
                EnvoyConfigSourceSpecifier::Ads(AggregatedConfigSource {}) => Ok(Self::ADS),
                EnvoyConfigSourceSpecifier::ApiConfigSource(_) => {
                    Err(GenericError::unsupported_variant("ApiConfigSource"))
                },
                EnvoyConfigSourceSpecifier::Path(_) => Err(GenericError::unsupported_variant("Path")),
                EnvoyConfigSourceSpecifier::PathConfigSource(_) => {
                    Err(GenericError::unsupported_variant("PathConfigSource"))
                },
                EnvoyConfigSourceSpecifier::Self_(_) => Err(GenericError::unsupported_variant("Self_")),
            }
        }
    }
}
