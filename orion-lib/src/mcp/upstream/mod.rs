//mod openapi;
mod sse;
mod stdio;
mod streamablehttp;

use ahash::HashMap;
use indexmap::IndexMap;
use orion_error::Context;
use rmcp::model::{ClientNotification, ClientRequest, JsonRpcRequest};
use rmcp::transport::{TokioChildProcess, streamable_http_client::StreamableHttpPostResponse};
use std::collections::BTreeSet;
use std::io;
use std::result::Result;
use std::sync::Arc;
use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, trace, warn};

//use crate::proxy::httpproxy::PolicyClient;
use super::{mergestream, upstream};
use super::{
    router::{McpBackendGroup, McpTarget},
    types::agent::McpTargetSpec,
};
use crate::mcp::{ClientError, Request};
use crate::transport::HttpChannel;

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
    McpSSE(sse::Client),
    McpStdio(stdio::Process),
    //OpenAPI(Box<openapi::Handler>),
}

impl Upstream {
    pub(crate) async fn delete(&self, ctx: &IncomingRequestContext) -> Result<(), UpstreamError> {
        match &self {
            Upstream::McpStdio(c) => {
                c.stop().await?;
            },
            Upstream::McpStreamable(c) => {
                c.send_delete(ctx).await?;
            },
            Upstream::McpSSE(c) => {
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
            Upstream::McpSSE(c) => c.connect_to_event_stream(ctx).await,
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
            Upstream::McpSSE(c) => Ok(mergestream::Messages::from(c.send_message(request, ctx).await?)),
            Upstream::McpStreamable(c) => {
                let is_init = matches!(&request.request, &ClientRequest::InitializeRequest(_));
                let res = c.send_request(request, ctx).await?;
                if is_init {
                    let sid = match &res {
                        StreamableHttpPostResponse::Accepted => None,
                        StreamableHttpPostResponse::Json(_, sid) => sid.as_ref(),
                        StreamableHttpPostResponse::Sse(_, sid) => sid.as_ref(),
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
            Upstream::McpSSE(c) => {
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
    http_channels: HashMap<String, HttpChannel>,
    initialized_channels: BTreeSet<String>,
}

impl UpstreamGroup {
    pub(crate) fn new(http_channels: HashMap<String, HttpChannel>) -> Result<Self, orion_error::Error> {
        let mut s = Self { http_channels, initialized_channels: BTreeSet::new() };
        s.setup_connections()?;
        Ok(s)
    }

    pub(crate) fn setup_connections(&mut self) -> Result<(), orion_error::Error> {
        for (name, channel) in &self.http_channels {
            debug!("initializing target: {}", name);
            if let Ok(()) = self.setup_upstream(name, channel) {
                self.initialized_channels.insert(name.clone());
            } else {
                warn!("Unable to initialize {name}");
            }
        }
        Ok(())
    }

    // fn setup_upstream(&self, name: &str, http_channel: &HttpChannel) -> Result<upstream::Upstream, orion_error::Error> {
    //     debug!("connecting to target: {name}");

    //     McpTargetSpec::Mcp(mcp) => {
    //         debug!("starting streamable http transport for target: {}", target.name);
    //         let path = match mcp.path.as_str() {
    //             "" => "/mcp",
    //             _ => mcp.path.as_str(),
    //         };
    //         let be = crate::proxy::resolve_simple_backend(&mcp.backend, &self.pi)?;
    //         let client =
    //             streamablehttp::Client::new(be, path.into(), self.client.clone(), target.backend_policies.clone())?;

    //         upstream::Upstream::McpStreamable(client)
    //     };
    //     Ok(target)
    // }
}
