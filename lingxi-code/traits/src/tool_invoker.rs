//! `ToolInvoker` — narrow trait used by the M4-05 `AgentTool` to dispatch
//! sub-tool calls into a recursive subagent without taking a direct dep on
//! `lingxi-tools` (which would form a cycle).
//!
//! The recursion-lock invariant (a child subagent reuses the same
//! `Arc<ToolRegistry>` as its parent) is asserted in `lingxi-tools` tests via
//! `Arc::ptr_eq` on the trait-object passed through this surface.
//!
//! See M4-05 wiring follow-up plan.

use async_trait::async_trait;
use protocol::AgentId;
use serde_json::Value;
use std::any::Any;
use thiserror::Error;

/// Per-call invocation context handed to a [`ToolInvoker`].
///
/// Carries the identity of the parent agent (used by the recursion-lock test
/// to verify the child inherits the parent's registry / budget) plus any
/// metadata the trait abstraction needs to expose without leaking the
/// concrete `ToolUseContext` from `lingxi-tools`.
#[derive(Debug, Clone)]
pub struct SubagentInvocationContext {
    /// Parent agent id (the agent that is dispatching the child).
    pub parent_agent_id: Option<AgentId>,
    /// DISPLAY NAME of the teammate dispatching this tool call, if known
    /// (claude-code `getAgentName()` — the teammate's human name, e.g.
    /// `"researcher"`, NOT the `agent:<uuid>` form). `None` for the main
    /// thread / leader. Mapped straight into
    /// `ToolUseContext.agent_name` so the swarm-only `TaskUpdate` side-effects
    /// (auto-owner, owner-change mailbox notification) key on the name.
    pub agent_name: Option<String>,
    /// TEAM NAME the dispatching teammate belongs to, if known (claude-code
    /// `getTeammateContext()?.teamName`). Mapped into `ToolUseContext.team_name`
    /// so `getTaskListId()` resolves an in-process teammate to the leader's
    /// on-disk task directory. `None` for the main thread / standalone session.
    pub team_name: Option<String>,
    /// Whether the dispatching subagent runs ASYNC (backgrounded). claude-code's
    /// `runAgent` sets the child tools' `isNonInteractiveSession: true` for an
    /// async agent (else it inherits the parent's flag, default `false`) —
    /// `runAgent.ts:668-672`. Mapped into `ToolUseContext.is_non_interactive_session`.
    pub is_async: bool,
    /// Whether the dispatching subagent may SURFACE permission prompts to the
    /// user (claude-code's permission-prompt eligibility). Threaded from
    /// `SubagentContext.can_show_permission_prompts`. When `true` (a named
    /// in-process teammate), a tool call that needs permission surfaces in the
    /// main session ATTRIBUTED to this worker (the `● @name` badge); when `false`
    /// the worker is not presented as a permission-prompt origin. Consulted by
    /// the dispatch invoker to decide whether to attach a
    /// [`crate::permission_gate::PromptWorker`] to the gate check.
    pub can_show_permission_prompts: bool,
    /// Per-agent working directory OVERRIDE — `Some` when the dispatching
    /// subagent is isolated in a git worktree (`isolation:"worktree"`) or was
    /// given an explicit `cwd`. The invoker maps it into `ToolUseContext.cwd` so
    /// the agent's filesystem + shell tools operate in that directory instead of
    /// the shared session workspace (claude-code's per-agent `agentWorktree`/cwd
    /// `AsyncLocalStorage`). `None` for the main thread / a non-isolated agent.
    pub cwd: Option<std::path::PathBuf>,
}

/// Failure modes for [`ToolInvoker::invoke`].
#[derive(Debug, Error)]
pub enum ToolInvokerError {
    /// The named tool was not found in the registry.
    #[error("ToolInvoker: tool '{0}' not found")]
    NotFound(String),
    /// The tool surfaced an invalid input.
    #[error("ToolInvoker: invalid input: {0}")]
    InvalidInput(String),
    /// Any other internal failure.
    #[error("ToolInvoker: internal error: {0}")]
    Internal(String),
}

impl ToolInvokerError {
    /// The bare model-facing message — claude's `formatError(error)` =
    /// `error.message`, WITHOUT the LingXi-internal `ToolInvoker: …` `Display`
    /// prefix. Used as the subagent tool_result content so the child model
    /// never sees an `invalid input: `/`internal error: ` variant prefix
    /// (which is `Display`-only, for logging). Mirrors
    /// [`tool_api::ToolError::model_facing_message`]. `NotFound` carries only
    /// the tool name, so it is rendered into a full message here.
    #[must_use]
    pub fn model_facing_message(&self) -> String {
        match self {
            Self::NotFound(name) => format!("tool '{name}' not found"),
            Self::InvalidInput(s) | Self::Internal(s) => s.clone(),
        }
    }
}

/// Tool invocation seam used by `AgentTool` to recurse into the registry.
///
/// Concrete impls live in `lingxi-tools` (production wrapper around
/// `ToolRegistry`) and in test fixtures (recording mock).
#[async_trait]
pub trait ToolInvoker: Send + Sync + Any {
    /// Invoke the tool named `name` with the supplied JSON `input`.
    async fn invoke(
        &self,
        name: &str,
        input: Value,
        ctx: SubagentInvocationContext,
    ) -> Result<Value, ToolInvokerError>;

    /// Cast to `&dyn Any` for downcast-based test introspection.
    /// Default impl works for all `Sized + 'static` implementors.
    fn as_any(&self) -> &dyn Any;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn ToolInvoker>> = None;
    }
}
