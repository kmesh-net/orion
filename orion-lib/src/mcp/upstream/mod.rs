//mod openapi;
mod sse;
mod stdio;
mod streamablehttp;

use indexmap::IndexMap;
use orion_error::Context;
use rmcp::model::{ClientNotification, ClientRequest, JsonRpcRequest};
use rmcp::transport::{TokioChildProcess, streamable_http_client::StreamableHttpPostResponse};
use std::io;
use std::result::Result;
use std::sync::Arc;
use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, trace};

//use crate::proxy::httpproxy::PolicyClient;
use super::{mergestream, upstream};
use super::{
    router::{McpBackendGroup, McpTarget},
    types::agent::McpTargetSpec,
};
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
    pi: Arc<ProxyInputs>,
    backend: McpBackendGroup,
    by_name: IndexMap<String, Arc<upstream::Upstream>>,
}

impl UpstreamGroup {
    pub(crate) fn new(pi: Arc<ProxyInputs>, backend: McpBackendGroup) -> Result<Self, orion_error::Error> {
        let mut s = Self { backend, pi, by_name: IndexMap::new() };
        s.setup_connections()?;
        Ok(s)
    }

    pub(crate) fn setup_connections(&mut self) -> Result<(), orion_error::Error> {
        for tgt in &self.backend.targets {
            debug!("initializing target: {}", tgt.name);
            let transport = self.setup_upstream(tgt.as_ref())?;
            self.by_name.insert(tgt.name.clone(), Arc::new(transport));
        }
        Ok(())
    }

    pub(crate) fn iter_named(&self) -> impl Iterator<Item = (String, Arc<upstream::Upstream>)> {
        self.by_name.iter().map(|(k, v)| (k.clone(), v.clone()))
    }
    pub(crate) fn get(&self, name: &str) -> Result<&upstream::Upstream, orion_error::Error> {
        self.by_name
            .get(name)
            .map(|v| v.as_ref())
            .ok_or_else(|| orion_error::Error::from("requested target {name} is not initialized"))
    }

    fn setup_upstream(&self, target: &McpTarget) -> Result<upstream::Upstream, orion_error::Error> {
        trace!("connecting to target: {}", target.name);
        let target = match &target.spec {
            McpTargetSpec::Sse(sse) => {
                debug!("starting streamable http transport for target: {}", target.name);
                let path = match sse.path.as_str() {
                    "" => "/sse",
                    _ => sse.path.as_str(),
                };
                let be = crate::proxy::resolve_simple_backend(&sse.backend, &self.pi)?;
                let client = sse::Client::new(be, path.into(), self.client.clone(), target.backend_policies.clone())?;

                upstream::Upstream::McpSSE(client)
            },
            McpTargetSpec::Mcp(mcp) => {
                debug!("starting streamable http transport for target: {}", target.name);
                let path = match mcp.path.as_str() {
                    "" => "/mcp",
                    _ => mcp.path.as_str(),
                };
                let be = crate::proxy::resolve_simple_backend(&mcp.backend, &self.pi)?;
                let client =
                    streamablehttp::Client::new(be, path.into(), self.client.clone(), target.backend_policies.clone())?;

                upstream::Upstream::McpStreamable(client)
            },
            McpTargetSpec::Stdio { cmd, args, env } => {
                debug!("starting stdio transport for target: {}", target.name);
                #[cfg(target_os = "windows")]
                // Command has some weird behavior on Windows where it expects the executable extension to be
                // .exe. The which create will resolve the actual command for us.
                // See https://github.com/rust-lang/rust/issues/37519#issuecomment-1694507663
                // for more context.
                let cmd = which::which(cmd)?;
                #[cfg(target_family = "unix")]
                let mut c = Command::new(cmd);
                #[cfg(target_os = "windows")]
                let mut c = Command::new(&cmd);
                c.args(args);
                for (k, v) in env {
                    c.env(k, v);
                }
                let proc =
                    TokioChildProcess::new(c).with_context_msg(format!("failed to run command '{:?}'", &cmd).into())?;
                upstream::Upstream::McpStdio(upstream::stdio::Process::new(proc))
            },
            // McpTargetSpec::OpenAPI(open) => {
            //     // Renamed for clarity
            //     debug!("starting OpenAPI transport for target: {}", target.name);
            //     panic!("Not ported yet");

            //     // let tools = openapi::parse_openapi_schema(&open.schema).map_err(|e| {
            //     //     anyhow!("Failed to parse tools from OpenAPI schema for target {}: {}", target.name, e)
            //     // })?;

            //     // let prefix = openapi::get_server_prefix(&open.schema).map_err(|e| {
            //     //     anyhow!("Failed to get server prefix from OpenAPI schema for target {}: {}", target.name, e)
            //     // })?;
            //     // let be = crate::proxy::resolve_simple_backend(&open.backend, &self.pi)?;
            //     // upstream::Upstream::OpenAPI(Box::new(openapi::Handler {
            //     //     backend: be,
            //     //     client: self.client.clone(),
            //     //     default_policies: target.backend_policies.clone(),
            //     //     tools,  // From parse_openapi_schema
            //     //     prefix, // From get_server_prefix
            //     // }))
            // },
        };

        Ok(target)
    }
}
