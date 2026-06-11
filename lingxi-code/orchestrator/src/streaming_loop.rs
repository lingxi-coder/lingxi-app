//! Streaming turn loop core helpers.
//!
//! ## `StreamingError` → `OrchestratorError` mapping
//!
//! Each [`crate::sse::StreamingError`] variant is converted to
//! [`crate::error::OrchestratorError::StreamingProtocol`] via its
//! `Display` impl. The Display strings (locked at Task 1 step 3) are
//! the public-facing reason carried in the orchestrator error.
#![forbid(unsafe_code)]

use crate::conversation::ConversationOrchestrator;
use crate::error::OrchestratorError;
use crate::sse::accumulator::BlockAccumulator;
use crate::sse::event_router::{dispatch_event, RouterAction};
use crate::turn_loop::dispatch_tool_uses_tracked;
use futures::stream::{BoxStream, StreamExt};
use llm_client::{LlmError, LlmEvent, Usage as LlmUsage};
use protocol::{ContentBlock, ToolUseId};
use serde_json::Value;
use std::sync::Arc;
use traits::OutputStream;

/// One tool dispatch request observed during the stream. Carries the
/// id/name/input the orchestrator must invoke. The dispatch itself is
/// performed by the streaming-loop caller (so this module stays free of
/// `ToolRegistry` / `HookExecutor` / `PermissionGate` deps).
#[derive(Debug, Clone)]
pub struct ObservedToolUse {
    /// Stable identifier echoed back in the matching `ToolResult`.
    pub id: ToolUseId,
    /// Tool name.
    pub name: String,
    /// Reassembled tool input.
    pub input: Value,
}

/// Outcome of consuming one stream.
#[derive(Debug, Default)]
pub struct PumpedTurn {
    /// Content blocks (text + thinking) accumulated during the stream,
    /// in observation order. Used to construct the assistant message
    /// after the stream ends.
    pub assistant_blocks: Vec<ContentBlock>,
    /// Tool uses observed during the stream, in observation order
    /// (i.e. order of their `content_block_stop` events).
    pub tool_uses: Vec<ObservedToolUse>,
    /// Final `stop_reason` (from `message_delta`). `None` if the stream
    /// ended without a `message_delta` carrying one.
    pub stop_reason: Option<String>,
    /// A3: this turn's output-token count, taken from the final
    /// `message_delta` usage snapshot (the cumulative output tokens of the
    /// streamed message). `0` if the stream carried no usage. The token-budget
    /// continuation loop accumulates this into `global_turn_tokens`.
    pub output_tokens: u64,
    /// Full usage snapshot for billing. The `message_delta` usage is
    /// authoritative when present (it includes both input + output tokens
    /// as the final cumulative snapshot). Falls back to `message_start`
    /// usage when `message_delta` carried no usage. `None` only when the
    /// stream carried no usage at all (unusual; treated as zero-cost).
    ///
    /// Used by `try_run_turn_streaming` to record into `CostTracker`
    /// (mirrors the non-streaming path in `turn_loop.rs`).
    pub usage: Option<LlmUsage>,
}

/// Consume the given stream to completion, routing events through the
/// accumulator + output sink. Returns the per-block summary; the
/// streaming-loop caller is responsible for tool dispatch + appending
/// to session history.
///
/// # Errors
/// - [`OrchestratorError::Streaming`] wrapping an [`LlmError`] if the
///   underlying transport surfaces an error mid-stream.
/// - [`OrchestratorError::StreamingProtocol`] if the wire-level event
///   sequence violates the per-block protocol (out-of-order delta,
///   double stop, type mismatch, malformed `tool_use` input JSON).
/// - [`OrchestratorError::StreamEndedWithoutStop`] if the stream
///   produced no `MessageStop` event before terminating.
pub async fn pump_stream(
    mut stream: BoxStream<'static, Result<LlmEvent, LlmError>>,
    output: &Arc<dyn OutputStream>,
) -> Result<PumpedTurn, OrchestratorError> {
    let mut acc = BlockAccumulator::new();
    let mut turn = PumpedTurn::default();
    // Capture the MessageStart usage as the fallback billing source for
    // input tokens, in case MessageDelta carries no usage (rare). The
    // MessageDelta usage supersedes this when present.
    let mut message_start_usage: Option<LlmUsage> = None;

    while let Some(item) = stream.next().await {
        let event = item.map_err(OrchestratorError::Streaming)?;
        // Capture MessageStart usage before dispatching (dispatch consumes the event).
        if let LlmEvent::MessageStart { ref response } = event {
            message_start_usage = Some(response.usage.clone());
        }
        let action = dispatch_event(event, &mut acc, output)
            .await
            .map_err(|e| OrchestratorError::StreamingProtocol(e.to_string()))?;
        match action {
            RouterAction::Continue => {}
            RouterAction::AppendAssistantBlock(block) => {
                turn.assistant_blocks.push(block);
            }
            RouterAction::DispatchToolUse { id, name, input } => {
                turn.tool_uses.push(ObservedToolUse { id, name, input });
            }
            RouterAction::RecordStopReason {
                stop_reason,
                output_tokens,
                usage,
            } => {
                turn.stop_reason = Some(stop_reason);
                // The final delta's usage supersedes any earlier snapshot.
                if output_tokens > 0 {
                    turn.output_tokens = output_tokens;
                }
                // BILLING: take the MessageDelta usage as authoritative; fall
                // back to MessageStart if absent (preserves input-token billing
                // even when the delta carries no snapshot).
                turn.usage = usage.or_else(|| message_start_usage.clone());
            }
            RouterAction::RecordUsage { output_tokens, usage } => {
                // Usage-only delta (no stop_reason yet): keep the latest count.
                turn.output_tokens = output_tokens;
                turn.usage = usage.or_else(|| message_start_usage.clone());
            }
            RouterAction::EndOfStream => {
                return Ok(turn);
            }
        }
    }
    // Stream ended without a MessageStop.
    Err(OrchestratorError::StreamEndedWithoutStop)
}

/// One concurrently-dispatched tool's result, tagged with its original stream
/// index so the caller can restore stream order after `join_all`. Carries the
/// tool's `tool_result` block, its injected `new_messages` (each paired with the
/// injecting tool's `tool_use_id` — TS `sourceToolUseID`), and any
/// `context_modifier`s. Aliased to keep the type below clippy's
/// `type_complexity` threshold.
type IndexedDispatchResult = (
    usize,
    ContentBlock,
    Vec<(protocol::ConversationMessage, ToolUseId)>,
    Vec<tool_api::ContextModifier>,
);

/// Dispatch N `tool_use` blocks concurrently. Each dispatch goes through
/// the same pre-tool-hook → permission → tool-call → post-tool-hook
/// pipeline as the batched path ([`crate::turn_loop::dispatch_tool_uses_tracked`]
/// is reused per-tool to keep the byte-locked hook + permission
/// ordering for each individual dispatch, AND to thread out each tool's
/// injected `new_messages`).
///
/// Returns `(blocks, injected_messages)`: the [`ContentBlock::ToolResult`]
/// blocks IN ORIGINAL ORDER (matching the `tool_use` block order in the
/// stream), plus any tool-injected `new_messages` (SKILLEXEC.3 — the Skill
/// tool's expanded prompt) flattened in the same tool order so the caller
/// replays them into history after the `tool_result`. The
/// `OutputStream::emit_tool_call` / `emit_tool_result` events fire in
/// COMPLETION order (not dispatch order) — that's the visible
/// streaming behavior.
///
/// Concurrency model: `futures::future::join_all` polls all futures
/// on the current task. For dispatches with `.await` points (hooks,
/// permission checks, registry lookups, tool bodies), this yields
/// true cooperative concurrency without `'static` requirements.
///
/// # Errors
/// Returns the first [`OrchestratorError`] surfaced by any underlying
/// dispatch; the remaining results are dropped (consistent with the
/// batched path's fail-fast semantics).
pub async fn dispatch_tool_uses_concurrent(
    orch: &ConversationOrchestrator,
    observed: &[ObservedToolUse],
) -> Result<
    (
        Vec<ContentBlock>,
        Vec<(protocol::ConversationMessage, ToolUseId)>,
        Vec<tool_api::ContextModifier>,
    ),
    OrchestratorError,
> {
    use futures::future::join_all;

    let futures: Vec<_> = observed
        .iter()
        .enumerate()
        .map(|(idx, tu)| {
            let single = vec![(tu.id, tu.name.clone(), tu.input.clone())];
            async move {
                // SKILLEXEC.3 (streaming): use the TRACKED dispatch so a tool's
                // injected `new_messages` (the Skill tool's expanded prompt) are
                // threaded out and replayed into history, mirroring the batched
                // path. `prevent_continuation` (.1) is dropped here — the streaming
                // loop sources that signal separately. The `context_modifier`s
                // (.3, e.g. a skill's `model:` override) ARE threaded out, in tool
                // order, for the caller to fold POST-BATCH.
                let (mut blocks, _prevent, injected, modifiers) =
                    dispatch_tool_uses_tracked(orch, &single).await?;
                let block = blocks.pop().ok_or_else(|| {
                    OrchestratorError::StreamingProtocol(format!(
                        "dispatch returned empty for tool index {idx}"
                    ))
                })?;
                Ok::<IndexedDispatchResult, OrchestratorError>((
                    idx, block, injected, modifiers,
                ))
            }
        })
        .collect();

    let mut indexed: Vec<IndexedDispatchResult> = Vec::with_capacity(observed.len());
    for r in join_all(futures).await {
        indexed.push(r?);
    }
    indexed.sort_by_key(|(idx, _, _, _)| *idx);
    // Blocks IN ORIGINAL ORDER; injected messages + context_modifiers flattened
    // in the same tool order so the Skill prompt + model override land
    // deterministically after the tool_result.
    let mut blocks = Vec::with_capacity(indexed.len());
    let mut injected_all: Vec<(protocol::ConversationMessage, ToolUseId)> = Vec::new();
    let mut modifiers_all: Vec<tool_api::ContextModifier> = Vec::new();
    for (_, b, inj, mods) in indexed {
        blocks.push(b);
        injected_all.extend(inj);
        modifiers_all.extend(mods);
    }
    Ok((blocks, injected_all, modifiers_all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockOutputStream;
    use crate::test_support_stream::{
        content_block_start_text, content_block_start_thinking, content_block_start_tool_use,
        content_block_stop, input_json_delta, message_delta_stop, message_delta_stop_with_usage,
        message_start, message_stop, text_delta, thinking_delta,
    };
    use futures::stream;
    use protocol::ToolUseId;

    fn boxed(events: Vec<LlmEvent>) -> BoxStream<'static, Result<LlmEvent, LlmError>> {
        stream::iter(events.into_iter().map(Ok)).boxed()
    }

    #[tokio::test]
    async fn text_only_pump_assembles_one_text_block() {
        let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::new());
        let evs = vec![
            message_start("m1", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "he"),
            text_delta(0, "llo"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ];
        let turn = pump_stream(boxed(evs), &out).await.expect("pump");
        assert_eq!(turn.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(turn.assistant_blocks.len(), 1);
        if let ContentBlock::Text { text } = &turn.assistant_blocks[0] {
            assert_eq!(text, "hello");
        } else {
            panic!("expected Text block, got {:?}", turn.assistant_blocks[0]);
        }
        assert!(turn.tool_uses.is_empty());
    }

    #[tokio::test]
    async fn tool_use_pump_collects_dispatch_request() {
        let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::new());
        let tu = ToolUseId::new();
        let evs = vec![
            message_start("m1", "claude-opus-4-7"),
            content_block_start_tool_use(1, tu, "Read"),
            input_json_delta(1, "{\"file"),
            input_json_delta(1, "_path\":\"foo.rs\"}"),
            content_block_stop(1),
            message_delta_stop("tool_use"),
            message_stop(),
        ];
        let turn = pump_stream(boxed(evs), &out).await.expect("pump");
        assert_eq!(turn.tool_uses.len(), 1);
        assert_eq!(turn.tool_uses[0].name, "Read");
        assert_eq!(turn.tool_uses[0].input["file_path"], "foo.rs");
        assert_eq!(turn.stop_reason.as_deref(), Some("tool_use"));
    }

    #[tokio::test]
    async fn stream_without_message_stop_errors() {
        let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::new());
        let evs = vec![
            message_start("m1", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "partial"),
            content_block_stop(0),
            // no message_stop
        ];
        let err = pump_stream(boxed(evs), &out).await.expect_err("no stop");
        assert!(matches!(err, OrchestratorError::StreamEndedWithoutStop));
    }

    #[tokio::test]
    async fn streaming_protocol_error_propagates() {
        let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::new());
        // delta before start → BlockNotFound
        let evs = vec![
            message_start("m1", "claude-opus-4-7"),
            text_delta(0, "oops"),
            message_stop(),
        ];
        let err = pump_stream(boxed(evs), &out).await.expect_err("proto");
        match err {
            OrchestratorError::StreamingProtocol(reason) => {
                assert!(reason.contains("block index 0"), "{reason}");
            }
            other => panic!("expected StreamingProtocol, got {other:?}"),
        }
    }

    /// §0.7 "light up thinking/usage": a stream carrying a `ThinkingDelta`
    /// and a `MessageDelta` with a final `usage` snapshot must surface both
    /// to the `OutputStream` via `emit_thinking` + `emit_usage`, while the
    /// stop-reason / assembled-turn behavior stays exactly as before.
    #[tokio::test]
    async fn thinking_and_usage_deltas_emit_to_output() {
        use crate::test_support::MockOutputStream;
        use llm_client::Usage;
        use traits::OutputEvent;

        let mock = Arc::new(MockOutputStream::new());
        let out: Arc<dyn OutputStream> = mock.clone();
        let evs = vec![
            message_start("m1", "claude-opus-4-7"),
            // a thinking block streamed as a delta
            content_block_start_thinking(0),
            thinking_delta(0, "let me reason"),
            content_block_stop(0),
            // a text block so the assembled turn is non-trivial
            content_block_start_text(1),
            text_delta(1, "answer"),
            content_block_stop(1),
            // final message_delta with stop_reason AND usage
            message_delta_stop_with_usage(
                "end_turn",
                Usage {
                    billable_tokens: llm_client::TokenUsage {
                        input: 120,
                        output: 35,
                        cache_write: 10,
                        cache_read: 5,
                        reasoning_output: 0,
                    },
                    ..Usage::default()
                },
            ),
            message_stop(),
        ];
        let turn = pump_stream(boxed(evs), &out).await.expect("pump");

        // Existing behavior is unchanged: stop reason + assembled blocks.
        assert_eq!(turn.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(turn.assistant_blocks.len(), 2);
        assert!(matches!(
            &turn.assistant_blocks[0],
            ContentBlock::Thinking { thinking, signature }
                if thinking == "let me reason" && signature.is_none()
        ));

        let events = mock.snapshot().await;

        // emit_thinking fired exactly once with the live delta + None sig.
        let thinking: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                OutputEvent::Thinking {
                    thinking,
                    signature,
                } => Some((thinking.clone(), signature.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(thinking, vec![("let me reason".to_string(), None)]);

        // emit_usage fired with the message_delta usage mapped field-for-field.
        // (message_start carried a default all-zero usage, emitted first.)
        let usages: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                OutputEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_creation_tokens,
                } => Some((
                    *input_tokens,
                    *output_tokens,
                    *cache_read_tokens,
                    *cache_creation_tokens,
                )),
                _ => None,
            })
            .collect();
        assert!(
            usages.contains(&(120, 35, 5, 10)),
            "expected final usage (120,35,5,10), got {usages:?}"
        );
        // message_start's default-zero usage was surfaced too.
        assert_eq!(usages.first(), Some(&(0, 0, 0, 0)));
    }

    #[tokio::test]
    async fn underlying_stream_error_surfaces_as_streaming_variant() {
        let out: Arc<dyn OutputStream> = Arc::new(MockOutputStream::new());
        let s: BoxStream<'static, Result<LlmEvent, LlmError>> = stream::iter(vec![
            Ok(message_start("m1", "claude-opus-4-7")),
            Err(LlmError::Transport {
                message: "dropped".into(),
            }),
        ])
        .boxed();
        let err = pump_stream(s, &out).await.expect_err("network");
        assert!(matches!(err, OrchestratorError::Streaming(_)));
    }
}
