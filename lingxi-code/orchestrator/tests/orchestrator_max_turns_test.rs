use llm_client::ContentBlock as LlmContentBlock;
use orchestrator::test_support::{
    mock_message_response, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{
    ConversationOrchestrator, ConversationOutcome, OrchestratorConfig, OrchestratorError,
};
use std::sync::Arc;
use tool_api::registry::ToolRegistry;

#[tokio::test]
async fn never_ending_loop_aborts_with_max_turns_reached() {
    let make_resp = || {
        mock_message_response(
            vec![LlmContentBlock::Text {
                text: "still thinking".into(),
                cache_control: None,
            }],
            Some("max_tokens"), // any value other than "end_turn"
        )
    };
    let api = Arc::new(MockApiClient::new(vec![
        make_resp(),
        make_resp(),
        make_resp(),
        make_resp(),
        make_resp(),
    ]));
    let output = Arc::new(MockOutputStream::new());
    let hooks = orchestrator::test_support::noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let tools = Arc::new(ToolRegistry::new());

    let config = OrchestratorConfig {
        max_turns: 3,
        ..OrchestratorConfig::default()
    };

    let orch = ConversationOrchestrator::new(
        config,
        api.clone(),
        tools,
        hooks,
        perms,
        output.clone(),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );

    let err = orch.run_turn("forever").await.expect_err("must fail");
    assert!(matches!(
        err,
        OrchestratorError::MaxTurnsReached { max_turns: 3 }
    ));
    assert_eq!(err.to_string(), "Reached maximum number of turns (3)");

    // API was called exactly max_turns (= 3) times.
    assert_eq!(api.captured_msgs().await.len(), 3);
}

#[tokio::test]
async fn max_turns_default_30_is_the_construction_default() {
    let api = Arc::new(MockApiClient::new(vec![mock_message_response(
        vec![],
        Some("end_turn"),
    )]));
    let output = Arc::new(MockOutputStream::new());
    let hooks = orchestrator::test_support::noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let tools = Arc::new(ToolRegistry::new());

    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        tools,
        hooks,
        perms,
        output,
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );

    let outcome = orch.run_turn("ping").await.expect("happy");
    assert!(matches!(
        outcome,
        ConversationOutcome::EndTurn { turn_count: 1, .. }
    ));
}

#[tokio::test]
async fn api_error_propagates_as_orchestrator_error_api_call() {
    // Empty mock → first call returns ApiError::Server (mock-exhaustion shape).
    let api = Arc::new(MockApiClient::new(vec![]));
    let output = Arc::new(MockOutputStream::new());
    let hooks = orchestrator::test_support::noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let tools = Arc::new(ToolRegistry::new());

    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        tools,
        hooks,
        perms,
        output,
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );

    let err = orch.run_turn("anything").await.expect_err("must fail");
    let msg = err.to_string();
    assert!(msg.starts_with("api call failed: "), "got: {msg}");
}
