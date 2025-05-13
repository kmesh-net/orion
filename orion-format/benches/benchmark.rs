use criterion::{black_box, criterion_group, criterion_main, Criterion};
use http::{Request, Response, StatusCode};
use orion_format::{
    context::{Context, DownStreamContext, DownStreamRequest, DownStreamResponse, UpStreamContext},
    smol_cow::SmolCow,
    LogFormatter,
};
use smol_str::SmolStr;
use std::time::Duration;

const DEF_FMT : &str = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#;

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
            _ = black_box(|| { fmt.write_to(&mut sink) });
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

criterion_group!(benches, benchmark_default_format, benchmark_format_and_write, benchmark_clone_formatter, benchmark_request, benchmark_small_cow);
criterion_main!(benches);
