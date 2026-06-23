//! Inner turn-by-turn loop helpers. Private to `ConversationOrchestrator`.

use crate::conversation::ConversationOrchestrator;
use crate::error::OrchestratorError;
use crate::test_support::{PermissionDecision, PermissionDecisionSource, PermissionResolution};
use llm_client::{ContentBlock as LlmContentBlock, LlmError, LlmResponse};
use hooks::events::HookEvent;
use hooks::registry::HookContext;
use hooks::response::HookDecision;
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
const READ_FILE_STATE_TOOLS: &[&str] =
    &["Read", "Edit", "Write", "MultiEdit", "NotebookEdit"];

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

/// Lexically expand a tool's `file_path` argument to an absolute, normalized
/// path — the cache key for [`ConversationOrchestrator::files_in_context`].
///
/// 1:1 with the observable behavior of TS `expandPath(path, baseDir)`
/// (`src/utils/path.ts`): trim; bare `~` / `~/…` expand against the home
/// directory; absolute paths are kept; relative paths resolve against `cwd`;
/// the result is then collapsed lexically (`.` dropped, `..` popped).
///
/// FORCED divergence from `expandPath` (documented, not a parity gap):
/// - Windows POSIX-path conversion (`/c/Users/…`) is skipped — the port's
///   parity target is the macOS/Linux path shape, and the cache key only
///   feeds `relative(cwd, key)` rendering which is already platform-native.
/// - Unicode NFC normalization is a no-op for the ASCII paths exercised
///   here and `OsStr` carries no portable NFC primitive, so it is omitted.
/// - This is LEXICAL only (mirrors `expandPath`, NOT `realpath`): it never
///   touches the disk, so symlinks are preserved and a non-existent path
///   still resolves to the joined string — matching the TS keys and the
///   `relative()` output `/files` renders.
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

/// Record a successful `Read`/`Edit`/`Write`/… into the read-file-state cache.
///
/// Best-effort and order-preserving: pulls `file_path` (or `notebook_path`
/// for `NotebookEdit`) from the post-hook effective input, absolutizes it
/// against `orch.cwd`, and inserts it into `orch.read_file_state` keeping the
/// FIRST insertion (a re-read is a no-op; the 2-file case `a,b,a → [a, b]`
/// matches TS, but TS's MRU-promoting LRU diverges at ≥3 files — read a,b,c,a
/// → TS `[a, c, b]` vs this `[a, b, c]`). Never fails the tool: an absent /
/// non-string path or an unknown tool is silently skipped.
///
/// Two TS write-sites are intentionally NOT mirrored: TS `NotebookEditTool`
/// resolves `notebook_path` WITHOUT `expandPath` (no `~`/trim), whereas this
/// routes it through the shared [`absolutize`] (harmless unless a notebook path
/// literally starts with `~` or has surrounding whitespace); and `BashTool`'s
/// `readFileState.set` for files a bash command writes is out of scope (the
/// bash file-write interception is itself unported).
///
/// This function only populates the ordered `Vec` backing `/files`. The
/// RICHER `{content, mtime_ms, offset, limit}` registry
/// ([`ConversationOrchestrator::read_state_map`], the 1:1 port of TS
/// `readFileState`) is populated by the *tools themselves* — each file tool's
/// `call` does `ctx.read_file_state.set(…)` on its construction-time
/// [`tool_api::BuiltinToolContext`] (matching TS, where every file tool calls
/// `readFileState.set`). The composition root (`engine-desktop` / `mobile`)
/// shares the SAME `Arc` between `orch.read_state_map` and the
/// `BuiltinToolContext` it hands the file tools, so a tool's write is visible
/// to the orchestrator. The orchestrator never constructs the file tools (they
/// arrive pre-built in `orch.tools`), so there is no `BuiltinToolContext`
/// construction in this crate to thread the `Arc` through.
/// #40 Apply a hook's folded `terminalSequence` (claude-code `szn`, BIN off
/// 205755390). Runs the allowlist validator
/// ([`hooks::terminal_seq::validate_terminal_sequence`], the `NEo` port):
/// - REJECT → warn with claude-code's byte-faithful message (the observable
///   half).
/// - ACCEPT → write the validated string to the active terminal (`BEo`). The
///   orchestrator process holds no TTY handle (the TUI owns the terminal in a
///   separate process), so it forwards the validated bytes through the
///   [`OutputStream::emit_terminal_sequence`] seam (#6 main-loop parity); the
///   interactive host (TUI) writes them to its stdout. Non-interactive hosts
///   (print mode, CLI sink, tests) keep the default no-op, so the terminal
///   write only happens where a controlling terminal exists.
///
/// Strict no-op when `seq` is `None` (no hook returned a `terminalSequence`).
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

// ============================================================================
// #6 (main-loop parity): a hook's allowlisted `terminalSequence` is FORWARDED
// to the host's terminal-write seam (`OutputStream::emit_terminal_sequence`),
// not merely debug-logged. A rejected sequence is dropped (warned only).
// ============================================================================
#[cfg(test)]
mod terminal_sequence_tests {
    use super::apply_terminal_sequence;
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    fn orch_with_output(output: Arc<MockOutputStream>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output,
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// An ACCEPTED OSC sequence (here OSC 9 desktop-notification) is forwarded
    /// to the terminal-write seam verbatim (BEL-normalized).
    #[tokio::test]
    async fn accepted_sequence_is_forwarded_to_terminal_seam() {
        let output = Arc::new(MockOutputStream::new());
        let orch = orch_with_output(output.clone());
        apply_terminal_sequence(&orch, "Notification", Some("\u{001B}]9;hello\u{0007}")).await;
        assert_eq!(
            output.terminal_sequences().await,
            vec!["\u{001B}]9;hello\u{0007}".to_string()],
            "an allowlisted terminalSequence must reach emit_terminal_sequence"
        );
    }

    /// A REJECTED sequence (OSC 8 hyperlink is not in the allowlist) is dropped:
    /// nothing reaches the terminal-write seam.
    #[tokio::test]
    async fn rejected_sequence_is_not_forwarded() {
        let output = Arc::new(MockOutputStream::new());
        let orch = orch_with_output(output.clone());
        apply_terminal_sequence(&orch, "Notification", Some("\u{001B}]8;;http://x\u{0007}")).await;
        assert!(
            output.terminal_sequences().await.is_empty(),
            "a rejected terminalSequence must NOT be forwarded"
        );
    }

    /// `None` (no hook returned a sequence) is a strict no-op.
    #[tokio::test]
    async fn none_is_a_noop() {
        let output = Arc::new(MockOutputStream::new());
        let orch = orch_with_output(output.clone());
        apply_terminal_sequence(&orch, "Notification", None).await;
        assert!(output.terminal_sequences().await.is_empty());
    }
}

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
    "The PermissionDenied hook indicated this command is now approved. You may retry it if you would like.";

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
    Ok(execute_one_turn_with_recovery_tracked(orch, system, recovery)
        .await?
        .0)
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

    // §F: per-turn, transient `conditional_rules` reminder — path-gated CLAUDE.md
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
    // ONLY when the `CLAUDE_CODE_AGENT_LIST_IN_MESSAGES` gate is ON (default OFF
    // ⇒ `None`, keeping the locked turn-loop fixtures byte-identical and the
    // inline `AgentTool` catalog in place). Appended to THIS call's OUTGOING
    // snapshot only (never `session.history` / JSONL), after the conditional-
    // rules reminder. See [`ConversationOrchestrator::agent_listing_reminder_message`].
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
    let response = match call_api_with_ptl_recovery(
        orch,
        system,
        &model,
        model_profile.as_deref(),
        history_snapshot,
        tools,
        max_tokens_override,
    )
    .await?
    {
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
        PtlCallOutcome::RapidRefillBreaker => {
            // #54 reactive trip: surface the thrashing message + end the turn
            // with `invalid_request` (the binary's `reason:"rapid_refill_breaker"`).
            let assistant_id = surface_rapid_refill_thrashing(orch).await;
            return Ok((
                TurnStepOutcome::Ended {
                    final_message_id: assistant_id,
                    stop_reason: "invalid_request".to_string(),
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
        let model_ref = crate::cost_wiring::model_ref_from_string(&model);
        let _cost_for_this_call = tracker
            .record_api_response_v2(
                model_ref,
                usage,
                api_call_started.elapsed(),
                orch.api.last_retry_count(),
                cache_read,
                cache_create,
                false, // is_batch_request — M6 always false
                orch.analytics_bus.as_ref(), // M7: fire tengu_cost_recorded on the live path
            )
            .await;
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
    orch.persist_message_to_jsonl(&assistant_msg).await;

    // 4. Emit each Text block to the output stream (whole-body in M5-02;
    //    M5-04 will switch to per-delta).
    for blk in &assistant_blocks {
        if let ContentBlock::Text { text } = blk {
            orch.output.emit_text(text).await;
        }
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
                s.injected_message_sources.insert(m.id(), tool_use_id.clone());
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
    let invoked_tool_names: Vec<String> =
        tool_uses.iter().map(|(_, name, _, _)| name.clone()).collect();
    orch.note_todo_reminder_tool_call(&invoked_tool_names).await;

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
                if recovery
                    .as_deref()
                    .is_some_and(|s| !s.thinking_only_nudged)
                    && !has_visible_text(&assistant_blocks) =>
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
    /// The blocking-limit preempt fired, or the PTL retry budget +
    /// reactive-compact fallback were all exhausted. End the turn.
    PromptTooLong,
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
/// NOT A PARITY GAP (verified 2026-06-23, codex finding #4 REFUTED): this tail
/// is the "PTL-truncate ×N → one full compact → error" sequence, and that
/// matches claude-code's DEFAULT behavior. claude-code's fuller multi-stage
/// recovery (`contextCollapse.recoverFromOverflow` / `reactiveCompact.
/// tryReactiveCompact`, the "marble-origami" subsystem) lives behind
/// build/runtime feature gates that are OFF by default:
///   * `feature('CONTEXT_COLLAPSE')` — the flag isn't even present in the
///     `FEATURE_FLAGS` map (`shims/bun-bundle.ts`), so `feature()` returns
///     `false` UNCONDITIONALLY; the `contextCollapse` module is never required.
///   * `feature('REACTIVE_COMPACT')` — `envBool('CLAUDE_CODE_REACTIVE_COMPACT',
///     false)`, i.e. default-off, opt-in only.
/// Cross-checked against the v2.1.186 binary: the service symbols
/// `applyCollapsesIfNeeded` / `recoverFromOverflow` / `tryReactiveCompact` /
/// `collapse_drain_retry` / `isContextCollapseEnabled` are ALL 0-hit — the
/// algorithm is dead-code-eliminated from the shipping binary (only the dormant
/// `marble-origami-*` session-storage recorders remain, never called when the
/// feature is off). So the model's DEFAULT overflow recovery is exactly the
/// simpler path this tail implements; porting the gated subsystem would make
/// LingXi DIVERGE from default claude-code behavior. Details + the refutation
/// evidence: project memory `mainloop-parity-2026-06-23`.
///
/// `betas` for the blocking-limit window math is `&[]` (conservative): the
/// orchestrator does not currently thread the per-request beta set down to this
/// call site, and the default window is the parity 200k. Documented divergence,
/// not a frozen-surface change.
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
    orch.output.emit_context_pressure(banner).await;

    if warning.is_at_blocking_limit {
        tracing::warn!(
            estimate,
            model,
            "prompt at blocking limit — preempting before API call"
        );
        return Ok(PtlCallOutcome::PromptTooLong);
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
            .messages_create_with_opts(model, profile, system, history_snapshot, tools.clone(), max_tokens)
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
        let Some(truncated) = compaction::ptl_retry::truncate_head_for_ptl_retry(history, token_gap)
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
/// `catch (FallbackTriggeredError)` arm).
///
/// Invoked when the primary batched call should fall back to a secondary model —
/// only possible when `config.fallback_model.is_some()` (see
/// [`call_api_with_ptl_recovery`]). Ports the TS arm MINIMALLY and faithfully:
///
/// 1. **(i) switch the working/session model** to `fallback_model` (TS
///    `currentModel = fallbackModel`). The next turn step re-snapshots
///    `session.model`, so the whole conversation continues on the fallback.
/// 2. **(ii) clear the in-flight assistant + `tool_use`/`tool_result`
///    accumulators** for the current step — a STRUCTURAL no-op in this port: the
///    assistant reply and its `tool_result`s are appended to history only AFTER a
///    successful
///    response (see [`execute_one_turn_with_recovery_tracked`]), so at the
///    `FallbackTriggered` point nothing has been appended for this step. TS
///    mutates JS-side arrays (`assistantMessages.length = 0`, etc.) that have no
///    standing analog here — documented, not a parity gap.
/// 3. **(iii) surface a user-visible `warning`** conveying the switch (TS
///    `createSystemMessage('Switched to … due to high demand for …', 'warning')`).
///    We emit it on the output stream — the turn loop's user-visible notice
///    mechanism (same channel [`surface_prompt_too_long`] uses) — rather than
///    pushing a `ConversationMessage::System` into history: TS's
///    `createSystemMessage` is a UI/progress message filtered out of the model
///    request, and a `role:"system"` entry in the `messages` array is rejected by
///    the Anthropic API, so keeping it out of model-bound history is both
///    faithful and correct for the re-issue + subsequent turns.
/// 4. **(iv) emit the `tengu_model_fallback_triggered` analytic** via the loop's
///    `tracing` telemetry path, with an INLINE event-name string (NOT a locked
///    telemetry const) so the event-name fixture lock is not perturbed. This is
///    the success-path orchestrator event, distinct from the api-client
///    request-failed `error_kind = "fallback_triggered"` label.
/// 5. **(v) re-issue ONE round-trip** via `messages_create_with_fallback` against
///    the fallback model with `fallback_model = None`: the fallback model is
///    non-Opus, so the consecutive-529 gate is closed → this cannot recurse into
///    another `FallbackTriggered` (TS `continue` re-enters the loop once).
///
/// DOCUMENTED bounded divergences: TS also sets
/// `toolUseContext.options.mainLoopModel = fallbackModel`, but this port derives
/// the tool context's `main_loop_model` from `config.model` (immutable `&self`),
/// so only `session.model` (which drives the API model) switches. TS's ant-only
/// `stripSignatureBlocks` thinking-signature scrub is omitted — it is
/// `USER_TYPE === 'ant'`-gated and this port carries no protected-thinking replay.
///
/// NOTE: Task 5 dead code — the `FallbackTriggered` interception was removed from
/// the turn loop; this function is called by `messages_create_with_fallback` in
/// Task 6 once the adapter wires the fallback logic. Kept to preserve the
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
    let warning =
        format!("Switched to {fallback_model} due to high demand for {original_model}");
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
pub(crate) async fn surface_rapid_refill_thrashing(
    orch: &ConversationOrchestrator,
) -> MessageId {
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
pub(crate) fn terminal_api_error_text(
    model: &str,
    interactive: bool,
    stop_reason: &str,
    request_id: Option<&str>,
    stop_details: Option<&llm_client::StopDetails>,
) -> Option<String> {
    match stop_reason {
        "max_tokens" => Some(format!(
            "API Error: Claude's response exceeded the {} output token maximum. To configure this behavior, set the CLAUDE_CODE_MAX_OUTPUT_TOKENS environment variable.",
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
                    // `A`: the cyber/bio variant (`rnt`) vs the generic one.
                    let a = if cyber_or_bio {
                        format!(
                            "{label} has safety measures that flag messages on most cybersecurity or biology topics (https://www.anthropic.com/legal/aup). They may flag safe, normal content as well. These measures let us bring you Mythos-level capability in other areas sooner, and we're working to refine them."
                        )
                    } else {
                        format!(
                            "{label} has safety measures that flagged something in this session (https://www.anthropic.com/legal/aup). This sometimes happens with safe, normal conversations."
                        )
                    };
                    format!("API Error: {a} Claude Code can't respond to this request with {label}.\n\n{m}\n\n{f}")
                }
                None => {
                    // NO-LABEL branch.
                    let m = if interactive {
                        "Please double press esc to edit your last message or start a new session for Claude Code to assist with a different task."
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
                        format!(
                            "API Error: This model has safety measures that flagged this message for a cybersecurity topic. If your work requires this access, you can apply for an exemption: {exemption}\n\n{m}\n\n{f}"
                        )
                    } else {
                        format!(
                            "API Error: Claude Code is unable to respond to this request, which appears to violate our Usage Policy (https://www.anthropic.com/legal/aup). {m}"
                        )
                    }
                }
            };
            // `\n\nRequest ID: ${n}` suffix (REFUSAL-ONLY), binary @197279553.
            let suffix = match request_id {
                Some(id) if !id.is_empty() => format!("\n\nRequest ID: {id}"),
                _ => String::new(),
            };
            Some(format!("{base}{suffix}"))
        }
        _ => None,
    }
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
    // message's `\n\nRequest ID: …` suffix (recorded by the adapter from the
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
    orch.persist_message_to_jsonl(&assistant_msg).await;
    orch.output.emit_text(&text).await;
    Some(assistant_id)
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
        // Second failure → terminal NON-meta message, complete the turn.
        orch.output.emit_text(MALFORMED_TOOL_USE_RETRY_FAILED).await;
        let failed_msg = ConversationMessage::user(
            MessageId::new(),
            MALFORMED_TOOL_USE_RETRY_FAILED.to_string(),
        );
        {
            let mut s = orch.session.lock().await;
            s.history.push(failed_msg.clone());
        }
        orch.persist_message_to_jsonl(&failed_msg).await;
        return Ok(TurnStepOutcome::Ended {
            final_message_id: assistant_id,
            stop_reason: "end_turn".to_string(),
        });
    }
    let nudge_msg = ConversationMessage::user(
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
        ConversationMessage::user(MessageId::new(), THINKING_ONLY_NUDGE.to_string());
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
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
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
            let result_block = ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: format!("<tool_use_error>InputValidationError: {detail}</tool_use_error>"),
                is_error: true,
                provider_tool_use_id: provider_id.clone(),
                content_blocks: None,
            };
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    &serde_json::json!({ "error": detail }),
                )
                .await;
            results.push(result_block);
            continue;
        }

        // Synthesize a minimal ToolUseContext — needed by the validate_input
        // gate below and reused by the eventual `tool_handle.call()`.
        let messages = {
            let s = orch.session.lock().await;
            s.history.clone()
        };
        let ctx = ToolUseContext {
            options: ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: orch.config.model.clone(),
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
        };

        // validate_input gate (claude-code `toolExecution.ts:683-723`): a
        // `validateInput` failure wraps the message in `<tool_use_error>` and
        // short-circuits. Runs on the RAW `input` (pre-hook), BEFORE the
        // PreToolUse hooks/permission (claude-code order), so there is no
        // pre-hook context to fold.
        if let Err(tool_api::ValidationError(msg)) =
            tool_handle.validate_input(input, &ctx).await
        {
            let result_block = ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: format!("<tool_use_error>{msg}</tool_use_error>"),
                is_error: true,
                provider_tool_use_id: provider_id.clone(),
                content_blocks: None,
            };
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    &serde_json::json!({ "error": msg }),
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
            cwd: orch.cwd.clone(),
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
        // hook may return a top-level `terminalSequence` for Claude Code to emit
        // (OSC 9 / 777 desktop notification, etc.). Run the allowlist validator
        // (`NEo`) over the folded sequence: on REJECT, warn (the observable half,
        // byte-faithful to claude-code's reject message). On ACCEPT the
        // validated string would be written to the active terminal (`BEo`); the
        // orchestrator has no TTY handle (the TUI owns the terminal in a separate
        // process and the `OutputStream` has no raw-escape emit), so the
        // terminal-WRITE is a documented residual — the parse / merge / allowlist
        // validation all land here and are observable. No-op when no hook set it.
        apply_terminal_sequence(orch, &name, pre_agg.terminal_sequence.as_deref()).await;
        // HOOK.1: a PreToolUse hook's `hookSpecificOutput.additionalContext`
        // ONLY (the executor merge folds `additionalContext` into
        // `additional_contexts`, distinct from `system_messages`). claude-code
        // pushes this context as its OWN message into `resultingMessages`,
        // INDEPENDENT of the tool_result (`toolExecution.ts:845` — `case
        // 'additionalContext': resultingMessages.push(result.message)`). We
        // surface it the same way: a SEPARATE meta user message that rides the
        // existing per-tool `injected` channel (the SKILLEXEC.3 `new_messages`
        // mechanism), so both drivers append it AFTER this tool's tool_result —
        // never concatenated into the tool_result content. The shape mirrors TS
        // `messages.ts:4117-4128` (`hook_additional_context` attachment): a
        // `<system-reminder>`-wrapped meta user message,
        // `"PreToolUse:{tool} hook additional context: {content}"`, with the
        // collected `additional_contexts` joined by `\n`.
        //
        // CRITICAL (messages.ts:4117 vs :4258 parity): `systemMessage` is
        // DELIBERATELY excluded — claude-code routes it to a `hook_system_message`
        // attachment whose `normalizeAttachmentForAPI` returns `[]`, so it never
        // reaches the model (it is transcript/user-facing only). We therefore
        // build this message from `additional_contexts` ONLY, never from
        // `system_messages`. LingXi has no separate user-display sink for a hook's
        // `systemMessage`, so it simply does NOT reach the model — the faithful
        // API behavior.
        //
        // claude-code pushes this context in the PRE-hook phase
        // (`toolExecution.ts:846`), BEFORE the permission/block check, so it
        // surfaces even when the tool is later BLOCKED or DENIED. We build it once
        // below and emit it (as its own message, never folded into the error
        // result) on the success, block, AND deny arms — in each case ordered
        // AFTER that arm's tool_result.
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

        // #37 `permissionDecision: "defer"` (claude-code BIN off 202454844). A
        // `PreToolUse` hook may DEFER a solo tool call so it is re-attempted on a
        // later interactive resume rather than run now. claude-code gates this:
        //   1. ONLY in non-interactive (print) mode — in interactive mode it
        //      warns `... in interactive mode; ignoring (defer is print-mode
        //      only)` and proceeds normally.
        //   2. ONLY when the batch holds a SINGLE tool_use block — with >1 it
        //      warns `... but {n} tool calls are in this batch; ignoring (defer
        //      is solo-only — siblings would be orphaned on resume)`.
        // On the gated path it emits the `tengu_pre_tool_hook_deferred` analytic,
        // pushes a `hook_deferred_tool` meta message, and TERMINATES the turn
        // with the `tool_deferred` stop-reason — the tool is NOT executed.
        //
        // `is_non_interactive_session` = `!interactive_permissions` (the
        // orchestrator's print/headless signal: `interactive_permissions` is
        // `true` only when an interactive prompt transport is wired). DORMANT on
        // the default interactive REPL — the interactive-mode gate ignores defer
        // there, so the tool falls through to the normal permission gate.
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
            let result_block = ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: format!("Hook blocked: {reason}"),
                is_error: true,
                provider_tool_use_id: provider_id.clone(),
                content_blocks: None,
            };
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    &serde_json::json!({ "error": format!("Hook blocked: {reason}") }),
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
        // HOOK.4 — Plan-mode dynamic gate (parity with claude-code's live
        // `toolPermissionContext.mode = 'plan'`). When the session is in plan mode
        // (the model ran `EnterPlanMode` and has not yet exited), authorize under
        // `PermissionMode::Plan` so the mutation backstop activates IMMEDIATELY —
        // LingXi's policy mode is otherwise fixed at boot and the gate would miss a
        // runtime `EnterPlanMode` (see `PermissionGate::check_in_plan_mode`). Plan
        // mode binds OVER a hook 'allow': a `PreToolUse` hook must not silently push
        // a mutation through while the user is planning — the same principle as a
        // deny rule binding over a hook 'allow' (HOOK.3, issue 1). The session lock
        // is read-and-dropped on this line, so the gate `await` never holds it.
        // Deny-arm carry-overs from the SOURCED resolution (`toolExecution.ts:1040`).
        // `PermissionDecision::Deny` (the 2-valued type the deny arm matches) carries
        // only `reason`, so the richer `ask`-rejection shape rides these outer locals:
        //   - `reject_content_blocks`: `permissionDecision.contentBlocks`, appended at
        //     the TOP LEVEL of the deny user message ONLY when behavior is `ask`.
        //   - `deny_hook_says_retry`: set on the gated classifier-deny path when a
        //     `PermissionDenied` hook returned `{retry: true}` (`toolExecution.ts:1090`).
        // Both stay empty/false on every normal deny path, so the common deny message
        // is byte-identical to before.
        let mut reject_content_blocks: Vec<ContentBlock> = Vec::new();
        let mut deny_hook_says_retry = false;
        let plan_mode = orch.session.lock().await.plan_mode;
        let decision = if plan_mode {
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
            // R-D3: a PreToolUse hook `permissionDecision:"ask"` (HookDecision::Ask)
            // forces the interactive prompt even over a configured ALLOW rule
            // (claude-code `permissionBehavior="ask"`, `azn` off ~205721920). The
            // behavior precedence is deny > ask > allow, so a deny rule still binds
            // (resolved as Deny below) and plan mode already bound above; only an
            // Allow is upgraded to Ask so the prompt fires instead of silently
            // auto-allowing. No-op unless a hook returned `ask`.
            let resolution = if hook_ask
                && matches!(resolution, PermissionResolution::Allow)
            {
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
                    // rule/mode/plan deny. The auto-mode classifier is unwired in
                    // the public build, so this is dormant there — matching
                    // claude-code's public build (the `TRANSCRIPT_CLASSIFIER`
                    // feature gate is off).
                    if matches!(source, PermissionDecisionSource::Classifier) {
                        let denied_event = HookEvent::PermissionDenied {
                            tool_name: name.clone(),
                            tool_input: effective_input.clone(),
                            tool_use_id: tool_use_id.clone(),
                            reason: reason.clone(),
                        };
                        let denied_agg =
                            orch.hooks.execute(denied_event, hook_ctx.clone()).await;
                        // `{retry: true}` reply (`toolExecution.ts:1080-1091`): a
                        // PermissionDenied hook can signal the auto-mode classifier
                        // deny is now approved. We honour it ONLY behind the same gate
                        // claude-code uses — `feature('TRANSCRIPT_CLASSIFIER')` (the
                        // external build's `is_classifier_permissions_enabled()` const,
                        // hardcoded `false`) AND the runtime config bit that lets a test
                        // force the flag on. With BOTH off (the parity default) the
                        // retry message NEVER fires on the normal deny path.
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
                        _ => orch.perms.check(name, &effective_input).await,
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

        // One-shot progress channel — receiver dropped immediately.
        let (progress_tx, _progress_rx) =
            tokio::sync::mpsc::channel::<tool_api::progress::ToolProgress>(8);

        let tool_outcome = tool_handle
            .call(effective_input.clone(), ctx, progress_tx)
            .await;

        let (content, is_error, emit_payload) = match tool_outcome {
            Ok(result) => {
                let text = tool_result_to_model_text(&result.data);
                // SKILLEXEC.3 (Part A): stash any tool-injected conversation
                // messages so the caller can append them after this batch's
                // tool_result user message. Non-empty only for the Skill tool
                // (the expanded skill prompt); empty for every other tool, so
                // the locked turn-loop fixtures stay byte-identical. Each is
                // paired with THIS tool's `tool_use_id` (TS
                // `tagMessagesWithToolUseID` stamps the Skill tool's own block
                // id as `sourceToolUseID`) for the caller's in-memory
                // `injected_message_sources` side-table.
                injected_messages
                    .extend(result.new_messages.into_iter().map(|m| (m, tool_use_id.clone())));
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
                (text, false, result.data)
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
            .emit_tool_result(tool_use_id, name, &emit_payload)
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
            }
        } else {
            HookEvent::PostToolUse {
                tool_name: name.clone(),
                tool_input: effective_input.clone(),
                tool_output: emit_payload.clone(),
                tool_use_id: tool_use_id.clone(),
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
        // (legacy, MCP-only): a PostToolUse hook may REPLACE the tool's output.
        // claude-code (BIN off 202157140) yields `updatedToolOutput` for ALL
        // tools first, then yields `updatedMCPToolOutput` (mapped onto
        // `updatedToolOutput`) ONLY when `isMcpTool(tool)` — yielded SECOND so
        // for an MCP tool the MCP field overrides the all-tools field. The apply
        // (BIN off 202169384) then substitutes the value ONLY when the tool's
        // `outputSchema` either is absent OR validates the value successfully
        // (`e.outputSchema?.safeParse(D.updatedToolOutput)?.success!==!1`); on a
        // schema MISMATCH it keeps the ORIGINAL output and emits a
        // `hook_error_during_execution` meta message (BIN off 202465455). Only a
        // SUCCESSFUL result is mutated — the `PostToolUseFailure` arm carries no
        // such field in the TS schema, and these aggregate fields are only ever
        // set by a `PostToolUse` (success) dispatch. When applied, the
        // replacement JSON re-derives the model-facing text via
        // `tool_result_to_model_text`, exactly as the original did. A strict
        // no-op when no hook set either field (the common case) → byte-identical.
        //
        // Precedence (mirrors the yield order): start from the all-tools
        // `updated_tool_output` (the outer `Some` means a hook set the key, even
        // to `null` — `!== void 0` semantics), then for an MCP tool override with
        // the legacy `updated_mcp_tool_output` when present.
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
                    Some(schema) => crate::schema_validation::validate_tool_output_schema(
                        schema,
                        &new_output,
                    ),
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

        // SubagentStart + SubagentStop hooks (parity with claude-code's
        // `executeSubagentStartHooks` at `runAgent.ts:532` and
        // `executeStopHooks(…, subagentId, …)` → `hook_event_name:
        // 'SubagentStop'`, `utils/hooks.ts:3653-3678`). claude fires BOTH inside
        // `runAgent` keyed on ONE canonical `agentId` (runAgent.ts:347). The
        // LingXi port spawns subagents only through the registered,
        // turn_loop-dispatched `Agent` (legacy alias `Task`) tool: a completed
        // `spawner.spawn()` MEANS the subagent's loop ran and stopped. So we fire
        // both here at the spawn-completion site — same observable timing, the
        // fires just live in this orchestrator-side dispatch chokepoint
        // (alongside `PostToolUse`/`WorktreeCreate`) where `orch.hooks` is
        // reachable. SubagentStart precedes SubagentStop, matching claude's
        // start-then-stop ordering.
        //
        // #8 (real id): the Agent tool now surfaces the child's REAL pool
        // `AgentId` on its result `data.agentId` (C1 seam: `SubagentResult`
        // carries it back; `agent.rs` emits `agentId = agent_id.to_string()`).
        // We parse it back here so BOTH events fire with the SAME single
        // canonical id (claude runAgent.ts:347), replacing the two fresh
        // divergent `AgentId::new()`s the chokepoint used to mint. The failure
        // path is the single residual: a FAILED spawn returns `ToolError` (no
        // `data`), so the id cannot be recovered — we fall back to a fresh
        // `AgentId::new()` there (documented divergence; mitigated only if the
        // fires move fully into the runner where `ctx.agent_id` is always live).
        //
        // Fires on BOTH a successful AND a failed/killed dispatch: the subagent
        // always STARTED and STOPPED (claude-code's chokepoints run at the
        // loop's natural boundaries regardless of outcome). They do NOT fire on
        // a pre-hook Block or a permission denial — those `continue` above
        // before any spawn, so no subagent ever ran. Best-effort:
        // `orch.hooks.execute` is a strict no-op when no hook is registered, and
        // a failing hook never breaks the turn.
        //
        // SINGLE-FIRE (R7): the real `Agent`/`Task` tool drives a child runner
        // that ALREADY fires the canonical lifecycle hooks claude fires inside
        // `runAgent` — `SubagentStart` (session+plugin+frontmatter, general
        // execute, collecting `additionalContexts` for the child's initial
        // messages, runAgent.ts:530-555) and the child's OWN frontmatter
        // `Stop`→`SubagentStop` (agent-scoped, runAgent.ts finally). It marks
        // that on its success result `data.subagentHooksFired = true`. We read
        // that flag here to AVOID the historical double-fire:
        //   • SubagentStart — when the runner fired it, the chokepoint SKIPS its
        //     own fire entirely (one canonical fire). The runner is the only site
        //     that can inject the hooks' `additionalContexts` into the child, so
        //     it MUST be the SubagentStart owner. When the flag is absent
        //     (FakeAgentTool fixtures = no runner, or a FAILED spawn whose
        //     `ToolError` carries no `data`), the chokepoint fires it as before.
        //   • SubagentStop — the runner covers only the child's OWN frontmatter
        //     SubagentStop, so the chokepoint still fires the COMPLEMENT (session
        //     / plugin) via `execute_excluding_agent(child_id)`, which omits the
        //     child's frontmatter bucket so those are not re-fired. This is
        //     race-free vs. the runner's `clear_agent_hooks` regardless of
        //     ordering. For FakeAgentTool fixtures (no frontmatter bucket for the
        //     fake child id) this is byte-identical to the general `execute`.
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
            let sa_ctx = HookContext {
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
            };
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

        // MCP results carry a content-block array (`model_content_blocks`) so the
        // egress can send it VERBATIM as `tool_result.content` (claude-code passes
        // the MCP content array directly — images/resources stay structured). A
        // hook-mutated result (output replaced or additionalContext appended)
        // drops to the text-only `final_content`.
        let content_blocks = if mutated {
            None
        } else {
            emit_payload
                .get("model_content_blocks")
                .and_then(serde_json::Value::as_array)
                .cloned()
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
            cwd: orch.cwd.clone(),
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
        .map_or_else(
            || serde_json::to_string(data).unwrap_or_else(|_| "<unserializable>".into()),
            std::string::ToString::to_string,
        )
}

#[cfg(test)]
mod terminal_api_error_tests {
    use super::{refusal_exemption_url, terminal_api_error_text};
    use llm_client::StopDetails;

    fn details(category: &str, explanation: Option<&str>) -> StopDetails {
        StopDetails {
            category: Some(category.to_string()),
            explanation: explanation.map(str::to_string),
        }
    }

    /// The refusal message appends `\n\nRequest ID: {id}` when a request id is
    /// present (binary @197279553 `u = n ? `\n\nRequest ID: ${n}` : ""`), and
    /// omits it otherwise. The suffix is REFUSAL-ONLY.
    #[test]
    fn refusal_appends_request_id_suffix() {
        let with =
            terminal_api_error_text("claude-opus-4-8", true, "refusal", Some("req_011abc"), None)
                .expect("refusal text");
        assert!(with.ends_with("\n\nRequest ID: req_011abc"), "got: {with}");

        let without = terminal_api_error_text("claude-opus-4-8", true, "refusal", None, None)
            .expect("refusal text");
        assert!(!without.contains("Request ID:"), "got: {without}");

        // Empty id is treated as absent.
        let empty =
            terminal_api_error_text("claude-opus-4-8", true, "refusal", Some(""), None)
                .expect("refusal text");
        assert!(!empty.contains("Request ID:"), "got: {empty}");
    }

    /// The Request ID suffix is refusal-only: max_tokens / context-window
    /// messages never carry it, even when a request id is available.
    #[test]
    fn non_refusal_terminals_have_no_request_id() {
        for sr in ["max_tokens", "model_context_window_exceeded"] {
            let t = terminal_api_error_text("claude-opus-4-8", true, sr, Some("req_011abc"), None)
                .unwrap_or_else(|| panic!("{sr} text"));
            assert!(!t.contains("Request ID:"), "{sr}: {t}");
        }
    }

    /// No `stop_details` (the common refusal) → the generic label / Usage-Policy
    /// text, byte-exact (no cyber/bio wording).
    #[test]
    fn refusal_without_stop_details_is_generic() {
        // opus-4-8 HAS a marketing name → the label branch.
        let t = terminal_api_error_text("claude-opus-4-8", true, "refusal", None, None)
            .expect("refusal");
        assert!(t.contains("has safety measures that flagged something in this session"), "got: {t}");
        assert!(!t.contains("cybersecurity"), "got: {t}");
    }

    /// LABEL branch + cyber/bio category → the "flag messages on most
    /// cybersecurity or biology topics … They may flag safe, normal content…"
    /// variant (binary `U2e` `rnt(cat)` path).
    #[test]
    fn refusal_label_cyber_or_bio_variant() {
        for cat in ["cyber", "bio"] {
            let sd = details(cat, None);
            let t = terminal_api_error_text(
                "claude-opus-4-8",
                true,
                "refusal",
                None,
                Some(&sd),
            )
            .expect("refusal");
            assert!(
                t.contains("flag messages on most cybersecurity or biology topics"),
                "{cat}: {t}"
            );
            assert!(
                t.contains("They may flag safe, normal content as well."),
                "{cat}: {t}"
            );
            assert!(t.contains("Claude Code can't respond to this request with"), "{cat}: {t}");
        }
    }

    /// NO-LABEL branch + cyber category → the exemption-URL variant; the URL is
    /// extracted from the explanation, else the fallback.
    #[test]
    fn refusal_nolabel_cyber_exemption_url() {
        // A model with NO marketing name → the no-label branch. Use a bare id
        // that `marketing_name_for_model` does not resolve.
        let sd = details(
            "cyber",
            Some("see https://claude.com/form/abc123, thanks"),
        );
        let t = terminal_api_error_text("unknown-model-xyz", false, "refusal", None, Some(&sd))
            .expect("refusal");
        assert!(
            t.contains("flagged this message for a cybersecurity topic"),
            "got: {t}"
        );
        // Extracted URL (trailing comma stripped).
        assert!(t.contains("exemption: https://claude.com/form/abc123\n\n"), "got: {t}");
        assert!(!t.contains("abc123,"), "trailing punct must be stripped; got: {t}");
    }

    /// `oUi` exemption-URL extraction: form URL extracted (trailing punctuation
    /// stripped), else the fallback.
    #[test]
    fn exemption_url_extraction() {
        assert_eq!(
            refusal_exemption_url(Some("apply at https://claude.com/form/cyber-x).")),
            "https://claude.com/form/cyber-x"
        );
        assert_eq!(
            refusal_exemption_url(None),
            "https://claude.com/form/cyber-use-case"
        );
        assert_eq!(
            refusal_exemption_url(Some("no url here")),
            "https://claude.com/form/cyber-use-case"
        );
    }
}

#[cfg(test)]
mod model_text_tests {
    use super::tool_result_to_model_text;
    use serde_json::json;

    #[test]
    fn prefers_model_content_over_content() {
        // Read-shaped: the model sees the cat -n string, not the raw `content`.
        let data = json!({ "model_content": "1\thi\n2\t", "content": "hi\n" });
        assert_eq!(tool_result_to_model_text(&data), "1\thi\n2\t");
    }

    #[test]
    fn falls_back_to_content_string_verbatim() {
        // Bash/Edit/Write-shaped: no `model_content`, so the model sees the raw
        // `content` string verbatim — NOT a JSON dump of the object.
        let data = json!({ "content": "build ok\n", "exit_code": 0 });
        assert_eq!(tool_result_to_model_text(&data), "build ok\n");
    }

    #[test]
    fn falls_back_to_json_when_no_string_content() {
        // Structured-only result (no string `content`/`model_content`): legacy
        // JSON serialization is preserved.
        let data = json!({ "matches": ["a", "b"] });
        assert_eq!(tool_result_to_model_text(&data), r#"{"matches":["a","b"]}"#);
        // A non-string `content` also falls through to JSON.
        let data2 = json!({ "content": 42 });
        assert_eq!(tool_result_to_model_text(&data2), r#"{"content":42}"#);
    }
}

#[cfg(test)]
mod read_file_state_tests {
    use super::{absolutize, dispatch_tool_uses, execute_one_turn};
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream,
        NoOpPermissionGate, StaticMemoryProvider,
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
    use traits::OrchestratorHandle;

    /// Minimal Read/Edit/Write-shaped stub. Resolves `file_path` against
    /// `cwd` (mirroring how the real `FileReadTool` resolves against
    /// `getCwd()`) and reads it, so a missing file yields `is_error = true`
    /// exactly like the real tool. `name` is configurable so one stub can
    /// stand in for Read/Edit/Write.
    struct StubFileTool {
        name: &'static str,
        cwd: PathBuf,
    }

    #[async_trait]
    impl Tool for StubFileTool {
        fn name(&self) -> &str {
            self.name
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| {
                    json!({
                        "type": "object",
                        "properties": { "file_path": { "type": "string" } },
                        "required": ["file_path"],
                    })
                });
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
            "stub file tool".into()
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
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("file_path required".into()))?;
            // Resolve against the tool's cwd (mirrors getCwd()), then touch
            // disk so a missing file is a genuine error (mirrors Read).
            let resolved = self.cwd.join(path);
            let content = tokio::fs::read_to_string(&resolved)
                .await
                .map_err(|e| ToolError::Io(format!("read {}: {e}", resolved.display())))?;
            Ok(ToolCallResult {
                data: json!({ "content": content }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    /// Build an orchestrator whose registry contains the given stub tools,
    /// rooted at `cwd`. The API queue is empty (these tests drive
    /// `dispatch_tool_uses` directly, never `run_turn`).
    fn orch_with_tools(cwd: PathBuf, tools: Vec<Arc<dyn Tool>>) -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        for t in tools {
            registry.register_builtin(t);
        }
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            crate::test_support::noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            cwd,
        )
    }

    /// Drive one `(name, input)` `tool_use` through the dispatch chokepoint.
    async fn dispatch_one(orch: &ConversationOrchestrator, name: &str, input: serde_json::Value) {
        let uses = vec![(ToolUseId::new(), name.to_string(), input, None)];
        dispatch_tool_uses(orch, &uses).await.expect("dispatch");
    }

    // ----- JSON-schema input-validation gate -------------------------------
    // (claude-code `toolExecution.ts:615` `inputSchema.safeParse`). BEHAVIORAL
    // parity only — the message bytes intentionally differ from claude-code's
    // Zod `formatZodValidationError` output (unportable).

    fn schema_gate_tool_result(block: &protocol::ContentBlock) -> (&str, bool) {
        match block {
            protocol::ContentBlock::ToolResult {
                content, is_error, ..
            } => (content.as_str(), *is_error),
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    /// A tool whose `input_schema()` requires a string `path`, with a `call()`
    /// that records (via an `AtomicBool`) whether it was reached. Lets the
    /// pass-through test assert the gate did NOT short-circuit a valid input.
    struct SchemaCallTrackerTool {
        called: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl Tool for SchemaCallTrackerTool {
        fn name(&self) -> &str {
            "Schemic"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| {
                    json!({
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"],
                    })
                });
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
            "schema tracker".into()
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
            self.called
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(ToolCallResult {
                data: json!({ "ok": true }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    /// Missing a required field → the schema gate short-circuits with an
    /// `InputValidationError` `<tool_use_error>` block, and `call()` is never
    /// reached.
    #[tokio::test]
    async fn schema_gate_rejects_missing_required_field() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let orch = orch_with_tools(
            PathBuf::from("/tmp"),
            vec![Arc::new(SchemaCallTrackerTool {
                called: called.clone(),
            })],
        );
        let uses = vec![(ToolUseId::new(), "Schemic".to_string(), json!({}), None)];
        let results = dispatch_tool_uses(&orch, &uses).await.expect("dispatch");
        assert_eq!(results.len(), 1);
        let (content, is_error) = schema_gate_tool_result(&results[0]);
        assert!(is_error, "missing-required input must be an error");
        assert!(
            content.starts_with("<tool_use_error>InputValidationError:"),
            "expected InputValidationError wrapper, got: {content}"
        );
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "call() must NOT run when the schema gate rejects the input"
        );
    }

    /// A schema-valid input passes the gate and reaches `call()` without
    /// producing an `InputValidationError`.
    #[tokio::test]
    async fn schema_gate_passes_valid_input_through_to_call() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let orch = orch_with_tools(
            PathBuf::from("/tmp"),
            vec![Arc::new(SchemaCallTrackerTool {
                called: called.clone(),
            })],
        );
        let uses = vec![(
            ToolUseId::new(),
            "Schemic".to_string(),
            json!({ "path": "/x" }),
            None,
        )];
        let results = dispatch_tool_uses(&orch, &uses).await.expect("dispatch");
        assert_eq!(results.len(), 1);
        let (content, _is_error) = schema_gate_tool_result(&results[0]);
        assert!(
            !content.contains("InputValidationError"),
            "valid input must not trip the schema gate, got: {content}"
        );
        assert!(
            called.load(std::sync::atomic::Ordering::SeqCst),
            "call() must run for schema-valid input"
        );
    }

    // ----- build_wire_tools (registry -> wire `tools` array) -----

    #[tokio::test]
    async fn build_wire_tools_serializes_enabled_registry_tools() {
        // The orchestrator's wire tool array carries each enabled registry tool
        // as `{name, description, input_schema}`, sorted by name (the batched +
        // streaming legs both source their `tools` arg from here).
        let cwd = PathBuf::from("/tmp");
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubFileTool {
                name: "Read",
                cwd: cwd.clone(),
            }),
            Arc::new(StubFileTool {
                name: "Bash",
                cwd: cwd.clone(),
            }),
        ];
        let orch = orch_with_tools(cwd, tools);
        let wire = orch.build_wire_tools().await;

        let names: Vec<&str> = wire.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Bash", "Read"], "sorted by name");
        for t in &wire {
            // The base triple, nothing else.
            assert_eq!(t.as_object().unwrap().len(), 3, "base triple only: {t}");
            assert!(t.get("description").is_some());
            assert_eq!(t["input_schema"]["type"], "object");
        }
    }

    #[tokio::test]
    async fn build_wire_tools_empty_registry_is_empty() {
        let orch = orch_with_tools(PathBuf::from("/tmp"), vec![]);
        assert!(orch.build_wire_tools().await.is_empty());
    }

    // ----- FIX 1: tool-wide deny filter on the wire `tools` array -----------
    // claude-code `getTools`/`assembleToolPool` strip blanket-denied tools BEFORE
    // the model sees them (`filterToolsByDenyRules`, tools.ts:307-310). The
    // orchestrator now does the same in `build_wire_tools` via the gate's
    // `tool_wide_deny_names`.

    /// Build an orchestrator whose registry holds `builtins` + the MCP `mcp_tools`
    /// (a single connection), gated by a `PolicyPermissionGate` carrying the given
    /// tool-wide `deny` rule strings (e.g. `"WebFetch"`, `"mcp__github"`).
    fn orch_with_deny_rules(
        builtins: Vec<Arc<dyn Tool>>,
        mcp_tools: Vec<Arc<dyn Tool>>,
        deny: &[&str],
    ) -> ConversationOrchestrator {
        use permission::{
            PermissionBehavior, PermissionPolicy, PermissionRule, PermissionRuleSource,
            PermissionRuleValue, PolicyPermissionGate,
        };
        let mut registry = ToolRegistry::new();
        for t in builtins {
            registry.register_builtin(t);
        }
        if !mcp_tools.is_empty() {
            registry.register_mcp_tools(protocol::McpConnectionId::new(), mcp_tools);
        }
        let rules = deny.iter().map(|d| PermissionRule {
            value: PermissionRuleValue {
                tool_name: (*d).to_string(),
                rule_content: None,
            },
            behavior: PermissionBehavior::Deny,
            source: PermissionRuleSource::ProjectSettings,
        });
        let policy = Arc::new(PermissionPolicy::from_rules(
            permission::PermissionMode::Default,
            rules,
        ));
        let gate: Arc<dyn traits::permission_gate::PermissionGate> = Arc::new(
            PolicyPermissionGate::new(policy, Arc::new(NoOpPermissionGate)),
        );
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            crate::test_support::noop_hook_executor(),
            gate,
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    fn wire_tool_names(wire: &[serde_json::Value]) -> Vec<String> {
        wire.iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn build_wire_tools_hides_tool_wide_denied_tool() {
        let cwd = PathBuf::from("/tmp");
        let builtins: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubFileTool {
                name: "Read",
                cwd: cwd.clone(),
            }),
            Arc::new(StubFileTool {
                name: "WebFetch",
                cwd: cwd.clone(),
            }),
        ];
        let orch = orch_with_deny_rules(builtins, vec![], &["WebFetch"]);
        let names = wire_tool_names(&orch.build_wire_tools().await);
        assert_eq!(names, vec!["Read"], "deny:[WebFetch] hides WebFetch");
    }

    #[tokio::test]
    async fn build_wire_tools_mcp_server_prefix_deny_hides_all_server_tools() {
        // A tool-wide `mcp__github` deny strips EVERY `mcp__github__*` tool
        // (claude-code MCP server-prefix blanket strip) but leaves other servers.
        let cwd = PathBuf::from("/tmp");
        let builtins: Vec<Arc<dyn Tool>> = vec![Arc::new(StubFileTool {
            name: "Read",
            cwd: cwd.clone(),
        })];
        let mcp: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubFileTool {
                name: "mcp__github__issue",
                cwd: cwd.clone(),
            }),
            Arc::new(StubFileTool {
                name: "mcp__github__pr",
                cwd: cwd.clone(),
            }),
            Arc::new(StubFileTool {
                name: "mcp__slack__post",
                cwd: cwd.clone(),
            }),
        ];
        let orch = orch_with_deny_rules(builtins, mcp, &["mcp__github"]);
        let names = wire_tool_names(&orch.build_wire_tools().await);
        assert_eq!(
            names,
            vec!["Read", "mcp__slack__post"],
            "deny:[mcp__github] hides all mcp__github__* but keeps mcp__slack__post"
        );
    }

    #[tokio::test]
    async fn build_wire_tools_empty_deny_is_byte_identical() {
        // Regression safety: with ZERO deny rules the filtered output must equal
        // the unfiltered output (the default-gate path must not perturb anything).
        let cwd = PathBuf::from("/tmp");
        let mk = || -> Vec<Arc<dyn Tool>> {
            vec![
                Arc::new(StubFileTool {
                    name: "Bash",
                    cwd: cwd.clone(),
                }) as Arc<dyn Tool>,
                Arc::new(StubFileTool {
                    name: "WebFetch",
                    cwd: cwd.clone(),
                }) as Arc<dyn Tool>,
            ]
        };
        // No-deny gate (PolicyPermissionGate with empty rules) vs the default
        // NoOp gate: both must yield the same wire bytes as the plain registry.
        let baseline = orch_with_tools(cwd.clone(), mk()).build_wire_tools().await;
        let gated = orch_with_deny_rules(mk(), vec![], &[])
            .build_wire_tools()
            .await;
        assert_eq!(
            gated, baseline,
            "empty deny must be byte-identical to the unfiltered wire tools"
        );
        assert_eq!(wire_tool_names(&gated), vec!["Bash", "WebFetch"]);
    }

    #[tokio::test]
    async fn batched_turn_forwards_wire_tools_to_messages_create() {
        // End-to-end (batched leg): `execute_one_turn` must build the registry's
        // wire tools and pass them to `messages_create`. A no-tool `end_turn`
        // response terminates the step after a single round-trip. (The streaming
        // leg's twin is `streaming_concurrent_tools_test`.)
        let cwd = PathBuf::from("/tmp");
        let api = Arc::new(MockApiClient::new(vec![mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        )]));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(StubFileTool {
            name: "Read",
            cwd: cwd.clone(),
        }));
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api.clone(),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            cwd,
        );

        let _ = execute_one_turn(&orch, None).await.expect("turn step");

        let captured = api.captured_tools().await;
        assert_eq!(captured.len(), 1, "exactly one messages_create round-trip");
        let names: Vec<&str> = captured[0]
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["Read"],
            "batched turn must advertise the registry's wire tools to messages_create"
        );
    }

    // ----- absolutize (pure, lexical — NOT realpath) -----

    #[test]
    fn absolutize_helper_resolves_lexically() {
        let cwd = PathBuf::from("/repo");
        assert_eq!(absolutize(&cwd, "src/main.rs"), PathBuf::from("/repo/src/main.rs"));
        assert_eq!(absolutize(&cwd, "/abs/x.rs"), PathBuf::from("/abs/x.rs"));
        // `.` dropped, `..` popped — purely lexical.
        assert_eq!(absolutize(&cwd, "./a/../b.rs"), PathBuf::from("/repo/b.rs"));
        // Surrounding whitespace is trimmed (mirrors expandPath).
        assert_eq!(absolutize(&cwd, "  src/a.rs  "), PathBuf::from("/repo/src/a.rs"));
    }

    #[test]
    fn absolutize_does_not_canonicalize_disk() {
        // A path that does NOT exist must still resolve to the joined string
        // (expandPath is lexical, not realpath — no fs canonicalization).
        let cwd = PathBuf::from("/nonexistent-root-xyz");
        let got = absolutize(&cwd, "does/not/exist.rs");
        assert_eq!(got, PathBuf::from("/nonexistent-root-xyz/does/not/exist.rs"));
    }

    #[test]
    fn absolutize_expands_tilde() {
        let Some(home) = dirs::home_dir() else {
            return; // no home dir on this platform — skip
        };
        assert_eq!(absolutize(&PathBuf::from("/repo"), "~/x"), home.join("x"));
        assert_eq!(absolutize(&PathBuf::from("/repo"), "~"), home);
    }

    // ----- cache population through the dispatch loop -----

    #[tokio::test]
    async fn cache_records_successful_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("src");
        std::fs::create_dir_all(&sub).expect("mkdir");
        std::fs::write(sub.join("a.rs"), b"fn a() {}").expect("write");

        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "Read", cwd: cwd.clone() }) as Arc<dyn Tool>],
        );
        dispatch_one(&orch, "Read", json!({ "file_path": "src/a.rs" })).await;

        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("src").join("a.rs")]);
        // The richer `read_state_map` is a SEPARATE registry from the `/files`
        // `Vec`. `record_read_file_state` (which a `StubFileTool` dispatch
        // exercises) only touches the `Vec`; the map is populated by the real
        // file tools' `readFileState.set`, which the stub does not call. So the
        // `/files` ordering semantics above are unaffected by Batch B.
        assert!(
            orch.read_state_map.lock().unwrap().is_empty(),
            "the richer read-state map is independent of the /files Vec"
        );
    }

    #[tokio::test]
    async fn read_state_map_starts_empty_and_is_distinct_from_files_vec() {
        // Behavior-neutral wiring check: a fresh orchestrator has an empty
        // read-state registry, separate from the `/files` `Vec`.
        let orch = orch_with_tools(PathBuf::from("/tmp"), vec![]);
        assert!(orch.read_state_map.lock().unwrap().is_empty());
        assert!(orch.files_in_context().await.is_empty());
    }

    #[tokio::test]
    async fn read_state_map_arc_is_shareable_and_visible_through_orchestrator() {
        // Proves the composition-root contract: the SAME `Arc` the orchestrator
        // holds in `read_state_map` is what the file tools' `BuiltinToolContext`
        // share, so a `readFileState.set` performed against a clone of that
        // `Arc` (as the real `FileReadTool` does — see the `tool-file`
        // `read_populates_read_file_state_map_with_offset_limit` test) is
        // visible through `orch.read_state_map`. Simulated here with a direct
        // `set` (the orchestrator crate cannot depend on `tool-file`), keeping
        // the wiring assertion crate-local. The `/files` `Vec` is untouched.
        let orch = orch_with_tools(PathBuf::from("/tmp"), vec![]);
        let shared = orch.read_state_map.clone();
        tool_api::read_file_state::set(
            &shared,
            PathBuf::from("/tmp/a.txt"),
            tool_api::read_file_state::ReadFileEntry {
                content: "line2\n".into(),
                mtime_ms: 42,
                offset: Some(2),
                limit: Some(1),
                from_read: true,
            },
        );
        let entry =
            tool_api::read_file_state::get(&orch.read_state_map, std::path::Path::new("/tmp/a.txt"))
                .expect("orchestrator registry sees the shared-Arc set");
        assert_eq!(entry.content, "line2\n");
        assert_eq!(entry.offset, Some(2));
        assert_eq!(entry.limit, Some(1));
        // The `/files` `Vec` remains independent and empty.
        assert!(orch.files_in_context().await.is_empty());
    }

    // ----- #59 post-compact file/skill attachment restoration -----

    #[tokio::test]
    async fn force_compact_restores_recent_files_and_clears_read_state() {
        use compaction::CompactionOrchestrator;
        use protocol::{ConversationMessage, MessageId};
        use traits::OrchestratorHandle;

        // Compaction with a tiny threshold so a small seeded history compacts.
        let mut orch = orch_with_tools(PathBuf::from("/tmp"), vec![]);
        orch = orch.with_compaction(Arc::new(CompactionOrchestrator::new(10)));
        let orch = Arc::new(orch);

        // Seed enough history to trip the compactor.
        {
            let session = orch.session();
            let mut s = session.lock().await;
            for i in 0..20 {
                s.history.push(ConversationMessage::user(
                    MessageId::new(),
                    format!("turn-{i} padded body text to push the token estimate over the threshold"),
                ));
            }
        }

        // Seed the read-file-state registry with two files at distinct mtimes.
        tool_api::read_file_state::set(
            &orch.read_state_map,
            PathBuf::from("/tmp/old.rs"),
            tool_api::read_file_state::ReadFileEntry {
                content: "fn old() {}\n".into(),
                mtime_ms: 100,
                offset: None,
                limit: None,
                from_read: true,
            },
        );
        tool_api::read_file_state::set(
            &orch.read_state_map,
            PathBuf::from("/tmp/new.rs"),
            tool_api::read_file_state::ReadFileEntry {
                content: "fn fresh() {}\n".into(),
                mtime_ms: 200,
                offset: None,
                limit: None,
                from_read: true,
            },
        );

        orch.force_compact().await.expect("force_compact ok");

        // The read-state registry is cleared post-compact (`readFileState.clear`).
        assert!(
            orch.read_state_map.lock().unwrap().is_empty(),
            "read_state_map must be cleared after compaction"
        );

        // The restored file attachments ride after the boundary marker + summary.
        let session = orch.session();
        let s = session.lock().await;
        let restored: Vec<&String> = s
            .history
            .iter()
            .filter_map(|m| match m {
                ConversationMessage::User { content, is_meta: true, .. } => {
                    content.iter().find_map(|b| match b {
                        protocol::ContentBlock::Text { text }
                            if text.contains("restored after compaction") =>
                        {
                            Some(text)
                        }
                        _ => None,
                    })
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            restored.len(),
            2,
            "both seeded files should be restored as attachments"
        );
        // Most-recent file content is present.
        assert!(
            restored.iter().any(|t| t.contains("fn fresh() {}")),
            "the freshest file content must be restored"
        );
        assert!(
            restored.iter().any(|t| t.contains("/tmp/new.rs")),
            "the restored attachment names the file path"
        );
    }

    #[tokio::test]
    async fn cache_skips_errored_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "Read", cwd }) as Arc<dyn Tool>],
        );
        // File does not exist → the tool errors → nothing is cached.
        dispatch_one(&orch, "Read", json!({ "file_path": "missing.rs" })).await;
        assert!(orch.files_in_context().await.is_empty());
    }

    #[tokio::test]
    async fn cache_insertion_order_and_dedup() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), b"a").expect("write a");
        std::fs::write(dir.path().join("b.rs"), b"b").expect("write b");

        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "Read", cwd: cwd.clone() }) as Arc<dyn Tool>],
        );
        // a, b, then a again — first-insertion order [a, b], a not duplicated.
        // This 2-file re-read case coincides with TS's MRU LRU (also [a, b]);
        // the divergence only appears at ≥3 files — see
        // `three_file_reread_locks_first_insertion_order` below.
        dispatch_one(&orch, "Read", json!({ "file_path": "a.rs" })).await;
        dispatch_one(&orch, "Read", json!({ "file_path": "b.rs" })).await;
        dispatch_one(&orch, "Read", json!({ "file_path": "a.rs" })).await;

        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("a.rs"), cwd.join("b.rs")]);
    }

    #[tokio::test]
    async fn three_file_reread_locks_first_insertion_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        for f in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(dir.path().join(f), b"x").expect("write");
        }
        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "Read", cwd: cwd.clone() }) as Arc<dyn Tool>],
        );
        // read a, b, c, then a again. This `Vec` keeps first-insertion order
        // [a, b, c]; TS's MRU-promoting LRU would diverge to [a, c, b]. Locking
        // [a, b, c] pins the documented divergence so a future switch to MRU
        // semantics cannot pass silently.
        for f in ["a.rs", "b.rs", "c.rs", "a.rs"] {
            dispatch_one(&orch, "Read", json!({ "file_path": f })).await;
        }
        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("a.rs"), cwd.join("b.rs"), cwd.join("c.rs")]);
    }

    #[tokio::test]
    async fn notebook_edit_records_notebook_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("nb.ipynb"), b"{}").expect("write nb");
        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![Arc::new(StubFileTool { name: "NotebookEdit", cwd: cwd.clone() }) as Arc<dyn Tool>],
        );
        // `NotebookEdit` keys the cache on `notebook_path`; the stub reads
        // `file_path` to confirm the file exists, so pass both (same path).
        dispatch_one(
            &orch,
            "NotebookEdit",
            json!({ "notebook_path": "nb.ipynb", "file_path": "nb.ipynb" }),
        )
        .await;
        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("nb.ipynb")]);
    }

    #[tokio::test]
    async fn edit_and_write_record_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("e.rs"), b"e").expect("write e");
        std::fs::write(dir.path().join("w.rs"), b"w").expect("write w");

        let cwd = dir.path().to_path_buf();
        let orch = orch_with_tools(
            cwd.clone(),
            vec![
                Arc::new(StubFileTool { name: "Edit", cwd: cwd.clone() }) as Arc<dyn Tool>,
                Arc::new(StubFileTool { name: "Write", cwd: cwd.clone() }) as Arc<dyn Tool>,
            ],
        );
        dispatch_one(&orch, "Edit", json!({ "file_path": "e.rs" })).await;
        dispatch_one(&orch, "Write", json!({ "file_path": "w.rs" })).await;

        let files = orch.files_in_context().await;
        assert_eq!(files, vec![cwd.join("e.rs"), cwd.join("w.rs")]);
    }
}

// ============================================================================
// A1: max_output_tokens recovery (multi-turn nudge + escalation/exhaustion).
// Drives `execute_one_turn_with_recovery` directly with a `max_tokens`-scripted
// MockApiClient and asserts the nudge injection, counter increments, and
// disposition (Continue while under the limit; Ended on exhaustion).
// ============================================================================
#[cfg(test)]
mod max_output_tokens_recovery_tests {
    use super::{
        execute_one_turn_with_recovery, RecoveryState, TurnStepOutcome, ESCALATED_MAX_TOKENS,
        MAX_OUTPUT_TOKENS_RECOVERY_LIMIT, MAX_OUTPUT_TOKENS_RECOVERY_NUDGE,
    };
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use llm_client::LlmResponse;
    use protocol::{ContentBlock, ConversationMessage};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// Build an orchestrator whose batched API returns the given scripted
    /// `LlmResponse`s in order. No tools registered (recovery never needs
    /// them).
    fn orch_with_responses(
        responses: Vec<LlmResponse>,
    ) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(responses)),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// A `max_tokens` response carrying one text block.
    fn max_tokens_response() -> LlmResponse {
        mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "partial".into(),
                cache_control: None,
            }],
            Some("max_tokens"),
        )
    }

    /// Snapshot the current session history.
    async fn history(orch: &ConversationOrchestrator) -> Vec<ConversationMessage> {
        orch.session.lock().await.history.clone()
    }

    /// The exact-bytes nudge string is byte-faithful to TS `query.ts:1226-1227`,
    /// including the U+2014 em-dash and the single space joining the two literals.
    #[test]
    fn nudge_string_is_byte_exact() {
        assert_eq!(
            MAX_OUTPUT_TOKENS_RECOVERY_NUDGE,
            "Output token limit hit. Resume directly \u{2014} no apology, no recap of what you were doing. Pick up mid-thought if that is where the cut happened. Break remaining work into smaller pieces."
        );
        // The em-dash is U+2014, not an ASCII hyphen or U+2013 en-dash.
        assert!(MAX_OUTPUT_TOKENS_RECOVERY_NUDGE.contains('\u{2014}'));
        assert!(!MAX_OUTPUT_TOKENS_RECOVERY_NUDGE.contains("directly -"));
    }

    #[test]
    fn recovery_limit_is_three() {
        assert_eq!(MAX_OUTPUT_TOKENS_RECOVERY_LIMIT, 3);
    }

    /// (Test plan 1) `max_tokens` at recovery_count 0 → Continue, the exact
    /// nudge is appended as a User message, and the counter becomes 1.
    #[tokio::test]
    async fn max_tokens_at_count_zero_continues_and_injects_nudge() {
        let orch = orch_with_responses(vec![max_tokens_response()]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("turn step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert_eq!(state.max_output_tokens_recovery_count, 1);
        assert_eq!(state.max_output_tokens_override, None);

        // History: [assistant(max_tokens), user(nudge)].
        let h = history(&orch).await;
        let last = h.last().expect("nudge appended");
        match last {
            ConversationMessage::User { content, .. } => {
                assert_eq!(content.len(), 1, "single text block");
                match &content[0] {
                    // (Test plan 4) the nudge is a User message with exact bytes.
                    ContentBlock::Text { text } => {
                        assert_eq!(text, MAX_OUTPUT_TOKENS_RECOVERY_NUDGE);
                    }
                    other => panic!("expected text block, got {other:?}"),
                }
            }
            other => panic!("expected User nudge message, got {other:?}"),
        }
    }

    /// (Test plan 1) `max_tokens` at counts 1 and 2 → Continue, counter
    /// increments to 2 then 3. A fresh `max_tokens` is queued per step.
    #[tokio::test]
    async fn max_tokens_at_counts_one_and_two_continue_and_increment() {
        let orch = orch_with_responses(vec![max_tokens_response(), max_tokens_response()]);
        let mut state = RecoveryState {
            max_output_tokens_recovery_count: 1,
            max_output_tokens_override: None,
            max_output_tokens_escalated: false,
            ..Default::default()
        };

        // count 1 → 2
        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert_eq!(state.max_output_tokens_recovery_count, 2);

        // count 2 → 3 (still < limit, so still nudges)
        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert_eq!(state.max_output_tokens_recovery_count, 3);

        // Two nudges were appended (one per step).
        let h = history(&orch).await;
        let nudges = h
            .iter()
            .filter(|m| {
                matches!(
                    m,
                    ConversationMessage::User { content, .. }
                        if matches!(content.first(), Some(ContentBlock::Text { text })
                            if text == MAX_OUTPUT_TOKENS_RECOVERY_NUDGE)
                )
            })
            .count();
        assert_eq!(nudges, 2);
    }

    /// (Test plan 2) the 4th consecutive `max_tokens` (count already at the
    /// limit of 3) → Ended with stop_reason `max_tokens`, no further nudge.
    #[tokio::test]
    async fn fourth_consecutive_max_tokens_ends_turn() {
        let orch = orch_with_responses(vec![max_tokens_response()]);
        let mut state = RecoveryState {
            max_output_tokens_recovery_count: MAX_OUTPUT_TOKENS_RECOVERY_LIMIT,
            max_output_tokens_override: None,
            max_output_tokens_escalated: false,
            ..Default::default()
        };

        let len_before = history(&orch).await.len();
        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => {
                assert_eq!(stop_reason, "max_tokens");
            }
            TurnStepOutcome::Continue => panic!("expected Ended on exhaustion"),
        }
        // The counter is NOT incremented past the limit, and NO nudge is
        // appended on exhaustion. The step appends the response assistant message
        // AND the surfaced terminal `API Error: …` assistant message (#24 batched
        // parity with the streaming terminal arm) → +2.
        assert_eq!(state.max_output_tokens_recovery_count, MAX_OUTPUT_TOKENS_RECOVERY_LIMIT);
        let h = history(&orch).await;
        assert_eq!(
            h.len(),
            len_before + 2,
            "the step's assistant msg + the surfaced terminal API-error msg"
        );
        // The last message is the surfaced terminal API-error assistant.
        match h.last() {
            Some(ConversationMessage::Assistant {
                content,
                stop_reason,
                ..
            }) => {
                assert_eq!(stop_reason.as_deref(), Some("max_tokens"));
                let ContentBlock::Text { text } = &content[0] else {
                    panic!("expected a text block");
                };
                assert!(
                    text.starts_with("API Error: Claude's response exceeded"),
                    "got: {text}"
                );
            }
            other => panic!("expected the surfaced Assistant API-error, got {other:?}"),
        }
    }

    /// #24 batched parity: a terminal `model_context_window_exceeded` surfaces
    /// the byte-locked `API Error: …` assistant message AND ends the turn
    /// (previously fell through to `_ => Continue` and bare-re-called the API).
    #[tokio::test]
    async fn model_context_window_exceeded_surfaces_error_and_ends() {
        let orch = orch_with_responses(vec![mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "partial".into(),
                cache_control: None,
            }],
            Some("model_context_window_exceeded"),
        )]);
        let mut state = RecoveryState::default();
        let len_before = history(&orch).await.len();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => {
                assert_eq!(stop_reason, "model_context_window_exceeded");
            }
            TurnStepOutcome::Continue => panic!("expected Ended, not a bare re-call"),
        }
        let h = history(&orch).await;
        assert_eq!(h.len(), len_before + 2, "response asst + surfaced API-error asst");
        let Some(ConversationMessage::Assistant { content, stop_reason, .. }) = h.last() else {
            panic!("expected the surfaced Assistant API-error");
        };
        assert_eq!(stop_reason.as_deref(), Some("model_context_window_exceeded"));
        let ContentBlock::Text { text } = &content[0] else {
            panic!("expected a text block");
        };
        assert_eq!(text, "API Error: The model has reached its context window limit.");
    }

    /// #24 batched parity: a terminal `refusal` with NO `refusalFallbackModel`
    /// (the swap arm returns false) surfaces the byte-locked Usage-Policy
    /// `API Error: …` message AND ends the turn (previously bare-re-called).
    #[tokio::test]
    async fn terminal_refusal_without_fallback_surfaces_error_and_ends() {
        // Default config has no refusalFallbackModel → maybe_swap returns false.
        let orch = orch_with_responses(vec![mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "partial".into(),
                cache_control: None,
            }],
            Some("refusal"),
        )]);
        let mut state = RecoveryState::default();
        let len_before = history(&orch).await.len();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => assert_eq!(stop_reason, "refusal"),
            TurnStepOutcome::Continue => panic!("expected Ended, not a bare re-call"),
        }
        let h = history(&orch).await;
        assert_eq!(h.len(), len_before + 2, "response asst + surfaced API-error asst");
        let Some(ConversationMessage::Assistant { content, stop_reason, .. }) = h.last() else {
            panic!("expected the surfaced Assistant API-error");
        };
        assert_eq!(stop_reason.as_deref(), Some("refusal"));
        let ContentBlock::Text { text } = &content[0] else {
            panic!("expected a text block");
        };
        // Either the labelled "safety measures" or the generic Usage-Policy
        // variant — both are `API Error: …` and cite the AUP URL.
        assert!(text.starts_with("API Error:"), "got: {text}");
        assert!(
            text.contains("https://www.anthropic.com/legal/aup"),
            "got: {text}"
        );
    }

    /// (Test plan 3) a normal `end_turn` is unaffected by the recovery wiring:
    /// it Ends with `end_turn`, never touches the recovery counter, and appends
    /// no nudge.
    #[tokio::test]
    async fn normal_end_turn_unaffected_by_recovery() {
        let orch = orch_with_responses(vec![mock_message_response(
            vec![llm_client::ContentBlock::Text { text: "done".into(), cache_control: None }],
            Some("end_turn"),
        )]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => assert_eq!(stop_reason, "end_turn"),
            TurnStepOutcome::Continue => panic!("expected Ended"),
        }
        assert_eq!(state.max_output_tokens_recovery_count, 0);
        let h = history(&orch).await;
        // [assistant] only — no nudge.
        assert!(matches!(h.last(), Some(ConversationMessage::Assistant { .. })));
        assert!(!h.iter().any(|m| matches!(
            m,
            ConversationMessage::User { content, .. }
                if matches!(content.first(), Some(ContentBlock::Text { text })
                    if text == MAX_OUTPUT_TOKENS_RECOVERY_NUDGE)
        )));
    }

    /// As [`orch_with_responses`] but with the REC.A1 8k→64k escalation enabled.
    fn orch_with_responses_escalating(
        responses: Vec<LlmResponse>,
    ) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig {
                escalate_max_output_tokens: true,
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(responses)),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// REC.A1: with escalation ON, the FIRST `max_tokens` arms the 64k override
    /// and the once-per-episode gate, and returns `Continue` WITHOUT a nudge —
    /// the single-shot retry fires before the multi-turn nudge
    /// (TS `query.ts:1199-1221`).
    #[tokio::test]
    async fn escalation_arms_override_and_continues_without_nudge() {
        let orch = orch_with_responses_escalating(vec![max_tokens_response()]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("turn step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert_eq!(state.max_output_tokens_override, Some(ESCALATED_MAX_TOKENS));
        assert!(state.max_output_tokens_escalated);
        // No nudge counted/injected — the escalation precedes the nudge path.
        assert_eq!(state.max_output_tokens_recovery_count, 0);
        let h = history(&orch).await;
        assert!(
            !matches!(h.last(), Some(ConversationMessage::User { .. })),
            "escalation must not inject a nudge; got {:?}",
            h.last()
        );
    }

    /// REC.A1: once escalated, a SECOND `max_tokens` TAKEs the armed override
    /// (so the retry used 64k) and, since the episode already escalated, falls
    /// through to the multi-turn nudge instead of escalating again — no
    /// escalate-forever loop.
    #[tokio::test]
    async fn second_max_tokens_after_escalation_takes_override_then_nudges() {
        let orch = orch_with_responses_escalating(vec![max_tokens_response()]);
        let mut state = RecoveryState {
            max_output_tokens_override: Some(ESCALATED_MAX_TOKENS),
            max_output_tokens_escalated: true,
            ..Default::default()
        };

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("turn step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        // The one-shot override was consumed for this call; the nudge path ran.
        assert_eq!(state.max_output_tokens_override, None);
        assert!(
            state.max_output_tokens_escalated,
            "stays escalated for the rest of this episode"
        );
        assert_eq!(state.max_output_tokens_recovery_count, 1);
        assert!(
            matches!(history(&orch).await.last(), Some(ConversationMessage::User { .. })),
            "nudge appended after the escalation was exhausted"
        );
    }

    /// The legacy 2-arg shim (`recovery = None`) preserves the bare behavior:
    /// `max_tokens` falls through to Continue WITHOUT injecting a nudge — the
    /// cancelable REPL driver depends on this no-op.
    #[tokio::test]
    async fn legacy_shim_does_not_recover_on_max_tokens() {
        let orch = orch_with_responses(vec![max_tokens_response()]);
        let step = super::execute_one_turn(&orch, None).await.expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        let h = history(&orch).await;
        // Only the assistant message; no nudge appended by the shim.
        assert!(!h.iter().any(|m| matches!(
            m,
            ConversationMessage::User { content, .. }
                if matches!(content.first(), Some(ContentBlock::Text { text })
                    if text == MAX_OUTPUT_TOKENS_RECOVERY_NUDGE)
        )));
    }
}

// ============================================================================
// #77 malformed-tool-use retry + #78 thinking-only nudge (BATCHED path).
// Drives `execute_one_turn_with_recovery_tracked` with a scripted `LlmResponse`
// whose stop_reason / blocks force each branch, then asserts the byte-exact
// nudge injection, the per-turn guard transitions, and the disposition.
// ============================================================================
#[cfg(test)]
mod malformed_and_thinking_only_tests {
    use super::{
        execute_one_turn, execute_one_turn_with_recovery, RecoveryState, TurnStepOutcome,
        MALFORMED_TOOL_USE_RETRY_FAILED, MALFORMED_TOOL_USE_RETRY_NUDGE, THINKING_ONLY_NUDGE,
    };
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use llm_client::LlmResponse;
    use protocol::{ContentBlock, ConversationMessage};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    fn orch_with_responses(responses: Vec<LlmResponse>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(responses)),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// A response whose `stop_reason` is `tool_use` but which carries ZERO
    /// `tool_use` blocks (only a text block) — the #77 malformed shape.
    fn malformed_tool_use_response() -> LlmResponse {
        mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "I'll call the tool".into(),
                cache_control: None,
            }],
            Some("tool_use"),
        )
    }

    /// A thinking-only response: `end_turn` `stop_reason` but only a `Reasoning`
    /// (thinking) block — no visible text. The #78 shape.
    fn thinking_only_response(stop_reason: &str) -> LlmResponse {
        mock_message_response(
            vec![llm_client::ContentBlock::Reasoning {
                text: "thinking quietly".into(),
                signature: None,
            }],
            Some(stop_reason),
        )
    }

    async fn history(orch: &ConversationOrchestrator) -> Vec<ConversationMessage> {
        orch.session.lock().await.history.clone()
    }

    fn last_user_text(h: &[ConversationMessage]) -> Option<String> {
        match h.last()? {
            ConversationMessage::User { content, .. } => match content.first()? {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    // ---- byte-exact strings ------------------------------------------------

    #[test]
    fn malformed_nudge_strings_are_byte_exact() {
        // Default build: clean-retry feature flag OFF (`PZa()` defaults false),
        // so the first-failure string is the non-clean-retry variant.
        assert_eq!(
            MALFORMED_TOOL_USE_RETRY_NUDGE,
            "Your tool call was malformed and could not be parsed. Please retry."
        );
        assert_eq!(
            MALFORMED_TOOL_USE_RETRY_FAILED,
            "The model's tool call could not be parsed (retry also failed)."
        );
    }

    #[test]
    fn thinking_only_nudge_is_byte_exact() {
        assert_eq!(
            THINKING_ONLY_NUDGE,
            "[Your previous response had no visible output. Please continue and produce a user-visible response.]"
        );
    }

    // ---- #77 malformed-tool-use -------------------------------------------

    /// First malformed `tool_use` → Continue, byte-exact nudge appended as a
    /// user message, guard armed, recovery reset.
    #[tokio::test]
    async fn malformed_tool_use_first_failure_continues_and_nudges() {
        let orch = orch_with_responses(vec![malformed_tool_use_response()]);
        let mut state = RecoveryState {
            // Pre-seed a non-zero recovery count to prove it gets reset.
            max_output_tokens_recovery_count: 2,
            ..Default::default()
        };

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert!(state.malformed_tool_use_retried, "guard armed");
        assert_eq!(
            state.max_output_tokens_recovery_count, 0,
            "recovery reset on retry transition"
        );

        let h = history(&orch).await;
        assert_eq!(
            last_user_text(&h).as_deref(),
            Some(MALFORMED_TOOL_USE_RETRY_NUDGE)
        );
    }

    /// Second malformed `tool_use` (guard already armed) → Ended with
    /// stop_reason `end_turn`, the non-meta terminal message appended.
    #[tokio::test]
    async fn malformed_tool_use_second_failure_ends_turn() {
        let orch = orch_with_responses(vec![malformed_tool_use_response()]);
        let mut state = RecoveryState {
            malformed_tool_use_retried: true,
            ..Default::default()
        };

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => {
                assert_eq!(stop_reason, "end_turn");
            }
            TurnStepOutcome::Continue => panic!("expected Ended on second failure"),
        }
        let h = history(&orch).await;
        assert_eq!(
            last_user_text(&h).as_deref(),
            Some(MALFORMED_TOOL_USE_RETRY_FAILED)
        );
    }

    /// A NORMAL `tool_use` response (with an actual tool_use block) must NOT
    /// trigger the malformed path — it Continues to dispatch as usual and
    /// injects no malformed nudge.
    #[tokio::test]
    async fn normal_tool_use_does_not_trigger_malformed_path() {
        let resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: "toolu_1".into(),
                name: "Nope".into(), // unknown tool → synthetic error, still dispatched
                input: serde_json::json!({}),
            }],
            Some("tool_use"),
        );
        let orch = orch_with_responses(vec![resp]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert!(!state.malformed_tool_use_retried, "guard NOT armed");
        let h = history(&orch).await;
        assert!(!h.iter().any(|m| matches!(
            m,
            ConversationMessage::User { content, .. }
                if matches!(content.first(), Some(ContentBlock::Text { text })
                    if text == MALFORMED_TOOL_USE_RETRY_NUDGE)
        )));
    }

    /// The legacy shim (`recovery == None`) keeps the historical `_ => Continue`
    /// no-op on a malformed `tool_use` — no nudge.
    #[tokio::test]
    async fn legacy_shim_does_not_handle_malformed_tool_use() {
        let orch = orch_with_responses(vec![malformed_tool_use_response()]);
        let step = execute_one_turn(&orch, None).await.expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        let h = history(&orch).await;
        assert!(!h.iter().any(|m| matches!(
            m,
            ConversationMessage::User { content, .. }
                if matches!(content.first(), Some(ContentBlock::Text { text })
                    if text == MALFORMED_TOOL_USE_RETRY_NUDGE)
        )));
    }

    // ---- #78 thinking-only -------------------------------------------------

    /// An `end_turn` thinking-only response (not yet nudged) → Continue with
    /// the byte-exact nudge appended, guard armed.
    #[tokio::test]
    async fn thinking_only_end_turn_first_time_nudges() {
        let orch = orch_with_responses(vec![thinking_only_response("end_turn")]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert!(state.thinking_only_nudged, "guard armed");
        let h = history(&orch).await;
        assert_eq!(last_user_text(&h).as_deref(), Some(THINKING_ONLY_NUDGE));
    }

    /// A `stop_sequence` thinking-only response also triggers the nudge.
    #[tokio::test]
    async fn thinking_only_stop_sequence_nudges() {
        let orch = orch_with_responses(vec![thinking_only_response("stop_sequence")]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert!(state.thinking_only_nudged);
        let h = history(&orch).await;
        assert_eq!(last_user_text(&h).as_deref(), Some(THINKING_ONLY_NUDGE));
    }

    /// Once nudged, a still-thinking-only `end_turn` ends the turn normally
    /// (no second nudge).
    #[tokio::test]
    async fn thinking_only_already_nudged_ends_turn() {
        let orch = orch_with_responses(vec![thinking_only_response("end_turn")]);
        let mut state = RecoveryState {
            thinking_only_nudged: true,
            ..Default::default()
        };

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => assert_eq!(stop_reason, "end_turn"),
            TurnStepOutcome::Continue => panic!("expected Ended once already nudged"),
        }
        let h = history(&orch).await;
        assert_ne!(last_user_text(&h).as_deref(), Some(THINKING_ONLY_NUDGE));
    }

    /// An `end_turn` response WITH visible text ends the turn — never nudged.
    #[tokio::test]
    async fn end_turn_with_visible_text_does_not_nudge() {
        let resp = mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "Here is the answer.".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = orch_with_responses(vec![resp]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Ended { .. }));
        assert!(!state.thinking_only_nudged);
        let h = history(&orch).await;
        assert_ne!(last_user_text(&h).as_deref(), Some(THINKING_ONLY_NUDGE));
    }

    /// Whitespace-only text counts as NOT visible (`.trim()` empty) → nudged.
    #[tokio::test]
    async fn whitespace_only_text_is_not_visible() {
        let resp = mock_message_response(
            vec![llm_client::ContentBlock::Text {
                text: "   \n  ".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = orch_with_responses(vec![resp]);
        let mut state = RecoveryState::default();

        let step = execute_one_turn_with_recovery(&orch, None, Some(&mut state))
            .await
            .expect("step");
        assert!(matches!(step, TurnStepOutcome::Continue));
        assert!(state.thinking_only_nudged);
    }

    /// The legacy shim (`recovery == None`) does NOT nudge on a thinking-only
    /// `end_turn`; it ends the turn as before.
    #[tokio::test]
    async fn legacy_shim_does_not_handle_thinking_only() {
        let orch = orch_with_responses(vec![thinking_only_response("end_turn")]);
        let step = execute_one_turn(&orch, None).await.expect("step");
        match step {
            TurnStepOutcome::Ended { stop_reason, .. } => assert_eq!(stop_reason, "end_turn"),
            TurnStepOutcome::Continue => panic!("legacy shim should end on end_turn"),
        }
        let h = history(&orch).await;
        assert_ne!(last_user_text(&h).as_deref(), Some(THINKING_ONLY_NUDGE));
    }
}

/// HOOK.1 / HOOK.2 / HOOK.3 — `PreToolUse` hook behaviors surfaced by the turn
/// loop's `dispatch_tool_uses` chokepoint (TS `services/tools/toolExecution.ts`
/// + `toolHooks.ts` + `query.ts:1518-1521`).
#[cfg(test)]
mod pre_tool_hook_tests {
    use super::{dispatch_tool_uses_tracked, execute_one_turn, TurnStepOutcome};
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        mock_message_response, MockApiClient, MockOutputStream, PermissionDecision,
        PermissionDecisionSource, PermissionGate, PermissionResolution, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
    use hooks::events::{HookEvent, HookEventType};
    use hooks::executor::BuiltinHookHandler;
    use hooks::registry::{HookContext, HookRegistry};
    use hooks::response::{HookDecision, HookOutcome, HookResponse, HookResult};
    use hooks::HookExecutorImpl;
    use protocol::{ContentBlock, ConversationMessage, HookId, MessageId, ToolUseId};
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

    // ----- unused transport/runtime stubs for the builtin-only executor -----
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

    /// Builtin `PreToolUse` handler that returns a fixed [`HookResponse`].
    struct FixedPreHook {
        response: HookResponse,
    }
    #[async_trait]
    impl BuiltinHookHandler for FixedPreHook {
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
            "fixed-pre"
        }
    }

    /// Build a `HookExecutorImpl` with a single unconditional `PreToolUse` hook
    /// that yields `response`.
    fn pre_hook_executor(response: HookResponse) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "fixed-pre".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "fixed-pre".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(FixedPreHook { response }));
        Arc::new(exec)
    }

    /// Build a `HookExecutorImpl` with a single hook registered for `event` that
    /// yields `response` (reusing the `FixedPreHook` handler, which answers any
    /// event it is invoked for). Used to register a `PermissionRequest` hook.
    fn event_hook_executor(event: HookEventType, response: HookResponse) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "fixed-evt".into(),
            events: vec![event],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "fixed-pre".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(FixedPreHook { response }));
        Arc::new(exec)
    }

    /// Builtin hook handler that COUNTS its invocations (so a test can assert a
    /// hook event fired — or did NOT fire), returning a default success response.
    struct RecordingHook {
        fired: Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait]
    impl BuiltinHookHandler for RecordingHook {
        async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
            self.fired.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: Some(HookResponse::default()),
            }
        }
        fn id(&self) -> &str {
            "recording"
        }
    }

    /// Build a `HookExecutorImpl` with a single counting hook registered for
    /// `event`; `fired` is incremented each time the hook runs.
    fn recording_executor(
        event: HookEventType,
        fired: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "recording".into(),
            events: vec![event],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "recording".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(RecordingHook { fired }));
        Arc::new(exec)
    }

    // ----- FIX 2: HookContext transcript_path + permission_mode -------------

    /// Builtin PreToolUse hook that CAPTURES the [`HookContext`] it was handed,
    /// so a test can assert the fire-site populated `transcript_path` +
    /// `permission_mode` (claude-code `createBaseHookInput` always sets
    /// `transcript_path`; PreToolUse/PostToolUse add `permission_mode`).
    struct CtxCapturingHook {
        seen: Arc<std::sync::Mutex<Option<HookContext>>>,
    }
    #[async_trait]
    impl BuiltinHookHandler for CtxCapturingHook {
        async fn handle(&self, _event: &HookEvent, ctx: &HookContext) -> HookResult {
            *self.seen.lock().unwrap() = Some(ctx.clone());
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: Some(HookResponse::default()),
            }
        }
        fn id(&self) -> &str {
            "ctx-capture"
        }
    }

    fn ctx_capturing_executor(
        seen: Arc<std::sync::Mutex<Option<HookContext>>>,
    ) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "ctx-capture".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "ctx-capture".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(CtxCapturingHook { seen }));
        Arc::new(exec)
    }

    #[tokio::test]
    async fn pre_tool_hook_ctx_carries_transcript_path_and_permission_mode() {
        // Wire a real JSONL writer so `transcript_path` is non-empty (it sources
        // the live writer's path), register a PreToolUse hook that captures the
        // context, dispatch a tool, and assert the fields are populated.
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let fs: Arc<dyn traits::FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(dir.path().to_path_buf()));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
            session_path.clone(),
            fs,
        ));

        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            ctx_capturing_executor(seen.clone()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
        .with_jsonl_writer(writer);

        let uses = vec![(ToolUseId::new(), "Echo".into(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();

        let ctx = seen.lock().unwrap().clone().expect("PreToolUse hook fired");
        assert_eq!(
            ctx.transcript_path, session_path,
            "transcript_path must be the live JSONL writer's path (claude-code createBaseHookInput)"
        );
        assert_eq!(
            ctx.permission_mode.as_deref(),
            Some("default"),
            "permission_mode must be 'default' outside plan mode (toolHooks.ts:471)"
        );
    }

    #[tokio::test]
    async fn pre_tool_hook_ctx_transcript_path_is_computed_when_no_writer() {
        // FIX A PRODUCTION PATH: with NO `JsonlWriter` wired (the real production
        // shape — every `with_jsonl_writer` call site is a test) but a `config_home`
        // set, the PreToolUse hook's `transcript_path` must be the
        // deterministically-computed `<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`
        // — claude-code `getTranscriptPathForSession`, which `createBaseHookInput`
        // ALWAYS stamps — instead of the empty string the old `unwrap_or_default()`
        // produced.
        use protocol::SessionId;

        let config_home = std::path::PathBuf::from("/home/user/.claude");
        let cwd = std::path::PathBuf::from("/Users/me/proj");
        // Pin a known session id so the expected path is deterministic.
        let session_id = SessionId::new();
        let expected = session::jsonl::path::session_path(
            &config_home,
            &cwd.to_string_lossy(),
            &session_id.as_uuid().to_string(),
        );

        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            ctx_capturing_executor(seen.clone()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            cwd.clone(),
        )
        // NOTE: deliberately NO `.with_jsonl_writer(...)` — this is the production
        // shape. Only the config home + a pinned session id are wired.
        .with_config_home(config_home.clone())
        .with_session_id(session_id);

        let uses = vec![(ToolUseId::new(), "Echo".into(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();

        let ctx = seen.lock().unwrap().clone().expect("PreToolUse hook fired");
        assert!(
            !ctx.transcript_path.as_os_str().is_empty(),
            "production transcript_path must be NON-EMPTY when a config_home is wired"
        );
        assert_eq!(
            ctx.transcript_path, expected,
            "transcript_path must be the computed <config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl \
             (claude-code getTranscriptPathForSession) when no JsonlWriter is wired"
        );
        // Correctly shaped: under <config_home>/projects and a `.jsonl` leaf named
        // by the BARE uuid (no `sess:` prefix), matching the on-disk filename.
        assert!(
            ctx.transcript_path.starts_with(config_home.join("projects")),
            "computed path must live under <config_home>/projects"
        );
        assert_eq!(
            ctx.transcript_path.file_name().and_then(|s| s.to_str()),
            Some(format!("{}.jsonl", session_id.as_uuid()).as_str()),
            "leaf must be <bare-uuid>.jsonl"
        );
    }

    #[tokio::test]
    async fn pre_tool_hook_ctx_permission_mode_is_plan_in_plan_mode() {
        // When the session is in plan mode, `permission_mode` is "plan" — the
        // faithful approximation of claude-code's permission-mode enum.
        let seen = Arc::new(std::sync::Mutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            ctx_capturing_executor(seen.clone()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        orch.session().lock().await.plan_mode = true;

        let uses = vec![(ToolUseId::new(), "Echo".into(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None)
            .await
            .unwrap();

        let ctx = seen.lock().unwrap().clone().expect("PreToolUse hook fired");
        assert_eq!(
            ctx.permission_mode.as_deref(),
            Some("plan"),
            "permission_mode must be 'plan' in plan mode"
        );
    }

    /// Permission gate that denies every tool call AT THE PROMPT (`check`), but
    /// leaves `check_after_hook_allow` at the default (Allow) — modeling a gate
    /// with NO deny RULE, only a would-be prompt. A hook 'allow' therefore skips
    /// the prompt and the tool runs (HOOK.3 issue 1: hook-allow skips the prompt).
    struct DenyAllGate;
    #[async_trait]
    impl PermissionGate for DenyAllGate {
        async fn check(&self, _tool: &str, _input: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "denied-by-gate".into(),
            }
        }
    }

    /// Permission gate modeling an explicit DENY RULE: it denies on BOTH `check`
    /// and `check_after_hook_allow`, so even a hook 'allow' cannot override it
    /// (HOOK.3 issue 1 / claude-code `checkRuleBasedPermissions`).
    struct DenyRuleGate;
    #[async_trait]
    impl PermissionGate for DenyRuleGate {
        async fn check(&self, _tool: &str, _input: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "denied-by-rule".into(),
            }
        }
        async fn check_after_hook_allow(
            &self,
            _tool: &str,
            _input: &serde_json::Value,
        ) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "denied-by-rule".into(),
            }
        }
    }

    /// Permission gate that returns a DISTINGUISHABLE denial from each entry
    /// point, so a test can assert WHICH method the turn loop routed to:
    /// `check` → "via-check", `check_after_hook_allow` → "via-hook-allow",
    /// `check_in_plan_mode` → "via-plan-mode".
    /// Gate that ALLOWS every call on every path (used by the #37 defer tests
    /// where an IGNORED defer must fall through to a gate that lets the tool
    /// run).
    struct AllowAllGate;
    #[async_trait]
    impl PermissionGate for AllowAllGate {
        async fn check(&self, _t: &str, _i: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Allow
        }
        async fn check_after_hook_allow(
            &self,
            _t: &str,
            _i: &serde_json::Value,
        ) -> PermissionDecision {
            PermissionDecision::Allow
        }
    }

    struct RouteProbeGate;
    #[async_trait]
    impl PermissionGate for RouteProbeGate {
        async fn check(&self, _t: &str, _i: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "via-check".into(),
            }
        }
        async fn check_after_hook_allow(
            &self,
            _t: &str,
            _i: &serde_json::Value,
        ) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "via-hook-allow".into(),
            }
        }
        async fn check_in_plan_mode(
            &self,
            _t: &str,
            _i: &serde_json::Value,
        ) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "via-plan-mode".into(),
            }
        }
    }

    /// Gate that is ABOUT TO ASK (`resolve_detailed` → `Ask`). Its `check` denies
    /// (models the prompt / headless auto-deny) and `check_after_hook_allow`
    /// allows (no deny rule), so a `PermissionRequest` 'allow' rescues an
    /// otherwise-denied ask, while no PermissionRequest decision delegates to the
    /// (denying) inner.
    struct AskGate;
    #[async_trait]
    impl PermissionGate for AskGate {
        async fn check(&self, _t: &str, _i: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "prompt-denied".into(),
            }
        }
        async fn check_after_hook_allow(
            &self,
            _t: &str,
            _i: &serde_json::Value,
        ) -> PermissionDecision {
            PermissionDecision::Allow
        }
        async fn resolve_detailed(
            &self,
            _t: &str,
            _i: &serde_json::Value,
        ) -> PermissionResolution {
            PermissionResolution::Ask
        }
    }

    /// Gate that denies with a configurable SOURCE from `resolve_detailed` (and
    /// denies on `check`), to assert PermissionDenied fires only on a classifier
    /// deny.
    struct SourcedDenyGate(PermissionDecisionSource);
    #[async_trait]
    impl PermissionGate for SourcedDenyGate {
        async fn check(&self, _t: &str, _i: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "sourced-deny".into(),
            }
        }
        async fn resolve_detailed(
            &self,
            _t: &str,
            _i: &serde_json::Value,
        ) -> PermissionResolution {
            PermissionResolution::Deny {
                reason: "sourced-deny".into(),
                source: self.0,
                behavior_ask: false,
                content_blocks: Vec::new(),
            }
        }
    }

    /// A tool that always succeeds with the fixed string `ECHOED-OUTPUT`.
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
                data: json!({ "content": "ECHOED-OUTPUT" }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    /// FORK (codex #5 follow-up): a tool that records the
    /// `fork_parent_system_prompt` from the `ToolUseContext` it is dispatched
    /// with, so a test can assert `dispatch_tool_uses_tracked` threads the
    /// turn's recorded system prompt onto every tool's context.
    struct CaptureSystemPromptTool {
        captured: Arc<std::sync::Mutex<Option<Option<String>>>>,
    }
    #[async_trait]
    impl Tool for CaptureSystemPromptTool {
        fn name(&self) -> &str {
            "Capture"
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
            "capture".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            *self.captured.lock().unwrap() = Some(ctx.fork_parent_system_prompt.clone());
            Ok(ToolCallResult {
                data: json!({ "content": "ok" }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    /// FORK (codex #5 follow-up): after the turn driver records the rendered
    /// system prompt via `save_current_turn_system_prompt`,
    /// `dispatch_tool_uses_tracked` must thread those exact bytes onto every
    /// tool's `ToolUseContext::fork_parent_system_prompt` (the field the fork
    /// path reads to give the child a byte-identical cache prefix).
    #[tokio::test]
    async fn dispatch_threads_recorded_system_prompt_onto_tool_ctx() {
        let captured = Arc::new(std::sync::Mutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(CaptureSystemPromptTool {
            captured: captured.clone(),
        }) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let parent_bytes = "PARENT RENDERED SYSTEM PROMPT";
        orch.save_current_turn_system_prompt(Some(parent_bytes)).await;

        let uses = vec![(ToolUseId::new(), "Capture".to_string(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None).await.unwrap();

        let got = captured.lock().unwrap().clone();
        assert_eq!(
            got,
            Some(Some(parent_bytes.to_string())),
            "tool ctx must carry the turn's recorded system prompt bytes"
        );
    }

    /// FORK (codex #5 follow-up): when no system prompt has been recorded (no
    /// successful turn yet, or a turn with no system prompt), the tool ctx
    /// carries `None` — the existing non-fork behavior is unchanged.
    #[tokio::test]
    async fn dispatch_threads_none_when_no_system_prompt_recorded() {
        let captured = Arc::new(std::sync::Mutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(CaptureSystemPromptTool {
            captured: captured.clone(),
        }) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );

        let uses = vec![(ToolUseId::new(), "Capture".to_string(), json!({}), None)];
        let _ = dispatch_tool_uses_tracked(&orch, &uses, None).await.unwrap();

        let got = captured.lock().unwrap().clone();
        assert_eq!(got, Some(None), "tool ctx must carry None with no recorded prompt");
    }

    /// SKILLEXEC.3 (Part A): a tool that succeeds AND injects a follow-up
    /// conversation message (the Skill-tool shape — `ToolCallResult.new_messages`
    /// carrying the expanded skill prompt). Mirrors `EchoTool` but with a
    /// non-empty `new_messages`.
    struct InjectingTool;
    #[async_trait]
    impl Tool for InjectingTool {
        fn name(&self) -> &str {
            "Inject"
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
            "inject".into()
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
                data: json!({
                    "content": "TOOL-RESULT",
                    "model_content": "Launching skill: demo",
                }),
                new_messages: vec![ConversationMessage::user(
                    MessageId::new(),
                    "EXPANDED-SKILL-PROMPT".into(),
                )],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    /// Build an orchestrator wired with the given hook executor + permission gate
    /// and a single `Echo` tool.
    fn orch_with(
        hooks: Arc<HookExecutorImpl>,
        perms: Arc<dyn PermissionGate>,
        responses: Vec<llm_client::LlmResponse>,
    ) -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(responses)),
            Arc::new(registry),
            hooks,
            perms,
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    fn uses() -> Vec<(ToolUseId, String, serde_json::Value, Option<String>)> {
        vec![(ToolUseId::new(), "Echo".into(), json!({}), None)]
    }

    fn tool_result(block: &ContentBlock) -> (&str, bool) {
        match block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => (content.as_str(), *is_error),
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    // ----- SKILLEXEC.3 (Part A): tool-injected new_messages -----------------

    /// A tool that returns `new_messages` has those messages threaded out of
    /// `dispatch_tool_uses_tracked` as the third tuple element (the Skill-tool
    /// expanded-prompt injection path).
    #[tokio::test]
    async fn dispatch_threads_out_tool_injected_new_messages() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(InjectingTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let skill_tu = ToolUseId::new();
        let uses = vec![(skill_tu.clone(), "Inject".to_string(), json!({}), None)];
        let (results, _prevent, injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses, None).await.unwrap();
        // The tool_result block still rides the first tuple element.
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error);
        assert_eq!(content, "Launching skill: demo");
        // The injected message is surfaced for the caller to append, PAIRED
        // with the injecting tool's `tool_use_id` (TS `sourceToolUseID`).
        assert_eq!(injected.len(), 1);
        let (injected_msg, injected_tu) = &injected[0];
        assert_eq!(
            *injected_tu, skill_tu,
            "injected message is tagged with the injecting tool's tool_use_id"
        );
        match injected_msg {
            ConversationMessage::User { content, .. } => match content.first() {
                Some(ContentBlock::Text { text }) => assert_eq!(text, "EXPANDED-SKILL-PROMPT"),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    /// SOURCE-TOOL-USE-ID parity: after a skill-style tool injects `new_messages`
    /// through a full turn step, `SessionState::injected_message_sources` maps
    /// each injected message's id → the injecting tool's `tool_use_id` (faithful
    /// port of TS `tagMessagesWithToolUseID` stamping `sourceToolUseID`).
    #[tokio::test]
    async fn injected_message_sources_records_tool_use_id() {
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Inject".into(),
                input: json!({}),
            }],
            Some("tool_use"),
        );
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(InjectingTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![api_resp])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let _ = execute_one_turn(&orch, None).await.expect("turn step");
        let s = orch.session.lock().await;
        // Find the injected expanded-skill-prompt message in history.
        let injected = s
            .history
            .iter()
            .find(|m| {
                matches!(m, ConversationMessage::User { content, .. }
                    if content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == "EXPANDED-SKILL-PROMPT")))
            })
            .expect("injected skill-prompt message present in history");
        assert_eq!(
            s.injected_message_sources.get(&injected.id()),
            Some(&tu),
            "injected message id maps to the Skill tool's tool_use_id"
        );
        assert_eq!(
            s.injected_message_sources.len(),
            1,
            "exactly one association recorded for one injected message"
        );
    }

    /// SOURCE-TOOL-USE-ID parity (negative): a normal tool that injects NO
    /// `new_messages` (e.g. `Echo`) records NOTHING in the side-table, and the
    /// in-memory association is `#[serde(skip)]` so the JSONL transcript bytes
    /// are unchanged (no `sourceToolUseID` ever written, matching TS).
    #[tokio::test]
    async fn normal_tool_records_no_source_and_serializes_no_field() {
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Echo".into(),
                input: json!({}),
            }],
            Some("tool_use"),
        );
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![api_resp],
        );
        let _ = execute_one_turn(&orch, None).await.expect("turn step");
        let s = orch.session.lock().await;
        assert!(
            s.injected_message_sources.is_empty(),
            "a tool with no injected messages records no source associations"
        );
        // The side-table is `#[serde(skip)]`: serializing the session never
        // emits a `sourceToolUseID`/`injected_message_sources` key, so the
        // persisted JSONL bytes stay byte-identical to before this change.
        let json = serde_json::to_string(&*s).expect("serialize session");
        assert!(
            !json.contains("injected_message_sources"),
            "side-table must not serialize: {json}"
        );
        assert!(
            !json.contains("sourceToolUseID"),
            "sourceToolUseID must never reach the wire: {json}"
        );
    }

    /// End-to-end through `execute_one_turn`: the injected message lands in
    /// history IMMEDIATELY AFTER this turn's tool_result user message, in order.
    #[tokio::test]
    async fn new_messages_appended_to_history_after_tool_result() {
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Inject".into(),
                input: json!({}),
            }],
            Some("tool_use"),
        );
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(InjectingTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![api_resp])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let _ = execute_one_turn(&orch, None).await.expect("turn step");
        let h = orch.session.lock().await.history.clone();
        // Locate the tool_result user message; the very next message must be the
        // injected expanded-skill-prompt user message.
        let tr_idx = h
            .iter()
            .position(|m| {
                matches!(m, ConversationMessage::User { content, .. }
                    if content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. })))
            })
            .expect("tool_result user message present");
        let injected = &h[tr_idx + 1];
        match injected {
            ConversationMessage::User { content, .. } => match content.first() {
                Some(ContentBlock::Text { text }) => assert_eq!(text, "EXPANDED-SKILL-PROMPT"),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message after tool_result, got {other:?}"),
        }
    }

    /// Byte-identical guard: a tool with EMPTY `new_messages` (every existing
    /// tool, e.g. `Echo`) threads out an empty injected vec → no extra history.
    #[tokio::test]
    async fn empty_new_messages_injects_nothing() {
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let (_results, _prevent, injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        assert!(
            injected.is_empty(),
            "Echo injects no messages → history is byte-identical to before"
        );
    }

    // ----- HOOK.1: additionalContext / systemMessage surfaced ---------------

    #[tokio::test]
    async fn hook1_additional_context_is_a_separate_message_not_folded() {
        // Parity with claude-code `toolExecution.ts:845` — a PreToolUse hook's
        // `additionalContext` is pushed as its OWN message into
        // `resultingMessages`, INDEPENDENT of the tool_result. It must NOT be
        // concatenated onto the tool_result content. The faithful message shape
        // (`messages.ts:4117-4128`) is a meta user message:
        // `<system-reminder>\nPreToolUse:{tool} hook additional context:
        // {content}\n</system-reminder>`. The hook supplies `additionalContext`
        // (the model-facing channel) — NOT `systemMessage`.
        let resp = HookResponse {
            additional_context: Some("INJECTED-CTX".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let uses = uses();
        let tool_use_id = uses[0].0.clone();
        let (results, prevent, injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses, None).await.unwrap();
        assert!(!prevent);

        // (a) the tool_result is the tool's ORIGINAL output, NO appended context.
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error, "tool ran successfully");
        assert!(content.contains("ECHOED-OUTPUT"), "tool output preserved");
        assert!(
            !content.contains("INJECTED-CTX"),
            "additionalContext must NOT be folded into the tool_result content: {content:?}"
        );

        // (b) a SEPARATE message carries the additionalContext, tagged with this
        //     tool's `tool_use_id` so it rides the existing `injected` channel
        //     (appended AFTER the tool_result by both drivers, matching the TS
        //     `resultingMessages` push order).
        assert_eq!(
            injected.len(),
            1,
            "additionalContext surfaces as one separate injected message"
        );
        let (msg, tagged_tu) = &injected[0];
        assert_eq!(*tagged_tu, tool_use_id, "tagged with the dispatching tool");
        match msg {
            ConversationMessage::User { content, .. } => match content.first() {
                Some(ContentBlock::Text { text }) => assert_eq!(
                    text,
                    "<system-reminder>\nPreToolUse:Echo hook additional context: INJECTED-CTX\n</system-reminder>"
                ),
                other => panic!("expected leading Text block, got {other:?}"),
            },
            other => panic!("expected injected User message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn hook1_system_message_does_not_reach_the_model() {
        // Parity with claude-code `messages.ts:4258` — a PreToolUse hook's
        // `systemMessage` is routed to a `hook_system_message` attachment whose
        // `normalizeAttachmentForAPI` returns `[]`: it is transcript/user-facing
        // only and NEVER reaches the model. So a hook returning ONLY
        // `systemMessage` (no `additionalContext`) must produce NO model-facing
        // additionalContext message — the `injected` channel stays empty and the
        // tool_result content is untouched.
        let resp = HookResponse {
            system_message: Some("USER-ONLY-NOTE".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let uses = uses();
        let (results, prevent, injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses, None).await.unwrap();
        assert!(!prevent);

        // (a) the tool_result is the tool's ORIGINAL output, untouched.
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error, "tool ran successfully");
        assert!(content.contains("ECHOED-OUTPUT"), "tool output preserved");
        assert!(
            !content.contains("USER-ONLY-NOTE"),
            "systemMessage must NOT leak into the tool_result content: {content:?}"
        );

        // (b) NO model-facing additionalContext message is emitted.
        assert!(
            injected.is_empty(),
            "systemMessage must NOT reach the model — no injected message expected, got {injected:?}"
        );
    }

    // ----- HOOK.2: continue:false stops the loop ----------------------------

    #[tokio::test]
    async fn hook2_prevent_continuation_flag_is_tracked() {
        let resp = HookResponse {
            prevent_continuation: true,
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let (_results, prevent, _injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        assert!(prevent, "continue:false must surface as prevent_continuation");
    }

    #[tokio::test]
    async fn hook2_prevent_continuation_ends_the_turn_step() {
        // A turn step that runs a tool whose PreToolUse hook set continue:false
        // ends with stop_reason "hook_stopped" (TS query.ts `{reason:'hook_stopped'}`).
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Echo".into(),
                input: json!({}),
            }],
            Some("tool_use"),
        );
        let resp = HookResponse {
            prevent_continuation: true,
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![api_resp],
        );
        match execute_one_turn(&orch, None).await.expect("turn step") {
            TurnStepOutcome::Ended { stop_reason, .. } => {
                assert_eq!(stop_reason, "hook_stopped");
            }
            TurnStepOutcome::Continue => panic!("expected Ended(hook_stopped), got Continue"),
        }
    }

    #[tokio::test]
    async fn hook2_no_prevent_continuation_continues() {
        // Without continue:false a tool-bearing step keeps looping (Continue).
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Echo".into(),
                input: json!({}),
            }],
            Some("tool_use"),
        );
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![api_resp],
        );
        assert!(matches!(
            execute_one_turn(&orch, None).await.expect("turn step"),
            TurnStepOutcome::Continue
        ));
    }

    // ----- HOOK.3: allow bypasses / deny denies / ask falls through ---------

    #[tokio::test]
    async fn hook3_allow_skips_the_prompt_when_no_deny_rule() {
        // permissionDecision "allow"/legacy "approve" parses to Approve and SKIPS
        // the interactive prompt (claude-code `resolveHookPermissionDecision`).
        // `DenyAllGate` would deny at the PROMPT (`check`) but has no deny RULE
        // (`check_after_hook_allow` defaults to Allow), so the hook-allow skips
        // the prompt and the tool runs.
        let resp = HookResponse {
            decision: Some(HookDecision::Approve),
            ..HookResponse::default()
        };
        let orch = orch_with(pre_hook_executor(resp), Arc::new(DenyAllGate), vec![]);
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error, "hook allow skipped the prompt; tool ran");
        assert!(content.contains("ECHOED-OUTPUT"));
        assert!(!content.contains("Permission denied"));
    }

    #[tokio::test]
    async fn hook3_allow_cannot_override_a_deny_rule() {
        // (HOOK.3 issue 1) A hook 'allow' skips the prompt but must NOT override
        // an explicit deny RULE (claude-code `checkRuleBasedPermissions`).
        // `DenyRuleGate.check_after_hook_allow` denies, so the tool is DENIED even
        // though the hook approved.
        let resp = HookResponse {
            decision: Some(HookDecision::Approve),
            ..HookResponse::default()
        };
        let orch = orch_with(pre_hook_executor(resp), Arc::new(DenyRuleGate), vec![]);
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "a deny rule must override a hook 'allow'");
        // The deny reason reaches the model VERBATIM (no "Permission denied: "
        // wrapper); this test gate emits a raw reason string.
        assert!(content.contains("denied-by-rule"));
        assert!(!content.contains("Permission denied: denied-by-rule"));
        assert!(!content.contains("ECHOED-OUTPUT"), "tool never ran");
    }

    #[tokio::test]
    async fn hook3_deny_denies_before_the_tool_runs() {
        // permissionDecision "deny"/legacy "block" parses to Block → error result.
        let resp = HookResponse {
            decision: Some(HookDecision::Block),
            reason: Some("nope".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            // allow-all gate proves the BLOCK came from the hook, not the gate.
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert!(content.contains("Hook blocked: nope"));
        assert!(!content.contains("ECHOED-OUTPUT"), "tool never ran");
    }

    #[tokio::test]
    async fn hook3_ask_falls_through_to_the_gate() {
        // No decision (the "ask"/passthrough case) leaves the gate authoritative.
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(DenyAllGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "gate denial applies when the hook makes no decision");
        // Verbatim deny reason (no "Permission denied: " wrapper).
        assert!(content.contains("denied-by-gate"));
        assert!(!content.contains("Permission denied: denied-by-gate"));
    }

    /// UNKNOWN-TOOL: when the model calls a tool name that is not in the
    /// registry, `dispatch_tool_uses_tracked` must return a `ToolResult` whose
    /// content is wrapped in `<tool_use_error>…</tool_use_error>` and whose
    /// `is_error` flag is `true` — matching claude-code byte-for-byte
    /// (`toolExecution.ts`: `"<tool_use_error>Error: No such tool available: …</tool_use_error>"`).
    #[tokio::test]
    async fn unknown_tool_returns_tool_use_error_wrapper() {
        // `orch_with` registers only `EchoTool`, so "NoSuchTool" is not in the registry.
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let uses = vec![(ToolUseId::new(), "NoSuchTool".to_string(), json!({}), None)];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses, None).await.unwrap();
        assert_eq!(results.len(), 1);
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "unknown tool must set is_error=true");
        assert_eq!(
            content,
            "<tool_use_error>Error: No such tool available: NoSuchTool</tool_use_error>",
            "content must match claude-code format byte-for-byte"
        );
    }

    /// A tool whose `call()` always returns `Err(ToolError::Internal("kaboom"))`.
    /// Used to drive the tool-execution-error path in `dispatch_tool_uses_tracked`.
    struct AlwaysFailTool;
    #[async_trait]
    impl Tool for AlwaysFailTool {
        fn name(&self) -> &str {
            "AlwaysFail"
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
            "always fails".into()
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
            Err(ToolError::Internal("kaboom".into()))
        }
    }

    /// TOOL-EXEC-ERROR (parity): when a registered tool's `call()` returns
    /// `Err(ToolError)`, `dispatch_tool_uses_tracked` must pass the error text
    /// BARE — NOT wrapped in `<tool_use_error>` — matching claude-code's
    /// `toolExecution.ts:1691`:
    ///
    ///   ```js
    ///   const content = formatError(error)   // bare string, e.g. "Error: …"
    ///   ```
    ///
    /// followed by `tool_result.content = content` (line 1721), and every
    /// per-tool `mapToolResultToToolResultBlockParam` (e.g. `NotebookEditTool.ts:137`,
    /// `BashTool.tsx:617`, `ConfigTool.ts:427`) returns raw error content.
    ///
    /// Only PRE-execution paths wrap: unknown-tool (inlined literal) and
    /// input-schema validation — NOT tool execution errors.
    ///
    /// Reference: claude-code/src/services/tools/toolExecution.ts:1691 +
    ///            claude-code/src/utils/toolErrors.ts (formatError returns bare)
    #[tokio::test]
    async fn tool_execution_error_is_bare_not_wrapped() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(AlwaysFailTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let uses = vec![(ToolUseId::new(), "AlwaysFail".to_string(), json!({}), None)];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses, None).await.unwrap();
        assert_eq!(results.len(), 1);
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "a failing tool must set is_error=true");
        // Exact content: bare "Error: kaboom" — no XML envelope AND no
        // LingXi-internal `ToolError` variant prefix. claude-code's
        // `toolExecution.ts:1691` passes `formatError(error)` (= `error.message`,
        // bare) RAW into `tool_result.content`; the model never sees an
        // `internal: `/`invalid input: ` prefix (that prefix is `Display`-only,
        // for logging). Only unknown-tool and schema-validation paths wrap.
        assert_eq!(
            content,
            "Error: kaboom",
            "tool-execution errors must be BARE (no <tool_use_error> wrapper, no variant prefix)"
        );
        assert!(
            !content.contains("<tool_use_error>"),
            "tool-execution error must NOT be wrapped in <tool_use_error>, got: {content:?}"
        );
    }

    /// A registered tool whose `validate_input` ALWAYS fails with a fixed
    /// message. Drives the new pre-execution `validate_input` gate
    /// (claude-code `toolExecution.ts:683-723`, which wraps a `validateInput`
    /// failure in `<tool_use_error>${message}</tool_use_error>`). Its `call`
    /// panics: a passing validate gate would (incorrectly) reach `call`, so the
    /// panic surfaces any regression that lets a validation failure through.
    struct ValidatingFailTool;
    #[async_trait]
    impl Tool for ValidatingFailTool {
        fn name(&self) -> &str {
            "ValidatingFail"
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
            Err(ValidationError("bad path".into()))
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
            "always-invalid".into()
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
            panic!("validate_input gate must short-circuit before call()");
        }
    }

    /// `PreToolUse` handler that flips a shared flag the instant it fires, so a
    /// test can assert whether the hook ran. Returns the default (no-op)
    /// response otherwise.
    struct SpyPreHook {
        fired: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait]
    impl BuiltinHookHandler for SpyPreHook {
        async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
            self.fired.store(true, std::sync::atomic::Ordering::SeqCst);
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: Some(HookResponse::default()),
            }
        }
        fn id(&self) -> &str {
            "spy-pre"
        }
    }

    /// Build a `HookExecutorImpl` with a single `PreToolUse` hook that sets
    /// `fired` when invoked.
    fn spy_pre_hook_executor(fired: Arc<std::sync::atomic::AtomicBool>) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "spy-pre".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "spy-pre".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(SpyPreHook { fired }));
        Arc::new(exec)
    }

    /// VALIDATE-INPUT GATE (parity): when a registered tool's `validate_input`
    /// returns `Err(ValidationError(msg))`, `dispatch_tool_uses_tracked` must
    /// return a `ToolResult` whose content is `<tool_use_error>${msg}</tool_use_error>`
    /// with `is_error = true`, and the tool's `call` must NOT run — matching
    /// claude-code `toolExecution.ts:683-723`.
    #[tokio::test]
    async fn validate_input_failure_returns_tool_use_error_wrapper() {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(ValidatingFailTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            pre_hook_executor(HookResponse::default()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let uses = vec![(ToolUseId::new(), "ValidatingFail".to_string(), json!({}), None)];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses, None).await.unwrap();
        assert_eq!(results.len(), 1);
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "validate_input failure must set is_error=true");
        assert_eq!(
            content, "<tool_use_error>bad path</tool_use_error>",
            "content must match claude-code <tool_use_error>${{message}}</tool_use_error>"
        );
    }

    /// VALIDATE-INPUT runs BEFORE the PreToolUse hook (claude-code validates at
    /// `toolExecution.ts:683` BEFORE `runPreToolUseHooks` at ~800). A tool whose
    /// `validate_input` fails must short-circuit so the registered PreToolUse
    /// hook NEVER fires.
    #[tokio::test]
    async fn validate_input_gate_runs_before_pre_tool_use_hook() {
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(ValidatingFailTool) as Arc<dyn Tool>);
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            spy_pre_hook_executor(fired.clone()),
            Arc::new(crate::test_support::NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let uses = vec![(ToolUseId::new(), "ValidatingFail".to_string(), json!({}), None)];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses, None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert_eq!(content, "<tool_use_error>bad path</tool_use_error>");
        assert!(
            !fired.load(std::sync::atomic::Ordering::SeqCst),
            "validate_input gate must run BEFORE the PreToolUse hook; the hook must not fire"
        );
    }

    // ----- HOOK.4: plan-mode dynamic gate routing --------------------------

    #[tokio::test]
    async fn hook4_plan_mode_routes_to_check_in_plan_mode() {
        // With the session in plan mode, the gate is consulted via
        // check_in_plan_mode (the dynamic Plan-mode path) — NOT the boot-mode
        // check — so a runtime EnterPlanMode activates the mutation backstop.
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(RouteProbeGate),
            vec![],
        );
        orch.session.lock().await.plan_mode = true;
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert!(
            content.contains("via-plan-mode"),
            "plan mode must route to check_in_plan_mode, got: {content}"
        );
    }

    #[tokio::test]
    async fn hook4_plan_mode_binds_over_a_hook_allow() {
        // Plan mode binds OVER a PreToolUse hook 'allow': even when a hook
        // approved the call, an active plan mode still routes through
        // check_in_plan_mode (a hook cannot push a mutation through during
        // planning — same principle as HOOK.3 issue 1's deny-rule binding).
        let resp = HookResponse {
            decision: Some(HookDecision::Approve),
            ..HookResponse::default()
        };
        let orch = orch_with(pre_hook_executor(resp), Arc::new(RouteProbeGate), vec![]);
        orch.session.lock().await.plan_mode = true;
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "plan mode binds over the hook 'allow'");
        assert!(
            content.contains("via-plan-mode"),
            "plan mode must override the hook-allow path, got: {content}"
        );
    }

    #[tokio::test]
    async fn hook4_non_plan_mode_still_routes_to_check() {
        // The default (non-plan, no-hook) path is unchanged: route to `check`.
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(RouteProbeGate),
            vec![],
        );
        // plan_mode defaults to false.
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert!(
            content.contains("via-check"),
            "non-plan mode must route to check, got: {content}"
        );
    }

    // ----- HOOK.3 issue 2: PermissionRequest on the ask path ---------------

    #[tokio::test]
    async fn hook3_issue2_permission_request_allow_rescues_an_ask() {
        // The gate is about to ASK (resolve_detailed → Ask). A PermissionRequest
        // hook 'allow' RESCUES the call (resolved via check_after_hook_allow →
        // Allow), so the tool runs — the headless rescue claude-code provides.
        let resp = HookResponse {
            decision: Some(HookDecision::Approve),
            ..HookResponse::default()
        };
        let orch = orch_with(
            event_hook_executor(HookEventType::PermissionRequest, resp),
            Arc::new(AskGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(
            !is_error,
            "PermissionRequest 'allow' rescued the ask; tool ran: {content}"
        );
        assert!(content.contains("ECHOED-OUTPUT"));
    }

    #[tokio::test]
    async fn hook3_issue2_permission_request_deny_denies_an_ask() {
        // A PermissionRequest hook 'deny' denies the about-to-ask call.
        let resp = HookResponse {
            decision: Some(HookDecision::Block),
            reason: Some("hook-said-no".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            event_hook_executor(HookEventType::PermissionRequest, resp),
            Arc::new(AskGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert!(
            content.contains("hook-said-no"),
            "PermissionRequest 'deny' reason surfaces: {content}"
        );
    }

    // ----- #37 permissionDecision "defer" ----------------------------------

    /// A `PreToolUse` hook returning `permissionDecision: "defer"` in
    /// NON-interactive (print) mode for a SOLO tool call defers the tool: it is
    /// NOT executed (no tool_result), the turn is terminated
    /// (`prevent_continuation`), and a `hook_deferred_tool` meta message is
    /// injected carrying the faithful fields.
    #[tokio::test]
    async fn defer_in_print_mode_solo_tool_defers_and_terminates() {
        let resp = HookResponse {
            decision: Some(HookDecision::Defer),
            ..HookResponse::default()
        };
        // OrchestratorConfig::default() has interactive_permissions=false
        // (= non-interactive / print mode), and uses() is a single tool — so
        // both defer gates pass and the gated path fires.
        let orch = orch_with(
            event_hook_executor(HookEventType::PreToolUse, resp),
            Arc::new(AllowAllGate),
            vec![],
        );
        let (results, prevent, injected, _) =
            dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        assert!(
            results.is_empty(),
            "the deferred tool produces NO tool_result: {results:?}"
        );
        assert!(prevent, "defer terminates the turn (prevent_continuation)");
        // a hook_deferred_tool meta message was injected
        let joined: String = injected
            .iter()
            .map(|(m, _)| match m {
                ConversationMessage::User { content, .. } => content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
                _ => String::new(),
            })
            .collect();
        assert!(
            joined.contains("hook_deferred_tool"),
            "a hook_deferred_tool meta message must be injected: {joined}"
        );
        assert!(
            joined.contains("\"hookEvent\":\"PreToolUse\""),
            "the meta carries hookEvent=PreToolUse: {joined}"
        );
    }

    /// A `PreToolUse` `defer` in INTERACTIVE mode is IGNORED (warn) — the tool
    /// proceeds through the normal permission gate and runs.
    #[tokio::test]
    async fn defer_in_interactive_mode_is_ignored_and_tool_runs() {
        let resp = HookResponse {
            decision: Some(HookDecision::Defer),
            ..HookResponse::default()
        };
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(EchoTool) as Arc<dyn Tool>);
        let cfg = OrchestratorConfig {
            interactive_permissions: true, // interactive → defer ignored
            ..OrchestratorConfig::default()
        };
        let orch = ConversationOrchestrator::new(
            cfg,
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            event_hook_executor(HookEventType::PreToolUse, resp),
            Arc::new(AllowAllGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        );
        let (results, prevent, _, _) =
            dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        assert!(!prevent, "ignored defer does NOT terminate the turn");
        assert_eq!(results.len(), 1, "the tool ran and produced a tool_result");
        let (_, is_error) = tool_result(&results[0]);
        assert!(!is_error, "the tool ran successfully (defer ignored)");
    }

    /// A `PreToolUse` `defer` in a MULTI-tool batch is IGNORED (solo-only) — the
    /// tools proceed normally.
    #[tokio::test]
    async fn defer_in_multi_tool_batch_is_ignored() {
        let resp = HookResponse {
            decision: Some(HookDecision::Defer),
            ..HookResponse::default()
        };
        // non-interactive (default) but TWO tool_use blocks → solo-only gate
        // ignores the defer.
        let orch = orch_with(
            event_hook_executor(HookEventType::PreToolUse, resp),
            Arc::new(AllowAllGate),
            vec![],
        );
        let two = vec![
            (ToolUseId::new(), "Echo".to_string(), json!({}), None),
            (ToolUseId::new(), "Echo".to_string(), json!({}), None),
        ];
        let (results, prevent, _, _) =
            dispatch_tool_uses_tracked(&orch, &two, None).await.unwrap();
        assert!(!prevent, "multi-tool defer does NOT terminate the turn");
        assert_eq!(results.len(), 2, "both tools ran (defer ignored)");
    }

    /// #39 PostToolBatch fires ONCE after a batch of resolved tools, carrying
    /// the full batch in `tool_calls`.
    #[tokio::test]
    async fn post_tool_batch_fires_once_with_the_full_batch() {
        use std::sync::Mutex as StdMutex;
        // a capturing PostToolBatch hook recording the tool_calls count it saw.
        struct CaptureBatch {
            seen: Arc<StdMutex<Vec<usize>>>,
        }
        #[async_trait]
        impl BuiltinHookHandler for CaptureBatch {
            async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
                if let HookEvent::PostToolBatch { tool_calls } = event {
                    self.seen.lock().unwrap().push(tool_calls.len());
                }
                HookResult {
                    outcome: HookOutcome::Success,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: Some(0),
                    response: Some(HookResponse::default()),
                }
            }
            fn id(&self) -> &str {
                "capture-batch"
            }
        }
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let hook = HookDefinition {
            id: HookId::new(),
            name: "capture-batch".into(),
            events: vec![HookEventType::PostToolBatch],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "capture-batch".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(tokio::sync::RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(CaptureBatch { seen: seen.clone() }));
        let orch = orch_with(Arc::new(exec), Arc::new(AllowAllGate), vec![]);
        let two = vec![
            (ToolUseId::new(), "Echo".to_string(), json!({}), None),
            (ToolUseId::new(), "Echo".to_string(), json!({}), None),
        ];
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &two, None).await.unwrap();
        assert_eq!(results.len(), 2, "both tools ran");
        let captured = seen.lock().unwrap().clone();
        assert_eq!(
            captured,
            vec![2],
            "PostToolBatch fires exactly once with the full 2-tool batch"
        );
    }

    /// #39 PostToolBatch is a strict no-op when NO PostToolBatch hook is
    /// registered (the common path) — the batch still dispatches normally.
    #[tokio::test]
    async fn post_tool_batch_no_hook_is_noop() {
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(AllowAllGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        assert_eq!(results.len(), 1, "the tool still ran (no PostToolBatch hook)");
    }

    #[tokio::test]
    async fn hook3_issue2_ask_without_request_hook_delegates_to_inner() {
        // With no PermissionRequest hook the ask delegates to the inner transport
        // (AskGate.check denies) — the prior behavior is preserved.
        let orch = orch_with(
            pre_hook_executor(HookResponse::default()),
            Arc::new(AskGate),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error);
        assert!(
            content.contains("prompt-denied"),
            "ask delegated to the inner transport: {content}"
        );
    }

    // ----- HOOK.3 issue 3: PermissionDenied only on a classifier deny ------

    #[tokio::test]
    async fn hook3_issue3_permission_denied_fires_only_on_classifier_deny() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // A RULE deny must NOT fire PermissionDenied (claude-code fires it only on
        // an auto-mode classifier deny, toolExecution.ts:1075).
        let fired_rule = Arc::new(AtomicUsize::new(0));
        let orch = orch_with(
            recording_executor(HookEventType::PermissionDenied, fired_rule.clone()),
            Arc::new(SourcedDenyGate(PermissionDecisionSource::Rule)),
            vec![],
        );
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses(), None).await.unwrap();
        assert!(tool_result(&results[0]).1, "rule deny still denies the tool");
        assert_eq!(
            fired_rule.load(Ordering::SeqCst),
            0,
            "rule deny must NOT fire the PermissionDenied hook"
        );

        // A CLASSIFIER deny DOES fire PermissionDenied.
        let fired_cls = Arc::new(AtomicUsize::new(0));
        let orch2 = orch_with(
            recording_executor(HookEventType::PermissionDenied, fired_cls.clone()),
            Arc::new(SourcedDenyGate(PermissionDecisionSource::Classifier)),
            vec![],
        );
        let (results2, _, _, _) = dispatch_tool_uses_tracked(&orch2, &uses(), None).await.unwrap();
        assert!(tool_result(&results2[0]).1, "classifier deny denies the tool");
        assert_eq!(
            fired_cls.load(Ordering::SeqCst),
            1,
            "classifier deny MUST fire the PermissionDenied hook"
        );
    }
}

// RECOV.4: the `max_output_tokens` recovery-reset helper used by both the
// token-budget continuation and the Stop-hook blocking continuation.
#[cfg(test)]
mod recovery_state_reset_tests {
    use super::{RecoveryState, ESCALATED_MAX_TOKENS};

    /// `reset_max_output_tokens_recovery` zeroes the consecutive nudge count,
    /// drops any armed escalation override, and re-arms the 8k→64k single-shot
    /// — exactly the TS continuation reset (`query.ts:1291`/`1332`,
    /// `maxOutputTokensRecoveryCount: 0` + `maxOutputTokensOverride: undefined`).
    #[test]
    fn reset_zeroes_all_three_fields() {
        let mut s = RecoveryState {
            max_output_tokens_recovery_count: 2,
            max_output_tokens_override: Some(ESCALATED_MAX_TOKENS),
            max_output_tokens_escalated: true,
            ..Default::default()
        };
        s.reset_max_output_tokens_recovery();
        assert_eq!(s.max_output_tokens_recovery_count, 0);
        assert_eq!(s.max_output_tokens_override, None);
        assert!(!s.max_output_tokens_escalated);
    }
}
