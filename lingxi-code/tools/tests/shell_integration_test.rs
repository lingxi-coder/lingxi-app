//! Real-process integration tests for `BashTool` (M4-02 Task 7).
//!
//! Spawns commands through the real `PosixProcess` runner so the
//! end-to-end spawn pipeline is verified: stdout capture, exit-code
//! routing, timeout error string, and background-spawn `task_output_path`.

#![cfg(unix)]

use async_trait::async_trait;
use permission::PermissionMode;
use platform_posix::process::PosixProcess;
use sandbox::decision::ProjectTrustLevel;
use sandbox::runtime_config::{Platform, SandboxRuntimeConfig};
use serde_json::json;
use std::sync::Arc;
use telemetry::AnalyticsBus;
use tools::builtin::bash::BashTool;
use tools::builtin::repl::REPLTool;
use tools::builtin::BuiltinToolContext;
use tools::context::{ToolUseContext, ToolUseOptions};
use tools::progress::progress_channel;
use tools::tool_trait::Tool;
use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
use traits::sandbox::{
    ProcessCommand as SbxCommand, Sandbox, SandboxBackend, SandboxCapability, SandboxError,
    SandboxFeatures, SandboxPolicy, SandboxedCommand, SandboxedTag,
};

struct PanickingFs;
#[async_trait]
impl FileSystem for PanickingFs {
    async fn read_file(
        &self,
        _: &str,
        _: Option<u64>,
        _: Option<u64>,
    ) -> Result<FileContent, FsError> {
        panic!("shell integration tests do not call FS")
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
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError> {
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

struct BypassSandbox;
#[async_trait]
impl Sandbox for BypassSandbox {
    fn is_available(&self) -> bool {
        true
    }
    fn backend(&self) -> SandboxBackend {
        SandboxBackend::None
    }
    fn prepare(
        &self,
        cmd: SbxCommand,
        _: &SandboxPolicy,
    ) -> Result<SandboxedCommand, SandboxError> {
        Ok(SandboxedCommand::__new_sandboxed(
            cmd,
            SandboxedTag::BypassAuditedWithReason {
                reason: "integ_test".into(),
            },
        ))
    }
    fn bypass_with_audit(&self, cmd: SbxCommand, reason: &str) -> SandboxedCommand {
        SandboxedCommand::__new_sandboxed(
            cmd,
            SandboxedTag::BypassAuditedWithReason {
                reason: reason.into(),
            },
        )
    }
    async fn probe_capability(&self) -> SandboxCapability {
        SandboxCapability {
            available: true,
            reason: None,
            features: SandboxFeatures::default(),
        }
    }
}

struct RealClock;
impl traits::Clock for RealClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::SystemTime::now()
    }
}

fn make_ctx() -> BuiltinToolContext {
    BuiltinToolContext {
        fs: Arc::new(PanickingFs),
        bus: Arc::new(AnalyticsBus::new()),
        trusted_dirs: vec![std::env::temp_dir()],
        process: Arc::new(PosixProcess::new()),
        sandbox: Arc::new(BypassSandbox),
        clock: Arc::new(RealClock),
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
        default_model: "claude-sonnet-4-20250514".to_string(),
        worktree: Arc::new(NoopWorktree),
        subagent_spawner: None,
        task_registry: None,
        mailbox_router: None,
        budget_enforcer: None,
        mcp_registry: None,
        lsp_registry: None,
    }
}

// M4-04: shell tests never invoke worktree tools; the `worktree` field on
// `BuiltinToolContext` is satisfied by an unsupported stub.
struct NoopWorktree;
#[async_trait::async_trait]
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

struct NoopHttp;
#[async_trait::async_trait]
impl traits::http::HttpTransport for NoopHttp {
    async fn request(
        &self,
        _: protocol::HttpRequest,
    ) -> Result<protocol::HttpResponse, traits::http::HttpError> {
        Err(traits::http::HttpError::InvalidRequest(
            "NoopHttp: not configured for shell integration".into(),
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

#[tokio::test]
async fn bash_echo_hello_returns_stdout() {
    let tool = BashTool::new(make_ctx());
    let (tx, _rx) = progress_channel();
    let res = tool
        .call(json!({"command": "echo hello"}), fresh_ctx(), tx)
        .await
        .expect("echo should succeed");
    assert_eq!(res.data["exit_code"], 0);
    assert_eq!(res.data["stdout"].as_str().unwrap().trim(), "hello");
    assert_eq!(res.data["is_error"], false);
}

#[tokio::test]
async fn bash_nonzero_exit_is_data_not_err() {
    let tool = BashTool::new(make_ctx());
    let (tx, _rx) = progress_channel();
    let res = tool
        .call(json!({"command": "exit 7"}), fresh_ctx(), tx)
        .await
        .expect("non-zero exit is data");
    assert_eq!(res.data["exit_code"], 7);
    assert_eq!(res.data["is_error"], true);
}

#[tokio::test]
async fn bash_timeout_emits_locked_error_string() {
    let tool = BashTool::new(make_ctx());
    let (tx, _rx) = progress_channel();
    let err = tool
        .call(
            json!({"command": "sleep 30", "timeout_ms": 300}),
            fresh_ctx(),
            tx,
        )
        .await
        .expect_err("timeout");
    let msg = err.to_string();
    assert!(
        msg.contains("Bash command timed out after 300ms"),
        "expected locked string, got: {msg}",
    );
}

#[tokio::test]
async fn bash_run_in_background_returns_pid_and_task_output_path() {
    let tool = BashTool::new(make_ctx());
    let (tx, _rx) = progress_channel();
    let res = tool
        .call(
            json!({"command": "sleep 3", "run_in_background": true}),
            fresh_ctx(),
            tx,
        )
        .await
        .expect("bg ok");
    assert!(res.data["pid"].as_u64().unwrap() > 0);
    let p = res.data["task_output_path"].as_str().unwrap();
    assert!(p.contains("lingxi-task-output"), "got task_output_path={p}");
}

fn which_in_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for d in std::env::split_paths(&path) {
        let c = d.join(name);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

#[tokio::test]
async fn repl_python_executes_print() {
    if which_in_path("python3").is_none() {
        eprintln!("python3 not on PATH; skipping");
        return;
    }
    let tool = REPLTool::new(make_ctx());
    let (tx, _rx) = progress_channel();
    let r = tool
        .call(
            json!({"language": "python", "code": "print('hi from repl')"}),
            fresh_ctx(),
            tx,
        )
        .await
        .expect("ok");
    assert!(r.data["stdout"].as_str().unwrap().contains("hi from repl"));
    assert_eq!(r.data["exit_code"], 0);
}
