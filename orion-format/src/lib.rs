pub mod context;
pub mod envoy;
pub mod token;

use std::borrow::Cow;
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

#[derive(PartialEq, Debug)]
pub enum Template {
    Literal(String),
    Placeholder(Token, Option<TokenArgument>), // eg. (Pattern::Duration, None), (Pattern::Req, Some(":METHOD"))
}

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

    pub fn with_context<C: Context>(mut self, ctx: &C) -> Self {
        for part in &mut self.template {
            if let Template::Placeholder(token, arg) = part {
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
                Template::Placeholder(token, arg) => {
                    f.write_str(&format!("%{}", token))?;
                    if let Some(a) = arg {
                        f.write_str("(")?;
                        f.write_str(&format!("{}", a))?;
                        f.write_str(")")?;
                    }
                    f.write_str(&format!("%"))?;
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

pub trait Context {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<Cow<'a, str>>;
}
