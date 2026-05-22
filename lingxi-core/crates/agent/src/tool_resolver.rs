//! Resolve the tool set exposed to a subagent.
//!
//! [`AgentToolResolver`] projects the parent agent's tool set onto the child
//! according to the child's [`AgentToolPolicy`], appends the per-agent MCP
//! tools, and then applies the read-only filter when the child runs in
//! [`AgentPermissionMode::Plan`]. See spec §10.8.

use crate::definition::{AgentDefinition, AgentPermissionMode, AgentToolPolicy};
use lingxi_tools::Tool;
use std::sync::Arc;

/// Stateless utility that computes the effective tool set for an agent
/// spawn from the agent definition plus the surrounding tool sets.
pub struct AgentToolResolver;

impl AgentToolResolver {
    /// Compute the effective tool list for a subagent.
    ///
    /// * `agent_def` — the spawning agent's definition (drives the policy).
    /// * `parent_tools` — tools the parent agent had access to.
    /// * `agent_mcp_tools` — tools surfaced by the agent's MCP servers.
    /// * `_coordinator_mode` — reserved; future coordinator-only filters
    ///   will land in Plan 07.
    #[must_use]
    pub fn resolve(
        agent_def: &AgentDefinition,
        parent_tools: &[Arc<dyn Tool>],
        agent_mcp_tools: &[Arc<dyn Tool>],
        _coordinator_mode: bool,
    ) -> Vec<Arc<dyn Tool>> {
        let mut tools = match &agent_def.tools {
            AgentToolPolicy::All { use_exact_tools } => {
                // Both branches return the same vector today; the
                // `use_exact_tools` flag will diverge in Plan 08 when fork
                // mode needs byte-exact tool instances.
                #[allow(clippy::if_same_then_else)]
                if *use_exact_tools {
                    parent_tools.to_vec()
                } else {
                    parent_tools.to_vec()
                }
            }
            AgentToolPolicy::Explicit(names) => parent_tools
                .iter()
                .filter(|t| names.contains(&t.name().to_string()))
                .cloned()
                .collect(),
            AgentToolPolicy::Except(names) => parent_tools
                .iter()
                .filter(|t| !names.contains(&t.name().to_string()))
                .cloned()
                .collect(),
        };
        tools.extend(agent_mcp_tools.iter().cloned());
        if agent_def.permission_mode == AgentPermissionMode::Plan {
            tools.retain(|t| {
                matches!(t.name(), "Read" | "Grep" | "Glob" | "WebSearch" | "WebFetch")
            });
        }
        tools
    }
}
