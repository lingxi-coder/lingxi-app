//! Cross-tool integration: exercise multiple tools in sequence against the
//! same tempdir to confirm they cooperate. NO mocks — uses real
//! `tokio::fs` inside a sandboxed `trusted_dirs = [tempdir]`.

#![allow(clippy::format_collect, clippy::too_many_lines)]

use async_trait::async_trait;
use lingxi_telemetry::{AnalyticsBus, InMemorySink};
use lingxi_tools::builtin::{
    BuiltinToolContext, FileEditTool, FileReadTool, FileWriteTool, GlobTool, GrepTool,
    NotebookEditTool,
};
use lingxi_tools::context::{ToolUseContext, ToolUseOptions};
use lingxi_tools::progress::progress_channel;
use lingxi_tools::tool_trait::{Tool, ToolError};
use lingxi_traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
use serde_json::json;
use std::pin::Pin;
use std::sync::Arc;
use tempfile::TempDir;

struct PanickingFs;
#[async_trait]
impl FileSystem for PanickingFs {
    async fn read_file(
        &self,
        _: &str,
        _: Option<u64>,
        _: Option<u64>,
    ) -> Result<FileContent, FsError> {
        panic!("integration test does not call FileSystem::read_file")
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

fn make_ctx(tmp: &TempDir) -> (BuiltinToolContext, Arc<InMemorySink>) {
    use lingxi_permission::PermissionMode;
    use lingxi_sandbox::decision::ProjectTrustLevel;
    use lingxi_sandbox::runtime_config::{Platform, SandboxRuntimeConfig};

    let bus = Arc::new(AnalyticsBus::new());
    let sink = Arc::new(InMemorySink::default());
    let fs: Arc<dyn FileSystem> = Arc::new(PanickingFs);
    (
        BuiltinToolContext {
            fs,
            bus,
            trusted_dirs: vec![tmp.path().to_path_buf()],
            process: Arc::new(NoopProcess),
            sandbox: Arc::new(NoopSandbox),
            clock: Arc::new(NoopClock),
            sandbox_runtime: SandboxRuntimeConfig::default(),
            permission_mode: PermissionMode::Default,
            project_trust: ProjectTrustLevel::Trusted,
            sandbox_available: false,
            workspace: tmp.path().to_path_buf(),
            platform: if cfg!(target_os = "macos") {
                Platform::Mac
            } else {
                Platform::Linux
            },
            http: Arc::new(NoopHttp),
            provider: Arc::new(lingxi_api_client::AnthropicProvider::new("test-key", None)),
            default_model: "claude-sonnet-4-20250514".to_string(),
            worktree: Arc::new(NoopWorktree),
        },
        sink,
    )
}

// HTTP stub for the M4-03 field — foundation tests never call web tools.
struct NoopHttp;
#[async_trait::async_trait]
impl lingxi_traits::http::HttpTransport for NoopHttp {
    async fn request(
        &self,
        _: lingxi_protocol::HttpRequest,
    ) -> Result<lingxi_protocol::HttpResponse, lingxi_traits::http::HttpError> {
        Err(lingxi_traits::http::HttpError::InvalidRequest(
            "NoopHttp: not configured for foundation integration".into(),
        ))
    }
    async fn stream_sse(
        &self,
        _: lingxi_protocol::HttpRequest,
    ) -> Result<lingxi_traits::http::SseStream, lingxi_traits::http::HttpError> {
        Err(lingxi_traits::http::HttpError::InvalidRequest(
            "NoopHttp: stream_sse not supported".into(),
        ))
    }
}

// No-op stubs for the M4-02 fields — these foundation tests never exercise
// the process/sandbox/clock seams (only file-op tools).
struct NoopProcess;
#[async_trait::async_trait]
impl lingxi_traits::process::ProcessRunner for NoopProcess {
    async fn run(
        &self,
        _: &lingxi_traits::sandbox::SandboxedCommand,
    ) -> Result<lingxi_traits::process::ProcessOutput, lingxi_traits::process::ProcessError> {
        panic!("foundation tests do not invoke process runner")
    }
    async fn spawn_background(
        &self,
        _: &lingxi_traits::sandbox::SandboxedCommand,
    ) -> Result<lingxi_traits::process::ProcessHandle, lingxi_traits::process::ProcessError> {
        panic!("not called")
    }
    async fn kill(
        &self,
        _: &lingxi_traits::process::ProcessHandle,
    ) -> Result<(), lingxi_traits::process::ProcessError> {
        Ok(())
    }
    fn is_available(&self) -> bool {
        true
    }
}

struct NoopSandbox;
#[async_trait::async_trait]
impl lingxi_traits::sandbox::Sandbox for NoopSandbox {
    fn is_available(&self) -> bool {
        true
    }
    fn backend(&self) -> lingxi_traits::sandbox::SandboxBackend {
        lingxi_traits::sandbox::SandboxBackend::None
    }
    fn prepare(
        &self,
        cmd: lingxi_traits::sandbox::ProcessCommand,
        _: &lingxi_traits::sandbox::SandboxPolicy,
    ) -> Result<lingxi_traits::sandbox::SandboxedCommand, lingxi_traits::sandbox::SandboxError>
    {
        Ok(lingxi_traits::sandbox::SandboxedCommand::__new_sandboxed(
            cmd,
            lingxi_traits::sandbox::SandboxedTag::BypassAuditedWithReason {
                reason: "test".into(),
            },
        ))
    }
    fn bypass_with_audit(
        &self,
        cmd: lingxi_traits::sandbox::ProcessCommand,
        reason: &str,
    ) -> lingxi_traits::sandbox::SandboxedCommand {
        lingxi_traits::sandbox::SandboxedCommand::__new_sandboxed(
            cmd,
            lingxi_traits::sandbox::SandboxedTag::BypassAuditedWithReason {
                reason: reason.into(),
            },
        )
    }
    async fn probe_capability(&self) -> lingxi_traits::sandbox::SandboxCapability {
        lingxi_traits::sandbox::SandboxCapability {
            available: true,
            reason: None,
            features: lingxi_traits::sandbox::SandboxFeatures::default(),
        }
    }
}

struct NoopClock;
impl lingxi_traits::Clock for NoopClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::UNIX_EPOCH
    }
}

// M4-04: foundation tests never invoke worktree tools, but the new field on
// `BuiltinToolContext` must be populated. `Unsupported` keeps the type-system
// happy without pulling in `lingxi-test-harness`.
struct NoopWorktree;
#[async_trait::async_trait]
impl lingxi_traits::worktree::WorktreeManager for NoopWorktree {
    async fn create_worktree(
        &self,
        _: &str,
        _: Option<&str>,
        _: &[std::path::PathBuf],
    ) -> Result<lingxi_traits::worktree::WorktreeHandle, lingxi_traits::worktree::WorktreeError>
    {
        Err(lingxi_traits::worktree::WorktreeError::Unsupported)
    }
    async fn remove_worktree(
        &self,
        _: &lingxi_traits::worktree::WorktreeHandle,
    ) -> Result<(), lingxi_traits::worktree::WorktreeError> {
        Err(lingxi_traits::worktree::WorktreeError::Unsupported)
    }
    async fn list_worktrees(
        &self,
    ) -> Result<Vec<lingxi_traits::worktree::WorktreeInfo>, lingxi_traits::worktree::WorktreeError>
    {
        Ok(Vec::new())
    }
    async fn cleanup_stale(
        &self,
        _: std::time::Duration,
    ) -> Result<Vec<std::path::PathBuf>, lingxi_traits::worktree::WorktreeError> {
        Ok(Vec::new())
    }
    fn is_supported(&self) -> bool {
        false
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
    }
}

#[tokio::test]
async fn read_edit_read_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("file.txt");
    std::fs::write(&target, "hello world").unwrap();
    let (ctx, _sink) = make_ctx(&tmp);

    let reader = FileReadTool::new(ctx.clone());
    let editor = FileEditTool::new(ctx.clone());

    let (tx, _rx) = progress_channel();
    let before = reader
        .call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            tx,
        )
        .await
        .unwrap();
    assert_eq!(before.data["content"], "hello world");

    let (tx2, _rx2) = progress_channel();
    let _ = editor
        .call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "world",
                "new_string": "Rust"
            }),
            fresh_ctx(),
            tx2,
        )
        .await
        .unwrap();

    let (tx3, _rx3) = progress_channel();
    let after = reader
        .call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            tx3,
        )
        .await
        .unwrap();
    assert_eq!(after.data["content"], "hello Rust");
}

#[tokio::test]
async fn glob_then_grep_on_tempdir() {
    let tmp = TempDir::new().unwrap();
    for i in 0..5 {
        std::fs::write(
            tmp.path().join(format!("f{i}.rs")),
            format!("fn f{i}() {{}}\n"),
        )
        .unwrap();
    }
    for i in 0..5 {
        std::fs::write(tmp.path().join(format!("g{i}.txt")), "no_fn_here\n").unwrap();
    }
    let (ctx, _sink) = make_ctx(&tmp);

    let glob = GlobTool::new(ctx.clone());
    let grep = GrepTool::new(ctx.clone());

    let (tx, _rx) = progress_channel();
    let glob_res = glob
        .call(json!({ "pattern": "*.rs" }), fresh_ctx(), tx)
        .await
        .unwrap();
    assert_eq!(glob_res.data["matches"].as_array().unwrap().len(), 5);

    let (tx2, _rx2) = progress_channel();
    let grep_res = grep
        .call(json!({ "pattern": r"fn f\d" }), fresh_ctx(), tx2)
        .await
        .unwrap();
    assert_eq!(grep_res.data["total_matches"], 5);
}

#[tokio::test]
async fn notebook_edit_replace_then_read() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("nb.ipynb");
    let nb = serde_json::to_string_pretty(&json!({
        "cells": [
            { "cell_type": "code", "id": "c1", "source": "x = 1", "metadata": {}, "outputs": [], "execution_count": null }
        ],
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 5
    })).unwrap();
    std::fs::write(&target, nb).unwrap();
    let (ctx, _sink) = make_ctx(&tmp);

    let notebook = NotebookEditTool::new(ctx.clone());
    let reader = FileReadTool::new(ctx.clone());

    let (tx, _rx) = progress_channel();
    let _ = notebook
        .call(
            json!({
                "notebook_path": target.to_str().unwrap(),
                "cell_id": "c1",
                "edit_mode": "replace",
                "new_source": "x = 42"
            }),
            fresh_ctx(),
            tx,
        )
        .await
        .unwrap();

    let (tx2, _rx2) = progress_channel();
    let after = reader
        .call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            tx2,
        )
        .await
        .unwrap();
    let body = after.data["content"].as_str().unwrap();
    assert!(body.contains("x = 42"));
    assert!(!body.contains("x = 1"));
}

#[tokio::test]
async fn read_oversize_rejects_with_byte_locked_string() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("big.txt");
    std::fs::write(&target, vec![b'A'; 300_000]).unwrap();
    let (ctx, _sink) = make_ctx(&tmp);
    let reader = FileReadTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let err = reader
        .call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            tx,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("exceeds 256KB read limit"));
}

#[tokio::test]
async fn read_binary_rejects_with_byte_locked_string() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("bin");
    let mut data = vec![b'A'; 100];
    data[10] = 0;
    std::fs::write(&target, data).unwrap();
    let (ctx, _sink) = make_ctx(&tmp);
    let reader = FileReadTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let err = reader
        .call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            tx,
        )
        .await
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("appears to be binary (first 8KB contains NUL bytes)"));
}

#[tokio::test]
async fn read_outside_trusted_rejects() {
    let tmp = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let target = outside.path().join("a.txt");
    std::fs::write(&target, "x").unwrap();
    let (ctx, _sink) = make_ctx(&tmp);
    let reader = FileReadTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let err = reader
        .call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            tx,
        )
        .await
        .unwrap_err();
    match err {
        ToolError::PathBlocked { .. } => {}
        other => panic!("expected PathBlocked, got {other:?}"),
    }
}

#[tokio::test]
async fn glob_caps_at_100_in_integration() {
    let tmp = TempDir::new().unwrap();
    for i in 0..150 {
        std::fs::write(tmp.path().join(format!("f{i}.rs")), "x").unwrap();
    }
    let (ctx, _sink) = make_ctx(&tmp);
    let glob = GlobTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let result = glob
        .call(json!({ "pattern": "*.rs" }), fresh_ctx(), tx)
        .await
        .unwrap();
    assert_eq!(result.data["matches"].as_array().unwrap().len(), 100);
    assert_eq!(result.data["truncated"], true);
}

#[tokio::test]
async fn edit_patch_truncation_suffix_appears_in_integration() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("big.txt");
    let big: String = (0..100).map(|i| format!("L{i}\n")).collect();
    std::fs::write(&target, big).unwrap();
    let (ctx, _sink) = make_ctx(&tmp);
    let editor = FileEditTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let result = editor
        .call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "L",
                "new_string": "M",
                "replace_all": true
            }),
            fresh_ctx(),
            tx,
        )
        .await
        .unwrap();
    let preview = result.data["patch_preview"].as_str().unwrap();
    assert!(preview.contains("lines truncated] ..."));
}

#[tokio::test]
async fn write_blocks_missing_parent_without_mkdir_in_integration() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("sub").join("dir").join("out.txt");
    let (ctx, _sink) = make_ctx(&tmp);
    let writer = FileWriteTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let err = writer
        .call(
            json!({ "file_path": target.to_str().unwrap(), "content": "x" }),
            fresh_ctx(),
            tx,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("mkdir=true"));
}

#[tokio::test]
async fn write_creates_parent_with_mkdir_in_integration() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("sub").join("dir").join("out.txt");
    let (ctx, _sink) = make_ctx(&tmp);
    let writer = FileWriteTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let _ = writer
        .call(
            json!({
                "file_path": target.to_str().unwrap(),
                "content": "deep",
                "mkdir": true
            }),
            fresh_ctx(),
            tx,
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "deep");
}
