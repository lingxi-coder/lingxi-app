//! MCP stdio transport — Windows.
//!
//! M2.03 ships the type surface and method routing. Full `JSON-RPC` framing
//! (line-delimited and `Content-Length`-prefixed), request id tracking, and
//! notification streaming land in M2 phase 3.

use async_trait::async_trait;
use lingxi_protocol::McpConnectionId;
use lingxi_traits::{
    ElicitRequestDto, ElicitResultDto, McpError, McpNotificationStream, McpPromptDto,
    McpRawConnection, McpResourceContentDto, McpResourceDto, McpToolDto, McpToolResultDto,
    McpTransport, McpTransportKind, McpTransportSpec, ServerCapabilitiesDto,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::process::Child;

/// Windows MCP transport — supports the `Stdio` variant only in M2.03.
///
/// Other transports (`Sse`, `Http`, `WebSocket`, `InProcess`, `SseIde`,
/// `SdkControl`) return `McpError::UnsupportedTransport`. Most request
/// methods are intentionally stubbed pending full `JSON-RPC` framing in
/// M2 phase 3.
#[derive(Default)]
pub struct WindowsMcpTransport {
    connections: Mutex<HashMap<McpConnectionId, Child>>,
}

impl WindowsMcpTransport {
    /// Construct a new `WindowsMcpTransport`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl McpTransport for WindowsMcpTransport {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        let (command, args, env) = match spec {
            McpTransportSpec::Stdio { command, args, env } => {
                (command.clone(), args.clone(), env.clone())
            }
            other => return Err(McpError::UnsupportedTransport(map_kind(other))),
        };
        let mut cmd = tokio::process::Command::new(&command);
        cmd.args(&args);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = cmd
            .spawn()
            .map_err(|e| McpError::Connection(e.to_string()))?;
        let id = McpConnectionId::new();
        // Recover from a poisoned std `Mutex` by extracting the inner map.
        if let Ok(mut conns) = self.connections.lock() {
            conns.insert(id, child);
        }
        Ok(McpRawConnection { connection_id: id })
    }

    async fn initialize(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<ServerCapabilitiesDto, McpError> {
        // M2.03 stub — full `JSON-RPC` initialize lands in M2 phase 3.
        Ok(ServerCapabilitiesDto {
            tools: true,
            resources: false,
            prompts: false,
            logging: false,
            experimental: HashMap::new(),
        })
    }

    async fn list_tools(&self, _conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
        Err(McpError::Internal(
            "windows mcp stdio list_tools: M2 follow-up".into(),
        ))
    }

    async fn list_resources(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceDto>, McpError> {
        Ok(Vec::new())
    }

    async fn list_prompts(&self, _conn: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
        Ok(Vec::new())
    }

    async fn call_tool(
        &self,
        _conn: &McpRawConnection,
        _tool: &str,
        _input: Value,
    ) -> Result<McpToolResultDto, McpError> {
        Err(McpError::Internal(
            "windows mcp stdio call_tool: M2 follow-up".into(),
        ))
    }

    async fn read_resource(
        &self,
        _conn: &McpRawConnection,
        _uri: &str,
    ) -> Result<McpResourceContentDto, McpError> {
        Err(McpError::Internal(
            "windows mcp stdio read_resource: M2 follow-up".into(),
        ))
    }

    async fn ping(&self, _conn_id: McpConnectionId) -> Result<(), McpError> {
        Ok(())
    }

    async fn notifications(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<McpNotificationStream, McpError> {
        use futures::stream::empty;
        Ok(Box::pin(empty()))
    }

    async fn handle_elicitation(
        &self,
        _conn: &McpRawConnection,
        _req: ElicitRequestDto,
    ) -> Result<ElicitResultDto, McpError> {
        Err(McpError::Internal(
            "windows mcp elicitation: M2 follow-up".into(),
        ))
    }

    async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError> {
        let child_opt = self
            .connections
            .lock()
            .ok()
            .and_then(|mut conns| conns.remove(&conn_id));
        if let Some(mut child) = child_opt {
            let _ = child.kill().await;
        }
        Ok(())
    }

    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![McpTransportKind::Stdio]
    }
}

fn map_kind(spec: &McpTransportSpec) -> McpTransportKind {
    match spec {
        McpTransportSpec::Stdio { .. } => McpTransportKind::Stdio,
        McpTransportSpec::Sse { .. } => McpTransportKind::Sse,
        McpTransportSpec::Http { .. } => McpTransportKind::Http,
        McpTransportSpec::WebSocket { .. } => McpTransportKind::WebSocket,
        McpTransportSpec::InProcess { .. } => McpTransportKind::InProcess,
        McpTransportSpec::SseIde { .. } => McpTransportKind::SseIde,
        McpTransportSpec::SdkControl { .. } => McpTransportKind::SdkControl,
    }
}
