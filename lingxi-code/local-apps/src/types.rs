//! Core data model for local apps (spec §A).
//!
//! Serde conventions follow the repo's persisted-JSON style (see
//! `cron::tasks_file`): camelCase struct fields, `snake_case` enum variant
//! values, epoch **milliseconds** timestamps as `u64`, optionals omitted when
//! absent. These types are persistence/domain types — the client-protocol
//! crate defines its own wire DTOs and maps to/from these in the engine.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Schema version stamped on every persisted local-apps file.
pub const APPS_SCHEMA_VERSION: u32 = 1;

/// Designer/generation workflow state of an app (spec §B state machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppWorkflowState {
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
    /// Code generation is running (phase 3 drives this).
    Generating,
    /// Generated output is being validated.
    Validating,
    /// The preview interaction is pending user confirmation.
    AwaitingPreviewConfirmation,
    /// A revision pass is running after feedback or failed validation.
    Revising,
    /// The app is generated, validated and user-approved.
    Ready,
    /// Generation failed; retry requires a confirmed, unchanged draft, and
    /// `open_designer` re-opens the draft when that input must change.
    GenerationFailed,
    /// Validation failed; `begin_revision` starts a fix-up pass.
    ValidationFailed,
}

impl AppWorkflowState {
    /// Canonical `snake_case` name (the persisted/wire value).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthoringQuestionnaire => "authoring_questionnaire",
            Self::QuestionnaireFailed => "questionnaire_failed",
            Self::CollectingSpec => "collecting_spec",
            Self::Planning => "planning",
            Self::PlanFailed => "plan_failed",
            Self::AwaitingSpecConfirmation => "awaiting_spec_confirmation",
            Self::Generating => "generating",
            Self::Validating => "validating",
            Self::AwaitingPreviewConfirmation => "awaiting_preview_confirmation",
            Self::Revising => "revising",
            Self::Ready => "ready",
            Self::GenerationFailed => "generation_failed",
            Self::ValidationFailed => "validation_failed",
        }
    }
}

impl fmt::Display for AppWorkflowState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Runtime (dev-server) state of an app (spec §C).
///
/// Phase 1 persists the record only — no process management exists yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppRuntimeState {
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

impl AppRuntimeState {
    /// Canonical `snake_case` name (the persisted/wire value).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for AppRuntimeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One app as listed in `apps/index.json` (and mirrored into the app's
/// `workspace/.lingxi/app.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRecord {
    /// Stable app id matching `^[a-z0-9][a-z0-9-]{0,63}$`.
    pub id: String,
    /// User-facing display name.
    pub name: String,
    /// One-line description the user gave at creation time. All three LLM
    /// stages (authoring the questionnaire, planning, writing source) read
    /// it. Stored ONCE — the list page displays it, a failed questionnaire
    /// authoring retries from it, and `generate_source` already calls
    /// `service.record()` to reach it. Storing a second copy would
    /// inevitably drift.
    pub brief: String,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: u64,
    /// Last mutation time, epoch milliseconds.
    pub updated_at_ms: u64,
    /// Current designer/generation workflow state.
    pub workflow_state: AppWorkflowState,
    /// Conversation the app was created from (`origin: chat`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Workspace directory relative to the data root, always
    /// `apps/<id>/workspace` with forward slashes.
    pub workspace_rel: String,
}

/// Density choice for [`DesignValue::Density`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DensityLevel {
    /// Tight spacing.
    Compact,
    /// Relaxed spacing.
    Comfortable,
}

/// A single draft field value, tagged by field kind.
///
/// Serialized adjacently tagged as `{ "kind": …, "value": … }`. Drafts are
/// schema-agnostic field maps in phase 1 — validation against a per-template
/// `design-schema.json` is phase 3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DesignValue {
    /// One-line free text.
    ShortText(String),
    /// Multi-line free text.
    LongText(String),
    /// Exactly one choice out of a template-defined set.
    SingleChoice(String),
    /// Any number of choices out of a template-defined set.
    MultipleChoice(Vec<String>),
    /// On/off toggle.
    Boolean(bool),
    /// Color value (e.g. `#aabbcc`).
    Color(String),
    /// Layout density.
    Density(DensityLevel),
    /// Ordered list of screen names.
    ScreenList(Vec<String>),
    /// Ordered list of feature names.
    FeatureList(Vec<String>),
    /// Structured data-field declarations rendered by the dynamic designer.
    DataFieldList(Vec<crate::manifest::DataFieldSchema>),
    /// HTTPS host names declared for the native network bridge.
    DomainList(Vec<String>),
    /// The user explicitly chose to let the LLM decide this field.
    Deferred,
}

/// One patch operation against the draft field map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AppDesignPatchOp {
    /// Insert or replace `field_id` with `value`.
    #[serde(rename_all = "camelCase")]
    Set {
        /// Field to set.
        field_id: String,
        /// New value.
        value: DesignValue,
    },
    /// Remove `field_id` (a no-op when the field is absent).
    #[serde(rename_all = "camelCase")]
    Remove {
        /// Field to remove.
        field_id: String,
    },
}

/// An ordered batch of draft edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDesignPatch {
    /// Operations applied in order.
    pub ops: Vec<AppDesignPatchOp>,
    /// Optional human-readable summary of the edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An agent-proposed draft patch awaiting explicit user application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDesignSuggestion {
    /// Id the client must echo back to apply the suggestion.
    pub suggestion_id: String,
    /// The proposed edit.
    pub patch: AppDesignPatch,
    /// Draft revision the suggestion was computed against. LOAD-BEARING:
    /// `apply_suggestion` refuses (and consumes, with a typed
    /// `revision_conflict` + `DesignConflict` event) any suggestion whose
    /// `based_on_revision` is not the CURRENT draft revision — a stale
    /// suggestion must never silently overwrite newer user edits.
    pub based_on_revision: u64,
}

/// The design draft persisted at `workspace/.lingxi/design-spec.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDesignDraft {
    /// Persisted schema version ([`APPS_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Monotonic edit counter; bumped by every applied field change.
    pub revision: u64,
    /// The questionnaire the LLM authored. Immutable once authoring
    /// succeeds.
    #[serde(default)]
    pub questionnaire: Vec<crate::questionnaire::AppDesignStep>,
    /// Schema-agnostic answer map, keyed by [`crate::questionnaire::AppDesignField::id`].
    #[serde(default)]
    pub fields: BTreeMap<String, DesignValue>,
    /// The "will create" summary shown on the confirmation page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<crate::questionnaire::AppPlan>,
    /// Which revision `plan` was computed against. Any further answer edit
    /// invalidates it — the same staleness guard
    /// [`AppDesignSuggestion::based_on_revision`] uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_for_revision: Option<u64>,
    /// At most one agent suggestion awaiting application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_suggestion: Option<AppDesignSuggestion>,
    /// Revision the user confirmed for generation, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_revision: Option<u64>,
}

/// What kind of human gate an interaction opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppInteractionKind {
    /// The design-spec confirmation gate.
    Designer,
    /// The generated-preview confirmation gate.
    Preview,
}

impl AppInteractionKind {
    /// Canonical `snake_case` name (the persisted/wire value).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Designer => "designer",
            Self::Preview => "preview",
        }
    }
}

impl fmt::Display for AppInteractionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A pending human gate. At most ONE exists per app at any time. The
/// `interaction_id` gates the CLIENT protocol surface via events: it is
/// announced through `DesignerRequested` / `PreviewReady` when the gate opens
/// and re-announced for every pending gate after a service load, and a client
/// echoes it back to `confirm_*` to prove it saw the gate. In-process read
/// APIs (`pending_interaction`, `interactions`) DO expose the id to embedding
/// code — the gate is a UI-flow handshake, not a secret from the host
/// process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInteractionRequest {
    /// Id the client must echo back to confirm.
    pub interaction_id: String,
    /// App the gate belongs to.
    pub app_id: String,
    /// Which gate this is.
    pub kind: AppInteractionKind,
    /// Draft revision at the time the gate was opened (informational —
    /// confirmation always validates against the CURRENT revision).
    pub revision: u64,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: u64,
}

/// Progress report emitted while generating (phase 3 produces these).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppGenerationProgress {
    /// App being generated.
    pub app_id: String,
    /// Free-form stage label (e.g. `scaffold`, `pages`).
    pub stage: String,
    /// Optional 0–100 completion estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// Optional human-readable detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Preview handle for a generated revision. `url` stays `None` until the
/// phase-4 runtime can actually serve the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPreview {
    /// App the preview belongs to.
    pub app_id: String,
    /// Draft revision the preview was generated from.
    pub revision: u64,
    /// Where the preview is served, once a runtime exists (phase 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Why a checkpoint was recorded (git wiring is phase 5; type only for now).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCheckpointKind {
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

/// One restorable checkpoint of an app workspace (phase 5 wires git).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCheckpoint {
    /// Stable checkpoint id.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Why the checkpoint was recorded.
    pub kind: AppCheckpointKind,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: u64,
}

/// What a continuation resumes (spec §E).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppContinuationKind {
    /// The user confirmed the design spec.
    DesignConfirmed,
    /// The user cancelled out of the design confirmation gate.
    DesignCancelled,
    /// The user approved the preview.
    PreviewConfirmed,
    /// The user asked for a revision (payload carries the prompt).
    RevisionRequested,
}

impl AppContinuationKind {
    /// Canonical `snake_case` name (the persisted/wire value).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DesignConfirmed => "design_confirmed",
            Self::DesignCancelled => "design_cancelled",
            Self::PreviewConfirmed => "preview_confirmed",
            Self::RevisionRequested => "revision_requested",
        }
    }
}

impl fmt::Display for AppContinuationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A resumable human-gate outcome queued for at-least-once delivery to the
/// agent conversation. Consumers dedup by `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppContinuation {
    /// Per-app monotonically increasing sequence number (dedup key).
    pub seq: u64,
    /// App the continuation belongs to.
    pub app_id: String,
    /// What was decided at the gate.
    pub kind: AppContinuationKind,
    /// Structured summary, e.g. `{ "revision": 3 }` or `{ "prompt": "…" }`.
    pub payload: serde_json::Value,
    /// Enqueue time, epoch milliseconds.
    pub created_at_ms: u64,
}

/// Per-app runtime record persisted at `apps/<id>/runtime.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppRuntimeMode {
    /// Store/Play static export served by the Rust loopback asset server.
    StaticExport,
    /// Full/Direct fixed Next production server.
    NextProduction,
}

/// Per-app runtime record persisted at `apps/<id>/runtime.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRuntimeRecord {
    /// Persisted schema version ([`APPS_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// App the record belongs to.
    pub app_id: String,
    /// Current runtime state.
    pub state: AppRuntimeState,
    /// Explicit runtime mode; absent only for pre-1.2 migrated records that
    /// have not been started since upgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AppRuntimeMode>,
    /// Dev-server port (phase 4). Once assigned it is NEVER reassigned —
    /// the `localhost:<port>` origin anchors the app's `IndexedDB` data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Dev-server pid (phase 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Last runtime failure, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Last mutation time, epoch milliseconds.
    pub updated_at_ms: u64,
}

/// Per-app interaction + continuation store persisted at
/// `apps/<id>/interactions.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInteractions {
    /// Persisted schema version ([`APPS_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The single pending human gate, if one is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<AppInteractionRequest>,
    /// Next continuation sequence number to mint (starts at 1).
    pub next_seq: u64,
    /// Highest seq for which a sink delivery has succeeded.
    pub last_delivered_seq: u64,
    /// Continuations enqueued but not yet successfully delivered, in seq order.
    #[serde(default)]
    pub undelivered: Vec<AppContinuation>,
}

impl AppInteractions {
    /// Fresh store for a new app: no pending gate, seq counter at 1.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema_version: APPS_SCHEMA_VERSION,
            pending: None,
            next_seq: 1,
            last_delivered_seq: 0,
            undelivered: Vec::new(),
        }
    }
}

impl Default for AppInteractions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_serialize_to_spec_snake_case_strings() {
        assert_eq!(
            serde_json::to_string(&AppWorkflowState::AwaitingSpecConfirmation).unwrap(),
            "\"awaiting_spec_confirmation\""
        );
        assert_eq!(
            serde_json::to_string(&AppRuntimeState::Stopped).unwrap(),
            "\"stopped\""
        );
        assert_eq!(
            serde_json::to_string(&AppContinuationKind::RevisionRequested).unwrap(),
            "\"revision_requested\""
        );
        assert_eq!(
            serde_json::to_string(&AppCheckpointKind::PreRestore).unwrap(),
            "\"pre_restore\""
        );
        assert_eq!(
            serde_json::to_string(&AppInteractionKind::Designer).unwrap(),
            "\"designer\""
        );
    }

    #[test]
    fn design_value_is_kind_value_tagged() {
        let v = DesignValue::ShortText("Team Board".into());
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"kind":"short_text","value":"Team Board"}"#
        );
        let v = DesignValue::Density(DensityLevel::Compact);
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"kind":"density","value":"compact"}"#
        );
        let v = DesignValue::ScreenList(vec!["home".into(), "detail".into()]);
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"kind":"screen_list","value":["home","detail"]}"#
        );
        let v = DesignValue::Boolean(true);
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, r#"{"kind":"boolean","value":true}"#);
        let back: DesignValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn patch_ops_are_op_tagged_camel_case() {
        let patch = AppDesignPatch {
            ops: vec![
                AppDesignPatchOp::Set {
                    field_id: "title".into(),
                    value: DesignValue::ShortText("Hi".into()),
                },
                AppDesignPatchOp::Remove {
                    field_id: "accent".into(),
                },
            ],
            note: None,
        };
        let json = serde_json::to_string(&patch).unwrap();
        assert!(json.contains(r#""op":"set""#));
        assert!(json.contains(r#""op":"remove""#));
        assert!(json.contains(r#""fieldId":"title""#));
        // Absent note is omitted.
        assert!(!json.contains("note"));
        let back: AppDesignPatch = serde_json::from_str(&json).unwrap();
        assert_eq!(back, patch);
    }

    #[test]
    fn record_serializes_camel_case_and_omits_absent_conversation() {
        let record = AppRecord {
            id: "abc123".into(),
            name: "Habits".into(),
            brief: "Track daily habits".into(),
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_001,
            workflow_state: AppWorkflowState::CollectingSpec,
            conversation_id: None,
            workspace_rel: "apps/abc123/workspace".into(),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"createdAtMs\":1700000000000"));
        assert!(json.contains("\"workflowState\":\"collecting_spec\""));
        assert!(json.contains("\"workspaceRel\":\"apps/abc123/workspace\""));
        assert!(json.contains("\"brief\":\"Track daily habits\""));
        assert!(!json.contains("conversationId"));
        let back: AppRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn draft_and_interactions_round_trip() {
        let mut fields = BTreeMap::new();
        fields.insert("title".to_string(), DesignValue::ShortText("T".into()));
        let draft = AppDesignDraft {
            schema_version: APPS_SCHEMA_VERSION,
            revision: 4,
            questionnaire: Vec::new(),
            fields,
            plan: None,
            plan_for_revision: None,
            pending_suggestion: Some(AppDesignSuggestion {
                suggestion_id: "sugg-1".into(),
                patch: AppDesignPatch {
                    ops: vec![],
                    note: Some("polish".into()),
                },
                based_on_revision: 4,
            }),
            confirmed_revision: None,
        };
        let json = serde_json::to_string(&draft).unwrap();
        assert!(json.contains("\"schemaVersion\":1"));
        assert!(json.contains("\"basedOnRevision\":4"));
        assert!(!json.contains("confirmedRevision"));
        assert!(!json.contains("\"plan\""));
        assert!(!json.contains("planForRevision"));
        let back: AppDesignDraft = serde_json::from_str(&json).unwrap();
        assert_eq!(back, draft);

        let ints = AppInteractions::new();
        let json = serde_json::to_string(&ints).unwrap();
        assert!(json.contains("\"nextSeq\":1"));
        assert!(json.contains("\"lastDeliveredSeq\":0"));
        assert!(!json.contains("\"pending\""));
        let back: AppInteractions = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ints);
    }

    #[test]
    fn continuation_payload_round_trips() {
        let cont = AppContinuation {
            seq: 7,
            app_id: "abc123".into(),
            kind: AppContinuationKind::RevisionRequested,
            payload: serde_json::json!({ "prompt": "make it blue" }),
            created_at_ms: 42,
        };
        let json = serde_json::to_string(&cont).unwrap();
        assert!(json.contains("\"kind\":\"revision_requested\""));
        assert!(json.contains("\"prompt\":\"make it blue\""));
        let back: AppContinuation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cont);
    }
}
