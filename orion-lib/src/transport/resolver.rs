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

// Based on
// https://github.com/hickory-dns/hickory-dns/blob/v0.24.1/crates/resolver/examples/global_resolver.rs

use std::{io, net::SocketAddr, sync::OnceLock};

use hickory_resolver::{name_server::TokioConnectionProvider, TokioAsyncResolver};

static GLOBAL_DNS_RESOLVER: OnceLock<TokioAsyncResolver> = OnceLock::new();

pub async fn resolve(host: &str, port: u16) -> io::Result<SocketAddr> {
    if let Ok(addr) = (host.to_owned() + ":" + &port.to_string()).parse::<SocketAddr>() {
        return Ok(addr);
    }
    match GLOBAL_DNS_RESOLVER
        .get_or_init(|| -> TokioAsyncResolver {
            // The TokioAsyncResolver needs a Tokio runtime already running. By encapsulating the
            // initialization of the OnceLock in this lambda function, we make sure that it is
            // called from the parent `async` context, which 'weakly' guarantees a Tokio runtime is on.

            #[cfg(any(unix, windows))]
            {
                match TokioAsyncResolver::from_system_conf(TokioConnectionProvider::default()) {
                    Ok(resolver) => resolver,
                    Err(err) => unimplemented!("Could not initialize the DNS resolver: {err}"),
                }
            }
            #[cfg(not(any(unix, windows)))]
            {
                compile_error!("DNS resolver not implemented for this platform");
            }
        })
        .lookup_ip(host)
        .await
        .map(|lookup_ip| lookup_ip.into_iter().next())
    {
        Ok(Some(ip)) => Ok(SocketAddr::new(ip, port)),
        Ok(None) => Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("dns resolution error for {host}: no ip found "),
        )),
        Err(e) => Err(io::Error::new(io::ErrorKind::AddrNotAvailable, format!("dns resolution error for {host}: {e}"))),
    }
}
