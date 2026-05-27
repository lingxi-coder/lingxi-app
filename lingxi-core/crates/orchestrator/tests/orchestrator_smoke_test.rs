//! M5-02 Task 9: smoke test that drives `ConversationOrchestrator::run_turn`
//! happy path — single turn, no tools.

use lingxi_api_client::types::ContentBlockApi;
use lingxi_orchestrator::test_support::{
    mock_message_response, MockApiClient, MockOutputStream, NoOpHookExecutor, NoOpPermissionGate,
};
use lingxi_orchestrator::{ConversationOrchestrator, ConversationOutcome, OrchestratorConfig};
use lingxi_traits::OutputEvent;
use std::sync::Arc;

#[tokio::test]
async fn single_turn_no_tools_returns_end_turn_and_emits_text_and_end_turn() {
    let response = mock_message_response(
        vec![ContentBlockApi::Text {
            text: "hello world".into(),
        }],
        Some("end_turn"),
    );
    let api = Arc::new(MockApiClient::new(vec![response]));
    let output = Arc::new(MockOutputStream::new());
    let hooks = Arc::new(NoOpHookExecutor);
    let perms = Arc::new(NoOpPermissionGate);
    let tools = Arc::new(lingxi_tools::registry::ToolRegistry::new());

    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api.clone(),
        tools,
        hooks,
        perms,
        output.clone(),
    );

    let outcome = orch
        .run_turn("say hello")
        .await
        .expect("turn must succeed");

    // ConversationOutcome is #[non_exhaustive] — wildcard arm satisfies the
    // checker even though EndTurn is the only variant in M5-02.
    match outcome {
        ConversationOutcome::EndTurn { turn_count, .. } => assert_eq!(turn_count, 1),
        _ => panic!("unexpected ConversationOutcome variant"),
    }

    let events = output.snapshot().await;
    assert_eq!(events.len(), 2, "expected Text + EndTurn; got {events:?}");
    assert!(matches!(events[0], OutputEvent::Text { ref text } if text == "hello world"));
    assert!(matches!(events[1], OutputEvent::EndTurn { ref stop_reason, .. } if stop_reason == "end_turn"));

    // Mock observed exactly one API call.
    assert_eq!(api.captured_msgs().await.len(), 1);
}
