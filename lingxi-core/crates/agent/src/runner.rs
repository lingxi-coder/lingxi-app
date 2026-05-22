//! Subagent state-machine loop.
//!
//! [`run_subagent`] is the future that
//! [`crate::pool::StateMachinePool::allocate`] hands to the runtime. M1.11
//! ships a stub completion after the first inbound event; the full agentic
//! loop in Plan 09+ uses `§22 SessionStorage` and `§23 FileStateCache`.

use crate::context::SubagentContext;
use lingxi_protocol::AgentId;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Events emitted by [`run_subagent`] back to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubagentEvent {
    /// Periodic progress beacon while the agent is still running.
    Progress {
        /// Agent emitting the progress event.
        agent_id: AgentId,
        /// Number of tool calls processed so far.
        tool_use_count: u32,
        /// Cumulative token count consumed so far.
        token_count: u64,
    },
    /// Agent finished normally with `result`.
    Completed {
        /// Agent that completed.
        agent_id: AgentId,
        /// Final result payload (free-form JSON).
        result: serde_json::Value,
    },
    /// Agent terminated due to an error.
    Failed {
        /// Agent that failed.
        agent_id: AgentId,
        /// Human-readable error message.
        error: String,
    },
    /// Agent was cancelled by the host.
    Killed {
        /// Agent that was killed.
        agent_id: AgentId,
    },
    /// A raw message produced by the agent (assistant or tool result).
    Message {
        /// Agent that produced the message.
        agent_id: AgentId,
        /// Free-form message payload (full schema lands in Plan 09+).
        message: serde_json::Value,
    },
}

/// Subagent state-machine loop.
///
/// Drives [`lingxi_core::reduce`] over `event_rx` and emits
/// [`SubagentEvent`]s on `out_tx`. M1.11 stubs completion after the first
/// event so the pool can be wired end-to-end before the full agentic loop
/// arrives in Plan 09+.
pub async fn run_subagent(
    ctx: SubagentContext,
    mut event_rx: mpsc::Receiver<lingxi_core::Event>,
    out_tx: mpsc::Sender<SubagentEvent>,
) {
    let _ = event_rx.recv().await;
    let _ = out_tx
        .send(SubagentEvent::Completed {
            agent_id: ctx.agent_id,
            result: serde_json::json!({"stub": true}),
        })
        .await;
}
