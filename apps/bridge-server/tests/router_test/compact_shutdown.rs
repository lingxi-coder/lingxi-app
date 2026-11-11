//! Manual compact cancellation before commit and draining after commit. Only
//! the summarizer and scheduling pauses are doubles: framing, routing, slash
//! dispatch, compaction, JSONL, and cold replay use production implementations.

use super::*;
use lingxi_core::host::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
use orchestrator::test_support::{
    noop_hook_executor, with_compaction_client, MockApiClient, MockOutputStream,
    NoOpPermissionGate, SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse,
    StaticMemoryProvider,
};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
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

    async fn read_file_rooted_no_follow(
        &self,
        root: &Path,
        relative: &Path,
    ) -> Result<FileContent, FsError> {
        self.inner.read_file_rooted_no_follow(root, relative).await
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

const SUMMARY: &str = "durable compact summary survives disconnect";
const FOCUS: &str = "preserve exact profile and thinking context";

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct ControlledSummary {
    block: bool,
    calls: AtomicUsize,
    requests: StdMutex<Vec<SideQueryRequest>>,
    started: Notify,
    cancelled: Notify,
}

impl ControlledSummary {
    fn new(block: bool) -> Arc<Self> {
        Arc::new(Self {
            block,
            calls: AtomicUsize::new(0),
            requests: StdMutex::new(Vec::new()),
            started: Notify::new(),
            cancelled: Notify::new(),
        })
    }
}

struct PendingSummary<'a>(&'a Notify);

impl Drop for PendingSummary<'_> {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[async_trait]
impl SideQueryClient for ControlledSummary {
    async fn query(&self, request: SideQueryRequest) -> Result<SideQueryResponse, SideQueryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request);
        self.started.notify_one();
        if self.block {
            let _pending = PendingSummary(&self.cancelled);
            std::future::pending::<()>().await;
        }
        Ok(SideQueryResponse {
            text: Some(format!("<summary>{SUMMARY}</summary>").into()),
            structured: None,
            tool_calls: Vec::new(),
            usage: Default::default(),
            stop_reason: Some("end_turn".into()),
            retry_count: 0,
        })
    }
}

#[derive(Default)]
struct CountingTurnDriver(AtomicUsize);

#[async_trait]
impl bridge_server::server::TurnDriver for CountingTurnDriver {
    async fn run_turn(&self, _prompt: Utf16JsonProjection) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct CompactFixture {
    _root: tempfile::TempDir,
    cwd: String,
    config_home: PathBuf,
    session_id: uuid::Uuid,
    transcript: PathBuf,
    fs: Arc<PauseAfterCompactBoundary>,
    orchestrator: Arc<orchestrator::ConversationOrchestrator>,
    api: Arc<MockApiClient>,
    driver: Arc<CountingTurnDriver>,
    endpoint: McpEndpoint,
}

async fn compact_fixture(summary: Arc<ControlledSummary>) -> CompactFixture {
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
    writer
        .initialize_fixed_session_mode(
            &session_id.to_string(),
            lingxi_core::host::session_context::ConversationMode::Code,
            0,
        )
        .await
        .unwrap();
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
    let api = Arc::new(MockApiClient::new(vec![]));
    let orchestrator = orchestrator::ConversationOrchestrator::with_resume(
        orchestrator::OrchestratorConfig::default(),
        session_id,
        config_home.clone(),
        cwd.clone(),
        fs.clone(),
        api.clone(),
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
    let orchestrator = Arc::new(with_compaction_client(
        orchestrator.with_config_home(config_home.clone()),
        summary,
    ));
    let mut commands = command_api::registry::CommandRegistry::new();
    command_api::builtins::register_all_builtin_commands(&mut commands);
    commands.register_builtin_handler(Arc::new(command_api::builtins::CompactHandler::new(
        orchestrator.clone(),
    )));
    let dispatcher = Arc::new(command_api::dispatcher::RegistrySlashDispatcher::new(
        Arc::new(tokio::sync::RwLock::new(commands)),
    ));
    let router = Arc::new(EngineCommandRouter::new(
        orchestrator.clone(),
        Arc::new(MockTaskRegistry { rows: vec![] }),
        Some(dispatcher),
        None,
    ));
    let driver = Arc::new(CountingTurnDriver::default());
    let connection = BridgeConnection::new()
        .bind(
            Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink))),
            driver.clone(),
        )
        .bind_router(router);
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .unwrap();
    endpoint.set_auth_token(E2E_TOKEN.to_string());
    CompactFixture {
        _root: root,
        cwd,
        config_home,
        session_id,
        transcript,
        fs,
        orchestrator,
        api,
        driver,
        endpoint,
    }
}
fn compact_command(slash: bool) -> ClientCommand {
    if slash {
        ClientCommand::RunSlashCommand {
            raw: format!("/compact {FOCUS}").into(),
            images: Vec::new(),
            turn_id: Some(41),
        }
    } else {
        ClientCommand::ForceCompact
    }
}

async fn try_authenticated_connection(port: u16) -> Result<TestSocket, String> {
    let mut ws = connect(port).await;
    let hello = Frame::Request(BridgeRequest {
        trace_context: None,
        id: 1,
        method: "hello".into(),
        params: bridge_source(&ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
            client_name: "compact-reconnect-test".into(),
            capabilities: Capabilities::default(),
        }),
    });
    ws.send(Message::Text(serde_json::to_string(&hello).unwrap()))
        .await
        .unwrap();
    match next_frame(&mut ws).await {
        Frame::Response(response) if response.error.is_none() => Ok(ws),
        Frame::Response(response) => Err(format!("{:?}", response.error)),
        other => panic!("expected authenticated hello response, got {other:?}"),
    }
}

async fn reconnect(port: u16) -> TestSocket {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(ws) = try_authenticated_connection(port).await {
                return ws;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled or committed compact must release the same runtime's socket owner")
}

async fn assert_no_replay(fixture: &CompactFixture, summary: &ControlledSummary, slash: bool) {
    assert_eq!(summary.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.driver.0.load(Ordering::SeqCst), 0);
    assert!(fixture.api.captured_requests().await.is_empty());
    assert_eq!(
        fixture
            .orchestrator
            .session()
            .lock()
            .await
            .session_id
            .as_uuid(),
        fixture.session_id,
    );
    if slash {
        let requests = summary.requests.lock().unwrap();
        assert!(requests[0]
            .messages
            .iter()
            .any(|message| message.text_content().contains(FOCUS)));
    }
}

async fn assert_disconnect_cancels_summary(slash: bool) {
    let summary = ControlledSummary::new(true);
    let fixture = compact_fixture(summary.clone()).await;
    let original_bytes = std::fs::read(&fixture.transcript).unwrap();
    let original_history = fixture.orchestrator.session().lock().await.history.clone();
    let mut ws = connect(fixture.endpoint.port()).await;
    send_hello(&mut ws).await;
    send_command(&mut ws, &compact_command(slash)).await;
    tokio::time::timeout(Duration::from_secs(5), summary.started.notified())
        .await
        .expect("manual compact reaches the real summarizer path");
    drop(ws);
    tokio::time::timeout(Duration::from_secs(1), summary.cancelled.notified())
        .await
        .expect("disconnect must cancel the precommit summary");
    let replacement = reconnect(fixture.endpoint.port()).await;
    assert_eq!(std::fs::read(&fixture.transcript).unwrap(), original_bytes);
    assert_eq!(
        fixture.orchestrator.session().lock().await.history,
        original_history
    );
    assert_no_replay(&fixture, &summary, slash).await;
    drop(replacement);
    fixture.endpoint.shutdown().await;
}

async fn assert_disconnect_drains_commit(slash: bool) {
    let summary = ControlledSummary::new(false);
    let fixture = compact_fixture(summary.clone()).await;
    let mut ws = connect(fixture.endpoint.port()).await;
    send_hello(&mut ws).await;
    send_command(&mut ws, &compact_command(slash)).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture.fs.boundary_durable.notified(),
    )
    .await
    .expect("real compaction durably writes its boundary");
    let before = std::fs::read_to_string(&fixture.transcript).unwrap();
    assert!(before.contains("compact_boundary"));
    assert!(!before.contains(SUMMARY));
    drop(ws);

    // The replacement must not acquire the writer's runtime before the full
    // transition settles, including when compact runs on the slash task.
    // Timing out also abandons a hello parked behind the drain: that socket's
    // close notification must not wait behind its own unpolled admission.
    let replacement_before_commit = tokio::time::timeout(
        Duration::from_millis(100),
        try_authenticated_connection(fixture.endpoint.port()),
    )
    .await;
    fixture.fs.finish_commit.notify_one();
    assert!(
        !matches!(replacement_before_commit, Ok(Ok(_))),
        "reconnect crossed an unfinished compact commit"
    );
    let replacement = reconnect(fixture.endpoint.port()).await;
    let replayed = orchestrator::replay_session_state(
        &fixture.config_home,
        &fixture.cwd,
        fixture.session_id,
        fixture.fs.clone(),
    )
    .await
    .expect("committed transcript must cold replay");
    assert_eq!(
        replayed
            .state
            .history
            .iter()
            .filter(|message| message.text_content().contains(SUMMARY))
            .count(),
        1,
    );
    assert_eq!(
        fixture
            .orchestrator
            .session()
            .lock()
            .await
            .history
            .iter()
            .filter(|message| message.text_content().contains(SUMMARY))
            .count(),
        1,
    );
    assert_no_replay(&fixture, &summary, slash).await;
    drop(replacement);
    fixture.endpoint.shutdown().await;
}

#[tokio::test]
async fn force_compact_disconnect_cancels_precommit_summary_and_reconnects() {
    assert_disconnect_cancels_summary(false).await;
}

#[tokio::test]
async fn slash_compact_disconnect_cancels_precommit_summary_and_reconnects() {
    assert_disconnect_cancels_summary(true).await;
}

#[tokio::test]
async fn force_compact_disconnect_drains_irreversible_commit_before_reconnect() {
    assert_disconnect_drains_commit(false).await;
}

#[tokio::test]
async fn slash_compact_disconnect_drains_irreversible_commit_before_reconnect() {
    assert_disconnect_drains_commit(true).await;
}

#[tokio::test]
async fn shutdown_finishes_real_compaction_after_durable_boundary_before_summary() {
    let fixture = compact_fixture(ControlledSummary::new(false)).await;
    let mut ws = connect(fixture.endpoint.port()).await;
    send_hello(&mut ws).await;
    send_command(&mut ws, &ClientCommand::ForceCompact).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture.fs.boundary_durable.notified(),
    )
    .await
    .expect("real compaction must durably write its boundary");
    let before = std::fs::read_to_string(&fixture.transcript).unwrap();
    assert!(before.contains("compact_boundary"));
    assert!(!before.contains(SUMMARY));

    let mut shutdown = tokio::spawn(fixture.endpoint.shutdown());
    let stopped_before_commit = tokio::time::timeout(Duration::from_millis(100), &mut shutdown)
        .await
        .is_ok();
    fixture.fs.finish_commit.notify_one();
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
    let replayed = orchestrator::replay_session_state(
        &fixture.config_home,
        &fixture.cwd,
        fixture.session_id,
        fixture.fs,
    )
    .await
    .expect("committed transcript must cold replay");
    assert!(replayed
        .state
        .history
        .iter()
        .any(|message| message.text_content().contains(SUMMARY)));
    assert!(fixture
        .orchestrator
        .session()
        .lock()
        .await
        .history
        .iter()
        .any(|message| message.text_content().contains(SUMMARY)));
}
