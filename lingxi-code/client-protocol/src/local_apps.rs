//! Local-apps DTOs — the wire shapes for the on-device "Apps" capability
//! (local-apps phase 1: agent-designed Next.js mini-apps).
//!
//! Protocol-local mirrors of the `local-apps` core types (`AppRecord`,
//! `AppDesignPatch`, `DesignValue`, `AppCheckpoint`, `AppErrorCode`, …) — the
//! core crate never appears here; the engine lowers core ⇄ DTO at the dispatch
//! boundary, exactly like the other listing/row DTOs.
//!
//! Serde conventions (decision §0.1) with two DELIBERATE departures, both
//! following the [`crate::computer_access::AccessTierDto`] precedent of staying
//! byte-identical to the source contract:
//! - the fieldless enums ([`AppWorkflowStateDto`],
//!   [`AppRuntimeStateDto`], [`AppCreateOriginDto`], [`AppErrorCodeDto`],
//!   [`AppCheckpointKindDto`], [`DensityLevelDto`]) ride as bare wire STRINGS
//!   (`"dashboard"`, `"collecting_spec"`, `"not_found"`, …) — a plain
//!   `#[serde(rename_all = "snake_case")]` fieldless enum, byte-identical to
//!   the core enums' canonical `as_str()` values;
//! - [`DesignValueDto`] is tagged on `kind` (`{ "kind": "short_text",
//!   "value": … }`) and [`AppDesignPatchOpDto`] on `op` (`{ "op": "set",
//!   "field_id": …, "value": … }`) — the discriminator names the local-apps
//!   spec §A fixes. [`DesignValueDto`] is byte-compatible with the core
//!   `DesignValue` wire form; [`AppDesignPatchOpDto`] is NOT — this wire
//!   keeps protocol `snake_case` `"field_id"` while the core persists
//!   camelCase `"fieldId"` (each pinned by its own tests/fixtures), so patch
//!   ops must cross the seam through the engine's `raise_patch` /
//!   `lower_patch`, never by re-serializing one side's serde form as the
//!   other's.
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

/// Designer/generation workflow state of an app — mirrors the core
/// `AppWorkflowState` (spec §B state machine). A bare wire STRING
/// (`"collecting_spec"`, …). `#[non_exhaustive]` so a future state is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppWorkflowStateDto {
    /// LLM is authoring the questionnaire for this brief. Designer is
    /// read-only.
    AuthoringQuestionnaire,
    /// Authoring failed; retryable, or the brief can be changed and
    /// re-authored.
    QuestionnaireFailed,
    /// Draft is being filled in; no confirmation gate is open.
    CollectingSpec,
    /// LLM is deriving the plan from the answers. Designer is read-only.
    Planning,
    /// Planning failed; retryable.
    PlanFailed,
    /// The designer interaction is pending user confirmation.
    AwaitingSpecConfirmation,
    /// Code generation is running.
    Generating,
    /// Generated output is being validated.
    Validating,
    /// The preview interaction is pending user confirmation.
    AwaitingPreviewConfirmation,
    /// A revision pass is running after feedback or failed validation.
    Revising,
    /// The app is generated, validated and user-approved.
    Ready,
    /// Generation failed; retry requires a confirmed, unchanged draft.
    GenerationFailed,
    /// Validation failed; a revision pass fixes it up.
    ValidationFailed,
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

/// Density choice for [`DesignValueDto::Density`] — mirrors the core
/// `DensityLevel`. A bare wire STRING (`"compact"` / `"comfortable"`).
/// `#[non_exhaustive]` so a future level is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DensityLevelDto {
    /// Tight spacing.
    Compact,
    /// Relaxed spacing.
    Comfortable,
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

/// Dynamic field kind rendered by the platform design wizards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppDesignFieldTypeDto {
    ShortText,
    LongText,
    SingleChoice,
    MultipleChoice,
    Boolean,
    Color,
    Density,
    ScreenList,
    FeatureList,
    DataFieldList,
    DomainList,
}

/// One selectable option for a dynamic design field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppDesignFieldOptionDto {
    pub value: String,
    pub label: String,
}

/// One Rust-defined input rendered by Android and iOS without hard-coded forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppDesignFieldDto {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub field_type: AppDesignFieldTypeDto,
    pub required: bool,
    /// 渲染 `Other…` 自由文本框。
    #[serde(default)]
    pub allows_custom: bool,
    /// 渲染「由你决定」。
    #[serde(default)]
    pub allows_defer: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<DesignValueDto>,
    pub options: Vec<AppDesignFieldOptionDto>,
}

/// One ordered step in the canonical five-step app designer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppDesignStepDto {
    pub id: String,
    pub order: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub fields: Vec<AppDesignFieldDto>,
}

/// 确认页展示的「将创建」摘要。由 LLM 从答案推导，经 `local-apps` 校验。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppPlanDto {
    pub collections: Vec<AppDataCollectionDto>,
    pub capabilities: Vec<AppCapabilityKindDto>,
    /// 外部 HTTPS 主机名。
    pub domains: Vec<String>,
    /// 给用户读的一段人话；每个被 defer 的字段最终定成什么写在这里。
    pub summary: String,
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
    /// Current designer/generation workflow state.
    pub workflow_state: AppWorkflowStateDto,
    /// Conversation the app was created from (`origin: chat`), if any. Skipped
    /// from the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Workspace directory relative to the engine data root (always
    /// `apps/<id>/workspace`, forward slashes).
    pub workspace_rel: String,
}

/// A single draft field value, tagged by field kind — mirrors the core
/// `DesignValue`. Tagged on `kind` with the payload under `value`
/// (`{ "kind": "short_text", "value": "…" }`; see the module doc), the shape
/// the local-apps spec §A fixes. `#[non_exhaustive]` so a future field kind is
/// additive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DesignValueDto {
    /// One-line free text.
    ShortText {
        /// The text value.
        value: String,
    },
    /// Multi-line free text.
    LongText {
        /// The text value.
        value: String,
    },
    /// Exactly one choice out of a template-defined set.
    SingleChoice {
        /// The chosen option.
        value: String,
    },
    /// Any number of choices out of a template-defined set.
    MultipleChoice {
        /// The chosen options.
        value: Vec<String>,
    },
    /// On/off toggle.
    Boolean {
        /// The toggle state.
        value: bool,
    },
    /// Color value (e.g. `#aabbcc`).
    Color {
        /// The color string.
        value: String,
    },
    /// Layout density.
    Density {
        /// The density level.
        value: DensityLevelDto,
    },
    /// Ordered list of screen names.
    ScreenList {
        /// The screen names in order.
        value: Vec<String>,
    },
    /// Ordered list of feature names.
    FeatureList {
        /// The feature names in order.
        value: Vec<String>,
    },
    /// Structured collection field declarations.
    DataFieldList { value: Vec<AppDataFieldDto> },
    /// HTTPS host names an app may request through the native network bridge.
    DomainList { value: Vec<String> },
    /// The user explicitly chose to let the LLM decide this field. Carries no
    /// payload — `{ "kind": "deferred" }` is the complete wire form. Mirrors
    /// the core `DesignValue::Deferred` (local-apps#questionnaire, Task 1/2);
    /// `local-apps` and `engine-mobile` gate every write/load/lowering path
    /// so this variant only ever appears once a legitimate answer exists.
    Deferred,
}

/// One patch operation against the draft field map — mirrors the core
/// `AppDesignPatchOp` in shape, NOT in bytes: this wire keeps protocol
/// `snake_case` `field_id` while the core persists camelCase `fieldId` (see the
/// module doc). Tagged on `op` (`{ "op": "set", "field_id": …, "value": … }` /
/// `{ "op": "remove", "field_id": … }`).
/// `#[non_exhaustive]` so a future op is additive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(tag = "op", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppDesignPatchOpDto {
    /// Insert or replace `field_id` with `value`.
    Set {
        /// Field to set.
        field_id: String,
        /// New value.
        value: DesignValueDto,
    },
    /// Remove `field_id` (a no-op when the field is absent).
    Remove {
        /// Field to remove.
        field_id: String,
    },
}

/// An ordered batch of draft edits — mirrors the core `AppDesignPatch`.
/// Carried by
/// [`UpdateAppDesignDraft`](crate::commands::ClientCommand::UpdateAppDesignDraft)
/// and
/// [`AppDesignSuggestionAvailable`](crate::events::ClientEvent::AppDesignSuggestionAvailable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppDesignPatchDto {
    /// Operations applied in order.
    pub ops: Vec<AppDesignPatchOpDto>,
    /// Optional human-readable summary of the edit. Skipped from the wire when
    /// `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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

/// Persisted generation/build queue state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppGenerationJobStateDto {
    Queued,
    Scaffolding,
    Generating,
    Validating,
    Building,
    StartingPreview,
    AwaitingApproval,
    Succeeded,
    Failed,
    Cancelled,
}

/// One durable generation job, including enough state for restart recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppGenerationJobDto {
    pub id: String,
    pub app_id: String,
    pub revision: u64,
    pub continuation_seq: u64,
    pub state: AppGenerationJobStateDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_rel: Option<String>,
    pub updated_at_ms: u64,
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
    pub design_revision: u64,
    pub design_fields: Vec<AppDesignFieldValueDto>,
    /// The LLM-authored questionnaire driving the designer. Empty before
    /// authoring completes.
    pub questionnaire: Vec<AppDesignStepDto>,
    /// The LLM-derived plan awaiting confirmation, if one has been authored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<AppPlanDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<AppManifestDto>,
    pub runtime: AppRuntimeDetailsDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_job: Option<AppGenerationJobDto>,
    pub checkpoints: Vec<AppCheckpointDto>,
}

/// A deterministic design field/value pair; unlike a map it crosses `UniFFI`
/// and serializes in a stable order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppDesignFieldValueDto {
    pub field_id: String,
    pub value: DesignValueDto,
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
    /// The LLM finished (or discarded) authoring the questionnaire.
    AppQuestionnaireChanged {
        app_id: String,
        revision: u64,
        steps: Vec<AppDesignStepDto>,
    },
    /// The LLM finished (or discarded) deriving the plan.
    AppPlanChanged {
        app_id: String,
        revision: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<AppPlanDto>,
    },
    AppGenerationJobChanged {
        job: AppGenerationJobDto,
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
