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

use rustls::server::Acceptor;
use std::{pin::Pin, task::Poll};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};

pub struct TlsInspector<'a> {
    stream: &'a mut TcpStream,
    bytes_read: usize,
    buffer: Vec<u8>,
}

impl<'a> TlsInspector<'a> {
    fn new(stream: &'a mut TcpStream) -> Self {
        Self { stream, bytes_read: 0, buffer: Vec::with_capacity(0) }
    }
    pub async fn peek_sni(stream: &'a mut TcpStream) -> Option<String> {
        // we discard any errors here to simplify the code.
        // the tls inspector might fail to find a handshake if we don't configure TLS for the listener, or it might fail because of some other spurious IO error.
        // in the former case, we want to continue on as normal without SNI while in the latter case (which should be rare) we will fail the connection later anyways.
        let handshake = tokio_rustls::LazyConfigAcceptor::new(Acceptor::default(), Self::new(stream)).await.ok()?;
        handshake.client_hello().server_name().map(String::from)
    }
}

impl AsyncRead for TlsInspector<'_> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let Self { stream, bytes_read, buffer } = Pin::into_inner(self);
        //  on the first peek, we can attempt to peek directly into the provided buffer as an optimization
        if *bytes_read == 0 {
            let poll = Pin::new(stream).poll_peek(cx, buf);
            if let Poll::Ready(Ok(n_bytes)) = poll {
                *bytes_read = n_bytes;
                Poll::Ready(Ok(()))
            } else {
                poll.map(|p| p.map(|_| ()))
            }
        } else {
            //if we have little space left in the buffer, grow it.
            // maximum size should be capped by rustls failing the handshake.
            if buffer.len().checked_sub(*bytes_read).unwrap_or_default() <= 512 {
                buffer.resize(buffer.len() + 4 * (1 << 10), 0);
            }
            let mut peek_read_buf = tokio::io::ReadBuf::new(&mut buffer[..buf.remaining()]);
            let poll = Pin::new(stream).poll_peek(cx, &mut peek_read_buf);
            if let Poll::Ready(Ok(n_bytes)) = poll {
                //this should never fail, as that would imply we peeked less bytes than we did previously
                if n_bytes >= *bytes_read {
                    return Poll::Ready(Err(std::io::Error::other(
                        "TLS inspector peeked less bytes than it did in a previous iteration",
                    )));
                }
                let newly_read = &peek_read_buf.filled()[*bytes_read..];
                buf.put_slice(newly_read);
                *bytes_read = n_bytes;
                Poll::Ready(Ok(()))
            } else {
                poll.map(|p| p.map(|_| ()))
            }
        }
    }
}

// the rustls implementation requires we implement write, but we don't want to write here yet
// ideally we would refactor this code in such a way that we simply return the result of the tls handshake
// and continue from there instead of peeking
//
// for now, we simply error out on any writes. The TLS protocol should not require that we write anything before receiving the initial handshake
// see https://tls13.xargs.org/
impl AsyncWrite for TlsInspector<'_> {
    fn poll_flush(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        std::task::Poll::Ready(Err(std::io::Error::other("TLS inspector tried to write to read-only stream")))
    }
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &[u8],
    ) -> std::task::Poll<std::result::Result<usize, std::io::Error>> {
        std::task::Poll::Ready(Err(std::io::Error::other("TLS inspector tried to write to read-only stream")))
    }
    fn poll_shutdown(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        std::task::Poll::Ready(Err(std::io::Error::other("TLS inspector tried to write to read-only stream")))
    }
}
