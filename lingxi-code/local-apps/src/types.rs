//! Core data model for local apps (spec §A).
//!
//! Serde conventions follow the repo's persisted-JSON style (see
//! `cron::tasks_file`): camelCase struct fields, `snake_case` enum variant
//! values, epoch **milliseconds** timestamps as `u64`, optionals omitted when
//! absent. These types are persistence/domain types — the client-protocol
//! crate defines its own wire DTOs and maps to/from these in the engine.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Schema version stamped on every persisted local-apps file.
pub const APPS_SCHEMA_VERSION: u32 = 1;

/// New apps use Git-backed source version control unless the user opts out
/// during creation. Missing values on older records deserialize as enabled.
pub const DEFAULT_GIT_VERSION_CONTROL: bool = true;

fn default_git_version_control() -> bool {
    DEFAULT_GIT_VERSION_CONTROL
}

/// Two-state workflow of an app (v3): the conversation agent drives app
/// creation, so the record only tracks whether a runnable output exists yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppWorkflowState {
    /// Being worked on by the agent (v3). Every legacy pipeline stage maps
    /// here on load — the pipeline is gone, so "somewhere mid-pipeline" can
    /// only mean "not ready yet".
    #[serde(
        alias = "authoring_questionnaire",
        alias = "questionnaire_failed",
        alias = "collecting_spec",
        alias = "planning",
        alias = "plan_failed",
        alias = "awaiting_spec_confirmation",
        alias = "generating",
        alias = "validating",
        alias = "awaiting_preview_confirmation",
        alias = "revising",
        alias = "generation_failed",
        alias = "validation_failed"
    )]
    Draft,
    /// Has a successfully built, runnable output (host stamps this after a
    /// successful offline build).
    Ready,
}

impl AppWorkflowState {
    /// Canonical `snake_case` name (the persisted/wire value).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Ready => "ready",
        }
    }
}

impl fmt::Display for AppWorkflowState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Runtime (dev-server) state of an app (spec §C).
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
    /// One-line description the user gave at creation time. The agent reads
    /// it for context; the list page displays it. Stored ONCE — a second
    /// copy would inevitably drift.
    pub brief: String,
    /// Whether Git controls this app's source checkpoints and restores.
    #[serde(default = "default_git_version_control")]
    pub git_enabled: bool,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: u64,
    /// Last mutation time, epoch milliseconds.
    pub updated_at_ms: u64,
    /// Current workflow state.
    pub workflow_state: AppWorkflowState,
    /// Conversation the app was created from (`origin: chat`), if any —
    /// the SOURCE link only; the app's own conversations live in its
    /// workspace-scoped session catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// The app's pinned "init" session (bare uuid) — the conversation the
    /// app was set up in, listed first in the app's session catalog. Minted
    /// by the engine at create time (fork of the origin chat, or an empty
    /// anchor) and backfilled at boot for apps that predate it. Same
    /// default+skip serde shape as `conversation_id`, so old stores load
    /// unchanged and the goldens stay byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_session_id: Option<String>,
    /// Workspace directory relative to the data root, always
    /// `apps/<id>/workspace` with forward slashes.
    pub workspace_rel: String,
}

/// Why a checkpoint was recorded.
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

/// One restorable checkpoint of an app workspace.
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
    /// Dev-server port. Once assigned it is NEVER reassigned —
    /// the `localhost:<port>` origin anchors the app's `IndexedDB` data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Dev-server pid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Last runtime failure, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Last mutation time, epoch milliseconds.
    pub updated_at_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_serialize_to_spec_snake_case_strings() {
        assert_eq!(
            serde_json::to_string(&AppWorkflowState::Draft).unwrap(),
            "\"draft\""
        );
        assert_eq!(
            serde_json::to_string(&AppWorkflowState::Ready).unwrap(),
            "\"ready\""
        );
        assert_eq!(
            serde_json::to_string(&AppRuntimeState::Stopped).unwrap(),
            "\"stopped\""
        );
        assert_eq!(
            serde_json::to_string(&AppCheckpointKind::PreRestore).unwrap(),
            "\"pre_restore\""
        );
    }

    #[test]
    fn every_legacy_pipeline_state_deserializes_to_draft() {
        for legacy in [
            "authoring_questionnaire",
            "questionnaire_failed",
            "collecting_spec",
            "planning",
            "plan_failed",
            "awaiting_spec_confirmation",
            "generating",
            "validating",
            "awaiting_preview_confirmation",
            "revising",
            "generation_failed",
            "validation_failed",
            // The canonical v3 spelling parses too, of course.
            "draft",
        ] {
            let parsed: AppWorkflowState =
                serde_json::from_str(&format!("\"{legacy}\"")).unwrap();
            assert_eq!(parsed, AppWorkflowState::Draft, "{legacy}");
        }
        let parsed: AppWorkflowState = serde_json::from_str("\"ready\"").unwrap();
        assert_eq!(parsed, AppWorkflowState::Ready);
    }

    #[test]
    fn record_serializes_camel_case_and_omits_absent_conversation() {
        let record = AppRecord {
            id: "abc123".into(),
            name: "Habits".into(),
            brief: "Track daily habits".into(),
            git_enabled: true,
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_001,
            workflow_state: AppWorkflowState::Draft,
            conversation_id: None,
            init_session_id: None,
            workspace_rel: "apps/abc123/workspace".into(),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"createdAtMs\":1700000000000"));
        assert!(json.contains("\"workflowState\":\"draft\""));
        assert!(json.contains("\"workspaceRel\":\"apps/abc123/workspace\""));
        assert!(json.contains("\"brief\":\"Track daily habits\""));
        assert!(json.contains("\"gitEnabled\":true"));
        assert!(!json.contains("conversationId"));
        let back: AppRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn a_legacy_record_with_a_pipeline_state_loads_as_draft() {
        let json = r#"{
            "id": "abc123",
            "name": "Habits",
            "brief": "Track daily habits",
            "createdAtMs": 1,
            "updatedAtMs": 2,
            "workflowState": "awaiting_preview_confirmation",
            "workspaceRel": "apps/abc123/workspace"
        }"#;
        let record: AppRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.workflow_state, AppWorkflowState::Draft);
        assert!(record.git_enabled, "missing gitEnabled defaults to true");
    }
}
