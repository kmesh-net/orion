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

use http::Response;
use http_body::Body;

use orion_configuration::config::network_filters::http_connection_manager::{RetryOn, RetryPolicy};

use tokio::time::error::Elapsed;

use crate::Error as BoxError;
use std::error::Error;

const X_ENVOY_RATELIMITED: &str = "x-envoy-ratelimited";
const X_ORION_RATELIMITED: &str = "x-orion-ratelimited";

#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum EventError {
    #[error("ConnectFailure")]
    ConnectFailure,
    #[error("ConnectTimeout")]
    ConnectTimeout,
    #[error("PerTryTimeout)")]
    PerTryTimeout,
    #[error("RouteTimeout")]
    RouteTimeout,
    #[error("Reset")]
    Reset,
    #[error("RefusedStream")]
    RefusedStream,
    #[allow(unused)]
    #[error("Http3PostConnectFailure")]
    Http3PostConnectFailure,
}

#[derive(Clone, Copy, Debug)]
pub enum FailureKind<'a, B> {
    Event(EventError),
    EligibleForRetry(&'a Response<B>),
}

impl<'a, B: Body> FailureKind<'a, B> {
    pub fn inner_response(&self) -> Option<&Response<B>> {
        if let FailureKind::EligibleForRetry(resp) = self {
            Some(resp)
        } else {
            None
        }
    }

    pub fn try_infer(result: &'a Result<Response<B>, BoxError>) -> Option<FailureKind<'a, B>> {
        match result {
            Ok(ref resp) => {
                // NOTE: exclude a priory the evaluation of the retry policy for 1xx, and 2xx.
                if resp.status().is_informational() || resp.status().is_success() {
                    return None;
                }
                Some(FailureKind::EligibleForRetry(resp))
            },
            Err(err) => Self::try_infer_from_error(err.as_ref()),
        }
    }

    pub fn try_infer_from_error(err: &'a (dyn std::error::Error + 'static)) -> Option<FailureKind<'a, B>> {
        if let Some(h_err) = err.downcast_ref::<hyper::Error>() {
            if let Some(source) = h_err.source() {
                return Self::try_infer_from_error(source);
            }
        }

        if let Some(h_err) = err.downcast_ref::<orion_hyper_util::client::legacy::Error>() {
            if let Some(source) = h_err.source() {
                return Self::try_infer_from_error(source);
            }
        }

        if err.downcast_ref::<Elapsed>().is_some() {
            // Note: This should never happen, as the user should remap the Tokio timeout
            // to a suitable EventError (e.g., timeout(dur, fut).await.map_err(|_| EventError::ConnectTimeout)).
            // Just in case, the PerTryTimeout error is the closest one we can choose.
            return Some(FailureKind::Event(EventError::PerTryTimeout));
        }

        if let Some(failure) = err.downcast_ref::<EventError>() {
            return Some(FailureKind::Event(*failure));
        }

        if let Some(h2_reason) = err.downcast_ref::<h2::Error>().and_then(h2::Error::reason) {
            match h2_reason {
                h2::Reason::REFUSED_STREAM => return Some(FailureKind::Event(EventError::RefusedStream)),
                h2::Reason::CONNECT_ERROR => return Some(FailureKind::Event(EventError::ConnectFailure)),
                _ => return Some(FailureKind::Event(EventError::Reset)),
            }
        }

        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            match io_err.kind() {
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotConnected => {
                    return Some(FailureKind::Event(EventError::ConnectFailure))
                },
                _ => return Some(FailureKind::Event(EventError::Reset)),
            }
        }

        // the rest of the errors are remapped to Reset
        Some(FailureKind::Event(EventError::Reset))
    }
}

pub fn should_retry<B: Body>(retry_policy: &RetryPolicy, event: &FailureKind<B>) -> bool {
    let response_event = event.inner_response();

    if let FailureKind::Event(EventError::PerTryTimeout) = event {
        return true;
    }

    for policy in &retry_policy.retry_on {
        match policy {
            RetryOn::Err5xx => {
                if let Some(resp) = response_event {
                    let status = resp.status();
                    if (500..=599).contains(&status.as_u16()) {
                        return true;
                    }
                }
            },
            RetryOn::GatewayError => {
                if let Some(resp) = response_event {
                    let status = resp.status();
                    if (502..=504).contains(&status.as_u16()) {
                        return true;
                    }
                }
            },
            RetryOn::EnvoyRateLimited => {
                if let Some(resp) = response_event {
                    if resp
                        .headers()
                        .iter()
                        .any(|(name, _)| name.as_str() == X_ENVOY_RATELIMITED || name.as_str() == X_ORION_RATELIMITED)
                    {
                        return true;
                    }
                }
            },
            RetryOn::Retriable4xx => {
                if let Some(resp) = response_event {
                    let status = resp.status();
                    if status.as_u16() == 409 {
                        return true;
                    }
                }
            },
            RetryOn::RetriableStatusCodes => {
                if let Some(resp) = response_event {
                    return retry_policy.retriable_status_codes.contains(&resp.status());
                }
            },
            RetryOn::RetriableHeaders => {
                if let Some(resp) = response_event {
                    return retry_policy.retriable_headers.iter().any(|hm| hm.response_matches(resp));
                }
            },
            RetryOn::Reset => {
                if let FailureKind::Event(EventError::Reset) = event {
                    return true;
                }
            },
            RetryOn::ConnectFailure => {
                if let FailureKind::Event(EventError::ConnectFailure | EventError::ConnectTimeout) = event {
                    return true;
                }
            },
            RetryOn::RefusedStream => {
                if let FailureKind::Event(EventError::RefusedStream) = event {
                    return true;
                }
            },
            RetryOn::Http3PostConnectFailure => {
                if let FailureKind::Event(EventError::Http3PostConnectFailure) = event {
                    return true;
                }
            },
        }
    }
    false
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
