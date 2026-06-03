//! Self-contained SSE accumulator for the streaming subagent path.
//!
//! This is a deliberate copy of `orchestrator::sse`'s `BlockAccumulator` and
//! its `event_router` accumulation logic. The agent crate cannot depend on the
//! orchestrator (that would close a dependency cycle — the orchestrator already
//! depends on `agent`), so the byte-locked streaming semantics are reproduced
//! here verbatim. [`accumulate_stream`] drives a [`StreamEvent`] stream to a
//! single [`api_client::MessageResponse`] — the exact value the non-streaming
//! [`crate::api::SubagentApiClient::messages_create`] returns — so the
//! multi-turn [`crate::runner::run_subagent`] loop is byte-identical regardless
//! of which transport produced the turn.
//!
//! [`response_to_stream_events`] is the inverse: it synthesizes a lossless
//! [`StreamEvent`] sequence from a `MessageResponse`, which the default
//! [`crate::api::SubagentApiClient::messages_create_stream`] impl uses so a
//! client that only implements the non-streaming round-trip still presents a
//! streaming seam. The two functions round-trip exactly (see the
//! `round_trip_*` tests).
//!
//! Source semantics: `claude.ts:1995-2300`, mirrored through
//! `orchestrator::sse::accumulator` + `orchestrator::sse::event_router`.
#![forbid(unsafe_code)]

use api_client::types::{ContentBlockApi, ContentDelta, MessageDeltaPayload, MessageResponse, StreamEvent, UsageApi};
use api_client::ApiError;
use futures::stream::{BoxStream, StreamExt};
use protocol::ToolUseId;
use serde_json::Value;
use std::collections::HashMap;

/// Tag identifying what kind of content block is being accumulated.
///
/// Mirrors `orchestrator::sse::accumulator::BlockKind`. `ToolUse` carries the
/// API-provided id + name verbatim; the input JSON is reassembled from
/// `input_json_delta` chunks.
#[derive(Debug, Clone)]
enum BlockKind {
    /// A `text` block — accumulates `text_delta` chunks.
    Text,
    /// A `tool_use` block — accumulates `input_json_delta` chunks.
    ToolUse {
        /// Stable identifier echoed back in the matching `ToolResult`.
        id: ToolUseId,
        /// Tool name (e.g. `"Read"`).
        name: String,
    },
    /// A `thinking` block — accumulates `thinking_delta` chunks.
    Thinking,
    /// Any other variant (`server_tool_use`, `connector_text`,
    /// `advisor_tool_result`). Accumulator stores nothing; `stop_block`
    /// returns [`CompletedBlock::Skipped`].
    Other,
}

impl BlockKind {
    fn name(&self) -> &'static str {
        match self {
            BlockKind::Text => "text",
            BlockKind::ToolUse { .. } => "tool_use",
            BlockKind::Thinking => "thinking",
            BlockKind::Other => "other",
        }
    }
}

/// One finished content block, ready to be appended to the assistant message.
#[derive(Debug, Clone)]
enum CompletedBlock {
    /// Plain text.
    Text {
        /// Concatenated body of all `text_delta` chunks.
        text: String,
    },
    /// Tool invocation, with reassembled JSON input.
    ToolUse {
        /// Tool use identifier.
        id: ToolUseId,
        /// Tool name.
        name: String,
        /// Reassembled tool input.
        input: Value,
    },
    /// Extended thinking.
    Thinking {
        /// Concatenated thinking body.
        thinking: String,
        /// Optional cryptographic signature.
        signature: Option<String>,
    },
    /// A [`BlockKind::Other`] variant — caller drops it.
    Skipped,
}

impl CompletedBlock {
    /// Project a finished block back onto the wire `ContentBlockApi` shape the
    /// [`MessageResponse`] carries. [`CompletedBlock::Skipped`] yields `None`
    /// (the block is dropped, exactly as the non-streaming
    /// `translate_response_blocks` drops server-side variants).
    fn into_content_block(self) -> Option<ContentBlockApi> {
        match self {
            CompletedBlock::Text { text } => Some(ContentBlockApi::Text { text }),
            CompletedBlock::ToolUse { id, name, input } => {
                Some(ContentBlockApi::ToolUse { id, name, input })
            }
            CompletedBlock::Thinking { thinking, signature } => {
                Some(ContentBlockApi::Thinking { thinking, signature })
            }
            CompletedBlock::Skipped => None,
        }
    }
}

/// Per-block state held during accumulation.
#[derive(Debug)]
struct BlockState {
    kind: BlockKind,
    /// Used for `Text` and `Thinking`.
    text_buf: String,
    /// Used for `ToolUse` (raw `partial_json` concat).
    json_buf: String,
    /// Set by `signature_delta` on a `Thinking` block.
    signature: Option<String>,
}

/// In-progress accumulator. One per active stream consumer.
#[derive(Debug, Default)]
struct BlockAccumulator {
    blocks: HashMap<u32, BlockState>,
}

impl BlockAccumulator {
    fn new() -> Self {
        Self {
            blocks: HashMap::new(),
        }
    }

    /// Register a new block at `index`. An existing entry is overwritten,
    /// mirroring `claude.ts:1996-2070` (`contentBlocks[part.index] = { ... }`).
    fn start_block(&mut self, index: u32, kind: BlockKind) {
        self.blocks.insert(
            index,
            BlockState {
                kind,
                text_buf: String::new(),
                json_buf: String::new(),
                signature: None,
            },
        );
    }

    /// Append `text` to the `Text` or `Thinking` buffer of the block at
    /// `index`.
    fn append_text(&mut self, index: u32, text: &str) -> Result<(), ApiError> {
        let state = self
            .blocks
            .get_mut(&index)
            .ok_or_else(|| block_not_found(index))?;
        match &state.kind {
            BlockKind::Text | BlockKind::Thinking => {
                state.text_buf.push_str(text);
                Ok(())
            }
            other => Err(type_mismatch(index, other.name(), "text_delta")),
        }
    }

    /// Append a `partial_json` chunk to the `ToolUse` buffer of the block at
    /// `index`.
    fn append_json(&mut self, index: u32, partial: &str) -> Result<(), ApiError> {
        let state = self
            .blocks
            .get_mut(&index)
            .ok_or_else(|| block_not_found(index))?;
        match &state.kind {
            BlockKind::ToolUse { .. } => {
                state.json_buf.push_str(partial);
                Ok(())
            }
            other => Err(type_mismatch(index, other.name(), "input_json_delta")),
        }
    }

    /// Set the signature on a `Thinking` block (from a `signature_delta`).
    fn set_signature(&mut self, index: u32, sig: &str) -> Result<(), ApiError> {
        let state = self
            .blocks
            .get_mut(&index)
            .ok_or_else(|| block_not_found(index))?;
        match &state.kind {
            BlockKind::Thinking => {
                state.signature = Some(sig.to_string());
                Ok(())
            }
            other => Err(type_mismatch(index, other.name(), "signature_delta")),
        }
    }

    /// Finalize the block at `index`, removing it and returning a
    /// [`CompletedBlock`].
    fn stop_block(&mut self, index: u32) -> Result<CompletedBlock, ApiError> {
        let state = self
            .blocks
            .remove(&index)
            .ok_or_else(|| double_stop(index))?;
        let completed = match state.kind {
            BlockKind::Text => CompletedBlock::Text {
                text: state.text_buf,
            },
            BlockKind::Thinking => CompletedBlock::Thinking {
                thinking: state.text_buf,
                signature: state.signature,
            },
            BlockKind::ToolUse { id, name } => {
                let input = if state.json_buf.is_empty() {
                    Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str::<Value>(&state.json_buf)
                        .map_err(|e| tool_use_json_parse(index, &e.to_string(), &state.json_buf))?
                };
                CompletedBlock::ToolUse { id, name, input }
            }
            BlockKind::Other => CompletedBlock::Skipped,
        };
        Ok(completed)
    }
}

// ----- Error constructors (mirror `orchestrator::sse::StreamingError` Display,
// wrapped in `ApiError::MalformedStream` so the subagent loop's single
// `Result<MessageResponse, ApiError>` seam carries them) --------------------

fn block_not_found(index: u32) -> ApiError {
    ApiError::MalformedStream(format!(
        "streaming: delta for block index {index} without prior start"
    ))
}

fn double_stop(index: u32) -> ApiError {
    ApiError::MalformedStream(format!("streaming: double stop for block index {index}"))
}

fn type_mismatch(index: u32, expected: &str, got: &str) -> ApiError {
    ApiError::MalformedStream(format!(
        "streaming: type mismatch on block {index}: expected {expected}, got {got}"
    ))
}

fn tool_use_json_parse(index: u32, reason: &str, buffer: &str) -> ApiError {
    ApiError::MalformedStream(format!(
        "streaming: tool_use input failed to parse as JSON on block {index}: {reason}: buffer={buffer:?}"
    ))
}

/// Map a wire `content_block` payload to its accumulator [`BlockKind`].
/// Mirrors `orchestrator::sse::event_router`'s `ContentBlockStart` arm.
fn block_kind_of(content_block: &ContentBlockApi) -> BlockKind {
    match content_block {
        ContentBlockApi::Text { .. } => BlockKind::Text,
        ContentBlockApi::ToolUse { id, name, .. } => BlockKind::ToolUse {
            id: *id,
            name: name.clone(),
        },
        ContentBlockApi::Thinking { .. } => BlockKind::Thinking,
        ContentBlockApi::ServerToolUse { .. }
        | ContentBlockApi::ConnectorText { .. }
        | ContentBlockApi::AdvisorToolResult { .. } => BlockKind::Other,
    }
}

/// Merge a `message_delta` usage snapshot into the `message_start` seed.
///
/// On the wire, `message_start.usage` carries `input_tokens` + cache counts
/// with `output_tokens` still `0`, and `message_delta.usage` carries the final
/// `output_tokens` (Anthropic omits the input/cache counts there). So
/// `output_tokens` always takes the delta value, while the input/cache counts
/// keep the seed unless the delta reports a non-zero override. This makes the
/// [`response_to_stream_events`] round-trip exact (the seed already equals the
/// final usage) and stays correct against a real provider stream.
fn merge_usage(seed: UsageApi, delta: UsageApi) -> UsageApi {
    UsageApi {
        input_tokens: if delta.input_tokens > 0 {
            delta.input_tokens
        } else {
            seed.input_tokens
        },
        output_tokens: delta.output_tokens,
        cache_creation_input_tokens: if delta.cache_creation_input_tokens > 0 {
            delta.cache_creation_input_tokens
        } else {
            seed.cache_creation_input_tokens
        },
        cache_read_input_tokens: if delta.cache_read_input_tokens > 0 {
            delta.cache_read_input_tokens
        } else {
            seed.cache_read_input_tokens
        },
    }
}

/// Drive `stream` to completion, accumulating SSE events into a single
/// [`MessageResponse`] — the streaming analog of one non-streaming
/// `messages_create` round-trip. `id` / `model` / the usage seed come from
/// `message_start`; the final `stop_reason` + usage come from `message_delta`.
///
/// # Errors
/// - The transport [`ApiError`] verbatim if the stream yields `Err`.
/// - [`ApiError::MalformedStream`] if the event sequence violates the per-block
///   protocol (delta before start, double stop, type mismatch, unparseable
///   `tool_use` input) or the server emits an `error` event.
/// - [`ApiError::UnexpectedStreamEnd`] if the stream ends before `message_stop`.
pub(crate) async fn accumulate_stream(
    mut stream: BoxStream<'static, Result<StreamEvent, ApiError>>,
) -> Result<MessageResponse, ApiError> {
    let mut acc = BlockAccumulator::new();
    let mut content: Vec<ContentBlockApi> = Vec::new();
    let mut id = String::new();
    let mut model = String::new();
    let mut usage = UsageApi::default();
    let mut stop_reason: Option<String> = None;

    while let Some(item) = stream.next().await {
        // Transport-level error: surface verbatim (mirrors `pump_stream`'s
        // `OrchestratorError::Streaming` passthrough).
        let event = item?;
        match event {
            StreamEvent::MessageStart { message } => {
                // claude-code captures `partialMessage`/`ttftMs` here; the
                // subagent loop only needs id/model + the usage seed.
                id = message.id;
                model = message.model;
                usage = message.usage;
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                acc.start_block(index, block_kind_of(&content_block));
            }
            StreamEvent::ContentBlockDelta { index, delta } => match delta {
                ContentDelta::TextDelta { text } => acc.append_text(index, &text)?,
                ContentDelta::InputJsonDelta { partial_json } => {
                    acc.append_json(index, &partial_json)?;
                }
                ContentDelta::ThinkingDelta { thinking } => acc.append_text(index, &thinking)?,
                ContentDelta::SignatureDelta { signature } => acc.set_signature(index, &signature)?,
                // Dropped at the M5-02 `translate_response_blocks` boundary.
                ContentDelta::CitationsDelta { .. } | ContentDelta::ConnectorTextDelta { .. } => {}
            },
            StreamEvent::ContentBlockStop { index } => {
                if let Some(block) = acc.stop_block(index)?.into_content_block() {
                    content.push(block);
                }
            }
            StreamEvent::MessageDelta {
                delta,
                usage: delta_usage,
            } => {
                if let Some(sr) = delta.stop_reason {
                    stop_reason = Some(sr);
                }
                if let Some(u) = delta_usage {
                    usage = merge_usage(usage, u);
                }
            }
            StreamEvent::MessageStop => {
                return Ok(MessageResponse {
                    id,
                    model,
                    content,
                    stop_reason,
                    usage,
                });
            }
            StreamEvent::Ping => {}
            StreamEvent::Error { error } => {
                return Err(ApiError::MalformedStream(format!(
                    "server-emitted error event: {}: {}",
                    error.kind, error.message
                )));
            }
        }
    }
    // Stream ended without a `message_stop`.
    Err(ApiError::UnexpectedStreamEnd)
}

/// Synthesize a [`StreamEvent`] sequence that reconstructs `resp` exactly when
/// fed back through [`accumulate_stream`].
///
/// Used by the default [`crate::api::SubagentApiClient::messages_create_stream`]
/// impl so a client that only implements the non-streaming `messages_create`
/// still presents a streaming seam. The round-trip is lossless: `text` /
/// `thinking` bodies ride a single delta (the `content_block_start` payload is
/// empty, exactly as on the wire), `tool_use` input rides one `input_json_delta`
/// (re-parsed on stop), and the full usage is seeded on `message_start` so the
/// [`merge_usage`] reconstruction reproduces `resp.usage`.
pub(crate) fn response_to_stream_events(resp: MessageResponse) -> Vec<StreamEvent> {
    let mut events = Vec::with_capacity(resp.content.len() * 3 + 3);
    // `message_start` carries id/model + a usage seed. On the wire the seed
    // holds input/cache with `output_tokens == 0`; here we seed the FULL usage
    // so the round-trip is exact regardless of the field split.
    events.push(StreamEvent::MessageStart {
        message: MessageResponse {
            id: resp.id.clone(),
            model: resp.model.clone(),
            content: Vec::new(),
            stop_reason: None,
            usage: resp.usage,
        },
    });
    for (i, block) in resp.content.into_iter().enumerate() {
        // Content-block counts never approach `u32::MAX`; saturate rather than
        // panic on the (unreachable) overflow.
        let index = u32::try_from(i).unwrap_or(u32::MAX);
        match block {
            ContentBlockApi::Text { text } => {
                events.push(StreamEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlockApi::Text {
                        text: String::new(),
                    },
                });
                events.push(StreamEvent::ContentBlockDelta {
                    index,
                    delta: ContentDelta::TextDelta { text },
                });
            }
            ContentBlockApi::Thinking {
                thinking,
                signature,
            } => {
                events.push(StreamEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlockApi::Thinking {
                        thinking: String::new(),
                        signature: None,
                    },
                });
                events.push(StreamEvent::ContentBlockDelta {
                    index,
                    delta: ContentDelta::ThinkingDelta { thinking },
                });
                if let Some(signature) = signature {
                    events.push(StreamEvent::ContentBlockDelta {
                        index,
                        delta: ContentDelta::SignatureDelta { signature },
                    });
                }
            }
            ContentBlockApi::ToolUse { id, name, input } => {
                events.push(StreamEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlockApi::ToolUse {
                        id,
                        name,
                        input: Value::Null,
                    },
                });
                // The accumulator reassembles + parses this back to `input`.
                events.push(StreamEvent::ContentBlockDelta {
                    index,
                    delta: ContentDelta::InputJsonDelta {
                        partial_json: input.to_string(),
                    },
                });
            }
            // Server-side variants the subagent drops anyway: emit only a start
            // so the index is consumed; the accumulator yields `Skipped` on
            // stop and the block is dropped (round-trips to nothing, matching
            // `translate_response_blocks`).
            other => {
                events.push(StreamEvent::ContentBlockStart {
                    index,
                    content_block: other,
                });
            }
        }
        events.push(StreamEvent::ContentBlockStop { index });
    }
    events.push(StreamEvent::MessageDelta {
        delta: MessageDeltaPayload {
            stop_reason: resp.stop_reason,
        },
        usage: Some(resp.usage),
    });
    events.push(StreamEvent::MessageStop);
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_client::types::ErrorPayload;
    use futures::stream;

    fn boxed(events: Vec<StreamEvent>) -> BoxStream<'static, Result<StreamEvent, ApiError>> {
        stream::iter(events.into_iter().map(Ok)).boxed()
    }

    fn message_start(id: &str, model: &str) -> StreamEvent {
        StreamEvent::MessageStart {
            message: MessageResponse {
                id: id.to_string(),
                model: model.to_string(),
                content: Vec::new(),
                stop_reason: None,
                usage: UsageApi::default(),
            },
        }
    }

    #[tokio::test]
    async fn text_stream_accumulates_one_text_block() {
        let evs = vec![
            message_start("m1", "claude-mock"),
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockApi::Text {
                    text: String::new(),
                },
            },
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::TextDelta { text: "he".into() },
            },
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::TextDelta {
                    text: "llo".into(),
                },
            },
            StreamEvent::ContentBlockStop { index: 0 },
            StreamEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: Some("end_turn".into()),
                },
                usage: None,
            },
            StreamEvent::MessageStop,
        ];
        let resp = accumulate_stream(boxed(evs)).await.expect("accumulate");
        assert_eq!(resp.id, "m1");
        assert_eq!(resp.model, "claude-mock");
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(resp.content.len(), 1);
        match &resp.content[0] {
            ContentBlockApi::Text { text } => assert_eq!(text, "hello"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_use_stream_reassembles_input_json() {
        let id = ToolUseId::new();
        let evs = vec![
            message_start("m1", "claude-mock"),
            StreamEvent::ContentBlockStart {
                index: 1,
                content_block: ContentBlockApi::ToolUse {
                    id,
                    name: "Read".into(),
                    input: Value::Null,
                },
            },
            StreamEvent::ContentBlockDelta {
                index: 1,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: "{\"file".into(),
                },
            },
            StreamEvent::ContentBlockDelta {
                index: 1,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: "_path\":\"foo.rs\"}".into(),
                },
            },
            StreamEvent::ContentBlockStop { index: 1 },
            StreamEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: Some("tool_use".into()),
                },
                usage: None,
            },
            StreamEvent::MessageStop,
        ];
        let resp = accumulate_stream(boxed(evs)).await.expect("accumulate");
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(resp.content.len(), 1);
        match &resp.content[0] {
            ContentBlockApi::ToolUse {
                id: got_id,
                name,
                input,
            } => {
                assert_eq!(*got_id, id);
                assert_eq!(name, "Read");
                assert_eq!(input["file_path"], "foo.rs");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn thinking_stream_accumulates_body_and_signature() {
        let evs = vec![
            message_start("m1", "claude-mock"),
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockApi::Thinking {
                    thinking: String::new(),
                    signature: None,
                },
            },
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::ThinkingDelta {
                    thinking: "ponder".into(),
                },
            },
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::SignatureDelta {
                    signature: "sig-1".into(),
                },
            },
            StreamEvent::ContentBlockStop { index: 0 },
            StreamEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: Some("end_turn".into()),
                },
                usage: None,
            },
            StreamEvent::MessageStop,
        ];
        let resp = accumulate_stream(boxed(evs)).await.expect("accumulate");
        match &resp.content[0] {
            ContentBlockApi::Thinking {
                thinking,
                signature,
            } => {
                assert_eq!(thinking, "ponder");
                assert_eq!(signature.as_deref(), Some("sig-1"));
            }
            other => panic!("expected Thinking, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn final_usage_merges_start_seed_and_delta() {
        let evs = vec![
            StreamEvent::MessageStart {
                message: MessageResponse {
                    id: "m1".into(),
                    model: "claude-mock".into(),
                    content: Vec::new(),
                    stop_reason: None,
                    usage: UsageApi {
                        input_tokens: 42,
                        output_tokens: 0,
                        cache_creation_input_tokens: 7,
                        cache_read_input_tokens: 3,
                    },
                },
            },
            StreamEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: Some("end_turn".into()),
                },
                usage: Some(UsageApi {
                    input_tokens: 0,
                    output_tokens: 99,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
            },
            StreamEvent::MessageStop,
        ];
        let resp = accumulate_stream(boxed(evs)).await.expect("accumulate");
        // output_tokens from the delta; input/cache kept from the start seed.
        assert_eq!(resp.usage.input_tokens, 42);
        assert_eq!(resp.usage.output_tokens, 99);
        assert_eq!(resp.usage.cache_creation_input_tokens, 7);
        assert_eq!(resp.usage.cache_read_input_tokens, 3);
    }

    #[tokio::test]
    async fn stream_without_message_stop_errors() {
        let evs = vec![
            message_start("m1", "claude-mock"),
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockApi::Text {
                    text: String::new(),
                },
            },
            StreamEvent::ContentBlockStop { index: 0 },
            // no message_stop
        ];
        let err = accumulate_stream(boxed(evs)).await.expect_err("no stop");
        assert!(matches!(err, ApiError::UnexpectedStreamEnd));
    }

    #[tokio::test]
    async fn delta_before_start_is_malformed() {
        let evs = vec![
            message_start("m1", "claude-mock"),
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::TextDelta {
                    text: "oops".into(),
                },
            },
            StreamEvent::MessageStop,
        ];
        let err = accumulate_stream(boxed(evs)).await.expect_err("malformed");
        match err {
            ApiError::MalformedStream(reason) => {
                assert!(reason.contains("block index 0"), "{reason}");
            }
            other => panic!("expected MalformedStream, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn type_mismatch_is_malformed() {
        // `input_json_delta` on a text block.
        let evs = vec![
            message_start("m1", "claude-mock"),
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockApi::Text {
                    text: String::new(),
                },
            },
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: "{}".into(),
                },
            },
            StreamEvent::MessageStop,
        ];
        let err = accumulate_stream(boxed(evs)).await.expect_err("mismatch");
        match err {
            ApiError::MalformedStream(reason) => {
                assert!(reason.contains("type mismatch"), "{reason}");
            }
            other => panic!("expected MalformedStream, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn bad_tool_use_json_is_malformed() {
        let evs = vec![
            message_start("m1", "claude-mock"),
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockApi::ToolUse {
                    id: ToolUseId::new(),
                    name: "Read".into(),
                    input: Value::Null,
                },
            },
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: "{not json".into(),
                },
            },
            StreamEvent::ContentBlockStop { index: 0 },
            StreamEvent::MessageStop,
        ];
        let err = accumulate_stream(boxed(evs)).await.expect_err("bad json");
        match err {
            ApiError::MalformedStream(reason) => {
                assert!(reason.contains("failed to parse as JSON"), "{reason}");
            }
            other => panic!("expected MalformedStream, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn server_error_event_is_malformed() {
        let evs = vec![
            message_start("m1", "claude-mock"),
            StreamEvent::Error {
                error: ErrorPayload {
                    kind: "overloaded_error".into(),
                    message: "slow down".into(),
                },
            },
        ];
        let err = accumulate_stream(boxed(evs)).await.expect_err("server error");
        match err {
            ApiError::MalformedStream(reason) => {
                assert!(reason.contains("server-emitted error event"), "{reason}");
                assert!(reason.contains("overloaded_error"), "{reason}");
            }
            other => panic!("expected MalformedStream, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_error_passes_through_verbatim() {
        let s: BoxStream<'static, Result<StreamEvent, ApiError>> = stream::iter(vec![
            Ok(message_start("m1", "claude-mock")),
            Err(ApiError::Http(traits::HttpError::Connection("dropped".into()))),
        ])
        .boxed();
        let err = accumulate_stream(s).await.expect_err("transport");
        assert!(matches!(
            err,
            ApiError::Http(traits::HttpError::Connection(_))
        ));
    }

    #[tokio::test]
    async fn skipped_blocks_round_trip_to_nothing() {
        // A `server_tool_use` block is dropped, exactly like the non-streaming
        // `translate_response_blocks` path.
        let resp = MessageResponse {
            id: "m1".into(),
            model: "claude-mock".into(),
            content: vec![
                ContentBlockApi::ServerToolUse {
                    id: "srv-1".into(),
                    name: "advisor".into(),
                    input: Value::Null,
                },
                ContentBlockApi::Text {
                    text: "kept".into(),
                },
            ],
            stop_reason: Some("end_turn".into()),
            usage: UsageApi::default(),
        };
        let round = accumulate_stream(boxed(response_to_stream_events(resp)))
            .await
            .expect("round-trip");
        // Only the text block survives the round-trip.
        assert_eq!(round.content.len(), 1);
        assert!(matches!(round.content[0], ContentBlockApi::Text { .. }));
    }

    async fn assert_round_trips(resp: MessageResponse) {
        // Drive the synthetic stream back through the accumulator.
        let events = response_to_stream_events(resp.clone());
        let round = accumulate_stream(boxed(events))
            .await
            .expect("round-trip accumulate");
        assert_eq!(round.id, resp.id);
        assert_eq!(round.model, resp.model);
        assert_eq!(round.stop_reason, resp.stop_reason);
        assert_eq!(
            serde_json::to_value(&round.content).unwrap(),
            serde_json::to_value(&resp.content).unwrap()
        );
        assert_eq!(round.usage.input_tokens, resp.usage.input_tokens);
        assert_eq!(round.usage.output_tokens, resp.usage.output_tokens);
        assert_eq!(
            round.usage.cache_creation_input_tokens,
            resp.usage.cache_creation_input_tokens
        );
        assert_eq!(
            round.usage.cache_read_input_tokens,
            resp.usage.cache_read_input_tokens
        );
    }

    #[tokio::test]
    async fn round_trip_text_and_tool_use_and_thinking() {
        assert_round_trips(MessageResponse {
            id: "m1".into(),
            model: "claude-mock".into(),
            content: vec![
                ContentBlockApi::Text {
                    text: "answer".into(),
                },
                ContentBlockApi::Thinking {
                    thinking: "reason".into(),
                    signature: Some("sig".into()),
                },
                ContentBlockApi::ToolUse {
                    id: ToolUseId::new(),
                    name: "Read".into(),
                    input: serde_json::json!({"file_path": "x.rs", "limit": 10}),
                },
            ],
            stop_reason: Some("tool_use".into()),
            usage: UsageApi {
                input_tokens: 11,
                output_tokens: 22,
                cache_creation_input_tokens: 1,
                cache_read_input_tokens: 2,
            },
        })
        .await;
    }

    #[tokio::test]
    async fn round_trip_empty_tool_input() {
        assert_round_trips(MessageResponse {
            id: "m2".into(),
            model: "claude-mock".into(),
            content: vec![ContentBlockApi::ToolUse {
                id: ToolUseId::new(),
                name: "Now".into(),
                input: serde_json::json!({}),
            }],
            stop_reason: Some("end_turn".into()),
            usage: UsageApi::default(),
        })
        .await;
    }
}
