//! Inner turn-by-turn loop helpers. Private to `ConversationOrchestrator`.

use crate::conversation::{classify_api_error, ApiErrorEnvelope, ConversationOrchestrator};
use crate::error::OrchestratorError;
use crate::test_support::{PermissionDecision, PermissionDecisionSource, PermissionResolution};
use hooks::events::HookEvent;
use hooks::registry::HookContext;
use hooks::response::HookDecision;
use llm_client::{ContentBlock as LlmContentBlock, LlmError, LlmResponse};
use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
use std::path::{Component, Path, PathBuf};
use telemetry::tengu::orchestrator as orch_events;
use tool_api::context::{ToolUseContext, ToolUseOptions};
use tool_api::ContextModifier;

async fn forward_tool_progress(
    output: &dyn traits::OutputStream,
    parent_tool_use_id: &str,
    progress: tool_api::progress::ToolProgress,
) {
    if let Some(text) = progress
        .data
        .get("subagent_activity")
        .and_then(serde_json::Value::as_str)
    {
        output.emit_subagent_activity(text).await;
    } else if let Some(message) = progress.data.get("forward_subagent_message") {
        output
            .emit_forwarded_subagent_message(message, parent_tool_use_id)
            .await;
    }
}

/// Registry name of the worktree-creation tool (`tool_worktree::ENTER_TOOL_NAME`).
/// A successful invocation of this tool is the port's sole worktree-creation
/// path, so it is where the turn loop fires the `WorktreeCreate` hook. Held as a
/// literal (not imported) so `orchestrator` keeps no dependency on `tool-worktree`.
const ENTER_WORKTREE_TOOL_NAME: &str = "EnterWorktree";

/// Registry name of the subagent-spawning tool (`tools/agent` `AGENT_TOOL_NAME`)
/// and its legacy alias (`LEGACY_AGENT_TOOL_NAME`). A completed dispatch of this
/// tool means the spawned subagent's loop has stopped, so it is where the turn
/// loop fires the `SubagentStop` hook. Held as literals (not imported) so
/// `orchestrator` keeps no dependency on `tools/agent` — same precedent as
/// `ENTER_WORKTREE_TOOL_NAME`.
const AGENT_TOOL_NAME: &str = "Agent";
const LEGACY_AGENT_TOOL_NAME: &str = "Task";

/// Collapse `.` and `..` segments without touching the filesystem, mirroring
/// Node's `path.normalize`/`resolve` (used by `expandPath`). A `..` pops the
/// previous normal component; a leading `..` with nothing to pop is kept.
pub(crate) fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop the last NORMAL component; otherwise keep the `..`
                // (e.g. above the root prefix or a leading relative `..`).
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn canonical_or_normalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize_lexically(path))
}

fn resolve_tool_file_path(tool_input: &serde_json::Value, cwd: &Path) -> Option<PathBuf> {
    let raw = tool_input
        .get("file_path")
        .or_else(|| tool_input.get("path"))
        .and_then(serde_json::Value::as_str)?;
    let path = Path::new(raw);
    Some(if path.is_absolute() {
        normalize_lexically(path)
    } else {
        normalize_lexically(&cwd.join(path))
    })
}

fn memdir_index_notice_for_tool(
    orch: &ConversationOrchestrator,
    tool_name: &str,
    tool_input: &serde_json::Value,
    is_error: bool,
) -> Option<memory::MemoryIndexNotice> {
    if is_error || !matches!(tool_name, "Write" | "Edit" | "MultiEdit") {
        return None;
    }
    let memdir = orch.memory_prefetch.as_ref()?.user_memdir()?;
    let memory_index = canonical_or_normalize(&memdir.join("MEMORY.md"));
    let target = resolve_tool_file_path(tool_input, &orch.current_cwd())?;
    if canonical_or_normalize(&target) != memory_index {
        return None;
    }
    let content = std::fs::read_to_string(&memory_index).ok()?;
    memory::memory_index_cap_notice(&content)
}

/// #40: apply a hook's folded `terminalSequence` (claude-code `szn`, BIN off
/// 205755390) via the allowlist validator
/// ([`hooks::terminal_seq::validate_terminal_sequence`], the `NEo` port):
/// - REJECT → warn (claude-code's byte-faithful message; the observable half).
/// - ACCEPT → forward the validated string through the
///   [`OutputStream::emit_terminal_sequence`] seam (`BEo`, #6 main-loop parity).
///   The orchestrator holds no TTY (the TUI owns the terminal in a separate
///   process), so non-interactive hosts (print/CLI/tests) keep the no-op.
///
/// Strict no-op when `seq` is `None`.
async fn apply_terminal_sequence(
    orch: &ConversationOrchestrator,
    hook_name: &str,
    seq: Option<&str>,
) {
    let Some(seq) = seq else {
        return;
    };
    match hooks::terminal_seq::validate_terminal_sequence(seq) {
        Some(validated) => {
            // Forward the validated, BEL-normalized sequence to the host's
            // terminal-write seam (claude-code `BEo`). Default no-op off the TUI.
            orch.output.emit_terminal_sequence(&validated).await;
        }
        None => {
            tracing::warn!(
                "Hook {hook_name} returned a terminalSequence that was rejected by the allowlist (only OSC 0/1/2/9/99/777 and BEL are permitted, and OSC 9 bodies may not begin with a digit unless in the 9;4 progress form)"
            );
        }
    }
}

// Tests for this module live in the sibling `turn_loop_test.rs`.
#[cfg(test)]
#[path = "turn_loop_test.rs"]
mod turn_loop_test;

/// Maximum number of consecutive `max_tokens` recovery nudges before the
/// turn loop gives up and surfaces the `max_tokens` `stop_reason`. 1:1 with TS
/// `query.ts:164` `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT = 3`.
pub(crate) const MAX_OUTPUT_TOKENS_RECOVERY_LIMIT: u32 = 3;

/// Escalated output-token cap for the single-shot 8k→64k retry. 1:1 with TS
/// `utils/context.ts:25` `ESCALATED_MAX_TOKENS = 64_000`.
/// [`RecoveryState::max_output_tokens_override`] carries this value into the
/// next `messages_create_with_opts` request.
pub(crate) const ESCALATED_MAX_TOKENS: u32 = 64_000;

/// The byte-exact meta "resume directly" nudge injected as a user message on a
/// `max_tokens` `stop_reason`. 1:1 with TS `query.ts:1226-1227` (note the U+2014
/// em-dash in "directly —"). Concatenating these two string literals — exactly
/// as TS does — yields one contiguous line with NO separator between them.
pub(crate) const MAX_OUTPUT_TOKENS_RECOVERY_NUDGE: &str = concat!(
    "Output token limit hit. Resume directly — no apology, no recap of what you were doing. ",
    "Pick up mid-thought if that is where the cut happened. Break remaining work into smaller pieces.",
);

/// Byte-exact `isMeta` retry message pushed when a `PermissionDenied` hook
/// returns `{retry: true}` on the gated auto-mode classifier-deny path. 1:1 with
/// claude-code `toolExecution.ts:1096`. DORMANT in the external build — the
/// retry path is double-gated (see [`PERMISSION_DENIED_RETRY_MESSAGE`]'s only
/// emit site in the deny arm), so this string is never produced on the normal
/// deny path. When it does fire it is built as a META user message
/// ([`ConversationMessage::user_meta`]), matching CC's `isMeta:!0`.
pub(crate) const PERMISSION_DENIED_RETRY_MESSAGE: &str =
    "The PermissionDenied hook indicated you may retry this tool call.";

/// Byte-exact user-facing message surfaced when the prompt is too long and the
/// reactive 413 recovery (Batch 5) is exhausted. 1:1 with claude-code
/// `errors.ts` `PROMPT_TOO_LONG_ERROR_MESSAGE = 'Prompt is too long'`.
///
/// Re-exported from `model::prompt_too_long` (the orchestrator's own copy),
/// which is the authoritative source for this string in this crate.
pub(crate) use crate::model::prompt_too_long::PROMPT_TOO_LONG_ERROR_MESSAGE;

/// Byte-exact meta nudge injected when the model returns `stop_reason ==
/// "tool_use"` but produces ZERO `tool_use` blocks (a malformed / leaked-invoke
/// response). 1:1 with claude-code v2.1.183 (`bin/claude.exe` offset
/// ~202945837): the first-failure injection text.
///
/// claude-code gates the text on a `tengu_malformed_tool_use_clean_retry`
/// feature flag (`PZa()`) that DEFAULTS TO FALSE, so the default first-failure
/// string is this non-clean-retry variant. (The clean-retry variant would be
/// "The previous response failed to produce a valid tool call. Please retry the
/// tool call now." — gated behind the flag, not emitted in the default build.)
/// Injected as a META user message ([`ConversationMessage::user_meta`]), matching
/// CC's `createUserMessage({…, isMeta:!0})` — it persists with `isMeta:true`.
pub(crate) const MALFORMED_TOOL_USE_RETRY_NUDGE: &str =
    "Your tool call was malformed and could not be parsed. Please retry.";

/// Byte-exact NON-meta message emitted on the SECOND malformed-tool-use failure
/// (the retry also produced no `tool_use` block): the turn terminates as
/// completed. 1:1 with claude-code v2.1.183 (`bin/claude.exe` offset
/// ~202946360, the `tc({content:...})` terminal branch).
pub(crate) const MALFORMED_TOOL_USE_RETRY_FAILED: &str =
    "The model's tool call could not be parsed (retry also failed).";

/// Byte-exact meta nudge injected when the model returns an `end_turn` /
/// `stop_sequence` response with NO visible text (thinking-only output) and it
/// has not yet been nudged this turn. 1:1 with claude-code v2.1.183
/// (`bin/claude.exe` offset ~202947000). Injected as a META user message
/// ([`ConversationMessage::user_meta`]), matching CC's `isMeta:!0` — it persists
/// with top-level `isMeta:true` and is skipped by title/first-prompt extraction.
pub(crate) const THINKING_ONLY_NUDGE: &str =
    "[Your previous response had no visible output. Please continue and produce a user-visible response.]";

/// Byte-exact bare content returned as `is_error:true` `tool_result` when the
/// user-interrupt signal fires BEFORE a tool executes — the pre-cancellation
/// guard in `dispatch_tool_uses_tracked`. 1:1 with claude-code
/// `toolExecution.ts:413-453` `CANCEL_MESSAGE` (utils/messages.ts:210).
const CANCEL_MESSAGE: &str = "The user doesn't want to take this action right now. STOP what you are doing and wait for the user to tell you how to proceed.";

/// Per-conversation recovery bookkeeping carried by the turn drivers in
/// `conversation.rs` and threaded `&mut` into [`execute_one_turn_with_recovery`].
///
/// Mirrors the TS recovery sub-state on `query.ts`'s loop `State`
/// (`maxOutputTokensRecoveryCount`, `maxOutputTokensOverride`). One instance
/// lives per `try_run_turn` / `try_run_turn_streaming` invocation; it persists
/// the nudge count ACROSS turn-steps so the 3-retry limit is consecutive.
#[derive(Debug, Default)]
// The `max_output_tokens_*` prefix is the parity-faithful name for all three
// fields (TS `maxOutputTokens*`); the shared prefix is intentional.
#[allow(clippy::struct_field_names)]
pub(crate) struct RecoveryState {
    /// How many consecutive `max_tokens` nudges have been injected this
    /// conversation. Capped at [`MAX_OUTPUT_TOKENS_RECOVERY_LIMIT`]; once it
    /// reaches the limit the next `max_tokens` ends the turn.
    pub(crate) max_output_tokens_recovery_count: u32,
    /// When `Some(n)`, the NEXT API call uses `n` as its output-token cap
    /// (REC.A1 escalated retry). The turn loop TAKEs it (one-shot) before each
    /// call via [`crate::OrchestratorApiClient::messages_create_with_opts`], so
    /// it never leaks past the single escalated retry.
    pub(crate) max_output_tokens_override: Option<u32>,
    /// Whether the 8k→64k escalation has already fired this recovery episode
    /// (TS gates the single-shot retry on the override being unset; we use a
    /// separate flag because the override is TAKEN per call). Reset alongside
    /// [`Self::max_output_tokens_recovery_count`].
    pub(crate) max_output_tokens_escalated: bool,
    /// #77: whether a malformed-tool-use retry (`stop_reason == "tool_use"` with
    /// zero `tool_use` blocks) has already fired this turn. Mirrors claude-code's
    /// `transition.reason === "malformed_tool_use_retry"` guard so the SECOND
    /// such failure terminates instead of looping. NOT reset by
    /// [`Self::reset_max_output_tokens_recovery`] — it is a per-turn one-shot
    /// independent of the max-output-tokens escalation episode.
    #[allow(clippy::struct_field_names)]
    pub(crate) malformed_tool_use_retried: bool,
    /// #78: whether the thinking-only nudge (an `end_turn`/`stop_sequence`
    /// response with no visible text) has already fired this turn. Mirrors
    /// claude-code's `thinkingOnlyNudged` loop-state flag.
    #[allow(clippy::struct_field_names)]
    pub(crate) thinking_only_nudged: bool,
}

impl RecoveryState {
    /// Reset the `max_output_tokens` recovery bookkeeping to begin a fresh
    /// escalation episode: zero the consecutive nudge count, drop any armed
    /// escalation override, and re-arm the 8k→64k single-shot. 1:1 with the TS
    /// loop-state resets that set `maxOutputTokensRecoveryCount: 0` +
    /// `maxOutputTokensOverride: undefined` on a continuation — the token-budget
    /// continuation (`query.ts:1332`) AND the Stop-hook blocking continuation
    /// (RECOV.4, `query.ts:1291`).
    pub(crate) fn reset_max_output_tokens_recovery(&mut self) {
        self.max_output_tokens_recovery_count = 0;
        self.max_output_tokens_override = None;
        self.max_output_tokens_escalated = false;
    }
}

/// What one turn step decided.
pub(crate) enum TurnStepOutcome {
    /// Continue the loop (e.g. model returned `tool_use`).
    Continue,
    /// Loop should terminate — model returned `end_turn`.
    Ended {
        final_message_id: MessageId,
        stop_reason: String,
    },
}

/// Execute one `messages_create_non_stream` round-trip + tool dispatches.
///
/// `system` is the assembled system prompt for the conversation (built
/// once by `ConversationOrchestrator::try_run_turn`). It is passed
/// through to every API round-trip in the conversation, NOT re-built
/// per turn-step — the prompt is stable across the conversation lifetime
/// (see M5-03 plan "Out of scope" note: SSE M5-04 will not re-assemble
/// per turn-step either).
// Retained as a test-only convenience: the legacy no-recovery shim. As of #2
// (main-loop parity) the cancelable REPL driver no longer uses it — it now
// calls the recovery-aware [`execute_one_turn_with_recovery_tracked`] like the
// main batched [`ConversationOrchestrator::run_turn`] loop. The in-file
// `#[cfg(test)]` suites still drive this clean-signature wrapper, so it is kept
// (not `#[cfg(test)]`-gated, to preserve the intra-doc links from the live
// `_tracked` function).
#[allow(dead_code)]
pub(crate) async fn execute_one_turn(
    orch: &ConversationOrchestrator,
    system: Option<&str>,
) -> Result<TurnStepOutcome, OrchestratorError> {
    // Backward-compatible shim: no recovery state → legacy disposition
    // (any non-`end_turn` stop_reason Continues). Used by the in-file tests.
    // The recovery-aware drivers call [`execute_one_turn_with_recovery`] with a
    // live `RecoveryState`.
    execute_one_turn_with_recovery(orch, system, None).await
}

/// Recovery-aware twin of [`execute_one_turn`].
///
/// When `recovery` is `Some`, a `max_tokens` `stop_reason` triggers the A1
/// multi-turn nudge: while the consecutive recovery count is below
/// [`MAX_OUTPUT_TOKENS_RECOVERY_LIMIT`], a byte-exact "resume directly" meta
/// user message ([`MAX_OUTPUT_TOKENS_RECOVERY_NUDGE`]) is appended to history,
/// the counter is incremented, and the step returns
/// [`TurnStepOutcome::Continue`] (1:1 with TS `query.ts:1223-1252`). When the
/// count has reached the limit, the turn ends with `stop_reason = "max_tokens"`
/// (TS `query.ts:1254-1255` surfaces the withheld error). When `recovery` is
/// `None`, the `max_tokens` path falls through to the legacy disposition
/// (Continue), preserving the legacy disposition.
///
/// Test-only as of #2: the production drivers all call the `_tracked` variant
/// directly. Retained (not `#[cfg(test)]`) so intra-doc links resolve in the
/// normal doc build.
#[allow(dead_code)]
pub(crate) async fn execute_one_turn_with_recovery(
    orch: &ConversationOrchestrator,
    system: Option<&str>,
    recovery: Option<&mut RecoveryState>,
) -> Result<TurnStepOutcome, OrchestratorError> {
    // Drop the per-call output-token count (A3 callers use the `_tracked`
    // variant). Preserves the historical signature for every existing caller.
    Ok(
        execute_one_turn_with_recovery_tracked(orch, system, recovery)
            .await?
            .0,
    )
}

/// A3 twin of [`execute_one_turn_with_recovery`] that ALSO returns this turn
/// step's output-token count (`response.usage.output_tokens`).
///
/// The token-budget continuation loop (`conversation.rs`) accumulates these
/// into `global_turn_tokens` and feeds the running total to
/// [`crate::token_budget::check_token_budget`] — mirroring TS
/// `getTurnOutputTokens()`. The plain
/// [`execute_one_turn_with_recovery`] wrapper drops the count so existing
/// callers (the cancelable REPL driver + in-file tests) are unchanged.
#[allow(clippy::too_many_lines)]
pub(crate) async fn execute_one_turn_with_recovery_tracked(
    orch: &ConversationOrchestrator,
    system: Option<&str>,
    mut recovery: Option<&mut RecoveryState>,
) -> Result<(TurnStepOutcome, u64), OrchestratorError> {
    // P0.1 (batched twin): arm the memory-selector prefetch CONCURRENTLY with
    // this turn (claude-code `wAo`). Fired here at turn start so the in-flight
    // handle is ready when `relevant_memory_reminder_messages` consumes it below,
    // before the blocking-limit estimate. A strict no-op when no prefetch is
    // wired, keeping the locked turn-loop fixtures byte-identical. See
    // [`ConversationOrchestrator::start_memory_prefetch`].
    orch.start_memory_prefetch().await;
    // EXPERIMENTAL_SKILL_SEARCH (batched twin): arm the skill-discovery prefetch
    // CONCURRENTLY with this turn (claude-code `startSkillDiscoveryPrefetch`,
    // bundle `B=at1?.startSkillDiscoveryPrefetch(null,V,T)`). A strict no-op when
    // no prefetch is wired (default OFF), keeping the locked fixtures
    // byte-identical. See [`ConversationOrchestrator::start_skill_discovery_prefetch`].
    orch.start_skill_discovery_prefetch().await;
    // P1 (§6.5, batched twin): background-fork a session-memory extraction if the
    // tool-call threshold has crossed (inert unless wired + enabled).
    orch.maybe_extract_session_memory().await;

    // In-Loop Compaction Batch 4: proactively snip+micro+autocompact BEFORE
    // snapshotting history for the model call, so a long conversation
    // self-compacts mid-turn (TS pre-call pipeline `query.ts:365-467`). A strict
    // no-op when no compactor is wired or the history is under threshold, so the
    // locked turn-loop fixtures are unaffected. After a proactive compact, the
    // snapshot below reads the NEW, compacted history.
    orch.maybe_compact_before_call().await;

    // Snapshot the current session history for the API call.
    let (mut history_snapshot, model, model_profile) = {
        let s = orch.session.lock().await;
        (s.history.clone(), s.model.clone(), s.model_profile.clone())
    };

    // R-P1c/R-P1d: PREPEND the leading `additionalContext` meta message
    // (`# claudeMd` / `# userEmail` / `# currentDate`) to THIS call's OUTGOING
    // snapshot only (never `session.history` / JSONL). 1:1 with claude-code
    // `A6n(re, userContext)`, which prepends the meta message at every
    // `callModel`. Recomputed each turn, never accumulates.
    orch.prepend_leading_context(&mut history_snapshot).await;

    // Collected, not pushed: `call_api_with_ptl_recovery` rebuilds the request
    // from raw `session.history` on every retry, and each of these advances
    // session state when computed, so they must be reused rather than
    // recomputed. See the `turn_reminders` param there.
    let mut turn_reminders: Vec<ConversationMessage> = Vec::new();

    // OUTSTYLE.3: per-turn, transient output-style reminder. When a non-default
    // output style is active, claude-code injects a meta user message into EVERY
    // turn's model input (the `output_style` attachment). We append it to THIS
    // call's OUTGOING snapshot only — never to `session.history` / JSONL — so it
    // is recomputed each turn and never accumulates (TS recomputes attachments
    // each turn). Trailing position mirrors TS (`[userMessage,
    // ...attachmentMessages]`). `None` for the default style ⇒ no extra message,
    // keeping the locked turn-loop fixtures byte-identical. See
    // [`ConversationOrchestrator::output_style_reminder_message`].
    if let Some(reminder) = orch.output_style_reminder_message().await {
        turn_reminders.push(reminder);
    }

    // PLANMODE (batched twin): per-turn, transient `plan_mode` reminder (206
    // `xEg` attachment). Emitted ONLY while `session.plan_mode` is active — i.e.
    // after `EnterPlanMode` — so the DEFAULT build (plan mode OFF) stays
    // byte-identical and the locked turn-loop fixtures still pass. Placed right
    // after the output-style reminder and BEFORE the skill-listing reminder,
    // matching 206 `KJn` where the plan_mode attachment (`l=await xEg(t)`) is
    // spread before the invoked-skills bodies (`REg`) and the tool/mcp deltas
    // (`gYt`/`YJn`). Appended to THIS call's OUTGOING snapshot only (never
    // `session.history` / JSONL). See
    // [`ConversationOrchestrator::plan_mode_reminder_message`].
    if let Some(reminder) = orch.plan_mode_reminder_message().await {
        turn_reminders.push(reminder);
    }

    // SKILLLIST.1: per-turn, transient `skill_listing` reminder so the model can
    // discover the available skills. Appended to THIS call's OUTGOING snapshot
    // only (never to `session.history` / JSONL), after the output-style reminder
    // so the locked output-style fixtures stay byte-identical. `None` when no
    // provider is wired / no skills / the Skill tool is absent. See
    // [`ConversationOrchestrator::skill_listing_reminder_message`].
    if let Some(reminder) = orch.skill_listing_reminder_message().await {
        turn_reminders.push(reminder);
    }

    // §F: per-turn, transient `conditional_rules` reminder — path-gated LINGXI.md
    // rules that newly activate because a touched file matches their globs.
    // Appended to THIS call's OUTGOING snapshot only (never `session.history` /
    // JSONL), after the skill-listing reminder so the locked fixtures stay
    // byte-identical. `None` when no provider / no conditional rules / nothing
    // newly active. See [`ConversationOrchestrator::conditional_rules_reminder_message`].
    if let Some(reminder) = orch.conditional_rules_reminder_message().await {
        turn_reminders.push(reminder);
    }

    // Nested memory (`k$o` / `Rop`): the LINGXI.md governing the directory of a
    // file the session has touched. Appended to THIS call's OUTGOING snapshot
    // only (never `session.history` / JSONL), directly AFTER conditional rules
    // — the order matters, because a `paths:`-gated rule claimed there is
    // skipped here via the shared `sent_conditional_rules` set, so running the
    // two the other way round would change which mechanism reports the rule.
    // `None` when nothing has been touched / no ancestor memory / everything
    // already sent. See [`ConversationOrchestrator::nested_memory_reminder_message`].
    if let Some(reminder) = orch.nested_memory_reminder_message().await {
        turn_reminders.push(reminder);
    }

    // Per-turn, transient `<new-diagnostics>` reminder — newly-reported LSP
    // diagnostics not yet surfaced (claude-code `formatDiagnosticsBlock`).
    // Appended to THIS call's OUTGOING snapshot only. `None` when no LSP source
    // is wired (no servers) or no new diagnostics. See
    // [`ConversationOrchestrator::new_diagnostics_reminder_message`].
    if let Some(reminder) = orch.new_diagnostics_reminder_message().await {
        turn_reminders.push(reminder);
    }

    // `agent_listing_delta`: per-turn, transient agent catalog reminder, emitted
    // when the `LINGXI_AGENT_LIST_IN_MESSAGES` gate is ON (the v2.1.193
    // DEFAULT — the catalog is externalized here, the `AgentTool` description
    // carries only the pointer line; an explicit `=false` opts into the legacy
    // inline catalog ⇒ `None` here). Appended to THIS call's OUTGOING snapshot
    // only (never `session.history` / JSONL), after the conditional-rules
    // reminder. See [`ConversationOrchestrator::agent_listing_reminder_message`].
    if let Some(reminder) = orch.agent_listing_reminder_message().await {
        turn_reminders.push(reminder);
    }

    // Finding #73 (batched twin): per-turn, transient `todo_reminder` (V1) /
    // `task_reminder` (V2) reminder, emitted ONLY when the killswitch is not
    // `"off"`, the relevant tool is present (TodoWrite / TaskUpdate), the Brief
    // tool is absent, the history is non-empty, and BOTH counters
    // (`turns_since_last_todo_write` / `turns_since_last_reminder`) reach their
    // thresholds (10/10). The body is emitted RAW (no `<system-reminder>` wrap,
    // matching the binary's `Ln({content:r,isMeta:!0})`). Placed after the
    // agent-listing reminder and before the async-hook reminder, mirroring the
    // binary's `ytl` order (`todo_reminders` in the core `A` array, before the
    // main-only `async_hook_responses`). Appended to THIS call's OUTGOING
    // snapshot only (never `session.history` / JSONL). `None` keeps the locked
    // turn-loop fixtures byte-identical (default: counters start at 0). See
    // [`ConversationOrchestrator::todo_reminder_message`].
    if let Some(reminder) = orch.todo_reminder_message().await {
        turn_reminders.push(reminder);
    }

    // async_hook_response (batched twin): fold completed background (`async`)
    // hook responses into THIS call's OUTGOING snapshot only (never
    // `session.history` / JSONL), drained consume-once. `None` when no source is
    // wired / nothing completed since the last turn. See
    // [`ConversationOrchestrator::async_hook_response_reminder_message`].
    if let Some(reminder) = orch.async_hook_response_reminder_message().await {
        turn_reminders.push(reminder);
    }

    // T35 (batched twin — #3 main-loop parity): fold the terminal background
    // tasks finished since the last turn into THIS call's OUTGOING snapshot only
    // (never `session.history` / JSONL), drained consume-once so each completion
    // surfaces exactly one `<task-notification>`. Placed after the async-hook
    // reminder and before the relevant-memory reminder, identical to the
    // streaming twin — claude-code has ONE main loop, so both LingXi twins must
    // inject this reminder. `None` when no registry is wired / nothing finished.
    // See [`ConversationOrchestrator::task_notification_reminder_message`].
    if let Some(reminder) = orch.task_notification_reminder_message().await {
        turn_reminders.push(reminder);
    }

    // P0.1 (batched twin): per-turn, transient `relevant_memories` SURFACING
    // reminders — the memory-selector/prefetch result rendered as one meta user
    // message per surfaced memory. Appended to THIS call's OUTGOING
    // snapshot only (never `session.history` / JSONL), after the async-hook
    // reminder and BEFORE the blocking-limit estimate inside
    // `call_api_with_ptl_recovery` so its tokens are counted in the prompt size.
    // Awaits the prefetch armed by `start_memory_prefetch` at turn start. Empty
    // when no prefetch is wired / the result is empty / everything was already
    // injected. See [`ConversationOrchestrator::relevant_memory_reminder_messages`].
    turn_reminders.extend(orch.relevant_memory_reminder_messages().await);

    // EXPERIMENTAL_SKILL_SEARCH (batched twin): per-turn, transient
    // `skill_discovery` SURFACING reminder — the discovery prefetch result
    // rendered as one `<system-reminder>` meta user message. Collected AFTER the
    // memory consume above (matches the bundle order: memory `P.consumedOnIteration`
    // → then `at1.collectSkillDiscoveryPrefetch`). Appended to THIS call's
    // OUTGOING snapshot only. `None` when no prefetch is wired (default OFF) /
    // empty result / everything already surfaced. See
    // [`ConversationOrchestrator::skill_discovery_reminder_message`].
    if let Some(reminder) = orch.skill_discovery_reminder_message().await {
        history_snapshot.push(reminder);
    }

    // 1. Call the API. Advertise the registry's wire tool definitions
    //    (same set + serialization as the streaming path). Batch 5: the call is
    //    wrapped in the blocking-limit preempt + 413/prompt-too-long reactive
    //    recovery loop. When recovery is exhausted the helper returns
    //    `PtlCallOutcome::PromptTooLong`, and we end the turn with a byte-exact
    //    `PROMPT_TOO_LONG_ERROR_MESSAGE` assistant message instead of bubbling a
    //    hard error.
    let tools = orch.build_wire_tools().await;
    history_snapshot.extend(turn_reminders.iter().cloned());

    let deferred_tools_reminder = orch.deferred_tools_reminder_message();
    if let Some(reminder) = deferred_tools_reminder.clone() {
        orch.prepend_transient_leading_context(&mut history_snapshot, reminder);
    }
    // `date_change` (batched twin): sessions crossing local midnight tell the
    // model the new date once per changed date. Prepended AFTER the deferred
    // insert so the final order is [date_change, deferred_tools_delta, …] —
    // matching the oracle attachment batch order (`Ky("date_change")` before
    // `Ky("deferred_tools_delta")`). Computed ONCE per step and reused by
    // `call_api_with_ptl_recovery`'s retry/fallback re-snapshots, which rebuild
    // the SAME request from raw history; the dedupe is committed in there, past
    // the blocking-limit preempt. `None` (same-date turns) keeps the locked
    // turn-loop fixtures byte-identical. See
    // [`ConversationOrchestrator::date_change_reminder_message`].
    let date_change_reminder =
        orch.date_change_reminder_message(orch.session.lock().await.session_id);
    if let Some(reminder) = date_change_reminder.clone() {
        orch.prepend_transient_leading_context(&mut history_snapshot, reminder);
    }
    // REC.A1: consume the one-shot escalated `max_tokens` override (armed by a
    // prior `max_tokens` recovery via `handle_max_output_tokens`). TAKE it so it
    // applies to EXACTLY this call and never leaks to the next turn.
    let max_tokens_override = recovery
        .as_deref_mut()
        .and_then(|r| r.max_output_tokens_override.take());
    // #5: wall-clock the API round-trip (incl. any in-adapter retries + the PTL
    // reactive-recovery tail) so the CostTracker records a REAL duration instead
    // of `Duration::ZERO`. Paired with `orch.api.last_retry_count()` below.
    let api_call_started = std::time::Instant::now();
    // tengu_api_success `messageCount:n` / `messageTokens:r`: capture from the
    // input snapshot BEFORE it is moved into `call_api_with_ptl_recovery`.
    let api_success_message_count = u32::try_from(history_snapshot.len()).unwrap_or(u32::MAX);
    let api_success_message_tokens =
        compaction::grouping::estimate_tokens_for_range(&history_snapshot);
    let response = match call_api_with_ptl_recovery(
        orch,
        system,
        &model,
        model_profile.as_deref(),
        history_snapshot,
        tools,
        max_tokens_override,
        deferred_tools_reminder,
        date_change_reminder,
        &turn_reminders,
    )
    .await
    {
        Ok(outcome) => match outcome {
            PtlCallOutcome::Response(resp) => resp,
            PtlCallOutcome::PromptTooLong => {
                let assistant_id = surface_prompt_too_long(orch).await;
                return Ok((
                    TurnStepOutcome::Ended {
                        final_message_id: assistant_id,
                        stop_reason: "prompt_too_long".to_string(),
                    },
                    0,
                ));
            }
            PtlCallOutcome::BlockingLimit => {
                // PROACTIVE blocking-limit preempt: surface the prompt-too-long
                // message (its api-error field is `invalid_request`, like the
                // binary's `Ol({...,error:"invalid_request"})`) but end the turn with
                // the DISTINCT terminal reason `"blocking_limit"` — the binary's
                // `{reason:"blocking_limit"}` (offset ~208021400), kept separate from
                // the reactive-exhausted `prompt_too_long` so SDK/stream-json
                // consumers categorize the two preempt origins distinctly.
                let assistant_id = surface_prompt_too_long(orch).await;
                return Ok((
                    TurnStepOutcome::Ended {
                        final_message_id: assistant_id,
                        stop_reason: "blocking_limit".to_string(),
                    },
                    0,
                ));
            }
            PtlCallOutcome::RapidRefillBreaker => {
                // #54 reactive trip: surface the thrashing message (api-error field
                // `invalid_request`, matching the binary `Ol({...,error:"invalid_request"})`)
                // but end the turn with the terminal reason `"rapid_refill_breaker"`
                // — the binary's loop returns `{reason:"rapid_refill_breaker"}` even
                // though the assistant MESSAGE carries `error:"invalid_request"`
                // (`bin/claude.exe` offset ~208016504; terminal-reason enum lists
                // `rapid_refill_breaker`, never `invalid_request`).
                let assistant_id = surface_rapid_refill_thrashing(orch).await;
                return Ok((
                    TurnStepOutcome::Ended {
                        final_message_id: assistant_id,
                        stop_reason: "rapid_refill_breaker".to_string(),
                    },
                    0,
                ));
            }
        },
        // #10: a model/runtime error that escaped the API layer is NOT a hard
        // failure (faithful port of `query.ts:955-997` catch → `model_error`).
        // PROPAGATE the carve-outs that have dedicated downstream handling
        // (RateLimited → wrapper rate-limit enrichment; Overloaded /
        // RepeatedOverloaded → the "Repeated 529" surface); surface EVERYTHING
        // else gracefully as an `isApiErrorMessage` assistant message + end the
        // turn with `reason:"model_error"` (no Stop/StopFailure hooks — the catch
        // path runs neither). 0 output tokens.
        Err(e) if is_carveout_propagated(&e) => return Err(e),
        Err(e) => {
            // Classify the TYPED error into the api-error envelope (`Flp`/`KNn`)
            // BEFORE consuming it for the verbatim error text. The rendered
            // message stays `e.to_string()` (`createAssistantAPIErrorMessage`
            // renders content verbatim); the envelope adds `error`/`apiErrorStatus`.
            let env = classify_api_error(&e);
            // Content is rendered verbatim (`e.to_string()`) EXCEPT a 413
            // `request_too_large` (accumulated images/attachments), which the
            // 2.1.212 handler renders with the byte-exact `$Vi()` notice.
            let content = match &e {
                OrchestratorError::ApiCall(LlmError::RequestTooLarge)
                | OrchestratorError::Streaming(LlmError::RequestTooLarge) => {
                    crate::conversation::request_too_large_notice(orch.prompt_is_interactive())
                }
                _ => e.to_string(),
            };
            let assistant_id = surface_model_error(orch, &content, env).await;
            return Ok((
                TurnStepOutcome::Ended {
                    final_message_id: assistant_id,
                    stop_reason: "model_error".to_string(),
                },
                0,
            ));
        }
    };

    // A3: this call's output-token count, returned to the budget loop so it can
    // accumulate `global_turn_tokens` (TS `getTurnOutputTokens()`).
    let output_tokens = response.usage.billable_tokens.output;

    // #55: cache this response's total input tokens (the `Xtt` last-usage
    // snapshot) so the proactive fixed-prefix overflow guard can compute the
    // immovable prefix on the next `maybe_compact_before_call`.
    orch.record_response_input_tokens(&response.usage);

    // In-Loop Compaction Batch 6: snapshot the cache-safe prompt prefix now the
    // call has succeeded, so the forked autocompact summarizer can replay this
    // turn's prefix and share Anthropic's prompt cache. `session.history` here is
    // the exact message set the model saw (post any PTL truncation / reactive
    // compaction inside `call_api_with_ptl_recovery`), BEFORE the assistant reply
    // is appended below. Strict no-op when no cache-safe slot is wired.
    orch.save_cache_safe_params(system, &model).await;
    // FORK (codex #5 follow-up): record the rendered system prompt this turn
    // handed the model, so a fork-subagent spawn dispatched below in this same
    // turn can thread the exact bytes onto its child (cache-identical prefix).
    orch.save_current_turn_system_prompt(system).await;

    // Task 8 (llm-client future-work batch 3): the call succeeded — forward
    // the adapter's unified rate-limit snapshot to the output stream when it
    // changed since the last emission (emit-on-change; no-op for clients
    // without a snapshot). Covers the batched AND cancelable drivers (both
    // funnel through this function).
    orch.emit_rate_limit_if_changed().await;
    // Task 2 (llm-client future-work batch 5): same seam, raw per-window
    // utilization snapshot (emit-on-change; empty snapshot never emitted).
    orch.emit_raw_utilization_if_changed().await;

    // 1.5 M6-06: record this response's usage into the wired CostTracker (if any).
    // #5 (main-loop parity): pass the REAL wall-clock duration of the API
    // round-trip and the REAL retry count (`last_retry_count()`, the adapter's
    // `RetryState::attempt`) instead of the previous hardcoded `Duration::ZERO`
    // / `0`. claude-code's cost recorder receives both.
    if let Some(tracker) = orch.cost_tracker.as_ref() {
        let usage = crate::cost_wiring::llm_usage_to_cost_usage(&response.usage);
        let cache_read = response.usage.billable_tokens.cache_read;
        let cache_create = response.usage.billable_tokens.cache_write;
        let model_ref = crate::cost_wiring::model_ref_from_string(&model, model_profile.as_deref());
        let elapsed = api_call_started.elapsed();
        let retries = orch.api.last_retry_count();
        let cost_for_this_call = tracker
            .record_api_response_v2(
                model_ref.clone(),
                usage,
                elapsed,
                retries,
                cache_read,
                cache_create,
                false, // is_batch_request — M6 always false
                orch.analytics_bus.as_ref(),
            )
            .await;
        // strict-parity (2.1.195): fire `tengu_api_success` on the per-request
        // success path (claude `j("tengu_api_success", {...})`). The port-only
        // `tengu_cost_recorded` event was dropped. request id / stop reason /
        // provider live on the orchestrator, so we emit directly here.
        if let Some(bus) = orch.analytics_bus.as_ref() {
            #[allow(clippy::cast_possible_truncation)]
            let dur_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
            cost::emit_api_success(
                bus,
                &cost::ApiSuccessFields {
                    model: model.clone(),
                    input_tokens: response.usage.billable_tokens.input,
                    output_tokens: response.usage.billable_tokens.output,
                    cached_input_tokens: cache_read,
                    uncached_input_tokens: cache_create,
                    duration_ms: dur_ms,
                    duration_ms_including_retries: dur_ms,
                    attempt: retries + 1,
                    cost_nano_usd: cost_for_this_call,
                    provider: crate::cost_wiring::provider_tag(&model_ref.provider),
                    stop_reason: response.stop_reason.clone(),
                    request_id: orch.api.last_request_id(),
                    message_count: api_success_message_count,
                    message_tokens: api_success_message_tokens,
                    did_fall_back_to_non_streaming: false,
                    is_non_interactive_session: !orch.prompt_is_interactive(),
                    print: !orch.prompt_is_interactive(),
                    is_tty: false,
                    query_source: "user".into(),
                    permission_mode: if orch.session.lock().await.plan_mode {
                        "plan"
                    } else {
                        "default"
                    }
                    .to_string(),
                    ttft_ms: None,
                    fast_mode: response.usage.speed.as_deref() == Some("fast"),
                    time_since_last_api_call_ms: orch.record_api_call_gap_ms(),
                },
            )
            .await;
        }
        orch.api_calls_recorded
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    // 2. Translate `LlmResponse.content` -> `ContentBlock` history entry.
    let assistant_blocks = translate_response_blocks(&response.content);

    // 3. Append the assistant message to the session. We need the
    //    `final_message_id` to return to the caller.
    let assistant_id = MessageId::new();
    let assistant_msg = ConversationMessage::Assistant {
        id: assistant_id,
        content: assistant_blocks.clone(),
        stop_reason: response.stop_reason.clone(),
    };
    {
        let mut s = orch.session.lock().await;
        s.history.push(assistant_msg.clone());
    }
    // M5-07 T13: mirror the in-memory append to the optional JSONL writer.
    // Best-effort — write failures never fail the turn.
    //
    // NON-streaming (batched) parity: claude-code's non-streaming response
    // handler (`claude.ts:2571`) emits exactly ONE merged `AssistantMessage`
    // (single top-level uuid, ALL blocks via `...result` / full `content`) — it
    // does NOT split per content block. Only the STREAMING `content_block_stop`
    // writer (`claude.ts:2171-2211`) splits one line per block. So the batched
    // path persists ONE merged assistant JSONL line; the tool_results below
    // chain off that single line's uuid (shared parent), matching the
    // non-streaming transcript shape. The per-block split lives ONLY on the
    // streaming drain (`conversation.rs::persist_assistant_per_block`).
    //
    // Persist the FULL BetaMessage envelope (real model + usage + requestId) via
    // the batched counterpart — NOT the model-less `persist_message_to_jsonl`,
    // which recorded real replies as `model:"<synthetic>"` with `usage` dropped
    // (the `--print` / `--bg` mislabel + lost-cost bug). `claude.ts:2571` builds
    // the merged non-streaming AssistantMessage with `result.model`/`usage`.
    let request_id = orch.api.last_request_id();
    orch.persist_assistant_merged(&assistant_msg, Some(&response.usage), request_id.as_deref())
        .await;

    // 4. Emit each Text block to the output stream (whole-body in M5-02;
    //    M5-04 will switch to per-delta).
    for blk in &assistant_blocks {
        if let ContentBlock::Text { text } = blk {
            orch.output.emit_text(text).await;
        }
    }

    // OTEL_LOG_ASSISTANT_RESPONSES (claude-code opt-in): default OFF, byte-no-op.
    // When enabled, log the assistant text with req-id/model/stop/usage so OTEL
    // exporters capture response bodies. The gate reads the single authoritative
    // predicate in the OTEL monitoring module (H-BIN-06), which parses the var
    // with the byte-faithful `ct` truthy semantics (1/true/yes/on, trimmed,
    // case-insensitive). Default (var unset) keeps the locked turn fixtures OFF.
    if telemetry::otel::logs::assistant_responses_enabled() {
        let text: String = assistant_blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        tracing::info!(
            event = "assistant_response",
            request_id = orch.api.last_request_id().unwrap_or_default(),
            model = %model,
            stop_reason = response.stop_reason.as_deref().unwrap_or(""),
            input_tokens = response.usage.billable_tokens.input,
            output_tokens = response.usage.billable_tokens.output,
            body = %text,
        );
        telemetry::otel::emit_assistant_response_log(
            orch.api.last_request_id().as_deref().unwrap_or_default(),
            &model,
            response.stop_reason.as_deref().unwrap_or(""),
            response.usage.billable_tokens.input,
            response.usage.billable_tokens.output,
            &text,
        );
    }

    // 5. If there are tool_use blocks, dispatch them and feed results back.
    let tool_uses: Vec<(ToolUseId, String, serde_json::Value, Option<String>)> = assistant_blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse {
                id,
                name,
                input,
                provider_id,
            } => Some((id.clone(), name.clone(), input.clone(), provider_id.clone())),
            _ => None,
        })
        .collect();

    // HOOK.2: a PreToolUse hook returning `continue:false` (preventContinuation)
    // stops the agent loop AFTER this turn step's tools have run (TS
    // `query.ts:1518-1521` returns `{ reason: 'hook_stopped' }`). The tracked
    // dispatch ORs the per-tool `prevent_continuation` signal; the tool still
    // executes and its results are still appended below, exactly like TS (where
    // the tool runs and `hook_stopped_continuation` is yielded after success).
    let mut hook_prevent_continuation = false;
    if !tool_uses.is_empty() {
        let (tool_results, prevent, injected_messages, context_modifiers) =
            dispatch_tool_uses_tracked(orch, &tool_uses, None).await?;
        hook_prevent_continuation = prevent;
        // Append a fresh user message carrying the tool results.
        let user_id = MessageId::new();
        let tool_results_msg = ConversationMessage::User {
            id: user_id,
            content: tool_results,
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        {
            let mut s = orch.session.lock().await;
            s.history.push(tool_results_msg.clone());
            // SKILLEXEC.3 (Part A): a tool may inject follow-up conversation
            // messages (TS `ToolResult.newMessages` — e.g. the Skill tool's
            // expanded skill prompt). They enter history IMMEDIATELY AFTER this
            // turn's tool_result user message, in tool-dispatch order, so the
            // model processes them on the next API call. `injected_messages` is
            // empty for every existing tool, so this loop is a strict no-op and
            // the locked turn-loop parity fixtures stay byte-identical.
            //
            // Also record each injected message's id → originating tool_use_id
            // into the in-memory `injected_message_sources` side-table (faithful
            // port of TS `sourceToolUseID`; `#[serde(skip)]` so it never reaches
            // the JSONL wire). No-op when `injected_messages` is empty.
            for (m, tool_use_id) in &injected_messages {
                s.history.push(m.clone());
                s.injected_message_sources
                    .insert(m.id(), tool_use_id.clone());
            }
        }
        // M5-07 T13: persist the tool_result user message. Best-effort.
        orch.persist_message_to_jsonl(&tool_results_msg).await;
        // O3: flush this batch's hook `attachment` lines, in tool-dispatch
        // order, right after the tool_result they follow — where claude's own
        // stream order puts them (`insertMessageChain` writes the yielded
        // attachment message immediately after the yielded tool_result).
        for (tool_use_id, _, _, _) in &tool_uses {
            orch.flush_hook_attachments(tool_use_id).await;
        }
        // Persist the injected skill messages too (best-effort), mirroring the
        // tool_result persist above. No-op when empty. NOTE: the originating
        // tool_use_id is deliberately NOT persisted — TS does not write
        // `sourceToolUseID` to the transcript, so the JSONL bytes stay
        // byte-identical to before this change.
        //
        // O3: an `is_meta` injected message is the EPHEMERAL RENDERING of an
        // attachment (claude builds it from the attachment at
        // API-normalization time, renderer BIN off 238107100) — the attachment
        // line flushed above IS its on-disk record, so persisting it too would
        // duplicate it. Every non-hook injected message (the Skill tool's
        // expanded prompt) is non-meta and still persists.
        for (m, _tool_use_id) in &injected_messages {
            if m.is_meta() {
                continue;
            }
            orch.persist_message_to_jsonl(m).await;
        }
        // SKILLEXEC.3 (model scope): fold this batch's `context_modifier`s and
        // switch `session.model` if a skill declared a `model:` override. Applied
        // AFTER `injected_messages` so it mirrors the streaming twin's ordering.
        // Empty for every existing tool + non-`model:` skills → strict no-op
        // (session.model untouched → byte-identical turn-loop fixtures).
        apply_model_context_modifiers(orch, context_modifiers).await;
    }

    // Finding #73 (batched twin): advance the per-turn todo/task reminder
    // counters for THIS assistant turn, then reset `turns_since_last_todo_write`
    // to 0 if this turn's assistant response invoked the variant's "recent use"
    // tool (TodoWrite for V1; TaskCreate/TaskUpdate for V2). Mirrors the
    // binary's per-assistant-message counting in `L4p`/`N4p` (which zero `r` at
    // the last such tool_use). Order — bump THEN reset — so a turn that calls
    // TodoWrite lands at 0 (not 1), matching the binary scan that excludes the
    // TodoWrite message itself. No-op for the locked fixtures (a single-turn
    // run never reaches the threshold).
    orch.bump_reminder_turn_counters().await;
    let invoked_tool_names: Vec<String> = tool_uses
        .iter()
        .map(|(_, name, _, _)| name.clone())
        .collect();
    orch.note_todo_reminder_tool_call(&invoked_tool_names).await;

    // #78 nudge guard `!Pt(ce)`: suppress the thinking-only nudge during a
    // StructuredOutput exchange. Computed here (a match guard cannot `.await`
    // the session lock); the scan is cheap — it stops at the first real user
    // message. The current assistant response is already in `history` (pushed at
    // the top of this fn), mirroring the binary's `se` including `se.at(-1)`.
    let prior_structured_output = {
        let session = orch.session();
        let s = session.lock().await;
        prior_assistant_used_structured_output(&s.history)
    };

    // EndConversation (2.1.206): the tool's 2nd consecutive call raised the
    // shared end-request slot during tool execution above. Consume it; if
    // raised, terminate the conversation and surface the end message to the
    // user. Default-OFF (no slot wired) → never raised → byte-identical.
    let end_conversation_requested = orch
        .end_conversation_slot
        .as_ref()
        .is_some_and(|s| s.swap(false, std::sync::atomic::Ordering::SeqCst));
    if end_conversation_requested {
        orch.output
            .emit_text(crate::prompt::end_conversation::END_CONVERSATION_ENDED_MESSAGE)
            .await;
    }

    // 6. Decide loop disposition.
    let outcome = if end_conversation_requested {
        // The model confirmed (2nd EndConversation call) — end the query.
        TurnStepOutcome::Ended {
            final_message_id: assistant_id,
            stop_reason: "end_conversation".to_string(),
        }
    } else if hook_prevent_continuation {
        // HOOK.2: honor the PreToolUse `continue:false` request — end the turn
        // step so the driver stops the loop (TS `{ reason: 'hook_stopped' }`).
        // Takes precedence over the `stop_reason`-derived disposition (a step
        // that ran tools never has `stop_reason == "end_turn"`).
        TurnStepOutcome::Ended {
            final_message_id: assistant_id,
            stop_reason: "hook_stopped".to_string(),
        }
    } else {
        match response.stop_reason.as_deref() {
            // #1 needsFollowUp gate (claude-code `query.ts:554-558`, `832-835`,
            // `1062`): continuation is keyed on tool-block PRESENCE, NOT the raw
            // `stop_reason` string — the ref explicitly notes `stop_reason ==
            // "tool_use"` "is unreliable -- it's not always set correctly", so it
            // sets `needsFollowUp = true` whenever the assistant message carried
            // ANY tool_use block (regardless of stop_reason) and `if (!needsFollowUp)`
            // is the SOLE end-vs-continue gate. So a response that dispatched tools
            // but reported a non-`tool_use` stop_reason (e.g. `end_turn`,
            // `stop_sequence`, or a truncated `max_tokens` that still carried a
            // complete tool block) must run the tools AND continue, feeding the
            // tool_results back — NOT end the turn. This leading arm fires only when
            // tools were dispatched (`!tool_uses.is_empty()`); a withheld
            // `max_output_tokens` response carries NO tool_uses, so it falls through
            // to the recovery/terminal arms below unchanged. The common `tool_use`+
            // tools case (previously handled by the `_ => Continue` fallback) is
            // unaffected.
            _ if !tool_uses.is_empty() => TurnStepOutcome::Continue,
            // #77 malformed-tool-use retry (batched twin): `stop_reason ==
            // "tool_use"` but the response produced ZERO tool_use blocks. Only
            // the recovery-aware drivers participate (the per-turn guard lives on
            // `RecoveryState`); the legacy shim (`None`) keeps the historical
            // `_ => Continue` no-op (re-call with no nudge). `tool_uses` is the
            // dispatched set computed above — empty here means a malformed
            // response (`tool_use` stop with no parseable tool_use block).
            Some("tool_use") if recovery.is_some() && tool_uses.is_empty() => {
                let state = recovery.as_deref_mut().expect("recovery is Some");
                handle_malformed_tool_use(orch, assistant_id, state).await?
            }
            // #78 thinking-only nudge (batched twin): an `end_turn` /
            // `stop_sequence` (or absent → treated as `end_turn`) response with
            // no visible text gets ONE nudge before the turn ends. Recovery-aware
            // drivers only; the compact-source exclusion (`a !== "compact" &&
            // !GRe(a)`) is satisfied unconditionally (compaction runs in a
            // separate code path, never this turn step).
            Some("end_turn" | "stop_sequence") | None
                if recovery.as_deref().is_some_and(|s| !s.thinking_only_nudged)
                    && !has_visible_text(&assistant_blocks)
                    && !prior_structured_output =>
            {
                let state = recovery.as_deref_mut().expect("recovery is Some");
                handle_thinking_only(orch, state).await?
            }
            Some("end_turn") => TurnStepOutcome::Ended {
                final_message_id: assistant_id,
                stop_reason: "end_turn".to_string(),
            },
            // A1: max_output_tokens recovery (TS `query.ts:1223-1255`). Only the
            // recovery-aware drivers (`Some(state)`) participate; the legacy shim
            // (`None`) falls through to Continue, unchanged.
            Some("max_tokens") if recovery.is_some() => {
                // `recovery.is_some()` guarded above — unwrap is infallible.
                let state = recovery.expect("recovery is Some");
                handle_max_output_tokens(orch, assistant_id, state).await?
            }
            // Finding #80 (batched twin, claude-code `bin/claude.exe` offset
            // ~205871579): a `refusal` response swaps to the configured
            // `refusalFallbackModel` ONCE per session, warns the user, and Continues
            // (the next step re-snapshots `session.model`, so it re-issues against
            // the fallback). When no fallback is configured (or the latch is already
            // set), the helper returns `false` and this falls through to the
            // historical `_ => Continue` bare re-call — byte-identical to before.
            Some("refusal") if orch.maybe_swap_to_refusal_fallback().await => {
                TurnStepOutcome::Continue
            }
            // Terminal error stop_reasons (batched twin of the streaming
            // `Some(other)` arm, claude.ts:2266/2279): surface the byte-locked
            // `API Error: …` assistant message and END the turn. Previously these
            // fell through to `_ => Continue` and bare-re-called the API, never
            // surfacing the error — the #24 batched-path gap. `model_context_window_exceeded`
            // and a terminal `refusal` (reached only when no `refusalFallbackModel`
            // is configured / the once-per-session latch is set, so the swap arm
            // above did not `continue`) both end here. `max_tokens` (recovery
            // exhausted) is surfaced inside `handle_max_output_tokens`.
            Some(other @ ("model_context_window_exceeded" | "refusal")) => {
                // Pass the response's refusal `stop_details` so the cyber/bio
                // variant fires (no-op for model_context_window_exceeded).
                let surfaced_id =
                    surface_terminal_api_error(orch, other, response.stop_details.as_ref()).await;
                TurnStepOutcome::Ended {
                    final_message_id: surfaced_id.unwrap_or(assistant_id),
                    stop_reason: other.to_string(),
                }
            }
            _ => TurnStepOutcome::Continue,
        }
    };
    Ok((outcome, output_tokens))
}

/// Outcome of [`call_api_with_ptl_recovery`]: either a successful
/// `LlmResponse`, or a signal that the prompt-too-long reactive recovery
/// (Batch 5) was exhausted and the turn should end with the byte-exact
/// [`PROMPT_TOO_LONG_ERROR_MESSAGE`].
pub(crate) enum PtlCallOutcome {
    /// The API call (or a retry after truncation/compaction) succeeded.
    Response(Box<LlmResponse>),
    /// The PTL retry budget + reactive-compact fallback were all exhausted.
    /// End the turn with terminal reason `"prompt_too_long"` (the REACTIVE
    /// exhaustion path, `query.ts:1175`).
    PromptTooLong,
    /// The PROACTIVE blocking-limit preempt fired: the prompt was already at
    /// the hard blocking limit (`token_usage >= effective_window −
    /// MANUAL_COMPACT_BUFFER_TOKENS`) BEFORE the call, so the turn ends with
    /// the DISTINCT terminal reason `"blocking_limit"` — not the reactive
    /// `"prompt_too_long"`. The binary keeps these two terminals separate
    /// (`bin/claude.exe` offset ~208021400: the proactive arm returns
    /// `{reason:"blocking_limit"}` while the reactive arm returns
    /// `{reason:"prompt_too_long"}`; the terminal-reason enum lists both).
    BlockingLimit,
    /// #54: the rapid-refill (thrashing) breaker tripped on the reactive PTL
    /// path — re-compacting cannot help, so surface the byte-exact thrashing
    /// message and end the turn with `reason:"rapid_refill_breaker"`
    /// (`bin/claude.exe` offset 202942256).
    RapidRefillBreaker,
}

/// Wrap the batched `messages_create` with the 413 / prompt-too-long reactive
/// recovery loop (In-Loop Compaction Batch 5, BATCHED path only).
///
/// TS refs: `query.ts:628-648` (blocking-limit preempt),
/// `compact.ts:227-291` (`truncateHeadForPTLRetry`, `MAX_PTL_RETRIES`),
/// `compact.ts:450-491` (the PTL retry loop), `query.ts:1070-1183` (the
/// reactive recovery after 413 — feature-gated, treated as fallback semantics).
///
/// Flow:
/// 1. **Blocking-limit preempt**: estimate tokens on the pre-call history; if
///    the prompt is already at the hard blocking limit
///    ([`compaction::calculate_token_warning_state`]`.is_at_blocking_limit`,
///    i.e. `effective_window − MANUAL_COMPACT_BUFFER_TOKENS`), surface
///    `PromptTooLong` WITHOUT calling the API.
/// 2. Call the API. On `Ok` → `Response`. On a non-PTL `Err` → bubble.
/// 3. On `Err(LlmError::ContextOverflow)` run a PTL retry loop
///    (≤ [`compaction::MAX_PTL_RETRIES`]):
///    [`compaction::ptl_retry::truncate_head_for_ptl_retry`]`(history, gap)` →
///    if `Some`, swap `session.history`, retry; if `None`, break (nothing safe
///    to drop).
/// 4. On loop exhaustion, attempt ONE reactive full compact
///    (`process_iteration_tracked` + [`ConversationOrchestrator::apply_post_compact`])
///    and retry once more. If that STILL returns `PromptTooLong`, return
///    `PromptTooLong` (the caller ends the turn).
///
/// NOT A PARITY GAP (codex finding #4 REFUTED, 2026-06-23): "PTL-truncate ×N →
/// one full compact → error" IS claude-code's DEFAULT. The fuller multi-stage
/// recovery (`contextCollapse.recoverFromOverflow` / `reactiveCompact.
/// tryReactiveCompact`) is behind default-OFF gates: `feature('CONTEXT_COLLAPSE')`
/// (absent from `FEATURE_FLAGS`, always `false`) and `feature('REACTIVE_COMPACT')`
/// (`CLAUDE_CODE_REACTIVE_COMPACT`, opt-in). v2.1.186 binary: `recoverFromOverflow`
/// /`tryReactiveCompact`/`isContextCollapseEnabled` are 0-hit (DCE'd). Porting it
/// would DIVERGE. Evidence: memory `mainloop-parity-2026-06-23`. `betas` for the
#[allow(clippy::too_many_lines)]
pub(crate) async fn call_api_with_ptl_recovery(
    orch: &ConversationOrchestrator,
    system: Option<&str>,
    model: &str,
    profile: Option<&str>,
    history_snapshot: Vec<ConversationMessage>,
    tools: Vec<serde_json::Value>,
    max_tokens_override: Option<u32>,
    // This step's transient deferred-tool delta. Like the date-change reminder,
    // it must be reattached when a retry rebuilds from raw session history.
    deferred_tools_reminder: Option<ConversationMessage>,
    // This step's transient `date_change` reminder (first on desktop, directly
    // after the fixed runtime snapshot on mobile). Retry/fallback paths rebuild
    // from raw `session.history`, so it is reattached there too.
    date_change_reminder: Option<ConversationMessage>,
    // This step's per-turn transient reminders (skill listing, conditional
    // rules, nested memory, diagnostics, …), already appended to
    // `history_snapshot`. Computing them ADVANCES session state — sent-sets,
    // delta trackers, consume-once drains — so they can never be recomputed for
    // a retry; recomputing returns `None` and the reminder is lost for the rest
    // of the session. Re-appended below wherever the request is rebuilt from
    // raw `session.history`.
    turn_reminders: &[ConversationMessage],
) -> Result<PtlCallOutcome, OrchestratorError> {
    // (1) Blocking-limit preempt. `is_at_blocking_limit` is
    // `token_usage >= effective_window − MANUAL_COMPACT_BUFFER_TOKENS`
    // (`autoCompact.ts` `calculateTokenWarningState`). `auto_compact_enabled`
    // is `true` to mirror the always-on default of this port (no GrowthBook).
    let estimate = compaction::grouping::estimate_tokens_for_range(&history_snapshot);
    let active_betas = orch.api.active_betas();
    let warning = compaction::calculate_token_warning_state(estimate, model, &active_betas, true);

    // Push the live context-pressure banner to the UI — the orchestrator-side
    // twin of claude-code's `<TokenWarning>` render
    // (`PromptInput/Notifications.tsx:321`), which recomputes
    // `calculateTokenWarningState` as `tokenUsage` grows. We reuse the SAME
    // `estimate` the auto-compact gate uses (claude-code's `tokenUsage`), so the
    // banner's thresholds match the gate exactly. `None` clears a previously
    // shown banner once the context drops back below the warning threshold
    // (e.g. after a compaction). Default no-op for non-interactive sinks.
    let banner = compaction::token_warning_banner(
        &warning,
        compaction::thresholds::is_auto_compact_enabled(true),
        compaction::is_compact_warning_suppressed(),
        None,
    )
    .map(|b| traits::ContextPressureBanner {
        text: b.text,
        level: match b.color {
            compaction::TokenWarningColor::Dim => traits::ContextPressureLevel::Dim,
            compaction::TokenWarningColor::Warning => traits::ContextPressureLevel::Warning,
            compaction::TokenWarningColor::Error => traits::ContextPressureLevel::Error,
        },
    });
    // Context usage as a 0-1 fraction of the model's effective context window
    // (claude-code `calculateContextPercentages(currentUsage, contextWindowSize)`),
    // emitted every turn — even when no warning banner shows — so the custom
    // statusline's `context_window.used_percentage` is always live. Reuses the
    // SAME `estimate` and active request betas as the banner/auto-compact gate.
    let context_window =
        compaction::thresholds::effective_context_window_size(model, &active_betas);
    let used_fraction = if context_window == 0 {
        0.0
    } else {
        (estimate as f64 / context_window as f64) as f32
    };
    orch.output
        .emit_context_pressure(banner, used_fraction, estimate, context_window)
        .await;

    if warning.is_at_blocking_limit {
        tracing::warn!(
            estimate,
            model,
            "prompt at blocking limit — preempting before API call"
        );
        // PROACTIVE preempt ⇒ terminal reason `"blocking_limit"` (distinct from
        // the reactive-exhausted `PromptTooLong` returned at the tail). No
        // request is issued, so the `date_change` reminder stays UNCOMMITTED and
        // the next step re-emits it.
        return Ok(PtlCallOutcome::BlockingLimit);
    }
    // Past the preempt: the snapshot WILL be sent, so this step's `date_change`
    // reminder counts as delivered.
    orch.commit_date_change_reminder();

    // (2) Initial call. When an Opus-fallback model is configured, route the
    // primary request through the fallback-aware seam. In Task 6, `LlmError`
    // has no `FallbackTriggered` variant — fallback becomes adapter-internal.
    // The `messages_create_with_fallback` seam still passes the fallback hint to
    // `ProviderApiAdapter`, which handles the 529-triggered switch internally.
    // With NO fallback configured the plain `messages_create` seam is taken,
    // byte-identical to before — locked turn-loop fixtures are unaffected.
    // Context-hint negotiation (oracle `e1y`): offer the server a compact we
    // could perform, and act on a 422/424 asking us to. `None` unless BOTH the
    // route allows first-party betas and the controller's own env gate is on —
    // and the latter is off by default because the oracle's server-delivered
    // `tengu_hazel_osprey` is false. So this is inert on every ordinary turn.
    //
    // `repl_main_thread` is this driver by definition: `call_api_with_ptl_recovery`
    // is the MAIN turn's API seam. Subagents and side queries run their own
    // paths and never reach here, which is what the oracle's querySource prefix
    // check expresses.
    let mut hint_controller = compaction::context_hint::create_context_hint_controller(
        orch.config.include_first_party_betas,
        "repl_main_thread",
    );
    let hint_params = hint_controller
        .as_mut()
        .and_then(|c| c.build_request_params(&history_snapshot));

    let first = if let Some(params) = hint_params {
        // The controller is live: take the hint-carrying seam. `params.body` is
        // `None` when the estimated savings are under the floor — the oracle
        // still sends the beta in that case and omits only the body.
        orch.api
            .messages_create_with_context_hint(
                model,
                profile,
                system,
                history_snapshot,
                tools.clone(),
                params.body,
            )
            .await
    } else if let Some(max_tokens) = max_tokens_override {
        // REC.A1 escalated single-shot (TS `query.ts:1199-1221`): re-issue with
        // the override `max_tokens` (8k→64k). The escalation is orthogonal to the
        // Opus-fallback gate, so it takes the plain `_with_opts` seam regardless
        // of `fallback_model`. The no-override branches below are byte-identical
        // to before, so the locked turn-loop fixtures (which never arm an
        // override) are unaffected.
        orch.api
            .messages_create_with_opts(
                model,
                profile,
                system,
                history_snapshot,
                tools.clone(),
                max_tokens,
            )
            .await
    } else if orch.config.fallback_model.is_some() {
        orch.api
            .messages_create_with_fallback(
                model,
                profile,
                system,
                history_snapshot,
                tools.clone(),
                orch.config.fallback_model.as_deref(),
                orch.config.is_subscriber,
                orch.config.is_enterprise,
            )
            .await
    } else {
        orch.api
            .messages_create(model, profile, system, history_snapshot, tools.clone())
            .await
    };
    // NOTE: `ApiError::FallbackTriggered` interception is REMOVED — `LlmError`
    // has no `FallbackTriggered` variant. The model-fallback logic moves into
    // `ProviderApiAdapter` in Task 6 (the adapter handles the 529 switch
    // internally and falls back silently without emitting a separate warning).

    // Map `LlmError::ContextOverflow` to the PTL recovery path.
    // The `token_gap` field carries the actual-minus-limit count parsed from the
    // provider error message by `llm_client`; the PTL truncator treats `0` as
    // "unknown" and falls back to its 20% heuristic.
    let token_gap: u64 = match first {
        Ok(resp) => return Ok(PtlCallOutcome::Response(Box::new(resp))),
        Err(LlmError::ContextOverflow { token_gap }) => token_gap,
        Err(other) => {
            // Context-hint error half (oracle `onRequestError`). A 422/424 is
            // the server asking for the compact we offered: apply the edits and
            // re-issue ONCE. Every other outcome (beta unsupported, 409, 529)
            // falls through to the normal error return, exactly as the oracle
            // does — those branches edit nothing.
            //
            // The status is recoverable because the decoder writes it into the
            // message (`providers::api_error_message`); before that, a 422 and a
            // 400 were the same `LlmError`.
            if let Some(c) = hint_controller.as_mut() {
                let facts = compaction::context_hint::HttpErrorFacts::from_error(&other);
                // Re-snapshot rather than clone the history up front: the
                // snapshot was moved into the call, and every other recovery
                // path here rebuilds the same way.
                let history = {
                    let s = orch.session.lock().await;
                    s.history.clone()
                };
                if let compaction::context_hint::HintErrorOutcome::Reject(edits, _event) =
                    c.on_request_error(&facts, history)
                {
                    let mut retry = edits.messages.clone();
                    {
                        let mut s = orch.session.lock().await;
                        s.history.clone_from(&edits.messages);
                    }
                    orch.reattach_outgoing_context(
                        &mut retry,
                        deferred_tools_reminder.as_ref(),
                        date_change_reminder.as_ref(),
                        turn_reminders,
                    )
                    .await;
                    return match orch
                        .api
                        .messages_create(model, profile, system, retry, tools.clone())
                        .await
                    {
                        Ok(resp) => Ok(PtlCallOutcome::Response(Box::new(resp))),
                        Err(e) => Err(e.into()),
                    };
                }
            }
            return Err(other.into());
        }
    };

    // (3) PTL retry loop: drop oldest API-round groups and retry, ≤ MAX retries.
    for _attempt in 0..compaction::MAX_PTL_RETRIES {
        // Snapshot the current (possibly already-truncated) history.
        let history = {
            let s = orch.session.lock().await;
            s.history.clone()
        };
        let Some(mut truncated) =
            compaction::ptl_retry::truncate_head_for_ptl_retry(history, token_gap)
        else {
            // Nothing safe to drop (< 2 groups). Stop truncating and fall
            // through to the reactive-compact fallback.
            break;
        };
        {
            let mut s = orch.session.lock().await;
            s.history.clone_from(&truncated);
        }
        orch.reattach_outgoing_context(
            &mut truncated,
            deferred_tools_reminder.as_ref(),
            date_change_reminder.as_ref(),
            turn_reminders,
        )
        .await;
        match orch
            .api
            .messages_create(model, profile, system, truncated, tools.clone())
            .await
        {
            Ok(resp) => return Ok(PtlCallOutcome::Response(Box::new(resp))),
            Err(LlmError::ContextOverflow { .. }) => {
                // token_gap not used in the inner loop — truncation keeps halving.
            }
            Err(other) => return Err(other.into()),
        }
    }

    // (4) Reactive-compact fallback: one full compact, then retry once more.
    if let Some(compactor) = orch.compaction.clone() {
        let (snapshot, last_assistant_at) = {
            let s = orch.session.lock().await;
            (s.history.clone(), s.message_timing.last_assistant_at)
        };
        let messages_before = u32::try_from(snapshot.len()).unwrap_or(u32::MAX);
        let bytes_before: u64 = snapshot.iter().map(protocol::text_byte_size).sum();
        // Capture the token estimate before  is consumed by
        //  — used for the boundary `preTokens`.
        let pre_tokens_estimate = compaction::grouping::estimate_tokens_for_range(&snapshot);
        // hooks compaction lifecycle: PreCompact fires before the reactive
        // summary pass. The reactive 413/PTL fallback is part of the automatic
        // recovery pipeline, so the trigger is `auto` (TS treats reactive
        // overflow recovery as a non-manual compact). TS reactive arm: a
        // blocking PreCompact hook logs `Reactive compact blocked by PreCompact
        // hook: <blockedBy>` and aborts recovery — with no compaction the prompt
        // is still over the limit, so we surface the prompt-too-long outcome
        // (the same value this fn falls through to).
        let compact_started = std::time::Instant::now();
        let pre_compact = orch.fire_pre_compact("auto", None).await;
        if let Some(detail) = pre_compact.blocked_by {
            tracing::warn!("Reactive compact blocked by PreCompact hook: {detail}");
            return Ok(PtlCallOutcome::PromptTooLong);
        }
        orch.output.emit_compaction_started().await;
        // API duration = the summarizer pass only; `compact_started` (above,
        // pre-hooks) is the boundary durationMs clock. Folding hook wall-time
        // into `record_compaction_usage` would inflate /cost's API duration.
        let api_started = std::time::Instant::now();
        let compact_result = {
            let mut tracking = orch.compaction_tracking.lock().await;
            compactor
                .process_iteration_tracked_with_instructions_and_timing(
                    snapshot,
                    0,
                    &mut tracking,
                    pre_compact.additional_instructions.as_deref(),
                    last_assistant_at,
                    std::time::SystemTime::now(),
                )
                .await
        };
        if let Ok(result) = compact_result {
            let compact_duration = api_started.elapsed();
            orch.record_compaction_usage(&result, compact_duration)
                .await;
            // #54 reactive rapid-refill (thrashing) breaker: if the reactive
            // compact tripped the breaker, re-compacting cannot help (a single
            // file/tool output is too large). Emit telemetry + surface the
            // byte-exact thrashing message and end the turn — mirroring the
            // binary's reactive arm (`bin/claude.exe` offset 202942256).
            if result.rapid_refill_breaker_tripped {
                let turns_since = {
                    let tracking = orch.compaction_tracking.lock().await;
                    i64::from(tracking.turn_counter)
                };
                orch.fire_rapid_refill_breaker_telemetry_reactive(
                    result.consecutive_rapid_refills,
                    turns_since,
                )
                .await;
                return Ok(PtlCallOutcome::RapidRefillBreaker);
            }
            if result.was_compacted {
                // hooks compaction lifecycle: capture the PostCompact payload
                // BEFORE `apply_post_compact` consumes the result.
                let summary = ConversationOrchestrator::compaction_summary_text(&result);
                let tokens_freed = result.total_tokens_freed;
                // Apply the post-compact transition (history swap + boundary
                // marker + CompactionCompleted) via the shared helper.
                // `cancel: None` — the reactive PTL fallback has no
                // user-cancellable surface, so the apply is infallible.
                orch.apply_post_compact(
                    result,
                    compaction::CompactTrigger::Auto,
                    pre_tokens_estimate,
                    messages_before,
                    bytes_before,
                    compact_started,
                    None,
                )
                .await;
                // PostCompact fires AFTER the transition is applied.
                orch.fire_post_compact("auto", summary, tokens_freed).await;
                let mut history = {
                    let s = orch.session.lock().await;
                    s.history.clone()
                };
                orch.reattach_outgoing_context(
                    &mut history,
                    deferred_tools_reminder.as_ref(),
                    date_change_reminder.as_ref(),
                    turn_reminders,
                )
                .await;
                match orch
                    .api
                    .messages_create(model, profile, system, history, tools)
                    .await
                {
                    Ok(resp) => return Ok(PtlCallOutcome::Response(Box::new(resp))),
                    Err(LlmError::ContextOverflow { .. }) => {}
                    Err(other) => return Err(other.into()),
                }
            }
        }
    }

    // Still over the limit after truncation + one reactive compact: surface the
    // byte-exact prompt-too-long message and end the turn (no hard error).
    Ok(PtlCallOutcome::PromptTooLong)
}

/// Port of the Opus-fallback re-issue (claude-code `query.ts:894-948`'s
/// `catch (FallbackTriggeredError)` arm). Only reachable when
/// `config.fallback_model.is_some()` (see [`call_api_with_ptl_recovery`]):
///
/// 1. (i) switch `session.model` to `fallback_model` (TS `currentModel =
///    fallbackModel`); the conversation continues on it.
/// 2. (ii) clear in-flight accumulators — STRUCTURAL no-op: history is appended
///    only after success (see [`execute_one_turn_with_recovery_tracked`]).
/// 3. (iii) surface a `warning` on the output stream (TS `createSystemMessage`,
///    same channel as [`surface_prompt_too_long`]) — not pushed to history, as
///    a `role:"system"` entry is rejected by the API.
/// 4. (iv) emit `tengu_model_fallback_triggered` via `tracing` (INLINE name, not
///    a locked const, so the event-name fixture lock holds).
/// 5. (v) re-issue ONE round-trip with `fallback_model = None` (non-Opus → 529
///    gate closed → cannot recurse; TS `continue` re-enters once).
///
/// Bounded divergences: TS also sets `mainLoopModel`, but `main_loop_model`
/// derives from immutable `config.model`; TS's `ant`-gated `stripSignatureBlocks`
/// is unported (no protected-thinking replay).
///
/// NOTE: Task 5 dead code — `FallbackTriggered` interception was removed; this is
/// business logic until then.
#[allow(dead_code)]
async fn reissue_after_model_fallback(
    orch: &ConversationOrchestrator,
    system: Option<&str>,
    original_model: &str,
    fallback_model: String,
    tools: Vec<serde_json::Value>,
) -> Result<LlmResponse, LlmError> {
    // (i) Switch the working/session model to the fallback.
    {
        let mut s = orch.session.lock().await;
        s.model.clone_from(&fallback_model);
    }

    // (ii) Clear in-flight accumulators — structural no-op here (see doc above).

    // (iii) Surface the user-visible warning (byte-shaped on the TS intent;
    // includes both model names).
    let warning = format!("Switched to {fallback_model} due to high demand for {original_model}");
    orch.output.emit_text(&warning).await;

    // (iv) Success-path analytics — inline event name (NOT a locked const).
    tracing::info!(
        event = "tengu_model_fallback_triggered",
        original_model = %original_model,
        fallback_model = %fallback_model,
        entrypoint = "cli",
    );

    // (v) Re-issue ONE round-trip against the fallback model. Re-snapshot the
    // current history (unchanged by steps i–iv). `fallback_model = None` keeps
    // the 529 gate closed → no recursion.
    let history = {
        let s = orch.session.lock().await;
        s.history.clone()
    };
    orch.api
        .messages_create_with_fallback(
            &fallback_model,
            None, // fallback model has no associated profile
            system,
            history,
            tools,
            None,
            orch.config.is_subscriber,
            orch.config.is_enterprise,
        )
        .await
}

/// Append the byte-exact [`PROMPT_TOO_LONG_ERROR_MESSAGE`] as an assistant text
/// message to history (and emit it to the output stream), returning its id so
/// the caller can end the turn. Mirrors the TS path where the prompt-too-long
/// error is surfaced as the assistant turn before the loop terminates.
///
/// `pub(crate)` so the streaming turn driver's RECOV.1 blocking-limit preempt
/// (`conversation.rs`) can surface the same byte-exact message as the batched
/// path before ending the turn.
pub(crate) async fn surface_prompt_too_long(orch: &ConversationOrchestrator) -> MessageId {
    let assistant_id = MessageId::new();
    let assistant_msg = ConversationMessage::Assistant {
        id: assistant_id,
        content: vec![ContentBlock::Text {
            text: PROMPT_TOO_LONG_ERROR_MESSAGE.to_string(),
        }],
        stop_reason: Some("prompt_too_long".to_string()),
    };
    {
        let mut s = orch.session.lock().await;
        s.history.push(assistant_msg.clone());
    }
    // Non-streaming (batched) parity (claude.ts:2571): this surfaces the
    // prompt-too-long assistant turn on the BATCHED path, so persist ONE merged
    // assistant line (here a single text block → one line either way) — the
    // per-block split is streaming-only.
    orch.persist_message_to_jsonl(&assistant_msg).await;
    orch.output.emit_text(PROMPT_TOO_LONG_ERROR_MESSAGE).await;
    assistant_id
}

/// Surface the #54 rapid-refill (thrashing) breaker message on the reactive PTL
/// path and end the turn.
///
/// 1:1 with claude-code v2.1.183 (`bin/claude.exe` offset 202942256): the
/// reactive arm, on `kho(state) >= f6n`, emits the
/// `tengu_auto_compact_rapid_refill_breaker` telemetry and surfaces the
/// byte-exact thrashing message `Rho` as an `invalid_request` assistant error,
/// ending the turn with `reason:"rapid_refill_breaker"`. We surface it on the
/// same channel as [`surface_prompt_too_long`] (a stop-reason-bearing assistant
/// message + emit), so the turn ends cleanly.
pub(crate) async fn surface_rapid_refill_thrashing(orch: &ConversationOrchestrator) -> MessageId {
    let assistant_id = MessageId::new();
    let assistant_msg = ConversationMessage::Assistant {
        id: assistant_id,
        content: vec![ContentBlock::Text {
            text: compaction::RAPID_REFILL_THRASHING_MESSAGE.to_string(),
        }],
        // The binary surfaces this as `error:"invalid_request"`.
        stop_reason: Some("invalid_request".to_string()),
    };
    {
        let mut s = orch.session.lock().await;
        s.history.push(assistant_msg.clone());
    }
    orch.persist_message_to_jsonl(&assistant_msg).await;
    orch.output
        .emit_text(compaction::RAPID_REFILL_THRASHING_MESSAGE)
        .await;
    assistant_id
}

/// Build the user-visible `API Error: …` text claude-code surfaces for the
/// terminal stop_reasons it reports as errors: `max_tokens` (recovery
/// exhausted), `model_context_window_exceeded`, and `refusal`
/// (without a configured fallback). Returns `None` for every other terminal
/// (`stop_sequence` / `pause_turn` / …), which end silently.
///
/// Shared by the streaming ([`ConversationOrchestrator`] turn loop) and batched
/// ([`surface_terminal_api_error`]) terminal arms so both paths surface
/// byte-identical text (claude-code `claude.ts:2266/2279`, `U2e`). The refusal
/// cyber/bio category variant, the `stop_details.explanation` clause, and the
/// `\n\nRequest ID: …` suffix remain residuals on BOTH paths — LingXi does not
/// thread `stop_details`/requestId into the terminal arm, so the non-cyber,
/// no-explanation path (the common terminal) fires.
#[must_use]
pub(crate) fn terminal_api_error_text(
    model: &str,
    interactive: bool,
    stop_reason: &str,
    request_id: Option<&str>,
    stop_details: Option<&llm_client::StopDetails>,
) -> Option<String> {
    match stop_reason {
        "max_tokens" => Some(format!(
            "API Error: Claude's response exceeded the {} output token maximum. To configure this behavior, set the LINGXI_MAX_OUTPUT_TOKENS environment variable.",
            compaction::max_output_tokens_for_model(model)
        )),
        "model_context_window_exceeded" => {
            Some("API Error: The model has reached its context window limit.".to_string())
        }
        "refusal" => {
            // Faithful port of the binary's `U2e` (@197278360): the message is
            // category-aware via `rnt(cat) = cat ∈ {"cyber","bio"}` and
            // `pd() = firstParty` (always true for LingXi's Anthropic path).
            let category = stop_details.and_then(|sd| sd.category.as_deref());
            let cyber_or_bio = matches!(category, Some("cyber" | "bio"));
            let is_cyber = matches!(category, Some("cyber"));
            let base = match crate::prompt::env_meta::marketing_name_for_model(model) {
                Some(label) => {
                    // LABEL branch. `m`/`f` are the interactive suffixes.
                    let m = if interactive {
                        "Double press esc to edit your last message, or try a different model with /model."
                    } else {
                        "Try rephrasing the request in a new session or change your model."
                    };
                    let f = if interactive {
                        "Send feedback with /feedback or learn more: https://support.claude.com/en/articles/15363606"
                    } else {
                        "Learn more: https://support.claude.com/en/articles/15363606"
                    };
                    // `h` (binary `U2e`): the cyber/bio variant (`Jct(cat)=cat∈
                    // {cyber,bio}`) appends `Saa`, the generic one a fixed tail.
                    //   Saa = `They may flag safe, normal content as well. ${elp}`
                    //   elp = `These measures let us bring you Mythos-level
                    //          capabilities sooner, and we're working to refine them.`
                    let a = if cyber_or_bio {
                        format!(
                            "{label}'s safeguards flagged this message (https://www.anthropic.com/legal/aup). They may flag safe, normal content as well. These measures let us bring you Mythos-level capabilities sooner, and we're working to refine them."
                        )
                    } else {
                        format!(
                            "{label}'s safeguards flagged this message (https://www.anthropic.com/legal/aup). This sometimes happens with safe, normal conversations."
                        )
                    };
                    // Frame `c = `${bT}: ${h} <brand> can't respond … with ${l}.\n\n${m}\n\n${f}``
                    // — DOUBLE `\n` separators (od -c verified on 2.1.195 @206804081;
                    // the `strings` dump misled an earlier pass into single `\n`).
                    // `<brand>` is the LingXi rebrand.
                    format!("API Error: {a} LingXi can't respond to this request with {label}.\n\n{m}\n\n{f}")
                }
                None => {
                    // NO-LABEL branch.
                    let m = if interactive {
                        "Please double press esc to edit your last message or start a new session for LingXi to assist with a different task."
                    } else {
                        "Try rephrasing the request in a new session or change your model."
                    };
                    if is_cyber {
                        // 2.1.206 (JS @217943553) replaced the old "apply for an
                        // exemption" message with the Cyber Verification Program
                        // interstitial (`t?.category==="cyber" && Xf()`; Xf() =
                        // the first-party cyber-safeguards gate, always on for the
                        // Anthropic path — the 195 code used the analogous
                        // `pd()`=firstParty). `m2 = n!=null ? Mf(n) : "This model"`
                        // = "This model" here (this is the no-marketing-name
                        // branch). The feedback tail is interactive-only (binary
                        // `g = p ? "" : "\n\nIf you were not…"`); there is NO
                        // `\n\n{m}\n\n{f}` tail — it was dropped in 206.
                        // Byte-verified: 'apply for an exemption' = 0 hits in 206;
                        // help-center URL and interstitial = 2 hits each.
                        let feedback_tail = if interactive {
                            "\n\nIf you were not engaging in a cybersecurity topic, please send feedback via /feedback."
                        } else {
                            ""
                        };
                        format!(
                            "API Error: This model has safety measures that flagged this message for a cybersecurity topic. To learn about the Cyber Verification Program and apply for access, visit our help center: https://support.claude.com/en/articles/14604842-real-time-cyber-safeguards-on-claude.{feedback_tail}"
                        )
                    } else {
                        // Binary final `else`: `${bT}: <brand> is unable to respond
                        // … aup).${a} `+f` — `${a}` is the optional explanation
                        // clause (`a=i?` ${i}${punct}`:""`). Empty ⇒ `). {m}`;
                        // present ⇒ `). <explanation>[.] {m}`. (`f` == the port's
                        // `m` rephrase message.) 2.1.206 REMOVED the
                        // `military_weapons` arm entirely — 0 hits for
                        // 'weapons-related content' / 'military_weapons' in 206.
                        let clause = refusal_explanation_clause(
                            stop_details.and_then(|sd| sd.explanation.as_deref()),
                        );
                        format!(
                            "API Error: LingXi is unable to respond to this request, which appears to violate our Usage Policy (https://www.anthropic.com/legal/aup).{clause} {m}"
                        )
                    }
                }
            };
            // Binary `u = n ? `\n\nRequest ID: ${n}` : ""` — DOUBLE `\n`, appended to
            // `base` only when a request id is present (REFUSAL-ONLY surface).
            // (od -c verified on 2.1.195 @206805081; strings dump misled an earlier
            // pass into single `\n`.)
            let suffix = match request_id {
                Some(id) if !id.is_empty() => format!("\n\nRequest ID: {id}"),
                _ => String::new(),
            };
            Some(format!("{base}{suffix}"))
        }
        _ => None,
    }
}

/// The binary `U2e` explanation clause `${a}`:
///   `let s=400, i = o && o.length>s ? o.slice(0,s).trimEnd()+"…" : o,
///    a = i ? ` ${i}${/[.!?…]$/.test(i)?"":"."}` : ""`
/// where `o` is the refusal explanation. Returns `""` when absent/empty; else a
/// LEADING-space clause ` <explanation>` plus a terminal `.` when it does not
/// already end with `.`/`!`/`?`/`…`. The explanation is truncated (+ `…`) past
/// 400 chars — only then is its tail trimmed (matching `o.length>s` gating the
/// `trimEnd`). The cap is by `char` count (the port's truncation convention; JS
/// uses UTF-16 units — identical for the typical ASCII refusal text).
fn refusal_explanation_clause(explanation: Option<&str>) -> String {
    let Some(o) = explanation.filter(|s| !s.is_empty()) else {
        return String::new();
    };
    const CAP: usize = 400;
    let i = if o.chars().count() > CAP {
        let head: String = o.chars().take(CAP).collect();
        format!("{}\u{2026}", head.trim_end())
    } else {
        o.to_string()
    };
    if i.is_empty() {
        return String::new();
    }
    let ends_punct = i
        .chars()
        .last()
        .is_some_and(|c| matches!(c, '.' | '!' | '?' | '\u{2026}'));
    format!(" {i}{}", if ends_punct { "" } else { "." })
}

/// Surface the terminal `API Error: …` assistant message on the BATCHED path
/// (the streaming twin inlines the same persist+emit before `emit_end_turn`).
///
/// Builds the text via [`terminal_api_error_text`]; when `Some`, pushes a
/// stop-reason-bearing assistant message into history, persists it (the
/// synthetic-envelope JSONL line), and emits the text. The caller still returns
/// [`TurnStepOutcome::Ended`], whose driver fires the end-of-turn bookkeeping
/// (`emit_end_turn`) exactly once — this helper deliberately does NOT emit the
/// end-of-turn marker. Returns `Some(assistant_id)` of the surfaced message, or
/// `None` when `stop_reason` is not one of the three error terminals.
pub(crate) async fn surface_terminal_api_error(
    orch: &ConversationOrchestrator,
    stop_reason: &str,
    stop_details: Option<&llm_client::StopDetails>,
) -> Option<MessageId> {
    let (model, interactive) = {
        let s = orch.session.lock().await;
        (s.model.clone(), orch.prompt_is_interactive())
    };
    // The just-completed call's Anthropic `request-id` — for the refusal
    // message's `\nRequest ID: …` suffix (recorded by the adapter from the
    // response headers; same slot the JSONL `requestId` reads from).
    let request_id = orch.api.last_request_id();
    let text = terminal_api_error_text(
        &model,
        interactive,
        stop_reason,
        request_id.as_deref(),
        stop_details,
    )?;
    let assistant_id = MessageId::new();
    let assistant_msg = ConversationMessage::Assistant {
        id: assistant_id,
        content: vec![ContentBlock::Text { text: text.clone() }],
        stop_reason: Some(stop_reason.to_string()),
    };
    {
        let mut s = orch.session.lock().await;
        s.history.push(assistant_msg.clone());
    }
    // Top-level api-error envelope per builder/stop_reason (verified vs the
    // 2.1.195 binary + on-disk transcripts):
    // - `max_tokens` / `model_context_window_exceeded`: claude-code's
    //   `ql({content,apiError:"max_output_tokens",error:"max_output_tokens"})` →
    //   `error:"max_output_tokens"` (no HTTP status), inner `stop_sequence`.
    // - `refusal`: the `fje("refusal", …)` builder keeps inner
    //   `stop_reason:"refusal"` and tags `error:"invalid_request"` (the sole
    //   on-disk refusal line: `stop_reason:"refusal", error:"invalid_request"`).
    let env = match stop_reason {
        "max_tokens" | "model_context_window_exceeded" => ApiErrorEnvelope {
            error: Some("max_output_tokens"),
            api_error_status: None,
            inner_stop_reason: None,
        },
        "refusal" => ApiErrorEnvelope {
            error: Some("invalid_request"),
            api_error_status: None,
            inner_stop_reason: Some("refusal"),
        },
        // `terminal_api_error_text` returned `Some` only for the three reasons
        // above; any other value can't reach here.
        _ => ApiErrorEnvelope::default(),
    };
    orch.persist_api_error_message_to_jsonl(&assistant_msg, env)
        .await;
    orch.output.emit_text(&text).await;
    Some(assistant_id)
}

/// Whether a turn error has DEDICATED downstream handling and must propagate as
/// a hard `Err` instead of being caught as a graceful `model_error` (#10):
/// - `RateLimited` — the `run_turn*` wrapper re-maps it onto the limits-specific
///   copy + emits the terminal rate-limit snapshot (`enrich_api_error` /
///   `emit_terminal_rate_limit_if_changed`).
/// - `Overloaded` / `RepeatedOverloaded` — the byte-locked "Repeated 529
///   Overloaded errors" surface (`errors.ts:166`).
/// - `PermissionAbort` — the auto-mode denial breaker deliberately terminates
///   a prompt-avoiding agent and must not be converted into `model_error`.
/// Mirrors claude-code, whose top-level `catch` is reached only AFTER the retry
/// layer has handled 429/529; everything else falls through to `model_error`.
#[must_use]
pub(crate) fn is_carveout_propagated(e: &OrchestratorError) -> bool {
    matches!(
        e,
        OrchestratorError::PermissionAbort { .. }
            | OrchestratorError::RepeatedOverloaded
            | OrchestratorError::ApiCall(
                LlmError::RateLimited { .. } | LlmError::Overloaded { .. }
            )
            | OrchestratorError::Streaming(
                LlmError::RateLimited { .. } | LlmError::Overloaded { .. }
            )
    )
}

/// Surface a `model_error` turn-end (port of `query.ts:955-997`'s top-level
/// `catch`). A runtime error that escaped the API layer (not PTL/overflow/rate/
/// overload — those propagate upstream) is NOT a hard failure: log
/// `tengu_query_error`, yield the raw text VERBATIM as an `isApiErrorMessage`
/// assistant (`createAssistantAPIErrorMessage`, no `API Error:` prefix), end with
/// `reason:'model_error'`. Session survives. `yieldMissingToolResultBlocks` is a
/// no-op (history appends only after success); `queryDepth=0` (subagents bypass
/// this orchestrator); per-turn `assistantMessages`/`toolUses` counts omitted.
pub(crate) async fn surface_model_error(
    orch: &ConversationOrchestrator,
    error_text: &str,
    env: ApiErrorEnvelope,
) -> MessageId {
    if let Some(bus) = orch.analytics_bus.as_ref() {
        let mut metadata = telemetry::LogEventMetadata::new();
        metadata.insert(
            "queryChainId".into(),
            telemetry::AnalyticsValue::String(orch.query_chain_id.clone()),
        );
        metadata.insert("queryDepth".into(), telemetry::AnalyticsValue::Int(0));
        bus.log_event("tengu_query_error", metadata).await;
    }
    surface_api_error_notice(orch, error_text, env).await
}

/// Persist + emit an api-error assistant message (the `createAssistantAPIErrorMessage`
/// shape) WITHOUT the `tengu_query_error` telemetry that the top-level `model_error`
/// catch logs. Shared by [`surface_model_error`] and the P1-04 partial-stream
/// finalize notice (cc 2.1.199 yields the incomplete-response notice via `tu(...)`
/// directly, not through the top-level catch — so it does NOT fire `tengu_query_error`).
pub(crate) async fn surface_api_error_notice(
    orch: &ConversationOrchestrator,
    error_text: &str,
    env: ApiErrorEnvelope,
) -> MessageId {
    // `createAssistantAPIErrorMessage({ content })` renders `content` verbatim,
    // falling back to the `NO_CONTENT_MESSAGE` placeholder when empty.
    let text = if error_text.is_empty() {
        "(no content)".to_string()
    } else {
        error_text.to_string()
    };
    let assistant_id = MessageId::new();
    let assistant_msg = ConversationMessage::Assistant {
        id: assistant_id,
        content: vec![ContentBlock::Text { text: text.clone() }],
        stop_reason: Some("model_error".to_string()),
    };
    {
        let mut s = orch.session.lock().await;
        s.history.push(assistant_msg.clone());
    }
    // The top-level `model_error` catch builds the assistant line via
    // `createAssistantAPIErrorMessage({content})` (content verbatim, inner
    // `stop_reason` stays `"stop_sequence"`). The top-level api-error envelope
    // — `error` category + optional `apiErrorStatus` — is computed by the
    // per-request classifier (`Flp`/`KNn`, ported as
    // [`crate::conversation::classify_api_error`]) at the call site from the
    // TYPED error and passed in here (the classifier deferral is now CLOSED).
    orch.persist_api_error_message_to_jsonl(&assistant_msg, env)
        .await;
    orch.output.emit_text(&text).await;
    assistant_id
}

/// A1 `max_tokens` recovery decision (TS `query.ts:1223-1255`).
///
/// While `count < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT`: append the byte-exact
/// meta nudge user message to history, increment the counter, and Continue.
/// On exhaustion (count has reached the limit): end the turn with
/// `stop_reason = "max_tokens"` (current behavior — surface the cap).
///
/// The 8k→64k escalation (TS `query.ts:1199-1221`) fires FIRST when
/// [`crate::OrchestratorConfig::escalate_max_output_tokens`] is on and it has
/// not yet fired this episode: it arms the override and returns `Continue` so
/// the SAME step re-issues once at [`ESCALATED_MAX_TOKENS`] with no nudge.
async fn handle_max_output_tokens(
    orch: &ConversationOrchestrator,
    assistant_id: MessageId,
    state: &mut RecoveryState,
) -> Result<TurnStepOutcome, OrchestratorError> {
    // REC.A1 escalation (8k→64k). TS (`query.ts:1199-1221`) does a single-shot
    // retry at the escalated cap BEFORE the multi-turn nudge, gated on
    // `tengu_otk_slot_v1` (here `escalate_max_output_tokens`) and "not already
    // escalated". We arm `max_output_tokens_override` — which the next
    // `execute_one_turn_with_recovery_tracked` TAKEs and passes to
    // `messages_create_with_opts` — and return `Continue` so the same step
    // re-issues at 64k with NO nudge injected. The override is taken per call,
    // so a separate `max_output_tokens_escalated` flag (reset alongside the
    // recovery count) gates this to once per episode and prevents an
    // escalate-forever loop when 64k also overflows.
    if orch.config.escalate_max_output_tokens && !state.max_output_tokens_escalated {
        state.max_output_tokens_override = Some(ESCALATED_MAX_TOKENS);
        state.max_output_tokens_escalated = true;
        return Ok(TurnStepOutcome::Continue);
    }

    if state.max_output_tokens_recovery_count < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT {
        // Inject the "resume directly" nudge as a META user message. CC 2.1.207
        // builds it via `createUserMessage({…, isMeta:!0})`, so it persists with
        // top-level `isMeta:true` and is skipped by title / first-prompt /
        // visible-count extraction.
        let nudge_msg = ConversationMessage::user_meta(
            MessageId::new(),
            MAX_OUTPUT_TOKENS_RECOVERY_NUDGE.to_string(),
        );
        {
            let mut s = orch.session.lock().await;
            s.history.push(nudge_msg.clone());
        }
        orch.persist_message_to_jsonl(&nudge_msg).await;

        state.max_output_tokens_recovery_count =
            state.max_output_tokens_recovery_count.saturating_add(1);
        // A clean nudge retry never carries an escalated override forward
        // (TS sets `maxOutputTokensOverride: undefined` here).
        state.max_output_tokens_override = None;
        return Ok(TurnStepOutcome::Continue);
    }

    // Recovery exhausted — surface the byte-locked `API Error: …` cap message
    // (the streaming twin does this in its terminal arm), then end the turn.
    let surfaced_id = surface_terminal_api_error(orch, "max_tokens", None).await;
    Ok(TurnStepOutcome::Ended {
        final_message_id: surfaced_id.unwrap_or(assistant_id),
        stop_reason: "max_tokens".to_string(),
    })
}

/// Whether any assistant content block is a non-whitespace [`ContentBlock::Text`]
/// — the #78 "visible output" predicate (claude-code `bin/claude.exe` offset
/// ~202946760). `false` = a thinking-only / text-empty response.
fn has_visible_text(blocks: &[ContentBlock]) -> bool {
    blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Text { text } if !text.trim().is_empty()))
}

/// `StructuredOutput` tool name (claude-code `bp`). It is the only tool that
/// sets the `endsTurn`/`toolEndsTurn` flag, so detecting its `tool_use` by name
/// is equivalent to the binary's `name===bp` check.
pub(crate) const STRUCTURED_OUTPUT_TOOL_NAME: &str = "StructuredOutput";

/// Port of claude-code's `Pt(ce)` (query module, `bin/claude.exe` offset
/// ~209123866): scanning the message history backward, return `true` when the
/// most recent assistant carried a `StructuredOutput` `tool_use` BEFORE any real
/// user turn. Meta user messages and tool-result-carrier user messages
/// (`Jde(e)` = a `user` message whose content array holds a `tool_result`) are
/// skipped; a real user message short-circuits to `false`.
///
/// Used as the `!Pt(ce)` guard on the #78 thinking-only nudge: in a
/// structured-output exchange the model's post-`StructuredOutput` `end_turn`
/// legitimately carries no visible text, so the "[Your previous response had no
/// visible output…]" nudge must NOT fire.
#[must_use]
pub(crate) fn prior_assistant_used_structured_output(history: &[ConversationMessage]) -> bool {
    for msg in history.iter().rev() {
        match msg.role() {
            protocol::MessageRole::User => {
                // `if(Sn.isMeta||Jde(Sn))continue; return!1`
                if msg.is_meta() || is_tool_result_carrier(msg) {
                    continue;
                }
                return false;
            }
            protocol::MessageRole::Assistant => {
                // `Sn.message.content.some(b=>b.type==="tool_use"&&b.name===bp)`
                if msg.tool_calls().iter().any(|b| {
                    matches!(b, ContentBlock::ToolUse { name, .. } if name == STRUCTURED_OUTPUT_TOOL_NAME)
                }) {
                    return true;
                }
            }
            // `if(Sn.type!=="assistant")continue` — system / other lines skipped.
            protocol::MessageRole::System => continue,
        }
    }
    false
}

/// claude-code `Jde(e)`: a `user` message whose content array contains any
/// `tool_result` block (a synthetic tool-result-carrier turn, not a real human
/// turn).
fn is_tool_result_carrier(msg: &ConversationMessage) -> bool {
    matches!(
        msg,
        ConversationMessage::User { content, .. }
            if content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }))
    )
}

/// #77 (batched twin, claude-code `bin/claude.exe` offset ~202945837): handle a
/// `stop_reason == "tool_use"` response that produced ZERO `tool_use` blocks. On
/// the FIRST failure inject the byte-exact meta retry nudge, reset the
/// max-output-tokens recovery bookkeeping, arm the per-turn guard, and Continue.
/// On the SECOND, surface the non-meta terminal message and end the turn as
/// completed (`stop_reason = "end_turn"`).
async fn handle_malformed_tool_use(
    orch: &ConversationOrchestrator,
    _assistant_id: MessageId,
    state: &mut RecoveryState,
) -> Result<TurnStepOutcome, OrchestratorError> {
    if state.malformed_tool_use_retried {
        // Second failure → terminal NON-meta message, complete the turn. The
        // binary builds this via `ql(...)`→`mcc({isApiErrorMessage:!0})`, i.e. an
        // ASSISTANT api-error message (`role:"assistant", stop_reason:
        // "stop_sequence", stop_details:null`) appended AFTER the malformed
        // assistant response — two assistant messages in a row, matching the
        // binary. (The port previously persisted a USER message here.) Shape
        // mirrors `surface_model_error`'s assistant-api-error message.
        orch.output.emit_text(MALFORMED_TOOL_USE_RETRY_FAILED).await;
        let failed_msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::Text {
                text: MALFORMED_TOOL_USE_RETRY_FAILED.to_string(),
            }],
            stop_reason: Some("stop_sequence".to_string()),
        };
        {
            let mut s = orch.session.lock().await;
            s.history.push(failed_msg.clone());
        }
        // claude-code builds the terminal via `ql({content})` with no `error:`
        // arg → `isApiErrorMessage: true`, `error`/`apiErrorStatus` OMITTED,
        // inner `stop_reason:"stop_sequence"`.
        orch.persist_api_error_message_to_jsonl(&failed_msg, ApiErrorEnvelope::default())
            .await;
        return Ok(TurnStepOutcome::Ended {
            final_message_id: failed_msg.id(),
            stop_reason: "end_turn".to_string(),
        });
    }
    let nudge_msg = ConversationMessage::user_meta(
        MessageId::new(),
        MALFORMED_TOOL_USE_RETRY_NUDGE.to_string(),
    );
    {
        let mut s = orch.session.lock().await;
        s.history.push(nudge_msg.clone());
    }
    orch.persist_message_to_jsonl(&nudge_msg).await;
    // TS resets the recovery counters on the retry transition.
    state.reset_max_output_tokens_recovery();
    state.malformed_tool_use_retried = true;
    Ok(TurnStepOutcome::Continue)
}

/// #78 (batched twin, claude-code `bin/claude.exe` offset ~202946760): inject the
/// once-per-turn thinking-only nudge as a meta user message and Continue. Caller
/// has already checked `!thinking_only_nudged && !has_visible_text(..)`.
async fn handle_thinking_only(
    orch: &ConversationOrchestrator,
    state: &mut RecoveryState,
) -> Result<TurnStepOutcome, OrchestratorError> {
    let nudge_msg =
        ConversationMessage::user_meta(MessageId::new(), THINKING_ONLY_NUDGE.to_string());
    {
        let mut s = orch.session.lock().await;
        s.history.push(nudge_msg.clone());
    }
    orch.persist_message_to_jsonl(&nudge_msg).await;
    state.thinking_only_nudged = true;
    Ok(TurnStepOutcome::Continue)
}

/// Translate llm-client content blocks into protocol content blocks.
/// Server-side variants (`RedactedThinking`, `ServerToolUse`, `ConnectorText`,
/// `AdvisorToolResult`) are PRESERVED verbatim (not dropped) so resume/replay
/// JSONL bytes stay intact when the protected-thinking/advisor/connector betas
/// are active. `ToolCall.id: String` becomes the canonical String-backed
/// `ToolUseId` directly (the provider id IS the id; no UUID round-trip), so
/// JSONL/resume bytes match upstream claude-code. Input-only variants
/// (`Image`/`ImageUrl`/`Document`/…) remain dropped on the response path.
#[must_use]
pub(crate) fn translate_response_blocks(content: &[LlmContentBlock]) -> Vec<ContentBlock> {
    use protocol::ToolUseId;
    content
        .iter()
        .filter_map(|b| match b {
            LlmContentBlock::Text { text, .. } => Some(ContentBlock::Text { text: text.clone() }),
            LlmContentBlock::ToolCall { id, name, input } => {
                // The provider-issued id (e.g. Anthropic `toolu_…`, OpenAI
                // `call_…`) IS the canonical `ToolUseId`, so JSONL/resume bytes
                // match upstream claude-code. The `provider_id` sidecar is left
                // `None` (vestigial) — the id already carries the canonical value.
                //
                // (cc 2.1.218 `jYd`) Repair literal `\uXXXX` TEXT the model
                // emitted instead of real characters, before the input is stored
                // or dispatched — otherwise `Edit.old_string` never matches and
                // paths don't resolve. Windows paths and genuinely-escaped
                // sequences are left verbatim; `Workflow.script` is restored.
                let (input, _stats) =
                    llm_client::unicode_repair::repair_tool_input(name, input);
                Some(ContentBlock::ToolUse {
                    id: ToolUseId::from(id.clone()),
                    name: name.clone(),
                    input,
                    provider_id: None,
                })
            }
            LlmContentBlock::Reasoning { text, signature } => Some(ContentBlock::Thinking {
                thinking: text.clone(),
                signature: signature.clone(),
            }),
            // Low-frequency server-side blocks: PRESERVED verbatim so resume/replay
            // JSONL bytes stay intact when protected-thinking/advisor/connector
            // betas are active (matches agent::runner::translate_response_blocks).
            // Output-only — claude-code keeps them; non-streaming twin of the
            // streaming `event_router`.
            LlmContentBlock::RedactedThinking { data } => {
                Some(ContentBlock::RedactedThinking { data: data.clone() })
            }
            LlmContentBlock::ServerToolUse { id, name, input } => {
                Some(ContentBlock::ServerToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                })
            }
            LlmContentBlock::ConnectorText {
                connector_text,
                signature,
            } => Some(ContentBlock::ConnectorText {
                connector_text: connector_text.clone(),
                signature: signature.clone(),
            }),
            LlmContentBlock::AdvisorToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some(ContentBlock::AdvisorToolResult {
                tool_use_id: tool_use_id.clone(),
                content: content.clone(),
                is_error: *is_error,
            }),
            // Input-only / non-output variants remain dropped on the response path.
            LlmContentBlock::Image { .. }
            | LlmContentBlock::ImageUrl { .. }
            | LlmContentBlock::Document { .. }
            | LlmContentBlock::ToolResult { .. }
            // cache_edits is a request-only directive — never in a response.
            | LlmContentBlock::CacheEdits { .. } => None,
        })
        .collect()
}

/// Fold a tool result's `structuredPatch` (if any) into the session's
/// cumulative code-change counters (claude-code `Bhn(added, removed)`,
/// surfaced by `/usage`). Only file-edit tools (Edit/Write/MultiEdit) put a
/// `structuredPatch` array in their result data; every other tool's payload
/// lacks the key, so this is a no-op for them. Also a no-op when no
/// `cost_tracker` is wired (M6-06 default `None`).
pub(crate) async fn accumulate_code_change(
    emit_payload: &serde_json::Value,
    tracker: Option<&std::sync::Arc<cost::CostTracker>>,
) {
    let Some(sp) = emit_payload.get("structuredPatch") else {
        return;
    };
    let (added, removed) = crate::cost_lines::count_structured_patch_lines(sp);
    if added > 0 || removed > 0 {
        if let Some(tracker) = tracker {
            tracker.record_code_change(added, removed).await;
        }
    }
}

/// Dispatch each `tool_use` block through hooks -> permission -> registry ->
/// hooks. Returns a list of `ContentBlock::ToolResult` blocks for the
/// next user message.
///
/// Test-only thin wrapper over [`dispatch_tool_uses_tracked`] that returns just
/// the `ContentBlock` results (dropping the `prevent_continuation` and injected
/// `new_messages` tuple elements) for the in-file tests' convenience.
///
/// The production streaming path
/// ([`crate::streaming_executor::StreamingToolExecutor`]) calls
/// `dispatch_tool_uses_tracked` per tool, so it DOES replay tool-injected
/// `new_messages` (the Skill tool's expanded prompt) into history after the
/// `tool_result` — mirroring the batched [`execute_one_turn`] path (SKILLEXEC.3).
#[cfg(test)]
pub(crate) async fn dispatch_tool_uses(
    orch: &ConversationOrchestrator,
    tool_uses: &[(ToolUseId, String, serde_json::Value, Option<String>)],
) -> Result<Vec<ContentBlock>, OrchestratorError> {
    Ok(dispatch_tool_uses_tracked(orch, tool_uses, None).await?.0)
}

/// claude-code `ZX_(rule.source, behavior)`: the OTEL decision-source label a
/// matched permission RULE contributes.
///
/// ```text
/// session                     → allow ? "user_temporary" : "user_reject"
/// localSettings|userSettings  → allow ? "user_permanent" : "user_reject"
/// default                     → "config"
/// ```
///
/// The three user-OWNED `SettingSource`s are the only ones that read as a user
/// decision; `projectSettings`/`policySettings`/`flagSettings`/`cliArg`/… (and a
/// decision with no rule at all — mode, classifier, safety check) stay "config".
/// claude-code's `toolDenialKind` classifier — byte-locked to the 2.1.220
/// mapping at binary offset 235392096:
///
/// ```js
/// if(e.behavior==="ask") return "user-rejected";
/// let t=e.decisionReason;
/// if(t.type==="classifier" && t.classifier==="auto-mode"){
///   if(t.reason===TRt)              return "automode-unavailable";
///   if(t.reason.startsWith(p2s))    return "automode-parsing-error";
///   return "automode-blocked";
/// }
/// return "permission-rule";
/// ```
///
/// with `TRt = "Classifier unavailable"` (offset 226748755) and
/// `p2s = "Auto mode could not evaluate this action and is blocking it for
/// safety"` (offset 235392399). `TRt` is matched by EQUALITY and `p2s` by
/// PREFIX — the oracle appends detail after `p2s`, so a prefix test is required.
///
/// The `behavior === "ask"` test runs FIRST and short-circuits: an ask-behavior
/// denial is `user-rejected` even when it also carries a classifier reason.
///
/// Documented narrowing: the oracle additionally requires
/// `t.classifier === "auto-mode"`. `permission::policy_gate::decision_reason_type`
/// maps BOTH classifier variants (`ClassifierApproved` / `ClassifierRejected`)
/// to `"classifier"`, and auto-mode is the only classifier LingXi has, so
/// `type == "classifier"` implies `classifier == "auto-mode"` here. Should a
/// second classifier ever land, this must gain the extra discriminator or it
/// will mislabel that classifier's denials as `automode-*`.
///
/// REACHABILITY (do not read the branches as live): today only
/// `automode-blocked` can actually fire. `PermissionDecisionReason::ClassifierRejected`
/// carries just `{classifier, score}` and no reason text
/// (`permission/src/result.rs`), and `permission::policy_gate::sysmsg_decision_reason`
/// returns `None` for it, so every classifier denial arrives here as
/// `(behavior_ask=false, Some("classifier"), None)`. The `TRt` / `p2s` branches
/// become reachable only once the classifier's reason text is threaded onto the
/// decision. The current OUTPUT is correct — the port's only classifier denial
/// is a genuine block — but the two sibling branches are inert until then.
/// `behavior_ask` is likewise hard-coded `false` at both producers, so the `ask`
/// short-circuit is inert too (same reason the contentBlocks path is documented
/// as dormant at its own call site).
pub(crate) fn tool_denial_kind(
    behavior_ask: bool,
    decision_reason_type: Option<&str>,
    decision_reason: Option<&str>,
) -> &'static str {
    /// claude-code `TRt`.
    const CLASSIFIER_UNAVAILABLE: &str = "Classifier unavailable";
    /// claude-code `p2s`.
    const SAFETY_BLOCK_PREFIX: &str =
        "Auto mode could not evaluate this action and is blocking it for safety";

    if behavior_ask {
        return "user-rejected";
    }
    if decision_reason_type == Some("classifier") {
        let reason = decision_reason.unwrap_or_default();
        if reason == CLASSIFIER_UNAVAILABLE {
            return "automode-unavailable";
        }
        if reason.starts_with(SAFETY_BLOCK_PREFIX) {
            return "automode-parsing-error";
        }
        return "automode-blocked";
    }
    "permission-rule"
}

pub(crate) fn rule_decision_otel_source(rule_source: Option<&str>, allow: bool) -> &'static str {
    match rule_source {
        Some("session") => {
            if allow {
                "user_temporary"
            } else {
                "user_reject"
            }
        }
        Some("localSettings" | "userSettings") => {
            if allow {
                "user_permanent"
            } else {
                "user_reject"
            }
        }
        _ => "config",
    }
}

/// HOOK.2 twin of [`dispatch_tool_uses`] that ALSO returns whether any
/// `PreToolUse` hook in this batch requested `continue:false`
/// (preventContinuation). The batched turn loop
/// ([`execute_one_turn_with_recovery_tracked`]) uses the flag to end the turn
/// step (TS `query.ts:1518-1521` `{ reason: 'hook_stopped' }`); the streaming
/// concurrent path keeps the plain [`dispatch_tool_uses`] wrapper.
#[allow(clippy::too_many_lines)]
/// `Gzg` (2.1.220 BIN off **230270568**, immediately above `F0u`):
///
/// ```text
/// if(!e)return!0;
/// if(typeof e==="string")return e.trim()==="";
/// if(!Array.isArray(e))return!1;
/// if(e.length===0)return!0;
/// return e.every(t=>typeof t==="object"&&"type"in t&&t.type==="text"
///                 &&"text"in t&&(typeof t.text!=="string"||t.text.trim()===""))
/// ```
///
/// The port's `tool_result` carries EITHER a plain string (`content_blocks ==
/// None`) or the verbatim block array — so the array arm is driven by
/// `content_blocks` and the string arm by `content`.
fn tool_result_is_blank(content: &str, content_blocks: Option<&[serde_json::Value]>) -> bool {
    match content_blocks {
        None => content.trim().is_empty(),
        Some(blocks) => {
            blocks.is_empty()
                || blocks.iter().all(|b| {
                    b.get("type").and_then(serde_json::Value::as_str) == Some("text")
                        && b.get("text").is_some()
                        && b.get("text")
                            .and_then(serde_json::Value::as_str)
                            .is_none_or(|t| t.trim().is_empty())
                })
        }
    }
}

/// `U0u` (2.1.220 BIN off **230271605**): an array containing ANY `image` or
/// `document` block is never persisted, regardless of size.
fn tool_result_has_media(content_blocks: Option<&[serde_json::Value]>) -> bool {
    content_blocks.is_some_and(|blocks| {
        blocks.iter().any(|b| {
            matches!(
                b.get("type").and_then(serde_json::Value::as_str),
                Some("image" | "document")
            )
        })
    })
}

/// `q0u` (2.1.220 BIN off **230271734**): a string's own length, or the sum of
/// the `text` block lengths in an array (non-text blocks contribute 0).
fn tool_result_size(content: &str, content_blocks: Option<&[serde_json::Value]>) -> usize {
    match content_blocks {
        None => content.len(),
        Some(blocks) => blocks
            .iter()
            .map(|b| {
                if b.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                    b.get("text")
                        .and_then(serde_json::Value::as_str)
                        .map_or(0, str::len)
                } else {
                    0
                }
            })
            .sum(),
    }
}

/// A1 — port of claude-code `F0u` (2.1.220 BIN off **230270568**), the
/// post-processor every SUCCESSFUL `tool_result` passes through on its way to
/// the model. Guards run in the oracle's order:
///
/// 1. `Gzg` — a blank result becomes `` `(${toolName} completed with no output)` ``
///    and fires `tengu_tool_empty_result`.
/// 2. `U0u` — an image/document-bearing result is returned UNCHANGED.
/// 3. `o<=i` — only a STRICTLY larger body is persisted.
/// 4. A persist failure returns the ORIGINAL content (the error never reaches
///    the model).
/// 5. On success, `tengu_tool_result_persisted` is fired and the envelope is
///    substituted.
///
/// `threshold == None` is the oracle's `maxResultSizeChars: 1/0`
/// (`!Number.isFinite(t)` → early return), i.e. NEVER persist. A missing
/// `config_home` (library/test callers) is likewise a strict no-op for the
/// persistence arm — the blank-result arm still applies, since it needs no
/// filesystem.
/// Outcome of [`apply_tool_result_persistence`].
///
/// `replaced` is load-bearing, not informational. claude-code's `F0u` returns
/// `{...e, content: a}` where `content` is the ONE model-facing payload — a
/// string OR an array — so a substitution replaces the whole payload. LingXi
/// splits that payload across `ContentBlock::ToolResult`'s `content` string and
/// its `content_blocks` array, and the wire conversion prefers the array when
/// present (`llm-client/src/convert.rs`: `content_blocks.map_or_else(|| String(content), Array)`).
/// So substituting only `content` would leave the oversized array to win at the
/// wire: the file gets written, the telemetry fires, and the model still
/// receives the full payload. The caller MUST clear `content_blocks` whenever
/// this reports `true`.
struct PersistenceOutcome {
    content: String,
    replaced: bool,
}

async fn apply_tool_result_persistence(
    orch: &ConversationOrchestrator,
    tool_name: &str,
    tool_use_id: &ToolUseId,
    threshold: Option<usize>,
    content: String,
    content_blocks: Option<&[serde_json::Value]>,
) -> PersistenceOutcome {
    use crate::tool_result_persistence as trp;

    if tool_result_is_blank(&content, content_blocks) {
        if let Some(bus) = orch.analytics_bus.as_ref() {
            let mut metadata = telemetry::LogEventMetadata::new();
            metadata.insert(
                "toolName".into(),
                telemetry::AnalyticsValue::String(tool_name.to_string()),
            );
            bus.log_event("tengu_tool_empty_result", metadata).await;
        }
        return PersistenceOutcome {
            content: format!("({tool_name} completed with no output)"),
            replaced: true,
        };
    }
    if tool_result_has_media(content_blocks) {
        return PersistenceOutcome {
            content,
            replaced: false,
        };
    }
    let Some(threshold) = threshold else {
        return PersistenceOutcome {
            content,
            replaced: false,
        };
    };
    let size = tool_result_size(&content, content_blocks);
    if size <= threshold {
        return PersistenceOutcome {
            content,
            replaced: false,
        };
    }
    let Some(home) = orch.config_home.as_ref() else {
        return PersistenceOutcome {
            content,
            replaced: false,
        };
    };

    // `x2e` serializes an ARRAY body with `JSON.stringify(e,null,2)` and a
    // string body verbatim; the extension follows (`kKr`).
    let (body, is_json) = match content_blocks {
        Some(blocks) => match serde_json::to_string_pretty(blocks) {
            Ok(s) => (s, true),
            // `e.some(l=>l.type!=="text")` already returned an error above in
            // the oracle; an unserializable array is the same "leave it alone".
            Err(_) => {
                return PersistenceOutcome {
                    content,
                    replaced: false,
                }
            }
        },
        None => (content.clone(), false),
    };

    let session_uuid = {
        let session = orch.session.lock().await;
        session.session_id.as_uuid().to_string()
    };
    let dir = trp::tool_results_dir(home, &orch.current_cwd().to_string_lossy(), &session_uuid);
    // The on-disk stem is the port's INTERNAL `ToolUseId`, matching the
    // oracle's `${e.tool_use_id}.txt` — claude-code's internal block-param id
    // likewise differs from the `toolu_…` id it records in the transcript.
    let persisted = match trp::persist(&dir, tool_use_id.as_str(), &body, is_json).await {
        Ok(p) => p,
        Err(msg) => {
            tracing::error!(
                path = %dir.join(tool_use_id.as_str()).display(),
                "Failed to persist tool result: {msg}"
            );
            return PersistenceOutcome {
                content,
                replaced: false,
            };
        }
    };
    let path_display = persisted.filepath.display().to_string();
    tracing::info!(
        "Persisted tool result to {path_display} ({})",
        trp::format_bytes(persisted.original_size)
    );
    let replacement = trp::wrap(
        persisted.original_size,
        &path_display,
        &persisted.preview,
        persisted.has_more,
    );
    if let Some(bus) = orch.analytics_bus.as_ref() {
        #[allow(clippy::cast_possible_wrap)]
        fn int(v: usize) -> telemetry::AnalyticsValue {
            telemetry::AnalyticsValue::Int(i64::try_from(v).unwrap_or(i64::MAX))
        }
        let mut metadata = telemetry::LogEventMetadata::new();
        metadata.insert(
            "toolName".into(),
            telemetry::AnalyticsValue::String(tool_name.to_string()),
        );
        metadata.insert("originalSizeBytes".into(), int(persisted.original_size));
        metadata.insert("persistedSizeBytes".into(), int(replacement.len()));
        metadata.insert(
            "estimatedOriginalTokens".into(),
            int(persisted.original_size.div_ceil(trp::CHARS_PER_TOKEN)),
        );
        metadata.insert(
            "estimatedPersistedTokens".into(),
            int(replacement.len().div_ceil(trp::CHARS_PER_TOKEN)),
        );
        metadata.insert("thresholdUsed".into(), int(threshold));
        bus.log_event("tengu_tool_result_persisted", metadata).await;
    }
    PersistenceOutcome {
        content: replacement,
        replaced: true,
    }
}

pub(crate) async fn dispatch_tool_uses_tracked(
    orch: &ConversationOrchestrator,
    tool_uses: &[(ToolUseId, String, serde_json::Value, Option<String>)],
    // PHASE-2 + DEFERRED-3: per-tool `CancellationToken` (a child of the streaming
    // executor's `tool_abort`) threaded into each tool's
    // `ToolUseContext::cancel`. It fires when the turn is discarded (streaming
    // fallback) OR — because `tool_abort` is parented to the turn's
    // user-interrupt token in `new_with_user_cancel` — when the USER interrupts
    // (DEFERRED-3, ESC / new message). A Cancel-behavior tool (e.g. an in-flight
    // Bash) observes it to return early / SIGKILL its subprocess; the executor
    // then substitutes the synthetic result. `None` for every non-streaming caller
    // (batched turn loop + tests) → no cancellation ever fires.
    cancel: Option<tokio_util::sync::CancellationToken>,
) -> Result<
    (
        Vec<ContentBlock>,
        bool,
        Vec<(ConversationMessage, ToolUseId)>,
        Vec<ContextModifier>,
    ),
    OrchestratorError,
> {
    let mut results = Vec::with_capacity(tool_uses.len());
    // HOOK.2: OR-fold each tool's PreToolUse `prevent_continuation` signal.
    let mut prevent_continuation = false;
    // SKILLEXEC.3 (Part A): conversation messages a tool wants injected AFTER
    // its tool_result (TS `ToolResult.newMessages`, e.g. the Skill tool's
    // expanded skill prompt). Accumulated in tool-dispatch order and returned to
    // the caller, which appends them to history right after this batch's
    // tool_result user message. Empty for every existing tool → no-op.
    //
    // Each injected message is paired with the `tool_use_id` of the tool that
    // injected it — the faithful port of TS `tagMessagesWithToolUseID`
    // (`tools/utils.ts:12-25`), which stamps every injected `UserMessage` with
    // the Skill tool's OWN `tool_use` block id (`sourceToolUseID`). The caller
    // records the pair into `SessionState::injected_message_sources` (an
    // in-memory side-table, never serialized to JSONL) when it appends the
    // message to history.
    let mut injected_messages: Vec<(ConversationMessage, ToolUseId)> = Vec::new();
    // SKILLEXEC.3 (model scope): one-shot `context_modifier`s a tool returns
    // (TS `ToolResult.contextModifier`, e.g. the Skill tool's `model:` override).
    // Collected in tool-dispatch order and folded POST-BATCH by the caller over a
    // seed context carrying the live `session.model` (see
    // [`apply_model_context_modifiers`]). Empty for every tool that returns
    // `context_modifier: None` (every existing tool + skills WITHOUT a `model:`
    // frontmatter) → the caller does NOTHING → byte-identical.
    let mut context_modifiers: Vec<ContextModifier> = Vec::new();
    // #39 PostToolBatch: accumulate one entry per RESOLVED tool call (claude-code
    // fires PostToolBatch ONCE after every tool in the batch resolves, before the
    // next model request — `tool_calls` = the full batch). A tool that is blocked
    // / deferred / denied before execution `continue`s and does not reach the
    // result push, so it is not part of the resolved batch (matching claude-code,
    // where only executed tools have a `tool_response`). Empty when no tool ran →
    // the PostToolBatch fire below is a strict no-op.
    let mut post_tool_batch_calls: Vec<hooks::events::PostToolBatchCall> = Vec::new();
    // FORK (codex #5 follow-up): the rendered system prompt this turn handed the
    // model, recorded by the turn driver after the successful API call. Threaded
    // onto each tool's `ToolUseContext::fork_parent_system_prompt` so a
    // fork-subagent spawn (`AgentTool` with no `subagent_type`) can run its child
    // with a byte-identical system prompt (cache-prefix parity, claude
    // `AgentTool.tsx:622-623`). `None` until the first successful turn / when the
    // turn ran with no system prompt — no non-fork tool reads this field.
    let fork_parent_system_prompt = orch.current_turn_system_prompt().await;
    for (tool_use_id, name, input, provider_id) in tool_uses {
        orch.output.emit_tool_call(tool_use_id, name, input).await;

        // claude-code order (`toolExecution.ts` runToolUse ~401 +
        // checkPermissionsAndCallTool ~683): the unknown-tool check and the
        // `validateInput` gate run at the TOP of `runToolUse` — BEFORE the
        // PreToolUse hooks (~800) and the permission gate. We mirror that here,
        // resolving the tool handle + synthesizing the per-call context first,
        // then running `validate_input`, and only after both clear do the
        // PreToolUse hook + permission gate run below.

        // Unknown-tool arm (claude-code `toolExecution.ts:401`). Runs BEFORE
        // any hook, so there is no pre-hook context to fold — emit the raw
        // wrapped literal verbatim (claude-code's unknown-tool has no hook
        // context).
        let Some(tool_handle) = orch.tools.find_by_name(name) else {
            // Shared builder so this parity-critical string lives in one place
            // (also used by the streaming executor's add_tool).
            let result_block = crate::streaming_executor::synthetic_unknown_tool(
                tool_use_id.clone(),
                name,
                provider_id.clone(),
            );
            // Pass the SAME wrapped string the result_block carries as the
            // model text, so the SDK frame's `content` matches the model wire.
            let model_text = match &result_block {
                ContentBlock::ToolResult { content, .. } => content.clone(),
                _ => format!(
                    "<tool_use_error>Error: No such tool available: {name}</tool_use_error>"
                ),
            };
            // O1: claude's unknown-tool arm (2.1.220 BIN off 235398500 /
            // 232971680) stamps the persisted line with the BARE string
            // `` `Error: No such tool available: ${name}${suffix}` `` — the
            // unwrapped twin of the `<tool_use_error>` model text. `suffix` is
            // claude's `Gks` "did you mean" hint, which the port does not
            // produce, so it is empty here (same as claude when no alias
            // matches).
            orch.record_tool_use_result(
                tool_use_id,
                serde_json::Value::String(format!("Error: No such tool available: {name}")),
            )
            .await;
            orch.emit_tool_result_frame(
                tool_use_id,
                name,
                &model_text,
                &serde_json::json!({ "error": format!("tool not found: {name}") }),
                None,
            )
            .await;
            results.push(result_block);
            continue;
        };

        // JSON-schema input gate (claude-code `toolExecution.ts:615`
        // `inputSchema.safeParse`): runs on the RAW `input` (pre-hook), AFTER the
        // unknown-tool arm and BEFORE the `validate_input` gate — the exact order
        // of `checkPermissionsAndCallTool` (safeParse ~615 precedes validateInput
        // ~683). BEHAVIORAL parity only: the `<tool_use_error>InputValidationError:
        // …>` wrapper matches, but the detail bytes intentionally differ from
        // claude-code's Zod `formatZodValidationError` output (unportable). A
        // malformed tool schema is treated as PASS (logged) — see
        // [`crate::schema_validation::validate_tool_input_schema`].
        if let Err(detail) =
            crate::schema_validation::validate_tool_input_schema(tool_handle.input_schema(), input)
        {
            let model_text =
                format!("<tool_use_error>InputValidationError: {detail}</tool_use_error>");
            let result_block = ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: model_text.clone(),
                is_error: true,
                provider_tool_use_id: provider_id.clone(),
                content_blocks: None,
            };
            // O1: the schema-gate arm (2.1.220 BIN off 235405560) stamps
            // `` toolUseResult: `InputValidationError: ${zodError.message}` `` —
            // the unwrapped twin of the model text. claude uses the RAW zod
            // message here while the model text carries its ENRICHED `ce`
            // rendering; the port has one detail string, which it reuses, so
            // these two bytes coincide (the detail bytes already diverge from
            // Zod's by design — see the comment above).
            orch.record_tool_use_result(
                tool_use_id,
                serde_json::Value::String(format!("InputValidationError: {detail}")),
            )
            .await;
            orch.emit_tool_result_frame(
                tool_use_id,
                name,
                &model_text,
                &serde_json::json!({ "error": detail }),
                None,
            )
            .await;
            results.push(result_block);
            continue;
        }

        // Synthesize a minimal ToolUseContext — needed by the validate_input
        // gate below and reused by the eventual `tool_handle.call()`.
        let (messages, model, model_profile) = {
            let s = orch.session.lock().await;
            (s.history.clone(), s.model.clone(), s.model_profile.clone())
        };
        let ctx = ToolUseContext {
            options: ToolUseOptions {
                debug: false,
                verbose: false,
                // The LIVE session model (updated by `/model` switches / resume),
                // not `config.model` (frozen at launch). Tools gate model-facing
                // behavior on this — e.g. WebSearch's hosted-vs-client-side split
                // needs the current model, so a switched/resumed non-Claude model
                // resolves correctly.
                main_loop_model: model,
                model_profile,
                max_budget_nano_usd: None,
                mcp_clients: Vec::new(),
                // FIX 3: claude-code's main REPL builds `getToolUseContext` with
                // `isNonInteractiveSession: false` (REPL.tsx:2427). LingXi hardcoded
                // `true` here — the OPPOSITE — which would flip the model-facing
                // verification-nudge + fork-subagent paths in an interactive
                // session. Use the orchestrator's own print/headless signal
                // `!interactive_permissions` (the SAME signal the defer path uses at
                // turn_loop.rs ~1729/1754): `true` only in a non-interactive
                // (print/headless) session. Inert under today's default-off feature
                // flags, but removes the latent divergence.
                is_non_interactive_session: !orch.config.interactive_permissions,
                custom_system_prompt: orch.config.system_prompt_override.clone(),
                append_system_prompt: None,
            },
            messages,
            tool_use_id: Some(tool_use_id.clone()),
            agent_id: None,
            // Main / leader thread: no teammate identity (TS getAgentName() /
            // getTeammateContext() are undefined here).
            agent_name: None,
            team_name: None,
            content_replacement_state: None,
            session: Some(orch.session.clone()),
            subagent_registry: Some(orch.tools.clone()),
            // PHASE-2: hand each tool a clone of the sibling cancel token (the
            // streaming executor passes a per-tool child; every other caller
            // passes `None`). Clone per-tool since this loop may dispatch a
            // batch (the streaming executor calls one-tool-at-a-time).
            cancel: cancel.clone(),
            // FORK-ONLY: the parent's rendered system prompt for THIS turn (the
            // bytes the model saw), recorded by the turn driver after the API
            // call. On the fork path `AgentTool` threads it onto the child's
            // `SubagentSpawnRequest.fork_parent_system_prompt` for a
            // byte-identical cache prefix. `None` until the first successful turn
            // / a turn with no system prompt; no non-fork tool reads it.
            fork_parent_system_prompt: fork_parent_system_prompt.clone(),
            // Main turn loop uses the shared session workspace (no per-agent
            // cwd override); only an isolated subagent sets this.
            cwd: None,
            depth: 0,
            observer: None,
            // (/rewind) Hand each write tool the file-history sink (a trait view
            // of the shared checkpoint store) so pre-edit content is backed up.
            file_history: orch
                .file_history
                .clone()
                .map(|fh| fh as std::sync::Arc<dyn traits::FileHistorySink>),
        };

        // validate_input gate (claude-code `toolExecution.ts:683-723`): a
        // `validateInput` failure wraps the message in `<tool_use_error>` and
        // short-circuits. Runs on the RAW `input` (pre-hook), BEFORE the
        // PreToolUse hooks/permission (claude-code order), so there is no
        // pre-hook context to fold.
        if let Err(tool_api::ValidationError(msg)) = tool_handle.validate_input(input, &ctx).await {
            let model_text = format!("<tool_use_error>{msg}</tool_use_error>");
            let result_block = ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: model_text.clone(),
                is_error: true,
                provider_tool_use_id: provider_id.clone(),
                content_blocks: None,
            };
            // O1: claude's validate_input arm (2.1.220 BIN off 235407190)
            // stamps `` toolUseResult: `Error: ${T.message}` `` — the unwrapped
            // twin of the `<tool_use_error>` model text.
            orch.record_tool_use_result(
                tool_use_id,
                serde_json::Value::String(format!("Error: {msg}")),
            )
            .await;
            orch.emit_tool_result_frame(
                tool_use_id,
                name,
                &model_text,
                &serde_json::json!({ "error": msg }),
                None,
            )
            .await;
            results.push(result_block);
            continue;
        }

        // claude-code `toolExecution.ts:413-453`: if the user-interrupt token
        // is already cancelled at the top of runToolUse (a pre-cancel — ESC
        // fired before this tool got CPU), emit the bare CANCEL_MESSAGE as an
        // is_error tool_result and skip execution. Mirrors the TS guard exactly:
        // the bare string (NOT `<tool_use_error>`-wrapped), `is_error: true`,
        // and `continue` without pushing to `post_tool_batch_calls` (tool
        // didn't run). A `None` cancel token → guard never fires.
        if cancel.as_ref().is_some_and(|t| t.is_cancelled()) {
            let result_block = ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: CANCEL_MESSAGE.to_string(),
                is_error: true,
                provider_tool_use_id: provider_id.clone(),
                content_blocks: None,
            };
            // Denial provenance: claude-code HARDCODES `toolDenialKind:"cancelled"`
            // at this site (binary offset 235399713) rather than routing through
            // its `YDd` abort-reason classifier, so this needs no abort-reason
            // plumbing to be faithful. `cancelled` is not one of the five kinds
            // the permission classifier emits, but it IS an ordinary
            // `toolDenialKind` value that produces a `tool_result_meta` entry.
            orch.record_tool_denial_kind(tool_use_id, "cancelled").await;
            // O1: the same site sets `toolUseResult: FK` (2.1.220 BIN off
            // 235398916), where `FK` (BIN off 229154836) IS `CANCEL_MESSAGE` —
            // the identical string this block's model content already carries.
            orch.record_tool_use_result(
                tool_use_id,
                serde_json::Value::String(CANCEL_MESSAGE.to_string()),
            )
            .await;
            orch.emit_tool_result_frame(
                tool_use_id,
                name,
                CANCEL_MESSAGE,
                &serde_json::json!({ "error": CANCEL_MESSAGE }),
                Some("cancelled"),
            )
            .await;
            results.push(result_block);
            continue;
        }

        // M5-06 Task 14: PreToolUse hook chain. Build the event + context,
        // call the executor, and either Block (turn the response into an
        // error ToolResult), apply modified_input, or continue.
        // FIX 2: populate `transcript_path` + `permission_mode` on the PreToolUse
        // context (and the PostToolUse fire below, which reuses this `hook_ctx`),
        // matching claude-code `createBaseHookInput` (always sets
        // `transcript_path: getTranscriptPathForSession(...)`, utils/hooks.ts:322)
        // plus PreToolUse/PostToolUse's `permission_mode =
        // appState.toolPermissionContext.mode` (toolHooks.ts:471). The hook reads
        // the enforcing gate's live wire mode; session `plan_mode` remains the
        // explicit override used while the plan workflow is active.
        //
        // FIX A: the transcript path is the live JSONL writer's path when one is
        // wired (preserves the writer-backed tests) ELSE the deterministically-
        // computed `<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`. In
        // PRODUCTION no writer is wired, so the prior `unwrap_or_default()` made
        // EVERY PreToolUse/PostToolUse hook carry an empty `transcript_path`.
        let (session_id, plan_mode) = {
            let s = orch.session.lock().await;
            (s.session_id, s.plan_mode)
        };
        let transcript_path = orch
            .jsonl_writer
            .as_ref()
            .map(|w| w.path().to_path_buf())
            .unwrap_or_else(|| orch.computed_transcript_path(&session_id));
        let permission_mode = Some(if plan_mode {
            "plan".to_string()
        } else {
            orch.permission_mode()
                .unwrap_or_else(|| "default".to_string())
        });
        let hook_ctx = HookContext {
            session_id,
            cwd: orch.current_cwd(),
            transcript_path,
            permission_mode,
            ..Default::default()
        };
        let pre_event = HookEvent::PreToolUse {
            tool_name: name.clone(),
            tool_input: input.clone(),
            tool_use_id: tool_use_id.clone(),
        };
        let pre_started = std::time::Instant::now();
        tracing::info!(
            event = orch_events::HOOK_PRE_STARTED,
            tool_name = %name,
        );
        telemetry::otel::emit_hook_lifecycle("pre", "started", &name, None);
        let pre_agg = orch.hooks.execute(pre_event, hook_ctx.clone()).await;
        // hook duration bounded by tokio timeout — u128 ms cannot exceed u64::MAX
        #[allow(clippy::cast_possible_truncation)]
        let pre_dur_ms = pre_started.elapsed().as_millis() as u64;

        // HOOK.2: a PreToolUse hook's `continue:false` (preventContinuation)
        // signal — OR-folded so a later turn-step disposition ends the loop.
        // Captured BEFORE any early `continue` so a blocking hook that also
        // requested preventContinuation still stops the loop (TS yields
        // preventContinuation in the pre-hook phase regardless of the block).
        if pre_agg.prevent_continuation {
            prevent_continuation = true;
        }
        // #40 terminalSequence apply (claude-code `szn`, BIN off 205755390): a
        // hook may return a top-level `terminalSequence` for LingXi to emit
        // (OSC 9 / 777 desktop notification, etc.). Run the allowlist validator
        // (`NEo`) over the folded sequence: on REJECT, warn (the observable half,
        // byte-faithful to claude-code's reject message). On ACCEPT the
        // validated string is emitted through the output bridge to the active
        // TUI terminal (`BEo`). No-op when no hook set it.
        apply_terminal_sequence(orch, &name, pre_agg.terminal_sequence.as_deref()).await;
        // HOOK.1: a PreToolUse hook's `additionalContext` (NOT `systemMessage`)
        // becomes its OWN meta message, not folded into the tool_result — claude
        // pushes it to `resultingMessages` (`toolExecution.ts:845`). Shape:
        // `<system-reminder>`-wrapped `PreToolUse:{tool} hook additional context:
        // {ctx}`, contexts joined by `\n` (`messages.ts:4117-4128`). `systemMessage`
        // is excluded — its `hook_system_message` `normalizeAttachmentForAPI`→`[]`
        // never reaches the model (`messages.ts:4258`). Built in the PRE-hook phase
        // (`toolExecution.ts:846`), so it surfaces on success/block/deny alike,
        // ordered after that arm's tool_result. Tagged with `tool_use_id` (TS
        // `toolUseID`); no-op when empty.
        let pre_hook_messages = pre_agg.additional_contexts.clone();
        // Build the standalone additionalContext message (HOOK.1) and queue it
        // on the `injected` channel, tagged with THIS tool's `tool_use_id` (TS
        // stamps `toolUseID` on the attachment). A strict no-op when the hook
        // emitted no context, so the locked turn-loop fixtures (noop hooks) are
        // unaffected. claude-code pushes `additionalContext` to
        // `resultingMessages` in the PRE-hook phase (`toolExecution.ts:846`),
        // BEFORE the permission/block check — so it surfaces even when the tool
        // is later BLOCKED (preventContinuation) or DENIED. We therefore emit it
        // on the SUCCESS, BLOCK, and DENY paths alike, in every case ordered
        // AFTER that path's tool_result (matching claude-code post-hoist).
        let pre_context_message: Option<ConversationMessage> = if pre_hook_messages.is_empty() {
            None
        } else {
            // O3: the PERSISTED record is a `hook_additional_context`
            // ATTACHMENT line (2.1.220 BIN off 234733097 for the PreToolUse
            // producer), queued here and flushed after this tool's tool_result.
            orch.queue_hook_attachment(
                tool_use_id,
                hooks::additional_context_attachment(
                    &format!("PreToolUse:{name}"),
                    tool_use_id.as_str(),
                    "PreToolUse",
                    &pre_hook_messages,
                ),
            )
            .await;
            let body = pre_hook_messages.join("\n");
            // O3: the model-facing rendering is `zr({content: Ww(…),
            // isMeta:true})` (renderer table BIN off 238107100) and is
            // EPHEMERAL — built from the attachment at API-normalization time
            // and never persisted. `user_meta` marks it so both drivers skip
            // persisting it; the attachment above IS the on-disk record.
            Some(ConversationMessage::user_meta(
                MessageId::new(),
                format!(
                    "<system-reminder>\nPreToolUse:{name} hook additional context: {body}\n</system-reminder>"
                ),
            ))
        };

        // FIX C (hook_stopped_continuation, PreToolUse twin): a PreToolUse hook's
        // `continue:false` (preventContinuation) becomes its OWN meta message —
        // claude yields it AFTER the tool_result on the SUCCESS path
        // (`toolExecution.ts:1571-1582`, inside the post-execution try-block),
        // using `stopReason || 'Execution stopped by hook'` and hookName
        // `PreToolUse:{tool}`. The tool STILL runs (the pre-hook only OR-folds the
        // end-turn signal at line ~2363); the message is emitted after that tool's
        // tool_result, tagged with this tool's `tool_use_id`. Built here alongside
        // `pre_context_message` but pushed ONLY on the success path below — a
        // Block/Defer never executes the tool, so claude's post-execution site
        // never fires there. `pre_agg.reason` carries the parsed `stopReason`
        // (`hook_payload.rs:1113`). `None` when the hook did not request
        // preventContinuation (the common case), a strict no-op.
        // O2: the message is paired with the `hook_stopped_continuation`
        // ATTACHMENT the oracle records beside it (BIN off 235403061). Both are
        // built here but published together on the success path below, so the
        // record cannot drift away from the prose it describes.
        let pre_prevent: Option<(ConversationMessage, serde_json::Value)> = if pre_agg
            .prevent_continuation
        {
            let reason = pre_agg
                .reason
                .clone()
                .unwrap_or_else(|| "Execution stopped by hook".to_string());
            let attachment = hooks::stopped_continuation_attachment(
                &hooks::HookAttachmentIdentity {
                    hook_name: format!("PreToolUse:{name}"),
                    hook_event: "PreToolUse".to_string(),
                    tool_use_id: tool_use_id.as_str().to_string(),
                },
                &reason,
            );
            Some((
                ConversationMessage::user_meta(
                    MessageId::new(),
                    format!(
                        "<system-reminder>\nPreToolUse:{name} hook stopped continuation: {reason}\n</system-reminder>"
                    ),
                ),
                attachment,
            ))
        } else {
            None
        };

        // #37 `permissionDecision:"defer"` (claude BIN off 202454844): a PreToolUse
        // hook defers a tool to a later interactive resume. Gated to (1) non-
        // interactive mode and (2) a SOLO batch; else warns and falls through.
        // On the gated path: emit `tengu_pre_tool_hook_deferred`, push a
        // `hook_deferred_tool` meta, TERMINATE (`tool_deferred`, tool not run).
        // `is_non_interactive_session = !interactive_permissions`; DORMANT on the
        // default REPL (interactive ignores defer).
        if matches!(pre_agg.decision, Some(HookDecision::Defer)) {
            // The deferred attachment uses the source of the hook that owns the
            // folded Defer decision. Fall back to the event-qualified name only
            // for legacy/custom executors that omitted provenance.
            let hook_name = pre_agg
                .hook_source
                .map(hooks::HookSource::deferred_label)
                .map(str::to_string)
                .unwrap_or_else(|| format!("PreToolUse:{name}"));
            let is_non_interactive = !orch.config.interactive_permissions;
            // batch size = the number of tool_use blocks this dispatch is
            // processing (claude-code counts `tool_use` blocks in the assistant
            // message via `Wn(s.message.content, te=>te.type==="tool_use")`).
            let batch_tool_count = tool_uses.len();
            if !is_non_interactive {
                tracing::warn!(
                    tool_name = %name,
                    "Hook {hook_name} returned permissionDecision=defer in interactive mode; ignoring (defer is print-mode only)"
                );
                // ignored → fall through to the normal gate by clearing Defer.
                // (handled below: the Defer decision is treated as no-decision)
            } else if batch_tool_count > 1 {
                tracing::warn!(
                    tool_name = %name,
                    "Hook {hook_name} returned permissionDecision=defer but {batch_tool_count} tool calls are in this batch; ignoring (defer is solo-only \u{2014} siblings would be orphaned on resume)"
                );
                // ignored → fall through to the normal gate.
            } else {
                // GATED path: honor the defer. Emit the analytic (inline event
                // name, NOT a locked const — same pattern as
                // `tengu_model_fallback_triggered`, so the 347 registry is
                // untouched), push the `hook_deferred_tool` meta message, and
                // terminate the turn (`tool_deferred` stop-reason — the tool is
                // not executed).
                // Persist the same live mode already supplied to the hook
                // payload, including acceptEdits/bypassPermissions/dontAsk/auto.
                let permission_mode = hook_ctx.permission_mode.as_deref().unwrap_or("default");
                tracing::info!(
                    event = "tengu_pre_tool_hook_deferred",
                    tool_name = %name,
                );
                tracing::info!(
                    event = orch_events::HOOK_PRE_COMPLETED,
                    tool_name = %name,
                    decision = "defer",
                    duration_ms = pre_dur_ms,
                );
                // O2: the `hook_deferred_tool` record is PERSISTED, never sent
                // to the model. Its renderer is `hook_deferred_tool:()=>[]`
                // (BIN off 238109388), and the record is FUNCTIONAL rather than
                // cosmetic — it is the resume protocol:
                //   * `QAs` (BIN off 237925753) scans the transcript's last
                //     1 MiB backwards for `'"hook_deferred_tool"'`, requiring
                //     `type:"attachment"` with that inner type, and rejects the
                //     deferral if a LATER line carries this `toolUseID`.
                //   * the stream-json engine (BIN off 240899919) rebuilds
                //     `{id, name, input}` from it with `stop_reason`
                //     `"tool_deferred"`.
                //
                // Previously the port pushed the raw JSON as a plain (non-meta)
                // user message: the model read a blob claude suppresses AND no
                // resume scanner could ever find the deferral. BEHAVIOR CHANGE:
                // the `-p`/print-mode defer path no longer sends that message.
                //
                // Persisted IMMEDIATELY rather than queued — the tool-keyed
                // queue is flushed after a `tool_result`, and a deferred tool
                // never produces one, so a queued record would strand.
                //
                // `toolInput` is the HOOK-UPDATED input (oracle `b`, set by the
                // `case"hookUpdatedInput"` arm before the defer yield at BIN
                // off 235409134) — NOT the raw model input. `effective_input`
                // is computed further down, after this block, so the same
                // `modified_input` fold is applied here.
                let deferred_input = pre_agg
                    .modified_input
                    .clone()
                    .unwrap_or_else(|| input.clone());
                orch.persist_hook_attachment_to_jsonl(hooks::deferred_tool_attachment(
                    tool_use_id.as_str(),
                    name,
                    &deferred_input,
                    &hook_name,
                    permission_mode,
                ))
                .await;
                // HOOK.1: any PreToolUse additionalContext, ordered AFTER the
                // deferred-tool record (matching the Block arm). Its attachment
                // was queued above against this tool id; drain it now, since
                // the usual post-`tool_result` flush will never run here.
                orch.flush_hook_attachments(tool_use_id).await;
                if let Some(msg) = pre_context_message {
                    injected_messages.push((msg, tool_use_id.clone()));
                }
                // TERMINATE the turn — the deferred tool is NOT executed. The
                // `tool_deferred` stop-reason has no distinct LingXi turn-stop
                // variant; reuse the `prevent_continuation` end-of-turn signal so
                // the agent loop stops after this batch (the deferred tool's
                // result is intentionally absent). `continue` skips this tool's
                // execution entirely.
                prevent_continuation = true;
                continue;
            }
        }

        if matches!(pre_agg.decision, Some(HookDecision::Block)) {
            // claude-code maps a PreToolUse `decision:"block"` to
            // `permissionBehavior:"deny"` with `blockingError = reason ||
            // "Blocked by hook"`, then renders the model-facing deny message via
            // `aAs(hookName, blockingError)` = `` `${hookName} hook error:
            // ${blockingError}` ``. For PreToolUse the hook name is
            // `PreToolUse:${toolName}`, so the tool_result the model sees is
            // `"PreToolUse:<name> hook error: <reason>"` (fallback reason
            // "Blocked by hook", capital B).
            let reason = pre_agg
                .reason
                .clone()
                .unwrap_or_else(|| "Blocked by hook".into());
            tracing::info!(
                event = orch_events::HOOK_PRE_COMPLETED,
                tool_name = %name,
                decision = "block",
                duration_ms = pre_dur_ms,
            );
            let model_text = format!("PreToolUse:{name} hook error: {reason}");
            let result_block = ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: model_text.clone(),
                is_error: true,
                provider_tool_use_id: provider_id.clone(),
                content_blocks: None,
            };
            orch.emit_tool_result_frame(
                tool_use_id,
                name,
                &model_text,
                &serde_json::json!({ "error": model_text.clone() }),
                None,
            )
            .await;
            results.push(result_block);
            // HOOK.1: even on a BLOCK, the PreToolUse `additionalContext` was
            // pushed in claude-code's pre-hook phase (`toolExecution.ts:846`),
            // before the block check — so surface it here, ordered AFTER this
            // path's error tool_result. No-op when the hook emitted no context.
            if let Some(msg) = pre_context_message {
                injected_messages.push((msg, tool_use_id.clone()));
            }
            continue;
        }

        // Apply modified_input if any hook mutated the tool input. Mutable so a
        // PermissionRequest hook 'allow' can further rewrite the input before the
        // tool runs (claude-code `updatedInput`).
        let mut effective_input = pre_agg
            .modified_input
            .clone()
            .unwrap_or_else(|| input.clone());
        tracing::info!(
            event = orch_events::HOOK_PRE_COMPLETED,
            tool_name = %name,
            decision = match pre_agg.decision {
                Some(HookDecision::Allow) => "allow",
                Some(HookDecision::Approve) => "approve",
                Some(HookDecision::Continue) => "continue",
                Some(HookDecision::Block) => "block",
                // #37: a Defer that reached here was IGNORED (interactive mode or
                // a multi-tool batch) — the gated path `continue`d above, so this
                // arm only fires for the ignored case, which proceeds to the
                // normal permission gate exactly like no decision.
                Some(HookDecision::Defer) => "defer-ignored",
                // R-D3: a `permissionDecision:"ask"` parses to `HookDecision::Ask`
                // and forces the interactive prompt even over a configured allow
                // rule — routed in the normal-gate branch below (an Allow
                // resolution is upgraded to Ask when `hook_ask`). Deny rules and
                // plan mode still bind (deny > ask > allow).
                Some(HookDecision::Ask) => "ask",
                None => "none",
            },
            duration_ms = pre_dur_ms,
        );
        telemetry::otel::emit_hook_lifecycle("pre", "completed", &name, Some(pre_dur_ms));

        // HOOK.3: a PreToolUse hook's permissionDecision "allow" (legacy
        // `decision: "approve"`) bypasses the permission gate for this tool call
        // (TS `resolveHookPermissionDecision`: a hook 'allow' skips the
        // interactive prompt). Both wire forms parse to `HookDecision::Approve`.
        // A hook "deny"/"block" already short-circuited above (parsed to
        // `HookDecision::Block`); "ask" / no-decision leave `pre_agg.decision`
        // unset and fall through to the normal gate.
        //
        // HOOK.3 resolution: a hook 'allow' skips the interactive PROMPT but
        // STILL applies rule-based deny/ask (claude-code
        // `resolveHookPermissionDecision` + `checkRuleBasedPermissions`) — a hook
        // CANNOT override an explicit deny rule or the active mode's mutation
        // backstop. So we ALWAYS consult the gate: `check_after_hook_allow`
        // (deny rules + mode bind, the prompt is skipped) when a hook approved,
        // else the normal `check` (which may delegate an `Ask` to the prompt
        // transport). Uses the post-hook `effective_input` so a Pre hook can
        // rewrite a tool argument before the permission check sees it.
        let hook_allowed = matches!(
            pre_agg.decision,
            Some(HookDecision::Approve | HookDecision::Allow)
        );
        // R-D3: a PreToolUse hook `permissionDecision:"ask"` forces the interactive
        // prompt even over a configured allow rule (the resolution upgrade in the
        // normal-gate branch below). Mutually exclusive with `hook_allowed`.
        let hook_ask = matches!(pre_agg.decision, Some(HookDecision::Ask));
        // HOOK.4 — plan-mode dynamic gate (claude's live `mode='plan'`): authorize
        // under `PermissionMode::Plan` so the mutation backstop activates on a
        // runtime `EnterPlanMode` (`check_in_plan_mode`), binding OVER a hook 'allow'
        // (HOOK.3 issue 1). Lock read-and-dropped here. Deny-arm carry-overs from the
        // SOURCED resolution (`toolExecution.ts:1040`), since `Deny` carries only
        // `reason`: `reject_content_blocks` (top-level deny blocks, `ask` only) +
        // `deny_hook_says_retry` (classifier `{retry:true}`, `toolExecution.ts:1090`).
        // Both inert on normal denies.
        let mut reject_content_blocks: Vec<ContentBlock> = Vec::new();
        let mut deny_hook_says_retry = false;
        // Same carry-over pattern for the denial provenance: `PermissionDecision`
        // collapses to `Deny { reason }`, so the structured signals
        // `tool_denial_kind` needs (`behavior_ask` / `decision_reason_type` /
        // `decision_reason`) must be classified in the SOURCED arm and carried
        // out to the emit site. "permission-rule" is the oracle's own fallthrough
        // and stays correct for every arm that carries no classifier reason
        // (hook Block, plan-mode, unknown).
        let mut denial_kind: &'static str = "permission-rule";
        let plan_mode = orch.session.lock().await.plan_mode;
        // ORPHAN RECOVERY: a re-dispatched orphaned tool carries a forced
        // permission decision (its recovered `control_response`) that REPLACES the
        // interactive gate — twin of claude-code's forced `canUseTool` in
        // `handleOrphanedPermission` (queryHelpers.ts:278-284). Consumed (removed)
        // on read so it binds exactly this `tool_use` once. The map is empty on
        // every normal turn, so this is a strict no-op there (byte-locked
        // turn-loop fixtures unchanged). PreToolUse hooks above STILL ran (so do
        // claude-code's, via `runTools`); only the permission decision is forced.
        let forced_decision = orch
            .orphan_forced_decisions
            .lock()
            .await
            .remove(tool_use_id);
        // OTEL `code_edit_tool.decision` / `tool_decision` source label,
        // threaded out of the decision branches below.
        //
        // claude-code has two publishers: `qtd` (driven by the permission
        // checker's own `logDecision`, which also records
        // `toolDecisions[toolUseID]`) and the dispatch-site one, guarded on
        // `toolDecisions?.[t] === void 0`, which derives the label from the
        // structured decision reason with `eQ_`. The port has no `logDecision`
        // twin — no gate emits OTEL and nothing writes a `toolDecisions`
        // record — so this site, which is the dispatch-site publisher's twin,
        // always fires and `eQ_` is the whole taxonomy:
        //   rule                       → `ZX_` (see [`rule_decision_otel_source`])
        //   hook                       → "hook"
        //   permissionPromptTool       → the host's `decisionClassification`,
        //                                defaulting to user_temporary/user_reject
        //   other (request aborted)    → "user_abort"
        //   mode/classifier/…/no reason→ "config"
        let mut decision_otel_source: &'static str = "config";
        let decision = if let Some(forced) = forced_decision {
            // A recovered orphan's forced `control_response` — no structured
            // decision reason survives the recovery (CC default arm).
            decision_otel_source = "unknown";
            forced
        } else if plan_mode {
            let plan_decision = orch.perms.check_in_plan_mode(name, &effective_input).await;
            // GATE-SYSMSG-01: a plan-mode mutation deny is a LOCAL deny the oracle
            // emits `permission_denied` for. `check_in_plan_mode` is 2-valued, so
            // the structured reason (mode vs a plan-visible deny rule) is not
            // available here — emit the message with the discriminants omitted
            // (still a valid oracle subsequence). No-op on non-stdio transports.
            if let PermissionDecision::Deny { reason } = &plan_decision {
                let sysmsg_ctx = traits::permission_gate::PermissionCheckContext {
                    tool_use_id: Some(tool_use_id.to_string()),
                    ..Default::default()
                };
                orch.perms
                    .on_permission_denied(name, &sysmsg_ctx, None, None, reason)
                    .await;
            }
            plan_decision
        } else if hook_allowed {
            // Carry the REAL tool_use_id so a hook-allow→ask-rule re-check emits a
            // byte-faithful stdio `can_use_tool` (correlatable id + decision_reason).
            let ctx = traits::permission_gate::PermissionCheckContext {
                tool_use_id: Some(tool_use_id.to_string()),
                ..Default::default()
            };
            let hook_outcome = orch
                .perms
                .check_after_hook_allow_outcome_ctx(name, &effective_input, &ctx)
                .await;
            let mut hook_decision_classification = None;
            let hook_decision = match hook_outcome {
                traits::permission_gate::PermissionOutcome::Allow {
                    updated_input,
                    decision_classification,
                    permission_updates: _,
                } => {
                    hook_decision_classification = decision_classification;
                    if let Some(updated) = updated_input {
                        effective_input = updated;
                    }
                    PermissionDecision::Allow
                }
                traits::permission_gate::PermissionOutcome::Deny { reason } => {
                    PermissionDecision::Deny { reason }
                }
            };
            // The hook only OWNS the label when its allow stands: `han` returns
            // the hook's own `{behavior:"allow"}` (decisionReason `hook`) there,
            // but when the re-check overrides it (`Hook returned '…' but deny
            // rule overrides`) the decision — and therefore the label — is the
            // RULE's, which `ZX_` renders as "config" for every non-user-owned
            // SettingSource. `PermissionDecision` is 2-valued, so the overriding
            // rule's own scope is not separable here.
            decision_otel_source = if matches!(hook_decision, PermissionDecision::Allow) {
                hook_decision_classification.map_or(
                    "hook",
                    traits::permission_gate::ToolDecisionClassification::as_str,
                )
            } else {
                "config"
            };
            hook_decision
        } else {
            // NORMAL permission path. Resolve the decision SOURCE first (without
            // delegating to the prompt transport) so the source-gated permission
            // hooks fire the way claude-code does.
            let resolution_ctx = traits::permission_gate::PermissionCheckContext {
                tool_use_id: Some(tool_use_id.to_string()),
                is_non_interactive_session: !orch.config.interactive_permissions,
                ..Default::default()
            };
            let resolution = orch
                .perms
                .resolve_detailed_or_abort(name, &effective_input, &resolution_ctx)
                .await
                .map_err(|abort| OrchestratorError::PermissionAbort {
                    message: abort.message,
                })?;
            // R-D3: a PreToolUse hook `permissionBehavior:"ask"` (HookDecision::Ask)
            // forces the interactive prompt even over a configured ALLOW rule, but a
            // DENY rule still overrides the hook. This is 1:1 with claude-code's
            // `applyHookPermissionResult` (`JWn`): on a hook `ask`/`allow` it RE-RUNS
            // the rule resolution (`EPe`) and `if (p?.behavior === "deny") return …
            // "deny rule overrides"`, so the deny rule wins; only when no deny rule
            // matches does the hook `ask` fall through to the full permission pipeline
            // (the interactive prompt). Here `resolve_detailed` has already applied
            // that rule precedence, so upgrading ONLY the resolved `Allow` to `Ask`
            // reproduces it exactly: a resolved `Deny` keeps binding (the deny rule
            // overrides), plan mode already bound above, and a resolved `Ask` already
            // prompts. Precedence is therefore deny > ask > allow — matching the
            // binary, NOT a divergence. No-op unless a hook returned `ask`.
            let requires_user_interaction = tool_handle.requires_user_interaction();
            // This flag is metadata for an existing Ask/callback path; it must
            // not create an Ask by itself. The normal TUI tool owns its question
            // UI, and a generic permission prompt here would duplicate it.
            let resolution = if hook_ask && matches!(resolution, PermissionResolution::Allow { .. })
            {
                PermissionResolution::Ask
            } else {
                resolution
            };
            let ask_reason_context = match &resolution {
                PermissionResolution::AskWithContext {
                    decision_reason_type,
                    decision_reason,
                } => (decision_reason_type.clone(), decision_reason.clone()),
                _ => (None, None),
            };
            match resolution {
                PermissionResolution::Allow { rule_source } => {
                    decision_otel_source = rule_decision_otel_source(rule_source.as_deref(), true);
                    PermissionDecision::Allow
                }
                PermissionResolution::Deny {
                    reason,
                    source,
                    rule_source,
                    decision_reason_type,
                    decision_reason,
                    behavior_ask,
                    content_blocks,
                } => {
                    decision_otel_source = rule_decision_otel_source(rule_source.as_deref(), false);
                    // Classify the denial for the stream-json `tool_result_meta`
                    // while the structured provenance is still in scope — the
                    // `PermissionDecision::Deny` this arm returns keeps only `reason`.
                    denial_kind = tool_denial_kind(
                        behavior_ask,
                        decision_reason_type.as_deref(),
                        decision_reason.as_deref(),
                    );
                    // `ask`-behavior rejection contentBlocks (`toolExecution.ts:1040-1043`):
                    // claude-code appends `permissionDecision.contentBlocks` to the deny
                    // user message at top level ONLY when `behavior === 'ask'`. Carry them
                    // to the deny arm via the outer local. DORMANT in the external build —
                    // no gate produces an `ask`+contentBlocks rejection, so this stays empty
                    // and the deny message is byte-identical to today.
                    if behavior_ask {
                        reject_content_blocks = content_blocks;
                    }
                    // HOOK.3 issue 3 — the PermissionDenied hook (claude-code
                    // `executePermissionDeniedHooks`, fired from
                    // `toolExecution.ts:1075`) fires ONLY on an auto-mode CLASSIFIER
                    // deny (`decisionReason.type === 'classifier'`), NOT on a
                    // rule/mode/plan deny. LingXi now wires a deterministic
                    // auto-mode classifier, so classifier-source denies can
                    // reach this path in normal builds.
                    if matches!(source, PermissionDecisionSource::Classifier) {
                        // NOTE: the OTEL label stays "config" — `eQ_` groups
                        // `classifier` with `mode`/`safetyCheck`/… in the
                        // "config" arm. (`fI_` does have a "classifier" arm, but
                        // it needs a `logDecision({source:{type:"classifier"}})`
                        // and 2.1.220 has no such call site — every
                        // `source:{type:…}` there is user/user_reject/
                        // user_abort/hook.)
                        let denied_event = HookEvent::PermissionDenied {
                            tool_name: name.clone(),
                            tool_input: effective_input.clone(),
                            tool_use_id: tool_use_id.clone(),
                            reason: reason.clone(),
                        };
                        let denied_agg = orch.hooks.execute(denied_event, hook_ctx.clone()).await;
                        // `{retry: true}` reply (`toolExecution.ts:1080-1091`): a
                        // PermissionDenied hook can signal the auto-mode classifier
                        // deny is now approved. We honour it when classifier
                        // permissions are enabled, or when the runtime config bit
                        // forces the transcript-classifier path in tests.
                        let classifier_feature_on =
                            permission::classifier::is_classifier_permissions_enabled()
                                || orch.config.transcript_classifier_enabled;
                        if classifier_feature_on && denied_agg.retry {
                            deny_hook_says_retry = true;
                        }
                    }
                    // GATE-SYSMSG-01: emit the `permission_denied` system message on
                    // the stdio outbound. The MAIN-conversation deny path resolves
                    // via `resolve_detailed` (source-first, for the source-gated
                    // hooks), NOT `check_with_context`, so the gate's own
                    // `decide_outcome_with_context` emission (subagent dispatch) is
                    // never reached here — emit through the outer gate, which
                    // forwards to the stdio transport. No-op on non-stdio transports.
                    let sysmsg_ctx = traits::permission_gate::PermissionCheckContext {
                        tool_use_id: Some(tool_use_id.to_string()),
                        ..Default::default()
                    };
                    orch.perms
                        .on_permission_denied(
                            name,
                            &sysmsg_ctx,
                            decision_reason_type.as_deref(),
                            decision_reason.as_deref(),
                            &reason,
                        )
                        .await;
                    PermissionDecision::Deny { reason }
                }
                PermissionResolution::Ask | PermissionResolution::AskWithContext { .. } => {
                    // HOOK.3 issue 2 — the gate is ABOUT TO ASK. Fire the
                    // PermissionRequest hook FIRST (claude-code
                    // `runPermissionRequestHooksForHeadlessAgent`, fired on the ask
                    // path before the fallback resolution). A hook 'allow' RESCUES
                    // the call — resolved via `check_after_hook_allow` so explicit
                    // deny rules still bind (a PermissionRequest 'allow', like a
                    // PreToolUse 'allow', skips only the PROMPT), applying any
                    // `updatedInput`; a hook 'deny' denies; otherwise we delegate to
                    // the inner transport (interactive prompt, or a headless
                    // auto-deny). Strict no-op when no PermissionRequest hook is
                    // registered → the inner transport resolves exactly as before.
                    let req_event = HookEvent::PermissionRequest {
                        tool_name: name.clone(),
                        tool_input: effective_input.clone(),
                        reason: format!("Tool {name} requires permission"),
                    };
                    let req_agg = orch.hooks.execute(req_event, hook_ctx.clone()).await;
                    match req_agg.decision {
                        Some(HookDecision::Approve | HookDecision::Allow) => {
                            // (cc 2.1.218 `Fxy`) The headless PermissionRequest
                            // rescue re-checks the rules (`epr(_pt(...))`, where an
                            // ask rule becomes a HARD DENY — no prompt is available
                            // on this surface) ONLY when the hook supplied
                            // `updatedInput` OR the tool `requiresUserInteraction`:
                            //   if(a.updatedInput||e.requiresUserInteraction?.()){…}
                            //   return {behavior:"allow", updatedInput:l, …}
                            // With NEITHER trigger the allow STANDS UNCHECKED — we
                            // must NOT re-run the rule/mode verdict, or the rescue is
                            // defeated in its primary use case (an ordinary ask rule
                            // the hook meant to pre-approve would re-prompt / hard
                            // deny). Both the reachable ask rule and a deny rule were
                            // already resolved before this Ask branch, so honouring
                            // the unchanged input directly is safe.
                            let rewritten = req_agg.modified_input.is_some();
                            if let Some(updated) = req_agg.modified_input {
                                effective_input = updated;
                            }
                            let hook_decision = if rewritten || requires_user_interaction {
                                // Rewritten input, or a tool that requires user
                                // interaction: re-check via `epr(_pt(...))` — an ask
                                // becomes a hard deny carrying the ask's `c.message`
                                // (identical for both triggers), so the rewritten
                                // resolver serves both.
                                orch.perms
                                    .check_after_hook_allow_rewritten(name, &effective_input)
                                    .await
                            } else {
                                // Standing allow (`Fxy`'s no-recheck arm): honour the
                                // allow directly, keeping the auto-mode non-deny
                                // bookkeeping every allow arm records — mode-less, no
                                // rule re-check, no backstop.
                                orch.perms.honour_hook_allow(name, &effective_input).await
                            };
                            // `handleHookAllow` logs `source:{type:"hook"}`, but the
                            // `updatedInput` re-check that DENIES logs
                            // `{decision:"reject",source:"config"}` instead — the hook
                            // owns the label only while its allow stands.
                            decision_otel_source =
                                if matches!(hook_decision, PermissionDecision::Allow) {
                                    "hook"
                                } else {
                                    "config"
                                };
                            hook_decision
                        }
                        Some(HookDecision::Block) => {
                            decision_otel_source = "hook";
                            PermissionDecision::Deny {
                                reason: req_agg
                                    .reason
                                    .unwrap_or_else(|| "permission denied by hook".into()),
                            }
                        }
                        _ => {
                            // Delegate to the inner prompt transport, carrying the
                            // REAL tool_use_id (so a stdio `can_use_tool` request is
                            // byte-faithful) and applying the host's `updatedInput`
                            // rewrite to the input the tool actually runs with.
                            let ctx = traits::permission_gate::PermissionCheckContext {
                                tool_use_id: Some(tool_use_id.to_string()),
                                requires_user_interaction,
                                // HOOK-ASKFLOOR-03: a PreToolUse hook `ask` sets the
                                // floor so the Auto classifier can't re-allow past it
                                // (policy_gate Ask arm gates the classifier on this).
                                hook_ask_floor: hook_ask,
                                is_non_interactive_session: !orch.config.interactive_permissions,
                                decision_reason_type: ask_reason_context.0.clone(),
                                decision_reason: ask_reason_context.1.clone(),
                                ..Default::default()
                            };
                            match orch
                                .perms
                                .check_with_context(name, &effective_input, &ctx)
                                .await
                            {
                                traits::permission_gate::PermissionOutcome::Allow {
                                    updated_input,
                                    // `permission_updates` (the host's
                                    // `updatedPermissions`) are applied + persisted
                                    // inside the stdio gate itself, which holds the
                                    // settings paths.
                                    permission_updates: _,
                                    decision_classification,
                                } => {
                                    // The host's explicit classification wins when
                                    // valid; absent/unknown values were normalized to
                                    // `None` by the transport and use Claude's
                                    // temporary-allow fallback.
                                    decision_otel_source = decision_classification.map_or(
                                        "user_temporary",
                                        traits::permission_gate::ToolDecisionClassification::as_str,
                                    );
                                    if let Some(u) = updated_input {
                                        effective_input = u;
                                    }
                                    PermissionDecision::Allow
                                }
                                traits::permission_gate::PermissionOutcome::Deny { reason } => {
                                    // An ABORTED prompt is a distinct label: claude-code
                                    // denies with `decisionReason: iYt` ("tool permission
                                    // request aborted") when `signal.aborted`, and `eQ_`
                                    // maps that `other` reason to "user_abort" (the
                                    // interactive twin is the prompt's `case "cancelled"`
                                    // → `source:{type:"user_abort"}`). The gate folds both
                                    // into `Deny`, so the turn's cancel token — the same
                                    // signal the stdio gate raced to produce this deny —
                                    // is what separates them.
                                    // Denial provenance: this arm keeps the
                                    // `permission-rule` fallthrough, and that is
                                    // CORRECT for the transport that can observe it.
                                    // claude-code's `JMn` (binary offset 246277535)
                                    // wraps a stdio `can_use_tool` result as
                                    // `{...hostResult, decisionReason:{type:
                                    // "permissionPromptTool", …}}`, PRESERVING the
                                    // host's `behavior`. So a host deny reaches the
                                    // kind classifier as `behavior === "deny"` with a
                                    // `permissionPromptTool` reason — neither the
                                    // `ask` branch nor the classifier branch — and
                                    // falls through to `permission-rule`.
                                    //
                                    // `user-rejected` means `behavior === "ask"`,
                                    // which is what the INTERACTIVE CLI prompt
                                    // returns (hence `userFeedback: behavior==="ask"
                                    // ? … : void 0` at offset 235412899). Real
                                    // transcripts from an interactive session are
                                    // therefore full of `user-rejected` — but
                                    // `tool_result_meta` is emitted only by the
                                    // stream-json transport, whose permission
                                    // transport is the stdio gate. Do NOT stamp
                                    // `user-rejected` here: this arm also covers
                                    // transport failure and a dropped response
                                    // channel, which claude-code maps to
                                    // `{type:"other"}` ⇒ `permission-rule` too.
                                    decision_otel_source =
                                        if cancel.as_ref().is_some_and(|t| t.is_cancelled()) {
                                            "user_abort"
                                        } else {
                                            "user_reject"
                                        };
                                    PermissionDecision::Deny { reason }
                                }
                            }
                        }
                    }
                }
            }
        };
        // OTEL: record the RESOLVED tool-permission decision — the CC
        // `XNr()?.add(1, await ICs(...))` counter (Edit/Write/NotebookEdit
        // only, gated inside) + the paired `tool_decision` `claude_code.events`
        // record (every tool). The path argument is the `getPath` twin: the
        // edit tools' `file_path` / NotebookEdit's `notebook_path` from the
        // input the decision was made on. Byte-noop when OTEL is off.
        telemetry::otel::record_tool_permission_decision(
            name,
            tool_use_id.as_str(),
            effective_input
                .get("file_path")
                .or_else(|| effective_input.get("notebook_path"))
                .and_then(serde_json::Value::as_str),
            if matches!(decision, PermissionDecision::Allow) {
                "accept"
            } else {
                "reject"
            },
            decision_otel_source,
            tool_handle.is_mcp(),
            Some(&effective_input),
        );
        match decision {
            PermissionDecision::Allow => {}
            PermissionDecision::Deny { reason } => {
                // Push the deny error `tool_result`. The PermissionDenied hook,
                // when applicable, already fired on the classifier-deny branch
                // above — claude-code fires it only for auto-mode classifier
                // denials, not for the rule/mode/plan denials that also reach here.
                // claude-code sends the permission deny message VERBATIM as the
                // tool_result content (e.g. "Permission to use Bash has been
                // denied." — built by the gate via `deny_reason_string`, or the
                // tool's explicit `explanation`), NOT wrapped in a
                // "Permission denied: " prefix.
                let result_block = ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: reason.clone(),
                    is_error: true,
                    provider_tool_use_id: provider_id.clone(),
                    content_blocks: None,
                };
                // Denial provenance (claude-code `toolDenialKind`), classified by
                // `tool_denial_kind` in the SOURCED resolution arm and carried
                // here via `denial_kind`. Transports that cannot carry it (all
                // but stream-json) inherit the trait default and ignore it.
                //
                // NOT yet distinguished: the stdio `can_use_tool` prompt-transport
                // deny (the `PermissionOutcome::Deny` arm below, which the port
                // labels `user_reject` / `user_abort` for OTEL) carries no
                // `behavior_ask`, so a host-side rejection currently reports the
                // `permission-rule` fallthrough instead of `user-rejected`.
                // Same provenance on BOTH surfaces: the stream-json frame gets
                // `tool_result_meta` via the emit below, and the persisted
                // transcript line gets the message-level `toolDenialKind` via
                // this record, consumed when the tool_result user line is
                // written.
                orch.record_tool_denial_kind(tool_use_id, denial_kind).await;
                // O1: claude's permission-deny arm (2.1.220 BIN off 235400200)
                // stamps `` toolUseResult: `Error: ${denyMessage}` `` — the
                // deny message with an `Error: ` prefix, while the model
                // content carries it wrapped in `<tool_use_error>`. LingXi
                // sends the deny message verbatim as the model content, so only
                // the persisted line gets the prefix.
                orch.record_tool_use_result(
                    tool_use_id,
                    serde_json::Value::String(format!("Error: {reason}")),
                )
                .await;
                orch.emit_tool_result_frame(
                    tool_use_id,
                    name,
                    &reason,
                    &serde_json::json!({ "error": reason }),
                    Some(denial_kind),
                )
                .await;
                results.push(result_block);
                // `ask`-behavior rejection contentBlocks (`toolExecution.ts:1039-1046`):
                // append the image/non-text blocks at the TOP LEVEL of the deny
                // user message — alongside, NOT inside, the text-only tool_result
                // (which rejects non-text when `is_error` is set). They join
                // `results`, which IS this turn's tool_result user message content,
                // so they land in the same message as the tool_result, exactly like
                // claude-code's `messageContent.push(...rejectContentBlocks)`.
                //
                // imagePasteId residual: claude-code assigns sequential
                // `imagePasteIds` via `getNextImagePasteId` (max prior id + 1, one
                // per image) — a TUI RENDER LABEL on the user message
                // (`messages.ts:801`). LingXi's `ConversationMessage::User` models no
                // `imagePasteIds` field (the same gap as `isMeta`; both are
                // display-only, never sent to the model and never written to JSONL),
                // so there is no home to store the id. The image BLOCKS themselves
                // are carried faithfully; the per-image label is the documented
                // residual. DORMANT: empty on every normal deny, so this loop is a
                // strict no-op and the common deny message is byte-identical.
                for block in reject_content_blocks {
                    results.push(block);
                }
                // HOOK.1: even on a permission DENY, claude-code's pre-hook
                // phase already pushed the PreToolUse `additionalContext`
                // (`toolExecution.ts:846`) before the gate ran — so surface it
                // here, ordered AFTER this path's deny error tool_result. No-op
                // when the hook emitted no context.
                if let Some(msg) = pre_context_message {
                    injected_messages.push((msg, tool_use_id.clone()));
                }
                // PermissionDenied-hook `{retry: true}` (`toolExecution.ts:1092-1099`):
                // after the deny user message, push a SECOND `isMeta` user message
                // with the verbatim approval-to-retry string. DOUBLE-GATED upstream
                // (the `deny_hook_says_retry` flag is set only when BOTH the
                // `TRANSCRIPT_CLASSIFIER` feature is on AND a classifier-source deny
                // ran a `PermissionDenied` hook that returned `{retry: true}`), so it
                // is DORMANT on the normal deny path — `deny_hook_says_retry` is
                // `false` there and this is a strict no-op. Built as a META user
                // message (CC `createUserMessage({…, isMeta:!0})`), so when it does
                // fire it persists with top-level `isMeta:true`.
                if deny_hook_says_retry {
                    let retry_msg = ConversationMessage::user_meta(
                        MessageId::new(),
                        PERMISSION_DENIED_RETRY_MESSAGE.to_string(),
                    );
                    injected_messages.push((retry_msg, tool_use_id.clone()));
                }
                continue;
            }
        }

        // #8 NOTE: the SubagentStart wire-event fire MOVED below — to the
        // post-`tool_handle.call()` site alongside `SubagentStop`. At this
        // pre-call point the spawn has not run yet, so the child's REAL pool
        // `AgentId` does not exist; firing here forced a fresh divergent id.
        // The Agent tool now surfaces the child id on its result
        // `data.agentId` (C1 seam: `SubagentResult` carries the real id back),
        // so BOTH SubagentStart and SubagentStop fire post-call with that one
        // canonical id — matching claude-code's single `agentId`
        // (runAgent.ts:347). The fire-only-on-actual-spawn semantics are
        // preserved: a pre-hook Block / permission denial `continue`s above
        // before `tool_handle.call()`, so no subagent spawns and neither event
        // fires.

        // Progress channel: drained CONCURRENTLY with the tool call. The Agent
        // tool forwards a `{"subagent_activity": "<line>"}` payload per nested
        // subagent tool call; re-emit each as `emit_subagent_activity` so the
        // subagent's work renders under its Task cell. Other tools send nothing,
        // so this is a no-op for them. The consumer exits when the tool drops
        // `progress_tx` (call returns).
        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::channel::<tool_api::progress::ToolProgress>(64);
        let progress_output = orch.output.clone();
        // The spawning Task tool_use_id — stamped as `parent_tool_use_id` on any
        // forwarded subagent assistant frame (`--forward-subagent-text`). Uses
        // the same `ToolUseId::as_str` form the stream-json tool_use block id
        // carries, so a forwarded child frame correlates to its parent Task call.
        let progress_parent_tool_use_id = tool_use_id.as_str().to_string();
        // A JoinSet owns every auxiliary event producer. Dropping this dispatch
        // scope (runtime shutdown, panic, or future host cancellation changes)
        // aborts the children instead of detaching them and allowing stale
        // heartbeat/progress events to escape into a later turn.
        let mut event_tasks = tokio::task::JoinSet::new();
        let event_tasks_done = tokio_util::sync::CancellationToken::new();
        let progress_done = event_tasks_done.clone();
        event_tasks.spawn(async move {
            loop {
                tokio::select! {
                    () = progress_done.cancelled() => {
                        // A tool is allowed to retain a progress sender in work it
                        // spawned. Close the receiver so those detached producers
                        // cannot keep this turn alive, then drain progress that was
                        // already accepted before the tool reached its terminal state.
                        progress_rx.close();
                        while let Some(progress) = progress_rx.recv().await {
                            forward_tool_progress(
                                progress_output.as_ref(),
                                &progress_parent_tool_use_id,
                                progress,
                            ).await;
                        }
                        break;
                    }
                    progress = progress_rx.recv() => {
                        let Some(progress) = progress else { break; };
                        forward_tool_progress(
                            progress_output.as_ref(),
                            &progress_parent_tool_use_id,
                            progress,
                        ).await;
                    }
                }
            }
        });
        // Periodic tool heartbeat for long-running calls: transports that care
        // can surface "still running" state between ToolCall and ToolResult,
        // while sinks that ignore it keep the default no-op behavior.
        let heartbeat_output = orch.output.clone();
        let heartbeat_id = tool_use_id.clone();
        let heartbeat_tool = name.to_string();
        let _heartbeat_cancel = cancel.clone();
        let heartbeat_done = event_tasks_done.clone();
        let heartbeat_started = std::time::Instant::now();
        event_tasks.spawn(async move {
            // (review #10) `interval` fires its FIRST tick immediately, which
            // would emit a spurious `elapsed_ms≈0` heartbeat on EVERY tool call
            // (even instant ones), defeating the "long-running" intent. Start the
            // first tick one period out so heartbeats only fire for tools that
            // actually run >= 1s.
            let period = std::time::Duration::from_secs(1);
            let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    () = heartbeat_done.cancelled() => break,
                    _ = ticker.tick() => {
                        #[allow(clippy::cast_possible_truncation)]
                        let elapsed_ms = heartbeat_started.elapsed().as_millis() as u64;
                        #[cfg(debug_assertions)]
                        eprintln!(
                            "[turn-diagnostic] tool heartbeat id={} name={} elapsed_ms={} cancelled={}",
                            heartbeat_id.as_str(),
                            heartbeat_tool,
                            elapsed_ms,
                            _heartbeat_cancel
                                .as_ref()
                                .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
                        );
                        heartbeat_output
                            .emit_tool_heartbeat(&heartbeat_id, &heartbeat_tool, elapsed_ms)
                            .await;
                    }
                }
            }
        });

        // Time the tool dispatch ONLY (excludes the permission prompt above and
        // the Post hooks below) — surfaced to PostToolUse/Failure hooks as
        // `duration_ms` (claude-code 2.1.195).
        let tool_started = std::time::Instant::now();
        #[cfg(debug_assertions)]
        eprintln!(
            "[turn-diagnostic] tool call started id={} name={} cancel_present={} cancelled={}",
            tool_use_id.as_str(),
            name,
            ctx.cancel.is_some(),
            ctx.cancel
                .as_ref()
                .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
        );
        // `InterruptBehavior::Cancel` is a dispatcher contract, not merely a
        // suggestion that every tool implementation must remember to honor.
        // Some network-backed tools (notably client-side WebSearch) await an
        // HTTP future that does not observe `ToolUseContext.cancel`. Race that
        // future at the boundary so a user cancellation always drops it and
        // yields the same `ToolError::Aborted` path as a cooperative tool.
        // `Block` tools intentionally keep their existing wait-to-completion
        // behavior.
        let interrupt_behavior = tool_handle.interrupt_behavior(&effective_input);
        let dispatch_cancel = ctx.cancel.clone();
        let tool_outcome = {
            // Keep the call future in this inner scope. When cancellation wins,
            // leaving the scope drops the non-cooperative future (and its
            // progress sender) before we await the progress consumer below.
            let tool_call = tool_handle.call(effective_input.clone(), ctx, progress_tx);
            tokio::pin!(tool_call);
            match (interrupt_behavior, dispatch_cancel) {
                (tool_api::tool_trait::InterruptBehavior::Cancel, Some(cancel)) => {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => Err(tool_api::ToolError::Aborted),
                        outcome = &mut tool_call => outcome,
                    }
                }
                _ => tool_call.await,
            }
        };
        #[cfg(debug_assertions)]
        eprintln!(
            "[turn-diagnostic] tool call returned id={} name={} elapsed_ms={} outcome={}",
            tool_use_id.as_str(),
            name,
            tool_started.elapsed().as_millis(),
            if tool_outcome.is_ok() { "ok" } else { "error" }
        );
        event_tasks_done.cancel();
        // Drain buffered progress and stop the heartbeat before publishing the
        // terminal tool result. The JoinSet aborts both tasks automatically if
        // this dispatch future is dropped by a parent turn/runtime shutdown.
        while event_tasks.join_next().await.is_some() {}
        #[allow(clippy::cast_possible_truncation)]
        let tool_duration_ms = tool_started.elapsed().as_millis() as u64;

        let (content, is_error, emit_payload, is_abort) = match tool_outcome {
            Ok(result) => {
                let text = result
                    .model_content
                    .clone()
                    .unwrap_or_else(|| tool_result_to_model_text(&result.data));
                // SKILLEXEC.3 (Part A): stash any tool-injected conversation
                // messages so the caller can append them after this batch's
                // tool_result user message. Non-empty only for the Skill tool
                // (the expanded skill prompt); empty for every other tool, so
                // the locked turn-loop fixtures stay byte-identical. Each is
                // paired with THIS tool's `tool_use_id` (TS
                // `tagMessagesWithToolUseID` stamps the Skill tool's own block
                // id as `sourceToolUseID`) for the caller's in-memory
                // `injected_message_sources` side-table.
                injected_messages.extend(
                    result
                        .new_messages
                        .into_iter()
                        .map(|m| (m, tool_use_id.clone())),
                );
                // SKILLEXEC.3 (model scope): stash any one-shot `context_modifier`
                // for the caller to fold POST-BATCH. NOT applied to the per-tool
                // `ctx` here (which is discarded at loop end) and NOT applied
                // per-tool — folding after the whole batch gives the concurrent
                // streaming path a single, race-free application point. `None`
                // for every existing tool + skills WITHOUT a `model:` frontmatter,
                // so this is a strict no-op there (byte-identical).
                if let Some(modifier) = result.context_modifier {
                    context_modifiers.push(modifier);
                }
                // O1: claude stamps the tool's RAW STRUCTURED result on the
                // persisted `tool_result` user line as `toolUseResult`
                // (2.1.220 BIN off 235420375: `toolUseResult: gt` where
                // `gt = se.data`) — NOT the model-facing string. Same value the
                // stream-json frame carries; recorded against the tool_use id
                // and consumed when the user line is persisted.
                orch.record_tool_use_result(tool_use_id, result.data.clone())
                    .await;
                // O1: an MCP server's `_meta`/`structuredContent` passthrough
                // rides as the TOP-LEVEL `mcpMeta` sibling. `Uks(agentId, meta)`
                // (BIN off 232969604) returns it verbatim on the main chain,
                // which is the only chain this orchestrator serves.
                if let Some(meta) = result.mcp_meta.clone() {
                    orch.record_tool_use_mcp_meta(tool_use_id, meta).await;
                }
                // `is_error` rides on the result (set by MCP tools from the
                // server's `isError`; `false` for every native success). A native
                // FAILURE is an `Err` handled below — this Ok arm only flags an
                // MCP logical-error RESULT.
                (text, result.is_error, result.data, false)
            }
            Err(err) => {
                // Bare error string — no <tool_use_error> wrapper.
                // claude-code/src/services/tools/toolExecution.ts:1691 does:
                //   const content = formatError(error)   // bare, from utils/toolErrors.ts
                // and feeds it raw into tool_result.content (line 1721).
                // Only pre-execution paths (unknown-tool, schema validation) wrap.
                //
                // The model-facing content uses `model_facing_message()` — the
                // BARE inner message (claude's `error.message`) — NOT the
                // `Display` form, which would leak a LingXi-internal variant
                // prefix (`invalid input: ` / `internal: `) into the wire bytes.
                //
                // O4-A: claude-code's `oQ_` catch (2.1.220 @235424972) also
                // stamps `toolDenialKind: YDd(err, signal)` on this very frame.
                // `YDd` (@235394375) returns a kind ONLY for an AbortError
                // (`tl`), an interrupted `ShellError` (`hW`), or an
                // abort-signalled `$7e`; LingXi's 1:1 analog of `tl` is
                // `ToolError::Aborted`. The `hW.interrupted` branch has NO port
                // analog today — `tools/shell/src/bash.rs` returns
                // `Ok(build_interrupted_result())` for a killed shell rather
                // than an `Err`, so it never reaches here; that branch is
                // deliberately UNMODELED. `YDd`'s `background` abort reason
                // (which maps to `"cancelled"`) likewise has no LingXi
                // equivalent, so every LingXi abort takes the `interrupted`
                // branch.
                let is_abort = matches!(err, tool_api::ToolError::Aborted);
                let bare = err.model_facing_message();
                let text = format!("Error: {bare}");
                // O1: on the ERROR arm claude stores the plain STRING
                // `` `Error: ${ae}` `` in `toolUseResult` (2.1.220 BIN off
                // 235424595), NOT a structured object. The `{"error": …}`
                // object below is the port's stream-json SDK frame — a
                // different wire that legitimately differs here.
                orch.record_tool_use_result(tool_use_id, serde_json::Value::String(text.clone()))
                    .await;
                (text, true, serde_json::json!({ "error": bare }), is_abort)
            }
        };

        if is_abort {
            // Denial provenance for an aborted tool. `record_tool_denial_kind`
            // feeds the persisted `tool_result` user line's `toolDenialKind`
            // (via `take_tool_denial_kind`); `emit_tool_result_denied` carries
            // the same kind on the stream-json frame. Same shape as the
            // hardcoded `"cancelled"` on the pre-cancel guard above.
            orch.record_tool_denial_kind(tool_use_id, "interrupted")
                .await;
            orch.emit_tool_result_frame(
                tool_use_id,
                name,
                &content,
                &emit_payload,
                Some("interrupted"),
            )
            .await;
        } else {
            orch.emit_tool_result_frame(tool_use_id, name, &content, &emit_payload, None)
                .await;
        }

        // (code-change stats for /usage — claude-code `Bhn(added, removed)`)
        // Only file-edit tools (Edit/Write/MultiEdit) put a `structuredPatch`
        // in their result data; sum its +/- lines into the session counters.
        accumulate_code_change(&emit_payload, orch.cost_tracker.as_ref()).await;

        // NOTE: the read-file-state registry (`context.readFileState`, backing
        // `/files`, conditional-rule matching, and the relevant-memory dedup) is
        // populated by the file tools themselves via `readFileState.set`
        // (Read/Edit/Write/MultiEdit/NotebookEdit) over the shared `Arc` the
        // composition root hands their `BuiltinToolContext` — 1:1 with
        // claude-code's single per-session map. The orchestrator no longer keeps
        // a separate insertion-ordered `Vec`, so there is nothing to record here.

        // M5-06 Task 14 + hooks B-tool-failure: the post-dispatch hook chain.
        // Byte-faithful to claude-code's split: a SUCCESSFUL tool result fires
        // `PostToolUse` (`executePostToolUseHooks`), a FAILED one fires
        // `PostToolUseFailure` (`executePostToolUseFailureHooks`,
        // `utils/hooks.ts:3492`) — never both. The `is_error` flag here is the
        // same `is_error` that lands on the `ToolResult` block (TS keys off the
        // tool result's `is_error`). Best-effort for BOTH arms — a Post hook's
        // `system_messages` are appended to the result text, but a hook failure
        // does NOT mutate `content` or `is_error`.
        //
        // The `PostToolUseFailure` variant carries `tool_name` / `tool_use_id`
        // (matching the prior `PreToolUse`) + the dispatched `tool_input` (the
        // same `effective_input` the `PostToolUse` success arm threads) + the
        // stringified `error`. We pass the raw error string the tool returned
        // (the `{"error": …}` envelope value = `format!("{err}")`), NOT the
        // `"Error: "`-prefixed model-facing `content`, mirroring the TS
        // `PostToolUseFailure` input's `error`.
        let post_event = if is_error {
            let error = emit_payload
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| content.clone(), ToString::to_string);
            HookEvent::PostToolUseFailure {
                tool_name: name.clone(),
                tool_input: effective_input.clone(),
                error,
                tool_use_id: tool_use_id.clone(),
                duration_ms: Some(tool_duration_ms),
            }
        } else {
            HookEvent::PostToolUse {
                tool_name: name.clone(),
                tool_input: effective_input.clone(),
                tool_output: emit_payload.clone(),
                tool_use_id: tool_use_id.clone(),
                duration_ms: Some(tool_duration_ms),
            }
        };
        // O2: the identity the post-hook attachment and model-facing records carry.
        // both off the event it actually fired, so the failure path renders as
        // `PostToolUseFailure:{tool}` (BIN off 234728254 / 234728470), not
        // `PostToolUse:{tool}`.
        let post_hook_event = if is_error {
            "PostToolUseFailure"
        } else {
            "PostToolUse"
        };
        let post_hook_name = format!("{post_hook_event}:{name}");
        let post_started = std::time::Instant::now();
        tracing::info!(
            event = orch_events::HOOK_POST_STARTED,
            tool_name = %name,
        );
        telemetry::otel::emit_hook_lifecycle("post", "started", &name, None);
        let post_agg = orch.hooks.execute(post_event, hook_ctx.clone()).await;
        // hook duration bounded by tokio timeout — u128 ms cannot exceed u64::MAX
        #[allow(clippy::cast_possible_truncation)]
        let post_dur_ms = post_started.elapsed().as_millis() as u64;
        let mut post_additional_contexts = post_agg.additional_contexts.clone();
        if let Some(notice) = memdir_index_notice_for_tool(orch, &name, &effective_input, is_error)
        {
            if let Some(bus) = orch.analytics_bus.as_ref() {
                let mut metadata = telemetry::LogEventMetadata::new();
                metadata.insert(
                    "over_cap".into(),
                    telemetry::AnalyticsValue::Bool(notice.over_cap),
                );
                bus.log_event(memory::TENGU_MEMDIR_ENTRYPOINT_NEAR_CAP, metadata)
                    .await;
            }
            post_additional_contexts.push(notice.text);
        }

        // #40 terminalSequence apply for the post-dispatch aggregate (claude-code
        // `szn` runs per hook result, all event types). Same as the PreToolUse
        // side: validate, warn on rejection, and write accepted bytes through
        // the active terminal bridge.
        apply_terminal_sequence(orch, &name, post_agg.terminal_sequence.as_deref()).await;

        // FIX C (hook_stopped_continuation, PostToolUse twin): a PostToolUse
        // hook's `continue:false` (preventContinuation) becomes its OWN meta
        // message — claude yields it AFTER the tool_result (`toolHooks.ts:118-130`)
        // using `stopReason || 'Execution stopped by PostToolUse hook'` and
        // hookName `PostToolUse:{tool}`, then RETURNS (before any additionalContext),
        // so we queue it BEFORE the additionalContext loop below. Tagged with this
        // tool's `tool_use_id`; injected messages are appended after the
        // tool_result by both drivers, matching claude's ordering. `post_agg.reason`
        // carries the parsed `stopReason` (`hook_payload.rs:1113`). Strict no-op
        // when the hook did not request preventContinuation.
        // O2: `hook_blocking_error`. The oracle's PostToolUse consumer
        // (BIN off 234726074) re-emits the runner's bare `{blockingError}`
        // signal as an attachment, positioned AFTER the pass-through run record
        // and BEFORE the `preventContinuation` yield — so this block sits above
        // the stopped-continuation one.
        //
        // The EXECUTOR deliberately publishes nothing on a blocking run (its
        // `build_run_attachment` returns `None` for a `Block` decision, matching
        // BIN off 237805098, where the exit-2 arm yields no `message`); the
        // CALLER owns this record. Do not move it into the executor.
        //
        // Unlike almost every other hook attachment, this one IS model-facing:
        // the normalizer renders it as an `isMeta` user message
        // (BIN off 238107476). Previously `post_agg.decision` was never read
        // here, so a blocking PostToolUse hook produced nothing at all.
        if matches!(
            post_agg.decision,
            Some(hooks::response::HookDecision::Block)
        ) {
            let err = hooks::BlockingError {
                // `e.reason || "Blocked by hook"` (BIN off 237775430). On the
                // plain-text exit-2 arm the executor already parked the fully
                // rendered `[{display}]: {stderr}` string in `reason`.
                blocking_error: post_agg
                    .reason
                    .clone()
                    .unwrap_or_else(|| "Blocked by hook".to_string()),
                // Frozen at the first blocker alongside `reason`; the executor
                // picks `iSe` vs `qq` per arm.
                command: post_agg.block_command.clone().unwrap_or_default(),
            };
            orch.queue_hook_attachment(
                tool_use_id,
                hooks::blocking_error_attachment(
                    &hooks::HookAttachmentIdentity {
                        hook_name: post_hook_name.clone(),
                        hook_event: post_hook_event.to_string(),
                        tool_use_id: tool_use_id.as_str().to_string(),
                    },
                    &err,
                ),
            )
            .await;
            let body = hooks::blocking_error_prose(&post_hook_name, &err);
            injected_messages.push((
                ConversationMessage::user_meta(
                    MessageId::new(),
                    format!("<system-reminder>\n{body}\n</system-reminder>"),
                ),
                tool_use_id.clone(),
            ));
        }

        if post_agg.prevent_continuation {
            let reason = post_agg
                .reason
                .clone()
                .unwrap_or_else(|| format!("Execution stopped by {post_hook_event} hook"));
            // O2: the PERSISTED record. The model-facing prose below was
            // already byte-correct, but nothing reached the transcript —
            // the oracle yields a `hook_stopped_continuation` attachment
            // (BIN off 234726408) whose `message` sits SECOND in key order.
            orch.queue_hook_attachment(
                tool_use_id,
                hooks::stopped_continuation_attachment(
                    &hooks::HookAttachmentIdentity {
                        hook_name: post_hook_name.clone(),
                        hook_event: post_hook_event.to_string(),
                        tool_use_id: tool_use_id.as_str().to_string(),
                    },
                    &reason,
                ),
            )
            .await;
            injected_messages.push((
                ConversationMessage::user_meta(
                    MessageId::new(),
                    format!(
                        "<system-reminder>\n{post_hook_name} hook stopped continuation: {reason}\n</system-reminder>"
                    ),
                ),
                tool_use_id.clone(),
            ));
        }

        // HOOK.1 (additionalContext, PostToolUse twin): a PostToolUse hook's
        // `additionalContext` is ALSO a separate `hook_additional_context`
        // attachment in claude-code (`toolHooks.ts:133-143`), injected AFTER the
        // tool_result — exactly what the injected-messages seam does. The
        // hookName prefix follows the actual fired event. `systemMessage` stays folded
        // (handled by `final_content` below); only additionalContext splits out.
        // Strict no-op when no PostToolUse hook returned additionalContext.
        //
        // O3: claude emits ONE attachment carrying the whole `content` ARRAY
        // (BIN off 234726655), not one per entry; the renderer joins them with
        // `\n` into a single `<system-reminder>` message. The port keeps its
        // per-entry renderings on the injected channel (same model bytes when
        // there is one entry, which is every observed case) but the PERSISTED
        // record is the single attachment queued below.
        if !post_additional_contexts.is_empty() {
            orch.queue_hook_attachment(
                tool_use_id,
                hooks::additional_context_attachment(
                    &post_hook_name,
                    tool_use_id.as_str(),
                    post_hook_event,
                    &post_additional_contexts,
                ),
            )
            .await;
        }
        for ctx in &post_additional_contexts {
            let wrapped = format!(
                "<system-reminder>\n{post_hook_name} hook additional context: {ctx}\n</system-reminder>"
            );
            // `user_meta`: the rendering is `zr({isMeta:true})` and is
            // ephemeral — the attachment line above is the on-disk record.
            injected_messages.push((
                ConversationMessage::user_meta(MessageId::new(), wrapped),
                tool_use_id.clone(),
            ));
        }

        // PostToolUse `updatedToolOutput` (#38, all-tools) + `updatedMCPToolOutput`
        // (legacy, MCP-only) may REPLACE a SUCCESSFUL tool's output. claude yields
        // all-tools first, MCP second so MCP overrides (BIN off 202157140); applies
        // only if `outputSchema` is absent or validates (BIN off 202169384), else
        // keeps original + emits `hook_error_during_execution` (BIN off 202465455).
        // The substituted JSON feeds `tool_result_to_model_text` for its model text.
        // No-op when unset. Outer `Some` = key set even to `null` (`!== void 0`).
        let replacement: Option<serde_json::Value> = if is_error {
            None
        } else {
            let mut repl = post_agg
                .updated_tool_output
                .as_ref()
                .map(|inner| inner.clone().unwrap_or(serde_json::Value::Null));
            if tool_handle.is_mcp() {
                if let Some(mcp) = post_agg.updated_mcp_tool_output.as_ref() {
                    repl = Some(mcp.clone());
                }
            }
            repl
        };
        let (content, mcp_output_mutated) = match replacement {
            Some(new_output) => {
                // Validate against the tool's output schema when one exists
                // (`e.outputSchema?.safeParse(...)?.success!==!1`): substitute
                // unless validation EXPLICITLY fails. No schema → substitute.
                let schema_ok = match tool_handle.output_schema() {
                    Some(schema) => {
                        crate::schema_validation::validate_tool_output_schema(schema, &new_output)
                    }
                    None => Ok(()),
                };
                match schema_ok {
                    Ok(()) => (tool_result_to_model_text(&new_output), true),
                    Err(detail) => {
                        // Schema MISMATCH: keep the ORIGINAL output and surface
                        // the `hook_error_during_execution` meta message
                        // (BIN off 202465455) to the model, after the tool_result.
                        let msg = format!(
                            "PostToolUse hook returned updatedToolOutput that does not match {name}'s output shape; using original output. {detail}"
                        );
                        tracing::warn!(tool_name = %name, "{msg}");
                        // O3: this is a `hook_error_during_execution`
                        // attachment (2.1.220 BIN off 235421957 — the exact
                        // same message text). Its renderer entry is
                        // `hook_error_during_execution: () => []` (BIN off
                        // 238107100), so the MODEL NEVER SEES IT — the port
                        // previously pushed it onto the injected channel, which
                        // sent the model text claude suppresses.
                        orch.queue_hook_attachment(
                            tool_use_id,
                            hooks::error_during_execution_attachment(
                                &msg,
                                &format!("PostToolUse:{name}"),
                                tool_use_id.as_str(),
                                "PostToolUse",
                            ),
                        )
                        .await;
                        (content, false)
                    }
                }
            }
            None => (content, false),
        };

        // HOOK.1: BOTH the PreToolUse and the PostToolUse `additionalContext`
        // ride the `injected` channel as their OWN messages — neither is folded
        // into the tool-result content.
        //
        // O3 fix: the port used to ALSO concatenate every
        // `post_additional_contexts` entry onto the tool_result string, so a
        // PostToolUse hook's context reached the model TWICE. claude does
        // neither fold: its success arm (2.1.220 BIN off 235420375) assembles
        // the result blocks as `[formattedResult, acceptFeedback?,
        // ...contentBlocks?]` with no hook context, and the PostToolUse
        // consumer (BIN off 234726655) only yields the
        // `hook_additional_context` ATTACHMENT.
        //
        // `system_messages` was never folded and still is not: a PostToolUse
        // `systemMessage` is transcript/user-facing only and must NOT reach the
        // model (claude-code `hook_system_message` → `normalizeAttachmentForAPI`
        // returns `[]`, `messages.ts:4258`).
        //
        // `mutated` now tracks ONLY a genuine output REPLACEMENT
        // (`updatedToolOutput` / `updatedMCPToolOutput`), which is what the
        // `mutated_response` telemetry field means.
        let mutated = mcp_output_mutated;
        let final_content = content;

        tracing::info!(
            event = orch_events::HOOK_POST_COMPLETED,
            tool_name = %name,
            duration_ms = post_dur_ms,
            mutated_response = mutated,
        );
        telemetry::otel::emit_hook_lifecycle("post", "completed", &name, Some(post_dur_ms));

        // Worktree-creation hook (parity with claude-code `executeWorktreeCreateHook`,
        // `utils/hooks.ts:4928`). claude-code fires `WorktreeCreate` from the
        // worktree-creation logic (`createWorktreeForSession` /
        // `createAgentWorktree`); the LingXi port creates worktrees only through
        // the registered, turn_loop-dispatched `EnterWorktree` tool, so we fire it
        // here — same TIMING (immediately after the worktree exists), the fire just
        // lives in the dispatch chokepoint alongside `PostToolUse`. Only a
        // SUCCESSFUL `EnterWorktree` result counts (an errored create never made a
        // worktree). The wire payload carries only `name` — the requested slug, the
        // single field claude-code passes to `executeWorktreeCreateHook(slug)`. We
        // thread the resolved `path`/`branch` as engine-side context too (not on the
        // wire). Best-effort: a failing/absent hook never breaks the worktree op
        // (`orch.hooks.execute` is a strict no-op when no `WorktreeCreate` hook is
        // registered, mirroring the `PostToolUse` arm above).
        if !is_error && name == ENTER_WORKTREE_TOOL_NAME {
            // `name` (slug) is the requested input; `path`/`branch_name` come from
            // the tool's result data (`{"path":…,"branch_name":…}`).
            let slug = effective_input
                .get("slug")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let wt_path = emit_payload
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let wt_branch = emit_payload
                .get("branch_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let wt_event = HookEvent::WorktreeCreate {
                name: slug,
                path: std::path::PathBuf::from(wt_path),
                branch: wt_branch,
            };
            let wt_started = std::time::Instant::now();
            // Reuse the same hook context (session_id / cwd) the pre/post hooks used.
            let _wt_agg = orch.hooks.execute(wt_event, hook_ctx.clone()).await;
            // hook duration bounded by tokio timeout — u128 ms cannot exceed u64::MAX
            #[allow(clippy::cast_possible_truncation)]
            let wt_dur_ms = wt_started.elapsed().as_millis() as u64;
            // No `tengu_*` analytic here: claude-code's worktree-create path emits
            // no orchestrator-lifecycle event, so we keep parity by logging only.
            tracing::debug!(
                tool_name = %name,
                duration_ms = wt_dur_ms,
                "fired WorktreeCreate hook after successful EnterWorktree",
            );
        }

        // SubagentStart + SubagentStop hooks (claude `executeSubagentStartHooks`
        // runAgent.ts:532; `executeStopHooks`→`SubagentStop` utils/hooks.ts:3653-3678).
        // claude fires both in `runAgent` on ONE canonical `agentId` (runAgent.ts:347).
        // The port spawns subagents only via the dispatched `Agent`/`Task` tool, so we
        // fire here at spawn-completion (alongside PostToolUse/WorktreeCreate). Fires on
        // success AND failure (subagent started+stopped), NOT on pre-hook Block/deny
        // (those `continue` before any spawn). Best-effort.
        //
        // #8 (real id): the Agent tool surfaces the child's REAL pool `AgentId` on
        // `data.agentId` (C1 seam) so both events use one canonical id; a FAILED spawn
        // (no `data`) falls back to a fresh `AgentId::new()` — the single residual.
        //
        // SINGLE-FIRE (R7): the real tool's runner ALREADY fires SubagentStart
        // (runAgent.ts:530-555) + the child's frontmatter SubagentStop, marking
        // `data.subagentHooksFired`. So: skip the chokepoint SubagentStart when the
        // runner fired it (else fire — FakeAgentTool/failure); fire only the COMPLEMENT
        // SubagentStop via `execute_excluding_agent(child_id)` (omits the re-fired
        // frontmatter bucket, race-free vs `clear_agent_hooks`).
        if name == AGENT_TOOL_NAME || name == LEGACY_AGENT_TOOL_NAME {
            let subagent_type = effective_input
                .get("subagent_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            // #8: recover the REAL child id surfaced on the success result's
            // `data.agentId`. Absent (the failure path carries `ToolError`, no
            // `data`) → fresh fallback id, the single residual divergence.
            let child_id = emit_payload
                .get("agentId")
                .and_then(serde_json::Value::as_str)
                .and_then(protocol::AgentId::parse_prefixed)
                .unwrap_or_else(protocol::AgentId::new);
            // R7: did the child runner already fire the canonical SubagentStart
            // (+ its own frontmatter SubagentStop)? Only the REAL Agent tool sets
            // this; FakeAgentTool fixtures and the failure path leave it absent.
            let runner_fired_start = emit_payload
                .get("subagentHooksFired")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            // Carry the dispatched `subagent_type` as the hook context's
            // `agent_type` so the wire payload's `agent_type` is faithful
            // (claude-code passes the subagent's `agentType` into the hooks).
            // The session_id / cwd reuse the same context the pre/post hooks used.
            let mut sa_ctx = HookContext {
                agent_type: Some(subagent_type.clone()),
                agent_id: Some(child_id),
                ..hook_ctx.clone()
            };
            // SubagentStart FIRST (claude start-then-stop), with the canonical id.
            // SKIP when the runner already fired it (no production double-fire).
            if !runner_fired_start {
                let start_event = HookEvent::SubagentStart {
                    agent_id: child_id,
                    agent_type: subagent_type,
                    parent_agent_id: None,
                };
                let _start_agg = orch.hooks.execute(start_event, sa_ctx.clone()).await;
            }

            let status = if is_error { "failed" } else { "completed" };
            let sa_event = HookEvent::SubagentStop {
                agent_id: child_id,
                status: status.to_string(),
                // Same subagent type as the SubagentStart above — claude keys
                // SubagentStop matchers on it. `subagent_type` was moved into the
                // SubagentStart event, so source it from the cloned `sa_ctx`.
                agent_type: sa_ctx.agent_type.clone().unwrap_or_default(),
            };
            // claude-code stamps `background_tasks` + `session_crons` onto the
            // SubagentStop payload too (the `$Ee` firer's `...m` covers both the
            // Stop and SubagentStop branches when the tool-use context is
            // present). Populate the snapshot onto the SubagentStop context ONLY
            // (NOT the SubagentStart cloned above, which claude never carries it
            // on).
            orch.populate_stop_hook_snapshot(&mut sa_ctx).await;
            let sa_started = std::time::Instant::now();
            // EXCLUDE the child's own frontmatter bucket — the runner fired those
            // agent-scoped (claude fires a subagent's stop hooks in-child). This
            // covers session / plugin SubagentStop without double-firing the
            // child's frontmatter ones, race-free vs. the runner's
            // `clear_agent_hooks`.
            let _sa_agg = orch
                .hooks
                .execute_excluding_agent(sa_event, sa_ctx, child_id)
                .await;
            // hook duration bounded by tokio timeout — u128 ms cannot exceed u64::MAX
            #[allow(clippy::cast_possible_truncation)]
            let sa_dur_ms = sa_started.elapsed().as_millis() as u64;
            // No `tengu_*` analytic here: claude-code's subagent-stop path emits
            // no orchestrator-lifecycle event, so we keep parity by logging only.
            tracing::debug!(
                tool_name = %name,
                status,
                runner_fired_start,
                duration_ms = sa_dur_ms,
                "fired chokepoint SubagentStart (if runner didn't) + session/plugin SubagentStop after Agent/Task tool completed",
            );
        }

        // MCP results carry the content-block array directly AS `data` (1:1 with
        // the binary's MCPTool result `data = mcpResult.content`) so the egress can
        // send it VERBATIM as `tool_result.content` (claude-code passes the MCP
        // content array directly — images/resources stay structured). When `data`
        // is an ARRAY it IS that wire form; a bare-string `data` (or large-output
        // file replacement) is not. Gated to MCP tools so non-MCP tools whose
        // `data` happens to be an array (e.g. the Agent tool's transcript blocks)
        // are unaffected. A hook-mutated result (output replaced or
        // additionalContext appended) drops to the text-only `final_content`.
        //
        // Non-MCP `{type:"image"}` results (Read on an image file, rendered PDF
        // pages) get the binary result-mapper's `case "image"` form: the image
        // block INSIDE the tool_result content (`image_tool_result_blocks`).
        // Bash `{isImage:true}` results get the binary's `hKn` form — the image
        // block derived from the stdout data-URI (`bash_image_tool_result_blocks`).
        let content_blocks = if mutated {
            None
        } else if tool_handle.is_mcp() {
            emit_payload.as_array().cloned()
        } else if name == "ToolSearch" {
            tool_search_reference_blocks(&emit_payload)
        } else {
            image_tool_result_blocks(&emit_payload)
                .or_else(|| bash_image_tool_result_blocks(&emit_payload))
        };
        // A1: the LAST thing that touches a successful `tool_result` before it
        // is handed to the model — claude-code's `yor` wrapper around the
        // result mapper (BIN off **235420440**:
        // `let Ft=[Dt ? await N0u(…) : await yor(e,gt,t)]`). Blank results get
        // the `(<tool> completed with no output)` sentinel; oversized ones are
        // written to `<session>/tool-results/` and replaced by a
        // `<persisted-output>` envelope.
        let persistence = apply_tool_result_persistence(
            orch,
            &name,
            &tool_use_id,
            tool_handle.persistence_threshold().map(|raw| {
                crate::tool_result_persistence::resolve_threshold(
                    raw,
                    tool_handle.persistence_threshold_ceiling(),
                )
            }),
            final_content,
            content_blocks.as_deref(),
        )
        .await;
        // claude-code's `F0u` substitutes the ONE model-facing payload
        // (`{...e, content: a}`, where `content` is a string OR an array).
        // LingXi splits that payload in two and the wire prefers the array when
        // present, so a substitution must drop the array too — otherwise the
        // envelope is computed, the file written, the telemetry fired, and the
        // model still receives the full oversized payload.
        let (final_content, content_blocks) = if persistence.replaced {
            (persistence.content, None)
        } else {
            (persistence.content, content_blocks)
        };
        results.push(ContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: final_content,
            is_error,
            provider_tool_use_id: provider_id.clone(),
            content_blocks,
        });

        // #39 PostToolBatch: record this resolved tool's call for the once-per-
        // batch fire after the loop. `tool_response` is the structured tool
        // output (the same `emit_payload` the PostToolUse hook saw). A failure
        // result still resolves the call, so it is included with its error
        // payload (claude-code's batch includes every resolved tool_use).
        post_tool_batch_calls.push(hooks::events::PostToolBatchCall {
            tool_name: name.clone(),
            tool_input: effective_input.clone(),
            tool_use_id: tool_use_id.clone(),
            tool_response: Some(emit_payload.clone()),
        });

        // HOOK.1: queue this tool's PreToolUse `additionalContext` as its OWN
        // message on the `injected` channel, tagged with this tool's
        // `tool_use_id` (TS `toolUseID`). Both drivers append `injected` AFTER
        // the tool_result user message, so the context is ordered after the
        // result — matching claude-code's `resultingMessages` push order
        // (`toolExecution.ts:845`). `None` (the common no-context case) is a
        // strict no-op.
        if let Some(msg) = pre_context_message {
            injected_messages.push((msg, tool_use_id.clone()));
        }

        // FIX C (PreToolUse hook_stopped_continuation): emit the stop-reason meta
        // AFTER this tool's tool_result, mirroring claude's post-execution push
        // (`toolExecution.ts:1571`). Ordered after `pre_context_message` so the
        // relative order matches claude (additionalContext at 846 → stopped at
        // 1571). Success path only — a Block/Defer `continue`d above without ever
        // executing the tool, so this site is unreached there. No-op when the
        // hook did not request preventContinuation.
        if let Some((msg, attachment)) = pre_prevent {
            // O2: the persisted record rides the same tool-keyed queue as the
            // additionalContext one, so it is flushed right after this tool's
            // tool_result — the position the oracle's post-execution yield puts
            // it in.
            orch.queue_hook_attachment(tool_use_id, attachment).await;
            injected_messages.push((msg, tool_use_id.clone()));
        }
    }

    // #39 PostToolBatch: fire ONCE after the whole batch resolved (claude-code
    // `G4t`, BIN off 205710327: fired after every tool call in a batch resolves,
    // before the next model request — distinct from per-tool `PostToolUse`).
    // Strict no-op when no tool ran (empty batch). Best-effort: a
    // failing/absent PostToolBatch hook never breaks the turn (the executor is a
    // no-op when no PostToolBatch hook is registered, mirroring the per-tool
    // PostToolUse fire).
    //
    // CORRECTED: this used to bind `let _batch_agg = …` under a comment saying
    // "the aggregate decision/output are not consumed — this is an
    // observational, post-batch event". It is not observational. The oracle
    // (@233161375) ends the turn on it — see
    // [`post_tool_batch_stop_reason`].
    if !post_tool_batch_calls.is_empty() {
        // FIX 2: populate `transcript_path` + `permission_mode` here too (the
        // batch firer builds its own context). Same sources as the PreToolUse
        // context above: the live JSONL writer path (FIX A: ELSE the computed
        // `<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`, non-empty in
        // production where no writer is wired) + the plan/default mode.
        let (session_id, plan_mode) = {
            let s = orch.session.lock().await;
            (s.session_id, s.plan_mode)
        };
        let transcript_path = orch
            .jsonl_writer
            .as_ref()
            .map(|w| w.path().to_path_buf())
            .unwrap_or_else(|| orch.computed_transcript_path(&session_id));
        let batch_ctx = HookContext {
            session_id,
            cwd: orch.current_cwd(),
            transcript_path,
            permission_mode: Some(if plan_mode { "plan" } else { "default" }.to_string()),
            ..Default::default()
        };
        let batch_event = HookEvent::PostToolBatch {
            tool_calls: post_tool_batch_calls,
        };
        let batch_agg = orch.hooks.execute(batch_event, batch_ctx).await;

        // `additionalContext` FIRST: the oracle yields it inside the per-hook
        // loop, while the stop record is only reached after that loop ends, so a
        // hook doing both produces the context ahead of the stop. Independent of
        // `preventContinuation` — a batch hook may contribute context without
        // stopping anything.
        //
        // FIDELITY: the oracle emits one attachment PER HOOK RESULT, each
        // carrying that hook's own `additionalContexts` array. This port's
        // aggregate flattens contexts across hooks, so it emits ONE attachment
        // with the combined array — identical for a single batch hook (every
        // observed case) and the same compromise the PostToolUse site already
        // makes.
        if !batch_agg.additional_contexts.is_empty() {
            let identity = post_tool_batch_identity();
            let batch_id = protocol::ToolUseId::from(identity.tool_use_id.clone());
            orch.persist_hook_attachment_to_jsonl(hooks::additional_context_attachment(
                &identity.hook_name,
                &identity.tool_use_id,
                &identity.hook_event,
                &batch_agg.additional_contexts,
            ))
            .await;
            for ctx in &batch_agg.additional_contexts {
                injected_messages.push((
                    // `user_meta`: the rendering is ephemeral (`zr({isMeta:true})`);
                    // the attachment above is the on-disk record.
                    ConversationMessage::user_meta(
                        MessageId::new(),
                        format!(
                            "<system-reminder>\nPostToolBatch hook additional context: {ctx}\n</system-reminder>"
                        ),
                    ),
                    batch_id.clone(),
                ));
            }
        }

        if let Some(reason) = post_tool_batch_stop_reason(&batch_agg) {
            // ONE identity for both records, so the persisted attachment and the
            // model-facing prose describe the same event and cannot drift.
            let identity = post_tool_batch_identity();
            let batch_id = protocol::ToolUseId::from(identity.tool_use_id.clone());
            // The oracle yields the record and returns `{reason:"hook_stopped"}`
            // in one expression, so the attachment is persisted BEFORE the flag
            // propagates — a stop must never reach the transcript unexplained.
            orch.persist_hook_attachment_to_jsonl(hooks::stopped_continuation_attachment(
                &identity, &reason,
            ))
            .await;
            // The oracle derives this prose from the attachment at
            // `normalizeAttachmentForAPI` time (@238107808). This port has no
            // such layer — `Stop`, `PreToolUse` and `PostToolUse` each build it
            // explicitly — so without this the model would never learn why the
            // turn ended.
            injected_messages.push((
                ConversationMessage::user(
                    MessageId::new(),
                    format!(
                        "<system-reminder>\nPostToolBatch hook stopped continuation: {reason}\n</system-reminder>"
                    ),
                ),
                batch_id,
            ));
            prevent_continuation = true;
        }
    }

    Ok((
        results,
        prevent_continuation,
        injected_messages,
        context_modifiers,
    ))
}

/// Identity for the once-per-batch `PostToolBatch` records.
///
/// `hookName`/`hookEvent` are the bare literal `PostToolBatch` — NOT suffixed
/// with a tool name the way `PostToolUse:${t.name}` is (@234726414), because the
/// event covers the whole batch rather than one call.
///
/// `toolUseID` is the oracle's `rt`, bound at the top of the batch block as
/// ``rt = `hook-${f.uuid()}` `` (@233159400) — a SYNTHETIC id, not any real
/// tool's. The `Stop`-hook site builds its id the same way
/// (`conversation.rs`), so the two stay consistent.
fn post_tool_batch_identity() -> hooks::HookAttachmentIdentity {
    hooks::HookAttachmentIdentity {
        hook_name: "PostToolBatch".to_string(),
        hook_event: "PostToolBatch".to_string(),
        tool_use_id: format!("hook-{}", protocol::HookId::new().as_uuid()),
    }
}

/// The stop reason a `PostToolBatch` aggregate implies, or `None` to continue.
///
/// Ports the oracle's `Mr`/`Qn` pair (@233161375):
///
/// ```js
/// if(Mn.blockingError)Mr=!0,Qn??=Mn.blockingError.blockingError;
/// if(Mn.preventContinuation)Mr=!0,Qn??=Mn.stopReason
/// …
/// if(Mr)… message:Qn||"Execution stopped by PostToolBatch hook" …
/// ```
///
/// Two details worth keeping:
///
/// - `Mr` is set by EITHER a blocking error or `preventContinuation`. Gating on
///   `prevent_continuation` alone would let a blocking batch hook run on.
/// - `Qn` is `??=` (first-wins) and falls back to the literal below when empty.
///   The aggregate's `reason` already freezes at the first blocker
///   (`executor.rs`), so reading it here preserves that ordering.
fn post_tool_batch_stop_reason(agg: &hooks::response::AggregateHookResult) -> Option<String> {
    let stopped = agg.prevent_continuation
        || matches!(agg.decision, Some(hooks::response::HookDecision::Block));
    if !stopped {
        return None;
    }
    Some(
        agg.reason
            .clone()
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "Execution stopped by PostToolBatch hook".to_string()),
    )
}

/// SKILLEXEC.3 (model scope): fold a tool batch's `context_modifier`s over a
/// seed context carrying the live `session.model`, then persist the resolved
/// model back to `session.model` when it changed (TS `contextModifier` sets
/// `options.mainLoopModel` for the rest of the session).
///
/// Called POST-BATCH by BOTH drivers (the batched [`execute_one_turn`] and the
/// streaming `try_run_turn_streaming`) at the same point they append injected
/// `new_messages`. Applying after the whole batch — rather than per tool —
/// gives the concurrent streaming dispatch a SINGLE application point, so there
/// is no race on `session.model` between concurrently-dispatched tools.
///
/// Empty `modifiers` (every existing tool + skills WITHOUT a `model:`
/// frontmatter) → an early return that never touches the session lock →
/// `session.model` is unchanged → byte-identical. The model override then
/// persists: subsequent turns read the new `session.model` (TS sets
/// `options.mainLoopModel` for the rest of the session).
pub(crate) async fn apply_model_context_modifiers(
    orch: &ConversationOrchestrator,
    modifiers: Vec<ContextModifier>,
) {
    if modifiers.is_empty() {
        return;
    }
    let mut s = orch.session.lock().await;
    let current = s.model.clone();
    let resolved = modifiers
        .into_iter()
        .fold(ToolUseContext::model_seed(current.clone()), |ctx, m| m(ctx))
        .options
        .main_loop_model;
    if resolved != current {
        s.model = resolved;
    }
}

/// Serialize a successful tool result's data into the model-facing string.
///
/// Mirrors claude-code's per-tool `mapToolResultToToolResultBlockParam`: the
/// model sees the tool's OWN string, never a JSON dump of the output object. A
/// tool exposes that string via `model_content` (used when it must differ from
/// the TUI payload — e.g. Read's cat -n + reminders, where the TUI shows raw
/// content) or, failing that, the verbatim `content` string (Bash stdout,
/// Edit/Write confirmations, where the model and TUI strings coincide). Tools
/// that expose neither fall back to the JSON object — the legacy behavior, kept
/// for structured-only results that have no human-facing string.
///
/// The full `result.data` object still flows to the TUI (`emit_tool_result`)
/// and the `PostToolUse` hook unchanged; only the model-facing string is derived
/// here.
fn tool_result_to_model_text(data: &serde_json::Value) -> String {
    data.get("model_content")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("content").and_then(|v| v.as_str()))
        // `result` is WebFetch's content field (claude-code's WebFetch result
        // `data` names the model-facing markdown `result`, byte-faithful to the
        // binary's `{bytes,code,codeText,result,durationMs,url}`). Without this
        // arm a WebFetch result (no `content`/`model_content`) would fall through
        // to the JSON dump below and show the model the whole object.
        .or_else(|| data.get("result").and_then(|v| v.as_str()))
        .map_or_else(
            || serde_json::to_string(data).unwrap_or_else(|_| "<unserializable>".into()),
            std::string::ToString::to_string,
        )
}

/// Map ToolSearch's matched names to Anthropic `tool_reference` blocks. Empty
/// results stay on the text path (`model_content` carries the upstream copy).
fn tool_search_reference_blocks(data: &serde_json::Value) -> Option<Vec<serde_json::Value>> {
    let matches = data.get("matches")?.as_array()?;
    if matches.is_empty() {
        return None;
    }
    let blocks: Vec<serde_json::Value> = matches
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(|name| {
            serde_json::json!({
                "type": "tool_reference",
                "tool_name": name,
            })
        })
        .collect();
    (!blocks.is_empty()).then_some(blocks)
}

/// The binary result-mapper's `case "image"` (`mapToolResultToToolResultBlockParam`):
/// a `{type:"image", file:{base64, type, …}}` result — Read on an image file, or a
/// rendered PDF page (`tbo`) — becomes a tool_result whose content is the image
/// block array `[{type:"image", source:{type:"base64", data, media_type}}]`,
/// emitted VERBATIM on egress via `ContentBlock::ToolResult.content_blocks`.
/// `None` for every other result shape (strict no-op: the wire form falls back to
/// the text `content` exactly as before).
fn image_tool_result_blocks(data: &serde_json::Value) -> Option<Vec<serde_json::Value>> {
    if data.get("type").and_then(serde_json::Value::as_str) != Some("image") {
        return None;
    }
    let file = data.get("file")?;
    let base64 = file.get("base64").and_then(serde_json::Value::as_str)?;
    let media_type = file.get("type").and_then(serde_json::Value::as_str)?;
    Some(vec![serde_json::json!({
        "type": "image",
        "source": {
            "type": "base64",
            "data": base64,
            "media_type": media_type,
        },
    })])
}

/// The binary's Bash image mapper `hKn`: an `{isImage:true, stdout:<data-URI>}`
/// result becomes a tool_result whose content is `[{type:"image", source:
/// {type:"base64", media_type:<SNIFFED>, data}}]` — the media type comes from
/// magic-byte sniffing of the DECODED payload (`Wfe`), NOT the URI's claimed
/// type; `data` is the URI's original base64. Any miss (no isImage, no data-URI
/// match on `/^data:([^;]+);base64,(.+)$/`, undecodable base64, unrecognized
/// magic) returns `None` — the tool_result falls back to the text `content`,
/// exactly the binary's `if(g)` fall-through.
fn bash_image_tool_result_blocks(data: &serde_json::Value) -> Option<Vec<serde_json::Value>> {
    use base64::Engine as _;
    if data.get("isImage") != Some(&serde_json::Value::Bool(true)) {
        return None;
    }
    let stdout = data.get("stdout").and_then(serde_json::Value::as_str)?;
    // `Xyu`: /^data:([^;]+);base64,(.+)$/ on the trimmed string.
    let rest = stdout.trim().strip_prefix("data:")?;
    let semi = rest.find(';')?;
    if semi == 0 {
        return None;
    }
    let payload = rest[semi..].strip_prefix(";base64,")?;
    if payload.is_empty() {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .ok()?;
    // `Wfe` — magic-byte sniff (shared impl in tool-api, same fn the Bash
    // tool's image gate uses, so gate and mapper always agree).
    let media_type = tool_api::util::image_sniff::sniff_image_media_type(&bytes)?;
    Some(vec![serde_json::json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": payload,
        },
    })])
}

#[cfg(test)]
mod image_tool_result_tests {
    use super::image_tool_result_blocks;
    use serde_json::json;

    #[test]
    fn image_result_becomes_the_binary_mapper_block_array() {
        let data = json!({
            "type": "image",
            "file": { "base64": "QUJD", "type": "image/png", "originalSize": 3 }
        });
        let blocks = image_tool_result_blocks(&data).expect("image data maps");
        // Byte shape of the binary mapper's `case "image"` — one image block,
        // source keys in written order (type, data, media_type).
        assert_eq!(
            serde_json::to_string(&blocks).unwrap(),
            r#"[{"type":"image","source":{"type":"base64","data":"QUJD","media_type":"image/png"}}]"#
        );
    }

    #[test]
    fn bash_isimage_result_maps_stdout_data_uri_with_sniffed_media_type() {
        use super::bash_image_tool_result_blocks;
        // `iVBORw0KGgo=` = base64 of the 8-byte PNG magic. The URI CLAIMS jpeg —
        // the mapper must emit the SNIFFED type (image/png), per `hKn`/`Wfe`.
        let data = json!({
            "stdout": "data:image/jpeg;base64,iVBORw0KGgo=",
            "stderr": "",
            "interrupted": false,
            "isImage": true,
        });
        let blocks = bash_image_tool_result_blocks(&data).expect("sniffable data-URI maps");
        assert_eq!(
            serde_json::to_string(&blocks).unwrap(),
            r#"[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}}]"#
        );
    }

    #[test]
    fn bash_isimage_misses_fall_back_to_none() {
        use super::bash_image_tool_result_blocks;
        // Not flagged as image.
        assert!(bash_image_tool_result_blocks(
            &json!({"stdout":"data:image/png;base64,iVBORw0KGgo=","isImage":false})
        )
        .is_none());
        // Flagged, but stdout is not a data-URI → text fallback (`hKn` null).
        assert!(
            bash_image_tool_result_blocks(&json!({"stdout":"plain text","isImage":true})).is_none()
        );
        // Valid URI shape but undecodable base64.
        assert!(bash_image_tool_result_blocks(
            &json!({"stdout":"data:image/png;base64,@@not-base64@@","isImage":true})
        )
        .is_none());
        // Decodable but unrecognized magic (claimed image, actually text bytes).
        assert!(bash_image_tool_result_blocks(
            &json!({"stdout":"data:image/png;base64,aGVsbG8gd29ybGQh","isImage":true})
        )
        .is_none());
    }

    #[test]
    fn non_image_and_malformed_results_map_to_none() {
        // Strict no-op for every other tool result shape.
        assert!(image_tool_result_blocks(&json!({"type":"text","file":{}})).is_none());
        assert!(image_tool_result_blocks(&json!({"filePath":"/a","content":"x"})).is_none());
        assert!(image_tool_result_blocks(&json!("just a string")).is_none());
        // `type:"image"` but missing/malformed file fields → None (text fallback).
        assert!(image_tool_result_blocks(&json!({"type":"image"})).is_none());
        assert!(image_tool_result_blocks(&json!({"type":"image","file":{}})).is_none());
        assert!(
            image_tool_result_blocks(&json!({"type":"image","file":{"base64":"QQ=="}})).is_none()
        );
    }
}

#[cfg(test)]
mod code_change_accumulation_tests {
    use super::accumulate_code_change;
    use cost::{CostTracker, PricingCatalog};
    use protocol::SessionId;
    use serde_json::json;
    use std::sync::Arc;

    fn make_tracker() -> Arc<CostTracker> {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        Arc::new(CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        ))
    }

    #[tokio::test]
    async fn structured_patch_lines_accumulate_into_the_tracker() {
        let tracker = make_tracker();
        let payload = json!({
            "structuredPatch": [
                { "lines": ["+a", "+b", "-c"] }
            ]
        });
        accumulate_code_change(&payload, Some(&tracker)).await;
        let snap = tracker.snapshot().await;
        assert_eq!(snap.total_lines_added, 2);
        assert_eq!(snap.total_lines_removed, 1);
    }

    #[tokio::test]
    async fn no_tracker_is_a_no_op() {
        // Absent tracker (M6-06 default `None`) must not panic — this is the
        // common case whenever no host has opted into cost tracking.
        let payload = json!({
            "structuredPatch": [ { "lines": ["+a"] } ]
        });
        accumulate_code_change(&payload, None).await;
    }

    #[tokio::test]
    async fn missing_structured_patch_is_a_no_op() {
        let tracker = make_tracker();
        // Every non-edit tool's result data lacks `structuredPatch` entirely.
        accumulate_code_change(&json!({"stdout": "ok"}), Some(&tracker)).await;
        let snap = tracker.snapshot().await;
        assert_eq!(snap.total_lines_added, 0);
        assert_eq!(snap.total_lines_removed, 0);
    }
}

// ORCH-1: the `ZX_` rule-scope → OTEL decision-source mapping, kept out of the
// concurrently-edited turn_loop_test.rs.
#[cfg(test)]
mod decision_otel_source_tests {
    use super::rule_decision_otel_source;

    #[test]
    fn session_rule_is_temporary_on_allow_and_reject_on_deny() {
        assert_eq!(
            rule_decision_otel_source(Some("session"), true),
            "user_temporary"
        );
        assert_eq!(
            rule_decision_otel_source(Some("session"), false),
            "user_reject"
        );
    }

    #[test]
    fn user_owned_settings_rules_are_permanent_on_allow() {
        for scope in ["localSettings", "userSettings"] {
            assert_eq!(
                rule_decision_otel_source(Some(scope), true),
                "user_permanent",
                "{scope}"
            );
            assert_eq!(
                rule_decision_otel_source(Some(scope), false),
                "user_reject",
                "{scope}"
            );
        }
    }

    #[test]
    fn every_other_setting_source_falls_through_to_config() {
        // `ZX_`'s `default:` arm — projectSettings is deliberately NOT in the
        // user-owned set even though `U0s` (the interactive persistence check)
        // includes it.
        for scope in [
            "projectSettings",
            "policySettings",
            "flagSettings",
            "cliArg",
            "command",
            "toolsNarrowing",
            "mcpServerPolicy",
        ] {
            assert_eq!(
                rule_decision_otel_source(Some(scope), true),
                "config",
                "{scope}"
            );
            assert_eq!(
                rule_decision_otel_source(Some(scope), false),
                "config",
                "{scope}"
            );
        }
    }

    #[test]
    fn no_matched_rule_is_config() {
        // `eQ_` reaches `ZX_` only for `decisionReason.type === "rule"`; a mode /
        // classifier / safety-check decision carries no rule and stays "config".
        assert_eq!(rule_decision_otel_source(None, true), "config");
        assert_eq!(rule_decision_otel_source(None, false), "config");
    }
}

// The `toolDenialKind` classifier, kept out of the concurrently-edited
// turn_loop_test.rs for the same reason as the module above.
#[cfg(test)]
mod tool_denial_kind_tests {
    use super::tool_denial_kind;

    #[test]
    fn ask_behavior_is_user_rejected_and_outranks_the_classifier() {
        // The oracle tests `behavior === "ask"` FIRST and returns before it ever
        // looks at decisionReason, so an ask-behavior denial that also carries a
        // classifier reason is still `user-rejected`.
        assert_eq!(tool_denial_kind(true, None, None), "user-rejected");
        assert_eq!(
            tool_denial_kind(true, Some("classifier"), Some("Classifier unavailable")),
            "user-rejected"
        );
    }

    #[test]
    fn classifier_unavailable_reason_is_automode_unavailable() {
        // `t.reason === TRt` — EXACT equality, not a prefix.
        assert_eq!(
            tool_denial_kind(false, Some("classifier"), Some("Classifier unavailable")),
            "automode-unavailable"
        );
        assert_eq!(
            tool_denial_kind(
                false,
                Some("classifier"),
                Some("Classifier unavailable later")
            ),
            "automode-blocked",
            "TRt is matched by equality, so a longer string is NOT unavailable"
        );
    }

    #[test]
    fn safety_block_prefix_is_automode_parsing_error() {
        // `t.reason.startsWith(p2s)` — a PREFIX test, so the trailing detail the
        // oracle appends must still classify as a parsing error.
        let p2s = "Auto mode could not evaluate this action and is blocking it for safety";
        assert_eq!(
            tool_denial_kind(false, Some("classifier"), Some(p2s)),
            "automode-parsing-error"
        );
        assert_eq!(
            tool_denial_kind(false, Some("classifier"), Some(&format!("{p2s}: bad JSON"))),
            "automode-parsing-error"
        );
    }

    #[test]
    fn other_classifier_reasons_are_automode_blocked() {
        assert_eq!(
            tool_denial_kind(
                false,
                Some("classifier"),
                Some("writes outside the workspace")
            ),
            "automode-blocked"
        );
        assert_eq!(
            tool_denial_kind(false, Some("classifier"), None),
            "automode-blocked",
            "a classifier denial with no reason still falls through to blocked"
        );
    }

    #[test]
    fn non_classifier_denials_are_permission_rule() {
        for kind in [
            None,
            Some("rule"),
            Some("mode"),
            Some("hook"),
            Some("safetyCheck"),
        ] {
            assert_eq!(
                tool_denial_kind(false, kind, Some("Classifier unavailable")),
                "permission-rule",
                "{kind:?} is not a classifier decision, so the reason text is irrelevant"
            );
        }
    }
}

// End-to-end pin for the denial-provenance WIRING (as opposed to the
// `tool_denial_kind` unit tests above, which only cover the pure classifier).
// Kept in its own module rather than in the concurrently-edited
// turn_loop_test.rs, per the convention the modules above already follow.
//
// This seam is exactly where the first version of this feature was wrong:
// `MockOutputStream` inherits the DEFAULTED `emit_tool_result_denied` unless it
// overrides it, so before that override existed every deny-path test passed no
// matter what kind the turn loop computed.
#[cfg(test)]
mod denial_kind_wiring_tests {
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, StaticMemoryProvider,
    };
    use crate::turn_loop::dispatch_tool_uses_tracked;
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use protocol::ToolUseId;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };
    use traits::permission_gate::{
        PermissionDecision, PermissionDecisionSource, PermissionGate, PermissionResolution,
    };

    /// A gate that always denies through the SOURCED resolution, with the
    /// structured provenance fields under test control.
    struct ProvenanceDenyGate {
        decision_reason_type: Option<String>,
        decision_reason: Option<String>,
        behavior_ask: bool,
    }

    #[async_trait]
    impl PermissionGate for ProvenanceDenyGate {
        async fn check(&self, _t: &str, _i: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "denied-for-test".into(),
            }
        }
        async fn resolve_detailed(&self, _t: &str, _i: &serde_json::Value) -> PermissionResolution {
            PermissionResolution::Deny {
                reason: "denied-for-test".into(),
                source: PermissionDecisionSource::Rule,
                rule_source: None,
                decision_reason_type: self.decision_reason_type.clone(),
                decision_reason: self.decision_reason.clone(),
                behavior_ask: self.behavior_ask,
                content_blocks: Vec::new(),
            }
        }
    }

    struct DeniedTool;
    #[async_trait]
    impl Tool for DeniedTool {
        fn name(&self) -> &str {
            "Denied"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _: &serde_json::Value,
            _: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _: &serde_json::Value,
            _: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(&self, _: &serde_json::Value, _: &DescriptionOptions) -> String {
            "denied".into()
        }
        async fn prompt(&self, _: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _: serde_json::Value,
            _: ToolUseContext,
            _: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            panic!("DeniedTool::call must never run — the gate denies it");
        }
    }

    /// Dispatch one denied tool through the given gate and return the
    /// `(tool_use_id, denial_kind)` pairs the output stream observed.
    async fn denial_kinds_for(gate: ProvenanceDenyGate) -> Vec<(ToolUseId, String)> {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(DeniedTool) as Arc<dyn Tool>);
        let output = MockOutputStream::new();
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(gate),
            Arc::new(output.clone()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let uses = vec![(ToolUseId::new(), "Denied".to_string(), json!({}), None)];
        dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch must succeed on a denied tool");
        output.denial_snapshot().await
    }

    /// A plain rule denial reaches the output stream stamped `permission-rule`.
    /// This is the value the SDK/stdio transport must carry: claude-code's
    /// `JMn` preserves the host's `behavior`, so a host deny is
    /// `behavior:"deny"` + a `permissionPromptTool` reason and never takes the
    /// `ask` branch. An earlier revision stamped `user-rejected` here.
    #[tokio::test]
    async fn rule_denial_is_emitted_as_permission_rule() {
        let kinds = denial_kinds_for(ProvenanceDenyGate {
            decision_reason_type: Some("rule".into()),
            decision_reason: None,
            behavior_ask: false,
        })
        .await;
        assert_eq!(
            kinds.len(),
            1,
            "the denied tool must reach emit_tool_result_denied exactly once"
        );
        assert_eq!(kinds[0].1, "permission-rule");
    }

    /// An auto-mode classifier denial is carried through as `automode-blocked`
    /// — proving the classifier reason really is threaded from the sourced
    /// resolution arm to the emit site, not just computed locally.
    #[tokio::test]
    async fn classifier_denial_is_emitted_as_automode_blocked() {
        let kinds = denial_kinds_for(ProvenanceDenyGate {
            decision_reason_type: Some("classifier".into()),
            decision_reason: None,
            behavior_ask: false,
        })
        .await;
        assert_eq!(kinds.len(), 1);
        assert_eq!(kinds[0].1, "automode-blocked");
    }

    /// A tool skipped by the PRE-CANCEL guard is stamped `cancelled`.
    ///
    /// claude-code hardcodes `toolDenialKind:"cancelled"` at this site (binary
    /// offset 235399713) — it does NOT route through the `YDd` abort-reason
    /// classifier, so no abort-reason plumbing is needed to be faithful here.
    #[tokio::test]
    async fn pre_cancelled_tool_is_emitted_as_cancelled() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(DeniedTool) as Arc<dyn Tool>);
        let output = MockOutputStream::new();
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(ProvenanceDenyGate {
                decision_reason_type: None,
                decision_reason: None,
                behavior_ask: false,
            }),
            Arc::new(output.clone()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel(); // fire BEFORE dispatch
        let uses = vec![(ToolUseId::new(), "Denied".to_string(), json!({}), None)];
        dispatch_tool_uses_tracked(&orch, &uses, Some(cancel))
            .await
            .expect("dispatch must succeed on a pre-cancelled tool");

        let kinds = output.denial_snapshot().await;
        assert_eq!(
            kinds.len(),
            1,
            "the pre-cancelled tool must report denial provenance"
        );
        assert_eq!(kinds[0].1, "cancelled");
    }

    /// An `ask`-behavior denial short-circuits to `user-rejected` end-to-end.
    #[tokio::test]
    async fn ask_behavior_denial_is_emitted_as_user_rejected() {
        let kinds = denial_kinds_for(ProvenanceDenyGate {
            decision_reason_type: Some("classifier".into()),
            decision_reason: None,
            behavior_ask: true,
        })
        .await;
        assert_eq!(kinds.len(), 1);
        assert_eq!(
            kinds[0].1, "user-rejected",
            "behavior_ask must outrank the classifier reason"
        );
    }
}

/// O4-A: the `interrupted` denial stamp.
///
/// claude-code stamps `toolDenialKind` for an ABORTED tool inside the per-tool
/// execution catch (`oQ_`, 2.1.220 @235424972):
/// ```js
/// toolDenialKind: YDd(ce, n.abortController.signal)
/// ```
/// with (`YDd` @235394375)
/// ```js
/// function YDd(e,t){
///   let r = e instanceof hW && e.interrupted;              // ShellError.interrupted
///   if(!(e instanceof tl || r || $7e(e)&&t.aborted)) return; // tl = AbortError
///   return t.aborted && H_(t.reason)==="background" ? "cancelled" : "interrupted";
/// }
/// ```
/// The LingXi analog of `tl` is [`tool_api::ToolError::Aborted`], and the
/// analog of `oQ_`'s catch is the `Err(err)` arm of
/// [`dispatch_tool_uses_tracked`] — so the stamp attaches at exactly the site
/// claude-code stamps at, with no emission-point change.
///
/// Real-transcript ground truth (2.1.220, `~/.claude/projects/**/*.jsonl`):
/// `toolDenialKind` census is `user-rejected` ×13 and `interrupted` ×1; the
/// `interrupted` line carries `"toolUseResult": "Error: [Request interrupted by
/// user for tool use]"`.
#[cfg(test)]
mod interrupted_denial_stamp_tests {
    use super::{dispatch_tool_uses_tracked, ConversationOrchestrator};
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use protocol::ToolUseId;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// A tool whose `call` returns the requested `ToolError` immediately.
    struct FailingTool {
        name: &'static str,
        abort: bool,
    }

    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &str {
            self.name
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "failing-tool".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            if self.abort {
                Err(ToolError::Aborted)
            } else {
                Err(ToolError::Internal("boom".into()))
            }
        }
    }

    fn orch_with(out: MockOutputStream) -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(FailingTool {
            name: "AbortTool",
            abort: true,
        }) as Arc<dyn Tool>);
        registry.register_builtin(Arc::new(FailingTool {
            name: "BoomTool",
            abort: false,
        }) as Arc<dyn Tool>);
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(out),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// `ToolError::Aborted` from a tool's own `call` ⇒ `toolDenialKind:
    /// "interrupted"`, both on the SDK frame and in the orchestrator's
    /// persistence side-table.
    #[tokio::test]
    async fn aborted_tool_is_stamped_interrupted() {
        let out = MockOutputStream::new();
        let orch = orch_with(out.clone());
        let id = ToolUseId::new();
        let uses = vec![(id.clone(), "AbortTool".to_string(), json!({}), None)];
        let (blocks, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");
        assert_eq!(blocks.len(), 1);

        let denials = out.denial_snapshot().await;
        assert_eq!(
            denials,
            vec![(id.clone(), "interrupted".to_string())],
            "an aborted tool must emit exactly one `interrupted` denial frame"
        );
        assert_eq!(
            orch.tool_denial_kinds
                .lock()
                .await
                .get(&id.to_string())
                .map(String::as_str),
            Some("interrupted"),
            "the kind must also be recorded for the persisted tool_result line"
        );
    }

    /// A NON-abort tool failure is an ordinary error result — no denial kind
    /// (claude-code `YDd` returns `undefined` unless the error is an
    /// AbortError / interrupted ShellError).
    #[tokio::test]
    async fn ordinary_tool_error_is_not_stamped() {
        let out = MockOutputStream::new();
        let orch = orch_with(out.clone());
        let id = ToolUseId::new();
        let uses = vec![(id.clone(), "BoomTool".to_string(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");
        assert!(
            out.denial_snapshot().await.is_empty(),
            "a plain tool failure must not carry a toolDenialKind"
        );
        assert!(orch.tool_denial_kinds.lock().await.is_empty());
    }
}

/// O3: hook `additionalContext` / `hook_error_during_execution` become
/// TRANSCRIPT ATTACHMENTS, and the model-facing rendering is EPHEMERAL.
///
/// Oracle — the attachment→model renderer table (2.1.220 BIN off 238107100):
/// ```text
/// hook_additional_context: (e) => { if (e.content.length === 0) return [];
///     return [ zr({ content: Ww(`${e.hookName} hook additional context: ${e.content.join("\n")}`), isMeta:!0 }) ] },
/// hook_error_during_execution: () => [],
/// ```
/// `Ww` (BIN off 238046823) is the `<system-reminder>` wrapper. The `zr(…)`
/// message is built at API-normalization time from the attachment and is never
/// written to the transcript — census of real 2.1.220 sessions finds 145
/// `hook_additional_context` attachment lines and ZERO persisted `user` lines
/// carrying the rendered text. `hook_error_during_execution` renders to `[]`,
/// so the MODEL NEVER SEES IT.
///
/// claude also never folds a PostToolUse `additionalContext` into the
/// tool_result string — the success arm (BIN off 235420375) assembles
/// `[formattedResult, acceptFeedback?, ...contentBlocks?]` with no hook
/// context, and the PostToolUse consumer (BIN off 234726655) only yields the
/// attachment.
#[cfg(test)]
mod hook_context_attachment_tests {
    use super::{dispatch_tool_uses_tracked, post_tool_batch_identity, ConversationOrchestrator};
    use crate::test_support::{MockApiClient, MockOutputStream, NoOpPermissionGate};
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
    use hooks::events::{HookEvent, HookEventType};
    use hooks::executor::{BuiltinHookHandler, HookExecutorImpl};
    use hooks::registry::HookRegistry;
    use hooks::response::HookResponse;
    use hooks::{HookContext, HookOutcome, HookResult};
    use protocol::{ContentBlock, ConversationMessage, HookId, ToolUseId};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "Echo"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        /// Declared so a PostToolUse `updatedToolOutput` can FAIL validation
        /// and exercise the `hook_error_during_execution` arm.
        fn output_schema(&self) -> Option<&serde_json::Value> {
            static OUT: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| {
                    json!({
                        "type": "object",
                        "properties": { "out": { "type": "string" } },
                        "required": ["out"]
                    })
                });
            Some(&OUT)
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "echo".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({ "out": "ECHOED-OUTPUT" }),
                model_content: Some("ECHOED-OUTPUT".into()),
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    struct UnusedHttp;
    #[async_trait]
    impl traits::HttpTransport for UnusedHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }

    struct UnusedRuntime;
    #[async_trait]
    impl traits::RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _name: &str,
            _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            Err(traits::RuntimeError::Internal("unused".into()))
        }
        async fn sleep(&self, _d: std::time::Duration) {}
        async fn cancel(
            &self,
            _h: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    struct FixedPostHook {
        response: HookResponse,
    }

    #[async_trait]
    impl BuiltinHookHandler for FixedPostHook {
        async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: Some(self.response.clone()),
            }
        }
        fn id(&self) -> &str {
            "fixed-post"
        }
    }

    fn post_hook_executor(response: HookResponse) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "fixed-post".into(),
            events: vec![HookEventType::PostToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "fixed-post".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
            async_rewake: false,
            async_timeout: None,
            rewake_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(FixedPostHook { response }));
        Arc::new(exec)
    }

    fn orch_with_post_hook(response: HookResponse) -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            post_hook_executor(response),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(crate::test_support::StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    fn uses() -> Vec<(ToolUseId, String, serde_json::Value, Option<String>)> {
        vec![(ToolUseId::new(), "Echo".into(), json!({}), None)]
    }

    /// Same as [`post_hook_executor`] but registered for `PostToolBatch`, the
    /// once-per-batch event fired after every tool in the batch has run.
    fn batch_hook_executor(response: HookResponse) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "fixed-batch".into(),
            events: vec![HookEventType::PostToolBatch],
            if_condition: None,
            // Must match `FixedPostHook::id()` — the registry resolves the
            // builtin by handler id, and a mismatch silently never fires.
            executor: DefHookExecutor::Builtin {
                handler_id: "fixed-post".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
            async_rewake: false,
            async_timeout: None,
            rewake_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(FixedPostHook { response }));
        Arc::new(exec)
    }

    fn orch_with_batch_hook(response: HookResponse) -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            batch_hook_executor(response),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(crate::test_support::StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// A `PostToolBatch` hook's `preventContinuation` STOPS the turn.
    ///
    /// The batch fire used to discard its aggregate entirely
    /// (`let _batch_agg = …`) under a comment calling `PostToolBatch`
    /// "observational". The oracle disagrees (2.1.220 @233161375):
    ///
    /// ```js
    /// if(Mn.blockingError)Mr=!0,Qn??=Mn.blockingError.blockingError;
    /// if(Mn.preventContinuation)Mr=!0,Qn??=Mn.stopReason
    /// …
    /// if(Mr)return yield Va({type:"hook_stopped_continuation",
    ///   message:Qn||"Execution stopped by PostToolBatch hook",
    ///   hookName:"PostToolBatch",toolUseID:rt,hookEvent:"PostToolBatch"},f),
    ///   n$e(er,a),{reason:"hook_stopped"}
    /// ```
    ///
    /// `{reason:"hook_stopped"}` is a turn-ending return, so the flag must
    /// propagate — unlike the PostToolUse case, where the port deliberately
    /// leaves the open question noted rather than guessing.
    #[tokio::test]
    async fn post_tool_batch_prevent_continuation_stops_the_turn() {
        let orch = orch_with_batch_hook(HookResponse {
            prevent_continuation: true,
            reason: Some("BATCH-STOP".into()),
            ..HookResponse::default()
        });
        let (_results, prevent, _injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses(), None)
                .await
                .expect("dispatch");
        assert!(
            prevent,
            "a PostToolBatch hook requesting preventContinuation must end the turn"
        );
    }

    /// The MODEL must be told why the turn stopped.
    ///
    /// The oracle yields the attachment into the message stream and derives the
    /// prose from it later, in `normalizeAttachmentForAPI` (@238107808):
    /// `hook_stopped_continuation:(e)=>[zr({content:Ww(`${e.hookName} hook
    /// stopped continuation: ${e.message}`),isMeta:!0})]`. This port has no such
    /// normalize layer — every other site (`Stop`, `PreToolUse`, `PostToolUse`)
    /// builds the `<system-reminder>` prose explicitly beside the attachment —
    /// so the batch site must too, or the stop reaches the transcript but never
    /// the model.
    ///
    /// Both records carry the SAME synthetic `hook-<uuid>` id, so the prose and
    /// the attachment describe one event rather than drifting apart.
    #[tokio::test]
    async fn post_tool_batch_stop_is_explained_to_the_model() {
        let orch = orch_with_batch_hook(HookResponse {
            prevent_continuation: true,
            reason: Some("BATCH-STOP".into()),
            ..HookResponse::default()
        });
        let (_results, _prevent, injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses(), None)
                .await
                .expect("dispatch");
        let stop_msg = injected
            .iter()
            .find(|(m, _)| m.text_content().contains("hook stopped continuation"))
            .expect("the batch stop must be explained to the model");
        assert_eq!(
            stop_msg.0.text_content(),
            "<system-reminder>\nPostToolBatch hook stopped continuation: BATCH-STOP\n</system-reminder>"
        );
        assert!(
            stop_msg.1.as_str().starts_with("hook-"),
            "prose and attachment must share the synthetic batch id, got {}",
            stop_msg.1.as_str()
        );
    }

    /// A `PostToolBatch` hook's `additionalContext` reaches the model.
    ///
    /// The same discarded aggregate carried this too (@233161375, inside the
    /// per-hook loop and therefore BEFORE the stop check):
    ///
    /// ```js
    /// if(Mn.additionalContexts&&Mn.additionalContexts.length>0){
    ///   let ko=Va({type:"hook_additional_context",content:Mn.additionalContexts,
    ///     hookName:"PostToolBatch",toolUseID:rt,hookEvent:"PostToolBatch"},f);
    ///   yield ko,Qe.push(ko)}
    /// ```
    ///
    /// It is independent of `preventContinuation`: a batch hook can contribute
    /// context without stopping anything.
    #[tokio::test]
    async fn post_tool_batch_additional_context_reaches_the_model() {
        let orch = orch_with_batch_hook(HookResponse {
            additional_context: Some("BATCH-CTX".into()),
            ..HookResponse::default()
        });
        let (_results, prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .expect("dispatch");
        assert!(!prevent, "additionalContext alone must not stop the turn");
        let ctx_msg = injected
            .iter()
            .find(|(m, _)| m.text_content().contains("BATCH-CTX"))
            .expect("the batch additionalContext must reach the model");
        assert_eq!(
            ctx_msg.0.text_content(),
            "<system-reminder>\nPostToolBatch hook additional context: BATCH-CTX\n</system-reminder>"
        );
    }

    /// Ordering: the oracle yields `hook_additional_context` inside the per-hook
    /// loop and the stop record only AFTER it, so a hook doing both produces the
    /// context first.
    #[tokio::test]
    async fn batch_additional_context_is_ordered_before_the_stop() {
        let orch = orch_with_batch_hook(HookResponse {
            additional_context: Some("CTX".into()),
            prevent_continuation: true,
            reason: Some("STOP".into()),
            ..HookResponse::default()
        });
        let (_results, prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses(), None)
            .await
            .expect("dispatch");
        assert!(prevent);
        let texts: Vec<String> = injected
            .iter()
            .map(|(m, _)| m.text_content())
            .filter(|t| t.contains("PostToolBatch"))
            .collect();
        assert_eq!(
            texts,
            vec![
                "<system-reminder>\nPostToolBatch hook additional context: CTX\n</system-reminder>",
                "<system-reminder>\nPostToolBatch hook stopped continuation: STOP\n</system-reminder>",
            ]
        );
    }

    /// A quiet batch hook injects nothing — the guard must not add a message to
    /// every turn.
    #[tokio::test]
    async fn a_quiet_post_tool_batch_hook_injects_no_message() {
        let orch = orch_with_batch_hook(HookResponse::default());
        let (_results, _prevent, injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses(), None)
                .await
                .expect("dispatch");
        assert!(
            !injected
                .iter()
                .any(|(m, _)| m.text_content().contains("hook stopped continuation")),
            "no stop, no explanation"
        );
    }

    /// `Mr` is set by a blocking error too, not only by `preventContinuation`.
    #[tokio::test]
    async fn post_tool_batch_blocking_error_also_stops_the_turn() {
        let orch = orch_with_batch_hook(HookResponse {
            decision: Some(hooks::response::HookDecision::Block),
            reason: Some("BATCH-BLOCK".into()),
            ..HookResponse::default()
        });
        let (_results, prevent, _injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses(), None)
                .await
                .expect("dispatch");
        assert!(
            prevent,
            "`if(Mn.blockingError)Mr=!0` — a batch blocking error stops the turn too"
        );
    }

    /// A batch hook that asks for nothing leaves the turn alone — the guard
    /// must not turn every batch into a stop.
    #[tokio::test]
    async fn a_quiet_post_tool_batch_hook_does_not_stop_the_turn() {
        let orch = orch_with_batch_hook(HookResponse::default());
        let (_results, prevent, _injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses(), None)
                .await
                .expect("dispatch");
        assert!(!prevent, "a no-op PostToolBatch hook must not end the turn");
    }

    /// The record the oracle yields alongside the stop: `hookName` and
    /// `hookEvent` are the bare literal `PostToolBatch` (NOT suffixed with a
    /// tool name the way `PostToolUse:{tool}` is), and `toolUseID` is the
    /// SYNTHETIC `hook-${uuid}` the oracle binds as `rt` — no real tool's id,
    /// because the event covers the whole batch.
    #[test]
    fn post_tool_batch_stopped_continuation_matches_the_oracle_shape() {
        let attachment = hooks::stopped_continuation_attachment(
            &post_tool_batch_identity(),
            "Execution stopped by PostToolBatch hook",
        );
        let id = attachment["toolUseID"].as_str().expect("toolUseID");
        assert!(
            id.starts_with("hook-"),
            "the batch attachment carries a synthetic `hook-<uuid>` id, got {id}"
        );
        assert_eq!(
            serde_json::to_string(&attachment).unwrap(),
            format!(
                r#"{{"type":"hook_stopped_continuation","message":"Execution stopped by PostToolBatch hook","hookName":"PostToolBatch","toolUseID":"{id}","hookEvent":"PostToolBatch"}}"#
            )
        );
    }

    /// The PostToolUse `additionalContext` reaches the model EXACTLY ONCE — as
    /// the injected `isMeta` rendering — and is NOT also concatenated onto the
    /// tool_result string.
    #[tokio::test]
    async fn post_tool_use_additional_context_is_not_folded_into_the_tool_result() {
        let orch = orch_with_post_hook(HookResponse {
            additional_context: Some("POST-CTX".into()),
            ..HookResponse::default()
        });
        let uses = uses();
        let (results, _prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");
        let ContentBlock::ToolResult { content, .. } = &results[0] else {
            panic!("expected ToolResult");
        };
        assert!(
            !content.contains("POST-CTX"),
            "claude never folds PostToolUse additionalContext into the \
             tool_result string (BIN off 235420375 / 234726655), got: {content}"
        );
        assert_eq!(injected.len(), 1, "exactly one model-facing rendering");
        assert!(
            injected[0].0.is_meta(),
            "the rendering is `zr({{isMeta:true}})` (BIN off 238107100)"
        );
    }

    /// The same context is queued as ONE `hook_additional_context` attachment
    /// keyed to the tool, ready for the driver to flush after the tool_result.
    #[tokio::test]
    async fn post_tool_use_additional_context_is_queued_as_an_attachment() {
        let orch = orch_with_post_hook(HookResponse {
            additional_context: Some("POST-CTX".into()),
            ..HookResponse::default()
        });
        let uses = uses();
        let id = uses[0].0.clone();
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");
        let queued = orch.take_queued_hook_attachments(&id).await;
        assert_eq!(queued.len(), 1, "one attachment, got {queued:?}");
        assert_eq!(
            serde_json::to_string(&queued[0]).unwrap(),
            format!(
                r#"{{"type":"hook_additional_context","content":["POST-CTX"],"hookName":"PostToolUse:Echo","toolUseID":"{id}","hookEvent":"PostToolUse"}}"#
            )
        );
    }

    /// O2: a PostToolUse hook that BLOCKS produces a `hook_blocking_error`
    /// attachment AND a model-facing `isMeta` rendering.
    ///
    /// Before this, `post_agg.decision` was never read at all — a PostToolUse
    /// hook exiting 2 produced NOTHING in the port, while the oracle produces
    /// both records (BIN off 234726074 for the attachment, 238107476 for the
    /// prose, which is one of the few hook attachments that IS model-facing).
    ///
    /// `blockingError.command` is `qq(hook)`; the fixture hook is a Builtin, so
    /// that renders as its handler id.
    #[tokio::test]
    async fn post_tool_use_block_emits_a_blocking_error_attachment_and_meta() {
        let orch = orch_with_post_hook(HookResponse {
            decision: Some(hooks::response::HookDecision::Block),
            reason: Some("nope".into()),
            ..HookResponse::default()
        });
        let uses = uses();
        let id = uses[0].0.clone();
        let (_results, _prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");

        let queued = orch.take_queued_hook_attachments(&id).await;
        assert_eq!(queued.len(), 1, "one attachment, got {queued:?}");
        assert_eq!(
            serde_json::to_string(&queued[0]).unwrap(),
            format!(
                r#"{{"type":"hook_blocking_error","hookName":"PostToolUse:Echo","toolUseID":"{id}","hookEvent":"PostToolUse","blockingError":{{"blockingError":"nope","command":"fixed-post"}}}}"#
            )
        );
        assert_eq!(injected.len(), 1, "one model-facing rendering");
        assert_eq!(
            injected[0].0.text_content(),
            "<system-reminder>\nPostToolUse:Echo hook blocking error from command: \"fixed-post\": nope\n</system-reminder>"
        );
    }

    /// The blocking-error default reason is `"Blocked by hook"` (capital B) —
    /// `e.reason||"Blocked by hook"` at BIN off 237775430.
    #[tokio::test]
    async fn post_tool_use_block_without_a_reason_uses_the_oracle_default() {
        let orch = orch_with_post_hook(HookResponse {
            decision: Some(hooks::response::HookDecision::Block),
            ..HookResponse::default()
        });
        let uses = uses();
        let id = uses[0].0.clone();
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");
        let queued = orch.take_queued_hook_attachments(&id).await;
        assert_eq!(
            queued[0]["blockingError"]["blockingError"],
            "Blocked by hook"
        );
    }

    /// O2: the PostToolUse `preventContinuation` message was already
    /// byte-correct for the MODEL, but nothing was ever PERSISTED. The oracle
    /// records a `hook_stopped_continuation` attachment beside it
    /// (BIN off 234726408), whose `message` sits SECOND in key order.
    #[tokio::test]
    async fn post_tool_use_prevent_continuation_is_persisted_as_an_attachment() {
        let orch = orch_with_post_hook(HookResponse {
            prevent_continuation: true,
            reason: Some("POST-STOP".into()),
            ..HookResponse::default()
        });
        let uses = uses();
        let id = uses[0].0.clone();
        // NOTE: the returned `prevent` flag is deliberately NOT asserted here.
        // The port sets its loop flag from the PRE-hook aggregate only
        // (`turn_loop.rs:2646`); `post_agg.prevent_continuation` reaches the
        // message/attachment but not the flag. Whether the oracle's `return` at
        // BIN off 234726408 ends the whole turn or only the post-hook generator
        // is a SEPARATE question this cluster did not investigate — see the
        // residual note rather than assuming an answer here.
        let (_results, _prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");

        let queued = orch.take_queued_hook_attachments(&id).await;
        assert_eq!(queued.len(), 1, "one attachment, got {queued:?}");
        assert_eq!(
            serde_json::to_string(&queued[0]).unwrap(),
            format!(
                r#"{{"type":"hook_stopped_continuation","message":"POST-STOP","hookName":"PostToolUse:Echo","toolUseID":"{id}","hookEvent":"PostToolUse"}}"#
            )
        );
        // The model-facing prose is UNCHANGED by this work.
        assert_eq!(
            injected[0].0.text_content(),
            "<system-reminder>\nPostToolUse:Echo hook stopped continuation: POST-STOP\n</system-reminder>"
        );
    }

    /// Both records for one hook, in the oracle's yield order: the
    /// blocking-error comes BEFORE the stopped-continuation (BIN off 234726074
    /// yields `hook_blocking_error`, then `preventContinuation` returns).
    #[tokio::test]
    async fn blocking_error_is_ordered_before_stopped_continuation() {
        let orch = orch_with_post_hook(HookResponse {
            decision: Some(hooks::response::HookDecision::Block),
            reason: Some("both".into()),
            prevent_continuation: true,
            ..HookResponse::default()
        });
        let uses = uses();
        let id = uses[0].0.clone();
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");
        let queued = orch.take_queued_hook_attachments(&id).await;
        let kinds: Vec<_> = queued
            .iter()
            .map(|v| v["type"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(kinds, ["hook_blocking_error", "hook_stopped_continuation"]);
    }

    /// END-TO-END (O1): the value recorded at DISPATCH reaches the transcript
    /// LINE. Guards against `record_tool_use_result` being computed but never
    /// published — the record site lives in `turn_loop.rs` and the consume site
    /// in `conversation.rs`, so neither file's unit tests alone prove the seam.
    #[tokio::test]
    async fn dispatched_tool_result_reaches_the_transcript_as_tool_use_result() {
        use protocol::MessageId;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let fs: Arc<dyn traits::FileSystem> = Arc::new(platform_posix::fs::PosixFileSystem::new(
            dir.path().to_path_buf(),
        ));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(path.clone(), fs));
        let mut tools = ToolRegistry::new();
        tools.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(tools),
            crate::test_support::noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(crate::test_support::StaticMemoryProvider::empty()),
            dir.path().to_path_buf(),
        )
        .with_jsonl_writer(writer);

        let uses = uses();
        let (results, _prevent, _injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: results,
            is_meta: false,
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
        };
        orch.persist_message_to_jsonl(&msg).await;

        let raw = std::fs::read_to_string(&path).expect("read jsonl");
        assert!(
            raw.contains(r#""toolUseResult":{"out":"ECHOED-OUTPUT"}"#),
            "the tool's structured `data` must reach the line verbatim, got: {raw}"
        );
    }

    /// O3: a PostToolUse `updatedToolOutput` that fails the tool's output
    /// schema produces a `hook_error_during_execution` ATTACHMENT and NOTHING
    /// the model can see — the renderer maps that attachment type to `[]`
    /// (BIN off 238107100). The port used to push the notice text onto the
    /// injected channel, so the model read a warning claude suppresses.
    #[tokio::test]
    async fn schema_mismatch_notice_is_an_attachment_the_model_never_sees() {
        let orch = orch_with_post_hook(HookResponse {
            updated_tool_output: Some(Some(json!({ "unexpected": true }))),
            ..HookResponse::default()
        });
        let uses = uses();
        let id = uses[0].0.clone();
        let (_results, _prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");
        assert!(
            !injected
                .iter()
                .any(|(m, _)| m.text_content().contains("does not match")),
            "the schema-mismatch notice must not reach the model: {injected:?}"
        );
        let queued = orch.take_queued_hook_attachments(&id).await;
        assert_eq!(queued.len(), 1, "one attachment, got {queued:?}");
        assert_eq!(
            queued[0].get("type").and_then(serde_json::Value::as_str),
            Some("hook_error_during_execution")
        );
        assert!(queued[0]
            .get("content")
            .and_then(serde_json::Value::as_str)
            .expect("string content")
            .contains("does not match Echo's output shape"));
    }

    /// Build an orchestrator whose only hook is a PreToolUse hook returning
    /// `response`, optionally with a JSONL writer so persisted attachment
    /// LINES (not just queued payloads) can be inspected.
    fn orch_with_pre_hook(
        response: HookResponse,
        jsonl: Option<&std::path::Path>,
    ) -> ConversationOrchestrator {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "fixed-pre".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "fixed-post".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
            async_rewake: false,
            async_timeout: None,
            rewake_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let mut exec = HookExecutorImpl::new(
            Arc::new(tokio::sync::RwLock::new(registry)),
            Arc::new(UnusedHttp),
            Arc::new(UnusedRuntime),
        );
        exec.register_builtin(Arc::new(FixedPostHook { response }));
        let mut tools = ToolRegistry::new();
        tools.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(tools),
            Arc::new(exec),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(crate::test_support::StaticMemoryProvider::empty()),
            jsonl.map_or_else(
                || PathBuf::from("/tmp"),
                |p| p.parent().expect("parent").to_path_buf(),
            ),
        );
        match jsonl {
            None => orch,
            Some(path) => {
                let root = path.parent().expect("parent").to_path_buf();
                let fs: Arc<dyn traits::FileSystem> =
                    Arc::new(platform_posix::fs::PosixFileSystem::new(root));
                orch.with_jsonl_writer(Arc::new(session::jsonl::writer::JsonlWriter::new(
                    path.to_path_buf(),
                    fs,
                )))
            }
        }
    }

    /// O2: the PreToolUse `preventContinuation` record. The model-facing prose
    /// was already byte-correct (`Execution stopped by hook` default, BIN off
    /// 235403061); only the persisted attachment was missing.
    #[tokio::test]
    async fn pre_tool_use_prevent_continuation_is_persisted_as_an_attachment() {
        let orch = orch_with_pre_hook(
            HookResponse {
                prevent_continuation: true,
                ..HookResponse::default()
            },
            None,
        );
        let uses = uses();
        let id = uses[0].0.clone();
        let (_results, _prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");
        let queued = orch.take_queued_hook_attachments(&id).await;
        assert_eq!(queued.len(), 1, "one attachment, got {queued:?}");
        assert_eq!(
            serde_json::to_string(&queued[0]).unwrap(),
            format!(
                r#"{{"type":"hook_stopped_continuation","message":"Execution stopped by hook","hookName":"PreToolUse:Echo","toolUseID":"{id}","hookEvent":"PreToolUse"}}"#
            )
        );
        assert!(
            injected.iter().any(|(m, _)| m.text_content()
                == "<system-reminder>\nPreToolUse:Echo hook stopped continuation: Execution stopped by hook\n</system-reminder>"),
            "the existing prose is unchanged: {injected:?}"
        );
    }

    /// O2 / Phase 4: a DEFERRED tool persists a `hook_deferred_tool` attachment
    /// LINE and sends the model NOTHING.
    ///
    /// The port previously pushed the raw JSON payload onto the injected
    /// channel as a plain user message, so the model read a blob the oracle
    /// suppresses (`hook_deferred_tool:()=>[]`, BIN off 238109388) while the
    /// resume scanner `QAs` (BIN off 237925753) — which greps the transcript
    /// for `'"hook_deferred_tool"'` inside a `type:"attachment"` line — found
    /// nothing at all.
    ///
    /// The record must be persisted IMMEDIATELY rather than queued: the queue
    /// is flushed after a tool_result, and a deferred tool never produces one.
    #[tokio::test]
    async fn deferred_tool_is_persisted_and_never_shown_to_the_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let orch = orch_with_pre_hook(
            HookResponse {
                decision: Some(hooks::response::HookDecision::Defer),
                ..HookResponse::default()
            },
            Some(&path),
        );
        let uses = uses();
        let id = uses[0].0.clone();
        let (results, prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");

        assert!(prevent, "a deferred tool terminates the turn");
        assert!(results.is_empty(), "the deferred tool never ran");
        assert!(
            !injected
                .iter()
                .any(|(m, _)| m.text_content().contains("hook_deferred_tool")),
            "the model must NEVER see the deferred-tool payload: {injected:?}"
        );

        let raw = std::fs::read_to_string(&path).expect("read jsonl");
        let line = raw
            .lines()
            .find(|l| l.contains("hook_deferred_tool"))
            .unwrap_or_else(|| panic!("no hook_deferred_tool attachment line in: {raw}"));
        let v: serde_json::Value = serde_json::from_str(line).expect("json line");
        assert_eq!(
            v["type"], "attachment",
            "`QAs` requires the enclosing line to be type:\"attachment\""
        );
        assert_eq!(
            serde_json::to_string(&v["attachment"]).unwrap(),
            format!(
                r#"{{"type":"hook_deferred_tool","toolUseID":"{id}","toolName":"Echo","toolInput":{{}},"hookName":"session","hookEvent":"PreToolUse","permissionMode":"default"}}"#
            )
        );
    }

    /// A PreToolUse `additionalContext` gets the same treatment.
    #[tokio::test]
    async fn pre_tool_use_additional_context_is_queued_and_rendered_as_meta() {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "fixed-pre".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "fixed-post".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
            async_rewake: false,
            async_timeout: None,
            rewake_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(FixedPostHook {
            response: HookResponse {
                additional_context: Some("PRE-CTX".into()),
                ..HookResponse::default()
            },
        }));
        let mut tools = ToolRegistry::new();
        tools.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(tools),
            Arc::new(exec),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(crate::test_support::StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let uses = uses();
        let id = uses[0].0.clone();
        let (_results, _prevent, injected, _mods) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");
        let queued = orch.take_queued_hook_attachments(&id).await;
        assert_eq!(queued.len(), 1, "one attachment, got {queued:?}");
        assert_eq!(
            queued[0]
                .get("hookName")
                .and_then(serde_json::Value::as_str),
            Some("PreToolUse:Echo")
        );
        let rendering = injected
            .iter()
            .find(|(m, _)| matches!(m, ConversationMessage::User { .. }))
            .expect("a rendering");
        assert!(
            rendering.0.is_meta(),
            "the PreToolUse rendering is isMeta too"
        );
    }
}

/// A1 — the `<persisted-output>` substitution wired into the SUCCESS-path
/// `tool_result` push (claude-code 2.1.220 `F0u`, BIN off **230270568**).
#[cfg(test)]
mod tool_result_persistence_wiring_tests {
    use super::{dispatch_tool_uses_tracked, ConversationOrchestrator};
    use crate::test_support::{MockApiClient, MockOutputStream, NoOpPermissionGate};
    use crate::tool_result_persistence::{PERSISTED_OUTPUT_OPEN, TOOL_RESULTS_DIR};
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use protocol::{ContentBlock, ToolUseId};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// Emits `input.len` bytes of `x` as its model content, plus (when
    /// `input.blocks` is set) a raw `content_blocks` array. Declares a 100-byte
    /// persistence threshold so the boundary is cheap to drive.
    struct SizedTool;

    const THRESHOLD: usize = 100;

    #[async_trait]
    impl Tool for SizedTool {
        fn name(&self) -> &str {
            "Sized"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn persistence_threshold(&self) -> Option<usize> {
            Some(THRESHOLD)
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "sized".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            let len = usize::try_from(
                input
                    .get("len")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap(),
            )
            .unwrap();
            let body = "x".repeat(len);
            Ok(ToolCallResult {
                data: json!(body),
                model_content: Some(body),
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    fn orch_with(tool: Arc<dyn Tool>, config_home: Option<PathBuf>) -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(tool);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            crate::test_support::noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(crate::test_support::StaticMemoryProvider::empty()),
            PathBuf::from("/tmp/wsp"),
        );
        match config_home {
            Some(h) => orch.with_config_home(h),
            None => orch,
        }
    }

    fn use_of(
        name: &str,
        len: usize,
    ) -> Vec<(ToolUseId, String, serde_json::Value, Option<String>)> {
        vec![(ToolUseId::new(), name.into(), json!({ "len": len }), None)]
    }

    async fn dispatch_content(
        orch: &ConversationOrchestrator,
        uses: &[(ToolUseId, String, serde_json::Value, Option<String>)],
    ) -> String {
        let (results, _p, _i, _m) = dispatch_tool_uses_tracked(orch, uses, None)
            .await
            .expect("dispatch");
        let ContentBlock::ToolResult { content, .. } = &results[0] else {
            panic!("expected ToolResult");
        };
        content.clone()
    }

    /// T5 — `o<=i` returns the result UNCHANGED; only a STRICTLY larger body
    /// is persisted.
    #[tokio::test]
    async fn exactly_at_the_threshold_is_not_persisted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let orch = orch_with(Arc::new(SizedTool), Some(tmp.path().to_path_buf()));
        let content = dispatch_content(&orch, &use_of("Sized", THRESHOLD)).await;
        assert_eq!(content, "x".repeat(THRESHOLD));
    }

    #[tokio::test]
    async fn one_byte_over_the_threshold_is_persisted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let orch = orch_with(Arc::new(SizedTool), Some(tmp.path().to_path_buf()));
        let content = dispatch_content(&orch, &use_of("Sized", THRESHOLD + 1)).await;
        assert!(
            content.starts_with(PERSISTED_OUTPUT_OPEN),
            "expected the persisted envelope, got: {content}"
        );
        assert!(content.contains("Output too large (101 bytes)."));
    }

    /// T8 — the file lands at
    /// `<config_home>/projects/<project_dir_name(cwd)>/<uuid>/tool-results/<id>.txt`.
    #[tokio::test]
    async fn persisted_file_is_session_scoped_and_holds_the_full_body() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let orch = orch_with(Arc::new(SizedTool), Some(tmp.path().to_path_buf()));
        let uses = use_of("Sized", 5_000);
        let content = dispatch_content(&orch, &uses).await;
        let session_uuid = {
            let s = orch.session.lock().await;
            s.session_id.as_uuid().to_string()
        };
        let dir = tmp
            .path()
            .join("projects")
            .join(session::jsonl::path::project_dir_name(
                &orch.current_cwd().to_string_lossy(),
            ))
            .join(session_uuid)
            .join(TOOL_RESULTS_DIR);
        let file = dir.join(format!("{}.txt", uses[0].0.as_str()));
        assert!(
            file.exists(),
            "expected {} to exist; envelope was: {content}",
            file.display()
        );
        assert_eq!(
            std::fs::read_to_string(&file).expect("read back").len(),
            5_000
        );
        assert!(content.contains(&file.display().to_string()));
    }

    /// T6 — `U0u`: a block array containing an image (or document) is NEVER
    /// persisted, however large its TEXT blocks are. The control below proves
    /// the same array WITHOUT the media block does persist, so the assertion
    /// isolates the guard rather than the size check.
    #[tokio::test]
    async fn media_bearing_block_arrays_are_never_persisted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let orch = orch_with(Arc::new(SizedTool), Some(tmp.path().to_path_buf()));
        let big_text = json!({ "type": "text", "text": "z".repeat(5_000) });
        let id = ToolUseId::new();

        for media in ["image", "document"] {
            let blocks = vec![big_text.clone(), json!({ "type": media })];
            let out = super::apply_tool_result_persistence(
                &orch,
                "Sized",
                &id,
                Some(THRESHOLD),
                "IGNORED".into(),
                Some(&blocks),
            )
            .await
            .content;
            assert_eq!(out, "IGNORED", "{media} block must suppress persistence");
        }
        assert!(!tmp.path().join("projects").exists());

        // Control: the identical array minus the media block IS persisted.
        let blocks = vec![big_text];
        let out = super::apply_tool_result_persistence(
            &orch,
            "Sized",
            &id,
            Some(THRESHOLD),
            "IGNORED".into(),
            Some(&blocks),
        )
        .await
        .content;
        assert!(out.starts_with(PERSISTED_OUTPUT_OPEN), "control: {out}");
        // An ARRAY body is written as pretty JSON under a `.json` stem (`kKr`).
        assert!(out.contains(&format!("{}.json", id.as_str())), "{out}");
    }

    /// An MCP-shaped result (array `data` ⇒ `content_blocks: Some(..)`) must
    /// have its ARRAY dropped when the payload is persisted.
    ///
    /// claude-code's `F0u` substitutes the ONE model-facing payload
    /// (`{...e, content: a}`, where `content` is a string OR an array). LingXi
    /// splits it across `content` and `content_blocks`, and the wire prefers
    /// the array when present (`llm-client/src/convert.rs`:
    /// `content_blocks.map_or_else(|| String(content), Array)`). Substituting
    /// only `content` therefore wrote the file, fired the telemetry, and still
    /// handed the model the full oversized array — the defect this pins.
    #[tokio::test]
    async fn persisting_an_mcp_array_result_drops_the_array() {
        struct McpArrayTool;
        #[async_trait]
        impl Tool for McpArrayTool {
            fn name(&self) -> &str {
                "mcp__srv__big"
            }
            fn input_schema(&self) -> &serde_json::Value {
                static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                    once_cell::sync::Lazy::new(|| json!({ "type": "object" }));
                &SCHEMA
            }
            fn is_enabled(&self, _: &ToolStaticContext) -> bool {
                true
            }
            fn is_mcp(&self) -> bool {
                true
            }
            fn max_result_size_chars(&self) -> usize {
                1024 * 1024
            }
            fn persistence_threshold(&self) -> Option<usize> {
                Some(THRESHOLD)
            }
            fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
                true
            }
            fn is_read_only(&self, _: &serde_json::Value) -> bool {
                true
            }
            async fn validate_input(
                &self,
                _: &serde_json::Value,
                _: &ToolUseContext,
            ) -> Result<(), ValidationError> {
                Ok(())
            }
            async fn check_permissions(
                &self,
                _: &serde_json::Value,
                _: &ToolUseContext,
            ) -> permission::PermissionResult {
                permission::PermissionResult::Allow {
                    reason: permission::PermissionDecisionReason::Other {
                        reason: "test".into(),
                    },
                    updated_input: None,
                    update_destination: None,
                    metadata: permission::result::PermissionMetadata::default(),
                }
            }
            async fn description(&self, _: &serde_json::Value, _: &DescriptionOptions) -> String {
                "big".into()
            }
            async fn prompt(&self, _: &PromptOptions) -> String {
                String::new()
            }
            async fn call(
                &self,
                _: serde_json::Value,
                _: ToolUseContext,
                _: ToolProgressSender,
            ) -> Result<ToolCallResult, ToolError> {
                let body = "y".repeat(THRESHOLD * 4);
                Ok(ToolCallResult {
                    data: json!([{ "type": "text", "text": body }]),
                    model_content: Some(body),
                    new_messages: vec![],
                    context_modifier: None,
                    is_error: false,
                    mcp_meta: None,
                })
            }
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let orch = orch_with(Arc::new(McpArrayTool), Some(tmp.path().to_path_buf()));
        let uses = vec![(
            ToolUseId::new(),
            "mcp__srv__big".to_string(),
            json!({}),
            None,
        )];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .expect("dispatch");

        match &results[0] {
            ContentBlock::ToolResult {
                content,
                content_blocks,
                ..
            } => {
                assert!(
                    content.starts_with(PERSISTED_OUTPUT_OPEN),
                    "oversized MCP result must be persisted, got: {content}"
                );
                assert!(
                    content_blocks.is_none(),
                    "the array must be dropped once the payload is substituted, \
                     else the wire sends it and the envelope is discarded"
                );
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    /// T7 — no `config_home` (library/test callers) is a STRICT no-op.
    #[tokio::test]
    async fn without_a_config_home_the_content_is_untouched() {
        let orch = orch_with(Arc::new(SizedTool), None);
        let content = dispatch_content(&orch, &use_of("Sized", 5_000)).await;
        assert_eq!(content, "x".repeat(5_000));
    }

    /// `Gzg` — a blank result becomes `(${toolName} completed with no output)`.
    /// 1 933 occurrences in the real binary's own transcripts; the port emitted
    /// 57 EMPTY Bash tool_results instead.
    #[tokio::test]
    async fn blank_results_become_the_no_output_sentinel() {
        let orch = orch_with(Arc::new(SizedTool), None);
        let content = dispatch_content(&orch, &use_of("Sized", 0)).await;
        assert_eq!(content, "(Sized completed with no output)");
    }
}
