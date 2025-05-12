use criterion::{black_box, criterion_group, criterion_main, Criterion};
use http::{Request, Response, StatusCode};
use orion_format::{FormatType, LogFormatter};

fn eval_format(req: &Request<()>, resp: &Response<()>, mut fmt: LogFormatter) {
    fmt.with_context(req);
    fmt.with_context(resp);
}

fn benchmark_request(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).unwrap();

    let fmt = LogFormatter::try_new(FormatType::Envoy, "%REQ(:PATH)%").unwrap();
    c.bench_function("REQ(:PATH)", |b| {
        b.iter(|| black_box(eval_format(black_box(&request), black_box(&response), fmt.clone())))
    });

    let fmt = LogFormatter::try_new(FormatType::Envoy, "%REQ(:METHOD)%").unwrap();
    c.bench_function("REQ(:METHOD)", |b| {
        b.iter(|| black_box(eval_format(black_box(&request), black_box(&response), fmt.clone())))
    });

    let fmt = LogFormatter::try_new(FormatType::Envoy, "%REQ(USER-AGENT)%").unwrap();
    c.bench_function("REQ(USER-AGENT)", |b| {
        b.iter(|| black_box(eval_format(black_box(&request), black_box(&response), fmt.clone())))
    });
}

fn benchmark_default_format(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let response = Response::builder().status(StatusCode::OK).body(()).unwrap();

    let def_fmt = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#;
    let fmt = LogFormatter::try_new(FormatType::Envoy, def_fmt).unwrap();

    c.bench_function("Envoy default formatter", |b| {
        b.iter(|| black_box(eval_format(black_box(&request), black_box(&response), fmt.clone())))
    });
}

fn benchmark_clone_formatter(c: &mut Criterion) {
    let def_fmt = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#;
    let fmt = LogFormatter::try_new(FormatType::Envoy, def_fmt).unwrap();
    c.bench_function("Clone formatter", |b| b.iter(|| black_box(black_box(fmt.clone()))));
}

criterion_group!(benches, benchmark_default_format, benchmark_clone_formatter, benchmark_request);
criterion_main!(benches);
