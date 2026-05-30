//! M4-04 cross-tool integration tests: `TodoWrite` state machine, plan mode
//! toggle, and worktree enter→exit roundtrip. All hermetic — no git CLI,
//! no real filesystem. Worktree calls go through a local `LocalMockWorktree`
//! `WorktreeManager` so the test never touches `lingxi-test-harness`.

use async_trait::async_trait;
use engine::{SessionState, TodoState};
use protocol::SessionId;
use serde_json::json;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use telemetry::AnalyticsBus;
use tokio::sync::Mutex;
use tools::builtin::plan_mode::{
    EnterPlanModeTool, ExitPlanModeTool, PLAN_MODE_ENTER_MARKER, PLAN_MODE_EXIT_MARKER,
};
use tools::builtin::todo_write::TodoWriteTool;
use tools::builtin::worktree::{EnterWorktreeTool, ExitWorktreeTool};
use tools::builtin::BuiltinToolContext;
use tools::context::{ToolUseContext, ToolUseOptions};
use tools::progress::progress_channel;
use tools::tool_trait::Tool;
use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
use traits::worktree::{WorktreeError, WorktreeHandle, WorktreeInfo, WorktreeManager};

// ----- Stubs ----------------------------------------------------------------

struct PanickingFs;
#[async_trait]
impl FileSystem for PanickingFs {
    async fn read_file(
        &self,
        _: &str,
        _: Option<u64>,
        _: Option<u64>,
    ) -> Result<FileContent, FsError> {
        panic!("workflow integration test does not call FileSystem::read_file")
    }
    async fn write_file(&self, _: &str, _: &str) -> Result<(), FsError> {
        panic!("not called")
    }
    fn is_within_workspace(&self, _: &str) -> bool {
        true
    }
    async fn watch(
        &self,
        _: &str,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError> {
        panic!("not called")
    }
    async fn append_file(&self, _: &str, _: &str) -> Result<(), FsError> {
        panic!("not called")
    }
    async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> {
        panic!("not called")
    }
    async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
        panic!("not called")
    }
    async fn file_size(&self, _: &str) -> Result<u64, FsError> {
        panic!("not called")
    }
    async fn delete_file(&self, _: &str) -> Result<(), FsError> {
        panic!("not called")
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
        panic!("not called")
    }
    async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
        panic!("not called")
    }
    async fn fsync(&self, _: &str) -> Result<(), FsError> {
        panic!("not called")
    }
}

struct NoopHttp;
#[async_trait]
impl traits::http::HttpTransport for NoopHttp {
    async fn request(
        &self,
        _: protocol::HttpRequest,
    ) -> Result<protocol::HttpResponse, traits::http::HttpError> {
        Err(traits::http::HttpError::InvalidRequest(
            "NoopHttp: workflow integration".into(),
        ))
    }
    async fn stream_sse(
        &self,
        _: protocol::HttpRequest,
    ) -> Result<traits::http::SseStream, traits::http::HttpError> {
        Err(traits::http::HttpError::InvalidRequest(
            "NoopHttp: stream_sse not supported".into(),
        ))
    }
}

struct NoopProcess;
#[async_trait]
impl traits::process::ProcessRunner for NoopProcess {
    async fn run(
        &self,
        _: &traits::sandbox::SandboxedCommand,
    ) -> Result<traits::process::ProcessOutput, traits::process::ProcessError> {
        panic!("not called")
    }
    async fn spawn_background(
        &self,
        _: &traits::sandbox::SandboxedCommand,
    ) -> Result<traits::process::ProcessHandle, traits::process::ProcessError> {
        panic!("not called")
    }
    async fn kill(
        &self,
        _: &traits::process::ProcessHandle,
    ) -> Result<(), traits::process::ProcessError> {
        Ok(())
    }
    fn is_available(&self) -> bool {
        true
    }
}

struct NoopSandbox;
#[async_trait]
impl traits::sandbox::Sandbox for NoopSandbox {
    fn is_available(&self) -> bool {
        true
    }
    fn backend(&self) -> traits::sandbox::SandboxBackend {
        traits::sandbox::SandboxBackend::None
    }
    fn prepare(
        &self,
        cmd: traits::sandbox::ProcessCommand,
        _: &traits::sandbox::SandboxPolicy,
    ) -> Result<traits::sandbox::SandboxedCommand, traits::sandbox::SandboxError> {
        Ok(traits::sandbox::SandboxedCommand::__new_sandboxed(
            cmd,
            traits::sandbox::SandboxedTag::BypassAuditedWithReason {
                reason: "test".into(),
            },
        ))
    }
    fn bypass_with_audit(
        &self,
        cmd: traits::sandbox::ProcessCommand,
        reason: &str,
    ) -> traits::sandbox::SandboxedCommand {
        traits::sandbox::SandboxedCommand::__new_sandboxed(
            cmd,
            traits::sandbox::SandboxedTag::BypassAuditedWithReason {
                reason: reason.into(),
            },
        )
    }
    async fn probe_capability(&self) -> traits::sandbox::SandboxCapability {
        traits::sandbox::SandboxCapability {
            available: true,
            reason: None,
            features: traits::sandbox::SandboxFeatures::default(),
        }
    }
}

struct NoopClock;
impl traits::Clock for NoopClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::UNIX_EPOCH
    }
}

/// Tracks `create_worktree` + `remove_worktree` calls hermetically.
#[derive(Default)]
struct LocalMockWorktree {
    created: StdMutex<Vec<(String, WorktreeHandle)>>,
    removed: StdMutex<Vec<WorktreeHandle>>,
}

#[async_trait]
impl WorktreeManager for LocalMockWorktree {
    async fn create_worktree(
        &self,
        slug: &str,
        _: Option<&str>,
        _: &[PathBuf],
    ) -> Result<WorktreeHandle, WorktreeError> {
        // Match the M2-01 contract: branch_name = "worktree-<flatten(slug)>"
        let flat = slug.replace('/', "+");
        let handle = WorktreeHandle {
            path: PathBuf::from("/tmp/repo-roundtrip/.claude/worktrees").join(&flat),
            branch_name: format!("worktree-{flat}"),
        };
        self.created
            .lock()
            .unwrap()
            .push((slug.to_string(), handle.clone()));
        Ok(handle)
    }
    async fn remove_worktree(&self, handle: &WorktreeHandle) -> Result<(), WorktreeError> {
        self.removed.lock().unwrap().push(handle.clone());
        Ok(())
    }
    async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError> {
        Ok(Vec::new())
    }
    async fn cleanup_stale(&self, _: Duration) -> Result<Vec<PathBuf>, WorktreeError> {
        Ok(Vec::new())
    }
    fn is_supported(&self) -> bool {
        true
    }
}

fn make_bctx(mock: Arc<LocalMockWorktree>) -> BuiltinToolContext {
    use permission::PermissionMode;
    use sandbox::decision::ProjectTrustLevel;
    use sandbox::runtime_config::{Platform, SandboxRuntimeConfig};
    BuiltinToolContext {
        fs: Arc::new(PanickingFs),
        bus: Arc::new(AnalyticsBus::new()),
        trusted_dirs: vec![std::env::temp_dir()],
        process: Arc::new(NoopProcess),
        sandbox: Arc::new(NoopSandbox),
        clock: Arc::new(NoopClock),
        sandbox_runtime: SandboxRuntimeConfig::default(),
        permission_mode: PermissionMode::Default,
        project_trust: ProjectTrustLevel::Trusted,
        sandbox_available: false,
        workspace: std::env::temp_dir(),
        platform: if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Linux
        },
        http: Arc::new(NoopHttp),
        provider: Arc::new(api_client::AnthropicProvider::new("test-key", None)),
        default_model: "claude-sonnet-4-20250514".into(),
        worktree: mock as Arc<dyn WorktreeManager>,
        subagent_spawner: None,
        task_registry: None,
        mailbox_router: None,
        budget_enforcer: None,
        mcp_registry: None,
        lsp_registry: None,
    }
}

fn make_use_ctx_with_session() -> (ToolUseContext, Arc<Mutex<SessionState>>) {
    let session = Arc::new(Mutex::new(SessionState::empty(
        SessionId::nil(),
        "claude-opus-4-7".into(),
    )));
    let ctx = ToolUseContext {
        options: ToolUseOptions {
            debug: false,
            verbose: false,
            main_loop_model: "test".into(),
            max_budget_nano_usd: None,
            mcp_clients: vec![],
            is_non_interactive_session: false,
            custom_system_prompt: None,
            append_system_prompt: None,
        },
        messages: vec![],
        tool_use_id: None,
        agent_id: None,
        content_replacement_state: None,
        session: Some(session.clone()),
        subagent_registry: None,
    };
    (ctx, session)
}

fn progress() -> tools::progress::ToolProgressSender {
    let (tx, _rx) = progress_channel();
    tx
}

// ----- Tests ----------------------------------------------------------------

#[tokio::test]
async fn todo_write_state_machine_roundtrip() {
    let bctx = make_bctx(Arc::new(LocalMockWorktree::default()));
    let (use_ctx, session) = make_use_ctx_with_session();
    let tool = TodoWriteTool::new(bctx);

    tool.call(
        json!({
            "todos": [
                { "id": "a", "content": "first",  "status": "pending"     },
                { "id": "b", "content": "second", "status": "in_progress" },
                { "id": "c", "content": "third",  "status": "completed"   }
            ]
        }),
        use_ctx.clone(),
        progress(),
    )
    .await
    .expect("first write");
    {
        let s = session.lock().await;
        assert_eq!(s.todos.len(), 3);
        assert_eq!(s.todos[0].status, TodoState::Pending);
        assert_eq!(s.todos[1].status, TodoState::InProgress);
        assert_eq!(s.todos[2].status, TodoState::Completed);
    }
    tool.call(
        json!({
            "todos": [
                { "id": "a", "content": "first",  "status": "completed" },
                { "id": "b", "content": "second", "status": "completed" },
                { "id": "c", "content": "third",  "status": "completed" }
            ]
        }),
        use_ctx,
        progress(),
    )
    .await
    .expect("second write");
    let s = session.lock().await;
    for t in &s.todos {
        assert_eq!(t.status, TodoState::Completed);
    }
}

#[tokio::test]
async fn todo_write_rejects_multiple_in_progress() {
    let bctx = make_bctx(Arc::new(LocalMockWorktree::default()));
    let (use_ctx, _session) = make_use_ctx_with_session();
    let tool = TodoWriteTool::new(bctx);
    let err = tool
        .call(
            json!({
                "todos": [
                    { "id": "a", "content": "x", "status": "in_progress" },
                    { "id": "b", "content": "y", "status": "in_progress" }
                ]
            }),
            use_ctx,
            progress(),
        )
        .await
        .expect_err("two in_progress must reject");
    assert_eq!(
        format!("{err}"),
        "invalid input: TodoWrite: at most one todo may be 'in_progress' (got 2)"
    );
}

#[tokio::test]
async fn todo_write_rejects_unknown_status() {
    let bctx = make_bctx(Arc::new(LocalMockWorktree::default()));
    let (use_ctx, _session) = make_use_ctx_with_session();
    let tool = TodoWriteTool::new(bctx);
    let err = tool
        .call(
            json!({
                "todos": [
                    { "id": "a", "content": "x", "status": "done" }
                ]
            }),
            use_ctx,
            progress(),
        )
        .await
        .expect_err("unknown variant must reject");
    let msg = format!("{err}");
    assert!(msg.contains("unknown variant"), "msg: {msg}");
}

#[tokio::test]
async fn plan_mode_toggle_roundtrip() {
    let bctx = make_bctx(Arc::new(LocalMockWorktree::default()));
    let (use_ctx, session) = make_use_ctx_with_session();
    let enter = EnterPlanModeTool::new(bctx.clone());
    let exit = ExitPlanModeTool::new(bctx);

    let res = enter
        .call(json!({}), use_ctx.clone(), progress())
        .await
        .expect("enter");
    assert_eq!(res.data["marker"], PLAN_MODE_ENTER_MARKER);
    assert!(session.lock().await.plan_mode);

    let res = exit
        .call(json!({}), use_ctx, progress())
        .await
        .expect("exit");
    assert_eq!(res.data["marker"], PLAN_MODE_EXIT_MARKER);
    assert!(!session.lock().await.plan_mode);
}

#[tokio::test]
async fn plan_mode_double_enter_fails() {
    let bctx = make_bctx(Arc::new(LocalMockWorktree::default()));
    let (use_ctx, _session) = make_use_ctx_with_session();
    let enter = EnterPlanModeTool::new(bctx);
    enter
        .call(json!({}), use_ctx.clone(), progress())
        .await
        .expect("first");
    let err = enter
        .call(json!({}), use_ctx, progress())
        .await
        .expect_err("second must reject");
    assert_eq!(
        format!("{err}"),
        "invalid input: EnterPlanMode: session is already in plan mode"
    );
}

#[tokio::test]
async fn plan_mode_exit_without_enter_fails() {
    let bctx = make_bctx(Arc::new(LocalMockWorktree::default()));
    let (use_ctx, _session) = make_use_ctx_with_session();
    let exit = ExitPlanModeTool::new(bctx);
    let err = exit
        .call(json!({}), use_ctx, progress())
        .await
        .expect_err("exit without enter must reject");
    assert_eq!(
        format!("{err}"),
        "invalid input: ExitPlanMode: session is not in plan mode"
    );
}

#[tokio::test]
async fn worktree_enter_exit_roundtrip() {
    let mock = Arc::new(LocalMockWorktree::default());
    let bctx = make_bctx(mock.clone());
    let (use_ctx, _session) = make_use_ctx_with_session();
    let enter = EnterWorktreeTool::new(bctx.clone());
    let exit = ExitWorktreeTool::new(bctx);
    let r = enter
        .call(
            json!({ "slug": "user/feature" }),
            use_ctx.clone(),
            progress(),
        )
        .await
        .expect("enter");
    assert_eq!(r.data["branch_name"], "worktree-user+feature");
    let path = r.data["path"].as_str().unwrap().to_string();
    let branch = r.data["branch_name"].as_str().unwrap().to_string();
    let r = exit
        .call(
            json!({ "path": path, "branch_name": branch }),
            use_ctx,
            progress(),
        )
        .await
        .expect("exit");
    assert_eq!(r.data["removed"], true);
    assert_eq!(mock.created.lock().unwrap().len(), 1);
    assert_eq!(mock.removed.lock().unwrap().len(), 1);
    assert_eq!(
        mock.removed.lock().unwrap()[0].branch_name,
        "worktree-user+feature"
    );
}

#[tokio::test]
async fn worktree_invalid_slug_rejected() {
    let bctx = make_bctx(Arc::new(LocalMockWorktree::default()));
    let (use_ctx, _session) = make_use_ctx_with_session();
    let enter = EnterWorktreeTool::new(bctx);
    let err = enter
        .call(json!({ "slug": "bad slug" }), use_ctx, progress())
        .await
        .expect_err("space rejects");
    let msg = format!("{err}");
    assert!(msg.contains("EnterWorktree: invalid slug:"), "msg: {msg}");
}
