//! Task 8 (llm-client future-work batch 3): the orchestrator forwards the
//! adapter's unified rate-limit header snapshot to
//! `OutputStream::emit_rate_limit` after each completed API call, emitting
//! ONLY when the snapshot differs from the last emitted value
//! (emit-on-change dedup).
//!
//! Mirrors the turn-driving pattern of `orchestrator_multi_turn_test.rs`:
//! `MockApiClient` (extended with `set_rate_limit_full`) + the recording
//! `MockOutputStream`.

use llm_client::ContentBlock as LlmContentBlock;
use orchestrator::model::rate_limit::RateLimitInfo;
use orchestrator::test_support::{
    mock_message_response, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use std::sync::Arc;
use tool_api::registry::ToolRegistry;
use traits::OutputEvent;

/// A single-text `end_turn` response so each `run_turn` is exactly one API call.
fn end_turn_response(text: &str) -> llm_client::LlmResponse {
    mock_message_response(
        vec![LlmContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
        Some("end_turn"),
    )
}

/// A fully-populated internal snapshot (all nine post-T7 fields).
fn sample_info() -> RateLimitInfo {
    RateLimitInfo {
        rate_limit_type: Some("five_hour".into()),
        overage_status: Some("allowed".into()),
        overage_disabled_reason: Some("out_of_credits".into()),
        status: Some("allowed_warning".into()),
        resets_at: Some(1_760_000_000),
        utilization: Some(0.85),
        claim_resets_at: Some(1_760_000_100),
        overage_resets_at: Some(1_760_000_200),
        fallback_available: Some(true),
    }
}

fn build_orch(
    api: Arc<MockApiClient>,
    output: Arc<MockOutputStream>,
) -> ConversationOrchestrator {
    ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        Arc::new(ToolRegistry::new()),
        orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        output,
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    )
}

/// Project the captured `RateLimit` events out of the full event stream.
fn rate_limit_events(events: &[OutputEvent]) -> Vec<OutputEvent> {
    events
        .iter()
        .filter(|e| matches!(e, OutputEvent::RateLimit { .. }))
        .cloned()
        .collect()
}

/// (a) First `Some` snapshot after a completed API call → exactly one
/// `RateLimit` event, with every field mapped 1:1 from the internal struct.
#[tokio::test]
async fn emits_rate_limit_on_first_snapshot() {
    let api = Arc::new(MockApiClient::new(vec![end_turn_response("hi")]));
    api.set_rate_limit_full(Some(sample_info()));
    let output = Arc::new(MockOutputStream::new());
    let orch = build_orch(api.clone(), output.clone());

    orch.run_turn("hello").await.expect("turn 1");

    let events = rate_limit_events(&output.snapshot().await);
    assert_eq!(events.len(), 1, "exactly one RateLimit event: {events:?}");
    let OutputEvent::RateLimit {
        status,
        rate_limit_type,
        utilization,
        resets_at,
        claim_resets_at,
        overage_status,
        overage_resets_at,
        overage_disabled_reason,
        fallback_available,
    } = &events[0]
    else {
        panic!("expected RateLimit event");
    };
    assert_eq!(status.as_deref(), Some("allowed_warning"));
    assert_eq!(rate_limit_type.as_deref(), Some("five_hour"));
    assert_eq!(*utilization, Some(0.85));
    assert_eq!(*resets_at, Some(1_760_000_000));
    assert_eq!(*claim_resets_at, Some(1_760_000_100));
    assert_eq!(overage_status.as_deref(), Some("allowed"));
    assert_eq!(*overage_resets_at, Some(1_760_000_200));
    assert_eq!(overage_disabled_reason.as_deref(), Some("out_of_credits"));
    assert_eq!(*fallback_available, Some(true));
}

/// (b) An IDENTICAL snapshot across two turns must NOT be re-emitted —
/// emit-on-change means the second turn produces no second event.
#[tokio::test]
async fn no_duplicate_emit_for_identical_snapshot_across_turns() {
    let api = Arc::new(MockApiClient::new(vec![
        end_turn_response("one"),
        end_turn_response("two"),
    ]));
    api.set_rate_limit_full(Some(sample_info()));
    let output = Arc::new(MockOutputStream::new());
    let orch = build_orch(api.clone(), output.clone());

    orch.run_turn("first").await.expect("turn 1");
    orch.run_turn("second").await.expect("turn 2");

    let events = rate_limit_events(&output.snapshot().await);
    assert_eq!(
        events.len(),
        1,
        "identical snapshot must emit exactly once: {events:?}"
    );
}

/// (c) A CHANGED snapshot re-emits: two turns with different snapshots →
/// two events, the second carrying the new values.
#[tokio::test]
async fn re_emits_when_snapshot_changes() {
    let api = Arc::new(MockApiClient::new(vec![
        end_turn_response("one"),
        end_turn_response("two"),
    ]));
    api.set_rate_limit_full(Some(sample_info()));
    let output = Arc::new(MockOutputStream::new());
    let orch = build_orch(api.clone(), output.clone());

    orch.run_turn("first").await.expect("turn 1");

    let changed = RateLimitInfo {
        status: Some("rejected".into()),
        utilization: Some(1.0),
        ..sample_info()
    };
    api.set_rate_limit_full(Some(changed));
    orch.run_turn("second").await.expect("turn 2");

    let events = rate_limit_events(&output.snapshot().await);
    assert_eq!(events.len(), 2, "changed snapshot must re-emit: {events:?}");
    let OutputEvent::RateLimit {
        status,
        utilization,
        rate_limit_type,
        ..
    } = &events[1]
    else {
        panic!("expected RateLimit event");
    };
    assert_eq!(status.as_deref(), Some("rejected"));
    assert_eq!(*utilization, Some(1.0));
    assert_eq!(rate_limit_type.as_deref(), Some("five_hour"));
}

/// (Task 9 bonus) The STREAMING seam in `try_run_turn_streaming` also
/// forwards the snapshot: `run_turn_streaming` over a scripted SSE stream →
/// exactly one `RateLimit` event. (`emit_rate_limit_if_changed` reads
/// `self.api`, which is the same `ProviderApiAdapter` as `streaming_api` in
/// production — here the batched mock carries the snapshot.)
#[tokio::test]
async fn streaming_turn_emits_rate_limit() {
    use orchestrator::test_support_stream::{
        content_block_start_text, content_block_stop, message_delta_stop, message_start,
        message_stop, text_delta, MockStreamingApiClient,
    };

    let stream = orchestrator::scripted![
        message_start("msg_01", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "hi"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];
    let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![stream]));
    let batched = Arc::new(MockApiClient::new(Vec::new()));
    batched.set_rate_limit_full(Some(sample_info()));
    let output = Arc::new(MockOutputStream::new());
    let orch = ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        streaming,
        Arc::new(ToolRegistry::new()),
        orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        output.clone(),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );

    orch.run_turn_streaming("hello").await.expect("turn 1");

    let events = rate_limit_events(&output.snapshot().await);
    assert_eq!(
        events.len(),
        1,
        "streaming seam must emit exactly once: {events:?}"
    );
    assert!(matches!(
        &events[0],
        OutputEvent::RateLimit { status, .. } if status.as_deref() == Some("allowed_warning")
    ));
}

/// No snapshot (the default `last_rate_limit_full() == None`, e.g. a
/// provider that never sent unified headers) → no `RateLimit` event at all.
#[tokio::test]
async fn no_emit_when_no_snapshot() {
    let api = Arc::new(MockApiClient::new(vec![end_turn_response("hi")]));
    let output = Arc::new(MockOutputStream::new());
    let orch = build_orch(api.clone(), output.clone());

    orch.run_turn("hello").await.expect("turn 1");

    let events = rate_limit_events(&output.snapshot().await);
    assert!(events.is_empty(), "no snapshot → no emit: {events:?}");
}
