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
//! [`AppErrorCodeDto`], [`AppCheckpointKindDto`]) ride as bare wire STRINGS
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

/// One app row — the lowered core `AppRecord`. Carried by
/// [`AppsChanged`](crate::events::ClientEvent::AppsChanged).
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

/// Native host context captured for platform-aware generated UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct DeviceContextDto {
    pub os: String,
    pub form_factor: String,
    pub viewport: DeviceViewportDto,
    pub safe_area: DeviceInsetsDto,
    pub color_scheme: String,
    pub reduced_motion: bool,
    pub input_mode: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct DeviceViewportDto {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[serde(rename_all = "camelCase")]
pub struct DeviceInsetsDto {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
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

/// Operations accepted by the versioned `window.lingxi.v1` bridge.
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
    /// One side-query chat completion against the user's model (`llm.chat`).
    LlmChat,
    /// Post one event into the app's conversation mailbox (`agent.post`).
    AgentPost,
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
    Llm,
    AgentNotify,
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
}
