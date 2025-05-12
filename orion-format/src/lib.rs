pub mod context;
pub mod envoy;
pub mod token;

use context::Context;
use std::fmt::{self, Display, Formatter};
use thiserror::Error;
use token::{Token, TokenArgument};

use crate::envoy::EnvoyGrammar;

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
    Literal(String),
    Placeholder(String, Token, Option<TokenArgument>), // eg. ("DURATION", Pattern::Duration, None), (Pattern::Req, Some(":METHOD"))
}

#[derive(Debug, Clone)]
pub struct LogFormatter {
    template: Vec<Template>,
}

impl LogFormatter {
    pub fn try_new(typ: FormatType, input: &str) -> Result<LogFormatter, FormatError> {
        match typ {
            FormatType::Envoy => {
                let template = EnvoyGrammar::parse(input)?;
                Ok(LogFormatter { template })
            },
            FormatType::GridRouter => todo!(),
        }
    }

    pub fn with_context<C: Context>(&mut self, ctx: &C) -> &mut Self {
        for part in &mut self.template {
            if let Template::Placeholder(_, token, arg) = part {
                if let Some(value) = ctx.eval_part(token, arg) {
                    *part = Template::Literal(value.into());
                }
            }
        }

        self
    }
}

impl Display for LogFormatter {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for part in &self.template {
            match part {
                Template::Literal(s) => f.write_str(s.as_ref())?,
                Template::Placeholder(orig, _, _) => {
                    f.write_str(&format!("{}", orig))?;
                },
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub enum FormatType {
    Envoy,
    GridRouter,
}

pub trait Grammar {
    fn parse(input: &str) -> Result<Vec<Template>, FormatError>;
}

#[cfg(test)]
mod tests {
    use http::Request;

    use super::*;

    fn build_request() -> Request<()> {
        Request::builder()
            .uri("https://www.rust-lang.org/")
            .header("User-Agent", "my-awesome-agent/1.0")
            .body(())
            .unwrap()
    }

    #[test]
    fn test_request_path() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new(FormatType::Envoy, "%REQ(:PATH)%").unwrap();
        let expected = "/";
        formatter.with_context(&req);
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_method() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new(FormatType::Envoy, "%REQ(:METHOD)%").unwrap();
        let expected = "GET";
        formatter.with_context(&req);
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_scheme() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new(FormatType::Envoy, "%REQ(:SCHEME)%").unwrap();
        let expected = "https";
        formatter.with_context(&req);
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_authority() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new(FormatType::Envoy, "%REQ(:AUTHORITY)%").unwrap();
        let expected = "www.rust-lang.org";
        formatter.with_context(&req);
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_request_user_agent() {
        let req = build_request();
        let mut formatter = LogFormatter::try_new(FormatType::Envoy, "%REQ(USER-AGENT)%").unwrap();
        let expected = "my-awesome-agent/1.0";
        formatter.with_context(&req);
        let actual = format!("{}", &formatter);
        assert_eq!(actual, expected);
    }

    #[test]
    fn default_format_string() {
        let req = build_request();
        let def_fmt = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(:PATH)% %PROTOCOL%"
            %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION%
            %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%"
            "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#;
        let mut formatter = LogFormatter::try_new(FormatType::Envoy, def_fmt).unwrap();
        formatter.with_context(&req);
        let actual = format!("{}", &formatter);
        println!("{}", actual);
    }
}
