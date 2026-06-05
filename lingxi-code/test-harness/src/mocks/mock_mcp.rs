//! `MockMcpTransport` — minimal in-memory MCP transport for engine tests.
//!
//! Tests pre-register tool DTOs with [`MockMcpTransport::add_tool`] and
//! then drive `mcp::McpRegistry::connect` against the mock. The
//! mock is intentionally tiny: it only implements enough of
//! [`McpTransport`] to walk the connect → initialize → `list_tools` path.

#![allow(clippy::unwrap_used)] // Mutex lock failures here mean the test is broken.

use async_trait::async_trait;
use bytes::Bytes;
use jsonrpc::{Connection, Mode};
use protocol::McpConnectionId;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use traits::{
    ElicitRequestDto, ElicitResultDto, McpError, McpNotificationStream, McpPromptDto,
    McpRawConnection, McpResourceContentDto, McpResourceDto, McpToolDto, McpToolResultDto,
    McpTransport, McpTransportKind, McpTransportSpec, ServerCapabilitiesDto,
};

/// In-memory MCP transport that returns canned responses to the registry.
pub struct MockMcpTransport {
    tools: Mutex<Vec<McpToolDto>>,
    /// Paired in-memory `jsonrpc::Connection`s minted per `connect`, keyed by
    /// the `McpConnectionId` handed back. Exposed via [`RawConnectionProvider`]
    /// so `McpRegistry::with_raw_conn` can bridge a live `McpClient`.
    conns: Mutex<HashMap<McpConnectionId, Arc<Connection>>>,
}

impl Default for MockMcpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MockMcpTransport {
    /// Build a fresh mock with no tools.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Mutex::new(Vec::new()),
            conns: Mutex::new(HashMap::new()),
        }
    }

    /// Register a tool named `name` under the server label `mock`.
    ///
    /// The full name follows the engine's convention `mcp__<server>__<tool>`.
    pub fn add_tool(&self, name: &str) {
        self.tools.lock().unwrap().push(McpToolDto {
            server_name: "mock".into(),
            tool_name: name.into(),
            description: format!("{name} test tool"),
            input_schema: serde_json::json!({"type": "object"}),
            full_name: format!("mcp__mock__{name}"),
        });
    }
}

/// Build a `Connection` over a fresh pair of `mpsc<Bytes>` channels (the
/// `paired_connection` pattern from `mcp/src/client.rs:536`). The peer ends are
/// dropped — lifecycle tests assert client *presence*, not wire round-trips.
fn paired_connection() -> Arc<Connection> {
    let (_peer_to_us_tx, peer_to_us_rx) = tokio::sync::mpsc::channel::<Bytes>(8);
    let (us_to_peer_tx, _us_to_peer_rx) = tokio::sync::mpsc::channel::<Bytes>(8);
    Arc::new(Connection::new_streams(
        peer_to_us_rx,
        us_to_peer_tx,
        Mode::Lines,
    ))
}

#[async_trait]
impl McpTransport for MockMcpTransport {
    async fn connect(&self, _spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        let connection_id = McpConnectionId::new();
        // Stash a paired connection so `RawConnectionProvider::connection_for`
        // can hand the registry a live `Arc<jsonrpc::Connection>`.
        self.conns
            .lock()
            .unwrap()
            .insert(connection_id, paired_connection());
        Ok(McpRawConnection { connection_id })
    }

    async fn initialize(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<ServerCapabilitiesDto, McpError> {
        Ok(ServerCapabilitiesDto {
            tools: true,
            resources: false,
            prompts: false,
            logging: false,
            experimental: std::collections::HashMap::new(),
        })
    }

    async fn list_tools(&self, _conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
        Ok(self.tools.lock().unwrap().clone())
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
        Ok(McpToolResultDto {
            content: serde_json::json!("ok"),
            is_error: false,
        })
    }

    async fn read_resource(
        &self,
        _conn: &McpRawConnection,
        _uri: &str,
    ) -> Result<McpResourceContentDto, McpError> {
        Err(McpError::Internal("not implemented".into()))
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
        _c: &McpRawConnection,
        _r: ElicitRequestDto,
    ) -> Result<ElicitResultDto, McpError> {
        Err(McpError::Internal("not implemented".into()))
    }

    async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError> {
        self.conns.lock().unwrap().remove(&conn_id);
        Ok(())
    }

    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![McpTransportKind::Stdio, McpTransportKind::InProcess]
    }
}

impl mcp::RawConnectionProvider for MockMcpTransport {
    fn connection_for(&self, id: McpConnectionId) -> Option<Arc<Connection>> {
        self.conns.lock().unwrap().get(&id).cloned()
    }
}
