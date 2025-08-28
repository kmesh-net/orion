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

use super::upgrade_utils;
use crate::{listeners::synthetic_http_response::SyntheticHttpResponse, PolyBody};
use http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, Response};
use orion_configuration::config::network_filters::http_connection_manager::XffSettings;
use orion_http_header::*;
use std::net::{IpAddr, SocketAddr};

const HOP_BY_HOP_HEADERS: &[HeaderName] = &[
    header::CONNECTION,
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
];
pub fn apply_prerouting_functions<T>(request: &mut Request<T>, downstream_addr: SocketAddr, xff_settings: XffSettings) {
    process_xff_headers(request, downstream_addr, xff_settings);
}

pub fn apply_preflight_functions<T>(request: &mut Request<T>) -> Option<Response<PolyBody>> {
    if let Some(direct_response) = filter_disallowed_requests(request) {
        return Some(direct_response);
    }
    strip_hop_headers(request.headers_mut());
    None
}

fn filter_disallowed_requests<T>(request: &Request<T>) -> Option<Response<PolyBody>> {
    if request.method() == Method::CONNECT {
        return Some(SyntheticHttpResponse::forbidden("CONNECT not permitted").into_response(request.version()));
    }
    if let Some(connection_header) = request.headers().get(header::CONNECTION) {
        if upgrade_utils::is_upgrade_connection(connection_header.to_str().ok()?) {
            return Some(SyntheticHttpResponse::forbidden("upgrade not permitted").into_response(request.version()));
        }
    }
    None
}

fn strip_hop_headers(headers: &mut HeaderMap) {
    for header in HOP_BY_HOP_HEADERS {
        headers.remove(header);
    }
}

fn process_xff_headers<T>(request: &mut Request<T>, downstream_addr: SocketAddr, xff_settings: XffSettings) {
    let headers = request.headers_mut();
    let downstream_is_internal = is_internal_ip(downstream_addr.ip());
    let downstream_is_external = !downstream_is_internal;

    if downstream_is_external {
        headers.remove(&X_ENVOY_EXTERNAL_ADDRESS);
        headers.remove(&X_ENVOY_INTERNAL);
    }

    let transparent_mode = !xff_settings.use_remote_address && xff_settings.skip_xff_append;
    if transparent_mode {
        return;
    }

    let existing_xff = headers.get(X_FORWARDED_FOR).and_then(|value| value.to_str().ok());
    let (trusted_client_address, xff_contains_single_ip) =
        determine_trusted_client_address(existing_xff, downstream_addr, xff_settings);
    let xff_contains_single_internal_ip = xff_contains_single_ip && is_internal_ip(trusted_client_address);
    let xff_contains_single_external_ip = xff_contains_single_ip && !is_internal_ip(trusted_client_address);
    let has_incoming_xff = existing_xff.is_some();

    let should_update_xff = xff_settings.use_remote_address && !xff_settings.skip_xff_append;
    let should_set_envoy_external = xff_settings.use_remote_address
        && !headers.contains_key(X_ENVOY_EXTERNAL_ADDRESS)
        && !is_internal_ip(trusted_client_address);
    let should_set_envoy_internal = (xff_settings.use_remote_address && downstream_is_internal && !has_incoming_xff)
        || xff_contains_single_internal_ip;
    let should_mark_envoy_internal_false =
        (xff_settings.use_remote_address && downstream_is_external && !has_incoming_xff)
            || xff_contains_single_external_ip;

    if should_update_xff {
        if let Ok(updated_xff) = HeaderValue::from_str(&append_hop_to_xff(existing_xff, downstream_addr.ip())) {
            headers.insert(X_FORWARDED_FOR, updated_xff);
        }
    }
    if should_set_envoy_external {
        if let Ok(envoy_external_addr) = HeaderValue::from_str(&trusted_client_address.to_string()) {
            headers.insert(X_ENVOY_EXTERNAL_ADDRESS, envoy_external_addr);
        }
    }
    if should_set_envoy_internal {
        headers.insert(X_ENVOY_INTERNAL, HeaderValue::from_static("true"));
    } else if should_mark_envoy_internal_false {
        headers.insert(X_ENVOY_INTERNAL, HeaderValue::from_static("false"));
    }
}

fn is_internal_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => ipv4.is_private() || ipv4.is_loopback(),
        IpAddr::V6(ipv6) => {
            ipv6.segments()[0] & 0xfe00 == 0xfc00
                || ipv6.is_unspecified()
                || ipv6.is_loopback()
                || ipv6.is_unique_local()
                || ipv6.is_unicast_link_local()
        },
    }
}

fn append_hop_to_xff(existing_xff: Option<&str>, downstream_ip: IpAddr) -> String {
    let ip_str = downstream_ip.to_string();
    match existing_xff {
        Some(existing_str) => {
            if existing_str.is_empty() {
                ip_str
            } else {
                format!("{existing_str}, {ip_str}")
            }
        },
        _ => ip_str,
    }
}

fn determine_trusted_client_address(
    existing_xff: Option<&str>,
    downstream_addr: SocketAddr,
    xff_settings: XffSettings,
) -> (IpAddr, bool) {
    let mut trusted_client_address = downstream_addr.ip();
    let mut xff_contains_single_ip = false;
    let xff_ips = existing_xff
        .map_or_else(Vec::new, |value| value.split(',').filter_map(|ip_str| ip_str.trim().parse().ok()).collect());
    let num_xff_ips = xff_ips.len();
    if !xff_settings.use_remote_address && !xff_ips.is_empty() {
        if xff_settings.xff_num_trusted_hops > 0 {
            let required_index_from_right = xff_settings.xff_num_trusted_hops as usize + 1;
            if num_xff_ips >= required_index_from_right {
                trusted_client_address = xff_ips[num_xff_ips - required_index_from_right];
            }
        } else {
            trusted_client_address = xff_ips[num_xff_ips - 1];
            xff_contains_single_ip = num_xff_ips == 1;
        }
    } else if xff_settings.use_remote_address && xff_settings.xff_num_trusted_hops > 0 && !xff_ips.is_empty() {
        let required_index_from_right = xff_settings.xff_num_trusted_hops as usize;
        if num_xff_ips >= required_index_from_right {
            trusted_client_address = xff_ips[num_xff_ips - required_index_from_right];
        }
    }
    (trusted_client_address, xff_contains_single_ip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn test_example_1_edge_proxy_no_trusted() {
        let mut request = Request::new(());
        request.headers_mut().insert("x-forwarded-for", "203.0.113.128, 203.0.113.10, 203.0.113.1".parse().unwrap());
        let downstream_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 5)), 80);
        let xff_settings = XffSettings { use_remote_address: true, skip_xff_append: false, xff_num_trusted_hops: 0 };

        process_xff_headers(&mut request, downstream_addr, xff_settings);

        assert_eq!(request.headers().get("x-envoy-external-address").unwrap(), "192.0.2.5");
        assert_eq!(
            request.headers().get("x-forwarded-for").unwrap(),
            "203.0.113.128, 203.0.113.10, 203.0.113.1, 192.0.2.5"
        );
        assert!(request.headers().get("x-envoy-internal").is_none());
    }

    #[test]
    fn test_example_2_internal_proxy_from_edge() {
        let mut request = Request::new(());
        request
            .headers_mut()
            .insert("x-forwarded-for", "203.0.113.128, 203.0.113.10, 203.0.113.1, 192.0.2.5".parse().unwrap());
        let downstream_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 11, 12, 13)), 80);
        let xff_settings = XffSettings { use_remote_address: false, skip_xff_append: false, xff_num_trusted_hops: 0 };

        process_xff_headers(&mut request, downstream_addr, xff_settings);

        assert!(request.headers().get("x-envoy-external-address").is_none());
        assert_eq!(
            request.headers().get("x-forwarded-for").unwrap(),
            "203.0.113.128, 203.0.113.10, 203.0.113.1, 192.0.2.5"
        );
        assert!(request.headers().get("x-envoy-internal").is_none());
    }

    #[test]
    fn test_example_3_edge_proxy_two_trusted_proxies() {
        let mut request = Request::new(());
        request.headers_mut().insert("x-forwarded-for", "203.0.113.128, 203.0.113.10, 203.0.113.1".parse().unwrap());
        let downstream_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 5)), 80);
        let xff_settings = XffSettings { use_remote_address: true, skip_xff_append: false, xff_num_trusted_hops: 2 };

        process_xff_headers(&mut request, downstream_addr, xff_settings);

        assert_eq!(request.headers().get("x-envoy-external-address").unwrap(), "203.0.113.10");
        assert_eq!(
            request.headers().get("x-forwarded-for").unwrap(),
            "203.0.113.128, 203.0.113.10, 203.0.113.1, 192.0.2.5"
        );
        assert!(request.headers().get("x-envoy-internal").is_none());
    }

    #[test]
    fn test_example_4_internal_proxy_from_trusted_edge() {
        let mut request = Request::new(());
        request
            .headers_mut()
            .insert("x-forwarded-for", "203.0.113.128, 203.0.113.10, 203.0.113.1, 192.0.2.5".parse().unwrap());
        let downstream_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 11, 12, 13)), 80);
        let xff_settings = XffSettings { use_remote_address: false, skip_xff_append: false, xff_num_trusted_hops: 0 };

        process_xff_headers(&mut request, downstream_addr, xff_settings);

        assert!(request.headers().get("x-envoy-external-address").is_none());
        assert_eq!(
            request.headers().get("x-forwarded-for").unwrap(),
            "203.0.113.128, 203.0.113.10, 203.0.113.1, 192.0.2.5"
        );
        assert!(request.headers().get("x-envoy-internal").is_none());
    }

    #[test]
    fn test_example_5_internal_proxy_from_internal_client_no_xff() {
        let mut request = Request::new(());
        let downstream_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 20, 30, 40)), 80);
        let xff_settings = XffSettings { use_remote_address: false, skip_xff_append: false, xff_num_trusted_hops: 0 };

        process_xff_headers(&mut request, downstream_addr, xff_settings);

        assert!(request.headers().get("x-envoy-external-address").is_none());
        assert!(request.headers().get("x-forwarded-for").is_none());
        assert!(request.headers().get("x-envoy-internal").is_none());
    }

    #[test]
    fn test_example_6_internal_proxy_from_another_proxy_with_xff() {
        let mut request = Request::new(());
        request.headers_mut().insert("x-forwarded-for", "10.20.30.40".parse().unwrap());
        let downstream_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 20, 30, 50)), 80);
        let xff_settings = XffSettings { use_remote_address: false, skip_xff_append: false, xff_num_trusted_hops: 0 };

        process_xff_headers(&mut request, downstream_addr, xff_settings);

        assert!(request.headers().get("x-envoy-external-address").is_none());
        assert_eq!(request.headers().get("x-forwarded-for").unwrap(), "10.20.30.40");
        //assert_eq!(request.headers().get("x-envoy-internal").unwrap(), "true");
    }
}
