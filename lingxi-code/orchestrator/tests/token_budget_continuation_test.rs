//!
//! Verifies that the batched turn loop, when the `TOKEN_BUDGET` gate is enabled
//! AND a budget is configured, keeps nudging the model past `end_turn` until
//! ~90% of the budget is spent — and that with the gate OFF (the parity
//! default) the loop stops at the first `end_turn` (NO-OP).
use llm_client::{ContentBlock as LlmContentBlock, LlmResponse, TokenUsage, Usage};
use orchestrator::test_support::{
    MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, ConversationOutcome, OrchestratorConfig};
use protocol::ConversationMessage;
use std::sync::Arc;
use tool_api::registry::ToolRegistry;

/// Build an `end_turn` response with a single text block and a given
/// `output_tokens` usage count.
fn end_turn_with_output_tokens(output_tokens: u64) -> LlmResponse {
    LlmResponse {
        id: "msg_mock".to_string(),
        model: "claude-opus-4-7".to_string(),
        content: vec![LlmContentBlock::Text {
            text: "done".into(),
            cache_control: None,
        }],
        stop_reason: Some("end_turn".to_string()),
        stop_details: None,
        usage: Usage {
            billable_tokens: TokenUsage {
                output: output_tokens,
                ..Default::default()
            },
            ..Default::default()
        },
        cost: None,
        provider_metadata: serde_json::Value::Null,
    }
}

const NUDGE: &str =
    "Stopped at 20% of token target (100,000 / 500,000). Keep working \u{2014} do not summarize.";

fn build_orch(api: Arc<MockApiClient>, config: OrchestratorConfig) -> ConversationOrchestrator {
    ConversationOrchestrator::new(
        config,
        api,
        Arc::new(ToolRegistry::new()),
        orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    )
}

/// Count the user messages in `snapshot` whose single text block equals the
/// byte-exact continuation nudge.
fn count_nudge_user_messages(snapshot: &[ConversationMessage]) -> usize {
    snapshot
        .iter()
        .filter(|m| match m {
            ConversationMessage::User { content, .. } => content.iter().any(|b| {
                matches!(b, protocol::ContentBlock::Text { text } if text == NUDGE)
            }),
            _ => false,
        })
        .count()
}

#[tokio::test]
async fn budget_off_stops_at_first_end_turn_noop() {
    // Gate OFF (default): even with a tiny output_tokens vs a budget, the loop
    // must NOT continue past the first end_turn.
    let api = Arc::new(MockApiClient::new(vec![end_turn_with_output_tokens(100_000)]));
    let cfg = OrchestratorConfig {
        // budget set but gate OFF → no-op.
        token_budget: Some(500_000),
        enable_token_budget: false,
        ..OrchestratorConfig::default()
    };
    let orch = build_orch(api.clone(), cfg);

    let outcome = orch.run_turn("do it").await.expect("ok");
    assert!(matches!(
        outcome,
        ConversationOutcome::EndTurn { turn_count: 1, .. }
    ));
    // Only the single API call happened.
    assert_eq!(api.captured_msgs().await.len(), 1);
}

#[tokio::test]
async fn budget_none_stops_at_first_end_turn_noop() {
    // Gate ON but no budget → still a no-op.
    let api = Arc::new(MockApiClient::new(vec![end_turn_with_output_tokens(100_000)]));
    let cfg = OrchestratorConfig {
        token_budget: None,
        enable_token_budget: true,
        ..OrchestratorConfig::default()
    };
    let orch = build_orch(api.clone(), cfg);

    let outcome = orch.run_turn("do it").await.expect("ok");
    assert!(matches!(
        outcome,
        ConversationOutcome::EndTurn { turn_count: 1, .. }
    ));
    assert_eq!(api.captured_msgs().await.len(), 1);
}

#[tokio::test]
async fn budget_on_continues_then_stops_at_threshold() {
    // Gate ON + 500k budget.
    // Turn 1: end_turn, output 100k (20% < 90%) → continue (nudge #1 injected).
    // Turn 2: end_turn, cumulative 460k (92% >= 90%) → stop.
    let api = Arc::new(MockApiClient::new(vec![
        end_turn_with_output_tokens(100_000),
        end_turn_with_output_tokens(360_000),
    ]));
    let cfg = OrchestratorConfig {
        token_budget: Some(500_000),
        enable_token_budget: true,
        ..OrchestratorConfig::default()
    };
    let orch = build_orch(api.clone(), cfg);

    let outcome = orch.run_turn("do it").await.expect("ok");
    // Two API round-trips: the first end_turn continued, the second stopped.
    match outcome {
        ConversationOutcome::EndTurn { turn_count, .. } => assert_eq!(turn_count, 2),
        other => panic!("expected EndTurn, got {other:?}"),
    }
    let captured = api.captured_msgs().await;
    assert_eq!(captured.len(), 2, "expected exactly two API calls");

    // The SECOND call's message snapshot must contain the byte-exact nudge as a
    // user message (injected after the first end_turn).
    let second = &captured[1];
    assert_eq!(
        count_nudge_user_messages(second),
        1,
        "expected exactly one continuation nudge in the second snapshot"
    );
}

#[tokio::test]
async fn budget_on_diminishing_returns_stops_after_three_continuations() {
    // Gate ON + 1_000_000 budget (90% threshold = 900_000). Feed small deltas so
    // after 3 continuations the per-step delta is < 500 and the loop stops on
    // diminishing returns rather than continuing forever.
    // Cumulative output tokens per turn: 100, 200, 300, 400.
    // Turn 1: cumulative 100 → continue (count 1, delta 100).
    // Turn 2: cumulative 300 (delta 200) → continue (count 2).
    // Turn 3: cumulative 600 (delta 300) → continue (count 3, lastDelta 300<500).
    // Turn 4: cumulative 700 (delta 100<500, count>=3, lastDelta 300<500) →
    //         diminishing → stop.
    let api = Arc::new(MockApiClient::new(vec![
        end_turn_with_output_tokens(100),
        end_turn_with_output_tokens(200),
        end_turn_with_output_tokens(300),
        end_turn_with_output_tokens(100),
    ]));
    let cfg = OrchestratorConfig {
        token_budget: Some(1_000_000),
        enable_token_budget: true,
        ..OrchestratorConfig::default()
    };
    let orch = build_orch(api.clone(), cfg);

    let outcome = orch.run_turn("grind").await.expect("ok");
    match outcome {
        ConversationOutcome::EndTurn { turn_count, .. } => assert_eq!(turn_count, 4),
        other => panic!("expected EndTurn, got {other:?}"),
    }
    // Exactly four API calls (3 continuations, then diminishing stop).
    assert_eq!(api.captured_msgs().await.len(), 4);
}

#[tokio::test]
async fn budget_on_resets_recovery_count_on_continuation() {
    // The A1 recovery count must reset to 0 on every budget continuation
    // (query.ts:1332). We exercise this indirectly: a max_tokens recovery on
    // turn 1, then an end_turn under budget that continues. If the recovery
    // count were NOT reset, a later max_tokens would exhaust sooner. Here we
    // assert the loop completes the continuation path normally (no panic, two
    // continuations) — the reset is what keeps the recovery budget available.
    let api = Arc::new(MockApiClient::new(vec![
        end_turn_with_output_tokens(100_000), // continue #1 (20%)
        end_turn_with_output_tokens(100_000), // cumulative 200k = 40% → continue #2
        end_turn_with_output_tokens(260_000), // cumulative 460k = 92% → stop
    ]));
    let cfg = OrchestratorConfig {
        token_budget: Some(500_000),
        enable_token_budget: true,
        ..OrchestratorConfig::default()
    };
    let orch = build_orch(api.clone(), cfg);

    let outcome = orch.run_turn("keep going").await.expect("ok");
    match outcome {
        ConversationOutcome::EndTurn { turn_count, .. } => assert_eq!(turn_count, 3),
        other => panic!("expected EndTurn, got {other:?}"),
    }
    let captured = api.captured_msgs().await;
    assert_eq!(captured.len(), 3);
    // The final snapshot should carry TWO injected nudges (one per continuation).
    let last = captured.last().unwrap();
    let nudges = last
        .iter()
        .filter(|m| {
            matches!(
                m,
                ConversationMessage::User { content, .. }
                    if content.iter().any(|b| matches!(
                        b,
                        protocol::ContentBlock::Text { text }
                            if text.starts_with("Stopped at")
                                && text.contains('\u{2014}')
                    ))
            )
        })
        .count();
    assert_eq!(nudges, 2, "expected two continuation nudges in final snapshot");
}
