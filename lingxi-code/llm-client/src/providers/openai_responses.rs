//! `OpenAI` Responses API codec.
//!
//! Wire shapes are pinned to the vendored Codex CLI Rust reference
//! (`codex-rs/codex-api/src/common.rs` `ResponsesApiRequest` and
//! `codex-rs/protocol/src/models.rs` `ResponseItem`/`ContentItem`):
//! a flat `input` item list (`message` / `function_call` /
//! `function_call_output`), top-level `instructions`, flattened tool
//! declarations, and `text.format` structured-output controls.

use crate::{
    ContentBlock, LlmError, LlmEvent, LlmRequest, LlmResponse, ProviderRequest, ProviderResponse,
    RawStreamFrame, ResponseFormat, StreamDecoder, ToolChoice, ToolDeclaration, WireCodec,
};

use base64::Engine;
use serde_json::Value;

#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct OpenAiResponsesCodec {
    base_url: String,
}

impl OpenAiResponsesCodec {
    #[must_use]
    #[allow(missing_docs)]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    fn responses_url(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }
}

impl WireCodec for OpenAiResponsesCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        reject_unsupported_content_blocks(request)?;

        if !request.stop_sequences.is_empty() {
            // The Responses API request (codex common.rs ResponsesApiRequest)
            // carries no stop-sequence parameter; reject instead of dropping.
            return Err(LlmError::InvalidRequest {
                message: "OpenAiResponsesCodec does not support stop_sequences (the Responses API has no stop parameter)".to_string(),
            });
        }

        let mut input = Vec::new();
        for message in &request.messages {
            encode_message(message, &mut input);
        }

        let mut body = serde_json::Map::new();
        body.insert("model".to_string(), Value::String(request.model.clone()));

        if !request.system.is_empty() {
            let instructions = request
                .system
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            body.insert("instructions".to_string(), Value::String(instructions));
        }

        body.insert("input".to_string(), Value::Array(input));

        if !request.tools.is_empty() {
            body.insert(
                "tools".to_string(),
                Value::Array(request.tools.iter().map(encode_tool).collect()),
            );
        }

        if let Some(tool_choice) = &request.tool_choice {
            body.insert("tool_choice".to_string(), encode_tool_choice(tool_choice));
        }

        if let Some(max_tokens) = request.max_tokens {
            body.insert("max_output_tokens".to_string(), Value::from(max_tokens));
        }
        if let Some(temperature) = request.temperature {
            body.insert("temperature".to_string(), Value::from(temperature));
        }
        if let Some(top_p) = request.top_p {
            body.insert("top_p".to_string(), Value::from(top_p));
        }

        if let Some(reasoning) = &request.reasoning {
            body.insert(
                "reasoning".to_string(),
                serde_json::json!({"effort": map_reasoning_effort(reasoning.budget_tokens)}),
            );
        }

        if let Some(response_format) = &request.response_format {
            body.insert("text".to_string(), encode_text_controls(response_format));
        }

        if request.stream {
            body.insert("stream".to_string(), Value::Bool(true));
        }

        // Stateless parity: never let the provider persist the response
        // server-side; conversation state always lives with the caller.
        body.insert("store".to_string(), Value::Bool(false));

        let mut provider_request =
            ProviderRequest::post_json(self.responses_url(), Value::Object(body));
        provider_request
            .headers
            .insert("content-type".to_string(), "application/json".to_string());

        Ok(provider_request)
    }

    fn decode_response(&self, _response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        // Stub: implemented by batch-3 Task 2.
        Err(LlmError::InvalidRequest {
            message: "OpenAiResponsesCodec decode_response lands in batch-3 T2".to_string(),
        })
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(OpenAiResponsesStreamDecoder)
    }

    fn clone_box(&self) -> Box<dyn WireCodec> {
        Box::new(self.clone())
    }
}

/// Stub stream decoder: implemented by batch-3 Task 3.
#[derive(Debug)]
struct OpenAiResponsesStreamDecoder;

impl StreamDecoder for OpenAiResponsesStreamDecoder {
    fn decode_frame(&mut self, _frame: RawStreamFrame) -> Result<Vec<LlmEvent>, LlmError> {
        Err(LlmError::InvalidRequest {
            message: "OpenAiResponsesCodec stream decoding lands in batch-3 T3".to_string(),
        })
    }
}

/// Map a provider-neutral reasoning token budget onto the Responses API
/// discrete `reasoning.effort` levels.
///
/// LOSSY by design: the Responses API only accepts `low` / `medium` / `high`
/// (codex common.rs `Reasoning { effort }`), so the numeric budget collapses
/// into buckets: `budget_tokens <= 1024` → `"low"`, `<= 8192` → `"medium"`,
/// else `"high"`.
fn map_reasoning_effort(budget_tokens: u32) -> &'static str {
    match budget_tokens {
        0..=1024 => "low",
        1025..=8192 => "medium",
        _ => "high",
    }
}

fn encode_message(message: &crate::Message, input: &mut Vec<Value>) {
    let mut parts: Vec<Value> = Vec::new();
    let text_part_type = if message.role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };

    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } => {
                parts.push(serde_json::json!({"type": text_part_type, "text": text}));
            }
            ContentBlock::Image { media_type, bytes } => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                parts.push(serde_json::json!({
                    "type": "input_image",
                    "image_url": format!("data:{media_type};base64,{b64}"),
                }));
            }
            ContentBlock::ImageUrl { url } => {
                parts.push(serde_json::json!({
                    "type": "input_image",
                    "image_url": url,
                }));
            }
            ContentBlock::Document { media_type, bytes } => {
                // ContentBlock::Document carries no filename; use the stable
                // default "document" (mirrors the batch-2 Chat decision).
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                parts.push(serde_json::json!({
                    "type": "input_file",
                    "filename": "document",
                    "file_data": format!("data:{media_type};base64,{b64}"),
                }));
            }
            ContentBlock::ToolCall { id, name, input: tool_input } => {
                flush_message_item(&message.role, &mut parts, input);
                // The Responses API carries function-call arguments as a JSON
                // *string*, not a parsed object (codex models.rs FunctionCall).
                input.push(serde_json::json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": tool_input.to_string(),
                }));
            }
            // The Responses API output item carries no error flag; the error
            // text itself is the model-visible signal, so is_error and
            // cache_control are intentionally unused (mirrors OpenAiChatCodec).
            ContentBlock::ToolResult { tool_call_id, output, .. } => {
                flush_message_item(&message.role, &mut parts, input);
                input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": tool_call_id,
                    "output": tool_result_output(output),
                }));
            }
            ContentBlock::Reasoning { .. }
            | ContentBlock::RedactedThinking { .. }
            | ContentBlock::ServerToolUse { .. }
            | ContentBlock::ConnectorText { .. }
            | ContentBlock::AdvisorToolResult { .. } => {}
        }
    }

    flush_message_item(&message.role, &mut parts, input);
}

fn flush_message_item(role: &str, parts: &mut Vec<Value>, input: &mut Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    input.push(serde_json::json!({
        "type": "message",
        "role": role,
        "content": std::mem::take(parts),
    }));
}

fn tool_result_output(output: &Value) -> Value {
    match output {
        Value::String(text) => Value::String(text.clone()),
        other => Value::String(other.to_string()),
    }
}

fn encode_tool(tool: &ToolDeclaration) -> Value {
    // Flattened Responses tool shape (codex common.rs serializes tools as
    // flat JSON values) — NOT nested under "function" like the Chat API.
    serde_json::json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
        "strict": false,
    })
}

fn encode_tool_choice(tool_choice: &ToolChoice) -> Value {
    match tool_choice {
        ToolChoice::Auto => Value::String("auto".to_string()),
        ToolChoice::None => Value::String("none".to_string()),
        ToolChoice::Required => Value::String("required".to_string()),
        ToolChoice::Tool { name } => serde_json::json!({
            "type": "function",
            "name": name,
        }),
    }
}

fn encode_text_controls(response_format: &ResponseFormat) -> Value {
    match response_format {
        ResponseFormat::JsonObject => serde_json::json!({
            "format": {"type": "json_object"},
        }),
        // Shape pinned to codex common.rs `TextFormat {type, strict, schema, name}`;
        // strict/name mirror the OpenAiChatCodec json_schema encoding.
        ResponseFormat::JsonSchema { schema } => serde_json::json!({
            "format": {
                "type": "json_schema",
                "strict": true,
                "schema": schema,
                "name": "response",
            },
        }),
    }
}

fn reject_unsupported_content_blocks(request: &LlmRequest) -> Result<(), LlmError> {
    for message in &request.messages {
        for block in &message.content {
            match block {
                ContentBlock::Reasoning { .. } | ContentBlock::RedactedThinking { .. } => {
                    return Err(LlmError::InvalidRequest {
                        message: "OpenAiResponsesCodec does not encode reasoning blocks yet".to_string(),
                    });
                }
                ContentBlock::ServerToolUse { .. }
                | ContentBlock::ConnectorText { .. }
                | ContentBlock::AdvisorToolResult { .. } => {
                    return Err(LlmError::InvalidRequest {
                        message: "OpenAiResponsesCodec does not encode Anthropic server-generated blocks".to_string(),
                    });
                }
                _ => {}
            }
        }
    }

    Ok(())
}
