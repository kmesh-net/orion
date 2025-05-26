use std::time::{Duration, SystemTime};

use chrono::{DateTime, Datelike, Timelike, Utc};
// use compact_str::CompactString;
use http::{uri::Authority, HeaderName, Request, Response, Version};
use smol_str::{SmolStr, SmolStrBuilder, ToSmolStr};

use crate::{
    operator::{Category, Operator},
    types::{ResponseFlags, ResponseFlagsShort},
    StringType,
};

const X_ENVOY_ORIGINAL_PATH: HeaderName = HeaderName::from_static("x-envoy-original-path");

pub trait Context {
    fn eval_part(&self, op: &Operator) -> StringType;
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
    pub response_flags: ResponseFlags,
}

#[derive(Clone, Debug)]
pub struct UpstreamContext<'a> {
    pub authority: &'a Authority,
}

#[derive(Clone, Debug)]
pub struct DownstreamContext {}

impl Context for UpstreamContext<'_> {
    fn category() -> Category {
        Category::UpstreamContext
    }
    fn eval_part(&self, op: &Operator) -> StringType {
        match op {
            Operator::UpstreamHost => StringType::Smol(SmolStr::new(self.authority.as_str())),
            _ => StringType::None,
        }
    }
}

impl Context for InitContext {
    fn category() -> Category {
        Category::InitContext
    }
    fn eval_part(&self, op: &Operator) -> StringType {
        match op {
            // Operator::StartTime => StringType::Compact(format_system_time_compact(self.start_time)),
            Operator::StartTime => StringType::Smol(format_system_time(self.start_time)),
            _ => StringType::None,
        }
    }
}

impl Context for FinishContext {
    fn category() -> Category {
        Category::FinishContext
    }
    fn eval_part(&self, op: &Operator) -> StringType {
        match op {
            Operator::ResponseFlags => StringType::Smol(ResponseFlagsShort(&self.response_flags).to_smolstr()),
            Operator::Duration => {
                let mut buffer = itoa::Buffer::new();
                StringType::Smol(SmolStr::new(buffer.format(self.duration.as_millis())))
            },
            Operator::BytesReceived => {
                let mut buffer = itoa::Buffer::new();
                StringType::Smol(SmolStr::new(buffer.format(self.bytes_received)))
            },
            Operator::BytesSent => {
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

impl<T> Context for DownstreamRequest<'_, T> {
    fn category() -> Category {
        Category::DownstreamRequest
    }
    fn eval_part(&self, op: &Operator) -> StringType {
        match op {
            Operator::RequestPath => StringType::Smol(SmolStr::new(self.0.uri().path())),
            Operator::RequestOriginalPathOrPath => {
                let path_str = self
                    .0
                    .headers()
                    .get(X_ENVOY_ORIGINAL_PATH)
                    .and_then(|p| p.to_str().ok())
                    .unwrap_or_else(|| self.0.uri().path());

                StringType::Smol(SmolStr::new(path_str))
            },
            Operator::RequestAuthority => {
                if let Some(a) = extract_authority_from_request(self.0) {
                    StringType::Smol(SmolStr::new(a))
                } else {
                    StringType::Smol(SmolStr::new_static("-"))
                }
            },
            Operator::RequestMethod => StringType::Smol(SmolStr::new(self.0.method().as_str())),
            Operator::RequestScheme => {
                if let Some(s) = self.0.uri().scheme() {
                    StringType::Smol(SmolStr::new(s.as_str()))
                } else {
                    StringType::Smol(SmolStr::new_static("-"))
                }
            },

            Operator::RequestStandard(h) => {
                let hv = self.0.headers().get(h);
                match hv {
                    Some(hv) => match String::from_utf8_lossy(hv.as_bytes()) {
                        std::borrow::Cow::Borrowed(s) => StringType::Smol(SmolStr::new(s)),
                        std::borrow::Cow::Owned(s) => StringType::Smol(SmolStr::new(s)),
                    },
                    None => StringType::Smol(SmolStr::new_static("-")),
                }
            },
            Operator::Protocol => StringType::Smol(SmolStr::new_static(into_protocol(self.0.version()))),
            _ => StringType::None,
        }
    }
}

impl<T> Context for UpstreamRequest<'_, T> {
    fn category() -> Category {
        Category::UpstreamRequest
    }
    fn eval_part(&self, op: &Operator) -> StringType {
        match op {
            Operator::UpstreamProtocol => StringType::Smol(SmolStr::new_static(into_protocol(self.0.version()))),
            _ => StringType::None,
        }
    }
}

impl<T> Context for DownstreamResponse<'_, T> {
    fn category() -> Category {
        Category::DownstreamResponse
    }
    fn eval_part(&self, op: &Operator) -> StringType {
        match op {
            Operator::ResponseCode => StringType::Smol(SmolStr::new_inline(self.0.status().as_str())),
            Operator::ResponseStandard(h) => {
                let hv = self.0.headers().get(h.as_str());
                match hv {
                    Some(hv) => match String::from_utf8_lossy(hv.as_bytes()) {
                        std::borrow::Cow::Borrowed(s) => StringType::Smol(SmolStr::new(s)),
                        std::borrow::Cow::Owned(s) => StringType::Smol(SmolStr::new(s)),
                    },

                    None => StringType::Smol(SmolStr::new_static("-")),
                }
            },
            _ => StringType::None,
        }
    }
}

pub fn extract_authority_from_request<T>(request: &Request<T>) -> Option<&str> {
    if let Some(authority) = request.uri().authority() {
        return Some(authority.as_str());
    }
    if let Some(host_header_value) = request.headers().get(http::header::HOST) {
        return host_header_value.to_str().ok();
    }

    None
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

#[inline]
fn into_protocol(ver: Version) -> &'static str {
    match ver {
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2",
        Version::HTTP_3 => "HTTP/3",
        _ => "HTTP/UNKNOWN",
    }
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
