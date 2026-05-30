//! M4-06 integration tests — exercise `TeamCreate` / `TeamDelete` against a
//! real filesystem (via `tempfile::TempDir`) with HOME redirected via env
//! to keep all touch operations inside the temp dir.

#![allow(clippy::too_many_lines)]

use async_trait::async_trait;
use serde_json::json;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use telemetry::AnalyticsBus;
use tempfile::TempDir;
use tools::builtin::{BuiltinToolContext, TeamCreateTool, TeamDeleteTool};
use tools::context::{ToolUseContext, ToolUseOptions};
use tools::progress::progress_channel;
use tools::tool_trait::{Tool, ToolError};
use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};

// `std::env::set_var` mutates process-wide state. The four tests in this file
// each redirect HOME, so we serialize them through a global Mutex to prevent
// parallel test threads from racing on $HOME.
//
// We use sync `#[test]` + a per-test `tokio::runtime::Runtime::block_on` so
// the `MutexGuard` is held only on the outer sync thread (never across an
// `.await`), keeping clippy's `await_holding_lock` lint happy.
static HOME_LOCK: Mutex<()> = Mutex::new(());

fn run<F: std::future::Future<Output = ()>>(f: F) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f);
}

struct PanickingFs;
#[async_trait]
impl FileSystem for PanickingFs {
    async fn read_file(
        &self,
        _: &str,
        _: Option<u64>,
        _: Option<u64>,
    ) -> Result<FileContent, FsError> {
        panic!("team integration tests do not call FileSystem::read_file")
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

// Minimal stubs for the other `BuiltinToolContext` fields.
struct NoopProcess;
#[async_trait]
impl traits::process::ProcessRunner for NoopProcess {
    async fn run(
        &self,
        _: &traits::sandbox::SandboxedCommand,
    ) -> Result<traits::process::ProcessOutput, traits::process::ProcessError> {
        panic!("team integration tests do not invoke process runner")
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

struct NoopHttp;
#[async_trait]
impl traits::http::HttpTransport for NoopHttp {
    async fn request(
        &self,
        _: protocol::HttpRequest,
    ) -> Result<protocol::HttpResponse, traits::http::HttpError> {
        Err(traits::http::HttpError::InvalidRequest(
            "NoopHttp: not configured for team integration".into(),
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

struct NoopWorktree;
#[async_trait]
impl traits::worktree::WorktreeManager for NoopWorktree {
    async fn create_worktree(
        &self,
        _: &str,
        _: Option<&str>,
        _: &[std::path::PathBuf],
    ) -> Result<traits::worktree::WorktreeHandle, traits::worktree::WorktreeError> {
        Err(traits::worktree::WorktreeError::Unsupported)
    }
    async fn remove_worktree(
        &self,
        _: &traits::worktree::WorktreeHandle,
    ) -> Result<(), traits::worktree::WorktreeError> {
        Err(traits::worktree::WorktreeError::Unsupported)
    }
    async fn list_worktrees(
        &self,
    ) -> Result<Vec<traits::worktree::WorktreeInfo>, traits::worktree::WorktreeError> {
        Ok(Vec::new())
    }
    async fn cleanup_stale(
        &self,
        _: std::time::Duration,
    ) -> Result<Vec<std::path::PathBuf>, traits::worktree::WorktreeError> {
        Ok(Vec::new())
    }
    fn is_supported(&self) -> bool {
        false
    }
}

fn make_ctx(home: &std::path::Path) -> BuiltinToolContext {
    use permission::PermissionMode;
    use sandbox::decision::ProjectTrustLevel;
    use sandbox::runtime_config::{Platform, SandboxRuntimeConfig};

    BuiltinToolContext {
        fs: Arc::new(PanickingFs),
        bus: Arc::new(AnalyticsBus::new()),
        trusted_dirs: vec![home.to_path_buf()],
        process: Arc::new(NoopProcess),
        sandbox: Arc::new(NoopSandbox),
        clock: Arc::new(NoopClock),
        sandbox_runtime: SandboxRuntimeConfig::default(),
        permission_mode: PermissionMode::Default,
        project_trust: ProjectTrustLevel::Trusted,
        sandbox_available: false,
        workspace: home.to_path_buf(),
        platform: if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Linux
        },
        http: Arc::new(NoopHttp),
        provider: Arc::new(api_client::AnthropicProvider::new("test-key", None)),
        default_model: "claude-sonnet-4-20250514".to_string(),
        worktree: Arc::new(NoopWorktree),
        subagent_spawner: None,
        task_registry: None,
        mailbox_router: None,
        budget_enforcer: None,
        mcp_registry: None,
        lsp_registry: None,
        camera: None,
        voice: None,
        share: None,
        computer_control: None,
    }
}

fn fresh_ctx() -> ToolUseContext {
    ToolUseContext {
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
        session: None,
        subagent_registry: None,
    }
}

#[test]
fn create_then_delete_roundtrip() {
    let _g = HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = TempDir::new().unwrap();
    std::env::set_var("HOME", tmp.path());
    let ctx_b = make_ctx(tmp.path());
    let create = TeamCreateTool::new(ctx_b.clone());
    let delete = TeamDeleteTool::new(ctx_b);
    run(async move {
        let (tx1, _r1) = progress_channel();
        let res = create
            .call(json!({"team_name": "alpha"}), fresh_ctx(), tx1)
            .await
            .unwrap();
        let dir = res.data["team_dir"].as_str().unwrap().to_string();
        assert!(dir.ends_with(".claude/team-mem/alpha"), "got {dir}");
        assert_eq!(res.data["created"], true);
        assert!(tokio::fs::try_exists(&dir).await.unwrap());

        let cfg = res.data["config_path"].as_str().unwrap().to_string();
        let bytes = tokio::fs::read(&cfg).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["team_name"], "alpha");
        assert_eq!(parsed["schema_version"], 1);

        let (tx2, _r2) = progress_channel();
        let res2 = delete
            .call(
                json!({"team_name": "alpha", "force": true}),
                fresh_ctx(),
                tx2,
            )
            .await
            .unwrap();
        assert_eq!(res2.data["deleted"], true);
        assert!(res2.data["file_count_at_delete"].as_u64().unwrap() >= 1);
        assert!(!tokio::fs::try_exists(&dir).await.unwrap());
    });
}

#[test]
fn create_rejects_duplicate() {
    let _g = HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = TempDir::new().unwrap();
    std::env::set_var("HOME", tmp.path());
    let ctx_b = make_ctx(tmp.path());
    let create = TeamCreateTool::new(ctx_b);
    run(async move {
        let (tx1, _r1) = progress_channel();
        create
            .call(json!({"team_name": "beta"}), fresh_ctx(), tx1)
            .await
            .unwrap();

        let (tx2, _r2) = progress_channel();
        let err = create
            .call(json!({"team_name": "beta"}), fresh_ctx(), tx2)
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(s) => {
                assert!(
                    s.starts_with("TeamCreate: team 'beta' already exists at"),
                    "got {s}"
                );
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    });
}

#[test]
fn delete_refuses_nonempty_without_force() {
    let _g = HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = TempDir::new().unwrap();
    std::env::set_var("HOME", tmp.path());
    let dir = tmp.path().join(".claude/team-mem/gamma");
    let ctx_b = make_ctx(tmp.path());
    let create = TeamCreateTool::new(ctx_b.clone());
    let delete = TeamDeleteTool::new(ctx_b);
    run(async move {
        let (tx1, _r1) = progress_channel();
        create
            .call(json!({"team_name": "gamma"}), fresh_ctx(), tx1)
            .await
            .unwrap();

        let (tx2, _r2) = progress_channel();
        let err = delete
            .call(json!({"team_name": "gamma"}), fresh_ctx(), tx2)
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(s) => {
                assert!(
                    s.starts_with("TeamDelete: team 'gamma' directory is non-empty"),
                    "got {s}"
                );
                assert!(s.contains("pass force=true to delete anyway"), "got {s}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        assert!(tokio::fs::try_exists(&dir).await.unwrap());
    });
}

#[test]
fn delete_with_force_clears_non_empty_dir() {
    let _g = HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tmp = TempDir::new().unwrap();
    std::env::set_var("HOME", tmp.path());
    let dir = tmp.path().join(".claude/team-mem/delta");
    let ctx_b = make_ctx(tmp.path());
    let create = TeamCreateTool::new(ctx_b.clone());
    let delete = TeamDeleteTool::new(ctx_b);
    run(async move {
        let (tx1, _r1) = progress_channel();
        create
            .call(json!({"team_name": "delta"}), fresh_ctx(), tx1)
            .await
            .unwrap();
        tokio::fs::write(dir.join("notes.md"), b"hi").await.unwrap();

        let (tx2, _r2) = progress_channel();
        let res = delete
            .call(
                json!({"team_name": "delta", "force": true}),
                fresh_ctx(),
                tx2,
            )
            .await
            .unwrap();
        assert_eq!(res.data["deleted"], true);
        assert!(res.data["file_count_at_delete"].as_u64().unwrap() >= 2);
        assert!(!tokio::fs::try_exists(&dir).await.unwrap());
    });
}
