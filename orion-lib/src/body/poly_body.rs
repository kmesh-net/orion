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

use super::body_with_timeout::{BodyWithTimeout, TimeoutBodyError};
use bytes::Bytes;
use http_body_util::{Empty, Full};
use hyper::body::{Body, Incoming};
use orion_xds::grpc_deps::{GrpcBody, Status as GrpcError};
use pin_project::pin_project;

#[pin_project(project = PolyBodyProj)]
pub enum PolyBody {
    Empty(#[pin] Empty<Bytes>),
    Full(#[pin] Full<Bytes>),
    Incoming(#[pin] Incoming),
    Timeout(#[pin] BodyWithTimeout<Incoming>),
    Grpc(#[pin] GrpcBody),
}

impl Default for PolyBody {
    #[inline]
    fn default() -> Self {
        PolyBody::Empty(Empty::<Bytes>::default())
    }
}

impl std::fmt::Debug for PolyBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolyBody::Empty(_) => f.write_str("PolyBody::Empty"),
            PolyBody::Full(body) => f.write_str(&format!("PolyBody::Full {body:?}")),
            PolyBody::Incoming(_) => f.write_str("PolyBody::Incoming"),
            PolyBody::Timeout(body) => f.write_str(&format!("PolyBody::Timeout: {body:?}")),
            PolyBody::Grpc(_) => f.write_str("PolyBody::Grpc"),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum PolyBodyError {
    #[error(transparent)]
    Hyper(#[from] hyper::Error),
    #[error(transparent)]
    Infallible(#[from] std::convert::Infallible),
    #[error(transparent)]
    Grpc(#[from] GrpcError),
    #[error(transparent)]
    Boxed(#[from] Box<dyn std::error::Error + std::marker::Send + std::marker::Sync>),
    #[error("data was not received within the designated timeout")]
    TimedOut,
}

//hyper::Error is the error type returned by incoming
impl From<TimeoutBodyError<hyper::Error>> for PolyBodyError {
    fn from(value: TimeoutBodyError<hyper::Error>) -> Self {
        match value {
            TimeoutBodyError::TimedOut => Self::TimedOut,
            TimeoutBodyError::BodyError(e) => Self::Hyper(e),
        }
    }
}

impl Body for PolyBody {
    type Data = Bytes;
    type Error = PolyBodyError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        match self.project() {
            PolyBodyProj::Empty(e) => e.poll_frame(cx).map_err(Into::into),
            PolyBodyProj::Full(f) => f.poll_frame(cx).map_err(Into::into),
            PolyBodyProj::Incoming(i) => i.poll_frame(cx).map_err(Into::into),
            PolyBodyProj::Timeout(t) => t.poll_frame(cx).map_err(Into::into),
            PolyBodyProj::Grpc(g) => g.poll_frame(cx).map_err(Into::into),
        }
    }
}

impl From<Empty<Bytes>> for PolyBody {
    #[inline]
    fn from(body: Empty<Bytes>) -> Self {
        PolyBody::Empty(body)
    }
}

impl From<Full<Bytes>> for PolyBody {
    #[inline]
    fn from(body: Full<Bytes>) -> Self {
        PolyBody::Full(body)
    }
}

impl From<Incoming> for PolyBody {
    #[inline]
    fn from(body: Incoming) -> Self {
        PolyBody::Incoming(body)
    }
}

impl From<BodyWithTimeout<Incoming>> for PolyBody {
    #[inline]
    fn from(body: BodyWithTimeout<Incoming>) -> Self {
        PolyBody::Timeout(body)
    }
}

impl From<GrpcBody> for PolyBody {
    #[inline]
    fn from(body: GrpcBody) -> Self {
        PolyBody::Grpc(body)
    }
}
