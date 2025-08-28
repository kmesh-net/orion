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

use futures::{future::BoxFuture, FutureExt, TryFutureExt};
use http::{
    uri::{Authority, Scheme},
    Request, Uri,
};
use std::{iter::Cycle, sync::Arc, vec::IntoIter};

use orion_xds::grpc_deps::{to_grpc_body, GrpcBody};
use tower::Service;

use crate::{
    body::{body_with_metrics::BodyWithMetrics, response_flags::BodyKind},
    listeners::http_connection_manager::{RequestHandler, TransactionHandler},
    transport::{policy::RequestExt, HttpChannel},
};

/// Adapts a [`HttpChannel`] to a [`Service`] that can be used as a channel for gRPC.
/// the inner value should be kept cheap to clone
#[derive(Clone, Debug)]
pub struct GrpcService {
    inner: HttpChannel,
    scheme: Scheme,
    authority: Authority,
}

impl GrpcService {
    pub fn try_new(inner: HttpChannel, authority: Authority) -> Result<Self, crate::Error> {
        let scheme = if inner.is_https() { Scheme::HTTPS } else { Scheme::HTTP };
        if !inner.http_version().is_http2() {
            return Err("gRPC endpoints need explicit HTTP 2".into());
        }

        Ok(GrpcService { inner, scheme, authority })
    }
}

impl GrpcService {
    async fn do_call(self, grpc_req: Request<GrpcBody>) -> std::result::Result<http::Response<GrpcBody>, crate::Error> {
        let (mut parts, grpc_body) = grpc_req.into_parts();

        // Add scheme and authority to gRPC URLs to make them valid HTTP
        let mut uri_parts = parts.uri.clone().into_parts();
        uri_parts.scheme = Some(self.scheme.clone());
        uri_parts.authority = Some(self.authority.clone());
        parts.uri = Uri::from_parts(uri_parts)?;

        let http_req = Request::from_parts(
            parts,
            BodyWithMetrics::new(BodyKind::Request, grpc_body.into(), |_bytes, _flags| {
                println!("gRPC request body finalized")
            }),
        );

        let svc_resp =
            self.inner.to_response(&Arc::new(TransactionHandler::default()), RequestExt::new(http_req)).await?;
        Ok(svc_resp.map(to_grpc_body))
    }
}

impl Service<Request<GrpcBody>> for GrpcService {
    type Response = http::Response<GrpcBody>;
    type Error = orion_xds::grpc_deps::Error;
    type Future = BoxFuture<'static, std::result::Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        // HttpService doesn't have poll_ready()
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, grpc_req: Request<GrpcBody>) -> Self::Future {
        self.clone()
            .do_call(grpc_req)
            .map_err(|e| Box::new(crate::Error::into_inner(e)) as orion_xds::grpc_deps::Error)
            .boxed()
    }
}

/// Simple `GrpcService` that does round-robin load balancing
/// over a static group of `GrpcService` instances
pub struct SimpleRoundRobinGrpcServiceLB {
    services: Cycle<IntoIter<GrpcService>>,
}

impl SimpleRoundRobinGrpcServiceLB {
    pub fn new(services: Vec<GrpcService>) -> Self {
        Self { services: services.into_iter().cycle() }
    }
    pub fn next_service(&mut self) -> Option<GrpcService> {
        self.services.next()
    }
}

impl Service<Request<GrpcBody>> for SimpleRoundRobinGrpcServiceLB {
    type Response = http::Response<GrpcBody>;
    type Error = orion_xds::grpc_deps::Error;
    type Future = BoxFuture<'static, std::result::Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, grpc_req: Request<GrpcBody>) -> Self::Future {
        if let Some(mut service) = self.next_service() {
            service.call(grpc_req)
        } else {
            Box::pin(futures::future::err("No gRPC endpoints available".into()))
        }
    }
}
