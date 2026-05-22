//! Subagent runner — filled in Task 3.

use lingxi_protocol::AgentId;

/// Placeholder event surface emitted by the subagent runner; replaced in
/// Task 3 with the full progress / completion variants.
#[derive(Debug, Clone)]
pub enum SubagentEvent {
    /// Stub variant carrying just the agent id.
    Placeholder(AgentId),
}
