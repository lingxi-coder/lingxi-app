//! Local-apps DTOs — the wire shapes for the on-device "Apps" capability
//! (local-apps phase 1: agent-designed Next.js mini-apps).
//!
//! Protocol-local mirrors of the `local-apps` core types (`AppRecord`,
//! `AppCheckpoint`, `AppErrorCode`, …) — the core crate never appears here;
//! the engine lowers core ⇄ DTO at the dispatch boundary, exactly like the
//! other listing/row DTOs.
//!
//! Serde conventions (decision §0.1) with one DELIBERATE departure, following
//! the [`crate::computer_access::AccessTierDto`] precedent of staying
//! byte-identical to the source contract: the fieldless enums
//! ([`AppWorkflowStateDto`], [`AppRuntimeStateDto`], [`AppCreateOriginDto`],
//! [`AppSurfaceDto`], [`AppErrorCodeDto`], [`AppCheckpointKindDto`]) ride as
//! bare wire STRINGS
//! (`"draft"`, `"stopped"`, `"not_found"`, …) — a plain
//! `#[serde(rename_all = "snake_case")]` fieldless enum, byte-identical to
//! the core enums' canonical `as_str()` values.
//!
//! Every optional field uses
//! `#[serde(default, skip_serializing_if = "Option::is_none")]`;
//! `serde_json::Value` never enters this crate (decision §0.4).

// Most members below are wire records whose meaning is defined by their
// enclosing DTO and frozen snapshot. Keeping field comments out of UniFFI
// metadata also avoids its fixed per-item metadata buffer.
#![allow(missing_docs)]

use serde::{Deserialize, Serialize};

fn default_git_version_control() -> bool {
    true
}

/// Workflow state of an app — mirrors the core `AppWorkflowState`. A bare
/// wire STRING (`"draft"` / `"ready"`). `#[non_exhaustive]` so a future state
/// is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppWorkflowStateDto {
    /// The app exists but has not been built/approved yet.
    Draft,
    /// The app is built and usable.
    Ready,
}

/// Runtime (dev-server) state of an app — mirrors the core `AppRuntimeState`
/// (spec §C). A bare wire STRING (`"stopped"`, …). `#[non_exhaustive]` so a
/// future state is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppRuntimeStateDto {
    /// No runtime process.
    Stopped,
    /// Runtime is starting up.
    Starting,
    /// Runtime is serving.
    Running,
    /// Runtime is shutting down.
    Stopping,
    /// Runtime failed; `last_error` explains why.
    Failed,
}

/// Where a [`CreateApp`](crate::commands::ClientCommand::CreateApp) originated.
/// A bare wire STRING (`"chat"` / `"library"`). `#[non_exhaustive]` so a future
/// origin is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppCreateOriginDto {
    /// Created from an agent conversation (carries a `conversation_id`).
    Chat,
    /// Created from the apps library screen.
    Library,
}

/// Which scaffold a [`CreateApp`](crate::commands::ClientCommand::CreateApp)
/// lays down — mirrors the core `AppSurface`. A bare wire STRING
/// (`"dom"` / `"canvas"`). `#[non_exhaustive]` so a future surface is additive.
///
/// Deliberately NOT called a template: the fixed template catalog was removed
/// from the protocol on purpose and a regression guard keeps those symbols out.
/// This names the SHAPE the app draws, which the create sheet must show and let
/// the user correct — a surface is immutable once scaffolded, so it cannot be
/// left to a value the user never saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppSurfaceDto {
    /// A routed, multi-screen interface built from Ionic components.
    Dom,
    /// A single drawn surface owning its own frame loop, rendering to `<canvas>`.
    Canvas,
}

/// Stable machine-readable failure code carried on
/// [`AppOperationFailed`](crate::events::ClientEvent::AppOperationFailed) —
/// mirrors the core `AppErrorCode`. A bare wire STRING (`"not_found"`, …).
/// `#[non_exhaustive]`: the code set is extensible in later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppErrorCodeDto {
    /// The addressed app (or sub-resource) does not exist.
    NotFound,
    /// A caller-supplied revision does not match the current draft revision.
    RevisionConflict,
    /// A caller-supplied interaction/suggestion id is not the pending one.
    InteractionInvalid,
    /// The operation is not legal in the app's current workflow state.
    WorkflowStateInvalid,
    /// The app runtime is in a state that blocks the operation.
    RuntimeBusy,
    /// The capability is gated behind a later phase (runtime = phase 4,
    /// git checkpoints = phase 5).
    NotYetAvailable,
    /// Persisted state failed to parse or violates invariants.
    StorageCorrupt,
    /// The request itself is malformed (bad id, empty name, …).
    InvalidRequest,
    /// Underlying I/O failure.
    Io,
    /// The model is unreachable: offline, unauthenticated, or timed out.
    LlmUnavailable,
    /// The model's output failed validation (bad shape or over a limit).
    LlmOutputRejected,
}

/// Whether a catalog row is the app's pinned init session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
pub enum AppSessionKindDto {
    /// The pinned set-up conversation (`AppRecordDto::init_session_id`).
    Init,
    /// Any other conversation in the app's workspace catalog.
    Conversation,
}

/// One row of an app's workspace-scoped session catalog. Field-for-field the
/// shared [`SessionRowDto`](crate::listings::SessionRowDto) shape plus the
/// init marker — no file paths cross the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "snake_case")]
pub struct AppSessionRowDto {
    /// Bare session uuid (the resume key).
    pub uuid: String,
    /// Display title (custom > ai > summary > first-prompt derivation).
    pub title: String,
    /// Last-modified time, RFC 3339 seconds.
    pub modified_rfc3339: String,
    /// Transcript line count (`usize` lowered to `u32`, matching
    /// [`SessionRowDto`](crate::listings::SessionRowDto)).
    pub message_count: u32,
    /// Init marker for the pinned first row.
    pub kind: AppSessionKindDto,
}

/// Why a checkpoint was recorded — mirrors the core `AppCheckpointKind` (git
/// wiring is phase 5). A bare wire STRING (`"scaffold_created"`, …).
/// `#[non_exhaustive]` so a future kind is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppCheckpointKindDto {
    /// Initial scaffold committed.
    ScaffoldCreated,
    /// Generation output passed validation.
    GenerationValidated,
    /// User approved the preview.
    PreviewApproved,
    /// Explicit user-requested checkpoint.
    UserApproved,
    /// Automatic safety checkpoint taken before a restore.
    PreRestore,
}

/// Supported field types in an app-owned structured data collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppDataFieldTypeDto {
    Text,
    LongText,
    Integer,
    Decimal,
    Boolean,
    DateTime,
    Enum,
    ImageRef,
}

/// One field in a structured app data collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppDataFieldDto {
    pub id: String,
    pub label: String,
    pub field_type: AppDataFieldTypeDto,
    pub required: bool,
    /// Choices for an `enum` field; empty for every other type.
    pub options: Vec<String>,
}

/// A collection declared by the design and exposed through the native data API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppDataCollectionDto {
    pub id: String,
    pub label: String,
    pub fields: Vec<AppDataFieldDto>,
    /// Whether the plan enables this collection by default.
    pub enabled_by_default: bool,
}

/// One app row — the lowered core `AppRecord`. Carried either by the full
/// [`AppsChanged`](crate::events::ClientEvent::AppsChanged) snapshot or by
/// the incremental `AppRecordChanged` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppRecordDto {
    /// Stable app id matching `^[a-z0-9][a-z0-9-]{0,63}$`.
    pub id: String,
    /// User-facing display name.
    pub name: String,
    /// One-line description the user gave at creation time.
    pub brief: String,
    /// Whether Git controls this app's source checkpoints and restores.
    #[serde(default = "default_git_version_control")]
    pub git_enabled: bool,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: u64,
    /// Last mutation time, epoch milliseconds.
    pub updated_at_ms: u64,
    /// Current workflow state (`draft` / `ready`).
    pub workflow_state: AppWorkflowStateDto,
    /// Conversation the app was created from (`origin: chat`), if any. Skipped
    /// from the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// The app's pinned "init" session (bare uuid) — listed first in its
    /// session catalog. Skipped from the wire when `None` (pre-v3 records
    /// before the boot backfill runs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_session_id: Option<String>,
    /// Workspace directory relative to the engine data root (always
    /// `apps/<id>/workspace`, forward slashes).
    pub workspace_rel: String,
}

/// One restorable checkpoint of an app workspace — the lowered core
/// `AppCheckpoint` (phase 5 wires git). Carried by
/// [`AppCheckpointCreated`](crate::events::ClientEvent::AppCheckpointCreated).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppCheckpointDto {
    /// Stable checkpoint id.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Why the checkpoint was recorded.
    pub kind: AppCheckpointKindDto,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: u64,
}

/// How an approved app build is served on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppRuntimeModeDto {
    StaticExport,
    NextProduction,
}

/// Why a runtime was stopped or suspended outside an explicit user stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppRuntimeSuspensionReasonDto {
    Backgrounded,
    MemoryWarning,
    RuntimeQuota,
    ProcessExited,
}

/// Foreground recovery state for a suspended local app runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppRuntimeRecoveryStateDto {
    NotNeeded,
    Pending,
    Recovering,
    Recovered,
    Failed,
}

/// Generated application manifest consumed by the host and app bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppManifestDto {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_api_version: Option<u16>,
    pub app_id: String,
    pub name: String,
    pub design_revision: u64,
    pub collections: Vec<AppDataCollectionDto>,
    pub allowed_domains: Vec<String>,
    /// Capabilities the confirmed plan declared. Absent on the wire (and
    /// empty) for pre-capability manifests.
    #[serde(default)]
    pub capabilities: Vec<AppCapabilityKindDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_context: Option<DeviceContextDto>,
}

/// Native host target the app was generated for.
///
/// Host-derived, not model-authored, and deliberately just the stable pair:
/// viewport, safe area, color scheme, reduced motion and input mode are live
/// values the page reads from `window.lingxi.v2.deviceContext`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct DeviceContextDto {
    pub os: String,
    pub form_factor: String,
}

/// Runtime snapshot included in an app detail response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppRuntimeDetailsDto {
    pub state: AppRuntimeStateDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AppRuntimeModeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loopback_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspension_reason: Option<AppRuntimeSuspensionReasonDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_state: Option<AppRuntimeRecoveryStateDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Full application detail snapshot requested by the library/detail screens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppDetailsDto {
    pub app: AppRecordDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<AppManifestDto>,
    pub runtime: AppRuntimeDetailsDto,
    pub checkpoints: Vec<AppCheckpointDto>,
}

/// Operations accepted by the versioned `window.lingxi.v2` bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppBridgeOperationDto {
    QueryData,
    MutateData,
    NetworkRequest,
    RuntimeStatus,
    /// Capture one photo with the device camera (`device.capturePhoto`).
    CapturePhoto,
    /// Pick one image from the photo library (`device.pickImage`).
    PickImage,
    /// Start a microphone recording (`device.recordAudioStart`).
    RecordAudioStart,
    /// Stop the recording and return its bytes (`device.recordAudioStop`).
    RecordAudioStop,
    /// One-shot current location (`device.getLocation`).
    GetLocation,
    /// Listen once and return the transcript (`device.transcribeSpeech`).
    /// This is how speech reaches the model: no provider on this stack
    /// accepts raw audio in a messages call.
    TranscribeSpeech,
    /// Post a local notification (`device.postNotification`).
    PostNotification,
    /// Read plain text from the system clipboard (`clipboard.getText`).
    ClipboardGetText,
    /// Write plain text to the system clipboard (`clipboard.setText`).
    ClipboardSetText,
    /// Open the native share sheet (`device.share`).
    Share,
    /// Synthesize bounded text to audio (`device.synthesizeSpeech`).
    SynthesizeSpeech,
    /// Read an app-private file (`files.read`).
    FileRead,
    /// Write an app-private file (`files.write`).
    FileWrite,
    /// Read non-sensitive device status (`device.status`).
    DeviceStatus,
    /// Trigger one bounded haptic event (`device.haptics`).
    Haptics,
    /// Open one authorized external URL (`device.deepLink`).
    DeepLink,
    /// One side-query chat completion against the user's model (`llm.chat`).
    LlmChat,
    /// Stream one side-query chat completion through `AppBridgeStreamFrameDto`.
    LlmStream,
    /// Post one event into the app's conversation mailbox (`agent.post`).
    AgentPost,
    /// Create a persistent app-owned Agent session.
    AgentSessionCreate,
    /// List persistent app-owned Agent sessions.
    AgentSessionList,
    /// Resume one persistent app-owned Agent session.
    AgentSessionResume,
    /// Close one persistent app-owned Agent session.
    AgentSessionClose,
    /// Run one non-streaming turn in an app-owned Agent session.
    AgentSend,
    /// Run one streaming turn in an app-owned Agent session.
    AgentStream,
    /// Cancel one in-flight app-owned Agent turn.
    AgentCancel,
    /// Propose a user-approved App Agent Profile revision.
    AgentProfileProposeUpdate,
    /// Register a declarative app flow with the host background scheduler.
    BackgroundSchedule,
    /// List app-owned background task lifecycle records.
    BackgroundList,
    /// Read one app-owned background task lifecycle record.
    BackgroundStatus,
    /// Cancel one app-owned background task.
    BackgroundCancel,
    /// Requeue one failed or cancelled app-owned background task.
    BackgroundRetry,
    /// List bounded calendar events (`calendar.listEvents`).
    CalendarListEvents,
    /// Search bounded contact projections (`contacts.search`).
    ContactsSearch,
    /// Read one media handle retained by this app (`media.get`).
    MediaGet,
}

/// One host-bound bridge request. Payloads are data, never executable script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppBridgeRequestDto {
    pub request_id: String,
    pub app_id: String,
    pub operation: AppBridgeOperationDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_json: Option<String>,
}

/// Result of a bridge request. JSON remains an opaque data string at `UniFFI`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppBridgeResponseDto {
    pub request_id: String,
    pub app_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Stable machine-readable failure code (`capability_not_declared`,
    /// `permission_denied`, `audio_session_busy`, `media_too_large`,
    /// `llm_busy`, …) so page code can branch without parsing `error` prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// Runtime API version advertised by a generated Local App.  The v2 cutover
/// is explicit: a v1 page is not silently interpreted as a v2 page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppRuntimeApiVersionDto {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

/// v2 invocation origin.  The host derives this from the execution path and
/// does not trust a page-supplied value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppInvocationOriginDto {
    PageForeground,
    ConversationAgent,
    AppRuntimeHeadless,
    SystemScheduler,
}

/// Host-created v2 attribution metadata.  `call_chain` is retained on the
/// wire for audit/debugging but is still validated and rebuilt by the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct AppInvocationContextDto {
    pub app_id: String,
    pub app_instance_id: String,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub origin: AppInvocationOriginDto,
    pub grant_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_instance: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_chain: Vec<AppInvocationFrameDto>,
}

/// One nested invocation edge for recursion and audit enforcement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct AppInvocationFrameDto {
    pub app_id: String,
    pub capability: String,
    pub input_hash: String,
}

/// Generic v2 bridge request.  `operation` is a registry id such as
/// `llm.stream` or `agent.sessions.resume`, not a handler name selected by the
/// app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct AppBridgeV2RequestDto {
    pub api_version: AppRuntimeApiVersionDto,
    pub context: AppInvocationContextDto,
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_json: Option<String>,
    #[serde(default)]
    pub stream: bool,
}

/// Generic v2 bridge response.  Streaming responses are delivered as
/// `AppBridgeStreamFrameDto` events with the same request id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct AppBridgeV2ResponseDto {
    pub request_id: String,
    pub app_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
}

/// Ordered v2 stream frame used by LLM and Agent session streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[non_exhaustive]
pub enum AppBridgeStreamFrameDto {
    Started {
        app_id: String,
        request_id: String,
        stream_id: String,
    },
    Data {
        app_id: String,
        request_id: String,
        stream_id: String,
        seq: u64,
        data_json: String,
    },
    Completed {
        app_id: String,
        request_id: String,
        stream_id: String,
        seq: u64,
    },
    Error {
        app_id: String,
        request_id: String,
        stream_id: String,
        seq: u64,
        code: String,
        message: String,
    },
    Cancelled {
        app_id: String,
        request_id: String,
        stream_id: String,
        seq: u64,
        reason: String,
    },
}

/// Persistent app Agent session status on the v2 wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppAgentSessionStatusDto {
    Active,
    Paused,
    Closed,
}

/// Bounded Agent execution budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct AppAgentBudgetDto {
    pub max_tokens: u32,
    pub max_wall_ms: u64,
    pub max_turns: u32,
    pub max_bridge_calls: u32,
    pub max_mcp_calls: u32,
    pub max_recursion_depth: u16,
}

/// Host-owned persistent Agent session row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct AppAgentSessionDto {
    pub session_id: String,
    pub app_id: String,
    pub app_instance_id: String,
    pub status: AppAgentSessionStatusDto,
    pub prompt_profile_revision: u64,
    pub budget: AppAgentBudgetDto,
    pub turn_count: u32,
    #[serde(default)]
    pub output_tokens_used: u64,
    #[serde(default)]
    pub bridge_calls_used: u32,
    #[serde(default)]
    pub mcp_calls_used: u32,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// User-approved prompt profile revision for an app Agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct AppAgentProfileDto {
    pub app_id: String,
    pub revision: u64,
    pub instructions: String,
    pub updated_at_ms: u64,
}

/// A profile proposal is inert until a separate user approval is applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct AppAgentProfileProposalDto {
    pub app_id: String,
    pub approval_token: String,
    pub base_revision: u64,
    pub current_revision: u64,
    pub instructions: String,
    pub reason: String,
}

/// Allow-list of UI operations; arbitrary JavaScript is intentionally absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppUiActionKindDto {
    Inspect,
    Click,
    Fill,
    Select,
    Toggle,
    Scroll,
    Navigate,
    Back,
    Reload,
    /// Capture a still image of the app's own `WebView` and return it as a
    /// base64 image. Read-only like [`Self::Inspect`], but strictly more
    /// revealing: `Inspect` redacts `password`/`hidden` input values and a
    /// pixel capture cannot, so the client prompts for it instead of
    /// auto-approving.
    ///
    /// `CaptureView`, deliberately not `Screenshot`. Two reasons, and the
    /// second is the load-bearing one:
    ///
    /// - It keeps one name across all three layers — builtin
    ///   `LocalAppCaptureUi`, provider operation `capture_ui`, wire
    ///   `capture_view` — instead of the wire layer alone jumping vocabulary.
    /// - "Screenshot" is the device-level word (`computer` / `android_use` /
    ///   `ios_use` all spell it `screenshot`). Those tools cannot appear in the
    ///   mobile tool set, so there is no identifier collision — but the label
    ///   derived from this variant is interpolated into the user's
    ///   authorization sheet, and a prompt saying "screen" for one app's view
    ///   next to a device-level prompt saying "screen" for the whole device is
    ///   an authorization the user cannot correctly reason about.
    ///
    /// Appended LAST — uniffi assigns FFI discriminants by declaration order,
    /// so inserting mid-enum renumbers every later variant for clients that
    /// were built against the old ordering. Kept FIELDLESS for the same
    /// reason a data-carrying variant is avoided elsewhere: uniffi renders a
    /// mixed enum as a Kotlin `sealed class`, which would rename every
    /// existing constant (`CLICK` → `Click`).
    CaptureView,
    /// Dispatch a pointer event at viewport coordinates.
    ///
    /// Distinct from [`Self::Click`], which resolves an ELEMENT and calls
    /// `.click()` on it — a synthetic `MouseEvent` at `(0, 0)`. A canvas app
    /// has no element to resolve and listens for `pointerdown`/`pointermove`/
    /// `pointerup` with real coordinates, so `Click` can neither address nor
    /// reach it.
    ///
    /// Fieldless: the coordinates ride in `AppUiRequestDto::value` as
    /// `"x,y"` or `"x,y,phase"`, the same comma-packed convention
    /// [`Self::Scroll`] already uses. A data-carrying variant would make uniffi
    /// render this enum as a Kotlin `sealed class` and rename every existing
    /// constant (`CLICK` → `Click`) — an Android-only break.
    Pointer,
    /// Dispatch a keyboard event.
    ///
    /// `AppUiRequestDto::value` carries `"key"` or `"key,phase"`. There is no
    /// existing key action at all, so a keyboard-driven app cannot be driven
    /// even in principle today.
    Key,
}

/// A structured target resolved by the `WebView` host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppUiTargetDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One permission-gated UI automation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppUiRequestDto {
    pub request_id: String,
    pub app_id: String,
    pub action: AppUiActionKindDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AppUiTargetDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// Native capability whose first use requires a user decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppCapabilityKindDto {
    DataMutation,
    UiControl,
    NetworkDomain,
    RestoreCheckpoint,
    Camera,
    PhotoLibrary,
    Microphone,
    Location,
    Notifications,
    /// Legacy combined files prompt. New grants use [`Self::FilesRead`] /
    /// [`Self::FilesWrite`].
    Files,
    Clipboard,
    Share,
    TextToSpeech,
    DeviceStatus,
    Haptics,
    DeepLink,
    Llm,
    AgentNotify,
    BackgroundSchedule,
    Calendar,
    Contacts,
    Media,
    FilesRead,
    FilesWrite,
}

/// A capability approval request surfaced by the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppCapabilityRequestDto {
    pub request_id: String,
    pub app_id: String,
    pub capability: AppCapabilityKindDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    pub reason: String,
}

/// User decision for data/UI/capability requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppAuthorizationDecisionDto {
    Deny,
    AllowOnce,
    AllowSession,
    AllowAlways,
}

/// Extensible local-app event payload carried by the single top-level
/// `ClientEvent::AppEvent` envelope to keep `UniFFI` enum metadata bounded.
// Boxing variants would change the generated mobile binding API; this is
// infrequent control-plane data rather than a hot-path value.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppEventDto {
    AppDetailsChanged {
        details: AppDetailsDto,
    },
    AppBridgeResponse {
        response: AppBridgeResponseDto,
    },
    AppUiRequest {
        request: AppUiRequestDto,
    },
    AppCapabilityRequested {
        request: AppCapabilityRequestDto,
    },
    AppCheckpointsChanged {
        app_id: String,
        checkpoints: Vec<AppCheckpointDto>,
    },
    /// An app-initiated `llm.chat` side query started (`active: true`) or
    /// finished/failed (`active: false`). Drives the client's "app is
    /// calling AI" indicator; always emitted in pairs.
    AppLlmActivityChanged {
        app_id: String,
        active: bool,
    },
    /// An app posted one event into its conversation mailbox via
    /// `agent.post`. Deliberately carries NO body — clients badge on it;
    /// the payload is read only through the MCP `read_app_events` tool.
    AppAgentEventPosted {
        app_id: String,
        seq: u64,
        topic: String,
        created_at_ms: u64,
    },
    /// One ordered v2 bridge stream frame.  This is appended so UniFFI enum
    /// ordinals for existing local-app events remain stable.
    AppBridgeStreamFrame {
        frame: AppBridgeStreamFrameDto,
        /// Pre-serialized camelCase frame for WebView delivery on FFI clients.
        #[serde(rename = "frameJson")]
        frame_json: String,
    },
    /// Incremental app-record update; appended to preserve existing enum ordinals.
    AppRecordChanged {
        record: AppRecordDto,
    },
    /// A trusted client must explicitly approve or reject this proposal.
    AppProfileProposal {
        proposal: AppAgentProfileProposalDto,
    },
    /// Host-owned local-app background task outcome. The result is bounded
    /// and optional; clients can fetch the durable record through the app's
    /// background status/list API when they need the full lifecycle view.
    /// Appended to preserve existing UniFFI enum ordinals.
    AppBackgroundTaskChanged {
        app_id: String,
        task_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_json: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        retryable: bool,
    },
    /// A new app record committed. `AppsChanged` carries the whole list; this
    /// names the one that is new, which a client that started the creation
    /// needs in order to land the user on it.
    ///
    /// Appended at the END, and that is load-bearing rather than tidy: UniFFI
    /// encodes this enum POSITIONALLY, so a variant inserted anywhere above
    /// renumbers every variant after it. A client still running the previous
    /// bindings would then decode one event as another — silently, because
    /// mobile has no protocol handshake to catch it. Verified after
    /// regeneration by reading the ordinals out of the generated Kotlin.
    AppCreated {
        record: AppRecordDto,
    },
}

#[cfg(test)]
mod tests {
    use super::AppBridgeStreamFrameDto;

    #[test]
    fn v2_stream_frame_fields_use_camel_case_on_json_wire() {
        let value = serde_json::to_value(AppBridgeStreamFrameDto::Data {
            app_id: "abc12345".into(),
            request_id: "request-1".into(),
            stream_id: "stream-1".into(),
            seq: 0,
            data_json: "{}".into(),
        })
        .expect("serialize stream frame");
        assert_eq!(value["appId"], "abc12345");
        assert_eq!(value["requestId"], "request-1");
        assert_eq!(value["streamId"], "stream-1");
        assert_eq!(value["dataJson"], "{}");
        assert!(value.get("app_id").is_none());
    }
}
