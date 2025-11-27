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

use http::{
    uri::{Authority, PathAndQuery, Scheme, Uri},
    Request, Response,
};
use orion_configuration::config::network_filters::http_connection_manager::http_filters::ext_proc::HeaderMutationRules;
use orion_data_plane_api::envoy_data_plane_api::envoy::{
    config::core::v3::{header_value_option::HeaderAppendAction, HeaderValueOption},
    service::ext_proc::v3::HeaderMutation,
};
use tracing::warn;

use crate::Error;

const PSEUDO_HEADER_METHOD: &str = ":method";
const PSEUDO_HEADER_SCHEME: &str = ":scheme";
const PSEUDO_HEADER_AUTHORITY: &str = ":authority";
const PSEUDO_HEADER_PATH: &str = ":path";
const PSEUDO_HEADER_STATUS: &str = ":status";

/// Extracts the string value from a `HeaderValueOption`.
/// Returns:
/// - None if the header is not present
/// - Some(Ok(s)) if the value was successfully extracted as a string
/// - Some(Err(())) if `raw_value` contains invalid UTF-8
fn try_extract_header_value_as_str(opt: &HeaderValueOption) -> Option<Result<&str, ()>> {
    match opt.header.as_ref() {
        Some(h) if !h.raw_value.is_empty() => Some(std::str::from_utf8(&h.raw_value).map_err(|_| ())),
        Some(h) => Some(Ok(h.value.as_str())),
        None => None,
    }
}

/// Holds the pseudo-headers that need to be applied to a request
#[derive(Debug)]
struct PseudoHeaders<'a> {
    method: Option<&'a HeaderValueOption>,
    scheme: Option<&'a HeaderValueOption>,
    authority: Option<&'a HeaderValueOption>,
    path: Option<&'a HeaderValueOption>,
    status: Option<&'a HeaderValueOption>,
}

impl PseudoHeaders<'_> {
    fn new() -> Self {
        PseudoHeaders { method: None, scheme: None, authority: None, path: None, status: None }
    }

    fn is_empty(&self) -> bool {
        self.method.is_none() && self.scheme.is_none() && self.authority.is_none() && self.path.is_none()
    }
}

impl<'a> From<&'a HeaderMutation> for PseudoHeaders<'a> {
    fn from(mutation: &'a HeaderMutation) -> Self {
        let mut pseudo_headers: PseudoHeaders<'a> = PseudoHeaders::new();

        for header_to_set in &mutation.set_headers {
            let Some(header) = &header_to_set.header else { continue };

            match header.key.as_str() {
                PSEUDO_HEADER_METHOD => pseudo_headers.method = Some(header_to_set),
                PSEUDO_HEADER_SCHEME => pseudo_headers.scheme = Some(header_to_set),
                PSEUDO_HEADER_AUTHORITY => pseudo_headers.authority = Some(header_to_set),
                PSEUDO_HEADER_PATH => pseudo_headers.path = Some(header_to_set),
                PSEUDO_HEADER_STATUS => pseudo_headers.status = Some(header_to_set),
                _ => {},
            }
        }

        pseudo_headers
    }
}

#[allow(clippy::str_to_string)]
#[allow(clippy::unnecessary_to_owned)]
pub fn apply_request_header_mutations<B>(
    req: &mut Request<B>,
    mutation: &HeaderMutation,
    mutation_rules: Option<&HeaderMutationRules>,
) -> Result<(), Error> {
    let pseudo_headers = PseudoHeaders::from(mutation);

    // Apply pseudo-headers
    if !pseudo_headers.is_empty() {
        // Handle :method separately as it's not part of URI
        if let Some(method_opt) = pseudo_headers.method {
            // NOTE: if mutation rules is not specified, we allow any modification. This is not the
            // same behavior as envoy, but is more permissive for users who don't set mutation rules.
            if mutation_rules.map(|r| r.is_modification_permitted(PSEUDO_HEADER_METHOD)).unwrap_or(true) {
                match try_extract_header_value_as_str(method_opt) {
                    Some(Ok(method)) => {
                        if let Ok(new_method) = http::Method::from_bytes(method.as_bytes()) {
                            *req.method_mut() = new_method;
                        } else {
                            warn!(target: "ext_proc", "Invalid method in request mutation: {}", method);
                        }
                    },
                    Some(Err(())) => {
                        warn!(target: "ext_proc", "Invalid UTF-8 in method mutation");
                    },
                    None => {},
                }
            }
        }

        // Handle URI-related pseudo-headers in a single pass
        if pseudo_headers.scheme.is_some() || pseudo_headers.authority.is_some() || pseudo_headers.path.is_some() {
            let mut parts = req.uri().clone().into_parts();

            if let Some(scheme_opt) = pseudo_headers.scheme {
                if mutation_rules.map(|r| r.is_modification_permitted(PSEUDO_HEADER_SCHEME)).unwrap_or(true) {
                    match try_extract_header_value_as_str(scheme_opt) {
                        Some(Ok(scheme)) => {
                            if let Ok(scheme) = Scheme::try_from(scheme) {
                                parts.scheme = Some(scheme);
                            } else {
                                warn!(target: "ext_proc", "Invalid scheme in request mutation: {}", scheme);
                            }
                        },
                        Some(Err(())) => {
                            warn!(target: "ext_proc", "Invalid UTF-8 in scheme mutation");
                        },
                        None => {},
                    }
                }
            }

            if let Some(authority_opt) = pseudo_headers.authority {
                if mutation_rules.map(|r| r.is_modification_permitted(PSEUDO_HEADER_AUTHORITY)).unwrap_or(true) {
                    match try_extract_header_value_as_str(authority_opt) {
                        Some(Ok(authority)) => {
                            if let Ok(authority) = Authority::try_from(authority) {
                                parts.authority = Some(authority);
                            } else {
                                warn!(target: "ext_proc", "Invalid authority in request mutation: {}", authority);
                            }
                        },
                        Some(Err(())) => {
                            warn!(target: "ext_proc", "Invalid UTF-8 in authority mutation");
                        },
                        None => {},
                    }
                }
            }

            if let Some(path_opt) = pseudo_headers.path {
                if mutation_rules.map(|r| r.is_modification_permitted(PSEUDO_HEADER_PATH)).unwrap_or(true) {
                    match try_extract_header_value_as_str(path_opt) {
                        Some(Ok(path)) => {
                            if let Ok(path_and_query) = PathAndQuery::try_from(path) {
                                parts.path_and_query = Some(path_and_query);
                            } else {
                                warn!(target: "ext_proc", "Invalid path in request mutation: {}", path);
                            }
                        },
                        Some(Err(())) => {
                            warn!(target: "ext_proc", "Invalid UTF-8 in path mutation");
                        },
                        None => {},
                    }
                }
            }

            if let Ok(new_uri) = Uri::from_parts(parts) {
                *req.uri_mut() = new_uri;
            }
        }
    }

    // Handle regular headers
    apply_header_mutations(req.headers_mut(), mutation, mutation_rules)
}

pub fn apply_response_header_mutations<B>(
    resp: &mut Response<B>,
    mutation: &HeaderMutation,
    mutation_rules: Option<&HeaderMutationRules>,
) -> Result<(), Error> {
    let pseudo_headers = PseudoHeaders::from(mutation);

    // Handle :status pseudo-header
    if let Some(status_opt) = pseudo_headers.status {
        if mutation_rules.map(|r| r.is_modification_permitted(PSEUDO_HEADER_STATUS)).unwrap_or(true) {
            match try_extract_header_value_as_str(status_opt) {
                Some(Ok(status_str)) => {
                    if let Ok(status_code) = status_str.parse::<u16>() {
                        if let Ok(new_code) = http::StatusCode::from_u16(status_code) {
                            *resp.status_mut() = new_code;
                        } else {
                            warn!(target: "ext_proc", "Invalid status code in response mutation: {}", status_code);
                        }
                    } else {
                        warn!(target: "ext_proc", "Failed to parse status code: {}", status_str);
                    }
                },
                Some(Err(())) => {
                    warn!(target: "ext_proc", "Invalid UTF-8 in status code mutation");
                },
                None => {},
            }
        }
    }

    // Handle regular headers
    apply_header_mutations(resp.headers_mut(), mutation, mutation_rules)
}

#[inline]
pub fn apply_trailer_mutations(
    trailers: &mut http::HeaderMap,
    mutation: &HeaderMutation,
    mutation_rules: Option<&HeaderMutationRules>,
) -> Result<(), Error> {
    apply_header_mutations(trailers, mutation, mutation_rules)
}

pub fn apply_header_mutations(
    headers: &mut http::HeaderMap,
    mutation: &HeaderMutation,
    mutation_rules: Option<&HeaderMutationRules>,
) -> Result<(), Error> {
    for header_to_remove in &mutation.remove_headers {
        if let Some(rules) = mutation_rules {
            if !rules.is_modification_permitted(header_to_remove) {
                if rules.disallow_is_error {
                    return Err(Error::from(format!(
                        "Header removal not permitted by configuration: {header_to_remove}"
                    )));
                }
                continue;
            }
        }
        if let Ok(header_name) = http::HeaderName::from_bytes(header_to_remove.as_bytes()) {
            headers.remove(&header_name);
        }
    }
    for header_to_set in &mutation.set_headers {
        let Some(header) = &header_to_set.header else { continue };
        if let Some(rules) = mutation_rules {
            if !rules.is_modification_permitted(&header.key) {
                if rules.disallow_is_error {
                    return Err(Error::from(format!(
                        "Header modification not permitted by configuration: {}",
                        header.key
                    )));
                }
                continue;
            }
        }
        let Ok(header_name) = http::HeaderName::from_bytes(header.key.as_bytes()) else { continue };
        let header_value = if header.raw_value.is_empty() {
            http::HeaderValue::from_str(&header.value)
        } else {
            http::HeaderValue::from_bytes(&header.raw_value)
        };
        let Ok(header_value) = header_value else { continue };
        match header_to_set.append_action() {
            HeaderAppendAction::AppendIfExistsOrAdd => {
                headers.append(header_name, header_value);
            },
            HeaderAppendAction::AddIfAbsent => {
                if !headers.contains_key(&header_name) {
                    headers.append(header_name, header_value);
                }
            },
            HeaderAppendAction::OverwriteIfExistsOrAdd => {
                headers.insert(header_name, header_value);
            },
            HeaderAppendAction::OverwriteIfExists => {
                if headers.contains_key(&header_name) {
                    headers.insert(header_name, header_value);
                }
            },
        }
    }
    Ok(())
}
