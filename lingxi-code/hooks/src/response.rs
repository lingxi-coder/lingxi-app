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
    /// Replacement tool output a `PostToolUse` hook returned via
    /// `hookSpecificOutput.updatedToolOutput` (claude-code `parseHookJSONOutput`,
    /// BIN off 205724076: `if(e.hookSpecificOutput.updatedToolOutput!==void 0)
    /// u.updatedToolOutput=e.hookSpecificOutput.updatedToolOutput`). Unlike the
    /// legacy [`Self::updated_mcp_tool_output`] (truthiness-gated, MCP-only),
    /// this field uses `!== void 0` semantics — an explicit JSON `null` IS a
    /// replacement — and the orchestrator substitutes it for the result of ALL
    /// tools with NO `isMcpTool` gate (BIN off 202169384:
    /// `if("updatedToolOutput"in D&&e.outputSchema?.safeParse(D.updatedToolOutput)
    /// ?.success!==!1)x.data=D.updatedToolOutput`). The schema describe string is
    /// `Replaces the tool output before it is sent to the model`. To preserve the
    /// `!== void 0` semantics we model it as an `Option<Option<Value>>`: outer
    /// `None` = key absent (no replacement); `Some(inner)` = key present where
    /// `inner` is the replacement value (`Some(Value::Null)` for an explicit
    /// `null`). Additive default `None`, so the common case (no `PostToolUse`
    /// hook, or a hook that omits the field) leaves the result untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_tool_output: Option<Option<Value>>,
    /// Elicitation answer a hook provided via
    /// `hookSpecificOutput.{action,content}` (claude-code
    /// `parseElicitationHookOutput`, `utils/hooks.ts:4434-4446` /
    /// `hooks.ts:674-688`). `Some` only for `Elicitation` / `ElicitationResult`
    /// hooks that returned an `action`. The MCP elicitation handler consumes
    /// this to PROVIDE the elicitation response; a `decline` action also
    /// drives a `Block` decision. Additive default `None`.
    pub elicitation_response: Option<ElicitationHookResponse>,
    /// `hookSpecificOutput.retry` a `PermissionDenied` hook returned (claude-code
    /// `parseHookJSONOutput`, `case 'PermissionDenied': result.retry =
    /// json.hookSpecificOutput.retry`, `utils/hooks.ts:654-655`). `Some(true)`
    /// signals the auto-mode classifier deny is now approved and the model may
    /// retry — the turn loop then pushes the verbatim `isMeta` retry message
    /// (`toolExecution.ts:1092-1099`). `None` for every other hook (and every
    /// `PermissionDenied` hook that omits it). DORMANT in the public build: the
    /// retry message is gated behind the `TRANSCRIPT_CLASSIFIER` feature
    /// (off externally) AND a classifier-source deny, so a hook setting this
    /// has no effect unless both hold — faithful to claude-code.
    #[serde(default)]
    pub retry: Option<bool>,
    /// Top-level `terminalSequence` a hook returned (#40, claude-code schema
    /// BIN off 200873127). A hook may ask LingXi to emit a terminal escape
    /// sequence (e.g. an OSC 9 / OSC 777 desktop notification). This is a
    /// TOP-LEVEL field (NOT under `hookSpecificOutput`) and applies for ALL hook
    /// result types (command/stdout, HTTP, mcp_tool, callback). The apply path
    /// (`szn`, BIN off 205755390) runs the [`crate::terminal_seq`] allowlist
    /// validator (`NEo`) — accepting only OSC ps in {0,1,2,9,99,777} plus BEL —
    /// and either writes the validated sequence to the active terminal or warns
    /// and drops it. Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_sequence: Option<String>,
    /// `hookSpecificOutput.sessionTitle` returned by a `UserPromptSubmit` hook
    /// (binary-confirmed at BIN off 201754804:
    /// `{hookEventName:"UserPromptSubmit", additionalContext?:string,
    /// sessionTitle?:string, suppressOriginalPrompt?:boolean}`).
    /// Allows a hook to rename the session. Additive default `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_title: Option<String>,
    /// `hookSpecificOutput.suppressOriginalPrompt` returned by a `UserPromptSubmit`
    /// hook (binary-confirmed at BIN off 201754804; description: "When decision is
    /// 'block', omit the original prompt from the block message"). Additive default
    /// `false`. Applied by the orchestrator's `fire_user_prompt_submit` block-message
    /// render (P2-04): on a `Block`, the warning collapses to bare `${reason}` when
    /// this is `true` instead of appending `\n\nOriginal prompt: ${prompt}`.
    #[serde(default)]
    pub suppress_original_prompt: bool,
    /// `hookSpecificOutput.displayContent` returned by a `MessageDisplay` hook
    /// (binary-confirmed at BIN off 201757586; description: "Text displayed in
    /// place of the delta. Omit (or return the delta unchanged) to display the
    /// original."). Additive default `None`. Applied by the orchestrator's
    /// completed-message `MessageDisplay` pass (P2-04): when `Some`, the joined
    /// assistant text is rendered ON SCREEN as this value while the stored
    /// message / JSONL keep the original (claude-code's `displayedMessageContent`,
    /// `Qff`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_content: Option<String>,
    /// `hookSpecificOutput.watchPaths` returned by a `FileChanged` / `CwdChanged`
    /// hook (claude-code `parseHookJSONOutput`: `"watchPaths" in
    /// e.hookSpecificOutput && e.hookSpecificOutput.watchPaths`). A hook may add
    /// paths to the file-changed watch set; the desktop watcher restarts over
    /// the union when the folded set is non-empty (`fileChangedWatcher.ts`'s
    /// `if (v.length > 0) updateWatchPaths(v)`), and the `CwdChanged` flow
    /// re-resolves them against the new cwd. Present-key capture: an empty array
    /// is still `Some(vec![])` (JS arrays are always truthy), distinguishing it
    /// from an absent key (`None`); a non-array value is ignored. Additive
    /// default `None`, so hooks that omit it leave the watch set untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_paths: Option<Vec<String>>,
    /// `hookSpecificOutput.initialUserMessage` returned by a `SessionStart` hook
    /// (claude-code schema: `initialUserMessage:S.string().optional()`; consumed
    /// as `if(p.initialUserMessage)$os=p.initialUserMessage` — the pending initial
    /// user prompt). Scoped to `SessionStart`. Additive default `None`; the
    /// orchestrator injects it as a (non-meta) user message at session start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_user_message: Option<String>,
    /// `hookSpecificOutput.reloadSkills` returned by a `SessionStart` hook
    /// (claude-code schema: `reloadSkills:S.boolean().describe("Re-scan skill and
    /// command directories")`; consumed as `if(p.reloadSkills)u=!0`). Scoped to
    /// `SessionStart`. Additive default `None`. NOTE: the port loads skills/
    /// commands once at boot and has no hot-reload seam, so this is parsed +
    /// folded (no longer silently dropped) but its re-scan ACTION is a documented
    /// follow-up — honoring it requires a skill/command registry reload path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload_skills: Option<bool>,
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
    /// Ask the user (`permissionDecision: "ask"`, claude-code BIN off
    /// ~205721920: `case"ask":u.permissionBehavior="ask"`; schema enum
    /// `["allow","deny","ask","defer"]`). A `PreToolUse` hook may force the
    /// tool call through the INTERACTIVE permission prompt regardless of any
    /// configured allow rule — `permissionBehavior="ask"` makes claude-code
    /// prompt the user even when a rule would otherwise auto-allow. Distinct
    /// from `Approve`/`Allow` (which SKIP the prompt) and `Block` (which
    /// denies outright). The turn-loop consumer must route this through the
    /// normal ask path (`PermissionGate::check`, which delegates to the prompt
    /// transport) rather than `check_after_hook_allow`. Unconditionally
    /// reassigned by the `hookSpecificOutput.permissionDecision` switch, like
    /// every other case (matching `azn`'s second switch).
    Ask,
    /// Defer the tool call (#37, `permissionDecision: "defer"`, claude-code BIN
    /// off 205722868: `case"defer":u.permissionBehavior="defer"`; schema off
    /// 205719537: `'"allow" | "deny" | "ask" | "defer" (optional)'`). A
    /// `PreToolUse` hook may DEFER a solo tool call so it is re-attempted on a
    /// later (interactive) resume rather than run now. The orchestrator gates
    /// this on PRINT/non-interactive mode AND a single tool_use block in the
    /// batch (BIN off 202454844): in interactive mode or a multi-tool batch it
    /// warns and ignores (proceeds normally); on the gated path it emits the
    /// `tengu_pre_tool_hook_deferred` analytic, pushes a `hook_deferred_tool`
    /// meta message, and TERMINATES the turn with the `tool_deferred`
    /// stop-reason (the tool is NOT executed). DORMANT on the default
    /// interactive REPL path (the interactive-mode gate ignores it there).
    Defer,
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
    /// The last `updatedToolOutput` any folded `PostToolUse` hook returned
    /// (claude-code keeps the most recent — BIN off 205724076). Unlike
    /// [`Self::updated_mcp_tool_output`] this applies to ALL tools (no
    /// `isMcpTool` gate) and uses `!== void 0` semantics (an explicit JSON
    /// `null` IS a replacement), modeled as `Option<Option<Value>>`: outer
    /// `None` = no hook set it; `Some(inner)` = a hook set it (`inner` is the
    /// replacement, `Some(Value::Null)` for explicit `null`). The orchestrator
    /// validates it against the tool's output schema and substitutes for the
    /// result (BIN off 202169384). Additive default `None` → byte-identical when
    /// no hook mutates the output.
    pub updated_tool_output: Option<Option<Value>>,
    /// `true` when ANY folded `PermissionDenied` hook returned
    /// `hookSpecificOutput.retry: true` (claude-code `toolExecution.ts:1090`,
    /// `if (result.retry) hookSaysRetry = true`). The turn loop reads this — only
    /// on the gated classifier-deny path — to push the verbatim `isMeta` retry
    /// message. OR-folded across hooks; defaults to `false`, so a registry with
    /// no retrying `PermissionDenied` hook leaves it untouched (behavior-neutral
    /// for existing callers). DORMANT in the public build (see [`HookResponse::retry`]).
    pub retry: bool,
    /// The last `terminalSequence` any folded hook returned (#40, claude-code
    /// keeps the most recent — `szn` is invoked per hook result). `Some` only
    /// when a hook supplied a `terminalSequence`. A consumer (the TUI terminal
    /// writer) validates it via [`crate::terminal_seq::validate_terminal_sequence`]
    /// and either emits the accepted sequence to the active terminal or warns +
    /// drops it. Additive default `None` → byte-identical when no hook sets it.
    pub terminal_sequence: Option<String>,
    /// The last `sessionTitle` any folded `UserPromptSubmit` hook returned
    /// (binary-confirmed at BIN off 201754804). Allows a hook to rename the
    /// session at prompt-submit time. `None` when no hook set it. The orchestrator
    /// applies this by triggering the session title update path.
    pub session_title: Option<String>,
    /// `true` when ANY folded `UserPromptSubmit` hook returned
    /// `suppressOriginalPrompt: true` (binary-confirmed at BIN off 201754804;
    /// description: "When decision is 'block', omit the original prompt from the
    /// block message"). OR-folded: a single hook setting it flips the aggregate.
    /// Consumed by the orchestrator's `fire_user_prompt_submit` block-message
    /// render (P2-04) to omit the `Original prompt:` tail on a `Block`.
    pub suppress_original_prompt: bool,
    /// The last `displayContent` any folded `MessageDisplay` hook returned
    /// (binary-confirmed at BIN off 201757586). When `Some`, the orchestrator
    /// should substitute this text for the assistant delta on-screen (without
    /// affecting the stored message). `None` when no hook set it. Consumed by the
    /// orchestrator's completed-message `MessageDisplay` pass (P2-04,
    /// `fire_message_display_completed`): the joined assistant text is rendered
    /// ON SCREEN as this value while the stored message / JSONL keep the original.
    pub display_content: Option<String>,
    /// Every `hookSpecificOutput.watchPaths` entry folded from the `FileChanged`
    /// / `CwdChanged` hook results, in execution order (claude-code `v3r` / `E3r`
    /// return `{ results, watchPaths, systemMessages }`, where `watchPaths` is
    /// the concatenation of each fired hook's `hookSpecificOutput.watchPaths`).
    /// The desktop file-changed watcher restarts over the union when non-empty
    /// (`if (v.length > 0) updateWatchPaths(v)`); an empty vec leaves the watch
    /// set unchanged. Additive default empty, so callers with no such hook are
    /// unaffected (byte-identical).
    pub watch_paths: Vec<String>,
    /// The last `hookSpecificOutput.initialUserMessage` any folded `SessionStart`
    /// hook returned (claude-code keeps the latest — `if(p.initialUserMessage)
    /// $os=p.initialUserMessage`). `Some` seeds a pending initial user prompt that
    /// the orchestrator injects as a (non-meta) user message at session start.
    /// Additive default `None` → byte-identical when no hook sets it.
    pub initial_user_message: Option<String>,
    /// `true` when ANY folded `SessionStart` hook returned `reloadSkills: true`
    /// (claude-code `if(p.reloadSkills)u=!0`, OR-folded). Signals the skill/command
    /// directories should be re-scanned. NOTE: the port has no hot-reload seam yet,
    /// so this is captured (no longer dropped at parse) but its re-scan action is a
    /// documented follow-up. Additive default `false`.
    pub reload_skills: bool,
}
