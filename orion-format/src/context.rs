use std::time::{Duration, SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use http::{Request, Response, Version};
use smol_str::{format_smolstr, SmolStr};

use crate::smol_cow::SmolCow;
use crate::token::{ReqArg, RespArg, Token, TokenArgument};

pub trait Context {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<SmolCow<'a>>;
}

pub struct StartContext {
    pub start_time: SystemTime,
}

pub struct EndContext {
    pub duration: Duration,
    pub bytes_received: usize,
    pub bytes_sent: usize,
}

impl Context for StartContext {
    fn eval_part<'a>(&'a self, token: &Token, _arg: &Option<TokenArgument>) -> Option<SmolCow<'a>> {
        match token {
            Token::StartTime => {
                let datetime_utc: DateTime<Utc> = self.start_time.into();
                let fmt_rfc3339 = datetime_utc.to_rfc3339_opts(SecondsFormat::Millis, true);
                Some(SmolCow::Owned(format_smolstr!("{fmt_rfc3339}")))
            },
            _ => None,
        }
    }
}

impl Context for EndContext {
    fn eval_part<'a>(&'a self, token: &Token, _arg: &Option<TokenArgument>) -> Option<SmolCow<'a>> {
        match token {
            Token::Duration => Some(SmolCow::Owned(format_smolstr!("{}", self.duration.as_millis()))),
            Token::BytesReceived => Some(SmolCow::Owned(format_smolstr!("{}", self.bytes_received))),
            Token::BytesSent => Some(SmolCow::Owned(format_smolstr!("{}", self.bytes_sent))),
            _ => None,
        }
    }
}

pub struct DownStreamRequest<'a, T>(pub &'a Request<T>);
pub struct UpStreamRequest<'a, T>(pub &'a Request<T>);
pub struct DownStreamResponse<'a, T>(pub &'a Response<T>);
pub struct UpStreamResponse<'a, T>(pub &'a Response<T>);

impl<'r, T> Context for DownStreamRequest<'r, T> {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<SmolCow<'a>> {
        match (token, arg) {
            (Token::Request, Some(TokenArgument::Request(ReqArg::Path))) => {
                Some(SmolCow::Borrowed(self.0.uri().path()))
            },
            (Token::Request, Some(TokenArgument::Request(ReqArg::Authority))) => {
                self.0.uri().authority().and_then(|a| Some(SmolCow::Borrowed(a.host())))
            },
            (Token::Request, Some(TokenArgument::Request(ReqArg::Method))) => {
                Some(SmolCow::Borrowed(self.0.method().as_str()))
            },
            (Token::Request, Some(TokenArgument::Request(ReqArg::Scheme))) => {
                self.0.uri().scheme().map(|s| SmolCow::Owned(format_smolstr!("{}", s)))
            },
            (Token::Request, Some(TokenArgument::Request(ReqArg::NormalHeader(h)))) => {
                let hv = self.0.headers().get(h);
                match hv {
                    Some(hv) => hv.to_str().ok().map(SmolCow::Borrowed),
                    None => Some(SmolCow::Borrowed("")),
                }
            },
            (Token::Protocol, _) => {
                let ver = match self.0.version() {
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

impl<'r, T> Context for DownStreamResponse<'r, T> {
    fn eval_part<'a>(&'a self, token: &Token, arg: &Option<TokenArgument>) -> Option<SmolCow<'a>> {
        match (token, arg) {
            (Token::ResponseCode, _) => Some(SmolCow::Owned(SmolStr::new_inline(self.0.status().as_str()))),
            (Token::Response, Some(TokenArgument::Response(RespArg::NormalHeader(h)))) => {
                let hv = self.0.headers().get(h.as_str());
                match hv {
                    Some(hv) => hv.to_str().ok().map(SmolCow::Borrowed),
                    None => Some(SmolCow::Borrowed("")),
                }
            },
            _ => None,
        }
    }
}
