//! Optional audio metadata must not reject an ordinary text session.
use async_trait::async_trait;
use bridge_server::router::{CommandRouter, EngineCommandRouter};
use client::adapter::ClientEventSink;
use client::protocol::commands::ClientCommand;
use client::protocol::events::{ClientEvent, ErrorKindDto};
use lingxi_core::host::auth::{AuthError, AuthHandle, LoginInfo};
use lingxi_core::host::orchestrator::StatusSnapshot;
use lingxi_core::host::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};
use orchestrator::test_support::MockOrchestratorHandle;
use std::sync::Arc;
use tokio::sync::Mutex;
#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<ClientEvent>>,
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
struct MockAuth;

#[async_trait]
impl AuthHandle for MockAuth {
    fn register_account_change_observer(
        &self,
        _observer: std::sync::Weak<dyn lingxi_core::host::auth::AccountChangeObserver>,
    ) {
    }

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

struct MockTaskRegistry {
    rows: Vec<TaskRecord>,
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

fn router_with(
    handle: Arc<MockOrchestratorHandle>,
    tasks: Arc<MockTaskRegistry>,
) -> EngineCommandRouter {
    EngineCommandRouter::new(
        handle as Arc<dyn lingxi_core::host::orchestrator::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        tasks as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    )
}

#[tokio::test]
async fn audio_context_probe_without_binding_keeps_text_session_usable() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    handle.set_available_models(vec!["deepseek-chat".into()]);
    handle.set_status_snapshot(StatusSnapshot {
        model: "deepseek-chat".into(),
        ..StatusSnapshot::default()
    });
    let router = router_with(handle, Arc::new(MockTaskRegistry { rows: vec![] }));
    let sink = CapturingSink::arc();

    router
        .route(ClientCommand::GetAudioSessionContext, sink.clone())
        .await;
    assert!(
        sink.events().await.is_empty(),
        "an optional audio probe must not publish a global text-session error"
    );

    router.route(ClientCommand::ListModels, sink.clone()).await;
    let events = sink.events().await;
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::ModelList { current, .. } if current == "deepseek-chat"
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, ClientEvent::Error { .. })));
}

#[tokio::test]
async fn explicit_realtime_audio_still_rejects_an_unavailable_runtime() {
    let router = router_with(
        Arc::new(MockOrchestratorHandle::new()),
        Arc::new(MockTaskRegistry { rows: vec![] }),
    );
    let sink = CapturingSink::arc();
    router
        .route(
            ClientCommand::StartRealtimeAudio {
                request_json: "{}".into(),
            },
            sink.clone(),
        )
        .await;
    assert!(matches!(
        sink.events().await.as_slice(),
        [ClientEvent::Error {
            kind: ErrorKindDto::Rejected,
            ..
        }]
    ));
}
