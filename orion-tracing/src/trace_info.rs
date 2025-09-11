// Copyright 2025 The kmesh Authors
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

use http::{header::InvalidHeaderValue, HeaderMap, HeaderValue};
use orion_http_header::*;
use rand::Rng;
use std::{
    fmt::{self, Display, Write},
    str::Utf8Error,
};
use uuid::Uuid;

macro_rules! insert_header {
    ($headers:expr, $header_name:expr, $len:literal, $fmt_literal:literal, $( $value:expr ),* ) => {
        {
            // This block creates a new scope to ensure the buffer is temporary.
            let mut buffer = arrayvec::ArrayString::<$len>::new();
            let _ = std::write!(&mut buffer, $fmt_literal, $( $value ),*);
            $headers.insert($header_name, http::HeaderValue::from_str(&buffer)?);
        }
    };
}

#[derive(thiserror::Error, Debug)]
pub enum TraceError {
    #[error("Invalid trace ID format")]
    InvalidFormat,
    #[error("Trace ID parsing error: {0}")]
    ParseError(#[from] Utf8Error),
    #[error("Invalid HeaderValue error: {0}")]
    InvalidHeaderValueError(#[from] InvalidHeaderValue),
    #[error("Format error: {0}")]
    FormatError(#[from] std::fmt::Error),
    #[error("Missing SpanId")]
    MissingSpanId,
    #[error("Missing TraceId")]
    MissingTraceId,
}

#[derive(thiserror::Error, Debug)]
pub enum ParseUuidError {
    #[error("Http header: {0}")]
    ToStrError(#[from] http::header::ToStrError),
    #[error("Uuuid error: {0}")]
    UuuidError(#[from] uuid::Error),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TraceProvider {
    W3CTraceContext,
    B3,
    B3Multi,
    Uber, // Jeager support
}

pub trait FromHeaderValue: Sized {
    type Err;
    fn from(value: &HeaderValue) -> Result<Self, Self::Err>;
}

impl FromHeaderValue for u128 {
    type Err = ParseUuidError;

    fn from(value: &HeaderValue) -> Result<Self, Self::Err> {
        let uuid = Uuid::parse_str(value.to_str()?)?;
        Ok(uuid.as_u128())
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct TraceInfo {
    trace_id: u128,
    span_id: Option<u64>,   // the span ID of this node, if set.
    parent_id: Option<u64>, // the parent span ID of this node.
    provider: TraceProvider,
    sampled: bool,
}

impl Display for TraceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.provider {
            TraceProvider::W3CTraceContext => {
                write!(
                    f,
                    "00-{:032x}-{:016x}-{}",
                    self.trace_id,
                    self.span_id.unwrap_or(0),
                    if self.sampled { "01" } else { "00" }
                )
            },
            TraceProvider::B3 | TraceProvider::B3Multi => match self.parent_id {
                Some(_) => {
                    write!(
                        f,
                        "{:032x}-{:016x}-{}-{:016x}",
                        self.trace_id,
                        self.span_id.unwrap_or(0),
                        if self.sampled { "1" } else { "0" },
                        self.parent_id.unwrap_or(0)
                    )
                },
                None => {
                    write!(
                        f,
                        "{:032x}-{:016x}-{}",
                        self.trace_id,
                        self.span_id.unwrap_or(0),
                        if self.sampled { "1" } else { "0" }
                    )
                },
            },
            TraceProvider::Uber => {
                write!(
                    f,
                    "{:032x}:{:016x}:{:016x}:{}",
                    self.trace_id,
                    self.span_id.unwrap_or(0),
                    self.parent_id.unwrap_or(0),
                    if self.sampled { "1" } else { "0" }
                )
            },
        }
    }
}

struct Hex<T>(T);
impl<T: fmt::LowerHex> fmt::Debug for Hex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

impl std::fmt::Debug for TraceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceInfo")
            .field("trace_id", &Hex(self.trace_id))
            .field("span_id", &Hex(self.span_id.unwrap_or(0)))
            .field("parent_id", &Hex(self.parent_id.unwrap_or(0)))
            .field("provider", &self.provider)
            .field("sampled", &self.sampled)
            .finish()
    }
}

impl TraceInfo {
    // Generate a new trace ID if Orion is the root of the trace...
    pub fn new(sampled: bool, provider: TraceProvider, trace_id: Option<u128>) -> Self {
        let span_id = None;
        let parent_id = None;
        let trace_id = trace_id.unwrap_or_else(|| uuid::Uuid::new_v4().as_u128());
        TraceInfo { trace_id, span_id, parent_id, provider, sampled }
    }

    pub fn into_child(self) -> Self {
        // Generate a new span ID for the child span
        let mut rng = rand::rng();
        let new_span_id = rng.random::<u64>();
        let prev_span_id = self.span_id;
        TraceInfo { span_id: Some(new_span_id), parent_id: prev_span_id, ..self }
    }

    // Parse a trace ID from the request headers, returning None if no trace ID is found. The error
    // is reported when the trace ID is found and invalid.
    pub fn extract_from(headers: &HeaderMap) -> Result<Option<Self>, TraceError> {
        // Check for W3C Trace Context first
        //
        if let Some(value) = headers.get(TRACEPARENT).and_then(|v| v.to_str().ok()) {
            let tp = traceparent::parse(value).map_err(|_| TraceError::InvalidFormat)?;
            return Ok(Some(TraceInfo {
                trace_id: tp.trace_id(),
                span_id: Some(tp.parent_id()), // despite the name, this is actual span id of this node.
                parent_id: None,
                provider: TraceProvider::W3CTraceContext,
                sampled: tp.sampled(),
            }));
        }

        // Check for Jeager header (uber-trce-id)
        //
        if let Some(value) = headers.get(UBER_TRACE_ID).and_then(|v| v.to_str().ok()) {
            let parts: Vec<&str> = value.split(':').collect();
            if parts.len() == 4 {
                // Uber trace id ID is the first part, parent ID is the second part
                let trace_id = u128::from_str_radix(parts[0], 16).map_err(|_| TraceError::InvalidFormat)?;
                let span_id = Some(u64::from_str_radix(parts[1], 16).map_err(|_| TraceError::InvalidFormat)?);
                let parent_id = Some(u64::from_str_radix(parts[2], 16).map_err(|_| TraceError::InvalidFormat)?);
                let sampled: bool = match parts[3] {
                    "1" => Ok(true),
                    "0" => Ok(false),
                    _ => Err(TraceError::InvalidFormat), // Invalid sampled value
                }?;

                return Ok(Some(TraceInfo { trace_id, span_id, parent_id, provider: TraceProvider::Uber, sampled }));
            }
            return Err(TraceError::InvalidFormat);
        }

        // Check for B3 header
        //
        if let Some(value) = headers.get(B3).and_then(|v| v.to_str().ok()) {
            let parts: Vec<&str> = value.split('-').collect();
            if parts.len() >= 3 {
                // let mut rng = rand::rng();
                // B3 trace ID is the first part, parent ID is the second part
                let trace_id = u128::from_str_radix(parts[0], 16).map_err(|_| TraceError::InvalidFormat)?;
                let span_id = Some(u64::from_str_radix(parts[1], 16).map_err(|_| TraceError::InvalidFormat)?);
                let sampled: bool = match parts[2] {
                    "1" => Ok(true),
                    "0" => Ok(false),
                    _ => Err(TraceError::InvalidFormat), // Invalid sampled value
                }?;

                let parent_id = if parts.len() == 4 {
                    Some(u64::from_str_radix(parts[3], 16).map_err(|_| TraceError::InvalidFormat)?)
                } else {
                    None
                };

                return Ok(Some(TraceInfo { trace_id, span_id, parent_id, provider: TraceProvider::B3, sampled }));
            }
            return Err(TraceError::InvalidFormat);
        }

        // Check for B3 multi-headers
        //

        // First, get both headers as Options
        let trace_id_header = headers.get(X_B3_TRACEID);
        let span_id_header = headers.get(X_B3_SPANID);
        let incoming_parent_id_header = headers.get(X_B3_PARENTSPANID);

        // Now, match on the tuple to handle all cases explicitly
        match (trace_id_header, span_id_header) {
            // Case 1: BOTH headers are present. This is the success path.
            (Some(trace_id), Some(span_id)) => {
                // Parse Trace ID
                let trace_id = {
                    let id = trace_id.to_str().map_err(|_| TraceError::InvalidFormat)?;
                    u128::from_str_radix(id, 16).map_err(|_| TraceError::InvalidFormat)?
                };

                // Parse Span ID
                let span_id = {
                    let id = span_id.to_str().map_err(|_| TraceError::InvalidFormat)?;
                    Some(u64::from_str_radix(id, 16).map_err(|_| TraceError::InvalidFormat)?)
                };

                let parent_id = incoming_parent_id_header
                    .map(|incoming_parent_id| -> Result<u64, TraceError> {
                        let id = incoming_parent_id.to_str().map_err(|_| TraceError::InvalidFormat)?;
                        u64::from_str_radix(id, 16).map_err(|_| TraceError::InvalidFormat)
                    })
                    .transpose()?;

                // Parse Sampled (optional)
                let sampled =
                    headers.get(X_B3_SAMPLED).and_then(|v| v.to_str().ok()).map_or(Ok(false), |s| match s {
                        "1" => Ok(true),
                        "0" => Ok(false),
                        _ => Err(TraceError::InvalidFormat), // Invalid sampled value
                    })?;

                // Return the successfully parsed context
                return Ok(Some(TraceInfo { trace_id, parent_id, span_id, provider: TraceProvider::B3Multi, sampled }));
            },
            // Case 2: ONLY ONE of the two is present. This is an error.
            (Some(_), None) => return Err(TraceError::MissingSpanId),
            (None, Some(_)) => return Err(TraceError::MissingTraceId),

            // Case 3: NEITHER is present. Do nothing and continue execution.
            (None, None) => {
                // The context is not here, so we continue to the next check
            },
        }

        // If no trace ID found, return None
        Ok(None)
    }

    pub fn update_headers(&self, headers: &mut HeaderMap) -> Result<(), TraceError> {
        match self.provider {
            TraceProvider::Uber => {
                insert_header!(headers, UBER_TRACE_ID, 80, "{}", self);
            },
            TraceProvider::W3CTraceContext => {
                insert_header!(headers, TRACEPARENT, 80, "{}", self);
            },
            TraceProvider::B3 => {
                insert_header!(headers, B3, 80, "{}", self);
            },

            TraceProvider::B3Multi => {
                insert_header!(headers, X_B3_SAMPLED, 1, "{}", if self.sampled { "1" } else { "0" });

                // The trace ID.
                insert_header!(headers, X_B3_TRACEID, 32, "{:032x}", self.trace_id);

                // A new span ID is generated for each request.
                if self.span_id.is_some() {
                    insert_header!(headers, X_B3_SPANID, 16, "{:016x}", self.span_id.unwrap_or(0));
                }

                // The parent span ID...
                if self.parent_id.is_some() {
                    insert_header!(headers, X_B3_PARENTSPANID, 16, "{:016x}", self.parent_id.unwrap_or(0));
                }
            },
        }
        Ok(())
    }

    pub fn span_id(&self) -> Option<u64> {
        self.span_id
    }

    pub fn trace_id(&self) -> u128 {
        self.trace_id
    }

    pub fn sampled(&self) -> bool {
        self.sampled
    }

    pub fn provider(&self) -> TraceProvider {
        self.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;

    #[test]
    fn trace_id_new() {
        let trace_id = TraceInfo::new(true, TraceProvider::W3CTraceContext, None);
        assert!(trace_id.sampled);
        assert!(trace_id.trace_id > 0);
        assert!(trace_id.span_id.is_none());
        assert!(trace_id.parent_id.is_none());
    }

    #[test]
    fn trace_id_try_new() {
        let req_id = HeaderValue::from_static("1234567890abcdef1234567890abcdef");
        let req_id = <u128 as FromHeaderValue>::from(&req_id).unwrap();
        let trace_id = TraceInfo::new(true, TraceProvider::W3CTraceContext, Some(req_id));
        assert!(trace_id.sampled);
        assert_eq!(trace_id.trace_id, req_id);
        assert!(trace_id.span_id.is_none());
        assert!(trace_id.parent_id.is_none());
    }

    #[test]
    fn trace_id_parse_valid_traceparent() {
        let req = Request::builder()
            .header(TRACEPARENT, "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .body(())
            .unwrap();

        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(trace_id.trace_id, 0x4bf92f3577b34da6a3ce929d0e0e4736);
        assert_eq!(trace_id.span_id, Some(0x00f067aa0ba902b7));
        assert_eq!(trace_id.parent_id, None);
        assert!(trace_id.sampled);
    }

    #[test]
    fn trace_id_parse_invalid_traceparent() {
        let req = Request::builder().header(TRACEPARENT, "invalid-format").body(()).unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        let req = Request::builder().header(TRACEPARENT, "00-abc-def").body(()).unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        // short trace ID
        let req = Request::builder().header(TRACEPARENT, "00-4bf92f35-00f067aa0ba902b7-01").body(()).unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        // Invalid hex character in trace ID
        let req = Request::builder()
            .header(TRACEPARENT, "00-4bf92f3577b34da6a3ce929d0e0e473g-00f067aa0ba902b7-01")
            .body(())
            .unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));
    }

    #[test]
    fn trace_id_parse_valid_b3_without_parent() {
        let req =
            Request::builder().header(B3, "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1").body(()).unwrap();

        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(trace_id.trace_id, 0x80f198ee56343ba864fe8b2a57d3eff7);
        assert_eq!(trace_id.span_id, Some(0xe457b5a2e4d86bd1));
        assert_eq!(trace_id.parent_id, None);
        assert!(trace_id.sampled);
    }

    #[test]
    fn trace_id_parse_valid_b3_with_parent() {
        let req = Request::builder()
            .header(B3, "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1-05e3ac9a4f6e3b90")
            .body(())
            .unwrap();

        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(trace_id.trace_id, 0x80f198ee56343ba864fe8b2a57d3eff7);
        assert_eq!(trace_id.span_id, Some(0xe457b5a2e4d86bd1));
        assert_eq!(trace_id.parent_id, Some(0x05e3ac9a4f6e3b90));
        assert!(trace_id.sampled);
    }

    #[test]
    fn trace_id_parse_invalid_b3() {
        let req = Request::builder().header(B3, "invalid-format").body(()).unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        // Too few parts
        let req = Request::builder().header(B3, "80f198ee56343ba864fe8b2a57d3eff7").body(()).unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        // Invalid hex character in trace ID
        let req = Request::builder()
            .header(B3, "xx80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1-05e3ac9a4f6e3b90g")
            .body(())
            .unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        // Invalid hex character in span ID
        let req = Request::builder()
            .header(B3, "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1XXX-1-05e3ac9a4f6e3b90g")
            .body(())
            .unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        // Invalid sampled flag
        let req = Request::builder()
            .header(B3, "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-2-05e3ac9a4f6e3b90")
            .body(())
            .unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));
    }

    #[test]
    fn trace_id_parse_valid_b3_multi() {
        let req = Request::builder()
            .header(X_B3_TRACEID, "80f198ee56343ba864fe8b2a57d3eff7")
            .header(X_B3_SPANID, "e457b5a2e4d86bd1")
            .header(X_B3_PARENTSPANID, "e217b5a1e4d96baa")
            .header(X_B3_SAMPLED, "1")
            .body(())
            .unwrap();

        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(trace_id.trace_id, 0x80f198ee56343ba864fe8b2a57d3eff7);
        assert_eq!(trace_id.span_id, Some(0xe457b5a2e4d86bd1));
        assert_eq!(trace_id.parent_id, Some(0xe217b5a1e4d96baa));
        assert!(trace_id.sampled);
    }

    #[test]
    fn trace_id_parse_valid_b3_multi_without_parent() {
        let req = Request::builder()
            .header(X_B3_TRACEID, "80f198ee56343ba864fe8b2a57d3eff7")
            .header(X_B3_SPANID, "e457b5a2e4d86bd1")
            .header(X_B3_SAMPLED, "1")
            .body(())
            .unwrap();

        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(trace_id.trace_id, 0x80f198ee56343ba864fe8b2a57d3eff7);
        assert_eq!(trace_id.span_id, Some(0xe457b5a2e4d86bd1));
        assert_eq!(trace_id.parent_id, None);
        assert!(trace_id.sampled);
    }

    #[test]
    fn trace_id_parse_invalid_b3_multi_incomplete() {
        // Missing span-id header
        let req = Request::builder().header(X_B3_TRACEID, "80f198ee56343ba864fe8b2a57d3eff7").body(()).unwrap();

        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::MissingSpanId));

        // Missing trace-id header
        let req = Request::builder().header(X_B3_SPANID, "e457b5a2e4d86bd1").body(()).unwrap();

        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::MissingTraceId));
    }

    #[test]
    fn trace_id_child() {
        let req = Request::builder()
            .header(B3, "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1-05e3ac9a4f6e3b90")
            .body(())
            .unwrap();

        let parent = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        let child = parent.clone().into_child();

        // Child should have the same trace ID and a new span ID...
        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.provider, parent.provider);
        assert_eq!(child.sampled, parent.sampled);

        assert_eq!(child.parent_id, parent.span_id);
        assert_ne!(child.span_id, parent.span_id);
        assert_ne!(child.parent_id, parent.parent_id);
    }

    #[test]
    fn trace_id_update_request_traceparent_root() {
        let mut req = Request::builder().body(()).unwrap();
        let parent = TraceInfo::new(true, TraceProvider::W3CTraceContext, None);
        let child = parent.clone().into_child();

        child.update_headers(req.headers_mut()).unwrap();

        let headers = req.headers();
        assert!(headers.contains_key(TRACEPARENT));
        let child = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.provider, parent.provider);
        assert_eq!(child.sampled, parent.sampled);
        assert_eq!(child.parent_id, parent.span_id);
    }

    #[test]
    fn trace_id_update_request_traceparent_child() {
        let mut req = Request::builder()
            .header(TRACEPARENT, "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .body(())
            .unwrap();
        let parent = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        let child = parent.clone().into_child();
        child.update_headers(req.headers_mut()).unwrap();

        let headers = req.headers();
        assert!(headers.contains_key(TRACEPARENT));
        let child = TraceInfo::extract_from(req.headers()).unwrap().unwrap();

        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.provider, parent.provider);
        assert_eq!(child.sampled, parent.sampled);
        // assert_eq!(child.parent_id, parent.span_id); traceparet do not support parent_id...

        assert!(child.parent_id.is_none());
        assert_ne!(child.span_id, parent.span_id);

        println!("PARENT: {parent:?}");
        println!("CHILD : {child:?}");
    }

    #[test]
    fn trace_id_update_request_b3_root() {
        let mut req = Request::builder().body(()).unwrap();
        let parent = TraceInfo::new(true, TraceProvider::B3, None);
        parent.update_headers(req.headers_mut()).unwrap();
        let child = parent.clone().into_child();
        child.update_headers(req.headers_mut()).unwrap();

        let headers = req.headers();
        assert!(headers.contains_key(B3));
        let child = TraceInfo::extract_from(req.headers_mut()).unwrap().unwrap();
        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.provider, parent.provider);
        assert_eq!(child.sampled, parent.sampled);

        assert_eq!(child.parent_id, parent.span_id);
    }

    #[test]
    fn trace_id_update_request_b3_child() {
        let mut req = Request::builder()
            .header(B3, "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1-05e3ac9a4f6e3b90")
            .body(())
            .unwrap();
        let parent = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        let child = parent.clone().into_child();
        child.update_headers(req.headers_mut()).unwrap();
        let headers = req.headers();
        assert!(headers.contains_key(B3));
        let child = TraceInfo::extract_from(req.headers()).unwrap().unwrap();

        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.provider, parent.provider);
        assert_eq!(child.sampled, parent.sampled);

        assert_eq!(child.parent_id, parent.span_id);
    }

    #[test]
    fn trace_id_update_request_b3multi_root() {
        let mut req = Request::builder().body(()).unwrap();
        let parent = TraceInfo::new(true, TraceProvider::B3Multi, None);
        let child = parent.clone().into_child();
        child.update_headers(req.headers_mut()).unwrap();

        let headers = req.headers();
        assert!(headers.contains_key(X_B3_SAMPLED));
        assert!(headers.contains_key(X_B3_SPANID));
        assert!(headers.contains_key(X_B3_TRACEID));

        let child = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.provider, parent.provider);
        assert_eq!(child.sampled, parent.sampled);
        assert_eq!(child.parent_id, parent.span_id);
    }

    #[test]
    fn trace_id_update_request_b3multi_child() {
        let mut req = Request::builder()
            .header(X_B3_TRACEID, "80f198ee56343ba864fe8b2a57d3eff7")
            .header(X_B3_SPANID, "e457b5a2e4d86bd1")
            .header(X_B3_SAMPLED, "1")
            .header(X_B3_PARENTSPANID, "05e3ac9a4f6e3b90")
            .body(())
            .unwrap();
        let parent = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        let child = parent.clone().into_child();
        child.update_headers(req.headers_mut()).unwrap();
        let headers = req.headers();
        assert!(headers.contains_key(X_B3_SAMPLED));
        assert!(headers.contains_key(X_B3_SPANID));
        assert!(headers.contains_key(X_B3_TRACEID));
        assert!(headers.contains_key(X_B3_PARENTSPANID));
        let child = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.provider, parent.provider);
        assert_eq!(child.sampled, parent.sampled);
        assert_eq!(child.parent_id, parent.span_id);
    }

    #[test]
    fn trace_id_parse_valid_uber_trace_id() {
        let req = Request::builder()
            .header(UBER_TRACE_ID, "4bf92f3577b34da6a3ce929d0e0e4736:00f067aa0ba902b7:0:1")
            .body(())
            .unwrap();

        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(trace_id.trace_id, 0x4bf92f3577b34da6a3ce929d0e0e4736);
        assert_eq!(trace_id.span_id, Some(0x00f067aa0ba902b7));
        assert_eq!(trace_id.parent_id, Some(0));
        assert!(trace_id.sampled);
    }

    #[test]
    fn trace_id_parse_invalid_uber_trace_id() {
        let req = Request::builder().header(UBER_TRACE_ID, "invalid-format").body(()).unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        // Too few parts
        let req = Request::builder().header(UBER_TRACE_ID, "80f198ee56343ba864fe8b2a57d3eff7").body(()).unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        // Invalid hex character in trace ID
        let req = Request::builder()
            .header(UBER_TRACE_ID, "xx80f198ee56343ba864fe8b2a57d3eff7:e457b5a2e4d86bd1:05e3ac9a4f6e3b90g:1")
            .body(())
            .unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        // Invalid hex character in span ID
        let req = Request::builder()
            .header(UBER_TRACE_ID, "80f198ee56343ba864fe8b2a57d3eff7:e457b5a2e4d86bd1XXX:05e3ac9a4f6e3b90g:1")
            .body(())
            .unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));

        // Invalid sampled flag
        let req = Request::builder()
            .header(UBER_TRACE_ID, "80f198ee56343ba864fe8b2a57d3eff7:e457b5a2e4d86bd1:05e3ac9a4f6e3b90:2")
            .body(())
            .unwrap();
        let result = TraceInfo::extract_from(req.headers());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TraceError::InvalidFormat));
    }
}
