use crate::{
    ContentBlock, ContentDelta, LlmError, LlmEvent, LlmRequest, LlmResponse,
    MessageDeltaPayload, ProviderRequest, ProviderResponse, RawStreamFrame, ResponseFormat,
    StreamDecoder, ToolDeclaration, ToolChoice, Usage, WireCodec,
};

use std::collections::BTreeMap;

use base64::Engine;
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
        reject_unsupported_content_blocks(request)?;

        // chat-completions reasoning is driven by the model id (e.g. deepseek-reasoner,
        // GLM, openrouter o-series), not by a per-request budget field — there is no
        // wire slot for it here. So the resolved reasoning intent is intentionally not
        // serialized: we drop any `request.reasoning` budget gracefully rather than error.

        let mut messages = Vec::new();

        if !request.system.is_empty() {
            let text = request
                .system
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            messages.push(serde_json::json!({"role": "system", "content": text}));
        }

        messages.extend(request.messages.iter().flat_map(encode_message));

        let mut body = serde_json::Map::new();
        body.insert("model".to_string(), Value::String(request.model.clone()));
        body.insert("messages".to_string(), Value::Array(messages));

        if request.stream {
            body.insert("stream".to_string(), Value::Bool(true));
        }

        if let Some(max_tokens) = request.max_tokens {
            body.insert("max_tokens".to_string(), Value::from(max_tokens));
        }
        if let Some(temperature) = request.temperature {
            body.insert("temperature".to_string(), Value::from(temperature));
        }
        if let Some(top_p) = request.top_p {
            body.insert("top_p".to_string(), Value::from(top_p));
        }
        if !request.stop_sequences.is_empty() {
            body.insert(
                "stop".to_string(),
                Value::Array(request.stop_sequences.iter().cloned().map(Value::String).collect()),
            );
        }

        if let Some(response_format) = &request.response_format {
            body.insert("response_format".to_string(), encode_response_format(response_format));
        }

        if let Some(tool_choice) = &request.tool_choice {
            body.insert("tool_choice".to_string(), encode_tool_choice(tool_choice));
        }

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

        // OpenRouter's optional attribution headers (surfaces LingXi on their app
        // leaderboard). Gated strictly on the base_url substring so other providers
        // sharing this codec (deepseek/zai/github-copilot) are unaffected.
        if self.base_url.contains("openrouter.ai") {
            provider_request
                .headers
                .insert("HTTP-Referer".to_string(), "https://lingxi.dev".to_string());
            provider_request
                .headers
                .insert("X-Title".to_string(), "LingXi-Code".to_string());
        }

        Ok(provider_request)
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        if response.status >= 400 {
            return Err(decode_error_response(&response));
        }
        decode_response_body(response.body_json)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(OpenAiStreamDecoder::default())
    }

    fn clone_box(&self) -> Box<dyn WireCodec> {
        Box::new(self.clone())
    }
}

/// Decode the `OpenAI` `{error: {message, code, type}}` envelope into the error
/// taxonomy, falling back to [`super::map_error_status`].
///
/// `pub(crate)` so [`super::OpenAiResponsesCodec`] can reuse it: the Responses
/// API shares the Chat API error envelope.
pub(crate) fn decode_error_response(response: &ProviderResponse) -> LlmError {
    let retry_after = crate::retry::retry_after_from_headers(&response.headers);
    let error = response.body_json.get("error");
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let code = error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .or_else(|| error.and_then(|error| error.get("type")).and_then(Value::as_str))
        .unwrap_or_default();

    match code {
        "insufficient_quota" => LlmError::QuotaExceeded,
        "context_length_exceeded" => LlmError::ContextOverflow { token_gap: 0 },
        "invalid_api_key" | "invalid_authentication" => LlmError::Authentication,
        "model_not_found" => LlmError::ModelUnavailable,
        _ => super::map_error_status(response.status, message, retry_after),
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
            stop_reason: None,
            stop_details: None,
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
                    signature: None,
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
                    cache_control: None,
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
        // Some OpenAI-compatible servers omit `index` when there is a single
        // tool call; treat that as slot zero instead of dropping the fragment.
        let openai_index = tool_call
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);

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
                stop_details: None,
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
    let mut media_parts: Vec<Value> = Vec::new();
    let mut tool_calls = Vec::new();
    let mut messages = Vec::new();
    let mut has_media = false;

    for block in &message.content {
        match block {
            ContentBlock::Text { text: block_text, .. } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(block_text);
            }
            ContentBlock::Image { media_type, bytes } => {
                has_media = true;
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                let url = format!("data:{media_type};base64,{b64}");
                media_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {"url": url},
                }));
            }
            ContentBlock::ImageUrl { url } => {
                has_media = true;
                media_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {"url": url},
                }));
            }
            ContentBlock::ToolCall { id, name, input } => tool_calls.push(serde_json::json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": input.to_string(),
                }
            })),
            // OpenAI tool messages carry no error flag; the error text itself
            // is the model-visible signal, so is_error and cache_control are intentionally unused.
            ContentBlock::ToolResult { tool_call_id, output, .. } => {
                if !text.is_empty() || !media_parts.is_empty() {
                    messages.push(build_user_message(&message.role, &text, &media_parts, has_media));
                    text.clear();
                    media_parts.clear();
                    has_media = false;
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
            ContentBlock::Document { media_type, bytes } => {
                // OpenAI Responses API / file-enabled chat supports a "file" content part.
                // ContentBlock::Document carries no filename; use the stable default "document".
                // The data URI format is: data:<media_type>;base64,<b64>.
                has_media = true;
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                let file_data = format!("data:{media_type};base64,{b64}");
                media_parts.push(serde_json::json!({
                    "type": "file",
                    "file": {
                        "filename": "document",
                        "file_data": file_data,
                    },
                }));
            }
            // Reasoning / RedactedThinking are skipped so they are omitted from the
            // outgoing `messages`: chat-completions APIs reject reasoning_content as
            // input, so a Reasoning block carried over from a prior turn's history must
            // not be serialized back.
            ContentBlock::Reasoning { .. }
            | ContentBlock::RedactedThinking { .. }
            | ContentBlock::ServerToolUse { .. }
            | ContentBlock::ConnectorText { .. }
            | ContentBlock::AdvisorToolResult { .. }
            // cache_edits is an Anthropic-1P request directive only; never OpenAI.
            | ContentBlock::CacheEdits { .. } => {}
        }
    }

    if !text.is_empty() || !media_parts.is_empty() {
        messages.push(build_user_message(&message.role, &text, &media_parts, has_media));
    }
    if !tool_calls.is_empty() {
        messages.push(assistant_tool_call_message(&message.role, &tool_calls));
    }
    messages
}

/// Build a user/assistant message, using array form only when media is present.
///
/// Wire-stability invariant: text-only messages keep the plain-string form
/// (`"content": "..."`) so existing downstream consumers are unaffected.
fn build_user_message(role: &str, text: &str, media_parts: &[Value], has_media: bool) -> Value {
    if !has_media {
        // Plain-string form — preserves existing wire pins.
        return text_message(role, text);
    }
    // Array form: text parts first, then media parts.
    let mut parts: Vec<Value> = Vec::new();
    if !text.is_empty() {
        parts.push(serde_json::json!({"type": "text", "text": text}));
    }
    parts.extend_from_slice(media_parts);
    serde_json::json!({
        "role": role,
        "content": parts,
    })
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

fn encode_response_format(response_format: &ResponseFormat) -> Value {
    match response_format {
        ResponseFormat::JsonObject => serde_json::json!({"type": "json_object"}),
        ResponseFormat::JsonSchema { schema } => serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "response",
                "strict": true,
                "schema": schema,
            }
        }),
    }
}

fn encode_tool_choice(tool_choice: &ToolChoice) -> Value {
    match tool_choice {
        ToolChoice::Auto => Value::String("auto".to_string()),
        ToolChoice::None => Value::String("none".to_string()),
        ToolChoice::Required => Value::String("required".to_string()),
        ToolChoice::Tool { name } => serde_json::json!({
            "type": "function",
            "function": {"name": name},
        }),
    }
}

fn reject_unsupported_content_blocks(request: &LlmRequest) -> Result<(), LlmError> {
    for message in &request.messages {
        for block in &message.content {
            match block {
                // Image, ImageUrl, and Document are now encoded as content parts.
                // Reasoning / RedactedThinking are intentionally NOT rejected: the stream
                // decoder emits Reasoning blocks into history, and chat-completions APIs
                // reject reasoning_content as input (deepseek docs say not to send it back),
                // so encode_message simply skips them. Erroring here would break turn 2+.
                ContentBlock::ServerToolUse { .. }
                | ContentBlock::ConnectorText { .. }
                | ContentBlock::AdvisorToolResult { .. } => {
                    return Err(LlmError::InvalidRequest {
                        message: "OpenAiChatCodec does not encode Anthropic server-generated blocks".to_string(),
                    });
                }
                _ => {}
            }
        }
    }

    Ok(())
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

    // Symmetric with the streaming decoder's handle_reasoning: deepseek-reasoner / GLM /
    // openrouter o-series report chain-of-thought in `message.reasoning_content`. Emit it
    // as a Reasoning block (no signature on the chat-completions wire) before text/tools.
    if let Some(reasoning) = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|reasoning| !reasoning.is_empty())
    {
        content.push(ContentBlock::Reasoning {
            text: reasoning.to_string(),
            signature: None,
        });
    }

    match message.get("content") {
        Some(Value::String(text)) if !text.is_empty() => {
            content.push(ContentBlock::Text {
                text: text.clone(),
                cache_control: None,
            });
        }
        Some(Value::Array(parts)) => {
            for part in parts {
                // Only emit text parts; unknown/refusal/image parts are skipped
                // (tolerant decode per OpenAI's extensible content-part schema).
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            content.push(ContentBlock::Text {
                                text: text.to_string(),
                                cache_control: None,
                            });
                        }
                    }
                }
            }
        }
        _ => {}
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

    let usage = body_json.get("usage").map(normalize_usage).unwrap_or_default();
    let stop_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(map_finish_reason);

    Ok(LlmResponse {
        id,
        model,
        content,
        stop_reason,
        stop_details: None,
        usage,
        cost: None,
        provider_metadata: body_json,
    })
}

fn normalize_usage(usage: &Value) -> Usage {
    let prompt_tokens = usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let completion_tokens = usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
    // OpenAI reports cached/reasoning tokens as subsets of prompt/completion
    // counts; subtract them so every TokenUsage bucket stays independently billable.
    let cached_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("completion_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    Usage {
        billable_tokens: crate::TokenUsage {
            input: prompt_tokens.saturating_sub(cached_tokens),
            output: completion_tokens.saturating_sub(reasoning_tokens),
            cache_read: cached_tokens,
            reasoning_output: reasoning_tokens,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Message, ReasoningConfig};

    fn body_of(request: &ProviderRequest) -> &Value {
        &request.body_json
    }

    /// (a) A request carrying a reasoning budget no longer errors, and the
    /// chat-completions body has no reasoning/budget field (there is no wire slot).
    #[test]
    fn reasoning_budget_is_dropped_not_rejected() {
        let mut request = LlmRequest::new("deepseek-reasoner");
        request.reasoning = Some(ReasoningConfig::Enabled { budget_tokens: 4096 });
        request.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
                cache_control: None,
            }],
        });

        let codec = OpenAiChatCodec::new("https://api.deepseek.com");
        let provider_request = codec.encode_request(&request).expect("reasoning budget must not error");

        let body = body_of(&provider_request).as_object().expect("body is an object");
        assert!(!body.contains_key("reasoning"), "no reasoning field on the wire");
        assert!(!body.contains_key("reasoning_content"), "no reasoning_content field");
        assert!(!body.contains_key("budget_tokens"), "no budget_tokens field");
    }

    /// (b) History containing a Reasoning block (emitted by the stream decoder on a
    /// prior turn) encodes successfully and is omitted from the outgoing messages.
    #[test]
    fn reasoning_block_in_history_is_omitted_not_rejected() {
        let mut request = LlmRequest::new("deepseek-reasoner");
        request.messages.push(Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Reasoning {
                    text: "let me think".to_string(),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "the answer is 42".to_string(),
                    cache_control: None,
                },
            ],
        });

        let codec = OpenAiChatCodec::new("https://api.deepseek.com");
        let provider_request = codec.encode_request(&request).expect("reasoning block must not error");

        let messages = body_of(&provider_request)
            .get("messages")
            .and_then(Value::as_array)
            .expect("messages array");
        // The assistant message keeps its text but never serializes the reasoning.
        let serialized = serde_json::to_string(messages).expect("serialize messages");
        assert!(serialized.contains("the answer is 42"), "text survives");
        assert!(!serialized.contains("let me think"), "reasoning is omitted");
        assert!(!serialized.contains("reasoning_content"), "no reasoning_content key");
    }

    /// (c) Non-streaming decode of a message carrying `reasoning_content` yields a
    /// Reasoning content block ahead of the text block (symmetric with streaming).
    #[test]
    fn decode_emits_reasoning_block_from_reasoning_content() {
        let body = serde_json::json!({
            "id": "chatcmpl-1",
            "model": "deepseek-reasoner",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "reasoning_content": "step-by-step thoughts",
                    "content": "final answer"
                },
                "finish_reason": "stop"
            }]
        });

        let response = decode_response_body(body).expect("decode succeeds");
        assert!(matches!(
            response.content.first(),
            Some(ContentBlock::Reasoning { text, signature: None }) if text == "step-by-step thoughts"
        ));
        assert!(matches!(
            response.content.get(1),
            Some(ContentBlock::Text { text, .. }) if text == "final answer"
        ));
    }

    /// (d) An openrouter base_url adds the attribution headers; other providers
    /// sharing this codec do not.
    #[test]
    fn openrouter_base_url_adds_attribution_headers() {
        let mut request = LlmRequest::new("openrouter/auto");
        request.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
                cache_control: None,
            }],
        });

        let openrouter = OpenAiChatCodec::new("https://openrouter.ai/api/v1");
        let with_attribution = openrouter.encode_request(&request).expect("encode");
        assert_eq!(
            with_attribution.headers.get("HTTP-Referer").map(String::as_str),
            Some("https://lingxi.dev")
        );
        assert_eq!(
            with_attribution.headers.get("X-Title").map(String::as_str),
            Some("LingXi-Code")
        );

        let deepseek = OpenAiChatCodec::new("https://api.deepseek.com");
        let without_attribution = deepseek.encode_request(&request).expect("encode");
        assert!(!without_attribution.headers.contains_key("HTTP-Referer"));
        assert!(!without_attribution.headers.contains_key("X-Title"));
    }
}
