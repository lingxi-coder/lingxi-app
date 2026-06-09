use crate::{ContentBlock, LlmError, LlmRequest, NoopStreamDecoder, ProviderRequest, ProviderResponse, StreamDecoder, ToolDeclaration, WireCodec};

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

    fn decode_response(&self, _response: ProviderResponse) -> Result<crate::LlmResponse, LlmError> {
        Err(LlmError::InvalidRequest {
            message: "Anthropic response decoding is not implemented yet".to_string(),
        })
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(NoopStreamDecoder)
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
