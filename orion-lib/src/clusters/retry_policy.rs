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

use http::Response;
use http_body::Body;

use crate::event_error::EventError;
use orion_configuration::config::network_filters::http_connection_manager::{RetryOn, RetryPolicy};
use tracing::warn;


use orion_http_header::{X_ENVOY_RATELIMITED, X_ORION_RATELIMITED};

#[derive(Debug)]
pub enum RetryCondition<'a, B> {
    Error(EventError),
    Response(&'a Response<B>),
}

impl<B: Body> RetryCondition<'_, B> {
    pub fn inner_response(&self) -> Option<&Response<B>> {
        if let RetryCondition::Response(resp) = self {
            Some(resp)
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub fn is_error(&self) -> bool {
        matches!(self, RetryCondition::Error(_))
    }

    pub fn is_per_try_timeout(&self) -> bool {
        matches!(self, RetryCondition::Error(EventError::PerTryTimeout))
    }

    #[allow(dead_code)]
    pub fn is_timeout(&self) -> bool {
        matches!(self, RetryCondition::Error(EventError::ConnectTimeout(_) | EventError::RouteTimeout))
    }

    pub fn should_retry(&self, retry_policy: &RetryPolicy) -> bool {
        if let RetryCondition::Error(EventError::PerTryTimeout) = self {
            return true;
        }

        let response = self.inner_response();

        for policy in &retry_policy.retry_on {
            match policy {
                RetryOn::Err5xx => {
                    if let Some(resp) = response {
                        let status = resp.status();
                        if (500..=599).contains(&status.as_u16()) {
                            return true;
                        }
                    }
                },
                RetryOn::GatewayError => {
                    if let Some(resp) = response {
                        let status = resp.status();
                        if (502..=504).contains(&status.as_u16()) {
                            return true;
                        }
                    }
                },
                RetryOn::EnvoyRateLimited => {
                    if let Some(resp) = response {
                        if resp.headers().iter().any(|(name, _)| {
                            name.as_str() == X_ENVOY_RATELIMITED || name.as_str() == X_ORION_RATELIMITED
                        }) {
                            return true;
                        }
                    }
                },
                RetryOn::Retriable4xx => {
                    if let Some(resp) = response {
                        let status = resp.status();
                        if status.as_u16() == 409 {
                            return true;
                        }
                    }
                },
                RetryOn::RetriableStatusCodes => {
                    if let Some(resp) = response {
                        return retry_policy.retriable_status_codes.contains(&resp.status());
                    }
                },
                RetryOn::RetriableHeaders => {
                    if let Some(resp) = response {
                        return retry_policy.retriable_headers.iter().any(|hm| hm.response_matches(resp));
                    }
                },
                RetryOn::Reset => {
                    if matches!(self, RetryCondition::Error(EventError::Reset)) {
                        return true;
                    }
                },
                RetryOn::ConnectFailure => {
                    if matches!(self, RetryCondition::Error(EventError::IoError(_) | EventError::ConnectTimeout(_))) {
                        return true;
                    }
                },
                RetryOn::RefusedStream => {
                    if matches!(self, RetryCondition::Error(EventError::RefusedStream)) {
                        return true;
                    }
                },
                RetryOn::Http3PostConnectFailure => {
                    if matches!(self, RetryCondition::Error(EventError::Http3PostConnectFailure)) {
                        return true;
                    }
                },
                _ => {
                    warn!("Unsupported retry policy {policy:?}");
                },
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn retry_on() {
        assert_eq!("5xx".parse::<RetryOn>().unwrap(), RetryOn::Err5xx);
        assert_eq!("gateway-error".parse::<RetryOn>().unwrap(), RetryOn::GatewayError);
        assert_eq!("reset".parse::<RetryOn>().unwrap(), RetryOn::Reset);
        assert_eq!("connect-failure".parse::<RetryOn>().unwrap(), RetryOn::ConnectFailure);
        assert_eq!("envoy-ratelimited".parse::<RetryOn>().unwrap(), RetryOn::EnvoyRateLimited);
        assert_eq!("retriable-4xx".parse::<RetryOn>().unwrap(), RetryOn::Retriable4xx);
        assert_eq!("refused-stream".parse::<RetryOn>().unwrap(), RetryOn::RefusedStream);
        assert_eq!("retriable-status-codes".parse::<RetryOn>().unwrap(), RetryOn::RetriableStatusCodes);
        assert_eq!("retriable-headers".parse::<RetryOn>().unwrap(), RetryOn::RetriableHeaders);
        assert!("unknown".parse::<RetryOn>().is_err());
    }
}
