pub mod context;
pub mod grammar;
pub mod operator;
pub mod types;

// use compact_str::CompactString;
use context::Context;
use operator::{Category, Operator};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::fmt::{self, Display, Formatter, Write};
use std::io::Write as IoWrite;
use std::sync::Arc;
use thiserror::Error;

use crate::grammar::EnvoyGrammar;

pub const DEFAULT_ENVOY_FORMAT: &str = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(X-ENVOY-ORIGINAL-PATH?:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#;

#[derive(Error, Debug)]
pub enum FormatError {
    #[error("invalid operator `{0}`")]
    InvalidOperator(String),
    #[error("missing argument `{0}`")]
    MissingArgument(String),
    #[error("missing braket `{0}`")]
    MissingBracket(String),
    #[error("missing delimiter `{0}`")]
    MissingDelimiter(String),
    #[error("empty argument `{0}`")]
    EmptyArgument(String),
    #[error("invalid request argument `{0}`")]
    InvalidRequestArg(String),
    #[error("invalid response argument `{0}`")]
    InvalidResponseArg(String),
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
    // Compact(CompactString),
    None,
}

#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct LogFormatter {
    pub template: Arc<Vec<Template>>,
    pub format: Vec<StringType>,
}

impl Default for LogFormatter {
    fn default() -> Self {
        LogFormatter::try_new(DEFAULT_ENVOY_FORMAT).unwrap()
    }
}

impl LogFormatter {
    pub fn try_new(input: &str) -> Result<LogFormatter, FormatError> {
        let template = EnvoyGrammar::parse(input)?;
        let mut format: Vec<StringType> = Vec::with_capacity(template.len());

        for t in template.iter() {
            match t {
                Template::Char(c) => format.push(StringType::Char(*c)),
                Template::Literal(smol_str) => format.push(StringType::Smol(smol_str.clone())),
                _ => format.push(StringType::None),
            }
        }

        Ok(LogFormatter { template: Arc::new(template), format })
    }

    pub fn with_context<'a, C: Context>(&mut self, ctx: &'a C) -> &mut Self {
        let context_category = C::category();
        for (part, out) in self.template.iter().zip(&mut self.format.iter_mut()) {
            if let Template::Placeholder(op, category) = part {
                if *category == context_category {
                    *out = ctx.eval_part(op);
                }
            }
        }

        self
    }

    pub fn write_to<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<usize> {
        let mut total_bytes = 0;
        for (_part, out) in self.template.iter().zip(self.format.iter()) {
            total_bytes += match out {
                StringType::Smol(s) => IoWrite::write(w, s.as_bytes())?,
                StringType::Char(c) => {
                    let mut buf = [0u8; 4];
                    let bytes = c.encode_utf8(&mut buf).as_bytes();
                    IoWrite::write(w, bytes)?
                },
                // StringType::Compact(s) => IoWrite::write(w, s.as_bytes())?,
                #[cfg(debug_assertions)]
                _ => IoWrite::write(w, format!("%{:?}%", _part).as_bytes())?,
                #[cfg(not(debug_assertions))]
                _ => IoWrite::write(w, "?".as_bytes())?,
            };
        }

        Ok(total_bytes)
    }

    #[inline]
    pub fn is_fully_formatted(&self) -> bool {
        self.format.iter().all(|f| *f != StringType::None)
    }
}

impl Display for LogFormatter {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (part, out) in self.template.iter().zip(self.format.iter()) {
            match out {
                StringType::Smol(s) => f.write_str(s.as_ref())?,
                StringType::Char(c) => f.write_char(*c)?,
                // StringType::Compact(s) => f.write_str(s.as_ref())?,
                _ => f.write_str(&format!("%{:?}%", part))?,
            };
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
            DownstreamRequest, DownstreamResponse, FinishContext, InitContext, UpstreamContext, UpstreamRequest,
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

        let mut formatter = LogFormatter::try_new("%REQ(:PATH)%").unwrap();
        let expected = "/";

        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_original_path() {
        let mut req = build_request();
        req.headers_mut().append("X-ENVOY-ORIGINAL-PATH", HeaderValue::from_static("/original"));

        let mut formatter = LogFormatter::try_new("%REQ(X-ENVOY-ORIGINAL-PATH?:PATH)%").unwrap();
        let expected = "/original";

        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_method() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new("%REQ(:METHOD)%").unwrap();
        println!("FORMATTER: {:?}", formatter);
        let expected = "GET";
        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_protocol() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new("%PROTOCOL%").unwrap();
        println!("FORMATTER: {:?}", formatter);
        let expected = "HTTP/1.1";
        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_upstream_protocol() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new("%UPSTREAM_PROTOCOL%").unwrap();
        println!("FORMATTER: {:?}", formatter);
        let expected = "HTTP/1.1";
        formatter.with_context(&UpstreamRequest(&req));
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_scheme() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new("%REQ(:SCHEME)%").unwrap();
        let expected = "https";
        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_authority() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new("%REQ(:AUTHORITY)%").unwrap();
        let expected = "www.rust-lang.org";
        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_user_agent() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new("%REQ(USER-AGENT)%").unwrap();
        let expected = "awesome/1.0";
        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_unevaluated_operator() {
        let formatter = LogFormatter::try_new("%REQ(USER-AGENT)%").unwrap();
        let actual = format!("{}", &formatter);
        println!("{actual}");
    }

    #[test]
    fn default_format_string() {
        let req = build_request();
        let resp = build_response();
        let mut formatter = LogFormatter::try_new(DEFAULT_ENVOY_FORMAT).unwrap();
        formatter.with_context(&InitContext { start_time: std::time::SystemTime::now() });
        formatter.with_context(&UpstreamContext { authority: &req.uri().authority().unwrap() });
        formatter.with_context(&DownstreamRequest(&req));
        formatter.with_context(&DownstreamResponse(&resp));
        formatter.with_context(&FinishContext {
            duration: Duration::from_millis(100),
            bytes_received: 128,
            bytes_sent: 256,
            response_flags: ResponseFlags::NO_HEALTHY_UPSTREAM,
        });
        assert!(formatter.is_fully_formatted());
        println!("{}", &formatter);
    }
}
