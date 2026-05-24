//! `WebSearchTool` — routes the agent's query through Anthropic's Messages
//! API with the `web-search-2025-03-05` anthropic-beta header and a
//! `web_search_20250305` tool block. Spec §7 web wire identifiers.
//!
//! Wire-locked constants (asserted byte-for-byte by `parity_web_tools.json`):
//! - `WEB_SEARCH_TOOL_BLOCK_TYPE = "web_search_20250305"` (tool block `type`)
//! - `WEB_SEARCH_TOOL_BLOCK_NAME = "web_search"` (tool block `name`)
//! - `WEB_SEARCH_MAX_USES = 8` (upstream `WebSearchTool.ts:80`)
//! - `WEB_SEARCH_DEFAULT_MAX_TOKENS = 4096`
//! - `anthropic-beta: web-search-2025-03-05` (via `lingxi_api_client::betas::WEB_SEARCH`)

use crate::builtin::web_fetch::WEBFETCH_USER_AGENT_PREFIX;
use crate::builtin::BuiltinToolContext;
use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use async_trait::async_trait;
use lingxi_api_client::betas::WEB_SEARCH as WEB_SEARCH_BETA;
use lingxi_api_client::types::{ContentBlockApi, MessageResponse};
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::{PiiTagged, Verified};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{WEB_SEARCH_COMPLETED, WEB_SEARCH_FAILED, WEB_SEARCH_STARTED};
use lingxi_traits::http::HttpError;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::time::Instant;

/// Wire `type` field on the WebSearch tool block. Spec §7 lock; matches
/// `claude-code/src/tools/WebSearchTool/WebSearchTool.ts:78`.
pub const WEB_SEARCH_TOOL_BLOCK_TYPE: &str = "web_search_20250305";

/// Wire `name` field on the WebSearch tool block. Matches upstream.
pub const WEB_SEARCH_TOOL_BLOCK_NAME: &str = "web_search";

/// `max_uses` cap on a single search request. claude-code lock.
pub const WEB_SEARCH_MAX_USES: u32 = 8;

/// `max_tokens` used when WebSearchTool calls `POST /v1/messages`.
pub const WEB_SEARCH_DEFAULT_MAX_TOKENS: u32 = 4096;

/// Canonical tool name in the registry.
pub const TOOL_NAME: &str = "WebSearch";

/// Input schema for `WebSearchTool`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchInput {
    /// Search query (>= 2 characters).
    pub query: String,
    /// If present, restrict results to these domains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
    /// If present, exclude these domains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_domains: Option<Vec<String>>,
}

/// Build the JSON tool block that goes into `MessageRequest.tools`.
///
/// Wire-locked: `{ "type": "web_search_20250305", "name": "web_search",
/// "max_uses": 8, "allowed_domains"?: [...], "blocked_domains"?: [...] }`.
#[must_use]
pub fn build_tool_block(input: &WebSearchInput) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!(WEB_SEARCH_TOOL_BLOCK_TYPE));
    m.insert("name".into(), json!(WEB_SEARCH_TOOL_BLOCK_NAME));
    m.insert("max_uses".into(), json!(WEB_SEARCH_MAX_USES));
    if let Some(a) = &input.allowed_domains {
        m.insert("allowed_domains".into(), json!(a));
    }
    if let Some(b) = &input.blocked_domains {
        m.insert("blocked_domains".into(), json!(b));
    }
    Value::Object(m)
}

/// Build the full `POST /v1/messages` body for WebSearch.
#[must_use]
pub fn build_request_body(model: &str, input: &WebSearchInput) -> Value {
    json!({
        "model": model,
        "max_tokens": WEB_SEARCH_DEFAULT_MAX_TOKENS,
        "messages": [
            { "role": "user", "content": input.query }
        ],
        "tools": [ build_tool_block(input) ],
    })
}

/// One parsed search-output entry: either a free-form text block or a raw
/// `server_tool_use` input payload.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SearchResultEntry {
    /// Free-form text from a `text` content block.
    Text(String),
    /// Raw input payload from a `server_tool_use` block.
    Hit(Value),
}

/// Walk a `MessageResponse.content` array and return the search output:
/// each `text` block as a [`SearchResultEntry::Text`], each
/// `server_tool_use` with `name == "web_search"` as a
/// [`SearchResultEntry::Hit`] carrying the block's raw `input` payload.
/// Mirrors upstream `makeOutputFromSearchResponse` (`WebSearchTool.ts`).
#[must_use]
pub fn parse_response_content(content: &[ContentBlockApi]) -> Vec<SearchResultEntry> {
    let mut out: Vec<SearchResultEntry> = Vec::new();
    for block in content {
        match block {
            ContentBlockApi::Text { text } => {
                out.push(SearchResultEntry::Text(text.clone()));
            }
            ContentBlockApi::ServerToolUse { name, input, .. }
                if name == WEB_SEARCH_TOOL_BLOCK_NAME =>
            {
                out.push(SearchResultEntry::Hit(input.clone()));
            }
            _ => {}
        }
    }
    out
}

/// `WebSearchTool` — routes the agent's query through Anthropic's Messages
/// API with `anthropic-beta: web-search-2025-03-05` and the
/// `web_search_20250305` tool block. Never self-retries.
pub struct WebSearchTool {
    ctx: BuiltinToolContext,
}

impl WebSearchTool {
    /// Construct a new tool.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    fn user_agent() -> String {
        format!(
            "{}{}",
            WEBFETCH_USER_AGENT_PREFIX,
            env!("CARGO_PKG_VERSION")
        )
    }

    async fn emit_started(
        &self,
        invocation_id: &str,
        query: &str,
        allowed_count: usize,
        blocked_count: usize,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "_PROTO_query".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(query.to_string()).into_inner(),
            ),
        );
        md.insert(
            "allowed_domains_count".into(),
            AnalyticsValue::Int(allowed_count as i64),
        );
        md.insert(
            "blocked_domains_count".into(),
            AnalyticsValue::Int(blocked_count as i64),
        );
        self.ctx.bus.log_event(WEB_SEARCH_STARTED, md).await;
    }

    async fn emit_completed(
        &self,
        invocation_id: &str,
        hits: u64,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: u64,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert("hits".into(), AnalyticsValue::Int(hits as i64));
        md.insert(
            "input_tokens".into(),
            AnalyticsValue::Int(input_tokens as i64),
        );
        md.insert(
            "output_tokens".into(),
            AnalyticsValue::Int(output_tokens as i64),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(WEB_SEARCH_COMPLETED, md).await;
    }

    async fn emit_failed(
        &self,
        invocation_id: &str,
        error_kind: &str,
        status: Option<u16>,
        duration_ms: u64,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "error_kind".into(),
            AnalyticsValue::String(Verified::assert_safe(error_kind.to_string()).into_inner()),
        );
        if let Some(s) = status {
            md.insert("status".into(), AnalyticsValue::Int(i64::from(s)));
        }
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(WEB_SEARCH_FAILED, md).await;
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["query"],
        "properties": {
            "query": { "type": "string", "minLength": 2 },
            "allowed_domains": { "type": "array", "items": { "type": "string" } },
            "blocked_domains": { "type": "array", "items": { "type": "string" } }
        }
    })
});

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }
    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }
    fn is_open_world(&self, _input: &Value) -> bool {
        true
    }
    fn interrupt_behavior(&self, _input: &Value) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "allow-all-gate (M4-03 default)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), crate::tool_trait::ValidationError> {
        let q = input.get("query").and_then(Value::as_str).ok_or_else(|| {
            crate::tool_trait::ValidationError("missing required field: query".into())
        })?;
        if q.chars().count() < 2 {
            return Err(crate::tool_trait::ValidationError(
                "query must be at least 2 characters".into(),
            ));
        }
        Ok(())
    }

    async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
        "Searches the web via Anthropic's hosted web_search tool.".into()
    }
    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "WebSearch executes a single search query and returns a mix of text \
         commentary and structured hits. Query must be at least 2 characters."
            .into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let parsed_input: WebSearchInput = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(format!("invalid input: {e}")))?;
        if parsed_input.query.chars().count() < 2 {
            return Err(ToolError::InvalidInput(
                "query must be at least 2 characters".into(),
            ));
        }

        let invocation_id = crate::builtin::file_read::ulid_or_uuid();
        let allowed_count = parsed_input.allowed_domains.as_ref().map_or(0, Vec::len);
        let blocked_count = parsed_input.blocked_domains.as_ref().map_or(0, Vec::len);
        self.emit_started(
            &invocation_id,
            &parsed_input.query,
            allowed_count,
            blocked_count,
        )
        .await;

        let started = Instant::now();
        let body = build_request_body(&self.ctx.default_model, &parsed_input);
        let mut req = self.ctx.provider.build_request(&body);
        req.headers
            .push(("anthropic-beta".into(), WEB_SEARCH_BETA.to_string()));
        req.headers.push(("user-agent".into(), Self::user_agent()));

        let resp_result = self.ctx.http.request(req).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;

        match resp_result {
            Ok(http_resp) if http_resp.status == 200 => {
                let parsed: MessageResponse = match serde_json::from_str(&http_resp.body) {
                    Ok(p) => p,
                    Err(e) => {
                        self.emit_failed(&invocation_id, "invalid_response", None, elapsed_ms)
                            .await;
                        return Err(ToolError::Transport(format!(
                            "WebSearch: invalid response: {e}"
                        )));
                    }
                };
                let results = parse_response_content(&parsed.content);
                let hits = results.len() as u64;
                let input_tokens = parsed.usage.input_tokens;
                let output_tokens = parsed.usage.output_tokens;
                self.emit_completed(
                    &invocation_id,
                    hits,
                    input_tokens,
                    output_tokens,
                    elapsed_ms,
                )
                .await;
                Ok(ToolCallResult {
                    data: json!({
                        "query": parsed_input.query,
                        "results": results,
                        "duration_ms": elapsed_ms,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Ok(http_resp) => {
                self.emit_failed(
                    &invocation_id,
                    "http_status",
                    Some(http_resp.status),
                    elapsed_ms,
                )
                .await;
                Err(ToolError::Transport(format!(
                    "WebSearch: HTTP {} from messages_create",
                    http_resp.status
                )))
            }
            Err(HttpError::Status { status, .. }) => {
                self.emit_failed(&invocation_id, "http_status", Some(status), elapsed_ms)
                    .await;
                Err(ToolError::Transport(format!(
                    "WebSearch: HTTP {status} from messages_create"
                )))
            }
            Err(HttpError::Timeout(_)) => {
                self.emit_failed(&invocation_id, "timeout", None, elapsed_ms)
                    .await;
                Err(ToolError::Transport("WebSearch: request timed out".into()))
            }
            Err(other) => {
                self.emit_failed(&invocation_id, "transport", None, elapsed_ms)
                    .await;
                Err(ToolError::Transport(format!(
                    "WebSearch: transport failure: {other}"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_constants_match_spec() {
        assert_eq!(WEB_SEARCH_TOOL_BLOCK_TYPE, "web_search_20250305");
        assert_eq!(WEB_SEARCH_TOOL_BLOCK_NAME, "web_search");
        assert_eq!(WEB_SEARCH_MAX_USES, 8);
        assert_eq!(WEB_SEARCH_DEFAULT_MAX_TOKENS, 4096);
        assert_eq!(TOOL_NAME, "WebSearch");
    }

    #[test]
    fn anthropic_beta_lock() {
        assert_eq!(WEB_SEARCH_BETA, "web-search-2025-03-05");
    }

    #[test]
    fn builds_correct_tool_block() {
        let input = WebSearchInput {
            query: "rust async traits".into(),
            allowed_domains: None,
            blocked_domains: None,
        };
        let block = build_tool_block(&input);
        assert_eq!(block["type"], "web_search_20250305");
        assert_eq!(block["name"], "web_search");
        assert_eq!(block["max_uses"], 8);
        assert!(block.get("allowed_domains").is_none());
        assert!(block.get("blocked_domains").is_none());
    }

    #[test]
    fn tool_block_includes_allowed_domains_when_set() {
        let input = WebSearchInput {
            query: "x".into(),
            allowed_domains: Some(vec!["docs.rs".into(), "crates.io".into()]),
            blocked_domains: None,
        };
        let block = build_tool_block(&input);
        let allowed = block["allowed_domains"].as_array().expect("array");
        assert_eq!(allowed.len(), 2);
        assert_eq!(allowed[0], "docs.rs");
        assert_eq!(allowed[1], "crates.io");
    }

    #[test]
    fn tool_block_includes_blocked_domains_when_set() {
        let input = WebSearchInput {
            query: "x".into(),
            allowed_domains: None,
            blocked_domains: Some(vec!["spam.example".into()]),
        };
        let block = build_tool_block(&input);
        assert_eq!(block["blocked_domains"][0], "spam.example");
    }

    #[test]
    fn body_has_model_and_message() {
        let input = WebSearchInput {
            query: "rust async traits".into(),
            allowed_domains: None,
            blocked_domains: None,
        };
        let body = build_request_body("claude-sonnet-4-20250514", &input);
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "rust async traits");
        assert_eq!(body["tools"][0]["type"], "web_search_20250305");
    }

    #[test]
    fn parses_text_only_response() {
        let blocks = vec![
            ContentBlockApi::Text {
                text: "Here are some results:".into(),
            },
            ContentBlockApi::Text {
                text: "1. ...".into(),
            },
        ];
        let parsed = parse_response_content(&blocks);
        assert_eq!(parsed.len(), 2);
        match &parsed[0] {
            SearchResultEntry::Text(s) => assert_eq!(s, "Here are some results:"),
            SearchResultEntry::Hit(_) => panic!("expected Text"),
        }
    }

    #[test]
    fn parses_mixed_text_and_server_tool_use() {
        let blocks = vec![
            ContentBlockApi::Text {
                text: "Found:".into(),
            },
            ContentBlockApi::ServerToolUse {
                id: "stu_1".into(),
                name: "web_search".into(),
                input: json!({ "url": "https://docs.rs", "title": "Docs.rs" }),
            },
        ];
        let parsed = parse_response_content(&blocks);
        assert_eq!(parsed.len(), 2);
        match &parsed[1] {
            SearchResultEntry::Hit(v) => assert_eq!(v["url"], "https://docs.rs"),
            SearchResultEntry::Text(_) => panic!("expected Hit"),
        }
    }

    #[test]
    fn ignores_non_web_search_server_tool_use() {
        let blocks = vec![ContentBlockApi::ServerToolUse {
            id: "stu_2".into(),
            name: "advisor".into(),
            input: json!({}),
        }];
        let parsed = parse_response_content(&blocks);
        assert!(parsed.is_empty(), "advisor tool use must be skipped");
    }

    #[test]
    fn ignores_thinking_blocks() {
        let blocks = vec![ContentBlockApi::Thinking {
            thinking: "let me think".into(),
            signature: None,
        }];
        let parsed = parse_response_content(&blocks);
        assert!(parsed.is_empty());
    }

    // ---- async impl Tool tests using MockHttpTransport ---------------------

    use crate::builtin::test_support::{fresh_ctx, fresh_tx};
    use lingxi_api_client::AnthropicProvider;
    use lingxi_telemetry::sinks::InMemorySink;
    use lingxi_telemetry::AnalyticsBus;
    use lingxi_test_harness::mocks::{MockHttpTransport, ScriptedResponse};
    use lingxi_traits::http::HttpTransport;
    use std::sync::Arc;

    fn make_web_ctx() -> (
        BuiltinToolContext,
        Arc<MockHttpTransport>,
        Arc<InMemorySink>,
    ) {
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        let http = Arc::new(MockHttpTransport::new());
        let mut ctx = crate::builtin::test_support::ctx_for_file_tools(
            crate::builtin::test_support::make_dummy_fs(),
            bus,
            vec![std::path::PathBuf::from("/tmp")],
        );
        ctx.http = http.clone() as Arc<dyn HttpTransport>;
        ctx.provider = Arc::new(AnthropicProvider::new("test-key", None));
        ctx.default_model = "claude-sonnet-4-20250514".into();
        (ctx, http, sink)
    }

    fn ok_response(status: u16, body: &str) -> ScriptedResponse {
        ScriptedResponse::Sync(lingxi_protocol::HttpResponse {
            status,
            headers: vec![],
            body: body.to_string(),
        })
    }

    #[tokio::test]
    async fn happy_path_returns_results_and_emits_completed() {
        let (ctx, http, sink) = make_web_ctx();
        ctx.bus.attach_sink(sink.clone()).await;
        let resp_body = json!({
            "id": "msg_1",
            "model": "claude-sonnet-4-20250514",
            "content": [
                { "type": "text", "text": "Here are results:" },
                {
                    "type": "server_tool_use",
                    "id": "stu_1",
                    "name": "web_search",
                    "input": { "url": "https://docs.rs", "title": "docs.rs" }
                }
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 42,
                "output_tokens": 17,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            }
        });
        http.enqueue(ok_response(200, &resp_body.to_string()));
        let tool = WebSearchTool::new(ctx);
        let res = tool
            .call(json!({ "query": "rust async" }), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let arr = res.data["results"].as_array().expect("results array");
        assert_eq!(arr.len(), 2);
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_web_search_completed"));
    }

    #[tokio::test]
    async fn attaches_anthropic_beta_header() {
        let (ctx, http, _sink) = make_web_ctx();
        let resp_body = json!({
            "id": "msg_2",
            "model": "x",
            "content": [],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 0, "output_tokens": 0 }
        });
        http.enqueue(ok_response(200, &resp_body.to_string()));
        let tool = WebSearchTool::new(ctx);
        let _ = tool
            .call(json!({ "query": "foo" }), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let reqs = http.received_requests();
        let last_req = reqs.last().expect("captured");
        let (_, beta) = last_req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("anthropic-beta"))
            .expect("anthropic-beta header present");
        assert!(
            beta.contains("web-search-2025-03-05"),
            "anthropic-beta `{beta}` must contain web-search-2025-03-05"
        );
    }

    #[tokio::test]
    async fn body_contains_locked_tool_block() {
        let (ctx, http, _sink) = make_web_ctx();
        let resp_body = json!({
            "id": "x",
            "model": "x",
            "content": [],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 0, "output_tokens": 0 }
        });
        http.enqueue(ok_response(200, &resp_body.to_string()));
        let tool = WebSearchTool::new(ctx);
        let _ = tool
            .call(
                json!({ "query": "rust async traits" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        let reqs = http.received_requests();
        let last_req = reqs.last().expect("captured");
        let body: Value =
            serde_json::from_str(last_req.body.as_ref().expect("has body")).expect("json");
        let tool_block = &body["tools"][0];
        assert_eq!(tool_block["type"], "web_search_20250305");
        assert_eq!(tool_block["name"], "web_search");
        assert_eq!(tool_block["max_uses"], 8);
    }

    #[tokio::test]
    async fn validation_rejects_short_query() {
        let (ctx, _http, _sink) = make_web_ctx();
        let tool = WebSearchTool::new(ctx);
        let err = tool
            .validate_input(&json!({ "query": "x" }), &fresh_ctx())
            .await
            .expect_err("too short");
        assert!(err.to_string().contains("at least 2 characters"));
    }

    #[tokio::test]
    async fn surfaces_http_500_as_transport() {
        let (ctx, http, sink) = make_web_ctx();
        ctx.bus.attach_sink(sink.clone()).await;
        http.enqueue(ok_response(500, "boom"));
        let tool = WebSearchTool::new(ctx);
        let err = tool
            .call(json!({ "query": "rust async" }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("500 must be Err");
        assert!(matches!(err, ToolError::Transport(_)));
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_web_search_failed"));
    }

    #[tokio::test]
    async fn http_500_does_not_retry_internally() {
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(ok_response(500, "boom"));
        let tool = WebSearchTool::new(ctx);
        let _ = tool
            .call(json!({ "query": "rust" }), fresh_ctx(), fresh_tx())
            .await;
        assert_eq!(
            http.received_requests().len(),
            1,
            "WebSearch must not self-retry"
        );
    }
}
