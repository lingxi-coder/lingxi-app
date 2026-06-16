//! Canonical protocol types and codec traits.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Capabilities, CostEstimate, LlmError, Usage};

/// Which on-wire streaming framing is used for this provider request.
///
/// Set by codecs before the request is sent; the bridge uses it to choose
/// between SSE splitting and raw binary framing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamFraming {
    /// Server-Sent Events (text, `data: …\n\n` boundaries).  This is the
    /// default for Anthropic, `OpenAI`, Gemini, and Azure.
    #[default]
    Sse,
    /// AWS binary event-stream framing used by Amazon Bedrock streaming
    /// responses.  The bridge passes raw byte chunks directly to the codec's
    /// [`crate::StreamDecoder`] without SSE parsing.
    AwsEventStream,
}

/// Canonical request passed to provider protocols.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmRequest {
    /// Requested model id or alias.
    pub model: String,
    /// Optional provider profile that disambiguates `model` when the same id is
    /// offered by multiple providers. `None` = resolve across all providers
    /// (ambiguous ids error).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Conversation messages, oldest first.
    pub messages: Vec<Message>,
    /// System prompt blocks, in order (empty = no system prompt).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<SystemBlock>,
    /// Tool declarations available to the model.
    pub tools: Vec<ToolDeclaration>,
    /// Optional tool-choice policy.
    pub tool_choice: Option<ToolChoice>,
    /// Optional structured-output request.
    pub response_format: Option<ResponseFormat>,
    /// Whether caller requested streaming.
    pub stream: bool,
    /// Optional maximum output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Optional sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Optional nucleus-sampling parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Sequences that end generation early.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    /// Optional reasoning/thinking budget request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
}

impl LlmRequest {
    /// Create an empty request for a model.
    #[must_use]
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Self::default()
        }
    }

    /// Pin the provider profile used to resolve `model`.
    #[must_use]
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    /// Add one user text message.
    #[must_use]
    pub fn with_user_text(mut self, text: impl Into<String>) -> Self {
        self.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text { text: text.into(), cache_control: None }],
        });
        self
    }

    /// Add an image block to the most recent user message, or start a new
    /// user message when the conversation does not end with one.
    #[must_use]
    pub fn with_image(mut self, media_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        let block = ContentBlock::Image {
            media_type: media_type.into(),
            bytes,
        };

        match self.messages.last_mut() {
            Some(message) if message.role == "user" => message.content.push(block),
            _ => self.messages.push(Message {
                role: "user".to_string(),
                content: vec![block],
            }),
        }

        self
    }
}

/// Provider-neutral reasoning/thinking budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningConfig {
    /// Maximum tokens the model may spend on reasoning.
    pub budget_tokens: u32,
}

/// One system-prompt block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemBlock {
    /// System text.
    pub text: String,
    /// Optional prompt-cache breakpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl SystemBlock {
    /// Create a plain system block without a cache breakpoint.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache_control: None,
        }
    }
}

/// Prompt-cache control marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheControl {
    /// Anthropic ephemeral cache breakpoint.
    Ephemeral,
}

/// Canonical chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Message role.
    pub role: String,
    /// Ordered content blocks.
    pub content: Vec<ContentBlock>,
}

/// Canonical content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text block.
    Text {
        /// Text payload.
        text: String,
        /// Optional prompt-cache breakpoint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Image block.
    Image {
        /// Image media type.
        media_type: String,
        /// Raw image bytes.
        bytes: Vec<u8>,
    },
    /// Image referenced by URL (Anthropic url image source).
    ImageUrl {
        /// Image URL.
        url: String,
    },
    /// Document block.
    Document {
        /// Document media type.
        media_type: String,
        /// Raw document bytes.
        bytes: Vec<u8>,
    },
    /// Tool-call block.
    ToolCall {
        /// Tool call id.
        id: String,
        /// Tool name.
        name: String,
        /// Tool input JSON.
        input: Value,
    },
    /// Tool-result block.
    ToolResult {
        /// Tool call id this result answers.
        tool_call_id: String,
        /// Tool result JSON.
        output: Value,
        /// Whether the result reports a tool failure.
        #[serde(default)]
        is_error: bool,
        /// Optional prompt-cache breakpoint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Reasoning block.
    Reasoning {
        /// Reasoning text or provider-supplied summary.
        text: String,
        /// Provider integrity signature required to round-trip the block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Opaque redacted-reasoning block that must round-trip unmodified.
    RedactedThinking {
        /// Provider-opaque payload.
        data: String,
    },
    /// Anthropic server-side tool invocation (e.g. advisor / `web_search`).
    ///
    /// Wire tag: `server_tool_use`. Mirrors `api-client::ContentBlockApi::ServerToolUse`
    /// exactly. Encode: round-trips back to `server_tool_use` (tool-use round-trip).
    ServerToolUse {
        /// Server-issued tool-use identifier.
        id: String,
        /// Name of the server tool being invoked.
        name: String,
        /// Tool input arguments (provider-specific JSON shape).
        #[serde(default)]
        input: Value,
    },
    /// Anthropic Connector-Text block.
    ///
    /// Wire tag: `connector_text`. Field name `connector_text` mirrors
    /// `api-client::ContentBlockApi::ConnectorText` exactly (NOT `text`).
    /// api-client decodes this unconditionally (no cfg gate) → llm-client
    /// also decodes it unconditionally. Encode: rejected with a message (no
    /// upstream use-case yet — mirrors Document handling).
    ConnectorText {
        /// Connector-emitted text payload.
        #[serde(default)]
        connector_text: String,
        /// Optional provider integrity signature.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Advisor tool result mirrored from the server.
    ///
    /// Wire tag: `advisor_tool_result`. Mirrors
    /// `api-client::ContentBlockApi::AdvisorToolResult` exactly.
    /// Encode: rejected with a message (no upstream use-case yet).
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

/// Canonical non-streaming response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmResponse {
    /// Provider response id.
    pub id: String,
    /// Model that produced the response.
    pub model: String,
    /// Output content blocks.
    pub content: Vec<ContentBlock>,
    /// Normalized terminal stop reason (Anthropic vocabulary: `end_turn`,
    /// `tool_use`, `max_tokens`, `stop_sequence`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Normalized usage.
    pub usage: Usage,
    /// Optional per-call cost estimate.
    pub cost: Option<CostEstimate>,
    /// Redacted provider metadata.
    #[serde(default)]
    pub provider_metadata: Value,
}

/// Canonical streaming event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmEvent {
    /// Response start snapshot.
    MessageStart {
        /// Response metadata snapshot.
        response: Box<LlmResponse>,
    },
    /// Content block start snapshot.
    ContentBlockStart {
        /// Block index in the response content list.
        index: u32,
        /// Content block snapshot at start.
        content_block: ContentBlock,
    },
    /// Incremental content block delta.
    ContentBlockDelta {
        /// Block index in the response content list.
        index: u32,
        /// Incremental delta payload.
        delta: ContentDelta,
    },
    /// Content block end marker.
    ContentBlockStop {
        /// Block index in the response content list.
        index: u32,
    },
    /// Terminal response delta.
    MessageDelta {
        /// Terminal response delta payload.
        delta: MessageDeltaPayload,
        /// Normalized usage at the terminal boundary.
        usage: Option<Usage>,
    },
    /// Terminal response stop marker.
    MessageStop,
    /// Final response event.
    Completed {
        /// Completed response.
        response: Box<LlmResponse>,
    },
}

/// Canonical content delta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentDelta {
    /// Text delta payload.
    TextDelta {
        /// Partial text.
        text: String,
    },
    /// Partial JSON payload.
    InputJsonDelta {
        /// Partial JSON text.
        partial_json: String,
    },
    /// Reasoning/thinking delta payload.
    ThinkingDelta {
        /// Thinking text.
        thinking: String,
    },
    /// Reasoning signature delta payload.
    SignatureDelta {
        /// Signature fragment for the open reasoning block.
        signature: String,
    },
    /// Append a citation reference to a `text` block.
    ///
    /// Wire tag: `citations_delta`. Field name `citation` mirrors
    /// `api-client::ContentDelta::CitationsDelta` exactly.
    CitationsDelta {
        /// Provider-specific citation payload (URL, title, range, etc.).
        citation: Value,
    },
    /// Append text to a `connector_text` block.
    ///
    /// Wire tag: `connector_text_delta`. Field name `connector_text` mirrors
    /// `api-client::ContentDelta::ConnectorTextDelta` exactly (NOT `text`).
    ConnectorTextDelta {
        /// Connector-text fragment to append.
        #[serde(default)]
        connector_text: String,
    },
}

/// Canonical terminal message delta payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageDeltaPayload {
    /// Optional terminal stop reason.
    pub stop_reason: Option<String>,
}

/// Canonical tool declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDeclaration {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON schema for tool input.
    pub input_schema: Value,
}

/// Tool-choice policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    /// Provider chooses whether to call tools.
    Auto,
    /// No tool calls are allowed.
    None,
    /// A tool call is required.
    Required,
    /// A specific tool must be called.
    Tool {
        /// Required tool name.
        name: String,
    },
}

/// Structured-output request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Provider-native JSON mode.
    JsonObject,
    /// Provider-native JSON schema mode.
    JsonSchema {
        /// JSON schema value.
        schema: Value,
    },
}

/// Provider-native request envelope with normalized single-value headers.
///
/// The `stream_framing` field is additive with `#[serde(default)]`: existing
/// serialised envelopes (and test literals that omit it) default to
/// [`StreamFraming::Sse`] without a compile error.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderRequest {
    /// HTTP method.
    pub method: String,
    /// Request URL.
    pub url: String,
    /// Normalized single-value request headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// JSON request body.
    pub body_json: Value,
    /// Which streaming framing protocol to use for this request.
    ///
    /// Defaults to [`StreamFraming::Sse`]; codecs that target the AWS
    /// event-stream binary protocol (e.g. Bedrock) set this to
    /// [`StreamFraming::AwsEventStream`] during `encode_request`.
    #[serde(default)]
    pub stream_framing: StreamFraming,
    /// Optional raw request body; takes precedence over `body_json` when set.
    ///
    /// Used by binary upload flows (e.g. the Gemini File API resumable
    /// protocol) where the request body is raw media bytes, not JSON. The
    /// transport bridge sends these bytes verbatim and suppresses the JSON
    /// body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_bytes: Option<Vec<u8>>,
}

impl ProviderRequest {
    /// Create a POST request with a JSON body.
    ///
    /// `stream_framing` defaults to [`StreamFraming::Sse`]; codecs that need
    /// AWS binary framing set `request.stream_framing = StreamFraming::AwsEventStream`
    /// after calling this constructor.
    #[must_use]
    pub fn post_json(url: impl Into<String>, body_json: Value) -> Self {
        Self {
            method: "POST".to_string(),
            url: url.into(),
            headers: BTreeMap::new(),
            body_json,
            stream_framing: StreamFraming::Sse,
            body_bytes: None,
        }
    }
}

/// Provider-native response envelope with normalized single-value headers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderResponse {
    /// HTTP status code.
    pub status: u16,
    /// Normalized single-value response headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// JSON response body.
    pub body_json: Value,
    /// Optional provider request id.
    pub request_id: Option<String>,
}

impl ProviderResponse {
    /// Create a JSON response envelope.
    #[must_use]
    pub fn json(status: u16, body_json: Value) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body_json,
            request_id: None,
        }
    }
}

/// Raw streaming frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawStreamFrame {
    /// Raw frame bytes.
    pub bytes: Vec<u8>,
}

impl RawStreamFrame {
    /// Create a raw stream frame.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

/// Provider wire codec.
pub trait WireCodec: std::fmt::Debug + Send + Sync {
    /// Encode a canonical request into a provider envelope.
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError>;
    /// Decode a provider envelope into a canonical response.
    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError>;
    /// Create a stream decoder for this codec.
    fn stream_decoder(&self) -> Box<dyn StreamDecoder>;
    #[allow(missing_docs)]
    fn clone_box(&self) -> Box<dyn WireCodec>;
}

impl Clone for Box<dyn WireCodec> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Stream decoder that emits no events.
#[derive(Debug, Default)]
pub struct NoopStreamDecoder;

impl StreamDecoder for NoopStreamDecoder {
    fn decode_frame(&mut self, _frame: RawStreamFrame) -> Result<Vec<LlmEvent>, LlmError> {
        Ok(Vec::new())
    }
}

/// Provider stream decoder.
pub trait StreamDecoder: std::fmt::Debug + Send {
    /// Decode one raw stream frame into zero or more canonical events.
    fn decode_frame(&mut self, frame: RawStreamFrame) -> Result<Vec<LlmEvent>, LlmError>;

    /// Finish the stream and emit any terminal events.
    fn finish(&mut self) -> Result<Vec<LlmEvent>, LlmError> {
        Ok(Vec::new())
    }
}

/// Validate request capabilities before transport I/O.
pub fn validate_capabilities(request: &LlmRequest, capabilities: Capabilities) -> Result<(), LlmError> {
    if request.stream && !capabilities.streaming {
        return Err(LlmError::UnsupportedCapability {
            capability: "streaming".to_string(),
        });
    }

    if (!request.tools.is_empty() || request.tool_choice.is_some()) && !capabilities.tools {
        return Err(LlmError::UnsupportedCapability {
            capability: "tools".to_string(),
        });
    }

    if request.reasoning.is_some() && !capabilities.reasoning {
        return Err(LlmError::UnsupportedCapability {
            capability: "reasoning".to_string(),
        });
    }

    if request.response_format.is_some() && !capabilities.structured_output {
        return Err(LlmError::UnsupportedCapability {
            capability: "structured_output".to_string(),
        });
    }

    for message in &request.messages {
        for block in &message.content {
            match block {
                ContentBlock::Image { .. } | ContentBlock::ImageUrl { .. } if !capabilities.vision => {
                    return Err(LlmError::UnsupportedCapability {
                        capability: "vision".to_string(),
                    });
                }
                ContentBlock::Document { .. } if !capabilities.documents => {
                    return Err(LlmError::UnsupportedCapability {
                        capability: "documents".to_string(),
                    });
                }
                ContentBlock::ToolCall { .. } | ContentBlock::ToolResult { .. }
                    if !capabilities.tools =>
                {
                    return Err(LlmError::UnsupportedCapability {
                        capability: "tools".to_string(),
                    });
                }
                ContentBlock::Reasoning { .. } | ContentBlock::RedactedThinking { .. }
                    if !capabilities.reasoning =>
                {
                    return Err(LlmError::UnsupportedCapability {
                        capability: "reasoning".to_string(),
                    });
                }
                _ => {}
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_profile_sets_field_and_new_defaults_none() {
        assert_eq!(LlmRequest::new("m").profile, None);
        assert_eq!(LlmRequest::new("m").with_profile("openai").profile.as_deref(), Some("openai"));
    }
}
