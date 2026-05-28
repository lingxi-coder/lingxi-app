//! M6-06 — `snapshot_cost` reads real numbers from the wired CostTracker.

use lingxi_api_client::types::{MessageResponse, UsageApi};
use lingxi_cost::pricing::PricingCatalog;
use lingxi_cost::CostTracker;
use lingxi_orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use lingxi_orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use lingxi_protocol::SessionId;
use lingxi_tools::registry::ToolRegistry;
use lingxi_traits::OrchestratorHandle;
use std::sync::Arc;
use tokio::sync::mpsc;

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

/// Construct an orchestrator pre-loaded with N priced responses (each
/// `input=1000, output=500`) and a fresh CostTracker.
async fn make_orch_with_n_responses(
    n: usize,
) -> (
    Arc<ConversationOrchestrator>,
    mpsc::Receiver<lingxi_cost::CostState>,
) {
    let responses: Vec<_> = (0..n).map(|_| end_turn_response_with_usage(1_000, 500)).collect();
    let api = Arc::new(MockApiClient::new(responses));

    let (tx, rx) = mpsc::channel(64);
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
    cfg.model = "claude-opus-4-6".into();

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
        .with_cost_tracker(tracker),
    );
    (orch, rx)
}

#[tokio::test]
async fn zero_turn_snapshot_is_zeros() {
    let (orch, _rx) = make_orch_with_n_responses(0).await;
    let snap = orch.snapshot_cost().await;
    assert_eq!(snap.total_usd, 0.0);
    assert_eq!(snap.input_tokens, 0);
    assert_eq!(snap.output_tokens, 0);
    assert_eq!(snap.api_calls, 0);
}

#[tokio::test]
async fn one_turn_snapshot_is_non_zero() {
    let (orch, _rx) = make_orch_with_n_responses(1).await;
    orch.run_turn("hi").await.unwrap();
    let snap = orch.snapshot_cost().await;

    // 17_500_000 nano-USD = $0.0175
    assert!(
        (snap.total_usd - 0.0175).abs() < 1e-9,
        "expected total_usd ≈ 0.0175, got {}",
        snap.total_usd
    );
    assert_eq!(snap.input_tokens, 1_000);
    assert_eq!(snap.output_tokens, 500);
    assert_eq!(snap.api_calls, 1);
    assert_eq!(snap.total_tokens, 1_500);
}

#[tokio::test]
async fn two_turns_accumulate() {
    let (orch, _rx) = make_orch_with_n_responses(2).await;
    orch.run_turn("hi").await.unwrap();
    orch.run_turn("hi again").await.unwrap();
    let snap = orch.snapshot_cost().await;

    assert_eq!(snap.input_tokens, 2_000);
    assert_eq!(snap.output_tokens, 1_000);
    assert_eq!(snap.api_calls, 2);
    assert!((snap.total_usd - 0.0350).abs() < 1e-9);
}

#[tokio::test]
async fn no_tracker_returns_zeroed_snapshot() {
    // Backward compat — library users who don't wire a tracker still get
    // a valid (zero) snapshot with the correct session_id.
    let api = Arc::new(MockApiClient::new(Vec::new()));
    let tools = Arc::new(ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());
    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        tools,
        hooks,
        perms,
        output,
        memory,
        std::env::temp_dir(),
    );
    let snap = orch.snapshot_cost().await;
    assert_eq!(snap.total_usd, 0.0);
    assert_eq!(snap.input_tokens, 0);
    assert_eq!(snap.api_calls, 0);
    assert_eq!(snap.session_id, orch.current_session_id().await);
}
