//! `run_turn_streaming_with_cancel` happy path + cancellation. (M6-03 Task 1)
//!
//! Mirrors the M5-13 `repl_turn_test.rs` structure but exercises the
//! streaming path. The orchestrator races `try_run_turn_streaming` against
//! the cancel token and returns `TurnOutcome::{EndTurn, Cancelled}`.

use orchestrator::test_support::{
    MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::test_support_stream::{
    content_block_start_text, content_block_stop, message_delta_stop, message_start, message_stop,
    text_delta, MockStreamingApiClient,
};
use orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig, TurnOutcome};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tool_api::registry::ToolRegistry;

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
