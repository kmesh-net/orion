use http::{Request, Response, Version};
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
                let hv = self.headers().get(h);
                match hv {
                    Some(hv) => hv.to_str().ok().map(Cow::Borrowed),
                    None => Some(Cow::Borrowed("")),
                }
            },
            (Token::Protocol, _) => {
                let ver = match self.version() {
                    Version::HTTP_10 => "HTTP/1.0",
                    Version::HTTP_11 => "HTTP/1.1",
                    Version::HTTP_2 => "HTTP/2",
                    Version::HTTP_3 => "HTTP/3",
                    _ => "HTTP/UNKNOWN",
                };

                Some(Cow::Borrowed(ver))
            },
            _ => None,
        }
    }
}

impl<T> Context for Response<T> {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<Cow<'a, str>> {
        match (token, arg) {
            (Token::ResponseCode, _) => Some(Cow::Owned(self.status().as_str().to_owned())),
            (Token::Response, Some(TokenArgument::Response(RespArg::NormalHeader(h)))) => {
                let hv = self.headers().get(h);
                match hv {
                    Some(hv) => hv.to_str().ok().map(Cow::Borrowed),
                    None => Some(Cow::Borrowed("")),
                }
            },
            _ => None,
        }
    }
}
