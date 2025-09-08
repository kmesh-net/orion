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

pub mod context;
pub mod grammar;
pub mod operator;
pub mod types;

use crate::grammar::EnvoyGrammar;
use context::Context;
use operator::{Category, Operator, NUM_OPERATOR_CATEGORIES};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::{
    fmt::{self, Display, Formatter, Write},
    io::Write as IoWrite,
    sync::Arc,
};
use thiserror::Error;
use thread_local::ThreadLocal;

pub const DEFAULT_ACCESS_LOG_FORMAT: &str = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(X-ENVOY-ORIGINAL-PATH?:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%"
"#;

pub const DEFAULT_ISTIO_ACCESS_LOG_FORMAT: &str = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(X-ENVOY-ORIGINAL-PATH?:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %RESPONSE_CODE_DETAILS% %CONNECTION_TERMINATION_DETAILS% "%UPSTREAM_TRANSPORT_FAILURE_REASON%" %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%" %UPSTREAM_CLUSTER_RAW% %UPSTREAM_LOCAL_ADDRESS%  %DOWNSTREAM_LOCAL_ADDRESS% %DOWNSTREAM_REMOTE_ADDRESS% %REQUESTED_SERVER_NAME% %ROUTE_NAME%"
"#;

#[derive(Error, Debug, Eq, PartialEq)]
pub enum FormatError {
    #[error("invalid operator `{0}`")]
    InvalidOperator(String),
    #[error("missing argument `{0}`")]
    MissingArgument(String),
    #[error("missing bracket `{0}`")]
    MissingBracket(String),
    #[error("missing delimiter `{0}`")]
    MissingDelimiter(String),
    #[error("empty argument `{0}`")]
    EmptyArgument(String),
    #[error("invalid request argument `{0}`")]
    InvalidRequestArg(String),
    #[error("invalid response argument `{0}`")]
    InvalidResponseArg(String),

    #[error("invalid operator index `{0}`")]
    InvalidOperatorIndex(#[from] std::num::TryFromIntError),
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum Template {
    Char(char),
    Literal(SmolStr),
    Placeholder(Operator, Category), // eg. ("DURATION", Pattern::Duration, None), (Pattern::Req, Some(":METHOD"))
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum StringType {
    Char(char),
    Smol(SmolStr),
    Bytes(Box<[u8]>),
    None,
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
struct LogFormatterConf {
    templates: Vec<Template>,
    indices: [Vec<u8>; NUM_OPERATOR_CATEGORIES],
    omit_empty_values: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(clippy::unsafe_derive_deserialize)]
pub struct LogFormatter {
    main: LogFormatterConf,
    #[serde(skip_serializing, skip_deserializing)]
    local: ThreadLocal<Arc<LogFormatterConf>>,
    format: Vec<StringType>,
}

impl PartialEq for LogFormatter {
    fn eq(&self, other: &Self) -> bool {
        self.main == other.main && self.format == other.format
    }
}

impl Eq for LogFormatter {}

impl Clone for LogFormatter {
    fn clone(&self) -> Self {
        LogFormatter { main: self.main.clone(), local: ThreadLocal::new(), format: self.format.clone() }
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct LogFormatterLocal {
    local: Arc<LogFormatterConf>,
    format: Vec<StringType>,
}

impl LogFormatter {
    pub fn try_new(input: &str, omit_empty_values: bool) -> Result<LogFormatter, FormatError> {
        let templates = EnvoyGrammar::parse(input)?;
        let mut indices: [Vec<u8>; NUM_OPERATOR_CATEGORIES] = std::array::from_fn(|_| vec![]);

        for (i, part) in templates.iter().enumerate() {
            if let Template::Placeholder(_, cat) = part {
                let idx = cat.bits().trailing_zeros();
                indices[idx as usize].push(u8::try_from(i)?);
            }
        }

        let mut format: Vec<StringType> = Vec::with_capacity(templates.len());

        for t in &templates {
            match t {
                Template::Char(c) => format.push(StringType::Char(*c)),
                Template::Literal(smol_str) => format.push(StringType::Smol(smol_str.clone())),
                Template::Placeholder(_, _) => format.push(StringType::None),
            }
        }

        Ok(LogFormatter {
            main: LogFormatterConf { templates, indices, omit_empty_values },
            local: ThreadLocal::new(),
            format,
        })
    }

    pub fn local_clone(&self) -> LogFormatterLocal {
        LogFormatterLocal {
            local: Arc::clone(self.local.get_or(|| Arc::new(self.main.clone()))),
            format: self.format.clone(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.format.is_empty()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.format.len()
    }
}

impl LogFormatterLocal {
    pub fn with_context<C: Context>(&mut self, ctx: &C) -> &Self {
        for cat in C::categories() {
            unsafe {
                for idx in self.local.indices.get_unchecked(cat.bits().trailing_zeros() as usize) {
                    if let Template::Placeholder(op, _) = self.local.templates.get_unchecked(*idx as usize) {
                        *self.format.get_unchecked_mut(*idx as usize) = ctx.eval_part(op);
                    }
                }
            }
        }
        self
    }

    #[inline]
    pub fn into_message(self) -> FormattedMessage {
        FormattedMessage { format: self.format, omit_empty_values: self.local.omit_empty_values }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.format.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.format.is_empty()
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct FormattedMessage {
    omit_empty_values: bool,
    format: Vec<StringType>,
}

impl FormattedMessage {
    pub fn write_to<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<usize> {
        let mut total_bytes = 0;
        for out in &self.format {
            total_bytes += match out {
                StringType::Smol(s) => IoWrite::write(w, s.as_bytes())?,
                StringType::Char(c) => {
                    let mut buf = [0u8; 4];
                    let bytes = c.encode_utf8(&mut buf).as_bytes();
                    IoWrite::write(w, bytes)?
                },
                StringType::Bytes(v) => IoWrite::write(w, v.as_ref())?,
                StringType::None => {
                    if self.omit_empty_values {
                        0
                    } else {
                        IoWrite::write(w, "-".as_bytes())?
                    }
                },
            };
        }

        Ok(total_bytes)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.format.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.format.is_empty()
    }
}

impl Display for FormattedMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for out in &self.format {
            match out {
                StringType::Smol(s) => f.write_str(s.as_ref())?,
                StringType::Char(c) => f.write_char(*c)?,
                StringType::Bytes(v) => f.write_str(&String::from_utf8_lossy(v))?,
                StringType::None => {
                    if !self.omit_empty_values {
                        f.write_str("-")?
                    }
                },
            }
        }
        Ok(())
    }
}

pub trait Grammar {
    fn parse(input: &str) -> Result<Vec<Template>, FormatError>;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use http::{HeaderValue, Request, Response, StatusCode};

    use crate::{
        context::{
            DownstreamContext, DownstreamResponse, FinishContext, InitContext, UpstreamContext, UpstreamRequest,
        },
        types::ResponseFlags,
    };

    use super::*;

    fn build_request() -> Request<()> {
        Request::builder().uri("https://www.rust-lang.org/").header("User-Agent", "awesome/1.0").body(()).unwrap()
    }

    fn build_response() -> Response<()> {
        let builder = Response::builder().status(StatusCode::OK);
        builder.body(()).unwrap()
    }

    #[test]
    fn test_request_path() {
        let mut req = build_request();
        req.headers_mut().append("X-ENVOY-ORIGINAL-PATH", HeaderValue::from_static("/original"));

        let source = LogFormatter::try_new("%REQ(:PATH)%", false).unwrap();
        let mut formatter = source.local_clone();
        let expected = "/";

        formatter.with_context(&DownstreamContext {
            request: &req,
            request_head_size: 0,
            trace_id: None,
            server_name: None,
        });
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_original_path() {
        let mut req = build_request();
        req.headers_mut().append("X-ENVOY-ORIGINAL-PATH", HeaderValue::from_static("/original"));

        let source = LogFormatter::try_new("%REQ(X-ENVOY-ORIGINAL-PATH?:PATH)%", false).unwrap();
        let mut formatter = source.local_clone();
        let expected = "/original";

        formatter.with_context(&DownstreamContext {
            request: &req,
            request_head_size: 0,
            trace_id: None,
            server_name: None,
        });
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_method() {
        let req = build_request();
        let source = LogFormatter::try_new("%REQ(:METHOD)%", false).unwrap();
        let mut formatter = source.local_clone();
        println!("FORMATTER: {formatter:?}");
        let expected = "GET";
        formatter.with_context(&DownstreamContext {
            request: &req,
            request_head_size: 0,
            trace_id: None,
            server_name: None,
        });
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_protocol() {
        let req = build_request();
        let source = LogFormatter::try_new("%PROTOCOL%", false).unwrap();
        let mut formatter = source.local_clone();
        println!("FORMATTER: {formatter:?}");
        let expected = "HTTP/1.1";
        formatter.with_context(&DownstreamContext {
            request: &req,
            request_head_size: 0,
            trace_id: None,
            server_name: None,
        });
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_upstream_protocol() {
        let req = build_request();
        let source = LogFormatter::try_new("%UPSTREAM_PROTOCOL%", false).unwrap();
        let mut formatter = source.local_clone();
        println!("FORMATTER: {formatter:?}");
        let expected = "HTTP/1.1";
        formatter.with_context(&UpstreamRequest(&req));
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_scheme() {
        let req = build_request();
        let source = LogFormatter::try_new("%REQ(:SCHEME)%", false).unwrap();
        let mut formatter = source.local_clone();
        let expected = "https";
        formatter.with_context(&DownstreamContext {
            request: &req,
            request_head_size: 0,
            trace_id: None,
            server_name: None,
        });
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_authority() {
        let req = build_request();
        let source = LogFormatter::try_new("%REQ(:AUTHORITY)%", false).unwrap();
        let mut formatter = source.local_clone();
        let expected = "www.rust-lang.org";
        formatter.with_context(&DownstreamContext {
            request: &req,
            request_head_size: 0,
            trace_id: None,
            server_name: None,
        });
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_user_agent() {
        let req = build_request();
        let source = LogFormatter::try_new("%REQ(USER-AGENT)%", false).unwrap();
        let mut formatter = source.local_clone();
        let expected = "awesome/1.0";
        formatter.with_context(&DownstreamContext {
            request: &req,
            request_head_size: 0,
            trace_id: None,
            server_name: None,
        });
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_unevaluated_operator() {
        let source = LogFormatter::try_new("%REQ(USER-AGENT)%", false).unwrap();
        let formatter = source.local_clone();
        let actual = format!("{}", &formatter.into_message());
        println!("{actual}");
    }

    #[test]
    fn default_format_string() {
        let req = build_request();
        let resp = build_response();
        let source = LogFormatter::try_new(DEFAULT_ACCESS_LOG_FORMAT, false).unwrap();
        let mut formatter = source.local_clone();
        formatter.with_context(&InitContext { start_time: std::time::SystemTime::now() });
        formatter.with_context(&DownstreamContext {
            request: &req,
            request_head_size: 0,
            trace_id: None,
            server_name: None,
        });
        formatter.with_context(&UpstreamContext {
            authority: Some(req.uri().authority().unwrap()),
            cluster_name: Some("test_cluster"),
            route_name: "test_route",
        });
        formatter.with_context(&DownstreamResponse { response: &resp, response_head_size: 0 });
        formatter.with_context(&FinishContext {
            duration: Duration::from_millis(100),
            bytes_received: 128,
            bytes_sent: 256,
            response_flags: ResponseFlags::NO_HEALTHY_UPSTREAM,
            upstream_failure: None,
            response_code_details: None,
            connection_termination_details: None,
        });
        println!("{}", &formatter.into_message());
    }

    #[test]
    fn default_istio_format_string() {
        let req = build_request();
        let resp = build_response();
        let source = LogFormatter::try_new(DEFAULT_ISTIO_ACCESS_LOG_FORMAT, false).unwrap();
        let mut formatter = source.local_clone();
        formatter.with_context(&InitContext { start_time: std::time::SystemTime::now() });
        formatter.with_context(&DownstreamContext {
            request: &req,
            request_head_size: 0,
            trace_id: None,
            server_name: None,
        });
        formatter.with_context(&UpstreamContext {
            authority: Some(req.uri().authority().unwrap()),
            cluster_name: Some("test_cluster"),
            route_name: "test_route",
        });
        formatter.with_context(&DownstreamResponse { response: &resp, response_head_size: 0 });
        formatter.with_context(&FinishContext {
            duration: Duration::from_millis(100),
            bytes_received: 128,
            bytes_sent: 256,
            response_flags: ResponseFlags::NO_HEALTHY_UPSTREAM,
            upstream_failure: None,
            response_code_details: None,
            connection_termination_details: None,
        });
        println!("{}", &formatter.into_message());
    }

    #[test]
    fn test_sizes() {
        println!("Vec:       {}", std::mem::size_of::<Vec<u8>>());
        println!("SmolStr:   {}", std::mem::size_of::<SmolStr>());
        println!("Box<[u8]>: {}", std::mem::size_of::<Box<[u8]>>());
    }
}
