use super::{CommandRouter, ConversationMode, EngineCommandRouter, SessionStoreContext};
use client::adapter::ClientEventSink;
use client::protocol::commands::ClientCommand;
use client::protocol::events::ClientEvent;
use client::protocol::listings::SessionModeDto;
use lingxi_core::host::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct UnusedTasks;

#[async_trait::async_trait]
impl TaskRegistryHandle for UnusedTasks {
    async fn create(&self, _: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
        unreachable!()
    }
    async fn get(&self, _: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
        unreachable!()
    }
    async fn list(&self, _: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
        unreachable!()
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
struct Sink(tokio::sync::Mutex<Vec<ClientEvent>>);

#[async_trait::async_trait]
impl ClientEventSink for Sink {
    async fn emit(&self, event: ClientEvent) {
        self.0.lock().await.push(event);
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
    source: PathBuf,
    id: uuid::Uuid,
    content: String,
    router: EngineCommandRouter,
}

impl Fixture {
    fn new(mode: ConversationMode) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let id = uuid::Uuid::new_v4();
        let source = orchestrator::transcript_paths::main_transcript_path(
            &home,
            &cwd.to_string_lossy(),
            &id.to_string(),
        );
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        let content = format!(
            "{}\n{}\n",
            serde_json::json!({"type":"fixed-session-mode","sessionId":id,"sessionMode":mode.as_str(),"instructionEpoch":7}),
            serde_json::json!({"type":"user","uuid":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","parentUuid":null,"sessionId":id,"timestamp":"2026-10-21T00:00:00.000Z","cwd":cwd,"isSidechain":false,"message":{"role":"user","content":"captured source"}}),
        );
        std::fs::write(&source, &content).unwrap();
        let router = EngineCommandRouter::new(
            Arc::new(orchestrator::test_support::MockOrchestratorHandle::new()),
            Arc::new(UnusedTasks),
            None,
            None,
        )
        .with_session_store(SessionStoreContext::new(
            home.clone(),
            cwd.to_string_lossy().into_owned(),
            Arc::new(crate::HostFileSystem::new(cwd.clone())),
        ));
        Self {
            temp,
            home,
            cwd,
            source,
            id,
            content,
            router,
        }
    }

    fn sibling(&self) -> PathBuf {
        self.home
            .join("projects")
            .join("another-worktree")
            .join(format!("{}.jsonl", self.id))
    }

    fn target(&self, id: uuid::Uuid) -> PathBuf {
        orchestrator::transcript_paths::main_transcript_path(
            &self.home,
            &self.cwd.to_string_lossy(),
            &id.to_string(),
        )
    }
}

fn transcript_count(path: &Path) -> usize {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .map(|path| {
            if path.is_dir() {
                transcript_count(&path)
            } else {
                usize::from(path.extension().is_some_and(|ext| ext == "jsonl"))
            }
        })
        .sum()
}

#[tokio::test]
async fn fork_source_rejects_ambiguous_uuid_in_public_command() {
    let fixture = Fixture::new(ConversationMode::Code);
    let sibling = fixture.sibling();
    std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
    std::fs::write(
        &sibling,
        fixture.content.replace("captured source", "newer sibling"),
    )
    .unwrap();
    let sibling_before = std::fs::read(&sibling).unwrap();
    let sink = Arc::new(Sink::default());
    fixture
        .router
        .route(
            ClientCommand::ForkSession {
                session_id: fixture.id.to_string(),
                target_mode: SessionModeDto::Code,
            },
            sink.clone(),
        )
        .await;
    let events = sink.0.lock().await;
    assert!(events
        .iter()
        .any(|event| matches!(event, ClientEvent::Error { message, .. }
        if String::from_utf16_lossy(&message.code_units).contains("ambiguous"))));
    assert!(!events
        .iter()
        .any(|event| matches!(event, ClientEvent::SessionForked { .. })));
    assert_eq!(
        std::fs::read_to_string(&fixture.source).unwrap(),
        fixture.content
    );
    assert_eq!(std::fs::read(sibling).unwrap(), sibling_before);
    assert_eq!(transcript_count(&fixture.home.join("projects")), 2);
}

#[tokio::test]
async fn fork_source_rejects_same_bytes_new_inode_before_claim() {
    let fixture = Fixture::new(ConversationMode::Code);
    let saved = fixture.temp.path().join("retired.jsonl");
    let result = fixture
        .router
        .fork_session_after_capture(fixture.id, ConversationMode::Code, || {
            std::fs::rename(&fixture.source, &saved).unwrap();
            std::fs::write(&fixture.source, &fixture.content).unwrap();
        })
        .await;
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(&fixture.source).unwrap(),
        fixture.content
    );
    assert_eq!(std::fs::read_to_string(saved).unwrap(), fixture.content);
    assert_eq!(transcript_count(&fixture.home.join("projects")), 1);
}

#[tokio::test]
async fn fork_source_and_hot_resume_reject_unterminated_complete_record() {
    let fixture = Fixture::new(ConversationMode::Code);
    let content = fixture.content.strip_suffix('\n').unwrap();
    std::fs::write(&fixture.source, content).unwrap();
    let error = super::replay_pinned_catalog_source(&fixture.home, &fixture.source, fixture.id)
        .err()
        .expect("hot resume must reject an unterminated record");
    assert!(error.contains("incomplete final record"), "{error}");
    let error = fixture
        .router
        .fork_session(fixture.id, ConversationMode::Code)
        .await
        .unwrap_err();
    assert!(error.contains("incomplete final record"), "{error}");
    assert_eq!(std::fs::read_to_string(&fixture.source).unwrap(), content);
    assert_eq!(transcript_count(&fixture.home.join("projects")), 1);
}

#[tokio::test]
async fn fork_source_uses_claimed_snapshot_after_original_is_replaced() {
    let fixture = Fixture::new(ConversationMode::Code);
    let retired = fixture.temp.path().join("retired-after-snapshot.jsonl");
    let replacement = fixture
        .content
        .replace("captured source", "replacement source");
    let target = fixture
        .router
        .fork_session_with_capture_hooks(
            fixture.id,
            ConversationMode::Code,
            || {},
            || {
                std::fs::rename(&fixture.source, &retired).unwrap();
                std::fs::write(&fixture.source, &replacement).unwrap();
            },
        )
        .await
        .expect("a claimed immutable snapshot remains valid after replacement");
    let branch = std::fs::read_to_string(fixture.target(target)).unwrap();
    assert!(branch.contains("captured source"));
    assert!(!branch.contains("replacement source"));
    assert_eq!(
        std::fs::read_to_string(&fixture.source).unwrap(),
        replacement
    );
    assert_eq!(std::fs::read_to_string(retired).unwrap(), fixture.content);
    assert_eq!(transcript_count(&fixture.home.join("projects")), 2);
}

#[tokio::test]
async fn fork_source_first_resume_keeps_published_target_identity() {
    let fixture = Fixture::new(ConversationMode::Code);
    let target = fixture
        .router
        .fork_session(fixture.id, ConversationMode::Code)
        .await
        .unwrap();
    let target_path = fixture.target(target);
    let target_content = std::fs::read(&target_path).unwrap();
    {
        let pending = fixture.router.pending_fork_sources.lock().unwrap();
        let receipt = pending.get(&target).unwrap();
        assert!(receipt.identity.expected_content.is_empty());
        assert_eq!(receipt.content_len, target_content.len());
        assert_eq!(
            receipt.content_sha256,
            remote_control_client::encoding::sha256(&target_content)
        );
    }
    let retired = fixture.temp.path().join("retired-published-target.jsonl");
    // The initial preflight uses the actual published target receipt.
    assert!(fixture
        .router
        .replay_resume_source(&fixture.home, target)
        .await
        .is_ok());
    std::fs::rename(&target_path, &retired).unwrap();
    std::fs::write(&target_path, &target_content).unwrap();
    let command = || ClientCommand::ResumeSession {
        session_id: target.to_string(),
        cwd: None,
        expected_mode: SessionModeDto::Code,
    };
    let error = fixture
        .router
        .preflight_session_transition(&command())
        .await
        .err()
        .expect("preflight must not recapture the replacement");
    assert!(error.1.contains("fork target changed"), "{}", error.1);
    let sink = Arc::new(Sink::default());
    fixture.router.route(command(), sink.clone()).await;
    assert!(sink.0.lock().await.iter().any(|event| matches!(event,
        ClientEvent::Error { message, .. }
        if String::from_utf16_lossy(&message.code_units).contains("fork target changed")
    )));
    assert!(
        fixture
            .router
            .replay_resume_source(&fixture.home, target)
            .await
            .is_err(),
        "failed attempts must retain the original receipt"
    );
    assert_eq!(std::fs::read(&target_path).unwrap(), target_content);
    assert_eq!(std::fs::read(&retired).unwrap(), target_content);
    std::fs::remove_file(&target_path).unwrap();
    std::fs::rename(&retired, &target_path).unwrap();
    assert!(
        fixture
            .router
            .replay_resume_source(&fixture.home, target)
            .await
            .is_ok(),
        "the original published inode remains admissible"
    );
    let mut changed = target_content.clone();
    changed.push(b' ');
    std::fs::write(&target_path, &changed).unwrap();
    assert!(
        fixture
            .router
            .replay_resume_source(&fixture.home, target)
            .await
            .is_err(),
        "same-inode content changes must also reject"
    );
    std::fs::write(&target_path, &target_content).unwrap();
    // This fixture's orchestrator rejects resume; the original receipt must
    // remain available after a failed mount, not only after a failed preflight.
    let sink = Arc::new(Sink::default());
    fixture.router.route(command(), sink.clone()).await;
    assert!(sink.0.lock().await.iter().any(|event| matches!(event,
        ClientEvent::Error { message, .. }
        if String::from_utf16_lossy(&message.code_units).contains("resume_session failed")
    )));
    assert!(fixture
        .router
        .pending_fork_sources
        .lock()
        .unwrap()
        .contains_key(&target));
}

#[tokio::test]
async fn fork_source_refuses_full_receipt_capacity_before_publication() {
    let fixture = Fixture::new(ConversationMode::Code);
    let _all_slots = fixture
        .router
        .pending_fork_capacity
        .clone()
        .try_acquire_many_owned(1024)
        .unwrap();
    let error = fixture
        .router
        .fork_session(fixture.id, ConversationMode::Code)
        .await
        .unwrap_err();
    assert!(error.contains("too many pending fork targets"), "{error}");
    assert_eq!(transcript_count(&fixture.home.join("projects")), 1);
    assert_eq!(
        std::fs::read_to_string(&fixture.source).unwrap(),
        fixture.content
    );
    assert!(fixture
        .router
        .pending_fork_sources
        .lock()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn fork_source_single_uuid_preserves_fixed_mode_and_snapshot() {
    for (mode, dto, other) in [
        (
            ConversationMode::Code,
            SessionModeDto::Code,
            ConversationMode::Chat,
        ),
        (
            ConversationMode::Chat,
            SessionModeDto::Chat,
            ConversationMode::Code,
        ),
    ] {
        let fixture = Fixture::new(mode);
        let error = fixture
            .router
            .fork_session(fixture.id, other)
            .await
            .unwrap_err();
        assert!(error.contains("session_mode_mismatch"));
        assert_eq!(transcript_count(&fixture.home.join("projects")), 1);

        let sink = Arc::new(Sink::default());
        fixture
            .router
            .route(
                ClientCommand::ForkSession {
                    session_id: fixture.id.to_string(),
                    target_mode: dto,
                },
                sink.clone(),
            )
            .await;
        let events = sink.0.lock().await;
        let target = events
            .iter()
            .find_map(|event| match event {
                ClientEvent::SessionForked {
                    source_session_id,
                    session_id,
                    mode: emitted,
                } if source_session_id == &fixture.id.to_string() && *emitted == dto => {
                    Some(uuid::Uuid::parse_str(session_id).unwrap())
                }
                _ => None,
            })
            .expect("a valid single source must produce a fork event");
        let branch = std::fs::read_to_string(fixture.target(target)).unwrap();
        let profile =
            session::JsonlWriter::fixed_session_profile_from_source(&branch, &target.to_string())
                .unwrap();
        assert_eq!(profile.mode, mode);
        assert_eq!(profile.instruction_epoch, 8);
        assert!(branch.contains("captured source"));
        assert_eq!(
            std::fs::read_to_string(&fixture.source).unwrap(),
            fixture.content
        );
        assert_eq!(transcript_count(&fixture.home.join("projects")), 2);
    }
}
