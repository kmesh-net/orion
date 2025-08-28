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

use super::AsyncReadWrite;
use crate::utils::rewindable_stream::RewindableHeadAsyncStream;

use rustls::server::Acceptor;
use std::io;

#[derive(Debug)]
pub enum InspectorResult {
    /// Handshake with valid TLS and SNI.
    Success(String),
    /// Handshake with valid TLS, without server name indication.
    SuccessNoSni,
    /// Failed TLS Handshake (e.g. not TLS, or I/O, etc.).
    TlsError(io::Error),
}

pub async fn inspect_client_hello(stream: Box<dyn AsyncReadWrite>) -> (InspectorResult, Box<dyn AsyncReadWrite>) {
    let mut inspector = RewindableHeadAsyncStream::new(stream);
    let acceptor = tokio_rustls::LazyConfigAcceptor::new(Acceptor::default(), &mut inspector);
    let result = match acceptor.await {
        Ok(handshake) => match handshake.client_hello().server_name() {
            Some(server_name) => InspectorResult::Success(server_name.to_owned()),
            None => InspectorResult::SuccessNoSni,
        },
        Err(e) => InspectorResult::TlsError(e),
    };
    let rewound_stream = inspector.into_rewound_stream();
    (result, Box::new(rewound_stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::{pki_types::ServerName, ClientConfig, ClientConnection};
    use std::{io::Cursor, sync::Arc};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn create_client_hello_with_sni(sni: &str) -> Vec<u8> {
        let config = Arc::new(
            ClientConfig::builder().with_root_certificates(rustls::RootCertStore::empty()).with_no_client_auth(),
        );
        let server_name = ServerName::try_from(sni.to_owned()).unwrap();
        let mut client = ClientConnection::new(config, server_name).unwrap();
        let mut tls_data = Vec::new();
        let mut cursor = Cursor::new(&mut tls_data);
        client.write_tls(&mut cursor).unwrap();
        tls_data
    }

    #[tokio::test]
    async fn test_sni_detection() {
        let tls_data = create_client_hello_with_sni("example.com");
        let cursor = std::io::Cursor::new(tls_data.clone());
        let inbound = Box::new(cursor) as Box<dyn AsyncReadWrite>;
        let (result, rewound) = inspect_client_hello(inbound).await;
        assert!(matches!(result, InspectorResult::Success(ref sni) if sni == "example.com"));

        let (result, mut rewound_again) = inspect_client_hello(rewound).await;
        assert!(matches!(result, InspectorResult::Success(ref sni) if sni == "example.com"));

        let mut full_data = Vec::new();
        rewound_again.read_to_end(&mut full_data).await.unwrap();
        assert_eq!(full_data, tls_data);
    }

    #[tokio::test]
    async fn test_no_sni() {
        let tls_data = create_client_hello_with_sni("127.0.0.1");
        let cursor = std::io::Cursor::new(tls_data);
        let inbound = Box::new(cursor) as Box<dyn AsyncReadWrite>;
        let (result, _) = inspect_client_hello(inbound).await;
        assert!(matches!(result, InspectorResult::SuccessNoSni));
    }

    #[tokio::test]
    async fn test_non_tls_data() {
        let http_data = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";

        let (mut write_stream, read_stream) = tokio::io::duplex(1024);
        write_stream.write_all(http_data).await.unwrap();
        write_stream.shutdown().await.unwrap();

        let inbound = Box::new(read_stream) as Box<dyn AsyncReadWrite>;
        let (result, mut rewound) = inspect_client_hello(inbound).await;
        assert!(matches!(result, InspectorResult::TlsError(_)));

        let mut full_data = Vec::new();
        rewound.read_to_end(&mut full_data).await.unwrap();
        assert_eq!(full_data, http_data);
    }
}
