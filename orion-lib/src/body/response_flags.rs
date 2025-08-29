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

use std::ops::BitOr;

use crate::body::poly_body::PolyBodyError;
use orion_format::types::ResponseFlags as FmtResponseFlags;

#[derive(Clone, Debug)]
pub struct ResponseFlags(pub FmtResponseFlags);

impl Default for ResponseFlags {
    fn default() -> Self {
        ResponseFlags(FmtResponseFlags::empty())
    }
}

impl BitOr for ResponseFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        ResponseFlags(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Request,
    Response,
}

impl From<(&'_ hyper::Error, BodyKind)> for ResponseFlags {
    fn from((err, kind): (&hyper::Error, BodyKind)) -> Self {
        let mut flags = FmtResponseFlags::empty();
        if err.is_body_write_aborted() {
            match kind {
                BodyKind::Request => flags |= FmtResponseFlags::UPSTREAM_CONNECTION_TERMINATION,
                BodyKind::Response => flags |= FmtResponseFlags::DOWNSTREAM_CONNECTION_TERMINATION,
            }
        }
        if err.is_canceled() {
            match kind {
                BodyKind::Request => flags |= FmtResponseFlags::UPSTREAM_REMOTE_RESET,
                BodyKind::Response => flags |= FmtResponseFlags::DOWNSTREAM_REMOTE_RESET,
            }
        }

        if err.is_closed() {
            match kind {
                BodyKind::Request => flags |= FmtResponseFlags::UPSTREAM_CONNECTION_TERMINATION,
                BodyKind::Response => flags |= FmtResponseFlags::DOWNSTREAM_CONNECTION_TERMINATION,
            }
        }

        if err.is_incomplete_message() || err.is_timeout() {
            match kind {
                BodyKind::Request => flags |= FmtResponseFlags::UPSTREAM_REQUEST_TIMEOUT,
                BodyKind::Response => flags |= FmtResponseFlags::STREAM_IDLE_TIMEOUT,
            }
        }

        if err.is_parse() || err.is_parse_status() || err.is_parse_too_large() {
            match kind {
                BodyKind::Request => flags |= FmtResponseFlags::INVALID_ENVOY_REQUEST_HEADERS,
                BodyKind::Response => flags |= FmtResponseFlags::UPSTREAM_PROTOCOL_ERROR,
            }
        }

        if err.is_user() {
            match kind {
                BodyKind::Request => flags |= FmtResponseFlags::LOCAL_RESET,
                BodyKind::Response => flags |= FmtResponseFlags::DOWNSTREAM_CONNECTION_TERMINATION,
            }
        }

        ResponseFlags(flags)
    }
}

impl From<(&'_ PolyBodyError, BodyKind)> for ResponseFlags {
    fn from((err, kind): (&PolyBodyError, BodyKind)) -> Self {
        match err {
            PolyBodyError::Hyper(error) => (error, kind).into(),
            PolyBodyError::Infallible(_) | PolyBodyError::Grpc(_) | PolyBodyError::Boxed(_) => {
                ResponseFlags(FmtResponseFlags::empty())
            },
            PolyBodyError::TimedOut => match kind {
                BodyKind::Request => ResponseFlags(FmtResponseFlags::UPSTREAM_REQUEST_TIMEOUT),
                BodyKind::Response => ResponseFlags(FmtResponseFlags::STREAM_IDLE_TIMEOUT),
            },
        }
    }
}
