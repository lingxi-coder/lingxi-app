//! Real router-to-process-presence regression for hot session replacement.

#![allow(clippy::unwrap_used)]

use async_trait::async_trait;
use bridge_server::router::{CommandRouter, EngineCommandRouter, SessionStoreContext};
use client_adapter::ClientEventSink;
use client_protocol::commands::ClientCommand;
use client_protocol::events::ClientEvent;
use platform_api::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};
use platform_api::{
    AuthError, AuthHandle, CompactionSummary, CostSnapshot, DoctorReport, HandleError, HookInfo,
    LoginInfo, McpServerInfo, MemoryEditorOutcome, OrchestratorHandle, SkillInfo, StatusSnapshot,
};
use platform_posix::PosixFileSystem;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Default)]
struct Sink(Mutex<Vec<ClientEvent>>);

#[async_trait]
impl ClientEventSink for Sink {
    async fn emit(&self, event: ClientEvent) {
        self.0.lock().await.push(event);
    }
}

struct SwitchHandle {
    current: Mutex<protocol::SessionId>,
    next: Mutex<protocol::SessionId>,
    fail: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl OrchestratorHandle for SwitchHandle {
    async fn current_session_id(&self) -> protocol::SessionId {
        *self.current.lock().await
    }

    async fn clear_session(&self) -> Result<(), HandleError> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(HandleError::ActionFailed(
                "destination durability preparation failed".into(),
            ));
        }
        *self.current.lock().await = *self.next.lock().await;
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

    async fn list_agents(&self) -> Vec<platform_api::AgentInfo> {
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
        _history: Vec<protocol::ConversationMessage>,
        _last_jsonl_uuid: Option<String>,
        _active_goal: Option<platform_api::ActiveGoalSnapshot>,
        _runtime: platform_api::ResumeRuntimeSnapshot,
    ) -> Result<(), HandleError> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(HandleError::ActionFailed(
                "destination durability preparation failed".into(),
            ));
        }
        *self.current.lock().await = session_id;
        Ok(())
    }
}

struct Auth;

#[async_trait]
impl AuthHandle for Auth {
    async fn login(&self) -> Result<LoginInfo, AuthError> {
        Err(AuthError::Network("test".into()))
    }

    async fn logout(&self) -> Result<(), AuthError> {
        Ok(())
    }

    async fn current_user(&self) -> Option<LoginInfo> {
        None
    }
}

struct Tasks;

#[async_trait]
impl TaskRegistryHandle for Tasks {
    async fn create(&self, _input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }

    async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
        Ok(None)
    }

    async fn list(&self, _filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
        Ok(Vec::new())
    }

    async fn update(
        &self,
        _id: &str,
        _patch: TaskUpdatePatch,
    ) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }

    async fn set_status(
        &self,
        _id: &str,
        _status: &str,
    ) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }

    async fn kill(&self, _id: &str) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }

    async fn output(
        &self,
        _id: &str,
        _offset: Option<u64>,
    ) -> Result<TaskOutputChunk, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
}

#[tokio::test]
async fn clear_updates_real_process_presence_and_failure_leaves_new_identity_intact() {
    let root = tempfile::tempdir().unwrap();
    let sessions = platform_api::live_sessions::LiveSessionDir::at(root.path().join("sessions"));
    let session_a = protocol::SessionId::parse_prefixed(
        "11111111-2222-4333-8444-555555555555",
    )
    .unwrap();
    let session_b = protocol::SessionId::parse_prefixed(
        "22222222-3333-4444-8555-666666666666",
    )
    .unwrap();
    let session_c = protocol::SessionId::parse_prefixed(
        "33333333-4444-4555-8666-777777777777",
    )
    .unwrap();
    let session_a_text = session_a.as_uuid().to_string();
    let session_b_text = session_b.as_uuid().to_string();
    let session_c_text = session_c.as_uuid().to_string();
    platform_api::live_sessions::set_process_dir(sessions.clone());
    platform_api::live_sessions::set_process_session_id(&session_a_text);
    platform_api::live_sessions::set_process_name("bridge-test");
    let socket = root.path().join("bridge.sock");
    let socket_text = socket.to_string_lossy().into_owned();
    platform_api::uds_inbox::start_process_inbox_for_session(
        &socket,
        &session_a_text,
    )
    .unwrap();
    sessions
        .upsert_identity(
            std::process::id(),
            &session_a_text,
            Some("bridge-test"),
            None,
            Some(&socket),
            None,
        )
        .unwrap();
    let mut old_generation = platform_api::live_sessions::outbound_peer_message(
        "sender",
        "source",
        "owned by A",
        None,
    );
    old_generation.msg_id = Some("old-generation".into());
    platform_api::uds_inbox::enqueue_accepted(old_generation);

    let handle = Arc::new(SwitchHandle {
        current: Mutex::new(session_a),
        next: Mutex::new(session_b),
        fail: std::sync::atomic::AtomicBool::new(false),
    });
    let router = EngineCommandRouter::new(
        handle.clone(),
        Arc::new(Auth),
        Arc::new(Tasks),
        None,
        None,
    );
    let sink = Arc::new(Sink::default());
    router.route(ClientCommand::ClearSession, sink.clone()).await;

    assert!(sink
        .0
        .lock()
        .await
        .iter()
        .any(|event| matches!(event, ClientEvent::SessionEnded)));
    assert_eq!(
        platform_api::live_sessions::process_session_id().as_deref(),
        Some(session_b_text.as_str())
    );
    let live = sessions.find_by_pid(std::process::id()).expect("live B");
    assert_eq!(live.sid(), session_b_text);
    assert_eq!(
        live.messaging_socket_path.as_deref(),
        Some(socket_text.as_str())
    );
    let old = sessions
        .drain_inbox(&session_a_text)
        .unwrap();
    assert_eq!(old.len(), 1);
    assert_eq!(old[0].msg_id.as_deref(), Some("old-generation"));

    handle.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    *handle.next.lock().await = session_c;
    let failed_sink = Arc::new(Sink::default());
    router
        .route(ClientCommand::ClearSession, failed_sink.clone())
        .await;
    assert!(matches!(
        failed_sink.0.lock().await.as_slice(),
        [ClientEvent::Error { .. }]
    ));
    assert_eq!(*handle.current.lock().await, session_b);
    assert_eq!(
        platform_api::live_sessions::process_session_id().as_deref(),
        Some(session_b_text.as_str())
    );
    assert_eq!(
        sessions.find_by_pid(std::process::id()).unwrap().sid(),
        session_b_text
    );

    handle.fail.store(false, std::sync::atomic::Ordering::SeqCst);
    let cwd = root.path().to_string_lossy().into_owned();
    let lingxi_home = root.path().join(branding::DOT_DIR);
    let resume_path = session::jsonl::path::session_path(
        &lingxi_home,
        &cwd,
        &session_c_text,
    );
    std::fs::create_dir_all(resume_path.parent().unwrap()).unwrap();
    std::fs::write(
        &resume_path,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "uuid": "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
                "parentUuid": null,
                "sessionId": session_c_text.clone(),
                "timestamp": "2026-09-06T00:00:00.000Z",
                "cwd": cwd.clone(),
                "version": "test",
                "isSidechain": false,
                "message": {"role": "user", "content": "resume C"}
            })
        ),
    )
    .unwrap();
    let resume_router = EngineCommandRouter::new(
        handle.clone(),
        Arc::new(Auth),
        Arc::new(Tasks),
        None,
        None,
    )
    .with_session_store(SessionStoreContext::new(
        lingxi_home,
        cwd,
        Arc::new(PosixFileSystem::new(root.path().to_path_buf())),
    ));
    let resumed_sink = Arc::new(Sink::default());
    resume_router
        .route(
            ClientCommand::ResumeSession {
                session_id: session_c_text.clone(),
                cwd: None,
            },
            resumed_sink.clone(),
        )
        .await;
    assert!(resumed_sink
        .0
        .lock()
        .await
        .iter()
        .any(|event| matches!(event, ClientEvent::SessionResumed { .. })));
    assert_eq!(*handle.current.lock().await, session_c);
    assert_eq!(
        platform_api::live_sessions::process_session_id().as_deref(),
        Some(session_c_text.as_str())
    );
    assert_eq!(
        sessions.find_by_pid(std::process::id()).unwrap().sid(),
        session_c_text
    );

    let stale = engine_desktop::refresh_process_session_presence(session_a, session_b)
        .await
        .expect_err("a stale A-to-B callback must not replace live C");
    assert!(stale.contains("stale session activation"), "{stale}");
    assert_eq!(
        platform_api::live_sessions::process_session_id().as_deref(),
        Some(session_c_text.as_str())
    );
    assert_eq!(
        sessions.find_by_pid(std::process::id()).unwrap().sid(),
        session_c_text
    );

    platform_api::uds_inbox::stop_process_inbox_checked().unwrap();
}
