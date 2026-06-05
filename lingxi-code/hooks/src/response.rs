//! Hook response + result types (spec §9.4).
//!
//! A hook executor returns a [`HookResult`] capturing the raw process /
//! transport outcome plus the JSON [`HookResponse`] parsed from the hook's
//! reply. The executor merges all matching hooks' responses into a single
//! [`AggregateHookResult`] consumed by the calling subsystem.

use protocol::HookId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Structured response a hook may return to influence the in-flight action.
///
/// All fields are optional. A hook may simply observe (returning the default
/// response) or actively intervene by setting `decision`, `updated_input`,
/// `system_message`, or `attachments`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookResponse {
    /// Engine action the hook recommends (block / allow / approve / continue).
    pub decision: Option<HookDecision>,
    /// Human-readable reason for the decision (rendered to the user on Block).
    pub reason: Option<String>,
    /// Replacement input for the in-flight action (e.g. mutated tool input).
    pub updated_input: Option<Value>,
    /// Free-form system message to splice into the agent's context.
    pub system_message: Option<String>,
    /// Additional content blocks (images, files, etc.) to attach.
    pub attachments: Vec<Value>,
    /// If `true` the engine should suppress the default user-visible output
    /// for the in-flight action even when not blocked.
    #[serde(default)]
    pub suppress_output: bool,
    /// `true` when the hook returned `continue: false` (claude-code
    /// `hooks.ts:404`). For lifecycle hooks (`Stop` / `SubagentStop` /
    /// `TaskCompleted`) this is the *preventContinuation* signal: the agent
    /// loop must terminate rather than keep working, regardless of any
    /// `decision: block` also present (B4 — `query.ts:1278`). Distinct from a
    /// bare `Block` decision (exit-2 / `decision:block` without `continue:false`),
    /// which for a Stop hook means "keep working" (`query.ts:1282`). Additive /
    /// `..Default::default()`-compatible; defaults to `false` (the prior
    /// behavior where `continue:false` only copied `stopReason` into `reason`).
    #[serde(default)]
    pub prevent_continuation: bool,
    /// Caller-defined structured payload — for hooks that need to return
    /// metadata not covered by the canonical fields above.
    pub structured_content: Option<Value>,
}

/// The action a hook recommends after observing an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookDecision {
    /// Allow the action to proceed. Equivalent to no decision.
    Allow,
    /// Approve a permission-gated action without prompting the user.
    Approve,
    /// Block the in-flight action. The engine short-circuits remaining hooks
    /// and surfaces `reason` to the user.
    Block,
    /// Explicitly continue — useful as a tie-breaker when later hooks may
    /// otherwise block.
    Continue,
}

/// Raw outcome of a single hook invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    /// Coarse status (`Success`, `Error`, `Cancelled`, `Timeout`).
    pub outcome: HookOutcome,
    /// Captured stdout (or transport body).
    pub stdout: String,
    /// Captured stderr (or transport diagnostic).
    pub stderr: String,
    /// Process exit code (for `Command` hooks); `None` for non-process
    /// executors.
    pub exit_code: Option<i32>,
    /// Parsed [`HookResponse`] if the hook returned valid JSON.
    pub response: Option<HookResponse>,
}

/// Coarse status of a single hook invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookOutcome {
    /// Hook executed successfully (status 0 / 2xx / `Ok`).
    Success,
    /// Hook executed but reported failure (non-zero exit, non-2xx, etc.).
    Error,
    /// Caller cancelled the hook before it finished.
    Cancelled,
    /// Hook exceeded its configured timeout.
    Timeout,
}

/// Merged result of all hooks fired for one event. The executor folds each
/// individual [`HookResult`] into this aggregate in priority order; the first
/// `Block` decision wins and stops further folding.
#[derive(Debug, Clone, Default)]
pub struct AggregateHookResult {
    /// Final aggregated decision (latest non-`None` wins until a `Block`).
    pub decision: Option<HookDecision>,
    /// Reason associated with the final decision.
    pub reason: Option<String>,
    /// Most recent `updated_input` if any hook mutated the action's input.
    pub modified_input: Option<Value>,
    /// All system messages emitted by hooks, in execution order.
    pub system_messages: Vec<String>,
    /// `true` when ANY folded hook requested *preventContinuation*
    /// (`continue: false`). For lifecycle (`Stop`) hooks this signals the
    /// turn loop to TERMINATE the agent rather than continue working — it
    /// takes precedence over a `Block` decision (B4 — `query.ts:1278`).
    /// OR-folded across hooks in [`crate::HookExecutorImpl::execute`]'s
    /// merge step; defaults to `false`, so a registry with no lifecycle
    /// hook leaves it untouched (behavior-neutral for existing callers).
    pub prevent_continuation: bool,
    /// All attachments produced by hooks, in execution order.
    pub attachments: Vec<Value>,
    /// Per-hook results, in execution order — for telemetry and debugging.
    pub all_results: Vec<(HookId, HookResult)>,
}
