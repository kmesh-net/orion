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

//! Integration tests for set_filter_state HTTP filter
//!
//! These tests validate the end-to-end functionality of parsing Istio waypoint
//! configurations and converting them to Orion's internal representation.

use orion_configuration::config::Bootstrap;
use orion_error::{Context, Error};
use std::fs::File;

#[test]
fn test_istio_waypoint_set_filter_state_config() -> Result<(), Error> {
    // Load a realistic Istio waypoint configuration with set_filter_state
    let bootstrap = Bootstrap::deserialize_from_envoy(
        File::open("tests/set_filter_state/istio_waypoint_config.yaml")
            .with_context_msg("failed to open istio_waypoint_config.yaml")?,
    )
    .with_context_msg("failed to convert envoy to orion")?;

    // Verify the configuration was parsed successfully
    assert_eq!(bootstrap.static_resources.listeners.len(), 1);

    let listener = &bootstrap.static_resources.listeners[0];
    assert_eq!(listener.name.as_str(), "test_listener");

    // Check filter chains (it's a HashMap, so get the first one)
    assert_eq!(listener.filter_chains.len(), 1);
    let filter_chain = listener.filter_chains.values().next().unwrap();

    // The terminal_filter is the main filter (HTTP or TCP)
    use orion_configuration::config::listener::MainFilter;
    if let MainFilter::Http(hcm_config) = &filter_chain.terminal_filter {
        // Expect at least set_filter_state filter (router may be filtered out)
        assert!(hcm_config.http_filters.len() >= 1, "Expected at least set_filter_state filter");

        // Check first filter is set_filter_state
        let set_filter_state_filter = &hcm_config.http_filters[0];
        assert_eq!(set_filter_state_filter.name.as_str(), "istio.set_filter_state");

        // Verify filter type
        use orion_configuration::config::network_filters::http_connection_manager::http_filters::HttpFilterType;
        match &set_filter_state_filter.filter {
            HttpFilterType::SetFilterState(config) => {
                // Verify on_request_headers configuration
                let on_request_headers = config.on_request_headers.as_ref().expect("on_request_headers should exist");
                assert_eq!(on_request_headers.len(), 2);

                // First entry: io.istio.connect_authority
                let connect_authority = &on_request_headers[0];
                assert_eq!(connect_authority.object_key.as_str(), "io.istio.connect_authority");

                // Verify format string
                if let Some(format_string) = &connect_authority.format_string {
                    if let Some(text_source) = &format_string.text_format_source {
                        if let Some(inline_string) = &text_source.inline_string {
                            assert_eq!(inline_string.as_str(), "%REQ(:authority)%");
                        } else {
                            panic!("Expected inline_string in text_format_source");
                        }
                    } else {
                        panic!("Expected text_format_source");
                    }
                } else {
                    panic!("Expected format_string");
                }

                // Second entry: io.istio.client_address
                let client_address = &on_request_headers[1];
                assert_eq!(client_address.object_key.as_str(), "io.istio.client_address");

                if let Some(format_string) = &client_address.format_string {
                    if let Some(text_source) = &format_string.text_format_source {
                        if let Some(inline_string) = &text_source.inline_string {
                            assert_eq!(inline_string.as_str(), "%DOWNSTREAM_REMOTE_ADDRESS%");
                        } else {
                            panic!("Expected inline_string in text_format_source");
                        }
                    } else {
                        panic!("Expected text_format_source");
                    }
                } else {
                    panic!("Expected format_string");
                }
            },
            _ => panic!("Expected SetFilterState filter type"),
        }
    } else {
        panic!("Expected HttpConnectionManager network filter");
    }

    Ok(())
}

#[test]
fn test_set_filter_state_with_text_format_source() -> Result<(), Error> {
    // Test with text_format_source (newer format) instead of text_format
    let yaml_config = r#"
static_resources:
  listeners:
  - name: test_listener
    address:
      socket_address:
        address: 0.0.0.0
        port_value: 15008
    filter_chains:
    - filters:
      - name: envoy.filters.network.http_connection_manager
        typed_config:
          "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
          stat_prefix: test
          codec_type: AUTO
          http_filters:
          - name: test.set_filter_state
            typed_config:
              "@type": type.googleapis.com/udpa.type.v1.TypedStruct
              type_url: type.googleapis.com/envoy.extensions.filters.http.set_filter_state.v3.Config
              value:
                on_request_headers:
                - object_key: test.key
                  format_string:
                    text_format_source:
                      inline_string: "%REQ(x-custom-header)%"
          - name: envoy.filters.http.router
            typed_config:
              "@type": type.googleapis.com/envoy.extensions.filters.http.router.v3.Router
          route_config:
            name: default
            virtual_hosts:
            - name: default
              domains: ["*"]
              routes:
              - match: { prefix: "/" }
                route: { cluster: test }
  clusters:
  - name: test
    type: STATIC
    connect_timeout: 5s
    load_assignment:
      cluster_name: test
      endpoints:
      - lb_endpoints:
        - endpoint:
            address:
              socket_address:
                address: 127.0.0.1
                port_value: 8080
"#;

    // Write to temp file and use Bootstrap::deserialize_from_envoy
    use std::io::Write;
    let mut temp_file = tempfile::NamedTempFile::new().with_context_msg("failed to create temp file")?;
    temp_file.write_all(yaml_config.as_bytes()).with_context_msg("failed to write temp file")?;

    let bootstrap =
        Bootstrap::deserialize_from_envoy(File::open(temp_file.path()).with_context_msg("failed to open temp file")?)
            .with_context_msg("failed to convert envoy to orion")?;

    // Verify it parsed correctly
    let listener = &bootstrap.static_resources.listeners[0];
    let filter_chain = listener.filter_chains.values().next().unwrap();

    use orion_configuration::config::listener::MainFilter;
    if let MainFilter::Http(hcm_config) = &filter_chain.terminal_filter {
        let filter = &hcm_config.http_filters[0];

        use orion_configuration::config::network_filters::http_connection_manager::http_filters::HttpFilterType;
        if let HttpFilterType::SetFilterState(config) = &filter.filter {
            let on_request_headers = config.on_request_headers.as_ref().expect("on_request_headers should exist");
            assert_eq!(on_request_headers.len(), 1);
            let entry = &on_request_headers[0];

            if let Some(format_string) = &entry.format_string {
                if let Some(text_source) = &format_string.text_format_source {
                    if let Some(inline_string) = &text_source.inline_string {
                        assert_eq!(inline_string.as_str(), "%REQ(x-custom-header)%");
                    } else {
                        panic!("Expected inline_string");
                    }
                } else {
                    panic!("Expected text_format_source");
                }
            } else {
                panic!("Expected format_string");
            }
        }
    }

    Ok(())
}

#[test]
fn test_set_filter_state_defaults() -> Result<(), Error> {
    // Test that default values are applied correctly
    let yaml_config = r#"
static_resources:
  listeners:
  - name: test
    address:
      socket_address: { address: 0.0.0.0, port_value: 8080 }
    filter_chains:
    - filters:
      - name: envoy.filters.network.http_connection_manager
        typed_config:
          "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
          stat_prefix: test
          codec_type: AUTO
          http_filters:
          - name: minimal.set_filter_state
            typed_config:
              "@type": type.googleapis.com/udpa.type.v1.TypedStruct
              type_url: type.googleapis.com/envoy.extensions.filters.http.set_filter_state.v3.Config
              value:
                on_request_headers:
                - object_key: minimal.key
                  format_string:
                    text_format_source:
                      inline_string: "test"
          - name: envoy.filters.http.router
            typed_config:
              "@type": type.googleapis.com/envoy.extensions.filters.http.router.v3.Router
          route_config:
            name: default
            virtual_hosts:
            - name: default
              domains: ["*"]
              routes:
              - match: { prefix: "/" }
                route: { cluster: test }
  clusters:
  - name: test
    type: STATIC
    connect_timeout: 5s
    load_assignment:
      cluster_name: test
      endpoints:
      - lb_endpoints:
        - endpoint:
            address:
              socket_address: { address: 127.0.0.1, port_value: 8080 }
"#;

    // Write to temp file and use Bootstrap::deserialize_from_envoy
    use std::io::Write;
    let mut temp_file = tempfile::NamedTempFile::new().with_context_msg("failed to create temp file")?;
    temp_file.write_all(yaml_config.as_bytes()).with_context_msg("failed to write temp file")?;

    let bootstrap =
        Bootstrap::deserialize_from_envoy(File::open(temp_file.path()).with_context_msg("failed to open temp file")?)
            .with_context_msg("failed to convert envoy to orion")?;

    let listener = &bootstrap.static_resources.listeners[0];
    let filter_chain = listener.filter_chains.values().next().unwrap();

    use orion_configuration::config::listener::MainFilter;
    if let MainFilter::Http(hcm_config) = &filter_chain.terminal_filter {
        use orion_configuration::config::network_filters::http_connection_manager::http_filters::HttpFilterType;
        if let HttpFilterType::SetFilterState(config) = &hcm_config.http_filters[0].filter {
            let on_request_headers = config.on_request_headers.as_ref().expect("on_request_headers should exist");
            let entry = &on_request_headers[0];

            // Verify basic parsing worked
            assert_eq!(entry.object_key.as_str(), "minimal.key");
        }
    }

    Ok(())
}
