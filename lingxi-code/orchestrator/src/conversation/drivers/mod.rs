//! Batched, cancelable, and streaming conversation turn drivers.

use super::*;
use crate::streaming_loop::ExecutorPump;
use protocol::ContentBlock;

/// Mutable state shared by the phases of one streaming turn.
///
/// Keeping these counters together makes their session/turn lifetime explicit
/// and lets the streaming driver hand a single state value to its preparation,
/// pump, finalize, tool, and disposition phases.
struct StreamingTurnState {
    recovery: RecoveryState,
    stop_hook_active: bool,
    stop_hook_blocking_count: u32,
    budget: Option<BudgetTracker>,
    global_turn_tokens: u64,
    turn_count: u32,
    malformed_tool_use_retried: bool,
    thinking_only_nudged: bool,
    last_message_id: MessageId,
}

impl StreamingTurnState {
    fn new(orch: &ConversationOrchestrator, last_message_id: MessageId) -> Self {
        orch.compaction_runtime.turn_start_output_baseline.store(
            orch.compaction_runtime
                .output_token_pool
                .load(std::sync::atomic::Ordering::Relaxed),
            std::sync::atomic::Ordering::Relaxed,
        );
        Self {
            recovery: RecoveryState::default(),
            stop_hook_active: false,
            stop_hook_blocking_count: 0,
            budget: orch.new_budget_tracker(),
            global_turn_tokens: 0,
            turn_count: 0,
            malformed_tool_use_retried: false,
            thinking_only_nudged: false,
            last_message_id,
        }
    }
}

enum StreamingIterationDisposition {
    Continue,
    /// A natural completion may absorb input queued while the model streamed.
    Complete(MessageId),
    /// A result-level end request is terminal for this query. Pending input is
    /// left for the next turn instead of causing another model invocation.
    ForcedComplete(MessageId),
    Return(ConversationOutcome),
}

struct StreamingTurnDriver<'a> {
    orch: &'a ConversationOrchestrator,
    prompt: &'a str,
    images: Vec<protocol::ImageSource>,
    user_cancel: Option<CancellationToken>,
    message_id: Option<MessageId>,
    transient_rewake: bool,
}

struct PreparedStreamingIteration {
    snapshot: Vec<ConversationMessage>,
    model: String,
    model_profile: Option<String>,
    outgoing_history_rewriter: Option<Arc<dyn OutgoingHistoryRewriter>>,
    turn_reminders: Vec<ConversationMessage>,
    wire_tools: Vec<serde_json::Value>,
    deferred_reminder: Option<ConversationMessage>,
    date_change_reminder: Option<ConversationMessage>,
    assistant_id: MessageId,
    partial_finalize: Option<crate::streaming_loop::PartialFinalizeCause>,
    partial_finalize_notice_id: Option<MessageId>,
    turn_id: String,
    display_hook_active: bool,
}

enum PrepareStreamingOutcome {
    Ready(PreparedStreamingIteration),
    Complete(MessageId),
}

enum OpenedModelStream {
    Stream(futures::stream::BoxStream<'static, Result<LlmEvent, LlmError>>),
    Recovered(crate::streaming_loop::PumpedTurn),
}

struct OpenedStreamingIteration<'a> {
    opened: OpenedModelStream,
    exec: crate::streaming_executor::StreamingToolExecutor<'a>,
    model: String,
    model_profile: Option<String>,
    outgoing_history_rewriter: Option<Arc<dyn OutgoingHistoryRewriter>>,
    turn_reminders: Vec<ConversationMessage>,
    wire_tools: Vec<serde_json::Value>,
    deferred_reminder: Option<ConversationMessage>,
    date_change_reminder: Option<ConversationMessage>,
    assistant_id: MessageId,
    partial_finalize: Option<crate::streaming_loop::PartialFinalizeCause>,
    partial_finalize_notice_id: Option<MessageId>,
    turn_id: String,
    display_hook_active: bool,
    api_call_started: std::time::Instant,
    api_success_message_count: u32,
    api_success_message_tokens: u64,
    did_fall_back_to_non_streaming: bool,
}

enum OpenStreamingOutcome<'a> {
    Opened(OpenedStreamingIteration<'a>),
    Complete(MessageId),
}

struct PumpedStreamingIteration<'a> {
    pumped: crate::streaming_loop::PumpedTurn,
    exec: crate::streaming_executor::StreamingToolExecutor<'a>,
    model: String,
    model_profile: Option<String>,
    wire_tools: Vec<serde_json::Value>,
    assistant_id: MessageId,
    partial_finalize: Option<crate::streaming_loop::PartialFinalizeCause>,
    partial_finalize_notice_id: Option<MessageId>,
    turn_id: String,
    display_hook_active: bool,
    api_call_started: std::time::Instant,
    api_success_message_count: u32,
    api_success_message_tokens: u64,
    did_fall_back_to_non_streaming: bool,
    pre_batch_mcp_tool_count: usize,
}

enum PumpStreamingOutcome<'a> {
    Pumped(PumpedStreamingIteration<'a>),
    Complete(MessageId),
}

struct FinalizedStreamingIteration {
    pumped: crate::streaming_loop::PumpedTurn,
    assistant_id: MessageId,
    tool_prevent_continuation: bool,
    post_tool_batch_calls: Vec<hooks::events::PostToolBatchCall>,
    pre_batch_mcp_tool_count: usize,
    partial_finalize: Option<crate::streaming_loop::PartialFinalizeCause>,
    partial_finalize_notice_id: Option<MessageId>,
    aborted_during_stream: bool,
}

impl StreamingTurnDriver<'_> {
    /// Collect this step's transient reminders, plus (separately) the durable
    /// task-completion message.
    ///
    /// The second value is NOT a reminder: it has already been appended to
    /// history and the JSONL, so it must be added to the outgoing snapshot
    /// exactly once. Returning it in the reminder vector would double it on
    /// every retry, because a retry rebuilds the request from history and
    /// re-appends the reminders on top.
    async fn collect_turn_reminders(
        orch: &ConversationOrchestrator,
    ) -> (Vec<ConversationMessage>, Option<ConversationMessage>) {
        let mut turn_reminders: Vec<ConversationMessage> = Vec::new();

        // `/brief` (2.1.252): consume the one-shot model-facing reminder
        // queued by the interactive toggle before this turn is snapshotted.
        if let Some(reminder) = orch.brief_mode_reminder_message() {
            turn_reminders.push(reminder);
        }

        // OUTSTYLE.3 (streaming twin): per-turn, transient output-style
        // reminder. Appended to THIS turn's OUTGOING snapshot only — never to
        // `session.history` / JSONL — so it is recomputed each turn and never
        // accumulates (TS recomputes attachments each turn). Injected BEFORE
        // the blocking-limit estimate below so the reminder's tokens are
        // counted in the prompt size, matching the model input. Trailing
        // position mirrors TS; `None` for the default style ⇒ no extra
        // message, keeping the locked streaming fixtures byte-identical. See
        // [`Self::output_style_reminder_message`].
        if let Some(reminder) = orch.output_style_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // PLANMODE (streaming twin): per-turn, transient `plan_mode` reminder
        // (206 `xEg` attachment). Emitted ONLY while `session.plan_mode` is
        // active (after `EnterPlanMode`), so the DEFAULT build stays
        // byte-identical and the locked streaming fixtures still pass. Placed
        // right after the output-style reminder and BEFORE the skill-listing
        // reminder — identical position to the batched twin (`turn_loop.rs`)
        // and to 206 `KJn` (`l=await xEg(t)` spread before the invoked-skills
        // bodies and tool/mcp deltas). claude-code has ONE main loop, so both
        // LingXi twins must inject this reminder. Appended to THIS turn's
        // OUTGOING snapshot only (never `session.history` / JSONL) and BEFORE
        // the blocking-limit estimate below so its tokens are counted in the
        // prompt size. See [`Self::plan_mode_reminder_message`].
        if let Some(reminder) = orch.plan_mode_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // SKILLLIST.1 (streaming twin): per-turn, transient `skill_listing`
        // reminder so the model can discover skills. Appended to THIS turn's
        // OUTGOING snapshot only (never to `session.history` / JSONL), after
        // the output-style reminder and BEFORE the blocking-limit estimate
        // below so its tokens are counted in the prompt size. `None` when no
        // provider is wired / no skills / the Skill tool is absent. See
        // [`Self::skill_listing_reminder_message`].
        if let Some(reminder) = orch.skill_listing_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // §F (streaming twin): per-turn, transient `conditional_rules`
        // reminder — path-gated LINGXI.md rules that newly activate because a
        // touched file matches their globs. Appended to THIS turn's OUTGOING
        // snapshot only (never `session.history` / JSONL), after the
        // skill-listing reminder and BEFORE the blocking-limit estimate below
        // so its tokens are counted in the prompt size. `None` when no
        // provider / no conditional rules / nothing newly active. See
        // [`Self::conditional_rules_reminder_message`].
        if let Some(reminder) = orch.conditional_rules_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // Nested memory (streaming twin): the LINGXI.md governing the
        // directory of a touched file. Appended to THIS turn's OUTGOING
        // snapshot only (never `session.history` / JSONL), directly after
        // the conditional-rules reminder and BEFORE the blocking-limit
        // estimate below so its tokens are counted in the prompt size —
        // identical position to the batched twin (`turn_loop.rs`).
        // claude-code has ONE main loop, so both LingXi twins must inject
        // it. See [`Self::nested_memory_reminder_message`].
        if let Some(reminder) = orch.nested_memory_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // `<new-diagnostics>` (streaming twin — #3 main-loop parity):
        // per-turn, transient reminder of newly-reported LSP diagnostics not
        // yet surfaced (claude-code `formatDiagnosticsBlock`). Appended to
        // THIS turn's OUTGOING snapshot only (never `session.history` /
        // JSONL), after the conditional-rules reminder and before the
        // agent-listing reminder — identical position to the batched twin
        // (`turn_loop.rs`). claude-code has ONE main loop, so both LingXi
        // twins must inject this reminder. `None` when no LSP source is wired
        // (no servers) or no new diagnostics, keeping the locked streaming
        // fixtures byte-identical. See
        // [`Self::new_diagnostics_reminder_message`].
        if let Some(reminder) = orch.new_diagnostics_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // `agent_listing_delta` (streaming twin): per-turn, transient agent
        // catalog reminder, emitted ONLY when the
        // `LINGXI_AGENT_LIST_IN_MESSAGES` gate is ON (default OFF ⇒
        // `None`, keeping the locked streaming fixtures byte-identical and
        // the inline catalog in place). Appended to THIS turn's OUTGOING
        // snapshot only (never `session.history` / JSONL). See
        // [`Self::agent_listing_reminder_message`].
        if let Some(reminder) = orch.agent_listing_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // REM-05 `changed_files` / `edited_text_file` (streaming twin):
        // one meta reminder per file whose on-disk mtime moved since the
        // model last saw it. Producer `Izm` @296537358; the oracle's
        // attachment fan-out (@296520120) places `changed_files`
        // immediately after `agent_listing_delta` and immediately before
        // `nested_memory` — but the port's `nested_memory` reminder is
        // injected above, so this sits directly after `agent_listing_delta`
        // to keep the relative order with everything downstream. LIVE (no
        // gate); empty in the common case because nothing fires unless a
        // tracked file changed outside this session's own writes. Appended
        // to THIS turn's OUTGOING snapshot only (never `session.history` /
        // JSONL). See [`Self::changed_files_reminder_messages`].
        turn_reminders.extend(orch.changed_files_reminder_messages().await);

        // Finding #73 (streaming twin): per-turn, transient `todo_reminder`
        // (V1) / `task_reminder` (V2) reminder. Same gates as the batched
        // twin (killswitch / tool-present / Brief-absent / non-empty history
        // / both counters at threshold). The body is wrapped in a
        // `<system-reminder>` envelope and marked `isMeta`, matching the
        // oracle's `Zy([kn({content:o,isMeta:!0})])` (2.1.238 @296690005).
        // Placed after the agent-listing reminder and before the async-hook
        // reminder, mirroring the binary `ytl` order (`todo_reminders` in the
        // core `A` array, before the main-only `async_hook_responses`).
        // Appended to THIS turn's OUTGOING snapshot only (never
        // `session.history` / JSONL). `None` keeps the locked streaming
        // fixtures byte-identical. See [`Self::todo_reminder_message`].
        let todo_reminder = orch.todo_reminder_message().await;
        let todo_reminder_fired = todo_reminder.is_some();
        if let Some(reminder) = todo_reminder {
            turn_reminders.push(reminder);
        }

        // REM-10 `tool_search_usage_reminder` (streaming twin): the periodic
        // nudge to load deferred tool schemas. Producer `Uzm` @296553134,
        // whose renderer sits immediately after `task_reminder` in the
        // oracle's `xBi` switch — hence this position, right after the
        // todo/task reminder whose presence it is gated against
        // (`task_reminder_same_turn`). INERT by default: the upstream gate
        // is a GrowthBook payload that is unset in a stock install, so
        // `config()` is `None` and this is a strict no-op unless
        // `LINGXI_TOOL_SEARCH_REMINDER` is set. Appended to THIS turn's
        // OUTGOING snapshot only. See
        // [`Self::tool_search_usage_reminder_message`].
        if let Some(reminder) = orch
            .tool_search_usage_reminder_message(todo_reminder_fired)
            .await
        {
            turn_reminders.push(reminder);
        }

        // async_hook_response (streaming twin): fold completed background
        // (`async`) hook responses into THIS turn's OUTGOING snapshot only
        // (never `session.history` / JSONL), drained consume-once. `None`
        // when no source is wired / nothing completed since the last turn.
        // See [`Self::async_hook_response_reminder_message`].
        if let Some(reminder) = orch.async_hook_response_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // T35: fold the terminal background tasks finished since the last
        // turn into THIS turn's OUTGOING snapshot only (never
        // `session.history` / JSONL), drained consume-once so each completion
        // surfaces exactly one `<task-notification>`. `None` when no registry
        // is wired / nothing finished. See
        // [`Self::task_notification_reminder_message`].
        // A completion notification is a real conversation event, not a
        // transient reminder (claude-code enqueues it and it becomes a durable
        // user message). Persist it here, and add it to this call's snapshot
        // below once that snapshot exists -- NOT to `turn_reminders`, which the
        // retry path re-appends on top of a request rebuilt from history and
        // would therefore send it twice.
        let task_notification = orch.task_notification_reminder_message().await;
        if let Some(reminder) = task_notification.as_ref() {
            {
                let mut session = orch.session.lock().await;
                session.history.push(reminder.clone());
            }
            orch.persist_message_to_jsonl(reminder).await;
        }

        // REM-14 `memory_update` (streaming twin): one meta reminder per
        // background memory consolidation that finished, telling the model
        // which memory files moved on disk and which of its loaded copies
        // are now stale. Producer `jzm` @296554545; the queue is filled by
        // the `dream` notifications the drain above just consumed, so this
        // MUST follow it. Appended to THIS turn's OUTGOING snapshot only
        // (never `session.history` / JSONL). Empty unless a `dream` task
        // completed. See [`Self::memory_update_reminder_messages`].
        turn_reminders.extend(orch.memory_update_reminder_messages().await);

        // P0.1 (streaming twin): per-turn, transient `relevant_memories`
        // SURFACING reminders — the memory-selector/prefetch result rendered
        // as one meta user message per surfaced memory. Appended to THIS
        // turn's OUTGOING snapshot only (never `session.history` / JSONL),
        // after the async-hook reminder and BEFORE the blocking-limit estimate
        // below so its tokens are counted in the prompt size. Awaits the
        // prefetch armed by `start_memory_prefetch` at turn start. Empty when
        // no prefetch is wired / empty result / everything already injected.
        // See [`Self::relevant_memory_reminder_messages`].
        turn_reminders.extend(orch.relevant_memory_reminder_messages().await);

        // EXPERIMENTAL_SKILL_SEARCH (streaming twin): per-turn, transient
        // `skill_discovery` SURFACING reminder, collected AFTER the memory
        // consume above (bundle order: memory consume → `collectSkill...`).
        // Appended to THIS turn's OUTGOING snapshot only. `None` when no
        // prefetch is wired (default OFF) / empty / everything already
        // surfaced. See [`Self::skill_discovery_reminder_message`].
        if let Some(reminder) = orch.skill_discovery_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // `silent_turn_reminder` (streaming twin, NEW in 2.1.238): nudge the
        // model to say something when it has worked several turns in a row
        // in silence. Producer `K4T` @296525255, gate @296520120 (main agent,
        // tool-round continuation only, model capability / env). The port has
        // no model-capability table, so the DEFAULT IS OFF (`None`) and the
        // locked streaming fixtures stay byte-identical. Positioned late in
        // the batch, mirroring the oracle's fan-out order
        // (`…critical_system_reminder, silent_turn_reminder,
        // total_tokens_reminder, budget_usd`). Appended to THIS turn's
        // OUTGOING snapshot only. See [`Self::silent_turn_reminder_message`].
        if let Some(reminder) = orch.silent_turn_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // `total_tokens_reminder` (streaming twin): the
        // `<total_tokens>N tokens left</total_tokens>` budget block. Producer
        // `D3T` @296556375; the oracle emits it after every tool-result batch
        // and (with `totalTokensReminderAfterUserTurn`) after each regular
        // user prompt. DEFAULT OFF in the port — see the divergence note on
        // [`crate::prompt::total_tokens`] — so this is a strict no-op unless
        // `CLAUDE_CODE_TOTAL_TOKENS_REMINDER` is set. Placed directly after
        // the silent-turn reminder, matching the oracle's fan-out order.
        // Appended to THIS turn's OUTGOING snapshot only. See
        // [`Self::total_tokens_reminder_message`].
        if let Some(reminder) = orch.total_tokens_reminder_message().await {
            turn_reminders.push(reminder);
        }

        // Rebuild on every model step: a ToolSearch result marks schemas as
        // discovered, so the immediately following request must include
        // those schemas with `defer_loading:true`. The accompanying
        // `deferred_tools_delta` reminder is transient and prepended exactly
        // once per outgoing request. `deferred_tools_reminder_message`
        // ADVANCES the announced-set tracking, so compute it ONCE per model
        // step here and reuse this value on every retry/fallback re-snapshot
        // below (each of which rebuilds the SAME step's request).
        (turn_reminders, task_notification)
    }

    async fn prepare_iteration(
        orch: &ConversationOrchestrator,
        system_prompt: &Option<String>,
        user_cancel: &Option<CancellationToken>,
        loop_state: &mut StreamingTurnState,
    ) -> Result<PrepareStreamingOutcome, OrchestratorError> {
        // P0.1 (streaming twin): arm the memory-selector prefetch CONCURRENTLY
        // with this turn (claude-code `wAo`). Fired here at turn start so the
        // in-flight handle is ready when `relevant_memory_reminder_messages`
        // awaits it below, before the blocking-limit estimate. A strict no-op
        // when no prefetch is wired, keeping the locked streaming fixtures
        // byte-identical. See [`Self::start_memory_prefetch`].
        orch.start_memory_prefetch().await;
        // EXPERIMENTAL_SKILL_SEARCH (streaming twin): arm the skill-discovery
        // prefetch CONCURRENTLY with this turn (claude-code
        // `startSkillDiscoveryPrefetch`). A strict no-op when no prefetch is
        // wired (default OFF), keeping the locked streaming fixtures
        // byte-identical. See [`Self::start_skill_discovery_prefetch`].
        orch.start_skill_discovery_prefetch().await;
        // P1 (§6.5): background-fork a session-memory extraction if the
        // tool-call threshold has crossed (inert unless wired + enabled).
        orch.maybe_extract_session_memory().await;

        // In-Loop Compaction Batch 4 (streaming twin): proactively
        // snip+micro+autocompact BEFORE snapshotting history for the
        // stream, so a long conversation orch-compacts mid-turn. A strict
        // no-op when no compactor is wired or the history is under
        // threshold, so the locked streaming fixtures are unaffected. After
        // a proactive compact the snapshot below reads the NEW history.
        //
        // The connect-phase streaming 413 path below reuses the same
        // reactive truncate/compact recovery loop as the batched path.
        orch.seed_compact_cache_safe_params(system_prompt.as_deref())
            .await;
        orch.maybe_compact_before_call().await;
        // 2.1.232: accepted peer inbox → user-role `<cross-session-message>`
        // before the outgoing snapshot is cloned from history.
        let _ = orch.drain_peer_inbox(false).await;

        // 2. Open the stream for this turn.
        let prepared_call = orch
            .prepare_model_call_snapshot(
                ModelCallPath::Streaming,
                system_prompt.as_deref(),
                user_cancel.as_ref(),
            )
            .await?;
        let mut snapshot = prepared_call.history_snapshot;
        let model = prepared_call.model;
        let model_profile = prepared_call.model_profile;
        let outgoing_history_rewriter = prepared_call.outgoing_history_rewriter;

        // R-P1c/R-P1d (streaming twin): PREPEND the leading `additionalContext`
        // meta message (`# claudeMd` / `# userEmail` / `# currentDate`) to THIS
        // turn's OUTGOING snapshot only (never `session.history` / JSONL). 1:1
        // with claude-code `A6n(re, userContext)`, which prepends the meta
        // message at every `callModel`. Recomputed each turn, never accumulates.
        // `currentDate` is always present, so this is `Some(_)` whenever a
        // LINGXI.md / email / date is sourceable (i.e. always for the date).
        orch.prepend_leading_context(&mut snapshot).await;

        // Per-turn TRANSIENT reminders are collected here rather than
        // pushed straight onto `snapshot`, because `snapshot` is MOVED into
        // `stream()` and every recovery path below rebuilds it from
        // `session.history`. Each of these advances session state when it
        // is computed (sent-sets, delta trackers, consume-once drains), so
        // recomputing them on a retry would return `None` and the reminder
        // would be silently lost for the rest of the session. Computed
        // ONCE, re-appended on every re-snapshot — the same discipline
        // `deferred_reminder` and `date_change_reminder` already follow.
        let (turn_reminders, task_notification) = Self::collect_turn_reminders(orch).await;
        // The durable completion message goes in first: it now lives in history,
        // so it belongs after the last real entry and before the transient
        // reminders. The snapshot was taken before it was appended.
        if let Some(reminder) = task_notification {
            snapshot.push(reminder);
        }
        snapshot.extend(turn_reminders.iter().cloned());

        let wire_tools = orch.build_wire_tools().await;
        // Carved-slate records the first eligible static prompt before opening
        // the stream. A resumed session with no valid attachment intentionally
        // stays live and does not create a replacement snapshot.
        orch.record_prompt_snapshot_if_needed(system_prompt.as_deref(), &wire_tools)
            .await;
        let deferred_reminder = orch.deferred_tools_reminder_message();
        if let Some(reminder) = deferred_reminder.clone() {
            orch.prepend_transient_leading_context(&mut snapshot, reminder);
        }

        // `date_change` (streaming twin): sessions crossing local midnight
        // tell the model the new date once. Prepended AFTER the deferred
        // insert so the final order is [date_change, deferred_tools_delta,
        // …] — matching the oracle attachment batch order (`Ky("date_change")`
        // before `Ky("deferred_tools_delta")`). Computed ONCE per model
        // step and reused by the retry/fallback re-snapshots below, exactly
        // like `deferred_reminder`; the dedupe is committed only once the
        // stream actually opens (below the blocking-limit preempt). `None`
        // (the overwhelmingly common same-date case) keeps the locked
        // streaming fixtures byte-identical. See
        // [`Self::date_change_reminder_message`].
        let date_change_reminder =
            orch.date_change_reminder_message(orch.session.lock().await.session_id);
        if let Some(reminder) = date_change_reminder.clone() {
            orch.prepend_transient_leading_context(&mut snapshot, reminder);
        }

        // RECOV.1: blocking-limit preempt — the streaming twin of the batched
        // `call_api_with_ptl_recovery` step (1) (TS `query.ts:592-648`). If the
        // pre-call prompt is already at the hard blocking limit
        // (`token_usage >= effective_window − MANUAL_COMPACT_BUFFER_TOKENS`),
        // surface the byte-exact `PROMPT_TOO_LONG_ERROR_MESSAGE` and END the
        // turn WITHOUT opening the stream — mirroring the batched path (which
        // returns `PtlCallOutcome::PromptTooLong` ⇒ ends with stop_reason
        // `"prompt_too_long"`). Use the exact same custom beta set as the
        // provider request so context-1m sessions are not preempted at the
        // default 200k boundary.
        let active_betas = orch.api.active_betas();
        let warning = compaction::calculate_token_warning_state(
            compaction::grouping::estimate_tokens_for_range(&snapshot),
            &model,
            &active_betas,
            true,
        );
        if warning.is_at_blocking_limit {
            tracing::warn!(
                model = %model,
                "prompt at blocking limit — preempting before stream"
            );
            let id = surface_prompt_too_long(orch).await;
            // PROACTIVE preempt ⇒ terminal reason `"blocking_limit"` (distinct
            // from the reactive-exhausted `prompt_too_long`), mirroring the
            // batched path's `PtlCallOutcome::BlockingLimit`. RECOV.2 chokepoint:
            // `handle_stop_at_end` treats `"blocking_limit"` as an api-error end
            // (fires `StopFailure`, skips the normal `Stop` hooks, returns
            // `FallThrough`), so the directive is discarded and the normal
            // end-of-turn tail runs — exactly mirroring the batched path.
            let _ = orch
                .handle_stop_at_end(
                    "blocking_limit",
                    &mut loop_state.stop_hook_active,
                    &mut loop_state.stop_hook_blocking_count,
                    loop_state.turn_count,
                    id,
                )
                .await;
            let cost = orch.snapshot_cost_real().await;
            orch.output.emit_end_turn("blocking_limit", &cost).await;
            return Ok(PrepareStreamingOutcome::Complete(id));
        }
        // Past the preempt: this step's snapshot WILL be sent, so the
        // `date_change` reminder it carries counts as delivered.
        orch.commit_date_change_reminder();

        // Mid-stream tool dispatch (claude-code `query.ts:562` + `837-844`):
        // create the executor + pre-allocate this turn's assistant id BEFORE
        // opening the stream, so each `tool_use` block can be `add_tool`'d and
        // dispatched the moment it streams in (instead of collecting all
        // tool_uses and only starting them after the stream ends). This is
        // BYTE-EQUIVALENT: only WHEN tools start changes. The per-block
        // assistant JSONL persistence + per-result drain/persist still run
        // post-stream below (see `pump_stream_with_executor`'s contract).
        //
        // DEFERRED-3: hand the executor the turn's user-interrupt token (if
        // any) so it can reject in-flight/queued Cancel-behavior tools with the
        // REJECT_MESSAGE; `None` → identical to before.
        let assistant_id = MessageId::new();

        // P1-04 (cc 2.1.199 partial-stream finalize): set by the pump error
        // arm when a completed partial is finalized in place. Drives the
        // "API Error: … may be incomplete." notice (surfaced after the partial
        // is persisted) and the terminal turn-end below. Reset per turn.
        let partial_finalize: Option<crate::streaming_loop::PartialFinalizeCause> = None;
        let partial_finalize_notice_id: Option<MessageId> = None;

        // hooks #39: MessageDisplay fires at the BEGIN of this assistant
        // message's stream (claude-code `begin(d)`, BIN off 208862320),
        // mirroring its `o={apiMessageId:d, messageId:randomUUID(),
        // turnId:r, index:0, …}` initialization. The `turn_id` is a fresh
        // per-turn UUID (`newTurn(){…; r=randomUUID()}`). Best-effort +
        // no-op when unregistered, so existing flows are byte-identical.
        let turn_id = uuid::Uuid::new_v4().to_string();
        orch.fire_message_display(&turn_id, assistant_id).await;

        // P2-04 (MessageDisplay `displayContent`): a registered
        // `MessageDisplay` hook makes the live pump SUPPRESS per-token text
        // deltas so the completed-message pass below renders the full
        // (possibly hook-substituted) text once — faithful to claude-code
        // `Qff` (BIN off 229876575), whose live render flows through the
        // display flush, not raw deltas. `false` (no hook) ⇒ byte-identical
        // live streaming. Cheap subscription gate (declared-event only).
        let display_hook_active = orch
            .hooks
            .has_hooks_for(&hooks::events::HookEventType::MessageDisplay)
            .await;

        Ok(PrepareStreamingOutcome::Ready(PreparedStreamingIteration {
            snapshot,
            model,
            model_profile,
            outgoing_history_rewriter,
            turn_reminders,
            wire_tools,
            deferred_reminder,
            date_change_reminder,
            assistant_id,
            partial_finalize,
            partial_finalize_notice_id,
            turn_id,
            display_hook_active,
        }))
    }

    async fn open_iteration<'a>(
        orch: &'a ConversationOrchestrator,
        prepared: PreparedStreamingIteration,
        system_prompt: &Option<String>,
        user_cancel: &Option<CancellationToken>,
        loop_state: &mut StreamingTurnState,
    ) -> Result<OpenStreamingOutcome<'a>, OrchestratorError> {
        let PreparedStreamingIteration {
            snapshot,
            model,
            model_profile,
            outgoing_history_rewriter,
            turn_reminders,
            wire_tools,
            deferred_reminder,
            date_change_reminder,
            assistant_id,
            partial_finalize,
            partial_finalize_notice_id,
            turn_id,
            display_hook_active,
        } = prepared;

        let mut exec = match &user_cancel {
            Some(token) => crate::streaming_executor::StreamingToolExecutor::new_with_user_cancel(
                orch,
                token.clone(),
            ),
            None => crate::streaming_executor::StreamingToolExecutor::new(orch),
        };
        // Hold `tool_result` frames until the collection point below can
        // release them in RECEIVED order, with a cancelled tool's synthetic
        // already substituted. Off again after the drive, so any later
        // emission on this orchestrator goes straight out.
        orch.set_tool_frame_buffering(true).await;

        // #5: wall-clock from stream-open through pump completion (incl. any
        // 529→non-stream fallback) so the CostTracker records a REAL duration
        // instead of `Duration::ZERO`. Paired with
        // `orch.streaming_api.last_retry_count()` at the billing site below.
        let api_call_started = std::time::Instant::now();

        // tengu_api_success `messageCount:n` / `messageTokens:r`: capture from
        // the OUTGOING snapshot BEFORE it is moved into `.stream(...)`.
        let api_success_message_count = u32::try_from(snapshot.len()).unwrap_or(u32::MAX);
        let api_success_message_tokens = compaction::grouping::estimate_tokens_for_range(&snapshot);
        let did_fall_back_to_non_streaming = false;

        orch.sync_thinking_signature_strip_flag_to_api().await;
        // Either an open stream to pump, or a turn already RECOVERED from a
        // connect-phase prompt-too-long (#1, see the ContextOverflow arm).
        let stream_result = orch
            .streaming_api
            .stream(
                &model,
                model_profile.as_deref(),
                system_prompt.as_deref(),
                snapshot,
                wire_tools.clone(),
            )
            .await;
        orch.persist_thinking_signature_strip_latch().await;
        let opened = match stream_result {
            Ok(s) => OpenedModelStream::Stream(s),
            // #1 (main-loop parity): a connect-phase 413 / prompt-too-long
            // surfaces HERE as `LlmError::ContextOverflow` — the adapter's
            // `drive_stream` returns `Err` on connect status >= 400, so it
            // never reaches the pump. The proactive blocking-limit preempt
            // above undershot the server's own limit, so recover REACTIVELY
            // via the SAME helper the batched path uses
            // (`call_api_with_ptl_recovery`): truncate-head xN -> one full
            // compact -> retry. On success we replay the recovered
            // non-streaming response exactly like the 529 fallback below; on
            // exhaustion we end the turn with the byte-exact prompt_too_long /
            // rapid_refill copy — identical to the proactive preempt and the
            // batched path. NOT gated on
            // `LINGXI_DISABLE_NONSTREAMING_FALLBACK` (that flag governs
            // the 529 overload fallback; PTL recovery is the always-on batched
            // behavior). Previously a streaming 413 bubbled as a hard
            // `OrchestratorError::Streaming` error (documented divergence).
            Err(LlmError::ContextOverflow { .. }) => {
                // Re-snapshot history — the per-turn `snapshot` was MOVED into
                // the failed `stream()` call. Prepend the additional-context
                // meta message, like every `callModel` (claude-code `A6n`).
                let (recov_snapshot_raw, recov_model, recov_profile) = {
                    let s = orch.session.lock().await;
                    (
                        s.model_context_history(),
                        s.model.clone(),
                        s.model_profile.clone(),
                    )
                };
                let mut recov_snapshot = orch
                    .rewrite_outgoing_history(
                        recov_snapshot_raw,
                        outgoing_history_rewriter.as_ref(),
                    )
                    .await?;
                orch.reattach_outgoing_context(
                    &mut recov_snapshot,
                    deferred_reminder.as_ref(),
                    date_change_reminder.as_ref(),
                    &turn_reminders,
                )
                .await;
                match call_api_with_ptl_recovery(
                    orch,
                    system_prompt.as_deref(),
                    &recov_model,
                    recov_profile.as_deref(),
                    recov_snapshot,
                    outgoing_history_rewriter.clone(),
                    wire_tools.clone(),
                    None,
                    deferred_reminder.clone(),
                    date_change_reminder.clone(),
                    &turn_reminders,
                )
                .await?
                {
                    PtlCallOutcome::Response(resp) => {
                        // Replay the recovered non-streaming response exactly
                        // like the 529 fallback below: emit text live, rebuild
                        // a fresh executor, register its tool_uses, and flow on
                        // as the turn's `pumped` result.
                        let pumped_from_recovery = llm_response_to_pumped_turn(&resp);
                        // P2-04: when a `MessageDisplay` hook is active the
                        // completed-message pass below is the single on-screen
                        // render (with `displayContent` substitution) — skip the
                        // direct whole-body emit here to avoid double display.
                        if !display_hook_active {
                            for blk in &pumped_from_recovery.assistant_blocks {
                                if let ContentBlock::Text { text } = blk {
                                    orch.output.emit_text(text).await;
                                }
                            }
                        }
                        exec = match &user_cancel {
                            Some(token) => {
                                crate::streaming_executor::StreamingToolExecutor::new_with_user_cancel(
                                    orch,
                                    token.clone(),
                                )
                            }
                            None => crate::streaming_executor::StreamingToolExecutor::new(orch),
                        };
                        for tu in &pumped_from_recovery.tool_uses {
                            exec.add_tool(
                                tu.id.clone(),
                                tu.name.clone(),
                                tu.input.clone(),
                                tu.provider_id.clone(),
                                assistant_id,
                            );
                        }
                        OpenedModelStream::Recovered(pumped_from_recovery)
                    }
                    PtlCallOutcome::PromptTooLong => {
                        // Reactive recovery exhausted — end with the reactive
                        // terminal reason `prompt_too_long` (`query.ts:1175`).
                        let id = surface_prompt_too_long(orch).await;
                        let _ = orch
                            .handle_stop_at_end(
                                "prompt_too_long",
                                &mut loop_state.stop_hook_active,
                                &mut loop_state.stop_hook_blocking_count,
                                loop_state.turn_count,
                                id,
                            )
                            .await;
                        let cost = orch.snapshot_cost_real().await;
                        orch.output.emit_end_turn("prompt_too_long", &cost).await;
                        return Ok(OpenStreamingOutcome::Complete(id));
                    }
                    PtlCallOutcome::BlockingLimit => {
                        // The recovery's own PROACTIVE step-1 preempt fired on
                        // the re-snapshotted history ⇒ terminal `"blocking_limit"`
                        // (distinct from reactive-exhausted `prompt_too_long`),
                        // mirroring the batched path's `BlockingLimit` arm.
                        let id = surface_prompt_too_long(orch).await;
                        let _ = orch
                            .handle_stop_at_end(
                                "blocking_limit",
                                &mut loop_state.stop_hook_active,
                                &mut loop_state.stop_hook_blocking_count,
                                loop_state.turn_count,
                                id,
                            )
                            .await;
                        let cost = orch.snapshot_cost_real().await;
                        orch.output.emit_end_turn("blocking_limit", &cost).await;
                        return Ok(OpenStreamingOutcome::Complete(id));
                    }
                    PtlCallOutcome::RapidRefillBreaker => {
                        // #54 reactive trip — surface the thrashing message and
                        // end with the terminal reason `rapid_refill_breaker`
                        // (the MESSAGE still carries api-error `invalid_request`),
                        // mirroring the batched path.
                        let id = surface_rapid_refill_thrashing(orch).await;
                        let _ = orch
                            .handle_stop_at_end(
                                "rapid_refill_breaker",
                                &mut loop_state.stop_hook_active,
                                &mut loop_state.stop_hook_blocking_count,
                                loop_state.turn_count,
                                id,
                            )
                            .await;
                        let cost = orch.snapshot_cost_real().await;
                        orch.output
                            .emit_end_turn("rapid_refill_breaker", &cost)
                            .await;
                        return Ok(OpenStreamingOutcome::Complete(id));
                    }
                }
            }
            // #10: a connect-phase RateLimited/Overloaded keeps its dedicated
            // downstream handling — propagate as a hard error.
            Err(e @ (LlmError::RateLimited { .. } | LlmError::Overloaded { .. })) => {
                return Err(OrchestratorError::Streaming(e));
            }
            // #10: any other connect-phase model/runtime error ends the turn
            // GRACEFULLY as `model_error` (faithful port of the `query.ts`
            // catch) — surface the raw error text as an api-error assistant
            // message instead of bubbling a hard error. No assistant message
            // was persisted this turn, so no orphaned tool_use to repair.
            Err(other) => {
                // Classify the typed connect-phase error (`Flp`/`KNn`) before
                // consuming it for the verbatim message text. Wrap into the
                // `Streaming` variant — this is the connect-phase streaming
                // surface — so the classifier sees the inner `LlmError`.
                let env = classify_api_error(&OrchestratorError::Streaming(other.clone()));
                let id = crate::turn_loop::surface_model_error(
                    orch,
                    &orch.model_error_text(&other).await,
                    env,
                )
                .await;
                let cost = orch.snapshot_cost_real().await;
                orch.output.emit_end_turn("model_error", &cost).await;
                return Ok(OpenStreamingOutcome::Complete(id));
            }
        };

        Ok(OpenStreamingOutcome::Opened(OpenedStreamingIteration {
            opened,
            exec,
            model,
            model_profile,
            outgoing_history_rewriter,
            turn_reminders,
            wire_tools,
            deferred_reminder,
            date_change_reminder,
            assistant_id,
            partial_finalize,
            partial_finalize_notice_id,
            turn_id,
            display_hook_active,
            api_call_started,
            api_success_message_count,
            api_success_message_tokens,
            did_fall_back_to_non_streaming,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    async fn fallback_after_stream_error<'a>(
        orch: &'a ConversationOrchestrator,
        error: OrchestratorError,
        outgoing_history_rewriter: Option<&Arc<dyn OutgoingHistoryRewriter>>,
        deferred_reminder: Option<&ConversationMessage>,
        date_change_reminder: Option<&ConversationMessage>,
        turn_reminders: &[ConversationMessage],
        wire_tools: &[serde_json::Value],
        system_prompt: &Option<String>,
        user_cancel: &Option<CancellationToken>,
        display_hook_active: bool,
        assistant_id: MessageId,
    ) -> Result<
        (
            crate::streaming_loop::PumpedTurn,
            crate::streaming_executor::StreamingToolExecutor<'a>,
        ),
        OrchestratorError,
    > {
        // Seed: a streaming overload counts as 1 toward the consecutive
        // 529 budget (LlmError::Overloaded = 529).  Other in-band errors
        // (e.g. ProviderInternal) seed 0 — matching TS
        // `is529Error(streamingError) ? 1 : 0` (claude.ts:2559).
        let seed: u8 = u8::from(matches!(
            error,
            OrchestratorError::Streaming(LlmError::Overloaded { .. })
        ));

        // Re-snapshot history for the non-streaming call (the partial
        // stream never touched session.history, so it is still the same
        // snapshot we used for the stream — no reset needed).
        let (non_stream_snapshot_raw, non_stream_model, non_stream_profile) = {
            let s = orch.session.lock().await;
            (
                s.model_context_history(),
                s.model.clone(),
                s.model_profile.clone(),
            )
        };
        let mut non_stream_snapshot = orch
            .rewrite_outgoing_history(non_stream_snapshot_raw, outgoing_history_rewriter)
            .await?;
        // R-P1c/R-P1d: claude-code's `A6n` prepends the additional-
        // context meta message on EVERY `callModel`, including this
        // non-streaming fallback. Prepend it to the re-snapshot too.
        orch.reattach_outgoing_context(
            &mut non_stream_snapshot,
            deferred_reminder,
            date_change_reminder,
            &turn_reminders,
        )
        .await;
        let tools_for_fallback = wire_tools.to_vec();

        let resp = orch
            .api
            .messages_create_seeded(
                &non_stream_model,
                non_stream_profile.as_deref(),
                system_prompt.as_deref(),
                non_stream_snapshot,
                tools_for_fallback,
                seed,
            )
            .await
            .map_err(OrchestratorError::ApiCall)?;

        // Convert LlmResponse → PumpedTurn so the rest of the streaming
        // turn loop can proceed identically.
        let pumped_from_fallback = llm_response_to_pumped_turn(&resp);

        // Emit text blocks from the non-streaming response to the output
        // stream, mirroring the batched path (turn_loop.rs step 4:
        // `orch.output.emit_text(text).await`).  In the normal streaming
        // path `pump_stream` calls `dispatch_event` → `emit_text` for each
        // `TextDelta`; the non-streaming path has no SSE events, so we
        // replicate the whole-body emit here.
        // P2-04: suppressed when a `MessageDisplay` hook is active — the
        // completed-message pass renders the (possibly substituted) text
        // once, so skip the direct emit to avoid double display.
        for blk in &pumped_from_fallback.assistant_blocks {
            match blk {
                ContentBlock::Text { text } if !display_hook_active => {
                    orch.output.emit_text(text).await;
                }
                ContentBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    orch.output
                        .emit_thinking(thinking, signature.as_deref())
                        .await;
                }
                ContentBlock::RedactedThinking { data } => {
                    orch.output.emit_redacted_thinking(data).await;
                }
                _ => {}
            }
        }

        // claude-code `query.ts:733-740`: discard the partial
        // streaming attempt's executor (its tool_uses have stale ids
        // and would orphan against the fallback response) and replace
        // it with a fresh one. Dropping the old executor cancels any
        // in-flight tool futures it had started mid-stream. The fresh
        // executor's tools are registered from the FALLBACK response's
        // tool_uses by the post-stream drive loop below (this is the
        // ONLY path that still `add_tool`s after the stream — the
        // normal path registers mid-stream).
        let mut exec = match user_cancel {
            Some(token) => crate::streaming_executor::StreamingToolExecutor::new_with_user_cancel(
                orch,
                token.clone(),
            ),
            None => crate::streaming_executor::StreamingToolExecutor::new(orch),
        };
        for tu in &pumped_from_fallback.tool_uses {
            exec.add_tool(
                tu.id.clone(),
                tu.name.clone(),
                tu.input.clone(),
                tu.provider_id.clone(),
                assistant_id,
            );
        }

        Ok((pumped_from_fallback, exec))
    }

    async fn pump_iteration<'a>(
        orch: &'a ConversationOrchestrator,
        prepared: PreparedStreamingIteration,
        system_prompt: &Option<String>,
        user_cancel: &Option<CancellationToken>,
        loop_state: &mut StreamingTurnState,
    ) -> Result<PumpStreamingOutcome<'a>, OrchestratorError> {
        let OpenedStreamingIteration {
            opened,
            mut exec,
            model,
            model_profile,
            outgoing_history_rewriter,
            turn_reminders,
            wire_tools,
            deferred_reminder,
            date_change_reminder,
            assistant_id,
            mut partial_finalize,
            partial_finalize_notice_id,
            turn_id,
            display_hook_active,
            api_call_started,
            api_success_message_count,
            api_success_message_tokens,
            mut did_fall_back_to_non_streaming,
        } = match Self::open_iteration(orch, prepared, system_prompt, user_cancel, loop_state)
            .await?
        {
            OpenStreamingOutcome::Opened(iteration) => iteration,
            OpenStreamingOutcome::Complete(message_id) => {
                return Ok(PumpStreamingOutcome::Complete(message_id));
            }
        };
        // Polling the stream can start tool execution immediately. Snapshot
        // the live MCP count before that point so a tool-triggered registry
        // refresh can be compared after PostToolBatch, as in the oracle.
        let pre_batch_mcp_tool_count = orch.filtered_mcp_tool_count().await;

        // 3. Pump the stream (with mid-stream 529 → non-streaming fallback OR
        //    the cc 2.1.199 partial-finalize, whichever applies).
        //
        // Task 7 / claude.ts parity: if the stream errors with `LlmError::Overloaded`
        // after the first event — AND `LINGXI_DISABLE_NONSTREAMING_FALLBACK` is not
        // set — AND no content block completed yet — issue a fresh non-streaming call
        // seeded with `initial_consecutive_overloaded = 1`.  This mirrors
        // `claude.ts:2469-2594` + `withRetry.ts:186` (`initialConsecutive529Errors`).
        //
        // The env gate name is locked byte-for-byte to TS:
        //   `process.env.LINGXI_DISABLE_NONSTREAMING_FALLBACK` (claude.ts:2470)
        // Truthiness follows `isEnvTruthy` (non-empty, non-"false", non-"0").
        //
        // P1-04 (cc 2.1.199, binary-verified): once a REAL content block has
        // completed — or a transport close leaves visible text whose stop
        // frame was lost — the partial is no longer discarded. The
        // `partial_has_output` arm in the `match pump_outcome` below finalizes
        // it in place (synthesized stop_reason + usage + `tengu_streaming_partial_finalized`),
        // persists the streamed blocks, and appends the "API Error: … may be
        // incomplete." notice — instead of the pre-2.1.199 discard-and-refetch.
        // The non-streaming fallback therefore fires only when the stream
        // produced no recoverable output. Provider errors before the first
        // completed block retain cc's `_r`-length behavior.
        // In both cases partial deltas already reached callers LIVE via
        // `event_router.rs` → `output.emit_text` at each `TextDelta`
        // (claude.ts:2210 `yield m`).
        // #1: a connect-phase prompt-too-long already recovered above (its
        // recovered non-streaming response was replayed) skips the pump; an
        // open stream is pumped as before.
        let pumped = match opened {
            OpenedModelStream::Recovered(pumped_from_recovery) => pumped_from_recovery,
            OpenedModelStream::Stream(first_stream) => {
                // cc 2.1.198 mid-response transient retry (`query.ts` stream
                // loop @219649648): on a transient network drop (ECONNRESET /
                // connection closed / reset) OR a watchdog idle-timeout, re-open
                // and re-pump the SAME streaming request with backoff — but ONLY
                // while `!real_content_started` (binary `!Hr`). Because a
                // `tool_use` block STARTING flips `real_content_started`, this
                // guard also guarantees NO tool has been dispatched, so a
                // non-idempotent tool is never re-run. The failed pump left
                // `session.history` untouched and (by the guard) the executor
                // clean, so the retry reuses `exec` and re-snapshots history.
                let mut cur_stream = first_stream;
                let mut mid_stream_retries: u32 = 0;
                let pump_outcome: Result<
                    crate::streaming_loop::PumpedTurn,
                    crate::streaming_loop::PumpFailure,
                > = loop {
                    match crate::streaming_loop::pump_stream_with_executor_tracked(
                        cur_stream,
                        &orch.output,
                        ExecutorPump {
                            executor: &mut exec,
                            assistant_id,
                            user_cancel: user_cancel.as_ref(),
                            suppress_live_text: display_hook_active,
                        },
                    )
                    .await
                    {
                        Ok(p) => break Ok(p),
                        Err(f)
                            if crate::streaming_loop::is_transient_mid_stream(&f.error)
                                && !f.real_content_started
                                && mid_stream_retries
                                    < crate::streaming_loop::mid_stream_retry_cap(&f.error) =>
                        {
                            mid_stream_retries += 1;
                            // Exponential backoff + jitter (binary `sle`).
                            let base = llm_client::model::retry::scaled_base_delay_ms(
                                mid_stream_retries - 1,
                                None,
                            );
                            tokio::time::sleep(llm_client::model::retry::jittered_delay(base))
                                .await;
                            tracing::warn!(
                                attempt = mid_stream_retries,
                                "mid-response transient stream error — retrying streaming request"
                            );
                            // Re-snapshot history (+ additional context) for
                            // the retry — same pattern as the 529 fallback.
                            let (re_snapshot_raw, re_model, re_profile) = {
                                let s = orch.session.lock().await;
                                (
                                    s.model_context_history(),
                                    s.model.clone(),
                                    s.model_profile.clone(),
                                )
                            };
                            let mut re_snapshot = orch
                                .rewrite_outgoing_history(
                                    re_snapshot_raw,
                                    outgoing_history_rewriter.as_ref(),
                                )
                                .await?;
                            orch.reattach_outgoing_context(
                                &mut re_snapshot,
                                deferred_reminder.as_ref(),
                                date_change_reminder.as_ref(),
                                &turn_reminders,
                            )
                            .await;
                            let retry_stream = orch
                                .streaming_api
                                .stream(
                                    &re_model,
                                    re_profile.as_deref(),
                                    system_prompt.as_deref(),
                                    re_snapshot,
                                    wire_tools.clone(),
                                )
                                .await;
                            orch.persist_thinking_signature_strip_latch().await;
                            match retry_stream {
                                Ok(s) => {
                                    cur_stream = s;
                                    continue;
                                }
                                // Re-open failed: surface as the terminal
                                // pump error for the arms below. No partial to
                                // finalize here — the retry only fires while
                                // `!real_content_started`, so nothing real was
                                // yielded on the attempt we are abandoning.
                                Err(e) => {
                                    break Err(crate::streaming_loop::PumpFailure {
                                        error: OrchestratorError::Streaming(e),
                                        real_content_started: false,
                                        partial: crate::streaming_loop::PumpedTurn::default(),
                                    });
                                }
                            }
                        }
                        Err(f) => break Err(f),
                    }
                };
                match pump_outcome {
                    Ok(p) => p,
                    // P1-04 (cc 2.1.199 partial-stream finalize, binary-verified): a
                    // finalize-class mid-stream error (server/overloaded/api error,
                    // watchdog stall, or connection close) that landed after useful
                    // output is not discarded. The already-streamed
                    // partial is finalized in place — persisted with a synthesized
                    // `stop_reason` (`tool_use` if any tool_use else `end_turn`) +
                    // usage — `tengu_streaming_partial_finalized` fires, and a byte-exact
                    // "API Error: … The response above may be incomplete." notice is
                    // surfaced after it (see the notice + terminal sites below, gated on
                    // `partial_finalize`). This runs BEFORE the 529 non-streaming
                    // fallback so a completed-partial 529 keeps its streamed output
                    // instead of re-fetching; a 529 that erred before any block
                    // completed (no output) falls through to the fallback as before.
                    Err(f)
                        if crate::streaming_loop::partial_has_output(&f.partial)
                            && crate::streaming_loop::partial_finalize_cause(&f.error)
                                .is_some() =>
                    {
                        let cause = crate::streaming_loop::partial_finalize_cause(&f.error)
                            .expect("finalize cause present (guarded above)");
                        let mut partial = f.partial;
                        // cc `gm=vd?"tool_use":"end_turn"`: a dispatched tool_use makes
                        // this a tool turn, else a natural end.
                        let synthesized_stop_reason = if partial.tool_uses.is_empty() {
                            "end_turn"
                        } else {
                            "tool_use"
                        };
                        partial.stop_reason = Some(synthesized_stop_reason.to_string());
                        // cc `_r.length`: one yielded message per completed content block.
                        let blocks_yielded =
                            partial.assistant_blocks.len() + partial.tool_uses.len();
                        if let Some(bus) = orch.model_runtime.analytics_bus.as_ref() {
                            let mut md = telemetry::LogEventMetadata::new();
                            md.insert(
                                "model".into(),
                                telemetry::AnalyticsValue::String(model.clone()),
                            );
                            md.insert(
                                "blocks_yielded".into(),
                                telemetry::AnalyticsValue::Int(
                                    i64::try_from(blocks_yielded).unwrap_or(i64::MAX),
                                ),
                            );
                            // has_output is always true on this arm (partial_has_output).
                            md.insert("has_output".into(), telemetry::AnalyticsValue::Bool(true));
                            md.insert(
                                "synthesized_stop_reason".into(),
                                telemetry::AnalyticsValue::String(
                                    synthesized_stop_reason.to_string(),
                                ),
                            );
                            md.insert(
                                "cause".into(),
                                telemetry::AnalyticsValue::String(cause.as_str().to_string()),
                            );
                            if let Some(rid) = orch.api.last_request_id() {
                                md.insert(
                                    "request_id".into(),
                                    telemetry::AnalyticsValue::String(rid),
                                );
                            }
                            bus.log_event("tengu_streaming_partial_finalized", md).await;
                        }
                        // Arm the notice + terminal-end sites below; the partial flows
                        // through the normal billing/persist/tool-drive path first.
                        partial_finalize = Some(cause);
                        partial
                    }
                    Err(f)
                        if matches!(
                            f.error,
                            OrchestratorError::Streaming(
                                LlmError::Overloaded { .. } | LlmError::ProviderInternal
                            )
                        ) && !is_env_truthy(
                            std::env::var("LINGXI_DISABLE_NONSTREAMING_FALLBACK")
                                .as_deref()
                                .ok(),
                        ) =>
                    {
                        did_fall_back_to_non_streaming = true;
                        let (pumped_from_fallback, replacement_exec) =
                            Self::fallback_after_stream_error(
                                orch,
                                f.error,
                                outgoing_history_rewriter.as_ref(),
                                deferred_reminder.as_ref(),
                                date_change_reminder.as_ref(),
                                &turn_reminders,
                                &wire_tools,
                                system_prompt,
                                user_cancel,
                                display_hook_active,
                                assistant_id,
                            )
                            .await?;
                        exec = replacement_exec;
                        pumped_from_fallback
                    }
                    // #10: RateLimited/Overloaded/RepeatedOverloaded keep dedicated
                    // downstream handling — propagate.
                    Err(f) if crate::turn_loop::is_carveout_propagated(&f.error) => {
                        return Err(f.error);
                    }
                    // #10: any other mid-stream model/runtime error (e.g. Transport)
                    // ends the turn GRACEFULLY as `model_error` (faithful port of the
                    // `query.ts` catch) rather than bubbling a hard error / phantom
                    // interrupt. Reached when the partial finalize above did NOT apply —
                    // either no recoverable output existed before the error (for
                    // example, an incomplete tool block) or the error is not a
                    // finalize class. The assistant message for a partial-with-real-
                    // -output turn is persisted by the finalize arm above; here nothing
                    // was persisted (TS `yieldMissingToolResultBlocks` no-op).
                    Err(f) => {
                        let other = f.error;
                        // Classify the typed mid-stream error (`Flp`/`KNn`) into the
                        // api-error envelope; the message text stays verbatim.
                        let env = classify_api_error(&other);
                        let id =
                            crate::turn_loop::surface_model_error(orch, &other.to_string(), env)
                                .await;
                        let cost = orch.snapshot_cost_real().await;
                        orch.output.emit_end_turn("model_error", &cost).await;
                        return Ok(PumpStreamingOutcome::Complete(id));
                    }
                }
            }
        };
        Ok(PumpStreamingOutcome::Pumped(PumpedStreamingIteration {
            pumped,
            exec,
            model,
            model_profile,
            wire_tools,
            assistant_id,
            partial_finalize,
            partial_finalize_notice_id,
            turn_id,
            display_hook_active,
            api_call_started,
            api_success_message_count,
            api_success_message_tokens,
            did_fall_back_to_non_streaming,
            pre_batch_mcp_tool_count,
        }))
    }

    async fn finalize_iteration(
        orch: &ConversationOrchestrator,
        pumped_iteration: PumpedStreamingIteration<'_>,
        system_prompt: &Option<String>,
        user_cancel: &Option<CancellationToken>,
        loop_state: &mut StreamingTurnState,
    ) -> FinalizedStreamingIteration {
        let PumpedStreamingIteration {
            pumped,
            mut exec,
            model,
            model_profile,
            wire_tools,
            assistant_id,
            partial_finalize,
            mut partial_finalize_notice_id,
            turn_id,
            display_hook_active,
            api_call_started,
            api_success_message_count,
            api_success_message_tokens,
            did_fall_back_to_non_streaming,
            pre_batch_mcp_tool_count,
        } = pumped_iteration;

        // Inline tool descriptions are committed only after a complete,
        // non-API-error response. Partial finalization is an error surface and
        // must not advance the persisted snapshot.
        if partial_finalize.is_none() {
            orch.record_inline_prompt_tools_after_success(&wire_tools)
                .await;
        }

        // A3: accumulate this turn's output tokens (TS `getTurnOutputTokens()`).
        loop_state.global_turn_tokens = loop_state
            .global_turn_tokens
            .saturating_add(pumped.output_tokens);

        // BILLING: record streaming-turn usage into CostTracker — mirrors the
        // non-streaming path in `turn_loop.rs`. #5 (main-loop parity): pass
        // the REAL wall-clock duration (stream-open → pump completion) and
        // the REAL connect-phase retry count (`last_retry_count()`) instead
        // of the previous hardcoded `Duration::ZERO` / `0`.
        if let Some(ref usage) = pumped.usage {
            // #55: cache this response's total input tokens (the `Xtt`
            // last-usage snapshot) for the fixed-prefix overflow guard.
            orch.record_response_input_tokens(usage);
        }
        if let Some(tracker) = orch.model_runtime.cost_tracker.as_ref() {
            if let Some(ref usage) = pumped.usage {
                let cost_usage = crate::cost_wiring::llm_usage_to_cost_usage(usage);
                let cache_read = usage.billable_tokens.cache_read;
                let cache_create = usage.billable_tokens.cache_write;
                let model_ref =
                    crate::cost_wiring::model_ref_from_string(&model, model_profile.as_deref());
                let elapsed = api_call_started.elapsed();
                let retries = orch.streaming_api.last_retry_count();
                let cost_for_this_call = tracker
                    .record_api_response_v2(
                        model_ref.clone(),
                        cost_usage,
                        elapsed,
                        retries,
                        cache_read,
                        cache_create,
                        false, // is_batch_request — streaming is never batch
                        orch.model_runtime.analytics_bus.as_ref(),
                    )
                    .await;
                // strict-parity (2.1.195): fire `tengu_api_success` on the
                // streaming per-request success path (claude
                // `j("tengu_api_success", {...})`). `tengu_cost_recorded`
                // was port-only and dropped.
                //
                // P1-04: a partial-stream finalize is NOT a per-request success —
                // cc records cost (`Ae+=zhe`, kept above) but does NOT emit
                // `tengu_api_success` (it already fired `tengu_streaming_partial_finalized`).
                // Skip the success emit when this turn was finalized from a partial.
                if let Some(bus) = orch
                    .model_runtime
                    .analytics_bus
                    .as_ref()
                    .filter(|_| partial_finalize.is_none())
                {
                    #[allow(clippy::cast_possible_truncation)]
                    let dur_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
                    cost::emit_api_success(
                        bus,
                        &cost::ApiSuccessFields {
                            model: model.clone(),
                            input_tokens: usage.billable_tokens.input,
                            output_tokens: usage.billable_tokens.output,
                            cached_input_tokens: cache_read,
                            uncached_input_tokens: cache_create,
                            duration_ms: dur_ms,
                            duration_ms_including_retries: dur_ms,
                            attempt: retries + 1,
                            cost_nano_usd: cost_for_this_call,
                            provider: crate::cost_wiring::provider_tag(&model_ref.provider),
                            stop_reason: pumped.stop_reason.clone(),
                            request_id: orch.api.last_request_id(),
                            message_count: api_success_message_count,
                            message_tokens: api_success_message_tokens,
                            did_fall_back_to_non_streaming,
                            is_non_interactive_session: !orch.prompt_is_interactive(),
                            print: orch.config.print,
                            is_tty: orch.config.is_tty,
                            query_source: crate::config::sanitize_query_source(
                                &orch.config.query_source,
                            )
                            .to_string(),
                            permission_mode: if orch.session.lock().await.plan_mode {
                                "plan"
                            } else {
                                "default"
                            }
                            .to_string(),
                            ttft_ms: None,
                            fast_mode: usage.speed.as_deref() == Some("fast"),
                            time_since_last_api_call_ms: orch.record_api_call_gap_ms(),
                        },
                    )
                    .await;
                }
                orch.model_runtime
                    .api_calls_recorded
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }

        // In-Loop Compaction Batch 6: snapshot the cache-safe prompt prefix
        // after a successful stream (streaming twin of the batched save).
        // `session.history` here equals the streamed snapshot — the streaming
        // path does not mutate history mid-call — taken before the assistant
        // reply is appended below. Strict no-op when no slot is wired.
        orch.save_cache_safe_params(system_prompt.as_deref(), &model, &wire_tools)
            .await;

        // Task 8 (llm-client future-work batch 3): the streamed call (or
        // its non-streaming 529 fallback) completed — forward the
        // adapter's unified rate-limit snapshot when it changed since the
        // last emission. `orch.api` is the same `ProviderApiAdapter` as
        // `orch.streaming_api` in production; the adapter records headers
        // on the `drive_stream` connect-success path too.
        orch.emit_rate_limit_if_changed().await;
        // Task 2 (batch 5): same seam, raw per-window snapshot.
        orch.emit_raw_utilization_if_changed().await;

        // 4. Assemble + append the assistant message.
        // `assistant_id` was pre-allocated before the stream (mid-stream
        // dispatch hands it to the executor as each tool registers).
        let mut blocks: Vec<ContentBlock> = pumped.assistant_blocks.clone();
        for t in &pumped.tool_uses {
            blocks.push(ContentBlock::ToolUse {
                id: t.id.clone(),
                name: t.name.clone(),
                input: t.input.clone(),
                provider_id: t.provider_id.clone(),
            });
        }
        let assistant_msg = ConversationMessage::Assistant {
            id: assistant_id,
            content: blocks,
            stop_reason: pumped.stop_reason.clone(),
        };
        {
            let mut s = orch.session.lock().await;
            s.history.push(assistant_msg.clone());
        }

        // P2-04 (MessageDisplay `displayContent`): completed-message pass.
        // Once the assistant text blocks are finalized, fire `MessageDisplay`
        // with `final:true` + the joined visible text and render the result
        // ON SCREEN — substituting the hook's `displayContent` when present,
        // else the original text. The stored `assistant_msg` / JSONL keep the
        // ORIGINAL content (claude-code stores the override on a separate
        // `displayedMessageContent`, never `message.content`). Gated on a
        // registered `MessageDisplay` hook — when active the live per-token
        // deltas were suppressed in the pump, so this is the single on-screen
        // render; when inactive this whole block is skipped (byte-identical).
        if display_hook_active {
            let joined: String = pumped
                .assistant_blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.as_str(),
                    _ => "",
                })
                .collect();
            // claude-code `Qff`: skip firing entirely when the joined text is
            // empty (`if(s==="")return i`).
            if !joined.is_empty() {
                let on_screen = orch
                    .fire_message_display_completed(&turn_id, &joined)
                    .await
                    .unwrap_or(joined);
                orch.output.emit_text(&on_screen).await;
            }
        }

        // Finding #73 (streaming twin): advance the per-turn todo/task
        // reminder counters for THIS assistant turn, then reset
        // `turns_since_last_todo_write` if this turn invoked the variant's
        // "recent use" tool (TodoWrite / TaskCreate / TaskUpdate). Bump THEN
        // reset so a TodoWrite turn lands at 0 (matching the binary scan that
        // excludes the TodoWrite message itself). Mirrors the batched twin.
        orch.bump_reminder_turn_counters().await;
        let invoked_tool_names: Vec<String> =
            pumped.tool_uses.iter().map(|t| t.name.clone()).collect();
        orch.note_todo_reminder_tool_call(&invoked_tool_names).await;

        // DEFERRED-3: advance the interrupt-guard's reported final id to this
        // turn's assistant message.
        loop_state.last_message_id = assistant_id;
        // WRITE-side per-block split (claude.ts:2171-2211): persist the turn
        // as one single-block assistant JSONL line per content block, sharing
        // the turn's inner `message.id` with distinct top-level uuids, and
        // capture each `tool_use`'s line uuid so its `tool_result` parents to
        // ITS line (TS `sourceToolAssistantUUID`) — NOT one shared per-turn
        // parent. The in-memory `s.history` above stays the single merged
        // assistant message (the Anthropic request needs all blocks in one
        // assistant turn).
        // Raw Anthropic `usage` object for the persisted BetaMessage envelope
        // (the streaming codec retains it on `Usage::provider_metadata`).
        let assistant_usage = pumped.usage.as_ref().map(assistant_usage_value);
        // The Anthropic `request-id` response header for THIS turn → the
        // top-level `requestId` on each persisted assistant line. The
        // adapter records it from the stream connect-success / non-stream
        // headers (the same `record_rate_limit_from_headers` pass), so it is
        // the just-completed call's id here. `orch.api` is the same adapter
        // as `orch.streaming_api` in production; the fallback non-stream call
        // records it on `orch.api` too.
        let request_id = orch.api.last_request_id();
        // stream-json P1: signal the message boundary to the output sink so
        // `StreamJsonStream` can flush its accumulated assistant frame.
        orch.output
            .emit_message_boundary(pumped.stop_reason.as_deref(), request_id.as_deref())
            .await;
        let tool_use_parent_uuids = orch
            .persist_assistant_per_block(
                &assistant_msg,
                assistant_usage.as_ref(),
                request_id.as_deref(),
            )
            .await;
        // Fallback parent (the LAST persisted block's uuid) for any
        // tool_result whose tool_use id is missing from the map (defensive).
        let assistant_uuid = orch.transcript.last_jsonl_uuid.lock().await.clone();

        // P1-04 (cc 2.1.199): after the finalized partial assistant is
        // persisted, yield the byte-exact incomplete-response notice as its own
        // api-error assistant message — cc yields `tu({content:…, error:"server_error"})`
        // RIGHT AFTER the patched partial and BEFORE any tool_results run. The
        // notice's api-error category is hardcoded `server_error` regardless of
        // the underlying finalize cause (cc `error:"server_error"`). Persisted
        // without the `tengu_query_error` telemetry (that fires only from the
        // top-level `model_error` catch, not this finalize path).
        if let Some(cause) = partial_finalize {
            let env = ApiErrorEnvelope {
                error: Some("server_error"),
                api_error_status: None,
                inner_stop_reason: None,
                truncated_after_output: true,
            };
            partial_finalize_notice_id = Some(
                crate::turn_loop::surface_api_error_notice(orch, cause.incomplete_notice(), env)
                    .await,
            );
        }

        // #5 aborted_streaming vs aborted_tools disambiguation (faithful port
        // of claude-code's TWO distinct abort checkpoints): query.ts:1015 runs
        // RIGHT AFTER `callModel`, BEFORE the tool-completion drive — an abort
        // observed there is `aborted_streaming` + `createUserInterruptionMessage({toolUse:false})`
        // (`[Request interrupted by user]`). query.ts:1485 runs AFTER the tool
        // drive — an abort observed only there is `aborted_tools` +
        // `createUserInterruptionMessage({toolUse:true})`
        // (`[Request interrupted by user for tool use]`). Capture the
        // checkpoint-1015 state HERE (before the drive); the single post-drive
        // abort check below picks the reason/message from it. The common
        // "interrupt during thinking/streaming" case (incl. no-tool responses)
        // lands as `aborted_streaming`, not the previous mislabel
        // `aborted_tools`. `None` token (plain `run_turn_streaming`) → always
        // false → byte-identical to before. The drive loop below still flushes
        // synthetic REJECT_MESSAGE tool_results (the executor was built with the
        // cancel token, so pending/queued tools reject rather than execute) —
        // matching ref's `getRemainingResults()` at the 1015 path.
        let aborted_during_stream = user_cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled);

        let (tool_prevent_continuation, post_tool_batch_calls) = orch
            .drive_streaming_tools(&mut exec, &pumped, &tool_use_parent_uuids, &assistant_uuid)
            .await;

        FinalizedStreamingIteration {
            pumped,
            assistant_id,
            tool_prevent_continuation,
            post_tool_batch_calls,
            pre_batch_mcp_tool_count,
            partial_finalize,
            partial_finalize_notice_id,
            aborted_during_stream,
        }
    }

    async fn run(self) -> Result<ConversationOutcome, OrchestratorError> {
        let Self {
            orch,
            prompt,
            images,
            user_cancel,
            message_id,
            transient_rewake,
        } = self;

        // Startup Responses WebSocket prewarm is strictly opportunistic. A real
        // user turn must never wait for an in-flight `generate=false` request to
        // finish before it can open its own stream.
        orch.abort_startup_responses_websocket_prewarm();

        // 0. Build the system prompt for THIS turn.
        // claude-code `nre` precedence: `--system-prompt` (override) wins; else
        // the `--agent`-adopted main-thread agent's prompt; else the default.
        let system_prompt: Option<String> = Some(orch.effective_system_prompt().await);

        // A side query left unfinished when the previous user turn ended was
        // keyed to that previous prompt. Never surface it against new intent.
        orch.discard_stale_prefetches().await;

        // 1. Append the user prompt (+ any pasted images) to session history.
        // `images` arrives already decoded (path-based callers ran `load_images`
        // first; the bridge converts inline `ImageRefDto`s straight to sources).
        let user_msg = ConversationMessage::user_with_images(
            message_id.unwrap_or_default(),
            prompt.to_string(),
            images,
        );
        let prior_message_id = {
            let mut s = orch.session.lock().await;
            let prior = s.history.last().map(ConversationMessage::id);
            if !transient_rewake {
                s.history.push(user_msg.clone());
            }
            prior
        };
        if !transient_rewake {
            orch.persist_message_to_jsonl(&user_msg).await;
        }

        // (/rewind) Snapshot the pre-turn file state IN MEMORY, keyed by this
        // user message, so `track_edit` (fired by Edit/Write/NotebookEdit during
        // the turn) records each file's pre-edit backup into it. The POPULATED
        // record is persisted to the transcript at TURN END (see below) — NOT
        // here: at turn start the backup map is empty (no edits yet), and
        // persisting it now would leave disk-based restore (`rewind_from_disk`,
        // which runs after the TUI unwinds and rebuilds the index from these
        // lines) with nothing to restore.
        let file_history_msg_id = (!transient_rewake).then(|| user_msg.id().as_uuid());
        if let (Some(fh), Some(message_id)) = (&orch.file_history, file_history_msg_id) {
            fh.make_snapshot(message_id).await;
        }

        // hooks B4: UserPromptSubmit (streaming twin). A Block aborts before the
        // first stream is opened. No-op when unregistered.
        if !transient_rewake && orch.fire_user_prompt_submit(prompt, user_msg.id()).await {
            return Ok(ConversationOutcome::StopHookPrevented {
                turn_count: 0,
                final_message_id: user_msg.id(),
            });
        }

        let mut loop_state =
            StreamingTurnState::new(orch, prior_message_id.unwrap_or_else(|| user_msg.id()));
        let final_message_id;
        loop {
            // MID-TURN DRAIN (claude-code query.ts ~1570-1580): drain BEFORE
            // every terminal top-of-loop guard, including `max_turns`. A message
            // can arrive while the previous model/tool step is running; returning
            // for the cap before this consume-once source is polled would discard
            // it. Claude Code 2.1.205 preserves that message when `--max-turns`
            // ends the turn, so inject it into the persisted history first.
            // With no source wired this remains a strict no-op.
            orch.drain_mid_turn_input().await;

            if orch.config.max_turns != 0 && loop_state.turn_count >= orch.config.max_turns {
                return Err(OrchestratorError::MaxTurnsReached {
                    max_turns: orch.config.max_turns,
                });
            }
            if orch.over_budget().await {
                return Err(OrchestratorError::MaxBudgetReached {
                    budget_nano_usd: orch.config.max_budget_nano_usd.unwrap_or(0),
                });
            }
            loop_state.turn_count = loop_state.turn_count.saturating_add(1);

            // DEFERRED-3 / esc-interrupt FIX: top-of-loop user-interrupt guard
            // (faithful port of claude-code `query.ts:1015` — the `aborted_streaming`
            // return). If the user-interrupt token is already set when we reach the
            // top of an iteration — a pre-cancel, or an abort that fired during the
            // previous iteration's streaming BEFORE any tool ran — we must STOP
            // BEFORE issuing the next `callModel`. claude-code consumes any
            // remaining streaming results then returns `aborted_streaming` with no
            // further sampling; here the previous iteration already drained its
            // results into history (the post-tools guard) or there were none, so we
            // simply break. This is the structural barrier that prevents a
            // Block-behavior tool on a post-interrupt continuation from ever
            // executing. `None` token → never fires → identical to before.
            if user_cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                let cost = orch.snapshot_cost_real().await;
                orch.output.emit_end_turn("aborted_streaming", &cost).await;
                // NOW-ABORT disambiguation: when the cancellation was driven by a
                // `Now`-priority enqueue (not a user Ctrl+C/ESC), the urgent
                // command IS the "interruption" and will be run next by the
                // between-turn drain — so DON'T inject the user-interrupt message
                // (which would mislabel the abort and pollute context). For a
                // plain user interrupt (the default when no reason flag is wired)
                // behavior is byte-identical to before: inject the message.
                // claude-code `query.ts:1046-1050`: `createUserInterruptionMessage`.
                if orch.cancel_reason_now()
                    != crate::prompt::mid_turn_input::CancelReason::QueueNowCommand
                {
                    orch.inject_user_message(INTERRUPT_MESSAGE).await;
                }
                final_message_id = loop_state.last_message_id;
                break;
            }

            let prepared =
                match Self::prepare_iteration(orch, &system_prompt, &user_cancel, &mut loop_state)
                    .await?
                {
                    PrepareStreamingOutcome::Ready(iteration) => iteration,
                    PrepareStreamingOutcome::Complete(message_id) => {
                        final_message_id = message_id;
                        break;
                    }
                };

            let pumped = Self::pump_iteration(
                orch,
                prepared,
                &system_prompt,
                &user_cancel,
                &mut loop_state,
            )
            .await;
            let pumped_iteration = match pumped {
                Err(error) => {
                    // `open_iteration` enables SDK frame buffering before the
                    // stream is opened. Every error path must release that
                    // session-scoped buffer or later tool frames disappear.
                    orch.set_tool_frame_buffering(false).await;
                    return Err(error);
                }
                Ok(PumpStreamingOutcome::Pumped(iteration)) => iteration,
                Ok(PumpStreamingOutcome::Complete(message_id)) => {
                    orch.set_tool_frame_buffering(false).await;
                    final_message_id = message_id;
                    break;
                }
            };

            let FinalizedStreamingIteration {
                pumped,
                assistant_id,
                tool_prevent_continuation,
                post_tool_batch_calls,
                pre_batch_mcp_tool_count,
                partial_finalize,
                partial_finalize_notice_id,
                aborted_during_stream,
            } = Self::finalize_iteration(
                orch,
                pumped_iteration,
                &system_prompt,
                &user_cancel,
                &mut loop_state,
            )
            .await;

            // DEFERRED-3 / esc-interrupt FIX: "we were aborted" — the single
            // post-drive abort checkpoint. Once the user-interrupt token has fired,
            // the executor above already drained the bare REJECT_MESSAGE
            // `tool_result`s into history (model-visible). The turn MUST now STOP —
            // claude-code returns with NO further `callModel`, honoring
            // REJECT_MESSAGE's "STOP what you are doing and wait for the user".
            // Looping into the `Some("tool_use") => continue` arm below would (1)
            // issue a wasted extra round-trip after every ESC and (2) let a
            // Block-behavior tool emitted on that continuation actually EXECUTE
            // (`abort_reason_for` returns `None` for Block tools) despite the
            // interrupt — both of which claude-code structurally prevents by
            // returning here first. `None` token (plain `run_turn_streaming`) →
            // never fires → identical to before.
            //
            // #5: the terminal reason + interrupt message depend on WHICH ref
            // checkpoint observed the abort (captured in `aborted_during_stream`
            // before the drive): an abort already set when the stream ended is
            // `aborted_streaming` / `[Request interrupted by user]` (query.ts:1015,
            // `toolUse:false`); an abort that fired only DURING the tool drive is
            // `aborted_tools` / `[Request interrupted by user for tool use]`
            // (query.ts:1485, `toolUse:true`).
            if user_cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                // Abort wins over a result-level end request. Results that were
                // allowed to finish (notably Block-behavior tools) may have
                // recorded one before this checkpoint, so drain the side table
                // even though the end-turn path below is intentionally skipped.
                let _ = orch
                    .take_pending_tool_result_turn_ends(
                        &pumped
                            .tool_uses
                            .iter()
                            .map(|tool_use| tool_use.id.clone())
                            .collect::<Vec<_>>(),
                    )
                    .await;
                let cost = orch.snapshot_cost_real().await;
                let (abort_reason, interrupt_message) = if aborted_during_stream {
                    ("aborted_streaming", INTERRUPT_MESSAGE)
                } else {
                    ("aborted_tools", INTERRUPT_MESSAGE_FOR_TOOL_USE)
                };
                orch.output.emit_end_turn(abort_reason, &cost).await;
                // NOW-ABORT disambiguation: a `Now`-driven cancellation means the
                // urgent queued command will run next via the between-turn drain —
                // DON'T inject the user-interrupt message. For a plain user
                // interrupt (the default with no reason flag wired) inject as
                // before. claude-code `query.ts:1046-1050`/`1501-1505`:
                // `createUserInterruptionMessage`.
                if orch.cancel_reason_now()
                    != crate::prompt::mid_turn_input::CancelReason::QueueNowCommand
                {
                    orch.inject_user_message(interrupt_message).await;
                }
                final_message_id = assistant_id;
                break;
            }

            // P1-04 (cc 2.1.199): a finalized partial ends the turn once its
            // dispatched tools have drained (above) and the incomplete-response
            // notice has been surfaced — cc `break e`s out of the stream loop after
            // yielding the notice; it does NOT re-enter the continuation logic. We
            // terminate here (reason `model_error`, matching the api-error catch)
            // rather than looping on the synthesized `tool_use`/`end_turn`, so the
            // user sees the partial + notice and can retry. The synthesized
            // stop_reason still rides on the persisted partial assistant line
            // (patched above) for resume fidelity.
            if partial_finalize.is_some() {
                if crate::turn_loop::truncated_response_recovery_eligible(
                    &orch.config.query_source,
                    orch.prompt_is_interactive(),
                ) && loop_state.recovery.max_output_tokens_recovery_count
                    < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
                {
                    let src = crate::config::sanitize_query_source(&orch.config.query_source);
                    let is_subagent = src.starts_with("agent") || src == "subagent";
                    let nudge = if is_subagent {
                        crate::turn_loop::TRUNCATED_RESPONSE_RECOVERY_NUDGE_SUBAGENT
                    } else {
                        crate::turn_loop::TRUNCATED_RESPONSE_RECOVERY_NUDGE_MAIN
                    };
                    orch.inject_meta_user_message(nudge).await;
                    loop_state.recovery.max_output_tokens_recovery_count = loop_state
                        .recovery
                        .max_output_tokens_recovery_count
                        .saturating_add(1);
                    loop_state.recovery.max_output_tokens_override = None;
                    continue;
                }
                // A finalized partial is terminal before normal tool-result
                // disposition. Do not leave an end marker from a completed
                // result live in the session-scoped side table.
                let _ = orch
                    .take_pending_tool_result_turn_ends(
                        &pumped
                            .tool_uses
                            .iter()
                            .map(|tool_use| tool_use.id.clone())
                            .collect::<Vec<_>>(),
                    )
                    .await;
                let cost = orch.snapshot_cost_real().await;
                orch.output.emit_end_turn("model_error", &cost).await;
                final_message_id = partial_finalize_notice_id.unwrap_or(assistant_id);
                break;
            }

            match orch
                .decide_streaming_disposition(
                    &mut loop_state,
                    &pumped,
                    assistant_id,
                    tool_prevent_continuation,
                    post_tool_batch_calls,
                    pre_batch_mcp_tool_count,
                )
                .await?
            {
                StreamingIterationDisposition::Continue => continue,
                StreamingIterationDisposition::Complete(message_id) => {
                    // Pending input can arrive while the final response is
                    // streaming, after the top-of-loop drain has already run.
                    // Give it one final drain before committing the natural
                    // end-turn so it continues this same turn.
                    if orch.drain_mid_turn_input().await {
                        continue;
                    }
                    final_message_id = message_id;
                    break;
                }
                StreamingIterationDisposition::ForcedComplete(message_id) => {
                    final_message_id = message_id;
                    break;
                }
                StreamingIterationDisposition::Return(outcome) => return Ok(outcome),
            }
        }

        // (/rewind) Persist THIS turn's now-populated file-history snapshot to the
        // transcript — `track_edit` filled its backup map (pre-edit content of
        // every file Edit/Write/NotebookEdit touched) during the turn above.
        // Restore (`rewind_from_disk`) and `--resume` rebuild the index from
        // these lines, so the record must carry the backups, not the empty map
        // it had at turn start. Persisted unconditionally (an edit-free turn
        // still records a restore point for conversation-only rewind).
        if let (Some(fh), Some(file_history_msg_id)) = (&orch.file_history, file_history_msg_id) {
            if let (Some(record), Some(writer)) = (
                fh.snapshot_record(file_history_msg_id),
                &orch.transcript.jsonl_writer,
            ) {
                let session_id = orch.session.lock().await.session_id;
                let session_uuid = session_id.as_uuid().to_string();
                let line = session::file_history::snapshot_line_json(&session_uuid, &record);
                if let Err(error) = writer.append_file_history_snapshot(&line).await {
                    orch.record_transcript_append_failure(
                        &session_id.to_string(),
                        "file_history_snapshot",
                        &error,
                    )
                    .await;
                }
            }
        }

        Ok(ConversationOutcome::EndTurn {
            turn_count: loop_state.turn_count,
            final_message_id,
        })
    }
}

impl ConversationOrchestrator {
    /// Read the active turn's [`crate::prompt::mid_turn_input::CancelReason`] from
    /// the wired flag, defaulting to `UserInterrupt` when no flag is wired (so an
    /// un-wired turn always takes the user-interrupt branch — today's behavior).
    fn cancel_reason_now(&self) -> crate::prompt::mid_turn_input::CancelReason {
        self.cancel_reason.get().map_or(
            crate::prompt::mid_turn_input::CancelReason::UserInterrupt,
            |f| f.get(),
        )
    }

    /// Mid-turn drain step: pull any queued main-thread, non-slash input from the
    /// wired source and inject it as a plain (non-meta) user message so the next
    /// sampling sees it — CC 2.1.207's `queued_command` guard leaves plain human
    /// input non-meta (`r!==void 0&&!Ree(r)||e.isMeta` → `{}`). A strict no-op
    /// when no source is wired (the default) or the queue
    /// is empty. Returns `true` if anything was injected (for the caller's
    /// observability — the loop continues regardless). Mirrors claude-code's
    /// `joinPromptValues` + meta-prompt injection at query.ts ~1570-1580.
    async fn drain_mid_turn_input(&self) -> bool {
        let mut injected = self.drain_peer_inbox(true).await;
        let Some(source) = self.mid_turn_input.get() else {
            return injected;
        };
        // Loop so a burst of consecutive enqueues all land before the next call.
        // The production source ([`MsgQueueMidTurnInput`]) is consume-once — it
        // REMOVES the commands it returns each call — so it self-terminates after
        // it has drained the queue. The bound below is a defensive guard against a
        // MALFUNCTIONING source impl (the trait is public; a buggy impl that fails
        // to consume could otherwise return `Some` forever and hang the turn loop):
        // we cap the per-iteration drain at a generous fixed number of batches so a
        // single drain step can never spin unboundedly.
        const MAX_DRAIN_BATCHES: usize = 1024;
        for _ in 0..MAX_DRAIN_BATCHES {
            match source.take_mid_turn_input().await {
                Some(text) => {
                    let wrapped = Self::wrap_mid_turn_user_message(&text);
                    self.inject_user_message(&wrapped).await;
                    injected = true;
                }
                None => break,
            }
        }
        injected
    }

    /// Wrap joined mid-turn user input in the 2.1.206 envelope (`YAt`, binary
    /// @225654300) before injection. The `human` / `auto-continuation` / unset
    /// arm — the only source the port's `MsgQueueMidTurnInput` produces (all
    /// mid-turn input is user-typed) — prefixes `jca` ("The user sent a new
    /// message while you were working:\n") and appends the explainer. Em-dash is
    /// U+2014; "Claude Code" -> "LingXi" per the brand rebrand. (206 dropped the
    /// 201 "IMPORTANT: After completing your current task…" suffix — 0 hits in
    /// 206.) A non-user source would instead use
    /// `"[MESSAGE FROM NON-USER SOURCE - NOT USER INPUT]\n{text}"`, but the port
    /// has no such mid-turn source today.
    #[must_use]
    fn wrap_mid_turn_user_message(text: &str) -> String {
        format!(
            "The user sent a new message while you were working:\n{text}\n\nThis is how LingXi surfaces messages the user sends mid-turn \u{2014} within the running turn, often alongside the next tool result, rather than as a separate conversation turn. Address the message above as you continue this turn."
        )
    }

    /// Inject an engine META user message (`isMeta:true`) into both the live
    /// session history and the JSONL persistence stream — the recovery /
    /// continuation nudges (thinking-only, malformed-tool retry) that claude-code
    /// creates via `createUserMessage({ …, isMeta: true })`. Persisting stamps the
    /// top-level `isMeta:true` envelope flag (see `to_jsonl_message`), so these
    /// lines are skipped by title / first-prompt / fork-name / visible-count
    /// extraction, exactly as in CC 2.1.207.
    async fn inject_meta_user_message(&self, text: &str) {
        self.inject_user_text(text, true).await;
    }

    /// Inject a PLAIN (non-meta) user text message. Used for interrupt markers
    /// (`[Request interrupted by user]`) and mid-turn drained HUMAN input, which
    /// CC 2.1.207 persists WITHOUT `isMeta` — interrupt lines are built with no
    /// `isMeta` field, and queued human input is non-meta per the `queued_command`
    /// guard (`r!==void 0&&!Ree(r)||e.isMeta` → `{}` for plain human input).
    pub(super) async fn inject_user_message(&self, text: &str) {
        self.inject_user_text(text, false).await;
    }

    /// Drain accepted cross-session inbox lines into history as meta user-role
    /// `<cross-session-message>` envelopes (2.1.232 `isMeta:!0`). Policy is
    /// applied at receive; this only injects already-accepted bodies.
    pub(crate) async fn drain_peer_inbox(&self, mid_turn: bool) -> bool {
        let reminders = platform_api::live_sessions::take_accepted_peer_reminders(mid_turn);
        if reminders.is_empty() {
            return false;
        }
        for body in reminders {
            // Peer / receipt text is never user intent (2.1.232 `isMeta:!0`).
            self.inject_user_text(&body, true).await;
        }
        true
    }

    async fn inject_user_text(&self, text: &str, is_meta: bool) {
        let msg = if is_meta {
            ConversationMessage::user_meta(MessageId::new(), text.to_string())
        } else {
            ConversationMessage::user(MessageId::new(), text.to_string())
        };
        {
            let mut s = self.session.lock().await;
            s.history.push(msg.clone());
        }
        self.persist_message_to_jsonl(&msg).await;
    }

    async fn maybe_continue_for_budget(
        &self,
        budget: Option<&mut BudgetTracker>,
        recovery: &mut RecoveryState,
        global_turn_tokens: u64,
    ) -> bool {
        // No tracker → feature off / no budget → never continue (parity no-op).
        let Some(tracker) = budget else {
            return false;
        };
        // The orchestrator turn loop has no sub-agent `agentId` concept here
        // (that lives in the agent-spawn path); pass `None`, matching the main
        // query loop where `toolUseContext.agentId` is undefined for the root.
        let decision =
            check_token_budget(tracker, None, self.config.token_budget, global_turn_tokens);
        match decision {
            TokenBudgetDecision::Continue {
                nudge_message,
                continuation_count,
                pct,
                turn_tokens,
                budget,
            } => {
                tracing::info!(
                    event = "token_budget_continuation",
                    continuation_count,
                    pct,
                    turn_tokens,
                    budget,
                    "token budget continuation #{continuation_count}: {pct}% ({turn_tokens} / {budget})"
                );
                // Inject the continuation nudge as a META user message
                // (`createUserMessage({content: nudgeMessage, isMeta: true})`,
                // query.ts:1327). It persists with top-level `isMeta:true` and is
                // skipped by title / first-prompt / visible-count extraction.
                let nudge_msg = ConversationMessage::user_meta(MessageId::new(), nudge_message);
                {
                    let mut s = self.session.lock().await;
                    s.history.push(nudge_msg.clone());
                }
                self.persist_message_to_jsonl(&nudge_msg).await;
                // Reset the A1 recovery count on each budget continuation
                // (TS `query.ts:1332` `maxOutputTokensRecoveryCount: 0` +
                // `maxOutputTokensOverride: undefined`; REC.A1: a fresh recovery
                // episode may escalate again).
                recovery.reset_max_output_tokens_recovery();
                true
            }
            TokenBudgetDecision::Stop { completion_event } => {
                if let Some(ev) = completion_event {
                    if ev.diminishing_returns {
                        tracing::info!(
                            event = "token_budget_completed",
                            pct = ev.pct,
                            "token budget early stop: diminishing returns at {}%",
                            ev.pct
                        );
                    }
                    tracing::info!(
                        event = "token_budget_completed",
                        continuation_count = ev.continuation_count,
                        pct = ev.pct,
                        turn_tokens = ev.turn_tokens,
                        budget = ev.budget,
                        diminishing_returns = ev.diminishing_returns,
                        duration_ms = u64::try_from(ev.duration_ms).unwrap_or(u64::MAX),
                    );
                }
                false
            }
        }
    }

    /// B6-T1: status-change emit parity for a turn that DIED on a rate limit.
    ///
    /// claude-code's terminal catch handler `extractQuotaStatusFromError`
    /// (claudeAiLimits.ts:487) forces the limits to `status='rejected'` and
    /// runs `emitStatusChange` (ts:509-511) ALONGSIDE rendering the terminal
    /// error copy — so the TUI shows the rate-limit banner (+ the T5 overage
    /// notice) next to the assistant error message. By the time a turn driver
    /// surfaces a terminal `RateLimited`, the drive fn has already PROMOTED
    /// the staged 429 into `self.api`'s caches (via
    /// [`crate::provider_adapter::ProviderApiAdapter::promote_pending_429`]),
    /// so these emit-on-change helpers flow the rejected snapshot (+ raw
    /// windows) out as an [`platform_api::OutputEvent::RateLimit`] (+ `RawUtilization`).
    ///
    /// Gated on the rate-limited discriminant a terminal error carries BEFORE
    /// enrichment — `ApiCall(RateLimited)` (batched) or `Streaming(RateLimited)`
    /// (stream connect-phase) — so a non-429 terminal never emits. Called
    /// AFTER the drive fn returned (promotion done) and BEFORE
    /// [`Self::enrich_api_error`].
    ///
    /// B1 — DOCUMENTED DIVERGENCE (not parity): TS forces `status='rejected'`
    /// and emits even on a HEADERLESS terminal 429 (claudeAiLimits.ts:506-507,
    /// outside the headers block). The Rust `last_rate_limit` has no "bare
    /// rejected, no windows" representation, so a headerless terminal 429
    /// promotes nothing and these emit-on-change helpers are no-ops — the
    /// terminal error copy already conveys the rejection.
    async fn emit_terminal_rate_limit_if_changed<T>(&self, result: &Result<T, OrchestratorError>) {
        if let Err(
            OrchestratorError::ApiCall(LlmError::RateLimited { .. })
            | OrchestratorError::Streaming(LlmError::RateLimited { .. }),
        ) = result
        {
            self.emit_rate_limit_if_changed().await;
            self.emit_raw_utilization_if_changed().await;
        }
    }

    /// Drive one user prompt through the turn loop until `end_turn` or
    /// `max_turns` is exhausted.
    ///
    /// Emits 3 telemetry events:
    /// - [`orch_events::CONVERSATION_STARTED`] at entry
    /// - [`orch_events::CONVERSATION_COMPLETED`] on success
    /// - [`orch_events::CONVERSATION_FAILED`] on error
    pub async fn run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError> {
        let _turn_guard = self.turn_gate.lock().await;
        tracing::info!(
            event = orch_events::CONVERSATION_STARTED,
            prompt_len = prompt.len()
        );
        let result = telemetry::otel::with_turn_span("lingxi.orchestrator.turn", async {
            platform_api::session_flags::scope_non_interactive_session(
                !self.prompt_is_interactive(),
                self.try_run_turn(prompt),
            )
            .await
        })
        .await;
        self.emit_terminal_rate_limit_if_changed(&result).await;
        let result = result.map_err(|e| self.enrich_api_error(e));
        // ConversationOutcome is #[non_exhaustive] so future variants will
        // also log as Completed when the only existing variant is EndTurn.
        match &result {
            Ok(
                ConversationOutcome::EndTurn { turn_count, .. }
                | ConversationOutcome::StopHookPrevented { turn_count, .. },
            ) => {
                tracing::info!(
                    event = orch_events::CONVERSATION_COMPLETED,
                    turn_count = *turn_count
                );
            }
            Err(err) => {
                tracing::error!(
                    event = orch_events::CONVERSATION_FAILED,
                    reason = %err
                );
            }
        }
        result
    }

    async fn try_run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError> {
        // 0. Build the system prompt for THIS turn.
        // claude-code `nre` precedence: `--system-prompt` (override) wins; else
        // the `--agent`-adopted main-thread agent's prompt; else the default.
        let system_prompt: Option<String> = Some(self.effective_system_prompt().await);

        // A side query left unfinished when the previous user turn ended was
        // keyed to that previous prompt. Never surface it against new intent.
        self.discard_stale_prefetches().await;

        // 1. Append the user prompt to session history.
        let user_msg = ConversationMessage::user(MessageId::new(), prompt.to_string());
        {
            let mut s = self.session.lock().await;
            s.history.push(user_msg.clone());
        }
        self.persist_message_to_jsonl(&user_msg).await;

        // hooks B4: fire UserPromptSubmit. A Block decision aborts the turn
        // BEFORE any API call (TS prompt-ingress hook). No-op when unregistered.
        if self.fire_user_prompt_submit(prompt, user_msg.id()).await {
            return Ok(ConversationOutcome::StopHookPrevented {
                turn_count: 0,
                final_message_id: user_msg.id(),
            });
        }

        // 2. Turn-by-turn driver.
        // A1: per-conversation max_output_tokens recovery bookkeeping carried
        // across turn-steps (the 3-retry limit is consecutive).
        let mut recovery = RecoveryState::default();
        // hooks B4: Stop-hook re-entry guard. Set true after a Stop hook blocks
        // and we loop once more; a second block then passes (no infinite loop).
        let mut stop_hook_active = false;
        // #2 consecutive Stop-hook block counter (binary `stopHookBlockingCount`):
        // bumped per block; ends the turn via the cap once it would exceed
        // LINGXI_STOP_HOOK_BLOCK_CAP (default 8). Fresh per turn-driver run.
        let mut stop_hook_blocking_count: u32 = 0;
        // A3: token-budget continuation bookkeeping. `Some` only when the gate
        // is enabled AND a budget is set; otherwise the budget check is a
        // NO-OP and the loop stops at the first `end_turn` (parity default).
        let mut budget = self.new_budget_tracker();
        let mut global_turn_tokens: u64 = 0;
        // Turn-start output baseline (claude-code `xtr` via `UAc(e)`): snapshot the
        // cumulative pool as this turn begins, so a workflow launched this turn
        // reads `budget.spent()` = `pool - baseline` (output spent THIS turn).
        self.compaction_runtime.turn_start_output_baseline.store(
            self.compaction_runtime
                .output_token_pool
                .load(std::sync::atomic::Ordering::Relaxed),
            std::sync::atomic::Ordering::Relaxed,
        );
        let mut turn_count: u32 = 0;
        let final_message_id;
        loop {
            // Streaming twin already drains here (query.ts ~1570). Claude Code
            // has one main loop; the batched print path must consume mid-turn
            // input before max_turns / budget so a queued message is not dropped.
            self.drain_mid_turn_input().await;
            if self.config.max_turns != 0 && turn_count >= self.config.max_turns {
                return Err(OrchestratorError::MaxTurnsReached {
                    max_turns: self.config.max_turns,
                });
            }
            if self.over_budget().await {
                return Err(OrchestratorError::MaxBudgetReached {
                    budget_nano_usd: self.config.max_budget_nano_usd.unwrap_or(0),
                });
            }
            turn_count = turn_count.saturating_add(1);

            let (step, output_tokens) = execute_one_turn_with_recovery_tracked(
                self,
                system_prompt.as_deref(),
                Some(&mut recovery),
            )
            .await?;
            // A3: accumulate the running per-turn output tokens (TS
            // `getTurnOutputTokens()`). No-op for accounting when budget is off.
            global_turn_tokens = global_turn_tokens.saturating_add(output_tokens);
            match step {
                TurnStepOutcome::Continue => continue,
                TurnStepOutcome::Ended {
                    final_message_id: id,
                    stop_reason,
                    allow_budget_continuation,
                    tool_requested_end,
                } => {
                    // hooks B4: fire Stop hooks BEFORE the token-budget check
                    // (order: recovery → stop-hooks → token-budget, TS
                    // `query.ts:1262-1308`).
                    if tool_requested_end {
                        self.fire_tool_result_end_stop_hooks(&stop_reason, stop_hook_active)
                            .await;
                    } else {
                        match self
                            .handle_stop_at_end(
                                &stop_reason,
                                &mut stop_hook_active,
                                &mut stop_hook_blocking_count,
                                turn_count,
                                id,
                            )
                            .await
                        {
                            StopHookFlow::Terminate(outcome) => return Ok(outcome),
                            StopHookFlow::TerminateMaxTurns => {
                                return Err(OrchestratorError::MaxTurnsReached {
                                    max_turns: self.config.max_turns,
                                });
                            }
                            StopHookFlow::LoopAgain => {
                                // RECOV.4: a Stop hook forced the loop to continue —
                                // reset the max_output_tokens recovery bookkeeping so the
                                // continued turn starts a fresh escalation episode (TS
                                // `query.ts:1291` sets `maxOutputTokensRecoveryCount: 0`
                                // + `maxOutputTokensOverride: undefined` on the
                                // stop-hook-blocking continuation).
                                recovery.reset_max_output_tokens_recovery();
                                continue;
                            }
                            StopHookFlow::FallThrough => {}
                        }
                    }
                    // A3: at a natural end-of-turn, consult the token budget. If
                    // it says `continue`, inject the meta nudge, reset the A1
                    // recovery count (per `query.ts:1332`), and loop again
                    // instead of breaking. When budget is off this is a no-op.
                    if allow_budget_continuation
                        && stop_reason == "end_turn"
                        && self
                            .maybe_continue_for_budget(
                                budget.as_mut(),
                                &mut recovery,
                                global_turn_tokens,
                            )
                            .await
                    {
                        continue;
                    }
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn(&stop_reason, &cost).await;
                    final_message_id = id;
                    break;
                }
            }
        }

        Ok(ConversationOutcome::EndTurn {
            turn_count,
            final_message_id,
        })
    }

    /// Drive one user prompt through the STREAMING turn loop. Mirrors
    /// the contract of [`Self::run_turn`] but consumes SSE events as
    /// they arrive (per-token `OutputStream::emit_text`) and dispatches
    /// `tool_use` blocks the moment their `content_block_stop` event is
    /// received.
    ///
    /// Emits 2 streaming-specific telemetry events at the boundaries:
    /// - [`orch_events::TURN_STREAMING_STARTED`] at entry.
    /// - [`orch_events::TURN_STREAMING_COMPLETED`] after success.
    ///
    /// On error, the existing [`orch_events::CONVERSATION_FAILED`] is
    /// reused (no new error event in M5-04).
    pub async fn run_turn_streaming(
        &self,
        prompt: &str,
    ) -> Result<ConversationOutcome, OrchestratorError> {
        let _turn_guard = self.turn_gate.lock().await;
        tracing::info!(
            event = orch_events::TURN_STREAMING_STARTED,
            prompt_len = prompt.len()
        );
        // DEFERRED-3: the plain (non-cancelable) streaming entry has no granular
        // user-interrupt token → `None` (behaviour byte-identical to before).
        let result = telemetry::otel::with_turn_span("lingxi.orchestrator.turn.streaming", async {
            platform_api::session_flags::scope_non_interactive_session(
                !self.prompt_is_interactive(),
                Box::pin(self.try_run_turn_streaming(prompt, Vec::new(), None, None, false)),
            )
            .await
        })
        .await;
        self.emit_terminal_rate_limit_if_changed(&result).await;
        let result = result.map_err(|e| self.enrich_api_error(e));
        match &result {
            Ok(
                ConversationOutcome::EndTurn { turn_count, .. }
                | ConversationOutcome::StopHookPrevented { turn_count, .. },
            ) => {
                tracing::info!(
                    event = orch_events::TURN_STREAMING_COMPLETED,
                    turn_count = *turn_count
                );
            }
            Err(err) => {
                tracing::error!(
                    event = orch_events::CONVERSATION_FAILED,
                    reason = %err
                );
            }
        }
        result
    }

    /// Start a model turn solely to deliver completed async-hook responses.
    ///
    /// No synthetic human prompt is added to history or JSONL. The completed
    /// hook buffer contributes the transient `async_hook_response` meta user
    /// message during request assembly, so the provider still receives a valid
    /// user boundary while the transcript remains faithful.
    pub async fn run_async_hook_rewake(&self) -> Result<TurnOutcome, OrchestratorError> {
        let _turn_guard = self.turn_gate.lock().await;
        self.output.emit_turn_started().await;
        let result = platform_api::session_flags::scope_non_interactive_session(
            !self.prompt_is_interactive(),
            Box::pin(self.try_run_turn_streaming("", Vec::new(), None, None, true)),
        )
        .await;
        self.emit_terminal_rate_limit_if_changed(&result).await;
        match result {
            Ok(
                ConversationOutcome::EndTurn { .. } | ConversationOutcome::StopHookPrevented { .. },
            ) => Ok(TurnOutcome::EndTurn),
            Err(OrchestratorError::MaxTurnsReached { .. }) => Ok(TurnOutcome::MaxTurns),
            Err(error) => {
                let error = self.enrich_api_error(error);
                self.output
                    .emit_system_notice(&error.to_string(), true)
                    .await;
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn("error", &cost).await;
                Err(error)
            }
        }
    }

    async fn drive_streaming_tools(
        &self,
        exec: &mut crate::streaming_executor::StreamingToolExecutor<'_>,
        pumped: &crate::streaming_loop::PumpedTurn,
        tool_use_parent_uuids: &std::collections::HashMap<protocol::ToolUseId, String>,
        assistant_uuid: &Option<String>,
    ) -> (bool, Vec<hooks::events::PostToolBatchCall>) {
        // 5. Drive tools through the StreamingToolExecutor (faithful port of
        //    claude-code's `StreamingToolExecutor` + `query.ts:826-862`).
        //    Each tool runs the same hook + permission + registry pipeline
        //    (via `dispatch_tool_uses_tracked` per tool) under concurrency
        //    control, and EACH result is persisted as its OWN `user` message
        //    parented to the originating assistant (per-result,
        //    assistant-parented topology), in RECEIVED order — diverging from
        //    the old single-batched-user-message shape and matching the TS
        //    `sessionStorage` `sourceToolAssistantUUID → parentUuid` mapping.
        let mut prevent_continuation = false;
        let mut post_tool_batch_calls = Vec::new();
        if !pumped.tool_uses.is_empty() {
            // The executor (`exec`) was created BEFORE the stream and its
            // tools were registered + dispatched MID-STREAM by
            // `pump_stream_with_executor` (claude-code `query.ts:837-844`);
            // on the non-streaming 529-fallback path it was rebuilt above and
            // its tools added from the fallback response. Here we only DRIVE
            // it to completion + persist — `add_tool` no longer happens
            // post-stream on the normal path.
            //
            // Drive to completion, persisting each result IN RECEIVED ORDER
            // as its own user message parented to the originating assistant.
            let mut all_modifiers: Vec<tool_api::ContextModifier> = Vec::new();
            loop {
                // (no-op unless a Bash sibling errored / the turn discarded)
                // A queued tool cancelled here gets the SAME
                // `user_interrupted` synthetic `drain_one` substitutes, and
                // claude-code stamps that message `user-rejected`
                // (`createSyntheticErrorMessage`, 2.1.220 @232972524), so
                // record the kind for the persisted tool_result line.
                for (id, reason, is_mcp) in exec.apply_abort_to_pending() {
                    if reason == crate::streaming_executor::AbortReason::UserInterrupted {
                        self.record_tool_denial_kind(
                            &id,
                            if is_mcp {
                                "interrupted"
                            } else {
                                "user-rejected"
                            },
                        )
                        .await;
                    }
                    // O1: the synthetic that survives carries claude's own
                    // short `toolUseResult` literal, not the block's text.
                    self.record_tool_use_result(
                        &id,
                        crate::streaming_executor::synthetic_tool_use_result_for_tool(
                            reason, is_mcp,
                        ),
                    )
                    .await;
                }
                exec.process_queue();
                // persist whatever just completed, in order
                for drained in exec.take_newly_completed() {
                    prevent_continuation |= drained.prevent_continuation;
                    post_tool_batch_calls.extend(drained.post_tool_batch_calls);
                    // Parent this tool_result to ITS tool_use's per-block
                    // assistant line uuid (TS `sourceToolAssistantUUID`),
                    // falling back to the turn's last assistant block uuid if
                    // the id isn't in the map (defensive — e.g. an append that
                    // failed and was skipped above).
                    let parent_uuid = match &drained.block {
                        ContentBlock::ToolResult { tool_use_id, .. } => tool_use_parent_uuids
                            .get(tool_use_id)
                            .cloned()
                            .or_else(|| assistant_uuid.clone()),
                        _ => assistant_uuid.clone(),
                    };
                    // Release this tool's SDK frame HERE — `take_newly_completed`
                    // yields in received order, and `drained.block` is the
                    // post-substitution content, so a cancelled tool reports
                    // its synthetic rather than the real outcome the executor
                    // discarded. A queued-then-cancelled tool never dispatched
                    // and so has no buffered frame; it gets one from here,
                    // where previously it got none at all.
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } = &drained.block
                    {
                        self.release_tool_frame(tool_use_id, &drained.tool, content, *is_error)
                            .await;
                    }
                    // O3: keep this result's tool id so its hook
                    // `attachment` lines can be flushed immediately after
                    // its tool_result — claude's stream order.
                    let drained_tool_use_id = match &drained.block {
                        ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                        _ => None,
                    };
                    let user_msg = ConversationMessage::User {
                        id: MessageId::new(),
                        content: vec![drained.block],
                        is_meta: false,
                        is_compact_summary: false,
                        is_visible_in_transcript_only: false,
                    };
                    {
                        let mut s = self.session.lock().await;
                        s.history.push(user_msg.clone());
                    }
                    // `bash_output_audience_note` (NEW in 2.1.238, gate `kpm`
                    // @294267076, emission @294300924): a Bash result whose
                    // stdout is longer than the few lines the user's terminal
                    // shows gets a one-line note telling the model the user
                    // did NOT see it. The oracle pushes it into the message
                    // STREAM right after the `tool_result` line, so it lands
                    // in history + JSONL like any other attachment message.
                    // Gated on the model capability
                    // `bash_output_audience_note` / the
                    // `CLAUDE_CODE_BASH_OUTPUT_AUDIENCE_NOTE` env var; the
                    // port has no capability table ⇒ DEFAULT OFF ⇒ strict
                    // no-op, so the locked streaming fixtures are unaffected.
                    //
                    // Computed BEFORE the persist below: the JSONL writer
                    // CONSUMES the recorded `toolUseResult` (it moves into
                    // the line's `toolUseResult` field), and the gate needs
                    // that payload's `stdout`.
                    let audience_note = match &drained_tool_use_id {
                        Some(id) => self.bash_output_audience_note_message(id).await,
                        None => None,
                    };
                    self.persist_message_to_jsonl_with_parent(&user_msg, parent_uuid)
                        .await;
                    if let Some(note) = audience_note {
                        {
                            let mut s = self.session.lock().await;
                            s.history.push(note.clone());
                        }
                        self.persist_message_to_jsonl(&note).await;
                    }
                    if let Some(id) = &drained_tool_use_id {
                        self.flush_hook_attachments(id).await;
                    }
                    // SKILLEXEC.3 (streaming): replay tool-injected
                    // `new_messages` (the Skill tool's expanded prompt) right
                    // after the tool_result, recording each injected message's
                    // id → originating tool_use_id into the in-memory
                    // `injected_message_sources` side-table (faithful port of
                    // TS `sourceToolUseID`). These chain normally (NOT a parent
                    // override) — matching the batched path — so the JSONL
                    // bytes stay byte-identical. Empty for every non-skill tool
                    // → strict no-op.
                    for (m, tool_use_id) in drained.injected {
                        let is_ephemeral_rendering = m.is_meta();
                        {
                            let mut s = self.session.lock().await;
                            s.history.push(m.clone());
                            s.injected_message_sources.insert(m.id(), tool_use_id);
                        }
                        // O3: an `is_meta` injected message is the
                        // EPHEMERAL rendering of the attachment flushed
                        // above — persisting it would duplicate the record.
                        if is_ephemeral_rendering {
                            continue;
                        }
                        self.persist_message_to_jsonl(&m).await;
                    }
                    all_modifiers.extend(drained.modifiers);
                }
                // Guaranteed-progress shape (mirrors the test-only
                // `run_to_completion`): an empty in-flight set after
                // `process_queue` means no Queued tool remains startable
                // (process_queue starts any runnable one) and nothing is
                // executing — so every tool is done + drained above. Break
                // here, else drain one future below. Never spins: each
                // iteration either breaks or `.await`s a completion.
                if exec.inflight_is_empty() {
                    break;
                }
                exec.drain_one().await;
            }
            // SKILLEXEC.3 (model scope, streaming twin): fold this turn's
            // `context_modifier`s and switch `session.model` if a skill
            // declared a `model:` override. Applied AFTER all results +
            // injected messages, so there is no race on `session.model`.
            // Empty for every non-`model:` tool → strict no-op.
            crate::turn_loop::apply_model_context_modifiers(self, all_modifiers).await;
        }
        // Drive finished: stop holding frames.
        self.set_tool_frame_buffering(false).await;
        // Concurrent safe tools can finish out of order. PostToolBatch is
        // defined from the assistant's original `toolUseBlocks.map(...)`, so
        // restore received order before firing the single batch event.
        let mut ordered_batch_calls = Vec::with_capacity(post_tool_batch_calls.len());
        for tool_use in &pumped.tool_uses {
            if let Some(index) = post_tool_batch_calls
                .iter()
                .position(|call| call.tool_use_id == tool_use.id)
            {
                ordered_batch_calls.push(post_tool_batch_calls.remove(index));
            }
        }
        // Defensive preservation for a future synthetic call whose id is not
        // represented in `pumped.tool_uses`.
        ordered_batch_calls.extend(post_tool_batch_calls);
        (prevent_continuation, ordered_batch_calls)
    }

    async fn finish_natural_streaming_end(
        &self,
        loop_state: &mut StreamingTurnState,
        assistant_id: MessageId,
    ) -> Result<StreamingIterationDisposition, OrchestratorError> {
        // hooks B4: Stop hooks BEFORE the budget check (streaming
        // twin; order recovery → stop-hooks → token-budget).
        match self
            .handle_stop_at_end(
                "end_turn",
                &mut loop_state.stop_hook_active,
                &mut loop_state.stop_hook_blocking_count,
                loop_state.turn_count,
                assistant_id,
            )
            .await
        {
            StopHookFlow::Terminate(outcome) => {
                return Ok(StreamingIterationDisposition::Return(outcome));
            }
            StopHookFlow::TerminateMaxTurns => {
                return Err(OrchestratorError::MaxTurnsReached {
                    max_turns: self.config.max_turns,
                });
            }
            StopHookFlow::LoopAgain => {
                // RECOV.4: a Stop hook forced the loop to continue —
                // reset the max_output_tokens recovery bookkeeping so the
                // continued turn starts a fresh escalation episode (TS
                // `query.ts:1291` sets `maxOutputTokensRecoveryCount: 0`
                // + `maxOutputTokensOverride: undefined` on the
                // stop-hook-blocking continuation).
                loop_state.recovery.reset_max_output_tokens_recovery();
                return Ok(StreamingIterationDisposition::Continue);
            }
            StopHookFlow::FallThrough => {}
        }
        // A3: token-budget continuation (streaming twin). On a
        // natural end, consult the budget; on `continue`, inject the
        // meta nudge, reset the A1 recovery count, and loop again.
        if self
            .maybe_continue_for_budget(
                loop_state.budget.as_mut(),
                &mut loop_state.recovery,
                loop_state.global_turn_tokens,
            )
            .await
        {
            return Ok(StreamingIterationDisposition::Continue);
        }
        let cost = self.snapshot_cost_real().await;
        self.output.emit_end_turn("end_turn", &cost).await;
        return Ok(StreamingIterationDisposition::Complete(assistant_id));
    }

    async fn finish_tool_requested_streaming_end(
        &self,
        loop_state: &mut StreamingTurnState,
        assistant_id: MessageId,
    ) -> Result<StreamingIterationDisposition, OrchestratorError> {
        self.fire_tool_result_end_stop_hooks("end_turn", loop_state.stop_hook_active)
            .await;
        let cost = self.snapshot_cost_real().await;
        self.output.emit_end_turn("end_turn", &cost).await;
        Ok(StreamingIterationDisposition::ForcedComplete(assistant_id))
    }

    async fn decide_streaming_disposition(
        &self,
        loop_state: &mut StreamingTurnState,
        pumped: &crate::streaming_loop::PumpedTurn,
        assistant_id: MessageId,
        tool_prevent_continuation: bool,
        post_tool_batch_calls: Vec<hooks::events::PostToolBatchCall>,
        pre_batch_mcp_tool_count: usize,
    ) -> Result<StreamingIterationDisposition, OrchestratorError> {
        // #78 nudge guard `!Pt(ce)` (streaming twin): suppress the
        // thinking-only nudge during a StructuredOutput exchange. Computed
        // before the match (a match guard cannot `.await` the session lock);
        // reused by all three streaming nudge sites (end_turn / stop_sequence
        // / missing). The current assistant response is already in `history`.
        let prior_structured_output = {
            let s = self.session.lock().await;
            crate::turn_loop::prior_assistant_used_structured_output(&s.history)
        };

        // 6. Decide loop disposition.
        match pumped.stop_reason.as_deref() {
            // #1 needsFollowUp gate (claude-code `query.ts:554-558`/`832-835`/
            // `1062`): continuation is keyed on tool-block PRESENCE, NOT the raw
            // `stop_reason` string (the ref notes `stop_reason == "tool_use"` is
            // "unreliable"). Any response that dispatched tool_use blocks runs
            // the tools (already driven above) AND continues — feeding the
            // tool_results back — regardless of whether the stop_reason was
            // `tool_use`, `end_turn`, `stop_sequence`, or a truncated
            // `max_tokens` that still carried a complete tool block. Fires only
            // when tools were dispatched; a withheld `max_output_tokens`
            // response carries NO tool_uses and falls through to the recovery/
            // terminal arms below. Subsumes the former
            // `Some("tool_use") if !pumped.tool_uses.is_empty()` arm.
            _ if !pumped.tool_uses.is_empty() => {
                let turn_end = self
                    .take_pending_tool_result_turn_ends(
                        &pumped
                            .tool_uses
                            .iter()
                            .map(|tool_use| tool_use.id.clone())
                            .collect::<Vec<_>>(),
                    )
                    .await;
                if tool_prevent_continuation {
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn("hook_stopped", &cost).await;
                    return Ok(StreamingIterationDisposition::Return(
                        ConversationOutcome::EndTurn {
                            turn_count: loop_state.turn_count,
                            final_message_id: assistant_id,
                        },
                    ));
                }
                if let Some(turn_end) = turn_end {
                    crate::turn_loop::emit_tool_result_ended_turn_telemetry(self, turn_end).await;
                    let batch_messages =
                        crate::turn_loop::run_post_tool_batch_hooks_after_turn_end(
                            self,
                            post_tool_batch_calls,
                        )
                        .await;
                    crate::turn_loop::append_tool_injected_messages(self, batch_messages).await;
                    return self
                        .finish_tool_requested_streaming_end(loop_state, assistant_id)
                        .await;
                }
                let (batch_prevent, batch_messages) =
                    crate::turn_loop::run_post_tool_batch_hooks(self, post_tool_batch_calls).await;
                crate::turn_loop::append_tool_injected_messages(self, batch_messages).await;
                if batch_prevent {
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn("hook_stopped", &cost).await;
                    return Ok(StreamingIterationDisposition::Return(
                        ConversationOutcome::EndTurn {
                            turn_count: loop_state.turn_count,
                            final_message_id: assistant_id,
                        },
                    ));
                }
                crate::turn_loop::emit_tools_refreshed_mid_turn_telemetry(
                    self,
                    pre_batch_mcp_tool_count,
                )
                .await;
                // EndConversation (2.1.206, streaming twin): a 2nd
                // consecutive EndConversation call raised the shared
                // end-request slot during tool dispatch above. Consume it;
                // if raised, surface the end message and terminate instead
                // of continuing. Default-OFF (no slot wired) → strict no-op
                // → byte-identical to before.
                if self
                    .end_conversation_slot
                    .as_ref()
                    .is_some_and(|s| s.swap(false, std::sync::atomic::Ordering::SeqCst))
                {
                    self.output
                        .emit_text(crate::prompt::end_conversation::END_CONVERSATION_ENDED_MESSAGE)
                        .await;
                    return Ok(StreamingIterationDisposition::Return(
                        ConversationOutcome::EndTurn {
                            turn_count: loop_state.turn_count,
                            final_message_id: assistant_id,
                        },
                    ));
                }
                return Ok(StreamingIterationDisposition::Continue);
            }
            Some("end_turn") => {
                // #78 thinking-only nudge (claude-code `bin/claude.exe`
                // offset ~202946760): an `end_turn` response with no visible
                // text gets ONE nudge to produce user-visible output. This
                // fires BEFORE the Stop hooks (binary order: malformed →
                // thinking-only → stop-hooks → budget), so a thinking-only
                // turn re-prompts the model without first running Stop hooks.
                // The `a !== "compact" && !GRe(a)` source guard is satisfied
                // unconditionally here (compact subturns run in a separate
                // code path — `CompactionOrchestrator` — never this loop), and
                // `!isApiErrorMessage` holds because API errors are caught as
                // `Err(..)` upstream of this match. Once nudged, a still-empty
                // continuation falls through to the normal end.
                if !loop_state.thinking_only_nudged
                    && !pumped_has_visible_text(&pumped.assistant_blocks)
                    && !prior_structured_output
                {
                    self.inject_meta_user_message(THINKING_ONLY_NUDGE).await;
                    loop_state.thinking_only_nudged = true;
                    return Ok(StreamingIterationDisposition::Continue);
                }
                return self
                    .finish_natural_streaming_end(loop_state, assistant_id)
                    .await;
            }
            // #77 malformed-tool-use retry (claude-code `bin/claude.exe`
            // offset ~202945837): `stop_reason == "tool_use"` but the
            // assistant produced ZERO tool_use blocks (a malformed /
            // leaked-invoke response). On the FIRST such failure, inject the
            // byte-exact meta retry nudge, reset the max-output-tokens
            // recovery bookkeeping (TS resets `maxOutputTokensRecoveryCount:
            // 0` + `hasAttemptedReactiveCompact: false`), arm the guard, and
            // loop. On the SECOND (`malformed_tool_use_retried` already set),
            // surface the non-meta terminal message and end the turn. The
            // `!isApiErrorMessage` guard holds (API errors are caught
            // upstream as `Err(..)`). Default build keeps the clean-retry
            // feature flag (`PZa()`) OFF, so we do NOT tombstone the leaked
            // assistant blocks and use the non-clean-retry nudge string.
            Some("tool_use") => {
                if loop_state.malformed_tool_use_retried {
                    // Second failure → terminal NON-meta message, complete.
                    // Binary `ql(...)`→`mcc({isApiErrorMessage:!0})`: an
                    // ASSISTANT api-error message (`role:"assistant",
                    // stop_reason:"stop_sequence", stop_details:null`) appended
                    // after the malformed assistant response (two assistants in
                    // a row, matching the binary). Shape mirrors
                    // `surface_model_error`. (Was a USER message.)
                    self.output.emit_text(MALFORMED_TOOL_USE_RETRY_FAILED).await;
                    let failed_msg = ConversationMessage::Assistant {
                        id: MessageId::new(),
                        content: vec![ContentBlock::Text {
                            text: MALFORMED_TOOL_USE_RETRY_FAILED.to_string(),
                        }],
                        stop_reason: Some("stop_sequence".to_string()),
                    };
                    {
                        let mut s = self.session.lock().await;
                        s.history.push(failed_msg.clone());
                    }
                    self.persist_message_to_jsonl(&failed_msg).await;
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn("end_turn", &cost).await;
                    return Ok(StreamingIterationDisposition::Complete(failed_msg.id()));
                }
                self.inject_meta_user_message(MALFORMED_TOOL_USE_RETRY_NUDGE)
                    .await;
                // TS resets the recovery counters on the retry transition so
                // the continued turn starts a fresh max-output-tokens
                // escalation episode.
                loop_state.recovery.reset_max_output_tokens_recovery();
                loop_state.malformed_tool_use_retried = true;
                return Ok(StreamingIterationDisposition::Continue);
            }
            // A1: intercept `max_tokens` BEFORE the generic terminal arm.
            // While recovery is not exhausted, inject the byte-exact meta
            // nudge user message, increment the counter, and Continue
            // (TS `query.ts:1223-1252`). On exhaustion, fall through to the
            // generic terminal below (end with stop_reason `max_tokens`).
            Some("max_tokens")
                if loop_state.recovery.max_output_tokens_recovery_count
                    < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT =>
            {
                // The nudge is a META user message carrying the byte-exact
                // string — CC 2.1.207 builds it via `createUserMessage({…,
                // isMeta:!0})`, so it persists with top-level `isMeta:true`.
                let nudge_msg = ConversationMessage::user_meta(
                    MessageId::new(),
                    MAX_OUTPUT_TOKENS_RECOVERY_NUDGE.to_string(),
                );
                {
                    let mut s = self.session.lock().await;
                    s.history.push(nudge_msg.clone());
                }
                self.persist_message_to_jsonl(&nudge_msg).await;
                loop_state.recovery.max_output_tokens_recovery_count = loop_state
                    .recovery
                    .max_output_tokens_recovery_count
                    .saturating_add(1);
                loop_state.recovery.max_output_tokens_override = None;
                return Ok(StreamingIterationDisposition::Continue);
            }
            // #78 thinking-only nudge for `stop_sequence` (claude-code
            // groups `end_turn` and `stop_sequence` under one guard). A
            // `stop_sequence` response with no visible text gets the same
            // once-per-turn nudge before terminating. Intercepted ahead of
            // the generic terminal arm; once nudged it falls through.
            Some("stop_sequence")
                if !loop_state.thinking_only_nudged
                    && !pumped_has_visible_text(&pumped.assistant_blocks)
                    && !prior_structured_output =>
            {
                self.inject_meta_user_message(THINKING_ONLY_NUDGE).await;
                loop_state.thinking_only_nudged = true;
                return Ok(StreamingIterationDisposition::Continue);
            }
            // Finding #80 (streaming twin): a `refusal` response swaps to the
            // configured `refusalFallbackModel` ONCE per session and retries.
            // Intercepted ahead of the generic terminal arm; when no fallback
            // is configured (or the latch is already set) it falls through to
            // the terminal `Some(other)` arm below, byte-identical to before.
            Some("refusal") if self.maybe_swap_to_refusal_fallback().await => {
                return Ok(StreamingIterationDisposition::Continue);
            }
            Some(other) => {
                // max_tokens (recovery exhausted) / stop_sequence (visible
                // text or already nudged) / pause_turn / refusal (no fallback
                // configured / already latched) — terminate the loop with the
                // value as-is, mirroring claude-code's behavior (claude.ts:2269).
                //
                // First surface the byte-locked user-visible `API Error: …`
                // assistant message claude-code emits for the terminal
                // stop_reasons it reports as errors (`claude.ts:2266`
                // max_tokens [recovery exhausted], `:2279`
                // model_context_window_exceeded). A strict no-op for every
                // other terminal (stop_sequence / pause_turn /
                // refusal-without-fallback), so those end byte-identically to
                // before. Mirrors `surface_prompt_too_long` (persist a new
                // assistant message carrying the error text + the originating
                // stop_reason, then emit it).
                // Build the byte-locked `API Error: …` via the shared
                // [`crate::turn_loop::terminal_api_error_text`] (the batched
                // twin uses the SAME builder, so both paths surface identical
                // text). `None` for stop_sequence / pause_turn / refusal-
                // without-fallback's other terminals → no message, end as-is.
                let api_error: Option<String> = {
                    let model = self.session.lock().await.model.clone();
                    let request_id = self.api.last_request_id();
                    crate::turn_loop::terminal_api_error_text(
                        &model,
                        self.prompt_is_interactive(),
                        other,
                        request_id.as_deref(),
                        pumped.stop_details.as_ref(),
                    )
                };
                let surfaced_id = if let Some(text) = api_error {
                    let err_msg = ConversationMessage::Assistant {
                        id: MessageId::new(),
                        content: vec![ContentBlock::Text { text: text.clone() }],
                        stop_reason: Some(other.to_string()),
                    };
                    self.session.lock().await.history.push(err_msg.clone());
                    self.persist_message_to_jsonl(&err_msg).await;
                    self.output.emit_text(&text).await;
                    Some(err_msg.id())
                } else {
                    None
                };
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn(other, &cost).await;
                return Ok(StreamingIterationDisposition::Complete(
                    surfaced_id.unwrap_or(assistant_id),
                ));
            }
            None => {
                // Stream ended without a stop_reason — treat as
                // end_turn (rare; claude.ts uses the same fallback). The
                // token-budget check applies here too (A3).
                // #78: a missing stop_reason is treated as `end_turn`
                // (claude-code `stop_reason ?? <default>`), so the
                // thinking-only nudge applies here on the same terms and,
                // like the `end_turn` arm, fires BEFORE the Stop hooks.
                if !loop_state.thinking_only_nudged
                    && !pumped_has_visible_text(&pumped.assistant_blocks)
                    && !prior_structured_output
                {
                    self.inject_meta_user_message(THINKING_ONLY_NUDGE).await;
                    loop_state.thinking_only_nudged = true;
                    return Ok(StreamingIterationDisposition::Continue);
                }
                return self
                    .finish_natural_streaming_end(loop_state, assistant_id)
                    .await;
            }
        }
    }

    /// Internal streaming turn driver (no telemetry — wrapped by
    /// `run_turn_streaming`).
    #[allow(clippy::too_many_lines)]
    async fn try_run_turn_streaming(
        &self,
        prompt: &str,
        images: Vec<protocol::ImageSource>,
        user_cancel: Option<CancellationToken>,
        message_id: Option<MessageId>,
        transient_rewake: bool,
    ) -> Result<ConversationOutcome, OrchestratorError> {
        StreamingTurnDriver {
            orch: self,
            prompt,
            images,
            user_cancel,
            message_id,
            transient_rewake,
        }
        .run()
        .await
    }

    /// Drive one user prompt through a REPL turn until `end_turn`,
    /// `max_turns`, or the given `cancel` token fires.
    ///
    /// Unlike [`Self::run_turn`] this method:
    /// - returns [`TurnOutcome`] so the REPL can distinguish natural end /
    ///   max-turns / cancellation.
    /// - takes a [`CancellationToken`] that fires on Ctrl+C (SIGINT); the
    ///   orchestrator checks it before each API round-trip.
    /// - does NOT close the session — the session accumulates messages across
    ///   REPL turns; the REPL persists via the JSONL writer when it exits.
    ///
    /// Telemetry: emits `CONVERSATION_STARTED` at entry; delegates to the
    /// same turn-loop body as `run_turn` (via `try_run_turn_cancelable`).
    pub async fn run_turn_with_cancel(
        &self,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, OrchestratorError> {
        let _turn_guard = self.turn_gate.lock().await;
        tracing::info!(
            event = orch_events::CONVERSATION_STARTED,
            prompt_len = prompt.len()
        );
        let result =
            telemetry::otel::with_turn_span("lingxi.orchestrator.turn.cancelable", async {
                platform_api::session_flags::scope_non_interactive_session(
                    !self.prompt_is_interactive(),
                    self.try_run_turn_cancelable(prompt, cancel),
                )
                .await
            })
            .await;
        self.emit_terminal_rate_limit_if_changed(&result).await;
        result.map_err(|e| self.enrich_api_error(e))
    }

    /// Internal implementation of the REPL turn loop with cancellation.
    async fn try_run_turn_cancelable(
        &self,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, OrchestratorError> {
        // 0. Build the system prompt (same as non-cancelable path).
        // claude-code `nre` precedence: `--system-prompt` (override) wins; else the
        // `--agent` main-thread agent's prompt; else the default (`build_system_prompt`).
        let system_prompt: Option<String> = Some(self.effective_system_prompt().await);

        // A side query left unfinished when the previous user turn ended was
        // keyed to that previous prompt. Never surface it against new intent.
        self.discard_stale_prefetches().await;

        // 1. Append the user prompt to session history.
        let user_msg = ConversationMessage::user(MessageId::new(), prompt.to_string());
        {
            let mut s = self.session.lock().await;
            s.history.push(user_msg.clone());
        }
        self.persist_message_to_jsonl(&user_msg).await;

        // hooks B4: UserPromptSubmit (cancelable REPL twin). A Block aborts the
        // turn before any API call. No-op when unregistered.
        if self.fire_user_prompt_submit(prompt, user_msg.id()).await {
            return Ok(TurnOutcome::EndTurn);
        }

        // 2. Turn-by-turn loop — check cancel before each API call.
        //
        // #2 (main-loop parity): the cancelable driver is recovery- AND
        // budget-aware, identical to the non-cancelable [`Self::run_turn`]
        // batched loop, except each API round-trip is raced against `cancel`.
        // The legacy no-recovery shim ([`execute_one_turn`]) is no longer used
        // here: a `max_tokens` stop_reason now drives the A1 multi-turn recovery
        // nudge (and exhaustion-ends) exactly as the main batched path does,
        // rather than legacy-continuing without the nudge.
        let mut recovery = RecoveryState::default();
        // hooks B4: Stop-hook re-entry guard (cancelable twin).
        let mut stop_hook_active = false;
        // #2 consecutive Stop-hook block counter (binary `stopHookBlockingCount`):
        // bumped per block; ends the turn via the cap once it would exceed
        // LINGXI_STOP_HOOK_BLOCK_CAP (default 8). Fresh per turn-driver run.
        let mut stop_hook_blocking_count: u32 = 0;
        // A3: token-budget continuation bookkeeping (no-op unless gated + set).
        let mut budget = self.new_budget_tracker();
        let mut global_turn_tokens: u64 = 0;
        // Turn-start output baseline (claude-code `xtr` via `UAc(e)`): snapshot
        // the cumulative pool as this turn begins, so a workflow launched this
        // turn reads `budget.spent()` = output spent THIS turn.
        self.compaction_runtime.turn_start_output_baseline.store(
            self.compaction_runtime
                .output_token_pool
                .load(std::sync::atomic::Ordering::Relaxed),
            std::sync::atomic::Ordering::Relaxed,
        );
        let mut turn_count: u32 = 0;
        loop {
            if cancel.is_cancelled() {
                // claude-code `query.ts:1046-1050`: inject the non-tool-use
                // interrupt message on a loop-top pre-cancel (ESC fired before
                // we even called the model this iteration). NOW-ABORT
                // disambiguation: skip the message when the cancel was a
                // `Now`-command (the urgent command runs next); default behavior
                // (no reason flag wired) is byte-identical to before.
                if self.cancel_reason_now()
                    != crate::prompt::mid_turn_input::CancelReason::QueueNowCommand
                {
                    self.inject_user_message(INTERRUPT_MESSAGE).await;
                }
                return Ok(TurnOutcome::Cancelled);
            }
            if self.config.max_turns != 0 && turn_count >= self.config.max_turns {
                return Ok(TurnOutcome::MaxTurns);
            }
            if self.over_budget().await {
                return Err(OrchestratorError::MaxBudgetReached {
                    budget_nano_usd: self.config.max_budget_nano_usd.unwrap_or(0),
                });
            }
            turn_count = turn_count.saturating_add(1);

            // Race the recovery-aware API turn-step against the cancellation
            // token. The `_tracked` variant returns this step's output-token
            // count for the A3 budget accumulation, mirroring `run_turn`.
            let (step, output_tokens) = tokio::select! {
                r = execute_one_turn_with_recovery_tracked(
                    self,
                    system_prompt.as_deref(),
                    Some(&mut recovery),
                ) => r?,
                () = cancel.cancelled() => {
                    // claude-code `query.ts:1046-1050`: inject the non-tool-use
                    // interrupt message when the cancel fires mid-API-call
                    // (model was in-flight, no tool_use blocks produced yet).
                    // NOW-ABORT disambiguation: skip the message for a
                    // `Now`-command abort (default behavior unchanged).
                    if self.cancel_reason_now()
                        != crate::prompt::mid_turn_input::CancelReason::QueueNowCommand
                    {
                        self.inject_user_message(INTERRUPT_MESSAGE).await;
                    }
                    return Ok(TurnOutcome::Cancelled);
                }
            };
            // A3: accumulate the running per-turn output tokens (TS
            // `getTurnOutputTokens()`). No-op for accounting when budget is off.
            global_turn_tokens = global_turn_tokens.saturating_add(output_tokens);
            match step {
                TurnStepOutcome::Continue => continue,
                TurnStepOutcome::Ended {
                    stop_reason,
                    final_message_id: id,
                    allow_budget_continuation,
                    tool_requested_end,
                } => {
                    // hooks B4: Stop hooks (cancelable twin). `TurnOutcome` does
                    // not distinguish StopHookPrevented from EndTurn, so both the
                    // Terminate and FallThrough dispositions end the REPL turn as
                    // EndTurn; only `LoopAgain` (a Stop hook asking to keep
                    // working) loops. `handle_stop_at_end` already emits the
                    // end-turn on Terminate, so we don't re-emit there.
                    if tool_requested_end {
                        self.fire_tool_result_end_stop_hooks(&stop_reason, stop_hook_active)
                            .await;
                    } else {
                        match self
                            .handle_stop_at_end(
                                &stop_reason,
                                &mut stop_hook_active,
                                &mut stop_hook_blocking_count,
                                turn_count,
                                id,
                            )
                            .await
                        {
                            StopHookFlow::Terminate(_) => return Ok(TurnOutcome::EndTurn),
                            // Binary blocking-branch max-turns end — mirror this fn's
                            // own top-of-loop guard, which returns `TurnOutcome::MaxTurns`.
                            StopHookFlow::TerminateMaxTurns => return Ok(TurnOutcome::MaxTurns),
                            StopHookFlow::LoopAgain => {
                                // RECOV.4: a Stop hook forced the loop to continue —
                                // reset the max_output_tokens recovery bookkeeping so
                                // the continued turn starts a fresh escalation episode
                                // (TS `query.ts:1291`), matching `run_turn`.
                                recovery.reset_max_output_tokens_recovery();
                                continue;
                            }
                            StopHookFlow::FallThrough => {}
                        }
                    }
                    // A3: at a natural end-of-turn, consult the token budget. If
                    // it says continue, inject the meta nudge, reset the A1
                    // recovery count, and loop again instead of ending. When the
                    // budget is off this is a no-op (parity default).
                    //
                    // Gate on `end_turn` (matching `run_turn`): unlike the
                    // streaming twin, which is structurally in the hardcoded
                    // `"end_turn"` branch, this driver carries the live
                    // `stop_reason`, so a TERMINAL end (blocking_limit /
                    // prompt_too_long) must NOT trigger budget continuation.
                    if allow_budget_continuation
                        && stop_reason == "end_turn"
                        && self
                            .maybe_continue_for_budget(
                                budget.as_mut(),
                                &mut recovery,
                                global_turn_tokens,
                            )
                            .await
                    {
                        continue;
                    }
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn(&stop_reason, &cost).await;
                    return Ok(TurnOutcome::EndTurn);
                }
            }
        }
    }

    /// Streaming twin of [`Self::run_turn_with_cancel`] (M6-03).
    ///
    /// DEFERRED-3: the `cancel` token is a GRANULAR user-interrupt (ESC / new
    /// message), threaded INTO [`Self::try_run_turn_streaming`] rather than raced
    /// against it. When it fires mid-tools the `StreamingToolExecutor` rejects
    /// in-flight/queued Cancel-behavior tools with the bare REJECT_MESSAGE,
    /// PERSISTS those `tool_result`s, and the turn ends gracefully:
    /// - natural completion, token never fired → `TurnOutcome::EndTurn`.
    /// - token fired (the turn finished gracefully with the interrupted results
    ///   recorded in history) → `TurnOutcome::Cancelled`.
    /// - a pre-cancelled token → `TurnOutcome::Cancelled` immediately (no API call).
    /// - `OrchestratorError::MaxTurnsReached` → `TurnOutcome::MaxTurns`.
    /// - any other API/streaming error → propagated as `Err`.
    ///
    /// This is the entry point the M6 TUI calls. M5-13 stdio REPL keeps
    /// using `run_turn_with_cancel` (batched) until M6 makes streaming
    /// the default.
    pub async fn run_turn_streaming_with_cancel(
        &self,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, OrchestratorError> {
        self.run_turn_streaming_with_cancel_images_and_message_id(prompt, &[], cancel, None)
            .await
    }

    /// As [`Self::run_turn_streaming_with_cancel`], but carrying pasted image
    /// file paths that are loaded + base64-encoded into `ContentBlock::Image`
    /// blocks on the outgoing user message (TUI paste→image). A failed image
    /// read aborts the turn with `Err` before any API call.
    pub async fn run_turn_streaming_with_cancel_images(
        &self,
        prompt: &str,
        image_paths: &[std::path::PathBuf],
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, OrchestratorError> {
        self.run_turn_streaming_with_cancel_images_and_message_id(prompt, image_paths, cancel, None)
            .await
    }

    /// As [`Self::run_turn_streaming_with_cancel_images`], but allows the
    /// caller to pin the persisted user-message UUID.
    pub async fn run_turn_streaming_with_cancel_images_and_message_id(
        &self,
        prompt: &str,
        image_paths: &[std::path::PathBuf],
        cancel: CancellationToken,
        message_id: Option<MessageId>,
    ) -> Result<TurnOutcome, OrchestratorError> {
        // Decode the pasted PATHS into canonical sources first, then hand off to
        // the already-decoded entry below — so the path-based and bridge (inline
        // base64) flows share ONE cancel race + ONE turn core. A failed image read
        // aborts the turn with `Err` before any API call (unchanged).
        let images = Self::load_images(image_paths)?;
        self.run_turn_streaming_with_cancel_image_sources_and_message_id(
            prompt, images, cancel, message_id,
        )
        .await
    }

    /// As [`Self::run_turn_streaming_with_cancel_images`], but taking
    /// ALREADY-DECODED [`protocol::ImageSource`]s instead of file paths.
    ///
    /// This is the entry the desktop bridge adapter drives: pasted/attached images
    /// arrive over the wire as inline base64 (`ImageRefDto`) and are converted
    /// straight to [`protocol::ImageSource::Base64`] with NO temp-file round-trip.
    /// The path-based entry above decodes its paths via [`Self::load_images`] then
    /// delegates here, so both paths share this cancel race and the single
    /// [`Self::try_run_turn_streaming`] core — which appends the images to the
    /// outgoing user message via [`ConversationMessage::user_with_images`]. With an
    /// empty `images` vector this is byte-identical to the text-only streaming path.
    pub async fn run_turn_streaming_with_cancel_image_sources(
        &self,
        prompt: &str,
        images: Vec<protocol::ImageSource>,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, OrchestratorError> {
        self.run_turn_streaming_with_cancel_image_sources_and_message_id(
            prompt, images, cancel, None,
        )
        .await
    }

    /// As [`Self::run_turn_streaming_with_cancel_image_sources`], but allows the
    /// caller to pin the persisted user-message UUID.
    pub async fn run_turn_streaming_with_cancel_image_sources_and_message_id(
        &self,
        prompt: &str,
        images: Vec<protocol::ImageSource>,
        cancel: CancellationToken,
        message_id: Option<MessageId>,
    ) -> Result<TurnOutcome, OrchestratorError> {
        let _turn_guard = self.turn_gate.lock().await;
        tracing::info!(
            event = orch_events::TURN_STREAMING_STARTED,
            prompt_len = prompt.len()
        );
        if cancel.is_cancelled() {
            self.abort_startup_responses_websocket_prewarm();
            if let Err(err) = self.api.close_responses_websocket_session().await {
                tracing::warn!(
                    error = %err,
                    "failed to close responses websocket session after pre-cancelled streaming turn"
                );
            }
            return Ok(TurnOutcome::Cancelled);
        }
        // DEFERRED-3: GRANULAR user-ESC interrupt — do NOT race-drop the turn.
        // Previously this `select!`'d `try_run_turn_streaming` against
        // `cancel.cancelled()` and on cancel DROPPED the whole turn future, so
        // in-flight tools vanished and no result reached the model. Now the token
        // is threaded INTO the turn core: the `StreamingToolExecutor` substitutes
        // the bare REJECT_MESSAGE for in-flight/queued Cancel-behavior tools,
        // PERSISTS those `tool_result`s (model-visible), and the streaming loop
        // FINISHES gracefully — mirroring claude-code's `StreamingToolExecutor`
        // user_interrupted path where the interrupted results become part of the
        // transcript. We then map the outcome to `Cancelled` for the TUI when the
        // token fired (so it still renders "interrupted"), else `EndTurn`. A turn
        // running only Block-behavior tools (or no tools) runs to its natural end
        // and is still reported `Cancelled` here — faithful: claude-code only
        // aborts Cancel-behavior tools; Block tools / the stream finish.
        let r = telemetry::otel::with_turn_span(
            "lingxi.orchestrator.turn.streaming.cancelable",
            async {
                platform_api::session_flags::scope_non_interactive_session(
                    !self.prompt_is_interactive(),
                    Box::pin(self.try_run_turn_streaming(
                        prompt,
                        images,
                        Some(cancel.clone()),
                        message_id,
                        false,
                    )),
                )
                .await
            },
        )
        .await;
        match r {
            Ok(
                ConversationOutcome::EndTurn { turn_count, .. }
                | ConversationOutcome::StopHookPrevented { turn_count, .. },
            ) => {
                tracing::info!(event = orch_events::TURN_STREAMING_COMPLETED, turn_count);
                if cancel.is_cancelled() {
                    self.abort_startup_responses_websocket_prewarm();
                    if let Err(err) = self.api.close_responses_websocket_session().await {
                        tracing::warn!(
                            error = %err,
                            "failed to close responses websocket session after cancelled streaming turn"
                        );
                    }
                    Ok(TurnOutcome::Cancelled)
                } else {
                    Ok(TurnOutcome::EndTurn)
                }
            }
            Err(OrchestratorError::MaxTurnsReached { .. }) => Ok(TurnOutcome::MaxTurns),
            Err(OrchestratorError::VisionDelegationCancelled) if cancel.is_cancelled() => {
                Ok(TurnOutcome::Cancelled)
            }
            Err(e) => {
                // B6-T1: status-change emit parity — fire the emit-on-change
                // helpers for a terminal rate-limited error (the drive fn
                // already promoted the staged 429), BEFORE enrichment. Same
                // discriminant + B1 divergence as
                // `emit_terminal_rate_limit_if_changed`.
                if matches!(
                    e,
                    OrchestratorError::ApiCall(LlmError::RateLimited { .. })
                        | OrchestratorError::Streaming(LlmError::RateLimited { .. })
                ) {
                    self.emit_rate_limit_if_changed().await;
                    self.emit_raw_utilization_if_changed().await;
                }
                Err(self.enrich_api_error(e))
            }
        }
    }

    /// Load + base64-encode each pasted image path into an [`ImageSource`].
    fn load_images(
        image_paths: &[std::path::PathBuf],
    ) -> Result<Vec<protocol::ImageSource>, OrchestratorError> {
        image_paths
            .iter()
            .map(|p| crate::image_input::load_image_source(p))
            .collect()
    }
}

// Streaming recovery conversion and visibility helpers.
/// Convert an `LlmResponse` (from the non-streaming fallback call) into the
/// same [`crate::streaming_loop::PumpedTurn`] shape the streaming loop uses,
/// so the remainder of the streaming turn handler works unchanged.
///
/// Mirrors the batched path's `translate_response_blocks` call: content blocks
/// are translated to `protocol::ContentBlock`; `ToolCall` blocks additionally
/// populate the `tool_uses` vec so the concurrent dispatch runs exactly as in a
/// real stream.
/// Whether the turn's assistant blocks contain any user-visible text, mirroring
/// claude-code's thinking-only guard predicate (`bin/claude.exe` offset
/// ~202946760):
/// `ie.some(msg => msg.content.some(b => b.type === "text" && b.text.trim().length > 0))`.
///
/// A `false` return = a thinking-only (or otherwise text-empty) response. Only
/// [`protocol::ContentBlock::Text`] blocks with a non-whitespace body count;
/// `Thinking`, `ToolUse`, etc. are not "visible output" for this gate. (Tool
/// uses live in `PumpedTurn::tool_uses`, not `assistant_blocks`, and this gate
/// only fires on `end_turn`/`stop_sequence` where no `tool_use` is present.)
pub(super) fn pumped_has_visible_text(blocks: &[protocol::ContentBlock]) -> bool {
    use protocol::ContentBlock;
    blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Text { text } if !text.trim().is_empty()))
}

pub(super) fn llm_response_to_pumped_turn(resp: &LlmResponse) -> crate::streaming_loop::PumpedTurn {
    use crate::streaming_loop::{ObservedToolUse, PumpedTurn};
    use crate::turn_loop::translate_response_blocks;
    use protocol::ContentBlock;

    let output_tokens = resp.usage.billable_tokens.output;
    let stop_reason = resp.stop_reason.clone();
    // BILLING: carry the full usage so the streaming turn loop records it
    // in CostTracker. The non-streaming fallback issues a real messages_create
    // call (seeded) whose response includes the authoritative usage.
    let usage = Some(resp.usage.clone());

    // Translate LlmResponse content → protocol::ContentBlock (same as batched path).
    // Then split into (assistant_blocks, tool_uses): text/thinking go into
    // assistant_blocks; ToolUse blocks go into tool_uses for the concurrent dispatch.
    // The streaming loop step 4 re-assembles them into the assistant message by
    // appending ToolUse blocks from tool_uses — so we must NOT include ToolUse in
    // assistant_blocks (or they appear twice in the assistant message).
    let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
    let mut tool_uses: Vec<ObservedToolUse> = Vec::new();

    for blk in translate_response_blocks(&resp.content) {
        match blk {
            ContentBlock::ToolUse {
                id,
                ref name,
                ref input,
                ref provider_id,
            } => {
                tool_uses.push(ObservedToolUse {
                    id,
                    name: name.clone(),
                    input: input.clone(),
                    provider_id: provider_id.clone(),
                });
                // Not pushed to assistant_blocks — step 4 appends ToolUse from tool_uses.
            }
            other => {
                assistant_blocks.push(other);
            }
        }
    }

    PumpedTurn {
        assistant_blocks,
        tool_uses,
        stop_reason,
        output_tokens,
        usage,
        // Non-streaming fallback: carry the response's refusal stop_details so
        // the terminal refusal arm gets the cyber/bio variant.
        stop_details: resp.stop_details.clone(),
    }
}

/// Mirror TS `isEnvTruthy` (`utils/envUtils.ts:32`): a value is truthy ONLY
/// when, lowercased and trimmed, it is one of the whitelist members
/// `"1"`, `"true"`, `"yes"`, `"on"`. Absent, empty, and every other value
/// (including `"no"`, `"off"`, `"2"`, `"enabled"`, …) are falsy.
///
/// Locked against the TS helper used at `claude.ts:2470`:
/// `isEnvTruthy(process.env.LINGXI_DISABLE_NONSTREAMING_FALLBACK)`.
pub(super) fn is_env_truthy(val: Option<&str>) -> bool {
    match val {
        None => false,
        Some(v) => matches!(v.to_lowercase().trim(), "1" | "true" | "yes" | "on"),
    }
}

pub(super) fn parse_generated_session_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let candidate = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|body| body.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let object = serde_json::from_str::<serde_json::Value>(candidate)
        .ok()
        .or_else(|| {
            let start = candidate.find('{')?;
            let end = candidate.rfind('}')?;
            serde_json::from_str(&candidate[start..=end]).ok()
        })?;
    object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

// NOTE: `AnthropicProviderAdapter` and `AnthropicProviderStreamingAdapter`
// were removed in Task 5 — they drove `api_client::AnthropicProvider` directly.
// The live path is now `ProviderApiAdapter` (provider_adapter.rs), retargeted
// in Task 6 to drive `llm_client::DefaultLlmClient`. (3b deletes api-client.)
