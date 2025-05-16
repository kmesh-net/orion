pub mod context;
pub mod grammar;
pub mod token;

use context::Context;
use smol_str::SmolStr;
use std::fmt::{self, Display, Formatter, Write};
use std::io::Write as IoWrite;
use std::sync::Arc;
use thiserror::Error;
use token::{Category, Token};

use crate::grammar::EnvoyGrammar;

#[derive(Error, Debug)]
pub enum FormatError {
    #[error("pattern `{0}` not supported")]
    UnsupportedPattern(String),
    #[error("missing argument for `{0}`")]
    MissingArgument(String),
    #[error("missing braket around `{0}`...")]
    MissingBracket(String),
    #[error("missing delimiter around `{0}`...")]
    MissingDelimiter(String),
    #[error("empty argument around `{0}`...")]
    EmptyArgument(String),
    #[error("invalid request argument (`{0}`)")]
    InvalidRequestArg(String),
    #[error("invalid response argument (`{0}`)")]
    InvalidResponseArg(String),
}

#[derive(PartialEq, Debug, Clone)]
pub enum Template {
    Char(char),
    Literal(SmolStr),
    Placeholder(Token, Category), // eg. ("DURATION", Pattern::Duration, None), (Pattern::Req, Some(":METHOD"))
}

#[derive(PartialEq, Debug, Clone)]
pub enum Smol {
    Str(SmolStr),
    Char(char),
    None,
}

#[derive(PartialEq, Debug, Clone)]
pub struct LogFormatter {
    pub template: Arc<Vec<Template>>,
    pub format: Vec<Smol>,
}

impl LogFormatter {
    pub fn try_new(input: &str) -> Result<LogFormatter, FormatError> {
        let template = EnvoyGrammar::parse(input)?;
        let mut format: Vec<Smol> = Vec::with_capacity(template.len());

        for t in template.iter() {
            match t {
                Template::Char(c) => format.push(Smol::Char(*c)),
                Template::Literal(smol_str) => format.push(Smol::Str(smol_str.clone())),
                _ => format.push(Smol::None),
            }
        }

        Ok(LogFormatter { template: Arc::new(template), format })
    }

    pub fn with_context<'a, C: Context>(&mut self, ctx: &'a C) -> &mut Self {
        let context_category = C::category();
        for (part, out) in self.template.iter().zip(&mut self.format.iter_mut()) {
            if let Template::Placeholder(token, categories) = part {
                if categories.contains(context_category) {
                    *out = ctx.eval_part(token);
                }
            }
        }

        self
    }

    pub fn write_to<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<usize> {
        let mut total_bytes = 0;
        for (part, out) in self.template.iter().zip(self.format.iter()) {
            total_bytes += match out {
                Smol::Str(s) => IoWrite::write(w, s.as_bytes())?,
                Smol::Char(c) => {
                    let mut buf = [0u8; 4];
                    let bytes = c.encode_utf8(&mut buf).as_bytes();
                    IoWrite::write(w, bytes)?
                },
                _ => IoWrite::write(w, format!("%{:?}%", part).as_bytes())?,
            };
        }

        Ok(total_bytes)
    }
}

impl Display for LogFormatter {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (part, out) in self.template.iter().zip(self.format.iter()) {
            match out {
                Smol::Str(s) => f.write_str(s.as_ref())?,
                Smol::Char(c) => f.write_char(*c)?,
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

    use http::{Request, Response, StatusCode};

    use crate::context::{DownstreamRequest, DownstreamResponse, FinishContext, InitContext, UpstreamContext};

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
        let req = build_request();
        let mut formatter = LogFormatter::try_new("%REQ(:PATH)%").unwrap();
        let expected = "/";
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
    fn default_format_string() {
        let req = build_request();
        let resp = build_response();
        let def_fmt = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#;
        let mut formatter = LogFormatter::try_new(def_fmt).unwrap();
        formatter.with_context(&InitContext { start_time: std::time::SystemTime::now() });
        formatter.with_context(&UpstreamContext { authority: &req.uri().authority().unwrap() });
        formatter.with_context(&DownstreamRequest(&req));
        formatter.with_context(&DownstreamResponse(&resp));
        formatter.with_context(&FinishContext {
            duration: Duration::from_millis(100),
            bytes_received: 128,
            bytes_sent: 256,
            response_flags: "UH",
        });
        println!("{}", &formatter);
    }
}
