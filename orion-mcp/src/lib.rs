mod handler;
mod json;
mod mergestream;
//mod metrics;
//mod rbac;
mod router;
mod session;
mod sse;
mod streamablehttp;
mod upstream;

use std::sync::Arc;

use orion_lib::Error as BoxError;

pub use router::App;
use thiserror::Error;

use arc_swap::{ArcSwap, ArcSwapOption};

type AtomicOption<T> = Arc<ArcSwapOption<T>>;
type Atomic<T> = Arc<ArcSwap<T>>;
type Response = http::Response<orion_lib::PolyBody>;
type Request = http::Request<orion_lib::PolyBody>;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("http request failed with code: {}", .0.status())]
    Status(Box<Response>),
    #[error("http request failed: {0}")]
    General(Arc<orion_lib::Error>),
}

impl ClientError {
    pub fn new(error: impl Into<BoxError>) -> Self {
        Self::General(Arc::new(orion_lib::Error::from(error.into())))
    }
}

#[derive(Debug, Default, Clone)]
pub struct MCPInfo {
    pub tool_call_name: Option<String>,
    pub target_name: Option<String>,
}
