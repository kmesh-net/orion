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
use orion_format::types::ResponseFlags as FmtResponseFlags;

use tokio::time::error::Elapsed;

use crate::{Error as BoxError, body::response_flags::ResponseFlags};
use std::{error::Error, io};

use orion_http_header::{X_ENVOY_RATELIMITED, X_ORION_RATELIMITED};

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("ConnectFailure: {0:?}")]
    ConnectFailure(#[from] std::io::Error),
    #[error("ConnectTimeout")]
    ConnectTimeout(#[from] Elapsed),
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

// DISCLAIMER: This is a workaround for the fact that `EventError` can't implement `Clone`.
// Cloning is not possible because `Elapsed` and `io::Error` do not implement `Clone`.
// Their presence in `EventError` is required by the `orion_client` crate, as it needs
// to traverse the `EventError` to extract either the underlying `io::Error` or `Elapsed`
// in order to produce a more specific error message.
// In this case, we create a new `EventError` by reconstructing the `io::Error`
// with the same kind and message as the original. It's a kind of "shallow clone" of the error,
// which is not perfect, but sufficient for our use case.

impl Clone for EventError {
    fn clone(&self) -> Self {
        match self {
            EventError::ConnectFailure(io_err) => {
                let new_io_err = io::Error::new(io_err.kind(), io_err.to_string());
                EventError::ConnectFailure(new_io_err)
            },
            EventError::ConnectTimeout(_) => EventError::ConnectTimeout(elapsed()),
            EventError::PerTryTimeout => EventError::PerTryTimeout,
            EventError::RouteTimeout => EventError::RouteTimeout,
            EventError::Reset => EventError::Reset,
            EventError::RefusedStream => EventError::RefusedStream,
            EventError::Http3PostConnectFailure => EventError::Http3PostConnectFailure,
        }
    }
}
impl From<EventError> for ResponseFlags {
    fn from(err: EventError) -> Self {
        match err {
            EventError::ConnectFailure(_) | EventError::ConnectTimeout(_) => {
                ResponseFlags(FmtResponseFlags::UPSTREAM_CONNECTION_FAILURE)
            },
            EventError::PerTryTimeout => ResponseFlags(FmtResponseFlags::UPSTREAM_REQUEST_TIMEOUT),
            EventError::RouteTimeout => ResponseFlags(FmtResponseFlags::empty()),
            EventError::Reset | EventError::RefusedStream | EventError::Http3PostConnectFailure => {
                ResponseFlags(FmtResponseFlags::UPSTREAM_REMOTE_RESET)
            },
        }
    }
}

pub trait TryInferFrom<F>: Sized {
    fn try_infer_from(source: F) -> Option<Self>;
}

#[derive(Debug)]
pub enum RetryCondition<'a, B> {
    Error(EventError),
    Response(&'a Response<B>),
}

impl<'a, B> TryInferFrom<&'a Result<Response<B>, BoxError>> for RetryCondition<'a, B> {
    fn try_infer_from(source: &'a Result<Response<B>, BoxError>) -> Option<Self> {
        match source {
            Ok(ref resp) => {
                // NOTE: exclude a priory the evaluation of the retry policy for 1xx, and 2xx.
                if resp.status().is_informational() || resp.status().is_success() {
                    return None;
                }
                Some(RetryCondition::Response(resp))
            },
            Err(err) => {
                let ev = EventError::try_infer_from(err.as_ref())?;
                Some(RetryCondition::Error(ev))
            },
        }
    }
}

impl<'a> TryInferFrom<&'a (dyn std::error::Error + 'static)> for EventError {
    fn try_infer_from(err: &'a (dyn std::error::Error + 'static)) -> Option<Self> {
        if let Some(h_err) = err.downcast_ref::<hyper::Error>() {
            if let Some(source) = h_err.source() {
                return Self::try_infer_from(source);
            }
        }

        if let Some(h_err) = err.downcast_ref::<orion_client::client::legacy::Error>() {
            if let Some(source) = h_err.source() {
                return Self::try_infer_from(source);
            }
        }

        if err.downcast_ref::<Elapsed>().is_some() {
            // Note: This should never happen, as the user should remap the Tokio timeout
            // to a suitable EventError (e.g., timeout(dur, fut).await.map_err(|_| EventError::ConnectTimeout)).
            // Just in case, the PerTryTimeout error is the closest one we can choose.
            return Some(EventError::PerTryTimeout);
        }

        if let Some(failure) = err.downcast_ref::<EventError>() {
            return Some(failure.clone());
        }

        if let Some(h2_reason) = err.downcast_ref::<h2::Error>().and_then(h2::Error::reason) {
            match h2_reason {
                h2::Reason::REFUSED_STREAM => return Some(EventError::RefusedStream),
                h2::Reason::CONNECT_ERROR => {
                    return Some(EventError::ConnectFailure(io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "H2 connection refused",
                    )));
                },
                _ => return Some(EventError::Reset),
            }
        }

        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            match io_err.kind() {
                std::io::ErrorKind::ConnectionRefused => {
                    return Some(EventError::ConnectFailure(io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "Connection refused",
                    )));
                },
                std::io::ErrorKind::NotConnected => {
                    return Some(EventError::ConnectFailure(io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "Not connected",
                    )));
                },
                _ => return Some(EventError::Reset),
            }
        }

        // the rest of the errors are remapped to Reset
        Some(EventError::Reset)
    }
}

impl<B: Body> RetryCondition<'_, B> {
    pub fn inner_response(&self) -> Option<&Response<B>> {
        if let RetryCondition::Response(resp) = self { Some(resp) } else { None }
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
                    if matches!(
                        self,
                        RetryCondition::Error(EventError::ConnectFailure(_) | EventError::ConnectTimeout(_))
                    ) {
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
            }
        }
        false
    }
}

#[inline]
pub fn elapsed() -> Elapsed {
    unsafe { std::mem::transmute(()) }
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
