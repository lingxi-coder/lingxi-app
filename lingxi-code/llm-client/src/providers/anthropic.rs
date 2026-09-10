use crate::reasoning_controls::{request_reasoning_intent, RequestReasoningIntent};
use crate::{
    normalize_anthropic_usage, ContentBlock, ContentDelta, LlmError, LlmEvent, LlmRequest,
    LlmResponse, MessageDeltaPayload, ProviderRequest, ProviderResponse, RawStreamFrame,
    ResponseFormat, StreamDecoder, ToolDeclaration, WireCodec,
};

use std::time::Duration;

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

    fn count_tokens_url(&self) -> String {
        format!(
            "{}/v1/messages/count_tokens",
            self.base_url.trim_end_matches('/')
        )
    }

    /// Encode a `count_tokens` request (same prompt shape, no generation
    /// controls).
    pub fn encode_count_tokens_request(
        &self,
        request: &LlmRequest,
    ) -> Result<ProviderRequest, LlmError> {
        let (body, json_string_overrides) = base_body(request)?;
        let mut provider_request =
            ProviderRequest::post_json(self.count_tokens_url(), Value::Object(body));
        provider_request.json_string_overrides = json_string_overrides;
        provider_request.headers.insert(
            "anthropic-version".to_string(),
            self.anthropic_version.clone(),
        );
        provider_request
            .headers
            .insert("content-type".to_string(), "application/json".to_string());
        Ok(provider_request)
    }

    /// Decode a `count_tokens` response into the input-token count.
    pub fn decode_count_tokens_response(
        &self,
        response: &ProviderResponse,
    ) -> Result<u64, LlmError> {
        if response.status >= 400 {
            return Err(decode_error_response(response));
        }
        response
            .body_json
            .get("input_tokens")
            .and_then(Value::as_u64)
            .ok_or_else(|| LlmError::InvalidRequest {
                message: "Anthropic count_tokens response missing input_tokens".to_string(),
            })
    }
}

impl WireCodec for AnthropicMessagesCodec {
    fn response_usage(
        &self,
        response: &ProviderResponse,
    ) -> Option<(crate::Usage, crate::ModelAttemptUsageCompleteness)> {
        let usage = response
            .body_json
            .get("usage")
            .filter(|value| value.is_object())
            .map(normalize_anthropic_usage)?;
        Some(crate::model_attempt::response_usage_observation(
            usage,
            response.status,
            "input_tokens",
            "output_tokens",
        ))
    }
    fn encode_request(&self, request: &LlmRequest) -> Result<ProviderRequest, LlmError> {
        let (mut body, json_string_overrides) = base_body(request)?;

        body.insert(
            "max_tokens".to_string(),
            Value::from(request.max_tokens.map_or(4096u64, u64::from)),
        );

        if let Some(temperature) = request.temperature {
            body.insert("temperature".to_string(), Value::from(temperature));
        }
        if let Some(top_p) = request.top_p {
            body.insert("top_p".to_string(), Value::from(top_p));
        }
        if !request.stop_sequences.is_empty() {
            body.insert(
                "stop_sequences".to_string(),
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

        if request.stream {
            body.insert("stream".to_string(), Value::Bool(true));
        }

        if let Some(tool_choice) = &request.tool_choice {
            body.insert("tool_choice".to_string(), encode_tool_choice(tool_choice));
        }

        // metadata.user_id — claude-code always sends `metadata: { user_id }`
        // (services/api/claude.ts:1699-1728). Emitted on the messages endpoint
        // only (not count_tokens, which is prompt-shape-only). `None` omits the
        // key, leaving wire bytes unchanged for callers that don't supply it.
        if let Some(metadata) = &request.metadata {
            body.insert(
                "metadata".to_string(),
                serde_json::json!({"user_id": metadata.user_id}),
            );
        }

        let mut provider_request =
            ProviderRequest::post_json(self.messages_url(), Value::Object(body));
        provider_request.json_string_overrides = json_string_overrides;
        provider_request.headers.insert(
            "anthropic-version".to_string(),
            self.anthropic_version.clone(),
        );
        provider_request
            .headers
            .insert("content-type".to_string(), "application/json".to_string());
        // Beta headers ACCUMULATE, comma-joined, matching the SDK's
        // `[...betas, "…"].toString()`. Overwriting instead would silently drop
        // one beta when a request needs two (a computer-use tool AND structured
        // output).
        let mut betas: Vec<String> = Vec::new();
        if let Some(beta) = computer_use_beta_header(request) {
            betas.push(beta);
        }
        if request.response_format.is_some() {
            betas.push(BETA_STRUCTURED_OUTPUTS.to_string());
        }
        if !betas.is_empty() {
            provider_request
                .headers
                .insert("anthropic-beta".to_string(), betas.join(","));
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

/// Prompt-shaped body fields shared by messages and `count_tokens`.
///
/// # Anthropic `response_format` / `output_config`
///
/// Anthropic's structured output is a BETA wire key: the body carries
/// `output_config: { format }` and the request carries
/// `anthropic-beta: structured-outputs-2025-12-15`. claude-code sends it via
/// `client.beta.messages.create` (`sideQuery.ts:190`); the equivalent here is
/// the same body key plus the beta header, since this codec talks to the REST
/// endpoint directly.
///
/// `JsonObject` is deliberately still rejected: the oracle only ever sends
/// `{type:"json_schema", schema}`, and there is no evidence the beta accepts a
/// bare JSON-object mode. Inventing one would be guessing at a wire contract.
fn base_body(
    request: &LlmRequest,
) -> Result<
    (
        serde_json::Map<String, Value>,
        std::collections::BTreeMap<String, Vec<u16>>,
    ),
    LlmError,
> {
    let mut json_string_overrides = std::collections::BTreeMap::new();
    let messages = request
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| encode_message(message, index, &mut json_string_overrides))
        .collect::<Result<Vec<_>, _>>()?;

    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(request.model.clone()));
    if let Some(format) = &request.response_format {
        body.insert(
            "output_config".to_string(),
            serde_json::json!({ "format": encode_output_format(format)? }),
        );
    }
    body.insert("messages".to_string(), Value::Array(messages));
    // Omitted when empty: an empty tools array changes prompt-cache keys.
    if !request.tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(request.tools.iter().map(encode_tool).collect()),
        );
    }
    if !request.system.is_empty() {
        let system: Vec<Value> = request
            .system
            .iter()
            .map(|block| {
                with_cache_control(
                    serde_json::json!({"type": "text", "text": block.text}),
                    block.cache_control,
                )
            })
            .collect();
        body.insert("system".to_string(), Value::Array(system));
    }
    let reasoning_intent = request_reasoning_intent(request);
    // Compact forks replay the parent's thinking and output effort together
    // (Claude Code 2.1.261). Other requests retain effort's existing precedence.
    if request.query_source.as_deref() == Some("compaction")
        && matches!(
            reasoning_intent,
            RequestReasoningIntent::Level(_) | RequestReasoningIntent::EffortBudget(_)
        )
        && crate::model::thinking::model_supports_thinking(&request.model)
    {
        match request.reasoning {
            Some(crate::ReasoningConfig::Adaptive)
                if crate::model::thinking::model_supports_adaptive_thinking(&request.model) =>
            {
                body.insert(
                    "thinking".to_string(),
                    serde_json::json!({"type": "adaptive"}),
                );
            }
            Some(crate::ReasoningConfig::Enabled { budget_tokens }) => {
                body.insert(
                    "thinking".to_string(),
                    serde_json::json!({"type": "enabled", "budget_tokens": budget_tokens}),
                );
            }
            _ => {}
        }
    }
    match reasoning_intent {
        RequestReasoningIntent::LegacyAdaptive => {
            body.insert(
                "thinking".to_string(),
                serde_json::json!({"type": "adaptive"}),
            );
        }
        RequestReasoningIntent::LegacyBudget(budget_tokens) => {
            body.insert(
                "thinking".to_string(),
                serde_json::json!({"type": "enabled", "budget_tokens": budget_tokens}),
            );
        }
        RequestReasoningIntent::Level(level) => {
            insert_output_effort(&mut body, Value::String(level));
        }
        RequestReasoningIntent::EffortBudget(tokens) => {
            insert_output_effort(&mut body, Value::from(tokens));
        }
        RequestReasoningIntent::Automatic
        | RequestReasoningIntent::Disabled
        | RequestReasoningIntent::Enabled => {}
    }
    if let Some(hint) = &request.context_hint {
        // claude-code `context_hint: {enabled, target_tokens_saved?}` — the
        // offer the server may reject with 422/424 to ask for a compact.
        body.insert("context_hint".to_string(), hint.clone());
    }
    if let Some(speed) = &request.speed {
        // claude-code fast mode: the top-level `speed` body key (read back by
        // `service::beta_context` to add the fast-mode beta). First-party only.
        body.insert("speed".to_string(), Value::String(speed.clone()));
    }
    Ok((body, json_string_overrides))
}

fn insert_output_effort(body: &mut serde_json::Map<String, Value>, effort: Value) {
    if let Some(oc) = body
        .entry("output_config".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
    {
        oc.insert("effort".to_string(), effort);
    }
}

fn encode_message(
    message: &crate::Message,
    message_index: usize,
    overrides: &mut std::collections::BTreeMap<String, Vec<u16>>,
) -> Result<Value, LlmError> {
    let content = message
        .content
        .iter()
        .enumerate()
        .map(|(index, block)| encode_content_block(block, message_index, index, overrides))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(serde_json::json!({
        "role": message.role,
        "content": content,
    }))
}

// Apply the Anthropic cache_control wrapper; no-op when None.
//
// Mirrors claude-code `getCacheControl` (`services/api/claude.ts:358-374`):
// always `{"type":"ephemeral"}`, plus `"ttl":"1h"` when the 1h-TTL gate is on,
// plus `"scope":"global"` when the block is the 1P global-scoped one. The plain
// `Ephemeral` (org default) emits neither extra key — byte-identical to the
// pre-feature `{"type":"ephemeral"}`.
fn with_cache_control(mut block: Value, cache_control: Option<crate::CacheControl>) -> Value {
    use crate::{CacheControl, CacheScope};
    match cache_control {
        None => {}
        Some(CacheControl::Ephemeral) => {
            block["cache_control"] = serde_json::json!({"type": "ephemeral"});
        }
        Some(CacheControl::EphemeralScoped { scope, ttl_1h }) => {
            let mut cc = serde_json::Map::new();
            cc.insert("type".into(), Value::String("ephemeral".into()));
            if ttl_1h {
                cc.insert("ttl".into(), Value::String("1h".into()));
            }
            if matches!(scope, Some(CacheScope::Global)) {
                cc.insert("scope".into(), Value::String("global".into()));
            }
            block["cache_control"] = Value::Object(cc);
        }
    }
    block
}

fn encode_content_block(
    block: &ContentBlock,
    message_index: usize,
    block_index: usize,
    overrides: &mut std::collections::BTreeMap<String, Vec<u16>>,
) -> Result<Value, LlmError> {
    match block {
        ContentBlock::Text {
            text,
            cache_control,
        } => Ok(with_cache_control(
            serde_json::json!({"type": "text", "text": text}),
            *cache_control,
        )),
        ContentBlock::TextJsUtf16 {
            text,
            utf16_code_units,
            cache_control,
        } => {
            overrides.insert(
                format!("/messages/{message_index}/content/{block_index}/text"),
                utf16_code_units.clone(),
            );
            Ok(with_cache_control(
                serde_json::json!({"type": "text", "text": text}),
                *cache_control,
            ))
        }
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
        ContentBlock::ToolResult {
            tool_call_id,
            output,
            is_error,
            cache_control,
            cache_reference,
        } => {
            let mut block = serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_call_id,
                "content": normalize_tool_result_content(output),
            });
            if let Some(units) = protocol::js_utf16::tool_result_units(output) {
                block["content"] = Value::String(String::from_utf16_lossy(&units));
                overrides.insert(
                    format!("/messages/{message_index}/content/{block_index}/content"),
                    units,
                );
            }
            if *is_error {
                block["is_error"] = Value::Bool(true);
            }
            // 1P experimental cache-editing tag (claude.ts:3201-3203). Emitted
            // only when the gate-armed request builder set it; `None` (the
            // default 3P path) leaves the key absent → byte-identical.
            if let Some(reference) = cache_reference {
                block["cache_reference"] = Value::String(reference.clone());
            }
            Ok(with_cache_control(block, *cache_control))
        }
        // 1P experimental cache-editing directive (claude.ts:3052-3055).
        // `{"type":"cache_edits","edits":[{"type":"delete","cache_reference":...}]}`.
        ContentBlock::CacheEdits { edits } => {
            let edits_json: Vec<Value> = edits
                .iter()
                .map(|e| match e {
                    crate::CacheEdit::Delete { cache_reference } => serde_json::json!({
                        "type": "delete",
                        "cache_reference": cache_reference,
                    }),
                })
                .collect();
            Ok(serde_json::json!({
                "type": "cache_edits",
                "edits": edits_json,
            }))
        }
        ContentBlock::Reasoning { text, signature } => {
            let Some(signature) = signature else {
                return Err(LlmError::InvalidRequest {
                    message: "Anthropic thinking blocks require a signature to round-trip"
                        .to_string(),
                });
            };
            Ok(serde_json::json!({
                "type": "thinking",
                "thinking": text,
                "signature": signature,
            }))
        }
        ContentBlock::RedactedThinking { data } => Ok(serde_json::json!({
            "type": "redacted_thinking",
            "data": data,
        })),
        ContentBlock::ImageUrl { url } => Ok(serde_json::json!({
            "type": "image",
            "source": {
                "type": "url",
                "url": url,
            },
        })),
        ContentBlock::Document { .. } => Err(LlmError::InvalidRequest {
            message: "AnthropicMessagesCodec does not encode document blocks yet".to_string(),
        }),
        // ServerToolUse round-trips back to the wire format (tool-use round-trip).
        ContentBlock::ServerToolUse { id, name, input } => Ok(serde_json::json!({
            "type": "server_tool_use",
            "id": id,
            "name": name,
            "input": input,
        })),
        // ConnectorText and AdvisorToolResult are server-generated; no client
        // use-case for encoding them back. Reject with a clear message, mirroring
        // Document handling.
        ContentBlock::ConnectorText { .. } => Err(LlmError::InvalidRequest {
            message: "AnthropicMessagesCodec does not encode connector_text blocks".to_string(),
        }),
        ContentBlock::AdvisorToolResult { .. } => Err(LlmError::InvalidRequest {
            message: "AnthropicMessagesCodec does not encode advisor_tool_result blocks"
                .to_string(),
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
        // A content-block ARRAY (e.g. an MCP result with image / resource blocks)
        // is sent VERBATIM as the Anthropic `tool_result.content` — claude-code
        // passes the MCP content array directly (text blocks stay separate,
        // images stay viewable). Other non-string shapes are stringified.
        arr @ Value::Array(_) => arr.clone(),
        other => Value::String(other.to_string()),
    }
}

fn encode_tool(tool: &ToolDeclaration) -> Value {
    // Hosted tool (computer use / web search / code execution): emit the typed
    // passthrough shape `{ "type", "name", ...extra }`. Ported 1:1 from codex
    // `liter-llm` `provider/anthropic.rs` hosted-tool passthrough, where hosted
    // tools carry a `type` and provider-specific fields (e.g. display dims)
    // rather than a caller-defined `input_schema`.
    if let Some(tool_type) = &tool.tool_type {
        let mut obj = serde_json::Map::new();
        obj.insert("type".to_string(), Value::String(tool_type.clone()));
        obj.insert("name".to_string(), Value::String(tool.name.clone()));
        for (key, value) in &tool.extra {
            obj.insert(key.clone(), value.clone());
        }
        return Value::Object(obj);
    }
    // Caller-defined tool. When `strict` is set (structured-output strict mode)
    // and the schema converts, send the strict schema + `strict: true`; on a
    // non-convertible schema, warn and fall back to a normal (non-strict) tool.
    // With `strict == false` (every tool by default) this produces the exact
    // `{name, description, input_schema}` object as before — byte-unchanged.
    let mut obj = serde_json::Map::new();
    obj.insert("name".to_string(), Value::String(tool.name.clone()));
    obj.insert(
        "description".to_string(),
        Value::String(tool.description.clone()),
    );
    if tool.strict {
        match crate::strict_schema::to_strict_schema(&tool.input_schema) {
            Ok(strict_schema) => {
                obj.insert("input_schema".to_string(), strict_schema);
                obj.insert("strict".to_string(), Value::Bool(true));
            }
            Err(reason) => {
                tracing::warn!(
                    "Tool {} has strict: true but its schema is not strict-compatible ({reason}); sending non-strict",
                    tool.name
                );
                obj.insert("input_schema".to_string(), tool.input_schema.clone());
            }
        }
    } else {
        obj.insert("input_schema".to_string(), tool.input_schema.clone());
    }
    if tool.defer_loading {
        obj.insert("defer_loading".to_string(), Value::Bool(true));
    }
    Value::Object(obj)
}

/// Anthropic beta header for the hosted computer-use tool. Ported 1:1 from
/// `structured-outputs-2025-12-15` — the beta that enables `output_config`.
const BETA_STRUCTURED_OUTPUTS: &str = "structured-outputs-2025-12-15";

/// Encode a [`ResponseFormat`] as Anthropic's `output_config.format`.
///
/// # Errors
/// [`LlmError::InvalidRequest`] for `JsonObject`, which this beta has no
/// documented encoding for.
fn encode_output_format(format: &ResponseFormat) -> Result<Value, LlmError> {
    match format {
        ResponseFormat::JsonSchema { schema } => Ok(serde_json::json!({
            "type": "json_schema",
            "schema": schema,
        })),
        ResponseFormat::JsonObject => Err(LlmError::InvalidRequest {
            message: "AnthropicMessagesCodec: response_format=json_object has no Anthropic \
                      encoding (the structured-outputs beta defines only json_schema)"
                .to_string(),
        }),
    }
}

/// codex `liter-llm` `provider/anthropic.rs` (`BETA_COMPUTER_USE`).
const BETA_COMPUTER_USE: &str = "computer-use-2025-01-24";

/// Hosted computer-use tool wire types. A request carrying either type must
/// send the `computer-use-2025-01-24` beta header. Mirrors codex `liter-llm`
/// `HOSTED_TOOL_TYPES` (computer-use entries).
const COMPUTER_USE_TOOL_TYPES: &[&str] = &["computer_20241022", "computer_use_20250124"];

/// Compute request-dependent Anthropic beta headers from the request's tools.
///
/// Ported 1:1 from codex `liter-llm` `provider/anthropic.rs::dynamic_headers`
/// (computer-use branch): a hosted `computer_20241022` / `computer_use_20250124`
/// tool adds `anthropic-beta: computer-use-2025-01-24`. Returns `None` when no
/// such tool is present so the header is omitted and wire bytes stay unchanged.
fn computer_use_beta_header(request: &LlmRequest) -> Option<String> {
    let has_computer_use = request.tools.iter().any(|tool| {
        tool.tool_type
            .as_deref()
            .is_some_and(|ty| COMPUTER_USE_TOOL_TYPES.contains(&ty))
    });
    has_computer_use.then(|| BETA_COMPUTER_USE.to_string())
}

fn decode_response_body(body_json: Value) -> Result<LlmResponse, LlmError> {
    let id = string_field(&body_json, "id")?;
    let model = string_field(&body_json, "model")?;
    let mut content = Vec::new();
    if let Some(items) = body_json.get("content").and_then(Value::as_array) {
        for item in items {
            if let Some(block) = decode_content_block(item)? {
                content.push(block);
            }
        }
    }
    let usage = body_json
        .get("usage")
        .map(normalize_anthropic_usage)
        .unwrap_or_default();
    let stop_reason = body_json
        .get("stop_reason")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    // Refusal `stop_details: {category, explanation}` (read before `body_json`
    // is moved into `provider_metadata`).
    let stop_details = decode_stop_details(body_json.get("stop_details"));

    Ok(LlmResponse {
        id,
        model,
        content,
        stop_reason,
        stop_details,
        usage,
        cost: None,
        provider_metadata: body_json,
    })
}

/// Decode the Anthropic `stop_details: {category, explanation}` object (present
/// on `stop_reason: "refusal"` responses + the streaming `message_delta.delta`).
fn decode_stop_details(value: Option<&Value>) -> Option<crate::StopDetails> {
    let obj = value?;
    if obj.is_null() {
        return None;
    }
    Some(crate::StopDetails {
        category: obj
            .get("category")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        explanation: obj
            .get("explanation")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

fn decode_content_block(value: &Value) -> Result<Option<ContentBlock>, LlmError> {
    match value.get("type").and_then(Value::as_str) {
        Some("text") => Ok(Some(ContentBlock::Text {
            text: string_field(value, "text")?,
            cache_control: None,
        })),
        Some("thinking") => Ok(Some(ContentBlock::Reasoning {
            text: string_field(value, "thinking")?,
            signature: value
                .get("signature")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        })),
        Some("redacted_thinking") => Ok(Some(ContentBlock::RedactedThinking {
            data: string_field(value, "data")?,
        })),
        Some("tool_use") => Ok(Some(ContentBlock::ToolCall {
            id: string_field(value, "id")?,
            name: string_field(value, "name")?,
            input: value.get("input").cloned().unwrap_or(Value::Null),
        })),
        Some("server_tool_use") => Ok(Some(ContentBlock::ServerToolUse {
            id: string_field(value, "id")?,
            name: string_field(value, "name")?,
            input: value.get("input").cloned().unwrap_or(Value::Null),
        })),
        Some("connector_text") => Ok(Some(ContentBlock::ConnectorText {
            connector_text: value
                .get("connector_text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            signature: value
                .get("signature")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        })),
        Some("advisor_tool_result") => Ok(Some(ContentBlock::AdvisorToolResult {
            tool_use_id: string_field(value, "tool_use_id")?,
            content: value.get("content").cloned().unwrap_or(Value::Null),
            is_error: value
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })),
        // Unknown block types must not break decoding; the raw payload stays
        // available through provider_metadata.
        Some(_other) => Ok(None),
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
        Some("content_block_start") => {
            let block_value =
                value
                    .get("content_block")
                    .ok_or_else(|| LlmError::InvalidRequest {
                        message: "Anthropic content_block_start missing content_block".to_string(),
                    })?;
            match decode_content_block(block_value)? {
                Some(content_block) => Ok(vec![LlmEvent::ContentBlockStart {
                    index: u32_field(value, "index")?,
                    content_block,
                }]),
                None => Ok(Vec::new()),
            }
        }
        Some("content_block_delta") => {
            let delta_value = value.get("delta").ok_or_else(|| LlmError::InvalidRequest {
                message: "Anthropic content_block_delta missing delta".to_string(),
            })?;
            match decode_content_delta(delta_value)? {
                Some(delta) => Ok(vec![LlmEvent::ContentBlockDelta {
                    index: u32_field(value, "index")?,
                    delta,
                }]),
                None => Ok(Vec::new()),
            }
        }
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
                stop_details: decode_stop_details(
                    value
                        .get("delta")
                        .and_then(|delta| delta.get("stop_details")),
                ),
            },
            usage: value.get("usage").map(normalize_anthropic_usage),
        }]),
        Some("message_stop") => Ok(vec![LlmEvent::MessageStop]),
        Some("ping") => Ok(Vec::new()),
        Some("error") => Err(decode_error_event(value)),
        // Anthropic's streaming contract requires clients to tolerate unknown
        // event types.
        Some(_other) => Ok(Vec::new()),
        None => Err(LlmError::InvalidRequest {
            message: "Anthropic stream frame missing type".to_string(),
        }),
    }
}

fn decode_error_event(value: &Value) -> LlmError {
    let (error_type, message) = error_envelope(value);
    // Status 0: a stream `error` event carries no HTTP status, so `makeMessage`
    // takes its no-status branch and the text stays unprefixed. That is exactly
    // what the oracle's `Htp` keys on (`if(e.status!==void 0)return!1`).
    let display = super::api_error_message(0, value, &message);
    map_error(error_type, &message, display, None)
}

fn decode_error_response(response: &ProviderResponse) -> LlmError {
    let retry_after = crate::retry::retry_after_from_headers(&response.headers);
    let (error_type, message) = error_envelope(&response.body_json);
    let display = super::api_error_message(response.status, &response.body_json, &message);
    if error_type.is_empty() {
        super::map_error_status(response.status, &message, display, retry_after)
    } else {
        map_error(error_type, &message, display, retry_after)
    }
}

fn error_envelope(value: &Value) -> (&str, String) {
    let error = value.get("error");
    (
        error
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// Parse the `token_gap` out of Anthropic's prompt-too-long error message.
///
/// Pattern: `(?i)prompt is too long[^0-9]*(\d+)\s*tokens?\s*>\s*(\d+)` — identical
/// to the orchestrator's `prompt_too_long.rs` regex. We do a manual no-regex parse
/// here to avoid adding a `regex` dependency to `llm-client`:
///
/// 1. Case-insensitively find "prompt is too long" in the message.
/// 2. Scan forward to the first digit run — that is `actual`.
/// 3. Skip the `tokens?` / `>` glyph run and parse the next digit run as `limit`.
/// 4. Return `actual - limit` when `actual > limit`, else `0` (unknown).
fn ptl_token_gap(message: &str) -> u64 {
    let lower = message.to_ascii_lowercase();
    let start = match lower.find("prompt is too long") {
        Some(i) => i + "prompt is too long".len(),
        None => return 0,
    };
    let rest = &message[start..];

    // Collect up to two digit runs.
    let mut nums: [u64; 2] = [0; 2];
    let mut found = 0usize;
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && found < 2 {
        if bytes[i].is_ascii_digit() {
            let start_i = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if let Ok(n) = rest[start_i..i].parse::<u64>() {
                nums[found] = n;
                found += 1;
            }
        } else {
            i += 1;
        }
    }

    if found >= 2 && nums[0] > nums[1] {
        nums[0] - nums[1]
    } else {
        0
    }
}

/// `message` classifies, `display` is stored — see [`super::map_error_status`].
fn map_error(
    error_type: &str,
    message: &str,
    display: String,
    retry_after: Option<Duration>,
) -> LlmError {
    match error_type {
        "authentication_error" => LlmError::Authentication { message: display },
        "permission_error" => LlmError::PermissionDenied { message: display },
        "not_found_error" => LlmError::ModelUnavailable,
        "rate_limit_error" => LlmError::RateLimited {
            retry_after,
            scope: None,
        },
        // 413 split (parity 2.1.212): a `request_too_large` whose message
        // mentions the context window is a token overflow → keep the
        // prompt-too-long / compaction path; otherwise it is accumulated
        // image/attachment bytes → the distinct `RequestTooLarge` notice.
        "request_too_large" if message.to_ascii_lowercase().contains("context window") => {
            LlmError::ContextOverflow { token_gap: 0 }
        }
        "request_too_large" => LlmError::RequestTooLarge,
        "invalid_request_error" if message.to_ascii_lowercase().contains("prompt is too long") => {
            LlmError::ContextOverflow {
                // The RAW message: the gap parser looks for a digit run, and a
                // leading status would be the first one it found.
                token_gap: ptl_token_gap(message),
            }
        }
        "invalid_request_error" => LlmError::InvalidRequest { message: display },
        "overloaded_error" => LlmError::Overloaded { repeated: false },
        // api_error, and unknown types stay retryable.
        _ => LlmError::ProviderInternal,
    }
}

fn decode_message_start(value: &Value) -> Result<LlmResponse, LlmError> {
    let message = value.get("message").unwrap_or(value);
    decode_response_body(message.clone())
}

fn decode_content_delta(value: &Value) -> Result<Option<ContentDelta>, LlmError> {
    match value.get("type").and_then(Value::as_str) {
        Some("text_delta") => Ok(Some(ContentDelta::TextDelta {
            text: string_field(value, "text")?,
        })),
        Some("input_json_delta") => Ok(Some(ContentDelta::InputJsonDelta {
            partial_json: string_field(value, "partial_json")?,
        })),
        Some("thinking_delta") => Ok(Some(ContentDelta::ThinkingDelta {
            thinking: string_field(value, "thinking")?,
        })),
        Some("signature_delta") => Ok(Some(ContentDelta::SignatureDelta {
            signature: string_field(value, "signature")?,
        })),
        Some("citations_delta") => Ok(Some(ContentDelta::CitationsDelta {
            citation: value.get("citation").cloned().unwrap_or(Value::Null),
        })),
        Some("connector_text_delta") => Ok(Some(ContentDelta::ConnectorTextDelta {
            connector_text: value
                .get("connector_text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })),
        // Unknown delta types are ignored, mirroring unknown event handling.
        Some(_other) => Ok(None),
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

#[cfg(test)]
mod effort_codec_tests {
    use super::{base_body, AnthropicMessagesCodec};
    use crate::{LlmError, LlmRequest, ResponseFormat, WireCodec};
    use serde_json::json;

    fn req_with_effort(effort: Option<serde_json::Value>) -> LlmRequest {
        LlmRequest {
            model: "claude-opus-4-8".into(),
            effort,
            ..Default::default()
        }
    }

    #[test]
    fn effort_emitted_as_output_config_effort() {
        // A level string.
        let (body, overrides) = base_body(&req_with_effort(Some(json!("high")))).unwrap();
        assert!(overrides.is_empty());
        assert_eq!(body["output_config"]["effort"], json!("high"));
        // An integer budget passes through verbatim.
        let (body, overrides) = base_body(&req_with_effort(Some(json!(8000)))).unwrap();
        assert!(overrides.is_empty());
        assert_eq!(body["output_config"]["effort"], json!(8000));
        // Unset ⇒ no output_config from effort (zero effect on existing requests).
        let (body, overrides) = base_body(&req_with_effort(None)).unwrap();
        assert!(overrides.is_empty());
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn compaction_preserves_inherited_thinking_alongside_effort() {
        let mut request = req_with_effort(Some(json!("high")));
        request.model = "claude-sonnet-4-6".into();
        request.reasoning = Some(crate::ReasoningConfig::Adaptive);
        request.query_source = Some("compaction".into());
        let (body, _) = base_body(&request).unwrap();
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
        assert_eq!(body["output_config"]["effort"], "high");

        request.model = "claude-sonnet-4-5".into();
        request.reasoning = Some(crate::ReasoningConfig::Enabled {
            budget_tokens: 8192,
        });
        let (body, _) = base_body(&request).unwrap();
        assert_eq!(
            body["thinking"],
            json!({"type": "enabled", "budget_tokens": 8192})
        );
        assert_eq!(body["output_config"]["effort"], "high");

        request.query_source = None;
        let (body, _) = base_body(&request).unwrap();
        assert!(
            body.get("thinking").is_none(),
            "other callers retain effort precedence"
        );

        request.query_source = Some("compaction".into());
        request.effort = Some(json!("disabled"));
        let (body, _) = base_body(&request).unwrap();
        assert!(body.get("thinking").is_none(), "explicit disable wins");

        request.effort = Some(json!("high"));
        request.model = "claude-3-opus-20240229".into();
        let (body, _) = base_body(&request).unwrap();
        assert!(
            body.get("thinking").is_none(),
            "unsupported models omit thinking"
        );
    }

    #[test]
    fn fast_mode_speed_emitted_as_top_level_key() {
        // `/fast` ON: the request carries `speed:"fast"` (read back by
        // `service::beta_context` to light the fast-mode beta).
        let req = LlmRequest {
            model: "claude-opus-4-8".into(),
            speed: Some("fast".into()),
            ..Default::default()
        };
        let (body, overrides) = base_body(&req).unwrap();
        assert!(overrides.is_empty());
        assert_eq!(body["speed"], json!("fast"));
        // Unset ⇒ no `speed` key (byte-identical to a pre-fast-mode request).
        let req = LlmRequest {
            model: "claude-opus-4-8".into(),
            ..Default::default()
        };
        let (body, overrides) = base_body(&req).unwrap();
        assert!(overrides.is_empty());
        assert!(body.get("speed").is_none());
    }

    #[test]
    fn structured_output_encodes_output_config_and_lights_the_beta() {
        // Anthropic's structured output is a BETA wire key: `output_config`
        // in the body plus `anthropic-beta: structured-outputs-2025-12-15`.
        let schema = json!({"type":"object","properties":{"a":{"type":"string"}}});
        let req = LlmRequest {
            model: "claude-opus-4-8".into(),
            response_format: Some(ResponseFormat::JsonSchema {
                schema: schema.clone(),
            }),
            ..Default::default()
        };
        let (body, overrides) = base_body(&req).unwrap();
        assert!(overrides.is_empty());
        assert_eq!(
            body["output_config"],
            json!({"format": {"type": "json_schema", "schema": schema}})
        );

        let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
        let encoded = codec.encode_request(&req).unwrap();
        assert_eq!(
            encoded.headers.get("anthropic-beta").map(String::as_str),
            Some("structured-outputs-2025-12-15")
        );

        // Unset ⇒ neither the key nor the header, so ordinary requests stay
        // byte-identical.
        let plain = LlmRequest {
            model: "claude-opus-4-8".into(),
            ..Default::default()
        };
        let (body, overrides) = base_body(&plain).unwrap();
        assert!(overrides.is_empty());
        assert!(body.get("output_config").is_none());
        assert!(codec
            .encode_request(&plain)
            .unwrap()
            .headers
            .get("anthropic-beta")
            .is_none());
    }

    #[test]
    fn json_object_mode_is_still_refused_for_anthropic() {
        // The beta defines only json_schema; inventing a bare JSON-object mode
        // would be guessing at a wire contract.
        let req = LlmRequest {
            model: "claude-opus-4-8".into(),
            response_format: Some(ResponseFormat::JsonObject),
            ..Default::default()
        };
        let err = base_body(&req).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("json_object")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn text_js_utf16_registers_override_and_serializes_exact_wire_body() {
        let req = LlmRequest {
            model: "claude-opus-4-8".into(),
            messages: vec![crate::Message {
                role: "user".into(),
                content: vec![crate::ContentBlock::TextJsUtf16 {
                    text: "A[]".into(),
                    utf16_code_units: vec![0x0041, 0xD83D, 0x005B, 0x005D],
                    cache_control: None,
                }],
            }],
            ..Default::default()
        };

        let (body, overrides) = base_body(&req).unwrap();
        assert_eq!(
            overrides.get("/messages/0/content/0/text"),
            Some(&vec![0x0041, 0xD83D, 0x005B, 0x005D])
        );
        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            json!("A[]"),
            "display-safe body remains valid Unicode"
        );

        let codec = AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01");
        let encoded = codec.encode_request(&req).unwrap();
        let wire = String::from_utf8(encoded.wire_body_bytes().unwrap()).unwrap();
        assert!(wire.contains("\"text\":\"A\\ud83d[]\""), "{wire}");
    }
}
