use crate::{
    ContentBlock, ContentDelta, LlmError, LlmEvent, LlmRequest, LlmResponse, MessageDeltaPayload,
    ProviderRequest, ProviderResponse, RawStreamFrame, ResponseFormat, StreamDecoder, ToolChoice,
    ToolDeclaration, Usage, WireCodec,
};

use std::collections::BTreeMap;

use base64::Engine;
use serde_json::Value;

#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct OpenAiChatCodec {
    base_url: String,
    profile_name: Option<String>,
}

impl OpenAiChatCodec {
    #[must_use]
    #[allow(missing_docs)]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            profile_name: None,
        }
    }

    /// Attach the provider profile name so compatible providers with
    /// provider-specific extensions can opt into them without relying on URL
    /// matching (which would fail for user-configured proxies).
    #[must_use]
    pub fn with_profile_name(mut self, profile_name: impl Into<String>) -> Self {
        self.profile_name = Some(profile_name.into());
        self
    }

    fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    fn is_kimi_profile(&self) -> bool {
        matches!(self.profile_name.as_deref(), Some("kimi" | "kimi-code"))
    }

    fn is_deepseek_profile(&self) -> bool {
        let endpoint = self.base_url.trim_end_matches('/');
        self.profile_name.as_deref() == Some("deepseek")
            || matches!(
                endpoint,
                "https://api.deepseek.com" | "https://api.deepseek.com/v1"
            )
    }

    fn deepseek_thinking_rejects_tool_choice(&self, model: &str) -> bool {
        self.is_deepseek_profile()
            && matches!(
                model,
                "deepseek-reasoner" | "deepseek-v4-flash" | "deepseek-v4-pro"
            )
    }

    fn kimi_reasoning_effort(&self, request: &LlmRequest) -> Option<&'static str> {
        if !self.is_kimi_profile()
            || !matches!(request.model.as_str(), "kimi-k3" | "k3" | "k3-256k")
        {
            return None;
        }

        match request.effort.as_ref().and_then(Value::as_str) {
            Some("low" | "minimum" | "light") => Some("low"),
            Some("high" | "medium") => Some("high"),
            Some("max" | "xhigh" | "ultra") => Some("max"),
            _ => None,
        }
    }

    fn kimi_thinking_mode(&self, request: &LlmRequest) -> Option<&'static str> {
        if !self.is_kimi_profile() || !matches!(request.model.as_str(), "kimi-k2.6" | "k2.6") {
            return None;
        }
        match request.effort.as_ref().and_then(Value::as_str) {
            Some("disabled" | "off" | "none") => Some("disabled"),
            Some("enabled" | "on") => Some("enabled"),
            _ => None,
        }
    }

    fn deepseek_legacy_model(&self, model: &str) -> Option<(&'static str, &'static str)> {
        let endpoint = self.base_url.trim_end_matches('/');
        if endpoint != "https://api.deepseek.com" && endpoint != "https://api.deepseek.com/v1" {
            return None;
        }
        match model {
            "deepseek-chat" => Some(("deepseek-v4-flash", "disabled")),
            "deepseek-reasoner" => Some(("deepseek-v4-flash", "enabled")),
            _ => None,
        }
    }

    fn deepseek_reasoning(&self, request: &LlmRequest) -> Option<(&'static str, Option<&str>)> {
        if !self.is_deepseek_profile()
            || matches!(
                request.model.as_str(),
                "deepseek-chat" | "deepseek-reasoner"
            )
        {
            return None;
        }
        match request.effort.as_ref().and_then(Value::as_str) {
            Some("disabled" | "off" | "none") => Some(("disabled", None)),
            Some("high") => Some(("enabled", Some("high"))),
            Some("max") => Some(("enabled", Some("max"))),
            _ => None,
        }
    }
}

impl WireCodec for OpenAiChatCodec {
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        reject_unsupported_content_blocks(request)?;

        // The provider-neutral numeric reasoning budget has no portable
        // chat-completions wire slot, so `request.reasoning` is still dropped
        // gracefully. Provider-specific controls such as Kimi's string-valued
        // `reasoning_effort` are encoded explicitly below.

        let mut messages = Vec::new();
        let deepseek_profile = self.is_deepseek_profile();
        let deepseek_rejects_tool_choice =
            self.deepseek_thinking_rejects_tool_choice(&request.model);
        let preserve_reasoning_content = self.is_kimi_profile() || deepseek_profile;
        let assistant_reasoning_blocks = request
            .messages
            .iter()
            .filter(|message| {
                message.role == "assistant"
                    && message
                        .content
                        .iter()
                        .any(|block| matches!(block, ContentBlock::Reasoning { .. }))
            })
            .count();

        if deepseek_profile {
            tracing::debug!(
                target = "llm_client::openai",
                model = %request.model,
                profile = %self.profile_name.as_deref().unwrap_or("<none>"),
                base_url = %self.base_url,
                incoming_messages = request.messages.len(),
                assistant_reasoning_blocks = assistant_reasoning_blocks,
                preserve_reasoning_content = preserve_reasoning_content,
                deepseek_rejects_tool_choice = deepseek_rejects_tool_choice,
                has_tool_choice = request.tool_choice.is_some(),
                tool_choice = ?request.tool_choice,
                event = "openai_encode_request_start",
            );
        }

        if !request.system.is_empty() {
            let text = request
                .system
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            messages.push(serde_json::json!({"role": "system", "content": text}));
        }

        let require_assistant_tool_content = deepseek_profile;
        // DeepSeek V4 thinking is on by default (including v4-flash 极速).
        // Any later turn that omits that turn's reasoning_content is a 400:
        // "The reasoning_content in the thinking mode must be passed back".
        // This is not limited to tool-call messages — generate-build and
        // other multi-turn agents fail on a thinking+text reply too.
        messages.extend(request.messages.iter().flat_map(|message| {
            encode_message(
                message,
                preserve_reasoning_content,
                require_assistant_tool_content,
            )
        }));

        let mut body = serde_json::Map::new();
        let legacy_deepseek = self.deepseek_legacy_model(&request.model);
        let wire_model = legacy_deepseek.map_or(request.model.as_str(), |(model, _)| model);
        body.insert("model".to_string(), Value::String(wire_model.to_string()));
        body.insert("messages".to_string(), Value::Array(messages));
        let (encoded_messages_with_reasoning_content, last_assistant_reasoning_len) = body
            .get("messages")
            .and_then(Value::as_array)
            .map_or((0usize, 0usize), |encoded_messages| {
                let messages_with_reasoning = encoded_messages
                    .iter()
                    .filter(|value| value.get("reasoning_content").is_some())
                    .count();
                let last_assistant_reasoning_len = encoded_messages
                    .iter()
                    .rev()
                    .find(|value| value.get("role").and_then(Value::as_str) == Some("assistant"))
                    .and_then(|value| value.get("reasoning_content"))
                    .and_then(Value::as_str)
                    .map_or(0, str::len);
                (messages_with_reasoning, last_assistant_reasoning_len)
            });
        let (deepseek_tool_calls, deepseek_tool_calls_without_reasoning) = body
            .get("messages")
            .and_then(Value::as_array)
            .map_or((0usize, 0usize), |encoded_messages| {
                let with_tool_calls = encoded_messages
                    .iter()
                    .filter(|value| value.get("role").and_then(Value::as_str) == Some("assistant"))
                    .filter(|value| value.get("tool_calls").is_some())
                    .count();
                let missing_reasoning = encoded_messages
                    .iter()
                    .filter(|value| value.get("role").and_then(Value::as_str) == Some("assistant"))
                    .filter(|value| value.get("tool_calls").is_some())
                    .filter(|value| value.get("reasoning_content").is_none())
                    .count();
                (with_tool_calls, missing_reasoning)
            });
        if let Some((_, thinking_type)) = legacy_deepseek {
            body.insert(
                "thinking".to_string(),
                serde_json::json!({"type": thinking_type}),
            );
        }
        if let Some((thinking_type, effort)) = self.deepseek_reasoning(request) {
            body.insert(
                "thinking".to_string(),
                serde_json::json!({"type": thinking_type}),
            );
            if let Some(effort) = effort {
                body.insert(
                    "reasoning_effort".to_string(),
                    Value::String(effort.to_string()),
                );
            }
        }

        if request.stream {
            body.insert("stream".to_string(), Value::Bool(true));
            if self.is_kimi_profile() {
                // Kimi omits the terminal usage object from SSE unless it is
                // explicitly requested. Without this, the session transcript,
                // cost tracker, and every client token counter all receive 0.
                body.insert(
                    "stream_options".to_string(),
                    serde_json::json!({"include_usage": true}),
                );
            }
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
        if let Some(effort) = self.kimi_reasoning_effort(request) {
            body.insert(
                "reasoning_effort".to_string(),
                Value::String(effort.to_string()),
            );
        }
        if let Some(thinking_type) = self.kimi_thinking_mode(request) {
            body.insert(
                "thinking".to_string(),
                serde_json::json!({"type": thinking_type}),
            );
        }
        if !request.stop_sequences.is_empty() {
            body.insert(
                "stop".to_string(),
                Value::Array(
                    request
                        .stop_sequences
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }

        if let Some(response_format) = &request.response_format {
            body.insert(
                "response_format".to_string(),
                encode_response_format(response_format),
            );
        }

        // DeepSeek V4 thinking mode rejects the `tool_choice` parameter. Keep
        // the tools themselves so workflow schema agents can follow their
        // StructuredOutput prompt and rely on the existing local validation,
        // nudge, and bounded retry path. The retired `deepseek-chat` alias
        // explicitly disables thinking above and can still use named choice.
        if !deepseek_rejects_tool_choice {
            if let Some(tool_choice) = &request.tool_choice {
                body.insert("tool_choice".to_string(), encode_tool_choice(tool_choice));
            }
        } else if request.tool_choice.is_some() {
            tracing::debug!(
                target = "llm_client::openai",
                model = %request.model,
                profile = %self.profile_name.as_deref().unwrap_or("<none>"),
                event = "openai_encode_request_tool_choice_dropped_by_deepseek",
            );
        }
        if !request.tools.is_empty() {
            body.insert(
                "tools".to_string(),
                Value::Array(request.tools.iter().map(encode_tool).collect()),
            );
        }
        if deepseek_profile {
            if deepseek_tool_calls_without_reasoning > 0 {
                tracing::warn!(
                    target = "llm_client::openai",
                    model = %request.model,
                    event = "openai_encode_request_deepseek_tool_call_without_reasoning_content",
                    assistant_tool_calls_without_reasoning_content =
                        deepseek_tool_calls_without_reasoning,
                    assistant_tool_calls_total = deepseek_tool_calls,
                );
            }
            tracing::debug!(
                target = "llm_client::openai",
                model = %request.model,
                event = "openai_encode_request_end",
                encoded_messages_with_reasoning_content =
                    encoded_messages_with_reasoning_content,
                last_assistant_reasoning_len = last_assistant_reasoning_len,
                deepseek_assistant_tool_calls = deepseek_tool_calls,
                deepseek_assistant_tool_calls_without_reasoning_content =
                    deepseek_tool_calls_without_reasoning,
                final_tool_choice = body.contains_key("tool_choice"),
                thinking_field = ?body.get("thinking"),
                tool_count = request.tools.len(),
            );
        }

        // DeepSeek thinking mode requires a `reasoning_content` key on EVERY
        // assistant message positioned after the LAST `user` message. Verified
        // against the live API (api.deepseek.com, deepseek-v4-flash):
        //   - an empty string satisfies the check;
        //   - assistant messages BEFORE the last user message are exempt;
        //   - `role: "tool"` does NOT reset the window, so in an agentic tool
        //     loop every assistant turn since the last real user message needs
        //     the field (an earlier round missing it 400s just as the last one does);
        //   - the constraint does not apply at all when thinking is disabled.
        //
        // Two shapes reach the wire without it, and both 400 with "The
        // reasoning_content in the thinking mode must be passed back to the API.":
        //   1. `encode_message` splits ONE assistant turn carrying text AND a
        //      tool call into TWO wire messages (a text message, then a
        //      tool_calls message) but attaches the turn's reasoning trace to
        //      only one of them — so the text message goes out bare.
        //   2. A turn whose model reply carried no reasoning block at all
        //      (compaction, replay, or simply a reply with no trace).
        // Backfilling the key covers both; the real trace still rides the
        // message that owns it.
        if deepseek_profile {
            let thinking_disabled = body
                .get("thinking")
                .and_then(|thinking| thinking.get("type"))
                .and_then(Value::as_str)
                == Some("disabled");
            if !thinking_disabled {
                if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
                    let after_last_user = messages
                        .iter()
                        .rposition(|message| {
                            message.get("role").and_then(Value::as_str) == Some("user")
                        })
                        .map_or(0, |index| index + 1);
                    for message in messages[after_last_user..].iter_mut() {
                        let Some(object) = message.as_object_mut() else {
                            continue;
                        };
                        if object.get("role").and_then(Value::as_str) != Some("assistant") {
                            continue;
                        }
                        object
                            .entry("reasoning_content")
                            .or_insert_with(|| Value::String(String::new()));
                    }
                }
            }
        }

        let mut provider_request =
            ProviderRequest::post_json(self.chat_completions_url(), Value::Object(body));
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
        .or_else(|| {
            error
                .and_then(|error| error.get("type"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default();

    match code {
        "insufficient_quota" => LlmError::QuotaExceeded,
        "context_length_exceeded" => LlmError::ContextOverflow { token_gap: 0 },
        "invalid_api_key" | "invalid_authentication" => LlmError::Authentication {
            message: super::api_error_message(response.status, &response.body_json, &message),
        },
        "model_not_found" => LlmError::ModelUnavailable,
        _ => super::map_error_status(
            response.status,
            &message,
            super::api_error_message(response.status, &response.body_json, &message),
            retry_after,
        ),
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

        let root: serde_json::Value =
            serde_json::from_str(data).map_err(|_| LlmError::InvalidRequest {
                message: "OpenAI stream frame is not valid JSON".to_string(),
            })?;

        self.ensure_started(&root, &mut out);

        if let Some(usage) = root.get("usage").filter(|value| !value.is_null()) {
            self.usage = Some(normalize_usage(usage));
        }

        if let Some(choice) = root
            .get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
        {
            if let Some(delta) = choice.get("delta") {
                if let Some(reasoning) = delta
                    .get("reasoning_content")
                    .and_then(serde_json::Value::as_str)
                {
                    self.handle_reasoning(reasoning, &mut out);
                }
                if let Some(content) = delta.get("content").and_then(serde_json::Value::as_str) {
                    self.handle_text(content, &mut out);
                }
                if let Some(tool_calls) = delta
                    .get("tool_calls")
                    .and_then(serde_json::Value::as_array)
                {
                    for tool_call in tool_calls {
                        self.handle_tool_fragment(tool_call, &mut out);
                    }
                }
            }
            if let Some(finish_reason) = choice
                .get("finish_reason")
                .and_then(serde_json::Value::as_str)
            {
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
            id: root
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            model: root
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            content: Vec::new(),
            stop_reason: None,
            stop_details: None,
            usage: Usage::default(),
            cost: None,
            provider_metadata: serde_json::Value::Null,
        };
        out.push(LlmEvent::MessageStart {
            response: Box::new(response),
        });
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

fn encode_message(
    message: &crate::Message,
    preserve_reasoning_content: bool,
    require_assistant_tool_content: bool,
) -> Vec<Value> {
    let mut text = String::new();
    let mut reasoning_content = String::new();
    let mut media_parts: Vec<Value> = Vec::new();
    let mut tool_calls = Vec::new();
    let mut messages = Vec::new();
    let mut has_media = false;

    for block in &message.content {
        match block {
            ContentBlock::Text { text: block_text, .. }
            | ContentBlock::TextJsUtf16 {
                text: block_text, ..
            } => {
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
                    messages.push(assistant_tool_call_message(
                        &message.role,
                        &tool_calls,
                        require_assistant_tool_content,
                    ));
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
            ContentBlock::Reasoning {
                text: reasoning_text,
                ..
            } if preserve_reasoning_content && message.role == "assistant" => {
                if !reasoning_content.is_empty() {
                    reasoning_content.push('\n');
                }
                reasoning_content.push_str(reasoning_text);
            }
            // Most chat-completions providers reject reasoning_content as input.
            // Kimi profiles opt into preserved thinking above; redacted thinking
            // never has a portable OpenAI-compatible representation.
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
        messages.push(build_user_message(
            &message.role,
            &text,
            &media_parts,
            has_media,
        ));
    }
    if !tool_calls.is_empty() {
        messages.push(assistant_tool_call_message(
            &message.role,
            &tool_calls,
            require_assistant_tool_content,
        ));
    }
    if !reasoning_content.is_empty() {
        let assistant_index = messages
            .iter_mut()
            .rposition(|value| {
                value.get("role").and_then(Value::as_str) == Some("assistant")
                    && value.get("tool_calls").is_some()
            })
            .or_else(|| {
                messages.iter_mut().rposition(|value| {
                    value.get("role").and_then(Value::as_str) == Some("assistant")
                })
            });
        let reason_len = reasoning_content.len();
        if let Some(Value::Object(assistant)) =
            assistant_index.and_then(|index| messages.get_mut(index))
        {
            let role = assistant
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let has_tool_calls = assistant.get("tool_calls").is_some();
            assistant.insert(
                "reasoning_content".to_string(),
                Value::String(reasoning_content),
            );
            if preserve_reasoning_content && message.role == "assistant" {
                tracing::trace!(
                    target = "llm_client::openai",
                    role = %role,
                    has_tool_calls = has_tool_calls,
                    reasoning_chars = reason_len,
                    assistant_message_index = assistant_index.unwrap_or_default(),
                    event = "openai_encode_message_reasoning_attached_to_assistant",
                );
            }
        } else {
            if preserve_reasoning_content && message.role == "assistant" {
                tracing::warn!(
                    target = "llm_client::openai",
                    role = %message.role,
                    reasoning_chars = reason_len,
                    reason = "no_assistant_message_emitted_for_reasoning",
                    event = "openai_encode_message_reasoning_synced_assistant_fallback",
                );
            }
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": null,
                "reasoning_content": reasoning_content,
            }));
        }
        if preserve_reasoning_content && message.role == "assistant" {
            tracing::debug!(
                target = "llm_client::openai",
                role = %message.role,
                reasoning_chars = reason_len,
                assistant_message_block_count = message.content.len(),
                event = "openai_encode_message_with_reasoning_content",
            );
        }
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

fn assistant_tool_call_message(role: &str, tool_calls: &[Value], require_content: bool) -> Value {
    let mut message_json = serde_json::Map::new();
    message_json.insert("role".to_string(), Value::String(role.to_string()));
    message_json.insert(
        "content".to_string(),
        if require_content {
            Value::String(String::new())
        } else {
            Value::Null
        },
    );
    message_json.insert("tool_calls".to_string(), Value::Array(tool_calls.to_vec()));
    Value::Object(message_json)
}

fn tool_result_content(output: &Value) -> Value {
    if let Some(text) = protocol::js_utf16::tool_result_display(output) {
        return Value::String(text);
    }
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
                // decoder emits Reasoning blocks into history. Most chat-completions
                // providers reject reasoning_content as input, so encode_message skips
                // it unless a provider-specific replay path opts in (Kimi, DeepSeek
                // thinking). Erroring here would break turn 2+.
                ContentBlock::ServerToolUse { .. }
                | ContentBlock::ConnectorText { .. }
                | ContentBlock::AdvisorToolResult { .. } => {
                    return Err(LlmError::InvalidRequest {
                        message:
                            "OpenAiChatCodec does not encode Anthropic server-generated blocks"
                                .to_string(),
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
            let function = tool_call
                .get("function")
                .ok_or_else(|| LlmError::InvalidRequest {
                    message: "OpenAI tool call missing function".to_string(),
                })?;
            let name = string_field(function, "name")?;
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or_else(|| LlmError::InvalidRequest {
                    message: "OpenAI tool call missing function.arguments".to_string(),
                })?;
            let input =
                serde_json::from_str::<Value>(arguments).map_err(|_| LlmError::InvalidRequest {
                    message: "OpenAI tool call has invalid function.arguments JSON".to_string(),
                })?;
            content.push(ContentBlock::ToolCall { id, name, input });
        }
    }

    let usage = body_json
        .get("usage")
        .map(normalize_usage)
        .unwrap_or_default();
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
    let prompt_tokens = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completion_tokens = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    // OpenAI reports cached/reasoning tokens as subsets of prompt/completion
    // counts; subtract them so every TokenUsage bucket stays independently billable.
    let cached_tokens = usage
        .get("prompt_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        // Kimi's documented wire shape reports this bucket directly on usage.
        .or_else(|| usage.get("cached_tokens").and_then(Value::as_u64))
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

    #[test]
    fn openrouter_numeric_400_preserves_overflow_message_for_retry() {
        let message = "This endpoint's maximum context length is 256000 tokens. However, you requested about 260085 tokens (10691 of text input, 24791 of tool input, 224603 in the output).";
        let response = ProviderResponse::json(
            400,
            serde_json::json!({
                "error": {
                    "message": message,
                    "code": 400,
                    "metadata": {"provider_name": null}
                }
            }),
        );

        match decode_error_response(&response) {
            LlmError::InvalidRequest { message: decoded } => {
                assert!(decoded.contains(message));
            }
            other => panic!("expected retry-visible InvalidRequest, got {other:?}"),
        }
    }

    fn request_with_named_tool_choice(model: &str) -> LlmRequest {
        let mut request = LlmRequest::new(model);
        request.tool_choice = Some(ToolChoice::Tool {
            name: "StructuredOutput".to_string(),
        });
        request.tools.push(ToolDeclaration {
            name: "StructuredOutput".to_string(),
            description: "Return structured output".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"summary": {"type": "string"}},
                "required": ["summary"]
            }),
            ..ToolDeclaration::default()
        });
        request
    }

    #[test]
    fn deepseek_thinking_omits_named_tool_choice_but_keeps_structured_output_tool() {
        for codec in [
            OpenAiChatCodec::new("https://api.deepseek.com"),
            OpenAiChatCodec::new("https://deepseek-proxy.example/v1").with_profile_name("deepseek"),
        ] {
            let encoded = codec
                .encode_request(&request_with_named_tool_choice("deepseek-v4-flash"))
                .expect("encode DeepSeek V4 structured-output request");
            let body = body_of(&encoded);

            assert!(
                body.get("tool_choice").is_none(),
                "DeepSeek V4 thinking mode rejects named tool_choice"
            );
            assert_eq!(
                body["tools"][0]["function"]["name"], "StructuredOutput",
                "the prompt-driven fallback still advertises StructuredOutput"
            );
        }
    }

    #[test]
    fn named_tool_choice_is_kept_when_deepseek_thinking_is_disabled_or_provider_supports_it() {
        let deepseek_chat = OpenAiChatCodec::new("https://api.deepseek.com")
            .encode_request(&request_with_named_tool_choice("deepseek-chat"))
            .expect("encode legacy non-thinking DeepSeek request");
        assert_eq!(body_of(&deepseek_chat)["thinking"]["type"], "disabled");
        assert_eq!(
            body_of(&deepseek_chat)["tool_choice"]["function"]["name"],
            "StructuredOutput"
        );

        let openai = OpenAiChatCodec::new("https://api.openai.com/v1")
            .with_profile_name("openai")
            .encode_request(&request_with_named_tool_choice("gpt-4o"))
            .expect("encode OpenAI structured-output request");
        assert_eq!(
            body_of(&openai)["tool_choice"]["function"]["name"],
            "StructuredOutput"
        );
    }

    /// Every assistant message after the LAST `user` message must carry a
    /// `reasoning_content` key when DeepSeek thinking is active.
    ///
    /// Verified against the live api.deepseek.com on `deepseek-v4-flash`: the
    /// exact wire array this encoder used to produce for a
    /// `[Reasoning, Text, ToolCall]` turn returns
    /// `400 "The reasoning_content in the thinking mode must be passed back to
    /// the API."`, while the same array with the key present on both assistant
    /// messages returns 200. An empty string satisfies the check; `role: "tool"`
    /// does NOT reset the window, so an EARLIER round missing the key 400s too.
    ///
    /// This asserts the invariant over EVERY assistant message rather than
    /// spot-checking indices — the predecessor test pinned the split shape by
    /// index and asserted `reasoning_content` was ABSENT on the text fragment,
    /// so it could never fail on the bug it was meant to guard.
    #[test]
    fn deepseek_thinking_never_emits_a_bare_assistant_message_after_last_user() {
        // The reported failing turn: reasoning + interstitial text + a tool call,
        // then the tool result, then a second tool-calling turn that carried NO
        // reasoning trace at all (the `E`/`G5` shapes, which 400 independently
        // of the split).
        let mut request = LlmRequest::new("deepseek-v4-flash");
        request.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "build me an app".to_string(),
                cache_control: None,
            }],
        });
        request.messages.push(Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Reasoning {
                    text: "I should look at the workspace".to_string(),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "Let me check the workspace context first.".to_string(),
                    cache_control: None,
                },
                ContentBlock::ToolCall {
                    id: "call_read_1".to_string(),
                    name: "Read".to_string(),
                    input: serde_json::json!({"file_path": "/w/LINGXI.md"}),
                },
            ],
        });
        request.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_call_id: "call_read_1".to_string(),
                output: Value::String("file contents".to_string()),
                is_error: false,
                cache_control: None,
                cache_reference: None,
            }],
        });
        // A later turn with NO Reasoning block — the model simply did not emit a
        // trace. Still after the last real `user` message, so it needs the key.
        request.messages.push(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolCall {
                id: "call_bash_1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
            }],
        });

        let encoded = OpenAiChatCodec::new("https://api.deepseek.com")
            .encode_request(&request)
            .expect("encode DeepSeek thinking history");
        let messages = encoded.body_json["messages"]
            .as_array()
            .expect("DeepSeek messages array");

        let last_user = messages
            .iter()
            .rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"))
            .expect("a user message exists");
        let bare: Vec<usize> = messages
            .iter()
            .enumerate()
            .skip(last_user + 1)
            .filter(|(_, m)| m.get("role").and_then(Value::as_str) == Some("assistant"))
            .filter(|(_, m)| m.get("reasoning_content").is_none())
            .map(|(i, _)| i)
            .collect();
        assert!(
            bare.is_empty(),
            "assistant messages after the last user message must all carry a \
             reasoning_content key; these did not: {bare:?} in {}",
            serde_json::to_string_pretty(messages).unwrap()
        );
        // The turn's real trace is preserved, not clobbered by the backfill.
        let traces: Vec<&str> = messages
            .iter()
            .filter_map(|m| m.get("reasoning_content").and_then(Value::as_str))
            .filter(|t| !t.is_empty())
            .collect();
        assert_eq!(
            traces,
            vec!["I should look at the workspace"],
            "the real reasoning trace must survive verbatim, exactly once"
        );

        // Thinking disabled ⇒ the constraint does not apply, so no backfill.
        // (`deepseek-chat` maps to v4-flash with thinking explicitly disabled.)
        let disabled = OpenAiChatCodec::new("https://api.deepseek.com")
            .encode_request(&{
                let mut r = request.clone();
                r.model = "deepseek-chat".to_string();
                r
            })
            .expect("encode DeepSeek with thinking disabled");
        assert_eq!(disabled.body_json["thinking"]["type"], "disabled");
        let disabled_messages = disabled.body_json["messages"]
            .as_array()
            .expect("messages array");
        assert!(
            disabled_messages
                .iter()
                .any(
                    |m| m.get("role").and_then(Value::as_str) == Some("assistant")
                        && m.get("reasoning_content").is_none()
                ),
            "thinking-disabled requests must not gain the backfilled key"
        );

        // Non-DeepSeek providers sharing this codec are untouched.
        let openai = OpenAiChatCodec::new("https://api.openai.com/v1")
            .with_profile_name("openai")
            .encode_request(&request)
            .expect("encode ordinary OpenAI history");
        assert!(
            !serde_json::to_string(&openai.body_json["messages"])
                .unwrap()
                .contains("reasoning_content"),
            "the backfill must not leak into ordinary OpenAI-compatible requests"
        );
    }

    #[test]
    fn deepseek_tool_history_preserves_reasoning_and_non_null_assistant_content() {
        let mut request = LlmRequest::new("deepseek-v4-flash");
        request.messages.push(Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Reasoning {
                    text: "choose the design skill".to_string(),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "I will use the design skill.".to_string(),
                    cache_control: None,
                },
                ContentBlock::ToolCall {
                    id: "call_skill".to_string(),
                    name: "Skill".to_string(),
                    input: serde_json::json!({"skill": "frontend-design"}),
                },
            ],
        });

        let encoded = OpenAiChatCodec::new("https://api.deepseek.com")
            .encode_request(&request)
            .expect("encode DeepSeek tool history");
        let messages = encoded.body_json["messages"]
            .as_array()
            .expect("DeepSeek messages array");
        assert_eq!(messages[0]["content"], "I will use the design skill.");
        // The turn's real trace rides the message that owns it (the tool_calls
        // one); the text fragment this turn was split into gets the empty-string
        // backfill. It previously went out with NO `reasoning_content` key at
        // all, which the live API rejects — see
        // `deepseek_thinking_never_emits_a_bare_assistant_message_after_last_user`.
        assert_eq!(messages[0]["reasoning_content"], "");
        assert_eq!(messages[1]["content"], "");
        assert_eq!(messages[1]["reasoning_content"], "choose the design skill");

        let openai = OpenAiChatCodec::new("https://api.openai.com/v1")
            .encode_request(&request)
            .expect("encode ordinary OpenAI tool history");
        let messages = openai.body_json["messages"]
            .as_array()
            .expect("OpenAI messages array");
        assert_eq!(messages[0]["content"], "I will use the design skill.");
        assert!(messages[1]["content"].is_null());
        assert!(messages[1].get("reasoning_content").is_none());
    }

    /// (a) A request carrying a reasoning budget no longer errors, and the
    /// chat-completions body has no reasoning/budget field (there is no wire slot).
    #[test]
    fn reasoning_budget_is_dropped_not_rejected() {
        let mut request = LlmRequest::new("deepseek-v4-flash");
        request.reasoning = Some(ReasoningConfig::Enabled {
            budget_tokens: 4096,
        });
        request.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
                cache_control: None,
            }],
        });

        let codec = OpenAiChatCodec::new("https://api.deepseek.com");
        let provider_request = codec
            .encode_request(&request)
            .expect("reasoning budget must not error");

        let body = body_of(&provider_request)
            .as_object()
            .expect("body is an object");
        assert!(
            !body.contains_key("reasoning"),
            "no reasoning field on the wire"
        );
        assert!(
            !body.contains_key("reasoning_content"),
            "no reasoning_content field"
        );
        assert!(
            !body.contains_key("budget_tokens"),
            "no budget_tokens field"
        );
    }

    #[test]
    fn retired_deepseek_ids_are_translated_with_their_previous_thinking_mode() {
        let codec = OpenAiChatCodec::new("https://api.deepseek.com");

        let chat = codec
            .encode_request(&LlmRequest::new("deepseek-chat"))
            .expect("legacy chat request");
        assert_eq!(body_of(&chat)["model"], "deepseek-v4-flash");
        assert_eq!(body_of(&chat)["thinking"]["type"], "disabled");

        let reasoner = codec
            .encode_request(&LlmRequest::new("deepseek-reasoner"))
            .expect("legacy reasoner request");
        assert_eq!(body_of(&reasoner)["model"], "deepseek-v4-flash");
        assert_eq!(body_of(&reasoner)["thinking"]["type"], "enabled");
    }

    #[test]
    fn retired_deepseek_ids_are_not_rewritten_for_custom_openai_endpoints() {
        let codec = OpenAiChatCodec::new("https://gateway.example/v1");
        let request = codec
            .encode_request(&LlmRequest::new("deepseek-chat"))
            .expect("custom endpoint request");

        assert_eq!(body_of(&request)["model"], "deepseek-chat");
        assert!(body_of(&request).get("thinking").is_none());
    }

    /// (b) History containing a Reasoning block encodes successfully. Ordinary
    /// OpenAI-compatible endpoints omit it; DeepSeek thinking must echo it.
    #[test]
    fn reasoning_block_in_history_is_omitted_for_generic_openai() {
        let mut request = LlmRequest::new("gpt-4o");
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

        let codec = OpenAiChatCodec::new("https://api.openai.com/v1").with_profile_name("openai");
        let provider_request = codec
            .encode_request(&request)
            .expect("reasoning block must not error");

        let messages = body_of(&provider_request)
            .get("messages")
            .and_then(Value::as_array)
            .expect("messages array");
        let serialized = serde_json::to_string(messages).expect("serialize messages");
        assert!(serialized.contains("the answer is 42"), "text survives");
        assert!(!serialized.contains("let me think"), "reasoning is omitted");
        assert!(
            !serialized.contains("reasoning_content"),
            "no reasoning_content key"
        );
    }

    #[test]
    fn deepseek_text_history_preserves_reasoning_content() {
        let mut request = LlmRequest::new("deepseek-v4-flash");
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

        let encoded = OpenAiChatCodec::new("https://api.deepseek.com")
            .encode_request(&request)
            .expect("encode DeepSeek thinking history");
        let messages = encoded.body_json["messages"]
            .as_array()
            .expect("DeepSeek messages array");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"], "the answer is 42");
        assert_eq!(messages[0]["reasoning_content"], "let me think");
    }

    #[test]
    fn kimi_reasoning_effort_uses_top_level_provider_field() {
        let codec = OpenAiChatCodec::new("https://proxy.example/v1").with_profile_name("kimi-code");

        for (configured, expected) in [
            ("low", "low"),
            ("minimum", "low"),
            ("medium", "high"),
            ("high", "high"),
            ("xhigh", "max"),
            ("max", "max"),
        ] {
            let mut request = LlmRequest::new("k3");
            request.effort = Some(Value::String(configured.to_string()));
            let encoded = codec.encode_request(&request).expect("encode Kimi K3");
            assert_eq!(
                encoded.body_json["reasoning_effort"], expected,
                "{configured} must map to Kimi's supported effort vocabulary"
            );
        }

        let mut non_k3 = LlmRequest::new("kimi-for-coding");
        non_k3.effort = Some(Value::String("high".to_string()));
        let encoded = codec.encode_request(&non_k3).expect("encode Kimi K2.7");
        assert!(
            encoded.body_json.get("reasoning_effort").is_none(),
            "K2.7 Code uses its fixed thinking mode, not K3 reasoning_effort"
        );
    }

    #[test]
    fn kimi_k26_toggle_is_encoded_as_thinking_mode() {
        let codec = OpenAiChatCodec::new("https://proxy.example/v1").with_profile_name("kimi");
        for (configured, expected) in [("enabled", "enabled"), ("disabled", "disabled")] {
            let mut request = LlmRequest::new("kimi-k2.6");
            request.effort = Some(Value::String(configured.to_string()));
            let encoded = codec.encode_request(&request).expect("encode Kimi K2.6");
            assert_eq!(encoded.body_json["thinking"]["type"], expected);
            assert!(encoded.body_json.get("reasoning_effort").is_none());
        }
    }

    #[test]
    fn kimi_preserves_reasoning_content_in_assistant_history() {
        let mut request = LlmRequest::new("kimi-k2.7-code");
        request.messages.push(Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Reasoning {
                    text: "preserved thought".to_string(),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "final answer".to_string(),
                    cache_control: None,
                },
            ],
        });

        let codec = OpenAiChatCodec::new("https://proxy.example/v1").with_profile_name("kimi");
        let encoded = codec.encode_request(&request).expect("encode Kimi history");
        assert_eq!(
            encoded.body_json["messages"][0]["reasoning_content"],
            "preserved thought"
        );
        assert_eq!(encoded.body_json["messages"][0]["content"], "final answer");
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
            with_attribution
                .headers
                .get("HTTP-Referer")
                .map(String::as_str),
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
