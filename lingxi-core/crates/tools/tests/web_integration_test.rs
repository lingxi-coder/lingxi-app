//! M4-03 integration tests. Spec §6 budget: ~6 integ tests for the 2 web tools.
//!
//! - `WebFetch` tests run against a real `axum` server bound to `127.0.0.1:0`
//!   (ephemeral port), exercised through the posix-side `PosixHttp` transport.
//!   Hermetic — no external network.
//! - `WebSearch` tests use `MockHttpTransport` because the response body is a
//!   `MessageResponse` JSON; an axum mock would add no value.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::similar_names)]

use async_trait::async_trait;
use axum::{routing::get, Router};
use lingxi_api_client::AnthropicProvider;
use lingxi_permission::PermissionMode;
use lingxi_sandbox::decision::ProjectTrustLevel;
use lingxi_sandbox::runtime_config::{Platform, SandboxRuntimeConfig};
use lingxi_telemetry::sinks::InMemorySink;
use lingxi_telemetry::AnalyticsBus;
use lingxi_test_harness::mocks::{MockHttpTransport, ScriptedResponse};
use lingxi_tools::builtin::BuiltinToolContext;
use lingxi_tools::context::{ToolUseContext, ToolUseOptions};
use lingxi_tools::progress::progress_channel;
use lingxi_tools::tool_trait::{Tool, ToolError};
use lingxi_tools::{WebFetchTool, WebSearchTool};
use lingxi_traits::filesystem::FileSystem;
use lingxi_traits::http::HttpTransport;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

// ---- minimal local stubs (same shape as foundation_integration_test) -----

struct PanickingFs;
#[async_trait]
impl FileSystem for PanickingFs {
    async fn read_file(
        &self,
        _: &str,
        _: Option<u64>,
        _: Option<u64>,
    ) -> Result<lingxi_traits::filesystem::FileContent, lingxi_traits::filesystem::FsError> {
        panic!("web integration test does not use FileSystem")
    }
    async fn write_file(&self, _: &str, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not used")
    }
    fn is_within_workspace(&self, _: &str) -> bool {
        true
    }
    async fn watch(
        &self,
        _: &str,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = lingxi_traits::filesystem::FileEvent> + Send>>,
        lingxi_traits::filesystem::FsError,
    > {
        panic!("not used")
    }
    async fn append_file(
        &self,
        _: &str,
        _: &str,
    ) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not used")
    }
    async fn truncate(&self, _: &str, _: u64) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not used")
    }
    async fn file_mtime(
        &self,
        _: &str,
    ) -> Result<std::time::SystemTime, lingxi_traits::filesystem::FsError> {
        panic!("not used")
    }
    async fn file_size(&self, _: &str) -> Result<u64, lingxi_traits::filesystem::FsError> {
        panic!("not used")
    }
    async fn delete_file(&self, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not used")
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not used")
    }
    async fn flock_exclusive(
        &self,
        _: &str,
    ) -> Result<Box<dyn lingxi_traits::filesystem::FlockGuard>, lingxi_traits::filesystem::FsError>
    {
        panic!("not used")
    }
    async fn fsync(&self, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not used")
    }
}

struct NoopProcess;
#[async_trait]
impl lingxi_traits::process::ProcessRunner for NoopProcess {
    async fn run(
        &self,
        _: &lingxi_traits::sandbox::SandboxedCommand,
    ) -> Result<lingxi_traits::process::ProcessOutput, lingxi_traits::process::ProcessError> {
        panic!("web integration test does not use ProcessRunner")
    }
    async fn spawn_background(
        &self,
        _: &lingxi_traits::sandbox::SandboxedCommand,
    ) -> Result<lingxi_traits::process::ProcessHandle, lingxi_traits::process::ProcessError> {
        panic!("not used")
    }
    async fn kill(
        &self,
        _: &lingxi_traits::process::ProcessHandle,
    ) -> Result<(), lingxi_traits::process::ProcessError> {
        Ok(())
    }
    fn is_available(&self) -> bool {
        false
    }
}

struct BypassSandbox;
#[async_trait]
impl lingxi_traits::sandbox::Sandbox for BypassSandbox {
    fn is_available(&self) -> bool {
        true
    }
    fn backend(&self) -> lingxi_traits::sandbox::SandboxBackend {
        lingxi_traits::sandbox::SandboxBackend::None
    }
    fn prepare(
        &self,
        cmd: lingxi_traits::sandbox::ProcessCommand,
        _policy: &lingxi_traits::sandbox::SandboxPolicy,
    ) -> Result<lingxi_traits::sandbox::SandboxedCommand, lingxi_traits::sandbox::SandboxError>
    {
        Ok(lingxi_traits::sandbox::SandboxedCommand::__new_sandboxed(
            cmd,
            lingxi_traits::sandbox::SandboxedTag::BypassAuditedWithReason {
                reason: "integ".into(),
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

struct RealClock;
impl lingxi_traits::Clock for RealClock {
    fn now(&self) -> std::time::SystemTime {
        std::time::SystemTime::now()
    }
}

fn make_web_ctx(http: Arc<dyn HttpTransport>) -> (BuiltinToolContext, Arc<InMemorySink>) {
    let bus = Arc::new(AnalyticsBus::new());
    let sink = Arc::new(InMemorySink::default());
    let provider = Arc::new(AnthropicProvider::new("test-key", None));
    let ctx = BuiltinToolContext {
        fs: Arc::new(PanickingFs),
        bus,
        trusted_dirs: vec![std::env::temp_dir()],
        process: Arc::new(NoopProcess),
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
        http,
        provider,
        default_model: "claude-sonnet-4-20250514".into(),
        worktree: Arc::new(NoopWorktree),
        subagent_spawner: None,
        task_registry: None,
        mailbox_router: None,
        budget_enforcer: None,
    };
    (ctx, sink)
}

// M4-04: web tests never invoke worktree tools; the `worktree` field on
// `BuiltinToolContext` is satisfied by an unsupported stub.
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

fn fresh_use_ctx() -> ToolUseContext {
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

// ---- axum mock server (hermetic, 127.0.0.1:0) -----------------------------

async fn start_mock_http(
    body: &'static str,
    status: u16,
) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
    let app = Router::new().route(
        "/",
        get(move || async move { (axum::http::StatusCode::from_u16(status).unwrap(), body) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
    });
    (addr, tx)
}

fn real_http() -> Arc<dyn HttpTransport> {
    Arc::new(lingxi_platform_posix::http::PosixHttp::new())
}

// ---- WebFetch integration tests (axum-backed) -----------------------------

#[tokio::test]
async fn webfetch_happy_path_against_axum() {
    let (addr, _shutdown) = start_mock_http("hello from axum", 200).await;
    let (ctx, sink) = make_web_ctx(real_http());
    ctx.bus.attach_sink(sink.clone()).await;
    let tool = WebFetchTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let res = tool
        .call(
            json!({ "url": format!("http://{addr}/") }),
            fresh_use_ctx(),
            tx,
        )
        .await
        .expect("ok");
    assert_eq!(res.data["status"], 200);
    assert_eq!(res.data["content"], "hello from axum");
    let events = sink.events().await;
    let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"tengu_tool_web_fetch_completed"));
}

#[tokio::test]
async fn webfetch_500_returns_transport_err() {
    let (addr, _shutdown) = start_mock_http("server boom", 500).await;
    let (ctx, sink) = make_web_ctx(real_http());
    ctx.bus.attach_sink(sink.clone()).await;
    let tool = WebFetchTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let err = tool
        .call(
            json!({ "url": format!("http://{addr}/") }),
            fresh_use_ctx(),
            tx,
        )
        .await
        .expect_err("500 must be Err");
    let msg = format!("{err}");
    assert!(
        msg.contains("WebFetch: HTTP 500 from http://"),
        "got: {msg}"
    );
    let events = sink.events().await;
    let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"tengu_tool_web_fetch_failed"));
}

#[tokio::test]
async fn webfetch_rejects_file_scheme() {
    let (ctx, _sink) = make_web_ctx(real_http());
    let tool = WebFetchTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let err = tool
        .call(json!({ "url": "file:///etc/passwd" }), fresh_use_ctx(), tx)
        .await
        .expect_err("file:// must be rejected");
    assert!(matches!(err, ToolError::InvalidInput(_)));
    assert!(
        format!("{err}").contains("URL scheme 'file' not allowed; only https/http"),
        "got: {err}"
    );
}

// ---- WebSearch integration tests (MockHttpTransport-backed) ---------------

#[tokio::test]
async fn websearch_happy_path_with_mock_messages_response() {
    let http = Arc::new(MockHttpTransport::new());
    let resp_body = json!({
        "id": "msg_x",
        "model": "claude-sonnet-4-20250514",
        "content": [
            { "type": "text", "text": "Top results:" },
            {
                "type": "server_tool_use",
                "id": "stu_a",
                "name": "web_search",
                "input": { "url": "https://docs.rs", "title": "docs.rs" }
            }
        ],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 5, "output_tokens": 3 }
    });
    http.enqueue(ScriptedResponse::Sync(lingxi_protocol::HttpResponse {
        status: 200,
        headers: vec![],
        body: resp_body.to_string(),
    }));
    let (ctx, sink) = make_web_ctx(http.clone() as Arc<dyn HttpTransport>);
    ctx.bus.attach_sink(sink.clone()).await;
    let tool = WebSearchTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let res = tool
        .call(json!({ "query": "rust async traits" }), fresh_use_ctx(), tx)
        .await
        .expect("ok");
    let arr = res.data["results"].as_array().expect("array");
    assert_eq!(arr.len(), 2);
    let reqs = http.received_requests();
    let last_req = reqs.last().expect("captured");
    let (_, beta) = last_req
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("anthropic-beta"))
        .expect("beta header");
    assert!(beta.contains("web-search-2025-03-05"));
    let events = sink.events().await;
    let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"tengu_tool_web_search_completed"));
}

#[tokio::test]
async fn websearch_503_returns_transport_err_without_retry() {
    let http = Arc::new(MockHttpTransport::new());
    http.enqueue(ScriptedResponse::Sync(lingxi_protocol::HttpResponse {
        status: 503,
        headers: vec![],
        body: "unavailable".into(),
    }));
    let (ctx, sink) = make_web_ctx(http.clone() as Arc<dyn HttpTransport>);
    ctx.bus.attach_sink(sink.clone()).await;
    let tool = WebSearchTool::new(ctx);
    let (tx, _rx) = progress_channel();
    let err = tool
        .call(json!({ "query": "rust" }), fresh_use_ctx(), tx)
        .await
        .expect_err("503 must be Err");
    assert!(
        format!("{err}").contains("HTTP 503"),
        "expected HTTP 503 in error, got: {err}"
    );
    assert_eq!(http.received_requests().len(), 1, "must NOT self-retry");
    let events = sink.events().await;
    let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"tengu_tool_web_search_failed"));
}

#[tokio::test]
async fn websearch_validation_rejects_one_char_query() {
    let http = Arc::new(MockHttpTransport::new());
    let (ctx, _sink) = make_web_ctx(http as Arc<dyn HttpTransport>);
    let tool = WebSearchTool::new(ctx);
    let v = tool
        .validate_input(&json!({ "query": "x" }), &fresh_use_ctx())
        .await;
    assert!(v.is_err(), "1-char query must fail validation");
}
