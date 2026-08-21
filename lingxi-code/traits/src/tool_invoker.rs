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
    /// Effective owning-session mode for this dispatch. This is distinct from
    /// `is_async`: a synchronous child of scheduled/headless work must still
    /// keep every invoked tool non-interactive.
    pub is_non_interactive_session: bool,
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
    /// The assistant message's `tool_use` block id this dispatch is for — the
    /// REAL id a stdio `can_use_tool` request should carry (claude-code
    /// `createCanUseTool(toolUseID)`), so the host can correlate + dedup the
    /// subagent's permission prompt. 1:1 with
    /// [`crate::permission_gate::PermissionCheckContext::tool_use_id`]: the
    /// dispatch invoker maps it straight into the gate check context so the
    /// subagent path is byte-faithful to the main loop's. `None` for a dispatch
    /// site that has no originating block id (test fixtures / legacy callers),
    /// in which case the gate mints a fresh id exactly as before.
    pub tool_use_id: Option<String>,
    /// The DISPATCHING agent's recursion depth (claude `agentContext.depth`).
    /// The dispatch invoker maps it into `ToolUseContext.depth`, so a recursive
    /// `Agent` call inside the dispatched tool computes the child's depth and the
    /// subagent tool-resolver can apply the configured spawn-depth cap. `0` for the main
    /// thread / a top-level dispatch (and every legacy/test call site).
    pub depth: u32,
    /// Observer declaration inherited from the dispatching agent. Recursive
    /// Agent calls use it only when the selected child has no declaration of
    /// its own; `observe_subagents:false` and the depth cap stop propagation.
    pub observer: Option<crate::subagent_spawn::ObserverSpec>,
    /// The DISPATCHING subagent's OWN resolved main-loop model (claude-code
    /// `runAgent.ts:678` seeds each child's `mainLoopModel: resolvedAgentModel`,
    /// so a NESTED `Agent` call inside a subagent resolves its child's model
    /// against the IMMEDIATE parent's resolved model, not the top-level main-loop
    /// model). The dispatch invoker maps it into `ToolUseContext.options.main_loop_model`
    /// so a recursive `Agent` tool call reads the parent's model (claude
    /// `AgentTool.tsx:418` `toolUseContext.options.mainLoopModel`). `None` for the
    /// main thread / legacy call sites (⇒ the invoker keeps its placeholder model).
    pub parent_model: Option<String>,
    /// Provider profile paired with [`Self::parent_model`]. Nested Agent calls
    /// must inherit both values; carrying only the wire id is ambiguous when
    /// multiple configured providers expose the same model.
    pub parent_model_profile: Option<String>,
    /// The dispatching subagent's EFFECTIVE permission mode as a WIRE string
    /// (claude-code 2.1.207 Agent `mode` → the child's
    /// `toolPermissionContext.mode`, `wKe`/`ve`). `Some("plan")` ⇒ the dispatch
    /// permission gate authorizes THIS call under that mode (a `mode:"plan"` child
    /// gates mutations — `Edit`/`Write`/`Bash` — while reads stay frictionless);
    /// `None` ⇒ the gate uses its live/boot mode (the main thread / a spawn with no
    /// mode override — byte-identical to before). Mapped straight into
    /// [`crate::permission_gate::PermissionCheckContext::mode_override`].
    pub mode_override: Option<String>,
    /// Command-deny rules FROZEN when a background fork launched, replayed for
    /// every tool call this subagent makes (claude `freezeCommandDenies`).
    ///
    /// Upstream rebuilds the permission context from LIVE app state on resume,
    /// so a settings edit made while a fork was parked could REMOVE a deny that
    /// was in force when it launched. These rules are re-applied as a
    /// `disallowed_tools` permission LAYER, which the fold applies ON TOP of the
    /// base policy — so a frozen deny WINS over a live rule that would now allow
    /// the same command.
    ///
    /// The port is exposed in BOTH directions, so this is not a cross-session-only
    /// concern: `PolicyPermissionGate::apply_permission_update` mutates the gate's
    /// `live_state` deny rules in-process (`addRules` / `replaceRules` /
    /// `removeRules`), so a host permission update can remove an in-force deny
    /// while a fork is parked in the SAME process. The scoping record additionally
    /// outlives the process, and a cross-session resume reads it against a freshly
    /// loaded policy.
    ///
    /// Note the converse gap, which this field does not close: the snapshot is
    /// taken from the BOOT policy (`PermissionPolicy::deny_rules`), not from the
    /// gate's `live_state`, so a deny ADDED at runtime is never frozen.
    ///
    /// Empty ⇒ no layer is added and the fold is byte-identical to before.
    pub frozen_command_denies: Vec<String>,
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
    /// Permission policy terminated the owning prompt-avoiding agent.
    ///
    /// Unlike an ordinary denial, callers must not convert this into a
    /// recoverable `tool_result` and continue the model loop.
    #[error("{0}")]
    Abort(String),
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
            Self::InvalidInput(s) | Self::Abort(s) | Self::Internal(s) => s.clone(),
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

    /// Invoke with an ephemeral workspace lease. Implementations that do not
    /// participate in lease-aware permission enforcement retain the legacy
    /// behavior by delegating to [`Self::invoke`].
    async fn invoke_with_workspace_lease(
        &self,
        name: &str,
        input: Value,
        ctx: SubagentInvocationContext,
        _workspace_lease_token: Option<u64>,
    ) -> Result<Value, ToolInvokerError> {
        self.invoke(name, input, ctx).await
    }

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
