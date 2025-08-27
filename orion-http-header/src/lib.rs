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

use http::HeaderName;

macro_rules! custom_header {
    ($(#[$attr:meta])* $const_name:ident, $header_string:literal) => {
        $(#[$attr])*
        pub const $const_name: HeaderName = HeaderName::from_static($header_string);
    };
}

/// The `x-envoy-original-path` header is used to store the original request path
custom_header!(X_ENVOY_ORIGINAL_PATH, "x-envoy-original-path");

/// The `B3` header is used for B3 single header tracing
custom_header!(B3, "b3");

/// The `uber-trace-id` header is used for Jaeger tracing
custom_header!(UBER_TRACE_ID, "uber-trace-id");

/// The `x-b3-traceid` header is used for B3 tracing
custom_header!(X_B3_TRACEID, "x-b3-traceid");

/// The `x-b3-spanid` header is used for B3 tracing
custom_header!(X_B3_SPANID, "x-b3-spanid");

/// The `x-b3-parentspanid` header is used for B3 tracing
custom_header!(X_B3_PARENTSPANID, "x-b3-parentspanid");

/// The `x-b3-sampled` header is used for B3 tracing
custom_header!(X_B3_SAMPLED, "x-b3-sampled");

/// The `X-Envoy-Force-Trace` header is used to force tracing in Envoy
custom_header!(X_ENVOY_FORCE_TRACE, "x-envoy-force-trace");

/// The `x-datadog-trace-id` header is used for Datadog tracing
custom_header!(X_DATADOG_TRACE_ID, "x-datadog-trace-id");

/// The `X-Request-ID` header is used to pass a unique request ID
custom_header!(X_REQUEST_ID, "x-request-id");

/// The `x-client-trace-id` header is used to pass a client trace ID
custom_header!(X_CLIENT_TRACE_ID, "x-client-trace-id");

/// The `traceparent` header is used for W3C Trace Context
custom_header!(TRACEPARENT, "traceparent");

/// The `X-Envoy-RateLimited` header is used to indicate rate limiting by Envoy
custom_header!(X_ENVOY_RATELIMITED, "x-envoy-ratelimited");

/// The `x-orion-ratelimited` header is used to indicate rate limiting by Orion
custom_header!(X_ORION_RATELIMITED, "x-orion-ratelimited");

/// The `x-forwarded-for` header is used to identify the originating IP address of a client
custom_header!(X_FORWARDED_FOR, "x-forwarded-for");

/// The `x-envoy-external-address` header is used to pass the external address in Envoy
custom_header!(X_ENVOY_EXTERNAL_ADDRESS, "x-envoy-external-address");

/// The `x-envoy-internal` header is used to indicate internal requests in Envoy
custom_header!(X_ENVOY_INTERNAL, "x-envoy-internal");
