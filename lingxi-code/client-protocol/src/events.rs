//! `ClientEvent` DTOs — outbound events the engine streams to a client.
//!
//! The `Error` variant + `ErrorKindDto` land in F1-01 (the first variant, so
//! every later DTO inherits the frozen serde conventions). The live-turn
//! streaming variants land in F1-03.
//!
//! Frozen serde conventions (decision §0.1):
//! - internally tagged: `#[serde(tag = "type", rename_all = "snake_case")]`
//!   (matches `protocol::ContentBlock` / api-client `StreamEvent`),
//! - top-level enum is `#[non_exhaustive]` (mirrors `traits::OutputEvent`),
//! - every optional field uses
//!   `#[serde(default, skip_serializing_if = "Option::is_none")]`.
//!
//! Tool payloads are JSON **Strings** (`input_json`/`result_json`); the
//! `serde_json::Value` lowering happens in `client-adapter`, NOT here
//! (decision §0.4).

use crate::listings::{
    AgentDto, AuthStateDto, CoordinatorWorkerDto, DoctorReportDto, HookDto, McpServerDto,
    MemoryEntryDto, SessionRowDto, SlashCommandDto, StatusSnapshotDto, TaskRowDto, TaskStatusDto,
};
use crate::message::MessageDto;
use serde::{Deserialize, Serialize};

/// Outbound events the engine streams to a client.
///
/// Each live-turn variant notes its engine source (from the area maps).
/// `ThinkingDelta` and `UsageUpdate` are now LIVE-FED by the §0.7 "light up
/// thinking/usage" follow-up: `event_router` emits them via
/// `OutputStream::emit_thinking` / `emit_usage`. The remaining reserved
/// variants (e.g. `CoordinatorStatus`) are defined so the contract freezes now
/// but still have no live engine source in the foundation (decisions §0.7 /
/// §0.9) and round-trip only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClientEvent {
    /// A terminal/protocol error surfaced to the client. The FIRST variant
    /// (F1-01); every later DTO inherits its serde conventions.
    Error {
        /// Coarse error class for client-side branching.
        kind: ErrorKindDto,
        /// Human-readable error message.
        message: String,
    },

    // ── Live-turn streaming events (F1-03) ────────────────────────────────

    /// Plain assistant text. 1:1 `OutputStream::emit_text`.
    TextDelta {
        /// The text payload emitted.
        text: String,
    },

    /// A tool invocation about to dispatch. 1:1 `emit_tool_call`; the
    /// `serde_json::Value` input is lowered to a JSON String (`input_json`,
    /// decision §0.4).
    ToolUseStarted {
        /// Stable id echoed in the matching [`ClientEvent::ToolUseResult`].
        id: String,
        /// Name of the tool being invoked.
        tool: String,
        /// Tool input as a JSON String.
        input_json: String,
    },

    /// A tool result returning to the conversation. 1:1 `emit_tool_result`;
    /// fires in COMPLETION order — clients key by `id`.
    ToolUseResult {
        /// Correlator with the matching [`ClientEvent::ToolUseStarted`].
        id: String,
        /// Name of the tool that returned.
        tool: String,
        /// Tool result as a JSON String.
        result_json: String,
        /// Whether the tool reported failure.
        is_error: bool,
    },

    /// The assistant message boundary. SYNTHESIZED by the adapter — there is no
    /// engine message-boundary event. Carries the reproduced [`MessageDto`].
    MessageComplete {
        /// Stop reason reported by the model (e.g. `"end_turn"`), if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
        /// The completed message block set, if reconstructable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<MessageDto>,
    },

    /// Adapter-synthesized on `SendPrompt` receipt — the engine never emits a
    /// turn-start event.
    TurnStarted {
        /// Optional client-supplied turn correlator.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<u64>,
    },

    /// End-of-turn marker. 1:1 `emit_end_turn`.
    TurnEnded {
        /// How the turn ended.
        outcome: TurnOutcomeDto,
        /// Stop reason reported by the model, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
        /// Cumulative cost snapshot at end of turn.
        cost: CostDto,
    },

    /// A cumulative cost update — the [`CostSnapshot`](traits) lowered
    /// (`Duration` → secs). Mirrors the `/cost` render fields.
    CostUpdate {
        /// Cumulative cost in USD.
        total_usd: f64,
        /// Cumulative input tokens across all turns.
        input_tokens: u64,
        /// Cumulative output tokens across all turns.
        output_tokens: u64,
        /// Cumulative successful API calls.
        api_calls: u32,
        /// Elapsed session time in whole seconds.
        session_duration_secs: u64,
        /// Pre-formatted display string (e.g. `"$0.0123"`).
        formatted: String,
    },

    /// A compaction finished. 1:1 `emit_compaction_completed`.
    CompactionCompleted {
        /// Message count BEFORE compaction.
        messages_before: u32,
        /// Message count AFTER compaction.
        messages_after: u32,
        /// UX estimate of bytes freed.
        bytes_saved: u64,
    },

    // ── Listing / screen events (F1-05) ───────────────────────────────────
    //
    // The pull/reply payloads for every client screen. The supporting row /
    // payload structs live in `crate::listings`. Name reconciliation (plan
    // line 149): the design spec §4.1 says `AgentList`, but the WIRE name is
    // `Agents`.

    /// Session lifecycle: a session began on this connection. Carries the
    /// `session_id` as a CONNECTION ATTRIBUTE (decision §0.5) — it travels on
    /// this event, never as a per-live-command param.
    SessionStarted {
        /// The session id now driving the connection's orchestrator.
        session_id: String,
    },

    /// Session lifecycle: the current session ended (unit-style marker).
    SessionEnded,

    /// Session lifecycle: a prior session was resumed on this connection.
    /// Carries the resumed `session_id` (a connection attribute, decision §0.5)
    /// AND the full restored transcript as `messages` (OLDEST-FIRST) so the
    /// client renders the rehydrated conversation atomically — the live
    /// `ResumeSession` path hot-restores the on-disk session into the running
    /// orchestrator, so the next turn continues with full prior context.
    ///
    /// `messages` is REQUIRED (always present, may be empty for a zero-message
    /// session) — it is the lowered [`MessageDto`] scrollback the host produces
    /// from the replayed history via `client_adapter::lowering::lower_transcript`.
    /// This is an ADDITIVE field on an existing variant (no new enum variant, no
    /// renamed/retyped leaf), so it does NOT change the event COUNT and does NOT
    /// require a `CLIENT_PROTOCOL_VERSION` major bump (decision §0.10).
    SessionResumed {
        /// The session id resumed onto the connection's orchestrator.
        session_id: String,
        /// The full restored transcript, OLDEST-FIRST. Always present (may be
        /// empty). Carries the rehydrated conversation so the client renders it
        /// atomically on resume.
        messages: Vec<MessageDto>,
    },

    /// The resumable-session catalog (`/resume` / session picker). Maps
    /// `SessionMetadata`; rows carry `.path` directly (plan line 152).
    SessionList {
        /// One row per resumable session, newest-first.
        sessions: Vec<SessionRowDto>,
    },

    /// The available-model catalog + the active model (`/model` no-arg list).
    ModelList {
        /// Model names the orchestrator will accept via `SetModel`.
        models: Vec<String>,
        /// The currently active model.
        current: String,
    },

    /// The active model changed (1:1 with a successful `SetModel` /
    /// `switch_model`).
    ModelChanged {
        /// The model now active for subsequent turns.
        model: String,
    },

    /// The MCP server listing (`/mcp`). Maps `McpServerInfo`.
    McpServers {
        /// One entry per configured MCP server.
        servers: Vec<McpServerDto>,
    },

    /// The hook listing (`/hooks`). Maps `HookInfo`.
    Hooks {
        /// One entry per registered hook (built-in + user).
        hooks: Vec<HookDto>,
    },

    /// The subagent listing (`/agents`). Maps `AgentInfo`. WIRE name `Agents`
    /// (reconciled from spec §4.1 `AgentList`, plan line 149).
    Agents {
        /// One entry per registered subagent.
        agents: Vec<AgentDto>,
    },

    /// The slash-command catalog. Maps the `SlashCommand` registry collapsed to
    /// display fields.
    SlashCommandCatalog {
        /// One entry per registered slash command.
        commands: Vec<SlashCommandDto>,
    },

    /// The CLAUDE.md memory listing (`/memory`). Maps `protocol::MemoryEntry`.
    MemoryEntries {
        /// One entry per loaded memory file, in tier order.
        entries: Vec<MemoryEntryDto>,
    },

    /// The `/status` panel snapshot. Maps `StatusSnapshot` (traits shape
    /// canonical; status-line fields appended OPTIONAL, plan line 155).
    StatusSnapshot {
        /// The full status snapshot.
        snapshot: StatusSnapshotDto,
    },

    /// Read-only effective settings + per-field provenance (`/config` view).
    /// Both payloads are JSON **Strings** on the wire (decision §0.4).
    SettingsSnapshot {
        /// The merged effective settings, as a JSON String.
        effective_json: String,
        /// Per-field provenance (which layer set each value), as a JSON String.
        provenance_json: String,
    },

    /// The auth state (`/login` / `/logout` / `/status`). Maps
    /// `Option<LoginInfo>`.
    AuthState {
        /// Signed-out, or a signed-in user.
        state: AuthStateDto,
    },

    /// The `/doctor` diagnostic report. Maps `DoctorReport`.
    DoctorReport {
        /// The aggregated check results + summary.
        report: DoctorReportDto,
    },

    /// One task row (`/tasks`). Maps `TaskRecord`.
    TaskRow {
        /// The task row.
        task: TaskRowDto,
    },

    /// A chunk of a task's accumulated stdout/stderr spool. Maps
    /// `TaskOutputChunk`.
    TaskOutputChunk {
        /// 9-char task id.
        task_id: String,
        /// Spooled content for this chunk.
        content: String,
        /// Total line count of the spool.
        total_lines: u64,
        /// `true` when the surfaced content was truncated by a limit.
        truncated: bool,
    },

    /// A push event when a task transitions state.
    TaskStatusChanged {
        /// 9-char task id.
        task_id: String,
        /// The new task status.
        status: TaskStatusDto,
    },

    // ── Live thinking/usage (§0.7 follow-up) + reserved (§0.9) ────────────

    /// Coordinator/team status. **LIVE-FED** (§0.9 coordinator-activation):
    /// a coordinator-mode desktop session constructs one `TeamRegistry` per
    /// `build()` and a `CoordinatorStatusSink` pushes the current
    /// `active_worker_count` on each worker status transition via
    /// `OutputStream::emit_coordinator_status` → `AdapterOutputStream`. The
    /// reserved→live flip is a feed-status change only: the DTO is
    /// byte-identical, so no `CLIENT_PROTOCOL_VERSION` bump. Default
    /// (non-coordinator) sessions never source it, so `active_workers` stays `0`.
    CoordinatorStatus {
        /// Number of active (non-terminal) workers in the coordinator's team.
        active_workers: u32,
        /// Optional team name. Skipped from the wire when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        team: Option<String>,
    },

    /// One per-worker roster row (T18). The PULL analog of
    /// [`Self::CoordinatorStatus`]'s scalar: the bridge `spawn_coordinator_poll`
    /// (T19) emits ONE of these per worker on a
    /// [`ListingKindDto::Coordinator`](crate::commands::ListingKindDto::Coordinator)
    /// refresh, exactly mirroring [`Self::TaskRow`] / [`TaskRowDto`]. Lowers 1:1
    /// onto the TUI `WorkerRow`.
    CoordinatorWorker {
        /// The roster row payload.
        worker: CoordinatorWorkerDto,
    },

    /// Streaming thinking delta. **LIVE-FED** (§0.7 follow-up): `event_router`
    /// emits one per `ContentDelta::ThinkingDelta` SSE chunk via
    /// `OutputStream::emit_thinking` → `AdapterOutputStream`. The `signature`
    /// arrives on the completed block, not per-delta, so it is `None` on the
    /// live stream.
    ThinkingDelta {
        /// The reasoning text delta.
        thinking: String,
        /// Optional cryptographic signature attesting to the trace.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// Incremental token-usage update. **LIVE-FED** (§0.7 follow-up):
    /// `event_router` emits it from the `MessageStart` / `MessageDelta` usage
    /// fields via `OutputStream::emit_usage` → `AdapterOutputStream`.
    UsageUpdate {
        /// Input tokens in the latest API call.
        input_tokens: u64,
        /// Output tokens in the latest API call.
        output_tokens: u64,
        /// Cache-read tokens in the latest API call.
        cache_read_tokens: u64,
        /// Cache-creation tokens in the latest API call.
        cache_creation_tokens: u64,
    },
}

/// Coarse error class carried by [`ClientEvent::Error`]. Internally tagged on
/// `type`, `snake_case`. `#[non_exhaustive]` so a future kind is additive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorKindDto {
    /// Transport-level failure (e.g. an `ApiError` while streaming).
    Transport,
    /// Protocol violation (e.g. malformed frame or stream-protocol error).
    Protocol,
    /// Server-reported error mid-stream.
    Server,
    /// The orchestrator's `max_turns` budget was reached.
    MaxTurns,
    /// Any other internal failure.
    Internal,
}

/// How a turn ended — the lowered analog of `traits::TurnOutcome`. Internally
/// tagged on `type`, `snake_case`. `#[non_exhaustive]` so a future outcome is
/// additive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TurnOutcomeDto {
    /// Model returned a natural stop reason and the turn loop ended.
    EndTurn,
    /// The `max_turns` budget was reached before `end_turn`.
    MaxTurns,
    /// The cancel token fired mid-turn; the orchestrator returned early.
    Cancelled,
}

/// Cumulative cost snapshot — the lowered analog of `traits::CostSnapshot`
/// (`Duration` → whole seconds; only the display-relevant fields, decision
/// §0.4). Carried by [`ClientEvent::TurnEnded`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct CostDto {
    /// Cumulative cost in USD.
    pub total_usd: f64,
    /// Cumulative input tokens across all turns.
    pub input_tokens: u64,
    /// Cumulative output tokens across all turns.
    pub output_tokens: u64,
    /// Cumulative successful API calls.
    pub api_calls: u32,
    /// Elapsed session time in whole seconds.
    pub session_duration_secs: u64,
    /// Pre-formatted display string (e.g. `"$0.0123"`).
    pub formatted: String,
}
