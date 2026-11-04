use super::{ConversationMode, SessionStoreContext};
use lingxi_core::types::SessionId;
#[cfg(unix)]
use platform_posix::PosixFileSystem as NativeFileSystem;
#[cfg(windows)]
use platform_windows::WindowsFileSystem as NativeFileSystem;
use session::jsonl::{DurableTranscriptWriter, JsonlWriter};
use std::path::PathBuf;
use std::sync::Arc;

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
    path: PathBuf,
    id: SessionId,
    fs: Arc<dyn lingxi_core::host::FileSystem>,
    writer: Arc<JsonlWriter>,
    store: SessionStoreContext,
}

impl Fixture {
    async fn new(mode: Option<ConversationMode>) -> Self {
        let id = SessionId::from_uuid(
            uuid::Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap(),
        );
        Self::with_id(mode, id).await
    }

    async fn with_id(mode: Option<ConversationMode>, id: SessionId) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("project");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&cwd).unwrap();
        let fs: Arc<dyn lingxi_core::host::FileSystem> =
            Arc::new(NativeFileSystem::new(temp.path().into()));
        let path = orchestrator::transcript_paths::main_transcript_path(
            &home,
            &cwd.to_string_lossy(),
            &id.as_uuid().to_string(),
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let state = home.join("session-state").join(id.as_uuid().to_string());
        std::fs::create_dir_all(&state).unwrap();
        let authority = Arc::new(DurableTranscriptWriter::open(state).unwrap());
        let writer =
            Arc::new(JsonlWriter::new(path.clone(), fs.clone()).with_durable_lock(authority));
        writer
            .activate_session_target(id, path.clone(), cwd.clone())
            .unwrap();
        if let Some(mode) = mode {
            writer
                .initialize_fixed_session_mode(&id.as_uuid().to_string(), mode, 0)
                .await
                .unwrap();
        }
        let store =
            SessionStoreContext::new(home.clone(), cwd.to_string_lossy().into_owned(), fs.clone())
                .with_transcript_writer(Some(writer.clone()));
        Self {
            _temp: temp,
            home,
            cwd,
            path,
            id,
            fs,
            writer,
            store,
        }
    }
}

#[tokio::test]
async fn scheduled_chat_anchor_is_listed_without_messages_and_is_idempotent() {
    for mode in [ConversationMode::Chat, ConversationMode::Code] {
        for metadata_only in [false, true] {
            let fixture = Fixture::new(Some(mode)).await;
            if metadata_only {
                fixture
                    .writer
                    .append_permission_mode("default")
                    .await
                    .unwrap();
            }
            let id = fixture.id.as_uuid().to_string();
            let prefixed = format!("sess:{id}");
            let (first, second) = tokio::join!(
                fixture
                    .store
                    .ensure_scheduled_chat(&prefixed, mode, &fixture.cwd, "Captured task"),
                fixture
                    .store
                    .ensure_scheduled_chat(&id, mode, &fixture.cwd, "Captured task"),
            );
            first.unwrap();
            second.unwrap();
            let content = std::fs::read_to_string(&fixture.path).unwrap();
            assert_eq!(content.matches("\"mobileEmptySession\":1").count(), 1);
            assert!(content.contains("\"customTitle\":\"Captured task\""));
            fixture
                .store
                .ensure_scheduled_chat(&id, mode, &fixture.cwd, "Captured task")
                .await
                .unwrap();
            assert_eq!(std::fs::read_to_string(&fixture.path).unwrap(), content);
            let catalog = session::jsonl::list_recent_sessions_with_diagnostics(
                &fixture.home,
                &fixture.cwd.to_string_lossy(),
                10,
                fixture.fs.clone(),
            )
            .await
            .unwrap();
            assert_eq!(catalog.sessions.len(), 1);
            assert_eq!(catalog.sessions[0].message_count, 0);
        }
    }
}

#[tokio::test]
async fn scheduled_anchor_preserves_existing_conversation_without_an_empty_marker() {
    use std::io::Write;

    let fixture = Fixture::new(Some(ConversationMode::Code)).await;
    let id = fixture.id.as_uuid().to_string();
    let user = serde_json::json!({
        "type": "user",
        "uuid": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "parentUuid": null,
        "sessionId": &id,
        "cwd": &fixture.cwd,
        "message": { "role": "user", "content": "existing conversation" },
    });
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&fixture.path)
            .unwrap(),
        "{user}"
    )
    .unwrap();
    let before = std::fs::read(&fixture.path).unwrap();
    fixture
        .store
        .ensure_scheduled_chat(&id, ConversationMode::Code, &fixture.cwd, "Captured task")
        .await
        .unwrap();
    assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
}

#[tokio::test]
async fn scheduled_anchor_rejects_missing_wrong_mode_and_foreign_owner_without_writing() {
    let fixture = Fixture::new(None).await;
    let id = fixture.id.as_uuid().to_string();
    assert!(fixture
        .store
        .ensure_scheduled_chat(&id, ConversationMode::Chat, &fixture.cwd, "Captured task")
        .await
        .is_err());
    assert!(!fixture.path.exists());

    let no_header =
        format!("{{\"type\":\"custom-title\",\"sessionId\":\"{id}\",\"customTitle\":\"old\"}}\n");
    std::fs::write(&fixture.path, &no_header).unwrap();
    assert!(fixture
        .store
        .ensure_scheduled_chat(&id, ConversationMode::Chat, &fixture.cwd, "Captured task")
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(&fixture.path).unwrap(), no_header);

    let valid = format!("{{\"type\":\"fixed-session-mode\",\"sessionId\":\"{id}\",\"sessionMode\":\"chat\",\"instructionEpoch\":0}}\n");
    std::fs::write(&fixture.path, &valid).unwrap();
    assert!(fixture
        .store
        .ensure_scheduled_chat(&id, ConversationMode::Code, &fixture.cwd, "Captured task")
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(&fixture.path).unwrap(), valid);
    let wrong_cwd = fixture.cwd.join("different");
    assert!(fixture
        .store
        .ensure_scheduled_chat(&id, ConversationMode::Chat, &wrong_cwd, "Captured task")
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(&fixture.path).unwrap(), valid);

    let foreign = format!("{valid}{{\"type\":\"custom-title\",\"sessionId\":\"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee\",\"customTitle\":\"foreign\"}}\n");
    std::fs::write(&fixture.path, &foreign).unwrap();
    assert!(fixture
        .store
        .ensure_scheduled_chat(&id, ConversationMode::Chat, &fixture.cwd, "Captured task")
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(&fixture.path).unwrap(), foreign);

    let corrupt = format!(
        "{valid}{{\"type\":\"custom-title\",\"sessionId\":\"{id}\",\"customTitle\":\"scheduled\",\"mobileEmptySession\":1,\"cwd\":{}}}\n{{broken\n",
        serde_json::to_string(&fixture.cwd.to_string_lossy()).unwrap()
    );
    std::fs::write(&fixture.path, &corrupt).unwrap();
    assert!(fixture
        .store
        .ensure_scheduled_chat(&id, ConversationMode::Chat, &fixture.cwd, "Captured task")
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(&fixture.path).unwrap(), corrupt);
}

#[tokio::test]
async fn scheduled_anchor_never_follows_a_retargeted_writer_to_another_owner() {
    let fixture = Fixture::new(Some(ConversationMode::Chat)).await;
    let old_id = fixture.id.as_uuid().to_string();
    let old_before = std::fs::read(&fixture.path).unwrap();
    let new_id = SessionId::from_uuid(
        uuid::Uuid::parse_str("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
    );
    let new_path = orchestrator::transcript_paths::main_transcript_path(
        &fixture.home,
        &fixture.cwd.to_string_lossy(),
        &new_id.as_uuid().to_string(),
    );
    let state = fixture
        .home
        .join("session-state")
        .join(new_id.as_uuid().to_string());
    std::fs::create_dir_all(&state).unwrap();
    let authority = Arc::new(DurableTranscriptWriter::open(state).unwrap());
    fixture.writer.activate_session_target_with_durable_lock(
        new_id,
        new_path.clone(),
        fixture.cwd.clone(),
        authority,
    );
    fixture
        .writer
        .initialize_fixed_session_mode(&new_id.as_uuid().to_string(), ConversationMode::Code, 0)
        .await
        .unwrap();
    let new_before = std::fs::read(&new_path).unwrap();

    assert!(fixture
        .store
        .ensure_scheduled_chat(
            &old_id,
            ConversationMode::Chat,
            &fixture.cwd,
            "Captured task"
        )
        .await
        .is_err());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), old_before);
    assert_eq!(std::fs::read(&new_path).unwrap(), new_before);
}

#[cfg(unix)]
#[tokio::test]
async fn scheduled_create_ack_follows_anchor_and_rejects_wrong_mode_without_a_job() {
    use super::{CommandRouter, EngineCommandRouter};
    use client::adapter::ClientEventSink;
    use client::protocol::commands::{ClientCommand, CronRequestDto};
    use client::protocol::events::ClientEvent;
    use lingxi_core::host::orchestrator::{OrchestratorHandle, StatusSnapshot};

    struct AckSink {
        path: PathBuf,
        events: tokio::sync::Mutex<Vec<(ClientEvent, Option<String>)>>,
    }
    #[async_trait::async_trait]
    impl ClientEventSink for AckSink {
        async fn emit(&self, event: ClientEvent) {
            let source = std::fs::read_to_string(&self.path).ok();
            self.events.lock().await.push((event, source));
        }
    }

    for matching_mode in [false, true] {
        let handle = Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
        let id = handle.current_session_id().await;
        let canonical_id = id.as_uuid().to_string();
        let fixture = Fixture::with_id(
            Some(if matching_mode {
                ConversationMode::Code
            } else {
                ConversationMode::Chat
            }),
            id,
        )
        .await;
        handle.set_workspace_trusted(true);
        handle.set_status_snapshot(StatusSnapshot {
            session_id: format!("sess:{}", canonical_id.to_uppercase()),
            cwd: fixture.cwd.clone(),
            ..StatusSnapshot::default()
        });
        let runtime = Arc::new(platform_posix::PosixRuntime::new());
        let registry = Arc::new(tasks::registry::TaskRegistry::new(
            runtime.clone(),
            fixture.fs.clone(),
            Arc::new(tasks::output_manager::TaskOutputManager::new(
                fixture.home.join("task-output"),
                fixture.fs.clone(),
            )),
        ));
        let scheduler = Arc::new(cron::CronScheduler::new(
            registry.clone(),
            fixture.fs.clone(),
            Arc::new(platform_posix::PosixClock),
            runtime,
            cron::scheduled_tasks_path(&fixture.cwd),
        ));
        scheduler.clone().start().await.unwrap();
        let path = fixture.path.clone();
        let cwd = fixture.cwd.clone();
        let router = EngineCommandRouter::new(handle, registry, None, None)
            .with_session_store(fixture.store);
        let sink = Arc::new(AckSink {
            path: path.clone(),
            events: tokio::sync::Mutex::new(Vec::new()),
        });
        router
            .route(
                ClientCommand::CronManage {
                    request_id: "create".into(),
                    request: CronRequestDto {
                        automation: Some(
                            serde_json::from_value(serde_json::json!({
                                "version":2,"name":"Weekly report","status":"active",
                                "model":"openai/gpt-test","reasoning":{"type":"automatic"},
                                "runMode":"new_session","sessionMode":"code",
                                "sessionScopeKey":"project.test","notificationPolicy":"all"
                            }))
                            .unwrap(),
                        ),
                        action: "create".into(),
                        id: None,
                        cron: Some("0 9 * * 1".into()),
                        prompt: Some("scheduled work".into()),
                        recurring: Some(true),
                        durable: Some(true),
                        expires_at: None,
                        no_expiry: Some(true),
                    },
                },
                sink.clone(),
            )
            .await;
        let events = sink.events.lock().await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            (ClientEvent::CronResult { jobs, error, .. }, source) if matching_mode => {
                assert!(error.is_none(), "unexpected create error: {error:?}");
                assert_eq!(jobs.len(), 1);
                assert_eq!(jobs[0].session_id.as_deref(), Some(canonical_id.as_str()));
                assert!(source
                    .as_deref()
                    .unwrap()
                    .contains("\"mobileEmptySession\":1"));
                assert!(source
                    .as_deref()
                    .unwrap()
                    .contains("\"customTitle\":\"Weekly report\""));
                assert!(cron::scheduled_tasks_path(&cwd).exists());
            }
            (ClientEvent::CronResult { jobs, error, .. }, source) => {
                assert!(jobs.is_empty());
                assert!(error.is_some());
                assert!(!source
                    .as_deref()
                    .unwrap()
                    .contains("\"mobileEmptySession\":1"));
                assert!(!cron::scheduled_tasks_path(&cwd).exists());
            }
            _ => panic!("expected one correlated cron result"),
        }
        drop(events);
        scheduler.stop().await.unwrap();
    }
}
