//! `ClientEvent` DTOs — outbound events the engine streams to a client.
//!
//! The `Error` variant + `ErrorKindDto` land in F1-01 (the first variant, so
//! every later DTO inherits the frozen serde conventions). The live-turn
//! streaming variants land in F1-03.
//!
//! Frozen serde conventions (decision §0.1):
//! - internally tagged: `#[serde(tag = "type", rename_all = "snake_case")]`
//!   (matches `protocol::ContentBlock` / api-client `StreamEvent`),
//! - top-level enum is `#[non_exhaustive]` (mirrors `platform_api::OutputEvent`),
//! - every optional field uses
//!   `#[serde(default, skip_serializing_if = "Option::is_none")]`.
//!
//! Tool payloads are JSON **Strings** (`input_json`/`result_json`); the
//! `serde_json::Value` lowering happens in `client-adapter`, NOT here
//! (decision §0.4).

use crate::ask_user_question::AskUserQuestionRequestDto;
use crate::controls::ConversationControlsDto;
use crate::listings::{
    AgentDto, AuthStateDto, ConfigurationDomainDto, ConfigurationEffectDto,
    ConfigurationOperationStatusDto, CoordinatorWorkerDto, DoctorReportDto, HookDto, McpServerDto,
    MemoryEntryDto, ModelDetailsDto, ProviderModelCatalogEntryDto, SessionAgentSummaryDto,
    SessionModeDto, SessionRowDto, SkillDto, SlashCommandDto, StatusSnapshotDto, TaskRowDto,
    TaskStatusDto,
};
use crate::local_apps::{
    AppCheckpointDto, AppErrorCodeDto, AppEventDto, AppRecordDto, AppRuntimeDetailsDto,
    AppRuntimeStateDto, AppSessionRowDto, AppWorkflowStateDto,
};
use crate::message::MessageDto;
use crate::permission::PermissionResolutionDto;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
        #[serde(default)]
        summary: String,
    },

    // ── Listing / screen events (F1-05) ───────────────────────────────────
    //
    // The pull/reply payloads for every client screen. The supporting row /
    // payload structs live in `crate::listings`. Name reconciliation (plan
    // line 149): the design spec §4.1 says `AgentList`, but the WIRE name is
    // `Agents`.
    SessionStarted {
        session_id: String,
        mode: SessionModeDto,
    },

    SessionEnded,

    SessionResumed {
        session_id: String,
        mode: SessionModeDto,
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
        /// Display-safe credential previews keyed by provider id. Values are
        /// fixed masks plus at most the final four characters; plaintext
        /// credentials never cross the client protocol.
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        credential_previews: HashMap<String, String>,
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

    /// The layered settings read path (`RefreshListings{Settings}`).
    ///
    /// Every structured payload here is a JSON **String**, not a nested
    /// object: `serde_json::Value` must never enter this crate (decision
    /// §0.4 — `Value` is not UniFFI-representable), so the bridge lowers
    /// each map to a string exactly the way `ToolUseStarted.input_json`
    /// does.
    ///
    /// `effective_json` is `{key: value}` after the merge; `provenance_json`
    /// is `{key: layer}` naming which layer each merged value came from.
    /// The three optional fields were ADDED to this variant (additive under
    /// decision §0.10 — no major bump): a client that predates them keeps
    /// reading the two required payloads unchanged.
    SettingsSnapshot {
        /// `{key: value}` — the merged effective settings.
        effective_json: String,
        /// `{key: layer}` — which layer each effective value came from.
        provenance_json: String,
        /// `[{layer, path, exists, parsed, parse_error?}]` — the on-disk
        /// state of every settings file layer, so the UI can show which file
        /// backs a layer and whether it parsed. `parsed` reports JSON
        /// validity only; it says nothing about OS write permission.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        files_json: Option<String>,
        /// `{key: value}` — the FILE-LAYER values as read at session start.
        /// NOT the session's live configuration: no `cli` / `managed` / `env`
        /// overlay is applied, so this is strictly "what the settings files
        /// said at boot" and can differ from `effective_json` both because of
        /// an on-disk edit not yet picked up AND because `effective_json`
        /// carries the managed overlay that this field does not.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_json: Option<String>,
        /// Keys an administrator pinned through the managed-settings layer;
        /// the UI must not offer to edit these.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locked: Option<Vec<String>>,
        /// `{layer: {key: value}}` — each FILE layer's OWN raw settings map,
        /// unmerged. `effective_json` is a cross-layer merge and `active_json`
        /// is the file-layer merge without the managed overlay; NEITHER can
        /// stand in for "what does layer L's file itself say", which a
        /// layered editor needs before it writes back to one layer: the
        /// generic `update_settings` command replaces a key WHOLESALE in one
        /// layer's file (`migrations/src/settings_update.rs`'s "top-level
        /// REPLACE, not deep-merge" contract), so pre-merging a write against
        /// the cross-layer `effective_json` view — which can carry another
        /// layer's entries for an object-valued key like `providers` — would
        /// silently fork that other layer's data into the one being saved.
        /// Additive under decision §0.10 — no major bump.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layers_json: Option<String>,
        /// The keys in `effective_json` whose value is a CROSS-LAYER union
        /// rather than one layer's value. The engine deep-merges or
        /// concat-dedups a specific set of keys (`hooks`, `permissions`,
        /// `providers`, `enabledPlugins`, `trustedDirectories`, … — its
        /// `settings::schema::MERGE_STRATEGIES` table), so when more than one
        /// layer contributes, the effective value belongs to no single layer
        /// and `provenance_json`'s entry for that key names only the
        /// highest-priority CONTRIBUTOR. A client must therefore not render a
        /// single-layer provenance badge for a key listed here; it says the
        /// value is merged across layers instead.
        ///
        /// Only keys the merge actually unioned are listed: a deep-merge key
        /// whose entries the winning layer entirely redefines is absent,
        /// because for that key the winning layer's badge is honest. A list
        /// of every key both layers mention would be useless.
        ///
        /// Additive under decision §0.10 — no major bump.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        merged_keys: Option<Vec<String>>,
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
        /// Failure reason accompanying a `failed` transition, when the
        /// producing handler reported one. APPENDED field (additive default
        /// `None`) — clients render it instead of a bare task id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
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
        /// Correlation key from the `CreateApp` (or other app command) that
        /// failed, echoed verbatim so the caller that started the operation
        /// can recognise its own failure. `None` for a failure the engine
        /// synthesized with no originating request.
        ///
        /// Appended LAST: UniFFI encodes struct variants POSITIONALLY, so a
        /// field inserted above `message` would be reinterpreted by a client
        /// built against the previous bindings.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
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

    FastModeChanged {
        enabled: bool,
    },

    /// Ask a client to perform one microphone/speaker operation. Mirrors the
    /// [`ComputerAccessRequestDto`](crate::computer_access::ComputerAccessRequestDto)
    /// engine->client request/response shape: correlated by `request_id`, and
    /// the client's outcome round-trips back as an
    /// [`AudioResultDto`](crate::commands::AudioResultDto) on
    /// [`ClientCommand::AudioResponse`](crate::commands::ClientCommand::AudioResponse).
    AudioRequest {
        /// Connection-scoped correlator; echoed back verbatim on the matching
        /// `AudioResponse`.
        request_id: u64,
        /// The operation the client should perform.
        op: AudioOpDto,
    },
    /// Authoritative durable state for one mobile turn. Emitted on attach,
    /// resume, recovery gating, and every terminal transition.
    TurnRecoveryState {
        snapshot: TurnRecoverySnapshotDto,
    },

    /// Sequenced retained copy of a turn event. It is emitted beside live
    /// delivery and replayed after
    /// [`ClientCommand::AttachTurn`](crate::commands::ClientCommand::AttachTurn).
    /// `event_json` is the original serialized `ClientEvent`; keeping it a
    /// string avoids a recursive UniFFI enum while preserving the exact wire
    /// payload for future SSE/WebSocket transports.
    TurnEventReplay {
        session_id: String,
        turn_id: u64,
        sequence: u64,
        event_json: String,
    },

    Skills {
        skills: Vec<SkillDto>,
    },

    /// Authoritative global TypeScript LSP policy. `effective` is `off` when
    /// the pinned runtime is unavailable even if the persisted request is
    /// `auto` or `on`.
    TypescriptLspModeChanged {
        requested: String,
        effective: String,
        available: bool,
    },

    /// A context-preserving copy created in another session capability mode.
    /// Kept at the end so existing UniFFI event ordinals remain stable.
    SessionForked {
        source_session_id: String,
        session_id: String,
        mode: SessionModeDto,
    },

    ProviderConnectionTested {
        operation_id: u64,
        provider_id: String,
        connected: bool,
        reachable: bool,
        authenticated: bool,
        model_available: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http_status: Option<u16>,
        latency_ms: u64,
        message: String,
        used_stored_credential: bool,
    },

    ConfigurationOperation {
        domain: ConfigurationDomainDto,
        operation_id: u64,
        status: ConfigurationOperationStatusDto,
        effect: ConfigurationEffectDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details_json: Option<String>,
    },

    SkillCatalog {
        catalog_json: String,
    },

    SkillDocument {
        document_json: String,
    },

    McpConfigurationSnapshot {
        snapshot_json: String,
    },

    PluginCatalog {
        catalog_json: String,
    },

    ProviderModelCatalog {
        providers: Vec<ProviderModelCatalogEntryDto>,
    },

    /// Observed compaction lifecycle, including attempts that fail or are cancelled.
    /// Connection-scoped so an idle manual `/compact` can publish live progress.
    /// Appended to preserve every existing UniFFI enum ordinal.
    CompactionStatus {
        /// `preparing`, `summarizing`, `restoring`, `complete`, `skipped`, `error`, or `cancelled`.
        /// A string allows clients to ignore future phases without decode failures.
        phase: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Authoritative scheduled task snapshot after a management operation.
    CronResult {
        request_id: String,
        jobs: Vec<CronJobDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
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

/// How a turn ended — the lowered analog of `platform_api::TurnOutcome`. Internally
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

/// Durable execution state for a mobile turn. Backgrounding itself never
/// changes this state; only execution, a recovery gate, or an explicit cancel
/// does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TurnRecoveryStateDto {
    /// The turn is currently executing or attached to a live executor.
    Running,
    /// Execution is parked until the user supplies permission or input.
    WaitingForUser,
    /// The platform lease expired, but the checkpoint may be resumed safely.
    PausedRecoverable,
    /// The turn produced its normal final outcome.
    Completed,
    /// The turn ended with an execution failure.
    Failed,
    /// The user explicitly cancelled the turn; it must never be resumed.
    Cancelled,
}

impl TurnRecoveryStateDto {
    /// Terminal durable states must never be resurrected by attach/resume.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Snapshot clients use to decide whether a durable turn can be reattached or
/// needs explicit user intervention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct TurnRecoverySnapshotDto {
    /// Stable session owning the turn.
    pub session_id: String,
    /// Stable client-provided turn identity.
    pub turn_id: u64,
    /// Current durable lifecycle state.
    pub state: TurnRecoveryStateDto,
    /// First event sequence still retained for replay.
    pub first_sequence: u64,
    /// Last committed event sequence, or zero when no events were committed.
    pub last_sequence: u64,
    /// Whether replaying from the persisted boundary is known to be safe.
    pub safe_to_resume: bool,
    /// Optional machine-readable explanation for a paused or terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Cumulative cost snapshot — the lowered analog of `platform_api::CostSnapshot`
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

/// One audio operation the engine asks a client to perform on its device
/// microphone/speaker. Carried by [`ClientEvent::AudioRequest`]. Internally
/// tagged on `type`, `snake_case`. `#[non_exhaustive]` so a future op is
/// additive.
///
/// Mirrors the argument shapes of `platform_api::{VoiceRecorder, SpeechToText,
/// TextToSpeech}` one-to-one, so this contract can carry every call those
/// traits make without loss: `StartRecording`/`StopRecording`/`IsRecording`
/// lower `VoiceRecorder::{start_recording, stop_recording, is_recording}`
/// (`StartRecording`'s fields are `platform_api::VoiceRecordingOpts`); `Transcribe`
/// lowers `SpeechToText::transcribe`'s `SttOpts`; `Synthesize` lowers
/// `TextToSpeech::synthesize`'s `TtsOpts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AudioOpDto {
    /// Begin a recording session — `VoiceRecordingOpts`.
    StartRecording {
        /// Target sample rate in Hz (e.g. `16_000` for speech).
        sample_rate_hz: u32,
        /// Container/codec hint (e.g. `"m4a"`, `"wav"`).
        format: String,
    },
    /// Stop the active recording session and return the captured audio.
    /// Answered by
    /// [`AudioResultDto::Recording`](crate::commands::AudioResultDto::Recording).
    StopRecording,
    /// Query whether a recording session is currently active. Answered by
    /// [`AudioResultDto::RecordingState`](crate::commands::AudioResultDto::RecordingState).
    /// `VoiceRecorder::is_recording` returns a bare `bool` with no error
    /// channel, so this op has no engine-defined `Failed` outcome to carry —
    /// only a transport-level failure could prevent an answer, and handling
    /// that is a proxy (Task 2) concern, not part of this contract.
    IsRecording,
    /// Open the microphone, listen for a single utterance, and return the
    /// final transcript — `SttOpts`.
    Transcribe {
        /// BCP-47 language hint (e.g. `"en-US"`, `"zh-CN"`). `None` = device
        /// default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
    },
    /// Synthesize text to speech — `TtsOpts`.
    Synthesize {
        /// Text to speak.
        text: String,
        /// Provider-specific voice id (`None` = the system default voice).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voice: Option<String>,
    },
}

/// A durable task from the workspace's existing cron scheduler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct CronJobDto {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub durable: bool,
    pub permanent: bool,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}
