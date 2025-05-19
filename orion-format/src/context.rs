use std::time::{Duration, SystemTime};

use chrono::{DateTime, Datelike, Timelike, Utc};
// use compact_str::CompactString;
use http::{uri::Authority, HeaderName, Request, Response, Version};
use smol_str::{SmolStr, SmolStrBuilder};

use crate::{
    token::{Category, Token},
    StringType,
};

pub trait Context {
    fn eval_part(&self, token: &Token) -> StringType;
    fn category() -> Category;
}

#[derive(Clone, Debug)]
pub struct InitContext {
    pub start_time: SystemTime,
}

#[derive(Clone, Debug)]
pub struct FinishContext<'a> {
    pub duration: Duration,
    pub bytes_received: usize,
    pub bytes_sent: usize,
    pub response_flags: &'a str,
}

#[derive(Clone, Debug)]
pub struct UpstreamContext<'a> {
    pub authority: &'a Authority,
}

#[derive(Clone, Debug)]
pub struct DownstreamContext {}

impl<'c> Context for UpstreamContext<'c> {
    fn category() -> Category {
        Category::UPSTREAM_CONTEXT
    }
    fn eval_part<'a>(&'a self, token: &Token) -> StringType {
        match token {
            Token::UpstreamHost => StringType::Smol(SmolStr::new(self.authority.host())),
            _ => StringType::None,
        }
    }
}

impl Context for InitContext {
    fn category() -> Category {
        Category::INIT_CONTEXT
    }
    fn eval_part(&self, token: &Token) -> StringType {
        match token {
            // Token::StartTime => StringType::Compact(format_system_time_compact(self.start_time)),
            Token::StartTime => StringType::Smol(format_system_time(self.start_time)),
            _ => StringType::None,
        }
    }
}

impl<'a> Context for FinishContext<'a> {
    fn category() -> Category {
        Category::FINISH_CONTEXT
    }
    fn eval_part(&self, token: &Token) -> StringType {
        match token {
            Token::ResponseFlags => StringType::Smol(SmolStr::new(self.response_flags)),
            Token::Duration => {
                let mut buffer = itoa::Buffer::new();
                StringType::Smol(SmolStr::new(buffer.format(self.duration.as_millis())))
            },
            Token::BytesReceived => {
                let mut buffer = itoa::Buffer::new();
                StringType::Smol(SmolStr::new(buffer.format(self.bytes_received)))
            },
            Token::BytesSent => {
                let mut buffer = itoa::Buffer::new();
                StringType::Smol(SmolStr::new(buffer.format(self.bytes_sent)))
            },
            _ => StringType::None,
        }
    }
}

pub struct DownstreamRequest<'a, T>(pub &'a Request<T>);
pub struct UpstreamRequest<'a, T>(pub &'a Request<T>);
pub struct DownstreamResponse<'a, T>(pub &'a Response<T>);
pub struct UpstreamResponse<'a, T>(pub &'a Response<T>);

impl<'a, T> Context for UpstreamRequest<'a, T> {
    fn category() -> Category {
        Category::UPSTREAM_REQUEST
    }
    fn eval_part(&self, token: &Token) -> StringType {
        match token {
            Token::RequestPath => StringType::Smol(SmolStr::new(self.0.uri().path())),
            Token::RequestOriginalPathOrPath => {
                let path_str = self
                    .0
                    .headers()
                    .get(HeaderName::from_static("x-envoy-original-path"))
                    .and_then(|p| p.to_str().ok())
                    .unwrap_or_else(|| self.0.uri().path());

                StringType::Smol(SmolStr::new(path_str))
            },
            Token::RequestAuthority => {
                if let Some(a) = self.0.uri().authority() {
                    StringType::Smol(SmolStr::new(a.host()))
                } else {
                    StringType::None
                }
            },
            Token::RequestMethod => StringType::Smol(SmolStr::new(self.0.method().as_str())),
            Token::RequestScheme => {
                if let Some(s) = self.0.uri().scheme() {
                    StringType::Smol(SmolStr::new(s.as_str()))
                } else {
                    StringType::None
                }
            },

            Token::RequestStandard(h) => {
                let hv = self.0.headers().get(h);
                match hv {
                    Some(hv) => {
                        if let Some(s) = hv.to_str().ok() {
                            StringType::Smol(SmolStr::new(s))
                        } else {
                            StringType::None
                        }
                    },
                    None => StringType::Smol(SmolStr::new_static("")),
                }
            },
            Token::Protocol => {
                let ver = match self.0.version() {
                    Version::HTTP_10 => "HTTP/1.0",
                    Version::HTTP_11 => "HTTP/1.1",
                    Version::HTTP_2 => "HTTP/2",
                    Version::HTTP_3 => "HTTP/3",
                    _ => "HTTP/UNKNOWN",
                };

                StringType::Smol(SmolStr::new_static(ver))
            },
            _ => StringType::None,
        }
    }
}

impl<'a, T> Context for DownstreamRequest<'a, T> {
    fn category() -> Category {
        Category::DOWNSTREAM_REQUEST
    }
    fn eval_part(&self, token: &Token) -> StringType {
        match token {
            Token::RequestPath => StringType::Smol(SmolStr::new(self.0.uri().path())),
            Token::RequestAuthority => {
                if let Some(a) = self.0.uri().authority() {
                    StringType::Smol(SmolStr::new(a.host()))
                } else {
                    StringType::None
                }
            },
            Token::RequestMethod => StringType::Smol(SmolStr::new(self.0.method().as_str())),
            Token::RequestScheme => {
                if let Some(s) = self.0.uri().scheme() {
                    StringType::Smol(SmolStr::new(s.as_str()))
                } else {
                    StringType::None
                }
            },

            Token::RequestStandard(h) => {
                let hv = self.0.headers().get(h);
                match hv {
                    Some(hv) => {
                        if let Some(s) = hv.to_str().ok() {
                            StringType::Smol(SmolStr::new(s))
                        } else {
                            StringType::None
                        }
                    },
                    None => StringType::Smol(SmolStr::new_static("")),
                }
            },
            Token::Protocol => {
                let ver = match self.0.version() {
                    Version::HTTP_10 => "HTTP/1.0",
                    Version::HTTP_11 => "HTTP/1.1",
                    Version::HTTP_2 => "HTTP/2",
                    Version::HTTP_3 => "HTTP/3",
                    _ => "HTTP/UNKNOWN",
                };

                StringType::Smol(SmolStr::new_static(ver))
            },
            _ => StringType::None,
        }
    }
}

impl<'a, T> Context for DownstreamResponse<'a, T> {
    fn category() -> Category {
        Category::DOWNSTREAM_RESPONSE
    }
    fn eval_part(&self, token: &Token) -> StringType {
        match token {
            Token::ResponseCode => StringType::Smol(SmolStr::new_inline(self.0.status().as_str())),
            Token::ResponseStandard(h) => {
                let hv = self.0.headers().get(h.as_str());
                match hv {
                    Some(hv) => {
                        if let Some(s) = hv.to_str().ok() {
                            StringType::Smol(SmolStr::new(s))
                        } else {
                            StringType::None
                        }
                    },

                    None => StringType::Smol(SmolStr::new_static("")),
                }
            },
            _ => StringType::None,
        }
    }
}

const TWO_DIGITS: [&str; 100] = [
    "00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13", "14", "15", "16", "17", "18",
    "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31", "32", "33", "34", "35", "36", "37",
    "38", "39", "40", "41", "42", "43", "44", "45", "46", "47", "48", "49", "50", "51", "52", "53", "54", "55", "56",
    "57", "58", "59", "60", "61", "62", "63", "64", "65", "66", "67", "68", "69", "70", "71", "72", "73", "74", "75",
    "76", "77", "78", "79", "80", "81", "82", "83", "84", "85", "86", "87", "88", "89", "90", "91", "92", "93", "94",
    "95", "96", "97", "98", "99",
];

pub fn format_system_time(time: SystemTime) -> SmolStr {
    let datetime: DateTime<Utc> = time.into();

    let mut builder = SmolStrBuilder::new();
    let mut buffer = itoa::Buffer::new();

    builder.push_str(buffer.format(datetime.year()));
    builder.push('-');
    builder.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.month() as usize) });
    builder.push('-');
    builder.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.day() as usize) });
    builder.push('T');
    builder.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.hour() as usize) });
    builder.push(':');
    builder.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.minute() as usize) });
    builder.push(':');
    builder.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.second() as usize) });
    builder.push(':');
    builder.push_str(buffer.format(datetime.nanosecond() / 1_000_000));
    // builder.push('Z');

    builder.finish()
}

#[cfg(any())]
pub fn format_system_time_heapless(time: SystemTime) -> heapless::String<24> {
    let datetime: DateTime<Utc> = time.into();
    let mut rfc3999: heapless::String<24> = heapless::String::new();
    let mut buffer = itoa::Buffer::new();
    _ = rfc3999.push_str(buffer.format(datetime.year()));
    _ = rfc3999.push('-');
    _ = rfc3999.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.month() as usize) });
    _ = rfc3999.push('-');
    _ = rfc3999.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.day() as usize) });
    _ = rfc3999.push('T');
    _ = rfc3999.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.hour() as usize) });
    _ = rfc3999.push(':');
    _ = rfc3999.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.minute() as usize) });
    _ = rfc3999.push(':');
    _ = rfc3999.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.second() as usize) });
    _ = rfc3999.push(':');
    _ = rfc3999.push_str(buffer.format(datetime.nanosecond() / 1_000_000));
    _ = rfc3999.push('Z');
    rfc3999
}

#[cfg(any())]
pub fn format_system_time_compact(time: SystemTime) -> CompactString {
    let datetime: DateTime<Utc> = time.into();
    let mut buffer = itoa::Buffer::new();
    let mut rfc3999 = CompactString::default();

    _ = rfc3999.push_str(buffer.format(datetime.year()));
    _ = rfc3999.push('-');
    _ = rfc3999.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.month() as usize) });
    _ = rfc3999.push('-');
    _ = rfc3999.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.day() as usize) });
    _ = rfc3999.push('T');
    _ = rfc3999.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.hour() as usize) });
    _ = rfc3999.push(':');
    _ = rfc3999.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.minute() as usize) });
    _ = rfc3999.push(':');
    _ = rfc3999.push_str(unsafe { TWO_DIGITS.get_unchecked(datetime.second() as usize) });
    _ = rfc3999.push(':');
    _ = rfc3999.push_str(buffer.format(datetime.nanosecond() / 1_000_000));
    _ = rfc3999.push('Z');
    rfc3999
}
