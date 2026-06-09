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

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use protocol::{HttpMethod, HttpRequest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{WEB_FETCH_COMPLETED, WEB_FETCH_FAILED, WEB_FETCH_STARTED};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use tool_api::BuiltinToolContext;
use traits::http::HttpError;

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
    // claude-code `validateURL` SSRF/abuse gate: overlong URL, embedded
    // credentials, single-label/internal hostname. (The scheme allow-list above
    // is a Rust-side addition kept for defence-in-depth — claude-code upgrades
    // http→https instead of rejecting non-https here.)
    crate::url_safety::validate_url_safety(url_str, &parsed)?;
    Ok(parsed)
}

/// Upgrade an `http:` URL to `https:` in place, mirroring `utils.ts:406-416`:
///
/// ```text
/// if (parsedUrl.protocol === 'http:') {
///   parsedUrl.protocol = 'https:'
///   upgradedUrl = parsedUrl.toString()
/// }
/// ```
///
/// Only the scheme changes; the URL is otherwise untouched, and `https:` (or any
/// other scheme) is left as-is. claude-code *upgrades* http→https rather than
/// rejecting non-https, so the fetch always hits the secure URL while the cache
/// and tool output still echo the caller's original URL.
pub fn upgrade_to_https(url: &mut url::Url) {
    if url.scheme() == "http" {
        // `set_scheme` only fails on an invalid scheme transition; http→https is
        // always valid, so the `Result` cannot be `Err` here. Ignore it.
        let _ = url.set_scheme("https");
    }
}

/// Map a redirect status code to its TS-exact status-text label, mirroring the
/// ternary in `WebFetchTool.ts:218-225`:
///
/// `301 → "Moved Permanently"`, `308 → "Permanent Redirect"`,
/// `307 → "Temporary Redirect"`, everything else → `"Found"`.
#[must_use]
pub fn redirect_status_text(code: u16) -> &'static str {
    match code {
        301 => "Moved Permanently",
        308 => "Permanent Redirect",
        307 => "Temporary Redirect",
        _ => "Found",
    }
}

/// Build the byte-exact "REDIRECT DETECTED" message from `WebFetchTool.ts:227-235`.
///
/// `prompt` is interpolated verbatim into the `- prompt: "${prompt}"` line; pass
/// the empty string when the caller supplied no prompt (TS interpolates
/// `undefined` as the string `"undefined"`, but the Rust input models an absent
/// prompt as `None`/`""` — Batch 4 wires the real per-hop value through, so this
/// scaffold takes the already-resolved string).
///
/// The returned string is suitable for the tool's `content`/`result` field once
/// Batch 4 adds real per-hop redirect detection.
#[must_use]
pub fn format_redirect_message(
    original_url: &str,
    redirect_url: &str,
    status_code: u16,
    prompt: &str,
) -> String {
    let status_text = redirect_status_text(status_code);
    format!(
        "REDIRECT DETECTED: The URL redirects to a different host.\n\
         \n\
         Original URL: {original_url}\n\
         Redirect URL: {redirect_url}\n\
         Status: {status_code} {status_text}\n\
         \n\
         To complete your request, I need to fetch content from the redirected URL. \
         Please use WebFetch again with these parameters:\n\
         - url: \"{redirect_url}\"\n\
         - prompt: \"{prompt}\""
    )
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

/// Whether the domain blocklist preflight should be skipped for this fetch.
///
/// Mirrors `settings.skipWebFetchPreflight` (`utils.ts:423-424`) — the
/// enterprise-customer escape hatch for hosts whose network policy blocks
/// outbound connections to `claude.ai`/`api.anthropic.com`.
///
/// **Interim wiring (flagged):** the faithful source is a settings-derived
/// `BuiltinToolContext::skip_web_fetch_preflight` field populated at tool
/// registration. Threading that field touches `tool-api`'s shared
/// `builtin_context.rs` plus the registration site in the composition crate —
/// out of this batch's `tool-web`-only scope (and it would collide with Batch 5's
/// `builtin_context.rs` edit). The batch spec explicitly sanctions an env-var
/// interim: `LINGXI_SKIP_WEBFETCH_PREFLIGHT` truthy (`1`/`true`/`yes`/`on`,
/// case-insensitive) skips the preflight. Follow-up: replace this with the
/// context field once both `builtin_context.rs` fields land together.
#[must_use]
fn skip_web_fetch_preflight() -> bool {
    std::env::var("LINGXI_SKIP_WEBFETCH_PREFLIGHT")
        .ok()
        .is_some_and(|v| {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        })
}

/// `WebFetchTool` — fetches an HTTPS/HTTP URL with a 5 MB cap and the locked
/// truncation suffix on overflow. Never self-retries on transient 5xx (spec §5).
pub struct WebFetchTool {
    ctx: BuiltinToolContext,
    /// Optional small-fast side-query client for the apply step. Wired only at
    /// the desktop composition root (None on mobile/minimal — see plan).
    side_query: Option<std::sync::Arc<dyn sidequery::SideQueryClient>>,
}

impl WebFetchTool {
    /// Construct a new tool (no apply step until [`Self::with_side_query`]).
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx, side_query: None }
    }

    /// Attach the side-query client that powers the secondary-model apply step.
    #[must_use]
    pub fn with_side_query(
        mut self,
        client: std::sync::Arc<dyn sidequery::SideQueryClient>,
    ) -> Self {
        self.side_query = Some(client);
        self
    }

    fn user_agent() -> String {
        format!(
            "{}{}",
            WEBFETCH_USER_AGENT_PREFIX,
            env!("CARGO_PKG_VERSION")
        )
    }

    /// Resolve the small-fast model id for the apply step. Anthropic-family
    /// default => Haiku; otherwise fall back to the configured default model.
    #[cfg(feature = "web-markdown")]
    fn apply_model(&self) -> String {
        if self.ctx.default_model.contains("claude") {
            "claude-haiku-4-5".to_string()
        } else {
            self.ctx.default_model.clone()
        }
    }

    /// Run the secondary-model apply step over `markdown` with `prompt`. Returns
    /// the model's text (or the fallback "No response from model").
    #[cfg(feature = "web-markdown")]
    async fn apply_prompt(
        &self,
        client: &std::sync::Arc<dyn sidequery::SideQueryClient>,
        host: &str,
        markdown: &str,
        prompt: &str,
    ) -> String {
        use protocol::{ConversationMessage, MessageId};
        use sidequery::{QuerySource, SideQueryRequest};
        let truncated = crate::markdown::truncate_markdown(markdown.to_string());
        let model_prompt = crate::markdown::make_secondary_model_prompt(
            &truncated,
            prompt,
            crate::markdown::is_preapproved_domain(host),
        );
        let req = SideQueryRequest {
            model: self.apply_model(),
            system_prompt: None,
            messages: vec![ConversationMessage::user(MessageId::new(), model_prompt)],
            tools: vec![],
            tool_choice: None,
            output_format: None,
            max_tokens: 1024,
            max_retries: 1,
            temperature: None,
            thinking_budget: None,
            stop_sequences: vec![],
            query_source: QuerySource::WebFetchApply,
            skip_system_prompt_prefix: true,
        };
        match client.query(req).await {
            Ok(resp) => resp.text.unwrap_or_else(|| "No response from model".to_string()),
            Err(_) => "No response from model".to_string(),
        }
    }

    /// Returns `Some(model_output)` when the apply step ran, else `None`.
    #[cfg(feature = "web-markdown")]
    async fn maybe_apply(
        &self,
        host: &str,
        content: &str,
        prompt: Option<&str>,
    ) -> Option<String> {
        if let (Some(client), Some(p)) = (self.side_query.as_ref(), prompt) {
            return Some(self.apply_prompt(client, host, content, p).await);
        }
        None
    }

    /// No-op fallback when `web-markdown` is disabled.
    #[cfg(not(feature = "web-markdown"))]
    #[allow(clippy::unused_async)]
    async fn maybe_apply(
        &self,
        _host: &str,
        _content: &str,
        _prompt: Option<&str>,
    ) -> Option<String> {
        None
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

    async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
        "Fetches a URL and returns its content (HTTPS/HTTP only, 5 MB cap).".into()
    }
    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "WebFetch fetches a single URL via GET. Use https:// or http:// only. \
         Bodies > 5 MB are truncated with a marker."
            .into()
    }

    /// Reject an unparseable URL early with the byte-exact upstream message.
    ///
    /// Mirrors `WebFetchTool.validateInput` (`WebFetchTool.ts:191-204`,
    /// `errorCode: 1`, `meta.reason: 'invalid_url'`): it only checks that the URL
    /// PARSES (`new URL(url)` ⇄ `url::Url::parse`) — scheme/SSRF gating happens
    /// later in `call()` via `validate_url`, exactly as upstream defers it to
    /// `getURLMarkdownContent`. A parseable-but-wrong-scheme URL (e.g. `file://`)
    /// passes this gate and is rejected in `call()`.
    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), tool_api::tool_trait::ValidationError> {
        let url = input.get("url").and_then(Value::as_str).unwrap_or("");
        if url::Url::parse(url).is_err() {
            return Err(tool_api::tool_trait::ValidationError(format!(
                "Error: Invalid URL \"{url}\". The URL provided could not be parsed."
            )));
        }
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let parsed_input: WebFetchInput = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(format!("invalid input: {e}")))?;
        let mut parsed_url = validate_url(&parsed_input.url).map_err(ToolError::InvalidInput)?;
        let invocation_id = tool_api::util::ids::ulid_or_uuid();

        self.emit_started(
            &invocation_id,
            &parsed_input.url,
            parsed_input.prompt.is_some(),
        )
        .await;

        // Cache check (keyed by the *original* URL), before the http→https
        // upgrade — mirrors `utils.ts:392-404`, which short-circuits on a hit
        // ahead of the upgrade block at `utils.ts:406-416`. Repeat fetches of
        // the same URL return instantly without a second network round-trip.
        if let Some(hit) = crate::cache::cache_get(&parsed_input.url) {
            // A fetch was truncated iff the raw body exceeded the cap; reproduce
            // the same `truncated` flag the live path would have set.
            let truncated = hit.bytes > WEBFETCH_MAX_BYTES;
            // Cache hits do no network work, so the reported duration is 0 ms.
            self.emit_completed(&invocation_id, hit.status, hit.bytes as u64, truncated, 0)
                .await;
            return Ok(ToolCallResult {
                data: json!({
                    "url": parsed_input.url,
                    "status": hit.status,
                    "content": hit.content,
                    "truncated": truncated,
                    "bytes": hit.bytes,
                }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            });
        }

        // Upgrade http→https before fetching (`utils.ts:406-416`). The cache and
        // tool output still echo the caller's original URL; only the network
        // request targets the upgraded one.
        upgrade_to_https(&mut parsed_url);
        let host = parsed_url.host_str().unwrap_or("<unknown>").to_string();

        // Domain blocklist preflight (`utils.ts:420-435`). Runs on every host
        // (cache-miss path only — a URL cache hit returned above) unless the
        // user opted to skip it. `Blocked`/`CheckFailed` map to the byte-locked
        // user-facing error messages; `Allowed` continues to the fetch.
        if !skip_web_fetch_preflight() {
            match crate::blocklist::check_domain_blocklist(self.ctx.http.as_ref(), &host).await {
                crate::blocklist::DomainCheckResult::Allowed => {}
                crate::blocklist::DomainCheckResult::Blocked => {
                    self.emit_failed(&invocation_id, "domain_blocked", None, 0)
                        .await;
                    return Err(ToolError::Transport(crate::blocklist::domain_blocked_msg(
                        &host,
                    )));
                }
                crate::blocklist::DomainCheckResult::CheckFailed(_) => {
                    self.emit_failed(&invocation_id, "domain_check_failed", None, 0)
                        .await;
                    return Err(ToolError::Transport(
                        crate::blocklist::domain_check_failed_msg(&host),
                    ));
                }
            }
        }

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
                let content_type = resp
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map_or_else(String::new, |(_, v)| v.clone());
                let (raw_body, truncated) = truncate_body(resp.body);

                // HTML->markdown (claude-code converts HTML; non-HTML is used as-is).
                // Behind `web-markdown`; feature off => content is the raw body.
                #[cfg(feature = "web-markdown")]
                let content = if crate::markdown::is_html_content_type(&content_type) {
                    crate::markdown::html_to_markdown(&raw_body)
                } else {
                    raw_body
                };
                #[cfg(not(feature = "web-markdown"))]
                let content = raw_body;

                crate::cache::cache_set(
                    parsed_input.url.clone(),
                    crate::cache::CachedFetch {
                        content: content.clone(),
                        status,
                        content_type,
                        bytes: body_bytes,
                        persisted_path: None,
                    },
                );
                self.emit_completed(&invocation_id, status, body_bytes as u64, truncated, elapsed_ms)
                    .await;

                let out_content = self
                    .maybe_apply(&host, &content, parsed_input.prompt.as_deref())
                    .await
                    .unwrap_or(content);

                Ok(ToolCallResult {
                    data: json!({
                        "url": parsed_input.url,
                        "status": status,
                        "content": out_content,
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

    // ---- http→https upgrade (utils.ts:406-416) -----------------------------

    #[test]
    fn upgrade_http_to_https() {
        let mut u = url::Url::parse("http://x.com/a").unwrap();
        upgrade_to_https(&mut u);
        assert_eq!(u.as_str(), "https://x.com/a");
        assert_eq!(u.scheme(), "https");
    }

    #[test]
    fn upgrade_leaves_https_untouched() {
        let mut u = url::Url::parse("https://x.com/a?q=1#frag").unwrap();
        let before = u.as_str().to_string();
        upgrade_to_https(&mut u);
        assert_eq!(u.as_str(), before);
    }

    #[test]
    fn upgrade_preserves_path_query_port() {
        let mut u = url::Url::parse("http://x.com:8080/a/b?q=1&z=2#h").unwrap();
        upgrade_to_https(&mut u);
        // url normalizes 8080 (non-default for https) — it is retained.
        assert_eq!(u.as_str(), "https://x.com:8080/a/b?q=1&z=2#h");
    }

    // ---- redirect status text (WebFetchTool.ts:218-225) --------------------

    #[test]
    fn redirect_status_text_matches_ts() {
        assert_eq!(redirect_status_text(301), "Moved Permanently");
        assert_eq!(redirect_status_text(308), "Permanent Redirect");
        assert_eq!(redirect_status_text(307), "Temporary Redirect");
        // Everything else (incl. 302, 303, 200, 0) falls through to "Found".
        assert_eq!(redirect_status_text(302), "Found");
        assert_eq!(redirect_status_text(303), "Found");
        assert_eq!(redirect_status_text(200), "Found");
        assert_eq!(redirect_status_text(0), "Found");
    }

    // ---- redirect message (WebFetchTool.ts:227-235) ------------------------

    #[test]
    fn format_redirect_message_byte_matches_ts() {
        // Byte-for-byte reproduction of the WebFetchTool.ts template literal.
        let expected = "REDIRECT DETECTED: The URL redirects to a different host.\n\
\n\
Original URL: https://orig.example/page\n\
Redirect URL: https://other.example/landing\n\
Status: 301 Moved Permanently\n\
\n\
To complete your request, I need to fetch content from the redirected URL. Please use WebFetch again with these parameters:\n\
- url: \"https://other.example/landing\"\n\
- prompt: \"summarize this\"";
        let got = format_redirect_message(
            "https://orig.example/page",
            "https://other.example/landing",
            301,
            "summarize this",
        );
        assert_eq!(got, expected);
    }

    #[test]
    fn format_redirect_message_uses_found_for_unknown_code() {
        let got = format_redirect_message("https://a/", "https://b/", 302, "");
        assert!(got.contains("Status: 302 Found"));
        assert!(got.contains("- prompt: \"\""));
        assert!(got.contains("- url: \"https://b/\""));
    }

    // ---- async impl Tool tests using MockHttpTransport ---------------------

    use std::sync::Arc;
    use telemetry::sinks::InMemorySink;
    use telemetry::AnalyticsBus;
    use test_harness::mocks::{MockHttpTransport, ScriptedResponse};
    use tool_api::test_support::{fresh_ctx, fresh_tx};
    use traits::http::HttpTransport;

    /// Serializes every test that touches the process-global
    /// `LINGXI_SKIP_WEBFETCH_PREFLIGHT` env var. The skip test *sets* it; the
    /// preflight-dependent `call()` tests *read* it (via `skip_web_fetch_preflight`)
    /// and would be corrupted if the skip test's mutation leaked into them while
    /// running in parallel. Mirrors the `HOME_LOCK` env-isolation idiom. A tokio
    /// mutex (not `std`) keeps the guard `Send` across the `.await` points in the
    /// async tests.
    static SKIP_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn make_web_ctx() -> (
        BuiltinToolContext,
        Arc<MockHttpTransport>,
        Arc<InMemorySink>,
    ) {
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        let http = Arc::new(MockHttpTransport::new());
        let mut ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![std::path::PathBuf::from("/tmp")],
        );
        ctx.http = http.clone() as Arc<dyn HttpTransport>;
        (ctx, http, sink)
    }

    fn ok_response(status: u16, body: &str) -> ScriptedResponse {
        ScriptedResponse::Sync(protocol::HttpResponse {
            status,
            headers: vec![],
            body: body.to_string(),
        })
    }

    /// A `domain_info` preflight response that allows the fetch. The mock
    /// transport is FIFO and URL-agnostic, so the preflight GET (which the
    /// `call()` pipeline issues first, on a cache miss) consumes whatever is at
    /// the front of the queue — enqueue this *before* the fetch body response.
    fn preflight_allow() -> ScriptedResponse {
        ok_response(200, r#"{"can_fetch":true}"#)
    }

    #[tokio::test]
    async fn surfaces_http_500_as_transport() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, sink) = make_web_ctx();
        ctx.bus.attach_sink(sink.clone()).await;
        // Preflight allows, then the fetch returns 500.
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(500, "server error"));

        let tool = WebFetchTool::new(ctx);
        let err = tool
            .call(
                json!({ "url": "https://http500.example/x" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("500 must be Err");
        match err {
            ToolError::Transport(msg) => {
                assert_eq!(msg, "WebFetch: HTTP 500 from https://http500.example/x");
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
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(500, "boom"));
        let tool = WebFetchTool::new(ctx);
        let _ = tool
            .call(
                json!({ "url": "https://noretry.example/" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await;
        // One preflight + exactly one fetch (no self-retry of the 500).
        let reqs = http.received_requests();
        assert_eq!(reqs.len(), 2, "must NOT self-retry");
        assert!(reqs[0].url.contains("/api/web/domain_info?domain="));
        assert_eq!(reqs[1].url, "https://noretry.example/");
    }

    #[tokio::test]
    async fn surfaces_dns_failure() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, sink) = make_web_ctx();
        ctx.bus.attach_sink(sink.clone()).await;
        // Preflight allows; the *fetch* then fails DNS resolution.
        http.enqueue(preflight_allow());
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
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, sink) = make_web_ctx();
        ctx.bus.attach_sink(sink.clone()).await;
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(200, "hello world"));
        let tool = WebFetchTool::new(ctx);
        // Unique URL so the process-global cache can't be pre-warmed by another
        // parallel test (which would skip the fetch).
        let res = tool
            .call(
                json!({ "url": "https://happy.example/happy-path" }),
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
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(200, "ok"));
        let tool = WebFetchTool::new(ctx);
        // Unique URL to avoid a process-global cache hit short-circuiting the
        // fetch (which would leave `received_requests()` empty).
        let _ = tool
            .call(
                json!({ "url": "https://useragent.example/user-agent" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await;
        // The fetch is the LAST request (the preflight has no UA header).
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

    // ---- validateInput parity (WebFetchTool.ts:191-204) --------------------

    #[tokio::test]
    async fn validate_input_rejects_unparseable_url() {
        let (ctx, _http, _sink) = make_web_ctx();
        let tool = WebFetchTool::new(ctx);
        let err = tool
            .validate_input(&json!({ "url": "not a url" }), &fresh_ctx())
            .await
            .expect_err("unparseable URL must be rejected");
        // `ValidationError`'s Display prepends `invalid tool input: `; the message
        // bytes must match the TS string exactly.
        assert!(
            err.to_string().contains(
                "Error: Invalid URL \"not a url\". The URL provided could not be parsed."
            ),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn validate_input_rejects_scheme_relative_url() {
        // Like JS `new URL('example.com')`, `url::Url::parse` rejects a URL with
        // no scheme/base.
        let (ctx, _http, _sink) = make_web_ctx();
        let tool = WebFetchTool::new(ctx);
        let err = tool
            .validate_input(&json!({ "url": "example.com/path" }), &fresh_ctx())
            .await
            .expect_err("schemeless URL must be rejected");
        assert!(err.to_string().contains(
            "Error: Invalid URL \"example.com/path\". The URL provided could not be parsed."
        ));
    }

    #[tokio::test]
    async fn validate_input_accepts_valid_https() {
        let (ctx, _http, _sink) = make_web_ctx();
        let tool = WebFetchTool::new(ctx);
        tool.validate_input(&json!({ "url": "https://example.com/page" }), &fresh_ctx())
            .await
            .expect("a parseable https URL is valid input");
    }

    #[tokio::test]
    async fn validate_input_passes_parseable_non_http_scheme() {
        // Parity: validateInput only checks parseability. `file://` parses, so it
        // passes this gate — the scheme is rejected later in `call()`.
        let (ctx, _http, _sink) = make_web_ctx();
        let tool = WebFetchTool::new(ctx);
        tool.validate_input(&json!({ "url": "file:///etc/passwd" }), &fresh_ctx())
            .await
            .expect("file:// parses, so validateInput accepts it");
    }

    // ---- http→https upgrade + 15-min cache integration ---------------------

    #[tokio::test]
    async fn upgrades_http_to_https_on_the_wire() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(200, "ok"));
        let tool = WebFetchTool::new(ctx);
        // Caller passes an http:// URL; the network request must target https://.
        let res = tool
            .call(
                json!({ "url": "http://upgrade.example/page" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok response");
        // Output echoes the ORIGINAL (un-upgraded) URL.
        assert_eq!(res.data["url"], "http://upgrade.example/page");
        // reqs[0] = the preflight (against the UPGRADED host); reqs[1] = the fetch.
        let reqs = http.received_requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(
            reqs[0].url,
            "https://api.anthropic.com/api/web/domain_info?domain=upgrade.example"
        );
        assert_eq!(reqs[1].url, "https://upgrade.example/page");
    }

    #[tokio::test]
    async fn second_call_is_served_from_cache() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        // One preflight + one fetch for the FIRST call only. The URL-cache hit on
        // the second call short-circuits before the preflight, so no further
        // requests are issued.
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(200, "cached body"));
        let tool = WebFetchTool::new(ctx);
        let url = json!({ "url": "https://cache-hit.example/doc" });

        let first = tool
            .call(url.clone(), fresh_ctx(), fresh_tx())
            .await
            .expect("first fetch ok");
        assert_eq!(first.data["content"], "cached body");

        let second = tool
            .call(url, fresh_ctx(), fresh_tx())
            .await
            .expect("second fetch ok (from cache)");
        assert_eq!(second.data["content"], "cached body");
        assert_eq!(second.data["status"], 200);
        assert_eq!(second.data["bytes"], "cached body".len());
        assert_eq!(second.data["truncated"], false);

        // First call: preflight + fetch. Second call: cache hit, zero requests.
        assert_eq!(
            http.received_requests().len(),
            2,
            "second call must hit the URL cache, not the network"
        );
    }

    #[tokio::test]
    async fn cache_keyed_by_original_url_so_http_and_https_share() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(200, "body"));
        let tool = WebFetchTool::new(ctx);

        // First fetch under http:// — stored under the original (http) key.
        let _ = tool
            .call(
                json!({ "url": "http://key.example/p" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("first ok");
        // Re-fetching the same original http:// URL is a URL-cache hit.
        let _ = tool
            .call(
                json!({ "url": "http://key.example/p" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("cache hit");
        // First call: preflight + fetch. Second call: URL-cache hit, zero requests.
        assert_eq!(http.received_requests().len(), 2);
    }

    #[tokio::test]
    async fn distinct_urls_each_fetch() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        // Both URLs share the host `distinct.example`. The first call runs the
        // preflight (allowing + caching the host); the second call's preflight is
        // a domain-cache hit (no request), so it issues only its fetch.
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(200, "a"));
        http.enqueue(ok_response(200, "b"));
        let tool = WebFetchTool::new(ctx);
        let _ = tool
            .call(
                json!({ "url": "https://distinct.example/a" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("a ok");
        let _ = tool
            .call(
                json!({ "url": "https://distinct.example/b" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("b ok");
        // preflight (1) + two distinct URL fetches (2) = 3; the second preflight
        // is a domain-cache hit (no cross-URL content-cache hit).
        assert_eq!(http.received_requests().len(), 3);
    }

    #[tokio::test]
    async fn errors_are_not_cached() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        // Preflight allows (and caches the host); both fetches then 500. If 500s
        // were content-cached, the second call would skip its fetch.
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(500, "boom"));
        http.enqueue(ok_response(500, "boom"));
        let tool = WebFetchTool::new(ctx);
        let url = json!({ "url": "https://err.example/x" });
        let _ = tool.call(url.clone(), fresh_ctx(), fresh_tx()).await;
        let _ = tool.call(url, fresh_ctx(), fresh_tx()).await;
        // preflight (1, cached after) + two un-cached 500 fetches (2) = 3.
        assert_eq!(
            http.received_requests().len(),
            3,
            "failed fetches must NOT be cached"
        );
    }

    // ---- domain blocklist preflight (utils.ts:420-435) ---------------------

    #[tokio::test]
    async fn preflight_blocked_fails_with_domain_blocked_msg() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, sink) = make_web_ctx();
        ctx.bus.attach_sink(sink.clone()).await;
        // The very first request is the preflight; `can_fetch:false` blocks.
        http.enqueue(ok_response(200, r#"{"can_fetch":false}"#));
        let tool = WebFetchTool::new(ctx);
        let err = tool
            .call(
                json!({ "url": "https://blocked-host.example/page" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("blocked domain must be Err");
        match err {
            ToolError::Transport(msg) => {
                assert_eq!(msg, "Claude Code is unable to fetch from blocked-host.example");
            }
            other => panic!("expected Transport, got {other:?}"),
        }
        // No fetch was attempted — only the preflight ran.
        assert_eq!(http.received_requests().len(), 1);
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.iter().any(|n| n == "tengu_tool_web_fetch_failed"));
        assert!(!names.iter().any(|n| n == "tengu_tool_web_fetch_completed"));
    }

    #[tokio::test]
    async fn preflight_non_200_fails_with_check_failed_msg() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        // Preflight returns a non-200 (but no transport error) → check_failed.
        http.enqueue(ok_response(503, "service unavailable"));
        let tool = WebFetchTool::new(ctx);
        let err = tool
            .call(
                json!({ "url": "https://check-failed-503.example/page" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("non-200 preflight must be Err");
        match err {
            ToolError::Transport(msg) => {
                assert_eq!(
                    msg,
                    "Unable to verify if domain check-failed-503.example is safe to fetch. \
                     This may be due to network restrictions or enterprise security policies \
                     blocking claude.ai."
                );
            }
            other => panic!("expected Transport, got {other:?}"),
        }
        assert_eq!(http.received_requests().len(), 1);
    }

    #[tokio::test]
    async fn preflight_transport_error_fails_with_check_failed_msg() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        // Preflight itself errors at the transport layer → fail-open check_failed.
        http.enqueue(ScriptedResponse::SyncErr(HttpError::Connection(
            "egress proxy refused connection".into(),
        )));
        let tool = WebFetchTool::new(ctx);
        let err = tool
            .call(
                json!({ "url": "https://check-failed-net.example/page" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("preflight transport error must be Err");
        match err {
            ToolError::Transport(msg) => {
                assert_eq!(
                    msg,
                    "Unable to verify if domain check-failed-net.example is safe to fetch. \
                     This may be due to network restrictions or enterprise security policies \
                     blocking claude.ai."
                );
            }
            other => panic!("expected Transport, got {other:?}"),
        }
        // Only the preflight ran; the fetch was never attempted.
        assert_eq!(http.received_requests().len(), 1);
    }

    #[tokio::test]
    async fn preflight_caches_allowed_host_across_distinct_urls() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        // One preflight (allows + caches the host) + one fetch per distinct path.
        // The second path's preflight is a domain-cache hit — NO second domain_info.
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(200, "one"));
        http.enqueue(ok_response(200, "two"));
        let tool = WebFetchTool::new(ctx);
        let _ = tool
            .call(
                json!({ "url": "https://cached-host.example/one" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("one ok");
        let _ = tool
            .call(
                json!({ "url": "https://cached-host.example/two" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("two ok");
        let reqs = http.received_requests();
        // Exactly ONE domain_info request for the host (the second path reuses
        // the 5-min domain cache).
        let preflight_count = reqs
            .iter()
            .filter(|r| r.url.contains("/api/web/domain_info?domain="))
            .count();
        assert_eq!(preflight_count, 1, "host preflight must be cached for 5 min");
        // preflight (1) + two fetches (2) = 3 total.
        assert_eq!(reqs.len(), 3);
    }

    #[tokio::test]
    async fn skip_preflight_setting_issues_no_domain_info_request() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        std::env::set_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT", "1");
        // Sanity-check the helper sees the truthy value.
        assert!(skip_web_fetch_preflight());

        let (ctx, http, _sink) = make_web_ctx();
        // ONLY the fetch is enqueued — no preflight response. If the preflight
        // fired, it would consume this and the body assertion would fail.
        http.enqueue(ok_response(200, "no preflight here"));
        let tool = WebFetchTool::new(ctx);
        let res = tool
            .call(
                json!({ "url": "https://skip-preflight.example/page" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await;
        std::env::remove_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT");

        let res = res.expect("fetch must proceed when preflight skipped");
        assert_eq!(res.data["content"], "no preflight here");
        let reqs = http.received_requests();
        // Exactly one request — the fetch — and NO domain_info preflight.
        assert_eq!(reqs.len(), 1);
        assert!(!reqs[0].url.contains("/api/web/domain_info"));
        assert_eq!(reqs[0].url, "https://skip-preflight.example/page");
    }

    #[cfg(feature = "web-markdown")]
    mod markdown_apply {
        use super::*;
        use sidequery::{SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse};

        pub(super) struct CapturingSideQuery {
            pub(super) captured: std::sync::Mutex<Option<String>>,
            pub(super) reply: String,
        }
        #[async_trait]
        impl SideQueryClient for CapturingSideQuery {
            async fn query(
                &self,
                request: SideQueryRequest,
            ) -> Result<SideQueryResponse, SideQueryError> {
                // `text_content()` concatenates the message's Text blocks (protocol).
                let user_text = request.messages.last().map(protocol::ConversationMessage::text_content);
                *self.captured.lock().unwrap() = user_text;
                Ok(SideQueryResponse {
                    text: Some(self.reply.clone()),
                    structured: None,
                    tool_calls: vec![],
                    usage: cost::Usage::default(),
                    stop_reason: Some("end_turn".into()),
                })
            }
        }
    }

    #[cfg(feature = "web-markdown")]
    #[tokio::test]
    async fn apply_step_runs_model_over_markdown() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(preflight_allow());
        http.enqueue(ScriptedResponse::Sync(protocol::HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            body: "<h1>Title</h1><p>Body text</p>".into(),
        }));
        let capture = std::sync::Arc::new(markdown_apply::CapturingSideQuery {
            captured: std::sync::Mutex::new(None),
            reply: "MODEL SUMMARY".into(),
        });
        let tool = WebFetchTool::new(ctx).with_side_query(capture.clone());
        let res = tool
            .call(
                json!({ "url": "https://apply.example/x", "prompt": "summarize" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(res.data["content"], "MODEL SUMMARY");
        let seen = capture.captured.lock().unwrap().clone().unwrap();
        assert!(seen.contains("# Title"), "model prompt should carry markdown: {seen}");
        assert!(seen.contains("summarize"));
        assert!(!seen.contains("<h1>"), "HTML must be converted, not raw");
    }

    #[cfg(feature = "web-markdown")]
    #[tokio::test]
    async fn no_side_query_returns_markdown_unchanged_behavior() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(preflight_allow());
        http.enqueue(ScriptedResponse::Sync(protocol::HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            body: "<h1>Hi</h1>".into(),
        }));
        let tool = WebFetchTool::new(ctx);
        let res = tool
            .call(
                json!({ "url": "https://nomarkdown.example/x", "prompt": "q" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(res.data["content"], "# Hi");
    }

    #[test]
    fn with_side_query_sets_the_client() {
        let (ctx, _http, _sink) = make_web_ctx();
        let tool = WebFetchTool::new(ctx);
        assert!(tool.side_query.is_none(), "default has no side-query client");
    }

    #[test]
    fn skip_preflight_env_parsing() {
        // Exercised under the env lock so it never races a parallel call() test.
        let _env = SKIP_ENV_LOCK.blocking_lock();
        for truthy in ["1", "true", "TRUE", "Yes", "on", " on "] {
            std::env::set_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT", truthy);
            assert!(skip_web_fetch_preflight(), "{truthy:?} must be truthy");
        }
        for falsy in ["0", "false", "no", "off", "", "garbage"] {
            std::env::set_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT", falsy);
            assert!(!skip_web_fetch_preflight(), "{falsy:?} must be falsy");
        }
        std::env::remove_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT");
        assert!(!skip_web_fetch_preflight(), "unset must be falsy");
    }
}
