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
use bridge_server::router::{CommandRouter, EngineCommandRouter, SessionStoreContext};
use bridge_server::server::BridgeConnection;
use client_adapter::{AdapterPermissionGate, ClientEventSink, PermissionRequestSink};
use client_protocol::commands::{ClientCommand, ListingKindDto, ProviderCredentialSecretDto};
use client_protocol::events::{ClientEvent, ErrorKindDto};
use client_protocol::permission::PermissionRequest;
use futures_util::{SinkExt, StreamExt};
use orchestrator::test_support::MockOrchestratorHandle;
use platform_posix::{PlainTextSecureStorage, PosixClock, PosixFileSystem, PosixHttp};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;
use traits::auth::{AuthError, AuthHandle, LoginInfo};
use traits::orchestrator::{
    AgentInfo, CompactionSummary, CostSnapshot, DoctorReport, HandleError, HookInfo, McpServerInfo,
    McpStatus, MemoryEditorOutcome, StatusSnapshot,
};
use traits::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};
use traits::{SlashCommandDispatcher, SlashDispatchResult};

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
impl traits::SecureStorage for SelectiveFailureStorage {
    async fn store(
        &self,
        service: &str,
        account: &str,
        data: protocol::SecureStorageData,
    ) -> Result<(), traits::SecureStorageError> {
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
    ) -> Result<Option<protocol::SecureStorageData>, traits::SecureStorageError> {
        if account == "provider-key-openrouter" {
            return Err(traits::SecureStorageError::PermissionDenied(
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

    async fn delete(&self, service: &str, account: &str) -> Result<(), traits::SecureStorageError> {
        self.values
            .lock()
            .unwrap()
            .remove(&(service.to_string(), account.to_string()));
        Ok(())
    }

    async fn list(&self, service: &str) -> Result<Vec<String>, traits::SecureStorageError> {
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

    fn backend(&self) -> traits::SecureStorageBackend {
        traits::SecureStorageBackend::MacOsKeychain
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
            Option<traits::ActiveGoalSnapshot>,
            traits::ResumeRuntimeSnapshot,
        )>,
    >,
}

impl ResumingHandle {
    fn new() -> Self {
        Self {
            session_id: protocol::SessionId::new(),
            resumed: Mutex::new(None),
        }
    }
}

#[async_trait]
impl traits::OrchestratorHandle for ResumingHandle {
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
        StatusSnapshot::default()
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
    async fn resume_session(
        &self,
        session_id: protocol::SessionId,
        history: Vec<protocol::ConversationMessage>,
        last_jsonl_uuid: Option<String>,
        active_goal: Option<traits::ActiveGoalSnapshot>,
        runtime: traits::ResumeRuntimeSnapshot,
    ) -> Result<(), HandleError> {
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
        handle as Arc<dyn traits::orchestrator::OrchestratorHandle>,
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
        handle as Arc<dyn traits::orchestrator::OrchestratorHandle>,
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
            },
            sink.clone(),
        )
        .await;

    let events = sink.events().await;
    let ClientEvent::ProviderCredentialStatus {
        configured_provider_ids,
        unavailable_provider_ids,
        storage_encrypted,
        error,
        ..
    } = &events[0]
    else {
        panic!("expected provider status event")
    };
    assert_eq!(configured_provider_ids, &["deepseek"]);
    assert_eq!(unavailable_provider_ids, &["openrouter"]);
    assert!(*storage_encrypted);
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
async fn set_model_keeps_provider_in_acknowledgement() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_model_listings(vec![traits::ModelListing {
        display_model: "GPT-5.5".into(),
        request_model: "gpt-5.5".into(),
        provider_id: "github-copilot".into(),
        provider_label: "GitHub Copilot".into(),
        description: None,
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
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        ClientEvent::ModelList {
            models: vec!["a".into(), "b".into()],
            current: "a".into(),
        }
    );
}

#[tokio::test]
async fn list_models_curates_and_preserves_provider_identity() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_available_models(vec!["gpt-5.5".into(), "gpt-4o".into()]);
    handle.set_model_listings(vec![
        traits::ModelListing {
            display_model: "GPT-5.5".into(),
            request_model: "gpt-5.5".into(),
            provider_id: "openai".into(),
            provider_label: "OpenAI".into(),
            description: None,
            supports_reasoning: true,
        },
        traits::ModelListing {
            display_model: "GPT-4o".into(),
            request_model: "gpt-4o".into(),
            provider_id: "openai".into(),
            provider_label: "OpenAI".into(),
            description: None,
            supports_reasoning: false,
        },
        traits::ModelListing {
            display_model: "GPT-5.5".into(),
            request_model: "gpt-5.5".into(),
            provider_id: "github-copilot".into(),
            provider_label: "GitHub Copilot".into(),
            description: None,
            supports_reasoning: true,
        },
    ]);
    handle.set_status_snapshot(StatusSnapshot {
        model: "gpt-5.5".into(),
        model_profile: Some("github-copilot".into()),
        ..StatusSnapshot::default()
    });
    let router = router_with(handle, Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router.route(ClientCommand::ListModels, sink.clone()).await;

    assert_eq!(
        sink.events().await,
        vec![ClientEvent::ModelList {
            models: vec!["github-copilot/gpt-5.5".into(), "openai/gpt-5.5".into(),],
            current: "github-copilot/gpt-5.5".into(),
        }]
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
        handle as Arc<dyn traits::orchestrator::OrchestratorHandle>,
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
            as Arc<dyn traits::orchestrator::OrchestratorHandle>,
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
        }
        other => panic!("expected SlashCommandCatalog, got {other:?}"),
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
            as Arc<dyn traits::orchestrator::OrchestratorHandle>,
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
    let expected_id = traits::OrchestratorHandle::current_session_id(&*handle)
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
            session_id: expected_id
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

#[tokio::test]
async fn resume_session_replays_adopts_and_emits_full_transcript() {
    let root = tempfile::tempdir().unwrap();
    let session_id = seed_replay_session(root.path());
    let handle = Arc::new(ResumingHandle::new());
    let cwd = root.path().to_string_lossy().into_owned();
    let router = EngineCommandRouter::new(
        handle.clone() as Arc<dyn traits::OrchestratorHandle>,
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
        messages,
    }] = events.as_slice()
    else {
        panic!("expected exactly one SessionResumed, got {events:?}");
    };
    assert_eq!(emitted_id, &session_id);
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

    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(E2E_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;
    send_hello(&mut ws).await;

    send_command(&mut ws, &ClientCommand::ForceCompact).await;

    match next_frame(&mut ws).await {
        Frame::Event(ClientEvent::CompactionCompleted { .. }) => {}
        other => panic!("expected idle CompactionCompleted over WS, got {other:?}"),
    }

    endpoint.shutdown().await;
}
