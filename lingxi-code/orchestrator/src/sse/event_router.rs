//! `StreamEvent` → router-action dispatch.
//!
//! The streaming loop ([`crate::streaming_loop::pump_stream`]) pumps one
//! `StreamEvent` at a time through [`dispatch_event`], which:
//!
//! - Mutates the [`BlockAccumulator`] for `content_block_*` events.
//! - Calls back into [`OutputStream::emit_text`] for `text_delta` events
//!   (true per-token streaming).
//! - Returns a [`RouterAction`] hint for actions the streaming loop must
//!   take ON its own — namely `DispatchToolUse` (when a tool block
//!   reaches stop) and `EndOfStream` (when `message_stop` arrives).
#![forbid(unsafe_code)]

use super::accumulator::{BlockAccumulator, BlockKind, CompletedBlock};
use super::StreamingError;
use llm_client::{ContentBlock as LlmContentBlock, ContentDelta, LlmEvent, Usage};
use protocol::{ContentBlock, ToolUseId};
use std::sync::Arc;
use traits::OutputStream;

/// Result of routing one `StreamEvent`. The streaming loop acts on each.
#[derive(Debug, Clone)]
pub enum RouterAction {
    /// No further action — event handled internally (state mutation or
    /// output emit only).
    Continue,
    /// A `tool_use` block just completed at `content_block_stop`. The
    /// streaming loop spawns a dispatch IMMEDIATELY.
    DispatchToolUse {
        /// Tool use identifier.
        id: ToolUseId,
        /// Tool name.
        name: String,
        /// Reassembled tool input.
        input: serde_json::Value,
        /// Verbatim provider-issued tool-call id, preserved for egress replay.
        provider_id: Option<String>,
    },
    /// A text or thinking block completed — append to the in-flight
    /// assistant message and continue.
    AppendAssistantBlock(ContentBlock),
    /// `message_delta` arrived with a `stop_reason`. Streaming loop
    /// records this and continues until `message_stop` arrives.
    /// `output_tokens` carries the delta's usage snapshot (A3 budget
    /// accounting); `0` when the delta had no usage.
    RecordStopReason {
        /// The final `stop_reason`.
        stop_reason: String,
        /// Cumulative output tokens from this delta's usage (`0` if absent).
        output_tokens: u64,
        /// Full usage snapshot from the `message_delta` (authoritative for
        /// billing). `None` when the delta carried no usage.
        usage: Option<Usage>,
    },
    /// `message_delta` arrived with usage but NO `stop_reason` (A3). The
    /// streaming loop records `output_tokens` and continues.
    RecordUsage {
        /// Cumulative output tokens from this delta's usage.
        output_tokens: u64,
        /// Full usage snapshot from this delta.
        usage: Option<Usage>,
    },
    /// `message_stop` arrived — terminate the per-turn loop.
    EndOfStream,
}

/// Route one event through the accumulator + output sink. Returns the
/// next action for the streaming loop.
///
/// # Errors
/// Propagates [`StreamingError`] from accumulator mutations.
pub async fn dispatch_event(
    event: LlmEvent,
    acc: &mut BlockAccumulator,
    output: &Arc<dyn OutputStream>,
) -> Result<RouterAction, StreamingError> {
    match event {
        LlmEvent::MessageStart { response } => {
            // No-op for state; the loop already knows the model + id from
            // the turn invocation. claude-code captures `partialMessage`
            // and `ttftMs` here; we don't need those at the M5-04 wire.
            //
            // §0.7 "light up thinking/usage": `message_start` carries the
            // initial usage snapshot (input + cache-read tokens). Surface
            // it to the output sink so consumers see an early token count;
            // the final `message_delta` usage supersedes it.
            emit_usage_if_present(output, &response.usage).await;
            Ok(RouterAction::Continue)
        }
        LlmEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            let kind = match &content_block {
                LlmContentBlock::Text { .. } => BlockKind::Text,
                LlmContentBlock::ToolCall { id, name, .. } => BlockKind::ToolUse {
                    // LlmContentBlock::ToolCall uses String ids; parse to ToolUseId.
                    id: serde_json::from_value::<ToolUseId>(
                        serde_json::Value::String(id.clone()),
                    )
                    .unwrap_or_else(|_| ToolUseId::new()),
                    name: name.clone(),
                    // Preserve the verbatim provider id (e.g. Anthropic `toolu_…`)
                    // for egress replay; the minted `ToolUseId` is internal-only.
                    provider_id: Some(id.clone()),
                },
                LlmContentBlock::Reasoning { .. } => BlockKind::Thinking,
                LlmContentBlock::ServerToolUse { .. }
                | LlmContentBlock::ConnectorText { .. }
                | LlmContentBlock::AdvisorToolResult { .. }
                | LlmContentBlock::Image { .. }
                | LlmContentBlock::ImageUrl { .. }
                | LlmContentBlock::Document { .. }
                | LlmContentBlock::ToolResult { .. }
                | LlmContentBlock::RedactedThinking { .. } => BlockKind::Other,
            };
            acc.start_block(index, kind)?;
            Ok(RouterAction::Continue)
        }
        LlmEvent::ContentBlockDelta { index, delta } => {
            match delta {
                ContentDelta::TextDelta { text } => {
                    acc.append_text(index, &text)?;
                    // Stream the token to the output sink RIGHT NOW.
                    // This is the key M5-04 behavior: tokens are
                    // surfaced as they arrive, not buffered per-block.
                    output.emit_text(&text).await;
                }
                ContentDelta::InputJsonDelta { partial_json } => {
                    acc.append_json(index, &partial_json)?;
                }
                ContentDelta::ThinkingDelta { thinking } => {
                    acc.append_text(index, &thinking)?;
                    // §0.7 "light up thinking/usage": stream the reasoning
                    // delta to the output sink RIGHT NOW, mirroring the
                    // `TextDelta` arm above. `signature` is `None` on the
                    // live delta — the cryptographic signature only arrives
                    // on the completed thinking block (`SignatureDelta`).
                    output.emit_thinking(&thinking, None).await;
                }
                ContentDelta::SignatureDelta { signature } => {
                    acc.set_signature(index, &signature)?;
                }
                ContentDelta::CitationsDelta { .. } | ContentDelta::ConnectorTextDelta { .. } => {
                    // Dropped at M5-04 boundary (parity with M5-02's
                    // `translate_response_blocks` which drops them).
                }
            }
            Ok(RouterAction::Continue)
        }
        LlmEvent::ContentBlockStop { index } => {
            let completed = acc.stop_block(index)?;
            match completed {
                CompletedBlock::Text { text } => {
                    Ok(RouterAction::AppendAssistantBlock(ContentBlock::Text {
                        text,
                    }))
                }
                CompletedBlock::Thinking {
                    thinking,
                    signature,
                } => Ok(RouterAction::AppendAssistantBlock(ContentBlock::Thinking {
                    thinking,
                    signature,
                })),
                CompletedBlock::ToolUse {
                    id,
                    name,
                    input,
                    provider_id,
                } => Ok(RouterAction::DispatchToolUse {
                    id,
                    name,
                    input,
                    provider_id,
                }),
                CompletedBlock::Skipped => Ok(RouterAction::Continue),
            }
        }
        LlmEvent::MessageDelta { delta, usage } => {
            // §0.7 "light up thinking/usage": `message_delta` carries the
            // final usage snapshot. Surface it to the output sink BEFORE
            // computing the router action — the stop-reason behavior below
            // is unchanged.
            // A3: capture the output-token count before `usage` is consumed
            // by the emit helper, so the budget loop can accumulate it.
            // BILLING: clone the full usage BEFORE emit consumes it so the
            // caller (pump_stream → try_run_turn_streaming) can record it in
            // CostTracker. The `message_delta` usage is the authoritative
            // final snapshot (includes both input and output tokens).
            let output_tokens = usage.as_ref().map_or(0, |u| u.billable_tokens.output);
            let usage_for_billing = usage.clone();
            if let Some(usage) = usage {
                emit_usage_if_present(output, &usage).await;
            }
            if let Some(sr) = delta.stop_reason {
                Ok(RouterAction::RecordStopReason {
                    stop_reason: sr,
                    output_tokens,
                    usage: usage_for_billing,
                })
            } else if output_tokens > 0 {
                Ok(RouterAction::RecordUsage {
                    output_tokens,
                    usage: usage_for_billing,
                })
            } else {
                Ok(RouterAction::Continue)
            }
        }
        // NOTE: LlmEvent has no Ping or Error variants — errors surface as
        // Err(LlmError) from the stream, and keepalives are never forwarded
        // from the transport layer. The Completed short-circuit terminal is
        // treated as an end-of-stream signal (the full response is available
        // in the response field but we forward the already-accumulated blocks).
        LlmEvent::MessageStop | LlmEvent::Completed { .. } => Ok(RouterAction::EndOfStream),
    }
}

/// Surface an SSE `usage` snapshot to the output sink as a live usage
/// update (§0.7 "light up thinking/usage").
///
/// Maps `llm_client::Usage` onto the four bare `u64` arguments of
/// [`OutputStream::emit_usage`] with the same field mapping the cost
/// pipeline uses: `input` ← `billable_tokens.input`, `output` ←
/// `billable_tokens.output`, `cache_read` ← `billable_tokens.cache_read`,
/// `cache_write` ← `billable_tokens.cache_write`.
async fn emit_usage_if_present(output: &Arc<dyn OutputStream>, usage: &Usage) {
    output
        .emit_usage(
            usage.billable_tokens.input,
            usage.billable_tokens.output,
            usage.billable_tokens.cache_read,
            usage.billable_tokens.cache_write,
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockOutputStream;
    use llm_client::MessageDeltaPayload;

    #[tokio::test]
    async fn text_delta_emits_to_output_and_accumulates() {
        let mut acc = BlockAccumulator::new();
        let mock = Arc::new(MockOutputStream::new());
        let out: Arc<dyn OutputStream> = mock.clone();
        // start a text block
        dispatch_event(
            LlmEvent::ContentBlockStart {
                index: 0,
                content_block: LlmContentBlock::Text {
                    text: String::new(),
                    cache_control: None,
                },
            },
            &mut acc,
            &out,
        )
        .await
        .expect("start");
        // delta
        dispatch_event(
            LlmEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::TextDelta { text: "hi".into() },
            },
            &mut acc,
            &out,
        )
        .await
        .expect("delta");
        // OutputStream observed exactly one emit_text("hi")
        let events = mock.snapshot().await;
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn message_delta_records_stop_reason() {
        let mut acc = BlockAccumulator::new();
        let mock = Arc::new(MockOutputStream::new());
        let out: Arc<dyn OutputStream> = mock.clone();
        let action = dispatch_event(
            LlmEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: Some("end_turn".into()),
                },
                usage: None,
            },
            &mut acc,
            &out,
        )
        .await
        .expect("ok");
        match action {
            RouterAction::RecordStopReason { stop_reason, .. } => {
                assert_eq!(stop_reason, "end_turn");
            }
            other => panic!("expected RecordStopReason, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn message_stop_ends_stream() {
        let mut acc = BlockAccumulator::new();
        let mock = Arc::new(MockOutputStream::new());
        let out: Arc<dyn OutputStream> = mock.clone();
        let action = dispatch_event(LlmEvent::MessageStop, &mut acc, &out)
            .await
            .expect("ok");
        assert!(matches!(action, RouterAction::EndOfStream));
    }

    #[tokio::test]
    async fn completed_event_ends_stream() {
        use llm_client::{LlmResponse, Usage};
        let mut acc = BlockAccumulator::new();
        let mock = Arc::new(MockOutputStream::new());
        let out: Arc<dyn OutputStream> = mock.clone();
        let resp = LlmResponse {
            id: "msg_1".into(),
            model: "claude-opus-4-7".into(),
            content: vec![],
            stop_reason: Some("end_turn".into()),
            usage: Usage::default(),
            cost: None,
            provider_metadata: serde_json::Value::Null,
        };
        let action = dispatch_event(
            LlmEvent::Completed { response: Box::new(resp) },
            &mut acc,
            &out,
        )
        .await
        .expect("ok");
        assert!(matches!(action, RouterAction::EndOfStream));
    }
}
