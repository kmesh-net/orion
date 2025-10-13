use std::sync::Arc;

use crate::mcp::Request;
use crate::mcp::Response;
use crate::mcp::handler::Relay;
use crate::mcp::session::SessionManager;
use crate::mcp::streamablehttp::StreamableHttpService;
use crate::transport::HttpChannel;
use ahash::HashMap;
use http::Uri;
use rmcp::transport::StreamableHttpServerConfig;

#[derive(Debug, Clone)]
pub struct App {
    session_manager: Arc<SessionManager>,
}

#[derive(Debug, Clone)]
pub enum McpBackend {
    Stdio { cmd: String, vars: Vec<String>, args: Vec<String> },
    StreamableHttp { http_channel: HttpChannel, uri: Uri },
}
impl App {
    pub fn new_with_session_manager(session_manager: Arc<SessionManager>) -> Self {
        Self { session_manager }
    }

    pub async fn serve(&self, original_request: Request, mcp_backends: HashMap<String, (McpBackend, Uri)>) -> Response {
        let sm = self.session_manager.clone();
        let default_target_name =
            if mcp_backends.keys().len() == 1 { mcp_backends.keys().last().cloned() } else { None };
        let streamable = StreamableHttpService::new(
            move || Relay::new(default_target_name.clone(), mcp_backends.clone()),
            sm,
            StreamableHttpServerConfig { stateful_mode: true, ..Default::default() },
        );
        streamable.handle(original_request).await
    }
}
