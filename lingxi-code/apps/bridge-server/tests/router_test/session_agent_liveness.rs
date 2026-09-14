use super::*;
use engine_desktop::session_agents::DesktopSessionAgentObserver;
use platform_api::subagent_spawn::{SubagentObservation, SubagentSpawnObserver};

fn observer(session_id: protocol::SessionId) -> Arc<DesktopSessionAgentObserver> {
    Arc::new(DesktopSessionAgentObserver::new(
        CapturingSink::arc(),
        session_id.as_uuid().to_string(),
    ))
}

fn allocation(agent_id: protocol::AgentId) -> SubagentObservation {
    SubagentObservation::Allocated {
        agent_id,
        agent_type: "reviewer".into(),
        name: Some("code-review".into()),
        model: "test-model".into(),
        model_profile: None,
        persistent: true,
        initial_message_index: 0,
        origin_session_id: None,
    }
}

async fn roster(
    router: &EngineCommandRouter,
) -> Vec<client_protocol::listings::SessionAgentSummaryDto> {
    let sink = CapturingSink::arc();
    router
        .route(ClientCommand::ListSessionAgents, sink.clone())
        .await;
    match sink.events().await.as_slice() {
        [ClientEvent::SessionAgentList { agents, .. }] => agents.clone(),
        events => panic!("expected roster, got {events:?}"),
    }
}

#[tokio::test]
async fn session_agent_reconnect_retains_foreground_execution_but_restart_clears_it() {
    let root = tempfile::tempdir().unwrap();
    let handle = Arc::new(MockOrchestratorHandle::new());
    let session_id = handle.current_session_id().await;
    let (id, path) = seed_session_agent_transcript(root.path(), session_id, true);
    std::fs::write(
        &path,
        std::fs::read_to_string(&path)
            .unwrap()
            .replace("\"idle\"", "\"running\""),
    )
    .unwrap();
    let live = observer(session_id);
    // A foreground child has no background task-registry row. The synchronous
    // allocation receipt is already authoritative before async UI delivery.
    live.on_allocated(&allocation(id));
    let router = router_with_store(handle.clone(), root.path()).with_session_agent_observer(live);
    for _ in 0..2 {
        assert_eq!(
            roster(&router)
                .await
                .iter()
                .find(|agent| agent.agent_id == id.to_string())
                .unwrap()
                .status,
            "running"
        );
    }
    let restarted =
        router_with_store(handle, root.path()).with_session_agent_observer(observer(session_id));
    assert_eq!(
        roster(&restarted)
            .await
            .iter()
            .find(|agent| agent.agent_id == id.to_string())
            .unwrap()
            .status,
        "cancelled"
    );
}

#[tokio::test]
async fn session_agent_allocation_without_transcript_is_visible_and_session_fenced() {
    let root = tempfile::tempdir().unwrap();
    let handle = Arc::new(MockOrchestratorHandle::new());
    let session_id = handle.current_session_id().await;
    let id = protocol::AgentId::new();
    let live = observer(session_id);
    live.on_allocated(&allocation(id));
    let router = router_with_store(handle.clone(), root.path()).with_session_agent_observer(live);
    assert!(roster(&router)
        .await
        .iter()
        .any(|agent| agent.agent_id == id.to_string() && agent.status == "running"));
    let foreign = observer(protocol::SessionId::new());
    foreign.on_allocated(&allocation(id));
    let router = router_with_store(handle, root.path()).with_session_agent_observer(foreign);
    assert_eq!(roster(&router).await.len(), 1);
}

#[tokio::test]
async fn session_agent_terminal_observation_wins_over_stale_disk_and_registry() {
    let root = tempfile::tempdir().unwrap();
    let handle = Arc::new(MockOrchestratorHandle::new());
    let session_id = handle.current_session_id().await;
    let (id, path) = seed_session_agent_transcript(root.path(), session_id, false);
    std::fs::write(
        &path,
        std::fs::read_to_string(&path)
            .unwrap()
            .replace("\"idle\"", "\"running\""),
    )
    .unwrap();
    let live = observer(session_id);
    live.on_event(allocation(id)).await;
    live.on_event(SubagentObservation::Killed { agent_id: id })
        .await;
    let tasks = Arc::new(MockTaskRegistry {
        rows: vec![TaskRecord {
            task_id: "live-task".into(),
            task_type: "local_agent".into(),
            status: "running".into(),
            owner_agent_id: Some(id.to_string()),
            ..Default::default()
        }],
    });
    let router =
        router_with_store_and_tasks(handle, root.path(), tasks).with_session_agent_observer(live);
    assert_eq!(
        roster(&router)
            .await
            .iter()
            .find(|agent| agent.agent_id == id.to_string())
            .unwrap()
            .status,
        "killed"
    );
}

#[tokio::test]
async fn session_agent_registry_overrides_saved_status_for_live_and_parked_workers() {
    for (status, parked, expected) in [
        ("running", false, "running"),
        ("completed", true, "idle"),
        ("failed", false, "failed"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let handle = Arc::new(MockOrchestratorHandle::new());
        let session_id = handle.current_session_id().await;
        let (id, _) = seed_session_agent_transcript(root.path(), session_id, false);
        let tasks = Arc::new(MockTaskRegistry {
            rows: vec![TaskRecord {
                task_id: "live-task".into(),
                task_type: "local_agent".into(),
                status: status.into(),
                is_parked: parked,
                owner_agent_id: Some(id.to_string()),
                ..Default::default()
            }],
        });
        let router = router_with_store_and_tasks(handle, root.path(), tasks);
        assert_eq!(
            roster(&router)
                .await
                .iter()
                .find(|agent| agent.agent_id == id.to_string())
                .unwrap()
                .status,
            expected
        );
    }
}

#[tokio::test]
async fn session_agent_idle_and_resume_observations_win_over_lagging_task_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let handle = Arc::new(MockOrchestratorHandle::new());
    let session_id = handle.current_session_id().await;
    let (id, _) = seed_session_agent_transcript(root.path(), session_id, false);
    let live = observer(session_id);
    live.on_event(allocation(id)).await;
    live.on_event(SubagentObservation::Completed {
        agent_id: id,
        content: serde_json::json!("done"),
        usage: Default::default(),
        total_tool_use_count: 0,
        total_duration_ms: 0,
        assistant_message_count: 0,
        last_request_id: None,
    })
    .await;
    let tasks = Arc::new(MockTaskRegistry {
        rows: vec![TaskRecord {
            task_id: "lagging-task".into(),
            task_type: "local_agent".into(),
            status: "running".into(),
            owner_agent_id: Some(id.to_string()),
            ..Default::default()
        }],
    });
    let router = router_with_store_and_tasks(handle.clone(), root.path(), tasks)
        .with_session_agent_observer(live.clone());
    assert_eq!(
        roster(&router)
            .await
            .iter()
            .find(|agent| agent.agent_id == id.to_string())
            .unwrap()
            .status,
        "idle"
    );
    live.on_event(SubagentObservation::Message {
        agent_id: id,
        message: protocol::ConversationMessage::user(protocol::MessageId::new(), "continue".into()),
    })
    .await;
    let tasks = Arc::new(MockTaskRegistry {
        rows: vec![TaskRecord {
            task_id: "lagging-task".into(),
            task_type: "local_agent".into(),
            status: "completed".into(),
            is_parked: true,
            owner_agent_id: Some(id.to_string()),
            ..Default::default()
        }],
    });
    let router =
        router_with_store_and_tasks(handle, root.path(), tasks).with_session_agent_observer(live);
    assert_eq!(
        roster(&router)
            .await
            .iter()
            .find(|agent| agent.agent_id == id.to_string())
            .unwrap()
            .status,
        "running"
    );
}
