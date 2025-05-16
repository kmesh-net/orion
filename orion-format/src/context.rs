use std::time::{Duration, SystemTime};

use chrono::{DateTime, Datelike, Timelike, Utc};
use http::{Request, Response, Version};
use smol_str::{format_smolstr, SmolStr, SmolStrBuilder};

use crate::{
    token::{Category, Token},
    Smol,
};

pub trait Context {
    fn eval_part<'a>(&'a self, token: &Token) -> Smol;
    fn category() -> Category;
}

#[derive(Clone, Debug)]
pub struct InitContext {
    pub start_time: SystemTime,
}

#[derive(Clone, Debug)]
pub struct FinishContext {
    pub duration: Duration,
    pub bytes_received: usize,
    pub bytes_sent: usize,
}

impl Context for InitContext {
    fn category() -> Category {
        Category::INIT_CONTEXT
    }
    fn eval_part<'a>(&'a self, token: &Token) -> Smol {
        match token {
            Token::StartTime => {
                if let Some(s) = format_system_time(self.start_time).ok() {
                    Smol::Str(s)
                } else {
                    Smol::None
                }
            },
            _ => Smol::None,
        }
    }
}

impl Context for FinishContext {
    fn category() -> Category {
        Category::FINISH_CONTEXT
    }
    fn eval_part<'a>(&'a self, token: &Token) -> Smol {
        match token {
            Token::Duration => Smol::Str(format_smolstr!("{}", self.duration.as_millis())),
            Token::BytesReceived => Smol::Str(format_smolstr!("{}", self.bytes_received)),
            Token::BytesSent => Smol::Str(format_smolstr!("{}", self.bytes_sent)),
            _ => Smol::None,
        }
    }
}

pub struct DownstreamRequest<'a, T>(pub &'a Request<T>);
pub struct UpstreamRequest<'a, T>(pub &'a Request<T>);
pub struct DownstreamResponse<'a, T>(pub &'a Response<T>);
pub struct UpstreamResponse<'a, T>(pub &'a Response<T>);

impl<'r, T> Context for DownstreamRequest<'r, T> {
    fn category() -> Category {
        Category::DOWNSTREAM_REQUEST
    }
    fn eval_part<'a>(&'a self, token: &Token) -> Smol {
        match token {
            Token::RequestPath => Smol::Str(SmolStr::new(self.0.uri().path())),
            Token::RequestAuthority => {
                if let Some(a) = self.0.uri().authority() {
                    Smol::Str(SmolStr::new(a.host()))
                } else {
                    Smol::None
                }
            },
            Token::RequestMethod => Smol::Str(SmolStr::new(self.0.method().as_str())),
            Token::RequestScheme => {
                if let Some(s) = self.0.uri().scheme() {
                    Smol::Str(format_smolstr!("{}", s))
                } else {
                    Smol::None
                }
            },

            Token::RequestStandard(h) => {
                let hv = self.0.headers().get(h);
                match hv {
                    Some(hv) => {
                        if let Some(s) = hv.to_str().ok() {
                            Smol::Str(SmolStr::new(s))
                        } else {
                            Smol::None
                        }
                    },
                    None => Smol::Str(SmolStr::new_static("")),
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

                Smol::Str(SmolStr::new_static(ver))
            },
            _ => Smol::None,
        }
    }
}

impl<'r, T> Context for DownstreamResponse<'r, T> {
    fn category() -> Category {
        Category::DOWNSTREAM_RESPONSE
    }
    fn eval_part<'a>(&'a self, token: &Token) -> Smol {
        match token {
            Token::ResponseCode => Smol::Str(SmolStr::new_inline(self.0.status().as_str())),
            Token::ResponseStandard(h) => {
                let hv = self.0.headers().get(h.as_str());
                match hv {
                    Some(hv) => {
                        if let Some(s) = hv.to_str().ok() {
                            Smol::Str(SmolStr::new(s))
                        } else {
                            Smol::None
                        }
                    },

                    None => Smol::Str(SmolStr::new_static("")),
                }
            },
            _ => Smol::None,
        }
    }
}

pub fn format_system_time(time: SystemTime) -> std::result::Result<SmolStr, std::fmt::Error> {
    let datetime: DateTime<Utc> = time.into();
    let mut builder = SmolStrBuilder::new();

    const TWO_DIGITS: [&str; 100] = [
        "00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13", "14", "15", "16", "17",
        "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31", "32", "33", "34", "35",
        "36", "37", "38", "39", "40", "41", "42", "43", "44", "45", "46", "47", "48", "49", "50", "51", "52", "53",
        "54", "55", "56", "57", "58", "59", "60", "61", "62", "63", "64", "65", "66", "67", "68", "69", "70", "71",
        "72", "73", "74", "75", "76", "77", "78", "79", "80", "81", "82", "83", "84", "85", "86", "87", "88", "89",
        "90", "91", "92", "93", "94", "95", "96", "97", "98", "99",
    ];

    four_digits(&mut builder, datetime.year());
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
    three_digits(&mut builder, datetime.nanosecond() / 1_000_000);

    Ok(builder.finish())
}

#[inline(always)]
fn three_digits(builder: &mut SmolStrBuilder, value: u32) {
    builder.push(unsafe { std::char::from_u32_unchecked((b'0' + ((value / 100 % 10) as u8)) as u32) });
    builder.push(unsafe { std::char::from_u32_unchecked((b'0' + ((value / 10 % 10) as u8)) as u32) });
    builder.push(unsafe { std::char::from_u32_unchecked((b'0' + ((value % 10) as u8)) as u32) });
}

#[inline(always)]
fn four_digits(builder: &mut SmolStrBuilder, value: i32) {
    builder.push(unsafe { std::char::from_u32_unchecked((b'0' + ((value / 1000 % 10) as u8)) as u32) });
    builder.push(unsafe { std::char::from_u32_unchecked((b'0' + ((value / 100 % 10) as u8)) as u32) });
    builder.push(unsafe { std::char::from_u32_unchecked((b'0' + ((value / 10 % 10) as u8)) as u32) });
    builder.push(unsafe { std::char::from_u32_unchecked((b'0' + ((value % 10) as u8)) as u32) });
}
