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
            .serve(|| |_request| async { Ok::<_, hyper::Error>(Response::new("Hello, world.".to_string())) })
            .await
            .expect("failed to serve a connection")
    };
    future.await
}
