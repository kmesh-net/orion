use criterion::{black_box, criterion_group, criterion_main, Criterion};
use http::Request;
use orion_format::{FormatType, LogFormatter};

fn eval_format(req: &Request<()>, mut fmt: LogFormatter) {
    fmt.with_context(req);
}

fn benchmark_with_request_context(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let fmt = LogFormatter::try_new(FormatType::Envoy, "%REQ(:PATH)%").unwrap();
    c.bench_function("req(:path)", |b| b.iter(|| black_box(eval_format(black_box(&request), fmt.clone()))));

    let fmt = LogFormatter::try_new(FormatType::Envoy, "%REQ(:METHOD)%").unwrap();
    c.bench_function("req(:method)", |b| b.iter(|| black_box(eval_format(black_box(&request), fmt.clone()))));

    let fmt = LogFormatter::try_new(FormatType::Envoy, "%REQ(USER-AGENT)%").unwrap();
    c.bench_function("req(:user-agent)", |b| b.iter(|| black_box(eval_format(black_box(&request), fmt.clone()))));
}

fn benchmark_default_format(c: &mut Criterion) {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())
        .unwrap();

    let fmt = LogFormatter::try_new(
        FormatType::Envoy,
        r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(:PATH)% %PROTOCOL%"
    %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION%
    %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%"
    "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#,
    )
    .unwrap();

    c.bench_function("Envoy default formatter", |b| {
        b.iter(|| black_box(eval_format(black_box(&request), fmt.clone())))
    });
}

criterion_group!(benches, benchmark_with_request_context, benchmark_default_format);
criterion_main!(benches);
