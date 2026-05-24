//! `SubagentSpawner` — narrow trait abstracting `StateMachinePool::allocate`
//! plus the loop until the spawned subagent reports a terminal event.
//!
//! Concrete impl lives in `lingxi-agent` (production adapter over
//! `StateMachinePool` + `ToolRegistry` + `BudgetEnforcer`). Tests inject a
//! recording mock that captures the parent registry / budget Arcs so the
//! `Arc::ptr_eq` recursion-lock + budget-inheritance assertions can fire.
//!
//! See M4-05 wiring follow-up plan.

use crate::budget::BudgetEnforcerHandle;
use crate::tool_invoker::ToolInvoker;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Locked subagent input passed to [`SubagentSpawner::spawn`].
///
/// Mirrors `AgentToolInput` in `lingxi-tools::builtin::agent` byte-for-byte
/// so the trait surface stays insulated from `lingxi-tools`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentSpawnRequest {
    /// One of the 6 built-in subagent types.
    pub subagent_type: String,
    /// Initial prompt seeded into the subagent's first turn.
    pub prompt: String,
    /// Optional context-path files injected as system-tagged messages.
    #[serde(default)]
    pub context_paths: Vec<PathBuf>,
}

/// Token-usage rollup returned at the end of a successful spawn.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentUsage {
    /// Total token count consumed by the subagent's turn(s).
    pub total_tokens: u64,
}

/// Terminal result of one [`SubagentSpawner::spawn`] call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubagentResult {
    /// The subagent finished normally.
    Completed {
        /// Free-form JSON payload returned by the subagent.
        content: Value,
        /// Token-usage rollup.
        usage: SubagentUsage,
    },
    /// The subagent terminated with an error.
    Failed {
        /// Human-readable reason.
        reason: String,
    },
    /// The subagent was cancelled by the host.
    Killed,
}

/// Failure modes for [`SubagentSpawner::spawn`].
#[derive(Debug, Error)]
pub enum SubagentSpawnError {
    /// The agent pool is at capacity.
    #[error("SubagentSpawner: pool full")]
    PoolFull,
    /// The runtime rejected the spawn.
    #[error("SubagentSpawner: runtime error: {0}")]
    Runtime(String),
    /// Any other internal failure.
    #[error("SubagentSpawner: internal error: {0}")]
    Internal(String),
}

/// Inheritance bundle the parent agent hands to a child spawn.
///
/// The recursion-lock + budget-inheritance invariants are asserted in
/// `lingxi-tools` tests via `Arc::ptr_eq` on these trait-object pointers.
#[derive(Clone)]
pub struct SubagentInheritance {
    /// Parent's tool invoker (`Arc<ToolRegistry>` wrapped). The recursion
    /// lock requires the child to reuse this exact `Arc`, NOT a fresh one.
    pub tool_invoker: Arc<dyn ToolInvoker>,
    /// Parent's budget enforcer. The child inherits this `Arc` so budget
    /// charges aggregate across the whole agent tree.
    pub budget: Arc<dyn BudgetEnforcerHandle>,
}

/// Spawn-a-subagent seam used by `AgentTool`.
#[async_trait]
pub trait SubagentSpawner: Send + Sync {
    /// Allocate a subagent slot, pump its state machine to completion, and
    /// return the terminal [`SubagentResult`].
    ///
    /// `inherit` carries the parent's tool invoker (registry recursion lock)
    /// and budget enforcer; the implementation MUST hand the same `Arc`s on
    /// to the child without cloning the inner value.
    async fn spawn(
        &self,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
    ) -> Result<SubagentResult, SubagentSpawnError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn SubagentSpawner>> = None;
    }
}
