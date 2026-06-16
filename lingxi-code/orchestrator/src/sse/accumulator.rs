//! Per-block accumulator for the streaming SSE path.
//!
//! Tracks one `BlockState` per `index: u32` for the lifetime of a single
//! `messages.create` response. Each block starts via
//! [`BlockAccumulator::start_block`], receives 0..N appends, and is
//! finalized via [`BlockAccumulator::stop_block`] which returns a
//! [`CompletedBlock`]. The accumulator drops the entry on stop —
//! subsequent operations on the same `index` error with
//! `StreamingError::DoubleStop`.
//!
//! See M5-04 plan reverse-engineered byte-locks for the source
//! semantics (claude.ts:1995-2300).
#![forbid(unsafe_code)]

use super::StreamingError;
use protocol::ToolUseId;
use serde_json::Value;
use std::collections::HashMap;

/// Tag identifying what kind of content block is being accumulated.
///
/// Constructed from a `StreamEvent::ContentBlockStart` payload at the call
/// site (formerly `api_client::types::StreamEvent`; now
/// `llm_client::LlmEvent`). `ToolUse` carries the API-provided id + name
/// verbatim; the input JSON is reassembled from `input_json_delta` chunks.
#[derive(Debug, Clone)]
pub enum BlockKind {
    /// A `text` block — accumulates `text_delta` chunks.
    Text,
    /// A `tool_use` block — accumulates `input_json_delta` chunks.
    ToolUse {
        /// Stable identifier echoed back in the matching `ToolResult`.
        id: ToolUseId,
        /// Tool name (e.g. `"Read"`).
        name: String,
        /// Verbatim provider-issued tool-call id, preserved for egress replay.
        provider_id: Option<String>,
    },
    /// A `thinking` block — accumulates `thinking_delta` chunks.
    /// Signature (if any) is set via [`BlockAccumulator::set_signature`].
    Thinking,
    /// Any other variant (`server_tool_use`, `connector_text`,
    /// `advisor_tool_result`). Accumulator stores nothing; `stop_block`
    /// returns `CompletedBlock::Skipped`.
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

/// One finished content block, ready to be appended to the assistant
/// message and (if `ToolUse`) dispatched as a tool call.
#[derive(Debug, Clone)]
pub enum CompletedBlock {
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
        /// Verbatim provider-issued tool-call id, preserved for egress replay.
        provider_id: Option<String>,
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
pub struct BlockAccumulator {
    blocks: HashMap<u32, BlockState>,
}

impl BlockAccumulator {
    /// Construct an empty accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
        }
    }

    /// Register a new block at `index` with the given kind. If `index`
    /// already exists, the previous entry is overwritten (mirrors
    /// claude.ts:1996-2070 which `contentBlocks[part.index] = { ... }`
    /// unconditionally).
    ///
    /// # Errors
    /// Never returns `Err` today; the `Result` shape is retained for
    /// future-proofing.
    pub fn start_block(&mut self, index: u32, kind: BlockKind) -> Result<(), StreamingError> {
        self.blocks.insert(
            index,
            BlockState {
                kind,
                text_buf: String::new(),
                json_buf: String::new(),
                signature: None,
            },
        );
        Ok(())
    }

    /// Append `text` to the `Text` or `Thinking` buffer of the block at
    /// `index`. Errors if no such block, or if the block is not
    /// text-shaped.
    ///
    /// # Errors
    /// `BlockNotFound` if `index` was never started; `TypeMismatch` if
    /// the block at `index` was started as a non-text variant.
    pub fn append_text(&mut self, index: u32, text: &str) -> Result<(), StreamingError> {
        let state = self
            .blocks
            .get_mut(&index)
            .ok_or(StreamingError::BlockNotFound { index })?;
        match &state.kind {
            BlockKind::Text | BlockKind::Thinking => {
                state.text_buf.push_str(text);
                Ok(())
            }
            other => Err(StreamingError::TypeMismatch {
                index,
                expected: other.name(),
                got: "text_delta",
            }),
        }
    }

    /// Append a `partial_json` chunk to the `ToolUse` buffer.
    ///
    /// # Errors
    /// `BlockNotFound` if `index` was never started; `TypeMismatch` if
    /// the block at `index` was started as a non-tool variant.
    pub fn append_json(&mut self, index: u32, partial: &str) -> Result<(), StreamingError> {
        let state = self
            .blocks
            .get_mut(&index)
            .ok_or(StreamingError::BlockNotFound { index })?;
        match &state.kind {
            BlockKind::ToolUse { .. } => {
                state.json_buf.push_str(partial);
                Ok(())
            }
            other => Err(StreamingError::TypeMismatch {
                index,
                expected: other.name(),
                got: "input_json_delta",
            }),
        }
    }

    /// Set the signature on a `Thinking` block (from a `signature_delta`).
    ///
    /// # Errors
    /// `BlockNotFound` if `index` was never started; `TypeMismatch` if
    /// the block at `index` was started as a non-thinking variant.
    pub fn set_signature(&mut self, index: u32, sig: &str) -> Result<(), StreamingError> {
        let state = self
            .blocks
            .get_mut(&index)
            .ok_or(StreamingError::BlockNotFound { index })?;
        match &state.kind {
            BlockKind::Thinking => {
                state.signature = Some(sig.to_string());
                Ok(())
            }
            other => Err(StreamingError::TypeMismatch {
                index,
                expected: other.name(),
                got: "signature_delta",
            }),
        }
    }

    /// Finalize the block at `index`, removing it from internal state
    /// and returning a `CompletedBlock`.
    ///
    /// # Errors
    /// `DoubleStop` if the block was already finalized (or never
    /// started). `ToolUseJsonParse` if a `ToolUse` block's accumulated
    /// `partial_json` buffer is not valid JSON.
    pub fn stop_block(&mut self, index: u32) -> Result<CompletedBlock, StreamingError> {
        let state = self
            .blocks
            .remove(&index)
            .ok_or(StreamingError::DoubleStop { index })?;
        let completed = match state.kind {
            BlockKind::Text => CompletedBlock::Text {
                text: state.text_buf,
            },
            BlockKind::Thinking => CompletedBlock::Thinking {
                thinking: state.text_buf,
                signature: state.signature,
            },
            BlockKind::ToolUse {
                id,
                name,
                provider_id,
            } => {
                let input = if state.json_buf.is_empty() {
                    Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str::<Value>(&state.json_buf).map_err(|e| {
                        StreamingError::ToolUseJsonParse {
                            index,
                            reason: e.to_string(),
                            buffer: state.json_buf.clone(),
                        }
                    })?
                };
                CompletedBlock::ToolUse {
                    id,
                    name,
                    input,
                    provider_id,
                }
            }
            BlockKind::Other => CompletedBlock::Skipped,
        };
        Ok(completed)
    }

    /// `true` when no blocks are currently in-flight.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.blocks.is_empty()
    }
}
