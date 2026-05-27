//! Mid-stream tool dispatch (M5-04 Task 10 — RED).
//!
//! Asserts that a `tool_use` block is dispatched THE MOMENT its
//! `content_block_stop` arrives — i.e. BEFORE the subsequent
//! `message_delta` + `message_stop` events.
//!
//! The visible signal is the `OutputEvent` ordering:
//!   Text("calling tool"), ToolCall("AlwaysOk"), ToolResult("AlwaysOk"),
//!   Text("done"), EndTurn { stop_reason: "end_turn" }
//!
//! Fails to compile until Tasks 11-13 land the integrated
//! `run_turn_streaming` + concurrent dispatch.

use async_trait::async_trait;
use lingxi_orchestrator::test_support::{
    content_block_start_text, content_block_start_tool_use, content_block_stop, input_json_delta,
    message_delta_stop, message_start, message_stop, text_delta, MockApiClient, MockOutputStream,
    MockStreamingApiClient, NoOpHookExecutor, NoOpPermissionGate, StaticMemoryProvider,
};
use lingxi_orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig};
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_protocol::ToolUseId;
use lingxi_tools::progress::ToolProgressSender;
use lingxi_tools::registry::ToolRegistry;
use lingxi_tools::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use lingxi_traits::OutputEvent;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

/// Always-ok test tool. Mirrors the AlwaysOkTool from
/// orchestrator_multi_turn_test.rs verbatim — reused here so the
/// streaming path can dispatch a real tool through the existing
/// `dispatch_tool_uses` pipeline.
struct AlwaysOkTool;

#[async_trait]
impl Tool for AlwaysOkTool {
    fn name(&self) -> &str {
        "AlwaysOk"
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
            once_cell::sync::Lazy::new(|| json!({"type": "object"}));
        &SCHEMA
    }
    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        1024
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }
    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _ctx: &lingxi_tools::context::ToolUseContext,
    ) -> Result<(), ValidationError> {
        Ok(())
    }
    async fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &lingxi_tools::context::ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "test".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }
    async fn description(&self, _input: &serde_json::Value, _opts: &DescriptionOptions) -> String {
        "AlwaysOk".into()
    }
    async fn prompt(&self, _opts: &PromptOptions) -> String {
        String::new()
    }
    async fn call(
        &self,
        _input: serde_json::Value,
        _ctx: lingxi_tools::context::ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        Ok(ToolCallResult {
            data: json!({"ok": true}),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[tokio::test]
async fn tool_use_dispatched_before_message_stop() {
    let tu_id = ToolUseId::new();
    let stream = scripted![
        message_start("m1", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "calling tool"),
        content_block_stop(0),
        content_block_start_tool_use(1, tu_id, "AlwaysOk"),
        input_json_delta(1, "{}"),
        content_block_stop(1), // dispatch fires HERE
        message_delta_stop("tool_use"),
        message_stop(),
    ];

    // Turn 2: model returns "done" after seeing the tool result.
    let turn2 = scripted![
        message_start("m2", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "done"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];

    let api = Arc::new(MockStreamingApiClient::with_turns(vec![stream, turn2]));
    let batched = Arc::new(MockApiClient::new(Vec::new())); // unused on streaming path
    let output = Arc::new(MockOutputStream::new());

    let mut registry = ToolRegistry::new();
    registry.register_builtin(Arc::new(AlwaysOkTool));
    let tools = Arc::new(registry);
    let hooks = Arc::new(NoOpHookExecutor);
    let perms = Arc::new(NoOpPermissionGate);
    let memory = Arc::new(StaticMemoryProvider::empty());

    let orch = ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        api.clone(),
        tools,
        hooks,
        perms,
        output.clone(),
        memory,
        PathBuf::from("/tmp"),
    );

    let _outcome = orch.run_turn_streaming("call a tool").await.expect("ok");

    // Output sequence: Text("calling tool"), ToolCall("AlwaysOk"),
    // ToolResult("AlwaysOk"), Text("done"), EndTurn.
    let events = output.snapshot().await;
    let mut iter = events.iter();
    assert!(
        matches!(iter.next(), Some(OutputEvent::Text { text }) if text == "calling tool"),
        "events: {events:?}"
    );
    assert!(
        matches!(iter.next(), Some(OutputEvent::ToolCall { tool, .. }) if tool == "AlwaysOk"),
        "events: {events:?}"
    );
    assert!(
        matches!(iter.next(), Some(OutputEvent::ToolResult { tool, .. }) if tool == "AlwaysOk"),
        "events: {events:?}"
    );
    assert!(
        matches!(iter.next(), Some(OutputEvent::Text { text }) if text == "done"),
        "events: {events:?}"
    );
    assert!(
        matches!(iter.next(), Some(OutputEvent::EndTurn { stop_reason, .. }) if stop_reason == "end_turn"),
        "events: {events:?}"
    );
    assert!(iter.next().is_none(), "events: {events:?}");

    assert_eq!(api.captured_calls().await.len(), 2);
}
