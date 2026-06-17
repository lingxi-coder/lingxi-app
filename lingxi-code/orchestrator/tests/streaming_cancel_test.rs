//!
//! Mirrors the M5-13 `repl_turn_test.rs` structure but exercises the
//! streaming path. The orchestrator races `try_run_turn_streaming` against
//! the cancel token and returns `TurnOutcome::{EndTurn, Cancelled}`.

use async_trait::async_trait;
use orchestrator::test_support::{
    MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::test_support_stream::{
    content_block_start_text, content_block_start_tool_use, content_block_stop, message_delta_stop,
    message_start, message_stop, text_delta, MockStreamingApiClient,
};
use orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig, TurnOutcome};
use protocol::{ContentBlock, ConversationMessage, ToolUseId};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tool_api::progress::ToolProgressSender;
use tool_api::registry::ToolRegistry;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

fn build_orch(api: Arc<MockStreamingApiClient>) -> ConversationOrchestrator {
    let batched = Arc::new(MockApiClient::new(Vec::new()));
    let output = Arc::new(MockOutputStream::new());
    let tools = Arc::new(ToolRegistry::new());
    let hooks = orchestrator::test_support::noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let memory = Arc::new(StaticMemoryProvider::empty());
    ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        api,
        tools,
        hooks,
        perms,
        output,
        memory,
        PathBuf::from("/tmp"),
    )
}

#[tokio::test]
async fn run_turn_streaming_with_cancel_returns_endturn_on_normal_completion() {
    let stream = scripted![
        message_start("msg_01", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "ok"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];
    let api = Arc::new(MockStreamingApiClient::with_turns(vec![stream]));
    let orch = build_orch(api);
    let cancel = CancellationToken::new();
    let outcome = orch
        .run_turn_streaming_with_cancel("hi", cancel)
        .await
        .unwrap();
    assert_eq!(outcome, TurnOutcome::EndTurn);
}

#[tokio::test]
async fn run_turn_streaming_with_cancel_returns_cancelled_when_token_fires_before_call() {
    let stream = scripted![
        message_start("msg_01", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "ok"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];
    let api = Arc::new(MockStreamingApiClient::with_turns(vec![stream]));
    let orch = build_orch(api);
    let cancel = CancellationToken::new();
    cancel.cancel(); // pre-cancel → should return Cancelled immediately
    let outcome = orch
        .run_turn_streaming_with_cancel("hi", cancel)
        .await
        .unwrap();
    assert_eq!(outcome, TurnOutcome::Cancelled);
}

#[tokio::test]
async fn handle_trait_run_turn_streaming_with_cancel_pre_cancelled_returns_cancelled() {
    // Exercise via the OrchestratorHandle trait surface (M6-03 T2) — the
    // TUI consumes this trait, not the concrete orchestrator type.
    let stream = scripted![
        message_start("msg_01", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "ok"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];
    let api = Arc::new(MockStreamingApiClient::with_turns(vec![stream]));
    let orch = Arc::new(build_orch(api));
    let handle: Arc<dyn traits::OrchestratorHandle> = orch;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let outcome = handle
        .run_turn_streaming_with_cancel("hi", cancel)
        .await
        .unwrap();
    assert!(matches!(outcome, traits::TurnOutcome::Cancelled));
}

// ============================================================================
// DEFERRED-3: user-ESC GRANULAR interrupt — in-flight Cancel-behavior tools are
// rejected with the bare REJECT_MESSAGE and the turn ENDS GRACEFULLY with those
// results recorded in history (NOT a whole-turn drop). Mirrors claude-code's
// `StreamingToolExecutor` user_interrupted path.
// ============================================================================

/// claude-code `REJECT_MESSAGE` (utils/messages.ts:212), duplicated here for the
/// assertion (the orchestrator's copy is private to the crate).
const REJECT_MESSAGE: &str = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";

/// A concurrency-SAFE tool whose `interrupt_behavior()==Cancel`. It blocks on its
/// `ctx.cancel` token (the per-tool child of the turn's user-interrupt token), so
/// it stays in-flight until the user interrupts; on cancel it returns `Aborted`.
struct CancelBlockingTool;

#[async_trait]
impl Tool for CancelBlockingTool {
    fn name(&self) -> &str {
        "CancelTool"
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
            once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
        &SCHEMA
    }
    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        1024 * 1024
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }
    fn interrupt_behavior(&self, _input: &serde_json::Value) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }
    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _ctx: &tool_api::context::ToolUseContext,
    ) -> Result<(), ValidationError> {
        Ok(())
    }
    async fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &tool_api::context::ToolUseContext,
    ) -> permission::PermissionResult {
        permission::PermissionResult::Allow {
            reason: permission::PermissionDecisionReason::Other { reason: "test".into() },
            updated_input: None,
            update_destination: None,
            metadata: permission::result::PermissionMetadata::default(),
        }
    }
    async fn description(
        &self,
        _input: &serde_json::Value,
        _opts: &DescriptionOptions,
    ) -> String {
        "cancel-tool".into()
    }
    async fn prompt(&self, _opts: &PromptOptions) -> String {
        String::new()
    }
    async fn call(
        &self,
        _input: serde_json::Value,
        ctx: tool_api::context::ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let token = ctx.cancel.clone();
        match token {
            Some(t) => {
                t.cancelled().await;
                Err(ToolError::Aborted)
            }
            // No token (non-streaming) → just run to "end" so the test fails loudly
            // if the token is not threaded through.
            None => Ok(ToolCallResult {
                data: json!({ "content": "cancel-tool-ran-to-end" }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            }),
        }
    }
}

fn build_orch_with_cancel_tool(api: Arc<MockStreamingApiClient>) -> ConversationOrchestrator {
    let batched = Arc::new(MockApiClient::new(Vec::new()));
    let output = Arc::new(MockOutputStream::new());
    let mut registry = ToolRegistry::new();
    registry.register_builtin(Arc::new(CancelBlockingTool) as Arc<dyn Tool>);
    let hooks = orchestrator::test_support::noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let memory = Arc::new(StaticMemoryProvider::empty());
    ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        api,
        Arc::new(registry),
        hooks,
        perms,
        output,
        memory,
        PathBuf::from("/tmp"),
    )
}

/// TDD anchor: a streaming turn emits ONE `tool_use` for a Cancel-behavior tool
/// that blocks until the user interrupts. We fire the user-cancel token while the
/// tool is in flight. The executor must substitute the bare `REJECT_MESSAGE` for
/// that tool, PERSIST it (model-visible), and the turn must END GRACEFULLY with
/// outcome `Cancelled` — NOT drop the turn (so the result IS in history).
#[tokio::test]
async fn user_interrupt_rejects_in_flight_tool_and_records_reject_message() {
    let id = ToolUseId::new();
    // turn 1: stream the tool_use for the blocking Cancel tool, stop_reason tool_use.
    let turn1 = scripted![
        message_start("m1", "claude-opus-4-7"),
        content_block_start_tool_use(0, id.clone(), "CancelTool"),
        content_block_stop(0),
        message_delta_stop("tool_use"),
        message_stop(),
    ];
    // turn 2: after the (rejected) tool result is appended, the loop continues and
    // makes a second call — which naturally ends the turn.
    let turn2 = scripted![
        message_start("m2", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "done"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];
    let api = Arc::new(MockStreamingApiClient::with_turns(vec![turn1, turn2]));
    let orch = Arc::new(build_orch_with_cancel_tool(api));
    let cancel = CancellationToken::new();

    // Fire the user-cancel shortly after the turn starts, so the blocking
    // CancelTool is in flight (awaiting its ctx.cancel) when it fires.
    let cancel_clone = cancel.clone();
    let firer = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });

    let outcome = orch
        .run_turn_streaming_with_cancel("call the cancel tool", cancel.clone())
        .await
        .unwrap();
    firer.await.unwrap();

    // The turn ended GRACEFULLY (not dropped). The TUI sees Cancelled because the
    // token fired.
    assert_eq!(outcome, TurnOutcome::Cancelled);

    // The interrupted tool's REJECT_MESSAGE result is recorded in history as its
    // own user message (model-visible transcript), NOT vanished by a turn drop.
    let session = orch.session();
    let s = session.lock().await;
    let reject = s.history.iter().find_map(|m| match m {
        ConversationMessage::User { content, .. }
            if content.len() == 1
                && matches!(&content[0], ContentBlock::ToolResult { .. }) =>
        {
            match &content[0] {
                ContentBlock::ToolResult { content, is_error, .. } => {
                    Some((content.clone(), *is_error))
                }
                _ => None,
            }
        }
        _ => None,
    });
    let (content, is_error) =
        reject.expect("interrupted tool_result must be recorded in history (no turn drop)");
    assert!(is_error, "user-interrupted tool_result must be is_error");
    assert_eq!(
        content, REJECT_MESSAGE,
        "in-flight Cancel tool must get the bare REJECT_MESSAGE on user interrupt"
    );
}
