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

/// Tools whose successful execution records `file_path` (or `notebook_path`)
/// into the orchestrator's read-file-state cache. Mirrors the TS sites that
/// call `readFileState.set(expandPath(file_path), …)` (`FileReadTool` +
/// `FileEditTool`/`FileWriteTool`/`MultiEditTool`/`NotebookEditTool`).
/// `/files` then renders this set (TS `cacheKeys(context.readFileState)`).
const READ_FILE_STATE_TOOLS: &[&str] = &["Read", "Edit", "Write", "MultiEdit", "NotebookEdit"];

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

/// Lexically expand a tool's `file_path` to an absolute, normalized path — the
/// cache key for [`ConversationOrchestrator::files_in_context`]. 1:1 with TS
/// `expandPath` (`src/utils/path.ts`): trim; `~`/`~/…` → home; absolute kept;
/// relative joined on `cwd`; then lexically collapsed (`.` dropped, `..` popped).
/// LEXICAL only (not `realpath`) — never touches disk, symlinks preserved.
/// Forced divergences (not gaps): Windows `/c/Users/…` conversion and NFC
/// normalization skipped (macOS/Linux ASCII parity target).
fn absolutize(cwd: &Path, raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    let expanded: PathBuf = if trimmed == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(trimmed))
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        dirs::home_dir().map_or_else(|| PathBuf::from(trimmed), |h| h.join(rest))
    } else {
        let p = Path::new(trimmed);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        }
    };
    normalize_lexically(&expanded)
}

/// Collapse `.` and `..` segments without touching the filesystem, mirroring
/// Node's `path.normalize`/`resolve` (used by `expandPath`). A `..` pops the
/// previous normal component; a leading `..` with nothing to pop is kept.
fn normalize_lexically(path: &Path) -> PathBuf {
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
                "Hook {hook_name} returned a terminalSequence that was rejected by the allowlist (only OSC 0/1/2/9/99/777 and BEL are permitted)"
            );
        }
    }
}

// Tests for this module live in the sibling `turn_loop_test.rs`.
#[cfg(test)]
#[path = "turn_loop_test.rs"]
mod turn_loop_test;

/// Record a successful `Read`/`Edit`/`Write`/… into the read-file-state cache
/// backing `/files`. Best-effort, order-preserving: pulls `file_path` (or
/// `notebook_path` for `NotebookEdit`), absolutizes it, and keeps the FIRST
/// insertion. Matches TS for ≤2 files (`a,b,a → [a,b]`); diverges from TS's
/// MRU LRU at ≥3 (`a,b,c,a` → TS `[a,c,b]` vs `[a,b,c]`). Silently skips an
/// absent/non-string path or unknown tool. Populates only the ordered `Vec`;
/// the richer [`ConversationOrchestrator::read_state_map`] (1:1 TS
/// `readFileState`) is filled by the tools' own `read_file_state.set` over a
/// shared `Arc`. NotebookEdit's `~`/trim skip and BashTool writes are unported.
async fn record_read_file_state(
    orch: &ConversationOrchestrator,
    name: &str,
    effective_input: &serde_json::Value,
) {
    if !READ_FILE_STATE_TOOLS.contains(&name) {
        return;
    }
    let key = if name == "NotebookEdit" {
        "notebook_path"
    } else {
        "file_path"
    };
    let Some(raw) = effective_input.get(key).and_then(serde_json::Value::as_str) else {
        return;
    };
    let absolute = absolutize(&orch.cwd, raw);
    let mut cache = orch.read_file_state.lock().await;
    if !cache.contains(&absolute) {
        cache.push(absolute);
    }
}

/// Maximum number of consecutive `max_tokens` recovery nudges before the
/// turn loop gives up and surfaces the `max_tokens` `stop_reason`. 1:1 with TS
/// `query.ts:164` `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT = 3`.
pub(crate) const MAX_OUTPUT_TOKENS_RECOVERY_LIMIT: u32 = 3;

/// Escalated output-token cap for the single-shot 8k→64k retry. 1:1 with TS
/// `utils/context.ts:25` `ESCALATED_MAX_TOKENS = 64_000`.
///
/// DEFERRED (A1): not wired into any API call yet. The api-client
/// `messages_create` signature carries no `max_tokens` override argument, so
/// the escalation retry cannot be performed crate-locally without editing
/// api-client (out of scope). [`RecoveryState::max_output_tokens_override`]
/// and [`crate::OrchestratorConfig::escalate_max_output_tokens`] are wired so
/// a follow-up can plumb this through without further config/struct churn.
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
/// deny path. LingXi has no protocol `isMeta` flag (cf. the
/// max-output-tokens nudge above), so the meta message is a plain user text
/// message carrying these exact bytes.
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
/// `LingXi` has no protocol `isMeta` flag (cf. the max-output-tokens nudge), so
/// the meta message is a plain user text message carrying these exact bytes.
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
/// (`bin/claude.exe` offset ~202947000). `LingXi` has no protocol `isMeta`
/// flag, so it is a plain user text message carrying these exact bytes.
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
    // handle is ready when `relevant_memory_reminder_message` awaits it below,
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
    if let Some(ctx_msg) = orch.additional_context_message().await {
        history_snapshot.insert(0, ctx_msg);
    }

    // OUTSTYLE.3: per-turn, transient output-style reminder. When a non-default
    // output style is active, claude-code injects a meta user message into EVERY
    // turn's model input (the `output_style` attachment). We append it to THIS
    // call's OUTGOING snapshot only — never to `session.history` / JSONL — so it
    // is recomputed each turn and never accumulates (TS recomputes attachments
    // each turn). Trailing position mirrors TS (`[userMessage,
    // ...attachmentMessages]`). `None` for the default style ⇒ no extra message,
    // keeping the locked turn-loop fixtures byte-identical. See
    // [`ConversationOrchestrator::output_style_reminder_message`].
    if let Some(reminder) = orch.output_style_reminder_message() {
        history_snapshot.push(reminder);
    }

    // SKILLLIST.1: per-turn, transient `skill_listing` reminder so the model can
    // discover the available skills. Appended to THIS call's OUTGOING snapshot
    // only (never to `session.history` / JSONL), after the output-style reminder
    // so the locked output-style fixtures stay byte-identical. `None` when no
    // provider is wired / no skills / the Skill tool is absent. See
    // [`ConversationOrchestrator::skill_listing_reminder_message`].
    if let Some(reminder) = orch.skill_listing_reminder_message().await {
        history_snapshot.push(reminder);
    }

    // §F: per-turn, transient `conditional_rules` reminder — path-gated LINGXI.md
    // rules that newly activate because a touched file matches their globs.
    // Appended to THIS call's OUTGOING snapshot only (never `session.history` /
    // JSONL), after the skill-listing reminder so the locked fixtures stay
    // byte-identical. `None` when no provider / no conditional rules / nothing
    // newly active. See [`ConversationOrchestrator::conditional_rules_reminder_message`].
    if let Some(reminder) = orch.conditional_rules_reminder_message().await {
        history_snapshot.push(reminder);
    }

    // Per-turn, transient `<new-diagnostics>` reminder — newly-reported LSP
    // diagnostics not yet surfaced (claude-code `formatDiagnosticsBlock`).
    // Appended to THIS call's OUTGOING snapshot only. `None` when no LSP source
    // is wired (no servers) or no new diagnostics. See
    // [`ConversationOrchestrator::new_diagnostics_reminder_message`].
    if let Some(reminder) = orch.new_diagnostics_reminder_message().await {
        history_snapshot.push(reminder);
    }

    // `agent_listing_delta`: per-turn, transient agent catalog reminder, emitted
    // when the `LINGXI_AGENT_LIST_IN_MESSAGES` gate is ON (the v2.1.193
    // DEFAULT — the catalog is externalized here, the `AgentTool` description
    // carries only the pointer line; an explicit `=false` opts into the legacy
    // inline catalog ⇒ `None` here). Appended to THIS call's OUTGOING snapshot
    // only (never `session.history` / JSONL), after the conditional-rules
    // reminder. See [`ConversationOrchestrator::agent_listing_reminder_message`].
    if let Some(reminder) = orch.agent_listing_reminder_message().await {
        history_snapshot.push(reminder);
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
        history_snapshot.push(reminder);
    }

    // async_hook_response (batched twin): fold completed background (`async`)
    // hook responses into THIS call's OUTGOING snapshot only (never
    // `session.history` / JSONL), drained consume-once. `None` when no source is
    // wired / nothing completed since the last turn. See
    // [`ConversationOrchestrator::async_hook_response_reminder_message`].
    if let Some(reminder) = orch.async_hook_response_reminder_message().await {
        history_snapshot.push(reminder);
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
        history_snapshot.push(reminder);
    }

    // P0.1 (batched twin): per-turn, transient `relevant_memories` SURFACING
    // reminder — the memory-selector/prefetch result rendered as one
    // `<system-reminder>` meta user message. Appended to THIS call's OUTGOING
    // snapshot only (never `session.history` / JSONL), after the async-hook
    // reminder and BEFORE the blocking-limit estimate inside
    // `call_api_with_ptl_recovery` so its tokens are counted in the prompt size.
    // Awaits the prefetch armed by `start_memory_prefetch` at turn start. `None`
    // when no prefetch is wired / empty result / everything already injected. See
    // [`ConversationOrchestrator::relevant_memory_reminder_message`].
    if let Some(reminder) = orch.relevant_memory_reminder_message().await {
        history_snapshot.push(reminder);
    }

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
            let assistant_id = surface_model_error(orch, &e.to_string(), env).await;
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
                    is_non_interactive_session: traits::session_flags::is_non_interactive_session(),
                    print: traits::session_flags::is_non_interactive_session(),
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
    // exporters capture response bodies. Gated env read keeps the locked turn
    // fixtures unchanged (var unset).
    if std::env::var_os("OTEL_LOG_ASSISTANT_RESPONSES").is_some_and(|v| v == "1" || v == "true") {
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
        // Persist the injected skill messages too (best-effort), mirroring the
        // tool_result persist above. No-op when empty. NOTE: the originating
        // tool_use_id is deliberately NOT persisted — TS does not write
        // `sourceToolUseID` to the transcript, so the JSONL bytes stay
        // byte-identical to before this change.
        for (m, _tool_use_id) in &injected_messages {
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

    // 6. Decide loop disposition.
    let outcome = if hook_prevent_continuation {
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
                surface_terminal_api_error(orch, other, response.stop_details.as_ref()).await;
                TurnStepOutcome::Ended {
                    final_message_id: assistant_id,
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
/// window math is `&[]` (conservative; default 200k) — documented divergence.
#[allow(clippy::too_many_lines)]
pub(crate) async fn call_api_with_ptl_recovery(
    orch: &ConversationOrchestrator,
    system: Option<&str>,
    model: &str,
    profile: Option<&str>,
    history_snapshot: Vec<ConversationMessage>,
    tools: Vec<serde_json::Value>,
    max_tokens_override: Option<u32>,
) -> Result<PtlCallOutcome, OrchestratorError> {
    // (1) Blocking-limit preempt. `is_at_blocking_limit` is
    // `token_usage >= effective_window − MANUAL_COMPACT_BUFFER_TOKENS`
    // (`autoCompact.ts` `calculateTokenWarningState`). `auto_compact_enabled`
    // is `true` to mirror the always-on default of this port (no GrowthBook).
    let estimate = compaction::grouping::estimate_tokens_for_range(&history_snapshot);
    let warning = compaction::calculate_token_warning_state(estimate, model, &[], true);

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
    // SAME `estimate` the banner/auto-compact gate uses; `betas` is empty here
    // to match the banner computation above.
    let context_window = compaction::thresholds::effective_context_window_size(model, &[]);
    let used_fraction = if context_window == 0 {
        0.0
    } else {
        (estimate as f64 / context_window as f64) as f32
    };
    orch.output
        .emit_context_pressure(banner, used_fraction)
        .await;

    if warning.is_at_blocking_limit {
        tracing::warn!(
            estimate,
            model,
            "prompt at blocking limit — preempting before API call"
        );
        // PROACTIVE preempt ⇒ terminal reason `"blocking_limit"` (distinct from
        // the reactive-exhausted `PromptTooLong` returned at the tail).
        return Ok(PtlCallOutcome::BlockingLimit);
    }

    // (2) Initial call. When an Opus-fallback model is configured, route the
    // primary request through the fallback-aware seam. In Task 6, `LlmError`
    // has no `FallbackTriggered` variant — fallback becomes adapter-internal.
    // The `messages_create_with_fallback` seam still passes the fallback hint to
    // `ProviderApiAdapter`, which handles the 529-triggered switch internally.
    // With NO fallback configured the plain `messages_create` seam is taken,
    // byte-identical to before — locked turn-loop fixtures are unaffected.
    let first = if let Some(max_tokens) = max_tokens_override {
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
        Err(other) => return Err(other.into()),
    };

    // (3) PTL retry loop: drop oldest API-round groups and retry, ≤ MAX retries.
    for _attempt in 0..compaction::MAX_PTL_RETRIES {
        // Snapshot the current (possibly already-truncated) history.
        let history = {
            let s = orch.session.lock().await;
            s.history.clone()
        };
        let Some(truncated) =
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
        let snapshot = {
            let s = orch.session.lock().await;
            s.history.clone()
        };
        let messages_before = u32::try_from(snapshot.len()).unwrap_or(u32::MAX);
        let bytes_before: u64 = snapshot.iter().map(protocol::text_byte_size).sum();
        // hooks compaction lifecycle: PreCompact fires before the reactive
        // summary pass. The reactive 413/PTL fallback is part of the automatic
        // recovery pipeline, so the trigger is `auto` (TS treats reactive
        // overflow recovery as a non-manual compact). Best-effort.
        orch.fire_pre_compact("auto").await;
        let compact_result = {
            let mut tracking = orch.compaction_tracking.lock().await;
            compactor
                .process_iteration_tracked(snapshot, 0, &mut tracking)
                .await
        };
        if let Ok(result) = compact_result {
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
                orch.apply_post_compact(
                    result,
                    compaction::CompactTrigger::Auto,
                    messages_before,
                    bytes_before,
                )
                .await;
                // PostCompact fires AFTER the transition is applied.
                orch.fire_post_compact("auto", summary, tokens_freed).await;
                let history = {
                    let s = orch.session.lock().await;
                    s.history.clone()
                };
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
            let is_military_weapons = matches!(category, Some("military_weapons"));
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
                        // Cyber-exemption variant (`cat==="cyber" && pd()`). `A`
                        // is "This model" (no marketing name in this branch).
                        let f = if interactive {
                            "Send feedback with /feedback or learn more: https://support.claude.com/en/articles/15363606"
                        } else {
                            "Learn more: https://support.claude.com/en/articles/15363606"
                        };
                        let exemption =
                            refusal_exemption_url(stop_details.and_then(|sd| sd.explanation.as_deref()));
                        // Binary: `${bT}: ${g}'s safeguards flagged this message
                        // for a cybersecurity topic. … exemption: ${_aa(expl)}\n\n${m}\n\n${h}`
                        // where `g = r!=null ? vp(r) : "This model"` — no marketing
                        // name here, so `g` = "This model". DOUBLE `\n` separators
                        // (od -c verified on 2.1.195 @206804566).
                        format!(
                            "API Error: This model's safeguards flagged this message for a cybersecurity topic. If your work requires this access, you can apply for an exemption: {exemption}\n\n{m}\n\n{f}"
                        )
                    } else if is_military_weapons {
                        // Binary no-label `else if (f === "military_weapons")` arm:
                        //   `${bT}: ${h} has added safeguards for weapons-related
                        //   content, which blocked this request. Not weapons-related?
                        //   This may be a false positive.\n\n${m}${p?"":`\n\nIf you
                        //   believe this was flagged in error, send feedback with
                        //   /feedback.`}`
                        // `h = r!=null ? vp(r) : "This model"` — no marketing name
                        // here ⇒ "This model". The feedback clause is interactive-only
                        // (`p` = non-interactive; the `${p?"":…}` tail fires when `!p`).
                        // DOUBLE `\n` separators (od -c verified on 2.1.195 @206804812).
                        let tail = if interactive {
                            "\n\nIf you believe this was flagged in error, send feedback with /feedback."
                        } else {
                            ""
                        };
                        format!(
                            "API Error: This model has added safeguards for weapons-related content, which blocked this request. Not weapons-related? This may be a false positive.\n\n{m}{tail}"
                        )
                    } else {
                        // Binary final `else`: `${bT}: <brand> is unable to respond
                        // … aup).${a} `+m` — `${a}` is the optional explanation
                        // clause (`a=i?` ${i}${punct}`:""`). Empty ⇒ `). {m}`
                        // (matches the prior port output); present ⇒
                        // `). <explanation>[.] {m}`.
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

/// The binary's `oUi(explanation)`: extract a `https://claude.com/form/\S+`
/// exemption URL from the refusal explanation (stripping trailing `.,;:!?)`),
/// return it when ≤ 400 chars, else the fallback
/// `https://claude.com/form/cyber-use-case`.
fn refusal_exemption_url(explanation: Option<&str>) -> String {
    const FALLBACK: &str = "https://claude.com/form/cyber-use-case";
    const PREFIX: &str = "https://claude.com/form/";
    if let Some(e) = explanation {
        if let Some(start) = e.find(PREFIX) {
            let rest = &e[start..];
            // `\S+`: up to the next whitespace.
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
            // `\S+` after `form/` requires at least one char; cap at dRd = 400.
            if url.len() > PREFIX.len() && url.len() <= 400 {
                return url.to_string();
            }
        }
    }
    FALLBACK.to_string()
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
        (s.model.clone(), orch.config.interactive_permissions)
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
/// Mirrors claude-code, whose top-level `catch` is reached only AFTER the retry
/// layer has handled 429/529; everything else falls through to `model_error`.
#[must_use]
pub(crate) fn is_carveout_propagated(e: &OrchestratorError) -> bool {
    matches!(
        e,
        OrchestratorError::RepeatedOverloaded
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
        // Inject the meta "resume directly" nudge as a fresh user message.
        // The protocol has no `isMeta` flag; the nudge is a plain user text
        // message carrying the byte-exact string (spec: "assert it's a User
        // message with the exact bytes").
        let nudge_msg = ConversationMessage::user(
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
    surface_terminal_api_error(orch, "max_tokens", None).await;
    Ok(TurnStepOutcome::Ended {
        final_message_id: assistant_id,
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
    assistant_id: MessageId,
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
            final_message_id: assistant_id,
            stop_reason: "end_turn".to_string(),
        });
    }
    let nudge_msg =
        ConversationMessage::user(MessageId::new(), MALFORMED_TOOL_USE_RETRY_NUDGE.to_string());
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
    let nudge_msg = ConversationMessage::user(MessageId::new(), THINKING_ONLY_NUDGE.to_string());
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
                Some(ContentBlock::ToolUse {
                    id: ToolUseId::from(id.clone()),
                    name: name.clone(),
                    input: input.clone(),
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

/// HOOK.2 twin of [`dispatch_tool_uses`] that ALSO returns whether any
/// `PreToolUse` hook in this batch requested `continue:false`
/// (preventContinuation). The batched turn loop
/// ([`execute_one_turn_with_recovery_tracked`]) uses the flag to end the turn
/// step (TS `query.ts:1518-1521` `{ reason: 'hook_stopped' }`); the streaming
/// concurrent path keeps the plain [`dispatch_tool_uses`] wrapper.
#[allow(clippy::too_many_lines)]
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
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    &model_text,
                    &serde_json::json!({ "error": format!("tool not found: {name}") }),
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
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    &model_text,
                    &serde_json::json!({ "error": detail }),
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
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    &model_text,
                    &serde_json::json!({ "error": msg }),
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
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    CANCEL_MESSAGE,
                    &serde_json::json!({ "error": CANCEL_MESSAGE }),
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
        // appState.toolPermissionContext.mode` (toolHooks.ts:471). LingXi's session
        // models `plan_mode: bool`, so plan-vs-default is the faithful approximation
        // (the defer path below uses the same logic).
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
        let permission_mode = Some(if plan_mode { "plan" } else { "default" }.to_string());
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
        // validated string would be written to the active terminal (`BEo`); the
        // orchestrator has no TTY handle (the TUI owns the terminal in a separate
        // process and the `OutputStream` has no raw-escape emit), so the
        // terminal-WRITE is a documented residual — the parse / merge / allowlist
        // validation all land here and are observable. No-op when no hook set it.
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
            let body = pre_hook_messages.join("\n");
            Some(ConversationMessage::user(
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
        let pre_prevent_message: Option<ConversationMessage> = if pre_agg.prevent_continuation {
            let reason = pre_agg
                .reason
                .clone()
                .unwrap_or_else(|| "Execution stopped by hook".to_string());
            Some(ConversationMessage::user(
                MessageId::new(),
                format!(
                    "<system-reminder>\nPreToolUse:{name} hook stopped continuation: {reason}\n</system-reminder>"
                ),
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
            let hook_name = format!("PreToolUse:{name}");
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
                let permission_mode = if orch.session.lock().await.plan_mode {
                    "plan"
                } else {
                    "default"
                };
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
                // `hook_deferred_tool` meta message (BIN off 202454844:
                // `{type:"hook_deferred_tool",toolUseID,toolName,toolInput,
                // hookName,hookEvent:"PreToolUse",permissionMode}`). LingXi has no
                // protocol `isMeta`/structured-meta channel, so the deferred-tool
                // record is surfaced as a plain meta user message carrying the
                // faithful fields, ordered after this tool's pre-hook context.
                let meta = serde_json::json!({
                    "type": "hook_deferred_tool",
                    "toolUseID": tool_use_id.to_string(),
                    "toolName": name,
                    "toolInput": input,
                    "hookName": hook_name,
                    "hookEvent": "PreToolUse",
                    "permissionMode": permission_mode,
                });
                injected_messages.push((
                    ConversationMessage::user(MessageId::new(), meta.to_string()),
                    tool_use_id.clone(),
                ));
                // HOOK.1: surface any PreToolUse additionalContext (built above),
                // ordered after the deferred-tool record, matching the Block arm.
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
            let reason = pre_agg
                .reason
                .clone()
                .unwrap_or_else(|| "blocked by hook".into());
            tracing::info!(
                event = orch_events::HOOK_PRE_COMPLETED,
                tool_name = %name,
                decision = "block",
                duration_ms = pre_dur_ms,
            );
            let model_text = format!("Hook blocked: {reason}");
            let result_block = ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: model_text.clone(),
                is_error: true,
                provider_tool_use_id: provider_id.clone(),
                content_blocks: None,
            };
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    &model_text,
                    &serde_json::json!({ "error": model_text.clone() }),
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
        let decision = if let Some(forced) = forced_decision {
            forced
        } else if plan_mode {
            orch.perms.check_in_plan_mode(name, &effective_input).await
        } else if hook_allowed {
            orch.perms
                .check_after_hook_allow(name, &effective_input)
                .await
        } else {
            // NORMAL permission path. Resolve the decision SOURCE first (without
            // delegating to the prompt transport) so the source-gated permission
            // hooks fire the way claude-code does.
            let resolution = orch.perms.resolve_detailed(name, &effective_input).await;
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
            let resolution = if hook_ask && matches!(resolution, PermissionResolution::Allow) {
                PermissionResolution::Ask
            } else {
                resolution
            };
            match resolution {
                PermissionResolution::Allow => PermissionDecision::Allow,
                PermissionResolution::Deny {
                    reason,
                    source,
                    behavior_ask,
                    content_blocks,
                } => {
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
                    PermissionDecision::Deny { reason }
                }
                PermissionResolution::Ask => {
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
                            if let Some(updated) = req_agg.modified_input {
                                effective_input = updated;
                            }
                            orch.perms
                                .check_after_hook_allow(name, &effective_input)
                                .await
                        }
                        Some(HookDecision::Block) => PermissionDecision::Deny {
                            reason: req_agg
                                .reason
                                .unwrap_or_else(|| "permission denied by hook".into()),
                        },
                        _ => {
                            // Delegate to the inner prompt transport, carrying the
                            // REAL tool_use_id (so a stdio `can_use_tool` request is
                            // byte-faithful) and applying the host's `updatedInput`
                            // rewrite to the input the tool actually runs with.
                            let ctx = traits::permission_gate::PermissionCheckContext {
                                tool_use_id: Some(tool_use_id.to_string()),
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
                                    // settings paths; nothing to do here.
                                    permission_updates: _,
                                } => {
                                    if let Some(u) = updated_input {
                                        effective_input = u;
                                    }
                                    PermissionDecision::Allow
                                }
                                traits::permission_gate::PermissionOutcome::Deny { reason } => {
                                    PermissionDecision::Deny { reason }
                                }
                            }
                        }
                    }
                }
            }
        };
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
                orch.output
                    .emit_tool_result(
                        tool_use_id,
                        name,
                        &reason,
                        &serde_json::json!({ "error": reason }),
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
                // `false` there and this is a strict no-op. LingXi has no protocol
                // `isMeta` flag, so the meta message is a plain user text message
                // carrying the exact bytes (cf. the max-output-tokens nudge).
                if deny_hook_says_retry {
                    let retry_msg = ConversationMessage::user(
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
        let progress_consumer = tokio::spawn(async move {
            while let Some(p) = progress_rx.recv().await {
                if let Some(text) = p
                    .data
                    .get("subagent_activity")
                    .and_then(serde_json::Value::as_str)
                {
                    progress_output.emit_subagent_activity(text).await;
                }
            }
        });

        // Time the tool dispatch ONLY (excludes the permission prompt above and
        // the Post hooks below) — surfaced to PostToolUse/Failure hooks as
        // `duration_ms` (claude-code 2.1.195).
        let tool_started = std::time::Instant::now();
        let tool_outcome = tool_handle
            .call(effective_input.clone(), ctx, progress_tx)
            .await;
        // The tool has dropped `progress_tx`; drain the consumer to completion.
        let _ = progress_consumer.await;
        #[allow(clippy::cast_possible_truncation)]
        let tool_duration_ms = tool_started.elapsed().as_millis() as u64;

        let (content, is_error, emit_payload) = match tool_outcome {
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
                // `is_error` rides on the result (set by MCP tools from the
                // server's `isError`; `false` for every native success). A native
                // FAILURE is an `Err` handled below — this Ok arm only flags an
                // MCP logical-error RESULT.
                (text, result.is_error, result.data)
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
                let bare = err.model_facing_message();
                let text = format!("Error: {bare}");
                (text, true, serde_json::json!({ "error": bare }))
            }
        };

        orch.output
            .emit_tool_result(tool_use_id, name, &content, &emit_payload)
            .await;

        // Record the file into the read-file-state cache backing `/files`
        // (TS `readFileState.set(expandPath(file_path), …)` in FileReadTool /
        // FileEditTool / FileWriteTool / MultiEditTool / NotebookEditTool).
        // Only on success — an errored tool never populates the cache.
        if !is_error {
            record_read_file_state(orch, name, &effective_input).await;
        }

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
        let post_started = std::time::Instant::now();
        tracing::info!(
            event = orch_events::HOOK_POST_STARTED,
            tool_name = %name,
        );
        let post_agg = orch.hooks.execute(post_event, hook_ctx.clone()).await;
        // hook duration bounded by tokio timeout — u128 ms cannot exceed u64::MAX
        #[allow(clippy::cast_possible_truncation)]
        let post_dur_ms = post_started.elapsed().as_millis() as u64;

        // #40 terminalSequence apply for the PostToolUse aggregate (claude-code
        // `szn` runs per hook result, all event types). Same as the PreToolUse
        // side: validate + warn-on-reject (observable); the terminal-WRITE is a
        // documented residual.
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
        if post_agg.prevent_continuation {
            let reason = post_agg
                .reason
                .clone()
                .unwrap_or_else(|| "Execution stopped by PostToolUse hook".to_string());
            injected_messages.push((
                ConversationMessage::user(
                    MessageId::new(),
                    format!(
                        "<system-reminder>\nPostToolUse:{name} hook stopped continuation: {reason}\n</system-reminder>"
                    ),
                ),
                tool_use_id.clone(),
            ));
        }

        // HOOK.1 (additionalContext, PostToolUse twin): a PostToolUse hook's
        // `additionalContext` is ALSO a separate `hook_additional_context`
        // attachment in claude-code (`toolHooks.ts:133-143`), injected AFTER the
        // tool_result — exactly what the injected-messages seam does. The
        // hookName prefix is `PostToolUse:{tool}`. `systemMessage` stays folded
        // (handled by `final_content` below); only additionalContext splits out.
        // Strict no-op when no PostToolUse hook returned additionalContext.
        for ctx in &post_agg.additional_contexts {
            let wrapped = format!(
                "<system-reminder>\nPostToolUse:{name} hook additional context: {ctx}\n</system-reminder>"
            );
            injected_messages.push((
                ConversationMessage::user(MessageId::new(), wrapped),
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
                        injected_messages.push((
                            ConversationMessage::user(MessageId::new(), msg),
                            tool_use_id.clone(),
                        ));
                        (content, false)
                    }
                }
            }
            None => (content, false),
        };

        // HOOK.1: the PreToolUse `additionalContext`/`systemMessage` rides the
        // `injected` channel as its OWN message (`pre_context_message`, queued
        // just below this tool's tool_result) — it is NO LONGER folded into the
        // tool-result content (claude-code `toolExecution.ts:845` pushes it as a
        // standalone `resultingMessages` entry). The PostToolUse hooks'
        // model-facing context (`additional_contexts`) IS folded onto the
        // tool-result content here — a separate concern (TS surfaces PostToolUse
        // `additionalContext` to the model), each on its own line. We use
        // `additional_contexts` ONLY, never `system_messages`: a PostToolUse
        // `systemMessage` is transcript/user-facing only and must NOT reach the
        // model (claude-code `hook_system_message` → `normalizeAttachmentForAPI`
        // returns `[]`, `messages.ts:4258`). A strict no-op when empty, so the
        // result text is byte-identical to before for the locked turn-loop
        // fixtures (noop hooks).
        let mutated = !post_agg.additional_contexts.is_empty() || mcp_output_mutated;
        let final_content = if mutated {
            let mut out = content;
            for msg in &post_agg.additional_contexts {
                out.push('\n');
                out.push_str(msg);
            }
            out
        } else {
            content
        };

        tracing::info!(
            event = orch_events::HOOK_POST_COMPLETED,
            tool_name = %name,
            duration_ms = post_dur_ms,
            mutated_response = mutated,
        );

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
        let content_blocks = if mutated || !tool_handle.is_mcp() {
            None
        } else {
            emit_payload.as_array().cloned()
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
        if let Some(msg) = pre_prevent_message {
            injected_messages.push((msg, tool_use_id.clone()));
        }
    }

    // #39 PostToolBatch: fire ONCE after the whole batch resolved (claude-code
    // `G4t`, BIN off 205710327: fired after every tool call in a batch resolves,
    // before the next model request — distinct from per-tool `PostToolUse`).
    // Strict no-op when no tool ran (empty batch). Best-effort: a
    // failing/absent PostToolBatch hook never breaks the turn (the executor is a
    // no-op when no PostToolBatch hook is registered, mirroring the per-tool
    // PostToolUse fire). The aggregate decision/output are not consumed — this is
    // an observational, post-batch event.
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
        let _batch_agg = orch.hooks.execute(batch_event, batch_ctx).await;
    }

    Ok((
        results,
        prevent_continuation,
        injected_messages,
        context_modifiers,
    ))
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
