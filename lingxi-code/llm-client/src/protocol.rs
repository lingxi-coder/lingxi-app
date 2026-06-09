//! Canonical protocol types and codec traits.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Capabilities, CostEstimate, LlmError, Usage};

/// Canonical request passed to provider protocols.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmRequest {
    /// Requested model id or alias.
    pub model: String,
    /// Conversation messages, oldest first.
    pub messages: Vec<Message>,
    /// Optional top-level system prompt.
    pub system: Option<String>,
    /// Tool declarations available to the model.
    pub tools: Vec<ToolDeclaration>,
    /// Optional tool-choice policy.
    pub tool_choice: Option<ToolChoice>,
    /// Optional structured-output request.
    pub response_format: Option<ResponseFormat>,
    /// Whether caller requested streaming.
    pub stream: bool,
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

    /// Add one user text message.
    #[must_use]
    pub fn with_user_text(mut self, text: impl Into<String>) -> Self {
        self.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text { text: text.into() }],
        });
        self
    }

    /// Add an image block to the most recent user message.
    #[must_use]
    pub fn with_image(mut self, media_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        let block = ContentBlock::Image {
            media_type: media_type.into(),
            bytes,
        };

        if let Some(message) = self.messages.last_mut() {
            message.content.push(block);
        } else {
            self.messages.push(Message {
                role: "user".to_string(),
                content: vec![block],
            });
        }

        self
    }
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
    },
    /// Image block.
    Image {
        /// Image media type.
        media_type: String,
        /// Raw image bytes.
        bytes: Vec<u8>,
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
    },
    /// Reasoning block.
    Reasoning {
        /// Reasoning text or provider-supplied summary.
        text: String,
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
    /// Text delta event.
    TextDelta {
        /// Delta text payload.
        text: String,
    },
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
        delta: MessageDelta,
        /// Normalized usage at the terminal boundary.
        usage: Option<Usage>,
    },
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
}

/// Canonical terminal message delta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageDelta {
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

/// Encoded provider request body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PreparedBody {
    /// JSON request body.
    Json(Value),
    /// Raw bytes request body.
    Bytes(Vec<u8>),
}

/// Raw provider response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawResponse {
    /// HTTP status code.
    pub status: u16,
    /// Raw body bytes.
    pub body: Vec<u8>,
    /// Optional provider request id.
    pub request_id: Option<String>,
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

/// Provider protocol codec.
pub trait Protocol: std::fmt::Debug + Send + Sync {
    /// Encode a canonical request into provider body bytes or JSON.
    fn encode(&self, request: &LlmRequest) -> Result<PreparedBody, LlmError>;
    /// Decode a raw response into a canonical response.
    fn decode_response(&self, response: RawResponse) -> Result<LlmResponse, LlmError>;
    /// Create a stream decoder for this protocol.
    fn stream_decoder(&self) -> Box<dyn StreamDecoder>;
}

/// Provider stream decoder.
pub trait StreamDecoder: std::fmt::Debug + Send {
    /// Decode one raw stream frame into zero or more canonical events.
    fn decode_frame(&mut self, frame: RawStreamFrame) -> Result<Vec<LlmEvent>, LlmError>;
}

/// Validate request capabilities before transport I/O.
pub fn validate_capabilities(request: &LlmRequest, capabilities: Capabilities) -> Result<(), LlmError> {
    if request.stream && !capabilities.streaming {
        return Err(LlmError::UnsupportedCapability {
            capability: "streaming".to_string(),
        });
    }

    for message in &request.messages {
        for block in &message.content {
            match block {
                ContentBlock::Image { .. } if !capabilities.vision => {
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
                ContentBlock::Reasoning { .. } if !capabilities.reasoning => {
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
