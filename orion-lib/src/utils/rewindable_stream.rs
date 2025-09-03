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

use crate::transport::AsyncReadWrite;
use bytes::Bytes;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub enum RewindableHeadAsyncStream<R>
where
    R: AsyncReadWrite + ?Sized,
{
    HeadBufferingReadOnlyMode { inner: Box<R>, buffer: Vec<u8> },
    FullReplayMode { inner: Box<R>, replay_buffer: Bytes, read_pos: usize },
}

impl<R> RewindableHeadAsyncStream<R>
where
    R: AsyncReadWrite + ?Sized,
{
    pub fn new(inner: Box<R>) -> Self {
        Self::HeadBufferingReadOnlyMode { inner, buffer: Vec::new() }
    }

    pub fn into_stream(self) -> Box<R> {
        match self {
            Self::FullReplayMode { inner, .. } | Self::HeadBufferingReadOnlyMode { inner, .. } => inner,
        }
    }

    pub fn into_rewound_stream(self) -> Self {
        match self {
            Self::HeadBufferingReadOnlyMode { inner, buffer } => {
                Self::FullReplayMode { inner, replay_buffer: buffer.into(), read_pos: 0 }
            },
            replaying @ Self::FullReplayMode { .. } => replaying,
        }
    }
}

impl<R> AsyncRead for RewindableHeadAsyncStream<R>
where
    R: AsyncReadWrite + ?Sized,
{
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::HeadBufferingReadOnlyMode { inner, buffer } => {
                let initial_len = buf.filled().len();
                match Pin::new(inner).poll_read(cx, buf) {
                    Poll::Ready(Ok(())) => {
                        let bytes_read = &buf.filled()[initial_len..];
                        buffer.extend_from_slice(bytes_read);
                        Poll::Ready(Ok(()))
                    },
                    other => other,
                }
            },
            Self::FullReplayMode { inner, replay_buffer, read_pos } => {
                if *read_pos < replay_buffer.len() {
                    let remaining_buffer = &replay_buffer[*read_pos..];
                    let to_copy = std::cmp::min(remaining_buffer.len(), buf.remaining());
                    buf.put_slice(&remaining_buffer[..to_copy]);
                    *read_pos += to_copy;
                    return Poll::Ready(Ok(()));
                }
                Pin::new(inner).poll_read(cx, buf)
            },
        }
    }
}

impl<R> AsyncWrite for RewindableHeadAsyncStream<R>
where
    R: AsyncReadWrite + ?Sized,
{
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, write_buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            Self::HeadBufferingReadOnlyMode { .. } => Poll::Ready(Err(std::io::Error::other(
                "RewindableHeadAsyncStream: write operations are not supported in HeadBufferingReadOnlyMode",
            ))),
            Self::FullReplayMode { inner, replay_buffer, read_pos } => {
                if *read_pos >= replay_buffer.len() {
                    Pin::new(inner).poll_write(cx, write_buf)
                } else {
                    Poll::Ready(Err(std::io::Error::other(
                        "RewindableHeadAsyncStream: write operations are not supported while replaying buffered data",
                    )))
                }
            },
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::HeadBufferingReadOnlyMode { .. } => Poll::Ready(Ok(())),
            Self::FullReplayMode { inner, .. } => Pin::new(inner).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::FullReplayMode { inner, .. } | Self::HeadBufferingReadOnlyMode { inner, .. } => {
                Pin::new(inner).poll_shutdown(cx)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn test_head_rewinding() {
        let test_data = b"hello world".to_vec();
        let (mut write_side, read_side) = tokio::io::duplex(1024);
        write_side.write_all(&test_data).await.unwrap();
        let mut rewindable = RewindableHeadAsyncStream::new(Box::new(read_side));

        let mut read_buf = vec![0u8; 5];
        rewindable.read_exact(&mut read_buf).await.unwrap();
        assert_eq!(&read_buf, b"hello");

        let mut read_buf2 = vec![0u8; 6];
        rewindable.read_exact(&mut read_buf2).await.unwrap();
        assert_eq!(&read_buf2, b" world");

        let mut rewound = rewindable.into_rewound_stream();

        let mut rewound_full = vec![0u8; 11];
        rewound.read_exact(&mut rewound_full).await.unwrap();
        assert_eq!(&rewound_full, b"hello world");
    }

    #[tokio::test]
    async fn test_stream_continuation() {
        let test_data = b"hello world".to_vec();
        let (mut write_side, read_side) = tokio::io::duplex(1024);
        write_side.write_all(&test_data).await.unwrap();
        let mut rewindable = RewindableHeadAsyncStream::new(Box::new(read_side));

        let mut read_buf = vec![0u8; 11];
        rewindable.read_exact(&mut read_buf).await.unwrap();
        assert_eq!(&read_buf, b"hello world");

        let additional_data = b" more data".to_vec();
        write_side.write_all(&additional_data).await.unwrap();

        let mut continuted_stream = *rewindable.into_stream();

        let mut read_buf = vec![0u8; 10];
        continuted_stream.read_exact(&mut read_buf).await.unwrap();
        assert_eq!(&read_buf, b" more data");
    }
}
