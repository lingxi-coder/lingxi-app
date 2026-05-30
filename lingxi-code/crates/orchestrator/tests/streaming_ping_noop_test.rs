//! `ping` event is a no-op (M5-04 Task 15).

use lingxi_orchestrator::test_support::{
    content_block_start_text, content_block_stop, message_delta_stop, message_start, message_stop,
    ping, text_delta, MockApiClient, MockOutputStream, MockStreamingApiClient, NoOpPermissionGate,
    StaticMemoryProvider,
};
use lingxi_orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig};
use lingxi_tools::registry::ToolRegistry;
use lingxi_traits::OutputEvent;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn ping_between_deltas_does_not_disturb_output() {
    let turn = scripted![
        message_start("m1", "claude-opus-4-7"),
        ping(),
        content_block_start_text(0),
        ping(),
        text_delta(0, "abc"),
        ping(),
        text_delta(0, "def"),
        ping(),
        content_block_stop(0),
        ping(),
        message_delta_stop("end_turn"),
        ping(),
        message_stop(),
    ];

    let api = Arc::new(MockStreamingApiClient::with_turns(vec![turn]));
    let batched = Arc::new(MockApiClient::new(Vec::new()));
    let output = Arc::new(MockOutputStream::new());
    let orch = ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        api,
        Arc::new(ToolRegistry::new()),
        lingxi_orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        output.clone(),
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    );
    orch.run_turn_streaming("hi").await.expect("ok");

    let events = output.snapshot().await;
    // Expect: Text("abc"), Text("def"), EndTurn — pings filtered out.
    assert_eq!(events.len(), 3, "got {events:?}");
    assert!(matches!(&events[0], OutputEvent::Text { text } if text == "abc"));
    assert!(matches!(&events[1], OutputEvent::Text { text } if text == "def"));
    assert!(matches!(&events[2], OutputEvent::EndTurn { .. }));
}
