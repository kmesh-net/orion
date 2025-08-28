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

use http::{HeaderMap, HeaderValue, header::InvalidHeaderValue};
use orion_http_header::*;
use rand::Rng;
use std::{fmt::Write, str::Utf8Error};
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

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TraceInfo {
    trace_id: u128,
    span_id: u64, // can either be a new span ID or the parent span ID
    provider: TraceProvider,
    sampled: bool,
}

impl TraceInfo {
    // Generate a new trace ID if Orion is the root of the trace...
    pub fn new(sampled: bool, provider: TraceProvider, trace_id: Option<u128>) -> Self {
        // Generate a new trace ID and span ID
        let mut rng = rand::thread_rng();
        let span_id = rng.gen::<u64>();
        let trace_id = trace_id.unwrap_or_else(|| uuid::Uuid::new_v4().as_u128());
        TraceInfo { trace_id, span_id, provider, sampled }
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
                span_id: tp.parent_id(),
                provider: TraceProvider::W3CTraceContext,
                sampled: tp.sampled(),
            }));
        }

        // Check for B3 header
        //
        if let Some(value) = headers.get(B3).and_then(|v| v.to_str().ok()) {
            let parts: Vec<&str> = value.split('-').collect();
            if parts.len() >= 3 {
                // B3 trace ID is the first part, parent ID is the second part
                let trace_id = u128::from_str_radix(parts[0], 16).map_err(|_| TraceError::InvalidFormat)?;
                let span_id = u64::from_str_radix(parts[1], 16).map_err(|_| TraceError::InvalidFormat)?;
                let sampled: bool = match parts[2] {
                    "1" => Ok(true),
                    "0" => Ok(false),
                    _ => Err(TraceError::InvalidFormat), // Invalid sampled value
                }?;

                return Ok(Some(TraceInfo { trace_id, span_id, provider: TraceProvider::B3, sampled }));
            }
            return Err(TraceError::InvalidFormat);
        }

        // Check for B3 multi-headers
        //

        // First, get both headers as Options
        let trace_id_header = headers.get(X_B3_TRACEID);
        let span_id_header = headers.get(X_B3_SPANID);

        // Now, match on the tuple to handle all cases explicitly
        match (trace_id_header, span_id_header) {
            // Case 1: BOTH headers are present. This is the success path.
            (Some(trace_id), Some(span_id_val)) => {
                // Parse Trace ID
                let trace_id_str = trace_id.to_str().map_err(|_| TraceError::InvalidFormat)?;
                let trace_id = u128::from_str_radix(trace_id_str, 16).map_err(|_| TraceError::InvalidFormat)?;

                // Parse Span ID
                let span_id_str = span_id_val.to_str().map_err(|_| TraceError::InvalidFormat)?;
                let span_id = u64::from_str_radix(span_id_str, 16).map_err(|_| TraceError::InvalidFormat)?;

                // Parse Sampled (optional)
                let sampled =
                    headers.get(X_B3_SAMPLED).and_then(|v| v.to_str().ok()).map_or(Ok(false), |s| match s {
                        "1" => Ok(true),
                        "0" => Ok(false),
                        _ => Err(TraceError::InvalidFormat), // Invalid sampled value
                    })?;

                // Return the successfully parsed context
                return Ok(Some(TraceInfo { trace_id, span_id, provider: TraceProvider::B3Multi, sampled }));
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

    pub fn into_child(self) -> Self {
        // Generate a new span ID for the child span
        let mut rng = rand::thread_rng();
        let span_id = rng.gen::<u64>();
        TraceInfo { trace_id: self.trace_id, span_id, provider: self.provider, sampled: self.sampled }
    }

    pub fn sampled(&self) -> bool {
        self.sampled
    }

    pub fn update_headers(&self, headers: &mut HeaderMap, parent: Option<&TraceInfo>) -> Result<(), TraceError> {
        match self.provider {
            TraceProvider::W3CTraceContext => {
                insert_header!(
                    headers,
                    TRACEPARENT,
                    64,
                    "00-{:032x}-{:016x}-{}",
                    self.trace_id,
                    self.span_id,
                    if self.sampled { "01" } else { "00" }
                );
            },
            TraceProvider::B3 => {
                if let Some(parent) = parent {
                    insert_header!(
                        headers,
                        B3,
                        80,
                        "{:032x}-{:016x}-{}-{:016x}",
                        self.trace_id,
                        self.span_id,
                        if self.sampled { "1" } else { "0" },
                        parent.span_id
                    );
                } else {
                    // If no parent, we can skip it.
                    insert_header!(
                        headers,
                        B3,
                        51,
                        "{:032x}-{:016x}-{}",
                        self.trace_id,
                        self.span_id,
                        if self.sampled { "1" } else { "0" }
                    );
                }
            },

            TraceProvider::B3Multi => {
                insert_header!(headers, X_B3_SAMPLED, 1, "{}", if self.sampled { "1" } else { "0" });

                // A new span ID is generated for each request.
                insert_header!(headers, X_B3_SPANID, 16, "{:016x}", self.span_id);

                // If not present, add the trace ID.
                if !headers.contains_key(X_B3_TRACEID) {
                    insert_header!(headers, X_B3_TRACEID, 32, "{:032x}", self.trace_id);
                }

                // if the parent span ID is present, we can add it.
                if let Some(parent) = parent {
                    insert_header!(headers, X_B3_PARENTSPANID, 16, "{:016x}", parent.span_id);
                }
            },
        }
        Ok(())
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
        assert!(trace_id.span_id > 0);
    }

    #[test]
    fn trace_id_try_new() {
        let req_id = HeaderValue::from_static("1234567890abcdef1234567890abcdef");
        let req_id = <u128 as FromHeaderValue>::from(&req_id).unwrap();
        let trace_id = TraceInfo::new(true, TraceProvider::W3CTraceContext, Some(req_id));
        assert!(trace_id.sampled);
        assert_eq!(trace_id.trace_id, req_id);
        assert!(trace_id.span_id > 0);
    }

    #[test]
    fn trace_id_parse_valid_traceparent() {
        let req = Request::builder()
            .header(TRACEPARENT, "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .body(())
            .unwrap();

        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(trace_id.trace_id, 0x4bf92f3577b34da6a3ce929d0e0e4736);
        assert!(trace_id.sampled);
        assert!(trace_id.span_id > 0);
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
    fn trace_id_parse_valid_b3() {
        let req = Request::builder()
            .header(B3, "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1-05e3ac9a4f6e3b90")
            .body(())
            .unwrap();

        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(trace_id.trace_id, 0x80f198ee56343ba864fe8b2a57d3eff7);
        assert!(trace_id.sampled);
        assert!(trace_id.span_id > 0);
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
            .header(X_B3_SAMPLED, "1")
            .body(())
            .unwrap();

        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(trace_id.trace_id, 0x80f198ee56343ba864fe8b2a57d3eff7);
        assert!(trace_id.sampled);
        assert_eq!(trace_id.span_id, 0xe457b5a2e4d86bd1);
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
        let parent = TraceInfo::new(true, TraceProvider::W3CTraceContext, None);
        let child = parent.clone().into_child();

        println!("Parent: {parent:?}");
        println!("Parent: {child:?}");

        // Child should have the same trace ID and a new span ID
        assert_eq!(child.trace_id, parent.trace_id);
        assert_ne!(child.span_id, parent.span_id);
        assert_eq!(child.provider, parent.provider);
        assert_eq!(child.sampled, parent.sampled);
    }

    #[test]
    fn trace_id_update_request_traceparent_root() {
        let mut req = Request::builder().body(()).unwrap();
        let trace_id = TraceInfo::new(true, TraceProvider::W3CTraceContext, None);
        trace_id.update_headers(req.headers_mut(), None).unwrap();

        let headers = req.headers();
        assert!(headers.contains_key(TRACEPARENT));
        let new_trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(new_trace_id.trace_id, trace_id.trace_id);
        assert_eq!(new_trace_id.span_id, trace_id.span_id);
        assert_eq!(new_trace_id.provider, trace_id.provider);
        assert_eq!(new_trace_id.sampled, trace_id.sampled);
    }

    #[test]
    fn trace_id_update_request_traceparent_child() {
        let mut req = Request::builder()
            .header(TRACEPARENT, "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .body(())
            .unwrap();
        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        let child = trace_id.clone().into_child();
        child.update_headers(req.headers_mut(), Some(&trace_id)).unwrap();
        let headers = req.headers();
        assert!(headers.contains_key(TRACEPARENT));
        let new_trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();

        assert_eq!(new_trace_id.trace_id, trace_id.trace_id);
        assert_ne!(new_trace_id.span_id, trace_id.span_id);
        assert_eq!(new_trace_id.provider, trace_id.provider);
        assert_eq!(new_trace_id.sampled, trace_id.sampled);
    }

    #[test]
    fn trace_id_update_request_b3_root() {
        let mut req = Request::builder().body(()).unwrap();
        let trace_id = TraceInfo::new(true, TraceProvider::B3, None);
        trace_id.update_headers(req.headers_mut(), None).unwrap();

        let headers = req.headers();
        assert!(headers.contains_key(B3));
        let new_trace_id = TraceInfo::extract_from(req.headers_mut()).unwrap().unwrap();
        assert_eq!(new_trace_id.trace_id, trace_id.trace_id);
        assert_eq!(new_trace_id.span_id, trace_id.span_id);
        assert_eq!(new_trace_id.provider, trace_id.provider);
        assert_eq!(new_trace_id.sampled, trace_id.sampled);
    }

    #[test]
    fn trace_id_update_request_b3_child() {
        let mut req = Request::builder()
            .header(B3, "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1-05e3ac9a4f6e3b90")
            .body(())
            .unwrap();
        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        let child = trace_id.clone().into_child();
        child.update_headers(req.headers_mut(), Some(&trace_id)).unwrap();
        let headers = req.headers();
        assert!(headers.contains_key(B3));
        let new_trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();

        assert_eq!(new_trace_id.trace_id, trace_id.trace_id);
        assert_ne!(new_trace_id.span_id, trace_id.span_id);
        assert_eq!(new_trace_id.provider, trace_id.provider);
        assert_eq!(new_trace_id.sampled, trace_id.sampled);
    }

    #[test]
    fn trace_id_update_request_b3multi_root() {
        let mut req = Request::builder().body(()).unwrap();
        let trace_id = TraceInfo::new(true, TraceProvider::B3Multi, None);
        trace_id.update_headers(req.headers_mut(), None).unwrap();

        let headers = req.headers();
        assert!(headers.contains_key(X_B3_SAMPLED));
        assert!(headers.contains_key(X_B3_SPANID));
        assert!(headers.contains_key(X_B3_TRACEID));
        let new_trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(new_trace_id.trace_id, trace_id.trace_id);
        assert_eq!(new_trace_id.span_id, trace_id.span_id);
        assert_eq!(new_trace_id.provider, trace_id.provider);
        assert_eq!(new_trace_id.sampled, trace_id.sampled);
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
        let trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        let child = trace_id.clone().into_child();
        child.update_headers(req.headers_mut(), Some(&trace_id)).unwrap();
        let headers = req.headers();
        assert!(headers.contains_key(X_B3_SAMPLED));
        assert!(headers.contains_key(X_B3_SPANID));
        assert!(headers.contains_key(X_B3_TRACEID));
        assert!(headers.contains_key(X_B3_PARENTSPANID));
        let new_trace_id = TraceInfo::extract_from(req.headers()).unwrap().unwrap();
        assert_eq!(new_trace_id.trace_id, trace_id.trace_id);
        assert_ne!(new_trace_id.span_id, trace_id.span_id);
        assert_eq!(new_trace_id.provider, trace_id.provider);
        assert_eq!(new_trace_id.sampled, trace_id.sampled);
    }
}
