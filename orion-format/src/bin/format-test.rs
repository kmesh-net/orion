// Copyright 2025 The kmesh Authors
//
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
//

use criterion::black_box;
use http::{Request, Response, StatusCode};
use orion_format::{
    context::{Context, DownstreamContext, DownstreamResponse, FinishContext, InitContext},
    types::ResponseFlags,
    LogFormatter, LogFormatterLocal,
};
use std::time::{Duration, Instant};

const DEF_FMT: &str = r#"[%START_TIME%] "%REQ(:METHOD)% %REQ(X-ENVOY-ORIGINAL-PATH?:PATH)% %PROTOCOL%" %RESPONSE_CODE% %RESPONSE_FLAGS% %BYTES_RECEIVED% %BYTES_SENT% %DURATION% %RESP(X-ENVOY-UPSTREAM-SERVICE-TIME)% "%REQ(X-FORWARDED-FOR)%" "%REQ(USER-AGENT)%" "%REQ(X-REQUEST-ID)%" "%REQ(:AUTHORITY)%" "%UPSTREAM_HOST%""#;

#[inline]
fn eval_format<C1, C2, C3, C4>(req: &C1, resp: &C2, start: &C3, end: &C4, fmt: &mut LogFormatterLocal) -> bool
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
    true
}

const TOTAL: u64 = 100_000_000;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[allow(clippy::cast_precision_loss)]
fn main() -> Result<(), BoxError> {
    let request = Request::builder()
        .uri("https://www.rust-lang.org/hello")
        .header("User-Agent", "my-awesome-agent/1.0")
        .body(())?;

    let response = Response::builder().status(StatusCode::OK).body(())?;
    let start = InitContext { start_time: std::time::SystemTime::now() };
    let end = FinishContext {
        duration: Duration::from_millis(100),
        bytes_received: 128,
        bytes_sent: 256,
        response_flags: ResponseFlags::empty(),
    };

    let fmt = LogFormatter::try_new(DEF_FMT, false)?;
    // let mut sink = std::io::sink();

    println!("Running {TOTAL} log format...");

    let now = Instant::now();

    for _ in 0..TOTAL {
        let mut fmt = black_box(fmt.local_clone());
        black_box(eval_format(
            &DownstreamContext { request: &request, request_head_size: 0, trace_id: None },
            &DownstreamResponse { response: &response, response_head_size: 0 },
            &start,
            &end,
            &mut fmt,
        ));
    }

    let dur = now.elapsed();

    println!(
        "LogFormat: {:.2} msg/sec - avg duration {:.2} nsec",
        TOTAL as f64 / dur.as_secs_f64(),
        (dur.as_secs_f64() * 1_000_000_000.0) / TOTAL as f64
    );
    Ok(())
}
