//! `WebSearchTool` — routes the agent's query through Anthropic's Messages
//! API with the `web-search-2025-03-05` anthropic-beta header and a
//! `web_search_20250305` tool block. Spec §7 web wire identifiers.
//!
//! Wire-locked constants (asserted byte-for-byte by `parity_web_tools.json`):
//! - `WEB_SEARCH_TOOL_BLOCK_TYPE = "web_search_20250305"` (tool block `type`)
//! - `WEB_SEARCH_TOOL_BLOCK_NAME = "web_search"` (tool block `name`)
//! - `WEB_SEARCH_MAX_USES = 8` (upstream `WebSearchTool.ts:80`)
//! - `WEB_SEARCH_DEFAULT_MAX_TOKENS = 4096`
//! - `anthropic-beta: web-search-2025-03-05` (via `WEB_SEARCH_BETA` local const)

use crate::web_fetch::WEBFETCH_USER_AGENT_PREFIX;
use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::time::Instant;
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{WEB_SEARCH_COMPLETED, WEB_SEARCH_FAILED, WEB_SEARCH_STARTED};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use tool_api::BuiltinToolContext;
use traits::http::HttpError;

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

/// `anthropic-beta` value that gates the web-search tool on the Anthropic API.
///
/// Wire-locked byte-for-byte against `claude-code/src/constants/betas.ts`
/// (`WEB_SEARCH = "web-search-2025-03-05"`). Local copy so tools/web does not
/// depend on api-client.
const WEB_SEARCH_BETA: &str = "web-search-2025-03-05";

/// Minimal usage counters decoded from a WebSearch `POST /v1/messages` response.
///
/// Only `input_tokens` and `output_tokens` are needed; other fields on the
/// Anthropic `usage` object are ignored. Matches the two fields `WebSearchTool`
/// read from the two usage fields in the response — ported here so tools/web does not
/// depend on api-client.
///
/// The usage field in `WebSearchMessageResponse` is itself `#[serde(default)]`,
/// which requires `Default`. `input_tokens` and `output_tokens` also carry
/// `#[serde(default)]` so that a partial or missing `usage` object decodes to
/// zeros rather than failing — consistent with the parent field's defaulting
/// contract (unlike the standalone `UsageApi` type in the old api-client, which
/// required both fields).
#[derive(Debug, Default, Clone, serde::Deserialize)]
struct WebSearchUsage {
    /// Number of input tokens billed.
    #[serde(default)]
    input_tokens: u64,
    /// Number of output tokens billed.
    #[serde(default)]
    output_tokens: u64,
}

/// API provider family — the subset of claude-code's `getAPIProvider()` values
/// that `WebSearchTool.isEnabled` branches on (`WebSearchTool.ts:168-193`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiProvider {
    /// Anthropic first-party API (`api.anthropic.com`).
    FirstParty,
    /// Google Vertex AI.
    Vertex,
    /// Azure AI Foundry.
    Foundry,
    /// Anything else (Bedrock, custom gateways, OpenAI-compatible, …).
    Other,
}

/// Infer the [`ApiProvider`] from a request `base_url`. WebSearch routes through
/// `BuiltinToolContext.provider` (an `AnthropicRequestBuilder`) whose `base_url`
/// is the only provider signal reachable from `tools/web` — the typed
/// `getAPIProvider()` value lives in host config, which is out of this crate's
/// scope (a `BuiltinToolContext::api_provider` field would be the faithful source
/// but touches the frozen `tool-api`). Mapping: `api.anthropic.com` ⇒ first-party;
/// a `*.aiplatform.googleapis.com` / `…-aiplatform.…` host ⇒ Vertex; an Azure /
/// Foundry host (`.azure.com`, `cognitiveservices`, `models.ai.azure.com`) ⇒
/// Foundry; everything else ⇒ Other. LingXi's default base_url is
/// `https://api.anthropic.com`, so the default is `FirstParty`.
#[must_use]
pub fn infer_api_provider(base_url: &str) -> ApiProvider {
    let host = url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    if host == "api.anthropic.com" {
        ApiProvider::FirstParty
    } else if host.contains("aiplatform.googleapis.com") || host.contains("-aiplatform.") {
        ApiProvider::Vertex
    } else if host.ends_with(".azure.com")
        || host.contains("cognitiveservices")
        || host.contains("ai.azure.com")
    {
        ApiProvider::Foundry
    } else {
        ApiProvider::Other
    }
}

/// Whether WebSearch is enabled for `provider` + `model` — 1:1 with claude-code
/// `WebSearchTool.isEnabled` (`WebSearchTool.ts:168-193`):
/// - `firstParty` ⇒ always enabled;
/// - `vertex` ⇒ enabled only for Claude 4.x (`claude-opus-4` / `claude-sonnet-4`
///   / `claude-haiku-4` substring);
/// - `foundry` ⇒ always enabled (Foundry only ships web-search-capable models);
/// - anything else ⇒ disabled.
#[must_use]
pub fn web_search_is_enabled(provider: ApiProvider, model: &str) -> bool {
    match provider {
        // firstParty: any model. foundry: only ships web-search-capable models,
        // so it is likewise unconditionally enabled (TS treats them as separate
        // `if` branches both returning `true` — merged here, same behavior).
        ApiProvider::FirstParty | ApiProvider::Foundry => true,
        ApiProvider::Vertex => {
            model.contains("claude-opus-4")
                || model.contains("claude-sonnet-4")
                || model.contains("claude-haiku-4")
        }
        ApiProvider::Other => false,
    }
}

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
///
/// This is the **non-streaming** body used by the blocking fallback path
/// ([`WebSearchTool::call`] when `stream_sse` is unavailable). The primary path
/// uses [`build_streaming_request_body`], which is identical except for the
/// added `"stream": true`.
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

/// Build the full `POST /v1/messages` body for WebSearch with **streaming**
/// enabled (`"stream": true`).
///
/// This mirrors claude-code's `queryModelWithStreaming` path: the same request
/// as [`build_request_body`] but asking the server to deliver the response as a
/// Server-Sent-Events message stream so incremental progress (the
/// `server_tool_use` / `web_search_tool_result` blocks) can be observed as the
/// hosted search runs, rather than as one blocking POST.
#[must_use]
pub fn build_streaming_request_body(model: &str, input: &WebSearchInput) -> Value {
    json!({
        "model": model,
        "max_tokens": WEB_SEARCH_DEFAULT_MAX_TOKENS,
        "messages": [
            { "role": "user", "content": input.query }
        ],
        "tools": [ build_tool_block(input) ],
        "stream": true,
    })
}

/// One parsed search-output entry: either a free-form text segment or a
/// structured search-result hit object.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SearchResultEntry {
    /// Free-form text accumulated from one or more consecutive `text` blocks
    /// (also used for the `Web search error: <code>` string emitted on a
    /// `web_search_tool_result` error payload).
    Text(String),
    /// Structured search result from a `web_search_tool_result` block, shaped
    /// `{ "tool_use_id": <id>, "content": [{ "title", "url" }, ...] }`.
    Hit(Value),
}

/// Minimal view of the `messages` response consumed by WebSearch.
///
/// `content` is intentionally kept as raw `serde_json::Value` blocks rather than
/// a typed content-block enum: the web-search response carries
/// `web_search_tool_result` blocks that `ContentBlockApi` does not model, and
/// adding that variant would break the exhaustive `ContentBlockApi` matches in
/// the agent/orchestrator/sidequery crates. This mirrors how upstream consumes
/// the response as an opaque `BetaContentBlock[]` (`WebSearchTool.ts:86`).
#[derive(Debug, Deserialize)]
struct WebSearchMessageResponse {
    #[serde(default)]
    content: Vec<Value>,
    #[serde(default)]
    usage: WebSearchUsage,
}

/// Walk a response `content` array and produce the search output, mirroring
/// upstream `makeOutputFromSearchResponse` (`WebSearchTool.ts:86-150`)
/// byte-for-byte.
///
/// The block sequence is, repeated per search:
/// `server_tool_use` → `web_search_tool_result` → intermingled `text`/citation
/// blocks. The faithful state machine:
/// - `text` blocks accumulate into a running buffer while `in_text` is set;
/// - a `server_tool_use` block flushes the trimmed buffer (when non-empty) as a
///   [`SearchResultEntry::Text`], clears it, and drops `in_text` — the
///   `server_tool_use` block itself carries only the QUERY and is **never**
///   emitted as a result;
/// - a `web_search_tool_result` block whose `content` is an array emits a
///   [`SearchResultEntry::Hit`] of `{ tool_use_id, content: [{title, url}] }`;
///   when `content` is an error object instead, it emits the
///   `Web search error: <error_code>` string as a [`SearchResultEntry::Text`];
/// - a `text` block seen after a flush starts a fresh buffer.
///
/// Any trailing buffered text is flushed at the end.
#[must_use]
pub fn parse_response_content(content: &[Value]) -> Vec<SearchResultEntry> {
    let mut out: Vec<SearchResultEntry> = Vec::new();
    let mut text_acc = String::new();
    let mut in_text = true;

    for block in content {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "server_tool_use" => {
                if in_text {
                    in_text = false;
                    let trimmed = text_acc.trim();
                    if !trimmed.is_empty() {
                        out.push(SearchResultEntry::Text(trimmed.to_string()));
                    }
                    text_acc.clear();
                }
            }
            "web_search_tool_result" => match block.get("content") {
                // Success case — `content` is an array of search hits.
                Some(Value::Array(items)) => {
                    let hits: Vec<Value> = items
                        .iter()
                        .map(|r| {
                            json!({
                                "title": r.get("title").cloned().unwrap_or(Value::Null),
                                "url": r.get("url").cloned().unwrap_or(Value::Null),
                            })
                        })
                        .collect();
                    out.push(SearchResultEntry::Hit(json!({
                        "tool_use_id": block.get("tool_use_id").cloned().unwrap_or(Value::Null),
                        "content": hits,
                    })));
                }
                // Error case — `content` is a `WebSearchToolResultError`.
                other => {
                    let code = other
                        .and_then(|c| c.get("error_code"))
                        .map_or_else(
                            || "undefined".to_string(),
                            |v| match v {
                                Value::String(s) => s.clone(),
                                _ => v.to_string(),
                            },
                        );
                    out.push(SearchResultEntry::Text(format!("Web search error: {code}")));
                }
            },
            "text" => {
                let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                if in_text {
                    text_acc.push_str(text);
                } else {
                    in_text = true;
                    text_acc = text.to_string();
                }
            }
            _ => {}
        }
    }

    // Flush any trailing buffered text (`if (textAcc.length)` in upstream).
    if !text_acc.is_empty() {
        out.push(SearchResultEntry::Text(text_acc.trim().to_string()));
    }
    out
}

/// Streaming-SSE block reassembler — rebuilds the raw `Vec<serde_json::Value>`
/// content-block array from an Anthropic message-stream event sequence so it can
/// be fed to the EXISTING [`parse_response_content`] verbatim.
///
/// This is the streaming analogue of decoding a blocking `messages` response's
/// `content[]`. claude-code's `queryModelWithStreaming` accumulates
/// `BetaContentBlock[]` from the same events (`WebSearchTool.ts:293-388`); here
/// we reassemble the blocks into the identical raw-JSON shape rather than a
/// typed enum, because WebSearch's `web_search_tool_result` / `server_tool_use`
/// blocks are not modelled by the engine's typed content-block enum (the
/// orchestrator's typed accumulator collapses them to `Skipped`).
///
/// Handled events (Anthropic documented message-stream shape):
/// - `content_block_start { index, content_block }` → store the block's initial
///   JSON at `index` (e.g. `{type:"server_tool_use",id,name,input:{}}`,
///   `{type:"web_search_tool_result",tool_use_id,content:[...]}`,
///   `{type:"text",text:""}`).
/// - `content_block_delta { index, delta }`:
///   - `text_delta` → append `delta.text` to `block[index].text`;
///   - `input_json_delta` → append `delta.partial_json` to a per-index buffer;
///   - `citations_delta` → push `delta.citation` into `block[index].citations`.
/// - `content_block_stop { index }` → if an `input_json` buffer accumulated for
///   `index`, parse it and set `block[index].input`.
/// - `message_start { message: { usage } }` / `message_delta { usage }` →
///   capture `input_tokens` / `output_tokens` for telemetry parity with the
///   blocking path's `usage` object.
/// - `message_stop` → terminal (the stream also ends naturally).
#[derive(Debug, Default)]
struct StreamReassembler {
    /// Reassembled blocks keyed by their stream `index`. A `BTreeMap` so the
    /// final `into_blocks()` yields blocks in ascending index order — the same
    /// order they appear in a blocking response's `content[]`.
    blocks: std::collections::BTreeMap<u64, Value>,
    /// Per-index `input_json_delta` accumulators (raw `partial_json` concat),
    /// finalized into `block[index].input` on `content_block_stop`.
    json_bufs: std::collections::BTreeMap<u64, String>,
    /// Usage counters captured from `message_start` / `message_delta`, mirroring
    /// the two fields the blocking path read off the response `usage` object.
    usage: WebSearchUsage,
}

impl StreamReassembler {
    /// Apply one decoded message-stream event (the parsed `SseEvent.data` JSON).
    ///
    /// Returns `true` when this event is a `content_block_start` for a
    /// `server_tool_use` block — the caller uses that signal to emit a
    /// "searching" progress event (mirrors claude-code's per-search progress on
    /// the server-tool-use start, `WebSearchTool.ts:306-318`).
    fn apply(&mut self, event: &Value) -> bool {
        let ev_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        match ev_type {
            "message_start" => {
                if let Some(u) = event.get("message").and_then(|m| m.get("usage")) {
                    self.capture_usage(u);
                }
                false
            }
            "message_delta" => {
                // `message_delta` carries a top-level `usage` with the running
                // `output_tokens` (and sometimes refined `input_tokens`).
                if let Some(u) = event.get("usage") {
                    self.capture_usage(u);
                }
                false
            }
            "content_block_start" => {
                let Some(index) = event.get("index").and_then(Value::as_u64) else {
                    return false;
                };
                let block = event.get("content_block").cloned().unwrap_or(json!({}));
                let is_server_tool_use =
                    block.get("type").and_then(Value::as_str) == Some("server_tool_use");
                self.blocks.insert(index, block);
                // A fresh block starts an empty JSON buffer slot; `input_json_delta`
                // chunks append to it and `content_block_stop` finalizes it.
                self.json_bufs.insert(index, String::new());
                is_server_tool_use
            }
            "content_block_delta" => {
                let Some(index) = event.get("index").and_then(Value::as_u64) else {
                    return false;
                };
                if let Some(delta) = event.get("delta") {
                    self.apply_delta(index, delta);
                }
                false
            }
            "content_block_stop" => {
                if let Some(index) = event.get("index").and_then(Value::as_u64) {
                    self.finalize_block(index);
                }
                false
            }
            // `message_stop`, `ping`, and any other events carry no block data.
            _ => false,
        }
    }

    /// Apply a single `content_block_delta`'s `delta` payload to block `index`.
    fn apply_delta(&mut self, index: u64, delta: &Value) {
        match delta.get("type").and_then(Value::as_str).unwrap_or("") {
            "text_delta" => {
                let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                let block = self.blocks.entry(index).or_insert_with(|| json!({}));
                append_str_field(block, "text", text);
            }
            "input_json_delta" => {
                let partial = delta
                    .get("partial_json")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                self.json_bufs
                    .entry(index)
                    .or_default()
                    .push_str(partial);
            }
            "citations_delta" => {
                if let Some(citation) = delta.get("citation") {
                    let block = self.blocks.entry(index).or_insert_with(|| json!({}));
                    push_array_field(block, "citations", citation.clone());
                }
            }
            // `signature_delta`, `thinking_delta`, etc. are irrelevant to the
            // WebSearch block types `parse_response_content` consumes.
            _ => {}
        }
    }

    /// Finalize block `index` on `content_block_stop`: if an `input_json` buffer
    /// accumulated, parse it and set `block[index].input` (the reassembled
    /// `server_tool_use` query lives here). A non-empty buffer that fails to
    /// parse is left unset — `parse_response_content` never reads `input`, so
    /// this only affects faithfulness of the reassembled block, not the result.
    fn finalize_block(&mut self, index: u64) {
        let buf = self.json_bufs.remove(&index).unwrap_or_default();
        if buf.is_empty() {
            return;
        }
        if let Ok(parsed) = serde_json::from_str::<Value>(&buf) {
            if let Some(block) = self.blocks.get_mut(&index) {
                if let Some(obj) = block.as_object_mut() {
                    obj.insert("input".to_string(), parsed);
                }
            }
        }
    }

    /// Capture `input_tokens` / `output_tokens` from a usage object, taking the
    /// max seen so far (later `message_delta` usages refine `output_tokens`).
    fn capture_usage(&mut self, usage: &Value) {
        if let Some(v) = usage.get("input_tokens").and_then(Value::as_u64) {
            self.usage.input_tokens = self.usage.input_tokens.max(v);
        }
        if let Some(v) = usage.get("output_tokens").and_then(Value::as_u64) {
            self.usage.output_tokens = self.usage.output_tokens.max(v);
        }
    }

    /// Consume the reassembler into the ordered raw content-block array (ascending
    /// `index`) ready for [`parse_response_content`].
    fn into_blocks(self) -> Vec<Value> {
        self.blocks.into_values().collect()
    }
}

/// Append `text` to `block[field]` (a string), creating the field if absent.
fn append_str_field(block: &mut Value, field: &str, text: &str) {
    if let Some(obj) = block.as_object_mut() {
        let existing = obj.get(field).and_then(Value::as_str).unwrap_or("");
        let combined = format!("{existing}{text}");
        obj.insert(field.to_string(), Value::String(combined));
    }
}

/// Push `item` onto `block[field]` (a JSON array), creating the array if absent.
fn push_array_field(block: &mut Value, field: &str, item: Value) {
    if let Some(obj) = block.as_object_mut() {
        match obj.get_mut(field).and_then(Value::as_array_mut) {
            Some(arr) => arr.push(item),
            None => {
                obj.insert(field.to_string(), Value::Array(vec![item]));
            }
        }
    }
}

/// Build the model-facing text block for a successful search, mirroring
/// upstream `mapToolResultToToolResultBlockParam` (`WebSearchTool.ts:401`)
/// byte-for-byte: a `Web search results for query: "<q>"` header, one
/// rendered segment per result entry, and a trailing `REMINDER:` footer,
/// with the whole string `.trim()`-ed.
///
/// Per-entry rendering follows the TS branches:
/// - a text segment is appended verbatim plus a blank line;
/// - a hit object renders `Links: <json>` when it carries a non-empty
///   `content` array, otherwise `No links found.`. After WEB.4,
///   [`SearchResultEntry::Hit`] is `{tool_use_id, content:[{title,url}]}`, so
///   `content` is the hits array serialized verbatim into the `Links:` line.
#[must_use]
pub fn build_model_content(query: &str, results: &[SearchResultEntry]) -> String {
    let mut out = format!("Web search results for query: \"{query}\"\n\n");
    for entry in results {
        match entry {
            SearchResultEntry::Text(s) => {
                out.push_str(s);
                out.push_str("\n\n");
            }
            SearchResultEntry::Hit(v) => {
                match v.get("content").and_then(Value::as_array) {
                    Some(arr) if !arr.is_empty() => {
                        let rendered = serde_json::to_string(arr).unwrap_or_default();
                        out.push_str(&format!("Links: {rendered}\n\n"));
                    }
                    _ => out.push_str("No links found.\n\n"),
                }
            }
        }
    }
    out.push_str(
        "\nREMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks.",
    );
    out.trim().to_string()
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
        // Provider gating — 1:1 with `WebSearchTool.isEnabled`
        // (`WebSearchTool.ts:168-193`). The provider is inferred from the request
        // builder's `base_url` (see [`infer_api_provider`]) since the typed
        // `getAPIProvider()` value is not reachable from `tools/web`; the model is
        // the session's default model. LingXi's default (`api.anthropic.com`) maps
        // to first-party, so WebSearch stays enabled by default.
        web_search_is_enabled(
            infer_api_provider(&self.ctx.provider.base_url),
            &self.ctx.default_model,
        )
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
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
    ) -> Result<(), tool_api::tool_trait::ValidationError> {
        let q = input.get("query").and_then(Value::as_str).ok_or_else(|| {
            tool_api::tool_trait::ValidationError("missing required field: query".into())
        })?;
        if q.chars().count() < 2 {
            return Err(tool_api::tool_trait::ValidationError(
                "query must be at least 2 characters".into(),
            ));
        }
        // Mirror upstream `validateInput` (`WebSearchTool.ts:244`, errorCode 2):
        // reject when BOTH allowed_domains and blocked_domains are non-empty.
        let allowed_non_empty = input
            .get("allowed_domains")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());
        let blocked_non_empty = input
            .get("blocked_domains")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());
        if allowed_non_empty && blocked_non_empty {
            return Err(tool_api::tool_trait::ValidationError(
                "Error: Cannot specify both allowed_domains and blocked_domains in the same request"
                    .into(),
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
        ctx: ToolUseContext,
        tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let parsed_input: WebSearchInput = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(format!("invalid input: {e}")))?;
        if parsed_input.query.chars().count() < 2 {
            return Err(ToolError::InvalidInput(
                "query must be at least 2 characters".into(),
            ));
        }

        let invocation_id = tool_api::util::ids::ulid_or_uuid();
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

        // PRIMARY PATH — stream the same `/v1/messages` request with
        // `"stream": true` and reassemble the content blocks incrementally, so
        // search progress is observable as the hosted search runs (1:1 with
        // claude-code's `queryModelWithStreaming`, `WebSearchTool.ts:268-388`).
        // The streaming body's tool block / beta header / model are identical to
        // the blocking body.
        let stream_body = build_streaming_request_body(&self.ctx.default_model, &parsed_input);
        let stream_req = self.build_messages_request(&stream_body);

        match self.ctx.http.stream_sse(stream_req).await {
            Ok(stream) => {
                // Connected — drive the SSE stream to completion, reassembling
                // the raw content-block array and emitting progress as blocks
                // start. A mid-stream transport error aborts the search.
                let (blocks, usage) = match Self::consume_stream(
                    stream,
                    &parsed_input.query,
                    &ctx,
                    &tx,
                )
                .await
                {
                    Ok(out) => out,
                    Err(err) => {
                        let elapsed_ms = started.elapsed().as_millis() as u64;
                        return Err(self
                            .map_stream_error(&invocation_id, err, elapsed_ms)
                            .await);
                    }
                };
                let elapsed_ms = started.elapsed().as_millis() as u64;
                let results = parse_response_content(&blocks);
                Ok(self
                    .build_success_result(
                        &invocation_id,
                        &parsed_input.query,
                        results,
                        usage.input_tokens,
                        usage.output_tokens,
                        elapsed_ms,
                    )
                    .await)
            }
            // FALLBACK PATH — `stream_sse` failed at connect (e.g. a transport or
            // test mock that does not implement SSE). Fall back to the original
            // blocking POST + `parse_response_content`, keeping non-SSE
            // transports and the existing blocking tests working. The functional
            // result is identical; only the incremental progress is lost.
            Err(_connect_err) => {
                let body = build_request_body(&self.ctx.default_model, &parsed_input);
                let req = self.build_messages_request(&body);
                let resp_result = self.ctx.http.request(req).await;
                let elapsed_ms = started.elapsed().as_millis() as u64;
                self.finish_blocking(&invocation_id, &parsed_input.query, resp_result, elapsed_ms)
                    .await
            }
        }
    }
}

impl WebSearchTool {
    /// Assemble a `POST /v1/messages` request for `body` with the WebSearch
    /// `anthropic-beta` + `user-agent` headers attached. Shared by the streaming
    /// and blocking-fallback paths so both send byte-identical headers.
    fn build_messages_request(&self, body: &Value) -> protocol::HttpRequest {
        let mut req = self.ctx.provider.build_request(body);
        req.headers
            .push(("anthropic-beta".into(), WEB_SEARCH_BETA.to_string()));
        req.headers.push(("user-agent".into(), Self::user_agent()));
        req
    }

    /// Drive an SSE message stream to completion, reassembling the raw
    /// content-block array via [`StreamReassembler`] and emitting an incremental
    /// "searching" progress event each time a `server_tool_use` block starts.
    ///
    /// Returns the ordered `Vec<Value>` content blocks (feed straight to
    /// [`parse_response_content`]) plus the captured usage counters. A mid-stream
    /// transport error is surfaced as `Err(HttpError)`.
    async fn consume_stream(
        mut stream: traits::http::SseStream,
        query: &str,
        ctx: &ToolUseContext,
        tx: &ToolProgressSender,
    ) -> Result<(Vec<Value>, WebSearchUsage), HttpError> {
        use futures_util::StreamExt;

        let mut acc = StreamReassembler::default();
        let mut progress_counter: u64 = 0;
        while let Some(item) = stream.next().await {
            let ev = item?;
            // Each SSE frame's `data` is one Anthropic message-stream event JSON.
            // A frame whose data is not parseable JSON (e.g. a `[DONE]` sentinel
            // or a comment) carries no block data and is skipped.
            let Ok(event) = serde_json::from_str::<Value>(&ev.data) else {
                continue;
            };
            let server_tool_use_started = acc.apply(&event);
            if server_tool_use_started {
                progress_counter += 1;
                Self::emit_progress(ctx, tx, query, progress_counter);
            }
            if event.get("type").and_then(Value::as_str) == Some("message_stop") {
                break;
            }
        }
        let usage = acc.usage.clone();
        Ok((acc.into_blocks(), usage))
    }

    /// Emit one incremental "searching" progress event, mirroring claude-code's
    /// per-search `onProgress({ type: 'query_update', query })`
    /// (`WebSearchTool.ts:344-354`). The payload carries the synthetic
    /// `search-progress-N` id and the query in `data` (TS keeps it on the event's
    /// `toolUseID`/`data`); here the channel key is the model's tool-use id when
    /// present, falling back to a fresh id (the channel key must be a real
    /// [`protocol::ToolUseId`], unlike TS's free-form string). Best-effort
    /// (`try_send`), matching TS's synchronous fire-and-forget `onProgress`.
    fn emit_progress(ctx: &ToolUseContext, tx: &ToolProgressSender, query: &str, counter: u64) {
        // `Default for ToolUseId` generates a fresh random id (same as `new()`),
        // so the channel key is the model's tool-use id when present, else fresh.
        let tool_use_id = ctx.tool_use_id.clone().unwrap_or_default();
        let _ = tx.try_send(tool_api::progress::ToolProgress {
            tool_use_id,
            data: json!({
                "type": "query_update",
                "toolUseID": format!("search-progress-{counter}"),
                "query": query,
            }),
        });
    }

    /// Build the success [`ToolCallResult`] (shared by the streaming and blocking
    /// paths) and fire `WEB_SEARCH_COMPLETED`. Output bytes are identical to the
    /// original blocking path: a `model_content` header + per-entry segments +
    /// cite-sources footer, plus the structured `results` array for the TUI.
    async fn build_success_result(
        &self,
        invocation_id: &str,
        query: &str,
        results: Vec<SearchResultEntry>,
        input_tokens: u64,
        output_tokens: u64,
        elapsed_ms: u64,
    ) -> ToolCallResult {
        let hits = results.len() as u64;
        self.emit_completed(invocation_id, hits, input_tokens, output_tokens, elapsed_ms)
            .await;
        let model_content = build_model_content(query, &results);
        ToolCallResult {
            data: json!({
                "query": query,
                "results": results,
                "duration_ms": elapsed_ms,
                "model_content": model_content,
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        }
    }

    /// Map a mid-stream / connect-phase [`HttpError`] to a [`ToolError`] and emit
    /// the matching `WEB_SEARCH_FAILED` telemetry. Mirrors the blocking path's
    /// error arms so failure classification is identical across both paths.
    async fn map_stream_error(
        &self,
        invocation_id: &str,
        err: HttpError,
        elapsed_ms: u64,
    ) -> ToolError {
        match err {
            HttpError::Status { status, .. } => {
                self.emit_failed(invocation_id, "http_status", Some(status), elapsed_ms)
                    .await;
                ToolError::Transport(format!("WebSearch: HTTP {status} from messages_create"))
            }
            HttpError::Timeout(_) => {
                self.emit_failed(invocation_id, "timeout", None, elapsed_ms)
                    .await;
                ToolError::Transport("WebSearch: request timed out".into())
            }
            other => {
                self.emit_failed(invocation_id, "transport", None, elapsed_ms)
                    .await;
                ToolError::Transport(format!("WebSearch: transport failure: {other}"))
            }
        }
    }

    /// Finish the blocking fallback path: parse the buffered response via
    /// [`parse_response_content`] and build the result, or map the error. This is
    /// the original blocking `call` body, factored out so the fallback reuses it
    /// verbatim.
    async fn finish_blocking(
        &self,
        invocation_id: &str,
        query: &str,
        resp_result: Result<protocol::HttpResponse, HttpError>,
        elapsed_ms: u64,
    ) -> Result<ToolCallResult, ToolError> {
        match resp_result {
            Ok(http_resp) if http_resp.status == 200 => {
                let parsed: WebSearchMessageResponse = match serde_json::from_str(&http_resp.body) {
                    Ok(p) => p,
                    Err(e) => {
                        self.emit_failed(invocation_id, "invalid_response", None, elapsed_ms)
                            .await;
                        return Err(ToolError::Transport(format!(
                            "WebSearch: invalid response: {e}"
                        )));
                    }
                };
                let results = parse_response_content(&parsed.content);
                Ok(self
                    .build_success_result(
                        invocation_id,
                        query,
                        results,
                        parsed.usage.input_tokens,
                        parsed.usage.output_tokens,
                        elapsed_ms,
                    )
                    .await)
            }
            Ok(http_resp) => {
                self.emit_failed(invocation_id, "http_status", Some(http_resp.status), elapsed_ms)
                    .await;
                Err(ToolError::Transport(format!(
                    "WebSearch: HTTP {} from messages_create",
                    http_resp.status
                )))
            }
            Err(err) => Err(self.map_stream_error(invocation_id, err, elapsed_ms).await),
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

    // ---- isEnabled provider gating (WebSearchTool.ts:168-193) ---------------

    #[test]
    fn is_enabled_first_party_any_model() {
        assert!(web_search_is_enabled(ApiProvider::FirstParty, "claude-sonnet-4-20250514"));
        assert!(web_search_is_enabled(ApiProvider::FirstParty, "claude-3-5-haiku"));
        assert!(web_search_is_enabled(ApiProvider::FirstParty, "literally-anything"));
    }

    #[test]
    fn is_enabled_vertex_only_claude_4x() {
        assert!(web_search_is_enabled(ApiProvider::Vertex, "claude-opus-4-20250514"));
        assert!(web_search_is_enabled(ApiProvider::Vertex, "claude-sonnet-4-5"));
        assert!(web_search_is_enabled(ApiProvider::Vertex, "claude-haiku-4-5"));
        // Pre-4.x and non-Claude models on Vertex are disabled.
        assert!(!web_search_is_enabled(ApiProvider::Vertex, "claude-3-5-sonnet"));
        assert!(!web_search_is_enabled(ApiProvider::Vertex, "gemini-2.5-pro"));
    }

    #[test]
    fn is_enabled_foundry_any_model() {
        assert!(web_search_is_enabled(ApiProvider::Foundry, "anything"));
    }

    #[test]
    fn is_enabled_other_provider_disabled() {
        assert!(!web_search_is_enabled(ApiProvider::Other, "claude-opus-4-20250514"));
        assert!(!web_search_is_enabled(ApiProvider::Other, "anything"));
    }

    #[test]
    fn infer_api_provider_from_base_url() {
        assert_eq!(
            infer_api_provider("https://api.anthropic.com"),
            ApiProvider::FirstParty
        );
        assert_eq!(
            infer_api_provider("https://us-central1-aiplatform.googleapis.com"),
            ApiProvider::Vertex
        );
        assert_eq!(
            infer_api_provider("https://aiplatform.googleapis.com/v1"),
            ApiProvider::Vertex
        );
        assert_eq!(
            infer_api_provider("https://my-resource.openai.azure.com"),
            ApiProvider::Foundry
        );
        assert_eq!(
            infer_api_provider("https://bedrock-runtime.us-east-1.amazonaws.com"),
            ApiProvider::Other
        );
        // Unparseable base_url => Other (disabled).
        assert_eq!(infer_api_provider("not a url"), ApiProvider::Other);
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
    fn consecutive_text_blocks_concatenate_into_one_entry() {
        // Mirrors upstream: while `in_text`, consecutive `text` blocks append to
        // the same buffer and flush as a SINGLE entry at the end (not one per
        // block).
        let blocks = vec![
            json!({ "type": "text", "text": "Here are some results:" }),
            json!({ "type": "text", "text": "1. ..." }),
        ];
        let parsed = parse_response_content(&blocks);
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            SearchResultEntry::Text(s) => assert_eq!(s, "Here are some results:1. ..."),
            SearchResultEntry::Hit(_) => panic!("expected Text"),
        }
    }

    #[test]
    fn web_search_tool_result_success_produces_hit() {
        // The QUERY-carrying `server_tool_use` block is NOT a result; the actual
        // results live in the `web_search_tool_result` block's `content` array.
        let blocks = vec![
            json!({
                "type": "server_tool_use",
                "id": "stu_1",
                "name": "web_search",
                "input": { "query": "rust async" }
            }),
            json!({
                "type": "web_search_tool_result",
                "tool_use_id": "stu_1",
                "content": [
                    { "title": "Docs.rs", "url": "https://docs.rs", "encrypted_content": "zzz" },
                    { "title": "crates.io", "url": "https://crates.io" }
                ]
            }),
        ];
        let parsed = parse_response_content(&blocks);
        assert_eq!(parsed.len(), 1, "server_tool_use must not produce a result");
        match &parsed[0] {
            SearchResultEntry::Hit(v) => {
                assert_eq!(v["tool_use_id"], "stu_1");
                let hits = v["content"].as_array().expect("content array");
                assert_eq!(hits.len(), 2);
                // Only `title` and `url` are projected (extra fields dropped).
                assert_eq!(hits[0], json!({ "title": "Docs.rs", "url": "https://docs.rs" }));
                assert_eq!(hits[1], json!({ "title": "crates.io", "url": "https://crates.io" }));
            }
            SearchResultEntry::Text(_) => panic!("expected Hit"),
        }
    }

    #[test]
    fn web_search_tool_result_error_produces_error_string() {
        // When `content` is an error object (not an array), upstream pushes the
        // `Web search error: <error_code>` string.
        let blocks = vec![json!({
            "type": "web_search_tool_result",
            "tool_use_id": "stu_9",
            "content": { "type": "web_search_tool_result_error", "error_code": "max_uses_exceeded" }
        })];
        let parsed = parse_response_content(&blocks);
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            SearchResultEntry::Text(s) => assert_eq!(s, "Web search error: max_uses_exceeded"),
            SearchResultEntry::Hit(_) => panic!("expected Text error string"),
        }
    }

    #[test]
    fn server_tool_use_flushes_accumulated_text() {
        // Leading text accumulates, then a `server_tool_use` flushes it (trimmed)
        // as one entry; the `server_tool_use` itself is never emitted.
        let blocks = vec![
            json!({ "type": "text", "text": "Found: " }),
            json!({ "type": "text", "text": "  things  " }),
            json!({
                "type": "server_tool_use",
                "id": "stu_1",
                "name": "web_search",
                "input": { "query": "q" }
            }),
        ];
        let parsed = parse_response_content(&blocks);
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            SearchResultEntry::Text(s) => assert_eq!(s, "Found:   things"),
            SearchResultEntry::Hit(_) => panic!("expected Text"),
        }
    }

    #[test]
    fn canonical_sequence_text_tooluse_result_text() {
        // The documented per-search block order, with a trailing commentary text
        // block after the result.
        let blocks = vec![
            json!({ "type": "text", "text": "Let me search." }),
            json!({
                "type": "server_tool_use",
                "id": "stu_1",
                "name": "web_search",
                "input": { "query": "q" }
            }),
            json!({
                "type": "web_search_tool_result",
                "tool_use_id": "stu_1",
                "content": [ { "title": "T", "url": "https://t.example" } ]
            }),
            json!({ "type": "text", "text": "Here is what I found." }),
        ];
        let parsed = parse_response_content(&blocks);
        assert_eq!(parsed.len(), 3);
        assert!(matches!(&parsed[0], SearchResultEntry::Text(s) if s == "Let me search."));
        assert!(matches!(&parsed[1], SearchResultEntry::Hit(_)));
        assert!(matches!(&parsed[2], SearchResultEntry::Text(s) if s == "Here is what I found."));
    }

    #[test]
    fn server_tool_use_alone_yields_no_results() {
        // A lone `server_tool_use` (query carrier) with no result block produces
        // nothing — it only flushes the (empty) text buffer.
        let blocks = vec![json!({
            "type": "server_tool_use",
            "id": "stu_2",
            "name": "advisor",
            "input": {}
        })];
        let parsed = parse_response_content(&blocks);
        assert!(parsed.is_empty(), "server_tool_use must not become a result");
    }

    #[test]
    fn model_content_has_header_and_reminder_footer() {
        let results = vec![
            SearchResultEntry::Text("First summary.".into()),
            SearchResultEntry::Hit(json!({
                "content": [ { "title": "Docs.rs", "url": "https://docs.rs" } ]
            })),
            SearchResultEntry::Hit(json!({ "query": "rust async" })),
        ];
        let mc = build_model_content("rust async", &results);
        // Exact TS header bytes.
        assert!(
            mc.starts_with("Web search results for query: \"rust async\"\n\n"),
            "model_content must start with the TS header, got: {mc}"
        );
        // Text entry rendered verbatim.
        assert!(mc.contains("First summary."));
        // Hit with a non-empty `content` array renders a `Links:` JSON line.
        assert!(
            mc.contains("Links: [{\"title\":\"Docs.rs\",\"url\":\"https://docs.rs\"}]"),
            "hit with content must render Links: <json>, got: {mc}"
        );
        // Hit without a `content` array renders the `No links found.` fallback.
        assert!(mc.contains("No links found."));
        // Exact TS footer bytes, and trailing `.trim()` means it ends there.
        assert!(
            mc.ends_with(
                "REMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks."
            ),
            "model_content must end with the REMINDER footer, got: {mc}"
        );
    }

    #[test]
    fn model_content_trims_and_keeps_footer_when_no_results() {
        let mc = build_model_content("q", &[]);
        assert!(mc.starts_with("Web search results for query: \"q\""));
        assert!(mc.ends_with(
            "REMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks."
        ));
        // `.trim()` removes any trailing whitespace; no trailing newline.
        assert_eq!(mc, mc.trim());
    }

    #[test]
    fn ignores_thinking_blocks() {
        let blocks = vec![json!({ "type": "thinking", "thinking": "let me think" })];
        let parsed = parse_response_content(&blocks);
        assert!(parsed.is_empty());
    }

    // ---- async impl Tool tests ---------------------------------------------
    //
    // WebSearch now streams the `/v1/messages` request (`stream_sse`) and
    // reassembles the content blocks incrementally; the blocking `request` path
    // survives only as the connect-failure fallback. The HTTP-exercising tests
    // below therefore drive the STREAMING path via a scripted SSE event
    // sequence, except `fallback_*` which force a `stream_sse` connect error and
    // assert the blocking fallback.

    use std::sync::Arc;
    use telemetry::sinks::InMemorySink;
    use telemetry::AnalyticsBus;
    use tool_api::progress::{progress_channel, ToolProgress, ToolProgressReceiver};
    use tool_api::test_support::fresh_ctx;
    use traits::http::HttpTransport;

    /// A streaming-aware mock `HttpTransport` with INDEPENDENT queues for
    /// `stream_sse` (SSE event frames) and `request` (blocking responses), so a
    /// single test can script the streaming path AND a distinct blocking
    /// fallback. `fail_stream` forces `stream_sse` to error at connect (without
    /// touching the blocking queue), exercising the fallback path. Every
    /// received request (stream or blocking) is recorded for header/body asserts.
    #[derive(Default)]
    struct StreamingMockHttp {
        sse_events: std::sync::Mutex<Option<Vec<protocol::SseEvent>>>,
        blocking: std::sync::Mutex<std::collections::VecDeque<Result<protocol::HttpResponse, HttpError>>>,
        fail_stream: std::sync::atomic::AtomicBool,
        received: std::sync::Mutex<Vec<protocol::HttpRequest>>,
    }

    impl StreamingMockHttp {
        fn new() -> Self {
            Self::default()
        }
        /// Script the SSE frames `stream_sse` will yield (one per poll).
        fn set_stream(&self, events: Vec<protocol::SseEvent>) {
            *self.sse_events.lock().unwrap() = Some(events);
        }
        /// Force `stream_sse` to return a connect error (drives the fallback).
        fn fail_stream(&self) {
            self.fail_stream
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        /// Enqueue a blocking `request` response (used by the fallback path).
        fn enqueue_blocking(&self, resp: Result<protocol::HttpResponse, HttpError>) {
            self.blocking.lock().unwrap().push_back(resp);
        }
        fn received_requests(&self) -> Vec<protocol::HttpRequest> {
            self.received.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HttpTransport for StreamingMockHttp {
        async fn request(
            &self,
            req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, HttpError> {
            self.received.lock().unwrap().push(req);
            self.blocking.lock().unwrap().pop_front().unwrap_or_else(|| {
                Err(HttpError::InvalidResponse(
                    "StreamingMockHttp: no blocking response scripted".into(),
                ))
            })
        }
        async fn stream_sse(
            &self,
            req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, HttpError> {
            self.received.lock().unwrap().push(req);
            if self.fail_stream.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(HttpError::InvalidRequest(
                    "StreamingMockHttp: stream_sse forced failure".into(),
                ));
            }
            let events = self.sse_events.lock().unwrap().take().unwrap_or_default();
            Ok(Box::pin(VecSseStream {
                remaining: events.into(),
            }))
        }
    }

    /// Minimal `Stream` yielding the scripted SSE frames one per poll, then end.
    struct VecSseStream {
        remaining: std::collections::VecDeque<protocol::SseEvent>,
    }
    impl futures_util::Stream for VecSseStream {
        type Item = Result<protocol::SseEvent, HttpError>;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            std::task::Poll::Ready(self.remaining.pop_front().map(Ok))
        }
    }

    /// One SSE frame whose `data` is the JSON string of `event`.
    fn sse(event: Value) -> protocol::SseEvent {
        protocol::SseEvent {
            event_type: event
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string),
            data: event.to_string(),
            id: None,
        }
    }

    /// Build a ctx wired to `http`, returning the ctx + sink for event asserts.
    fn make_streaming_ctx(http: Arc<StreamingMockHttp>) -> (BuiltinToolContext, Arc<InMemorySink>) {
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        let mut ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![std::path::PathBuf::from("/tmp")],
        );
        ctx.http = http as Arc<dyn HttpTransport>;
        ctx.provider = Arc::new(tool_api::AnthropicRequestBuilder::new("test-key", None));
        ctx.default_model = "claude-sonnet-4-20250514".into();
        (ctx, sink)
    }

    /// A blocking-200 response wrapping `body`. Returns the `Result` shape that
    /// `enqueue_blocking` accepts (so a test may also enqueue an `Err`).
    #[allow(clippy::unnecessary_wraps)]
    fn ok_blocking(status: u16, body: &str) -> Result<protocol::HttpResponse, HttpError> {
        Ok(protocol::HttpResponse {
            status,
            headers: vec![],
            body: body.to_string(),
        })
    }

    /// Drain a progress receiver synchronously (the channel is unbuffered-ish
    /// 64-slot; `try_recv` until empty).
    fn drain_progress(rx: &mut ToolProgressReceiver) -> Vec<ToolProgress> {
        let mut out = Vec::new();
        while let Ok(p) = rx.try_recv() {
            out.push(p);
        }
        out
    }

    #[tokio::test]
    async fn streaming_path_reassembles_and_emits_completed() {
        // Scripted Anthropic message-stream for ONE search:
        //   message_start (usage) ; text block "Here are results:" via deltas ;
        //   server_tool_use block (query via input_json_delta) ;
        //   web_search_tool_result block with a 2-result `content` array ;
        //   trailing commentary text ; message_delta (output usage) ; message_stop.
        let http = Arc::new(StreamingMockHttp::new());
        http.set_stream(vec![
            sse(json!({ "type": "message_start", "message": { "usage": { "input_tokens": 42, "output_tokens": 0 } } })),
            // Block 0: leading text, delivered in two text_delta chunks.
            sse(json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } })),
            sse(json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Here are " } })),
            sse(json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "results:" } })),
            sse(json!({ "type": "content_block_stop", "index": 0 })),
            // Block 1: server_tool_use; the query arrives via input_json_delta.
            sse(json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "server_tool_use", "id": "stu_1", "name": "web_search", "input": {} } })),
            sse(json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "{\"query\":\"rust " } })),
            sse(json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "async\"}" } })),
            sse(json!({ "type": "content_block_stop", "index": 1 })),
            // Block 2: web_search_tool_result carrying a 2-hit content array.
            sse(json!({ "type": "content_block_start", "index": 2, "content_block": {
                "type": "web_search_tool_result", "tool_use_id": "stu_1",
                "content": [
                    { "title": "docs.rs", "url": "https://docs.rs", "encrypted_content": "zzz" },
                    { "title": "crates.io", "url": "https://crates.io" }
                ]
            } })),
            sse(json!({ "type": "content_block_stop", "index": 2 })),
            // Block 3: trailing commentary text + a citation delta.
            sse(json!({ "type": "content_block_start", "index": 3, "content_block": { "type": "text", "text": "" } })),
            sse(json!({ "type": "content_block_delta", "index": 3, "delta": { "type": "text_delta", "text": "Done." } })),
            sse(json!({ "type": "content_block_delta", "index": 3, "delta": { "type": "citations_delta", "citation": { "url": "https://docs.rs" } } })),
            sse(json!({ "type": "content_block_stop", "index": 3 })),
            sse(json!({ "type": "message_delta", "usage": { "output_tokens": 17 } })),
            sse(json!({ "type": "message_stop" })),
        ]);
        let (ctx, sink) = make_streaming_ctx(http);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = WebSearchTool::new(ctx);
        let (tx, _rx) = progress_channel();
        let res = tool
            .call(json!({ "query": "rust async" }), fresh_ctx(), tx)
            .await
            .expect("ok");

        // Reassembled blocks → parse_response_content yields: leading text entry,
        // one structured hit (server_tool_use dropped), trailing text entry.
        let arr = res.data["results"].as_array().expect("results array");
        assert_eq!(arr.len(), 3, "got: {arr:?}");
        assert_eq!(arr[0], "Here are results:");
        assert_eq!(arr[1]["tool_use_id"], "stu_1");
        let hits = arr[1]["content"].as_array().expect("hits");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0], json!({ "title": "docs.rs", "url": "https://docs.rs" }));
        assert_eq!(hits[1], json!({ "title": "crates.io", "url": "https://crates.io" }));
        assert_eq!(arr[2], "Done.");

        // Final formatted output matches the blocking path for the equivalent
        // full response (same header / Links: / footer bytes).
        let mc = res.data["model_content"].as_str().expect("model_content");
        let equivalent_blocks = vec![
            json!({ "type": "text", "text": "Here are results:" }),
            json!({ "type": "server_tool_use", "id": "stu_1", "name": "web_search", "input": { "query": "rust async" } }),
            json!({ "type": "web_search_tool_result", "tool_use_id": "stu_1", "content": [
                { "title": "docs.rs", "url": "https://docs.rs" },
                { "title": "crates.io", "url": "https://crates.io" }
            ] }),
            json!({ "type": "text", "text": "Done." }),
        ];
        let expected_mc = build_model_content(
            "rust async",
            &parse_response_content(&equivalent_blocks),
        );
        assert_eq!(mc, expected_mc, "streamed output must equal blocking output");

        // COMPLETED telemetry fired, with usage captured from the stream.
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_web_search_completed"));
    }

    #[tokio::test]
    async fn streaming_path_emits_progress_on_server_tool_use_start() {
        // A server_tool_use block START must yield at least one progress event,
        // mirroring TS's per-search `onProgress`.
        let http = Arc::new(StreamingMockHttp::new());
        http.set_stream(vec![
            sse(json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1, "output_tokens": 0 } } })),
            sse(json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "server_tool_use", "id": "stu_1", "name": "web_search", "input": {} } })),
            sse(json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"query\":\"q\"}" } })),
            sse(json!({ "type": "content_block_stop", "index": 0 })),
            sse(json!({ "type": "content_block_start", "index": 1, "content_block": {
                "type": "web_search_tool_result", "tool_use_id": "stu_1", "content": []
            } })),
            sse(json!({ "type": "content_block_stop", "index": 1 })),
            sse(json!({ "type": "message_stop" })),
        ]);
        let (ctx, _sink) = make_streaming_ctx(http);
        let tool = WebSearchTool::new(ctx);
        let (tx, mut rx) = progress_channel();
        let _res = tool
            .call(json!({ "query": "rust async" }), fresh_ctx(), tx)
            .await
            .expect("ok");
        let progress = drain_progress(&mut rx);
        assert!(
            !progress.is_empty(),
            "at least one progress event must be emitted on server_tool_use start"
        );
        // Payload mirrors TS's progress shape.
        let p0 = &progress[0];
        assert_eq!(p0.data["type"], "query_update");
        assert_eq!(p0.data["query"], "rust async");
        assert!(p0.data["toolUseID"]
            .as_str()
            .is_some_and(|s| s.starts_with("search-progress-")));
    }

    #[tokio::test]
    async fn fallback_to_blocking_when_stream_sse_errs() {
        // `stream_sse` errors at connect → the tool falls back to the blocking
        // `request` path and still returns results.
        let http = Arc::new(StreamingMockHttp::new());
        http.fail_stream();
        let resp_body = json!({
            "id": "msg_fb",
            "model": "claude-sonnet-4-20250514",
            "content": [
                { "type": "text", "text": "Here are results:" },
                { "type": "server_tool_use", "id": "stu_1", "name": "web_search", "input": { "query": "rust async" } },
                { "type": "web_search_tool_result", "tool_use_id": "stu_1", "content": [ { "title": "docs.rs", "url": "https://docs.rs" } ] }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 42, "output_tokens": 17 }
        });
        http.enqueue_blocking(ok_blocking(200, &resp_body.to_string()));
        let (ctx, sink) = make_streaming_ctx(http.clone());
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = WebSearchTool::new(ctx);
        let (tx, _rx) = progress_channel();
        let res = tool
            .call(json!({ "query": "rust async" }), fresh_ctx(), tx)
            .await
            .expect("fallback must succeed");
        let arr = res.data["results"].as_array().expect("results array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0], "Here are results:");
        assert_eq!(arr[1]["content"][0]["url"], "https://docs.rs");
        // Exactly two requests recorded: the failed stream_sse + the fallback.
        assert_eq!(http.received_requests().len(), 2);
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_web_search_completed"));
    }

    #[tokio::test]
    async fn streaming_attaches_anthropic_beta_header_and_stream_flag() {
        let http = Arc::new(StreamingMockHttp::new());
        http.set_stream(vec![sse(json!({ "type": "message_stop" }))]);
        let (ctx, _sink) = make_streaming_ctx(http.clone());
        let tool = WebSearchTool::new(ctx);
        let (tx, _rx) = progress_channel();
        let _ = tool
            .call(json!({ "query": "foo" }), fresh_ctx(), tx)
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
        // Streaming body must carry `"stream": true` + the locked tool block.
        let body: Value =
            serde_json::from_str(last_req.body.as_ref().expect("has body")).expect("json");
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["type"], "web_search_20250305");
        assert_eq!(body["tools"][0]["name"], "web_search");
        assert_eq!(body["tools"][0]["max_uses"], 8);
    }

    #[tokio::test]
    async fn streaming_surfaces_http_500_via_fallback_as_transport() {
        // Connect-phase stream failure → fallback `request` returns 500 → mapped
        // to Transport + FAILED telemetry.
        let http = Arc::new(StreamingMockHttp::new());
        http.fail_stream();
        http.enqueue_blocking(ok_blocking(500, "boom"));
        let (ctx, sink) = make_streaming_ctx(http);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = WebSearchTool::new(ctx);
        let (tx, _rx) = progress_channel();
        let err = tool
            .call(json!({ "query": "rust async" }), fresh_ctx(), tx)
            .await
            .expect_err("500 must be Err");
        assert!(matches!(err, ToolError::Transport(_)));
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_web_search_failed"));
    }

    #[tokio::test]
    async fn mid_stream_transport_error_is_surfaced() {
        // An error item mid-stream aborts the search as Transport (no fallback —
        // the stream was already open). Build a stream that yields one good frame
        // then an Err.
        struct ErrAfterOne;
        #[async_trait]
        impl HttpTransport for ErrAfterOne {
            async fn request(
                &self,
                _req: protocol::HttpRequest,
            ) -> Result<protocol::HttpResponse, HttpError> {
                Err(HttpError::InvalidRequest("not used".into()))
            }
            async fn stream_sse(
                &self,
                _req: protocol::HttpRequest,
            ) -> Result<traits::http::SseStream, HttpError> {
                struct S(u8);
                impl futures_util::Stream for S {
                    type Item = Result<protocol::SseEvent, HttpError>;
                    fn poll_next(
                        mut self: std::pin::Pin<&mut Self>,
                        _cx: &mut std::task::Context<'_>,
                    ) -> std::task::Poll<Option<Self::Item>> {
                        self.0 += 1;
                        match self.0 {
                            1 => std::task::Poll::Ready(Some(Ok(protocol::SseEvent {
                                event_type: Some("message_start".into()),
                                data: json!({ "type": "message_start", "message": { "usage": {} } })
                                    .to_string(),
                                id: None,
                            }))),
                            2 => std::task::Poll::Ready(Some(Err(HttpError::Connection(
                                "mid-stream drop".into(),
                            )))),
                            _ => std::task::Poll::Ready(None),
                        }
                    }
                }
                Ok(Box::pin(S(0)))
            }
        }
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        let mut ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![std::path::PathBuf::from("/tmp")],
        );
        ctx.http = Arc::new(ErrAfterOne) as Arc<dyn HttpTransport>;
        ctx.provider = Arc::new(tool_api::AnthropicRequestBuilder::new("test-key", None));
        ctx.default_model = "claude-sonnet-4-20250514".into();
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = WebSearchTool::new(ctx);
        let (tx, _rx) = progress_channel();
        let err = tool
            .call(json!({ "query": "rust async" }), fresh_ctx(), tx)
            .await
            .expect_err("mid-stream error must surface");
        assert!(matches!(err, ToolError::Transport(_)));
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_web_search_failed"));
    }

    /// Minimal ctx for tests that exercise `validate_input` only (no HTTP).
    fn validation_ctx() -> BuiltinToolContext {
        let http = Arc::new(StreamingMockHttp::new());
        let (ctx, _sink) = make_streaming_ctx(http);
        ctx
    }

    #[tokio::test]
    async fn validation_rejects_short_query() {
        let ctx = validation_ctx();
        let tool = WebSearchTool::new(ctx);
        let err = tool
            .validate_input(&json!({ "query": "x" }), &fresh_ctx())
            .await
            .expect_err("too short");
        assert!(err.to_string().contains("at least 2 characters"));
    }

    #[tokio::test]
    async fn validation_rejects_both_domain_filters() {
        let ctx = validation_ctx();
        let tool = WebSearchTool::new(ctx);
        let err = tool
            .validate_input(
                &json!({
                    "query": "rust async",
                    "allowed_domains": ["docs.rs"],
                    "blocked_domains": ["spam.example"]
                }),
                &fresh_ctx(),
            )
            .await
            .expect_err("both domain filters must be rejected");
        // `ValidationError`'s Display prepends `invalid tool input: `; the
        // message bytes must match the TS string exactly.
        assert!(
            err.to_string().contains(
                "Error: Cannot specify both allowed_domains and blocked_domains in the same request"
            ),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn validation_allows_single_domain_filter() {
        let ctx = validation_ctx();
        let tool = WebSearchTool::new(ctx);
        // Only allowed_domains set — must pass.
        tool.validate_input(
            &json!({ "query": "rust async", "allowed_domains": ["docs.rs"] }),
            &fresh_ctx(),
        )
        .await
        .expect("single allowed_domains is valid");
        // Empty arrays on both sides are not "specifying both".
        tool.validate_input(
            &json!({ "query": "rust async", "allowed_domains": [], "blocked_domains": [] }),
            &fresh_ctx(),
        )
        .await
        .expect("empty domain arrays are valid");
    }

    #[tokio::test]
    async fn call_exposes_model_content_with_header_and_footer() {
        // Streaming path: a single leading-text block.
        let http = Arc::new(StreamingMockHttp::new());
        http.set_stream(vec![
            sse(json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } })),
            sse(json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Here are results:" } })),
            sse(json!({ "type": "content_block_stop", "index": 0 })),
            sse(json!({ "type": "message_stop" })),
        ]);
        let (ctx, _sink) = make_streaming_ctx(http);
        let tool = WebSearchTool::new(ctx);
        let (tx, _rx) = progress_channel();
        let res = tool
            .call(json!({ "query": "rust async" }), fresh_ctx(), tx)
            .await
            .expect("ok");
        let mc = res.data["model_content"].as_str().expect("model_content str");
        assert!(mc.starts_with("Web search results for query: \"rust async\"\n\n"));
        assert!(mc.contains("Here are results:"));
        assert!(mc.ends_with(
            "REMINDER: You MUST include the sources above in your response to the user using markdown hyperlinks."
        ));
        // Structured results array is still present for the TUI.
        assert!(res.data["results"].as_array().is_some());
    }

    #[tokio::test]
    async fn streaming_path_does_not_retry_internally() {
        // A successful streaming run must issue exactly ONE request (no internal
        // retry / no spurious fallback when the stream connects).
        let http = Arc::new(StreamingMockHttp::new());
        http.set_stream(vec![
            sse(json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } })),
            sse(json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "ok" } })),
            sse(json!({ "type": "content_block_stop", "index": 0 })),
            sse(json!({ "type": "message_stop" })),
        ]);
        let (ctx, _sink) = make_streaming_ctx(http.clone());
        let tool = WebSearchTool::new(ctx);
        let (tx, _rx) = progress_channel();
        let _ = tool
            .call(json!({ "query": "rust" }), fresh_ctx(), tx)
            .await
            .expect("ok");
        assert_eq!(
            http.received_requests().len(),
            1,
            "successful stream must issue exactly one request (no self-retry)"
        );
    }
}
