use super::{CommandRouter, EngineCommandRouter, SessionStoreContext};
use lingxi_core::host::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};
use lingxi_core::host::team_registry::{TeamRegistryHandle, WorkerInfo};
use lingxi_core::host::{AuthError, AuthHandle, LoginInfo};
use std::sync::{Arc, Mutex};

struct SnapshotAuth;
#[async_trait::async_trait]
impl AuthHandle for SnapshotAuth {
    fn register_account_change_observer(
        &self,
        _observer: std::sync::Weak<dyn lingxi_core::host::auth::AccountChangeObserver>,
    ) {
    }

    async fn login(&self) -> Result<LoginInfo, AuthError> {
        unreachable!()
    }
    async fn logout(&self) -> Result<(), AuthError> {
        Ok(())
    }
    async fn current_user(&self) -> Option<LoginInfo> {
        None
    }
}

#[derive(Default)]
struct SnapshotTasks {
    records: Mutex<Vec<TaskRecord>>,
    error: Mutex<Option<String>>,
}
#[async_trait::async_trait]
impl TaskRegistryHandle for SnapshotTasks {
    async fn create(&self, _: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
        unreachable!()
    }
    async fn get(&self, _: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
        Ok(None)
    }
    async fn list(&self, filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
        if let Some(error) = self.error.lock().unwrap().clone() {
            Err(TaskRegistryError::Internal(error))
        } else {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .filter(|record| {
                    filter
                        .status
                        .as_ref()
                        .is_none_or(|status| status == &record.status)
                })
                .cloned()
                .collect())
        }
    }
    async fn update(&self, _: &str, _: TaskUpdatePatch) -> Result<TaskRecord, TaskRegistryError> {
        unreachable!()
    }
    async fn set_status(&self, _: &str, _: &str) -> Result<TaskRecord, TaskRegistryError> {
        unreachable!()
    }
    async fn kill(&self, _: &str) -> Result<TaskRecord, TaskRegistryError> {
        unreachable!()
    }
    async fn output(&self, _: &str, _: Option<u64>) -> Result<TaskOutputChunk, TaskRegistryError> {
        unreachable!()
    }
}

#[derive(Default)]
struct SnapshotTeam {
    workers: Mutex<Vec<WorkerInfo>>,
}
#[async_trait::async_trait]
impl TeamRegistryHandle for SnapshotTeam {
    async fn list_workers(&self) -> Vec<WorkerInfo> {
        self.workers.lock().unwrap().clone()
    }
    async fn team_name(&self) -> Option<String> {
        Some("snapshot-team".into())
    }
}

fn worker(id: &str, status: &str) -> WorkerInfo {
    WorkerInfo {
        agent_id: id.into(),
        name: id.into(),
        agent_type: "explorer".into(),
        status: status.into(),
        ..WorkerInfo::default()
    }
}

fn snapshot_router(team: Arc<SnapshotTeam>) -> EngineCommandRouter {
    EngineCommandRouter::new(
        Arc::new(orchestrator::test_support::MockOrchestratorHandle::new()),
        Arc::new(SnapshotAuth),
        Arc::new(SnapshotTasks::default()),
        None,
        None,
    )
    .with_team_registry(team)
}

#[tokio::test]
async fn runtime_snapshot_refreshes_coordinator_ownership_without_resuming_session() {
    use client::protocol::events::ClientEvent;
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let team = Arc::new(SnapshotTeam::default());
    *team.workers.lock().unwrap() = vec![
        worker("live", "working"),
        worker("waiting", "idle"),
        worker("done", "completed"),
    ];
    let router = snapshot_router(team.clone()).with_session_store(SessionStoreContext::new(
        root.path().join("config"),
        project.to_string_lossy().into_owned(),
        Arc::new(crate::HostFileSystem::new(project)),
    ));
    let events = router.runtime_snapshot().await.unwrap();
    assert!(matches!(
        events.first(),
        Some(ClientEvent::SessionAgentList { .. })
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ClientEvent::CoordinatorWorker { .. }))
            .count(),
        3
    );
    assert!(
        events.iter().any(|event| matches!(event, ClientEvent::CoordinatorStatus { active_workers: 2, team: Some(name) } if name == "snapshot-team"))
    );
    assert!(
        matches!(events.last(), Some(ClientEvent::TaskListComplete { request_id, active_count: 0, error: None }) if request_id == "desktop-runtime-snapshot")
    );
    // A completion missed during a socket gap is read back from the live
    // registry, while idle (message-ready) scopes remain owned until terminal.
    *team.workers.lock().unwrap() = vec![worker("live", "completed"), worker("waiting", "killed")];
    let events = router.runtime_snapshot().await.unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        ClientEvent::CoordinatorStatus {
            active_workers: 0,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(event, ClientEvent::CoordinatorWorker { worker } if worker.agent_id == "live" && worker.status == "completed")));
}

#[tokio::test]
async fn runtime_snapshot_refuses_missing_authoritative_roster() {
    let router = snapshot_router(Arc::new(SnapshotTeam::default()));
    assert_eq!(
        router.runtime_snapshot().await.unwrap_err(),
        "session agent snapshot is unavailable"
    );
    let router = EngineCommandRouter::new(
        Arc::new(orchestrator::test_support::MockOrchestratorHandle::new()),
        Arc::new(SnapshotAuth),
        Arc::new(SnapshotTasks::default()),
        None,
        None,
    );
    assert_eq!(
        router.runtime_snapshot().await.unwrap_err(),
        "desktop runtime snapshot is unavailable"
    );
}

#[tokio::test]
async fn runtime_snapshot_contains_all_live_task_kinds_and_terminal_rows() {
    use client::protocol::events::ClientEvent;
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let tasks = Arc::new(SnapshotTasks::default());
    *tasks.records.lock().unwrap() = [
        ("shell", "local_bash", "running"),
        ("workflow", "local_workflow", "paused"),
        ("fusion", "local_fusion", "pending"),
        ("done", "local_bash", "completed"),
    ]
    .into_iter()
    .map(|(id, kind, status)| TaskRecord {
        task_id: id.into(),
        task_type: kind.into(),
        status: status.into(),
        ..TaskRecord::default()
    })
    .collect();
    let router = EngineCommandRouter::new(
        Arc::new(orchestrator::test_support::MockOrchestratorHandle::new()),
        Arc::new(SnapshotAuth),
        tasks.clone(),
        None,
        None,
    )
    .with_team_registry(Arc::new(SnapshotTeam::default()))
    .with_session_store(SessionStoreContext::new(
        root.path().join("config"),
        project.to_string_lossy().into_owned(),
        Arc::new(crate::HostFileSystem::new(project)),
    ));
    let events = router.runtime_snapshot().await.unwrap();
    let task_ids = events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::TaskRow { task } => Some(task.task_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(task_ids, vec!["shell", "workflow", "fusion", "done"]);
    assert!(matches!(events.last(), Some(ClientEvent::TaskListComplete {
        request_id, active_count: 3, error: None,
    }) if request_id == "desktop-runtime-snapshot"));
    tasks.records.lock().unwrap().clear();
    let empty = router.runtime_snapshot().await.unwrap();
    assert!(!empty
        .iter()
        .any(|event| matches!(event, ClientEvent::TaskRow { .. })));
    assert!(matches!(
        empty.last(),
        Some(ClientEvent::TaskListComplete {
            active_count: 0,
            error: None,
            ..
        })
    ));
    *tasks.error.lock().unwrap() = Some("unreadable registry".into());
    assert!(
        router
            .runtime_snapshot()
            .await
            .unwrap_err()
            .contains("unreadable registry"),
        "partial snapshots must not clear ownership after an unsuccessful task roster read"
    );
}
