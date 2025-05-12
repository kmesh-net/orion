use http::{Request, Response};
use std::borrow::Cow;

use crate::token::{ReqArg, RespArg, Token, TokenArgument};

pub trait Context {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<Cow<'a, str>>;
}

impl<T> Context for Request<T> {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<Cow<'a, str>> {
        match (token, arg) {
            (Token::Request, Some(TokenArgument::Request(ReqArg::Path))) => Some(Cow::Borrowed(self.uri().path())),
            (Token::Request, Some(TokenArgument::Request(ReqArg::Authority))) => {
                self.uri().authority().and_then(|a| Some(Cow::Borrowed(a.host())))
            },
            (Token::Request, Some(TokenArgument::Request(ReqArg::Method))) => {
                Some(Cow::Borrowed(self.method().as_str()))
            },
            (Token::Request, Some(TokenArgument::Request(ReqArg::Scheme))) => {
                self.uri().scheme().map(|s| Cow::Owned(format!("{}", s)))
            },
            (Token::Request, Some(TokenArgument::Request(ReqArg::NormalHeader(h)))) => {
                self.headers().get(h).and_then(|v| v.to_str().ok().map(Cow::Borrowed))
            },
            _ => None,
        }
    }
}

impl<T> Context for Response<T> {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<Cow<'a, str>> {
        match (token, arg) {
            (Token::Request, Some(TokenArgument::Response(RespArg::NormalHeader(h)))) => {
                self.headers().get(h).and_then(|v| v.to_str().ok().map(Cow::Borrowed))
            },
            _ => None,
        }
    }
}
