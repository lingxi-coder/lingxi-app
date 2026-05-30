//! Multi-turn streaming — turn 1 `tool_use`, turn 2 `end_turn` (M5-04 Task 15).

use async_trait::async_trait;
use lingxi_orchestrator::test_support::{
    content_block_start_text, content_block_start_tool_use, content_block_stop, input_json_delta,
    message_delta_stop, message_start, message_stop, text_delta, MockApiClient, MockOutputStream,
    MockStreamingApiClient, NoOpPermissionGate, StaticMemoryProvider,
};
use lingxi_orchestrator::{
    scripted, ConversationOrchestrator, ConversationOutcome, OrchestratorConfig,
};
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

fn registry_with_always_ok() -> Arc<ToolRegistry> {
    let mut r = ToolRegistry::new();
    r.register_builtin(Arc::new(AlwaysOkTool));
    Arc::new(r)
}

#[tokio::test]
async fn two_streaming_turns_with_tool_in_between() {
    let tu = ToolUseId::new();
    let turn1 = scripted![
        message_start("m1", "claude-opus-4-7"),
        content_block_start_tool_use(0, tu, "AlwaysOk"),
        input_json_delta(0, "{}"),
        content_block_stop(0),
        message_delta_stop("tool_use"),
        message_stop(),
    ];
    let turn2 = scripted![
        message_start("m2", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "done"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];

    let api = Arc::new(MockStreamingApiClient::with_turns(vec![turn1, turn2]));
    let batched = Arc::new(MockApiClient::new(Vec::new()));
    let output = Arc::new(MockOutputStream::new());
    let orch = ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        api.clone(),
        registry_with_always_ok(),
        lingxi_orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        output.clone(),
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    );

    let outcome = orch.run_turn_streaming("call then echo").await.expect("ok");
    match outcome {
        ConversationOutcome::EndTurn { turn_count, .. } => {
            assert_eq!(turn_count, 2);
        }
        _ => panic!("unexpected outcome variant"),
    }

    assert_eq!(api.captured_calls().await.len(), 2);

    let events = output.snapshot().await;
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            OutputEvent::Text { .. } => "Text",
            OutputEvent::ToolCall { .. } => "ToolCall",
            OutputEvent::ToolResult { .. } => "ToolResult",
            OutputEvent::EndTurn { .. } => "EndTurn",
            _ => "Other",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["ToolCall", "ToolResult", "Text", "EndTurn"],
        "events: {events:?}"
    );
}
