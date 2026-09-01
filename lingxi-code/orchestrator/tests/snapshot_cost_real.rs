#![allow(
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::unused_async
)]

use cost::pricing::PricingCatalog;
use cost::CostTracker;
use llm_client::{ContentBlock, LlmResponse, TokenUsage, Usage};
use orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use protocol::SessionId;
use std::sync::Arc;
use tokio::sync::mpsc;
use tool_api::registry::ToolRegistry;
use platform_api::OrchestratorHandle;

fn end_turn_response_with_usage(input: u64, output: u64) -> LlmResponse {
    LlmResponse {
        id: "msg_mock".to_string(),
        model: "claude-opus-4-6".to_string(),
        // Visible text: an empty-content end_turn trips the #78 thinking-only
        // nudge, which would request another turn the single-response mock can't
        // serve.
        content: vec![ContentBlock::Text {
            text: "Done.".to_string(),
            cache_control: None,
        }],
        stop_reason: Some("end_turn".to_string()),
        stop_details: None,
        usage: Usage {
            billable_tokens: TokenUsage {
                input,
                output,
                ..Default::default()
            },
            ..Default::default()
        },
        cost: None,
        provider_metadata: serde_json::Value::Null,
    }
}

/// Construct an orchestrator pre-loaded with N priced responses (each
/// `input=1000, output=500`) and a fresh `CostTracker`.
async fn make_orch_with_n_responses(
    n: usize,
) -> (
    Arc<ConversationOrchestrator>,
    mpsc::Receiver<cost::CostState>,
) {
    let responses: Vec<_> = (0..n)
        .map(|_| end_turn_response_with_usage(1_000, 500))
        .collect();
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
async fn clear_session_resets_cost_snapshot() {
    // parity 2.1.212: claude-code's clearConversation calls resetCostState, so
    // `/clear` must zero the session cost counter instead of carrying the prior
    // conversation's accumulated total into the fresh session.
    let (orch, _rx) = make_orch_with_n_responses(1).await;
    orch.run_turn("hi").await.unwrap();
    let before = orch.snapshot_cost().await;
    assert!(before.total_nano_usd > 0, "turn accrued cost");
    assert_eq!(before.api_calls, 1);

    <ConversationOrchestrator as OrchestratorHandle>::clear_session(&*orch)
        .await
        .expect("clear session");

    let after = orch.snapshot_cost().await;
    assert_eq!(after.total_nano_usd, 0, "cost total reset by /clear");
    assert_eq!(after.total_usd, 0.0);
    assert_eq!(after.input_tokens, 0);
    assert_eq!(after.output_tokens, 0);
    assert_eq!(after.api_calls, 0, "api-call counter reset by /clear");
    assert!(after.by_model.is_empty(), "per-model breakdown cleared");
}

#[tokio::test]
async fn emit_end_turn_carries_real_cost() {
    // Build a fresh wiring where we can inspect the captured events.
    let response = end_turn_response_with_usage(1_000, 500);
    let api = Arc::new(MockApiClient::new(vec![response]));

    let (tx, _rx) = mpsc::channel(64);
    let tracker = Arc::new(CostTracker::new(
        SessionId::new(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ));

    let tools = Arc::new(ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output_capture = Arc::new(MockOutputStream::new());
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
            output_capture.clone(),
            memory,
            std::env::temp_dir(),
        )
        .with_cost_tracker(tracker),
    );
    orch.run_turn("hi").await.unwrap();

    let events = output_capture.snapshot().await;
    let end_turn_cost = events
        .iter()
        .find_map(|e| match e {
            platform_api::OutputEvent::EndTurn { cost, .. } => Some(cost.clone()),
            _ => None,
        })
        .expect("end_turn event present");

    // The cost embedded in emit_end_turn must reflect the real numbers,
    // not the legacy zeros from cost_snapshot_from_session.
    assert!(
        (end_turn_cost.total_usd - 0.0175).abs() < 1e-9,
        "got: {}",
        end_turn_cost.total_usd
    );
    assert_eq!(end_turn_cost.input_tokens, 1_000);
    assert_eq!(end_turn_cost.output_tokens, 500);
    assert_eq!(end_turn_cost.api_calls, 1);
}

#[tokio::test]
async fn restore_session_cost_seeds_the_snapshot_total() {
    // Resume parity: restoring a prior session's cost makes it visible in the
    // very next snapshot, before any new turn runs (the footer shows the
    // running total instead of $0 after `--resume`).
    let (orch, _rx) = make_orch_with_n_responses(0).await;
    orch.restore_session_cost(17_500_000).await; // $0.0175 from the prior run
    let snap = orch.snapshot_cost().await;
    assert!(
        (snap.total_usd - 0.0175).abs() < 1e-9,
        "restored total_usd should be 0.0175, got {}",
        snap.total_usd
    );
}

#[tokio::test]
async fn restore_then_run_turn_accumulates_on_top() {
    // A restored session keeps accumulating: the next turn adds to the restored
    // base, not to zero.
    let (orch, _rx) = make_orch_with_n_responses(1).await;
    orch.restore_session_cost(17_500_000).await; // prior $0.0175
    orch.run_turn("hi").await.unwrap(); // + this turn's $0.0175
    let snap = orch.snapshot_cost().await;
    assert!(
        (snap.total_usd - 0.0350).abs() < 1e-9,
        "restored + new turn should be 0.0350, got {}",
        snap.total_usd
    );
}

#[tokio::test]
async fn restore_session_cost_without_a_tracker_is_a_noop() {
    // Library users who never wire a tracker: restore must not panic and the
    // snapshot stays zero.
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
    orch.restore_session_cost(17_500_000).await; // no tracker → silently ignored
    assert_eq!(orch.snapshot_cost().await.total_usd, 0.0);
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
