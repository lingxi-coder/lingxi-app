//! recorded the response's token usage and computed a non-zero cost.
#![allow(clippy::field_reassign_with_default)]

use cost::pricing::PricingCatalog;
use cost::CostTracker;
use llm_client::{LlmResponse, TokenUsage, Usage};
use orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use protocol::SessionId;
use std::sync::Arc;
use tokio::sync::mpsc;
use tool_api::registry::ToolRegistry;

/// Build an `LlmResponse` that emulates a single `end_turn` API reply with
/// the given token usage.
fn end_turn_response_with_usage(input: u64, output: u64) -> LlmResponse {
    LlmResponse {
        id: "msg_mock".to_string(),
        model: "claude-opus-4-6".to_string(),
        content: Vec::new(),
        stop_reason: Some("end_turn".to_string()),
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

/// M7: with an `AnalyticsBus` wired (as the desktop composition root does), a
/// live turn fires `tengu_cost_recorded` per recorded API response — 1:1 with
/// claude-code's `logEvent('tengu_cost_recorded', …)`. Without a bus (every
/// other test here), the tracker accrues totals but emits no analytics event.
#[tokio::test]
async fn run_turn_emits_tengu_cost_recorded_when_bus_attached() {
    use telemetry::{AnalyticsBus, InMemorySink};

    let response = end_turn_response_with_usage(1_000, 500);
    let api = Arc::new(MockApiClient::new(vec![response]));

    let (tx, _rx) = mpsc::channel(8); // _rx held → persist channel stays open
    let tracker = Arc::new(CostTracker::new(
        SessionId::new(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ));

    let bus = Arc::new(AnalyticsBus::new());
    let sink = Arc::new(InMemorySink::new());
    bus.attach_sink(sink.clone()).await;

    let mut cfg = OrchestratorConfig::default();
    cfg.model = "claude-opus-4-6".into(); // priced in builtin_reference

    let orch = Arc::new(
        ConversationOrchestrator::new(
            cfg,
            api,
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
        .with_cost_tracker(tracker)
        .with_analytics_bus(bus),
    );

    orch.run_turn("hi").await.expect("run_turn ok");

    let events = sink.events().await;
    let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"tengu_cost_recorded"),
        "a live turn with a wired bus must fire tengu_cost_recorded; got {names:?}"
    );
}

/// A non-`end_turn` (looping) reply that records the same $0.0175 (17.5M nano)
/// cost per turn.
fn looping_response_with_usage(input: u64, output: u64) -> LlmResponse {
    let mut r = end_turn_response_with_usage(input, output);
    r.stop_reason = Some("max_tokens".to_string()); // not end_turn → loop continues
    r
}

fn budget_orch(
    api: Arc<MockApiClient>,
    tracker: Arc<CostTracker>,
    max_budget_nano_usd: Option<u64>,
) -> ConversationOrchestrator {
    let mut cfg = OrchestratorConfig::default();
    cfg.model = "claude-opus-4-6".into(); // priced in builtin_reference
    cfg.max_budget_nano_usd = max_budget_nano_usd;
    ConversationOrchestrator::new(
        cfg,
        api,
        Arc::new(ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    )
    .with_cost_tracker(tracker)
}

#[tokio::test]
async fn run_turn_stops_at_max_budget() {
    // Turn 1 records $0.0175 (17.5M nano); with a 1M-nano ($0.001) cap, the next
    // iteration's over-budget check stops with MaxBudgetReached — 1:1 with
    // claude-code `getTotalCost() >= maxBudgetUsd`.
    let api = Arc::new(MockApiClient::new(vec![
        looping_response_with_usage(1_000, 500),
        looping_response_with_usage(1_000, 500),
        looping_response_with_usage(1_000, 500),
    ]));
    let (tx, _rx) = mpsc::channel(8); // _rx held for the whole test → channel stays open
    let tracker = Arc::new(CostTracker::new(
        SessionId::new(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ));
    let orch = budget_orch(api, tracker, Some(1_000_000));
    let err = orch.run_turn("hi").await.expect_err("must stop on budget");
    assert!(
        matches!(err, orchestrator::OrchestratorError::MaxBudgetReached { .. }),
        "expected MaxBudgetReached, got {err:?}"
    );
}

#[tokio::test]
async fn no_budget_cap_never_stops_on_budget() {
    // max_budget_nano_usd = None (default) → the cap is inert; the loop runs to
    // queue exhaustion, never MaxBudgetReached.
    let api = Arc::new(MockApiClient::new(vec![
        looping_response_with_usage(1_000, 500),
        looping_response_with_usage(1_000, 500),
    ]));
    let (tx, _rx) = mpsc::channel(8);
    let tracker = Arc::new(CostTracker::new(
        SessionId::new(),
        Arc::new(PricingCatalog::builtin_reference()),
        tx,
    ));
    let orch = budget_orch(api, tracker, None);
    let err = orch.run_turn("hi").await.expect_err("queue exhausts");
    assert!(
        !matches!(err, orchestrator::OrchestratorError::MaxBudgetReached { .. }),
        "no cap must not stop on budget, got {err:?}"
    );
}
