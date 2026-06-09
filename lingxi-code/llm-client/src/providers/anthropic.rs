use crate::{
    normalize_anthropic_usage, ContentBlock, ContentDelta, LlmError, LlmEvent, LlmRequest,
    LlmResponse, MessageDeltaPayload, ProviderRequest, ProviderResponse,
    RawStreamFrame, StreamDecoder, ToolDeclaration, WireCodec,
};

use base64::Engine;
use serde_json::Value;

#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct AnthropicMessagesCodec {
    base_url: String,
    anthropic_version: String,
}

impl AnthropicMessagesCodec {
    #[must_use]
    #[allow(missing_docs)]
    pub fn new(base_url: impl Into<String>, anthropic_version: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            anthropic_version: anthropic_version.into(),
        }
    }

    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }
}

impl WireCodec for AnthropicMessagesCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        if request.response_format.is_some() {
            return Err(LlmError::InvalidRequest {
                message: "AnthropicMessagesCodec does not encode response_format yet".to_string(),
            });
        }

        let messages = request
            .messages
            .iter()
            .map(encode_message)
            .collect::<Result<Vec<_>, _>>()?;
        let tools = request.tools.iter().map(encode_tool).collect::<Vec<_>>();

        let mut body = serde_json::Map::new();
        body.insert("model".to_string(), Value::String(request.model.clone()));
        body.insert("max_tokens".to_string(), Value::from(4096u64));
        body.insert("messages".to_string(), Value::Array(messages));
        body.insert("tools".to_string(), Value::Array(tools));

        if let Some(system) = &request.system {
            body.insert("system".to_string(), Value::String(system.clone()));
        }

        if let Some(tool_choice) = &request.tool_choice {
            body.insert("tool_choice".to_string(), encode_tool_choice(tool_choice));
        }

        let mut provider_request = ProviderRequest::post_json(self.messages_url(), Value::Object(body));
        provider_request
            .headers
            .insert("anthropic-version".to_string(), self.anthropic_version.clone());
        provider_request
            .headers
            .insert("content-type".to_string(), "application/json".to_string());

        Ok(provider_request)
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        decode_response_body(response.body_json)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(AnthropicStreamDecoder)
    }

    fn clone_box(&self) -> Box<dyn WireCodec> {
        Box::new(self.clone())
    }
}

#[derive(Debug, Default)]
struct AnthropicStreamDecoder;

impl StreamDecoder for AnthropicStreamDecoder {
    fn decode_frame(&mut self, frame: RawStreamFrame) -> Result<Vec<LlmEvent>, LlmError> {
        let text = std::str::from_utf8(&frame.bytes).map_err(|_| LlmError::InvalidRequest {
            message: "Anthropic stream frame is not valid UTF-8".to_string(),
        })?;

        let value: Value = serde_json::from_str(text).map_err(|_| LlmError::InvalidRequest {
            message: "Anthropic stream frame is not valid JSON".to_string(),
        })?;

        decode_stream_event(&value)
    }
}

fn encode_message(message: &crate::Message) -> Result<Value, LlmError> {
    let content = message
        .content
        .iter()
        .map(encode_content_block)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(serde_json::json!({
        "role": message.role,
        "content": content,
    }))
}

fn encode_content_block(block: &ContentBlock) -> Result<Value, LlmError> {
    match block {
        ContentBlock::Text { text } => Ok(serde_json::json!({
            "type": "text",
            "text": text,
        })),
        ContentBlock::Image { media_type, bytes } => Ok(serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            },
        })),
        ContentBlock::ToolCall { id, name, input } => Ok(serde_json::json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        })),
        ContentBlock::ToolResult { tool_call_id, output } => Ok(serde_json::json!({
            "type": "tool_result",
            "tool_use_id": tool_call_id,
            "content": normalize_tool_result_content(output),
        })),
        ContentBlock::Document { .. } | ContentBlock::Reasoning { .. } => Err(LlmError::InvalidRequest {
            message: "AnthropicMessagesCodec does not encode document or reasoning blocks yet".to_string(),
        }),
    }
}

fn encode_tool_choice(tool_choice: &crate::ToolChoice) -> Value {
    match tool_choice {
        crate::ToolChoice::Auto => serde_json::json!({"type": "auto"}),
        crate::ToolChoice::None => serde_json::json!({"type": "none"}),
        crate::ToolChoice::Required => serde_json::json!({"type": "any"}),
        crate::ToolChoice::Tool { name } => serde_json::json!({"type": "tool", "name": name}),
    }
}

fn normalize_tool_result_content(output: &Value) -> Value {
    match output {
        Value::String(text) => Value::String(text.clone()),
        other => Value::String(other.to_string()),
    }
}

fn encode_tool(tool: &ToolDeclaration) -> Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.input_schema,
    })
}

fn decode_response_body(body_json: Value) -> Result<LlmResponse, LlmError> {
    let id = string_field(&body_json, "id")?;
    let model = string_field(&body_json, "model")?;
    let content = body_json
        .get("content")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(decode_content_block).collect::<Result<Vec<_>, _>>())
        .transpose()?
        .unwrap_or_default();
    let usage = body_json
        .get("usage")
        .map(normalize_anthropic_usage)
        .unwrap_or_default();

    Ok(LlmResponse {
        id,
        model,
        content,
        usage,
        cost: None,
        provider_metadata: body_json,
    })
}

fn decode_content_block(value: &Value) -> Result<ContentBlock, LlmError> {
    match value.get("type").and_then(Value::as_str) {
        Some("text") => Ok(ContentBlock::Text {
            text: string_field(value, "text")?,
        }),
        Some("thinking") => Ok(ContentBlock::Reasoning {
            text: string_field(value, "thinking")?,
        }),
        Some("tool_use") => Ok(ContentBlock::ToolCall {
            id: string_field(value, "id")?,
            name: string_field(value, "name")?,
            input: value.get("input").cloned().unwrap_or(Value::Null),
        }),
        Some(other) => Err(LlmError::InvalidRequest {
            message: format!("unsupported Anthropic content block type: {other}"),
        }),
        None => Err(LlmError::InvalidRequest {
            message: "Anthropic content block missing type".to_string(),
        }),
    }
}

fn decode_stream_event(value: &Value) -> Result<Vec<LlmEvent>, LlmError> {
    match value.get("type").and_then(Value::as_str) {
        Some("message_start") => Ok(vec![LlmEvent::MessageStart {
            response: Box::new(decode_message_start(value)?),
        }]),
        Some("content_block_start") => Ok(vec![LlmEvent::ContentBlockStart {
            index: u32_field(value, "index")?,
            content_block: decode_content_block(value.get("content_block").ok_or_else(|| LlmError::InvalidRequest {
                message: "Anthropic content_block_start missing content_block".to_string(),
            })?)?,
        }]),
        Some("content_block_delta") => Ok(vec![LlmEvent::ContentBlockDelta {
            index: u32_field(value, "index")?,
            delta: decode_content_delta(value.get("delta").ok_or_else(|| LlmError::InvalidRequest {
                message: "Anthropic content_block_delta missing delta".to_string(),
            })?)?,
        }]),
        Some("content_block_stop") => Ok(vec![LlmEvent::ContentBlockStop {
            index: u32_field(value, "index")?,
        }]),
        Some("message_delta") => Ok(vec![LlmEvent::MessageDelta {
            delta: MessageDeltaPayload {
                stop_reason: value
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            },
            usage: value.get("usage").map(normalize_anthropic_usage),
        }]),
        Some("message_stop") => Ok(vec![LlmEvent::MessageStop]),
        Some("ping") => Ok(Vec::new()),
        Some("error") => Err(LlmError::ProviderInternal),
        Some(other) => Err(LlmError::InvalidRequest {
            message: format!("unsupported Anthropic stream event type: {other}"),
        }),
        None => Err(LlmError::InvalidRequest {
            message: "Anthropic stream frame missing type".to_string(),
        }),
    }
}

fn decode_message_start(value: &Value) -> Result<LlmResponse, LlmError> {
    let message = value.get("message").unwrap_or(value);
    decode_response_body(message.clone())
}

fn decode_content_delta(value: &Value) -> Result<ContentDelta, LlmError> {
    match value.get("type").and_then(Value::as_str) {
        Some("text_delta") => Ok(ContentDelta::TextDelta {
            text: string_field(value, "text")?,
        }),
        Some("input_json_delta") => Ok(ContentDelta::InputJsonDelta {
            partial_json: string_field(value, "partial_json")?,
        }),
        Some("thinking_delta") => Ok(ContentDelta::ThinkingDelta {
            thinking: string_field(value, "thinking")?,
        }),
        Some(other) => Err(LlmError::InvalidRequest {
            message: format!("unsupported Anthropic content delta type: {other}"),
        }),
        None => Err(LlmError::InvalidRequest {
            message: "Anthropic content delta missing type".to_string(),
        }),
    }
}

fn string_field(value: &Value, field: &str) -> Result<String, LlmError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| LlmError::InvalidRequest {
            message: format!("Anthropic payload missing string field: {field}"),
        })
}

fn u32_field(value: &Value, field: &str) -> Result<u32, LlmError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| LlmError::InvalidRequest {
            message: format!("Anthropic payload missing u32 field: {field}"),
        })
}
