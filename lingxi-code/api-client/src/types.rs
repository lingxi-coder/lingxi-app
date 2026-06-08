//! API-shape DTOs. Provider-neutral where possible; Anthropic-specific
//! fields are flagged in their docs.

use protocol::{ConversationMessage, ToolUseId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Request body for the `messages` endpoint.
///
/// Mirrors the Anthropic Messages API shape, but stays provider-neutral where
/// possible. The `tools` field is intentionally `Vec<Value>` until Plan 2/3
/// introduces a typed tool schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRequest {
    /// Model identifier (e.g. `claude-sonnet-4-20250514`).
    pub model: String,
    /// Maximum number of output tokens the server is allowed to produce.
    pub max_tokens: u32,
    /// Conversation history, oldest first.
    pub messages: Vec<ConversationMessage>,
    /// Optional top-level system prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Tool schema declarations. Shape is provider-specific (Anthropic /
    /// `OpenAI`), so we keep it opaque here.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<Value>,
    /// Optional sampling temperature in `[0.0, 1.0]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

/// Extended-thinking config emitted on the request body's `thinking` field.
///
/// 1:1 with claude-code `BetaMessageStreamParams['thinking']`
/// (`claude.ts:1599-1630`): either an *adaptive* block (newer models that
/// support adaptive thinking — no budget) or an *enabled* block carrying an
/// explicit `budget_tokens`. The `disabled` arm of the TS union is intentionally
/// NOT modelled: when thinking is off, claude-code OMITS the `thinking` key
/// entirely, so the Rust port represents "off" as `Option::None` rather than a
/// serialized variant. This keeps non-thinking requests byte-identical.
///
/// Wire shape (matches the Anthropic Messages API exactly):
/// * adaptive → `{"type":"adaptive"}`
/// * enabled  → `{"type":"enabled","budget_tokens":N}`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingApi {
    /// Adaptive thinking — models that support it run without a fixed budget
    /// (claude-code `{ type: 'adaptive' }`, `claude.ts:1611-1613`).
    Adaptive,
    /// Budgeted thinking — `budget_tokens` reasoning tokens
    /// (claude-code `{ budget_tokens, type: 'enabled' }`, `claude.ts:1625-1628`).
    Enabled {
        /// Reasoning-token budget. The request builder clamps this to
        /// `max_tokens - 1` before serializing, mirroring
        /// `Math.min(maxOutputTokens - 1, thinkingBudget)` (`claude.ts:1624`).
        budget_tokens: u32,
    },
}

impl ThinkingApi {
    /// Serialize to the exact Anthropic `thinking` wire object, clamping an
    /// enabled budget to `max_tokens - 1` so the request always leaves room for
    /// at least one output token beyond the reasoning budget (claude-code
    /// `claude.ts:1624`). For [`ThinkingApi::Adaptive`] `max_tokens` is unused
    /// (the adaptive block carries no budget).
    #[must_use]
    pub fn to_wire(self, max_tokens: u32) -> Value {
        match self {
            Self::Adaptive => serde_json::json!({ "type": "adaptive" }),
            Self::Enabled { budget_tokens } => {
                // Math.min(maxOutputTokens - 1, thinkingBudget). `saturating_sub`
                // guards the (pathological) max_tokens == 0 case.
                let clamped = budget_tokens.min(max_tokens.saturating_sub(1));
                serde_json::json!({ "type": "enabled", "budget_tokens": clamped })
            }
        }
    }
}

/// Non-streaming response body from the `messages` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageResponse {
    /// Server-assigned message id.
    pub id: String,
    /// Model that actually produced the response (may differ from request).
    pub model: String,
    /// Output content blocks in order.
    pub content: Vec<ContentBlockApi>,
    /// Reason the model stopped generating (e.g. `end_turn`, `tool_use`).
    pub stop_reason: Option<String>,
    /// Token usage and cache statistics.
    pub usage: UsageApi,
}

/// Wire-format content block emitted by the API.
///
/// Distinct from `protocol::ContentBlock` because the API shape carries
/// raw `serde_json::Value` tool input and Anthropic-specific thinking blocks.
///
/// Mirrors claude-code's `content_block` matrix in
/// `services/api/claude.ts:1995-2295`. All new fields are
/// `#[serde(default)]` so a future server-added field never breaks
/// deserialization of an existing variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockApi {
    /// Plain text output.
    Text {
        /// The text payload.
        text: String,
    },
    /// Tool invocation requested by the model.
    ToolUse {
        /// Provider-issued tool use identifier.
        id: ToolUseId,
        /// Tool name being invoked.
        name: String,
        /// Tool input arguments (provider-specific JSON shape).
        input: Value,
    },
    /// Extended thinking block (Anthropic-specific).
    Thinking {
        /// Free-form chain-of-thought text.
        thinking: String,
        /// Optional signature used to verify the thinking block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Anthropic server-side tool invocation (e.g. advisor).
    ServerToolUse {
        /// Server-issued tool use identifier.
        id: String,
        /// Tool name being invoked.
        name: String,
        /// Tool input arguments (provider-specific JSON shape).
        #[serde(default)]
        input: Value,
    },
    /// Anthropic Connector-Text block (gated by `CONNECTOR_TEXT` feature flag
    /// in claude-code; we always accept it on the wire).
    ConnectorText {
        /// Connector-emitted text payload.
        #[serde(default)]
        connector_text: String,
        /// Optional signature used to verify the connector block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Advisor tool result, mirrored from the server.
    AdvisorToolResult {
        /// Identifier of the originating `server_tool_use` block.
        tool_use_id: String,
        /// Tool result content (provider-specific JSON shape).
        #[serde(default)]
        content: Value,
        /// Whether the tool reported an error.
        #[serde(default)]
        is_error: bool,
    },
}

/// Server-side tool counters reported in `usage.server_tool_use`.
///
/// Anthropic-specific. Mirrors claude-code's
/// `usage.server_tool_use.web_search_requests` (`services/api/claude.ts:2947`).
/// `#[serde(default)]` on the field means a missing or partial
/// `server_tool_use` object still deserializes (count defaults to `0`).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ServerToolUseApi {
    /// Number of server-side web search requests billed for this call.
    #[serde(default)]
    pub web_search_requests: u64,
}

/// Token usage and cache statistics for one API call.
///
/// Anthropic-specific: `cache_creation_input_tokens` and
/// `cache_read_input_tokens` map to prompt-caching counters; both default to
/// `0` when the provider omits them.
// NOTE: not `Copy` — the `speed: Option<String>` field is heap-backed. Callers
// `.clone()` where they previously relied on an implicit copy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageApi {
    /// Number of input tokens billed.
    pub input_tokens: u64,
    /// Number of output tokens billed.
    pub output_tokens: u64,
    /// Input tokens used to create a fresh cache entry (Anthropic-specific).
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    /// Input tokens served from cache (Anthropic-specific).
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// Server-side tool counters (web search, etc.). Anthropic-specific;
    /// absent on providers that do not bill server tools. Mirrors
    /// claude-code's `usage.server_tool_use` (`services/api/claude.ts:2947`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_tool_use: Option<ServerToolUseApi>,
    /// API speed tier actually used for this request (`"fast"` for the
    /// priority/low-latency tier, otherwise the standard tier or absent).
    /// Mirrors claude-code's `BetaUsage.speed` (`services/api/claude.ts:2985`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
}

/// Streaming event emitted on the SSE channel from the `messages` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Initial event carrying the message envelope.
    MessageStart {
        /// The (partially populated) message metadata and usage.
        message: MessageResponse,
    },
    /// Start of a new content block within the current message.
    ContentBlockStart {
        /// Zero-based index of the block within the message.
        index: u32,
        /// Initial block payload (may be empty for `text` blocks).
        content_block: ContentBlockApi,
    },
    /// Incremental update applied to the current content block.
    ContentBlockDelta {
        /// Zero-based index of the block being updated.
        index: u32,
        /// The delta to apply.
        delta: ContentDelta,
    },
    /// Marks the end of a content block.
    ContentBlockStop {
        /// Zero-based index of the block that just finished.
        index: u32,
    },
    /// Message-level metadata update (typically stop reason + final usage).
    MessageDelta {
        /// Stop reason and other top-level fields.
        delta: MessageDeltaPayload,
        /// Final usage snapshot, when the provider supplies one.
        usage: Option<UsageApi>,
    },
    /// Terminal event marking the end of the stream.
    MessageStop,
    /// SSE keepalive ping; safe to ignore.
    Ping,
    /// Server-emitted error event.
    Error {
        /// The error payload.
        error: ErrorPayload,
    },
}

/// Incremental update applied to a content block.
///
/// Mirrors claude-code's `content_block_delta` matrix in
/// `services/api/claude.ts:1995-2295`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentDelta {
    /// Append plain text to a `text` block.
    TextDelta {
        /// Text fragment to append.
        text: String,
    },
    /// Append a partial JSON fragment to a `tool_use` block's `input`.
    InputJsonDelta {
        /// Partial JSON string fragment.
        partial_json: String,
    },
    /// Append text to a `thinking` block.
    ThinkingDelta {
        /// Thinking-text fragment to append.
        thinking: String,
    },
    /// Set or update the signature of a `thinking` block.
    SignatureDelta {
        /// Signature value, finalising the thinking block.
        signature: String,
    },
    /// Append a citation reference to a `text` block.
    CitationsDelta {
        /// Provider-specific citation payload (URL, title, range, etc.).
        citation: Value,
    },
    /// Append text to a `connector_text` block.
    ConnectorTextDelta {
        /// Connector-text fragment to append.
        connector_text: String,
    },
}

/// Top-level fields carried by `message_delta` events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDeltaPayload {
    /// Reason the model stopped generating, when known at delta time.
    pub stop_reason: Option<String>,
}

/// Provider error payload nested inside a `StreamEvent::Error` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    /// Provider error type (e.g. `overloaded_error`, `invalid_request_error`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Human-readable error message.
    pub message: String,
}

#[cfg(test)]
mod stream_event_v2_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn thinking_content_block_roundtrip() {
        let raw = json!({
            "type": "thinking",
            "thinking": "let me think...",
            "signature": "sig-abc123"
        });
        let block: ContentBlockApi = serde_json::from_value(raw.clone()).expect("decode");
        match &block {
            ContentBlockApi::Thinking {
                thinking,
                signature,
            } => {
                assert_eq!(thinking, "let me think...");
                assert_eq!(signature.as_deref(), Some("sig-abc123"));
            }
            _ => panic!("expected Thinking"),
        }
        let re_encoded = serde_json::to_value(&block).expect("encode");
        assert_eq!(re_encoded, raw);
    }

    #[test]
    fn server_tool_use_content_block_decodes() {
        let raw = json!({
            "type": "server_tool_use",
            "id": "stu_01",
            "name": "advisor",
            "input": {"query": "?"}
        });
        let block: ContentBlockApi = serde_json::from_value(raw).expect("decode");
        assert!(matches!(block, ContentBlockApi::ServerToolUse { .. }));
    }

    #[test]
    fn connector_text_block_decodes() {
        let raw = json!({
            "type": "connector_text",
            "connector_text": "[connector] hi",
            "signature": "ct-sig"
        });
        let block: ContentBlockApi = serde_json::from_value(raw).expect("decode");
        assert!(matches!(block, ContentBlockApi::ConnectorText { .. }));
    }

    #[test]
    fn advisor_tool_result_block_decodes() {
        let raw = json!({
            "type": "advisor_tool_result",
            "tool_use_id": "stu_01",
            "content": "result",
            "is_error": false
        });
        let block: ContentBlockApi = serde_json::from_value(raw).expect("decode");
        assert!(matches!(block, ContentBlockApi::AdvisorToolResult { .. }));
    }

    #[test]
    fn signature_delta_decodes() {
        let raw = json!({"type": "signature_delta", "signature": "sig"});
        let d: ContentDelta = serde_json::from_value(raw).expect("decode");
        assert!(matches!(d, ContentDelta::SignatureDelta { .. }));
    }

    #[test]
    fn citations_delta_decodes_with_arbitrary_citation_shape() {
        let raw = json!({
            "type": "citations_delta",
            "citation": {"url": "https://x", "title": "X"}
        });
        let d: ContentDelta = serde_json::from_value(raw).expect("decode");
        assert!(matches!(d, ContentDelta::CitationsDelta { .. }));
    }

    #[test]
    fn connector_text_delta_decodes() {
        let raw = json!({
            "type": "connector_text_delta",
            "connector_text": " more"
        });
        let d: ContentDelta = serde_json::from_value(raw).expect("decode");
        assert!(matches!(d, ContentDelta::ConnectorTextDelta { .. }));
    }

    #[test]
    fn usage_decodes_server_tool_use_and_speed() {
        // A response usage carrying web_search_requests + speed "fast" — the
        // two cost-side billing signals (COST.5 / COST.3) — round-trips via
        // serde without any manual parsing.
        let raw = json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "server_tool_use": { "web_search_requests": 3 },
            "speed": "fast"
        });
        let u: UsageApi = serde_json::from_value(raw).expect("decode");
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
        assert_eq!(u.server_tool_use.map(|s| s.web_search_requests), Some(3));
        assert_eq!(u.speed.as_deref(), Some("fast"));
    }

    #[test]
    fn usage_absent_server_tool_use_and_speed_default_to_none() {
        // Existing wire shape (no server_tool_use / speed) still deserializes;
        // both new fields default to None — no regression.
        let raw = json!({ "input_tokens": 10, "output_tokens": 5 });
        let u: UsageApi = serde_json::from_value(raw).expect("decode");
        assert!(u.server_tool_use.is_none());
        assert!(u.speed.is_none());
    }

    #[test]
    fn usage_partial_server_tool_use_defaults_request_count() {
        // An empty server_tool_use object decodes with the count defaulting to 0.
        let raw = json!({
            "input_tokens": 1,
            "output_tokens": 1,
            "server_tool_use": {}
        });
        let u: UsageApi = serde_json::from_value(raw).expect("decode");
        assert_eq!(u.server_tool_use.map(|s| s.web_search_requests), Some(0));
    }
}
