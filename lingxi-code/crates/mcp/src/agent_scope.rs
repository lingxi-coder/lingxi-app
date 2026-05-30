//! Per-agent MCP connection scoping.
//!
//! Subagents may register the connections they own so they can be torn
//! down when the agent exits, even if the registry still holds them for
//! other agents.

use lingxi_protocol::{AgentId, McpConnectionId};
use std::collections::HashMap;

/// Tracks which connections each agent is responsible for.
#[derive(Default)]
pub struct AgentScopedConnections {
    inner: HashMap<AgentId, HashMap<String, McpConnectionId>>,
}

impl AgentScopedConnections {
    /// Build an empty scope table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `agent` owns the connection to `server`.
    pub fn register(&mut self, agent: AgentId, server: String, conn: McpConnectionId) {
        self.inner.entry(agent).or_default().insert(server, conn);
    }

    /// Drop the bookkeeping for `agent`, returning the connection IDs the
    /// caller now owns the responsibility to disconnect.
    pub fn cleanup(&mut self, agent: &AgentId) -> Vec<McpConnectionId> {
        self.inner
            .remove(agent)
            .map(|m| m.into_values().collect())
            .unwrap_or_default()
    }
}
