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

pub struct Router;

#[cfg(feature = "envoy-conversions")]
mod envoy_conversions {
    #![allow(deprecated)]
    use super::Router;
    use crate::config::common::*;
    use orion_data_plane_api::envoy_data_plane_api::envoy::extensions::filters::http::router::v3::Router as EnvoyRouter;
    impl TryFrom<EnvoyRouter> for Router {
        type Error = GenericError;
        fn try_from(value: EnvoyRouter) -> Result<Self, Self::Error> {
            let EnvoyRouter {
                dynamic_stats,
                start_child_span,
                upstream_log,
                upstream_log_options,
                suppress_envoy_headers,
                strict_check_headers,
                respect_expected_rq_timeout,
                suppress_grpc_request_failure_code_stats,
                upstream_http_filters,
                reject_connect_request_early_data: _,
            } = value;
            unsupported_field!(
                //note: docs say this field defaults to true. So depending on our behaviour we might have to check this is instead set to
                // Some(false)
                dynamic_stats,
                start_child_span,
                upstream_log,
                upstream_log_options,
                suppress_envoy_headers,
                strict_check_headers,
                respect_expected_rq_timeout,
                suppress_grpc_request_failure_code_stats,
                upstream_http_filters
            )?;
            Ok(Self)
        }
    }
}
