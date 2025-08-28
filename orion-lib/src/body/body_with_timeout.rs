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

/// Middleware that applies a timeout to request and response bodies.
///
/// Wrapper around a [`http_body::Body`] to time out if data is not ready within the specified duration.
///
/// Bodies must produce data at most within the specified timeout.
/// If the body does not produce a requested data frame within the timeout period, it will return an error.
///
/// This `TimeoutBody` variant differs from `tower_http::timeout::TimeoutBody` in two ways:
/// 1. Unpin: The original `TimeoutBody` is !Unpin, while this version is Unpin to enable use in certain asynchronous contexts.
/// 2. Optional Timeout: The timeout is wrapped in `Option`, allowing for cases where a timeout may not be necessary.
///
use http_body::Body;
use pin_project::pin_project;
use pingora_timeout::{
    Timeout as PingoraTimeout,
    fast_timeout::{FastTimeout, fast_timeout},
};
use std::{
    future::{Future, Pending, pending},
    pin::Pin,
    task::{Context, Poll, ready},
    time::Duration,
};

pub type Timeout = PingoraTimeout<Pending<()>, FastTimeout>;

#[pin_project]
pub struct BodyWithTimeout<B> {
    timeout: Option<Duration>,
    #[pin]
    sleep: Option<Pin<Box<Timeout>>>,
    #[pin]
    body: B,
}

impl<B> std::fmt::Debug for BodyWithTimeout<B>
where
    B: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeoutBody").field("timeout", &self.timeout).field("body", &self.body).finish_non_exhaustive()
    }
}

impl<B> BodyWithTimeout<B> {
    /// Creates a new [`TimeoutBody`].
    pub fn new(timeout: Option<Duration>, body: B) -> Self {
        BodyWithTimeout { timeout, sleep: None, body }
    }
}

impl<B> Body for BodyWithTimeout<B>
where
    B: Body,
    B::Error: std::error::Error,
{
    type Data = B::Data;
    type Error = TimeoutBodyError<B::Error>;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        if let Some(timeout) = this.timeout {
            // Start the `Sleep` if not active.
            let sleep_pinned = if let Some(some) = this.sleep.as_mut().as_pin_mut() {
                some
            } else {
                Pin::new(this.sleep.insert(Box::pin(fast_timeout(*timeout, pending()))))
            };

            // Error if the timeout has expired.
            if sleep_pinned.poll(cx).is_ready() {
                return Poll::Ready(Some(Err(TimeoutBodyError::TimedOut)));
            }

            // Check for body data.
            let frame = ready!(this.body.poll_frame(cx));

            // A frame is ready. Reset the `Sleep`...
            this.sleep.set(None);

            Poll::Ready(frame.transpose().map_err(TimeoutBodyError::BodyError).transpose())
        } else {
            this.body.poll_frame(cx).map_err(TimeoutBodyError::BodyError)
        }
    }
}

/// Error for [`TimeoutBody`].
#[derive(thiserror::Error, Debug)]
pub enum TimeoutBodyError<E: std::error::Error> {
    #[error("data was not received within the designated timeout")]
    TimedOut,
    #[error(transparent)]
    BodyError(E),
}

#[cfg(test)]
mod tests {
    use super::*;

    use bytes::Bytes;
    use http_body::Frame;
    use http_body_util::BodyExt;
    use pin_project::pin_project;
    use std::{error::Error, fmt::Display};
    use tokio::time::{Sleep, sleep};

    #[derive(Debug)]
    struct MockError;

    impl Error for MockError {}

    impl Display for MockError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "mock error")
        }
    }

    #[pin_project]
    struct MockBody {
        #[pin]
        sleep: Sleep,
    }

    impl Body for MockBody {
        type Data = Bytes;
        type Error = MockError;

        fn poll_frame(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            let this = self.project();
            this.sleep.poll(cx).map(|()| Some(Ok(Frame::data(vec![].into()))))
        }
    }

    #[tokio::test]
    async fn test_body_available_within_timeout() {
        let mock_sleep = Duration::from_secs(1);
        let timeout_sleep = Duration::from_secs(2);

        let mock_body = MockBody { sleep: sleep(mock_sleep) };
        let body_with_timeout = BodyWithTimeout::new(Some(timeout_sleep), mock_body);

        assert!(body_with_timeout.boxed_unsync().frame().await.expect("no frame").is_ok());
    }

    #[tokio::test]
    async fn test_body_unavailable_within_timeout_error() {
        let mock_sleep = Duration::from_secs(2);
        let timeout_sleep = Duration::from_secs(1);

        let mock_body = MockBody { sleep: sleep(mock_sleep) };
        let body_with_timeout = BodyWithTimeout::new(Some(timeout_sleep), mock_body);

        assert!(body_with_timeout.boxed_unsync().frame().await.unwrap().is_err());
    }
}
