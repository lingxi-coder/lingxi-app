use llm_client::{LlmError, LlmEvent};
use orchestrator::test_support::{
    content_block_start_text, message_start, text_delta, MockApiClient, MockOutputStream,
    MockStreamingApiClient, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{
    ConversationOrchestrator, ConversationOutcome, OrchestratorConfig, OrchestratorError,
};
use std::path::PathBuf;
use std::sync::Arc;
use tool_api::registry::ToolRegistry;

fn orch(
    streaming: Arc<MockStreamingApiClient>,
    output: Arc<MockOutputStream>,
) -> ConversationOrchestrator {
    ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        Arc::new(MockApiClient::new(Vec::new())),
        streaming,
        Arc::new(ToolRegistry::new()),
        orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        output,
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    )
}

/// #10 (faithful port of `query.ts:955-997` catch): a mid-stream model/runtime
/// error (here a `Transport` connection reset) is NOT a hard failure. The turn
/// ends GRACEFULLY with `reason:"model_error"` and the raw error text is surfaced
/// as an api-error assistant message — instead of the prior `OrchestratorError`
/// bubble (which showed a phantom "[Request interrupted by user]" to SDK callers).
#[tokio::test]
async fn mid_stream_err_ends_turn_gracefully_as_model_error() {
    let turn: Vec<Result<LlmEvent, LlmError>> = vec![
        Ok(message_start("m1", "claude-opus-4-7")),
        Ok(content_block_start_text(0)),
        Ok(text_delta(0, "before err")),
        Err(LlmError::Transport {
            message: "connection reset by peer".into(),
        }),
    ];
    let api = Arc::new(MockStreamingApiClient::with_fallible_turns(vec![turn]));
    let output = Arc::new(MockOutputStream::new());
    let orch = orch(api, output.clone());

    let outcome = orch
        .run_turn_streaming("hi")
        .await
        .expect("a model/runtime error must end the turn gracefully, not bubble a hard error");
    assert!(
        matches!(outcome, ConversationOutcome::EndTurn { .. }),
        "{outcome:?}"
    );

    let events = output.snapshot().await;
    // Terminal reason is `model_error`.
    assert!(
        events.iter().any(|e| matches!(
            e,
            traits::OutputEvent::EndTurn { stop_reason, .. } if stop_reason == "model_error"
        )),
        "turn must end with stop_reason model_error; events={events:#?}"
    );
    // The raw error text is surfaced verbatim (createAssistantAPIErrorMessage,
    // NOT an `API Error:`-prefixed template).
    assert!(
        output
            .text_events()
            .await
            .iter()
            .any(|t| t.contains("connection reset by peer")),
        "the raw error text must be surfaced as the model_error message"
    );
}

/// #10 carve-out: a `RateLimited` error must STILL propagate as a hard
/// `OrchestratorError` so the `run_turn*` wrapper can re-map it onto the
/// limits-specific copy + emit the terminal rate-limit snapshot — it must NOT be
/// swallowed as a graceful `model_error`.
#[tokio::test]
async fn rate_limited_still_propagates_as_hard_error() {
    let api = Arc::new(MockStreamingApiClient::with_open_error(
        LlmError::RateLimited {
            retry_after: None,
            scope: None,
        },
        Vec::new(),
    ));
    let output = Arc::new(MockOutputStream::new());
    let orch = orch(api, output);

    let err = orch
        .run_turn_streaming("hi")
        .await
        .expect_err("RateLimited must remain a hard error for wrapper enrichment");
    assert!(
        matches!(
            err,
            OrchestratorError::Streaming(LlmError::RateLimited { .. })
                | OrchestratorError::ApiCall(LlmError::RateLimited { .. })
        ),
        "expected a RateLimited hard error, got {err:?}"
    );
}
