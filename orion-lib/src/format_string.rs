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

//! Format String Evaluator - Envoy-style format string substitution
//!
//! This module implements Envoy's format string substitution mechanism used in
//! access logging, filter state values, and other dynamic string generation.
//!
//! # Supported Command Operators
//!
//! - `%REQ(header_name)%` - Request header value
//! - `%DOWNSTREAM_REMOTE_ADDRESS%` - Client IP address
//! - `%DOWNSTREAM_LOCAL_ADDRESS%` - Local address connection was made to
//! - `%UPSTREAM_HOST%` - Upstream host IP:port
//! - `%PROTOCOL%` - Protocol (HTTP/1.1, HTTP/2, HTTP/3)
//! - `%REQ_METHOD%` - HTTP request method (GET, POST, etc.)
//! - `%REQ_PATH%` - Request path
//! - `%REQ_AUTHORITY%` - :authority header (or Host)
//! - `%ROUTE_NAME%` - Name of the matched route
//!
//! # Format String Syntax
//!
//! Format strings contain literal text mixed with command operators:
//! ```text
//! "Host: %REQ(:authority)%, IP: %DOWNSTREAM_REMOTE_ADDRESS%"
//! ```
//!
//! Command operators support optional parameters:
//! - Max length: `%REQ(user-agent):100%` (truncate to 100 chars)
//! - Missing value handling: Returns "-" for missing values
//!
//! # Example
//! ```rust,ignore
//! use orion_lib::format_string::{FormatStringEvaluator, RequestContext};
//!
//! let evaluator = FormatStringEvaluator::new();
//! let ctx = RequestContext::new()
//!     .with_header(":authority", "example.com")
//!     .with_downstream_remote_address("192.168.1.1:54321");
//!
//! let result = evaluator.evaluate(
//!     "Connecting to %REQ(:authority)% from %DOWNSTREAM_REMOTE_ADDRESS%",
//!     &ctx
//! );
//! assert_eq!(result, "Connecting to example.com from 192.168.1.1:54321");
//! ```

use compact_str::CompactString;
use std::collections::HashMap;
use std::net::SocketAddr;

/// Request context containing all information needed for format string evaluation
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// Request headers (lowercase names)
    headers: HashMap<CompactString, CompactString>,
    /// Downstream (client) remote address
    downstream_remote_address: Option<SocketAddr>,
    /// Downstream local address (what the client connected to)
    downstream_local_address: Option<SocketAddr>,
    /// Upstream host address
    upstream_host: Option<CompactString>,
    /// Protocol (HTTP/1.1, HTTP/2, etc.)
    protocol: Option<CompactString>,
    /// Request method (GET, POST, etc.)
    method: Option<CompactString>,
    /// Request path
    path: Option<CompactString>,
    /// Route name
    route_name: Option<CompactString>,
}

impl RequestContext {
    /// Creates a new empty request context
    pub fn new() -> Self {
        Self {
            headers: HashMap::new(),
            downstream_remote_address: None,
            downstream_local_address: None,
            upstream_host: None,
            protocol: None,
            method: None,
            path: None,
            route_name: None,
        }
    }

    /// Sets a request header (name will be lowercased)
    pub fn with_header(mut self, name: impl Into<CompactString>, value: impl Into<CompactString>) -> Self {
        let name = name.into();
        self.headers.insert(name.to_lowercase().into(), value.into());
        self
    }

    /// Sets the downstream remote address
    pub fn with_downstream_remote_address(mut self, addr: SocketAddr) -> Self {
        self.downstream_remote_address = Some(addr);
        self
    }

    /// Sets the downstream local address
    pub fn with_downstream_local_address(mut self, addr: SocketAddr) -> Self {
        self.downstream_local_address = Some(addr);
        self
    }

    /// Sets the upstream host
    pub fn with_upstream_host(mut self, host: impl Into<CompactString>) -> Self {
        self.upstream_host = Some(host.into());
        self
    }

    /// Sets the protocol
    pub fn with_protocol(mut self, protocol: impl Into<CompactString>) -> Self {
        self.protocol = Some(protocol.into());
        self
    }

    /// Sets the request method
    pub fn with_method(mut self, method: impl Into<CompactString>) -> Self {
        self.method = Some(method.into());
        self
    }

    /// Sets the request path
    pub fn with_path(mut self, path: impl Into<CompactString>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Sets the route name
    pub fn with_route_name(mut self, route_name: impl Into<CompactString>) -> Self {
        self.route_name = Some(route_name.into());
        self
    }

    /// Gets a header value (case-insensitive)
    pub fn get_header(&self, name: &str) -> Option<&CompactString> {
        self.headers.get(&name.to_lowercase() as &str)
    }

    /// Gets the :authority header (or falls back to Host)
    pub fn get_authority(&self) -> Option<&CompactString> {
        self.get_header(":authority").or_else(|| self.get_header("host"))
    }
}

impl Default for RequestContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Format string evaluator that substitutes command operators with actual values
#[derive(Debug, Clone)]
pub struct FormatStringEvaluator {}

impl FormatStringEvaluator {
    /// Creates a new format string evaluator
    pub fn new() -> Self {
        Self {}
    }

    /// Evaluates a format string with the given request context
    ///
    /// # Arguments
    /// * `format` - Format string containing command operators
    /// * `ctx` - Request context with values to substitute
    ///
    /// # Returns
    /// String with all command operators replaced with actual values
    pub fn evaluate(&self, format: &str, ctx: &RequestContext) -> CompactString {
        let mut out = String::with_capacity(format.len());
        let bytes = format.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'%' {
                out.push(bytes[i] as char);
                i += 1;
                continue;
            }

            let mut j = i + 1;

            let start_cmd = j;
            while j < bytes.len() {
                let c = bytes[j];
                if (b'A'..=b'Z').contains(&c) || c == b'_' {
                    j += 1;
                } else {
                    break;
                }
            }

            if j == start_cmd {
                out.push('%');
                i += 1;
                continue;
            }

            let command = &format[start_cmd..j];

            let mut arg: Option<&str> = None;
            if j < bytes.len() && bytes[j] == b'(' {
                j += 1;
                let start_arg = j;
                while j < bytes.len() && bytes[j] != b')' {
                    j += 1;
                }
                if j >= bytes.len() {
                    out.push('%');
                    i += 1;
                    continue;
                }
                arg = Some(&format[start_arg..j]);
                j += 1;
            }

            let mut max_len: Option<usize> = None;
            if j < bytes.len() && bytes[j] == b':' {
                j += 1;
                let start_num = j;
                while j < bytes.len() && (b'0'..=b'9').contains(&bytes[j]) {
                    j += 1;
                }
                if j == start_num {
                    out.push('%');
                    i += 1;
                    continue;
                }
                if let Ok(n) = format[start_num..j].parse::<usize>() {
                    max_len = Some(n);
                }
            }

            if j < bytes.len() && bytes[j] == b'%' {
                let value = self.evaluate_command(command, arg, ctx);
                let truncated = self.apply_max_length(value, max_len);
                out.push_str(&truncated);
                i = j + 1;
                continue;
            } else {
                out.push('%');
                i += 1;
                continue;
            }
        }

        out.into()
    }

    /// Evaluates a single command operator
    fn evaluate_command(&self, command: &str, arg: Option<&str>, ctx: &RequestContext) -> CompactString {
        match command {
            "REQ" => {
                // %REQ(header_name)%
                if let Some(header_name) = arg {
                    ctx.get_header(header_name).cloned().unwrap_or_else(|| "-".into())
                } else {
                    "-".into()
                }
            },
            "DOWNSTREAM_REMOTE_ADDRESS" => {
                // %DOWNSTREAM_REMOTE_ADDRESS%
                ctx.downstream_remote_address.map(|addr| addr.to_string().into()).unwrap_or_else(|| "-".into())
            },
            "DOWNSTREAM_LOCAL_ADDRESS" => {
                // %DOWNSTREAM_LOCAL_ADDRESS%
                ctx.downstream_local_address.map(|addr| addr.to_string().into()).unwrap_or_else(|| "-".into())
            },
            "UPSTREAM_HOST" => {
                // %UPSTREAM_HOST%
                ctx.upstream_host.clone().unwrap_or_else(|| "-".into())
            },
            "PROTOCOL" => {
                // %PROTOCOL%
                ctx.protocol.clone().unwrap_or_else(|| "-".into())
            },
            "REQ_METHOD" => {
                // %REQ_METHOD%
                ctx.method.clone().unwrap_or_else(|| "-".into())
            },
            "REQ_PATH" => {
                // %REQ_PATH%
                ctx.path.clone().unwrap_or_else(|| "-".into())
            },
            "REQ_AUTHORITY" => {
                // %REQ_AUTHORITY% - alias for :authority header
                ctx.get_authority().cloned().unwrap_or_else(|| "-".into())
            },
            "ROUTE_NAME" => {
                // %ROUTE_NAME%
                ctx.route_name.clone().unwrap_or_else(|| "-".into())
            },
            _ => {
                // Unknown command - return as-is with % markers
                format!("%{}%", command).into()
            },
        }
    }

    /// Applies max length truncation to a value
    fn apply_max_length(&self, value: CompactString, max_len: Option<usize>) -> CompactString {
        if let Some(max_len) = max_len {
            if let Some((end_index, _)) = value.char_indices().nth(max_len) {
                return value[..end_index].into();
            }
        }
        value
    }
}

impl Default for FormatStringEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_evaluator_new() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new();
        let result = evaluator.evaluate("hello world", &ctx);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_req_header() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_header(":authority", "example.com").with_header("user-agent", "test/1.0");

        let result = evaluator.evaluate("Host: %REQ(:authority)%", &ctx);
        assert_eq!(result, "Host: example.com");

        let result = evaluator.evaluate("UA: %REQ(user-agent)%", &ctx);
        assert_eq!(result, "UA: test/1.0");
    }

    #[test]
    fn test_req_header_missing() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new();

        let result = evaluator.evaluate("Host: %REQ(:authority)%", &ctx);
        assert_eq!(result, "Host: -");
    }

    #[test]
    fn test_req_header_case_insensitive() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_header("Content-Type", "application/json");

        let result = evaluator.evaluate("%REQ(content-type)%", &ctx);
        assert_eq!(result, "application/json");

        let result = evaluator.evaluate("%REQ(CONTENT-TYPE)%", &ctx);
        assert_eq!(result, "application/json");
    }

    #[test]
    fn test_downstream_remote_address() {
        let evaluator = FormatStringEvaluator::new();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 54321);
        let ctx = RequestContext::new().with_downstream_remote_address(addr);

        let result = evaluator.evaluate("Client: %DOWNSTREAM_REMOTE_ADDRESS%", &ctx);
        assert_eq!(result, "Client: 192.168.1.100:54321");
    }

    #[test]
    fn test_downstream_local_address() {
        let evaluator = FormatStringEvaluator::new();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 8080);
        let ctx = RequestContext::new().with_downstream_local_address(addr);

        let result = evaluator.evaluate("Local: %DOWNSTREAM_LOCAL_ADDRESS%", &ctx);
        assert_eq!(result, "Local: 10.0.0.1:8080");
    }

    #[test]
    fn test_upstream_host() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_upstream_host("backend.example.com:8080");

        let result = evaluator.evaluate("Upstream: %UPSTREAM_HOST%", &ctx);
        assert_eq!(result, "Upstream: backend.example.com:8080");
    }

    #[test]
    fn test_protocol() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_protocol("HTTP/2");

        let result = evaluator.evaluate("Protocol: %PROTOCOL%", &ctx);
        assert_eq!(result, "Protocol: HTTP/2");
    }

    #[test]
    fn test_req_method() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_method("POST");

        let result = evaluator.evaluate("Method: %REQ_METHOD%", &ctx);
        assert_eq!(result, "Method: POST");
    }

    #[test]
    fn test_req_path() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_path("/api/v1/users");

        let result = evaluator.evaluate("Path: %REQ_PATH%", &ctx);
        assert_eq!(result, "Path: /api/v1/users");
    }

    #[test]
    fn test_req_authority() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_header(":authority", "api.example.com");

        let result = evaluator.evaluate("Authority: %REQ_AUTHORITY%", &ctx);
        assert_eq!(result, "Authority: api.example.com");
    }

    #[test]
    fn test_req_authority_fallback_to_host() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_header("host", "www.example.com");

        let result = evaluator.evaluate("Authority: %REQ_AUTHORITY%", &ctx);
        assert_eq!(result, "Authority: www.example.com");
    }

    #[test]
    fn test_route_name() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_route_name("my-route");

        let result = evaluator.evaluate("Route: %ROUTE_NAME%", &ctx);
        assert_eq!(result, "Route: my-route");
    }

    #[test]
    fn test_max_length_truncation() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_header("user-agent", "Mozilla/5.0 (Very Long User Agent String)");

        let result = evaluator.evaluate("%REQ(user-agent):10%", &ctx);
        assert_eq!(result, "Mozilla/5.");
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn test_max_length_no_truncation() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new().with_header("x-short", "abc");

        let result = evaluator.evaluate("%REQ(x-short):10%", &ctx);
        assert_eq!(result, "abc");
    }

    #[test]
    fn test_multiple_operators() {
        let evaluator = FormatStringEvaluator::new();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 54321);
        let ctx = RequestContext::new()
            .with_header(":authority", "example.com")
            .with_method("GET")
            .with_path("/api/test")
            .with_downstream_remote_address(addr);

        let result =
            evaluator.evaluate("%REQ_METHOD% %REQ_PATH% from %DOWNSTREAM_REMOTE_ADDRESS% to %REQ(:authority)%", &ctx);
        assert_eq!(result, "GET /api/test from 192.168.1.1:54321 to example.com");
    }

    #[test]
    fn test_missing_values_default_to_dash() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new();

        let result = evaluator.evaluate("%REQ_METHOD% %PROTOCOL% %UPSTREAM_HOST% %ROUTE_NAME%", &ctx);
        assert_eq!(result, "- - - -");
    }

    #[test]
    fn test_unknown_command() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new();

        let result = evaluator.evaluate("Test: %UNKNOWN_COMMAND%", &ctx);
        assert_eq!(result, "Test: %UNKNOWN_COMMAND%");
    }

    #[test]
    fn test_istio_connect_authority_use_case() {
        let evaluator = FormatStringEvaluator::new();

        // Simulate Istio waypoint extracting connect authority from CONNECT request
        let ctx = RequestContext::new()
            .with_header(":authority", "backend.default.svc.cluster.local:8080")
            .with_method("CONNECT");

        let result = evaluator.evaluate("%REQ(:authority)%", &ctx);
        assert_eq!(result, "backend.default.svc.cluster.local:8080");

        // This value would then be stored in filter state as io.istio.connect_authority
    }

    #[test]
    fn test_complex_format_string() {
        let evaluator = FormatStringEvaluator::new();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 244, 0, 5)), 45678);
        let ctx = RequestContext::new()
            .with_header(":authority", "my-service.ns.svc.cluster.local")
            .with_method("POST")
            .with_path("/v1/resource")
            .with_protocol("HTTP/2")
            .with_downstream_remote_address(addr)
            .with_upstream_host("10.96.1.10:8080")
            .with_route_name("default-route");

        let result = evaluator.evaluate(
            "[%ROUTE_NAME%] %PROTOCOL% %REQ_METHOD% %REQ_PATH% | Host: %REQ_AUTHORITY% | Client: %DOWNSTREAM_REMOTE_ADDRESS% | Upstream: %UPSTREAM_HOST%",
            &ctx,
        );

        assert_eq!(
            result,
            "[default-route] HTTP/2 POST /v1/resource | Host: my-service.ns.svc.cluster.local | Client: 10.244.0.5:45678 | Upstream: 10.96.1.10:8080"
        );
    }

    #[test]
    fn test_empty_format_string() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new();

        let result = evaluator.evaluate("", &ctx);
        assert_eq!(result, "");
    }

    #[test]
    fn test_only_literal_text() {
        let evaluator = FormatStringEvaluator::new();
        let ctx = RequestContext::new();

        let result = evaluator.evaluate("No operators here", &ctx);
        assert_eq!(result, "No operators here");
    }
}
