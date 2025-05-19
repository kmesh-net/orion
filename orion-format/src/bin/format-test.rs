use criterion::black_box;
use http::{Request, Response, StatusCode};
use orion_format::{
    context::{Context, DownstreamRequest, DownstreamResponse, FinishContext, InitContext},
    types::ResponseFlags,
    LogFormatter,
};
use std::time::{Duration, Instant};

const DEF_FMT: &str = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(X-ENVOY-ORIGINAL-PATH?:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#;

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

const TOTAL: u64 = 100_000_000;

fn main() {
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
        response_flags: ResponseFlags::empty(),
    };

    let fmt = LogFormatter::try_new(DEF_FMT).unwrap();
    // let mut sink = std::io::sink();

    println!("Running {} log format...", TOTAL);

    let now = Instant::now();

    for _ in 0..TOTAL {
        let mut fmt_ = black_box(fmt.clone());
        black_box(eval_format(&DownstreamRequest(&request), &DownstreamResponse(&response), &start, &end, &mut fmt_));
        // _ = black_box(|| fmt_.write_to(&mut sink));
    }

    let dur = now.elapsed();

    println!(
        "LogFormat: {:.2} msg/sec - avg duration {:.2} nsec",
        TOTAL as f64 / dur.as_secs_f64(),
        (dur.as_secs_f64() * 1_000_000_000.0) / TOTAL as f64
    );
}
