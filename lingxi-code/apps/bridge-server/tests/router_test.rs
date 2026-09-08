//! F2-08 — Full command/event routing in `bridge-server`.
//!
//! The walking skeleton (F2-05/F2-06) routed only the turn + permission path.
//! This file proves the FULL `ClientCommand` surface reaches its engine entry
//! and that each pull produces the correct framed reply — exercised at the
//! routing seam ([`bridge_server::router::CommandRouter`]) the connection loop
//! delegates non-turn/non-permission commands to.
//!
//! The seam mirrors the proven [`bridge_server::server::TurnDriver`] shape: the
//! production server binds an [`bridge_server::router::EngineCommandRouter`]
//! wrapping the real engine handles (`OrchestratorHandle`, `AuthHandle`,
//! `TaskRegistryHandle`); these tests bind the SAME `EngineCommandRouter` over
//! the engine's own `MockOrchestratorHandle` / mock task + auth handles, so the
//! routing-and-lowering path under test is the production one (no test-only
//! router shim) — only the engine handles are doubles.
//!
//! Tests (plan §2):
//! - `set_model_routes` — `SetModel` → `switch_model` → `ModelChanged`.
//! - `list_models_routes` — `ListModels` → `list_available_models` → `ModelList`.
//! - `list_mcp_routes` — `RefreshListings{Mcp}` → `list_mcp_servers` → `McpServers`.
//! - `slash_command_routes_to_registry` — `RunSlashCommand` reaches the dispatcher.
//! - `task_list_poll_emits_task_row` — the adapter's task poll loop emits a
//!   `TaskRow` per task from `TaskRegistryHandle::list`.
//! - `task_list_command_emits_task_rows` — `TaskList` → `list` → one `TaskRow` each.
//! - `clear_session_rejected_mid_turn` — `ClearSession` is refused with an
//!   `Error` while a turn is in flight, and routed once the turn ends.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::mcp_bridge::McpPaths;
use bridge_server::router::{CommandRouter, EngineCommandRouter, SessionStoreContext};
use bridge_server::server::BridgeConnection;
use bridge_server::settings_bridge::{SettingsContext, SettingsPaths};
use client_adapter::{AdapterPermissionGate, ClientEventSink, PermissionRequestSink};
use client_protocol::commands::{
    ClientCommand, ListingKindDto, McpScopeDto, PermissionBehaviorDto, ProviderCredentialSecretDto,
    SettingsDestinationDto,
};
use client_protocol::events::{ClientEvent, ErrorKindDto};
use client_protocol::listings::SessionModeDto;
use client_protocol::permission::PermissionRequest;
use futures_util::{SinkExt, StreamExt};
use orchestrator::test_support::MockOrchestratorHandle;
use platform_api::auth::{AuthError, AuthHandle, LoginInfo};
use platform_api::orchestrator::{
    AgentInfo, CompactionSummary, CostSnapshot, DoctorReport, HandleError, HookInfo, McpServerInfo,
    McpStatus, MemoryEditorOutcome, SkillInfo, StatusSnapshot,
};
use platform_api::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};
use platform_api::{OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult};
use platform_posix::{PlainTextSecureStorage, PosixClock, PosixFileSystem, PosixHttp};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;

#[path = "router_test/compact_shutdown.rs"]
mod compact_shutdown;

// ── Test sink ───────────────────────────────────────────────────────────────

/// A [`ClientEventSink`] that captures every emitted event in emission order so
/// a routing test can assert the reply set.
#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<ClientEvent>>,
}

#[derive(Default)]
struct SelectiveFailureStorage {
    values:
        std::sync::Mutex<std::collections::HashMap<(String, String), protocol::SecureStorageData>>,
}

#[async_trait]
impl platform_api::SecureStorage for SelectiveFailureStorage {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: protocol::SecureStorageData,
    ) -> Result<(), platform_api::SecureStorageError> {
        self.values
            .lock()
            .unwrap()
            .insert((service.to_string(), account.to_string()), data);
        Ok(())
    }

    async fn retrieve(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<protocol::SecureStorageData>, platform_api::SecureStorageError> {
        if account == "provider-key-openrouter" {
            return Err(platform_api::SecureStorageError::PermissionDenied(
                "test keychain denial".to_string(),
            ));
        }
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(&(service.to_string(), account.to_string()))
            .cloned())
    }

    async fn delete(
        &self,
        service: &str,
        account: &str,
    ) -> Result<(), platform_api::SecureStorageError> {
        self.values
            .lock()
            .unwrap()
            .remove(&(service.to_string(), account.to_string()));
        Ok(())
    }

    async fn list(&self, service: &str) -> Result<Vec<String>, platform_api::SecureStorageError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .keys()
            .filter(|(stored_service, _)| stored_service == service)
            .map(|(_, account)| account.clone())
            .collect())
    }

    fn is_encrypted(&self) -> bool {
        true
    }

    fn backend(&self) -> platform_api::SecureStorageBackend {
        platform_api::SecureStorageBackend::MacOsKeychain
    }
}

impl CapturingSink {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
    async fn events(&self) -> Vec<ClientEvent> {
        self.events.lock().await.clone()
    }
}

#[async_trait]
impl ClientEventSink for CapturingSink {
    async fn emit(&self, event: ClientEvent) {
        self.events.lock().await.push(event);
    }
}

// ── Mock auth + task handles ─────────────────────────────────────────────────

/// Auth double — `current_user` returns a fixed signed-in user.
struct MockAuth;

#[async_trait]
impl AuthHandle for MockAuth {
    async fn login(&self) -> Result<LoginInfo, AuthError> {
        Ok(LoginInfo {
            email: "u@x.com".into(),
            org_id: "org_1".into(),
        })
    }
    async fn logout(&self) -> Result<(), AuthError> {
        Ok(())
    }
    async fn current_user(&self) -> Option<LoginInfo> {
        Some(LoginInfo {
            email: "u@x.com".into(),
            org_id: "org_1".into(),
        })
    }
}

/// Task-registry double — `list` returns the pre-loaded rows; other CRUD is a
/// no-op success default.
struct MockTaskRegistry {
    rows: Vec<TaskRecord>,
}

/// Minimal handle that records the history adopted by `ResumeSession`.
struct ResumingHandle {
    session_id: protocol::SessionId,
    resumed: Mutex<
        Option<(
            protocol::SessionId,
            Vec<protocol::ConversationMessage>,
            Option<String>,
            Option<platform_api::ActiveGoalSnapshot>,
            platform_api::ResumeRuntimeSnapshot,
        )>,
    >,
    resume_context: Mutex<Option<(Option<String>, bool)>>,
    permission_mode: Mutex<Option<String>>,
    plan_mode: Mutex<bool>,
    operation_log: Mutex<Vec<String>>,
    resume_error: Mutex<Option<String>>,
    permission_mode_error: Mutex<Option<String>>,
    plan_mode_error: Mutex<Option<String>>,
}

impl ResumingHandle {
    fn new() -> Self {
        Self {
            session_id: protocol::SessionId::new(),
            resumed: Mutex::new(None),
            resume_context: Mutex::new(None),
            permission_mode: Mutex::new(Some("default".to_string())),
            plan_mode: Mutex::new(false),
            operation_log: Mutex::new(Vec::new()),
            resume_error: Mutex::new(None),
            permission_mode_error: Mutex::new(None),
            plan_mode_error: Mutex::new(None),
        }
    }

    async fn set_current_permission_mode(&self, mode: &str) {
        *self.permission_mode.lock().await = Some(mode.to_string());
    }

    async fn set_current_plan_mode(&self, on: bool) {
        *self.plan_mode.lock().await = on;
    }

    async fn set_resume_error(&self, reason: &str) {
        *self.resume_error.lock().await = Some(reason.to_string());
    }

    async fn set_plan_mode_error(&self, reason: &str) {
        *self.plan_mode_error.lock().await = Some(reason.to_string());
    }

    async fn set_permission_mode_error(&self, reason: &str) {
        *self.permission_mode_error.lock().await = Some(reason.to_string());
    }

    async fn operation_log(&self) -> Vec<String> {
        self.operation_log.lock().await.clone()
    }

    async fn current_permission_mode(&self) -> Option<String> {
        self.permission_mode.lock().await.clone()
    }

    async fn current_plan_mode(&self) -> bool {
        *self.plan_mode.lock().await
    }
}

#[async_trait]
impl platform_api::OrchestratorHandle for ResumingHandle {
    async fn current_session_id(&self) -> protocol::SessionId {
        self.session_id
    }
    async fn clear_session(&self) -> Result<(), HandleError> {
        Ok(())
    }
    async fn force_compact(&self) -> Result<CompactionSummary, HandleError> {
        Ok(CompactionSummary::default())
    }
    async fn snapshot_cost(&self) -> CostSnapshot {
        CostSnapshot::default()
    }
    async fn switch_model(&self, _model: &str, _profile: Option<&str>) -> Result<(), HandleError> {
        Ok(())
    }
    async fn request_exit(&self) {}
    async fn current_should_exit(&self) -> bool {
        false
    }
    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError> {
        Err(HandleError::Unimplemented("test".into()))
    }
    async fn list_mcp_servers(&self) -> Vec<McpServerInfo> {
        Vec::new()
    }
    async fn list_skills(&self) -> Vec<SkillInfo> {
        Vec::new()
    }
    async fn list_hooks(&self) -> Vec<HookInfo> {
        Vec::new()
    }
    async fn list_agents(&self) -> Vec<AgentInfo> {
        Vec::new()
    }
    async fn run_doctor_checks(&self) -> DoctorReport {
        DoctorReport::default()
    }
    async fn get_status_snapshot(&self) -> StatusSnapshot {
        let resumed = self.resumed.lock().await;
        let Some((_, _, _, _, runtime)) = resumed.as_ref() else {
            return StatusSnapshot::default();
        };
        StatusSnapshot {
            model: runtime.model.clone(),
            model_profile: runtime.model_profile.clone(),
            ..StatusSnapshot::default()
        }
    }
    async fn edit_config_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
        Err(HandleError::Unimplemented("test".into()))
    }
    async fn edit_permissions_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
        Err(HandleError::Unimplemented("test".into()))
    }
    async fn list_available_models(&self) -> Vec<String> {
        Vec::new()
    }
    async fn plan_mode(&self) -> bool {
        *self.plan_mode.lock().await
    }
    async fn set_plan_mode(&self, on: bool) -> Result<(), HandleError> {
        self.operation_log
            .lock()
            .await
            .push(format!("set_plan_mode:{on}"));
        if let Some(reason) = self.plan_mode_error.lock().await.take() {
            return Err(HandleError::ActionFailed(reason));
        }
        *self.plan_mode.lock().await = on;
        Ok(())
    }
    async fn permission_mode(&self) -> Option<String> {
        self.permission_mode.lock().await.clone()
    }
    async fn set_permission_mode(&self, mode: &str) -> Result<(), HandleError> {
        self.operation_log
            .lock()
            .await
            .push(format!("set_permission_mode:{mode}"));
        if let Some(reason) = self.permission_mode_error.lock().await.take() {
            return Err(HandleError::ActionFailed(reason));
        }
        *self.permission_mode.lock().await = Some(mode.to_string());
        Ok(())
    }
    async fn resume_session(
        &self,
        session_id: protocol::SessionId,
        history: Vec<protocol::ConversationMessage>,
        last_jsonl_uuid: Option<String>,
        active_goal: Option<platform_api::ActiveGoalSnapshot>,
        runtime: platform_api::ResumeRuntimeSnapshot,
    ) -> Result<(), HandleError> {
        self.operation_log
            .lock()
            .await
            .push("resume_session".to_string());
        let permission_mode = self.permission_mode.lock().await.clone();
        let plan_mode = *self.plan_mode.lock().await;
        *self.resume_context.lock().await = Some((permission_mode, plan_mode));
        if let Some(reason) = self.resume_error.lock().await.take() {
            return Err(HandleError::ActionFailed(reason));
        }
        *self.resumed.lock().await =
            Some((session_id, history, last_jsonl_uuid, active_goal, runtime));
        Ok(())
    }
}

#[async_trait]
impl TaskRegistryHandle for MockTaskRegistry {
    async fn create(&self, _input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
    async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
        Ok(None)
    }
    async fn list(&self, _filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
        Ok(self.rows.clone())
    }
    async fn update(
        &self,
        _id: &str,
        _patch: TaskUpdatePatch,
    ) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
    async fn set_status(&self, _id: &str, _status: &str) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
    async fn kill(&self, id: &str) -> Result<TaskRecord, TaskRegistryError> {
        Ok(TaskRecord {
            task_id: id.to_string(),
            task_type: "local_bash".into(),
            status: "killed".into(),
            description: "stopped".into(),
            command: None,
            ..Default::default()
        })
    }
    async fn output(
        &self,
        id: &str,
        _offset: Option<u64>,
    ) -> Result<TaskOutputChunk, TaskRegistryError> {
        Ok(TaskOutputChunk {
            task_id: id.to_string(),
            content: "line1\nline2".into(),
            total_lines: 2,
            truncated: false,
            ..Default::default()
        })
    }
}

// ── Router fixtures ───────────────────────────────────────────────────────────

fn router_with(
    handle: Arc<MockOrchestratorHandle>,
    tasks: Arc<MockTaskRegistry>,
) -> EngineCommandRouter {
    EngineCommandRouter::new(
        handle as Arc<dyn platform_api::orchestrator::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        tasks as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    )
}

async fn router_with_credentials() -> (
    EngineCommandRouter,
    Arc<secret::CredentialManager>,
    tempfile::TempDir,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let storage = Arc::new(
        PlainTextSecureStorage::new(temp.path().join("credentials"))
            .await
            .expect("storage"),
    );
    let credentials = Arc::new(secret::CredentialManager::new(
        storage,
        Arc::new(PosixClock::new()),
        Arc::new(PosixHttp::new()),
    ));
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: Vec::new() }),
    )
    .with_credentials(credentials.clone());
    (router, credentials, temp)
}

fn router_with_store(
    handle: Arc<MockOrchestratorHandle>,
    root: &std::path::Path,
) -> EngineCommandRouter {
    let cwd = root.to_string_lossy().into_owned();
    EngineCommandRouter::new(
        handle as Arc<dyn platform_api::orchestrator::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    )
    .with_session_store(SessionStoreContext::new(
        root.join(".lingxi"),
        cwd,
        Arc::new(PosixFileSystem::new(root.to_path_buf())),
    ))
}

fn seed_session_file(root: &std::path::Path) -> String {
    seed_session_file_with_ids(
        root,
        "11111111-2222-4333-8444-555555555555",
        "aaaaaaaa-2222-4333-8444-555555555555",
        "prior session",
    )
}

fn seed_session_file_with_ids(
    root: &std::path::Path,
    session_id: &str,
    message_id: &str,
    prompt: &str,
) -> String {
    let cwd = root.to_string_lossy().into_owned();
    let lingxi_home = root.join(".lingxi");
    let project_dir = lingxi_home
        .join("projects")
        .join(session::jsonl::project_dir_name(&cwd));
    std::fs::create_dir_all(&project_dir).unwrap();
    let uuid = session_id.to_string();
    let line = serde_json::json!({
        "type": "user",
        "uuid": message_id,
        "parentUuid": serde_json::Value::Null,
        "sessionId": uuid,
        "timestamp": "2026-05-25T12:00:00.000Z",
        "cwd": cwd,
        "version": "0.9.0",
        "isSidechain": false,
        "userType": "external",
        "message": {"role": "user", "content": prompt}
    });
    std::fs::write(
        project_dir.join(format!("{uuid}.jsonl")),
        format!("{}\n", serde_json::to_string(&line).unwrap()),
    )
    .unwrap();
    uuid
}

fn seed_replay_session(root: &std::path::Path) -> String {
    let cwd = root.to_string_lossy().into_owned();
    let project_dir = root
        .join(".lingxi")
        .join("projects")
        .join(session::jsonl::project_dir_name(&cwd));
    std::fs::create_dir_all(&project_dir).unwrap();
    let session_id = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa".to_string();
    let user_id = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";
    let assistant_id = "cccccccc-3333-4333-8333-cccccccccccc";
    let boundary_id = "dddddddd-4444-4444-8444-dddddddddddd";
    // Persist the real post-compaction chain shape: the boundary resets the
    // parent chain, the transcript-only summary chains from it, and the next
    // assistant response becomes the resumable tip.
    let boundary = serde_json::json!({
        "type": "system",
        "subtype": "compact_boundary",
        "uuid": boundary_id,
        "parentUuid": serde_json::Value::Null,
        "logicalParentUuid": serde_json::Value::Null,
        "sessionId": session_id,
        "timestamp": "2026-05-25T12:00:00.000Z",
        "cwd": cwd,
        "version": "0.9.0",
        "isSidechain": false,
        "content": "Conversation compacted",
        "level": "info",
        "compactMetadata": {
            "cumulativeDroppedTokens": 4321,
            "preCompactDiscoveredTools": ["DeferredTool"]
        }
    });
    let user = serde_json::json!({
        "type": "user",
        "uuid": user_id,
        "parentUuid": boundary_id,
        "sessionId": session_id,
        "timestamp": "2026-05-25T12:00:01.000Z",
        "cwd": cwd,
        "version": "0.9.0",
        "isSidechain": false,
        "userType": "external",
        "isCompactSummary": true,
        "isVisibleInTranscriptOnly": true,
        "message": {"role": "user", "content": "resume from disk"}
    });
    let assistant = serde_json::json!({
        "type": "assistant",
        "uuid": assistant_id,
        "parentUuid": user_id,
        "sessionId": session_id,
        "timestamp": "2026-05-25T12:00:02.000Z",
        "cwd": cwd,
        "version": "0.9.0",
        "isSidechain": false,
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "restored"}],
            "model": "claude-opus-4-1"
        },
        "effort": "high"
    });
    std::fs::write(
        project_dir.join(format!("{session_id}.jsonl")),
        format!("{boundary}\n{user}\n{assistant}\n"),
    )
    .unwrap();
    session_id
}

fn seed_plan_replay_session(root: &std::path::Path) -> String {
    let cwd = root.to_string_lossy().into_owned();
    let project_dir = root
        .join(".lingxi")
        .join("projects")
        .join(session::jsonl::project_dir_name(&cwd));
    std::fs::create_dir_all(&project_dir).unwrap();
    let session_id = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa".to_string();
    let boundary_id = "dddddddd-4444-4444-8444-dddddddddddd";
    let summary_id = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";
    let assistant_id = "cccccccc-3333-4333-8333-cccccccccccc";
    let user_id = "eeeeeeee-5555-4555-8555-eeeeeeeeeeee";
    let boundary = serde_json::json!({
        "type": "system",
        "subtype": "compact_boundary",
        "uuid": boundary_id,
        "parentUuid": serde_json::Value::Null,
        "logicalParentUuid": serde_json::Value::Null,
        "sessionId": session_id,
        "timestamp": "2026-05-25T12:00:00.000Z",
        "cwd": cwd,
        "version": "0.9.0",
        "isSidechain": false,
        "content": "Conversation compacted",
        "level": "info",
        "compactMetadata": {
            "cumulativeDroppedTokens": 4321,
            "preCompactDiscoveredTools": ["DeferredTool"]
        }
    });
    let summary = serde_json::json!({
        "type": "user",
        "uuid": summary_id,
        "parentUuid": boundary_id,
        "sessionId": session_id,
        "timestamp": "2026-05-25T12:00:01.000Z",
        "cwd": cwd,
        "version": "0.9.0",
        "isSidechain": false,
        "userType": "external",
        "isCompactSummary": true,
        "isVisibleInTranscriptOnly": true,
        "message": {"role": "user", "content": "resume from disk"}
    });
    let assistant = serde_json::json!({
        "type": "assistant",
        "uuid": assistant_id,
        "parentUuid": summary_id,
        "sessionId": session_id,
        "timestamp": "2026-05-25T12:00:02.000Z",
        "cwd": cwd,
        "version": "0.9.0",
        "isSidechain": false,
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "restored"}],
            "model": "claude-opus-4-1"
        },
        "effort": "high"
    });
    let user = serde_json::json!({
        "type": "user",
        "uuid": user_id,
        "parentUuid": assistant_id,
        "sessionId": session_id,
        "timestamp": "2026-08-25T12:00:03.000Z",
        "cwd": cwd,
        "version": "0.12.0",
        "isSidechain": false,
        "permissionMode": "plan",
        "message": {"role": "user", "content": "keep planning"}
    });
    std::fs::write(
        project_dir.join(format!("{session_id}.jsonl")),
        format!("{boundary}\n{summary}\n{assistant}\n{user}\n"),
    )
    .unwrap();
    session_id
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn provider_credentials_round_trip_through_shared_engine_store() {
    let (router, credentials, _temp) = router_with_credentials().await;
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::SetProviderCredential {
                operation_id: 1,
                provider_id: "deepseek".into(),
                credential: ProviderCredentialSecretDto::new("sk-shared-secret".into()),
            },
            sink.clone(),
        )
        .await;

    let stored = credentials
        .get_provider_key("deepseek")
        .await
        .expect("read")
        .expect("stored");
    assert_eq!(stored.expose_secret(), "sk-shared-secret");
    assert_eq!(
        sink.events().await.last(),
        Some(&ClientEvent::ProviderCredentialStatus {
            operation_id: 1,
            configured_provider_ids: vec!["deepseek".into()],
            unavailable_provider_ids: Vec::new(),
            storage_encrypted: false,
            credential_previews: std::collections::HashMap::from([(
                "deepseek".into(),
                "••••cret".into(),
            )]),
            error: None,
        })
    );

    router
        .route(
            ClientCommand::DeleteProviderCredential {
                operation_id: 2,
                provider_id: "deepseek".into(),
            },
            sink.clone(),
        )
        .await;

    assert!(credentials
        .get_provider_key("deepseek")
        .await
        .expect("read after delete")
        .is_none());
    assert_eq!(
        sink.events().await.last(),
        Some(&ClientEvent::ProviderCredentialStatus {
            operation_id: 2,
            configured_provider_ids: Vec::new(),
            unavailable_provider_ids: Vec::new(),
            storage_encrypted: false,
            credential_previews: std::collections::HashMap::new(),
            error: None,
        })
    );

    router
        .route(
            ClientCommand::SetProviderCredential {
                operation_id: 3,
                provider_id: "anthropic".into(),
                credential: ProviderCredentialSecretDto::new("sk-ant-shared-secret".into()),
            },
            sink.clone(),
        )
        .await;
    let anthropic = credentials
        .get_anthropic_api_key()
        .await
        .expect("read Anthropic key")
        .expect("stored Anthropic key");
    assert_eq!(anthropic.expose_secret(), "sk-ant-shared-secret");

    router
        .route(
            ClientCommand::DeleteProviderCredential {
                operation_id: 4,
                provider_id: "anthropic".into(),
            },
            sink,
        )
        .await;
    assert!(credentials
        .get_anthropic_api_key()
        .await
        .expect("read Anthropic key after delete")
        .is_none());
}

#[tokio::test]
async fn externally_owned_provider_credentials_stay_process_local() {
    let (router, credentials, _temp) = router_with_credentials().await;
    let router = router.with_ephemeral_provider_credentials(true);
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::SetProviderCredential {
                operation_id: 41,
                provider_id: "openrouter".into(),
                credential: ProviderCredentialSecretDto::new("or-session-secret".into()),
            },
            sink.clone(),
        )
        .await;

    assert_eq!(
        credentials
            .get_provider_key("openrouter")
            .await
            .expect("read process cache")
            .expect("cached key")
            .expose_secret(),
        "or-session-secret"
    );
    credentials
        .delete_provider_key_ephemeral("openrouter")
        .await;
    assert!(
        credentials
            .get_provider_key("openrouter")
            .await
            .expect("inspect persistent fallback")
            .is_none(),
        "the packaged Desktop route must not persist the broker-owned secret"
    );

    router
        .route(
            ClientCommand::SetProviderCredential {
                operation_id: 42,
                provider_id: "openrouter".into(),
                credential: ProviderCredentialSecretDto::new("replacement-secret".into()),
            },
            sink.clone(),
        )
        .await;
    router
        .route(
            ClientCommand::DeleteProviderCredential {
                operation_id: 43,
                provider_id: "openrouter".into(),
            },
            sink,
        )
        .await;
    assert!(credentials
        .get_provider_key("openrouter")
        .await
        .expect("read after process-cache delete")
        .is_none());
}

#[tokio::test]
async fn provider_credential_listing_reports_partial_success_without_erasing_unknown_state() {
    let credentials = Arc::new(secret::CredentialManager::new(
        Arc::new(SelectiveFailureStorage::default()),
        Arc::new(PosixClock::new()),
        Arc::new(PosixHttp::new()),
    ));
    credentials
        .set_provider_key("deepseek", "sk-deepseek")
        .await
        .expect("seed readable credential");
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: Vec::new() }),
    )
    .with_credentials(credentials);
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::ListProviderCredentials {
                operation_id: 9,
                provider_ids: vec!["deepseek".into(), "openrouter".into()],
                preview_provider_ids: vec!["deepseek".into()],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let ClientEvent::ProviderCredentialStatus {
        configured_provider_ids,
        unavailable_provider_ids,
        storage_encrypted,
        credential_previews,
        error,
        ..
    } = &events[0]
    else {
        panic!("expected provider status event")
    };
    assert_eq!(configured_provider_ids, &["deepseek"]);
    assert_eq!(unavailable_provider_ids, &["openrouter"]);
    assert!(*storage_encrypted);
    assert_eq!(
        credential_previews.get("deepseek").map(String::as_str),
        Some("••••seek")
    );
    assert!(error
        .as_deref()
        .is_some_and(|message| message.contains("openrouter")));
}

#[tokio::test]
async fn set_model_routes() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::SetModel {
                model: "claude-opus-4-8".into(),
            },
            sink.clone(),
        )
        .await;

    // The command reached the engine handle.
    assert_eq!(handle.switch_model_call_count(), 1);
    assert_eq!(
        handle.last_switched_model().as_deref(),
        Some("claude-opus-4-8")
    );

    // …and the reply is a `ModelChanged` carrying the new model.
    let events = sink.events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        ClientEvent::ModelChanged {
            model: "claude-opus-4-8".into()
        }
    );
}

#[tokio::test]
async fn set_fast_mode_routes_and_acknowledges_authoritative_state() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(ClientCommand::SetFastMode { enabled: true }, sink.clone())
        .await;

    assert!(handle.current_fast_mode());
    assert_eq!(
        sink.events().await,
        vec![ClientEvent::FastModeChanged { enabled: true }]
    );
}

#[tokio::test]
async fn set_model_keeps_provider_in_acknowledgement() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_model_listings(vec![platform_api::ModelListing {
        display_model: "GPT-5.5".into(),
        request_model: "gpt-5.5".into(),
        provider_id: "github-copilot".into(),
        provider_label: "GitHub Copilot".into(),
        description: None,
        metadata: Default::default(),
        capabilities: Default::default(),
        reasoning: Default::default(),
        supports_reasoning: true,
    }]);
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::SetModel {
                model: "github-copilot/gpt-5.5".into(),
            },
            sink.clone(),
        )
        .await;

    assert_eq!(handle.last_switched_model().as_deref(), Some("gpt-5.5"));
    assert_eq!(
        sink.events().await,
        vec![ClientEvent::ModelChanged {
            model: "github-copilot/gpt-5.5".into(),
        }]
    );
}

#[tokio::test]
async fn set_permission_mode_routes_and_acknowledges_authoritative_mode() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::SetPermissionMode {
                mode: "acceptEdits".into(),
            },
            sink.clone(),
        )
        .await;

    assert_eq!(
        handle.current_permission_mode().as_deref(),
        Some("acceptEdits")
    );
    assert_eq!(
        sink.events().await,
        vec![ClientEvent::PermissionModeChanged {
            mode: "acceptEdits".into(),
        }]
    );
}

#[tokio::test]
async fn set_permission_mode_is_rejected_while_a_turn_is_active() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();
    router.set_turn_active(true);

    router
        .route(
            ClientCommand::SetPermissionMode {
                mode: "acceptEdits".into(),
            },
            sink.clone(),
        )
        .await;

    assert_eq!(handle.current_permission_mode().as_deref(), Some("default"));
    assert_eq!(
        sink.events().await,
        vec![ClientEvent::Error {
            kind: ErrorKindDto::Rejected,
            message: "cannot change permission mode while a turn is active".into(),
        }]
    );
}

#[tokio::test]
async fn set_permission_mode_rejection_is_not_reported_as_internal() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_permission_mode_error("bypassPermissions is disabled".into());
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::SetPermissionMode {
                mode: "bypassPermissions".into(),
            },
            sink.clone(),
        )
        .await;

    assert_eq!(handle.current_permission_mode().as_deref(), Some("default"));
    assert_eq!(
        sink.events().await,
        vec![ClientEvent::Error {
            kind: ErrorKindDto::Rejected,
            message:
                "set_permission_mode failed: handle action failed: bypassPermissions is disabled"
                    .into(),
        }]
    );
}

#[tokio::test]
async fn list_models_routes() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_available_models(vec!["a".into(), "b".into()]);
    handle.set_status_snapshot(StatusSnapshot {
        model: "a".into(),
        ..StatusSnapshot::default()
    });
    let router = router_with(handle, Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router.route(ClientCommand::ListModels, sink.clone()).await;

    let events = sink.events().await;
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0],
        ClientEvent::ProviderModelCatalog {
            providers: Vec::new(),
        }
    );
    assert_eq!(
        events[1],
        ClientEvent::ModelList {
            models: vec!["a".into(), "b".into()],
            current: "a".into(),
            details: Vec::new(),
        }
    );
}

#[tokio::test]
async fn list_models_curates_and_preserves_provider_identity() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_available_models(vec!["gpt-5.6-sol".into(), "gpt-4o".into()]);
    handle.set_model_listings(vec![
        platform_api::ModelListing {
            display_model: "GPT-5.6 Sol".into(),
            request_model: "gpt-5.6-sol".into(),
            provider_id: "openai".into(),
            provider_label: "OpenAI".into(),
            description: None,
            metadata: Default::default(),
            capabilities: platform_api::ModelCapabilities {
                tools: true,
                ..Default::default()
            },
            reasoning: Default::default(),
            supports_reasoning: true,
        },
        platform_api::ModelListing {
            display_model: "GPT-4o".into(),
            request_model: "gpt-4o".into(),
            provider_id: "openai".into(),
            provider_label: "OpenAI".into(),
            description: None,
            metadata: Default::default(),
            capabilities: platform_api::ModelCapabilities {
                tools: true,
                ..Default::default()
            },
            reasoning: Default::default(),
            supports_reasoning: false,
        },
        platform_api::ModelListing {
            display_model: "GPT-5.6 Sol".into(),
            request_model: "gpt-5.6-sol".into(),
            provider_id: "github-copilot".into(),
            provider_label: "GitHub Copilot".into(),
            description: None,
            metadata: Default::default(),
            capabilities: platform_api::ModelCapabilities {
                tools: true,
                ..Default::default()
            },
            reasoning: Default::default(),
            supports_reasoning: true,
        },
    ]);
    handle.set_status_snapshot(StatusSnapshot {
        model: "gpt-5.6-sol".into(),
        model_profile: Some("github-copilot".into()),
        ..StatusSnapshot::default()
    });
    let router = router_with(handle, Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router.route(ClientCommand::ListModels, sink.clone()).await;

    let events = sink.events().await;
    assert_eq!(events.len(), 2);
    let ClientEvent::ProviderModelCatalog { providers } = &events[0] else {
        panic!("expected provider model catalog event");
    };
    assert_eq!(
        providers
            .iter()
            .map(|provider| provider.provider_id.as_str())
            .collect::<Vec<_>>(),
        vec!["openai", "github-copilot"]
    );
    let ClientEvent::ModelList {
        models,
        current,
        details,
    } = &events[1]
    else {
        panic!("expected model list event");
    };
    assert_eq!(
        models,
        &["github-copilot/gpt-5.6-sol", "openai/gpt-5.6-sol"]
    );
    assert_eq!(current, "github-copilot/gpt-5.6-sol");
    assert_eq!(
        details
            .iter()
            .map(|detail| detail.reference.as_str())
            .collect::<Vec<_>>(),
        vec!["github-copilot/gpt-5.6-sol", "openai/gpt-5.6-sol"]
    );
}

#[tokio::test]
async fn list_models_uses_full_provider_catalog_when_provided() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_available_models(vec!["deepseek-v4-flash".into()]);
    handle.set_model_listings(vec![platform_api::ModelListing {
        display_model: "DeepSeek V4 Flash".into(),
        request_model: "deepseek-v4-flash".into(),
        provider_id: "deepseek".into(),
        provider_label: "DeepSeek".into(),
        description: None,
        metadata: Default::default(),
        capabilities: platform_api::ModelCapabilities {
            tools: true,
            ..Default::default()
        },
        reasoning: Default::default(),
        supports_reasoning: true,
    }]);
    handle.set_status_snapshot(StatusSnapshot {
        model: "deepseek-v4-flash".into(),
        model_profile: Some("deepseek".into()),
        ..StatusSnapshot::default()
    });
    let router = EngineCommandRouter::new(
        handle as Arc<dyn platform_api::orchestrator::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    )
    .with_provider_model_catalog_listings(vec![
        platform_api::ModelListing {
            display_model: "Claude Sonnet 5".into(),
            request_model: "claude-sonnet-5".into(),
            provider_id: "anthropic".into(),
            provider_label: "Anthropic".into(),
            description: None,
            metadata: Default::default(),
            capabilities: platform_api::ModelCapabilities {
                tools: true,
                ..Default::default()
            },
            reasoning: Default::default(),
            supports_reasoning: true,
        },
        platform_api::ModelListing {
            display_model: "Internal 7B".into(),
            request_model: "internal-7b".into(),
            provider_id: "my-proxy".into(),
            provider_label: "My Proxy".into(),
            description: None,
            metadata: Default::default(),
            capabilities: platform_api::ModelCapabilities {
                tools: true,
                ..Default::default()
            },
            reasoning: Default::default(),
            supports_reasoning: true,
        },
    ]);
    let sink = CapturingSink::arc();

    router.route(ClientCommand::ListModels, sink.clone()).await;

    let events = sink.events().await;
    let ClientEvent::ProviderModelCatalog { providers } = &events[0] else {
        panic!("expected provider model catalog event");
    };
    assert_eq!(
        providers
            .iter()
            .map(|provider| provider.provider_id.as_str())
            .collect::<Vec<_>>(),
        vec!["anthropic", "my-proxy"]
    );
}

#[tokio::test]
async fn list_mcp_routes() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_mcp_servers(vec![McpServerInfo {
        name: "fs".into(),
        status: McpStatus::Connected,
        transport: "stdio".into(),
    }]);
    let router = router_with(handle, Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::RefreshListings {
                which: vec![ListingKindDto::Mcp],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        ClientEvent::McpServers { servers } => {
            assert_eq!(servers.len(), 1);
            assert_eq!(servers[0].name, "fs");
            assert_eq!(servers[0].transport, "stdio");
        }
        other => panic!("expected McpServers, got {other:?}"),
    }
}

/// Router-level plumbing for the Skills listing: proves `ListingKindDto::Skills`
/// dispatches through `OrchestratorHandle::list_skills`, the `SkillInfo` →
/// `SkillDto` lowering runs (a `PathBuf` becomes a display string), and the
/// result reaches the wire as `ClientEvent::Skills`. Real discovery-from-disk
/// coverage lives in `orchestrator/tests/list_skills_real.rs`; this test
/// instead proves the router ARM itself is wired — an unwired/forgotten arm
/// (the `_ => debug!(...)` catch-all) would leave `events` empty here.
#[tokio::test]
async fn list_skills_routes() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_skills(vec![SkillInfo {
        name: "greet".into(),
        source_dir: std::path::PathBuf::from("/home/user/.lingxi/skills/greet"),
    }]);
    let router = router_with(handle, Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::RefreshListings {
                which: vec![ListingKindDto::Skills],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        ClientEvent::Skills { skills } => {
            assert_eq!(skills.len(), 1);
            assert_eq!(skills[0].name, "greet");
            assert_eq!(skills[0].source_dir, "/home/user/.lingxi/skills/greet");
        }
        other => panic!("expected Skills, got {other:?}"),
    }
}

#[tokio::test]
async fn slash_command_routes_to_registry() {
    // A dispatcher seeded with the builtin handlers — the SAME registry the
    // desktop composition root builds; the routed `/clear` reaches it and yields
    // a non-empty display string, proving the command crossed into the registry.
    use command_api::dispatcher::RegistrySlashDispatcher;
    use command_api::registry::CommandRegistry;
    use command_core::register_all_builtin_commands;
    use tokio::sync::RwLock;

    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let dispatcher = Arc::new(RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg))));

    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = EngineCommandRouter::new(
        handle as Arc<dyn platform_api::orchestrator::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        Some(dispatcher),
        None,
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::RunSlashCommand {
                raw: "/clear".into(),
                turn_id: Some(7),
            },
            sink.clone(),
        )
        .await;

    // The dispatcher handled the command and preserves the caller's turn id in
    // a structured local-result event.
    let events = sink.events().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClientEvent::SlashCommandResult { turn_id: Some(7), display, is_error: false } if !display.is_empty())),
        "the slash command must route to the registry and surface a display, got {events:?}"
    );
}

#[tokio::test]
async fn refresh_slash_commands_reads_live_registry_catalog() {
    use command_api::dispatcher::RegistrySlashDispatcher;
    use command_api::model::{CommandFrontmatter, CommandSource, SlashCommand, SlashCommandKind};
    use command_api::registry::CommandRegistry;
    use tokio::sync::RwLock;

    let mut reg = CommandRegistry::new();
    command_core::register_bundled_skills(&mut reg, true);
    reg.register_command(SlashCommand {
        name: "deploy".to_string(),
        description: "ship it".to_string(),
        source: CommandSource::Project,
        kind: SlashCommandKind::Markdown {
            file_path: std::path::PathBuf::from("/tmp/deploy.md"),
            frontmatter: CommandFrontmatter {
                description: "ship it".to_string(),
                ..CommandFrontmatter::default()
            },
            prompt_template: "deploy".to_string(),
        },
        ..SlashCommand::default()
    });
    let shared = Arc::new(RwLock::new(reg));
    let dispatcher = Arc::new(RegistrySlashDispatcher::new(shared.clone()));
    let router = EngineCommandRouter::new(
        Arc::new(MockOrchestratorHandle::new())
            as Arc<dyn platform_api::orchestrator::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        Some(dispatcher),
        Some(shared),
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::RefreshListings {
                which: vec![ListingKindDto::SlashCommands],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    match events.as_slice() {
        [ClientEvent::SlashCommandCatalog { commands }] => {
            assert!(
                commands.iter().any(|cmd| {
                    cmd.name == "deploy" && cmd.description == "ship it" && cmd.source == "project"
                }),
                "expected live registry command in catalog, got {commands:?}"
            );
            assert!(
                commands.iter().any(|cmd| {
                    cmd.name == "cron"
                        && cmd.source == "bundled"
                        && cmd.argument_hint.as_deref() == Some("<schedule or action>")
                }),
                "expected the enabled /cron bundle in the Desktop catalog, got {commands:?}"
            );
        }
        other => panic!("expected SlashCommandCatalog, got {other:?}"),
    }
}

#[tokio::test]
async fn desktop_plan_enables_plan_state_and_runs_the_argument_as_a_turn() {
    let handle = Arc::new(ResumingHandle::new());
    let router = EngineCommandRouter::new(
        handle.clone() as Arc<dyn OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    );

    let outcome = router
        .dispatch_slash("/plan design the migration")
        .await
        .expect("Desktop /plan must not require the TUI dispatcher");

    assert_eq!(
        outcome.result,
        SlashDispatchResult::RunAsTurn {
            prompt: "design the migration".to_string(),
        }
    );
    assert_eq!(
        handle.current_permission_mode().await.as_deref(),
        Some("plan")
    );
    assert!(handle.current_plan_mode().await);
    assert_eq!(
        handle.operation_log().await,
        vec!["set_permission_mode:plan", "set_plan_mode:true"]
    );
}

#[tokio::test]
async fn desktop_diff_runs_as_a_read_only_turn_instead_of_returning_tui_only() {
    let router = EngineCommandRouter::new(
        Arc::new(MockOrchestratorHandle::new()) as Arc<dyn OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    );

    let outcome = router
        .dispatch_slash("/diff renderer")
        .await
        .expect("Desktop /diff must not require the TUI dispatcher");

    match outcome.result {
        SlashDispatchResult::RunAsTurn { prompt } => {
            assert!(prompt.contains("uncommitted diff"));
            assert!(prompt.contains("renderer"));
            assert!(prompt.contains("Do not modify any files"));
        }
        other => panic!("expected /diff to start a read-only turn, got {other:?}"),
    }
}

struct MutatingDispatcher {
    registry: Arc<tokio::sync::RwLock<command_api::registry::CommandRegistry>>,
}

#[async_trait]
impl SlashCommandDispatcher for MutatingDispatcher {
    async fn dispatch(&self, raw: &str) -> SlashDispatchResult {
        use command_api::model::{
            CommandFrontmatter, CommandSource, SlashCommand, SlashCommandKind,
        };

        if raw == "/install" {
            self.registry.write().await.register_command(SlashCommand {
                name: "newcmd".to_string(),
                description: "added during dispatch".to_string(),
                source: CommandSource::User,
                kind: SlashCommandKind::Markdown {
                    file_path: std::path::PathBuf::from("/tmp/newcmd.md"),
                    frontmatter: CommandFrontmatter {
                        description: "added during dispatch".to_string(),
                        ..CommandFrontmatter::default()
                    },
                    prompt_template: "hello".to_string(),
                },
                ..SlashCommand::default()
            });
            SlashDispatchResult::Handled {
                display: "installed".to_string(),
            }
        } else {
            SlashDispatchResult::Handled {
                display: raw.to_string(),
            }
        }
    }
}

#[tokio::test]
async fn cron_display_result_is_persisted_for_session_resume() {
    use command_api::registry::CommandRegistry;
    use tokio::sync::RwLock;

    let handle = Arc::new(MockOrchestratorHandle::new());
    let shared = Arc::new(RwLock::new(CommandRegistry::new()));
    let router = EngineCommandRouter::new(
        handle.clone() as Arc<dyn OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        Some(Arc::new(MutatingDispatcher {
            registry: shared.clone(),
        })),
        Some(shared),
    );

    let outcome = router.dispatch_slash("/cron list").await.expect("dispatch");
    assert_eq!(
        outcome.result,
        SlashDispatchResult::Handled {
            display: "/cron list".to_string(),
        },
    );
    assert_eq!(
        handle.slash_command_transcript(),
        vec![("/cron list".to_string(), "/cron list".to_string())],
    );
}

#[tokio::test]
async fn run_slash_command_emits_commands_changed_when_registry_mutates() {
    use command_api::model::{CommandFrontmatter, CommandSource, SlashCommand, SlashCommandKind};
    use command_api::registry::CommandRegistry;
    use tokio::sync::RwLock;

    let mut reg = CommandRegistry::new();
    reg.register_command(SlashCommand {
        name: "install".to_string(),
        description: "install command".to_string(),
        source: CommandSource::Builtin,
        kind: SlashCommandKind::Markdown {
            file_path: std::path::PathBuf::from("/tmp/install.md"),
            frontmatter: CommandFrontmatter {
                description: "install command".to_string(),
                ..CommandFrontmatter::default()
            },
            prompt_template: "install".to_string(),
        },
        ..SlashCommand::default()
    });
    let shared = Arc::new(RwLock::new(reg));
    let router = EngineCommandRouter::new(
        Arc::new(MockOrchestratorHandle::new())
            as Arc<dyn platform_api::orchestrator::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        Some(Arc::new(MutatingDispatcher {
            registry: shared.clone(),
        })),
        Some(shared),
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::RunSlashCommand {
                raw: "/install".into(),
                turn_id: Some(9),
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::SlashCommandResult { turn_id: Some(9), display, is_error: false } if display == "installed")),
        "dispatcher reply must be surfaced, got {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            ClientEvent::CommandsChanged { commands }
                if commands.iter().any(|cmd| cmd.name == "newcmd" && cmd.source == "user")
        )),
        "registry mutation must emit CommandsChanged, got {events:?}"
    );
}

#[tokio::test]
async fn task_list_command_emits_task_rows() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let tasks = Arc::new(MockTaskRegistry {
        rows: vec![
            TaskRecord {
                task_id: "b3f9zk2xq".into(),
                task_type: "local_bash".into(),
                status: "running".into(),
                description: "build".into(),
                command: None,
                ..Default::default()
            },
            TaskRecord {
                task_id: "a1c2d3e4f".into(),
                task_type: "agent".into(),
                status: "completed".into(),
                description: "review".into(),
                command: None,
                ..Default::default()
            },
        ],
    });
    let router = router_with(handle, tasks);
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::TaskList {
                status_filter: None,
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let rows: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::TaskRow { task } => Some(task.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(rows.len(), 2, "one TaskRow per task, got {events:?}");
    assert_eq!(rows[0].task_id, "b3f9zk2xq");
    assert_eq!(rows[1].task_id, "a1c2d3e4f");
}

#[tokio::test]
async fn task_list_poll_emits_task_row() {
    // The adapter OWNS a task poll loop (matches the TUI; `TaskRegistryHandle::list`
    // on an interval). One tick of the poll loop emits a `TaskRow` per task.
    let handle = Arc::new(MockOrchestratorHandle::new());
    let tasks = Arc::new(MockTaskRegistry {
        rows: vec![TaskRecord {
            task_id: "p0lle3dt1".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "poll me".into(),
            command: None,
            ..Default::default()
        }],
    });
    let router = router_with(handle, tasks);
    let sink = CapturingSink::arc();

    // Spawn the poll loop on a short interval, then stop it after one tick.
    let poll = router.spawn_task_poll(sink.clone(), Duration::from_millis(10));

    // Wait until at least one TaskRow lands (bounded so a regression can't hang).
    let mut saw_row = false;
    for _ in 0..200 {
        if sink
            .events()
            .await
            .iter()
            .any(|e| matches!(e, ClientEvent::TaskRow { .. }))
        {
            saw_row = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    poll.stop();
    assert!(
        saw_row,
        "the task poll loop must emit a TaskRow per live task"
    );

    let events = sink.events().await;
    let row = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::TaskRow { task } => Some(task.clone()),
            _ => None,
        })
        .expect("a TaskRow must have been emitted");
    assert_eq!(row.task_id, "p0lle3dt1");
}

#[tokio::test]
async fn clear_session_rejected_mid_turn() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    // Mark a turn in flight: `ClearSession` must be REJECTED (an `Error` reply)
    // and must NOT reach the engine handle.
    router.set_turn_active(true);
    router
        .route(ClientCommand::ClearSession, sink.clone())
        .await;

    assert!(
        !handle.was_clear_session_called(),
        "ClearSession must not reach the engine while a turn is in flight"
    );
    let events = sink.events().await;
    assert_eq!(events.len(), 1);
    assert!(
        matches!(events[0], ClientEvent::Error { .. }),
        "mid-turn ClearSession must surface an Error, got {:?}",
        events[0]
    );

    // Once the turn ends, `ClearSession` routes to the engine and reports
    // `SessionEnded`.
    router.set_turn_active(false);
    let sink2 = CapturingSink::arc();
    router
        .route(ClientCommand::ClearSession, sink2.clone())
        .await;

    assert!(
        handle.was_clear_session_called(),
        "ClearSession must reach the engine once the turn has ended"
    );
    let events2 = sink2.events().await;
    assert!(
        events2
            .iter()
            .any(|e| matches!(e, ClientEvent::SessionEnded)),
        "a successful ClearSession must report SessionEnded, got {events2:?}"
    );
}

#[tokio::test]
async fn list_sessions_reads_jsonl_metadata_and_empty_store_replies() {
    let populated = tempfile::tempdir().unwrap();
    let uuid = seed_session_file(populated.path());
    let router = router_with_store(Arc::new(MockOrchestratorHandle::new()), populated.path());
    let sink = CapturingSink::arc();
    router
        .route(ClientCommand::ListSessions { limit: None }, sink.clone())
        .await;

    let events = sink.events().await;
    let sessions = events
        .iter()
        .find_map(|event| match event {
            ClientEvent::SessionList { sessions } => Some(sessions),
            _ => None,
        })
        .expect("list must always emit SessionList");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].uuid, uuid);
    assert_eq!(sessions[0].message_count, 1);
    assert!(sessions[0].path.ends_with(&format!("{uuid}.jsonl")));

    let empty = tempfile::tempdir().unwrap();
    let empty_router = router_with_store(Arc::new(MockOrchestratorHandle::new()), empty.path());
    let empty_sink = CapturingSink::arc();
    empty_router
        .route(
            ClientCommand::RefreshListings {
                which: vec![ListingKindDto::Sessions],
            },
            empty_sink.clone(),
        )
        .await;
    assert_eq!(
        empty_sink.events().await,
        vec![ClientEvent::SessionList {
            sessions: Vec::new()
        }]
    );
}

#[tokio::test]
async fn list_sessions_reports_corrupt_catalog_as_error() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_string_lossy().into_owned();
    let project_dir = root
        .path()
        .join(".lingxi")
        .join("projects")
        .join(session::jsonl::project_dir_name(&cwd));
    std::fs::create_dir_all(&project_dir).unwrap();

    let corrupt_session_id = "99999999-2222-4333-8444-555555555555";
    std::fs::write(project_dir.join(format!("{corrupt_session_id}.jsonl")), "{").unwrap();

    let router = router_with_store(Arc::new(MockOrchestratorHandle::new()), root.path());
    let sink = CapturingSink::arc();
    router
        .route(ClientCommand::ListSessions { limit: None }, sink.clone())
        .await;

    let events = sink.events().await;
    assert!(
        matches!(
            events.as_slice(),
            [ClientEvent::Error { kind, message }]
                if *kind == ErrorKindDto::Internal
                    && message.contains("session catalog")
                    && message.contains("retry")
                    && !message.contains(&root.path().display().to_string())
                    && !message.contains(corrupt_session_id)
        ),
        "unexpected corrupt-catalog response: {events:?}"
    );
}

#[tokio::test]
async fn list_sessions_preserves_readable_rows_when_one_transcript_is_corrupt() {
    let root = tempfile::tempdir().unwrap();
    let first = seed_session_file(root.path());
    let second = seed_session_file_with_ids(
        root.path(),
        "22222222-3333-4444-8555-666666666666",
        "bbbbbbbb-3333-4444-8555-666666666666",
        "second session",
    );
    let cwd = root.path().to_string_lossy().into_owned();
    let project_dir = root
        .path()
        .join(".lingxi")
        .join("projects")
        .join(session::jsonl::project_dir_name(&cwd));
    let corrupt_session_id = "99999999-2222-4333-8444-555555555555";
    std::fs::write(project_dir.join(format!("{corrupt_session_id}.jsonl")), "{").unwrap();

    let router = router_with_store(Arc::new(MockOrchestratorHandle::new()), root.path());
    let sink = CapturingSink::arc();
    router
        .route(ClientCommand::ListSessions { limit: None }, sink.clone())
        .await;

    let events = sink.events().await;
    assert_eq!(events.len(), 2, "expected rows plus a recoverable warning");
    let ClientEvent::SessionList { sessions } = &events[0] else {
        panic!("expected session list first, got {events:?}");
    };
    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().any(|session| session.uuid == first));
    assert!(sessions.iter().any(|session| session.uuid == second));
    assert!(matches!(
        &events[1],
        ClientEvent::Error { kind, message }
            if *kind == ErrorKindDto::Internal
                && message.contains("unreadable sessions were skipped")
                && message.contains("retry")
                && !message.contains(&root.path().display().to_string())
                && !message.contains(corrupt_session_id)
    ));
}

#[tokio::test]
async fn new_session_clears_applies_model_and_emits_started() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let expected_id = platform_api::OrchestratorHandle::current_session_id(&*handle)
        .await
        .to_string();
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::NewSession {
                cwd: None,
                model: Some("claude-opus-4-8".into()),
            },
            sink.clone(),
        )
        .await;

    assert!(handle.was_clear_session_called());
    assert_eq!(
        handle.last_switched_model().as_deref(),
        Some("claude-opus-4-8")
    );
    assert_eq!(
        sink.events().await,
        vec![ClientEvent::SessionStarted {
            session_id: expected_id,
            mode: SessionModeDto::Code,
        }]
    );
}

#[tokio::test]
async fn new_and_resume_are_rejected_mid_turn_and_bad_resume_is_honest() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = router_with(handle.clone(), Arc::new(MockTaskRegistry { rows: vec![] }));
    router.set_turn_active(true);

    for command in [
        ClientCommand::NewSession {
            cwd: None,
            model: None,
        },
        ClientCommand::ResumeSession {
            session_id: "not-a-uuid".into(),
            cwd: None,
        },
    ] {
        let sink = CapturingSink::arc();
        router.route(command, sink.clone()).await;
        assert!(matches!(
            sink.events().await.as_slice(),
            [ClientEvent::Error { .. }]
        ));
    }
    assert!(!handle.was_clear_session_called());

    router.set_turn_active(false);
    let sink = CapturingSink::arc();
    router
        .route(
            ClientCommand::ResumeSession {
                session_id: "not-a-uuid".into(),
                cwd: None,
            },
            sink.clone(),
        )
        .await;
    let events = sink.events().await;
    assert!(matches!(
        events.as_slice(),
        [ClientEvent::Error { message, .. }] if message.contains("malformed session id")
    ));
}

fn seed_session_agent_transcript(
    root: &std::path::Path,
    session_id: protocol::SessionId,
    nested: bool,
) -> (protocol::AgentId, std::path::PathBuf) {
    let cwd = root.to_string_lossy().into_owned();
    let base = orchestrator::transcript_paths::subagents_dir(
        &root.join(".lingxi"),
        &cwd,
        &session_id.as_uuid().to_string(),
    );
    let dir = if nested {
        base.join("workflows").join("wf_test")
    } else {
        base
    };
    std::fs::create_dir_all(&dir).unwrap();
    let agent_id = protocol::AgentId::new();
    let message = protocol::ConversationMessage::user(
        protocol::MessageId::new(),
        "inspect the runtime".to_string(),
    );
    let path = dir.join(format!("agent-{agent_id}.jsonl"));
    std::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::json!({
                "message": message,
                "status": "idle",
                "agent_name": "Runtime reviewer",
                "agent_type": "reviewer",
                "model": "test-model",
            })
        ),
    )
    .unwrap();
    (agent_id, path)
}

#[tokio::test]
async fn session_agent_routes_list_nested_transcripts_and_load_real_messages() {
    let root = tempfile::tempdir().unwrap();
    let handle = Arc::new(MockOrchestratorHandle::new());
    let session_id = handle.current_session_id().await;
    let (agent_id, _) = seed_session_agent_transcript(root.path(), session_id, true);
    let router = router_with_store(handle, root.path());

    let list_sink = CapturingSink::arc();
    router
        .route(ClientCommand::ListSessionAgents, list_sink.clone())
        .await;
    let list_events = list_sink.events().await;
    let [ClientEvent::SessionAgentList {
        session_id: emitted_session,
        agents,
    }] = list_events.as_slice()
    else {
        panic!("expected one session-agent list, got {list_events:?}");
    };
    assert_eq!(emitted_session, &session_id.as_uuid().to_string());
    assert_eq!(agents[0].agent_id, "main");
    assert!(agents.iter().any(|agent| {
        agent.agent_id == agent_id.to_string()
            && agent.name == "Runtime reviewer"
            && agent.status == "idle"
    }));

    let transcript_sink = CapturingSink::arc();
    router
        .route(
            ClientCommand::LoadSessionAgentTranscript {
                agent_id: agent_id.to_string(),
            },
            transcript_sink.clone(),
        )
        .await;
    assert!(matches!(
        transcript_sink.events().await.as_slice(),
        [ClientEvent::SessionAgentTranscript {
            session_id: emitted_session,
            agent_id: emitted_agent,
            messages,
            next_message_index: 1,
            revision: 1,
        }] if emitted_session == &session_id.as_uuid().to_string()
            && emitted_agent == &agent_id.to_string()
            && messages.len() == 1
    ));
}

#[tokio::test]
async fn session_agent_transcript_route_reports_absent_and_corrupt_files() {
    let root = tempfile::tempdir().unwrap();
    let handle = Arc::new(MockOrchestratorHandle::new());
    let session_id = handle.current_session_id().await;
    let router = router_with_store(handle, root.path());
    let missing_id = protocol::AgentId::new();
    let missing_sink = CapturingSink::arc();
    router
        .route(
            ClientCommand::LoadSessionAgentTranscript {
                agent_id: missing_id.to_string(),
            },
            missing_sink.clone(),
        )
        .await;
    assert!(matches!(
        missing_sink.events().await.as_slice(),
        [ClientEvent::Error { kind: ErrorKindDto::Rejected, message }]
            if message.contains("not found")
    ));

    let (corrupt_id, corrupt_path) = seed_session_agent_transcript(root.path(), session_id, false);
    std::fs::write(corrupt_path, "not-json\n").unwrap();
    let corrupt_sink = CapturingSink::arc();
    router
        .route(
            ClientCommand::LoadSessionAgentTranscript {
                agent_id: corrupt_id.to_string(),
            },
            corrupt_sink.clone(),
        )
        .await;
    assert!(matches!(
        corrupt_sink.events().await.as_slice(),
        [ClientEvent::Error { kind: ErrorKindDto::Internal, message }]
            if message.contains("corrupt")
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn session_agent_route_never_follows_a_symlinked_transcript_root() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let handle = Arc::new(MockOrchestratorHandle::new());
    let session_id = handle.current_session_id().await;
    let cwd = root.path().to_string_lossy().into_owned();
    let subagents = orchestrator::transcript_paths::subagents_dir(
        &root.path().join(".lingxi"),
        &cwd,
        &session_id.as_uuid().to_string(),
    );
    std::fs::create_dir_all(subagents.parent().unwrap()).unwrap();
    symlink(outside.path(), &subagents).unwrap();
    let router = router_with_store(handle, root.path());
    let sink = CapturingSink::arc();

    router
        .route(ClientCommand::ListSessionAgents, sink.clone())
        .await;
    let events = sink.events().await;
    assert!(
        matches!(events.first(), Some(ClientEvent::SessionAgentList { agents, .. }) if agents.len() == 1 && agents[0].agent_id == "main")
    );
    assert!(
        matches!(events.get(1), Some(ClientEvent::Error { message, .. }) if message.contains("skipped"))
    );
}

#[tokio::test]
async fn resume_session_replays_adopts_and_emits_full_transcript() {
    let root = tempfile::tempdir().unwrap();
    let session_id = seed_replay_session(root.path());
    let handle = Arc::new(ResumingHandle::new());
    let cwd = root.path().to_string_lossy().into_owned();
    let router = EngineCommandRouter::new(
        handle.clone() as Arc<dyn platform_api::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    )
    .with_session_store(SessionStoreContext::new(
        root.path().join(".lingxi"),
        cwd,
        Arc::new(PosixFileSystem::new(root.path().to_path_buf())),
    ));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::ResumeSession {
                session_id: session_id.clone(),
                cwd: None,
            },
            sink.clone(),
        )
        .await;

    let adopted = handle
        .resumed
        .lock()
        .await
        .clone()
        .expect("resume_session must adopt replayed state");
    assert_eq!(adopted.0.as_uuid().to_string(), session_id);
    assert_eq!(adopted.1.len(), 3);
    assert!(matches!(
        &adopted.1[0],
        protocol::ConversationMessage::System {
            subtype: Some(subtype),
            compact_metadata: Some(metadata),
            ..
        } if subtype == "compact_boundary" && metadata.cumulative_dropped_tokens == Some(4_321)
    ));
    assert_eq!(
        adopted.2.as_deref(),
        Some("cccccccc-3333-4333-8333-cccccccccccc")
    );
    assert_eq!(adopted.4.cumulative_dropped_tokens, 4_321);
    assert!(adopted.4.compacted);
    assert_eq!(adopted.4.model, "claude-opus-4-1");
    assert_eq!(adopted.4.effort.as_deref(), Some("high"));
    assert_eq!(adopted.4.loaded_tool_names, vec!["DeferredTool"]);
    assert_eq!(adopted.4.transcript_only_message_ids.len(), 1);
    assert_eq!(adopted.4.compact_summary_message_ids.len(), 1);
    // `lower_transcript` FOLDS the `is_compact_summary` user row into the
    // PRECEDING message's `CompactBoundary` block (see
    // `client-adapter/src/lowering.rs`, and the DTO's own doc: "Full compact
    // summary paired from the transcript-only summary row"). So the 3 adopted
    // history messages lower to 2 client messages — this assertion still
    // expected the pre-fold count.
    let events = sink.events().await;
    let [ClientEvent::SessionResumed {
        session_id: emitted_id,
        mode,
        messages,
    }, ClientEvent::ModelChanged { model }] = events.as_slice()
    else {
        panic!("expected SessionResumed followed by its model, got {events:?}");
    };
    assert_eq!(emitted_id, &session_id);
    assert_eq!(mode, &SessionModeDto::Code);
    assert_eq!(model, "claude-opus-4-1");
    assert_eq!(
        messages.len(),
        2,
        "the compact-summary row folds into the boundary block"
    );
    // Assert the fold HAPPENED rather than the row simply being dropped —
    // a count alone cannot tell those two apart.
    assert!(
        messages.iter().flat_map(|m| &m.blocks).any(|b| matches!(
            b,
            client_protocol::message::MessageBlockDto::CompactBoundary { summary, .. }
                if !summary.is_empty()
        )),
        "the folded summary must survive into a CompactBoundary block"
    );
}

#[tokio::test]
async fn resume_session_sets_plan_state_before_replay() {
    let root = tempfile::tempdir().unwrap();
    let session_id = seed_plan_replay_session(root.path());
    let handle = Arc::new(ResumingHandle::new());
    handle.set_current_permission_mode("acceptEdits").await;
    let cwd = root.path().to_string_lossy().into_owned();
    let router = EngineCommandRouter::new(
        handle.clone() as Arc<dyn platform_api::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    )
    .with_session_store(SessionStoreContext::new(
        root.path().join(".lingxi"),
        cwd,
        Arc::new(PosixFileSystem::new(root.path().to_path_buf())),
    ));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::ResumeSession {
                session_id: session_id.clone(),
                cwd: None,
            },
            sink.clone(),
        )
        .await;

    assert_eq!(
        handle.operation_log().await,
        vec![
            "set_permission_mode:plan".to_string(),
            "set_plan_mode:true".to_string(),
            "resume_session".to_string(),
        ]
    );
    assert_eq!(
        *handle.resume_context.lock().await,
        Some((Some("plan".to_string()), true))
    );
    assert_eq!(
        handle.current_permission_mode().await.as_deref(),
        Some("plan")
    );
    assert!(handle.current_plan_mode().await);
    assert!(matches!(
        sink.events().await.as_slice(),
        [
            ClientEvent::SessionResumed { .. },
            ClientEvent::ModelChanged { .. }
        ]
    ));
}

#[tokio::test]
async fn resume_session_rolls_back_plan_preset_when_plan_mode_enable_fails() {
    let root = tempfile::tempdir().unwrap();
    let session_id = seed_plan_replay_session(root.path());
    let handle = Arc::new(ResumingHandle::new());
    handle.set_current_permission_mode("acceptEdits").await;
    handle.set_plan_mode_error("plan latch failed").await;
    let cwd = root.path().to_string_lossy().into_owned();
    let router = EngineCommandRouter::new(
        handle.clone() as Arc<dyn platform_api::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    )
    .with_session_store(SessionStoreContext::new(
        root.path().join(".lingxi"),
        cwd,
        Arc::new(PosixFileSystem::new(root.path().to_path_buf())),
    ));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::ResumeSession {
                session_id,
                cwd: None,
            },
            sink.clone(),
        )
        .await;

    assert_eq!(
        handle.operation_log().await,
        vec![
            "set_permission_mode:plan".to_string(),
            "set_plan_mode:true".to_string(),
            "set_permission_mode:acceptEdits".to_string(),
        ]
    );
    assert!(handle.resume_context.lock().await.is_none());
    assert_eq!(
        handle.current_permission_mode().await.as_deref(),
        Some("acceptEdits")
    );
    assert!(!handle.current_plan_mode().await);
    assert!(matches!(
        sink.events().await.as_slice(),
        [ClientEvent::Error { message, .. }] if message.contains("resume plan mode failed")
    ));
}

#[tokio::test]
async fn resume_session_does_not_adopt_when_plan_permission_preset_fails() {
    let root = tempfile::tempdir().unwrap();
    let session_id = seed_plan_replay_session(root.path());
    let handle = Arc::new(ResumingHandle::new());
    handle.set_current_permission_mode("acceptEdits").await;
    handle.set_current_plan_mode(true).await;
    handle.set_permission_mode_error("plan gate failed").await;
    let cwd = root.path().to_string_lossy().into_owned();
    let router = EngineCommandRouter::new(
        handle.clone() as Arc<dyn platform_api::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    )
    .with_session_store(SessionStoreContext::new(
        root.path().join(".lingxi"),
        cwd,
        Arc::new(PosixFileSystem::new(root.path().to_path_buf())),
    ));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::ResumeSession {
                session_id,
                cwd: None,
            },
            sink.clone(),
        )
        .await;

    assert_eq!(
        handle.operation_log().await,
        vec!["set_permission_mode:plan".to_string()]
    );
    assert!(handle.resume_context.lock().await.is_none());
    assert_eq!(
        handle.current_permission_mode().await.as_deref(),
        Some("acceptEdits")
    );
    assert!(handle.current_plan_mode().await);
    assert!(matches!(
        sink.events().await.as_slice(),
        [ClientEvent::Error { message, .. }]
            if message.contains("resume plan permission mode failed")
    ));
}

#[tokio::test]
async fn resume_session_rolls_back_plan_state_when_replay_fails() {
    let root = tempfile::tempdir().unwrap();
    let session_id = seed_plan_replay_session(root.path());
    let handle = Arc::new(ResumingHandle::new());
    handle.set_current_permission_mode("acceptEdits").await;
    handle.set_current_plan_mode(false).await;
    handle.set_resume_error("resume replay failed").await;
    let cwd = root.path().to_string_lossy().into_owned();
    let router = EngineCommandRouter::new(
        handle.clone() as Arc<dyn platform_api::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry { rows: vec![] }) as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    )
    .with_session_store(SessionStoreContext::new(
        root.path().join(".lingxi"),
        cwd,
        Arc::new(PosixFileSystem::new(root.path().to_path_buf())),
    ));
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::ResumeSession {
                session_id,
                cwd: None,
            },
            sink.clone(),
        )
        .await;

    assert_eq!(
        handle.operation_log().await,
        vec![
            "set_permission_mode:plan".to_string(),
            "set_plan_mode:true".to_string(),
            "resume_session".to_string(),
            "set_plan_mode:false".to_string(),
            "set_permission_mode:acceptEdits".to_string(),
        ]
    );
    assert_eq!(
        *handle.resume_context.lock().await,
        Some((Some("plan".to_string()), true))
    );
    assert_eq!(
        handle.current_permission_mode().await.as_deref(),
        Some("acceptEdits")
    );
    assert!(!handle.current_plan_mode().await);
    assert!(matches!(
        sink.events().await.as_slice(),
        [ClientEvent::Error { message, .. }] if message.contains("resume_session failed")
    ));
}

// ── End-to-end: routing over the real WebSocket transport ─────────────────────
//
// The unit tests above exercise the router directly; this proves the connection
// loop ([`BridgeConnection`]) actually DELEGATES a non-turn command to the bound
// router over a live loopback WebSocket and frames the reply back as a
// `Frame::Event` — i.e. the F2-08 `bind_router` integration works on the wire,
// not just in isolation.

const E2E_TOKEN: &str = "router-e2e-token-32chars00000000";

/// A no-op turn driver (the e2e command under test is not a turn) — `bind`
/// requires one.
struct NoopTurnDriver;
#[async_trait]
impl bridge_server::server::TurnDriver for NoopTurnDriver {
    async fn run_turn(&self, _prompt: String) {}
}

/// A no-op permission sink — the routed command never triggers `check()`.
struct NoopPermissionSink;
#[async_trait]
impl PermissionRequestSink for NoopPermissionSink {
    async fn emit_request(&self, _request: PermissionRequest) {}
}

async fn connect(
    port: u16,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://127.0.0.1:{port}/mcp");
    let req = http::Request::builder()
        .method("GET")
        .uri(&url)
        .header("host", format!("127.0.0.1:{port}"))
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", generate_key())
        .header("sec-websocket-protocol", "mcp")
        .header("x-lingxi-ide-authorization", E2E_TOKEN)
        .body(())
        .unwrap();
    let (ws, response) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws upgrade must succeed");
    assert_eq!(response.status(), 101, "upgrade must return 101");
    ws
}

async fn next_frame<S>(ws: &mut S) -> Frame
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("a frame must arrive within the timeout")
            .expect("stream must yield a message")
            .expect("message must not be a ws error");
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).expect("decode Frame");
        }
    }
}

async fn send_command<S>(ws: &mut S, command: &ClientCommand)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = Frame::Request(bridge::BridgeRequest {
        id: 7,
        method: "submit".into(),
        params: serde_json::to_value(command).expect("serialize command"),
    });
    let text = serde_json::to_string(&frame).expect("serialize frame");
    ws.send(Message::Text(text)).await.expect("send command");
}

async fn send_hello<S>(ws: &mut S)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error>
        + StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let frame = Frame::Request(BridgeRequest {
        id: 1,
        method: "hello".into(),
        params: serde_json::to_value(ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
            client_name: "router-test".into(),
            capabilities: Capabilities::default(),
        })
        .unwrap(),
    });
    ws.send(Message::Text(serde_json::to_string(&frame).unwrap()))
        .await
        .expect("send hello");
    match next_frame(ws).await {
        Frame::Response(response) => assert!(response.error.is_none()),
        other => panic!("expected ServerHello response, got {other:?}"),
    }
}

#[tokio::test]
async fn set_model_routes_over_ws() {
    // A connection bound to BOTH the (unused) turn/permission path and the F2-08
    // command router over the engine's mock handle.
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = Arc::new(router_with(
        handle.clone(),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    )) as Arc<dyn CommandRouter>;

    let connection = BridgeConnection::new();
    let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
    let connection = connection
        .bind(gate, Arc::new(NoopTurnDriver))
        .bind_router(router);

    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(E2E_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    send_hello(&mut ws).await;

    // Drive a `SetModel` command over the wire after the mandatory handshake.
    send_command(
        &mut ws,
        &ClientCommand::SetModel {
            model: "claude-opus-4-8".into(),
        },
    )
    .await;

    // The reply is framed back as a `Frame::Event(ModelChanged)`.
    match next_frame(&mut ws).await {
        Frame::Event(ClientEvent::ModelChanged { model }) => {
            assert_eq!(model, "claude-opus-4-8");
        }
        other => panic!("expected Frame::Event(ModelChanged) over WS, got {other:?}"),
    }
    // …and the command reached the engine handle.
    assert_eq!(handle.switch_model_call_count(), 1);
    assert_eq!(
        handle.last_switched_model().as_deref(),
        Some("claude-opus-4-8")
    );

    endpoint.shutdown().await;
}

#[tokio::test]
async fn force_compact_completion_routes_over_ws_without_an_active_turn() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    let router = Arc::new(router_with(
        handle,
        Arc::new(MockTaskRegistry { rows: vec![] }),
    )) as Arc<dyn CommandRouter>;

    let connection = BridgeConnection::new();
    let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
    let connection = connection
        .bind(gate, Arc::new(NoopTurnDriver))
        .bind_router(router);

    let output = client_adapter::AdapterOutputStream::new(connection.event_sink());
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(E2E_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;
    send_hello(&mut ws).await;

    // The production orchestrator emits to this owned sink. Idle compaction
    // phases must still reach the wire while no conversation turn is active.
    platform_api::OutputStream::emit_compaction_started(&output).await;
    platform_api::OutputStream::emit_compaction_phase(&output, "summarizing").await;
    platform_api::OutputStream::emit_compaction_phase(&output, "restoring").await;
    platform_api::OutputStream::emit_compaction_finished(&output, None).await;
    for phase in ["preparing", "summarizing", "restoring", "complete"] {
        match next_frame(&mut ws).await {
            Frame::Event(ClientEvent::CompactionStatus {
                phase: actual,
                error,
            }) => {
                assert_eq!(actual, phase);
                assert!(error.is_none());
            }
            other => panic!("expected idle CompactionStatus over WS, got {other:?}"),
        }
    }

    send_command(&mut ws, &ClientCommand::ForceCompact).await;

    match next_frame(&mut ws).await {
        Frame::Event(ClientEvent::CompactionCompleted { .. }) => {}
        other => panic!("expected idle CompactionCompleted over WS, got {other:?}"),
    }

    endpoint.shutdown().await;
}

// ── Settings listing ─────────────────────────────────────────────────────────

/// Build a router carrying a settings context, over the same mock engine
/// handles every other routing test uses.
fn router_with_settings(settings: SettingsContext) -> EngineCommandRouter {
    router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    )
    .with_settings_context(settings)
}

/// `RefreshListings{Settings}` was defined in the protocol from the start and
/// never routed — `router.rs` matched it alongside `Memory` and emitted a
/// `tracing::debug!` line and nothing else. This pins that it must emit a real
/// snapshot.
///
/// The provenance assertion is not a restatement of a constant: the user layer
/// also defines `outputStyle`, so `"project"` is only correct because the merge
/// actually ranked project above user. Reverse the precedence and it fails.
#[tokio::test]
async fn the_settings_listing_emits_a_snapshot_instead_of_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(project.join(branding::DOT_DIR)).unwrap();
    std::fs::write(home.join("settings.json"), r#"{"outputStyle":"from-user"}"#).unwrap();
    std::fs::write(
        project.join(branding::DOT_DIR).join("settings.json"),
        r#"{"outputStyle":"from-project"}"#,
    )
    .unwrap();

    let mut managed = std::collections::BTreeMap::new();
    managed.insert(
        "telemetryEnabled".to_string(),
        serde_json::Value::from(false),
    );
    let router = router_with_settings(SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: project.clone(),
        },
        active: std::sync::Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
        managed,
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::RefreshListings {
                which: vec![ListingKindDto::Settings],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let (effective_json, provenance_json, files_json, locked) = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::SettingsSnapshot {
                effective_json,
                provenance_json,
                files_json,
                locked,
                ..
            } => Some((
                effective_json.clone(),
                provenance_json.clone(),
                files_json.clone(),
                locked.clone(),
            )),
            _ => None,
        })
        .expect("the settings listing must emit a SettingsSnapshot, not just a debug log");

    let effective: serde_json::Value = serde_json::from_str(&effective_json).unwrap();
    assert_eq!(
        effective["outputStyle"], "from-project",
        "project must beat user in the merged effective settings"
    );
    let provenance: serde_json::Value = serde_json::from_str(&provenance_json).unwrap();
    assert_eq!(
        provenance["outputStyle"], "project",
        "provenance must name the layer the winning value came from"
    );
    assert_eq!(
        locked.as_deref(),
        Some(&["telemetryEnabled".to_string()][..]),
        "locked must carry through from the settings context's managed overlay, not be dropped"
    );

    // The per-file layer states ride along so the UI can show which files back
    // each layer and which of them exist.
    let files: serde_json::Value =
        serde_json::from_str(&files_json.expect("files_json must be populated")).unwrap();
    let project_file = files
        .as_array()
        .expect("files_json is an array")
        .iter()
        .find(|f| f["layer"] == "project")
        .expect("the project layer must be reported");
    assert_eq!(project_file["exists"], true);
    assert_eq!(
        project_file["path"],
        serde_json::Value::String(
            project
                .join(branding::DOT_DIR)
                .join("settings.json")
                .to_string_lossy()
                .into_owned()
        ),
        "each file layer must report its real on-disk path"
    );
}

/// Without a settings context the listing must say so. Silently emitting
/// nothing is what this whole task exists to remove; falling back to silence in
/// the un-wired case would reintroduce it.
#[tokio::test]
async fn the_settings_listing_reports_a_missing_context_instead_of_staying_silent() {
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::RefreshListings {
                which: vec![ListingKindDto::Settings],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let message = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("a missing settings context must be reported, not swallowed");
    assert!(
        message.contains("settings context"),
        "the error must name what is missing, got: {message}"
    );
    // Reporting the gap must REPLACE the snapshot, not accompany it: an empty
    // `SettingsSnapshot` alongside the error would tell a client that the user
    // has no settings at all.
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClientEvent::SettingsSnapshot { .. })),
        "no SettingsSnapshot may be emitted without a settings context, got {events:?}"
    );
}

/// `UpdateSettings` decodes `patch_json` (`bridge_server::router::parse_settings_patch`
/// is unit-tested directly for the decode step in isolation), but only an
/// end-to-end route through a real settings context proves the wire-level
/// `null` actually reaches disk as a DELETE rather than a stored JSON `null` —
/// asserting on the file's parsed content, not on the intermediate `Vec` shape.
#[tokio::test]
async fn update_settings_with_a_null_value_deletes_the_key_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    let user_settings_path = home.join("settings.json");
    std::fs::write(
        &user_settings_path,
        r#"{"outputStyle":"terse","model":"opus"}"#,
    )
    .unwrap();

    let router = router_with_settings(SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        },
        active: std::sync::Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
        managed: std::collections::BTreeMap::new(),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpdateSettings {
                destination: SettingsDestinationDto::User,
                patch_json: r#"{"outputStyle": null}"#.to_string(),
            },
            sink.clone(),
        )
        .await;

    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&user_settings_path).unwrap()).unwrap();
    assert!(
        !on_disk.as_object().unwrap().contains_key("outputStyle"),
        "a null patch value must DELETE the key from the file, not write it as \
         JSON null; got {on_disk}"
    );
    assert_eq!(
        on_disk["model"], "opus",
        "an untouched key must survive the patch"
    );

    let events = sink.events().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClientEvent::SettingsSnapshot { .. })),
        "a successful update must resend the settings snapshot (I3), got {events:?}"
    );
}

/// A patch that parses as JSON but is not an OBJECT (here, an array) must be
/// rejected with a `Protocol`-kind error naming the shape problem, not
/// silently coerced or treated as an internal write failure.
#[tokio::test]
async fn update_settings_rejects_a_non_object_patch_with_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();

    let router = router_with_settings(SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        },
        active: std::sync::Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
        managed: std::collections::BTreeMap::new(),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpdateSettings {
                destination: SettingsDestinationDto::User,
                patch_json: r#"["outputStyle"]"#.to_string(),
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let (kind, message) = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::Error { kind, message } => Some((kind.clone(), message.clone())),
            _ => None,
        })
        .expect("a non-object patch must be reported, not swallowed");
    assert_eq!(
        kind,
        ErrorKindDto::Protocol,
        "a malformed wire patch is a protocol violation, not an internal failure"
    );
    assert!(
        message.contains("object"),
        "the message must say what shape was expected, got: {message}"
    );
}

/// Syntactically invalid JSON in `patch_json` must be rejected the same way —
/// `Protocol`-kind, with an actionable message — never a panic and never
/// silently treated as an empty patch.
#[tokio::test]
async fn update_settings_rejects_invalid_json_with_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();

    let router = router_with_settings(SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        },
        active: std::sync::Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
        managed: std::collections::BTreeMap::new(),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpdateSettings {
                destination: SettingsDestinationDto::User,
                patch_json: "{ not json".to_string(),
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let (kind, message) = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::Error { kind, message } => Some((kind.clone(), message.clone())),
            _ => None,
        })
        .expect("invalid JSON must be reported, not swallowed");
    assert_eq!(kind, ErrorKindDto::Protocol);
    assert!(
        message.contains("JSON"),
        "the message must say the patch is not valid JSON, got: {message}"
    );
}

/// `Memory` shared the do-nothing arm with `Settings`. Splitting `Settings` out
/// must not start emitting anything for `Memory`.
#[tokio::test]
async fn the_memory_listing_stays_unrouted() {
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::RefreshListings {
                which: vec![ListingKindDto::Memory],
            },
            sink.clone(),
        )
        .await;

    assert!(
        sink.events().await.is_empty(),
        "Memory has no engine handle in the foundation and must stay silent"
    );
}

// ── Permissions (persisted) ──────────────────────────────────────────────
//
// These three commands route to `permission::persist` (per-destination
// exclusive locks, atomic root-confined replacement, alias-normalizing
// de-duplication, unknown-key preservation) rather than through
// `UpdateSettings`, which refuses the `permissions` key precisely to avoid a
// second write path to it. Each positive test below asserts BOTH that the
// change landed in the right file AND that an unrelated key in that same
// file survived — the second half is what proves the write went through
// `persist.rs` rather than a hand-rolled overwrite that could pass the first
// half alone.

/// The rule must land in the project layer's `permissions.allow`, and an
/// unrelated key already in that file must survive untouched.
#[tokio::test]
async fn update_permission_rules_writes_the_named_layer_and_preserves_other_keys() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(project.join(branding::DOT_DIR)).unwrap();
    let target = project.join(branding::DOT_DIR).join("settings.json");
    std::fs::write(&target, r#"{"outputStyle":"terse"}"#).unwrap();

    let router = router_with_settings(SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        },
        active: std::sync::Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
        managed: std::collections::BTreeMap::new(),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpdatePermissionRules {
                destination: SettingsDestinationDto::Project,
                behavior: PermissionBehaviorDto::Allow,
                add: vec!["Bash(ls:*)".to_string()],
                remove: vec![],
            },
            sink.clone(),
        )
        .await;

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(
        written["permissions"]["allow"][0], "Bash(ls:*)",
        "the rule must land in the project layer's permissions.allow"
    );
    assert_eq!(
        written["outputStyle"], "terse",
        "unrelated keys must survive verbatim"
    );

    let events = sink.events().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClientEvent::SettingsSnapshot { .. })),
        "a successful update must resend the settings snapshot, got {events:?}"
    );
}

/// A request whose `add`/`remove` are both empty changes nothing on disk —
/// the router must say so rather than silently resending an unchanged
/// snapshot that looks identical to a successful write.
#[tokio::test]
async fn update_permission_rules_reports_when_nothing_was_requested() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();

    let router = router_with_settings(SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        },
        active: std::sync::Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
        managed: std::collections::BTreeMap::new(),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpdatePermissionRules {
                destination: SettingsDestinationDto::User,
                behavior: PermissionBehaviorDto::Allow,
                add: vec![],
                remove: vec![],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let (kind, message) = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::Error { kind, message } => Some((kind.clone(), message.clone())),
            _ => None,
        })
        .expect("a no-op request must be reported, not silently resent as a snapshot");
    assert_eq!(kind, ErrorKindDto::Rejected);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClientEvent::SettingsSnapshot { .. })),
        "a no-op must not also emit a snapshot, got {events:?}: {message}"
    );
}

/// Complements the "partial write" fix below: when NOTHING had changed yet
/// at the point a persist call errors (here, `add` is empty so only `remove`
/// runs, against a file with broken JSON), the router must report ONLY the
/// error — no snapshot. This pins the `if changed { snapshot }` branch's
/// FALSE side, so the fix for partial writes cannot regress into always
/// emitting a snapshot on top of an error regardless of whether anything
/// actually landed.
#[tokio::test]
async fn update_permission_rules_reports_only_the_error_when_nothing_changed_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("settings.json"), "{ not json").unwrap();

    let router = router_with_settings(SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        },
        active: std::sync::Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
        managed: std::collections::BTreeMap::new(),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpdatePermissionRules {
                destination: SettingsDestinationDto::User,
                behavior: PermissionBehaviorDto::Allow,
                add: vec![],
                remove: vec!["Bash".to_string()],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                ..
            }
        )),
        "a broken destination file must be reported as an internal failure, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClientEvent::SettingsSnapshot { .. })),
        "nothing changed before the error, so no snapshot may be emitted, got {events:?}"
    );
}

/// The default mode must land in the user layer's `defaultMode`, and an
/// unrelated key in that file must survive.
#[tokio::test]
async fn set_default_permission_mode_writes_the_named_layer_and_preserves_other_keys() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    let target = home.join("settings.json");
    std::fs::write(&target, r#"{"outputStyle":"terse"}"#).unwrap();

    let router = router_with_settings(SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        },
        active: std::sync::Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
        managed: std::collections::BTreeMap::new(),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::SetDefaultPermissionMode {
                destination: SettingsDestinationDto::User,
                mode: "acceptEdits".to_string(),
            },
            sink.clone(),
        )
        .await;

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(written["permissions"]["defaultMode"], "acceptEdits");
    assert_eq!(
        written["outputStyle"], "terse",
        "unrelated keys must survive verbatim"
    );

    let events = sink.events().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClientEvent::SettingsSnapshot { .. })),
        "a successful update must resend the settings snapshot, got {events:?}"
    );
}

/// `persist_permission_mode` deliberately refuses to persist
/// `"bypassPermissions"` (a security property: persisting it would silently
/// re-enter bypass mode on the next session load). The refusal must be
/// reported honestly — the caller must NOT see a `SettingsSnapshot` that
/// looks like the write happened, and the file must be left untouched.
#[tokio::test]
async fn set_default_permission_mode_reports_the_bypass_permissions_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    let target = home.join("settings.json");
    std::fs::write(&target, r#"{"outputStyle":"terse"}"#).unwrap();

    let router = router_with_settings(SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        },
        active: std::sync::Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
        managed: std::collections::BTreeMap::new(),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::SetDefaultPermissionMode {
                destination: SettingsDestinationDto::User,
                mode: "bypassPermissions".to_string(),
            },
            sink.clone(),
        )
        .await;

    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert!(
        on_disk
            .get("permissions")
            .and_then(|p| p.get("defaultMode"))
            .is_none(),
        "bypassPermissions must never be written to disk, got {on_disk}"
    );

    let events = sink.events().await;
    let (kind, message) = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::Error { kind, message } => Some((kind.clone(), message.clone())),
            _ => None,
        })
        .expect("the refusal must be reported, not swallowed");
    assert_eq!(kind, ErrorKindDto::Rejected);
    assert!(
        message.contains("bypassPermissions") || message.contains("session-scoped"),
        "the message must name what happened, got: {message}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClientEvent::SettingsSnapshot { .. })),
        "a refused persist must not also claim success via a snapshot, got {events:?}"
    );
}

/// The directory must land in the local layer's
/// `permissions.additionalDirectories`, and an unrelated key in that file
/// must survive.
#[tokio::test]
async fn update_workspace_directories_writes_the_named_layer_and_preserves_other_keys() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(project.join(branding::DOT_DIR)).unwrap();
    let target = project.join(branding::DOT_DIR).join("settings.local.json");
    std::fs::write(&target, r#"{"outputStyle":"terse"}"#).unwrap();

    let router = router_with_settings(SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: project,
        },
        active: std::sync::Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
        managed: std::collections::BTreeMap::new(),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpdateWorkspaceDirectories {
                destination: SettingsDestinationDto::Local,
                add: vec!["/tmp/extra".to_string()],
                remove: vec![],
            },
            sink.clone(),
        )
        .await;

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(
        written["permissions"]["additionalDirectories"][0], "/tmp/extra",
        "the directory must land in the local layer's additionalDirectories"
    );
    assert_eq!(
        written["outputStyle"], "terse",
        "unrelated keys must survive verbatim"
    );

    let events = sink.events().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClientEvent::SettingsSnapshot { .. })),
        "a successful update must resend the settings snapshot, got {events:?}"
    );
}

/// Without a settings context, all three permission commands must report the
/// gap instead of panicking or staying silent — the same contract
/// `apply_settings_patch` already honors for `UpdateSettings`.
#[tokio::test]
async fn update_permission_rules_reports_a_missing_context_instead_of_staying_silent() {
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpdatePermissionRules {
                destination: SettingsDestinationDto::User,
                behavior: PermissionBehaviorDto::Allow,
                add: vec!["Bash".to_string()],
                remove: vec![],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let message = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("a missing settings context must be reported, not swallowed");
    assert!(
        message.contains("settings context"),
        "the error must name what is missing, got: {message}"
    );
}

/// Same contract as above, for `SetDefaultPermissionMode` — the shared
/// `require_permission_paths` preflight is exercised by all three handlers,
/// but only pinning it through one caller would miss a mis-wiring of this
/// one to a different (or missing) guard.
#[tokio::test]
async fn set_default_permission_mode_reports_a_missing_context_instead_of_staying_silent() {
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::SetDefaultPermissionMode {
                destination: SettingsDestinationDto::User,
                mode: "acceptEdits".to_string(),
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let message = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("a missing settings context must be reported, not swallowed");
    assert!(
        message.contains("settings context"),
        "the error must name what is missing, got: {message}"
    );
}

/// Same contract as above, for `UpdateWorkspaceDirectories`.
#[tokio::test]
async fn update_workspace_directories_reports_a_missing_context_instead_of_staying_silent() {
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpdateWorkspaceDirectories {
                destination: SettingsDestinationDto::User,
                add: vec!["/tmp/extra".to_string()],
                remove: vec![],
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let message = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("a missing settings context must be reported, not swallowed");
    assert!(
        message.contains("settings context"),
        "the error must name what is missing, got: {message}"
    );
}

// ── MCP server writes ─────────────────────────────────────────────────────────
//
// `mcp_bridge`'s own unit tests (in `apps/bridge-server/src/mcp_bridge.rs`)
// already cover the scope-to-storage-location mapping across all three
// scopes against the real `mcp::json_config` parser. What is genuinely new
// HERE — the router's translation from the wire (`config_json` string,
// missing-context handling, error-kind selection) into that call — gets its
// own coverage below, the same way `update_settings_*` covers
// `apply_settings_patch` end-to-end rather than trusting the unit-tested
// `parse_settings_patch` decode step alone.

/// Build a router carrying an MCP context, over the same mock engine handles
/// every other routing test uses.
fn router_with_mcp(mcp: McpPaths) -> EngineCommandRouter {
    router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    )
    .with_mcp_paths(mcp)
}

/// End-to-end: `UpsertMcpServer` routed through a real MCP context actually
/// lands on disk at `<project>/.mcp.json`, and a second `RemoveMcpServer`
/// deletes it — mirroring `update_settings_with_a_null_value_deletes_the_key_on_disk`'s
/// "assert on disk, not on an intermediate value" shape.
#[tokio::test]
async fn upsert_then_remove_mcp_server_round_trips_through_the_router() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("repo");
    std::fs::create_dir_all(&project).unwrap();
    let router = router_with_mcp(McpPaths {
        project_dir: project.clone(),
        global_config_path: dir.path().join(".lingxi.json"),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpsertMcpServer {
                scope: McpScopeDto::Project,
                name: "linear".to_string(),
                config_json: r#"{"command":"npx","args":["-y","linear-mcp"]}"#.to_string(),
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClientEvent::Error { .. })),
        "a successful upsert must not emit an Error, got {events:?}"
    );
    let raw = std::fs::read_to_string(project.join(".mcp.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["mcpServers"]["linear"]["command"], "npx");

    router
        .route(
            ClientCommand::RemoveMcpServer {
                scope: McpScopeDto::Project,
                name: "linear".to_string(),
            },
            sink.clone(),
        )
        .await;

    let raw = std::fs::read_to_string(project.join(".mcp.json")).unwrap();
    let after: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(
        after["mcpServers"].get("linear").is_none(),
        "removal routed through the router must delete the entry, got: {after}"
    );
}

/// A `config_json` that is syntactically valid JSON but not an OBJECT
/// (`.mcp.json` entries are always object-shaped) must be rejected as a
/// PROTOCOL violation — the router's own decode step, not `mcp_bridge`'s
/// file-safety checks.
#[tokio::test]
async fn upsert_mcp_server_rejects_a_non_object_config_with_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let router = router_with_mcp(McpPaths {
        project_dir: dir.path().to_path_buf(),
        global_config_path: dir.path().join(".lingxi.json"),
    });
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::UpsertMcpServer {
                scope: McpScopeDto::Project,
                name: "linear".to_string(),
                config_json: r#"["npx"]"#.to_string(),
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let (kind, message) = events
        .iter()
        .find_map(|e| match e {
            ClientEvent::Error { kind, message } => Some((kind.clone(), message.clone())),
            _ => None,
        })
        .expect("a non-object config must be reported, not swallowed");
    assert_eq!(
        kind,
        ErrorKindDto::Protocol,
        "a malformed wire config is a protocol violation, not an internal failure"
    );
    assert!(
        message.contains("object"),
        "the message must say what shape was expected, got: {message}"
    );
    assert!(
        !dir.path().join(".mcp.json").exists(),
        "a rejected config must never create the target file"
    );
}

/// Without an MCP context, both commands must say so rather than silently
/// doing nothing — the same contract `apply_settings_patch` /
/// `require_permission_paths` already hold for their own missing-context case.
#[tokio::test]
async fn mcp_commands_report_a_missing_context_instead_of_staying_silent() {
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    );

    for command in [
        ClientCommand::UpsertMcpServer {
            scope: McpScopeDto::Project,
            name: "x".to_string(),
            config_json: r#"{"command":"x"}"#.to_string(),
        },
        ClientCommand::RemoveMcpServer {
            scope: McpScopeDto::Project,
            name: "x".to_string(),
        },
    ] {
        let sink = CapturingSink::arc();
        router.route(command, sink.clone()).await;
        let events = sink.events().await;
        let message = events
            .iter()
            .find_map(|e| match e {
                ClientEvent::Error { message, .. } => Some(message.clone()),
                _ => None,
            })
            .expect("a missing MCP context must be reported, not swallowed");
        assert!(
            message.contains("MCP context"),
            "the error must name what is missing, got: {message}"
        );
    }
}

/// Fix round 1 / Minor 3: an empty or whitespace-only name must be rejected
/// at the wire boundary with a Protocol error, never written — the storage
/// layer (`mcp_bridge`) has no name validation of its own, so this is the
/// only place that stops `mcp::json_config::build_servers_from_map` from
/// ever seeing a `""` map key.
#[tokio::test]
async fn upsert_mcp_server_rejects_an_empty_name_with_protocol_error() {
    let dir = tempfile::tempdir().unwrap();
    let router = router_with_mcp(McpPaths {
        project_dir: dir.path().to_path_buf(),
        global_config_path: dir.path().join(".lingxi.json"),
    });

    for empty_name in ["", "   "] {
        let sink = CapturingSink::arc();
        router
            .route(
                ClientCommand::UpsertMcpServer {
                    scope: McpScopeDto::Project,
                    name: empty_name.to_string(),
                    config_json: r#"{"command":"npx"}"#.to_string(),
                },
                sink.clone(),
            )
            .await;

        let events = sink.events().await;
        let (kind, message) = events
            .iter()
            .find_map(|e| match e {
                ClientEvent::Error { kind, message } => Some((kind.clone(), message.clone())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("an empty name ({empty_name:?}) must be reported"));
        assert_eq!(kind, ErrorKindDto::Protocol);
        assert!(
            message.to_lowercase().contains("name"),
            "the message must say what's wrong, got: {message}"
        );
    }
    assert!(
        !dir.path().join(".mcp.json").exists(),
        "a rejected empty name must never create the target file"
    );
}

// ── Durable turn recovery is not a desktop-bridge capability ─────────────────

/// `attach_turn` / `resume_turn` / `pause_turn` are request-reply commands on
/// the MOBILE host (`engine-mobile`'s `host.rs`): the client sends one and
/// waits for a `turn_recovery_state` snapshot (and, after `attach_turn`, a
/// `turn_event_replay` burst). This bridge has no retained turn-event window
/// and no recovery state machine, so it cannot answer them — and until this
/// test's arms existed they fell into `route`'s `#[non_exhaustive]` catch-all
/// and were only `tracing::debug!`-logged, which leaves a mobile-shaped client
/// waiting forever for a reply that is never coming.
///
/// The assertion is on the EXACT emitted event, so a future regression back to
/// the silent catch-all fails here as "left: [] right: [Error ...]" rather than
/// passing on an empty sink.
#[tokio::test]
async fn durable_turn_recovery_commands_are_explicitly_rejected_not_silently_dropped() {
    let cases: Vec<(ClientCommand, &str)> = vec![
        (
            ClientCommand::AttachTurn {
                turn_id: 7,
                after_sequence: Some(3),
            },
            "attach_turn is unavailable on this bridge",
        ),
        (
            ClientCommand::ResumeTurn { turn_id: 7 },
            "resume_turn is unavailable on this bridge",
        ),
        (
            ClientCommand::PauseTurn {
                turn_id: 7,
                reason: "background_time_expired".to_string(),
            },
            "pause_turn is unavailable on this bridge",
        ),
    ];

    for (command, expected_message) in cases {
        let router = router_with(
            Arc::new(MockOrchestratorHandle::new()),
            Arc::new(MockTaskRegistry { rows: vec![] }),
        );
        let sink = CapturingSink::arc();

        router.route(command.clone(), sink.clone()).await;

        assert_eq!(
            sink.events().await,
            vec![ClientEvent::Error {
                kind: ErrorKindDto::Rejected,
                message: expected_message.to_string(),
            }],
            "{command:?} must be answered with one typed rejection naming the command"
        );
    }
}

/// `attach_turn` with no `after_sequence` (the "replay the whole retained
/// window" request) takes the same rejection path — the arm must not be keyed
/// on the optional field.
#[tokio::test]
async fn attach_turn_without_a_sequence_is_rejected_the_same_way() {
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::AttachTurn {
                turn_id: 1,
                after_sequence: None,
            },
            sink.clone(),
        )
        .await;

    assert_eq!(
        sink.events().await,
        vec![ClientEvent::Error {
            kind: ErrorKindDto::Rejected,
            message: "attach_turn is unavailable on this bridge".to_string(),
        }]
    );
}

/// The sibling rejection this arm was modelled on, pinned so the two stay one
/// convention: `resume_workflow` is likewise a host capability this bridge does
/// not have, and likewise answers rather than drops.
#[tokio::test]
async fn resume_workflow_is_explicitly_rejected() {
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    );
    let sink = CapturingSink::arc();

    router
        .route(
            ClientCommand::ResumeWorkflow {
                task_id: "wf-1".to_string(),
            },
            sink.clone(),
        )
        .await;

    assert_eq!(
        sink.events().await,
        vec![ClientEvent::Error {
            kind: ErrorKindDto::Rejected,
            message: "resume_workflow is unavailable on this bridge".to_string(),
        }]
    );
}
