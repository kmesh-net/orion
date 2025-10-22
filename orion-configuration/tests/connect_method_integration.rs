// Copyright 2025 Orion Proxy Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");

#[cfg(test)]
mod connect_method_integration_tests {
    use http::Request;
    use orion_configuration::config::network_filters::http_connection_manager::route::{
        MethodMatcher, MethodSpecifier, RouteMatch,
    };
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::route::v3::{
        route_match::{ConnectMatcher as EnvoyConnectMatcher, PathSpecifier as EnvoyPathSpecifier},
        RouteMatch as EnvoyRouteMatch,
    };

    #[test]
    fn test_connect_matcher_converts_to_method_matcher() {
        // Test that Envoy's ConnectMatcher is properly converted to our MethodMatcher
        let envoy_route_match = EnvoyRouteMatch {
            path_specifier: Some(EnvoyPathSpecifier::ConnectMatcher(EnvoyConnectMatcher {})),
            ..Default::default()
        };

        let route_match = RouteMatch::try_from(envoy_route_match).unwrap();

        assert!(route_match.method_matcher.is_some(), "ConnectMatcher should create MethodMatcher");
        assert!(route_match.path_matcher.is_none(), "ConnectMatcher should not create PathMatcher");

        if let Some(method_matcher) = route_match.method_matcher {
            assert!(matches!(method_matcher.specifier, MethodSpecifier::Connect));
        }
    }

    #[test]
    fn test_connect_method_routing_priority() {
        // Test that method matching has highest priority in routing
        let route_match = RouteMatch {
            method_matcher: Some(MethodMatcher {
                specifier: MethodSpecifier::Connect
            }),
            path_matcher: Some(
                orion_configuration::config::network_filters::http_connection_manager::route::PathMatcher {
                    specifier: orion_configuration::config::network_filters::http_connection_manager::route::PathSpecifier::Prefix("/api".into()),
                    ignore_case: false,
                }
            ),
            headers: Vec::new(),
            query_parameters: Vec::new(),
        };

        // CONNECT method should match even with path that would otherwise match
        let req_connect = Request::builder().method(http::Method::CONNECT).uri("/api/test").body(()).unwrap();
        let result = route_match.match_request(&req_connect);
        assert!(result.matched(), "CONNECT method should match regardless of path");

        // Non-CONNECT method should fail even with matching path
        let req_get = Request::builder().method(http::Method::GET).uri("/api/test").body(()).unwrap();
        let result = route_match.match_request(&req_get);
        assert!(!result.matched(), "GET method should not match when MethodMatcher expects CONNECT");
    }

    #[test]
    fn test_waypoint_connect_route_pattern() {
        // Test the typical Istio waypoint CONNECT route pattern
        // In Istio waypoint, CONNECT is used for HTTP tunneling

        // Create a route match with only ConnectMatcher (no path matching)
        let envoy_route_match = EnvoyRouteMatch {
            path_specifier: Some(EnvoyPathSpecifier::ConnectMatcher(EnvoyConnectMatcher {})),
            ..Default::default()
        };
        let route_match = RouteMatch::try_from(envoy_route_match).unwrap();

        // CONNECT request to any destination
        let req = Request::builder().method(http::Method::CONNECT).uri("example.com:443").body(()).unwrap();
        let result = route_match.match_request(&req);
        assert!(result.matched(), "Should match CONNECT to any destination");

        // CONNECT with path (for HTTP/2+ extended CONNECT)
        let req = Request::builder().method(http::Method::CONNECT).uri("/connect/path").body(()).unwrap();
        let result = route_match.match_request(&req);
        assert!(result.matched(), "Should match CONNECT with path");
    }

    #[test]
    fn test_method_matcher_all_http_methods() {
        // Verify all HTTP methods work correctly with MethodMatcher
        let test_cases = vec![
            (http::Method::CONNECT, MethodSpecifier::Connect, true),
            (http::Method::GET, MethodSpecifier::Get, true),
            (http::Method::POST, MethodSpecifier::Post, true),
            (http::Method::PUT, MethodSpecifier::Put, true),
            (http::Method::DELETE, MethodSpecifier::Delete, true),
            (http::Method::PATCH, MethodSpecifier::Patch, true),
            (http::Method::HEAD, MethodSpecifier::Head, true),
            (http::Method::OPTIONS, MethodSpecifier::Options, true),
            (http::Method::GET, MethodSpecifier::Post, false), // Mismatch
            (http::Method::POST, MethodSpecifier::Get, false), // Mismatch
        ];

        for (method, specifier, should_match) in test_cases {
            let method_matcher = MethodMatcher { specifier };
            let result = method_matcher.matches(Some(&method));

            if should_match {
                assert!(
                    result.matched(),
                    "MethodMatcher with {:?} should match {:?}",
                    method_matcher.specifier,
                    method
                );
            } else {
                assert!(
                    !result.matched(),
                    "MethodMatcher with {:?} should NOT match {:?}",
                    method_matcher.specifier,
                    method
                );
            }
        }
    }

    #[test]
    fn test_regular_routes_without_method_matcher() {
        // Test that regular routes without MethodMatcher still work
        let route_match = RouteMatch {
            method_matcher: None,
            path_matcher: Some(
                orion_configuration::config::network_filters::http_connection_manager::route::PathMatcher {
                    specifier: orion_configuration::config::network_filters::http_connection_manager::route::PathSpecifier::Prefix("/api".into()),
                    ignore_case: false,
                }
            ),
            headers: Vec::new(),
            query_parameters: Vec::new(),
        };

        // GET request should match based on path
        let req = Request::builder().method(http::Method::GET).uri("/api/users").body(()).unwrap();
        let result = route_match.match_request(&req);
        assert!(result.matched(), "Regular route should match GET with matching path");

        // POST request should also match
        let req = Request::builder().method(http::Method::POST).uri("/api/users").body(()).unwrap();
        let result = route_match.match_request(&req);
        assert!(result.matched(), "Regular route should match POST with matching path");

        // CONNECT request should also match (no method restriction)
        let req = Request::builder().method(http::Method::CONNECT).uri("/api/users").body(()).unwrap();
        let result = route_match.match_request(&req);
        assert!(result.matched(), "Regular route should match CONNECT with matching path");
    }
}
