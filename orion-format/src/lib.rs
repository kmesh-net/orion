pub mod context;
pub mod grammar;
pub mod operator;
pub mod types;

use crate::grammar::EnvoyGrammar;
use context::Context;
use operator::{Category, Operator};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::fmt::{self, Display, Formatter, Write};
use std::io::Write as IoWrite;
use std::sync::Arc;
use strum::EnumCount;
use thiserror::Error;

pub const DEFAULT_ENVOY_FORMAT: &str = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(X-ENVOY-ORIGINAL-PATH?:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%"
"#;

#[derive(Error, Debug)]
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
struct IndexedTemplate {
    templates: Vec<Template>,
    indices: [Vec<u8>; Category::COUNT],
}

#[allow(clippy::unsafe_derive_deserialize)]
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct LogFormatter {
    catalog: Arc<IndexedTemplate>,
    format: Vec<StringType>,
}

impl Default for LogFormatter {
    fn default() -> Self {
        if let Ok(log) = LogFormatter::try_new(DEFAULT_ENVOY_FORMAT) {
            log
        } else {
            unimplemented!("Failed to create default LogFormatter")
        }
    }
}

impl LogFormatter {
    pub fn try_new(input: &str) -> Result<LogFormatter, FormatError> {
        let templates = EnvoyGrammar::parse(input)?;
        let mut indices: [Vec<u8>; Category::COUNT] = std::array::from_fn(|_| vec![]);

        for (i, part) in templates.iter().enumerate() {
            if let Template::Placeholder(_, category) = part {
                indices[*category as usize].push(u8::try_from(i)?);
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

        Ok(LogFormatter { catalog: Arc::new(IndexedTemplate { templates, indices }), format })
    }

    pub fn with_context<C: Context>(&mut self, ctx: &C) -> &Self {
        for idx in &self.catalog.indices[C::category() as usize] {
            unsafe {
                if let Template::Placeholder(op, _) = self.catalog.templates.get_unchecked(*idx as usize) {
                    *self.format.get_unchecked_mut(*idx as usize) = ctx.eval_part(op);
                }
            }
        }

        self
    }

    #[inline]
    pub fn into_message(self) -> FormattedMessage {
        FormattedMessage { format: self.format }
    }

    #[inline]
    pub fn is_fully_formatted(&self) -> bool {
        self.format.iter().all(|f| *f != StringType::None)
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
    format: Vec<StringType>,
}

impl FormattedMessage {
    #[inline]
    pub fn is_fully_formatted(&self) -> bool {
        self.format.iter().all(|f| *f != StringType::None)
    }

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
                StringType::None => IoWrite::write(w, "?".as_bytes())?, // this corresponds to an operator not evaluated from contexts
            };
        }

        Ok(total_bytes)
    }
}

impl Display for FormattedMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for out in &self.format {
            match out {
                StringType::Smol(s) => f.write_str(s.as_ref())?,
                StringType::Char(c) => f.write_char(*c)?,
                StringType::Bytes(v) => f.write_str(&String::from_utf8_lossy(v))?,
                StringType::None => f.write_str("?")?,
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

        let source = LogFormatter::try_new("%REQ(:PATH)%").unwrap();
        let mut formatter = source.clone();
        let expected = "/";

        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_original_path() {
        let mut req = build_request();
        req.headers_mut().append("X-ENVOY-ORIGINAL-PATH", HeaderValue::from_static("/original"));

        let source = LogFormatter::try_new("%REQ(X-ENVOY-ORIGINAL-PATH?:PATH)%").unwrap();
        let mut formatter = source.clone();
        let expected = "/original";

        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_method() {
        let req = build_request();
        let source = LogFormatter::try_new("%REQ(:METHOD)%").unwrap();
        let mut formatter = source.clone();
        println!("FORMATTER: {formatter:?}");
        let expected = "GET";
        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_protocol() {
        let req = build_request();
        let source = LogFormatter::try_new("%PROTOCOL%").unwrap();
        let mut formatter = source.clone();
        println!("FORMATTER: {formatter:?}");
        let expected = "HTTP/1.1";
        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_upstream_protocol() {
        let req = build_request();
        let source = LogFormatter::try_new("%UPSTREAM_PROTOCOL%").unwrap();
        let mut formatter = source.clone();
        println!("FORMATTER: {formatter:?}");
        let expected = "HTTP/1.1";
        formatter.with_context(&UpstreamRequest(&req));
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_scheme() {
        let req = build_request();
        let source = LogFormatter::try_new("%REQ(:SCHEME)%").unwrap();
        let mut formatter = source.clone();
        let expected = "https";
        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_authority() {
        let req = build_request();
        let source = LogFormatter::try_new("%REQ(:AUTHORITY)%").unwrap();
        let mut formatter = source.clone();
        let expected = "www.rust-lang.org";
        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_user_agent() {
        let req = build_request();
        let source = LogFormatter::try_new("%REQ(USER-AGENT)%").unwrap();
        let mut formatter = source.clone();
        let expected = "awesome/1.0";
        formatter.with_context(&DownstreamRequest(&req));
        let actual = format!("{}", &formatter.into_message());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_unevaluated_operator() {
        let source = LogFormatter::try_new("%REQ(USER-AGENT)%").unwrap();
        let formatter = source.clone();
        let actual = format!("{}", &formatter.into_message());
        println!("{actual}");
    }

    #[test]
    fn default_format_string() {
        let req = build_request();
        let resp = build_response();
        let source = LogFormatter::try_new(DEFAULT_ENVOY_FORMAT).unwrap();
        let mut formatter = source.clone();
        formatter.with_context(&InitContext { start_time: std::time::SystemTime::now() });
        formatter.with_context(&DownstreamRequest(&req));
        formatter.with_context(&UpstreamContext { authority: req.uri().authority().unwrap() });
        formatter.with_context(&DownstreamResponse(&resp));
        formatter.with_context(&FinishContext {
            duration: Duration::from_millis(100),
            bytes_received: 128,
            bytes_sent: 256,
            response_flags: ResponseFlags::NO_HEALTHY_UPSTREAM,
        });
        assert!(formatter.is_fully_formatted());
        println!("{}", &formatter.into_message());
    }

    #[test]
    fn test_sizes() {
        println!("Vec:       {}", std::mem::size_of::<Vec<u8>>());
        println!("SmolStr:   {}", std::mem::size_of::<SmolStr>());
        println!("Box<[u8]>: {}", std::mem::size_of::<Box<[u8]>>());
    }
}
