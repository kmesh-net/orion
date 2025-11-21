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

use super::body_with_timeout::{BodyWithTimeout, TimeoutBodyError};
use crate::body::channel_body::ChannelBody;
use crate::Error;
use bytes::Bytes;
use http_body::Frame;
use http_body_util::combinators::WithTrailers;
use http_body_util::{BodyExt, Collected, Empty, Full, StreamBody};
use hyper::body::{Body, Incoming};
use orion_xds::grpc_deps::{GrpcBody, Status as GrpcError};
use pin_project::pin_project;
use std::convert::Infallible;
use std::future::Ready;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

pub type TrailersType = Option<Result<http::HeaderMap, Infallible>>;

#[pin_project(project = PolyBodyProj)]
pub enum PolyBody {
    Empty(#[pin] Empty<Bytes>),
    Full(#[pin] Full<Bytes>),
    Incoming(#[pin] Incoming),
    Timeout(#[pin] BodyWithTimeout<Incoming>),
    Grpc(#[pin] GrpcBody),
    Stream(#[pin] StreamBody<ReceiverStream<Result<Frame<Bytes>, Error>>>),
    ChannelBody(#[pin] ChannelBody),
    Collected(#[pin] Collected<Bytes>),
    FullWithTrailers(#[pin] WithTrailers<Full<Bytes>, Ready<TrailersType>>),
    EmptyWithTrailers(#[pin] WithTrailers<Empty<Bytes>, Ready<TrailersType>>),
    CollectedWithTrailers(#[pin] WithTrailers<Collected<Bytes>, Ready<TrailersType>>),
}

impl PolyBody {
    pub fn with_trailers(self, trailers: http::HeaderMap) -> Result<Self, PolyBodyError> {
        match self {
            PolyBody::Empty(e) => {
                let ready = std::future::ready(Some(Ok::<_, Infallible>(trailers)));
                Ok(PolyBody::EmptyWithTrailers(e.with_trailers(ready)))
            },
            PolyBody::Full(f) => {
                let ready = std::future::ready(Some(Ok::<_, Infallible>(trailers)));
                Ok(PolyBody::FullWithTrailers(f.with_trailers(ready)))
            },
            PolyBody::Collected(c) => {
                let ready = std::future::ready(Some(Ok::<_, Infallible>(trailers)));
                Ok(PolyBody::CollectedWithTrailers(c.with_trailers(ready)))
            },
            b => Err(PolyBodyError::Trailers(format!("{b:?}"))),
        }
    }

    pub async fn wait_frame(&mut self) {
        if let PolyBody::ChannelBody(m) = self {
            m.wait_frame().await;
        } else { // No-op for other body types
        }
    }
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
            PolyBody::Empty(b) => f.write_fmt(format_args!("PolyBody::Empty<Bytes>: {b:?}")),
            PolyBody::Full(b) => f.write_fmt(format_args!("PolyBody::Full<Bytes>: {b:?}")),
            PolyBody::Incoming(b) => f.write_fmt(format_args!("PolyBody::Incoming: {b:?}")),
            PolyBody::Timeout(b) => f.write_fmt(format_args!("PolyBody::Timeout<Incoming>: {b:?}")),
            PolyBody::Grpc(b) => f.write_fmt(format_args!("PolyBody::Grpc: {b:?}")),
            PolyBody::Stream(b) => f.write_fmt(format_args!("PolyBody::Stream: {b:?}")),
            PolyBody::ChannelBody(b) => f.write_fmt(format_args!("PolyBody::ChannelBody: {b:?}")),
            PolyBody::Collected(b) => f.write_fmt(format_args!("PolyBody::Collected: {b:?}")),
            PolyBody::FullWithTrailers(_) => f.write_str("PolyBody::WithTrailers<Full<Bytes>, Ready<TrailersType>>"),
            PolyBody::EmptyWithTrailers(_) => {
                f.write_str("PolyBody::EmptyWithTrailers<Empty<Bytes>, Ready<TrailersType>>")
            },
            PolyBody::CollectedWithTrailers(_) => {
                f.write_str("PolyBody::CollectedWithTrailers<Empty<Bytes>, Ready<TrailersType>>")
            },
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
    Grpc(#[from] Box<GrpcError>),
    #[error(transparent)]
    Boxed(#[from] Box<dyn std::error::Error + std::marker::Send + std::marker::Sync>),
    #[error("data was not received within the designated timeout")]
    TimedOut,
    #[error("could not build trailers for {0} body type")]
    Trailers(String),
    #[error("No valid variant to convert to")]
    BadVariant,
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
            PolyBodyProj::Grpc(g) => g.poll_frame(cx).map_err(|e| Into::into(Box::new(e))),
            PolyBodyProj::Stream(s) => {
                s.poll_frame(cx).map_err(|e| PolyBodyError::Boxed(Box::new(std::io::Error::other(e.to_string()))))
            },
            PolyBodyProj::ChannelBody(m) => {
                m.poll_frame(cx).map_err(|e| PolyBodyError::Boxed(Box::new(std::io::Error::other(e.to_string()))))
            },
            PolyBodyProj::Collected(s) => {
                s.poll_frame(cx).map_err(|e| PolyBodyError::Boxed(Box::new(std::io::Error::other(e.to_string()))))
            },
            PolyBodyProj::FullWithTrailers(w) => w.poll_frame(cx).map_err(Into::into),
            PolyBodyProj::EmptyWithTrailers(w) => w.poll_frame(cx).map_err(Into::into),
            PolyBodyProj::CollectedWithTrailers(w) => w.poll_frame(cx).map_err(Into::into),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            PolyBody::Empty(e) => e.is_end_stream(),
            PolyBody::Full(f) => f.is_end_stream(),
            PolyBody::Incoming(i) => i.is_end_stream(),
            PolyBody::Timeout(t) => t.is_end_stream(),
            PolyBody::Grpc(g) => g.is_end_stream(),
            PolyBody::Stream(s) => s.is_end_stream(),
            PolyBody::ChannelBody(m) => m.is_end_stream(),
            PolyBody::Collected(s) => s.is_end_stream(),
            PolyBody::FullWithTrailers(w) => w.is_end_stream(),
            PolyBody::EmptyWithTrailers(w) => w.is_end_stream(),
            PolyBody::CollectedWithTrailers(w) => w.is_end_stream(),
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

impl From<Collected<Bytes>> for PolyBody {
    #[inline]
    fn from(body: Collected<Bytes>) -> Self {
        PolyBody::Collected(body)
    }
}

impl From<StreamBody<ReceiverStream<Result<Frame<Bytes>, Error>>>> for PolyBody {
    #[inline]
    fn from(body: StreamBody<ReceiverStream<Result<Frame<Bytes>, Error>>>) -> Self {
        PolyBody::Stream(body)
    }
}

impl From<ChannelBody> for PolyBody {
    #[inline]
    fn from(body: ChannelBody) -> Self {
        PolyBody::ChannelBody(body)
    }
}

impl From<WithTrailers<Full<Bytes>, Ready<TrailersType>>> for PolyBody {
    #[inline]
    fn from(body: WithTrailers<Full<Bytes>, Ready<TrailersType>>) -> Self {
        PolyBody::FullWithTrailers(body)
    }
}

impl From<WithTrailers<Empty<Bytes>, Ready<TrailersType>>> for PolyBody {
    #[inline]
    fn from(body: WithTrailers<Empty<Bytes>, Ready<TrailersType>>) -> Self {
        PolyBody::EmptyWithTrailers(body)
    }
}

impl From<WithTrailers<Collected<Bytes>, Ready<TrailersType>>> for PolyBody {
    #[inline]
    fn from(body: WithTrailers<Collected<Bytes>, Ready<TrailersType>>) -> Self {
        PolyBody::CollectedWithTrailers(body)
    }
}

impl TryFrom<PolyBody> for Empty<Bytes> {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::Empty(e) => Ok(e),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl TryFrom<PolyBody> for Full<Bytes> {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::Full(f) => Ok(f),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl TryFrom<PolyBody> for Incoming {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::Incoming(i) => Ok(i),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl TryFrom<PolyBody> for GrpcBody {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::Grpc(g) => Ok(g),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl TryFrom<PolyBody> for Collected<Bytes> {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::Collected(c) => Ok(c),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl TryFrom<PolyBody> for StreamBody<ReceiverStream<Result<Frame<Bytes>, Error>>> {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::Stream(s) => Ok(s),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl TryFrom<PolyBody> for ChannelBody {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::ChannelBody(s) => Ok(s),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl TryFrom<PolyBody> for BodyWithTimeout<Incoming> {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::Timeout(t) => Ok(t),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl TryFrom<PolyBody> for WithTrailers<Full<Bytes>, Ready<TrailersType>> {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::FullWithTrailers(w) => Ok(w),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl TryFrom<PolyBody> for WithTrailers<Empty<Bytes>, Ready<TrailersType>> {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::EmptyWithTrailers(w) => Ok(w),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl TryFrom<PolyBody> for WithTrailers<Collected<Bytes>, Ready<TrailersType>> {
    type Error = PolyBodyError;
    fn try_from(value: PolyBody) -> Result<Self, Self::Error> {
        match value {
            PolyBody::CollectedWithTrailers(w) => Ok(w),
            _ => Err(PolyBodyError::BadVariant),
        }
    }
}

impl PolyBody {
    pub fn new_stream_body(buffer_size: usize) -> (Self, mpsc::Sender<Result<Frame<Bytes>, Error>>) {
        let (tx, rx) = mpsc::channel(buffer_size);
        let stream = ReceiverStream::new(rx);
        let body = StreamBody::new(stream);
        (PolyBody::Stream(body), tx)
    }
}
