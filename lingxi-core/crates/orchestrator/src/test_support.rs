//! Test fixtures.
//!
//! Gated behind `#[cfg(any(test, feature = "test-support"))]` so the
//! cli + tui crates can re-use the fixtures in M5-12 / M6 without
//! pulling them into release builds.

use crate::conversation::OrchestratorApiClient;
use async_trait::async_trait;
use lingxi_api_client::{
    types::{MessageResponse, UsageApi},
    ApiError,
};
use lingxi_protocol::ConversationMessage;
use lingxi_traits::{CostSnapshot, OutputEvent, OutputStream};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

// ============================================================================
// MockApiClient (Task 6)
// ============================================================================

/// Scripted mock API client. Returns the responses queued at construction
/// time, in order. Captures each `msgs` argument for later assertion.
///
/// If the queue is exhausted, `messages_create` returns
/// `ApiError::Server { status: 500, body: "mock script exhausted" }` so the
/// orchestrator's max-turns guard is exercised honestly (the M3-03 `ApiError`
/// enum has no generic `ProviderError` variant; `Server` is the closest match
/// for a synthetic upstream-side failure with a string payload).
pub struct MockApiClient {
    queue: Arc<Mutex<VecDeque<MessageResponse>>>,
    captured_msgs: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
    captured_systems: Arc<Mutex<Vec<Option<String>>>>,
}

impl MockApiClient {
    /// Construct a mock with a script of `responses` returned in order.
    #[must_use]
    pub fn new(responses: Vec<MessageResponse>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::from(responses))),
            captured_msgs: Arc::new(Mutex::new(Vec::new())),
            captured_systems: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Snapshot the captured `msgs` arguments (one entry per `messages_create` call).
    pub async fn captured_msgs(&self) -> Vec<Vec<ConversationMessage>> {
        self.captured_msgs.lock().await.clone()
    }

    /// Snapshot the captured `system` arguments (one entry per call;
    /// `None` for calls that passed no system prompt). Added M5-03 to
    /// support prompt-wiring assertions.
    pub async fn captured_systems(&self) -> Vec<Option<String>> {
        self.captured_systems.lock().await.clone()
    }

    /// Number of responses still queued.
    pub async fn remaining(&self) -> usize {
        self.queue.lock().await.len()
    }
}

#[async_trait]
impl OrchestratorApiClient for MockApiClient {
    async fn messages_create(
        &self,
        _model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError> {
        self.captured_msgs.lock().await.push(msgs);
        self.captured_systems
            .lock()
            .await
            .push(system.map(str::to_string));
        let mut q = self.queue.lock().await;
        q.pop_front().ok_or_else(|| ApiError::Server {
            status: 500,
            body: "mock script exhausted".into(),
        })
    }
}

/// Tiny helper for tests to construct a fully populated `MessageResponse`
/// without typing out every field. Defaults: zero usage, no thinking,
/// caller picks the content blocks + `stop_reason`.
#[must_use]
pub fn mock_message_response(
    content: Vec<lingxi_api_client::types::ContentBlockApi>,
    stop_reason: Option<&str>,
) -> MessageResponse {
    MessageResponse {
        id: "msg_mock".to_string(),
        model: "claude-opus-4-7".to_string(),
        content,
        stop_reason: stop_reason.map(str::to_string),
        usage: UsageApi::default(),
    }
}

// ============================================================================
// MockOutputStream (Task 7)
// ============================================================================

/// Capture all `OutputStream` events into an in-memory `Vec` for assertion.
pub struct MockOutputStream {
    events: Arc<Mutex<Vec<OutputEvent>>>,
}

impl MockOutputStream {
    /// Construct an empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Snapshot the captured events.
    pub async fn snapshot(&self) -> Vec<OutputEvent> {
        self.events.lock().await.clone()
    }

    /// Convenience: text events in capture order.
    pub async fn text_events(&self) -> Vec<String> {
        self.events
            .lock()
            .await
            .iter()
            .filter_map(|e| match e {
                OutputEvent::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Convenience: tool-call events in capture order.
    pub async fn tool_calls(&self) -> Vec<(String, serde_json::Value)> {
        self.events
            .lock()
            .await
            .iter()
            .filter_map(|e| match e {
                OutputEvent::ToolCall { tool, input } => Some((tool.clone(), input.clone())),
                _ => None,
            })
            .collect()
    }
}

impl Default for MockOutputStream {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OutputStream for MockOutputStream {
    async fn emit_text(&self, text: &str) {
        self.events.lock().await.push(OutputEvent::Text {
            text: text.to_string(),
        });
    }
    async fn emit_tool_call(&self, tool: &str, input: &serde_json::Value) {
        self.events.lock().await.push(OutputEvent::ToolCall {
            tool: tool.to_string(),
            input: input.clone(),
        });
    }
    async fn emit_tool_result(&self, tool: &str, result: &serde_json::Value) {
        self.events.lock().await.push(OutputEvent::ToolResult {
            tool: tool.to_string(),
            result: result.clone(),
        });
    }
    async fn emit_end_turn(&self, stop_reason: &str, cost: &CostSnapshot) {
        self.events.lock().await.push(OutputEvent::EndTurn {
            stop_reason: stop_reason.to_string(),
            cost: cost.clone(),
        });
    }
}

// ============================================================================
// HookExecutor + PermissionGate stubs (Task 8)
// ============================================================================
//
// These local traits will be renamespaced or replaced by M5-05 (real
// PermissionGate) and M5-06 (real 4-arm HookExecutor). M5-02 ships
// allow-all stubs against minimal trait surfaces so the orchestrator can
// be constructed in tests without dragging in the full hooks/permission
// machinery.

// M5-06 Task 14: the local `HookExecutor` trait that M5-02 introduced is
// replaced by the real `lingxi_hooks::HookExecutorImpl`. We re-export the
// concrete type so existing imports (crate::test_support::HookExecutor)
// keep working as a type alias.
pub use lingxi_hooks::HookExecutorImpl as HookExecutor;

/// Construct an empty `HookExecutorImpl` suitable for tests + the
/// orchestrator's "no hooks configured" path. The registry is empty so
/// `execute()` always returns a fresh `AggregateHookResult::default()`
/// without ever calling the supplied http/runtime stubs.
///
/// M5-06 Task 14: replaces the M5-02 `NoOpHookExecutor` unit struct so
/// the orchestrator can carry an `Arc<HookExecutorImpl>` instead of an
/// `Arc<dyn local::HookExecutor>` trait object.
#[must_use]
pub fn noop_hook_executor() -> Arc<lingxi_hooks::HookExecutorImpl> {
    use lingxi_hooks::registry::HookRegistry;

    struct UnusedHttp;
    #[async_trait]
    impl lingxi_traits::HttpTransport for UnusedHttp {
        async fn request(
            &self,
            _req: lingxi_protocol::HttpRequest,
        ) -> Result<lingxi_protocol::HttpResponse, lingxi_traits::HttpError> {
            Err(lingxi_traits::HttpError::InvalidRequest(
                "noop hook executor — http arm is never called with an empty registry".into(),
            ))
        }
        async fn stream_sse(
            &self,
            _req: lingxi_protocol::HttpRequest,
        ) -> Result<lingxi_traits::http::SseStream, lingxi_traits::HttpError> {
            Err(lingxi_traits::HttpError::InvalidRequest(
                "noop hook executor — sse arm is never called".into(),
            ))
        }
    }

    struct UnusedRuntime;
    #[async_trait]
    impl lingxi_traits::RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _name: &str,
            _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<lingxi_traits::BackgroundTaskHandle, lingxi_traits::RuntimeError> {
            Err(lingxi_traits::RuntimeError::Internal(
                "noop hook executor — runtime arm is never called".into(),
            ))
        }
        async fn sleep(&self, _duration: std::time::Duration) {}
        async fn cancel(
            &self,
            _handle: &lingxi_traits::BackgroundTaskHandle,
        ) -> Result<(), lingxi_traits::RuntimeError> {
            Ok(())
        }
    }

    let registry = Arc::new(tokio::sync::RwLock::new(HookRegistry::new()));
    let http: Arc<dyn lingxi_traits::HttpTransport> = Arc::new(UnusedHttp);
    let runtime: Arc<dyn lingxi_traits::RuntimeSpawner> = Arc::new(UnusedRuntime);
    Arc::new(lingxi_hooks::HookExecutorImpl::new(registry, http, runtime))
}

// M5-06 Task 14: M5-02's `pub struct NoOpHookExecutor;` is gone — the
// orchestrator now carries `Arc<HookExecutorImpl>` directly. Call sites
// previously using `Arc::new(NoOpHookExecutor)` should now call
// `crate::test_support::noop_hook_executor()` (which returns the Arc
// directly).

// M5-05 Task 2: PermissionGate + PermissionDecision are promoted to
// lingxi-traits::permission_gate. We re-export them here so existing
// orchestrator imports (crate::test_support::PermissionGate, …) keep
// working unchanged.
pub use lingxi_permission::gate::{PermissionDecision, PermissionGate};

/// Allow-all permission gate. Always returns `Allow`.
///
/// **M5-05:** the trait surface moved to `lingxi-traits` but the impl
/// stays here for back-compat with M5-02 / M5-04 tests that import
/// `crate::test_support::NoOpPermissionGate`. Production wiring (M5-12
/// CLI) chooses between this no-op and
/// [`lingxi_permission::InteractivePromptingGate`] based on
/// [`crate::OrchestratorConfig::interactive_permissions`].
pub struct NoOpPermissionGate;

#[async_trait]
impl PermissionGate for NoOpPermissionGate {
    async fn check(&self, _tool_name: &str, _input: &serde_json::Value) -> PermissionDecision {
        PermissionDecision::Allow
    }
}

// ============================================================================
// StaticMemoryProvider (Task 9)
// ============================================================================

/// Test fixture: returns a fixed `Vec<MemoryFile>` regardless of cwd.
/// Used by the prompt-wiring integration tests in M5-03 so they can
/// drive the orchestrator without touching the filesystem.
pub struct StaticMemoryProvider {
    files: Vec<crate::prompt::MemoryFile>,
}

impl StaticMemoryProvider {
    /// Empty fixture — `load()` always returns `vec![]`.
    #[must_use]
    pub fn empty() -> Self {
        Self { files: Vec::new() }
    }

    /// Pre-loaded fixture — `load()` always returns the provided files.
    #[must_use]
    pub fn with_files(files: Vec<crate::prompt::MemoryFile>) -> Self {
        Self { files }
    }
}

#[async_trait]
impl crate::prompt::MemoryHierarchyProvider for StaticMemoryProvider {
    async fn load(&self, _cwd: &std::path::Path) -> Vec<crate::prompt::MemoryFile> {
        self.files.clone()
    }
}

// ============================================================================
// Streaming-path test fixtures (re-exports — M5-04)
// ============================================================================
//
// `test_support_stream` is the home of `MockStreamingApiClient`,
// `MockToolDispatchClock`, the per-event helpers (`message_start`,
// `text_delta`, …) and the `scripted!` macro. Re-export them through
// the `test_support` namespace so integration tests can `use
// lingxi_orchestrator::test_support::{MockStreamingApiClient, …}`
// without importing two distinct modules.

pub use crate::test_support_stream::{
    content_block_start_text, content_block_start_tool_use, content_block_stop, input_json_delta,
    message_delta_stop, message_start, message_stop, ping, text_delta, MockStreamingApiClient,
    MockToolDispatchClock,
};

// ============================================================================
// MockOrchestratorHandle (M5-10 Task 2)
// ============================================================================
//
// Scripted mock of `lingxi_traits::OrchestratorHandle` for the M5-10/M5-11
// slash-command handler tests. Captures every call as a flag/counter and
// returns whatever the test pre-loaded via setter methods.

use lingxi_protocol::SessionId;
use lingxi_traits::{
    AgentInfo, CompactionSummary, DoctorReport, HandleError, HookInfo, McpServerInfo,
    MemoryEditorOutcome, OrchestratorHandle, StatusSnapshot,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;

/// Test double for `OrchestratorHandle`.
///
/// Defaults: `current_session_id` returns a stable v4 UUID; all mutators
/// return `Ok(())` (or appropriate success defaults); flags are recorded
/// for later assertion via `was_*_called()` accessors.
pub struct MockOrchestratorHandle {
    /// Stable session id returned by `current_session_id`.
    session_id: SessionId,
    /// Number of `clear_session` calls.
    clear_calls: AtomicUsize,
    /// If `Some`, `clear_session` returns `ActionFailed(_)` instead of `Ok(())`.
    ///
    /// Uses `std::sync::Mutex` (NOT `tokio::sync::Mutex`) so test code can
    /// set the value synchronously without an `await` and without
    /// `blocking_lock()` (which would panic inside the tokio runtime).
    clear_error: StdMutex<Option<String>>,
    /// Pre-loaded `CompactionSummary` returned by `force_compact`. If not
    /// set, defaults to `CompactionSummary::default()`.
    compact_summary: StdMutex<Option<CompactionSummary>>,
    /// If `Some`, `force_compact` returns `ActionFailed(_)`.
    compact_error: StdMutex<Option<String>>,
    /// Bumped each `switch_model` call. Records the most-recent value too.
    switch_model_calls: AtomicUsize,
    switch_model_last: StdMutex<Option<String>>,
    /// If `Some`, `switch_model` returns `ActionFailed(_)`.
    switch_model_error: StdMutex<Option<String>>,
    /// Set by `request_exit`. Readable via `was_exit_requested`.
    exit_requested: AtomicBool,
    /// Pre-loaded path for `open_memory_editor`.
    memory_path: StdMutex<Option<PathBuf>>,
    /// Pre-loaded exit code for `open_memory_editor`.
    editor_exit_code: AtomicI32,
    /// If `Some`, `open_memory_editor` returns `ActionFailed(_)`.
    memory_error: StdMutex<Option<String>>,
    /// Cost snapshot fields (rarely exercised in M5-10).
    cost_nano_usd: AtomicU64,
    cost_tokens: AtomicU64,
    /// Optional pre-loaded full cost snapshot returned by `snapshot_cost`.
    /// If `Some`, used verbatim (with `session_id` overwritten to mock's id).
    cost_snapshot: StdMutex<Option<lingxi_traits::CostSnapshot>>,
    // M5-11 additions:
    /// Pre-loaded MCP server list returned by `list_mcp_servers`.
    mcp_servers: StdMutex<Vec<McpServerInfo>>,
    /// Pre-loaded hooks list returned by `list_hooks`.
    hooks_list: StdMutex<Vec<HookInfo>>,
    /// Pre-loaded agents list returned by `list_agents`.
    agents_list: StdMutex<Vec<AgentInfo>>,
    /// Pre-loaded doctor report returned by `run_doctor_checks`.
    doctor_report: StdMutex<DoctorReport>,
    /// Pre-loaded status snapshot returned by `get_status_snapshot`.
    status_snapshot: StdMutex<StatusSnapshot>,
    /// If `Some`, `edit_config_file` returns `ActionFailed(_)`.
    config_editor_error: StdMutex<Option<String>>,
    /// If `Some`, `edit_permissions_file` returns `ActionFailed(_)`.
    permissions_editor_error: StdMutex<Option<String>>,
    /// Pre-loaded available models list returned by `list_available_models`.
    available_models: StdMutex<Vec<String>>,
}

impl MockOrchestratorHandle {
    /// Construct a fresh mock with sane defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            session_id: SessionId::new(),
            clear_calls: AtomicUsize::new(0),
            clear_error: StdMutex::new(None),
            compact_summary: StdMutex::new(None),
            compact_error: StdMutex::new(None),
            switch_model_calls: AtomicUsize::new(0),
            switch_model_last: StdMutex::new(None),
            switch_model_error: StdMutex::new(None),
            exit_requested: AtomicBool::new(false),
            memory_path: StdMutex::new(None),
            editor_exit_code: AtomicI32::new(0),
            memory_error: StdMutex::new(None),
            cost_nano_usd: AtomicU64::new(0),
            cost_tokens: AtomicU64::new(0),
            cost_snapshot: StdMutex::new(None),
            mcp_servers: StdMutex::new(Vec::new()),
            hooks_list: StdMutex::new(Vec::new()),
            agents_list: StdMutex::new(Vec::new()),
            doctor_report: StdMutex::new(DoctorReport::default()),
            status_snapshot: StdMutex::new(StatusSnapshot::default()),
            config_editor_error: StdMutex::new(None),
            permissions_editor_error: StdMutex::new(None),
            available_models: StdMutex::new(Vec::new()),
        }
    }

    /// Make the next `clear_session` call return `ActionFailed(reason)`.
    pub fn set_clear_session_error(&self, reason: String) {
        *self.clear_error.lock().unwrap() = Some(reason);
    }
    /// True if `clear_session` was called at least once.
    pub fn was_clear_session_called(&self) -> bool {
        self.clear_calls.load(Ordering::SeqCst) > 0
    }

    /// Pre-load the `CompactionSummary` returned by `force_compact`.
    pub fn set_compact_summary(&self, s: CompactionSummary) {
        *self.compact_summary.lock().unwrap() = Some(s);
    }
    /// Make the next `force_compact` call return `ActionFailed(reason)`.
    pub fn set_compact_error(&self, reason: String) {
        *self.compact_error.lock().unwrap() = Some(reason);
    }

    /// True if `request_exit` was called.
    pub fn was_exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::SeqCst)
    }

    /// Pre-load the path `open_memory_editor` reports.
    pub fn set_memory_path(&self, p: PathBuf) {
        *self.memory_path.lock().unwrap() = Some(p);
    }
    /// Pre-load the exit code `open_memory_editor` reports.
    pub fn set_editor_exit_code(&self, c: i32) {
        self.editor_exit_code.store(c, Ordering::SeqCst);
    }
    /// Make the next `open_memory_editor` call return `ActionFailed(reason)`.
    pub fn set_memory_editor_error(&self, reason: String) {
        *self.memory_error.lock().unwrap() = Some(reason);
    }

    /// Number of `switch_model` calls so far.
    pub fn switch_model_call_count(&self) -> usize {
        self.switch_model_calls.load(Ordering::SeqCst)
    }
    /// Most-recent model passed to `switch_model`, or `None`.
    pub fn last_switched_model(&self) -> Option<String> {
        self.switch_model_last.lock().unwrap().clone()
    }
    /// Make the next `switch_model` call return `ActionFailed(reason)`.
    pub fn set_switch_model_error(&self, reason: String) {
        *self.switch_model_error.lock().unwrap() = Some(reason);
    }
    /// Pre-load the full `CostSnapshot` returned by `snapshot_cost`. If set,
    /// the snapshot is returned verbatim (with `session_id` overwritten to
    /// the mock's stable id).
    pub fn set_cost_snapshot(&self, s: lingxi_traits::CostSnapshot) {
        *self.cost_snapshot.lock().unwrap() = Some(s);
    }
    // M5-11 setters:
    /// Pre-load the MCP server list returned by `list_mcp_servers`.
    pub fn set_mcp_servers(&self, v: Vec<McpServerInfo>) {
        *self.mcp_servers.lock().unwrap() = v;
    }
    /// Pre-load the hooks list returned by `list_hooks`.
    pub fn set_hooks(&self, v: Vec<HookInfo>) {
        *self.hooks_list.lock().unwrap() = v;
    }
    /// Pre-load the agents list returned by `list_agents`.
    pub fn set_agents(&self, v: Vec<AgentInfo>) {
        *self.agents_list.lock().unwrap() = v;
    }
    /// Pre-load the doctor report returned by `run_doctor_checks`.
    pub fn set_doctor_report(&self, r: DoctorReport) {
        *self.doctor_report.lock().unwrap() = r;
    }
    /// Pre-load the status snapshot returned by `get_status_snapshot`.
    pub fn set_status_snapshot(&self, s: StatusSnapshot) {
        *self.status_snapshot.lock().unwrap() = s;
    }
    /// Make the next `edit_config_file` call return `ActionFailed(reason)`.
    pub fn set_config_editor_error(&self, e: String) {
        *self.config_editor_error.lock().unwrap() = Some(e);
    }
    /// Make the next `edit_permissions_file` call return `ActionFailed(reason)`.
    pub fn set_permissions_editor_error(&self, e: String) {
        *self.permissions_editor_error.lock().unwrap() = Some(e);
    }
    /// Pre-load the list returned by `list_available_models`.
    pub fn set_available_models(&self, m: Vec<String>) {
        *self.available_models.lock().unwrap() = m;
    }
}

impl Default for MockOrchestratorHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OrchestratorHandle for MockOrchestratorHandle {
    async fn current_session_id(&self) -> SessionId {
        self.session_id
    }

    async fn clear_session(&self) -> Result<(), HandleError> {
        self.clear_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(reason) = self.clear_error.lock().unwrap().take() {
            return Err(HandleError::ActionFailed(reason));
        }
        Ok(())
    }

    async fn force_compact(&self) -> Result<CompactionSummary, HandleError> {
        if let Some(reason) = self.compact_error.lock().unwrap().take() {
            return Err(HandleError::ActionFailed(reason));
        }
        Ok(self
            .compact_summary
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default())
    }

    async fn snapshot_cost(&self) -> lingxi_traits::CostSnapshot {
        if let Some(s) = self.cost_snapshot.lock().unwrap().clone() {
            // Force the session id to match the mock's stable id for
            // consistency with other handle methods.
            return lingxi_traits::CostSnapshot {
                session_id: self.session_id,
                ..s
            };
        }
        lingxi_traits::CostSnapshot {
            session_id: self.session_id,
            total_nano_usd: self.cost_nano_usd.load(Ordering::SeqCst),
            total_tokens: self.cost_tokens.load(Ordering::SeqCst),
            ..lingxi_traits::CostSnapshot::default()
        }
    }

    async fn switch_model(&self, model: &str) -> Result<(), HandleError> {
        self.switch_model_calls.fetch_add(1, Ordering::SeqCst);
        *self.switch_model_last.lock().unwrap() = Some(model.to_string());
        if let Some(reason) = self.switch_model_error.lock().unwrap().take() {
            return Err(HandleError::ActionFailed(reason));
        }
        Ok(())
    }

    async fn request_exit(&self) {
        self.exit_requested.store(true, Ordering::SeqCst);
    }

    async fn current_should_exit(&self) -> bool {
        self.exit_requested.load(Ordering::SeqCst)
    }

    async fn open_memory_editor(&self) -> Result<MemoryEditorOutcome, HandleError> {
        if let Some(reason) = self.memory_error.lock().unwrap().take() {
            return Err(HandleError::ActionFailed(reason));
        }
        Ok(MemoryEditorOutcome {
            edited_path: self
                .memory_path
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| PathBuf::from("/dev/null/CLAUDE.md")),
            exit_code: self.editor_exit_code.load(Ordering::SeqCst),
        })
    }

    // M5-11 additions:

    async fn list_mcp_servers(&self) -> Vec<McpServerInfo> {
        self.mcp_servers.lock().unwrap().clone()
    }

    async fn list_hooks(&self) -> Vec<HookInfo> {
        self.hooks_list.lock().unwrap().clone()
    }

    async fn list_agents(&self) -> Vec<AgentInfo> {
        self.agents_list.lock().unwrap().clone()
    }

    async fn run_doctor_checks(&self) -> DoctorReport {
        self.doctor_report.lock().unwrap().clone()
    }

    async fn get_status_snapshot(&self) -> StatusSnapshot {
        self.status_snapshot.lock().unwrap().clone()
    }

    async fn edit_config_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
        if let Some(e) = self.config_editor_error.lock().unwrap().take() {
            return Err(HandleError::ActionFailed(e));
        }
        Ok(MemoryEditorOutcome {
            edited_path: PathBuf::from("/tmp/mock/config.json"),
            exit_code: 0,
        })
    }

    async fn edit_permissions_file(&self) -> Result<MemoryEditorOutcome, HandleError> {
        if let Some(e) = self.permissions_editor_error.lock().unwrap().take() {
            return Err(HandleError::ActionFailed(e));
        }
        Ok(MemoryEditorOutcome {
            edited_path: PathBuf::from("/tmp/mock/permissions.json"),
            exit_code: 0,
        })
    }

    async fn list_available_models(&self) -> Vec<String> {
        self.available_models.lock().unwrap().clone()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_api_client::types::ContentBlockApi;

    // -------- MockApiClient (Task 6) --------

    #[tokio::test]
    async fn mock_returns_responses_in_order() {
        let r1 = mock_message_response(
            vec![ContentBlockApi::Text { text: "one".into() }],
            Some("end_turn"),
        );
        let r2 = mock_message_response(
            vec![ContentBlockApi::Text { text: "two".into() }],
            Some("end_turn"),
        );
        let mock = MockApiClient::new(vec![r1, r2]);
        let resp1 = mock
            .messages_create("m", None, vec![])
            .await
            .expect("first");
        let resp2 = mock
            .messages_create("m", None, vec![])
            .await
            .expect("second");
        let ContentBlockApi::Text { text: first_text } = &resp1.content[0] else {
            panic!("expected text block");
        };
        let ContentBlockApi::Text { text: second_text } = &resp2.content[0] else {
            panic!("expected text block");
        };
        assert_eq!(first_text, "one");
        assert_eq!(second_text, "two");
        assert_eq!(mock.remaining().await, 0);
    }

    #[tokio::test]
    async fn mock_captures_msgs_per_call() {
        let r = mock_message_response(vec![], Some("end_turn"));
        let mock = MockApiClient::new(vec![r]);
        let msgs = vec![];
        mock.messages_create("m", None, msgs).await.expect("call");
        assert_eq!(mock.captured_msgs().await.len(), 1);
    }

    #[tokio::test]
    async fn mock_exhaustion_returns_server_error() {
        let mock = MockApiClient::new(vec![]);
        let err = mock
            .messages_create("m", None, vec![])
            .await
            .expect_err("exhausted");
        assert!(format!("{err}").contains("mock script exhausted"));
    }

    // -------- MockOutputStream (Task 7) --------

    #[tokio::test]
    async fn mock_output_stream_captures_text() {
        let m = MockOutputStream::new();
        m.emit_text("hello").await;
        m.emit_text("world").await;
        let texts = m.text_events().await;
        assert_eq!(texts, vec!["hello".to_string(), "world".to_string()]);
    }

    #[tokio::test]
    async fn mock_output_stream_captures_tool_lifecycle() {
        let m = MockOutputStream::new();
        let input = serde_json::json!({"file_path": "/tmp/x"});
        let result = serde_json::json!({"content": "ok"});
        m.emit_tool_call("Read", &input).await;
        m.emit_tool_result("Read", &result).await;
        let snap = m.snapshot().await;
        assert_eq!(snap.len(), 2);
        assert!(matches!(snap[0], OutputEvent::ToolCall { .. }));
        assert!(matches!(snap[1], OutputEvent::ToolResult { .. }));
    }

    #[tokio::test]
    async fn mock_output_stream_captures_end_turn() {
        let m = MockOutputStream::new();
        // SessionId::default() mints a fresh v4 UUID, so we can't compare two
        // `CostSnapshot::default()` instances structurally. Bind a single
        // cost value and check the captured Clone matches that instance.
        let cost = CostSnapshot::default();
        m.emit_end_turn("end_turn", &cost).await;
        let snap = m.snapshot().await;
        assert_eq!(snap.len(), 1);
        match &snap[0] {
            OutputEvent::EndTurn {
                stop_reason,
                cost: c,
            } => {
                assert_eq!(stop_reason, "end_turn");
                assert_eq!(c, &cost);
                assert_eq!(c.total_nano_usd, 0);
                assert_eq!(c.total_tokens, 0);
            }
            _ => panic!("expected EndTurn"),
        }
    }

    // -------- NoOp hooks + permission (Task 8) --------

    #[tokio::test]
    async fn noop_hook_executor_returns_empty_aggregate() {
        let h = noop_hook_executor();
        let event = lingxi_hooks::events::HookEvent::PreToolUse {
            tool_name: "Read".into(),
            tool_input: serde_json::json!({}),
            tool_use_id: lingxi_protocol::ToolUseId::new(),
        };
        let ctx = lingxi_hooks::registry::HookContext::default();
        let agg = h.execute(event, ctx).await;
        assert!(agg.decision.is_none());
        assert!(agg.modified_input.is_none());
        assert!(agg.system_messages.is_empty());
    }

    #[tokio::test]
    async fn noop_permission_gate_always_allows() {
        let g = NoOpPermissionGate;
        let v = serde_json::json!({});
        assert_eq!(g.check("Read", &v).await, PermissionDecision::Allow);
        assert_eq!(g.check("Bash", &v).await, PermissionDecision::Allow);
    }
}
