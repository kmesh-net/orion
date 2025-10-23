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

use std::path::PathBuf;

use clap::Parser;
use hyper::Response;
use hyperlocal::UnixListenerExt;
use tokio::net::UnixListener;

#[derive(Parser, Debug, Clone)]
pub struct Options {
    #[arg(long)]
    path_name: PathBuf,
}
#[tokio::main]
#[allow(clippy::expect_used)]
async fn main() {
    let args: Options = Options::parse();

    let future = async move {
        let listener = UnixListener::bind(args.path_name).expect("parsed unix path");

        listener
            .serve(|| |_request| async { Ok::<_, hyper::Error>(Response::new("Hello, world.".to_owned())) })
            .await
            .expect("failed to serve a connection")
    };
    future.await
}
