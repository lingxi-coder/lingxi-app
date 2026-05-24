//! `WebFetchTool` — fetches a URL via the M1 `HttpTransport` trait, with a
//! 5 MB body cap, scheme allow-list (`https`/`http`), and the locked
//! `claude-code-tool/<CARGO_PKG_VERSION>` User-Agent. Spec §4 Flow B + §7
//! Web wire identifiers.
//!
//! Wire-locked constants (all byte-checked against `parity_web_tools.json`):
//! - `WEBFETCH_MAX_BYTES = 5_242_880` (5 MB)
//! - `WEBFETCH_TRUNCATION_SUFFIX = "\n\n[Content truncated due to length...]"`
//! - `WEBFETCH_USER_AGENT_PREFIX = "claude-code-tool/"`
//! - `WEBFETCH_ALLOWED_SCHEMES = ["https", "http"]`
//! - HTTP error format: `"WebFetch: HTTP {status} from {url}"`
//! - DNS error format: `"WebFetch: cannot resolve {host}"`

use crate::builtin::BuiltinToolContext;
use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_protocol::{HttpMethod, HttpRequest};
use lingxi_telemetry::pii::{PiiTagged, Verified};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{WEB_FETCH_COMPLETED, WEB_FETCH_FAILED, WEB_FETCH_STARTED};
use lingxi_traits::http::HttpError;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Maximum response-body size before truncation (5 MB). Spec §7 lock.
pub const WEBFETCH_MAX_BYTES: usize = 5 * 1024 * 1024;

/// Suffix appended to the body when it overflows [`WEBFETCH_MAX_BYTES`].
/// Spec §7 lock; matches `claude-code/src/tools/WebFetchTool/utils.ts:532`.
pub const WEBFETCH_TRUNCATION_SUFFIX: &str = "\n\n[Content truncated due to length...]";

/// User-Agent prefix for the tool-side fetch (distinct from api-client UA).
/// The full header value is `concat!(WEBFETCH_USER_AGENT_PREFIX, env!("CARGO_PKG_VERSION"))`.
/// Spec §7 lock.
pub const WEBFETCH_USER_AGENT_PREFIX: &str = "claude-code-tool/";

/// Schemes that `WebFetchTool` will accept. Spec §7 lock.
pub const WEBFETCH_ALLOWED_SCHEMES: &[&str] = &["https", "http"];

/// Per-request HTTP timeout for WebFetch.
pub const WEBFETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Canonical tool name in the registry.
pub const TOOL_NAME: &str = "WebFetch";

/// Input schema for `WebFetchTool`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebFetchInput {
    /// Absolute URL to fetch. Must be `https://` or `http://`.
    pub url: String,
    /// Optional prompt for downstream model use (not consumed by the tool itself).
    #[serde(default)]
    pub prompt: Option<String>,
}

/// Validate that `url_str` parses AND uses an allowed scheme.
///
/// Returns the parsed `url::Url` on success, or a descriptive error string on
/// failure. On scheme miss, the error string is byte-locked to
/// `"URL scheme '{scheme}' not allowed; only https/http"` (spec §5).
///
/// # Errors
/// Returns an error if the URL fails to parse or uses a non-allowed scheme.
pub fn validate_url(url_str: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(url_str).map_err(|e| format!("invalid URL: {e}"))?;
    let scheme = parsed.scheme();
    if !WEBFETCH_ALLOWED_SCHEMES.contains(&scheme) {
        return Err(format!(
            "URL scheme '{scheme}' not allowed; only https/http"
        ));
    }
    Ok(parsed)
}

/// Truncate `body` so that its byte length is `<= WEBFETCH_MAX_BYTES`, falling
/// back to the nearest UTF-8 char boundary so we never split a multi-byte
/// codepoint. If truncated, [`WEBFETCH_TRUNCATION_SUFFIX`] is appended.
///
/// Returns `(possibly_truncated_body, truncated_flag)`.
#[must_use]
pub fn truncate_body(body: String) -> (String, bool) {
    if body.len() <= WEBFETCH_MAX_BYTES {
        return (body, false);
    }
    let mut cut = WEBFETCH_MAX_BYTES;
    while cut > 0 && !body.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut truncated = body[..cut].to_string();
    truncated.push_str(WEBFETCH_TRUNCATION_SUFFIX);
    (truncated, true)
}

/// Format the byte-locked HTTP-error string. Spec §5:
/// `"WebFetch: HTTP {status} from {url}"`.
#[must_use]
pub fn fmt_http_error(status: u16, url: &str) -> String {
    format!("WebFetch: HTTP {status} from {url}")
}

/// Format the byte-locked DNS-error string. Spec §5:
/// `"WebFetch: cannot resolve {host}"`.
#[must_use]
pub fn fmt_dns_error(host: &str) -> String {
    format!("WebFetch: cannot resolve {host}")
}

/// Heuristic: does an `HttpError::Connection(msg)` look like a DNS failure?
///
/// Matches strings containing "resolve", "name resolution", or "nodename nor
/// servname" (case-insensitive).
#[must_use]
pub fn is_dns_failure(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("resolve")
        || lower.contains("name resolution")
        || lower.contains("nodename nor servname")
}

/// `WebFetchTool` — fetches an HTTPS/HTTP URL with a 5 MB cap and the locked
/// truncation suffix on overflow. Never self-retries on transient 5xx (spec §5).
pub struct WebFetchTool {
    ctx: BuiltinToolContext,
}

impl WebFetchTool {
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

    async fn emit_started(&self, invocation_id: &str, url: &str, prompt_present: bool) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "_PROTO_url".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(url.to_string()).into_inner(),
            ),
        );
        md.insert(
            "prompt_present".into(),
            AnalyticsValue::String(Verified::assert_safe(prompt_present.to_string()).into_inner()),
        );
        self.ctx.bus.log_event(WEB_FETCH_STARTED, md).await;
    }

    async fn emit_completed(
        &self,
        invocation_id: &str,
        status: u16,
        body_bytes: u64,
        truncated: bool,
        duration_ms: u64,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert("status".into(), AnalyticsValue::Int(i64::from(status)));
        md.insert("body_bytes".into(), AnalyticsValue::Int(body_bytes as i64));
        md.insert(
            "truncated".into(),
            AnalyticsValue::String(Verified::assert_safe(truncated.to_string()).into_inner()),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(WEB_FETCH_COMPLETED, md).await;
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
        self.ctx.bus.log_event(WEB_FETCH_FAILED, md).await;
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["url"],
        "properties": {
            "url": { "type": "string", "format": "uri" },
            "prompt": { "type": "string" }
        }
    })
});

#[async_trait]
impl Tool for WebFetchTool {
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

    async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
        "Fetches a URL and returns its content (HTTPS/HTTP only, 5 MB cap).".into()
    }
    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "WebFetch fetches a single URL via GET. Use https:// or http:// only. \
         Bodies > 5 MB are truncated with a marker."
            .into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let parsed_input: WebFetchInput = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(format!("invalid input: {e}")))?;
        let parsed_url = validate_url(&parsed_input.url).map_err(ToolError::InvalidInput)?;
        let host = parsed_url.host_str().unwrap_or("<unknown>").to_string();
        let invocation_id = crate::builtin::file_read::ulid_or_uuid();

        self.emit_started(
            &invocation_id,
            &parsed_input.url,
            parsed_input.prompt.is_some(),
        )
        .await;

        let started = Instant::now();
        let req = HttpRequest {
            method: HttpMethod::Get,
            url: parsed_url.to_string(),
            headers: vec![
                ("user-agent".into(), Self::user_agent()),
                ("accept".into(), "text/markdown, text/html, */*".into()),
            ],
            body: None,
            timeout: Some(WEBFETCH_TIMEOUT),
        };
        let resp_result = self.ctx.http.request(req).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;

        match resp_result {
            Ok(resp) if resp.status >= 400 => {
                let err_msg = fmt_http_error(resp.status, &parsed_input.url);
                self.emit_failed(&invocation_id, "http_status", Some(resp.status), elapsed_ms)
                    .await;
                Err(ToolError::Transport(err_msg))
            }
            Ok(resp) => {
                let status = resp.status;
                let body_bytes = resp.body.len();
                let (final_body, truncated) = truncate_body(resp.body);
                self.emit_completed(
                    &invocation_id,
                    status,
                    body_bytes as u64,
                    truncated,
                    elapsed_ms,
                )
                .await;
                Ok(ToolCallResult {
                    data: json!({
                        "url": parsed_input.url,
                        "status": status,
                        "content": final_body,
                        "truncated": truncated,
                        "bytes": body_bytes,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Err(HttpError::Status { status, body: _ }) => {
                let err_msg = fmt_http_error(status, &parsed_input.url);
                self.emit_failed(&invocation_id, "http_status", Some(status), elapsed_ms)
                    .await;
                Err(ToolError::Transport(err_msg))
            }
            Err(HttpError::Connection(msg)) if is_dns_failure(&msg) => {
                let err_msg = fmt_dns_error(&host);
                self.emit_failed(&invocation_id, "dns", None, elapsed_ms)
                    .await;
                Err(ToolError::Transport(err_msg))
            }
            Err(HttpError::Connection(msg)) => {
                let err_msg = format!("WebFetch: connection failed: {msg}");
                self.emit_failed(&invocation_id, "connection", None, elapsed_ms)
                    .await;
                Err(ToolError::Transport(err_msg))
            }
            Err(HttpError::Timeout(_)) => {
                let err_msg = "WebFetch: request timed out".to_string();
                self.emit_failed(&invocation_id, "timeout", None, elapsed_ms)
                    .await;
                Err(ToolError::Transport(err_msg))
            }
            Err(other) => {
                let err_msg = format!("WebFetch: invalid response: {other}");
                self.emit_failed(&invocation_id, "invalid_response", None, elapsed_ms)
                    .await;
                Err(ToolError::Transport(err_msg))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_url() {
        let u = validate_url("https://example.com/path").expect("https must be allowed");
        assert_eq!(u.scheme(), "https");
        assert_eq!(u.host_str(), Some("example.com"));
    }

    #[test]
    fn accepts_http_url() {
        let u = validate_url("http://example.com").expect("http must be allowed");
        assert_eq!(u.scheme(), "http");
    }

    #[test]
    fn rejects_file_scheme() {
        let err = validate_url("file:///etc/passwd").expect_err("file:// must be rejected");
        assert_eq!(err, "URL scheme 'file' not allowed; only https/http");
    }

    #[test]
    fn rejects_data_scheme() {
        let err = validate_url("data:text/plain,hello").expect_err("data: must be rejected");
        assert_eq!(err, "URL scheme 'data' not allowed; only https/http");
    }

    #[test]
    fn rejects_ftp_scheme() {
        let err = validate_url("ftp://example.com/file").expect_err("ftp:// must be rejected");
        assert_eq!(err, "URL scheme 'ftp' not allowed; only https/http");
    }

    #[test]
    fn rejects_malformed_url() {
        let err = validate_url("not-a-url").expect_err("malformed must be rejected");
        assert!(err.starts_with("invalid URL:"));
    }

    #[test]
    fn locked_constants_match_spec() {
        assert_eq!(WEBFETCH_MAX_BYTES, 5_242_880);
        assert_eq!(
            WEBFETCH_TRUNCATION_SUFFIX,
            "\n\n[Content truncated due to length...]"
        );
        assert_eq!(WEBFETCH_USER_AGENT_PREFIX, "claude-code-tool/");
        assert_eq!(WEBFETCH_ALLOWED_SCHEMES, &["https", "http"]);
    }

    #[test]
    fn tool_name_is_webfetch() {
        assert_eq!(TOOL_NAME, "WebFetch");
    }

    #[test]
    fn does_not_truncate_small_body() {
        let small = "hello world".to_string();
        let (out, flag) = truncate_body(small.clone());
        assert_eq!(out, small);
        assert!(!flag);
    }

    #[test]
    fn does_not_truncate_exactly_at_cap() {
        let exact = "a".repeat(WEBFETCH_MAX_BYTES);
        let (out, flag) = truncate_body(exact.clone());
        assert_eq!(out, exact);
        assert!(!flag);
    }

    #[test]
    fn truncates_oversized_body() {
        let big = "a".repeat(WEBFETCH_MAX_BYTES + 1024);
        let (out, flag) = truncate_body(big);
        assert!(flag);
        assert!(out.ends_with(WEBFETCH_TRUNCATION_SUFFIX));
        let suffix_len = WEBFETCH_TRUNCATION_SUFFIX.len();
        assert_eq!(out.len(), WEBFETCH_MAX_BYTES + suffix_len);
    }

    #[test]
    fn truncates_multibyte_at_char_boundary() {
        let mut s = "a".repeat(WEBFETCH_MAX_BYTES - 1);
        s.push('日');
        s.push('日');
        let (out, flag) = truncate_body(s);
        assert!(flag);
        let body_only = &out[..out.len() - WEBFETCH_TRUNCATION_SUFFIX.len()];
        assert!(body_only.is_char_boundary(body_only.len()));
        // Walk-back from MAX_BYTES (which lands inside the first "日") must end
        // at MAX_BYTES - 1 (the byte just before "日").
        assert_eq!(body_only.len(), WEBFETCH_MAX_BYTES - 1);
    }

    #[test]
    fn fmt_http_error_matches_lock() {
        assert_eq!(
            fmt_http_error(500, "https://example.com/"),
            "WebFetch: HTTP 500 from https://example.com/"
        );
        assert_eq!(
            fmt_http_error(404, "http://localhost/path"),
            "WebFetch: HTTP 404 from http://localhost/path"
        );
    }

    #[test]
    fn fmt_dns_error_matches_lock() {
        assert_eq!(
            fmt_dns_error("doesnotexist.invalid"),
            "WebFetch: cannot resolve doesnotexist.invalid"
        );
    }

    #[test]
    fn is_dns_failure_detects_resolve_text() {
        assert!(is_dns_failure("failed to resolve host"));
        assert!(is_dns_failure("FAILED TO RESOLVE HOST"));
        assert!(is_dns_failure("name resolution error"));
        assert!(is_dns_failure("nodename nor servname provided"));
    }

    #[test]
    fn is_dns_failure_returns_false_for_unrelated() {
        assert!(!is_dns_failure("connection refused"));
        assert!(!is_dns_failure("tls handshake failed"));
        assert!(!is_dns_failure(""));
    }

    // ---- async impl Tool tests using MockHttpTransport ---------------------

    use crate::builtin::test_support::{fresh_ctx, fresh_tx};
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
    async fn surfaces_http_500_as_transport() {
        let (ctx, http, sink) = make_web_ctx();
        ctx.bus.attach_sink(sink.clone()).await;
        http.enqueue(ok_response(500, "server error"));

        let tool = WebFetchTool::new(ctx);
        let err = tool
            .call(
                json!({ "url": "https://example.com/x" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("500 must be Err");
        match err {
            ToolError::Transport(msg) => {
                assert_eq!(msg, "WebFetch: HTTP 500 from https://example.com/x");
            }
            other => panic!("expected Transport, got {other:?}"),
        }
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_web_fetch_started"));
        assert!(names.contains(&"tengu_tool_web_fetch_failed"));
        assert!(!names.contains(&"tengu_tool_web_fetch_completed"));
    }

    #[tokio::test]
    async fn http_500_does_not_retry_internally() {
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(ok_response(500, "boom"));
        let tool = WebFetchTool::new(ctx);
        let _ = tool
            .call(
                json!({ "url": "https://example.com/" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await;
        assert_eq!(http.received_requests().len(), 1, "must NOT self-retry");
    }

    #[tokio::test]
    async fn surfaces_dns_failure() {
        let (ctx, http, sink) = make_web_ctx();
        ctx.bus.attach_sink(sink.clone()).await;
        http.enqueue(ScriptedResponse::SyncErr(HttpError::Connection(
            "failed to resolve host doesnotexist.invalid".into(),
        )));
        let tool = WebFetchTool::new(ctx);
        let err = tool
            .call(
                json!({ "url": "https://doesnotexist.invalid/" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("DNS failure must be Err");
        match err {
            ToolError::Transport(msg) => {
                assert_eq!(msg, "WebFetch: cannot resolve doesnotexist.invalid");
            }
            other => panic!("expected Transport, got {other:?}"),
        }
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_web_fetch_failed"));
    }

    #[tokio::test]
    async fn happy_path_emits_completed() {
        let (ctx, http, sink) = make_web_ctx();
        ctx.bus.attach_sink(sink.clone()).await;
        http.enqueue(ok_response(200, "hello world"));
        let tool = WebFetchTool::new(ctx);
        let res = tool
            .call(
                json!({ "url": "https://example.com/" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok response");
        assert_eq!(res.data["status"], 200);
        assert_eq!(res.data["content"], "hello world");
        assert_eq!(res.data["truncated"], false);
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_web_fetch_completed"));
    }

    #[tokio::test]
    async fn sets_user_agent_header() {
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(ok_response(200, "ok"));
        let tool = WebFetchTool::new(ctx);
        let _ = tool
            .call(
                json!({ "url": "https://example.com/" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await;
        let reqs = http.received_requests();
        let last_req = reqs.last().expect("captured");
        let (_, ua_value) = last_req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
            .expect("must have user-agent header");
        assert!(
            ua_value.starts_with(WEBFETCH_USER_AGENT_PREFIX),
            "UA `{ua_value}` must start with `{WEBFETCH_USER_AGENT_PREFIX}`"
        );
        assert!(
            ua_value.ends_with(env!("CARGO_PKG_VERSION")),
            "UA `{ua_value}` must end with crate version"
        );
    }

    #[tokio::test]
    async fn rejects_file_scheme_in_call() {
        let (ctx, _http, _sink) = make_web_ctx();
        let tool = WebFetchTool::new(ctx);
        let err = tool
            .call(
                json!({ "url": "file:///etc/passwd" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("file:// must be rejected");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        assert!(format!("{err}").contains("URL scheme 'file' not allowed; only https/http"));
    }
}
