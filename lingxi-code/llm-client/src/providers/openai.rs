use crate::{ContentBlock, LlmError, LlmRequest, LlmResponse, NoopStreamDecoder, ProviderRequest, ProviderResponse, StreamDecoder, ToolDeclaration, WireCodec};

use serde_json::Value;

#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct OpenAiChatCodec {
    base_url: String,
}

impl OpenAiChatCodec {
    #[must_use]
    #[allow(missing_docs)]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

impl WireCodec for OpenAiChatCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        let mut messages = Vec::new();

        if let Some(system) = &request.system {
            messages.push(serde_json::json!({"role": "system", "content": system}));
        }

        messages.extend(request.messages.iter().flat_map(encode_message));

        let mut body = serde_json::Map::new();
        body.insert("model".to_string(), Value::String(request.model.clone()));
        body.insert("messages".to_string(), Value::Array(messages));

        if !request.tools.is_empty() {
            body.insert(
                "tools".to_string(),
                Value::Array(request.tools.iter().map(encode_tool).collect()),
            );
        }

        let mut provider_request = ProviderRequest::post_json(self.chat_completions_url(), Value::Object(body));
        provider_request
            .headers
            .insert("content-type".to_string(), "application/json".to_string());

        Ok(provider_request)
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        decode_response_body(response.body_json)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(NoopStreamDecoder)
    }
}

fn encode_message(message: &crate::Message) -> Vec<Value> {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut messages = Vec::new();

    for block in &message.content {
        match block {
            ContentBlock::Text { text: block_text } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(block_text);
            }
            ContentBlock::ToolCall { id, name, input } => tool_calls.push(serde_json::json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": input.to_string(),
                }
            })),
            ContentBlock::ToolResult { tool_call_id, output } => {
                if !text.is_empty() {
                    messages.push(text_message(&message.role, &text));
                    text.clear();
                }
                if !tool_calls.is_empty() {
                    messages.push(assistant_tool_call_message(&message.role, &tool_calls));
                    tool_calls.clear();
                }
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "content": tool_result_content(output),
                }));
            }
            ContentBlock::Image { .. } | ContentBlock::Document { .. } | ContentBlock::Reasoning { .. } => {}
        }
    }

    if !text.is_empty() {
        messages.push(text_message(&message.role, &text));
    }
    if !tool_calls.is_empty() {
        messages.push(assistant_tool_call_message(&message.role, &tool_calls));
    }
    messages
}

fn text_message(role: &str, text: &str) -> Value {
    serde_json::json!({
        "role": role,
        "content": text,
    })
}

fn assistant_tool_call_message(role: &str, tool_calls: &[Value]) -> Value {
    let mut message_json = serde_json::Map::new();
    message_json.insert("role".to_string(), Value::String(role.to_string()));
    message_json.insert("content".to_string(), Value::Null);
    message_json.insert("tool_calls".to_string(), Value::Array(tool_calls.to_vec()));
    Value::Object(message_json)
}

fn tool_result_content(output: &Value) -> Value {
    match output {
        Value::String(text) => Value::String(text.clone()),
        other => Value::String(other.to_string()),
    }
}

fn encode_tool(tool: &ToolDeclaration) -> Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

fn decode_response_body(body_json: Value) -> Result<LlmResponse, LlmError> {
    let id = string_field(&body_json, "id")?;
    let model = string_field(&body_json, "model")?;
    let choice = body_json
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| LlmError::InvalidRequest {
            message: "OpenAI response missing choices[0]".to_string(),
        })?;
    let message = choice
        .get("message")
        .ok_or_else(|| LlmError::InvalidRequest {
            message: "OpenAI response missing choices[0].message".to_string(),
        })?;

    let mut content = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            content.push(ContentBlock::Text {
                text: text.to_string(),
            });
        }
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            let id = string_field(tool_call, "id")?;
            let function = tool_call.get("function").ok_or_else(|| LlmError::InvalidRequest {
                message: "OpenAI tool call missing function".to_string(),
            })?;
            let name = string_field(function, "name")?;
            let arguments = function.get("arguments").and_then(Value::as_str).ok_or_else(|| LlmError::InvalidRequest {
                message: "OpenAI tool call missing function.arguments".to_string(),
            })?;
            let input = serde_json::from_str::<Value>(arguments).map_err(|_| LlmError::InvalidRequest {
                message: "OpenAI tool call has invalid function.arguments JSON".to_string(),
            })?;
            content.push(ContentBlock::ToolCall { id, name, input });
        }
    }

    let usage = body_json
        .get("usage")
        .map(|usage| crate::Usage {
            billable_tokens: crate::TokenUsage {
                input: usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
                output: usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
                ..Default::default()
            },
            provider_metadata: usage.clone(),
            ..Default::default()
        })
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

fn string_field(value: &Value, field: &str) -> Result<String, LlmError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| LlmError::InvalidRequest {
            message: format!("OpenAI response missing {field}"),
        })
}
