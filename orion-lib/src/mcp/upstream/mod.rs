//mod sse;
pub mod response_channel;
mod stdio;
mod streamablehttp;
use ahash::HashMap;
use rmcp::model::{ClientNotification, ClientRequest, JsonRpcRequest};
use rmcp::transport::TokioChildProcess;
use rmcp::transport::child_process::TokioChildProcessBuilder;
use rmcp::transport::streamable_http_client::StreamableHttpPostResponse;
use tokio::process::Command;

use std::io;
use std::result::Result;
use std::sync::Arc;

use thiserror::Error;
use tracing::{debug, warn};

use super::{mergestream, upstream};
use crate::mcp::router::McpBackend;
use crate::mcp::{ClientError, Request};

#[derive(Debug, Clone)]
pub struct IncomingRequestContext {
    headers: http::HeaderMap,
}

impl IncomingRequestContext {
    #[cfg(test)]
    pub fn empty() -> Self {
        Self { headers: http::HeaderMap::new() }
    }
    pub fn new(parts: ::http::request::Parts) -> Self {
        Self { headers: parts.headers }
    }
    pub fn apply(&self, req: &mut Request) {
        for (k, v) in &self.headers {
            // Remove headers we do not want to propagate to the backend
            if k == http::header::CONTENT_ENCODING || k == http::header::CONTENT_LENGTH {
                continue;
            }
            if !req.headers().contains_key(k) {
                req.headers_mut().insert(k.clone(), v.clone());
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum UpstreamError {
    #[error("unauthorized tool call")]
    Authorization,
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("unsupported method: {0}")]
    InvalidMethod(String),
    #[error("method {0} is unsupported with multiplexing")]
    InvalidMethodWithMultiplexing(String),
    #[error("stdio upstream error: {0}")]
    ServiceError(#[from] rmcp::ServiceError),
    #[error("http upstream error: {0}")]
    Http(#[from] ClientError),
    #[error("openapi upstream error: {0}")]
    OpenAPIError(#[from] orion_error::Error),
    #[error("stdio upstream error: {0}")]
    Stdio(#[from] io::Error),
    #[error("upstream closed on send")]
    Send,
    #[error("upstream closed on receive")]
    Recv,
}

// UpstreamTarget defines a source for MCP information.

#[derive(Debug)]
pub(crate) enum Upstream {
    McpStreamable(streamablehttp::Client),
    McpStdio(upstream::stdio::Process),
    //OpenAPI(Box<openapi::Handler>),
}

impl Upstream {
    pub(crate) async fn delete(&self, ctx: &IncomingRequestContext) -> Result<(), UpstreamError> {
        match &self {
            Upstream::McpStreamable(c) => {
                c.send_delete(ctx).await?;
            },
            Upstream::McpStdio(c) => {
                c.stop().await?;
            },
        }
        Ok(())
    }
    pub(crate) async fn get_event_stream(
        &self,
        ctx: &IncomingRequestContext,
    ) -> Result<mergestream::Messages, UpstreamError> {
        match &self {
            Upstream::McpStdio(c) => Ok(c.get_event_stream().await),
            Upstream::McpStreamable(c) => c.get_event_stream(ctx).await?.try_into().map_err(Into::into),
        }
    }
    pub(crate) async fn generic_stream(
        &self,
        request: JsonRpcRequest<ClientRequest>,
        ctx: &IncomingRequestContext,
    ) -> Result<mergestream::Messages, UpstreamError> {
        match &self {
            Upstream::McpStdio(c) => Ok(mergestream::Messages::from(c.send_message(request, ctx).await?)),
            Upstream::McpStreamable(c) => {
                let is_init = matches!(&request.request, &ClientRequest::InitializeRequest(_));
                let res = c.send_request(request, ctx).await?;
                if is_init {
                    let sid = match &res {
                        StreamableHttpPostResponse::Accepted => None,
                        StreamableHttpPostResponse::Json(_, sid) | StreamableHttpPostResponse::Sse(_, sid) => {
                            sid.as_ref()
                        },
                    };
                    if let Some(sid) = sid {
                        c.set_session_id(sid.clone())
                    }
                }
                res.try_into().map_err(Into::into)
            },
        }
    }

    pub(crate) async fn generic_notification(
        &self,
        request: ClientNotification,
        ctx: &IncomingRequestContext,
    ) -> Result<(), UpstreamError> {
        match &self {
            Upstream::McpStdio(c) => {
                c.send_notification(request, ctx).await?;
            },
            Upstream::McpStreamable(c) => {
                c.send_notification(request, ctx).await?;
            },
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct UpstreamGroup {
    streamable_clients: HashMap<String, upstream::Upstream>,
}

impl UpstreamGroup {
    pub(crate) fn new(mcp_backends: HashMap<String, McpBackend>) -> Result<Self, orion_error::Error> {
        let streamable_clients = mcp_backends
            .into_iter()
            .map(|(name, mcp_backend)| match mcp_backend {
                McpBackend::Stdio { cmd, args, envs } => {
                    let mut c = Command::new(cmd);
                    c.args(args);
                    for (k, v) in envs {
                        c.env(k, v);
                    }
                    let maybe_proc = TokioChildProcess::new(c);
                    if let Ok(proc) = maybe_proc {
                        Ok((name, upstream::Upstream::McpStdio(upstream::stdio::Process::new(proc))))
                    } else {
                        Err(orion_error::Error::new(format!(
                            "Could not create a child process for {:?}",
                            maybe_proc.err()
                        )))
                    }
                },

                McpBackend::StreamableHttp { http_channel, uri } => {
                    Ok((name, upstream::Upstream::McpStreamable(streamablehttp::Client::new(http_channel, uri))))
                },
            })
            .collect::<Vec<_>>();
        if streamable_clients.iter().any(|e| e.is_err()) {
            streamable_clients.into_iter().for_each(|e| {
                if e.is_err() {
                    warn!("Problematic backned {:?}", e.err());
                }
            });
            return Err("Problem with configuring backends".into());
        }
        let streamable_clients = streamable_clients.into_iter().filter_map(|f| f.ok()).collect::<HashMap<_, _>>();

        Ok(Self { streamable_clients })
    }

    // pub(crate) fn setup_connections(&mut self) {
    //     self.streamable_clients.iter().for_each(|(name, _)| {
    //         debug!("initializing target: {}", name);
    //     });
    // }

    pub(crate) fn iter_named(&self) -> impl Iterator<Item = (&str, &upstream::Upstream)> {
        self.streamable_clients.iter().map(|(k, v)| (k.as_str(), v))
    }
    pub(crate) fn get(&self, name: &str) -> Result<&upstream::Upstream, orion_error::Error> {
        self.streamable_clients
            .get(name)
            .ok_or_else(|| orion_error::Error::from(format!("requested target {name} is not initialized")))
    }
}
