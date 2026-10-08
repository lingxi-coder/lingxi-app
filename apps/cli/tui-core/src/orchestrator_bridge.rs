//! Orchestrator → TUI event bridge. (M6-03)
//!
//! The `BridgeOutputStream` is an [`lingxi_core::host::OutputStream`] impl that
//! forwards every orchestrator callback as a [`TurnEvent`] on an mpsc
//! channel. The TUI render loop drains the receiver and feeds events into
//! `crate::streaming::apply_event`, which mutates `AppState` and pokes a
//! `tokio::sync::Notify`.
//!
//! The lifecycle:
//! 1. TUI app creates an `mpsc::unbounded_channel::<TurnEvent>()`.
//! 2. TUI wraps the sender in `BridgeOutputStream` and passes it to the
//!    `ConversationOrchestrator` as its `output: Arc<dyn OutputStream>`.
//! 3. TUI keeps the receiver and the sender (so the channel stays open
//!    across multiple turns).
//! 4. For each user submit: TUI emits `TurnEvent::TurnStarted` directly,
//!    spawns `run_turn_streaming_with_cancel`, then awaits delta events.
//! 5. On `emit_end_turn` the bridge fires `TurnEvent::TurnEnded(_)`.

use async_trait::async_trait;
use lingxi_core::host::{
    ContextPressureBanner, CostSnapshot, OutputStream, RefusalContinuationJoin,
    RefusalContinuationPhase, ServerFallbackTombstoneMessage, TurnOutcome,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

/// One live heartbeat value retained by the bridge. Heartbeats are transient
/// render state and therefore may be replaced by a newer value for the same
/// tool while the UI is backpressured.
#[derive(Debug, Clone)]
pub struct ToolHeartbeatUpdate {
    /// Stable tool-use identifier.
    pub id: lingxi_core::types::ToolUseId,
    /// Tool name used by the activity renderer.
    pub tool: String,
    /// Latest elapsed wall time in milliseconds.
    pub elapsed_ms: u64,
}

#[derive(Debug, Default)]
struct CoalescedToolHeartbeatsState {
    latest: Vec<ToolHeartbeatUpdate>,
    signal_queued: bool,
}

/// Latest-value mailbox used to ensure slow TUI consumers can have at most one
/// heartbeat wake-up queued. Ordinary transcript-bearing events keep their
/// FIFO semantics; only replaceable heartbeat state is coalesced.
#[derive(Debug, Default)]
pub struct CoalescedToolHeartbeats {
    state: Mutex<CoalescedToolHeartbeatsState>,
}

impl CoalescedToolHeartbeats {
    fn publish(&self, update: ToolHeartbeatUpdate) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(current) = state
            .latest
            .iter_mut()
            .find(|current| current.id == update.id)
        {
            *current = update;
        } else {
            state.latest.push(update);
        }
        if state.signal_queued {
            false
        } else {
            state.signal_queued = true;
            true
        }
    }

    /// Drain the newest heartbeat for each running tool and permit the bridge
    /// to queue the next wake-up.
    #[must_use]
    pub fn drain(&self) -> Vec<ToolHeartbeatUpdate> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.signal_queued = false;
        std::mem::take(&mut state.latest)
    }

    fn reset_after_send_failure(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.signal_queued = false;
        state.latest.clear();
    }
}

/// A slash-command row used to atomically refresh the interactive completion
/// catalog after plugins or skills are reconciled. This transport type lives
/// in `tui-core` so app-level async effects can update the owning widget
/// without process-global mutable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCatalogEntry {
    /// Command name including the leading slash.
    pub name: String,
    /// Full searchable description.
    pub description: String,
    /// Optional compact menu description.
    pub menu_description: Option<String>,
    /// Alternate names including the leading slash.
    pub aliases: Vec<String>,
    /// Hidden entries still dispatch and may surface on an exact name match.
    pub hidden: bool,
    /// A described static TUI command replaces its original menu row.
    pub replaces_builtin: bool,
    /// Argument hint returned by `command.describe`.
    pub argument_hint: Option<String>,
    /// Declared positional argument names for progressive hints after each argument.
    pub argument_names: Vec<String>,
}

/// One live agent rendered next to the TUI composer.
///
/// Foreground Agent/Task tool calls are synthesized by the widget; background
/// agents arrive as full snapshots from the CLI's task-registry poller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningAgentStatus {
    /// The teammate is waiting for its leader's plan decision.
    pub awaiting_plan_approval: bool,
    /// Stable agent/task id (or the foreground tool-use id before launch).
    pub id: String,
    /// Claude task-registry type used by the compact footer summarizer.
    pub task_type: String,
    /// Display agent type, e.g. `Explore` or `general-purpose`.
    pub agent_type: String,
    /// Short task description supplied to the Agent tool.
    pub description: String,
    /// Lifecycle wire state. The input-adjacent surface keeps pending/running.
    pub status: String,
    /// Custom `subagentStatusLine` content. `None` uses the built-in summary;
    /// an empty string intentionally hides this task's row.
    pub custom_content: Option<String>,
}

/// Events flowing from the orchestrator into the TUI render loop.
///
/// Created in M6-03 as a TUI-local enum (not exposed on any orchestrator
/// trait). The bridge translates `OutputStream` callbacks into this enum.
/// `PermissionRequest` is wired in M6-05; `ThinkingDelta` in M5 (2.1.198
/// live-streaming parity).
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// Bind the just-completed assistant response to its transcript identity.
    MessageIdentity(lingxi_core::types::MessageId),
    /// Remove only the assistant response with this transcript identity.
    MessageRetracted(lingxi_core::types::MessageId),
    /// Model selection confirmed by the live orchestrator.
    ModelChanged {
        /// Provider-local model identifier.
        model: String,
        /// Provider profile selected by the user.
        profile: Option<String>,
    },
    /// Authoritative permission mode after a live change attempt.
    PermissionModeChanged(String),
    /// The host completed the owned backend reset; only now may the UI clear.
    SessionCleared {
        /// New backend session identity and history persistence scope.
        session_id: String,
        /// Config home for rebuilding the session's prompt-history store.
        home: std::path::PathBuf,
        /// Current tool cwd, independent of process cwd.
        cwd: std::path::PathBuf,
    },
    /// Backend reset failed; preserve the previous conversation on screen.
    SessionClearFailed(String),
    /// Streaming text chunk from the assistant.
    TextDelta(String),
    /// A completed assistant thinking block (M5 live streaming). The
    /// orchestrator's `emit_thinking` fires once per COMPLETED block (not
    /// per-delta — see `lingxi_core::host::OutputStream::emit_thinking`), so one event
    /// carries the whole reasoning text.
    ThinkingDelta(String),
    /// A tool invocation is about to dispatch.
    ToolUseStart {
        /// Stable id (the model-supplied `tool_use_id`) — correlates with
        /// the matching `ToolUseResult`.
        id: lingxi_core::types::ToolUseId,
        /// Name of the tool being invoked.
        tool: String,
        /// JSON input passed to the tool.
        input: serde_json::Value,
    },
    /// Periodic liveness update for a tool that has started but not completed.
    /// This is live state only: consumers must not append it to transcript
    /// history.
    ToolHeartbeat {
        /// Stable id of the running tool call.
        id: lingxi_core::types::ToolUseId,
        /// Tool name, retained so stateless clients can render the update.
        tool: String,
        /// Wall-clock age of the tool invocation in milliseconds.
        elapsed_ms: u64,
    },
    /// Coalesced heartbeat wake-up produced by [`BridgeOutputStream`]. The
    /// consumer drains the mailbox and applies each latest update exactly as a
    /// [`Self::ToolHeartbeat`] without adding transcript rows.
    ToolHeartbeatBatch {
        /// Shared latest-value mailbox.
        heartbeats: Arc<CoalescedToolHeartbeats>,
    },
    /// A blocking or asynchronous hook began running. This is transient UI
    /// state and must never be appended to transcript history.
    HookProgressStarted {
        /// Unique identity for this hook run, not merely the hook definition.
        id: String,
        /// Configured status text, or a bounded generic fallback.
        text: String,
    },
    /// The matching hook run reached a terminal outcome.
    HookProgressFinished {
        /// Run identity supplied by [`Self::HookProgressStarted`].
        id: String,
    },
    /// A tool result has returned.
    ToolUseResult {
        /// Correlator with the paired `ToolUseStart`.
        id: lingxi_core::types::ToolUseId,
        /// Tool name (used to gate Bash → ANSI parser at render time).
        tool: String,
        /// JSON result payload.
        result: serde_json::Value,
    },
    /// Permission gate fired. Wired in M6-05; the variant is reserved
    /// here so the enum stays append-only.
    PermissionRequest {
        /// Name of the tool the permission gate is checking.
        tool: String,
        /// JSON input the permission gate is being asked to approve.
        input: serde_json::Value,
    },
    /// Fired SYNCHRONOUSLY before the orchestrator future is awaited so
    /// the UI shows the spinner immediately on Enter.
    TurnStarted,
    /// A host-started turn (for example, a teammate message waking the leader)
    /// carries its cancellation token because no composer submit created one.
    TurnStartedWithCancel(lingxi_core::host::CancellationToken),
    /// Fired when the orchestrator returns. Carries the [`TurnOutcome`].
    TurnEnded(TurnOutcome),
    /// Updated session-cumulative cost, formatted as `$0.0000` (4-decimal
    /// claude-code parity). M6-06: fired by [`BridgeOutputStream::emit_end_turn`]
    /// using the `CostSnapshot` the orchestrator now populates. The TUI
    /// `apply_event` writes the value into `state.status.cost`, refreshing
    /// the status-line render.
    CostUpdated(String),
    /// Full cumulative and most-recent request usage used by the custom status
    /// line payload. Kept separate from the formatted footer string so legacy
    /// consumers remain source-compatible.
    CostSnapshotUpdated(CostSnapshot),
    /// The live context-pressure banner (or `None` to clear it). `apply_event`
    /// stores it on `state.context_pressure`; the prompt chrome renders it as a
    /// `<TokenWarning>`-equivalent line. Emitted by the orchestrator before
    /// every API call with the current `compaction::token_warning_banner`.
    ContextPressure {
        /// `Some` shows the banner; `None` clears a previously-shown one.
        banner: Option<ContextPressureBanner>,
        /// Context usage as a 0-1 fraction of the model's effective context
        /// window (fed to the custom statusline's `context_window.used_percentage`).
        used_fraction: f32,
        /// Raw token estimate behind the fraction (the auto-compact gate's
        /// input estimate) — the statusline payload's
        /// `context_window.total_input_tokens` and the `exceeds_200k_tokens`
        /// derivation input.
        used_tokens: u64,
        /// The model's effective context window in tokens
        /// (`context_window.context_window_size`).
        context_window_tokens: u64,
    },
    /// An allowlisted terminal escape sequence a hook returned (#6 main-loop
    /// parity). `apply_event` stages it on `state.pending_terminal_sequence`;
    /// the async pump writes the bytes directly to the TUI's stdout (the host
    /// that actually owns the controlling terminal — claude-code `BEo`). Already
    /// validated + BEL-normalized by the orchestrator.
    TerminalSequence {
        /// The validated OSC/BEL sequence to write to the terminal.
        seq: String,
    },
    /// A successful `force_compact` finished. The TUI appends a
    /// `CompactBoundary` variant, rendered as a compact hint in normal mode
    /// and the complete summary when Ctrl-O enables verbose transcript mode.
    /// (M6-08 emitted a `[Compacted N → M messages]` placeholder.)
    CompactionCompleted {
        /// Message count BEFORE compaction.
        messages_before: u32,
        /// Message count AFTER compaction.
        messages_after: u32,
        /// UX estimate of bytes freed.
        bytes_saved: u64,
        /// Full transcript-only compact summary revealed by Ctrl-O.
        summary: String,
    },
    /// Unified rate-limit header snapshot (llm-runtime future-work batch 3,
    /// Task 9). Mirrors `lingxi_core::host::OutputEvent::RateLimit`'s nine fields —
    /// see that variant's per-field docs for the
    /// `anthropic-ratelimit-unified-*` header each value comes from. The
    /// orchestrator emits on-change only; `apply_event` additionally dedupes
    /// on the COMPOSED text so identical consecutive notices never stack.
    RateLimit {
        /// `anthropic-ratelimit-unified-status`.
        status: Option<String>,
        /// `anthropic-ratelimit-unified-representative-claim`.
        rate_limit_type: Option<String>,
        /// Representative claim's 0-1 utilization fraction.
        utilization: Option<f64>,
        /// `anthropic-ratelimit-unified-reset` (Unix-epoch seconds).
        resets_at: Option<u64>,
        /// Per-claim reset (Unix-epoch seconds).
        claim_resets_at: Option<u64>,
        /// `anthropic-ratelimit-unified-overage-status`.
        overage_status: Option<String>,
        /// `anthropic-ratelimit-unified-overage-reset` (Unix-epoch seconds).
        overage_resets_at: Option<u64>,
        /// `anthropic-ratelimit-unified-overage-disabled-reason`.
        overage_disabled_reason: Option<String>,
        /// `anthropic-ratelimit-unified-fallback` == `available`.
        fallback_available: Option<bool>,
        /// `anthropic-ratelimit-unified-upgrade-paths` (2.1.206), parsed to
        /// a list; `None` when the header is absent or empty.
        upgrade_paths: Option<Vec<String>>,
        /// 2.1.206 `credits_required` derivation — see
        /// `lingxi_core::host::OutputEvent::RateLimit::credits_required`.
        credits_required: bool,
    },
    /// Raw per-window utilization snapshot (llm-runtime future-work batch 5,
    /// Task 4). Mirrors `lingxi_core::host::OutputEvent::RawUtilization`'s four fields —
    /// tracked on every API response (unlike the warning-gated
    /// [`Self::RateLimit`]) and stored on `AppState.raw_utilization` for the
    /// statusline command input's `rate_limits` field (`StatusLine.tsx:50-65`).
    /// Windows are atomic: a window's two fields are both `Some` or both `None`.
    RawUtilization {
        /// `anthropic-ratelimit-unified-5h-utilization` (0-1 fraction).
        five_hour_utilization: Option<f64>,
        /// `anthropic-ratelimit-unified-5h-reset` (Unix-epoch seconds).
        five_hour_resets_at: Option<u64>,
        /// `anthropic-ratelimit-unified-7d-utilization` (0-1 fraction).
        seven_day_utilization: Option<f64>,
        /// `anthropic-ratelimit-unified-7d-reset` (Unix-epoch seconds).
        seven_day_resets_at: Option<u64>,
    },
    /// A one-off system notice for the transcript, fired by an app-level
    /// async effect that isn't itself a turn (e.g. the `/web` picker's
    /// secret/settings save or test-search result). NOT emitted by
    /// [`BridgeOutputStream`] — the embedding CLI sends it directly on the
    /// same `TurnEvent` channel so the result lands in the transcript on the
    /// next render tick.
    SystemNotice {
        /// The notice text.
        body: String,
        /// `true` → render as an error (red); `false` → dim informational.
        is_error: bool,
    },
    /// A Mod log line displayed independently of assistant text.
    UiLog { plugin: String, text: String },
    /// A transient Mod notification shown near the composer.
    UiToast {
        plugin: String,
        text: String,
        timeout_ms: u64,
    },
    /// Set or clear a plugin's pinned line below the composer.
    UiStatus {
        plugin: String,
        text: Option<String>,
    },
    /// The persisted top-level JSONL UUID for a live user row token.
    TranscriptRowIdentity { row_token: String, uuid: String },
    /// Persisted top-level JSONL UUIDs for text rows in a live assistant turn.
    AssistantTranscriptRowUuids {
        message_id: lingxi_core::types::MessageId,
        uuids: Vec<Option<String>>,
    },
    /// Per-query fallback model; does not alter the selected model snapshot.
    ServerFallbackQueryModelChange { to_model: String },
    /// Start of one provider content block before its transcript UUID exists.
    AssistantBlockStart { block_key: u64 },
    /// Durable UUID assigned when one assistant content block completes.
    AssistantBlockIdentity {
        block_key: u64,
        message_uuid: String,
    },
    /// Remove a durable assistant row superseded by an accepted server fallback.
    ServerFallbackTombstone {
        message: ServerFallbackTombstoneMessage,
        display_only: bool,
    },
    /// Begin exact-text continuation from retained refusal rows.
    RefusalContinuation {
        phase: RefusalContinuationPhase,
        salvage_text: String,
        join: RefusalContinuationJoin,
        replaces_uuids: Vec<lingxi_core::types::MessageId>,
        display_salvage_text: bool,
    },
    /// The live command registry was reconciled. The render-thread owner swaps
    /// this complete snapshot into the bottom pane in one event, preserving the
    /// composer and repairing any now-invalid completion selection.
    CommandCatalogRefreshed {
        /// Complete registry-backed completion catalog.
        commands: Vec<CommandCatalogEntry>,
    },
    /// Full snapshot of background agents that are pending or running. Sent by
    /// the CLI task-registry poller; replaces the previous snapshot atomically.
    AgentStatusSnapshot {
        /// Currently live background agents.
        agents: Vec<RunningAgentStatus>,
    },
    /// Event-driven workflow/task lifecycle update. This shares the same
    /// render-loop channel as turn output, so an open `/workflows` view updates
    /// immediately without a timer or registry poll.
    MultiAgent(crate::multiagent::MultiAgentEvent),
    /// The summary pass started. Retained for direct embedders; production
    /// progress uses `CompactPhase` for separate preparation and summary clocks.
    CompactStarted,
    /// The compaction pass finished (success or failure): the TUI clears the
    /// `Compacting conversation…` spinner/progress bar. The terminal
    /// `Compacted (ctrl+o to see full summary)` line arrives separately as a
    /// [`Self::SystemNotice`]. Paired with [`Self::CompactStarted`].
    CompactEnded,
    /// Engine-observed compaction lifecycle, independent of the manual task
    /// completion acknowledgement. Terminal phases clear progress without
    /// releasing the manual submit guard before its boundary event arrives.
    CompactPhase { phase: String },
    /// Captured output of a `!`-prefixed bash-mode command (run off the model
    /// path). Rendered as a `UserBashOutput` cell — ANSI-parsed stdout then
    /// error-tinted stderr. Sent by the CLI `on_bash` closure after the
    /// sandboxed [`tool_api::bash_runner::BashRunner`] returns.
    BashOutput {
        /// Captured stdout.
        stdout: String,
        /// Captured stderr.
        stderr: String,
    },
    /// A provider gained a usable credential mid-session (a successful
    /// `/connect` StoreKey / OAuth / Copilot login). The CLI's connect closure
    /// sends this so the widget flips its live availability map — which gates
    /// the `/model` picker (and badges the `/connect` picker) — WITHOUT a
    /// restart. NOT emitted by [`BridgeOutputStream`]; sent directly like
    /// [`Self::SystemNotice`]. Keyed by `profile_name` (e.g. `"openrouter"`),
    /// the same key space as the launch availability map.
    ProviderConnected {
        /// The provider/profile that just became available.
        provider_id: String,
    },
    /// One nested execution line from a running subagent (its tool calls, as
    /// they happen), surfaced under the `Task` cell — otherwise a subagent's
    /// inner work is invisible. Rendered as an indented `⎿` line.
    SubagentActivity {
        /// Pre-formatted one-line summary, e.g. `"Read(src/main.rs)"`.
        text: String,
    },
    /// An API request failed with a retry-worthy error and is backing off before
    /// the next attempt. Rendered as Claude Code's `SystemAPIErrorMessage`:
    /// `"<message> · Retrying in Ns… (attempt X/Y)"`, with `delay_ms` seeding a
    /// live countdown. Cleared when the turn produces content or ends.
    ApiRetry {
        /// User-facing error text (e.g. `"provider internal error"`).
        message: String,
        /// 1-based attempt number about to be retried.
        attempt: u32,
        /// Configured retry cap (default 10).
        max_retries: u32,
        /// Backoff before the next attempt, in ms (the countdown seed).
        delay_ms: u64,
    },
    /// A user-visible attachment surfaced during the turn — the oracle's `k$o`
    /// records, drawn as the "Listed directory …" family.
    Attachment {
        /// Which attachment this is.
        attachment: crate::message::Attachment,
    },
}

/// `OutputStream` impl that forwards every callback as a `TurnEvent` on
/// an mpsc channel. Cloneable via `tx.clone()` if multiple producers are
/// ever needed (currently one bridge per session — the channel lives for
/// the whole TUI lifetime).
pub struct BridgeOutputStream {
    tx: UnboundedSender<TurnEvent>,
    heartbeats: Arc<CoalescedToolHeartbeats>,
    omit_thinking: AtomicBool,
}

impl BridgeOutputStream {
    /// Wrap a sender. The receiver lives on the TUI side and is drained
    /// by the render loop.
    #[must_use]
    pub fn new(tx: UnboundedSender<TurnEvent>) -> Self {
        Self {
            tx,
            heartbeats: Arc::new(CoalescedToolHeartbeats::default()),
            omit_thinking: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl OutputStream for BridgeOutputStream {
    async fn emit_assistant_message_identity(&self, message_id: &lingxi_core::types::MessageId) {
        let _ = self.tx.send(TurnEvent::MessageIdentity(*message_id));
    }

    async fn emit_message_retracted(&self, message_id: &lingxi_core::types::MessageId) {
        let _ = self.tx.send(TurnEvent::MessageRetracted(*message_id));
    }

    async fn emit_turn_started(&self) {
        let _ = self.tx.send(TurnEvent::TurnStarted);
    }

    async fn emit_text(&self, text: &str, _utf16_code_units: Option<&[u16]>) {
        let _ = self.tx.send(TurnEvent::TextDelta(text.to_string()));
    }

    async fn emit_system_notice(&self, body: &str, is_error: bool) {
        let _ = self.tx.send(TurnEvent::SystemNotice {
            body: body.to_string(),
            is_error,
        });
    }

    async fn emit_mod_log(&self, plugin: &str, text: &str) {
        let _ = self.tx.send(TurnEvent::UiLog {
            plugin: plugin.to_string(),
            text: text.to_string(),
        });
    }

    async fn emit_mod_toast(&self, plugin: &str, text: &str, timeout_ms: u64) {
        let _ = self.tx.send(TurnEvent::UiToast {
            plugin: plugin.to_string(),
            text: text.to_string(),
            timeout_ms,
        });
    }

    async fn emit_mod_status(&self, plugin: &str, text: Option<&str>) {
        let _ = self.tx.send(TurnEvent::UiStatus {
            plugin: plugin.to_string(),
            text: text.map(str::to_string),
        });
    }

    async fn emit_user_transcript_row_identity(&self, row_token: &str, uuid: &str) {
        let _ = self.tx.send(TurnEvent::TranscriptRowIdentity {
            row_token: row_token.to_string(),
            uuid: uuid.to_string(),
        });
    }

    async fn emit_assistant_transcript_row_uuids(
        &self,
        message_id: &lingxi_core::types::MessageId,
        uuids: &[Option<String>],
    ) {
        let _ = self.tx.send(TurnEvent::AssistantTranscriptRowUuids {
            message_id: *message_id,
            uuids: uuids.to_vec(),
        });
    }

    async fn emit_server_fallback_query_model_change(&self, to_model: &str) {
        let _ = self.tx.send(TurnEvent::ServerFallbackQueryModelChange {
            to_model: to_model.to_string(),
        });
    }

    async fn emit_assistant_block_start(&self, block_key: u64) {
        let _ = self.tx.send(TurnEvent::AssistantBlockStart { block_key });
    }

    async fn emit_assistant_block_identity(
        &self,
        block_key: u64,
        row_id: &lingxi_core::types::MessageId,
    ) {
        let _ = self.tx.send(TurnEvent::AssistantBlockIdentity {
            block_key,
            message_uuid: row_id.as_uuid().to_string(),
        });
    }

    async fn emit_server_fallback_tombstone(
        &self,
        message: &ServerFallbackTombstoneMessage,
        display_only: bool,
    ) {
        let _ = self.tx.send(TurnEvent::ServerFallbackTombstone {
            message: message.clone(),
            display_only,
        });
    }

    async fn emit_refusal_continuation_begin(
        &self,
        salvage_text: &str,
        replaces_uuids: &[lingxi_core::types::MessageId],
        display_salvage_text: bool,
    ) {
        let _ = self.tx.send(TurnEvent::RefusalContinuation {
            phase: RefusalContinuationPhase::Begin,
            salvage_text: salvage_text.to_string(),
            join: RefusalContinuationJoin::Exact,
            replaces_uuids: replaces_uuids.to_vec(),
            display_salvage_text,
        });
    }

    async fn emit_thinking(&self, thinking: &str, _signature: Option<&str>) {
        if self.omit_thinking.load(Ordering::Relaxed) {
            return;
        }
        let _ = self.tx.send(TurnEvent::ThinkingDelta(thinking.to_string()));
    }

    fn set_thinking_display(&self, mode: Option<&str>) {
        self.omit_thinking
            .store(mode == Some("omitted"), Ordering::Relaxed);
    }

    async fn emit_subagent_activity(&self, text: &str) {
        let _ = self.tx.send(TurnEvent::SubagentActivity {
            text: text.to_string(),
        });
    }

    async fn emit_attachment(&self, attachment: lingxi_core::host::AttachmentKind) {
        let attachment = match attachment {
            lingxi_core::host::AttachmentKind::NestedMemory { display_path } => {
                crate::message::Attachment::NestedMemory { display_path }
            }
            // Unmodelled kind: drop rather than guess a variant. Dropping draws
            // nothing, which is visible; guessing draws something false.
            _ => return,
        };
        let _ = self.tx.send(TurnEvent::Attachment { attachment });
    }

    async fn emit_api_retry(&self, message: &str, attempt: u32, max_retries: u32, delay_ms: u64) {
        let _ = self.tx.send(TurnEvent::ApiRetry {
            message: message.to_string(),
            attempt,
            max_retries,
            delay_ms,
        });
    }

    async fn emit_tool_call(
        &self,
        id: &lingxi_core::types::ToolUseId,
        tool: &str,
        input: &serde_json::Value,
        _input_projection: Option<&lingxi_core::types::utf16_json::Utf16JsonProjection>,
    ) {
        let _ = self.tx.send(TurnEvent::ToolUseStart {
            id: id.clone(),
            tool: tool.to_string(),
            input: input.clone(),
        });
    }

    async fn emit_tool_heartbeat(
        &self,
        id: &lingxi_core::types::ToolUseId,
        tool: &str,
        elapsed_ms: u64,
    ) {
        let update = ToolHeartbeatUpdate {
            id: id.clone(),
            tool: tool.to_string(),
            elapsed_ms,
        };
        if self.heartbeats.publish(update)
            && self
                .tx
                .send(TurnEvent::ToolHeartbeatBatch {
                    heartbeats: Arc::clone(&self.heartbeats),
                })
                .is_err()
        {
            self.heartbeats.reset_after_send_failure();
        }
    }

    async fn emit_hook_progress_started(
        &self,
        progress_id: &str,
        hook_name: &str,
        hook_event: &str,
        status_message: Option<&str>,
    ) {
        let text = status_message
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Running {hook_event} hook {hook_name}\u{2026}"));
        let _ = self.tx.send(TurnEvent::HookProgressStarted {
            id: progress_id.to_string(),
            text,
        });
    }

    async fn emit_hook_progress_finished(&self, progress_id: &str) {
        let _ = self.tx.send(TurnEvent::HookProgressFinished {
            id: progress_id.to_string(),
        });
    }

    async fn emit_tool_result(
        &self,
        id: &lingxi_core::types::ToolUseId,
        tool: &str,
        model_text: &str,
        result: &serde_json::Value,
        _projection: Option<&lingxi_core::host::ToolResultProjection>,
    ) {
        // (gap-3 general) Many tools return a structured `data` object with no
        // human-display string (Read's `{type,file:{…}}`, …), so the scrollback
        // renderer pretty-printed the raw JSON. `model_text` is the human/model
        // -facing body the tool already produced ("the render rides on
        // model_content" — read.rs). Carry it into the result object as
        // `model_content` so `body_text` can surface it; the structured fields
        // stay intact for tools that render off them (Bash stdout/stderr,
        // Edit/Write diffs).
        let result = match result.as_object() {
            Some(obj) if !model_text.is_empty() && !obj.contains_key("model_content") => {
                let mut obj = obj.clone();
                obj.insert(
                    "model_content".to_string(),
                    serde_json::Value::String(model_text.to_string()),
                );
                serde_json::Value::Object(obj)
            }
            _ => result.clone(),
        };
        let _ = self.tx.send(TurnEvent::ToolUseResult {
            id: id.clone(),
            tool: tool.to_string(),
            result,
        });
    }

    async fn emit_compaction_started(&self) {
        self.emit_compaction_phase("preparing").await;
    }

    async fn emit_compaction_phase(&self, phase: &str) {
        let _ = self.tx.send(TurnEvent::CompactPhase {
            phase: phase.into(),
        });
    }

    async fn emit_compaction_skipped(&self) {
        self.emit_compaction_phase("skipped").await;
    }

    async fn emit_compaction_finished(&self, error: Option<&str>) {
        self.emit_compaction_phase(match error {
            None => "complete",
            Some("Compaction canceled.") => "cancelled",
            Some(_) => "error",
        })
        .await;
    }

    async fn emit_compaction_completed(
        &self,
        messages_before: u32,
        messages_after: u32,
        bytes_saved: u64,
        summary: &str,
    ) {
        let _ = self.tx.send(TurnEvent::CompactionCompleted {
            messages_before,
            messages_after,
            bytes_saved,
            summary: summary.to_string(),
        });
    }

    async fn emit_context_pressure(
        &self,
        banner: Option<ContextPressureBanner>,
        used_fraction: f32,
        used_tokens: u64,
        context_window_tokens: u64,
    ) {
        let _ = self.tx.send(TurnEvent::ContextPressure {
            banner,
            used_fraction,
            used_tokens,
            context_window_tokens,
        });
    }

    async fn emit_terminal_sequence(&self, seq: &str) {
        let _ = self.tx.send(TurnEvent::TerminalSequence {
            seq: seq.to_string(),
        });
    }

    async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot) {
        // M6-06: emit a CostUpdated event before TurnEnded so the
        // status-line refreshes to the post-turn cost in the next
        // render pass. Format follows claude-code's `toFixed(4)` parity.
        let cost_str = format!("${:.4}", cost.total_usd);
        let _ = self.tx.send(TurnEvent::CostUpdated(cost_str));
        let _ = self.tx.send(TurnEvent::CostSnapshotUpdated(cost.clone()));

        // Map stop_reason → TurnOutcome. Mirrors the M5-13 stdio REPL
        // mapping. Unknown/unrecognised reasons fall back to EndTurn.
        let outcome = match stop_reason {
            "max_tokens" => TurnOutcome::MaxTurns,
            _ => TurnOutcome::EndTurn,
        };
        let _ = self.tx.send(TurnEvent::TurnEnded(outcome));
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "mirrors the trait method's eleven header-derived fields (see lingxi_core::host::OutputStream::emit_rate_limit)"
    )]
    async fn emit_rate_limit(
        &self,
        status: Option<&str>,
        rate_limit_type: Option<&str>,
        utilization: Option<f64>,
        resets_at: Option<u64>,
        claim_resets_at: Option<u64>,
        overage_status: Option<&str>,
        overage_resets_at: Option<u64>,
        overage_disabled_reason: Option<&str>,
        fallback_available: Option<bool>,
        upgrade_paths: Option<&[String]>,
        credits_required: bool,
    ) {
        let _ = self.tx.send(TurnEvent::RateLimit {
            status: status.map(str::to_owned),
            rate_limit_type: rate_limit_type.map(str::to_owned),
            utilization,
            resets_at,
            claim_resets_at,
            overage_status: overage_status.map(str::to_owned),
            overage_resets_at,
            overage_disabled_reason: overage_disabled_reason.map(str::to_owned),
            fallback_available,
            upgrade_paths: upgrade_paths.map(<[String]>::to_vec),
            credits_required,
        });
    }

    async fn emit_raw_utilization(
        &self,
        five_hour_utilization: Option<f64>,
        five_hour_resets_at: Option<u64>,
        seven_day_utilization: Option<f64>,
        seven_day_resets_at: Option<u64>,
    ) {
        let _ = self.tx.send(TurnEvent::RawUtilization {
            five_hour_utilization,
            five_hour_resets_at,
            seven_day_utilization,
            seven_day_resets_at,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn server_fallback_callbacks_keep_row_identity_and_continuation_facts_ordered() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let stream = BridgeOutputStream::new(tx);
        let row_id = lingxi_core::types::MessageId::new();
        stream
            .emit_server_fallback_query_model_change("claude-sonnet-4")
            .await;
        stream.emit_assistant_block_start(41).await;
        stream.emit_assistant_block_identity(41, &row_id).await;
        stream
            .emit_server_fallback_tombstone(
                &ServerFallbackTombstoneMessage {
                    uuid: row_id,
                    message_type: "assistant".into(),
                    timestamp: "2026-10-03T12:00:00.000Z".into(),
                    request_id: Some("request-1".into()),
                    request_ref: Some(serde_json::json!({"lane": "main"})),
                    provider_message_id: Some("provider-1".into()),
                    model: Some("claude-sonnet-4".into()),
                    stop_reason: Some("refusal".into()),
                    stop_details: None,
                    usage: None,
                    content: vec![lingxi_core::types::ContentBlock::Text {
                        text: "old refusal".into(),
                    }],
                    is_api_error_message: None,
                    supersedes_uuids: None,
                },
                true,
            )
            .await;
        stream
            .emit_refusal_continuation_begin("retained", &[row_id], true)
            .await;

        assert!(matches!(
            rx.recv().await,
            Some(TurnEvent::ServerFallbackQueryModelChange { to_model }) if to_model == "claude-sonnet-4"
        ));
        assert!(matches!(
            rx.recv().await,
            Some(TurnEvent::AssistantBlockStart { block_key: 41 })
        ));
        assert!(matches!(
            rx.recv().await,
            Some(TurnEvent::AssistantBlockIdentity { block_key: 41, message_uuid })
                if message_uuid == row_id.as_uuid().to_string()
        ));
        assert!(matches!(
            rx.recv().await,
            Some(TurnEvent::ServerFallbackTombstone { message, display_only: true })
                if message.uuid == row_id && message.request_id.as_deref() == Some("request-1")
        ));
        assert!(matches!(
            rx.recv().await,
            Some(TurnEvent::RefusalContinuation {
                phase: RefusalContinuationPhase::Begin,
                salvage_text,
                join: RefusalContinuationJoin::Exact,
                replaces_uuids,
                display_salvage_text: true,
            }) if salvage_text == "retained" && replaces_uuids == vec![row_id]
        ));
    }

    #[tokio::test]
    async fn assistant_identity_and_retraction_preserve_exact_id() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let stream = BridgeOutputStream::new(tx);
        let id = lingxi_core::types::MessageId::new();
        stream.emit_assistant_message_identity(&id).await;
        stream.emit_message_retracted(&id).await;
        assert!(
            matches!(rx.recv().await, Some(TurnEvent::MessageIdentity(actual)) if actual == id)
        );
        assert!(
            matches!(rx.recv().await, Some(TurnEvent::MessageRetracted(actual)) if actual == id)
        );
    }
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn emit_text_translates_to_text_delta() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge.emit_text("hello", None).await;
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, TurnEvent::TextDelta(ref s) if s == "hello"));
    }

    #[tokio::test]
    async fn emit_system_notice_translates_to_error_notice() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_system_notice("transcript unavailable", true)
            .await;
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::SystemNotice { body, is_error }
                if body == "transcript unavailable" && is_error
        ));
    }

    #[tokio::test]
    async fn emit_mod_log_keeps_plugin_and_text_separate() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge.emit_mod_log("review", "Found a mismatch").await;
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::UiLog { plugin, text }
                if plugin == "review" && text == "Found a mismatch"
        ));
    }

    #[tokio::test]
    async fn transcript_row_identity_events_keep_persisted_uuids_separate_from_message_ids() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let message_id = lingxi_core::types::MessageId::new();
        let persisted_uuid = "83f67a72-a806-49b7-9a18-e57307177a86";
        bridge
            .emit_user_transcript_row_identity("ui-row-token", persisted_uuid)
            .await;
        bridge
            .emit_assistant_transcript_row_uuids(&message_id, &[Some(persisted_uuid.to_string())])
            .await;

        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::TranscriptRowIdentity { row_token, uuid }
                if row_token == "ui-row-token" && uuid == persisted_uuid
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::AssistantTranscriptRowUuids { message_id: actual, uuids }
                if actual == message_id && uuids == vec![Some(persisted_uuid.to_string())]
        ));
    }

    #[tokio::test]
    async fn emit_compaction_started_prepares_without_starting_the_summary_clock() {
        // The orchestrator emits this before every compaction pass (manual
        // AND auto); the bridge must forward it or the TUI's `Compacting
        // conversation…` progress UI never appears.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        lingxi_core::host::OutputStream::emit_compaction_started(&bridge).await;
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, TurnEvent::CompactPhase { phase } if phase == "preparing"));
    }

    #[tokio::test]
    async fn compaction_phases_and_failure_finish_reach_the_tui() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge.emit_compaction_phase("summarizing").await;
        bridge.emit_compaction_phase("restoring").await;
        bridge
            .emit_compaction_finished(Some("summary failed"))
            .await;
        bridge
            .emit_compaction_finished(Some("Compaction canceled."))
            .await;
        bridge.emit_compaction_finished(None).await;
        bridge.emit_compaction_skipped().await;
        for expected in [
            "summarizing",
            "restoring",
            "error",
            "cancelled",
            "complete",
            "skipped",
        ] {
            assert!(
                matches!(rx.recv().await.unwrap(), TurnEvent::CompactPhase { phase } if phase == expected)
            );
        }
    }

    #[tokio::test]
    async fn emit_thinking_translates_to_thinking_delta() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge.emit_thinking("let me reason", Some("sig-abc")).await;
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, TurnEvent::ThinkingDelta(ref s) if s == "let me reason"));
    }

    #[tokio::test]
    async fn omitted_thinking_display_drops_thinking_until_restored() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge.set_thinking_display(Some("omitted"));
        bridge.emit_thinking("hidden", None).await;
        assert!(rx.try_recv().is_err());

        bridge.set_thinking_display(Some("summarized"));
        bridge.emit_thinking("visible", None).await;
        assert!(matches!(
            rx.recv().await,
            Some(TurnEvent::ThinkingDelta(ref s)) if s == "visible"
        ));
    }

    #[tokio::test]
    async fn emit_tool_call_translates_to_tool_use_start() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let id = lingxi_core::types::ToolUseId::new();
        bridge
            .emit_tool_call(
                &id,
                "Read",
                &serde_json::json!({"file_path": "/tmp/x"}),
                None,
            )
            .await;
        match rx.recv().await.unwrap() {
            TurnEvent::ToolUseStart {
                id: gid,
                tool,
                input,
            } => {
                assert_eq!(gid, id);
                assert_eq!(tool, "Read");
                assert_eq!(input["file_path"], "/tmp/x");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn hook_progress_is_transient_and_preserves_status_message() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_hook_progress_started(
                "hook-1:run-1",
                "formatter",
                "PostToolUse",
                Some("Formatting\u{2026}"),
            )
            .await;
        bridge.emit_hook_progress_finished("hook-1:run-1").await;

        assert!(matches!(
            rx.recv().await,
            Some(TurnEvent::HookProgressStarted { id, text })
                if id == "hook-1:run-1" && text == "Formatting\u{2026}"
        ));
        assert!(matches!(
            rx.recv().await,
            Some(TurnEvent::HookProgressFinished { id }) if id == "hook-1:run-1"
        ));
    }

    #[tokio::test]
    async fn emit_tool_heartbeat_translates_without_transcript_payload() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let id = lingxi_core::types::ToolUseId::new();
        bridge.emit_tool_heartbeat(&id, "Bash", 12_345).await;
        match rx.recv().await.unwrap() {
            TurnEvent::ToolHeartbeatBatch { heartbeats } => {
                let updates = heartbeats.drain();
                assert_eq!(updates.len(), 1);
                let update = &updates[0];
                let got = &update.id;
                let tool = &update.tool;
                let elapsed_ms = update.elapsed_ms;
                assert_eq!(got, &id);
                assert_eq!(tool, "Bash");
                assert_eq!(elapsed_ms, 12_345);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_heartbeats_coalesce_under_consumer_backpressure() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let id = lingxi_core::types::ToolUseId::new();

        bridge.emit_tool_heartbeat(&id, "Bash", 1_000).await;
        bridge.emit_tool_heartbeat(&id, "Bash", 2_000).await;
        bridge.emit_tool_heartbeat(&id, "Bash", 3_000).await;

        let TurnEvent::ToolHeartbeatBatch { heartbeats } = rx.recv().await.unwrap() else {
            panic!("expected a coalesced heartbeat wake-up");
        };
        let updates = heartbeats.drain();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].elapsed_ms, 3_000);
        assert!(rx.try_recv().is_err(), "only one wake-up may be queued");
    }

    #[tokio::test]
    async fn emit_end_turn_endturn_reason_translates_to_endturn() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let cost = lingxi_core::host::CostSnapshot::default();
        bridge.emit_end_turn("end_turn", &cost).await;
        // M6-06: emit_end_turn now precedes TurnEnded with a CostUpdated event.
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::CostUpdated(ref s) if s == "$0.0000"
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::CostSnapshotUpdated(_)
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::TurnEnded(TurnOutcome::EndTurn)
        ));
    }

    #[tokio::test]
    async fn emit_end_turn_max_tokens_translates_to_maxturns() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let cost = lingxi_core::host::CostSnapshot::default();
        bridge.emit_end_turn("max_tokens", &cost).await;
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::CostUpdated(_)
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::CostSnapshotUpdated(_)
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            TurnEvent::TurnEnded(TurnOutcome::MaxTurns)
        ));
    }

    #[tokio::test]
    async fn emit_end_turn_formats_real_cost_4dp() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        let cost = lingxi_core::host::CostSnapshot {
            total_usd: 0.0234,
            ..lingxi_core::host::CostSnapshot::default()
        };
        bridge.emit_end_turn("end_turn", &cost).await;
        match rx.recv().await.unwrap() {
            TurnEvent::CostUpdated(s) => assert_eq!(s, "$0.0234"),
            other => panic!("expected CostUpdated($0.0234), got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn emit_rate_limit_translates_to_rate_limit_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_rate_limit(
                Some("rejected"),
                Some("five_hour"),
                Some(0.95),
                Some(1_900_000_000),
                Some(1_900_000_100),
                Some("allowed_warning"),
                Some(1_900_000_200),
                Some("out_of_credits"),
                Some(true),
                Some(&["overage".to_string()]),
                true,
            )
            .await;
        match rx.try_recv().expect("bridge must forward a TurnEvent") {
            TurnEvent::RateLimit {
                status,
                rate_limit_type,
                utilization,
                resets_at,
                claim_resets_at,
                overage_status,
                overage_resets_at,
                overage_disabled_reason,
                fallback_available,
                upgrade_paths,
                credits_required,
            } => {
                assert_eq!(status.as_deref(), Some("rejected"));
                assert_eq!(rate_limit_type.as_deref(), Some("five_hour"));
                assert_eq!(utilization, Some(0.95));
                assert_eq!(resets_at, Some(1_900_000_000));
                assert_eq!(claim_resets_at, Some(1_900_000_100));
                assert_eq!(overage_status.as_deref(), Some("allowed_warning"));
                assert_eq!(overage_resets_at, Some(1_900_000_200));
                assert_eq!(overage_disabled_reason.as_deref(), Some("out_of_credits"));
                assert_eq!(fallback_available, Some(true));
                assert_eq!(upgrade_paths, Some(vec!["overage".to_string()]));
                assert!(credits_required);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn emit_rate_limit_all_none_still_forwards() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_rate_limit(
                None, None, None, None, None, None, None, None, None, None, false,
            )
            .await;
        assert!(matches!(
            rx.try_recv().expect("bridge must forward a TurnEvent"),
            TurnEvent::RateLimit { status: None, .. }
        ));
    }

    #[tokio::test]
    async fn emit_context_pressure_translates_to_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_context_pressure(
                Some(lingxi_core::host::ContextPressureBanner {
                    text: "Context low (8% remaining) \u{00b7} Run /compact to compact & continue"
                        .into(),
                    level: lingxi_core::host::ContextPressureLevel::Error,
                }),
                0.92,
                184_000,
                200_000,
            )
            .await;
        match rx.try_recv().expect("bridge must forward a TurnEvent") {
            TurnEvent::ContextPressure {
                banner: Some(b),
                used_fraction,
                used_tokens,
                context_window_tokens,
            } => {
                assert_eq!(
                    b.text,
                    "Context low (8% remaining) \u{00b7} Run /compact to compact & continue"
                );
                assert_eq!(b.level, lingxi_core::host::ContextPressureLevel::Error);
                assert!((used_fraction - 0.92).abs() < 1e-6);
                assert_eq!(used_tokens, 184_000);
                assert_eq!(context_window_tokens, 200_000);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn emit_context_pressure_none_clears() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge.emit_context_pressure(None, 0.0, 0, 0).await;
        assert!(matches!(
            rx.try_recv().expect("bridge must forward a TurnEvent"),
            TurnEvent::ContextPressure { banner: None, .. }
        ));
    }

    #[tokio::test]
    async fn emit_raw_utilization_translates_to_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = BridgeOutputStream::new(tx);
        bridge
            .emit_raw_utilization(Some(0.42), Some(1_750_000_000), None, None)
            .await;
        match rx.try_recv().expect("bridge must forward a TurnEvent") {
            TurnEvent::RawUtilization {
                five_hour_utilization,
                five_hour_resets_at,
                seven_day_utilization,
                seven_day_resets_at,
            } => {
                assert_eq!(five_hour_utilization, Some(0.42));
                assert_eq!(five_hour_resets_at, Some(1_750_000_000));
                assert_eq!(seven_day_utilization, None);
                assert_eq!(seven_day_resets_at, None);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn turn_started_is_caller_emitted_not_bridge() {
        // TurnStarted is fired by the SPAWNER (app.rs), not the bridge.
        // Documenting that contract: the bridge has no method that
        // produces TurnStarted; callers send it manually.
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(TurnEvent::TurnStarted).unwrap();
        assert!(matches!(rx.recv().await.unwrap(), TurnEvent::TurnStarted));
    }
}
