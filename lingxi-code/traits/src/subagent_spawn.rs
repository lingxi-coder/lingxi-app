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
    /// The subagent type to resolve (built-in or user/project catalog). NOT
    /// validated here — the spawner resolves it with claude-code precedence
    /// (catalog overrides built-ins; unknown → `general-purpose`).
    pub subagent_type: String,
    /// Initial prompt seeded into the subagent's first turn.
    pub prompt: String,
    /// Optional context-path files injected as system-tagged messages.
    #[serde(default)]
    pub context_paths: Vec<PathBuf>,
    // ===== AgentTool spawn-surface parity (coordinator batch D2a) =====
    // Additive optional fields mirroring claude-code's `AgentTool` schema
    // (`AgentTool.tsx:82-101`). All `#[serde(default)]`/`Option` so existing
    // call sites and serialized payloads remain valid (frozen-crate rule).
    /// Short (3-5 word) human description of the task (TS `description`,
    /// required in the model-facing schema). Carried for telemetry / display.
    #[serde(default)]
    pub description: Option<String>,
    /// Model-family override (`"sonnet"` | `"opus"` | `"haiku"`). Takes
    /// precedence over the resolved [`crate::…`] agent definition's model
    /// (TS `model`). The spawner maps this onto the agent model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Name for the spawned agent, making it addressable via `SendMessage`
    /// (TS `name`). Carried through; teammate routing is deferred.
    #[serde(default)]
    pub name: Option<String>,
    /// Team name for spawning (TS `team_name`). Carried through; teammate
    /// routing is deferred.
    #[serde(default)]
    pub team_name: Option<String>,
    /// Permission mode for a spawned teammate (TS `mode`, e.g. `"plan"`).
    /// Carried through; permission-mode application is deferred.
    #[serde(default)]
    pub mode: Option<String>,
    /// Isolation mode (`"worktree"` | `"remote"`, TS `isolation`). Carried
    /// through; worktree/remote isolation behavior is deferred.
    #[serde(default)]
    pub isolation: Option<String>,
    /// Absolute path to run the agent in (TS `cwd`). Carried through; the
    /// cwd-override behavior is deferred.
    #[serde(default)]
    pub cwd: Option<String>,
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

/// One resolved subagent type, surfaced to `AgentTool` so it can build the
/// dynamic tool prompt (claude-code's `formatAgentLine`,
/// `AgentTool/prompt.ts:43-46`: `- {agentType}: {whenToUse} (Tools: …)`).
///
/// Lives in `traits` (a leaf crate) so `tool-agent` — which must NOT depend on
/// the `agent` engine crate — can render the catalog without a cyclic dep. The
/// concrete [`SubagentSpawner`] impl in `agent` populates it from the resolved
/// built-in + user/project [`AgentDefinition`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentListingEntry {
    /// Agent type label (TS `agentType`).
    pub agent_type: String,
    /// "When to use" guidance (TS `whenToUse`).
    pub when_to_use: String,
    /// Pre-rendered tools description (TS `getToolsDescription`): `All tools`,
    /// `All tools except X, Y`, an explicit `A, B, C`, or `None`.
    pub tools_description: String,
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

    /// The resolved subagent catalog (built-ins + any wired user/project
    /// agents), used by `AgentTool` to render its dynamic tool prompt.
    ///
    /// Defaulted to empty so existing impls/tests need no change (frozen-crate
    /// rule). The production [`SubagentSpawner`] overrides this to surface the
    /// real catalog with claude-code's later-wins precedence.
    async fn agent_listing(&self) -> Vec<SubagentListingEntry> {
        Vec::new()
    }
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
