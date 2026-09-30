//! Real ForceCompact commit versus endpoint shutdown. Only the summarizer and
//! the scheduling pause are doubles: routing, compaction, JSONL, and replay run
//! their production implementations.

use super::*;
use lingxi_core::host::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
use orchestrator::test_support::{
    noop_hook_executor, with_scripted_compactor, MockApiClient, MockOutputStream,
    NoOpPermissionGate, StaticMemoryProvider,
};
use std::pin::Pin;
use tokio::sync::Notify;

struct PauseAfterCompactBoundary {
    inner: NativeFileSystem,
    boundary_durable: Notify,
    finish_commit: Notify,
}

#[async_trait]
impl FileSystem for PauseAfterCompactBoundary {
    async fn read_file(
        &self,
        path: &str,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> Result<FileContent, FsError> {
        self.inner.read_file(path, offset, limit).await
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<(), FsError> {
        self.inner.write_file(path, content).await
    }

    fn is_within_workspace(&self, path: &str) -> bool {
        self.inner.is_within_workspace(path)
    }

    async fn watch(
        &self,
        dir: &str,
    ) -> Result<Pin<Box<dyn futures_util::Stream<Item = FileEvent> + Send>>, FsError> {
        self.inner.watch(dir).await
    }

    async fn append_file(&self, path: &str, content: &str) -> Result<(), FsError> {
        self.append_file_with_mode(path, content, 0o600).await
    }

    async fn append_file_with_mode(
        &self,
        path: &str,
        content: &str,
        mode: u32,
    ) -> Result<(), FsError> {
        self.inner
            .append_file_with_mode(path, content, mode)
            .await?;
        self.inner.fsync(path).await?;
        let is_boundary = content.lines().any(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|record| record["subtype"] == "compact_boundary")
        });
        if is_boundary {
            // The chain reset is already on disk. The command still owes the
            // summary, so cancelling its future here would truncate context.
            self.boundary_durable.notify_one();
            self.finish_commit.notified().await;
        }
        Ok(())
    }

    async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError> {
        self.inner.truncate(path, len).await
    }

    async fn file_mtime(&self, path: &str) -> Result<std::time::SystemTime, FsError> {
        self.inner.file_mtime(path).await
    }

    async fn file_size(&self, path: &str) -> Result<u64, FsError> {
        self.inner.file_size(path).await
    }

    async fn delete_file(&self, path: &str) -> Result<(), FsError> {
        self.inner.delete_file(path).await
    }

    async fn symlink(&self, target: &str, link: &str) -> Result<(), FsError> {
        self.inner.symlink(target, link).await
    }

    async fn flock_exclusive(&self, path: &str) -> Result<Box<dyn FlockGuard>, FsError> {
        self.inner.flock_exclusive(path).await
    }

    async fn fsync(&self, path: &str) -> Result<(), FsError> {
        self.inner.fsync(path).await
    }
}

#[tokio::test]
async fn shutdown_finishes_real_compaction_after_durable_boundary_before_summary() {
    const SUMMARY: &str = "durable compact summary survives endpoint shutdown";
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_string_lossy().into_owned();
    let config_home = root.path().join("home");
    let session_id = uuid::Uuid::new_v4();
    let transcript = config_home
        .join("projects")
        .join(session::jsonl::project_dir_name(&cwd))
        .join(format!("{session_id}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let fs = Arc::new(PauseAfterCompactBoundary {
        inner: NativeFileSystem::new(root.path().to_path_buf()),
        boundary_durable: Notify::new(),
        finish_commit: Notify::new(),
    });
    let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
        transcript.clone(),
        fs.clone(),
    ));
    let mut parent = None;
    for index in 0..6 {
        let id = uuid::Uuid::new_v4().to_string();
        let role = if index % 2 == 0 { "user" } else { "assistant" };
        let row = serde_json::json!({
            "type": role,
            "uuid": id,
            "parentUuid": parent,
            "sessionId": session_id,
            "timestamp": format!("2026-09-07T12:00:0{index}.000Z"),
            "cwd": cwd,
            "version": "0.9.0",
            "isSidechain": false,
            "message": {
                "role": role,
                "content": [{"type": "text", "text": format!("history {index}: preserve this conversational context") }]
            }
        });
        writer
            .append(&serde_json::from_value(row).unwrap())
            .await
            .unwrap();
        parent = Some(id);
    }
    let orchestrator = orchestrator::ConversationOrchestrator::with_resume(
        orchestrator::OrchestratorConfig::default(),
        session_id,
        config_home.clone(),
        cwd.clone(),
        fs.clone(),
        Arc::new(MockApiClient::new(vec![])),
        Arc::new(tool_api::registry::ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        root.path().to_path_buf(),
        Some(writer),
    )
    .await
    .unwrap();
    let orchestrator = Arc::new(with_scripted_compactor(
        orchestrator.with_config_home(config_home.clone()),
        SUMMARY,
    ));
    let router = Arc::new(EngineCommandRouter::new(
        orchestrator.clone(),
        Arc::new(MockAuth),
        Arc::new(MockTaskRegistry { rows: vec![] }),
        None,
        None,
    ));
    let connection = BridgeConnection::new()
        .bind(
            Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink))),
            Arc::new(NoopTurnDriver),
        )
        .bind_router(router);
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .unwrap();
    endpoint.set_auth_token(E2E_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;
    send_hello(&mut ws).await;
    send_command(&mut ws, &ClientCommand::ForceCompact).await;
    tokio::time::timeout(Duration::from_secs(5), fs.boundary_durable.notified())
        .await
        .expect("real compaction must durably write its boundary");
    let before = std::fs::read_to_string(&transcript).unwrap();
    assert!(before.contains("compact_boundary"));
    assert!(!before.contains(SUMMARY));

    let mut shutdown = tokio::spawn(endpoint.shutdown());
    let stopped_before_commit = tokio::time::timeout(Duration::from_millis(100), &mut shutdown)
        .await
        .is_ok();
    // Always release the writer, including the regression case, so failures
    // cannot leave a parked command holding a filesystem/writer resource.
    fs.finish_commit.notify_one();
    if !stopped_before_commit {
        tokio::time::timeout(Duration::from_secs(5), &mut shutdown)
            .await
            .expect("shutdown must finish after command commit")
            .unwrap();
    }
    assert!(
        !stopped_before_commit,
        "shutdown dropped an accepted compaction mid-commit"
    );
    let replayed = orchestrator::replay_session_state(&config_home, &cwd, session_id, fs)
        .await
        .expect("committed transcript must cold replay");
    assert!(replayed
        .state
        .history
        .iter()
        .any(|message| message.text_content().contains(SUMMARY)));
    assert!(orchestrator
        .session()
        .lock()
        .await
        .history
        .iter()
        .any(|message| message.text_content().contains(SUMMARY)));
}
