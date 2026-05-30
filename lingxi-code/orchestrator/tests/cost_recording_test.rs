//! M6-06 — after a successful turn, the wired `CostTracker` must have
//! recorded the response's token usage and computed a non-zero cost.
#![allow(clippy::field_reassign_with_default)]

use api_client::types::{MessageResponse, UsageApi};
use cost::pricing::PricingCatalog;
use cost::CostTracker;
use orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use protocol::SessionId;
use std::sync::Arc;
use tokio::sync::mpsc;
use tool_api::registry::ToolRegistry;

/// Build a `MessageResponse` that emulates a single `end_turn` API reply with
/// the given token usage.
fn end_turn_response_with_usage(input: u64, output: u64) -> MessageResponse {
    MessageResponse {
        id: "msg_mock".to_string(),
        model: "claude-opus-4-6".to_string(),
        content: Vec::new(),
        stop_reason: Some("end_turn".to_string()),
        usage: UsageApi {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        },
    }
}

#[tokio::test]
async fn turn_with_known_tokens_records_real_cost() {
    let response = end_turn_response_with_usage(1_000, 500);
    let api = Arc::new(MockApiClient::new(vec![response]));

    let (tx, mut rx) = mpsc::channel(8);
    let tracker = Arc::new(CostTracker::new(
        SessionId::new(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ));

    let tools = Arc::new(ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());

    let mut cfg = OrchestratorConfig::default();
    cfg.model = "claude-opus-4-6".into(); // priced in builtin_reference catalog

    let orch = Arc::new(
        ConversationOrchestrator::new(
            cfg,
            api,
            tools,
            hooks,
            perms,
            output,
            memory,
            std::env::temp_dir(),
        )
        .with_cost_tracker(tracker.clone()),
    );

    orch.run_turn("hi").await.expect("run_turn ok");

    // Drain the persist channel (CostTracker pushes a snapshot per record).
    let snap = rx.recv().await.expect("persist_tx received a snapshot");
    // 1000*5000 + 500*25000 = 17_500_000 nano-USD = $0.0175 (matches the
    // M3-05 tracker test).
    assert_eq!(snap.total_nano_usd, 17_500_000);
}
