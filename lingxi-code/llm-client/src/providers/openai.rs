use crate::{
    ContentBlock, ContentDelta, LlmError, LlmEvent, LlmRequest, LlmResponse,
    MessageDeltaPayload, ProviderRequest, ProviderResponse, RawStreamFrame, StreamDecoder,
    ToolDeclaration, Usage, WireCodec,
};

use std::collections::BTreeMap;

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
        Box::new(OpenAiStreamDecoder::default())
    }
}

#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
struct OpenAiStreamDecoder {
    started: bool,
    reasoning_open: bool,
    reasoning_index: u32,
    text_open: bool,
    text_index: u32,
    next_index: u32,
    tool_index: BTreeMap<u64, u32>,
    stop_reason: Option<String>,
    usage: Option<Usage>,
    done: bool,
}

impl StreamDecoder for OpenAiStreamDecoder {
    fn decode_frame(&mut self, frame: RawStreamFrame) -> Result<Vec<LlmEvent>, LlmError> {
        let text = std::str::from_utf8(&frame.bytes).map_err(|_| LlmError::InvalidRequest {
            message: "OpenAI stream frame is not valid UTF-8".to_string(),
        })?;

        let mut out = Vec::new();
        let data = text.trim();
        if data == "[DONE]" {
            self.finish_into(&mut out);
            return Ok(out);
        }

        let root: serde_json::Value = serde_json::from_str(data).map_err(|_| LlmError::InvalidRequest {
            message: "OpenAI stream frame is not valid JSON".to_string(),
        })?;

        self.ensure_started(&root, &mut out);

        if let Some(usage) = root.get("usage").filter(|value| !value.is_null()) {
            self.usage = Some(normalize_usage(usage));
        }

        if let Some(choice) = root.get("choices").and_then(|choices| choices.as_array()).and_then(|choices| choices.first()) {
            if let Some(delta) = choice.get("delta") {
                if let Some(reasoning) = delta.get("reasoning_content").and_then(serde_json::Value::as_str) {
                    self.handle_reasoning(reasoning, &mut out);
                }
                if let Some(content) = delta.get("content").and_then(serde_json::Value::as_str) {
                    self.handle_text(content, &mut out);
                }
                if let Some(tool_calls) = delta.get("tool_calls").and_then(serde_json::Value::as_array) {
                    for tool_call in tool_calls {
                        self.handle_tool_fragment(tool_call, &mut out);
                    }
                }
            }
            if let Some(finish_reason) = choice.get("finish_reason").and_then(serde_json::Value::as_str) {
                self.stop_reason = Some(map_finish_reason(finish_reason));
            }
        }

        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<LlmEvent>, LlmError> {
        let mut out = Vec::new();
        self.finish_into(&mut out);
        Ok(out)
    }
}

impl OpenAiStreamDecoder {
    fn ensure_started(&mut self, root: &serde_json::Value, out: &mut Vec<LlmEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        let response = LlmResponse {
            id: root.get("id").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
            model: root.get("model").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
            content: Vec::new(),
            usage: Usage::default(),
            cost: None,
            provider_metadata: serde_json::Value::Null,
        };
        out.push(LlmEvent::MessageStart { response: Box::new(response) });
    }

    fn handle_reasoning(&mut self, reasoning: &str, out: &mut Vec<LlmEvent>) {
        if reasoning.is_empty() {
            return;
        }
        if !self.reasoning_open {
            self.reasoning_open = true;
            self.reasoning_index = self.next_index;
            self.next_index += 1;
            out.push(LlmEvent::ContentBlockStart {
                index: self.reasoning_index,
                content_block: ContentBlock::Reasoning {
                    text: String::new(),
                },
            });
        }
        out.push(LlmEvent::ContentBlockDelta {
            index: self.reasoning_index,
            delta: ContentDelta::ThinkingDelta {
                thinking: reasoning.to_string(),
            },
        });
    }

    fn handle_text(&mut self, text: &str, out: &mut Vec<LlmEvent>) {
        if text.is_empty() {
            return;
        }
        if !self.text_open {
            self.text_open = true;
            self.text_index = self.next_index;
            self.next_index += 1;
            out.push(LlmEvent::ContentBlockStart {
                index: self.text_index,
                content_block: ContentBlock::Text {
                    text: String::new(),
                },
            });
        }
        out.push(LlmEvent::ContentBlockDelta {
            index: self.text_index,
            delta: ContentDelta::TextDelta {
                text: text.to_string(),
            },
        });
    }

    fn handle_tool_fragment(&mut self, tool_call: &serde_json::Value, out: &mut Vec<LlmEvent>) {
        let Some(openai_index) = tool_call.get("index").and_then(serde_json::Value::as_u64) else {
            return;
        };

        let index = *self.tool_index.entry(openai_index).or_insert_with(|| {
            let index = self.next_index;
            self.next_index += 1;
            let id = tool_call
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = tool_call
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            out.push(LlmEvent::ContentBlockStart {
                index,
                content_block: ContentBlock::ToolCall {
                    id,
                    name,
                    input: serde_json::Value::Object(serde_json::Map::new()),
                },
            });
            index
        });

        if let Some(arguments) = tool_call
            .get("function")
            .and_then(|function| function.get("arguments"))
            .and_then(serde_json::Value::as_str)
            .filter(|arguments| !arguments.is_empty())
        {
            out.push(LlmEvent::ContentBlockDelta {
                index,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: arguments.to_string(),
                },
            });
        }
    }

    fn finish_into(&mut self, out: &mut Vec<LlmEvent>) {
        if !self.started || self.done {
            return;
        }
        self.done = true;
        self.close_open_blocks(out);
        out.push(LlmEvent::MessageDelta {
            delta: MessageDeltaPayload {
                stop_reason: self.stop_reason.clone(),
            },
            usage: self.usage.clone(),
        });
        out.push(LlmEvent::MessageStop);
    }

    fn close_open_blocks(&mut self, out: &mut Vec<LlmEvent>) {
        if self.reasoning_open {
            out.push(LlmEvent::ContentBlockStop {
                index: self.reasoning_index,
            });
            self.reasoning_open = false;
        }
        if self.text_open {
            out.push(LlmEvent::ContentBlockStop {
                index: self.text_index,
            });
            self.text_open = false;
        }
        let mut tool_indices: Vec<u32> = self.tool_index.values().copied().collect();
        tool_indices.sort_unstable();
        for index in tool_indices {
            out.push(LlmEvent::ContentBlockStop { index });
        }
        self.tool_index.clear();
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

fn normalize_usage(usage: &Value) -> Usage {
    Usage {
        billable_tokens: crate::TokenUsage {
            input: usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
            output: usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
            reasoning_output: usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            ..Default::default()
        },
        context_tokens: usage.get("total_tokens").and_then(Value::as_u64),
        provider_reported_total_tokens: usage.get("total_tokens").and_then(Value::as_u64),
        provider_metadata: usage.clone(),
        ..Default::default()
    }
}

fn map_finish_reason(finish_reason: &str) -> String {
    match finish_reason {
        "stop" => "end_turn".to_string(),
        "length" => "max_tokens".to_string(),
        "tool_calls" => "tool_use".to_string(),
        other => other.to_string(),
    }
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
