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
//! - the fieldless enums ([`AppTemplateKindDto`], [`AppWorkflowStateDto`],
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
//!   keeps protocol snake_case `"field_id"` while the core persists
//!   camelCase `"fieldId"` (each pinned by its own tests/fixtures), so patch
//!   ops must cross the seam through the engine's `raise_patch` /
//!   `lower_patch`, never by re-serializing one side's serde form as the
//!   other's.
//!
//! Every optional field uses
//! `#[serde(default, skip_serializing_if = "Option::is_none")]`;
//! `serde_json::Value` never enters this crate (decision §0.4).

use serde::{Deserialize, Serialize};

/// Which scaffold template an app is designed from — mirrors the core
/// `AppTemplateKind`. A bare wire STRING (`"dashboard"`, …; see the module
/// doc). `#[non_exhaustive]` so a future template is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppTemplateKindDto {
    /// Read-mostly metric/dashboard app.
    Dashboard,
    /// Create/read/update/delete tracker.
    CrudTracker,
    /// Content/gallery showcase.
    ContentShowcase,
    /// Single-purpose form utility.
    FormUtility,
}

/// Designer/generation workflow state of an app — mirrors the core
/// `AppWorkflowState` (spec §B state machine). A bare wire STRING
/// (`"collecting_spec"`, …). `#[non_exhaustive]` so a future state is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppWorkflowStateDto {
    /// Draft is being filled in; no confirmation gate is open.
    CollectingSpec,
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

/// One app row — the lowered core `AppRecord`. Carried by
/// [`AppsChanged`](crate::events::ClientEvent::AppsChanged).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct AppRecordDto {
    /// Stable app id matching `^[a-z0-9][a-z0-9-]{0,63}$`.
    pub id: String,
    /// User-facing display name.
    pub name: String,
    /// Template the app is designed from.
    pub template: AppTemplateKindDto,
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
}

/// One patch operation against the draft field map — mirrors the core
/// `AppDesignPatchOp` in shape, NOT in bytes: this wire keeps protocol
/// snake_case `field_id` while the core persists camelCase `fieldId` (see the
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
