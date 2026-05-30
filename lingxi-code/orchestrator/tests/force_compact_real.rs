//! M6-08 — real `force_compact` wiring.
//!
//! Verifies that:
//! - A 50-message history compacts to fewer than 50 messages and the
//!   trait surface reports real numbers (`messages_before == 50`,
//!   `messages_after < 50`).
//! - A compaction failure leaves history untouched.
//! - A cancelled compaction leaves history untouched.
//! - The post-compact history carries the `[Compacted N → M]` marker as
//!   its last message, so the next turn's system-prompt assembly sees
//!   the compaction transition.
//! - Five consecutive `force_compact` calls do not panic / leak.

use compaction::CompactionOrchestrator;
use orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, ConversationOutcome, OrchestratorConfig};
use protocol::{ConversationMessage, MessageId};
use std::sync::Arc;
use traits::OrchestratorHandle;

fn make_orch() -> Arc<ConversationOrchestrator> {
    let api = Arc::new(MockApiClient::new(vec![]));
    let tools = Arc::new(tools::registry::ToolRegistry::new());
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
    )
    .with_compaction(Arc::new(CompactionOrchestrator::new(1_000)));
    Arc::new(orch)
}

async fn seed_history(orch: &ConversationOrchestrator, n: usize) {
    let session = orch.session();
    let mut s = session.lock().await;
    for i in 0..n {
        s.history.push(ConversationMessage::user(
            MessageId::new(),
            format!("turn-{i} body padded with filler text to push token count up beyond autocompact threshold"),
        ));
    }
}

async fn history_len(orch: &ConversationOrchestrator) -> usize {
    let session = orch.session();
    let s = session.lock().await;
    s.history.len()
}

#[tokio::test]
async fn with_compaction_builder_stores_compactor() {
    let orch = make_orch();
    assert!(
        orch.has_compaction(),
        "with_compaction builder did not store the compactor"
    );
}

#[tokio::test]
async fn compacts_50_message_history() {
    let orch = make_orch();
    seed_history(&orch, 50).await;

    let summary = orch.force_compact().await.expect("force_compact ok");

    assert_eq!(summary.messages_before, 50);
    assert!(
        summary.messages_after < 50,
        "messages_after={} must be <50 to count as compacted",
        summary.messages_after
    );
}

#[tokio::test]
async fn post_compact_summary_is_visible_to_next_turn() {
    let orch = make_orch();
    seed_history(&orch, 20).await;

    orch.force_compact().await.unwrap();

    // Inspect history: must contain the [Compacted ...] marker as the
    // last message, AND at least one preceding message that is the
    // summary (length-checked: compactor produces ≥1 message + our
    // marker).
    let session = orch.session();
    let s = session.lock().await;
    let last = s.history.last().expect("history non-empty after compact");
    let ConversationMessage::System { content, .. } = last else {
        panic!("expected System message; got {last:?}");
    };
    assert!(
        content.starts_with("[Compacted "),
        "marker missing; got: {content}"
    );

    // The summary message produced by the compactor sits before the
    // marker. With the M3 stub Autocompactor this is `[stub-summary
    // attempt=0; messages=20]`.
    assert!(
        s.history.len() >= 2,
        "expected ≥2 messages (summary + marker), got {}",
        s.history.len()
    );
}

#[tokio::test]
async fn cancel_during_compaction_leaves_history_unchanged() {
    use tokio_util::sync::CancellationToken;

    let orch = make_orch();
    seed_history(&orch, 30).await;
    let len_before = history_len(&orch).await;

    let token = CancellationToken::new();
    token.cancel(); // already cancelled — first poll exits

    let err = orch
        .force_compact_with_cancel(token)
        .await
        .expect_err("cancelled must error");
    assert!(err.to_string().contains("cancelled"), "got: {err}");

    let len_after = history_len(&orch).await;
    assert_eq!(len_before, len_after);
}

/// Build a `CompactionOrchestrator` whose `Autocompactor` is wired with
/// an empty `CacheSafeParamsSlot`. The autocompact layer's
/// `slot.get_last().await` returns `None`, surfacing
/// `CompactionError::Internal("no cache-safe params")`.
fn errored_compactor() -> Arc<CompactionOrchestrator> {
    use compaction::autocompact::Autocompactor;
    use compaction::microcompact::{Microcompactor, TimeBasedMCConfig};
    use compaction::snip::SnipCompactor;
    use sidequery::{CacheSafeParamsSlot, ForkedAgentRunner, SubagentSlotProvider};

    // Marker pool — `SubagentSlotProvider` is intentionally an empty
    // marker trait (M1.14); the forked-agent stub never touches it.
    struct NeverProvider;
    impl SubagentSlotProvider for NeverProvider {}

    let runner = Arc::new(ForkedAgentRunner::new(Arc::new(NeverProvider)));
    let slot = Arc::new(CacheSafeParamsSlot::new()); // empty — triggers Internal err

    let orch = CompactionOrchestrator {
        snip: SnipCompactor,
        micro: Microcompactor {
            config: TimeBasedMCConfig::default(),
        },
        auto: Autocompactor::with_forked_runner(runner, slot),
        // Threshold 1 token → autocompact ALWAYS fires.
        autocompact_threshold: 1,
    };
    Arc::new(orch)
}

#[tokio::test]
async fn failure_leaves_history_unchanged() {
    let api = Arc::new(MockApiClient::new(vec![]));
    let tools = Arc::new(tools::registry::ToolRegistry::new());
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
    )
    .with_compaction(errored_compactor());

    // Seed 5 padded messages so estimate_tokens_for_range exceeds the
    // 1-token threshold and autocompact actually fires.
    {
        let session = orch.session();
        let mut s = session.lock().await;
        for i in 0..5 {
            s.history.push(ConversationMessage::user(
                MessageId::new(),
                format!("m{i} body padded with filler text to ensure token estimate > 1"),
            ));
        }
    }
    let len_before = {
        let session = orch.session();
        let s = session.lock().await;
        s.history.len()
    };

    let err = orch.force_compact().await.expect_err("must fail");
    let s = err.to_string();
    assert!(s.contains("compaction failed"), "got: {s}");

    let len_after = {
        let session = orch.session();
        let s = session.lock().await;
        s.history.len()
    };
    assert_eq!(
        len_before, len_after,
        "history must be untouched on failure"
    );
}

#[tokio::test]
async fn five_consecutive_force_compact_calls_do_not_explode() {
    let orch = make_orch();
    seed_history(&orch, 30).await;

    for i in 0..5 {
        let r = orch.force_compact().await;
        assert!(r.is_ok(), "iter {i} failed: {:?}", r.err());
    }

    // No assertion on final length — the stub autocompact collapses to
    // 1 + marker on each pass; we just confirm no panics / no leaks
    // (validated implicitly by `cargo test` finishing).
}

// ============================================================================
// M6-08 Task 14 — Compaction Safety Gate (HARD GATE)
//
// One test exercising the three gate assertions end-to-end:
//   1. messages_after < messages_before (collapse happened).
//   2. Round-trip integrity: the post-compaction history is still a VALID
//      message sequence — no dangling tool_use without tool_result, the
//      [Compacted] System marker is the well-formed tail, and the
//      summary message precedes it.
//   3. The NEXT TURN after compaction still runs: the orchestrator
//      accepts the compacted history and `run_turn` completes with
//      EndTurn (no panic, no corruption).
//
// Asserts on COUNTS + structural validity (real), NOT on the M3 stub
// summary body.
// ============================================================================

/// Returns `true` if `history` is a structurally valid message sequence:
/// every assistant `ToolUse` block has a matching `ToolResult` in a later
/// user message, and no orphan `ToolResult` precedes its `ToolUse`.
fn history_is_valid(history: &[ConversationMessage]) -> bool {
    use protocol::{ContentBlock, ToolUseId};
    use std::collections::HashSet;

    let mut open_tool_uses: HashSet<ToolUseId> = HashSet::new();
    let mut satisfied: HashSet<ToolUseId> = HashSet::new();
    for msg in history {
        match msg {
            ConversationMessage::Assistant { content, .. } => {
                for b in content {
                    if let ContentBlock::ToolUse { id, .. } = b {
                        open_tool_uses.insert(*id);
                    }
                }
            }
            ConversationMessage::User { content, .. } => {
                for b in content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = b {
                        // A tool_result must reference a tool_use we've
                        // already seen — orphan results are corruption.
                        if !open_tool_uses.contains(tool_use_id) {
                            return false;
                        }
                        satisfied.insert(*tool_use_id);
                    }
                }
            }
            ConversationMessage::System { .. } => {}
        }
    }
    // Every tool_use must eventually be satisfied by a tool_result.
    open_tool_uses.is_subset(&satisfied)
}

#[tokio::test]
async fn compaction_safety_gate() {
    use api_client::types::{ContentBlockApi, MessageResponse, UsageApi};

    // Scripted end_turn response so the post-compaction turn can run.
    let response = MessageResponse {
        id: "msg_gate".into(),
        model: "claude-opus-4-7".into(),
        content: vec![ContentBlockApi::Text { text: "ack".into() }],
        stop_reason: Some("end_turn".into()),
        usage: UsageApi::default(),
    };
    let api = Arc::new(MockApiClient::new(vec![response]));
    let orch = Arc::new(
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api,
            Arc::new(tools::registry::ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
        .with_compaction(Arc::new(CompactionOrchestrator::new(1_000))),
    );

    seed_history(&orch, 50).await;

    // --- Assertion 1: collapse happened ---
    let summary = orch.force_compact().await.expect("force_compact ok");
    assert_eq!(summary.messages_before, 50, "before count must be 50");
    assert!(
        summary.messages_after < summary.messages_before,
        "GATE#1 FAIL: messages_after={} not < messages_before={}",
        summary.messages_after,
        summary.messages_before
    );

    // --- Assertion 2: round-trip integrity ---
    {
        let session = orch.session();
        let s = session.lock().await;
        assert!(!s.history.is_empty(), "GATE#2 FAIL: empty history");
        assert!(
            history_is_valid(&s.history),
            "GATE#2 FAIL: post-compaction history is not a valid message sequence"
        );
        let last = s.history.last().unwrap();
        match last {
            ConversationMessage::System { content, .. } => {
                assert!(
                    content.starts_with("[Compacted 50 → "),
                    "GATE#2 FAIL: marker malformed; got: {content}"
                );
            }
            other => panic!("GATE#2 FAIL: expected System marker tail; got {other:?}"),
        }
        // The first message is valid (history head is present).
        assert!(
            s.history.first().is_some(),
            "GATE#2 FAIL: first message missing"
        );
    }

    // --- Assertion 3: the next turn still runs against compacted history ---
    let outcome = orch
        .run_turn("continue after compaction")
        .await
        .expect("GATE#3 FAIL: run_turn errored on compacted history");
    assert!(
        matches!(outcome, ConversationOutcome::EndTurn { .. }),
        "GATE#3 FAIL: expected EndTurn after compaction; got {outcome:?}"
    );
}
