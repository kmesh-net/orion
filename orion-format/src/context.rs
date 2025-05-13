use http::{Request, Response, Version};
use smol_str::{format_smolstr, SmolStr};

use crate::smol_cow::SmolCow;
use crate::token::{ReqArg, RespArg, Token, TokenArgument};

pub trait Context {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<SmolCow<'a>>;
}

impl<T> Context for Request<T> {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<SmolCow<'a>> {
        match (token, arg) {
            (Token::Request, Some(TokenArgument::Request(ReqArg::Path))) => Some(SmolCow::Borrowed(self.uri().path())),
            (Token::Request, Some(TokenArgument::Request(ReqArg::Authority))) => {
                self.uri().authority().and_then(|a| Some(SmolCow::Borrowed(a.host())))
            },
            (Token::Request, Some(TokenArgument::Request(ReqArg::Method))) => {
                Some(SmolCow::Borrowed(self.method().as_str()))
            },
            (Token::Request, Some(TokenArgument::Request(ReqArg::Scheme))) => {
                self.uri().scheme().map(|s| SmolCow::Owned(format_smolstr!("{}", s)))
            },
            (Token::Request, Some(TokenArgument::Request(ReqArg::NormalHeader(h)))) => {
                let hv = self.headers().get(h);
                match hv {
                    Some(hv) => hv.to_str().ok().map(SmolCow::Borrowed),
                    None => Some(SmolCow::Borrowed("")),
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

                Some(SmolCow::Borrowed(ver))
            },
            _ => None,
        }
    }
}

impl<T> Context for Response<T> {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<SmolCow<'a>> {
        match (token, arg) {
            (Token::ResponseCode, _) => Some(SmolCow::Owned(SmolStr::new_inline(self.status().as_str()))),
            (Token::Response, Some(TokenArgument::Response(RespArg::NormalHeader(h)))) => {
                let hv = self.headers().get(h.as_str());
                match hv {
                    Some(hv) => hv.to_str().ok().map(SmolCow::Borrowed),
                    None => Some(SmolCow::Borrowed("")),
                }
            },
            _ => None,
        }
    }
}
