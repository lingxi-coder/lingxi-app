//! Owns every MCP connection and drives the state machine.
//!
//! Engine code holds an `Arc<McpRegistry>` and uses [`Self::connect`] /
//! [`Self::disconnect`] to manage servers. Health checks and reconnects
//! land in Plan 13.

use crate::connection::{McpConnectionState, McpServerConfig};
use lingxi_protocol::{AgentId, McpConnectionId};
use lingxi_traits::{McpError, McpTransport};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

/// In-memory registry of every known MCP connection.
pub struct McpRegistry {
    /// Map of server name to current state.
    connections: RwLock<HashMap<String, McpConnectionState>>,
    /// Per-agent connection scoping (subagent isolation).
    #[allow(dead_code)] // populated by `register_for_agent` in Plan 13
    agent_scoped: RwLock<HashMap<AgentId, HashMap<String, McpConnectionId>>>,
    /// Transport boundary supplied by the host platform.
    transport: Arc<dyn McpTransport>,
    /// Interval used by the background health-check task.
    #[allow(dead_code)] // consumed by the health-check loop in Plan 13
    pub health_check_interval: Duration,
    /// Maximum consecutive reconnect attempts before declaring `Failed`.
    #[allow(dead_code)] // consumed by the reconnect loop in Plan 13
    pub max_retry_count: u32,
}

impl McpRegistry {
    /// Build a registry bound to a platform transport.
    #[must_use]
    pub fn new(transport: Arc<dyn McpTransport>) -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            agent_scoped: RwLock::new(HashMap::new()),
            transport,
            health_check_interval: Duration::from_secs(30),
            max_retry_count: 5,
        }
    }

    /// Connect, run `initialize`, and discover the server's catalog.
    ///
    /// Re-uses the existing connection if `config.name` is already in the
    /// `Connected` state.
    pub async fn connect(&self, config: McpServerConfig) -> Result<McpConnectionId, McpError> {
        if let Some(McpConnectionState::Connected { connection_id, .. }) =
            self.connections.read().await.get(&config.name)
        {
            return Ok(*connection_id);
        }

        self.connections.write().await.insert(
            config.name.clone(),
            McpConnectionState::Connecting {
                config: config.clone(),
                started_at: SystemTime::now(),
            },
        );

        let conn = self.transport.connect(&config.spec).await?;
        let caps = self.transport.initialize(&conn).await?;
        let tools = self.transport.list_tools(&conn).await?;
        let resources = self.transport.list_resources(&conn).await?;
        let prompts = self.transport.list_prompts(&conn).await?;

        let connection_id = conn.connection_id;
        self.connections.write().await.insert(
            config.name.clone(),
            McpConnectionState::Connected {
                config,
                connection_id,
                capabilities: caps,
                tools,
                resources,
                prompts,
                connected_at: SystemTime::now(),
            },
        );
        Ok(connection_id)
    }

    /// Drop the named connection and transition it to `Stopped`.
    pub async fn disconnect(&self, name: &str) -> Result<(), McpError> {
        let mut conns = self.connections.write().await;
        if let Some(McpConnectionState::Connected {
            connection_id,
            config,
            ..
        }) = conns.remove(name)
        {
            self.transport.disconnect(connection_id).await?;
            conns.insert(name.into(), McpConnectionState::Stopped { config });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Full mock-transport test lives in test-harness/tests/mcp_lifecycle.rs.
}
