//! S1 — the PRODUCTION [`OrchestratorTurnDriver`] (in the bridge-server library)
//! streams a real turn's engine events into a captured [`ClientEventSink`].
//!
//! This proves the driver lifted out of the F2-06 e2e test (`e2e_permission_test.rs`)
//! works as a library type: it wraps an `Arc<ConversationOrchestrator>` whose
//! `AdapterOutputStream` feeds the SAME sink the connection would hand it, and a
//! single `run_turn(prompt)` drives `run_turn_streaming_with_cancel` so the
//! emitted engine events (`TextDelta` … `TurnEnded`) flow out through that sink.
//!
//! No network and no API key: the orchestrator is wired to a deterministic
//! `MockStreamingApiClient` (one scripted turn: assistant text, then `end_turn`).

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;

use bridge_server::driver::OrchestratorTurnDriver;
use bridge_server::server::TurnDriver;
use client_adapter::{AdapterOutputStream, MockSink};
use client_protocol::events::{ClientEvent, TurnOutcomeDto};
use orchestrator::test_support::{
    content_block_start_text, content_block_stop, message_delta_stop, message_start, message_stop,
    noop_hook_executor, text_delta, MockApiClient, MockStreamingApiClient, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig};

/// Drive ONE streaming turn through the production driver and prove the engine's
/// streamed events reach the captured sink in order: `TextDelta` then `TurnEnded`.
#[tokio::test]
async fn run_turn_streams_text_delta_then_turn_ended_into_sink() {
    // One scripted turn: the assistant emits text, then ends the turn.
    let turn = scripted![
        message_start("m1", "claude-sonnet-4-20250514"),
        content_block_start_text(0),
        text_delta(0, "hello from the engine"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];

    let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![turn]));
    let batched = Arc::new(MockApiClient::new(Vec::new())); // unused on the streaming path

    // The captured sink stands in for the connection's `event_sink()`: the
    // orchestrator's `AdapterOutputStream` lowers every callback into a
    // `ClientEvent` and forwards it here.
    let sink = MockSink::arc();
    let output: Arc<dyn traits::OutputStream> = Arc::new(AdapterOutputStream::new(
        sink.clone() as Arc<dyn client_adapter::ClientEventSink>
    ));

    let tools = Arc::new(tool_api::registry::ToolRegistry::new());

    let orchestrator = Arc::new(ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        streaming,
        tools,
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate) as Arc<dyn permission::gate::PermissionGate>,
        output,
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    ));

    // The PRODUCTION driver under test.
    let driver: Arc<dyn TurnDriver> = Arc::new(OrchestratorTurnDriver::new(orchestrator));

    driver.run_turn("say hi".to_string()).await;

    let events = sink.events().await;

    // The streamed assistant text arrives first.
    let saw_text = events
        .iter()
        .any(|e| matches!(e, ClientEvent::TextDelta { text } if text == "hello from the engine"));
    assert!(
        saw_text,
        "expected a TextDelta with the streamed text, got: {events:?}"
    );

    // The turn ends with `end_turn`.
    let turn_ended_idx = events.iter().position(|e| {
        matches!(
            e,
            ClientEvent::TurnEnded { outcome, stop_reason, .. }
                if *outcome == TurnOutcomeDto::EndTurn
                    && stop_reason.as_deref() == Some("end_turn")
        )
    });
    assert!(
        turn_ended_idx.is_some(),
        "expected a TurnEnded(end_turn), got: {events:?}"
    );

    // Ordering: TextDelta must precede TurnEnded.
    let text_idx = events
        .iter()
        .position(|e| matches!(e, ClientEvent::TextDelta { .. }))
        .expect("a TextDelta was emitted");
    assert!(
        text_idx < turn_ended_idx.unwrap(),
        "TextDelta must stream before TurnEnded, got: {events:?}"
    );
}
