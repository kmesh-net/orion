use opentelemetry::{global::BoxedSpan, trace::Span};
use parking_lot::Mutex;

#[derive(Debug)]
pub struct SpanState {
    pub server_span: Mutex<Option<BoxedSpan>>, // SERVER span
    pub client_span: Mutex<Option<BoxedSpan>>, // CLIENT span
}

impl SpanState {
    #[inline]
    pub fn new(server_span: Option<BoxedSpan>) -> Self {
        SpanState { server_span: Mutex::new(server_span), client_span: Mutex::new(None) }
    }

    pub fn end(&self) {
        // emit the server span if created...
        let mut guard = self.server_span.lock();
        if let Some(ref mut span) = *guard {
            span.end();
        }

        // emit the client span if created...
        let mut guard = self.client_span.lock();
        if let Some(ref mut span) = *guard {
            span.end();
        }
    }
}

// The following macros simplify the manipulation of server and client spans.
// For example: the macro with_server_span! implements the following code:
// if let Some(span_state) = self.span_state.as_ref() {
//     if let Some(ref mut span) = *span_state.server_span.lock() {
//         span.set_attribute(KeyValue::new(HTTP_RESPONSE_STATUS_CODE, 500));
//     }
// }

#[macro_export]
#[cfg(feature = "tracing")]
macro_rules! with_server_span {
    ($span_state:expr, $closure:expr) => {
        if let Some(valid_span_state) = $span_state.as_ref() {
            if let Some(span) = valid_span_state.server_span.lock().as_mut() {
                ($closure)(span);
            }
        }
    };
}

#[macro_export]
#[cfg(feature = "tracing")]
macro_rules! with_client_span {
    ($span_state:expr, $closure:expr) => {
        if let Some(valid_span_state) = $span_state.as_ref() {
            if let Some(span) = valid_span_state.client_span.lock().as_mut() {
                ($closure)(span);
            }
        }
    };
}

#[macro_export]
#[cfg(not(feature = "tracing"))]
macro_rules! with_server_span {
    ($span_state:expr, $closure:expr) => {
        // No-op if metrics feature is not enabled
        if false {
            // This creates a tuple containing the results of the expressions,
            // effectively "using" them without generating runtime code.
            let _ = $span_state;
            let _ = $closure;
        }
    };
}

#[macro_export]
#[cfg(not(feature = "tracing"))]
macro_rules! with_client_span {
    ($span_state:expr, $closure:expr) => {
        // No-op if metrics feature is not enabled
        if false {
            // This creates a tuple containing the results of the expressions,
            // effectively "using" them without generating runtime code.
            let _ = $span_state;
            let _ = $closure;
        }
    };
}
