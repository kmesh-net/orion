use chrono::{DateTime, SecondsFormat, Utc};
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, Version};
use orion_format::{
    DEFAULT_ACCESS_LOG_FORMAT, LogFormatter, LogFormatterLocal,
    context::{Context, DownstreamContext, DownstreamResponse, FinishContext, InitContext},
    types::{ResponseFlags, ResponseFlagsShort},
};
use smol_str::ToSmolStr;
use std::time::Duration;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[inline]
fn eval_format<C1, C2, C3, C4>(req: &C1, resp: &C2, start: &C3, end: &C4, fmt: &mut LogFormatterLocal)
where
    C1: Context,
    C2: Context,
    C3: Context,
    C4: Context,
{
    fmt.with_context(req);
    fmt.with_context(resp);
    fmt.with_context(start);
    fmt.with_context(end);
}

fn header_lookup<'a>(name: &'a str, m: &'a HeaderMap, default_header: &'a HeaderValue) -> &'a HeaderValue {
    m.get(name).unwrap_or(default_header)
}

// A workaround for the `unwrap` lint in Clippy, which is too strict for our benchmarks.
// This trait provides a method to unwrap `Option` and `Result` types without triggering Clippy warnings.
trait UnwrapClippyWorkaround {
    type Target;
    fn stealth_unwrap(self) -> Self::Target;
}

impl<T> UnwrapClippyWorkaround for Option<T> {
    type Target = T;

    fn stealth_unwrap(self) -> Self::Target {
        match self {
            Some(value) => value,
            None => unimplemented!("Called stealth_unwrap on None"),
        }
    }
}

impl<T, E> UnwrapClippyWorkaround for Result<T, E>
where
    E: std::fmt::Display,
{
    type Target = T;

    fn stealth_unwrap(self) -> Self::Target {
        match self {
            Ok(value) => value,
            Err(e) => unimplemented!("Called stealth_unwrap on Err({e})"),
        }
    }
}

#[allow(clippy::unit_arg)]
fn benchmark_rust_format(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .stealth_unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).stealth_unwrap();
    let start = InitContext { start_time: std::time::SystemTime::now() };
    let end = FinishContext {
        duration: Duration::from_millis(100),
        bytes_received: 128,
        bytes_sent: 256,
        response_flags: ResponseFlags::empty(),
    };

    let default_header_value = HeaderValue::from_static("");
    let upstream_host: String = "www.upstream.com".into();

    c.bench_function("rust_format!", |b| {
        b.iter(|| {
            let start_time = start.start_time;
            let datetime_utc: DateTime<Utc> = start_time.into();
            let rfc3339 = datetime_utc.to_rfc3339_opts(SecondsFormat::Millis, true);

            let method = request.method().clone();
            let uri = request.uri().clone();
            let protocol = request.version();
            let ver = match protocol {
                Version::HTTP_10 => "HTTP/1.0",
                Version::HTTP_11 => "HTTP/1.1",
                Version::HTTP_2 => "HTTP/2",
                Version::HTTP_3 => "HTTP/3",
                _ => "HTTP/UNKNOWN",
            };

            let response_code = response.status();
            let end_context = end.clone();

            let x_envoy_upstream_service_time =
                header_lookup("X-ENVOY-UPSTREAM-SERVICE-TIME", request.headers(), &default_header_value);
            let x_forwarded_for = header_lookup("X-FORWARDED-FOR", request.headers(), &default_header_value);
            let user_agent = header_lookup("USER-AGENT", request.headers(), &default_header_value);
            let x_request_id = header_lookup("X-REQUEST-ID", request.headers(), &default_header_value);
            let upstream_host = upstream_host.clone();

            let path: String = request
                .headers()
                .get(HeaderName::from_static("x-envoy-original-path"))
                .and_then(|p| p.to_str().ok())
                .unwrap_or_else(|| request.uri().path())
                .into();

            black_box(format!(
                r#"[{}] "{} {} {} {} {} {} {} {} {} {} {} {} {} "{}""#,
                rfc3339,
                method.as_str(),
                path,
                ver,
                response_code.as_u16(),
                ResponseFlagsShort(&ResponseFlags::empty()).to_smolstr(),
                end_context.bytes_received,
                end_context.bytes_sent,
                end_context.duration.as_millis(),
                x_envoy_upstream_service_time.to_str().stealth_unwrap(),
                x_forwarded_for.to_str().stealth_unwrap(),
                user_agent.to_str().stealth_unwrap(),
                x_request_id.to_str().stealth_unwrap(),
                uri.authority().stealth_unwrap().host(),
                upstream_host,
            ));
        });
    });
}

#[allow(clippy::unit_arg)]
fn benchmark_log_formatter(c: &mut Criterion) {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .stealth_unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).stealth_unwrap();
    let start = InitContext { start_time: std::time::SystemTime::now() };
    let end = FinishContext {
        duration: Duration::from_millis(100),
        bytes_received: 128,
        bytes_sent: 256,
        response_flags: ResponseFlags::empty(),
    };

    let fmt = LogFormatter::try_new(DEFAULT_ACCESS_LOG_FORMAT, false).stealth_unwrap();
    let mut sink = std::io::sink();

    c.bench_function("log_formatter_full", |b| {
        b.iter(|| {
            let mut fmt = fmt.local_clone();
            black_box(eval_format(
                &DownstreamContext { request: &request, trace_id: None, request_head_size: 0 },
                &DownstreamResponse { response: &response, response_head_size: 0 },
                &start,
                &end,
                &mut fmt,
            ));
        })
    });

    c.bench_function("log_formatter_full_write", |b| {
        b.iter(|| {
            let mut fmt = fmt.local_clone();
            black_box(eval_format(
                &DownstreamContext { request: &request, trace_id: None, request_head_size: 0 },
                &DownstreamResponse { response: &response, response_head_size: 0 },
                &start,
                &end,
                &mut fmt,
            ));
            _ = black_box(|| fmt.into_message().write_to(&mut sink));
        })
    });

    c.bench_function("log_formatter_clone_only", |b| {
        b.iter(|| {
            black_box(fmt.local_clone());
        })
    });

    let mut formatted = fmt.local_clone();
    eval_format(
        &DownstreamContext { request: &request, trace_id: None, request_head_size: 0 },
        &DownstreamResponse { response: &response, response_head_size: 0 },
        &start,
        &end,
        &mut formatted,
    );

    let message = formatted.into_message();
    c.bench_function("log_formatter_write_only", |b| {
        b.iter(|| {
            _ = black_box(|| message.write_to(&mut sink));
        })
    });
}

#[allow(clippy::unit_arg)]
fn benchmark_request_parts(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .stealth_unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).stealth_unwrap();
    let start = InitContext { start_time: std::time::SystemTime::now() };
    let end = FinishContext {
        duration: Duration::from_millis(100),
        bytes_received: 128,
        bytes_sent: 256,
        response_flags: ResponseFlags::empty(),
    };

    let fmt = LogFormatter::try_new("%START_TIME%", false).stealth_unwrap();
    c.bench_function("%START_TIME%", |b| {
        b.iter(|| {
            let mut fmt = fmt.local_clone();
            black_box(eval_format(
                &DownstreamContext { request: &request, trace_id: None, request_head_size: 0 },
                &DownstreamResponse { response: &response, response_head_size: 0 },
                &start,
                &end,
                &mut fmt,
            ))
        })
    });

    let fmt = LogFormatter::try_new("%REQ(:PATH)%", false).stealth_unwrap();
    c.bench_function("REQ(:PATH)", |b| {
        b.iter(|| {
            let mut fmt = fmt.local_clone();
            black_box(eval_format(
                &DownstreamContext { request: &request, trace_id: None, request_head_size: 0 },
                &DownstreamResponse { response: &response, response_head_size: 0 },
                &start,
                &end,
                &mut fmt,
            ))
        })
    });

    let fmt = LogFormatter::try_new("%REQ(:METHOD)%", false).stealth_unwrap();
    c.bench_function("REQ(:METHOD)", |b| {
        b.iter(|| {
            let mut fmt = fmt.local_clone();
            black_box(eval_format(
                &DownstreamContext { request: &request, trace_id: None, request_head_size: 0 },
                &DownstreamResponse { response: &response, response_head_size: 0 },
                &start,
                &end,
                &mut fmt,
            ))
        })
    });

    let fmt = LogFormatter::try_new("%REQ(USER-AGENT)%", false).stealth_unwrap();
    c.bench_function("REQ(USER-AGENT)", |b| {
        b.iter(|| {
            let mut fmt = fmt.local_clone();
            black_box(eval_format(
                &DownstreamContext { request: &request, trace_id: None, request_head_size: 0 },
                &DownstreamResponse { response: &response, response_head_size: 0 },
                &start,
                &end,
                &mut fmt,
            ))
        })
    });
}

#[allow(clippy::unit_arg)]
fn benchmark_log_headers(c: &mut Criterion) {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    const ENVOY_FORMAT: &str = "%REQ(X-Header-0)% %REQ(X-Header-1)% %REQ(X-Header-2)% %REQ(X-Header-3)% %REQ(X-Header-4)% %REQ(X-Header-5)% %REQ(X-Header-6)% %REQ(X-Header-7)% %REQ(X-Header-8)% %REQ(X-Header-9)% \
        %REQ(X-Header-10)% %REQ(X-Header-11)% %REQ(X-Header-12)% %REQ(X-Header-13)% %REQ(X-Header-14)% %REQ(X-Header-15)% %REQ(X-Header-16)% %REQ(X-Header-17)% %REQ(X-Header-18)% %REQ(X-Header-19)% %REQ(X-Header-20)% \
        %REQ(X-Header-21)% %REQ(X-Header-22)% %REQ(X-Header-23)% %REQ(X-Header-24)% %REQ(X-Header-25)% %REQ(X-Header-26)% %REQ(X-Header-27)% %REQ(X-Header-28)% %REQ(X-Header-29)% %REQ(X-Header-30)% %REQ(X-Header-31)% \
        %REQ(X-Header-32)%";

    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("X-Header-0", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-1", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-2", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-3", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-4", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-5", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-6", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-7", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-8", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-9", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-10", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-11", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-12", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-13", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-14", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-15", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-16", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-17", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-18", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-19", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-20", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-21", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-23", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-24", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-25", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-26", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-27", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-28", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-29", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-30", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-31", "XXXXXXXXXXXXXXXXXXXX")
        .header("X-Header-32", "XXXXXXXXXXXXXXXXXXXX")
        .body(())
        .stealth_unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).stealth_unwrap();
    let start = InitContext { start_time: std::time::SystemTime::now() };
    let end = FinishContext {
        duration: Duration::from_millis(100),
        bytes_received: 128,
        bytes_sent: 256,
        response_flags: ResponseFlags::empty(),
    };

    let fmt = LogFormatter::try_new(ENVOY_FORMAT, false).stealth_unwrap();

    c.bench_function("log_format_headers", |b| {
        b.iter(|| {
            let mut fmt = fmt.local_clone();
            black_box(eval_format(
                &DownstreamContext { request: &request, trace_id: None, request_head_size: 0 },
                &DownstreamResponse { response: &response, response_head_size: 0 },
                &start,
                &end,
                &mut fmt,
            ));
        })
    });
}

criterion_group!(
    benches,
    benchmark_rust_format,
    benchmark_log_formatter,
    benchmark_log_headers,
    benchmark_request_parts
);

criterion_main!(benches);
