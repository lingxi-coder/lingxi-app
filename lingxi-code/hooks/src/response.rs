//! Hook response + result types (spec §9.4).
//!
//! A hook executor returns a [`HookResult`] capturing the raw process /
//! transport outcome plus the JSON [`HookResponse`] parsed from the hook's
//! reply. The executor merges all matching hooks' responses into a single
//! [`AggregateHookResult`] consumed by the calling subsystem.

use crate::events::HookProgressEvent;
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
    /// Free-form `systemMessage` the hook returned (claude-code
    /// `result.systemMessage`). This is **user/transcript-facing only** — it is
    /// NOT sent to the model: claude-code routes it to a `hook_system_message`
    /// attachment whose `normalizeAttachmentForAPI` returns `[]`
    /// (`utils/messages.ts:4258`). Kept DISTINCT from [`Self::additional_context`]
    /// (which IS model-facing); the two must never be merged.
    pub system_message: Option<String>,
    /// `hookSpecificOutput.additionalContext` the hook returned (claude-code
    /// `result.additionalContext`). This IS model-facing: claude-code yields it
    /// as a `hook_additional_context` attachment whose `normalizeAttachmentForAPI`
    /// returns a `<system-reminder>` user message that reaches the model
    /// (`utils/messages.ts:4117`). DISTINCT from [`Self::system_message`].
    pub additional_context: Option<String>,
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
    /// Replacement tool output a `PostToolUse` hook returned via
    /// `hookSpecificOutput.updatedMCPToolOutput` (claude-code
    /// `parseHookJSONOutput`, `utils/hooks.ts:646-649`). `Some` only when a
    /// `PostToolUse` hook supplied a replacement output. The orchestrator
    /// substitutes it for the tool's result — but ONLY for MCP tools, mirroring
    /// TS's `isMcpTool(tool)` gate (`toolHooks.ts:146` / `toolExecution.ts:1494`).
    /// Additive default `None`, so non-`PostToolUse` hooks (and `PostToolUse`
    /// hooks that don't set it) leave the result untouched.
    pub updated_mcp_tool_output: Option<Value>,
    /// Elicitation answer a hook provided via
    /// `hookSpecificOutput.{action,content}` (claude-code
    /// `parseElicitationHookOutput`, `utils/hooks.ts:4434-4446` /
    /// `hooks.ts:674-688`). `Some` only for `Elicitation` / `ElicitationResult`
    /// hooks that returned an `action`. The MCP elicitation handler consumes
    /// this to PROVIDE the elicitation response; a `decline` action also
    /// drives a `Block` decision. Additive default `None`.
    pub elicitation_response: Option<ElicitationHookResponse>,
}

/// Structured elicitation answer a hook can return, mirroring claude-code's
/// `ElicitationResponse` (`{ action, content? }`). 1:1 with the
/// `hookSpecificOutput.action` / `hookSpecificOutput.content` fields parsed by
/// `parseElicitationHookOutput`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElicitationHookResponse {
    /// One of `"accept"` / `"decline"` / `"cancel"` — forwarded verbatim so the
    /// MCP handler can return it as the elicitation `action`. Kept as a raw
    /// `String` to avoid coupling the hooks crate to the MCP action enum.
    pub action: String,
    /// Optional structured form content (only meaningful for `accept`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
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
    /// All `systemMessage`s emitted by hooks, in execution order. These are
    /// **user/transcript-facing only** and must NOT reach the model (claude-code
    /// `hook_system_message` → `normalizeAttachmentForAPI` returns `[]`,
    /// `utils/messages.ts:4258`). Kept DISTINCT from [`Self::additional_contexts`].
    pub system_messages: Vec<String>,
    /// All `additionalContext`s emitted by hooks, in execution order. These ARE
    /// model-facing: claude-code surfaces them via `hook_additional_context` as a
    /// `<system-reminder>` user message (`utils/messages.ts:4117`). The turn loop
    /// builds the PreToolUse model-facing context message from THIS field only —
    /// never from [`Self::system_messages`].
    pub additional_contexts: Vec<String>,
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
    /// One `hook_progress` event per matching hook, in execution order,
    /// emitted *before* each hook runs (claude-code `utils/hooks.ts:2094-2116`).
    /// Carries the per-hook `status_message` so the spinner can substitute it
    /// for the generic running line. Additive / `..Default::default()`-compatible;
    /// defaults to empty, so existing callers that ignore it are unaffected.
    pub progress: Vec<HookProgressEvent>,
    /// The last elicitation answer any folded hook provided (claude-code
    /// `executeElicitationHooks`: the loop keeps the latest non-empty
    /// `elicitationResponse`). `Some` only for `Elicitation` /
    /// `ElicitationResult` dispatches where a hook set
    /// `hookSpecificOutput.action`. The MCP elicitation handler reads this to
    /// PROVIDE the response. Additive default `None`, so non-elicitation
    /// callers are unaffected.
    pub elicitation_response: Option<ElicitationHookResponse>,
    /// The last `updatedMCPToolOutput` any folded `PostToolUse` hook returned
    /// (claude-code keeps the most recent — `result.updatedMCPToolOutput =
    /// json.hookSpecificOutput.updatedMCPToolOutput`, `utils/hooks.ts:647`). The
    /// orchestrator substitutes it for the tool's result, but ONLY for MCP tools
    /// (`isMcpTool(tool)`, `toolHooks.ts:146`). Additive default `None`, so a
    /// dispatch with no mutating `PostToolUse` hook leaves the result unchanged
    /// (byte-identical).
    pub updated_mcp_tool_output: Option<Value>,
}
