use chrono::{DateTime, SecondsFormat, Utc};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode, Version};
use orion_format::{
    context::{Context, DownstreamRequest, DownstreamResponse, FinishContext, InitContext},
    LogFormatter,
};
use std::time::Duration;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const DEF_FMT: &str = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#;

#[inline]
fn eval_format<C1, C2, C3, C4>(req: &C1, resp: &C2, start: &C3, end: &C4, fmt: &mut LogFormatter)
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
fn header_lookup<'a>(name: &'a str, m: &'a HeaderMap, default_header: &'a HeaderValue) -> &'a HeaderValue {
    m.get(name).unwrap_or(default_header)
}

fn benchmark_rust_format(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).unwrap();
    let start = InitContext { start_time: std::time::SystemTime::now() };
    let end = FinishContext {
        duration: Duration::from_millis(100),
        bytes_received: 128,
        bytes_sent: 256,
        response_flags: "-",
    };
    let mut sink = std::io::sink();

    c.bench_function("Rust format!", |b| {
        b.iter(|| {
            let s =
                black_box(eval_rust_format(&DownstreamRequest(&request), &DownstreamResponse(&response), &start, &end));
            _ = black_box(write_to_format(&mut sink, s.as_bytes()));
        })
    });

    let default_haader_value = HeaderValue::from_static("");

    c.bench_function("Rust format! (full clone)", |b| {
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

            let x_envoy_upstream_service_time =
                header_lookup("X-ENVOY-UPSTREAM-SERVICE-TIME", request.headers(), &default_haader_value);
            let x_forwarded_for = header_lookup("SX-FORWARDED-FORER-AGENT", request.headers(), &default_haader_value);
            let user_agent = header_lookup("USER-AGENT", request.headers(), &default_haader_value);
            let x_request_id = header_lookup("X-REQUEST-ID", request.headers(), &default_haader_value);

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

fn write_to_format<W: std::io::Write>(w: &mut W, log: &[u8]) -> std::io::Result<usize> {
    let len = w.write(log)?;
    Ok(len)
}

fn eval_rust_format(
    request: &DownstreamRequest<()>,
    response: &DownstreamResponse<()>,
    init_context: &InitContext,
    finish_context: &FinishContext,
) -> String {
    let request = request.0;
    let response = response.0;
    let start_time = &init_context.start_time;
    let datetime_utc: DateTime<Utc> = start_time.clone().into();
    let rfc3339 = datetime_utc.to_rfc3339_opts(SecondsFormat::Millis, true);

    let method = request.method();
    let uri = request.uri();

    let protocol = request.version();
    let ver = match protocol {
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2",
        Version::HTTP_3 => "HTTP/3",
        _ => "HTTP/UNKNOWN",
    };

    let response_code = response.status();

    let default_header: HeaderValue = HeaderValue::from_static("");
    let x_envoy_upstream_service_time =
        header_lookup("X-ENVOY-UPSTREAM-SERVICE-TIME", request.headers(), &default_header);
    let x_forwarded_for = header_lookup("SX-FORWARDED-FORER-AGENT", request.headers(), &default_header);
    let user_agent = header_lookup("USER-AGENT", request.headers(), &default_header);
    let x_request_id = header_lookup("X-REQUEST-ID", request.headers(), &default_header);

    format!(
        r#"[{}] "{} {} {} {} %RESPONSE_FLAGS% {} {} {} {} {} {} {} {} "%UPSTREAM_HOST%""#,
        rfc3339,
        method.as_str(),
        uri.path(),
        ver,
        response_code.as_u16(),
        finish_context.bytes_received,
        finish_context.bytes_sent,
        finish_context.duration.as_millis(),
        x_envoy_upstream_service_time.to_str().unwrap(),
        x_forwarded_for.to_str().unwrap(),
        user_agent.to_str().unwrap(),
        x_request_id.to_str().unwrap(),
        uri.authority().unwrap().host()
    )
}

fn benchmark_log_formatter(c: &mut Criterion) {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).unwrap();
    let start = InitContext { start_time: std::time::SystemTime::now() };
    let end = FinishContext {
        duration: Duration::from_millis(100),
        bytes_received: 128,
        bytes_sent: 256,
        response_flags: "-",
    };

    let fmt = LogFormatter::try_new(DEF_FMT).unwrap();
    let mut sink = std::io::sink();

    c.bench_function("LogFormatter: clone only", |b| {
        b.iter(|| {
            black_box(fmt.clone());
        })
    });

    c.bench_function("LogFromatter (full)", |b| {
        b.iter(|| {
            let mut fmt = fmt.clone();
            black_box(eval_format(
                &DownstreamRequest(&request),
                &DownstreamResponse(&response),
                &start,
                &end,
                &mut fmt,
            ));
        })
    });

    c.bench_function("LogFormatter (full) + write", |b| {
        b.iter(|| {
            let mut fmt = fmt.clone();
            black_box(eval_format(
                &DownstreamRequest(&request),
                &DownstreamResponse(&response),
                &start,
                &end,
                &mut fmt,
            ));
            _ = black_box(|| fmt.write_to(&mut sink));
        })
    });

    let mut formatted = fmt.clone();
    eval_format(&DownstreamRequest(&request), &DownstreamResponse(&response), &start, &end, &mut formatted);

    c.bench_function("LogFormatter: write only", |b| {
        b.iter(|| {
            _ = black_box(|| formatted.write_to(&mut sink));
        })
    });
}

fn benchmark_request_parts(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).unwrap();
    let start = InitContext { start_time: std::time::SystemTime::now() };
    let end = FinishContext {
        duration: Duration::from_millis(100),
        bytes_received: 128,
        bytes_sent: 256,
        response_flags: "-",
    };

    let fmt = LogFormatter::try_new("%REQ(:PATH)%").unwrap();
    c.bench_function("REQ(:PATH)", |b| {
        b.iter(|| {
            let mut fmt = fmt.clone();
            black_box(eval_format(&DownstreamRequest(&request), &DownstreamResponse(&response), &start, &end, &mut fmt))
        })
    });

    let fmt = LogFormatter::try_new("%REQ(:METHOD)%").unwrap();
    c.bench_function("REQ(:METHOD)", |b| {
        b.iter(|| {
            let mut fmt = fmt.clone();
            black_box(eval_format(&DownstreamRequest(&request), &DownstreamResponse(&response), &start, &end, &mut fmt))
        })
    });

    let fmt = LogFormatter::try_new("%REQ(USER-AGENT)%").unwrap();
    c.bench_function("REQ(USER-AGENT)", |b| {
        b.iter(|| {
            let mut fmt = fmt.clone();
            black_box(eval_format(&DownstreamRequest(&request), &DownstreamResponse(&response), &start, &end, &mut fmt))
        })
    });
}

criterion_group!(benches, benchmark_rust_format, benchmark_log_formatter, benchmark_request_parts);

criterion_main!(benches);
