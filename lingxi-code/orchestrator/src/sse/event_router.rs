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
use api_client::types::{ContentBlockApi, ContentDelta, StreamEvent, UsageApi};
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
    },
    /// A text or thinking block completed — append to the in-flight
    /// assistant message and continue.
    AppendAssistantBlock(ContentBlock),
    /// `message_delta` arrived with a `stop_reason`. Streaming loop
    /// records this and continues until `message_stop` arrives.
    RecordStopReason(String),
    /// `message_stop` arrived — terminate the per-turn loop.
    EndOfStream,
    /// A server-emitted `Error` event — surface as a streaming error.
    ServerError(String),
}

/// Route one event through the accumulator + output sink. Returns the
/// next action for the streaming loop.
///
/// # Errors
/// Propagates [`StreamingError`] from accumulator mutations.
pub async fn dispatch_event(
    event: StreamEvent,
    acc: &mut BlockAccumulator,
    output: &Arc<dyn OutputStream>,
) -> Result<RouterAction, StreamingError> {
    match event {
        StreamEvent::MessageStart { message } => {
            // No-op for state; the loop already knows the model + id from
            // the turn invocation. claude-code captures `partialMessage`
            // and `ttftMs` here; we don't need those at the M5-04 wire.
            //
            // §0.7 "light up thinking/usage": `message_start` carries the
            // initial usage snapshot (input + cache-read tokens). Surface
            // it to the output sink so consumers see an early token count;
            // the final `message_delta` usage supersedes it.
            emit_usage_if_present(output, &message.usage).await;
            Ok(RouterAction::Continue)
        }
        StreamEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            let kind = match &content_block {
                ContentBlockApi::Text { .. } => BlockKind::Text,
                ContentBlockApi::ToolUse { id, name, .. } => BlockKind::ToolUse {
                    id: *id,
                    name: name.clone(),
                },
                ContentBlockApi::Thinking { .. } => BlockKind::Thinking,
                ContentBlockApi::ServerToolUse { .. }
                | ContentBlockApi::ConnectorText { .. }
                | ContentBlockApi::AdvisorToolResult { .. } => BlockKind::Other,
            };
            acc.start_block(index, kind)?;
            Ok(RouterAction::Continue)
        }
        StreamEvent::ContentBlockDelta { index, delta } => {
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
        StreamEvent::ContentBlockStop { index } => {
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
                CompletedBlock::ToolUse { id, name, input } => {
                    Ok(RouterAction::DispatchToolUse { id, name, input })
                }
                CompletedBlock::Skipped => Ok(RouterAction::Continue),
            }
        }
        StreamEvent::MessageDelta { delta, usage } => {
            // §0.7 "light up thinking/usage": `message_delta` carries the
            // final usage snapshot. Surface it to the output sink BEFORE
            // computing the router action — the stop-reason behavior below
            // is unchanged.
            if let Some(usage) = usage {
                emit_usage_if_present(output, &usage).await;
            }
            if let Some(sr) = delta.stop_reason {
                Ok(RouterAction::RecordStopReason(sr))
            } else {
                Ok(RouterAction::Continue)
            }
        }
        StreamEvent::MessageStop => Ok(RouterAction::EndOfStream),
        StreamEvent::Ping => Ok(RouterAction::Continue),
        StreamEvent::Error { error } => Ok(RouterAction::ServerError(format!(
            "{}: {}",
            error.kind, error.message
        ))),
    }
}

/// Surface an SSE `usage` snapshot to the output sink as a live usage
/// update (§0.7 "light up thinking/usage").
///
/// Maps `UsageApi` onto the four bare `u64` arguments of
/// [`OutputStream::emit_usage`] with the SAME field mapping the cost
/// pipeline uses (`api-client/src/anthropic.rs`): `input` ←
/// `input_tokens`, `output` ← `output_tokens`, `cache_read` ←
/// `cache_read_input_tokens`, `cache_write` ← `cache_creation_input_tokens`.
async fn emit_usage_if_present(output: &Arc<dyn OutputStream>, usage: &UsageApi) {
    output
        .emit_usage(
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_input_tokens,
            usage.cache_creation_input_tokens,
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockOutputStream;
    use api_client::types::MessageDeltaPayload;

    #[tokio::test]
    async fn text_delta_emits_to_output_and_accumulates() {
        let mut acc = BlockAccumulator::new();
        let mock = Arc::new(MockOutputStream::new());
        let out: Arc<dyn OutputStream> = mock.clone();
        // start a text block
        dispatch_event(
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockApi::Text {
                    text: String::new(),
                },
            },
            &mut acc,
            &out,
        )
        .await
        .expect("start");
        // delta
        dispatch_event(
            StreamEvent::ContentBlockDelta {
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
    async fn ping_is_noop() {
        let mut acc = BlockAccumulator::new();
        let mock = Arc::new(MockOutputStream::new());
        let out: Arc<dyn OutputStream> = mock.clone();
        let action = dispatch_event(StreamEvent::Ping, &mut acc, &out)
            .await
            .expect("ok");
        assert!(matches!(action, RouterAction::Continue));
        assert!(acc.is_idle());
    }

    #[tokio::test]
    async fn message_delta_records_stop_reason() {
        let mut acc = BlockAccumulator::new();
        let mock = Arc::new(MockOutputStream::new());
        let out: Arc<dyn OutputStream> = mock.clone();
        let action = dispatch_event(
            StreamEvent::MessageDelta {
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
            RouterAction::RecordStopReason(sr) => assert_eq!(sr, "end_turn"),
            other => panic!("expected RecordStopReason, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn message_stop_ends_stream() {
        let mut acc = BlockAccumulator::new();
        let mock = Arc::new(MockOutputStream::new());
        let out: Arc<dyn OutputStream> = mock.clone();
        let action = dispatch_event(StreamEvent::MessageStop, &mut acc, &out)
            .await
            .expect("ok");
        assert!(matches!(action, RouterAction::EndOfStream));
    }
}
