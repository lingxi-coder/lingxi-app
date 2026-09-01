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
pub const APPS_SCHEMA_VERSION: u32 = 4;

/// New apps use Git-backed source version control unless the user opts out
/// during creation. Missing values on older records deserialize as enabled.
pub const DEFAULT_GIT_VERSION_CONTROL: bool = true;

fn default_git_version_control() -> bool {
    DEFAULT_GIT_VERSION_CONTROL
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

/// Workspace dependency install state for one app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDependencyState {
    /// The workspace exists but its app-local `node_modules` have not been
    /// prepared yet.
    Queued,
    /// A host-owned install task is currently preparing `node_modules`.
    Installing,
    /// The app-local dependency tree is ready for use.
    Ready,
    /// The last install attempt failed; `last_error` explains why.
    Failed,
}

impl AppDependencyState {
    /// Canonical `snake_case` name (the persisted/wire value).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Installing => "installing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for AppDependencyState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One fixed runtime family from the global local-app catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppRuntimeProfile {
    /// Routed Ionic/React DOM application scaffold.
    #[serde(rename = "react_dom")]
    ReactDom,
    /// Canvas 2D drawn-surface scaffold.
    #[serde(rename = "canvas_2d")]
    Canvas2d,
    /// Three.js 3D drawn-surface scaffold.
    #[serde(rename = "three_3d")]
    Three3d,
    /// Phaser 2D drawn-surface scaffold.
    #[serde(rename = "phaser_2d")]
    Phaser2d,
    /// Babylon.js 3D drawn-surface scaffold.
    #[serde(rename = "babylon_3d")]
    Babylon3d,
}

impl AppRuntimeProfile {
    /// Canonical `snake_case` persisted/wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReactDom => "react_dom",
            Self::Canvas2d => "canvas_2d",
            Self::Three3d => "three_3d",
            Self::Phaser2d => "phaser_2d",
            Self::Babylon3d => "babylon_3d",
        }
    }

    /// Parse the persisted or tool spelling of one runtime profile family.
    pub fn parse(value: &str) -> Result<Self, crate::error::AppError> {
        match value {
            "react_dom" => Ok(Self::ReactDom),
            "canvas_2d" => Ok(Self::Canvas2d),
            "three_3d" => Ok(Self::Three3d),
            "phaser_2d" => Ok(Self::Phaser2d),
            "babylon_3d" => Ok(Self::Babylon3d),
            other => Err(crate::error::AppError::InvalidRequest(format!(
                "unknown app runtime profile {other:?}; expected react_dom, canvas_2d, three_3d, phaser_2d, or babylon_3d"
            ))),
        }
    }
}

impl fmt::Display for AppRuntimeProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One app as listed in `apps/index.json` (and mirrored into the app's
/// `workspace/.lingxi/app.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRecord {
    /// Stable app id matching `^[a-z0-9][a-z0-9-]{0,53}$`.
    pub id: String,
    /// User-facing display name.
    pub name: String,
    /// One-line description the user gave at creation time. The agent reads
    /// it for context; the list page displays it. Stored ONCE — a second
    /// copy would inevitably drift.
    pub brief: String,
    /// Provider-qualified model selected for app-creation workflows. The
    /// mobile workflow launcher reads this from the app-scoped metadata file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_model: Option<String>,
    /// Whether Git controls this app's source checkpoints and restores.
    #[serde(default = "default_git_version_control")]
    pub git_enabled: bool,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: u64,
    /// Last mutation time, epoch milliseconds.
    pub updated_at_ms: u64,
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
    /// 工作区里是否已经落下脚手架。
    ///
    /// 每一条新记录都显式写入；缺字段是无效的旧 store（§A.1），不是空壳判据。
    ///
    /// 三个写入点，缺一不可：
    ///   1. `CreateMode::Shell` 在构造记录时写 `false`；
    ///   2. `CreateMode::Scaffolded`（`LocalAppCreate` 的 create+scaffold 路径）
    ///      在构造记录时写 `true`；
    ///   3. `LocalAppScaffold` 的提交点把 `false` 翻成 `true`。
    ///
    /// ⛔ 不要加 `#[serde(default)]`：缺字段必须加载失败并提示清除开发数据，
    /// 而不是静默变成一个可以被 `LocalAppScaffold` 清空的 shell。
    pub scaffolded: bool,
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
    /// Legacy persisted mode from builds that used a framework server. New
    /// runtimes always write [`Self::StaticExport`].
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

/// Per-app dependency-install record persisted at `apps/<id>/dependencies.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDependencyRecord {
    /// Persisted schema version ([`APPS_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// App the record belongs to.
    pub app_id: String,
    /// Current dependency install state.
    pub state: AppDependencyState,
    /// SHA-256 of the host-managed dependency lockfile used for the install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockfile_sha256: Option<String>,
    /// Toolchain identity (for example `pnpm@11.22.0/node@24.18.1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain_key: Option<String>,
    /// Monotonic count of attempted installs.
    #[serde(default)]
    pub install_attempts: u32,
    /// Last install failure, if any.
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
            serde_json::to_string(&AppRuntimeState::Stopped).unwrap(),
            "\"stopped\""
        );
        assert_eq!(
            serde_json::to_string(&AppDependencyState::Installing).unwrap(),
            "\"installing\""
        );
        assert_eq!(
            serde_json::to_string(&AppRuntimeProfile::Babylon3d).unwrap(),
            "\"babylon_3d\""
        );
        assert_eq!(
            serde_json::to_string(&AppCheckpointKind::PreRestore).unwrap(),
            "\"pre_restore\""
        );
    }

    #[test]
    fn record_serializes_camel_case_and_omits_absent_conversation() {
        let record = AppRecord {
            id: "abc123".into(),
            name: "Habits".into(),
            brief: "Track daily habits".into(),
            workflow_model: None,
            git_enabled: true,
            scaffolded: true,
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_001,
            conversation_id: None,
            init_session_id: None,
            workspace_rel: "apps/abc123/workspace".into(),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"createdAtMs\":1700000000000"));
        assert!(json.contains("\"workspaceRel\":\"apps/abc123/workspace\""));
        assert!(json.contains("\"brief\":\"Track daily habits\""));
        assert!(json.contains("\"gitEnabled\":true"));
        assert!(json.contains("\"scaffolded\":true"));
        assert!(!json.contains("conversationId"));
        let back: AppRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn a_legacy_record_without_git_enabled_still_defaults_true() {
        let json = r#"{
            "id": "abc123",
            "name": "Habits",
            "brief": "Track daily habits",
            "scaffolded": true,
            "createdAtMs": 1,
            "updatedAtMs": 2,
            "workspaceRel": "apps/abc123/workspace"
        }"#;
        let record: AppRecord = serde_json::from_str(json).unwrap();
        assert!(record.git_enabled, "missing gitEnabled defaults to true");
    }
}
