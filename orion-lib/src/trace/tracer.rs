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

use bounded_integer::BoundedU16;
use http::{HeaderValue, Request};
use orion_configuration::config::network_filters::tracing::Tracing;
use orion_http_header::*;

use crate::trace::{
    request_id::RequestId,
    trace_id::{FromHeaderValue, TraceInfo, TraceProvider},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext {
    parent: Option<TraceInfo>,
    child: Option<TraceInfo>,
    request_id: Option<RequestId>,
    client_trace_id: Option<HeaderValue>,
}

impl TraceContext {
    /// Creates a new (root) `TraceContext` instance.
    pub fn new() -> Self {
        Self { parent: None, child: None, request_id: None, client_trace_id: None }
    }

    pub fn with_parent(self, parent: TraceInfo) -> Self {
        Self {
            parent: Some(parent),
            child: self.child,
            request_id: self.request_id,
            client_trace_id: self.client_trace_id,
        }
    }

    pub fn with_child(self, child: TraceInfo) -> Self {
        Self {
            parent: self.parent,
            child: Some(child),
            request_id: self.request_id,
            client_trace_id: self.client_trace_id,
        }
    }

    pub fn with_request_id(self, request_id: Option<RequestId>) -> Self {
        Self { parent: self.parent, child: self.child, request_id, client_trace_id: self.client_trace_id }
    }

    pub fn with_client_trace_id(self, client_trace_id: Option<HeaderValue>) -> Self {
        Self { parent: self.parent, child: self.child, request_id: self.request_id, client_trace_id }
    }

    #[allow(dead_code)]
    pub fn parent(&self) -> Option<&TraceInfo> {
        self.parent.as_ref()
    }

    #[allow(dead_code)]
    pub fn child(&self) -> Option<&TraceInfo> {
        self.child.as_ref()
    }

    #[allow(dead_code)]
    pub fn request_id(&self) -> Option<&HeaderValue> {
        self.request_id.as_ref().map(|id| id.as_ref())
    }

    #[allow(dead_code)]
    pub fn client_trace_id(&self) -> Option<&HeaderValue> {
        self.client_trace_id.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracer {
    tracing: Tracing,
}

impl Tracer {
    /// Creates a new `Tracer` instance with the provided tracing configuration.
    pub fn new(tracing: Tracing) -> Self {
        Self { tracing }
    }

    /// Analyzes request headers to decide if and how to trace.
    ///
    pub fn build_trace_context<B>(&self, req: &Request<B>, request_id: Option<RequestId>) -> TraceContext {
        let headers = req.headers();
        let x_client_trace_id = headers.get(X_CLIENT_TRACE_ID);

        if let Some(trace_id) = TraceInfo::extract_from(headers).ok().flatten() {
            if trace_id.sampled() {
                return TraceContext::new()
                    .with_parent(trace_id.clone())
                    .with_child(trace_id.into_child())
                    .with_request_id(request_id)
                    .with_client_trace_id(x_client_trace_id.cloned());
            }

            // If the trace ID is not sampled, we still return it but indicate no sampling (in both parent and child).
            return TraceContext::new()
                .with_parent(trace_id.clone())
                .with_child(trace_id.into_child())
                .with_request_id(request_id)
                .with_client_trace_id(x_client_trace_id.cloned());
        }

        // No tracer was found. Let's analyze the headers to see if we should emit a new trace ID.

        let forced = headers.contains_key(X_ENVOY_FORCE_TRACE);

        let dont_trace =
            || TraceContext::new().with_client_trace_id(x_client_trace_id.cloned()).with_request_id(request_id.clone());

        // 1. trigger: x_client_trace_id...

        if let Some(trace_id) = x_client_trace_id.and_then(|val| <u128 as FromHeaderValue>::from(val).ok()) {
            if forced
                || (Self::should_sample(self.tracing.client_sampling)
                    && Self::should_sample(self.tracing.overall_sampling))
            {
                return TraceContext::new()
                    .with_child(TraceInfo::new(true, TraceProvider::W3CTraceContext, Some(trace_id)))
                    .with_request_id(request_id)
                    .with_client_trace_id(x_client_trace_id.cloned());
            }

            return dont_trace();
        }

        // Trigger: x_request_id...

        if let Some(trace_id) = request_id.as_ref().and_then(|val| <u128 as FromHeaderValue>::from(val.as_ref()).ok()) {
            println!("here!");
            if forced
                || (Self::should_sample(self.tracing.random_sampling)
                    && Self::should_sample(self.tracing.overall_sampling))
            {
                return TraceContext::new()
                    .with_child(TraceInfo::new(true, TraceProvider::W3CTraceContext, Some(trace_id)))
                    .with_request_id(request_id)
                    .with_client_trace_id(x_client_trace_id.cloned());
            }

            return dont_trace();
        }

        // last resort: if forced, create a new trace ID.
        //

        if forced {
            return TraceContext::new()
                .with_child(TraceInfo::new(true, TraceProvider::W3CTraceContext, None))
                .with_request_id(request_id)
                .with_client_trace_id(x_client_trace_id.cloned());
        }

        // ... otherwise don't trace.
        //

        dont_trace()
    }

    pub fn update_tracing_headers<B>(&self, context: &TraceContext, request: &mut Request<B>) {
        let headers = request.headers_mut();
        let parent = context.parent.as_ref();
        context.child.as_ref().inspect(|child| {
            _ = (*child).update_headers(headers, parent);
        });
    }

    fn should_sample(sampling: Option<BoundedU16<0, 100>>) -> bool {
        sampling.is_none_or(|s| {
            let random_value: u16 = rand::random::<u16>() % 100; // Random value between 0 and 99
            random_value < s.get()
        })
    }
}
