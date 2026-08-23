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

use crate::ask_user_question::AskUserQuestionRequestDto;
use crate::controls::ConversationControlsDto;
use crate::listings::{
    AgentDto, AuthStateDto, CoordinatorWorkerDto, DoctorReportDto, HookDto, McpServerDto,
    MemoryEntryDto, ModelDetailsDto, SessionAgentSummaryDto, SessionRowDto, SlashCommandDto,
    StatusSnapshotDto, TaskRowDto, TaskStatusDto,
};
use crate::local_apps::{
    AppCheckpointDto, AppErrorCodeDto, AppEventDto, AppRecordDto, AppRuntimeDetailsDto,
    AppRuntimeStateDto, AppSessionRowDto, AppWorkflowStateDto,
};
use crate::message::MessageDto;
use crate::permission::PermissionResolutionDto;
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
// Boxing the app payload would change the generated Swift/Kotlin protocol API.
#[allow(clippy::large_enum_variant)]
// UniFFI 0.28 stores an enum's variant/field documentation in the same
// fixed-size metadata buffer as its wire schema. Keep this high-cardinality
// envelope undocumented at the derive site; the payload DTOs and protocol
// snapshots remain the source of API documentation without risking a build
// failure when an additive event is introduced.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClientEvent {
    Error {
        kind: ErrorKindDto,
        message: String,
    },

    SystemNotice {
        message: String,
        is_error: bool,
    },

    AskUserQuestion {
        request: AskUserQuestionRequestDto,
    },

    AskUserQuestionResolved {
        request_id: u64,
    },

    PermissionRequestResolved {
        request_id: u64,
        resolution: PermissionResolutionDto,
    },

    // ── Live-turn streaming events (F1-03) ────────────────────────────────
    TextDelta {
        text: String,
    },

    ToolUseStarted {
        id: String,
        tool: String,
        input_json: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        header: Option<crate::tool_display::ToolHeaderDto>,
    },

    ToolHeartbeat {
        id: String,
        tool: String,
        elapsed_ms: u64,
    },

    ToolUseResult {
        id: String,
        tool: String,
        result_json: String,
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<crate::tool_display::ToolResultDisplayDto>,
    },

    MessageComplete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<MessageDto>,
    },

    TurnStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<u64>,
    },

    TurnEnded {
        outcome: TurnOutcomeDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
        cost: CostDto,
    },

    CostUpdate {
        total_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
        api_calls: u32,
        session_duration_secs: u64,
        formatted: String,
    },

    CompactionCompleted {
        messages_before: u32,
        messages_after: u32,
        bytes_saved: u64,
    },

    // ── Listing / screen events (F1-05) ───────────────────────────────────
    //
    // The pull/reply payloads for every client screen. The supporting row /
    // payload structs live in `crate::listings`. Name reconciliation (plan
    // line 149): the design spec §4.1 says `AgentList`, but the WIRE name is
    // `Agents`.
    SessionStarted {
        session_id: String,
    },

    SessionEnded,

    SessionResumed {
        session_id: String,
        messages: Vec<MessageDto>,
    },

    SessionAgentList {
        session_id: String,
        agents: Vec<SessionAgentSummaryDto>,
    },

    SessionAgentTranscript {
        session_id: String,
        agent_id: String,
        messages: Vec<MessageDto>,
        next_message_index: u64,
        revision: u64,
    },

    SessionAgentUpdated {
        session_id: String,
        agent: SessionAgentSummaryDto,
    },

    SessionAgentMessage {
        session_id: String,
        agent_id: String,
        message_index: u64,
        message: MessageDto,
    },

    SessionList {
        sessions: Vec<SessionRowDto>,
    },

    ModelList {
        models: Vec<String>,
        current: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        details: Vec<ModelDetailsDto>,
    },

    ModelChanged {
        model: String,
    },

    PermissionModeChanged {
        mode: String,
    },

    ProviderCredentialStatus {
        operation_id: u64,
        configured_provider_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unavailable_provider_ids: Vec<String>,
        storage_encrypted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    McpServers {
        servers: Vec<McpServerDto>,
    },

    Hooks {
        hooks: Vec<HookDto>,
    },

    Agents {
        agents: Vec<AgentDto>,
    },

    SlashCommandCatalog {
        commands: Vec<SlashCommandDto>,
    },

    SlashCommandResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<u64>,
        display: String,
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
    },

    MemoryEntries {
        entries: Vec<MemoryEntryDto>,
    },

    StatusSnapshot {
        snapshot: StatusSnapshotDto,
    },

    SettingsSnapshot {
        effective_json: String,
        provenance_json: String,
    },

    AuthState {
        state: AuthStateDto,
    },

    DoctorReport {
        report: DoctorReportDto,
    },

    TaskRow {
        task: TaskRowDto,
    },

    TaskOutputChunk {
        task_id: String,
        content: String,
        total_lines: u64,
        truncated: bool,
    },

    TaskStatusChanged {
        task_id: String,
        status: TaskStatusDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_session_id: Option<String>,
    },

    CommandsChanged {
        commands: Vec<SlashCommandDto>,
    },

    // ── Local apps ────────────────────────────────────────────────────────
    //
    // Docstrings in this section are deliberately terse: uniffi bakes every
    // docstring into a fixed-capacity per-item metadata buffer, and the
    // `ClientEvent` enum is near that cap. Full semantics live on the
    // `crate::local_apps` DTOs and the matching `ClientCommand` variants.
    AppsChanged {
        apps: Vec<AppRecordDto>,
    },

    AppEvent {
        event: AppEventDto,
    },

    AppWorkflowChanged {
        app_id: String,
        state: AppWorkflowStateDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },

    AppRuntimeChanged {
        app_id: String,
        state: AppRuntimeStateDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<AppRuntimeDetailsDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_error: Option<String>,
    },

    AppSessionsChanged {
        app_id: String,
        sessions: Vec<AppSessionRowDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_offset: Option<u64>,
    },

    AppCheckpointCreated {
        app_id: String,
        checkpoint: AppCheckpointDto,
    },

    AppOperationFailed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        code: AppErrorCodeDto,
        message: String,
    },

    // ── Live thinking/usage (§0.7 follow-up) + reserved (§0.9) ────────────
    CoordinatorStatus {
        active_workers: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        team: Option<String>,
    },

    CoordinatorWorker {
        worker: CoordinatorWorkerDto,
    },

    ThinkingDelta {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    UsageUpdate {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    },

    Attachment {
        attachment: AttachmentDto,
    },

    ApiRetry {
        message: String,
        attempt: u32,
        max_retries: u32,
        delay_ms: u64,
    },

    // ⚠️ APPEND NEW VARIANTS BELOW THIS LINE, NEVER ABOVE IT.
    // UniFFI lowers this enum by 1-based ORDINAL, so inserting a variant
    // anywhere but the end silently shifts every later one and both mobile
    // clients mis-decode every event — with no compile error anywhere.
    PlanUpdated {
        tasks: Vec<crate::tool_display::PlanTaskDto>,
    },

    /// A paused workflow was replaced by a newly launched resumed run.
    WorkflowResumed {
        previous_task_id: String,
        task: TaskRowDto,
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_session_id: Option<String>,
    },

    ConversationControlsChanged {
        controls: ConversationControlsDto,
    },
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// A user-visible attachment. Internally tagged on `type`, `snake_case`, and
/// `#[non_exhaustive]` so the remaining oracle kinds are additive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AttachmentDto {
    /// A nested LINGXI.md surfaced because a file under its directory was read.
    NestedMemory {
        /// Path shown to the user, relative to cwd.
        display_path: String,
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
    /// A valid command was rejected by the active policy or runtime state.
    Rejected,
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
