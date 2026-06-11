//! Inner turn-by-turn loop helpers. Private to `ConversationOrchestrator`.

use crate::conversation::ConversationOrchestrator;
use crate::error::OrchestratorError;
use crate::test_support::PermissionDecision;
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

/// Byte-exact user-facing message surfaced when the prompt is too long and the
/// reactive 413 recovery (Batch 5) is exhausted. 1:1 with claude-code
/// `errors.ts` `PROMPT_TOO_LONG_ERROR_MESSAGE = 'Prompt is too long'`.
///
/// Re-exported from `model::prompt_too_long` (the orchestrator's own copy),
/// which is the authoritative source for this string in this crate.
pub(crate) use crate::model::prompt_too_long::PROMPT_TOO_LONG_ERROR_MESSAGE;

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
pub(crate) async fn execute_one_turn(
    orch: &ConversationOrchestrator,
    system: Option<&str>,
) -> Result<TurnStepOutcome, OrchestratorError> {
    // Backward-compatible shim: no recovery state → legacy disposition
    // (any non-`end_turn` stop_reason Continues). Used by the cancelable
    // REPL driver and the in-file tests. The recovery-aware drivers call
    // [`execute_one_turn_with_recovery`] with a live `RecoveryState`.
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
/// (Continue), preserving the cancelable driver's behavior.
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
    // In-Loop Compaction Batch 4: proactively snip+micro+autocompact BEFORE
    // snapshotting history for the model call, so a long conversation
    // self-compacts mid-turn (TS pre-call pipeline `query.ts:365-467`). A strict
    // no-op when no compactor is wired or the history is under threshold, so the
    // locked turn-loop fixtures are unaffected. After a proactive compact, the
    // snapshot below reads the NEW, compacted history.
    orch.maybe_compact_before_call().await;

    // Snapshot the current session history for the API call.
    let (mut history_snapshot, model) = {
        let s = orch.session.lock().await;
        (s.history.clone(), s.model.clone())
    };

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
    let response = match call_api_with_ptl_recovery(
        orch,
        system,
        &model,
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
    };

    // A3: this call's output-token count, returned to the budget loop so it can
    // accumulate `global_turn_tokens` (TS `getTurnOutputTokens()`).
    let output_tokens = response.usage.billable_tokens.output;

    // In-Loop Compaction Batch 6: snapshot the cache-safe prompt prefix now the
    // call has succeeded, so the forked autocompact summarizer can replay this
    // turn's prefix and share Anthropic's prompt cache. `session.history` here is
    // the exact message set the model saw (post any PTL truncation / reactive
    // compaction inside `call_api_with_ptl_recovery`), BEFORE the assistant reply
    // is appended below. Strict no-op when no cache-safe slot is wired.
    orch.save_cache_safe_params(system, &model).await;

    // 1.5 M6-06: record this response's usage into the wired CostTracker (if any).
    // We pass `Duration::ZERO` (the api-client adapter does not currently
    // surface per-call wall-clock duration) and `retries = 0` (retries are
    // swallowed internally). Both inaccuracies are documented in v0.7.0
    // release notes; M7 wires through real timing.
    if let Some(tracker) = orch.cost_tracker.as_ref() {
        let usage = crate::cost_wiring::llm_usage_to_cost_usage(&response.usage);
        let cache_read = response.usage.billable_tokens.cache_read;
        let cache_create = response.usage.billable_tokens.cache_write;
        let model_ref = crate::cost_wiring::model_ref_from_string(&model);
        let _cost_for_this_call = tracker
            .record_api_response_v2(
                model_ref,
                usage,
                std::time::Duration::ZERO,
                0, // retries — not yet exposed from the adapter
                cache_read,
                cache_create,
                false, // is_batch_request — M6 always false
                None,  // bus — orchestrator does not yet carry an AnalyticsBus (M7 work)
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
    orch.persist_message_to_jsonl(&assistant_msg).await;

    // 4. Emit each Text block to the output stream (whole-body in M5-02;
    //    M5-04 will switch to per-delta).
    for blk in &assistant_blocks {
        if let ContentBlock::Text { text } = blk {
            orch.output.emit_text(text).await;
        }
    }

    // 5. If there are tool_use blocks, dispatch them and feed results back.
    let tool_uses: Vec<(ToolUseId, String, serde_json::Value)> = assistant_blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some((*id, name.clone(), input.clone())),
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
            dispatch_tool_uses_tracked(orch, &tool_uses).await?;
        hook_prevent_continuation = prevent;
        // Append a fresh user message carrying the tool results.
        let user_id = MessageId::new();
        let tool_results_msg = ConversationMessage::User {
            id: user_id,
            content: tool_results,
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
                s.injected_message_sources.insert(m.id(), *tool_use_id);
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
            _ => TurnStepOutcome::Continue,
        }
    };
    Ok((outcome, output_tokens))
}

/// Outcome of [`call_api_with_ptl_recovery`]: either a successful
/// `LlmResponse`, or a signal that the prompt-too-long reactive recovery
/// (Batch 5) was exhausted and the turn should end with the byte-exact
/// [`PROMPT_TOO_LONG_ERROR_MESSAGE`].
enum PtlCallOutcome {
    /// The API call (or a retry after truncation/compaction) succeeded.
    Response(Box<LlmResponse>),
    /// The blocking-limit preempt fired, or the PTL retry budget +
    /// reactive-compact fallback were all exhausted. End the turn.
    PromptTooLong,
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
/// DIVERGENCE (documented in SPECS §"Non-byte-faithful divergences" #1): TS's
/// `reactiveCompact.tryReactiveCompact` / `contextCollapse.recoverFromOverflow`
/// multi-stage drain is absent from this checkout, so the fallback is the
/// simpler "PTL-truncate ×N → one full compact → error" tail.
///
/// `betas` for the blocking-limit window math is `&[]` (conservative): the
/// orchestrator does not currently thread the per-request beta set down to this
/// call site, and the default window is the parity 200k. Documented divergence,
/// not a frozen-surface change.
#[allow(clippy::too_many_lines)]
async fn call_api_with_ptl_recovery(
    orch: &ConversationOrchestrator,
    system: Option<&str>,
    model: &str,
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
            .messages_create_with_opts(model, system, history_snapshot, tools.clone(), max_tokens)
            .await
    } else if orch.config.fallback_model.is_some() {
        orch.api
            .messages_create_with_fallback(
                model,
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
            .messages_create(model, system, history_snapshot, tools.clone())
            .await
    };
    // NOTE: `ApiError::FallbackTriggered` interception is REMOVED — `LlmError`
    // has no `FallbackTriggered` variant. The model-fallback logic moves into
    // `ProviderApiAdapter` in Task 6 (the adapter handles the 529 switch
    // internally and emits a `warning` on the output stream there).

    // Map `LlmError::ContextOverflow` to the PTL recovery path;
    // a non-zero `token_gap` is unknown at this level — use 0 as the sentinel
    // (the PTL truncation loop is best-effort without an exact gap).
    let token_gap: u64 = match first {
        Ok(resp) => return Ok(PtlCallOutcome::Response(Box::new(resp))),
        Err(LlmError::ContextOverflow) => 0,
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
            .messages_create(model, system, truncated, tools.clone())
            .await
        {
            Ok(resp) => return Ok(PtlCallOutcome::Response(Box::new(resp))),
            Err(LlmError::ContextOverflow) => {
                // token_gap stays 0 — truncation keeps halving the history.
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
                    .messages_create(model, system, history, tools)
                    .await
                {
                    Ok(resp) => return Ok(PtlCallOutcome::Response(Box::new(resp))),
                    Err(LlmError::ContextOverflow) => {}
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
    orch.persist_message_to_jsonl(&assistant_msg).await;
    orch.output.emit_text(PROMPT_TOO_LONG_ERROR_MESSAGE).await;
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

    // Recovery exhausted — surface the cap by ending the turn.
    Ok(TurnStepOutcome::Ended {
        final_message_id: assistant_id,
        stop_reason: "max_tokens".to_string(),
    })
}

/// Translate llm-client content blocks into protocol content blocks.
/// Server-side variants (`ServerToolUse`, `ConnectorText`, `AdvisorToolResult`)
/// are dropped. `ToolCall.id: String` is converted to `ToolUseId`: the string
/// is interpreted as a JSON string and deserialized via `ToolUseId`'s
/// `#[serde(transparent)]` UUID impl; if it fails a fresh UUID is minted to
/// keep history coherent.
fn translate_response_blocks(content: &[LlmContentBlock]) -> Vec<ContentBlock> {
    use protocol::ToolUseId;
    content
        .iter()
        .filter_map(|b| match b {
            LlmContentBlock::Text { text, .. } => Some(ContentBlock::Text { text: text.clone() }),
            LlmContentBlock::ToolCall { id, name, input } => {
                // `llm_client::ContentBlock::ToolCall.id` is a plain `String`;
                // `protocol::ContentBlock::ToolUse.id` is a `ToolUseId` (newtype
                // wrapping a UUID, serde transparent). Try to round-trip via JSON;
                // if the string is not a UUID (e.g. Anthropic `toolu_...`) mint a fresh
                // UUID so history stays coherent (Task 6 will preserve the Anthropic id
                // separately in provider_metadata).
                let tool_use_id = serde_json::from_value::<ToolUseId>(
                    serde_json::Value::String(id.clone()),
                )
                .unwrap_or_else(|_| ToolUseId::new());
                Some(ContentBlock::ToolUse {
                    id: tool_use_id,
                    name: name.clone(),
                    input: input.clone(),
                })
            }
            LlmContentBlock::Reasoning { text, signature } => Some(ContentBlock::Thinking {
                thinking: text.clone(),
                signature: signature.clone(),
            }),
            // Server-side variants are dropped (matches agent::runner::translate_response_blocks).
            LlmContentBlock::ServerToolUse { .. }
            | LlmContentBlock::ConnectorText { .. }
            | LlmContentBlock::AdvisorToolResult { .. }
            | LlmContentBlock::Image { .. }
            | LlmContentBlock::ImageUrl { .. }
            | LlmContentBlock::Document { .. }
            | LlmContentBlock::ToolResult { .. }
            | LlmContentBlock::RedactedThinking { .. } => None,
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
/// ([`crate::streaming_loop::dispatch_tool_uses_concurrent`]) now calls
/// `dispatch_tool_uses_tracked` directly, so it DOES replay tool-injected
/// `new_messages` (the Skill tool's expanded prompt) into history after the
/// `tool_result` — mirroring the batched [`execute_one_turn`] path (SKILLEXEC.3).
#[cfg(test)]
pub(crate) async fn dispatch_tool_uses(
    orch: &ConversationOrchestrator,
    tool_uses: &[(ToolUseId, String, serde_json::Value)],
) -> Result<Vec<ContentBlock>, OrchestratorError> {
    Ok(dispatch_tool_uses_tracked(orch, tool_uses).await?.0)
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
    tool_uses: &[(ToolUseId, String, serde_json::Value)],
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
    for (tool_use_id, name, input) in tool_uses {
        orch.output.emit_tool_call(tool_use_id, name, input).await;

        // M5-06 Task 14: PreToolUse hook chain. Build the event + context,
        // call the executor, and either Block (turn the response into an
        // error ToolResult), apply modified_input, or continue.
        let session_id = { orch.session.lock().await.session_id };
        let hook_ctx = HookContext {
            session_id,
            cwd: orch.cwd.clone(),
            ..Default::default()
        };
        let pre_event = HookEvent::PreToolUse {
            tool_name: name.clone(),
            tool_input: input.clone(),
            tool_use_id: *tool_use_id,
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
        // HOOK.1: a PreToolUse hook's `hookSpecificOutput.additionalContext` and
        // `systemMessage` (both folded into `system_messages` by the response
        // parser + executor merge, mirroring TS `result.{additionalContext,
        // systemMessage}`). TS injects each as a message into the conversation
        // the model sees; here they are folded into THIS tool's model-facing
        // tool-result content — the same mechanism the PostToolUse arm uses
        // below, which keeps the dispatch's one-block-per-tool contract intact
        // for the streaming concurrent path. Captured before any early `continue`
        // so the context surfaces even on a block / permission denial.
        let pre_hook_messages = pre_agg.system_messages.clone();
        // Fold the captured PreToolUse context into a tool-result content
        // string (HOOK.1). Mirrors the PostToolUse fold: each message on its
        // own line, appended after `base`. A strict no-op when empty, so the
        // locked turn-loop fixtures (noop hooks) are unaffected.
        let fold_pre_context = |base: String| -> String {
            let mut out = base;
            for msg in &pre_hook_messages {
                out.push('\n');
                out.push_str(msg);
            }
            out
        };

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
                tool_use_id: *tool_use_id,
                content: fold_pre_context(format!("Hook blocked: {reason}")),
                is_error: true,
            };
            orch.output
                .emit_tool_result(
                    tool_use_id,
                    name,
                    &serde_json::json!({ "error": format!("Hook blocked: {reason}") }),
                )
                .await;
            results.push(result_block);
            continue;
        }

        // Apply modified_input if any hook mutated the tool input.
        let effective_input = pre_agg
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
        // DOCUMENTED bounded divergence: TS still applies rule-based deny/ask
        // (`checkRuleBasedPermissions`) on top of a hook 'allow'; this port's
        // permission seam ([`orch.perms`]) is a single allow/deny gate with no
        // rule/prompt split to layer underneath, so a hook 'allow' bypasses it
        // wholesale.
        let hook_allowed = matches!(
            pre_agg.decision,
            Some(HookDecision::Approve | HookDecision::Allow)
        );
        // Permission gate. Use the post-hook effective_input so a Pre
        // hook can rewrite a tool argument before the permission check
        // sees it.
        if !hook_allowed {
            // PermissionRequest hook (parity with claude-code
            // `executePermissionRequestHooks`, `utils/hooks.ts:4157-4192`, fired
            // from the permission seam `permissions.ts:409`). claude-code fires
            // it when a tool call needs its permission RESOLVED (the engine is
            // "about to ask the user / auto-policy for permission"). The LingXi
            // permission seam ([`orch.perms`]) IS that single allow/deny
            // resolution step — it has no separate interactive "ask" branch — so
            // we fire `PermissionRequest` immediately BEFORE consulting the gate,
            // the faithful chokepoint where permission is about to be asked.
            // Best-effort / observe-only here: the gate's allow/deny verdict
            // governs the outcome (the LingXi `perms` seam carries no hook-return
            // override path), exactly as the gate did before this fire. Strict
            // no-op when no `PermissionRequest` hook is registered, like the
            // PostToolUse / SubagentStop arms. `reason` mirrors the engine-
            // supplied prompt rationale; the LingXi gate does not expose a
            // pre-decision rationale, so we carry the canonical "tool requires
            // permission" string.
            let req_event = HookEvent::PermissionRequest {
                tool_name: name.clone(),
                tool_input: effective_input.clone(),
                reason: format!("Tool {name} requires permission"),
            };
            let _req_agg = orch.hooks.execute(req_event, hook_ctx.clone()).await;

            match orch.perms.check(name, &effective_input).await {
                PermissionDecision::Allow => {}
                PermissionDecision::Deny { reason } => {
                    // PermissionDenied hook (parity with claude-code
                    // `executePermissionDeniedHooks`, `utils/hooks.ts:3529-3559`,
                    // fired from `toolExecution.ts:1081` when a permission
                    // decision denies a tool call). Fires at the gate's deny
                    // chokepoint, BEFORE the error `tool_result` is pushed, so a
                    // registered hook observes every denial. Best-effort /
                    // observe-only: the LingXi `perms` seam has no
                    // hook-driven `retry` re-resolution path, so the denial
                    // stands regardless of the hook's reply (the TS `{retry:true}`
                    // re-prompt rides on its interactive permission loop, which
                    // this single allow/deny seam does not have). Strict no-op
                    // when no `PermissionDenied` hook is registered.
                    let denied_event = HookEvent::PermissionDenied {
                        tool_name: name.clone(),
                        tool_input: effective_input.clone(),
                        tool_use_id: *tool_use_id,
                        reason: reason.clone(),
                    };
                    let _denied_agg = orch.hooks.execute(denied_event, hook_ctx.clone()).await;

                    let result_block = ContentBlock::ToolResult {
                        tool_use_id: *tool_use_id,
                        content: fold_pre_context(format!("Permission denied: {reason}")),
                        is_error: true,
                    };
                    orch.output
                        .emit_tool_result(
                            tool_use_id,
                            name,
                            &serde_json::json!({ "error": format!("Permission denied: {reason}") }),
                        )
                        .await;
                    results.push(result_block);
                    continue;
                }
            }
        }

        // Dispatch through ToolRegistry.
        let Some(tool_handle) = orch.tools.find_by_name(name) else {
            let result_block = ContentBlock::ToolResult {
                tool_use_id: *tool_use_id,
                content: fold_pre_context(format!("Error: tool not found: {name}")),
                is_error: true,
            };
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

        // Synthesize a minimal ToolUseContext.
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
                is_non_interactive_session: true,
                custom_system_prompt: orch.config.system_prompt_override.clone(),
                append_system_prompt: None,
            },
            messages,
            tool_use_id: Some(*tool_use_id),
            agent_id: None,
            content_replacement_state: None,
            session: Some(orch.session.clone()),
            subagent_registry: Some(orch.tools.clone()),
        };

        // SubagentStart hook (parity with claude-code `executeSubagentStartHooks`,
        // `utils/hooks.ts:3932-3952`, fired from `runAgent.ts:532` just before a
        // subagent begins). claude-code fires it at the START of a subagent's
        // run, the counterpart to the `SubagentStop` fired when it ends. The
        // LingXi port spawns subagents only through the registered,
        // turn_loop-dispatched `Agent` (legacy alias `Task`) tool, so the start
        // of that tool's dispatch IS the subagent spawn — we fire it immediately
        // BEFORE `tool_handle.call()`, after the pre-hook + permission gate have
        // cleared (a blocked / denied call `continue`s above, so no subagent
        // spawns and no SubagentStart fires — exactly like the SubagentStop arm).
        // It carries the dispatched `subagent_type` on the hook context's
        // `agent_type` (claude-code's `agentType`, also the `matchQuery`) and a
        // fresh `agent_id` for the wire payload's required field — the Agent tool
        // discards the child's pool id across the frozen `SubagentSpawner` seam
        // (same documented limitation as the SubagentStop arm). Best-effort:
        // `orch.hooks.execute` is a strict no-op when no `SubagentStart` hook is
        // registered, and a failing hook never breaks the spawn.
        if name == AGENT_TOOL_NAME || name == LEGACY_AGENT_TOOL_NAME {
            let subagent_type = effective_input
                .get("subagent_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let start_event = HookEvent::SubagentStart {
                agent_id: protocol::AgentId::new(),
                agent_type: subagent_type.clone(),
                parent_agent_id: None,
            };
            let start_ctx = HookContext {
                agent_type: Some(subagent_type),
                ..hook_ctx.clone()
            };
            let _start_agg = orch.hooks.execute(start_event, start_ctx).await;
        }

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
                    .extend(result.new_messages.into_iter().map(|m| (m, *tool_use_id)));
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
                let text = format!("Error: {err}");
                (text, true, serde_json::json!({ "error": format!("{err}") }))
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
                tool_use_id: *tool_use_id,
            }
        } else {
            HookEvent::PostToolUse {
                tool_name: name.clone(),
                tool_input: effective_input.clone(),
                tool_output: emit_payload.clone(),
                tool_use_id: *tool_use_id,
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

        // PostToolUse `updatedMCPToolOutput`: a PostToolUse hook may REPLACE the
        // tool's output (claude-code `parseHookJSONOutput`,
        // `utils/hooks.ts:646-649`). The replacement is applied ONLY for MCP
        // tools, mirroring TS's `isMcpTool(tool)` gate (`toolHooks.ts:146` /
        // `toolExecution.ts:1494-1496`): a non-MCP tool's result is left
        // untouched even if a hook returns the field. Only a SUCCESSFUL result
        // is mutated — the `PostToolUseFailure` arm carries no
        // `updatedMCPToolOutput` in the TS schema, and `post_agg
        // .updated_mcp_tool_output` is only ever set by a `PostToolUse` (success)
        // dispatch (the failure arm fires `PostToolUseFailure`, whose parser
        // never reads the field). When applied, the replacement JSON re-derives
        // the model-facing text via `tool_result_to_model_text`, exactly as the
        // original output did, so the model sees the mutated output. A strict
        // no-op when no hook set the field (the common case) → byte-identical.
        let (content, mcp_output_mutated) = match (
            is_error,
            tool_handle.is_mcp(),
            post_agg.updated_mcp_tool_output.as_ref(),
        ) {
            (false, true, Some(new_output)) => {
                (tool_result_to_model_text(new_output), true)
            }
            _ => (content, false),
        };

        // HOOK.1 + PostToolUse fold: the model-facing tool-result content carries
        // both this dispatch's PreToolUse `additionalContext`/`systemMessage`
        // (`pre_hook_messages`, folded via `fold_pre_context`) and the PostToolUse
        // hooks' `system_messages` — each on its own line. A strict no-op when
        // both are empty, so the result text is byte-identical to before for the
        // locked turn-loop fixtures (noop hooks).
        let mutated = !pre_hook_messages.is_empty()
            || !post_agg.system_messages.is_empty()
            || mcp_output_mutated;
        let final_content = if mutated {
            let mut out = fold_pre_context(content);
            for msg in &post_agg.system_messages {
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

        // SubagentStop hook (parity with claude-code's `executeStopHooks(…,
        // subagentId, …)` → `hook_event_name: 'SubagentStop'`,
        // `utils/hooks.ts:3653-3678`). claude-code fires it from the unified
        // turn-loop stop chokepoint (`runStopHooks`/`stopHooks.ts`) when a
        // subagent's query loop ENDS — keyed on `toolUseContext.agentId` being
        // set. The LingXi port spawns subagents only through the registered,
        // turn_loop-dispatched `Agent` (legacy alias `Task`) tool: a completed
        // `spawner.spawn()` MEANS the subagent's loop has stopped. So we fire it
        // here at the spawn-completion site — same TIMING (subagent stopped),
        // the fire just lives in this orchestrator-side dispatch chokepoint
        // (alongside `PostToolUse`/`WorktreeCreate`) where `orch.hooks` is
        // reachable, rather than inside the child runner (which has no hook
        // seam). The documented minor divergence: it fires at spawn-completion
        // vs. inside the subagent loop — identical observable timing.
        //
        // Fires on BOTH a successful AND a failed/killed dispatch: the subagent
        // always STOPS (claude-code's stop chokepoint runs at the loop's natural
        // end regardless of outcome). It does NOT fire on a pre-hook Block or a
        // permission denial — those `continue` above before any spawn, so no
        // subagent ever ran. Best-effort: `orch.hooks.execute` is a strict
        // no-op when no `SubagentStop` hook is registered, and a failing hook
        // never breaks the turn (mirroring the `PostToolUse`/`WorktreeCreate`
        // arms).
        //
        // Wire payload: the executor's `SubagentStop` arm builds the byte-
        // faithful `SubagentStopHookInput` (`hook_payload.rs` /
        // `executor.rs:698`). `agent_type` rides on the hook context (the
        // dispatched `subagent_type`, claude-code's `agentType`); `agent_id` is
        // the spawn-site `AgentId` (the orchestrator does not receive the
        // child's pool id back — see note below). `status` is engine-side
        // metadata (claude-code's `SubagentStop` wire schema has no status
        // field, mirroring the `Stop` schema it derives from).
        if name == AGENT_TOOL_NAME || name == LEGACY_AGENT_TOOL_NAME {
            let subagent_type = effective_input
                .get("subagent_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let status = if is_error { "failed" } else { "completed" };
            let sa_event = HookEvent::SubagentStop {
                // The Agent tool discards the child's pool `AgentId`
                // (`SubagentResult` carries no id back across the frozen
                // `SubagentSpawner` seam), so the orchestrator mints a fresh id
                // for the wire payload's `agent_id` — the field is required by
                // the claude-code schema but is not asserted against any locked
                // fixture. (Threading the real child id would require a frozen
                // `traits/` change to `SubagentResult`; reported as a follow-up.)
                agent_id: protocol::AgentId::new(),
                status: status.to_string(),
            };
            // Carry the dispatched `subagent_type` as the hook context's
            // `agent_type` so the wire payload's `agent_type` is faithful
            // (claude-code passes the subagent's `agentType` into
            // `executeStopHooks`). The session_id / cwd reuse the same context
            // the pre/post hooks used.
            let sa_ctx = HookContext {
                agent_type: Some(subagent_type),
                ..hook_ctx.clone()
            };
            let sa_started = std::time::Instant::now();
            let _sa_agg = orch.hooks.execute(sa_event, sa_ctx).await;
            // hook duration bounded by tokio timeout — u128 ms cannot exceed u64::MAX
            #[allow(clippy::cast_possible_truncation)]
            let sa_dur_ms = sa_started.elapsed().as_millis() as u64;
            // No `tengu_*` analytic here: claude-code's subagent-stop path emits
            // no orchestrator-lifecycle event, so we keep parity by logging only.
            tracing::debug!(
                tool_name = %name,
                status,
                duration_ms = sa_dur_ms,
                "fired SubagentStop hook after Agent/Task tool completed",
            );
        }

        results.push(ContentBlock::ToolResult {
            tool_use_id: *tool_use_id,
            content: final_content,
            is_error,
        });
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
        let uses = vec![(ToolUseId::new(), name.to_string(), input)];
        dispatch_tool_uses(orch, &uses).await.expect("dispatch");
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
        // appended on exhaustion (only the assistant message from this step).
        assert_eq!(state.max_output_tokens_recovery_count, MAX_OUTPUT_TOKENS_RECOVERY_LIMIT);
        let h = history(&orch).await;
        assert_eq!(h.len(), len_before + 1, "only the assistant msg, no nudge");
        assert!(matches!(h.last(), Some(ConversationMessage::Assistant { .. })));
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

/// HOOK.1 / HOOK.2 / HOOK.3 — `PreToolUse` hook behaviors surfaced by the turn
/// loop's `dispatch_tool_uses` chokepoint (TS `services/tools/toolExecution.ts`
/// + `toolHooks.ts` + `query.ts:1518-1521`).
#[cfg(test)]
mod pre_tool_hook_tests {
    use super::{dispatch_tool_uses_tracked, execute_one_turn, TurnStepOutcome};
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        mock_message_response, MockApiClient, MockOutputStream, PermissionDecision, PermissionGate,
        StaticMemoryProvider,
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

    /// Permission gate that denies every tool call.
    struct DenyAllGate;
    #[async_trait]
    impl PermissionGate for DenyAllGate {
        async fn check(&self, _tool: &str, _input: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny {
                reason: "denied-by-gate".into(),
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

    fn uses() -> Vec<(ToolUseId, String, serde_json::Value)> {
        vec![(ToolUseId::new(), "Echo".into(), json!({}))]
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
        let uses = vec![(skill_tu, "Inject".to_string(), json!({}))];
        let (results, _prevent, injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses).await.unwrap();
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
                id: tu.as_uuid().to_string(),
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
                id: tu.as_uuid().to_string(),
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
                id: tu.as_uuid().to_string(),
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
            dispatch_tool_uses_tracked(&orch, &uses()).await.unwrap();
        assert!(
            injected.is_empty(),
            "Echo injects no messages → history is byte-identical to before"
        );
    }

    // ----- HOOK.1: additionalContext / systemMessage surfaced ---------------

    #[tokio::test]
    async fn hook1_additional_context_is_surfaced_into_tool_result() {
        // The parser folds `additionalContext` + `systemMessage` into
        // `system_messages`; the turn loop must surface them to the model.
        let resp = HookResponse {
            system_message: Some("INJECTED-CTX".into()),
            ..HookResponse::default()
        };
        let orch = orch_with(
            pre_hook_executor(resp),
            Arc::new(crate::test_support::NoOpPermissionGate),
            vec![],
        );
        let (results, prevent, _injected, _mods) =
            dispatch_tool_uses_tracked(&orch, &uses()).await.unwrap();
        assert!(!prevent);
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error, "tool ran successfully");
        assert!(content.contains("ECHOED-OUTPUT"), "tool output preserved");
        assert!(
            content.contains("INJECTED-CTX"),
            "PreToolUse additionalContext/systemMessage surfaced into the model-facing result: {content:?}"
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
            dispatch_tool_uses_tracked(&orch, &uses()).await.unwrap();
        assert!(prevent, "continue:false must surface as prevent_continuation");
    }

    #[tokio::test]
    async fn hook2_prevent_continuation_ends_the_turn_step() {
        // A turn step that runs a tool whose PreToolUse hook set continue:false
        // ends with stop_reason "hook_stopped" (TS query.ts `{reason:'hook_stopped'}`).
        let tu = ToolUseId::new();
        let api_resp = mock_message_response(
            vec![llm_client::ContentBlock::ToolCall {
                id: tu.as_uuid().to_string(),
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
                id: tu.as_uuid().to_string(),
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
    async fn hook3_allow_bypasses_permission_gate() {
        // permissionDecision "allow"/legacy "approve" parses to Approve and must
        // bypass the (here deny-everything) permission gate.
        let resp = HookResponse {
            decision: Some(HookDecision::Approve),
            ..HookResponse::default()
        };
        let orch = orch_with(pre_hook_executor(resp), Arc::new(DenyAllGate), vec![]);
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses()).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(!is_error, "hook allow bypassed the deny gate; tool ran");
        assert!(content.contains("ECHOED-OUTPUT"));
        assert!(!content.contains("Permission denied"));
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
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses()).await.unwrap();
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
        let (results, _, _, _) = dispatch_tool_uses_tracked(&orch, &uses()).await.unwrap();
        let (content, is_error) = tool_result(&results[0]);
        assert!(is_error, "gate denial applies when the hook makes no decision");
        assert!(content.contains("Permission denied: denied-by-gate"));
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
        };
        s.reset_max_output_tokens_recovery();
        assert_eq!(s.max_output_tokens_recovery_count, 0);
        assert_eq!(s.max_output_tokens_override, None);
        assert!(!s.max_output_tokens_escalated);
    }
}
