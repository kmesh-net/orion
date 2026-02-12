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
//https://github.com/hickory-dns/hickory-dns/blob/main/crates/resolver/examples/global_resolver.rs

use std::{fmt::Display, future::pending, io, net::SocketAddr};

use hickory_resolver::{name_server::TokioConnectionProvider, IntoName, TokioResolver};

use once_cell::sync::Lazy;

static GLOBAL_DNS_RESOLVER: Lazy<TokioResolver> = Lazy::new(|| {
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;

    // We'll be using this condvar to get the Resolver from the thread...
    let pair = Arc::new((Mutex::new(None::<TokioResolver>), Condvar::new()));
    let pair2 = pair.clone();

    // Spawn the runtime to a new thread...
    //
    // This thread will manage the actual resolution runtime
    thread::spawn(move || {
        // A runtime for this new thread
        let runtime = tokio::runtime::Runtime::new().expect("failed to launch Runtime");

        // our platform independent future, result, see next blocks
        let resolver = {
            // To make this independent, if targeting macOS, BSD, Linux, or Windows, we can use the system's configuration:
            #[cfg(any(unix, windows))]
            {
                // use the system resolver configuration
                TokioResolver::builder(TokioConnectionProvider::default()).map(hickory_resolver::ResolverBuilder::build)
            }

            // For other operating systems, we can use one of the preconfigured definitions
            #[cfg(not(any(unix, windows)))]
            {
                // Directly reference the config types
                use hickory_resolver::config::{ResolverConfig, ResolverOpts};

                // Get a new resolver with the google nameservers as the upstream recursive resolvers
                TokioResolver::new(ResolverConfig::google(), ResolverOpts::default(), runtime.handle().clone())
            }
        };

        let (lock, cvar) = &*pair2;
        let mut started = lock.lock().expect("We expect this to work");

        let resolver = resolver.expect("failed to create hickory-resolver");

        *started = Some(resolver);
        cvar.notify_one();
        drop(started);

        runtime.block_on(pending::<()>())
    });

    // Wait for the thread to start up.
    let (lock, cvar) = &*pair;
    let mut resolver = lock.lock().expect("We expect this to work");
    while resolver.is_none() {
        resolver = cvar.wait(resolver).expect("We expect this to work");
    }

    // take the started resolver
    let resolver = resolver.take();

    // set the global resolver
    resolver.expect("resolver should not be none")
});

#[allow(clippy::expect_used)]
pub async fn resolve<N: IntoName + Display + 'static>(host: N, port: u16) -> io::Result<Vec<SocketAddr>> {
    // Now we use the global resolver to perform a lookup_ip.
    let name = host.to_string();
    if let Ok(addr) = format!("{name}:{port}").parse::<SocketAddr>() {
        return Ok(vec![addr]);
    }
    let result = GLOBAL_DNS_RESOLVER.lookup_ip(host).await;
    // map the result into what we want...
    result
        .map_err(move |err| {
            // we transform the error into a standard IO error for convenience
            io::Error::new(io::ErrorKind::AddrNotAvailable, format!("dns resolution error for {name}: {err}"))
        })
        .map(move |lookup_ip| -> Vec<SocketAddr> {
            // we take all the IPs returned, and then send back the set of IPs
            lookup_ip.iter().map(|ip| SocketAddr::new(ip, port)).collect::<Vec<_>>()
        })
}
