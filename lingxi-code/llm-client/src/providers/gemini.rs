use crate::{
    ContentBlock, ContentDelta, LlmError, LlmEvent, LlmRequest, LlmResponse,
    MessageDeltaPayload, ProviderRequest, ProviderResponse, RawStreamFrame,
    StreamDecoder, ToolDeclaration, Usage, WireCodec,
};

use serde_json::Value;

#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct GeminiCodec {
    base_url: String,
}

impl GeminiCodec {
    #[must_use]
    #[allow(missing_docs)]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    fn generate_content_url(&self, model: &str, stream: bool) -> String {
        let base_url = self.base_url.trim_end_matches('/');
        if stream {
            format!("{base_url}/models/{model}:streamGenerateContent?alt=sse")
        } else {
            format!("{base_url}/models/{model}:generateContent")
        }
    }
}

impl WireCodec for GeminiCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        reject_unsupported_request_intent(request)?;

        let tool_call_names = build_tool_call_name_map(&request.messages);
        let mut body = serde_json::Map::new();
        body.insert(
            "contents".to_string(),
            Value::Array(encode_messages(&request.messages, &tool_call_names)?),
        );

        if !request.system.is_empty() {
            let parts: Vec<Value> = request
                .system
                .iter()
                .map(|block| serde_json::json!({"text": block.text}))
                .collect();
            body.insert("systemInstruction".to_string(), serde_json::json!({"parts": parts}));
        }

        if !request.tools.is_empty() {
            body.insert("tools".to_string(), serde_json::json!([{ "functionDeclarations": request.tools.iter().map(encode_tool).collect::<Vec<_>>() }]));
        }

        let mut generation_config = serde_json::Map::new();
        if let Some(max_tokens) = request.max_tokens {
            generation_config.insert("maxOutputTokens".to_string(), Value::from(max_tokens));
        }
        if let Some(temperature) = request.temperature {
            generation_config.insert("temperature".to_string(), Value::from(temperature));
        }
        if let Some(top_p) = request.top_p {
            generation_config.insert("topP".to_string(), Value::from(top_p));
        }
        if !request.stop_sequences.is_empty() {
            generation_config.insert(
                "stopSequences".to_string(),
                Value::Array(request.stop_sequences.iter().cloned().map(Value::String).collect()),
            );
        }
        if let Some(reasoning) = &request.reasoning {
            generation_config.insert(
                "thinkingConfig".to_string(),
                serde_json::json!({"thinkingBudget": reasoning.budget_tokens}),
            );
        }
        if !generation_config.is_empty() {
            body.insert("generationConfig".to_string(), Value::Object(generation_config));
        }

        let mut provider_request = ProviderRequest::post_json(
            self.generate_content_url(&request.model, request.stream),
            Value::Object(body),
        );
        provider_request
            .headers
            .insert("content-type".to_string(), "application/json".to_string());

        Ok(provider_request)
    }

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        if response.status >= 400 {
            return Err(decode_error_response(&response));
        }
        decode_response_body(response.body_json)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(GeminiStreamDecoder::default())
    }

    fn clone_box(&self) -> Box<dyn WireCodec> {
        Box::new(self.clone())
    }
}

fn decode_error_response(response: &ProviderResponse) -> LlmError {
    let retry_after = crate::retry::retry_after_from_headers(&response.headers);
    let error = response.body_json.get("error");
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let google_status = error
        .and_then(|error| error.get("status"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    match google_status {
        "UNAUTHENTICATED" => LlmError::Authentication,
        "PERMISSION_DENIED" => LlmError::PermissionDenied,
        "NOT_FOUND" => LlmError::ModelUnavailable,
        "RESOURCE_EXHAUSTED" => LlmError::RateLimited {
            retry_after,
            scope: None,
        },
        "INVALID_ARGUMENT" | "FAILED_PRECONDITION" => LlmError::InvalidRequest { message },
        _ => super::map_error_status(response.status, message, retry_after),
    }
}

#[derive(Debug, Default)]
struct GeminiStreamDecoder {
    started: bool,
    done: bool,
    next_index: u32,
    text_index: Option<u32>,
    thinking_index: Option<u32>,
    usage: Option<Usage>,
    stop_reason: Option<String>,
    saw_function_call: bool,
}

impl StreamDecoder for GeminiStreamDecoder {
    fn decode_frame(&mut self, frame: RawStreamFrame) -> Result<Vec<LlmEvent>, LlmError> {
        let text = std::str::from_utf8(&frame.bytes).map_err(|_| LlmError::InvalidRequest {
            message: "Gemini stream frame is not valid UTF-8".to_string(),
        })?;

        let root: Value = serde_json::from_str(text.trim()).map_err(|_| LlmError::InvalidRequest {
            message: "Gemini stream frame is not valid JSON".to_string(),
        })?;

        let mut out = Vec::new();
        // Usage can arrive on a frame without candidates (e.g. the final
        // usage-only chunk), so capture it before the candidate guard.
        if let Some(usage) = root.get("usageMetadata") {
            self.usage = Some(decode_usage(usage));
        }

        let Some(candidate) = root
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
        else {
            return Ok(out);
        };

        if !self.started {
            self.started = true;
            out.push(LlmEvent::MessageStart {
                response: Box::new(decode_stream_start(&root)),
            });
        }

        if let Some(finish_reason) = candidate.get("finishReason").and_then(Value::as_str) {
            self.stop_reason = Some(map_finish_reason(finish_reason));
        }

        if let Some(parts) = candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    self.handle_text(text, part.get("thought").and_then(Value::as_bool).unwrap_or(false), &mut out);
                    continue;
                }

                if let Some(function_call) = part.get("functionCall") {
                    self.handle_function_call(function_call, &mut out)?;
                }
            }
        }

        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<LlmEvent>, LlmError> {
        let mut out = Vec::new();
        if !self.started || self.done {
            return Ok(out);
        }

        self.done = true;
        if let Some(index) = self.thinking_index.take() {
            out.push(LlmEvent::ContentBlockStop { index });
        }
        if let Some(index) = self.text_index.take() {
            out.push(LlmEvent::ContentBlockStop { index });
        }

        let stop_reason = if self.saw_function_call {
            Some("tool_use".to_string())
        } else {
            self.stop_reason.clone()
        };

        out.push(LlmEvent::MessageDelta {
            delta: MessageDeltaPayload { stop_reason },
            usage: self.usage.clone(),
        });
        out.push(LlmEvent::MessageStop);
        Ok(out)
    }
}

fn decode_stream_start(root: &Value) -> LlmResponse {
    LlmResponse {
        id: root
            .get("responseId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        model: root
            .get("modelVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        content: Vec::new(),
        stop_reason: None,
        usage: root.get("usageMetadata").map(decode_usage).unwrap_or_default(),
        cost: None,
        provider_metadata: Value::Null,
    }
}

fn decode_usage(value: &Value) -> Usage {
    let prompt_tokens = value
        .get("promptTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    // Gemini reports cached tokens as a subset of promptTokenCount; subtract
    // them so every TokenUsage bucket stays independently billable.
    let cached_tokens = value
        .get("cachedContentTokenCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    Usage {
        billable_tokens: crate::TokenUsage {
            input: prompt_tokens.saturating_sub(cached_tokens),
            output: value
                .get("candidatesTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read: cached_tokens,
            reasoning_output: value
                .get("thoughtsTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            ..Default::default()
        },
        context_tokens: value.get("totalTokenCount").and_then(Value::as_u64),
        provider_reported_total_tokens: value.get("totalTokenCount").and_then(Value::as_u64),
        provider_metadata: value.clone(),
        ..Default::default()
    }
}

fn map_finish_reason(finish_reason: &str) -> String {
    match finish_reason {
        "STOP" | "stop" => "end_turn".to_string(),
        "MAX_TOKENS" | "length" => "max_tokens".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

impl GeminiStreamDecoder {
    fn handle_text(&mut self, text: &str, thought: bool, out: &mut Vec<LlmEvent>) {
        if text.is_empty() {
            return;
        }

        if thought {
            let index = *self.thinking_index.get_or_insert_with(|| {
                let index = self.next_index;
                self.next_index += 1;
                out.push(LlmEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlock::Reasoning {
                        text: String::new(),
                        signature: None,
                    },
                });
                index
            });

            out.push(LlmEvent::ContentBlockDelta {
                index,
                delta: ContentDelta::ThinkingDelta {
                    thinking: text.to_string(),
                },
            });
            return;
        }

        let index = *self.text_index.get_or_insert_with(|| {
            let index = self.next_index;
            self.next_index += 1;
            out.push(LlmEvent::ContentBlockStart {
                index,
                content_block: ContentBlock::Text { text: String::new(), cache_control: None },
            });
            index
        });

        out.push(LlmEvent::ContentBlockDelta {
            index,
            delta: ContentDelta::TextDelta {
                text: text.to_string(),
            },
        });
    }

    fn handle_function_call(&mut self, function_call: &Value, out: &mut Vec<LlmEvent>) -> Result<(), LlmError> {
        self.saw_function_call = true;

        let name = function_call
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let input = function_call.get("args").cloned().unwrap_or_else(|| Value::Object(serde_json::Map::new()));

        let index = self.next_index;
        self.next_index += 1;
        out.push(LlmEvent::ContentBlockStart {
            index,
            content_block: ContentBlock::ToolCall {
                // Gemini does not assign tool-call ids; synthesize unique ones
                // so tool results can reference their call after a round trip.
                id: format!("call_{index}"),
                name,
                input: Value::Object(serde_json::Map::new()),
            },
        });
        out.push(LlmEvent::ContentBlockDelta {
            index,
            delta: ContentDelta::InputJsonDelta {
                partial_json: serde_json::to_string(&input).map_err(|_| LlmError::InvalidRequest {
                    message: "Gemini functionCall args are not serializable".to_string(),
                })?,
            },
        });
        out.push(LlmEvent::ContentBlockStop { index });
        Ok(())
    }
}

fn build_tool_call_name_map(messages: &[crate::Message]) -> std::collections::BTreeMap<String, String> {
    let mut tool_call_names = std::collections::BTreeMap::new();

    for message in messages {
        for block in &message.content {
            if let ContentBlock::ToolCall { id, name, .. } = block {
                tool_call_names.entry(id.clone()).or_insert_with(|| name.clone());
            }
        }
    }

    tool_call_names
}

fn encode_messages(messages: &[crate::Message], tool_call_names: &std::collections::BTreeMap<String, String>) -> Result<Vec<Value>, LlmError> {
    let mut out = Vec::new();

    for message in messages {
        let role = match message.role.as_str() {
            "assistant" => "model",
            other => other,
        };

        let mut parts = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } => parts.push(serde_json::json!({"text": text})),
                ContentBlock::ToolCall { name, input, .. } => parts.push(serde_json::json!({
                    "functionCall": {
                        "name": name,
                        "args": input,
                    }
                })),
                ContentBlock::ToolResult { tool_call_id, output, is_error, .. } => {
                    let Some(name) = tool_call_names.get(tool_call_id) else {
                        return Err(LlmError::InvalidRequest {
                            message: format!("tool result references unknown tool call id: {tool_call_id}"),
                        });
                    };
                    let response = if *is_error {
                        serde_json::json!({"error": output})
                    } else {
                        serde_json::json!({"result": output})
                    };
                    parts.push(serde_json::json!({
                        "functionResponse": {
                            "name": name,
                            "response": response,
                        }
                    }));
                }
                ContentBlock::Image { .. }
                | ContentBlock::ImageUrl { .. }
                | ContentBlock::Document { .. }
                | ContentBlock::Reasoning { .. }
                | ContentBlock::RedactedThinking { .. }
                | ContentBlock::ServerToolUse { .. }
                | ContentBlock::ConnectorText { .. }
                | ContentBlock::AdvisorToolResult { .. } => {}
            }
        }

        if !parts.is_empty() {
            out.push(serde_json::json!({"role": role, "parts": parts}));
        }
    }

    Ok(out)
}

fn encode_tool(tool: &ToolDeclaration) -> Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
    })
}

fn reject_unsupported_request_intent(request: &LlmRequest) -> Result<(), LlmError> {
    if request.response_format.is_some() {
        return Err(LlmError::InvalidRequest {
            message: "GeminiCodec does not encode response_format yet".to_string(),
        });
    }

    if request.tool_choice.is_some() {
        return Err(LlmError::InvalidRequest {
            message: "GeminiCodec does not encode tool_choice yet".to_string(),
        });
    }

    for message in &request.messages {
        for block in &message.content {
            match block {
                ContentBlock::Image { .. } | ContentBlock::ImageUrl { .. } => {
                    return Err(LlmError::InvalidRequest {
                        message: "GeminiCodec does not encode image blocks yet".to_string(),
                    });
                }
                ContentBlock::Document { .. } => {
                    return Err(LlmError::InvalidRequest {
                        message: "GeminiCodec does not encode document blocks yet".to_string(),
                    });
                }
                ContentBlock::Reasoning { .. } | ContentBlock::RedactedThinking { .. } => {
                    return Err(LlmError::InvalidRequest {
                        message: "GeminiCodec does not encode reasoning blocks yet".to_string(),
                    });
                }
                ContentBlock::ServerToolUse { .. }
                | ContentBlock::ConnectorText { .. }
                | ContentBlock::AdvisorToolResult { .. } => {
                    return Err(LlmError::InvalidRequest {
                        message: "GeminiCodec does not encode Anthropic server-generated blocks".to_string(),
                    });
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn decode_response_body(body_json: Value) -> Result<LlmResponse, LlmError> {
    let id = body_json
        .get("responseId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let model = body_json
        .get("modelVersion")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let candidate = body_json
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .ok_or_else(|| {
            let message = match body_json
                .get("promptFeedback")
                .and_then(|feedback| feedback.get("blockReason"))
                .and_then(Value::as_str)
            {
                Some(reason) => format!("Gemini blocked the prompt: {reason}"),
                None => "Gemini response missing candidates".to_string(),
            };
            LlmError::InvalidRequest { message }
        })?;

    let mut content = Vec::new();
    // Gemini does not assign tool-call ids; synthesize unique ones so tool
    // results can reference their call after a round trip.
    let mut tool_call_count = 0u32;
    if let Some(parts) = candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
    {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    content.push(ContentBlock::Text {
                        text: text.to_string(),
                        cache_control: None,
                    });
                }
                continue;
            }

            if let Some(function_call) = part.get("functionCall") {
                content.push(ContentBlock::ToolCall {
                    id: format!("call_{tool_call_count}"),
                    name: function_call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input: function_call
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
                });
                tool_call_count += 1;
            }
        }
    }

    let usage = body_json
        .get("usageMetadata")
        .map(decode_usage)
        .unwrap_or_default();

    // Mirror the stream decoder: any function call normalizes the stop reason
    // to tool_use regardless of Gemini's finishReason.
    let stop_reason = if content.iter().any(|block| matches!(block, ContentBlock::ToolCall { .. })) {
        Some("tool_use".to_string())
    } else {
        candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .map(map_finish_reason)
    };

    Ok(LlmResponse {
        id,
        model,
        content,
        stop_reason,
        usage,
        cost: None,
        provider_metadata: body_json,
    })
}
