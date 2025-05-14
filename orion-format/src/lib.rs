pub mod context;
pub mod grammar;
pub mod smol_cow;
pub mod token;

use context::Context;
use smol_str::SmolStr;
use std::fmt::{self, Display, Formatter};
use std::io::Write;
use thiserror::Error;
use token::{Category, Token, TokenArgument};

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
    Literal(SmolStr),
    Placeholder(Token, Category, Option<TokenArgument>), // eg. ("DURATION", Pattern::Duration, None), (Pattern::Req, Some(":METHOD"))
}

#[derive(PartialEq, Debug, Clone)]
pub struct LogFormatter {
    template: Vec<Template>,
}

impl LogFormatter {
    pub fn try_new(input: &str) -> Result<LogFormatter, FormatError> {
        let template = EnvoyGrammar::parse(input)?;
        Ok(LogFormatter { template })
    }

    pub fn with_context<'a, C: Context>(&mut self, ctx: &'a C) -> &mut Self {
        let total_keys = C::number_keys();
        let context_category = C::category();
        let mut eval_tokens = 0;
        for part in &mut self.template {
            if eval_tokens == total_keys {
                break;
            }
            if let Template::Placeholder(token, categories, arg) = part {
                eval_tokens += 1;
                if categories.contains(context_category) {
                    if let Some(value) = ctx.eval_part(token, arg) {
                        *part = Template::Literal(value.into_owned()); // TODO: Reduce memory allocations by grouping consecutive expanded templates
                    }
                }
            }
        }

        self
    }

    pub fn write_to<W: Write>(&self, w: &mut W) -> std::io::Result<usize> {
        let mut total_bytes = 0;
        for part in &self.template {
            total_bytes += match part {
                Template::Literal(s) => w.write(s.as_bytes())?,
                Template::Placeholder(_, _, _) => w.write("UNSUP".as_bytes())?,
            };
        }

        Ok(total_bytes)
    }
}

impl Display for LogFormatter {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for part in &self.template {
            match part {
                Template::Literal(s) => f.write_str(s.as_ref())?,
                Template::Placeholder(_, _, _) => {
                    f.write_str("UNSUP")?;
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

    use http::{Request, Response, StatusCode};

    use crate::context::{DownstreamRequest, DownstreamResponse, FinishContext, InitContext};

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
        formatter.with_context(&DownstreamRequest(&req));
        formatter.with_context(&DownstreamResponse(&resp));
        formatter.with_context(&FinishContext {
            duration: Duration::from_millis(100),
            bytes_received: 128,
            bytes_sent: 256,
        });
        println!("{}", &formatter);
    }
}
