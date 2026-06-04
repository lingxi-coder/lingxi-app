//! Inner turn-by-turn loop helpers. Private to `ConversationOrchestrator`.

use crate::conversation::ConversationOrchestrator;
use crate::error::OrchestratorError;
use crate::test_support::PermissionDecision;
use api_client::types::ContentBlockApi;
use hooks::events::HookEvent;
use hooks::registry::HookContext;
use hooks::response::HookDecision;
use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
use std::path::{Component, Path, PathBuf};
use telemetry::tengu::orchestrator as orch_events;
use tool_api::context::{ToolUseContext, ToolUseOptions};

/// Tools whose successful execution records `file_path` (or `notebook_path`)
/// into the orchestrator's read-file-state cache. Mirrors the TS sites that
/// call `readFileState.set(expandPath(file_path), …)` (`FileReadTool` +
/// `FileEditTool`/`FileWriteTool`/`MultiEditTool`/`NotebookEditTool`).
/// `/files` then renders this set (TS `cacheKeys(context.readFileState)`).
const READ_FILE_STATE_TOOLS: &[&str] =
    &["Read", "Edit", "Write", "MultiEdit", "NotebookEdit"];

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

/// Per-conversation recovery bookkeeping carried by the turn drivers in
/// `conversation.rs` and threaded `&mut` into [`execute_one_turn_with_recovery`].
///
/// Mirrors the TS recovery sub-state on `query.ts`'s loop `State`
/// (`maxOutputTokensRecoveryCount`, `maxOutputTokensOverride`). One instance
/// lives per `try_run_turn` / `try_run_turn_streaming` invocation; it persists
/// the nudge count ACROSS turn-steps so the 3-retry limit is consecutive.
#[derive(Debug, Default)]
pub(crate) struct RecoveryState {
    /// How many consecutive `max_tokens` nudges have been injected this
    /// conversation. Capped at [`MAX_OUTPUT_TOKENS_RECOVERY_LIMIT`]; once it
    /// reaches the limit the next `max_tokens` ends the turn.
    pub(crate) max_output_tokens_recovery_count: u32,
    /// When `Some(n)`, the next API call should use `n` as its output-token
    /// cap (the escalated retry). DEFERRED: never set today because the
    /// escalation is not wired into the api-client call (see
    /// [`ESCALATED_MAX_TOKENS`]).
    pub(crate) max_output_tokens_override: Option<u32>,
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
    // Snapshot the current session history for the API call.
    let (history_snapshot, model) = {
        let s = orch.session.lock().await;
        (s.history.clone(), s.model.clone())
    };

    // 1. Call the API. Advertise the registry's wire tool definitions
    //    (same set + serialization as the streaming path).
    let tools = orch.build_wire_tools().await;
    let response = orch
        .api
        .messages_create(&model, system, history_snapshot, tools)
        .await?;

    // 1.5 M6-06: record this response's usage into the wired CostTracker (if any).
    // We pass `Duration::ZERO` (the api-client adapter does not currently
    // surface per-call wall-clock duration) and `retries = 0` (retries are
    // swallowed internally). Both inaccuracies are documented in v0.7.0
    // release notes; M7 wires through real timing.
    if let Some(tracker) = orch.cost_tracker.as_ref() {
        let usage = crate::cost_wiring::usage_api_to_cost_usage(&response.usage);
        let cache_read = response.usage.cache_read_input_tokens;
        let cache_create = response.usage.cache_creation_input_tokens;
        let model_ref = crate::cost_wiring::model_ref_from_string(&model);
        let _cost_for_this_call = tracker
            .record_api_response_v2(
                model_ref,
                usage,
                std::time::Duration::ZERO,
                0, // retries — not exposed from api-client adapter today
                cache_read,
                cache_create,
                false, // is_batch_request — M6 always false
                None,  // bus — orchestrator does not yet carry an AnalyticsBus (M7 work)
            )
            .await;
        orch.api_calls_recorded
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    // 2. Translate `MessageResponse.content` -> `ContentBlock` history entry.
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

    if !tool_uses.is_empty() {
        let tool_results = dispatch_tool_uses(orch, &tool_uses).await?;
        // Append a fresh user message carrying the tool results.
        let user_id = MessageId::new();
        let tool_results_msg = ConversationMessage::User {
            id: user_id,
            content: tool_results,
        };
        {
            let mut s = orch.session.lock().await;
            s.history.push(tool_results_msg.clone());
        }
        // M5-07 T13: persist the tool_result user message. Best-effort.
        orch.persist_message_to_jsonl(&tool_results_msg).await;
    }

    // 6. Decide loop disposition.
    match response.stop_reason.as_deref() {
        Some("end_turn") => Ok(TurnStepOutcome::Ended {
            final_message_id: assistant_id,
            stop_reason: "end_turn".to_string(),
        }),
        // A1: max_output_tokens recovery (TS `query.ts:1223-1255`). Only the
        // recovery-aware drivers (`Some(state)`) participate; the legacy shim
        // (`None`) falls through to Continue, unchanged.
        Some("max_tokens") if recovery.is_some() => {
            // `recovery.is_some()` guarded above — unwrap is infallible.
            let state = recovery.expect("recovery is Some");
            handle_max_output_tokens(orch, assistant_id, state).await
        }
        _ => Ok(TurnStepOutcome::Continue),
    }
}

/// A1 `max_tokens` recovery decision (TS `query.ts:1223-1255`).
///
/// While `count < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT`: append the byte-exact
/// meta nudge user message to history, increment the counter, and Continue.
/// On exhaustion (count has reached the limit): end the turn with
/// `stop_reason = "max_tokens"` (current behavior — surface the cap).
///
/// The 8k→64k escalation (TS `query.ts:1199-1221`) is DEFERRED: it requires an
/// api-client `max_tokens` override the current signature lacks, so even with
/// [`crate::OrchestratorConfig::escalate_max_output_tokens`] enabled this code
/// goes straight to the multi-turn nudge. See [`ESCALATED_MAX_TOKENS`].
async fn handle_max_output_tokens(
    orch: &ConversationOrchestrator,
    assistant_id: MessageId,
    state: &mut RecoveryState,
) -> Result<TurnStepOutcome, OrchestratorError> {
    // Escalation (8k→64k) — DEFERRED. TS performs this single-shot retry
    // (`query.ts:1199-1221`) BEFORE the multi-turn nudge, gated by
    // `tengu_otk_slot_v1` and "no override already applied". The port keeps
    // the gate (`escalate_max_output_tokens`) and the target cap
    // ([`ESCALATED_MAX_TOKENS`]) wired, but the api-client `messages_create`
    // signature carries no `max_tokens` override argument, so the escalation
    // cannot be performed crate-locally. We therefore record the intended
    // override (so a follow-up that plumbs the api-client arg can act on it)
    // and fall through to the multi-turn nudge regardless.
    if orch.config.escalate_max_output_tokens && state.max_output_tokens_override.is_none() {
        // NOTE: setting this does not change the API call today (deferred); it
        // only documents the intended escalation target for the follow-up.
        state.max_output_tokens_override = Some(ESCALATED_MAX_TOKENS);
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

/// Translate api-client content blocks into protocol content blocks.
/// Server-side variants (`ServerToolUse`, `ConnectorText`, `AdvisorToolResult`)
/// are dropped in M5-02. M5-04 may revisit Thinking.
fn translate_response_blocks(content: &[ContentBlockApi]) -> Vec<ContentBlock> {
    content
        .iter()
        .filter_map(|b| match b {
            ContentBlockApi::Text { text } => Some(ContentBlock::Text { text: text.clone() }),
            ContentBlockApi::ToolUse { id, name, input } => Some(ContentBlock::ToolUse {
                id: *id,
                name: name.clone(),
                input: input.clone(),
            }),
            ContentBlockApi::Thinking {
                thinking,
                signature,
            } => Some(ContentBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            }),
            // Server-side variants are skipped in M5-02; M5-04 may revisit.
            ContentBlockApi::ServerToolUse { .. }
            | ContentBlockApi::ConnectorText { .. }
            | ContentBlockApi::AdvisorToolResult { .. } => None,
        })
        .collect()
}

/// Dispatch each `tool_use` block through hooks -> permission -> registry ->
/// hooks. Returns a list of `ContentBlock::ToolResult` blocks for the
/// next user message.
#[allow(clippy::too_many_lines)]
pub(crate) async fn dispatch_tool_uses(
    orch: &ConversationOrchestrator,
    tool_uses: &[(ToolUseId, String, serde_json::Value)],
) -> Result<Vec<ContentBlock>, OrchestratorError> {
    let mut results = Vec::with_capacity(tool_uses.len());
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
                content: format!("Hook blocked: {reason}"),
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

        // Permission gate. Use the post-hook effective_input so a Pre
        // hook can rewrite a tool argument before the permission check
        // sees it.
        match orch.perms.check(name, &effective_input).await {
            PermissionDecision::Allow => {}
            PermissionDecision::Deny { reason } => {
                let result_block = ContentBlock::ToolResult {
                    tool_use_id: *tool_use_id,
                    content: format!("Permission denied: {reason}"),
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

        // Dispatch through ToolRegistry.
        let Some(tool_handle) = orch.tools.find_by_name(name) else {
            let result_block = ContentBlock::ToolResult {
                tool_use_id: *tool_use_id,
                content: format!("Error: tool not found: {name}"),
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

        // One-shot progress channel — receiver dropped immediately.
        let (progress_tx, _progress_rx) =
            tokio::sync::mpsc::channel::<tool_api::progress::ToolProgress>(8);

        let tool_outcome = tool_handle
            .call(effective_input.clone(), ctx, progress_tx)
            .await;

        let (content, is_error, emit_payload) = match tool_outcome {
            Ok(result) => {
                let text = serde_json::to_string(&result.data)
                    .unwrap_or_else(|_| "<unserializable>".into());
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

        // M5-06 Task 14: PostToolUse hook chain. Best-effort — a Post
        // hook's system_messages are appended to the result text, but
        // failures do NOT mutate `content` or `is_error`.
        let post_event = HookEvent::PostToolUse {
            tool_name: name.clone(),
            tool_input: effective_input.clone(),
            tool_output: emit_payload.clone(),
            tool_use_id: *tool_use_id,
        };
        let post_started = std::time::Instant::now();
        tracing::info!(
            event = orch_events::HOOK_POST_STARTED,
            tool_name = %name,
        );
        let post_agg = orch.hooks.execute(post_event, hook_ctx).await;
        // hook duration bounded by tokio timeout — u128 ms cannot exceed u64::MAX
        #[allow(clippy::cast_possible_truncation)]
        let post_dur_ms = post_started.elapsed().as_millis() as u64;

        let mutated = !post_agg.system_messages.is_empty();
        let final_content = if mutated {
            let mut out = content.clone();
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

        results.push(ContentBlock::ToolResult {
            tool_use_id: *tool_use_id,
            content: final_content,
            is_error,
        });
    }
    Ok(results)
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
            vec![api_client::types::ContentBlockApi::Text {
                text: "done".into(),
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
        execute_one_turn_with_recovery, RecoveryState, TurnStepOutcome,
        MAX_OUTPUT_TOKENS_RECOVERY_LIMIT, MAX_OUTPUT_TOKENS_RECOVERY_NUDGE,
    };
    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{
        mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use api_client::types::ContentBlockApi;
    use protocol::{ContentBlock, ConversationMessage};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// Build an orchestrator whose batched API returns the given scripted
    /// `MessageResponse`s in order. No tools registered (recovery never needs
    /// them).
    fn orch_with_responses(
        responses: Vec<api_client::types::MessageResponse>,
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
    fn max_tokens_response() -> api_client::types::MessageResponse {
        mock_message_response(
            vec![ContentBlockApi::Text {
                text: "partial".into(),
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
            vec![ContentBlockApi::Text { text: "done".into() }],
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
