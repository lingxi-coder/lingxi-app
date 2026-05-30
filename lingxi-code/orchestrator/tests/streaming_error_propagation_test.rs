//! Mid-stream `Err` propagates as [`OrchestratorError::Streaming`] (M5-04 Task 17).

use lingxi_api_client::ApiError;
use lingxi_orchestrator::test_support::{
    content_block_start_text, message_start, text_delta, MockApiClient, MockOutputStream,
    MockStreamingApiClient, NoOpPermissionGate, StaticMemoryProvider,
};
use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig, OrchestratorError};
use lingxi_tools::registry::ToolRegistry;
use lingxi_traits::HttpError;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn mid_stream_err_surfaces_as_streaming_variant() {
    // First three events OK, fourth event is an Err.
    let turn: Vec<Result<lingxi_api_client::types::StreamEvent, ApiError>> = vec![
        Ok(message_start("m1", "claude-opus-4-7")),
        Ok(content_block_start_text(0)),
        Ok(text_delta(0, "before err")),
        Err(ApiError::Http(HttpError::Connection(
            "connection reset by peer".into(),
        ))),
    ];

    let api = Arc::new(MockStreamingApiClient::with_fallible_turns(vec![turn]));
    let batched = Arc::new(MockApiClient::new(Vec::new()));
    let output = Arc::new(MockOutputStream::new());
    let orch = ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        api,
        Arc::new(ToolRegistry::new()),
        lingxi_orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        output,
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    );

    let err = orch
        .run_turn_streaming("hi")
        .await
        .expect_err("network err");
    match err {
        OrchestratorError::Streaming(inner) => {
            let s = format!("{inner}");
            assert!(s.contains("connection reset"), "{s}");
        }
        other => panic!("expected Streaming variant, got {other:?}"),
    }
}
