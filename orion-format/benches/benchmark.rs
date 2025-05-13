use chrono::{DateTime, SecondsFormat, Utc};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode, Version};
use orion_format::{
    context::{Context, DownStreamContext, DownStreamRequest, DownStreamResponse, UpStreamContext},
    smol_cow::SmolCow,
    LogFormatter,
};
use smol_str::SmolStr;
use std::time::Duration;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const DEF_FMT: &str = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#;

#[inline]
fn eval_format<C1, C2, C3, C4>(req: &C1, resp: &C2, start: &C3, end: &C4, mut fmt: LogFormatter)
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

#[inline(always)]
fn header_lookup(name: &str, m: &HeaderMap) -> HeaderValue {
    m.get(name).map(HeaderValue::to_owned).unwrap_or(HeaderValue::from_static(""))
}

fn benchmark_rust_format(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).unwrap();
    let start = DownStreamContext { start_time: std::time::SystemTime::now() };
    let end = UpStreamContext { duration: Duration::from_millis(100), bytes_received: 128, bytes_sent: 256 };

    c.bench_function("Pure format!", |b| {
        b.iter(|| {
            let start_time = start.start_time.clone();
            let datetime_utc: DateTime<Utc> = start_time.into();
            let rfc3339 = datetime_utc.to_rfc3339_opts(SecondsFormat::Millis, true);

            let method = request.method().clone();
            let uri = request.uri().clone();

            let protocol = request.version().clone();
            let ver = match protocol {
                Version::HTTP_10 => "HTTP/1.0",
                Version::HTTP_11 => "HTTP/1.1",
                Version::HTTP_2 => "HTTP/2",
                Version::HTTP_3 => "HTTP/3",
                _ => "HTTP/UNKNOWN",
            };

            let response_code = response.status().clone();
            let end_context = end.clone();

            let x_envoy_upstream_service_time = header_lookup("X-ENVOY-UPSTREAM-SERVICE-TIME", request.headers());
            let x_forwarded_for = header_lookup("SX-FORWARDED-FORER-AGENT", request.headers());
            let user_agent = header_lookup("USER-AGENT", request.headers());
            let x_request_id = header_lookup("X-REQUEST-ID", request.headers());

            black_box(format!(
                r#"[{}] "{} {} {} {} %RESPONSE_FLAGS% {} {} {} {} {} {} {} {} "%UPSTREAM_HOST%""#,
                rfc3339,
                method.as_str(),
                uri.path(),
                ver,
                response_code.as_u16(),
                end_context.bytes_received,
                end_context.bytes_sent,
                end_context.duration.as_millis(),
                x_envoy_upstream_service_time.to_str().unwrap(),
                x_forwarded_for.to_str().unwrap(),
                user_agent.to_str().unwrap(),
                x_request_id.to_str().unwrap(),
                uri.authority().unwrap().host()
            ));
        });
    });
}

fn benchmark_request(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).unwrap();
    let start = DownStreamContext { start_time: std::time::SystemTime::now() };
    let end = UpStreamContext { duration: Duration::from_millis(100), bytes_received: 128, bytes_sent: 256 };

    let fmt = LogFormatter::try_new("%REQ(:PATH)%").unwrap();
    c.bench_function("REQ(:PATH)", |b| {
        b.iter(|| {
            black_box(eval_format(
                &DownStreamRequest(&request),
                &DownStreamResponse(&response),
                &start,
                &end,
                fmt.clone(),
            ))
        })
    });

    let fmt = LogFormatter::try_new("%REQ(:METHOD)%").unwrap();
    c.bench_function("REQ(:METHOD)", |b| {
        b.iter(|| {
            black_box(eval_format(
                &DownStreamRequest(&request),
                &DownStreamResponse(&response),
                &start,
                &end,
                fmt.clone(),
            ))
        })
    });

    let fmt = LogFormatter::try_new("%REQ(USER-AGENT)%").unwrap();
    c.bench_function("REQ(USER-AGENT)", |b| {
        b.iter(|| {
            black_box(eval_format(
                &DownStreamRequest(&request),
                &DownStreamResponse(&response),
                &start,
                &end,
                fmt.clone(),
            ))
        })
    });
}

fn benchmark_default_format(c: &mut Criterion) {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).unwrap();
    let start = DownStreamContext { start_time: std::time::SystemTime::now() };
    let end = UpStreamContext { duration: Duration::from_millis(100), bytes_received: 128, bytes_sent: 256 };

    let fmt = LogFormatter::try_new(DEF_FMT).unwrap();

    c.bench_function("Envoy default formatter", |b| {
        b.iter(|| {
            black_box(eval_format(
                &DownStreamRequest(&request),
                &DownStreamResponse(&response),
                &start,
                &end,
                fmt.clone(),
            ))
        })
    });
}

fn benchmark_format_and_write(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).unwrap();
    let start = DownStreamContext { start_time: std::time::SystemTime::now() };
    let end = UpStreamContext { duration: Duration::from_millis(100), bytes_received: 128, bytes_sent: 256 };

    let fmt = LogFormatter::try_new(DEF_FMT).unwrap();

    c.bench_function("Default formatter and write", |b| {
        b.iter(|| {
            black_box(eval_format(
                &DownStreamRequest(&request),
                &DownStreamResponse(&response),
                &start,
                &end,
                fmt.clone(),
            ));

            let mut sink = std::io::sink();
            _ = black_box(|| fmt.write_to(&mut sink));
        })
    });
}

fn benchmark_clone_formatter(c: &mut Criterion) {
    let fmt = LogFormatter::try_new(DEF_FMT).unwrap();
    c.bench_function("Clone formatter", |b| b.iter(|| black_box(fmt.clone())));
}

fn ret_small_cow() -> SmolCow<'static> {
    SmolCow::Borrowed("123456789012345678901234567890")
}

fn ret_small_str() -> SmolStr {
    SmolStr::new("123456789012345678901234567890")
}

fn benchmark_small_cow(c: &mut Criterion) {
    c.bench_function("small_cow", |b| {
        b.iter(|| {
            let s = black_box(ret_small_cow());
            black_box(s.into_owned());
        })
    });
    c.bench_function("small_str", |b| {
        b.iter(|| {
            let s = black_box(ret_small_str());
            black_box(s.clone());
        })
    });
}

criterion_group!(
    benches,
    benchmark_rust_format,
    benchmark_default_format,
    benchmark_format_and_write,
    benchmark_clone_formatter,
    benchmark_request,
    benchmark_small_cow
);
criterion_main!(benches);
