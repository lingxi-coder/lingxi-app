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
}

impl MockApiClient {
    /// Construct a mock with a script of `responses` returned in order.
    #[must_use]
    pub fn new(responses: Vec<MessageResponse>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::from(responses))),
            captured_msgs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Snapshot the captured `msgs` arguments (one entry per `messages_create` call).
    pub async fn captured_msgs(&self) -> Vec<Vec<ConversationMessage>> {
        self.captured_msgs.lock().await.clone()
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
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError> {
        self.captured_msgs.lock().await.push(msgs);
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

/// Local hook executor trait used by `ConversationOrchestrator` until
/// M5-06 wires the real 4-arm executor from `lingxi-hooks`. Lives here
/// (not in `lingxi-traits`) because M5-06 will move it.
#[async_trait]
pub trait HookExecutor: Send + Sync {
    /// Run before a tool dispatch. Errors abort the dispatch with the
    /// returned string surfaced as a tool error.
    async fn pre_tool_use(&self, tool_name: &str, input: &serde_json::Value) -> Result<(), String>;

    /// Run after a tool dispatch. Errors are logged but do NOT abort the
    /// turn — the orchestrator only inspects the returned `Result` for
    /// post-side hook failures.
    async fn post_tool_use(
        &self,
        tool_name: &str,
        output: &serde_json::Value,
        is_error: bool,
    ) -> Result<(), String>;
}

/// Allow-all hook executor. Does nothing on pre/post.
pub struct NoOpHookExecutor;

#[async_trait]
impl HookExecutor for NoOpHookExecutor {
    async fn pre_tool_use(
        &self,
        _tool_name: &str,
        _input: &serde_json::Value,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn post_tool_use(
        &self,
        _tool_name: &str,
        _output: &serde_json::Value,
        _is_error: bool,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Local permission gate trait. M5-05 will swap this for
/// `lingxi_permission::PermissionGate` (or extend it with a `PromptingGate`
/// arm). For M5-02 the orchestrator only needs an allow/deny decision.
#[async_trait]
pub trait PermissionGate: Send + Sync {
    /// Return the decision for a `(tool_name, input)` pair.
    async fn check(&self, tool_name: &str, input: &serde_json::Value) -> PermissionDecision;
}

/// Decision returned by `PermissionGate::check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Tool dispatch may proceed.
    Allow,
    /// Tool dispatch is denied. The orchestrator turns this into a
    /// `ContentBlock::ToolResult { is_error: true, content: "Permission denied: <reason>" }`.
    Deny {
        /// Human-readable explanation surfaced into the tool-result block.
        reason: String,
    },
}

/// Allow-all permission gate. Always returns `Allow`.
pub struct NoOpPermissionGate;

#[async_trait]
impl PermissionGate for NoOpPermissionGate {
    async fn check(&self, _tool_name: &str, _input: &serde_json::Value) -> PermissionDecision {
        PermissionDecision::Allow
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
        let resp1 = mock.messages_create("m", vec![]).await.expect("first");
        let resp2 = mock.messages_create("m", vec![]).await.expect("second");
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
        mock.messages_create("m", msgs).await.expect("call");
        assert_eq!(mock.captured_msgs().await.len(), 1);
    }

    #[tokio::test]
    async fn mock_exhaustion_returns_server_error() {
        let mock = MockApiClient::new(vec![]);
        let err = mock
            .messages_create("m", vec![])
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
    async fn noop_hook_executor_always_succeeds() {
        let h = NoOpHookExecutor;
        let v = serde_json::json!({});
        assert!(h.pre_tool_use("Read", &v).await.is_ok());
        assert!(h.post_tool_use("Read", &v, false).await.is_ok());
        assert!(h.post_tool_use("Read", &v, true).await.is_ok());
    }

    #[tokio::test]
    async fn noop_permission_gate_always_allows() {
        let g = NoOpPermissionGate;
        let v = serde_json::json!({});
        assert_eq!(g.check("Read", &v).await, PermissionDecision::Allow);
        assert_eq!(g.check("Bash", &v).await, PermissionDecision::Allow);
    }
}
