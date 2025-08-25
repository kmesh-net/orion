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

use http::HeaderName;

macro_rules! custom_header {
    ($const_name:ident, $header_string:literal) => {
        // This line generates the constant declaration.
        pub const $const_name: HeaderName = HeaderName::from_static($header_string);
    };
}

custom_header!(X_ENVOY_ORIGINAL_PATH, "x-envoy-original-path");

custom_header!(B3, "b3");

custom_header!(X_B3_TRACEID, "x-b3-traceid");

custom_header!(X_B3_SPANID, "x-b3-spanid");

custom_header!(X_B3_PARENTSPANID, "x-b3-parentspanid");

custom_header!(X_B3_SAMPLED, "x-b3-sampled");

custom_header!(X_ENVOY_FORCE_TRACE, "x-envoy-force-trace");

custom_header!(X_DATADOG_TRACE_ID, "x-datadog-trace-id");

custom_header!(X_REQUEST_ID, "x-request-id");

custom_header!(X_CLIENT_TRACE_ID, "x-client-trace-id");

custom_header!(TRACEPARENT, "traceparent");

custom_header!(X_ENVOY_RATELIMITED, "x-envoy-ratelimited");

custom_header!(X_ORION_RATELIMITED, "x-orion-ratelimited");

custom_header!(X_FORWARDED_FOR, "x-forwarded-for");

custom_header!(X_ENVOY_EXTERNAL_ADDRESS, "x-envoy-external-address");

custom_header!(X_ENVOY_INTERNAL, "x-envoy-internal");
