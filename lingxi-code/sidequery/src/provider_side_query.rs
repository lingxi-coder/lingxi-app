//! Concrete [`SideQueryClient`] backed by `llm_client::DefaultLlmClient`.
//!
//! A side query is a stateless one-shot LLM call (see [`crate::side_query`]).
//! [`ProviderSideQueryClient`] wires the [`SideQueryRequest`] DTO to the
//! Anthropic Messages endpoint and decodes the [`LlmResponse`] back into a
//! [`SideQueryResponse`]. It owns a [`DefaultLlmClient`] for routing, codec,
//! and auth middleware, plus an object-safe
//! [`Arc<dyn HttpTransport>`] handle so the whole client stays usable behind
//! `Arc<dyn SideQueryClient>` (`MemorySelector::new` takes exactly that).
//!
//! ## Field forwarding
//!
//! Forwards `model`, `system`, `messages`, `max_tokens`, `tools`, `temperature`,
//! plus `tool_choice`, `stop_sequences` and (cc 2.1.198) `thinking` — a
//! `Some` session [`llm_client::model::thinking::ThinkingConfig`] resolves
//! through the SAME `reasoning_for_request` rules as the main loop, so the
//! compaction fork call inherits the session's extended-thinking config.
//! `output_format` drives the structured-text decode (not a server-side
//! `response_format`); `max_retries` stays dropped. Existing callers pass
//! `None`/empty for the extras, so their wire stays byte-identical.
//!
//! ## System forwarding fix
//!
//! Unlike the previous `api_client::AnthropicProvider` path, this client NOW
//! correctly forwards the `system` field via [`llm_client::SystemBlock`], which
//! the Anthropic codec encodes as `"system": [{"type":"text","text":"..."}]`.
//! This fixes the pre-broken `text_response_decodes_text_usage_and_stop_reason`
//! test.

use crate::side_query::{SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse};
use async_trait::async_trait;
use llm_client::{
    AuthStrategy, Capabilities, ClientConfig, Credential, CredentialConfig, DefaultLlmClient,
    LlmRequest, ModelProfile, PricingConfig, ProtocolFamily, ProviderId, ProviderProfile,
    StaticCredentialProvider, SystemBlock,
};
use llm_client::LlmTransportBridge;
use protocol::{HttpRequest, HttpResponse};
use std::sync::Arc;
use traits::http::{RawByteStream, SseStream};
use traits::{HttpError, HttpTransport};

/// Default Anthropic API base URL used when the caller passes `None`.
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Static credential id used inside the internal config for a static API key.
const SIDEQUERY_CRED_ID: &str = "sidequery_key";

/// Sized newtype adapter around an `Arc<dyn HttpTransport>`.
///
/// `DefaultLlmClient::execute` is generic over `T: Transport` and carries an
/// implicit `Sized` bound, so an unsized `&dyn HttpTransport` cannot be passed
/// directly. Wrapping the trait object in this sized newtype (which itself
/// implements `HttpTransport` by delegating to the inner `Arc`) lets the client
/// store an object-safe handle yet still satisfy the generic, sized bound.
struct ArcTransport(Arc<dyn HttpTransport>);

#[async_trait]
impl HttpTransport for ArcTransport {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.0.request(req).await
    }

    async fn stream_sse(&self, req: HttpRequest) -> Result<SseStream, HttpError> {
        self.0.stream_sse(req).await
    }

    async fn stream_raw_bytes(&self, req: HttpRequest) -> Result<RawByteStream, HttpError> {
        self.0.stream_raw_bytes(req).await
    }
}

/// One-shot [`SideQueryClient`] that routes through Anthropic's Messages API.
///
/// Construct with [`Self::new`] for the common case. The `from_provider`
/// constructor is no longer available; callers that previously used it should
/// switch to [`Self::new`].
pub struct ProviderSideQueryClient {
    /// `DefaultLlmClient` owning routing, codec, and auth middleware.
    client: DefaultLlmClient,
    /// Object-safe transport handle. Stored as `Arc<dyn HttpTransport>` (not a
    /// generic `T`) so the struct is usable behind `Arc<dyn SideQueryClient>`.
    transport: Arc<dyn HttpTransport>,
}

impl ProviderSideQueryClient {
    /// Build a client from raw credentials.
    ///
    /// `None` for `base_url` uses `https://api.anthropic.com`.
    ///
    /// # Panics
    ///
    /// Panics if the internal config is structurally invalid (unreachable in
    /// normal use — the config is built from known-good constants).
    #[must_use]
    pub fn new(
        api_key: impl Into<String>,
        base_url: Option<String>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        let api_key = api_key.into();
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        let config = ClientConfig {
            providers: vec![ProviderProfile {
                provider_id: ProviderId::AnthropicFirstParty,
                profile_name: "anthropic".to_string(),
                base_url,
                protocol: ProtocolFamily::AnthropicMessages,
                auth: AuthStrategy::ApiKey,
                credential: CredentialConfig::Static {
                    id: SIDEQUERY_CRED_ID.to_string(),
                },
                pricing: PricingConfig::default(),
                signing: None,
                azure: None,
                supports_websockets: false,
                supports_websocket_compression: false,
                websocket_connect_timeout_ms: None,
                // Wildcard model support: sidequery uses any model string the
                // caller passes (e.g. "claude-haiku-4-5" for memory summaries,
                // "claude-opus-4-6" for compaction). We register a catch-all
                // entry keyed on the empty prefix so that ANY model string is
                // accepted, then override at request time with the actual model.
                //
                // Because `DefaultLlmClient::prepare` resolves models by exact
                // `display_model` or `aliases` match, we must register the
                // models that callers actually request. The known set is:
                //   - claude-haiku-4-5 (memory selector)
                //   - claude-opus-4-6  (compaction)
                // We register both here (plus common aliases) so the registry
                // resolves them. Unknown model strings will fail with
                // `LlmError::ModelUnavailable` — the caller should use a known
                // model id.
                models: sidequery_model_table(),
            }],
        };

        let cred_provider = Arc::new(StaticCredentialProvider::new(Credential::ApiKey(api_key)));
        let client = DefaultLlmClient::from_config(config)
            .expect("sidequery ClientConfig is structurally valid")
            .with_credential_provider(cred_provider);

        Self { client, transport }
    }
}

/// Build the minimal model table for side-query callers.
///
/// Side queries use "claude-haiku-4-5" (memory selector) and
/// "claude-opus-4-6" (compaction). All entries get the same capability set:
/// streaming=false (side queries are always non-streaming), tools, vision,
/// documents, no reasoning.
fn sidequery_model_table() -> Vec<ModelProfile> {
    fn model(display: &str, billing: &str, aliases: &[&str]) -> ModelProfile {
        ModelProfile {
            display_model: display.to_string(),
            request_model: display.to_string(),
            billing_model: billing.to_string(),
            aliases: aliases.iter().map(|s| (*s).to_string()).collect(),
            description: None,
            capabilities: Capabilities {
                streaming: false,
                tools: true,
                vision: true,
                documents: true,
                // cc 2.1.198: the compaction fork call inherits the session
                // extended-thinking config, so the side-query route must
                // accept a `reasoning` field (requests without one are
                // unaffected by this capability flag).
                reasoning: true,
                structured_output: false,
            },
        }
    }

    vec![
        // Memory-selector model
        model("claude-haiku-4-5", "claude-haiku-4-5", &[]),
        model(
            "claude-haiku-4-20250307",
            "claude-haiku-4",
            &["claude-haiku-4", "claude-haiku"],
        ),
        // Compaction model (AutocompactConfig::default)
        model("claude-opus-4-6", "claude-opus-4-6", &[]),
        model("claude-opus-4-7", "claude-opus-4-7", &[]),
        // Broad Sonnet/Opus/Haiku coverage for callers using any model string
        model("claude-sonnet-4-6", "claude-sonnet-4-6", &[]),
        model(
            "claude-opus-4-20250514",
            "claude-opus-4",
            &["claude-opus-4", "claude-opus"],
        ),
        model(
            "claude-sonnet-4-20250514",
            "claude-sonnet-4",
            &["claude-sonnet-4", "claude-sonnet", "claude"],
        ),
    ]
}

/// Decode an [`llm_client::LlmResponse`] into the side-query response shape.
///
/// * `Text` blocks are concatenated into the flattened `text`.
/// * `ToolCall` blocks become `{"id", "name", "input"}` JSON in `tool_calls`.
/// * Other block kinds (reasoning, server tool use, connector text, advisor
///   tool result, redacted thinking) are ignored for side queries.
/// * `structured` is populated only when `want_structured` is set (the caller
///   requested an `output_format`): the accumulated text is best-effort parsed
///   as JSON. A non-JSON body leaves `structured` as `None` rather than
///   erroring — `MemorySelector` tolerates `None` (empty selection), so the
///   best-effort path is the safer default.
/// * `usage` maps `llm_client::Usage.billable_tokens` → `cost::Usage` with the
///   same cross-naming the provider's own cost path uses: API `cache_write` →
///   cost `cache_write`, API `cache_read` → cost `cache_read`.
fn decode_response(resp: llm_client::LlmResponse, want_structured: bool) -> SideQueryResponse {
    let mut text_acc = String::new();
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();

    for block in resp.content {
        match block {
            llm_client::ContentBlock::Text { text, .. } => text_acc.push_str(&text),
            llm_client::ContentBlock::ToolCall { id, name, input } => {
                tool_calls.push(serde_json::json!({
                    "id": id,
                    "name": name,
                    "input": input,
                }));
            }
            // Side queries ignore reasoning / server-tool / connector / advisor
            // / redacted-thinking blocks.
            llm_client::ContentBlock::Reasoning { .. }
            | llm_client::ContentBlock::RedactedThinking { .. }
            | llm_client::ContentBlock::ServerToolUse { .. }
            | llm_client::ContentBlock::ConnectorText { .. }
            | llm_client::ContentBlock::AdvisorToolResult { .. }
            | llm_client::ContentBlock::Image { .. }
            | llm_client::ContentBlock::ImageUrl { .. }
            | llm_client::ContentBlock::Document { .. }
            | llm_client::ContentBlock::ToolResult { .. }
            // cache_edits is a request-only directive — never in a response.
            | llm_client::ContentBlock::CacheEdits { .. } => {}
        }
    }

    let text = (!text_acc.is_empty()).then(|| text_acc.clone());
    let structured = if want_structured {
        serde_json::from_str::<serde_json::Value>(&text_acc).ok()
    } else {
        None
    };

    let bt = resp.usage.billable_tokens;
    let usage = cost::Usage {
        tokens: cost::TokenUsage {
            input: bt.input,
            output: bt.output,
            cache_read: bt.cache_read,
            cache_write: bt.cache_write,
            cache_write_1h: 0,
            reasoning_output: bt.reasoning_output,
        },
        server_tool_use: None,
        speed: None,
    };

    SideQueryResponse {
        text,
        structured,
        tool_calls,
        usage,
        stop_reason: resp.stop_reason,
    }
}

#[async_trait]
impl SideQueryClient for ProviderSideQueryClient {
    async fn query(&self, request: SideQueryRequest) -> Result<SideQueryResponse, SideQueryError> {
        // Build the LlmRequest from the SideQueryRequest DTO.
        let system: Vec<SystemBlock> = request
            .system_prompt
            .as_deref()
            .map(|s| vec![SystemBlock::text(s)])
            .unwrap_or_default();

        // Convert protocol::ConversationMessage → llm_client::Message.
        // We inline a minimal conversion here so sidequery does not need to
        // depend on `agent` (which depends back on sidequery — a cycle).
        let messages = convert_messages(request.messages)?;

        // Convert JSON tool declarations → llm_client::ToolDeclaration.
        let tools = convert_tool_declarations(request.tools)?;

        // thinking (cc 2.1.198): a `Some` session config resolves through the
        // SAME `reasoning_for_request` rules `ApiService::build_request`
        // applies to every main-loop/subagent request (adaptive vs fixed
        // budget, model predicates, env kill switches, max_tokens-1 clamp) —
        // the compaction fork call therefore inherits the session's
        // extended-thinking config. `None` (all utility callers) keeps the
        // wire byte-identical to before (no `thinking` field).
        let reasoning = request.thinking.and_then(|t| {
            llm_client::model::thinking::reasoning_for_request(
                t,
                &request.model,
                Some(request.max_tokens),
            )
        });

        let llm_req = LlmRequest {
            model: request.model,
            system,
            messages,
            tools,
            // output_format drives the structured text decode (NOT
            // response_format); max_retries is a caller-side budget.
            tool_choice: convert_tool_choice(request.tool_choice.as_ref()),
            stop_sequences: request.stop_sequences,
            max_tokens: Some(request.max_tokens),
            temperature: request.temperature.map(f64::from),
            reasoning,
            ..LlmRequest::default()
        };

        // Route through the transport bridge so the existing Arc<dyn
        // HttpTransport> is usable as an llm_client::Transport.
        let arc_transport = ArcTransport(Arc::clone(&self.transport));
        let bridge = LlmTransportBridge::new(arc_transport);

        let resp = self.client.execute(&llm_req, &bridge).await?;

        Ok(decode_response(resp, request.output_format.is_some()))
    }
}

// ── Inline message/tool conversion (no dep on `agent` crate) ─────────────────

fn convert_messages(
    messages: Vec<protocol::ConversationMessage>,
) -> Result<Vec<llm_client::Message>, llm_client::LlmError> {
    messages.into_iter().map(convert_one_message).collect()
}

fn convert_one_message(
    msg: protocol::ConversationMessage,
) -> Result<llm_client::Message, llm_client::LlmError> {
    match msg {
        protocol::ConversationMessage::User { content, .. } => Ok(llm_client::Message {
            role: "user".to_string(),
            content: content
                .into_iter()
                .map(convert_content_block)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        protocol::ConversationMessage::Assistant { content, .. } => Ok(llm_client::Message {
            role: "assistant".to_string(),
            content: content
                .into_iter()
                .map(convert_content_block)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        protocol::ConversationMessage::System { .. } => Err(llm_client::LlmError::InvalidRequest {
            message:
                "System messages must not appear in the messages vec; pass them via system_prompt"
                    .to_string(),
        }),
    }
}

fn convert_content_block(
    block: protocol::ContentBlock,
) -> Result<llm_client::ContentBlock, llm_client::LlmError> {
    match block {
        protocol::ContentBlock::Text { text } => Ok(llm_client::ContentBlock::Text {
            text,
            cache_control: None,
        }),
        protocol::ContentBlock::ToolUse {
            id,
            name,
            input,
            provider_id,
        } => Ok(llm_client::ContentBlock::ToolCall {
            // Replay the verbatim provider id when preserved (see agent::convert).
            id: provider_id.unwrap_or_else(|| id.to_string()),
            name,
            input,
        }),
        protocol::ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            provider_tool_use_id,
            content_blocks,
        } => Ok(llm_client::ContentBlock::ToolResult {
            tool_call_id: provider_tool_use_id.unwrap_or_else(|| tool_use_id.to_string()),
            // A structured content-block array (e.g. MCP image/resource) rides
            // as the `Value::Array` output and is emitted verbatim; plain text
            // stays a `Value::String`.
            output: content_blocks.map_or_else(
                || serde_json::Value::String(content),
                serde_json::Value::Array,
            ),
            is_error,
            cache_control: None,
            cache_reference: None,
        }),
        protocol::ContentBlock::Thinking {
            thinking,
            signature,
        } => Ok(llm_client::ContentBlock::Reasoning {
            text: thinking,
            signature,
        }),
        protocol::ContentBlock::Image { source } => convert_image(source),
        protocol::ContentBlock::Document { source } => convert_document(source),
        // Low-frequency server-side blocks: replayed verbatim into the request
        // so the provider round-trips them (see agent::convert::convert_block).
        protocol::ContentBlock::RedactedThinking { data } => {
            Ok(llm_client::ContentBlock::RedactedThinking { data })
        }
        protocol::ContentBlock::ServerToolUse { id, name, input } => {
            Ok(llm_client::ContentBlock::ServerToolUse { id, name, input })
        }
        protocol::ContentBlock::ConnectorText {
            connector_text,
            signature,
        } => Ok(llm_client::ContentBlock::ConnectorText {
            connector_text,
            signature,
        }),
        protocol::ContentBlock::AdvisorToolResult {
            tool_use_id,
            content,
            is_error,
        } => Ok(llm_client::ContentBlock::AdvisorToolResult {
            tool_use_id,
            content,
            is_error,
        }),
    }
}

fn convert_image(
    source: protocol::ImageSource,
) -> Result<llm_client::ContentBlock, llm_client::LlmError> {
    match source {
        protocol::ImageSource::Base64 { media_type, data } => {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(|e| llm_client::LlmError::InvalidRequest {
                    message: format!("Image base64 decode failed: {e}"),
                })?;
            Ok(llm_client::ContentBlock::Image { media_type, bytes })
        }
        protocol::ImageSource::Url { url } => Ok(llm_client::ContentBlock::ImageUrl { url }),
    }
}

fn convert_document(
    source: protocol::DocumentSource,
) -> Result<llm_client::ContentBlock, llm_client::LlmError> {
    match source {
        protocol::DocumentSource::Base64 { media_type, data } => {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(|e| llm_client::LlmError::InvalidRequest {
                    message: format!("Document base64 decode failed: {e}"),
                })?;
            Ok(llm_client::ContentBlock::Document { media_type, bytes })
        }
    }
}

fn convert_tool_declarations(
    tools: Vec<serde_json::Value>,
) -> Result<Vec<llm_client::ToolDeclaration>, llm_client::LlmError> {
    tools.into_iter().map(convert_one_tool).collect()
}

/// Map the DTO's JSON `tool_choice` to [`llm_client::ToolChoice`]. Accepts the
/// Anthropic wire shapes: `{"type":"auto"}`, `{"type":"any"}`,
/// `{"type":"none"}`, `{"type":"tool","name":N}`. Unknown / absent ⇒ `None`
/// (provider default), so the memory selector that passes `None` is unchanged.
fn convert_tool_choice(choice: Option<&serde_json::Value>) -> Option<llm_client::ToolChoice> {
    let ty = choice?.get("type").and_then(serde_json::Value::as_str)?;
    match ty {
        "auto" => Some(llm_client::ToolChoice::Auto),
        "none" => Some(llm_client::ToolChoice::None),
        "any" | "required" => Some(llm_client::ToolChoice::Required),
        "tool" => choice?
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(|name| llm_client::ToolChoice::Tool { name: name.into() }),
        _ => None,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn convert_one_tool(
    value: serde_json::Value,
) -> Result<llm_client::ToolDeclaration, llm_client::LlmError> {
    let name = value
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| llm_client::LlmError::InvalidRequest {
            message: "Tool declaration missing required string field: name".to_string(),
        })?
        .to_string();

    let description = value
        .get("description")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| llm_client::LlmError::InvalidRequest {
            message: "Tool declaration missing required string field: description".to_string(),
        })?
        .to_string();

    let input_schema = value
        .get("input_schema")
        .cloned()
        .filter(|v| !v.is_null())
        .ok_or_else(|| llm_client::LlmError::InvalidRequest {
            message: "Tool declaration missing required field: input_schema".to_string(),
        })?;

    Ok(llm_client::ToolDeclaration {
        name,
        description,
        input_schema,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::purposes::QuerySource;
    use protocol::{ConversationMessage, MessageId};
    use std::sync::Mutex;

    /// Minimal in-process [`HttpTransport`] that returns a single scripted
    /// 200 body. Kept local so the sidequery crate needs no extra
    /// dev-dependency (mirrors the `OneShot` template in `traits/src/http.rs`).
    struct StubTransport {
        body: String,
        received: Mutex<Vec<HttpRequest>>,
    }

    impl StubTransport {
        fn new(body: impl Into<String>) -> Self {
            Self {
                body: body.into(),
                received: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl HttpTransport for StubTransport {
        async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
            self.received.lock().unwrap().push(req);
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: self.body.clone(),
            })
        }

        async fn stream_sse(&self, _req: HttpRequest) -> Result<SseStream, HttpError> {
            Err(HttpError::InvalidRequest(
                "sse not used in side query".into(),
            ))
        }
    }

    fn req(output_format: Option<serde_json::Value>) -> SideQueryRequest {
        SideQueryRequest {
            model: "claude-haiku-4-5".into(),
            system_prompt: Some("system".into()),
            messages: vec![ConversationMessage::user(MessageId::new(), "hi".into())],
            tools: vec![],
            tool_choice: None,
            output_format,
            max_tokens: 1024,
            max_retries: 2,
            temperature: Some(0.0),
            thinking: None,
            stop_sequences: vec![],
            query_source: QuerySource::MemorySelector,
            skip_system_prompt_prefix: false,
        }
    }

    #[tokio::test]
    async fn text_response_decodes_text_usage_and_stop_reason() {
        // A plain-text answer with a populated usage block.
        let body = serde_json::json!({
            "id": "msg_01",
            "model": "claude-haiku-4-5",
            "content": [
                { "type": "text", "text": "hello " },
                { "type": "text", "text": "world" }
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 12,
                "output_tokens": 7,
                "cache_creation_input_tokens": 3,
                "cache_read_input_tokens": 5
            }
        })
        .to_string();

        let transport = Arc::new(StubTransport::new(body));
        let client = ProviderSideQueryClient::new("sk-test", None, transport.clone());

        let resp = client.query(req(None)).await.expect("query ok");

        assert_eq!(resp.text.as_deref(), Some("hello world"));
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        // No output_format requested -> structured stays None.
        assert!(resp.structured.is_none());
        assert!(resp.tool_calls.is_empty());
        // Usage mapping: API cache_creation -> cost cache_write,
        // API cache_read -> cost cache_read.
        assert_eq!(resp.usage.tokens.input, 12);
        assert_eq!(resp.usage.tokens.output, 7);
        assert_eq!(resp.usage.tokens.cache_write, 3);
        assert_eq!(resp.usage.tokens.cache_read, 5);
        assert_eq!(resp.usage.tokens.reasoning_output, 0);

        // Wire-request mapping: inspect the body the provider actually sent.
        // Exactly one request issued through the injected transport.
        let received = transport.received.lock().unwrap();
        assert_eq!(received.len(), 1);
        let raw_body = received[0].body.as_deref().expect("request has a body");
        let body: serde_json::Value = serde_json::from_str(raw_body).expect("request body is JSON");

        // The forwarded fields reach the wire body.
        assert_eq!(body["model"].as_str(), Some("claude-haiku-4-5"));
        // llm-client encodes system as an array of blocks:
        // `"system": [{"type":"text","text":"system"}]`
        // (not the legacy string form the old api-client used).
        let system_arr = body["system"].as_array().expect("system is a JSON array");
        assert_eq!(system_arr.len(), 1);
        assert_eq!(system_arr[0]["type"].as_str(), Some("text"));
        assert_eq!(system_arr[0]["text"].as_str(), Some("system"));
        assert_eq!(
            body["messages"].as_array().map(Vec::len),
            Some(1),
            "the single request message is forwarded"
        );

        // `max_tokens` now carries the request's value (1024), and the request's
        // `temperature: Some(0.0)` is forwarded — both via the opts entrypoint.
        assert_eq!(
            body["max_tokens"].as_u64(),
            Some(1024),
            "request max_tokens (1024) forwarded, not the legacy 4096"
        );
        assert_eq!(
            body["temperature"].as_f64(),
            Some(0.0),
            "request temperature (0.0) forwarded"
        );
        // `tools` is empty on this request, so the `is_empty` guard keeps the
        // body key absent. The remaining DTO fields stay unforwardable.
        assert!(
            body.get("tools").is_none(),
            "tools absent when the request list is empty"
        );
        assert!(body.get("tool_choice").is_none(), "tool_choice dropped");
        assert!(
            body.get("stop_sequences").is_none(),
            "stop_sequences dropped"
        );
    }

    #[tokio::test]
    async fn structured_output_parses_json_text() {
        // JSON-shaped text + an output_format request -> structured populated.
        let body = serde_json::json!({
            "id": "msg_02",
            "model": "claude-haiku-4-5",
            "content": [
                { "type": "text", "text": "{\"filenames\":[\"a.md\",\"b.md\"]}" }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        })
        .to_string();

        let transport = Arc::new(StubTransport::new(body));
        let client = ProviderSideQueryClient::new("sk-test", None, transport);

        let resp = client
            .query(req(Some(serde_json::json!({"type": "json_schema"}))))
            .await
            .expect("query ok");

        let structured = resp.structured.expect("structured present");
        let names = structured["filenames"].as_array().expect("array");
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].as_str(), Some("a.md"));
    }

    #[tokio::test]
    async fn structured_output_falls_back_to_none_on_non_json() {
        // output_format requested but the model returns plain (non-JSON) text:
        // the best-effort parse leaves `structured` None and the query still
        // succeeds (the MemorySelector-tolerance contract).
        let body = serde_json::json!({
            "id": "msg_02b",
            "model": "claude-haiku-4-5",
            "content": [
                { "type": "text", "text": "sorry, no files" }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        })
        .to_string();

        let transport = Arc::new(StubTransport::new(body));
        let client = ProviderSideQueryClient::new("sk-test", None, transport);

        let resp = client
            .query(req(Some(serde_json::json!({"type": "json_schema"}))))
            .await
            .expect("query ok");

        // Non-JSON body + output_format -> structured stays None, no error.
        assert!(resp.structured.is_none());
        assert_eq!(resp.text.as_deref(), Some("sorry, no files"));
    }

    #[tokio::test]
    async fn ignored_content_blocks_are_dropped() {
        // A mixed-content response: thinking / server_tool_use / connector_text
        // / advisor_tool_result are all dropped, while the interleaved text and
        // tool_use survive.
        let tu = "11111111-2222-3333-4444-555555555555";
        let body = serde_json::json!({
            "id": "msg_mixed",
            "model": "claude-haiku-4-5",
            "content": [
                { "type": "thinking", "thinking": "let me think" },
                { "type": "text", "text": "answer" },
                { "type": "server_tool_use", "id": "srv1", "name": "advisor", "input": {} },
                { "type": "tool_use", "id": tu, "name": "search", "input": { "q": "x" } },
                { "type": "connector_text", "connector_text": "from connector" },
                { "type": "advisor_tool_result", "tool_use_id": "srv1", "content": {}, "is_error": false }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        })
        .to_string();

        let transport = Arc::new(StubTransport::new(body));
        let client = ProviderSideQueryClient::new("sk-test", None, transport);

        let resp = client.query(req(None)).await.expect("query ok");

        // Only the `text` block reaches `text` (thinking / connector dropped).
        assert_eq!(resp.text.as_deref(), Some("answer"));
        // Only the client-side `tool_use` reaches `tool_calls` (server_tool_use
        // / advisor_tool_result dropped).
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0]["name"].as_str(), Some("search"));
    }

    #[tokio::test]
    async fn tool_use_response_surfaces_in_tool_calls() {
        // ToolUseId is `#[serde(transparent)]` over a UUID, so the wire id is a
        // bare UUID string.
        let tu = "11111111-2222-3333-4444-555555555555";
        let body = serde_json::json!({
            "id": "msg_03",
            "model": "claude-haiku-4-5",
            "content": [
                {
                    "type": "tool_use",
                    "id": tu,
                    "name": "search",
                    "input": { "q": "rust" }
                }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 4, "output_tokens": 2 }
        })
        .to_string();

        let transport = Arc::new(StubTransport::new(body));
        let client = ProviderSideQueryClient::new("sk-test", None, transport);

        let resp = client.query(req(None)).await.expect("query ok");

        // No text blocks -> flattened text is None.
        assert!(resp.text.is_none());
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(resp.tool_calls.len(), 1);
        let call = &resp.tool_calls[0];
        assert_eq!(call["name"].as_str(), Some("search"));
        assert_eq!(call["input"]["q"].as_str(), Some("rust"));
        // ToolUseId serialises transparently as the bare UUID string.
        assert_eq!(call["id"].as_str(), Some(tu));
    }

    #[tokio::test]
    async fn forwards_tool_choice_stop_sequences_and_thinking() {
        let body = serde_json::json!({
            "id": "msg_fwd", "model": "claude-haiku-4-5",
            "content": [{ "type": "text", "text": "ok" }],
            "stop_reason": "end_turn", "usage": { "input_tokens": 1, "output_tokens": 1 }
        }).to_string();
        let transport = Arc::new(StubTransport::new(body));
        let client = ProviderSideQueryClient::new("sk-test", None, transport.clone());
        let mut r = req(None);
        r.tool_choice = Some(serde_json::json!({ "type": "any" }));
        r.stop_sequences = vec!["STOP".into()];
        client.query(r).await.expect("query ok");
        let received = transport.received.lock().unwrap();
        let body: serde_json::Value =
            serde_json::from_str(received[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["tool_choice"]["type"].as_str(), Some("any"), "any→required");
        assert_eq!(body["stop_sequences"][0].as_str(), Some("STOP"));
    }

    /// cc 2.1.198 "Subagents + compaction inherit extended thinking config" —
    /// the COMPACTION seam half, wire level: a `Some` session thinking config
    /// resolves through the SAME `reasoning_for_request` rules as a main-loop
    /// request and lands as the request's `thinking` field.
    #[tokio::test]
    async fn session_thinking_config_rides_on_the_wire() {
        let body = serde_json::json!({
            "id": "msg_think", "model": "claude-opus-4-6",
            "content": [{ "type": "text", "text": "ok" }],
            "stop_reason": "end_turn", "usage": { "input_tokens": 1, "output_tokens": 1 }
        }).to_string();
        let transport = Arc::new(StubTransport::new(body));
        let client = ProviderSideQueryClient::new("sk-test", None, transport.clone());
        let mut r = req(None);
        // opus-4-6 is in the adaptive-thinking set → the session Adaptive
        // intent renders exactly like a main-loop turn: {"type":"adaptive"}.
        r.model = "claude-opus-4-6".into();
        r.thinking = Some(llm_client::model::thinking::ThinkingConfig::default());
        client.query(r).await.expect("query ok");
        let received = transport.received.lock().unwrap();
        let body: serde_json::Value =
            serde_json::from_str(received[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body["thinking"],
            serde_json::json!({ "type": "adaptive" }),
            "the inherited session config renders like a main-loop request"
        );
    }

    /// `thinking: None` (every utility caller) keeps the wire byte-identical
    /// to before the cc 2.1.198 inheritance seam: no `thinking` field at all.
    #[tokio::test]
    async fn no_thinking_config_keeps_legacy_wire() {
        let body = serde_json::json!({
            "id": "msg_legacy", "model": "claude-haiku-4-5",
            "content": [{ "type": "text", "text": "ok" }],
            "stop_reason": "end_turn", "usage": { "input_tokens": 1, "output_tokens": 1 }
        }).to_string();
        let transport = Arc::new(StubTransport::new(body));
        let client = ProviderSideQueryClient::new("sk-test", None, transport.clone());
        client.query(req(None)).await.expect("query ok");
        let received = transport.received.lock().unwrap();
        let body: serde_json::Value =
            serde_json::from_str(received[0].body.as_deref().unwrap()).unwrap();
        assert!(
            body.get("thinking").is_none(),
            "legacy callers must not grow a thinking field: {body}"
        );
    }

    #[tokio::test]
    async fn api_error_maps_to_side_query_error_api() {
        // A 400 body that does not parse as a MessageResponse surfaces as
        // SideQueryError::Api (the codec maps the malformed body to LlmError;
        // the StubTransport returns 200 with a non-MessageResponse body to
        // exercise the decode-failure -> LlmError -> Api(..) path).
        let transport = Arc::new(StubTransport::new("not json"));
        let client = ProviderSideQueryClient::new("sk-test", None, transport);

        let err = client.query(req(None)).await.expect_err("should fail");
        assert!(matches!(err, SideQueryError::Api(_)), "got {err:?}");
    }
}
