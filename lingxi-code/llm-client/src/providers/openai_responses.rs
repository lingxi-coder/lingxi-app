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

    fn decode_response(&self, response: ProviderResponse) -> Result<LlmResponse, LlmError> {
        if response.status >= 400 {
            // The Responses API shares the Chat API `{error: {...}}` envelope.
            return Err(super::openai::decode_error_response(&response));
        }
        decode_response_body(response.body_json)
    }

    fn stream_decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(OpenAiResponsesStreamDecoder::default())
    }

    fn clone_box(&self) -> Box<dyn WireCodec> {
        Box::new(self.clone())
    }
}

/// Decode a non-error Responses API body into an [`LlmResponse`].
///
/// Output item shapes are pinned to the vendored Codex CLI reference
/// (`codex-rs/protocol/src/models.rs` `ResponseItem`): `message` carries
/// `content[]` with `output_text` parts, `function_call` carries
/// `{call_id, name, arguments}` with `arguments` a raw JSON *string*, and
/// `reasoning` carries `summary[]` of `{type: "summary_text", text}` parts.
/// Unknown item types are skipped (tolerant-decoder convention).
fn decode_response_body(body_json: Value) -> Result<LlmResponse, LlmError> {
    let id = string_field(&body_json, "id")?;
    let model = string_field(&body_json, "model")?;

    let mut content = Vec::new();
    let mut has_function_call = false;
    if let Some(output) = body_json.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => decode_message_item(item, &mut content),
                Some("function_call") => {
                    has_function_call = true;
                    content.push(decode_function_call_item(item)?);
                }
                Some("reasoning") => decode_reasoning_item(item, &mut content),
                // Unknown output item types (web_search_call, ...) are skipped.
                _ => {}
            }
        }
    }

    let stop_reason = map_stop_reason(&body_json, has_function_call);
    let usage = body_json.get("usage").map(normalize_usage).unwrap_or_default();

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

fn decode_message_item(item: &Value, content: &mut Vec<ContentBlock>) {
    let Some(parts) = item.get("content").and_then(Value::as_array) else {
        return;
    };
    for part in parts {
        // Only output_text parts carry model text; refusal/unknown parts are
        // skipped (tolerant decode, mirrors OpenAiChatCodec content parts).
        if part.get("type").and_then(Value::as_str) == Some("output_text") {
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

fn decode_function_call_item(item: &Value) -> Result<ContentBlock, LlmError> {
    let id = string_field(item, "call_id")?;
    let name = string_field(item, "name")?;
    let arguments = item.get("arguments").and_then(Value::as_str).unwrap_or("");
    // The wire carries arguments as a raw JSON string (codex models.rs
    // FunctionCall); unparseable arguments fall back to the raw string so a
    // malformed model emission never aborts the whole response decode.
    let input = serde_json::from_str::<Value>(arguments)
        .unwrap_or_else(|_| Value::String(arguments.to_string()));
    Ok(ContentBlock::ToolCall { id, name, input })
}

fn decode_reasoning_item(item: &Value, content: &mut Vec<ContentBlock>) {
    let Some(summary) = item.get("summary").and_then(Value::as_array) else {
        return;
    };
    let text = summary
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("summary_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n");
    // A reasoning item without summary text (encrypted-only) is skipped
    // tolerantly; the Responses API never returns raw reasoning text here.
    if !text.is_empty() {
        content.push(ContentBlock::Reasoning {
            text,
            signature: None,
        });
    }
}

/// Map the Responses API terminal `status` onto the normalized Anthropic
/// stop-reason vocabulary: `completed` → `tool_use`/`end_turn`, `incomplete`
/// with `incomplete_details.reason == "max_output_tokens"` → `max_tokens`,
/// any other incomplete reason (or the bare status when details are absent)
/// passes through verbatim. Missing status → `None`.
fn map_stop_reason(body_json: &Value, has_function_call: bool) -> Option<String> {
    let status = body_json.get("status").and_then(Value::as_str)?;
    match status {
        "completed" => Some(if has_function_call { "tool_use" } else { "end_turn" }.to_string()),
        "incomplete" => {
            let reason = body_json
                .get("incomplete_details")
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or(status);
            Some(if reason == "max_output_tokens" {
                "max_tokens".to_string()
            } else {
                reason.to_string()
            })
        }
        other => Some(other.to_string()),
    }
}

/// Normalize `ResponseCompletedUsage` (codex sse/responses.rs: `input_tokens`,
/// `input_tokens_details.cached_tokens`, `output_tokens`,
/// `output_tokens_details.reasoning_tokens`, `total_tokens`).
///
/// Cached/reasoning tokens are SUBSETS of the input/output counts — subtract
/// them (saturating) so every `TokenUsage` bucket stays independently billable
/// (same rule as `OpenAiChatCodec`). Missing details objects → zeros.
fn normalize_usage(usage: &Value) -> crate::Usage {
    let input_tokens = usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output_tokens = usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
    let cached_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage.get("total_tokens").and_then(Value::as_u64);

    crate::Usage {
        billable_tokens: crate::TokenUsage {
            input: input_tokens.saturating_sub(cached_tokens),
            output: output_tokens.saturating_sub(reasoning_tokens),
            cache_read: cached_tokens,
            reasoning_output: reasoning_tokens,
            ..Default::default()
        },
        context_tokens: total_tokens,
        provider_reported_total_tokens: total_tokens,
        provider_metadata: usage.clone(),
        ..Default::default()
    }
}

fn string_field(value: &Value, field: &str) -> Result<String, LlmError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| LlmError::InvalidRequest {
            message: format!("OpenAI Responses response missing {field}"),
        })
}

/// Per-wire-item block bookkeeping: the canonical block index assigned to a
/// wire `output_index`, and whether the block is currently open.
#[derive(Debug)]
struct BlockState {
    index: u32,
    open: bool,
}

/// Streaming decoder for the Responses API typed SSE events.
///
/// The discriminator is the JSON `type` field of each `data:` payload (codex
/// sse/responses.rs `ResponsesStreamEvent { #[serde(rename = "type")] kind }`).
/// Item-scoped events are keyed by the wire `output_index` (present on
/// `response.output_item.added/done` and every per-item delta event); a
/// missing `output_index` defaults to slot zero, mirroring the
/// `OpenAiChatCodec` tolerance for single-tool streams that omit `index`.
///
/// Unlike the Chat API, blocks are explicitly delimited on the wire
/// (`output_item.added` / `output_item.done`), but text/reasoning blocks are
/// still opened lazily on their first delta so an item that never produces
/// content (e.g. an encrypted-only reasoning item) emits no events.
#[derive(Debug, Default)]
struct OpenAiResponsesStreamDecoder {
    started: bool,
    next_index: u32,
    /// Wire `output_index` → block state.
    blocks: std::collections::BTreeMap<u64, BlockState>,
    has_function_call: bool,
    stop_reason: Option<String>,
    usage: Option<crate::Usage>,
    done: bool,
}

impl StreamDecoder for OpenAiResponsesStreamDecoder {
    fn decode_frame(&mut self, frame: RawStreamFrame) -> Result<Vec<LlmEvent>, LlmError> {
        let text = std::str::from_utf8(&frame.bytes).map_err(|_| LlmError::InvalidRequest {
            message: "OpenAI Responses stream frame is not valid UTF-8".to_string(),
        })?;

        let mut out = Vec::new();
        let data = text.trim();
        // The Responses API ends after response.completed without a [DONE]
        // sentinel, but OpenAI-compatible gateways may append one; tolerate it.
        if data == "[DONE]" {
            self.finish_into(&mut out);
            return Ok(out);
        }

        let root: Value = serde_json::from_str(data).map_err(|_| LlmError::InvalidRequest {
            message: "OpenAI Responses stream frame is not valid JSON".to_string(),
        })?;

        match root.get("type").and_then(Value::as_str) {
            Some("response.created") => {
                self.ensure_started(root.get("response"), &mut out);
            }
            Some("response.output_item.added") => {
                self.handle_item_added(&root, &mut out);
            }
            Some("response.output_text.delta") => {
                if let Some(delta) = root.get("delta").and_then(Value::as_str) {
                    self.handle_text_delta(&root, delta, &mut out);
                }
            }
            Some("response.function_call_arguments.delta") => {
                if let Some(delta) = root.get("delta").and_then(Value::as_str) {
                    self.handle_arguments_delta(&root, delta, &mut out);
                }
            }
            Some("response.reasoning_text.delta" | "response.reasoning_summary_text.delta") => {
                if let Some(delta) = root.get("delta").and_then(Value::as_str) {
                    self.handle_reasoning_delta(&root, delta, &mut out);
                }
            }
            Some("response.output_item.done") => {
                self.handle_item_done(&root, &mut out);
            }
            Some("response.completed" | "response.incomplete") => {
                self.handle_terminal_response(root.get("response"), &mut out);
            }
            Some("response.failed") => {
                return Err(decode_failed_event(&root));
            }
            // Unknown event types (response.in_progress, response.output_text.done,
            // future additions, ...) are ignored tolerantly.
            _ => {}
        }

        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<LlmEvent>, LlmError> {
        let mut out = Vec::new();
        self.finish_into(&mut out);
        Ok(out)
    }
}

impl OpenAiResponsesStreamDecoder {
    /// Emit the `MessageStart` snapshot once. `response.created` carries the
    /// response object with id/model; content events arriving first (no
    /// payload) fall back to an empty snapshot, mirroring `OpenAiChatCodec`.
    fn ensure_started(&mut self, response: Option<&Value>, out: &mut Vec<LlmEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        let response = response.unwrap_or(&Value::Null);
        out.push(LlmEvent::MessageStart {
            response: Box::new(LlmResponse {
                id: response.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                model: response.get("model").and_then(Value::as_str).unwrap_or_default().to_string(),
                content: Vec::new(),
                stop_reason: None,
                usage: crate::Usage::default(),
                cost: None,
                provider_metadata: Value::Null,
            }),
        });
    }

    /// Wire `output_index` of an item-scoped event; missing → slot zero.
    fn output_index(root: &Value) -> u64 {
        root.get("output_index").and_then(Value::as_u64).unwrap_or(0)
    }

    fn handle_item_added(&mut self, root: &Value, out: &mut Vec<LlmEvent>) {
        self.ensure_started(None, out);
        let Some(item) = root.get("item") else {
            return;
        };
        // Only function_call items open their block eagerly: the start event
        // must carry call_id/name, which never appear in argument deltas.
        // message/reasoning items open lazily on their first delta instead.
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        self.has_function_call = true;
        let output_index = Self::output_index(root);
        if self.blocks.contains_key(&output_index) {
            return;
        }
        let index = self.next_index;
        self.next_index += 1;
        self.blocks.insert(output_index, BlockState { index, open: true });
        out.push(LlmEvent::ContentBlockStart {
            index,
            content_block: ContentBlock::ToolCall {
                id: item.get("call_id").and_then(Value::as_str).unwrap_or_default().to_string(),
                name: item.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                input: Value::Object(serde_json::Map::new()),
            },
        });
    }

    /// Look up the block for an item-scoped event, lazily opening it with
    /// `make_block` (and emitting the `ContentBlockStart`) on first contact.
    fn block_index(
        &mut self,
        root: &Value,
        make_block: fn() -> ContentBlock,
        out: &mut Vec<LlmEvent>,
    ) -> u32 {
        let output_index = Self::output_index(root);
        if let Some(state) = self.blocks.get(&output_index) {
            return state.index;
        }
        let index = self.next_index;
        self.next_index += 1;
        self.blocks.insert(output_index, BlockState { index, open: true });
        out.push(LlmEvent::ContentBlockStart {
            index,
            content_block: make_block(),
        });
        index
    }

    fn handle_text_delta(&mut self, root: &Value, delta: &str, out: &mut Vec<LlmEvent>) {
        if delta.is_empty() {
            return;
        }
        self.ensure_started(None, out);
        let index = self.block_index(
            root,
            || ContentBlock::Text {
                text: String::new(),
                cache_control: None,
            },
            out,
        );
        out.push(LlmEvent::ContentBlockDelta {
            index,
            delta: crate::ContentDelta::TextDelta { text: delta.to_string() },
        });
    }

    fn handle_arguments_delta(&mut self, root: &Value, delta: &str, out: &mut Vec<LlmEvent>) {
        if delta.is_empty() {
            return;
        }
        self.ensure_started(None, out);
        // Normally opened by output_item.added; an arguments delta arriving
        // first opens a ToolCall block with empty id/name (same tolerance as
        // OpenAiChatCodec fragment-first streams).
        let index = self.block_index(
            root,
            || ContentBlock::ToolCall {
                id: String::new(),
                name: String::new(),
                input: Value::Object(serde_json::Map::new()),
            },
            out,
        );
        out.push(LlmEvent::ContentBlockDelta {
            index,
            delta: crate::ContentDelta::InputJsonDelta {
                partial_json: delta.to_string(),
            },
        });
    }

    /// `response.reasoning_text.delta` and `response.reasoning_summary_text.delta`
    /// both target the item's single Reasoning block (one start per item).
    fn handle_reasoning_delta(&mut self, root: &Value, delta: &str, out: &mut Vec<LlmEvent>) {
        if delta.is_empty() {
            return;
        }
        self.ensure_started(None, out);
        let index = self.block_index(
            root,
            || ContentBlock::Reasoning {
                text: String::new(),
                signature: None,
            },
            out,
        );
        out.push(LlmEvent::ContentBlockDelta {
            index,
            delta: crate::ContentDelta::ThinkingDelta {
                thinking: delta.to_string(),
            },
        });
    }

    fn handle_item_done(&mut self, root: &Value, out: &mut Vec<LlmEvent>) {
        // Tolerate a done without an added (function_call still influences
        // the terminal stop reason).
        if root
            .get("item")
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            == Some("function_call")
        {
            self.has_function_call = true;
        }
        let output_index = Self::output_index(root);
        if let Some(state) = self.blocks.get_mut(&output_index) {
            if state.open {
                state.open = false;
                out.push(LlmEvent::ContentBlockStop { index: state.index });
            }
        }
    }

    /// `response.completed` / `response.incomplete`: the payload `response`
    /// object is a full Responses body, so the non-streaming stop-reason and
    /// usage normalization apply verbatim.
    fn handle_terminal_response(&mut self, response: Option<&Value>, out: &mut Vec<LlmEvent>) {
        self.ensure_started(response, out);
        if let Some(response) = response {
            self.stop_reason = map_stop_reason(response, self.has_function_call);
            self.usage = response.get("usage").map(normalize_usage);
        }
        self.finish_into(out);
    }

    fn finish_into(&mut self, out: &mut Vec<LlmEvent>) {
        if !self.started || self.done {
            return;
        }
        self.done = true;
        self.close_open_blocks(out);
        out.push(LlmEvent::MessageDelta {
            delta: crate::MessageDeltaPayload {
                stop_reason: self.stop_reason.clone(),
            },
            usage: self.usage.clone(),
        });
        out.push(LlmEvent::MessageStop);
    }

    fn close_open_blocks(&mut self, out: &mut Vec<LlmEvent>) {
        let mut open_indices: Vec<u32> = self
            .blocks
            .values()
            .filter(|state| state.open)
            .map(|state| state.index)
            .collect();
        open_indices.sort_unstable();
        for index in open_indices {
            out.push(LlmEvent::ContentBlockStop { index });
        }
        for state in self.blocks.values_mut() {
            state.open = false;
        }
    }
}

/// Map a `response.failed` event onto the error taxonomy by routing its
/// `response.error {code, message}` through the shared Chat-envelope error
/// decoder (the code vocabulary is shared; there is no HTTP status on a
/// stream failure, so the fallback is the generic 5xx mapping).
///
/// Two wire codes the shared decoder only recognizes by HTTP status are
/// pre-matched here (codex sse/responses.rs `response.failed` handling):
/// `rate_limit_exceeded` → [`LlmError::RateLimited`] with the retry-after
/// delay parsed from the message text, and `invalid_prompt` →
/// [`LlmError::InvalidRequest`] (non-retryable).
fn decode_failed_event(root: &Value) -> LlmError {
    let error = root
        .get("response")
        .and_then(|response| response.get("error"))
        .cloned()
        .unwrap_or(Value::Null);

    let code = error.get("code").and_then(Value::as_str).unwrap_or_default();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match code {
        "rate_limit_exceeded" => {
            return LlmError::RateLimited {
                retry_after: parse_retry_after_from_message(message),
                scope: None,
            };
        }
        "invalid_prompt" => {
            return LlmError::InvalidRequest {
                message: message.to_string(),
            };
        }
        _ => {}
    }

    super::openai::decode_error_response(&ProviderResponse {
        status: 500,
        headers: std::collections::BTreeMap::new(),
        body_json: serde_json::json!({ "error": error }),
        request_id: None,
    })
}

/// Parse the retry-after hint out of a `rate_limit_exceeded` message such as
/// "... Please try again in 11.054s. ...".
///
/// Plain-string port of the codex sse/responses.rs `rate_limit_regex`
/// (`(?i)try again in\s*(\d+(?:\.\d+)?)\s*(s|ms|seconds?)`): seconds (`s`,
/// `second`, `seconds`) and milliseconds (`ms`) units are recognized; any
/// other shape yields `None`.
fn parse_retry_after_from_message(message: &str) -> Option<std::time::Duration> {
    const NEEDLE: &str = "try again in";
    let lower = message.to_ascii_lowercase();
    let start = lower.find(NEEDLE)? + NEEDLE.len();
    let rest = lower[start..].trim_start();

    let digits_end = rest
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.')
        .map(|(i, c)| i + c.len_utf8())
        .last()?;
    let value: f64 = rest[..digits_end].parse().ok()?;

    let unit = rest[digits_end..].trim_start();
    if unit.starts_with("ms") {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        return Some(std::time::Duration::from_millis(value as u64));
    }
    if unit.starts_with('s') {
        // try_ variant: a degenerate digit run can parse to inf, which the
        // panicking from_secs_f64 constructor would abort on.
        return std::time::Duration::try_from_secs_f64(value).ok();
    }
    None
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
            | ContentBlock::AdvisorToolResult { .. }
            // cache_edits is an Anthropic-1P request directive only; never OpenAI.
            | ContentBlock::CacheEdits { .. } => {}
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
