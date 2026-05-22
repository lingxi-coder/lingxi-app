//! `MockMcpTransport` — minimal in-memory MCP transport for engine tests.
//!
//! Tests pre-register tool DTOs with [`MockMcpTransport::add_tool`] and
//! then drive `lingxi_mcp::McpRegistry::connect` against the mock. The
//! mock is intentionally tiny: it only implements enough of
//! [`McpTransport`] to walk the connect → initialize → `list_tools` path.

#![allow(clippy::unwrap_used)] // Mutex lock failures here mean the test is broken.

use async_trait::async_trait;
use lingxi_protocol::McpConnectionId;
use lingxi_traits::{
    ElicitRequestDto, ElicitResultDto, McpError, McpNotificationStream, McpPromptDto,
    McpRawConnection, McpResourceContentDto, McpResourceDto, McpToolDto, McpToolResultDto,
    McpTransport, McpTransportKind, McpTransportSpec, ServerCapabilitiesDto,
};
use serde_json::Value;
use std::sync::Mutex;

/// In-memory MCP transport that returns canned responses to the registry.
pub struct MockMcpTransport {
    tools: Mutex<Vec<McpToolDto>>,
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

#[async_trait]
impl McpTransport for MockMcpTransport {
    async fn connect(&self, _spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        Ok(McpRawConnection {
            connection_id: McpConnectionId::new(),
        })
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

    async fn disconnect(&self, _conn_id: McpConnectionId) -> Result<(), McpError> {
        Ok(())
    }

    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![McpTransportKind::Stdio, McpTransportKind::InProcess]
    }
}
