//! reactive recovery on the BATCHED path.
//!
//! Verifies the `execute_one_turn` PTL recovery loop (TS `compact.ts:450-491`,
//! `query.ts:1070-1183`):
//!
//! - `ApiError::PromptTooLong` returned twice then a success → the loop
//!   truncates the head twice (`truncateHeadForPTLRetry`) and the third call
//!   succeeds. We assert the API saw 3 calls with strictly-shrinking message
//!   counts, and the turn ended normally with the model's text.
//! - `ApiError::PromptTooLong` returned `MAX_PTL_RETRIES`+1 times → after the
//!   retry budget the loop attempts ONE reactive full compact and retries; when
//!   that STILL 413s, the turn ends with the byte-exact
//!   `PROMPT_TOO_LONG_ERROR_MESSAGE` assistant message (no hard error bubbled).
use async_trait::async_trait;
use compaction::CompactionOrchestrator;
use llm_client::{ContentBlock as LlmContentBlock, LlmError, LlmResponse};
use orchestrator::test_support::{
    mock_message_response, noop_hook_executor, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorApiClient, OrchestratorConfig};
use protocol::{ContentBlock, ConversationMessage, MessageId};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;
use traits::OutputEvent;

/// A mock API client whose queue is `Result<LlmResponse, LlmError>` so a
/// test can script `LlmError::ContextOverflow` responses. Captures per-call
/// message counts so the test can assert head-truncation shrank the prompt.
struct PtlMockApi {
    queue: Mutex<VecDeque<Result<LlmResponse, LlmError>>>,
    captured_lens: Mutex<Vec<usize>>,
}

impl PtlMockApi {
    fn new(script: Vec<Result<LlmResponse, LlmError>>) -> Self {
        Self {
            queue: Mutex::new(VecDeque::from(script)),
            captured_lens: Mutex::new(Vec::new()),
        }
    }
    async fn call_lens(&self) -> Vec<usize> {
        self.captured_lens.lock().await.clone()
    }
}

#[async_trait]
impl OrchestratorApiClient for PtlMockApi {
    async fn messages_create(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        self.captured_lens.lock().await.push(msgs.len());
        let mut q = self.queue.lock().await;
        q.pop_front().unwrap_or_else(|| {
            Err(LlmError::Transport {
                message: "ptl mock script exhausted".into(),
            })
        })
    }
}

fn ptl_err(token_gap: u64) -> Result<LlmResponse, LlmError> {
    Err(LlmError::ContextOverflow { token_gap })
}

// The `Result` wrap is required: `ok_text` is pushed into the same scripted
// response vec as `ptl_err` (which returns `Err`), so the type must match.
#[allow(clippy::unnecessary_wraps)]
fn ok_text(text: &str) -> Result<LlmResponse, LlmError> {
    Ok(mock_message_response(
        vec![LlmContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
        Some("end_turn"),
    ))
}

/// Build an orchestrator over the given scripted `PtlMockApi`. Threshold is
/// high so the proactive Batch-4 trigger never fires (it would otherwise shrink
/// the prompt before the call and mask the reactive path). The PTL reactive
/// fallback in Batch 5 deliberately uses a LOW-threshold compactor only in the
/// exhaustion test.
fn make_orch(
    api: Arc<PtlMockApi>,
    compactor_threshold: Option<u64>,
) -> (Arc<ConversationOrchestrator>, Arc<MockOutputStream>) {
    let tools = Arc::new(tool_api::registry::ToolRegistry::new());
    let hooks = noop_hook_executor();
    let perms = Arc::new(NoOpPermissionGate);
    let output = Arc::new(MockOutputStream::new());
    let memory = Arc::new(StaticMemoryProvider::empty());
    let mut orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        tools,
        hooks,
        perms,
        output.clone(),
        memory,
        std::env::temp_dir(),
    );
    if let Some(t) = compactor_threshold {
        orch = orch.with_compaction(Arc::new(CompactionOrchestrator::new(t)));
    }
    (Arc::new(orch), output)
}

/// Seed `rounds` API rounds (`user, assistant(distinct id)` pairs) so the head
/// truncation has ≥2 groups to drop. Each message is padded so the token
/// estimate is meaningful.
async fn seed_rounds(orch: &ConversationOrchestrator, rounds: usize) {
    let session = orch.session();
    let mut s = session.lock().await;
    for i in 0..rounds {
        s.history.push(ConversationMessage::user(
            MessageId::new(),
            format!(
                "round-{i} user message with filler text to give the round a real token estimate"
            ),
        ));
        s.history.push(ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::Text {
                text: format!(
                    "round-{i} assistant reply with filler text to give the round weight"
                ),
            }],
            stop_reason: Some("end_turn".to_string()),
        });
    }
}

async fn history_len(orch: &ConversationOrchestrator) -> usize {
    let session = orch.session();
    let s = session.lock().await;
    s.history.len()
}

#[tokio::test]
async fn ptl_twice_then_success_truncates_twice() {
    // Script: PTL, PTL, then a successful response. A SMALL token_gap (30) so
    // each `truncateHeadForPTLRetry` drops only ~1 oldest round (the gap-driven
    // accumulation stops at the first group whose estimate clears 30 tokens),
    // letting us observe two distinct shrinks before the success.
    let api = Arc::new(PtlMockApi::new(vec![
        ptl_err(30),
        ptl_err(30),
        ok_text("recovered"),
    ]));
    // No compactor needed — truncation alone recovers before exhaustion.
    let (orch, output) = make_orch(api.clone(), None);
    // Use 20 rounds so the 20% fallback (used with ContextOverflow, gap=0)
    // drops 4+ groups — producing a strictly shorter message list on each retry.
    seed_rounds(&orch, 20).await;

    let outcome = orch.run_turn("trigger").await.expect("turn ends ok");
    let _ = outcome;

    // Exactly 3 batched calls: initial + 2 retries-after-truncation.
    let lens = api.call_lens().await;
    assert_eq!(
        lens.len(),
        3,
        "expected initial call + 2 truncation retries; got {lens:?}"
    );
    // Each truncation retry must carry STRICTLY fewer messages than the prior
    // call (head dropped at least one round). Two truncations => two shrinks.
    assert!(
        lens[1] < lens[0],
        "first retry must carry fewer messages ({} < {})",
        lens[1],
        lens[0]
    );
    assert!(
        lens[2] < lens[1],
        "second retry must carry fewer messages ({} < {})",
        lens[2],
        lens[1]
    );

    // The turn ended on the model's recovered text, not a PTL error.
    let events = output.snapshot().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OutputEvent::Text { text } if text == "recovered")),
        "recovered assistant text must be emitted; events = {events:#?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OutputEvent::Text { text } if text == "Prompt is too long")),
        "no prompt-too-long error message on the successful-recovery path"
    );
}

#[tokio::test]
async fn ptl_exhausted_attempts_reactive_compact_then_surfaces_error() {
    // Script: PTL on every call (more than MAX_PTL_RETRIES + the one
    // reactive-compact retry), so recovery exhausts and the turn ends with the
    // byte-exact error message.
    let script: Vec<Result<LlmResponse, LlmError>> = (0..12).map(|_| ptl_err(500)).collect();
    let api = Arc::new(PtlMockApi::new(script));
    // Low-threshold compactor so the reactive full-compact fallback actually
    // fires (autocompact > snip-alone) and we exercise the apply_post_compact
    // tail before surfacing the error.
    let (orch, output) = make_orch(api.clone(), Some(100));
    seed_rounds(&orch, 12).await;
    let before = history_len(&orch).await;

    // The turn must NOT bubble a hard error — it ends normally with the
    // prompt-too-long assistant message.
    orch.run_turn("trigger")
        .await
        .expect("turn ends without bubbling a hard error");

    // A compaction must have been attempted during recovery. We assert this via
    // the `CompactionCompleted` output event rather than a surviving boundary
    // marker in history: with #58 the (proactive/reactive) compaction now
    // preserves a verbatim recent-message tail, so the post-compact history is
    // larger and the SUBSEQUENT prompt-too-long retry can head-truncate the
    // boundary marker away (`truncateHeadForPTLRetry` drops oldest groups). The
    // `CompactionCompleted` event is emitted by `apply_post_compact` the instant
    // a compaction lands and is not subject to that later truncation — so it is
    // the robust signal that a compact ran. (Before #58 the compact output was
    // 2 messages, too small to truncate, so the marker happened to survive.)
    let _before = before;
    let session = orch.session();
    let after = {
        let s = session.lock().await;
        s.history.clone()
    };
    let events = output.snapshot().await;
    let compacted = events
        .iter()
        .any(|e| matches!(e, OutputEvent::CompactionCompleted { .. }));
    assert!(
        compacted,
        "a compaction must have run during PTL recovery (CompactionCompleted emitted); history len before={before}"
    );

    // The turn ended with the byte-exact PROMPT_TOO_LONG_ERROR_MESSAGE assistant
    // text and an EndTurn event.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OutputEvent::Text { text } if text == "Prompt is too long")),
        "byte-exact prompt-too-long message must be surfaced; events = {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OutputEvent::EndTurn { .. })),
        "the turn must end (EndTurn emitted)"
    );
    // The last assistant message in history is the prompt-too-long error.
    let last_assistant_text = after.iter().rev().find_map(|m| match m {
        ConversationMessage::Assistant { content, .. } => content.iter().find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    });
    assert_eq!(
        last_assistant_text.as_deref(),
        Some("Prompt is too long"),
        "the final assistant message must be the prompt-too-long error"
    );
}
