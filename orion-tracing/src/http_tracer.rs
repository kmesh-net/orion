use bounded_integer::BoundedU16;
use http::header::HOST;
use http::{HeaderValue, Request};
use opentelemetry::global::BoxedSpan;

use opentelemetry::trace::{Span, SpanContext, TraceContextExt, TraceState, Tracer};
use opentelemetry::KeyValue;
use opentelemetry::{Context, SpanId, TraceFlags, TraceId};
use rand::Rng;

use crate::attributes::*;
use orion_configuration::config::network_filters::tracing::{TracingConfig, TracingKey};
use orion_http_header::*;
use orion_interner::StringInterner;

use crate::trace_context::TraceContext;
use crate::{
    request_id::RequestId,
    trace_info::{FromHeaderValue, TraceInfo, TraceProvider},
};

pub use opentelemetry::trace::SpanKind as OtelSpanKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTracer {
    tracing: Option<TracingConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanKind {
    Server,
    Client,
}

#[derive(Debug)]
pub enum SpanName<'a, B> {
    Host(&'a Request<B>),
    Str(&'a str),
}

impl From<SpanKind> for OtelSpanKind {
    fn from(value: SpanKind) -> Self {
        match value {
            SpanKind::Server => OtelSpanKind::Server,
            SpanKind::Client => OtelSpanKind::Client,
        }
    }
}

impl HttpTracer {
    /// Creates a new `Tracer` instance with the provided tracing configuration.
    pub fn new() -> Self {
        Self { tracing: None }
    }

    pub fn with_config(self, tracing: TracingConfig) -> Self {
        Self { tracing: Some(tracing) }
    }

    /// Analyzes request headers to decide if and how to trace.
    ///
    pub fn try_build_trace_context<B>(&self, req: &Request<B>, request_id: Option<RequestId>) -> Option<TraceContext> {
        let Some(tracing) = self.tracing.as_ref() else {
            return None;
        };

        let headers = req.headers();
        let x_client_trace_id = headers.get(X_CLIENT_TRACE_ID);

        if let Some(parent_trace_id) = TraceInfo::extract_from(headers).ok().flatten() {
            return Some(
                TraceContext::new(None)
                    .with_parent(parent_trace_id.clone())
                    .with_request_id(request_id)
                    .with_client_trace_id(x_client_trace_id.cloned()),
            );
        }

        // No tracer was found. Let's analyze the headers to see if we should emit a new trace ID.

        let forced = headers.contains_key(X_ENVOY_FORCE_TRACE);

        let dont_trace = || {
            TraceContext::new(None).with_client_trace_id(x_client_trace_id.cloned()).with_request_id(request_id.clone())
        };

        // 1. trigger: x_client_trace_id...

        if let Some(trace_id) = x_client_trace_id.and_then(|val| <u128 as FromHeaderValue>::from(val).ok()) {
            if forced || (Self::should_sample(tracing.client_sampling) && Self::should_sample(tracing.overall_sampling))
            {
                return Some(
                    TraceContext::new(Some(trace_id))
                        .with_request_id(request_id)
                        .with_should_sample(true)
                        .with_client_trace_id(x_client_trace_id.cloned()),
                );
            }

            return Some(dont_trace());
        }

        // Trigger: x_request_id...

        if let Some(trace_id) = request_id.as_ref().and_then(|val| <u128 as FromHeaderValue>::from(val.as_ref()).ok()) {
            if forced || (Self::should_sample(tracing.random_sampling) && Self::should_sample(tracing.overall_sampling))
            {
                return Some(
                    TraceContext::new(Some(trace_id))
                        .with_request_id(request_id)
                        .with_should_sample(true)
                        .with_client_trace_id(x_client_trace_id.cloned()),
                );
            }

            return Some(dont_trace());
        }

        // last resort: if forced, create a new trace ID.
        //

        if forced {
            return Some(
                TraceContext::new(None)
                    .with_child(TraceInfo::new(true, TraceProvider::W3CTraceContext, None))
                    .with_request_id(request_id)
                    .with_should_sample(true)
                    .with_client_trace_id(x_client_trace_id.cloned()),
            );
        }

        // ... otherwise don't trace.
        //

        Some(dont_trace())
    }

    #[inline]
    fn should_sample(sampling: Option<BoundedU16<0, 100>>) -> bool {
        sampling.is_none_or(|s| {
            let random_value: u16 = rand::rng().random_range(0..100); // Random value between 0 and 99
            random_value < s.get()
        })
    }

    pub fn update_tracing_headers<B>(&self, context: &TraceContext, request: &mut Request<B>) {
        let headers = request.headers_mut();
        context.map_child(|ref child| {
            _ = child.update_headers(headers);
        });
    }

    /// Builds a span from the given request headers and the TraceContext.
    ///
    pub fn try_create_span<B>(
        &self,
        trace_context: Option<&TraceContext>,
        tracing_key: &TracingKey,
        span_kind: SpanKind,
        span_name: SpanName<'_, B>,
    ) -> Option<BoxedSpan> {
        // if trace_context is None, don't create a span
        let Some(trace_context) = trace_context else {
            return None;
        };

        let Some(ref tracing_conf) = self.tracing else {
            return None;
        };

        // if trace_context should not sample, don't create a span
        if !trace_context.should_sample() {
            return None;
        }

        // if span type is client and tracing config does not spawn upstream span, don't create a span
        if matches!(span_kind, SpanKind::Client) && !tracing_conf.spawn_upstream_span {
            return None;
        }

        // spawn a new child...
        trace_context.spawn_child();

        // Get the tracer and the plan for the child span.
        let tracer = crate::get_otel_tracer(tracing_key)?;

        let (span_id, trace_id) = trace_context.map_child(|child| (child.span_id(), child.trace_id()))?;

        let span_name = match span_name {
            SpanName::Host(request) => get_host_from_request(request).unwrap_or("no-host").to_string(),
            SpanName::Str(s) => s.to_string(),
        };

        // Build the span configuration from the trace context.
        let span_builder = tracer
            .span_builder(span_name)
            .with_span_id(SpanId::from_bytes(span_id.unwrap_or(0).to_be_bytes()))
            .with_trace_id(TraceId::from_bytes(trace_id.to_be_bytes()))
            .with_kind(span_kind.into());

        let parent_context = if let Some(parent_info) = trace_context.parent() {
            // If a parent exists, create a remote SpanContext for it.
            let remote_parent_span_context = SpanContext::new(
                TraceId::from_bytes(parent_info.trace_id().to_be_bytes()),
                SpanId::from_bytes(parent_info.span_id().unwrap_or(0).to_be_bytes()),
                if parent_info.sampled() { TraceFlags::SAMPLED } else { TraceFlags::default() },
                true, // is_remote
                TraceState::default(),
            );
            // Wrap it in a generic OpenTelemetry Context.
            Context::new().with_remote_span_context(remote_parent_span_context)
        } else {
            // If there's no parent, we start from a clean root context.
            Context::new()
        };

        // Start the span and immediately wrap it for manual management.
        let span: BoxedSpan = span_builder.start_with_context(tracer.as_ref(), &parent_context);

        // Wrap it in a Arc/Mutex for safe mutation and for shared ownership.
        Some(span)
    }

    pub fn set_attributes_from_request<B>(&self, span: &mut BoxedSpan, request: &Request<B>) {
        // Add few attributes, based on the request
        span.set_attributes([
            KeyValue::new(HTTP_REQUEST_METHOD, request.method().as_str().to_static_str()), // the number of HTTP methods is small, hence we can use the string interner here..
            KeyValue::new(URL_FULL, request.uri().to_string()),
            KeyValue::new(URL_PATH, request.uri().path().to_string()),
            KeyValue::new(NETWORK_PROTOCOL_NAME, "http"),
            KeyValue::new(
                NETWORK_PROTOCOL_VERSION,
                request.version().to_static_str().split_once('/').map(|(_, ver)| ver).unwrap_or("unknow"),
            ),
            KeyValue::new(
                USER_AGENT_ORIGINAL,
                request
                    .headers()
                    .get(::http::header::USER_AGENT)
                    .unwrap_or(&HeaderValue::from_static("unknown"))
                    .to_str()
                    .unwrap_or("invalid-user-agent")
                    .to_string(),
            ),
        ]);

        request.uri().query().inspect(|q| span.set_attribute(KeyValue::new(URL_QUERY, q.to_string())));
        request.uri().scheme().inspect(|s| span.set_attribute(KeyValue::new(URL_SCHEME, s.as_str().to_static_str())));
    }
}

pub fn get_host_from_request<B>(request: &Request<B>) -> Option<&str> {
    // 1. Prefer the ':authority' header, common in HTTP/2.
    if let Some(authority) = request.headers().get(":authority") {
        if let Ok(host) = authority.to_str() {
            return Some(host);
        }
    }

    // 2. Fallback to the 'Host' header, standard for HTTP/1.1.
    if let Some(host_header) = request.headers().get(HOST) {
        if let Ok(host) = host_header.to_str() {
            return Some(host);
        }
    }

    // 3. As a last resort, try to get the host from the URI.
    request.uri().host()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bounded_integer::BoundedU16;

    #[test]
    fn build_no_tracer() {
        let no_tracer = HttpTracer::new();
        let req = Request::builder().uri("http://example.com").body(()).unwrap();
        let context = no_tracer.try_build_trace_context(&req, None);
        assert!(context.is_none());
    }

    #[test]
    fn build_default_http_tracer() {
        let config = TracingConfig {
            client_sampling: None,
            random_sampling: None,
            overall_sampling: None,
            verbose: false,
            max_path_tag_length: None,
            spawn_upstream_span: false,
            provider: None,
        };

        let tracer = HttpTracer::new().with_config(config);
        let req = Request::builder().uri("http://example.com").body(()).unwrap();
        let context = tracer.try_build_trace_context(&req, None);

        let Some(ref ctx) = context else {
            panic!("context should be Some");
        };

        assert!(ctx.parent().is_none());
        assert!(ctx.map_child(|_| ()).is_none());
        assert!(ctx.is_root_node());
        assert!(!ctx.should_sample());
    }

    #[test]
    fn http_tracer_and_req_with_invalid_x_request_id() {
        let config = TracingConfig {
            client_sampling: None,
            random_sampling: None,
            overall_sampling: None,
            verbose: false,
            max_path_tag_length: None,
            spawn_upstream_span: false,
            provider: None,
        };

        let tracer = HttpTracer::new().with_config(config);
        let req = Request::builder().uri("http://example.com").header("X-Request-ID", "12345").body(()).unwrap();
        let context = tracer.try_build_trace_context(&req, None);

        let Some(ref ctx) = context else {
            panic!("context should be Some");
        };

        assert!(ctx.parent().is_none());
        assert!(ctx.map_child(|_| ()).is_none());
        assert!(ctx.is_root_node());
        assert!(!ctx.should_sample());
    }

    #[test]
    fn http_tracer_and_req_with_valid_x_request_id() {
        let config = TracingConfig {
            client_sampling: None,
            random_sampling: None,
            overall_sampling: None,
            verbose: false,
            max_path_tag_length: None,
            spawn_upstream_span: false,
            provider: None,
        };

        let tracer = HttpTracer::new().with_config(config);
        let req = Request::builder()
            .uri("http://example.com")
            .header("X-Request-ID", "9b13f5d0-9c42-4d27-a34f-08b7a8a5601a")
            .body(())
            .unwrap();

        let context = tracer.try_build_trace_context(
            &req,
            Some(RequestId::Propagate(HeaderValue::from_static("9b13f5d0-9c42-4d27-a34f-08b7a8a5601a"))),
        );

        let span = tracer.try_create_span(
            context.as_ref(),
            &TracingKey("test".into(), 0),
            SpanKind::Server,
            SpanName::Str::<()>("test"),
        );
        assert!(span.is_none()); // None because OTEL is not configured. The child node is created anyway.

        let Some(ref ctx) = context else {
            panic!("context should be Some");
        };

        assert!(ctx.parent().is_none());
        assert!(ctx.map_child(|_| ()).is_some());
        assert!(ctx.is_root_node());
        assert_eq!(ctx.map_child(|child| child.sampled()), Some(true));
        assert!(ctx.should_sample());
    }

    #[test]
    fn http_tracer_and_req_with_traceparent_with_sampling_100_percent() {
        let config = TracingConfig {
            client_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(100) }),
            random_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(100) }),
            overall_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(100) }),
            verbose: false,
            max_path_tag_length: None,
            spawn_upstream_span: false,
            provider: None,
        };

        let tracer = HttpTracer::new().with_config(config);
        let req = Request::builder()
            .uri("http://example.com")
            .header("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .body(())
            .unwrap();

        let context = tracer.try_build_trace_context(&req, None);

        let span = tracer.try_create_span(
            context.as_ref(),
            &TracingKey("test".into(), 0),
            SpanKind::Server,
            SpanName::Str::<()>("test"),
        );
        assert!(span.is_none()); // None because OTEL is not configured. The child node is created anyway.

        let Some(ref ctx) = context else {
            panic!("context should be Some");
        };

        assert!(ctx.parent().is_some());
        assert!(ctx.map_child(|_| ()).is_some());
        assert!(!ctx.is_root_node());
        assert_eq!(ctx.map_child(|child| child.sampled()), Some(true));
        assert!(ctx.should_sample());
    }

    #[test]
    fn http_tracer_and_req_with_traceparent_and_sampling_0_percent() {
        let config = TracingConfig {
            client_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(0) }),
            random_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(0) }),
            overall_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(0) }),
            verbose: false,
            max_path_tag_length: None,
            spawn_upstream_span: false,
            provider: None,
        };

        let tracer = HttpTracer::new().with_config(config);
        let req = Request::builder()
            .uri("http://example.com")
            .header("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .body(())
            .unwrap();

        let context = tracer.try_build_trace_context(&req, None);

        let span = tracer.try_create_span(
            context.as_ref(),
            &TracingKey("test".into(), 0),
            SpanKind::Server,
            SpanName::Str::<()>("test"),
        );
        assert!(span.is_none()); // None because OTEL is not configured. The child node is created anyway.

        let Some(ref ctx) = context else {
            panic!("context should be Some");
        };

        assert!(ctx.parent().is_some());
        assert!(ctx.map_child(|_| ()).is_some());
        assert!(!ctx.is_root_node());
        assert!(ctx.should_sample());
    }

    #[test]
    fn http_tracer_and_req_with_traceparent_with_sampled_false() {
        let config = TracingConfig {
            client_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(100) }),
            random_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(100) }),
            overall_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(100) }),
            verbose: false,
            max_path_tag_length: None,
            spawn_upstream_span: false,
            provider: None,
        };

        let tracer = HttpTracer::new().with_config(config);
        let req = Request::builder()
            .uri("http://example.com")
            .header("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00")
            .body(())
            .unwrap();

        let context = tracer.try_build_trace_context(&req, None);

        let span = tracer.try_create_span(
            context.as_ref(),
            &TracingKey("test".into(), 0),
            SpanKind::Server,
            SpanName::Str::<()>("test"),
        );
        assert!(span.is_none()); // None because OTEL is not configured. The child node is not created.

        let Some(ref ctx) = context else {
            panic!("context should be Some");
        };

        assert!(ctx.parent().is_some());
        assert!(ctx.map_child(|_| ()).is_none());
        assert!(!ctx.is_root_node());
        assert!(!ctx.should_sample());
    }

    #[test]
    fn build_no_tracer_create_span() {
        let no_tracer = HttpTracer::new();
        let req = Request::builder().uri("http://example.com").body(()).unwrap();
        let trace_context = no_tracer.try_build_trace_context(&req, None);
        assert!(trace_context.is_none());
        let span = no_tracer.try_create_span(
            trace_context.as_ref(),
            &TracingKey("test".into(), 0),
            SpanKind::Server,
            SpanName::Str::<()>("test"),
        );
        assert!(span.is_none());
    }

    #[test]
    fn build_tracer_no_sample_create_none_span() {
        let config = TracingConfig {
            client_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(100) }),
            random_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(100) }),
            overall_sampling: Some(unsafe { BoundedU16::<0, 100>::new_unchecked(100) }),
            verbose: false,
            max_path_tag_length: None,
            spawn_upstream_span: false,
            provider: None,
        };

        let tracer = HttpTracer::new().with_config(config);
        let req = Request::builder()
            .uri("http://example.com")
            .header("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00")
            .body(())
            .unwrap();

        let trace_context = tracer.try_build_trace_context(&req, None);
        assert!(trace_context.is_some());

        let span = tracer.try_create_span(
            trace_context.as_ref(),
            &TracingKey("test".into(), 0),
            SpanKind::Server,
            SpanName::Str::<()>("test"),
        );
        assert!(span.is_none());
        assert!(trace_context.as_ref().and_then(|ctx| ctx.parent()).is_some());

        println!("PARENT: {:?}", trace_context.as_ref().map(|ctx| ctx.parent()));
        println!("CHILD : {:?}", trace_context.as_ref().map(|ctx| ctx.map_child(|c| c.clone())));

        assert!(trace_context.as_ref().and_then(|ctx| ctx.map_child(|_| ())).is_none());
    }
}
