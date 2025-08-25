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

pub use crate::config::network_filters::network_rbac::Action;
use crate::config::{common::*, core::StringMatcher};
use http::{HeaderMap, HeaderName, Method, Request, Response, Uri};
use serde::{de::Visitor, Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderNames {
    Method,
    Path,
    Scheme,
    NormalHeader(HeaderName),
}

impl Serialize for HeaderNames {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Method => serializer.serialize_str(":method"),
            Self::Path => serializer.serialize_str(":path"),
            Self::Scheme => serializer.serialize_str(":scheme"),
            Self::NormalHeader(header) => http_serde_ext::header_name::serialize(header, serializer),
        }
    }
}

impl<'de> Deserialize<'de> for HeaderNames {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StrVisitor;
        impl Visitor<'_> for StrVisitor {
            type Value = HeaderNames;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("`str`")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v {
                    ":path" => Ok(HeaderNames::Path),
                    ":method" => Ok(HeaderNames::Method),
                    ":scheme" => Ok(HeaderNames::Scheme),
                    s => HeaderName::from_str(s).map(HeaderNames::NormalHeader).map_err(E::custom),
                }
            }
        }
        deserializer.deserialize_str(StrVisitor)
    }
}

impl From<HeaderName> for HeaderNames {
    fn from(value: HeaderName) -> Self {
        //don't need to check for headers starting with a colon here, because those are invalid
        Self::NormalHeader(value)
    }
}

impl FromStr for HeaderNames {
    type Err = GenericError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            ":method" => Ok(Self::Method),
            ":scheme" => Ok(Self::Scheme),
            ":path" => Ok(Self::Path),
            s => HeaderName::from_str(s).map(Self::NormalHeader).map_err(|e| {
                GenericError::from_msg_with_cause(format!("couldn't convert \"{s}\" into a HeaderName"), e)
            }),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HeaderMatcher {
    #[serde(rename = "name")]
    pub header_name: HeaderNames,
    #[serde(skip_serializing_if = "std::ops::Not::not", default = "Default::default")]
    pub invert_match: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not", default = "Default::default")]
    pub treat_missing_header_as_empty: bool,
    #[serde(flatten)]
    pub header_matcher: StringMatcher,
}

impl HeaderMatcher {
    pub fn request_matches<B>(&self, request: &Request<B>) -> bool {
        self.is_match(Some(request.method()), Some(request.uri()), request.headers())
    }
    pub fn response_matches<B>(&self, response: &Response<B>) -> bool {
        //todo(hayley): what is the expected behaviour here? responses don't have a uri or method
        // what if someone sets them in the config though, with treat_missing_header_as_empty?
        self.is_match(None, None, response.headers())
    }
    /// checks if this matcher matches any of the headers in header_map
    /// Header values that contain non visible-ascii characters are skipped
    fn is_match(&self, method: Option<&Method>, uri: Option<&Uri>, header_map: &HeaderMap) -> bool {
        match &self.header_name {
            HeaderNames::Method => {
                let method = match method {
                    None if self.treat_missing_header_as_empty => "",
                    Some(method) => method.as_str(),
                    None => {
                        return false;
                    },
                };
                self.header_matcher.matches(method) ^ self.invert_match
            },
            HeaderNames::Path => {
                let path = match uri {
                    None if self.treat_missing_header_as_empty => "",
                    Some(uri) => uri.path(),
                    None => {
                        return false;
                    },
                };
                self.header_matcher.matches(path) ^ self.invert_match
            },
            HeaderNames::Scheme => {
                let scheme = match uri.map(|uri| uri.scheme_str()) {
                    None | Some(None) if self.treat_missing_header_as_empty => "",
                    Some(Some(scheme)) => scheme,
                    None | Some(None) => {
                        return false;
                    },
                };
                self.header_matcher.matches(scheme) ^ self.invert_match
            },
            HeaderNames::NormalHeader(header_name) => {
                let mut header_values = header_map.get_all(header_name).into_iter().map(|hv| hv.to_str().ok());
                match header_values.next() {
                    //no header found
                    None => {
                        if self.treat_missing_header_as_empty {
                            self.header_matcher.matches("") ^ self.invert_match
                        } else {
                            // missing headers don't get inverted
                            false
                        }
                    },
                    Some(first) => {
                        let first_result = first.map(|s| self.header_matcher.matches(s)).unwrap_or(false);
                        let any_matched = first_result
                            | header_values.any(|hv| hv.map(|s| self.header_matcher.matches(s)).unwrap_or(false));
                        any_matched ^ self.invert_match
                    },
                }
            },
        }
    }
}

#[cfg(test)]
mod header_matcher_tests {
    use super::*;
    use crate::config::core::{StringMatcher, StringMatcherPattern};
    use http::header::*;
    use std::str::FromStr;

    #[test]
    fn test_header_exact() {
        let mut hm = HeaderMap::new();

        let mut h = HeaderMatcher {
            header_name: ACCEPT.into(),
            invert_match: false,
            treat_missing_header_as_empty: false,
            header_matcher: StringMatcher {
                ignore_case: false,
                pattern: StringMatcherPattern::Exact("text/html".into()),
            },
        };

        hm.insert(ACCEPT, HeaderValue::from_static("text/html"));

        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();

        assert!(h.request_matches(&req));

        h.header_matcher =
            StringMatcher { ignore_case: false, pattern: StringMatcherPattern::Exact("text/json".into()) };
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();

        assert!(!h.request_matches(&req));
    }

    #[test]
    fn test_missing() {
        let mut hm = HeaderMap::new();

        let mut h = HeaderMatcher {
            header_name: ACCEPT.into(),
            invert_match: false,
            treat_missing_header_as_empty: false,
            header_matcher: StringMatcher { ignore_case: false, pattern: StringMatcherPattern::Exact("".into()) },
        };

        h.treat_missing_header_as_empty = true;
        h.invert_match = false;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(h.request_matches(&req));

        h.treat_missing_header_as_empty = false;
        h.invert_match = false;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(!h.request_matches(&req));

        h.treat_missing_header_as_empty = false;
        h.invert_match = true;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(!h.request_matches(&req));

        h.treat_missing_header_as_empty = true;
        h.invert_match = true;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(!h.request_matches(&req));

        h.header_matcher =
            StringMatcher { ignore_case: false, pattern: StringMatcherPattern::Exact("not empty".into()) };

        h.treat_missing_header_as_empty = true;
        h.invert_match = false;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(!h.request_matches(&req));

        h.treat_missing_header_as_empty = false;
        h.invert_match = false;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(!h.request_matches(&req));

        h.treat_missing_header_as_empty = false;
        h.invert_match = true;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(!h.request_matches(&req));

        h.treat_missing_header_as_empty = true;
        h.invert_match = true;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(h.request_matches(&req));

        hm.insert(ACCEPT, HeaderValue::from_static("not empty"));
        h.treat_missing_header_as_empty = true;
        h.invert_match = false;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(h.request_matches(&req));

        h.treat_missing_header_as_empty = false;
        h.invert_match = false;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(h.request_matches(&req));

        h.treat_missing_header_as_empty = false;
        h.invert_match = true;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(!h.request_matches(&req));

        h.treat_missing_header_as_empty = true;
        h.invert_match = true;
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        let req = builder.body(()).unwrap();
        assert!(!h.request_matches(&req));
    }

    #[test]
    fn test_header_regex() {
        let mut hm = HeaderMap::new();
        let re = "[A-Za-z]+/[A-Za-z]+";

        let h = HeaderMatcher {
            header_name: ACCEPT.into(),
            invert_match: false,
            treat_missing_header_as_empty: false,
            header_matcher: StringMatcher {
                ignore_case: true,
                pattern: StringMatcherPattern::Regex(regex::Regex::from_str(re).unwrap()),
            },
        };

        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        assert!(!h.request_matches(&builder.body(()).unwrap()));

        hm.insert(ACCEPT, HeaderValue::from_static("text/json"));
        let mut builder = http::request::Builder::new();
        *builder.headers_mut().unwrap() = hm.clone();
        assert!(h.request_matches(&builder.body(()).unwrap()));
    }
}

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::{HeaderMatcher, HeaderNames};
    use crate::config::{
        common::*,
        core::{regex_from_envoy, StringMatcher, StringMatcherPattern},
    };
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::route::v3::{
        header_matcher::HeaderMatchSpecifier as EnvoyHeaderMatchSpecifier, HeaderMatcher as EnvoyHeaderMatcher,
    };
    use std::str::FromStr;

    impl TryFrom<EnvoyHeaderMatcher> for HeaderMatcher {
        type Error = GenericError;
        fn try_from(value: EnvoyHeaderMatcher) -> Result<Self, Self::Error> {
            let EnvoyHeaderMatcher { name, invert_match, treat_missing_header_as_empty, header_match_specifier } =
                value;
            let header_name = HeaderNames::from_str(&name).with_node("name")?;
            let header_matcher = convert_opt!(header_match_specifier)?;
            Ok(Self { header_name, treat_missing_header_as_empty, invert_match, header_matcher })
        }
    }

    impl TryFrom<EnvoyHeaderMatchSpecifier> for StringMatcher {
        type Error = GenericError;
        fn try_from(value: EnvoyHeaderMatchSpecifier) -> Result<Self, Self::Error> {
            match value {
                EnvoyHeaderMatchSpecifier::StringMatch(matcher) => matcher.try_into(),
                EnvoyHeaderMatchSpecifier::ContainsMatch(s) => {
                    Ok(Self { ignore_case: false, pattern: StringMatcherPattern::Contains(s.into()) })
                },
                EnvoyHeaderMatchSpecifier::ExactMatch(s) => {
                    Ok(Self { ignore_case: false, pattern: StringMatcherPattern::Exact(s.into()) })
                },
                EnvoyHeaderMatchSpecifier::SafeRegexMatch(r) => {
                    Ok(Self { ignore_case: false, pattern: StringMatcherPattern::Regex(regex_from_envoy(r)?) })
                },
                EnvoyHeaderMatchSpecifier::PrefixMatch(s) => {
                    Ok(Self { ignore_case: false, pattern: StringMatcherPattern::Prefix(s.into()) })
                },
                EnvoyHeaderMatchSpecifier::SuffixMatch(s) => {
                    Ok(Self { ignore_case: false, pattern: StringMatcherPattern::Suffix(s.into()) })
                },
                EnvoyHeaderMatchSpecifier::RangeMatch(_) => Err(GenericError::unsupported_variant("RangeMatch")),
                EnvoyHeaderMatchSpecifier::PresentMatch(_) => Err(GenericError::unsupported_variant("PresentMatch")),
            }
        }
    }
}
