//! Concrete [`SideQueryClient`] backed by `api_client::AnthropicProvider`.
//!
//! A side query is a stateless one-shot LLM call (see [`crate::side_query`]).
//! [`ProviderSideQueryClient`] wires the [`SideQueryRequest`] DTO to the
//! Anthropic Messages endpoint and decodes the [`MessageResponse`] back into a
//! [`SideQueryResponse`]. It owns an [`AnthropicProvider`] for request building
//! / retry / 401-refresh middleware, plus an object-safe
//! [`Arc<dyn HttpTransport>`] handle so the whole client stays usable behind
//! `Arc<dyn SideQueryClient>` (`MemorySelector::new` takes exactly that).
//!
//! ## Field-forwarding gap (documented)
//!
//! [`AnthropicProvider::messages_create_non_stream`] only forwards `model`,
//! `system`, and `messages` (it hard-codes `max_tokens: 4096` and ignores
//! `tools`, `tool_choice`, `output_format`, `temperature`, `stop_sequences`,
//! `max_retries`, and `thinking_budget`). This client therefore wires only
//! those three fields through to the wire request. That is sufficient for the
//! §6.3 memory selector, which relies on JSON-shaped *text* output (decoded
//! here into [`SideQueryResponse::structured`]) rather than a server-side
//! `response_format`. A follow-up that adds a richer provider entrypoint can
//! forward the remaining fields; until then they are accepted on the request
//! and intentionally dropped.

use crate::side_query::{SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse};
use api_client::types::ContentBlockApi;
use api_client::{AnthropicProvider, MessageResponse};
use async_trait::async_trait;
use protocol::{HttpRequest, HttpResponse};
use std::sync::Arc;
use traits::http::{RawByteStream, SseStream};
use traits::{HttpError, HttpTransport};

/// Sized newtype adapter around an `Arc<dyn HttpTransport>`.
///
/// [`AnthropicProvider::messages_create_non_stream`] is generic over
/// `T: HttpTransport` and carries an implicit `Sized` bound, so an unsized
/// `&dyn HttpTransport` cannot be passed directly. Wrapping the trait object
/// in this sized newtype (which itself implements `HttpTransport` by
/// delegating to the inner `Arc`) lets the client store an object-safe handle
/// yet still satisfy the generic, sized bound — without touching `api-client`.
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
/// Construct with [`Self::new`] for the common case, or
/// [`Self::from_provider`] when the caller has already attached a cost tracker
/// / analytics bus to the [`AnthropicProvider`] via its builder methods.
pub struct ProviderSideQueryClient {
    /// Provider that owns request building plus the retry / 429 / 401-refresh
    /// middleware. Generic-over-transport calls are made through `transport`.
    provider: AnthropicProvider,
    /// Object-safe transport handle. Stored as `Arc<dyn HttpTransport>` (not a
    /// generic `T`) so the struct is usable behind `Arc<dyn SideQueryClient>`.
    transport: Arc<dyn HttpTransport>,
}

impl ProviderSideQueryClient {
    /// Build a client from raw credentials.
    ///
    /// `None` for `base_url` uses `api_client::anthropic::DEFAULT_BASE_URL`.
    #[must_use]
    pub fn new(
        api_key: impl Into<String>,
        base_url: Option<String>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        Self {
            provider: AnthropicProvider::new(api_key, base_url),
            transport,
        }
    }

    /// Build a client from a pre-configured provider.
    ///
    /// Use this when the caller wants the provider's builder add-ons
    /// (`.with_cost_tracker(..)`, `.with_bus(..)`, `.with_oauth_hook(..)`) so
    /// COGS attribution and `tengu_*` telemetry fire on the side query.
    #[must_use]
    pub fn from_provider(provider: AnthropicProvider, transport: Arc<dyn HttpTransport>) -> Self {
        Self {
            provider,
            transport,
        }
    }
}

/// Decode a [`MessageResponse`] into the side-query response shape.
///
/// * `Text` blocks are concatenated into the flattened `text`.
/// * `ToolUse` blocks become `{"id", "name", "input"}` JSON in `tool_calls`.
/// * Other block kinds (thinking, server tool use, connector text, advisor
///   tool result) are ignored for side queries.
/// * `structured` is populated only when `want_structured` is set (the caller
///   requested an `output_format`): the accumulated text is best-effort parsed
///   as JSON. A non-JSON body leaves `structured` as `None` rather than
///   erroring — `MemorySelector` tolerates `None` (empty selection), so the
///   best-effort path is the safer default.
/// * `usage` maps `UsageApi` -> `cost::Usage` with the exact cross-naming the
///   provider's own cost path uses: API `cache_creation` -> cost `cache_write`,
///   API `cache_read` -> cost `cache_read`.
fn decode_response(resp: MessageResponse, want_structured: bool) -> SideQueryResponse {
    let mut text_acc = String::new();
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();

    for block in resp.content {
        match block {
            ContentBlockApi::Text { text } => text_acc.push_str(&text),
            ContentBlockApi::ToolUse { id, name, input } => {
                tool_calls.push(serde_json::json!({
                    "id": id,
                    "name": name,
                    "input": input,
                }));
            }
            // Side queries ignore thinking / server-tool / connector / advisor
            // blocks.
            ContentBlockApi::Thinking { .. }
            | ContentBlockApi::ServerToolUse { .. }
            | ContentBlockApi::ConnectorText { .. }
            | ContentBlockApi::AdvisorToolResult { .. } => {}
        }
    }

    let text = (!text_acc.is_empty()).then(|| text_acc.clone());
    let structured = if want_structured {
        serde_json::from_str::<serde_json::Value>(&text_acc).ok()
    } else {
        None
    };

    let usage = cost::Usage {
        tokens: cost::TokenUsage {
            input: resp.usage.input_tokens,
            output: resp.usage.output_tokens,
            cache_read: resp.usage.cache_read_input_tokens,
            cache_write: resp.usage.cache_creation_input_tokens,
            reasoning_output: 0,
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
        // `messages_create_non_stream<T: HttpTransport>` carries an implicit
        // `Sized` bound, so we pass a sized `&ArcTransport` (which delegates to
        // the stored `Arc<dyn HttpTransport>`) rather than an unsized
        // `&dyn HttpTransport`. The `?` auto-converts `ApiError` ->
        // `SideQueryError::Api` via the `#[from]` on the enum.
        let transport = ArcTransport(Arc::clone(&self.transport));
        let resp = self
            .provider
            .messages_create_non_stream(
                &request.model,
                request.system_prompt.as_deref(),
                request.messages,
                &transport,
            )
            .await?;

        Ok(decode_response(resp, request.output_format.is_some()))
    }
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
            Err(HttpError::InvalidRequest("sse not used in side query".into()))
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
            thinking_budget: None,
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
        let body: serde_json::Value =
            serde_json::from_str(raw_body).expect("request body is JSON");

        // The three forwarded fields reach the wire body.
        assert_eq!(body["model"].as_str(), Some("claude-haiku-4-5"));
        assert_eq!(body["system"].as_str(), Some("system"));
        assert_eq!(
            body["messages"].as_array().map(Vec::len),
            Some(1),
            "the single request message is forwarded"
        );

        // The documented forwarding gap holds: `max_tokens` is hard-coded to
        // 4096 by the provider (NOT the request's `max_tokens: 1024`), and the
        // dropped DTO fields never appear on the wire.
        assert_eq!(body["max_tokens"].as_u64(), Some(4096));
        assert!(body.get("tools").is_none(), "tools dropped");
        assert!(body.get("tool_choice").is_none(), "tool_choice dropped");
        assert!(body.get("temperature").is_none(), "temperature dropped");
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
    async fn api_error_maps_to_side_query_error_api() {
        // A 400 body that does not parse as a MessageResponse surfaces as
        // SideQueryError::Api (the provider maps the malformed 2xx-path body to
        // ApiError; the StubTransport returns 200 with a non-MessageResponse
        // body to exercise the decode-failure -> ApiError -> Api(..) path).
        let transport = Arc::new(StubTransport::new("not json"));
        let client = ProviderSideQueryClient::new("sk-test", None, transport);

        let err = client.query(req(None)).await.expect_err("should fail");
        assert!(matches!(err, SideQueryError::Api(_)), "got {err:?}");
    }
}
