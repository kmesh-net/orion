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

use http::HeaderName;

macro_rules! custom_header {
    ($(#[$attr:meta])* $const_name:ident, $header_string:literal) => {
        $(#[$attr])*
        pub const $const_name: HeaderName = HeaderName::from_static($header_string);
    };
}

custom_header!(
    /// The `x-envoy-original-path` header is used to store the original request path
    X_ENVOY_ORIGINAL_PATH, "x-envoy-original-path");

custom_header!(
    /// The `B3` header is used for B3 single header tracing
    B3, "b3");

custom_header!(
    /// The `uber-trace-id` header is used for Jaeger tracing
    UBER_TRACE_ID, "uber-trace-id");

custom_header!(
    /// The `x-b3-traceid` header is used for B3 tracing
    X_B3_TRACEID, "x-b3-traceid");

custom_header!(
    /// The `x-b3-spanid` header is used for B3 tracing
    X_B3_SPANID, "x-b3-spanid");

custom_header!(
    /// The `x-b3-parentspanid` header is used for B3 tracing
    X_B3_PARENTSPANID, "x-b3-parentspanid");

custom_header!(
    /// The `x-b3-sampled` header is used for B3 tracing
    X_B3_SAMPLED, "x-b3-sampled");

custom_header!(
    /// The `X-Envoy-Force-Trace` header is used to force tracing in Envoy
    X_ENVOY_FORCE_TRACE, "x-envoy-force-trace");

custom_header!(
    /// The `x-datadog-trace-id` header is used for Datadog tracing
    X_DATADOG_TRACE_ID, "x-datadog-trace-id");

custom_header!(
    /// The `X-Request-ID` header is used to pass a unique request ID
    X_REQUEST_ID, "x-request-id");

custom_header!(
    /// The `x-client-trace-id` header is used to pass a client trace ID
    X_CLIENT_TRACE_ID, "x-client-trace-id");

custom_header!(
    /// The `traceparent` header is used for W3C Trace Context
    TRACEPARENT, "traceparent");

custom_header!(
    /// The `X-Envoy-RateLimited` header is used to indicate rate limiting by Envoy
    X_ENVOY_RATELIMITED, "x-envoy-ratelimited");

custom_header!(
    /// The `x-orion-ratelimited` header is used to indicate rate limiting by Orion
    X_ORION_RATELIMITED, "x-orion-ratelimited");

custom_header!(
    /// The `x-forwarded-for` header is used to identify the originating IP address of a client
    X_FORWARDED_FOR, "x-forwarded-for");

custom_header!(
    /// The `x-envoy-external-address` header is used to pass the external address in Envoy
    X_ENVOY_EXTERNAL_ADDRESS, "x-envoy-external-address");

custom_header!(
    /// The `x-envoy-internal` header is used to indicate internal requests in Envoy
    X_ENVOY_INTERNAL, "x-envoy-internal");

custom_header!(
    /// The `x-envoy-original-dst-host` header is used to store the original destination host
    X_ENVOY_ORIGINAL_DST_HOST, "x-envoy-original-dst-host");

custom_header!(
    /// The `lb-header` header is used for load balancing decisions
    LB_HEADER, "lb-header");
