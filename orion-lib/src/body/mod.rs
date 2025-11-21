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

//! Middleware that applies a timeout to requests.
//!
//! If the request does not complete within the specified timeout it will be aborted and a `408
//! Request Timeout` response will be sent.
//!
//! # Differences from `tower::timeout`
//!
//! tower's [`Timeout`](tower::timeout::Timeout) middleware uses an error to signal timeout, i.e.
//! it changes the error type to [`BoxError`](tower::BoxError). For HTTP services that is rarely
//! what you want as returning errors will terminate the connection without sending a response.
//!
//! This middleware won't change the error type and instead return a `408 Request Timeout`
//! response. That means if your service's error type is [`Infallible`] it will still be
//! [`Infallible`] after applying this middleware.
//!

pub mod body_with_metrics;
pub mod body_with_timeout;
pub mod channel_body;
pub mod poly_body;
pub(crate) mod response_flags;
