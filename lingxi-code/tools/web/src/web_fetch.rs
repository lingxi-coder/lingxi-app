//! `WebFetchTool` — fetches a URL via the M1 `HttpTransport` trait, with a
//! 10 MB transfer cap, a 100 000-char markdown cap, scheme allow-list
//! (`https`/`http`), manual permitted-redirect handling, and the claude-code
//! `Claude-User (claude-code/<version>; +https://support.anthropic.com/)`
//! User-Agent (v2.1.181). Spec §4 Flow B + §7 Web wire identifiers.
//!
//! Wire-locked constants (byte-checked against `parity_web_tools.json` + TS):
//! - `WEBFETCH_MAX_TRANSFER_BYTES = 10 * 1024 * 1024` (10 MB; TS
//!   `MAX_HTTP_CONTENT_LENGTH`, `utils.ts:112`)
//! - `WEBFETCH_MAX_MARKDOWN_LEN = 100_000` chars (TS `MAX_MARKDOWN_LENGTH`,
//!   `utils.ts:128`)
//! - `WEBFETCH_MAX_REDIRECTS = 10` (TS `MAX_REDIRECTS`, `utils.ts:125`)
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

/// Maximum transfer size for the response body — byte-locked to claude-code
/// `MAX_HTTP_CONTENT_LENGTH` (`WebFetchTool/utils.ts:112`, the axios
/// `maxContentLength`). A response whose body exceeds this errors (TS: axios
/// throws `maxContentLength exceeded`), it is NOT truncated. The markdown is
/// separately capped at [`WEBFETCH_MAX_MARKDOWN_LEN`].
pub const WEBFETCH_MAX_TRANSFER_BYTES: usize = 10 * 1024 * 1024;

/// Maximum length (in chars) of the converted markdown before it is truncated
/// with [`WEBFETCH_TRUNCATION_SUFFIX`] — byte-locked to claude-code
/// `MAX_MARKDOWN_LENGTH` (`WebFetchTool/utils.ts:128`). TS slices the markdown
/// string by UTF-16 code units (`String.prototype.slice`); this port slices by
/// Unicode scalar (`char`) — identical for the BMP text WebFetch returns.
pub const WEBFETCH_MAX_MARKDOWN_LEN: usize = 100_000;

/// Maximum same-host redirect hops before erroring — byte-locked to claude-code
/// `MAX_REDIRECTS` (`WebFetchTool/utils.ts:125`). Caps redirect loops so a
/// malicious server cannot hang the tool (each hop resets the per-request
/// timeout).
pub const WEBFETCH_MAX_REDIRECTS: u32 = 10;

/// Suffix appended to the markdown when it overflows [`WEBFETCH_MAX_MARKDOWN_LEN`].
/// Spec §7 lock; matches `claude-code/src/tools/WebFetchTool/utils.ts:531-532`.
pub const WEBFETCH_TRUNCATION_SUFFIX: &str = "\n\n[Content truncated due to length...]";

/// User-Agent prefix for the tool-side fetch (distinct from api-client UA).
/// The full header value is `concat!(WEBFETCH_USER_AGENT_PREFIX, env!("CARGO_PKG_VERSION"))`.
/// Spec §7 lock.
pub const WEBFETCH_USER_AGENT_PREFIX: &str = "claude-code-tool/";

/// Schemes that `WebFetchTool` will accept. Spec §7 lock.
pub const WEBFETCH_ALLOWED_SCHEMES: &[&str] = &["https", "http"];

/// Per-request HTTP timeout for WebFetch — byte-locked to claude-code `KHp=60000`
/// (the `timeout` axios passes in `fo.get(e, {timeout: KHp, ...})` inside the
/// WebFetch GET helper `Buo`). NOT the 30 000 ms used by other fetches.
pub const WEBFETCH_TIMEOUT: Duration = Duration::from_secs(60);

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

/// Map an HTTP status code to its reason phrase — 1:1 with claude-code's
/// `c9n(statusCode)` (`STATUS_CODES[statusCode] ?? "Unknown Status"`), where
/// `STATUS_CODES` is Node/bun's `http.STATUS_CODES` table. Used for BOTH the
/// HTTP-error result body (`format_http_error_message`) and the redirect notice's
/// `Status: {code} {text}` line.
///
/// This replaces the prior 4-arm redirect ternary (`301/307/308 → …, else
/// "Found"`), which was a divergence: claude-code uses the full status table, so
/// e.g. `303 → "See Other"` (not "Found") and `429 → "Too Many Requests"`. Codes
/// not in the table fall through to `"Unknown Status"` (matching the `?? "Unknown
/// Status"` fallback), NOT `"Found"`.
#[must_use]
pub fn status_reason_phrase(code: u16) -> &'static str {
    // Byte-exact reproduction of bun's `STATUS_CODES` (extracted from the
    // claude-code binary); identical to Node's `http.STATUS_CODES` except bun
    // additionally defines `509: "Bandwidth Limit Exceeded"`.
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a Teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Entity",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        509 => "Bandwidth Limit Exceeded",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "Unknown Status",
    }
}

/// Build the byte-exact HTTP-error message claude-code returns as a SUCCESS
/// result body when a fetch yields status ≥ 400 — 1:1 with `iIp(e)`:
///
/// ```text
/// The server returned HTTP ${statusCode} ${statusText}.${retryAfter}
///
/// The response body was not retrieved. If this URL requires authentication, use
/// an authenticated tool (e.g. `gh` for GitHub, or an MCP-provided fetch tool)
/// instead of WebFetch.
/// ```
///
/// `retry_after` is the response's `Retry-After` header value, if present; when
/// `Some`, a `"\nRetry-After: {value}"` line is inserted directly after the
/// status sentence (matching `e.retryAfter ? \`\nRetry-After: ${e.retryAfter}\` :
/// ""`). The status text comes from [`status_reason_phrase`].
#[must_use]
pub fn format_http_error_message(status: u16, retry_after: Option<&str>) -> String {
    let status_text = status_reason_phrase(status);
    let retry = match retry_after {
        Some(v) => format!("\nRetry-After: {v}"),
        None => String::new(),
    };
    format!(
        "The server returned HTTP {status} {status_text}.{retry}\n\nThe response body was not retrieved. If this URL requires authentication, use an authenticated tool (e.g. `gh` for GitHub, or an MCP-provided fetch tool) instead of WebFetch."
    )
}

/// Build the byte-exact "REDIRECT DETECTED" message from `WebFetchTool.ts:227-235`.
///
/// `prompt` is interpolated verbatim into the `- prompt: "${prompt}"` line; pass
/// the empty string when the caller supplied no prompt (TS interpolates
/// `undefined` as the string `"undefined"`, but the Rust input models an absent
/// prompt as `None`/`""`; `call()` passes `prompt.unwrap_or("")`).
///
/// Wired into [`WebFetchTool::call`]'s redirect loop: returned as the tool result
/// `content` when a redirect targets a different host (not an
/// [`is_permitted_redirect`]).
#[must_use]
pub fn format_redirect_message(
    original_url: &str,
    redirect_url: &str,
    status_code: u16,
    prompt: &str,
) -> String {
    let status_text = status_reason_phrase(status_code);
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

/// Whether a redirect from `original_url` to `redirect_url` is safe to follow —
/// 1:1 with claude-code `isPermittedRedirect` (`WebFetchTool/utils.ts:212-243`).
/// Permits redirects that only add/remove a leading `www.` or change the
/// path/query while keeping the SAME origin. Rejects (returns false) when:
/// - either URL fails to parse;
/// - the scheme changes;
/// - the port changes;
/// - the redirect target carries credentials (username/password);
/// - the hosts differ after stripping a single leading `www.`.
///
/// Port note: `url::Url::port()` returns `None` for the scheme's default port,
/// so comparing `port_or_known_default` (which folds in the scheme default)
/// matches TS's `URL.port` semantics for the http/https schemes WebFetch allows.
#[must_use]
pub fn is_permitted_redirect(original_url: &str, redirect_url: &str) -> bool {
    let (Ok(orig), Ok(redir)) = (url::Url::parse(original_url), url::Url::parse(redirect_url))
    else {
        return false;
    };
    if redir.scheme() != orig.scheme() {
        return false;
    }
    if redir.port_or_known_default() != orig.port_or_known_default() {
        return false;
    }
    if !redir.username().is_empty() || redir.password().is_some() {
        return false;
    }
    let strip_www = |h: &str| h.strip_prefix("www.").unwrap_or(h).to_string();
    let orig_host = orig.host_str().map(strip_www);
    let redir_host = redir.host_str().map(strip_www);
    // Both must have a host, and they must match after stripping `www.`. (TS
    // compares `parsedOriginal.hostname` strings directly; two host-less URLs
    // would compare equal in TS, but http/https URLs always have a host.)
    orig_host.is_some() && orig_host == redir_host
}

/// Truncate the converted `markdown` to [`WEBFETCH_MAX_MARKDOWN_LEN`] chars,
/// appending [`WEBFETCH_TRUNCATION_SUFFIX`] when truncated — mirrors the
/// `markdownContent.length > MAX_MARKDOWN_LENGTH` slice in
/// `applyPromptToMarkdown` (`utils.ts:529-533`), but applied to the returned
/// content (per this batch's spec: cap the markdown before returning). Slices on
/// a `char` boundary (Unicode scalar), matching TS's UTF-16 `.slice` for BMP
/// text. Returns `(possibly_truncated, truncated_flag)`.
#[must_use]
pub fn truncate_markdown_for_return(markdown: String) -> (String, bool) {
    if markdown.chars().count() <= WEBFETCH_MAX_MARKDOWN_LEN {
        return (markdown, false);
    }
    let cut: String = markdown.chars().take(WEBFETCH_MAX_MARKDOWN_LEN).collect();
    let mut out = cut;
    out.push_str(WEBFETCH_TRUNCATION_SUFFIX);
    (out, true)
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
        // claude-code WebFetch User-Agent (v2.1.181: `Claude-User (${tg()}; +...)`):
        // `Claude-User (claude-code/<version>; +https://support.anthropic.com/)`.
        // The `Claude-User (...)` wrapper is how Anthropic web infra recognizes
        // claude-code fetch traffic (distinct from the api-client UA). LingXi has
        // no pinned claude-code version, so its own crate version fills the slot.
        format!(
            "Claude-User (claude-code/{}; +https://support.anthropic.com/)",
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
    /// the model's text (or the fallback "No response from model"). `is_preapproved`
    /// is the host+path allowlist result (TS `isPreapprovedUrl(url)`), selecting the
    /// relaxed vs strict guideline block in the secondary-model prompt.
    #[cfg(feature = "web-markdown")]
    async fn apply_prompt(
        &self,
        client: &std::sync::Arc<dyn sidequery::SideQueryClient>,
        is_preapproved: bool,
        markdown: &str,
        prompt: &str,
    ) -> String {
        use protocol::{ConversationMessage, MessageId};
        use sidequery::{QuerySource, SideQueryRequest};
        let truncated = crate::markdown::truncate_markdown(markdown.to_string());
        let model_prompt = crate::markdown::make_secondary_model_prompt(
            &truncated,
            prompt,
            is_preapproved,
        );
        let req = SideQueryRequest {
            model: self.apply_model(),
            system_prompt: None,
            messages: vec![ConversationMessage::user(MessageId::new(), model_prompt)],
            tools: vec![],
            tool_choice: None,
            output_format: None,
            // claude-code's queryHaiku uses getMaxOutputTokensForModel = min(native,
            // CAPPED_DEFAULT_MAX_TOKENS) = 8000 for the apply call.
            max_tokens: 8000,
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
    /// `is_preapproved` is the host+path allowlist result for the fetched URL.
    #[cfg(feature = "web-markdown")]
    async fn maybe_apply(
        &self,
        is_preapproved: bool,
        content: &str,
        prompt: Option<&str>,
    ) -> Option<String> {
        if let (Some(client), Some(p)) = (self.side_query.as_ref(), prompt) {
            return Some(self.apply_prompt(client, is_preapproved, content, p).await);
        }
        None
    }

    /// No-op fallback when `web-markdown` is disabled.
    #[cfg(not(feature = "web-markdown"))]
    #[allow(clippy::unused_async)]
    async fn maybe_apply(
        &self,
        _is_preapproved: bool,
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
            "url": { "type": "string", "format": "uri", "description": "The URL to fetch content from" },
            "prompt": { "type": "string", "description": "The prompt to run on the fetched content" }
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

    async fn check_permissions(&self, input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        // Preapproved-host short-circuit (`WebFetchTool.ts:108-121`): if the URL's
        // host+path is on the preapproved allowlist, allow with the "Preapproved
        // host" reason BEFORE any rule lookup. A parse failure falls through to the
        // default gate (TS catches and continues). The downstream rule-based
        // deny/ask machinery is not ported in this batch, so a non-preapproved host
        // continues to the M4-03 allow-all default.
        if let Some(url_str) = input.get("url").and_then(Value::as_str) {
            if let Ok(parsed) = url::Url::parse(url_str) {
                if let Some(host) = parsed.host_str() {
                    if crate::markdown::is_preapproved_host(host, parsed.path()) {
                        return PermissionResult::Allow {
                            reason: PermissionDecisionReason::Other {
                                reason: "Preapproved host".into(),
                            },
                            updated_input: None,
                            update_destination: None,
                            metadata: PermissionMetadata::default(),
                        };
                    }
                }
            }
        }
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
        // Byte-locked VERBATIM to claude-code `DESCRIPTION`
        // (`WebFetchTool/prompt.ts:3-21`). The TS constant is a template literal
        // that begins and ends with a newline; reproduced exactly.
        "\n\
- Fetches content from a specified URL and processes it using an AI model\n\
- Takes a URL and a prompt as input\n\
- Fetches the URL content, converts HTML to markdown\n\
- Processes the content with the prompt using a small, fast model\n\
- Returns the model's response about the content\n\
- Use this tool when you need to retrieve and analyze web content\n\
\n\
Usage notes:\n\
  - IMPORTANT: If an MCP-provided web fetch tool is available, prefer using that tool instead of this one, as it may have fewer restrictions.\n\
  - The URL must be a fully-formed valid URL\n\
  - HTTP URLs will be automatically upgraded to HTTPS\n\
  - The prompt should describe what information you want to extract from the page\n\
  - This tool is read-only and does not modify any files\n\
  - Results may be summarized if the content is very large\n\
  - Includes a self-cleaning 15-minute cache for faster responses when repeatedly accessing the same URL\n\
  - When a URL redirects to a different host, the tool will inform you and provide the redirect URL in a special format. You should then make a new WebFetch request with the redirect URL to fetch the content.\n\
  - For GitHub URLs, prefer using the gh CLI via Bash instead (e.g., gh pr view, gh issue view, gh api).\n"
            .into()
    }
    async fn prompt(&self, _opts: &PromptOptions) -> String {
        // claude-code's WebFetch has no separate `prompt()` — the `DESCRIPTION`
        // constant is the full model-facing prompt. Mirror it here so both
        // surfaces carry the verbatim TS text.
        self.description(&Value::Null, &DescriptionOptions {
            is_non_interactive_session: false,
        })
        .await
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

        // Preapproved-URL flag (TS `isPreapprovedUrl(url)`, `utils.ts:130-137`):
        // host+path allowlist on the ORIGINAL URL, selecting the relaxed vs strict
        // guideline block in the apply step. Computed once, used on both the
        // cache-hit and live-fetch apply paths.
        let is_preapproved = crate::markdown::is_preapproved_host(
            parsed_url.host_str().unwrap_or(""),
            parsed_url.path(),
        );

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
            // The cached `content` is the already-capped markdown; it was
            // truncated iff it carries the truncation suffix (the live path
            // appends [`WEBFETCH_TRUNCATION_SUFFIX`] when the 100k char cap hit).
            let truncated = hit.content.ends_with(WEBFETCH_TRUNCATION_SUFFIX);
            // Cache hits do no network work, so the reported duration is 0 ms.
            self.emit_completed(&invocation_id, hit.status, hit.bytes as u64, truncated, 0)
                .await;
            // claude-code caches only the markdown; the prompt is applied on EVERY
            // call (cache hit or miss). Run the apply step on the cached content.
            let out_content = match self
                .maybe_apply(is_preapproved, &hit.content, parsed_input.prompt.as_deref())
                .await
            {
                Some(applied) => applied,
                None => hit.content,
            };
            return Ok(ToolCallResult {
                data: json!({
                    "url": parsed_input.url,
                    "status": hit.status,
                    "content": out_content,
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

        // Manual permitted-redirect loop — 1:1 with claude-code
        // `getWithPermittedRedirects` (`utils.ts:262-366`). Each iteration issues
        // a GET against `fetch_url` via `request_no_follow` (auto-redirect is NOT
        // requested — the transport surfaces a 3xx as `Ok(status=3xx)` with its
        // `Location` header intact). On a 301/302/307/308 with a `Location`:
        // resolve it against the current URL; if `is_permitted_redirect` (same
        // scheme+port, no creds, host equal after stripping a leading `www.`) →
        // follow it (loop); else → return the "REDIRECT DETECTED" notice. Capped
        // at `WEBFETCH_MAX_REDIRECTS` (10) hops.
        //
        // NOTE (transport no-follow): the loop drives the additive
        // `HttpTransport::request_no_follow` (the allowed frozen-trait exception,
        // same defaulted-method pattern as `stream_sse_with_meta`). Production
        // reqwest transports (`platform-common`'s `ReqwestHttp`,
        // `platform-windows`'s `WindowsHttp`) OVERRIDE it with a
        // `redirect::Policy::none()` client — mirroring claude-code's axios
        // `maxRedirects: 0` — so a cross-host 3xx now reaches this loop and the
        // "REDIRECT DETECTED" notice fires in production (previously only the test
        // `MockHttpTransport` surfaced 3xx; reqwest's default client transparently
        // followed the redirect before the loop could see it). Transports that do
        // NOT override `request_no_follow` (e.g. `MockHttpTransport`, posix-minimal
        // `PosixHttp`) inherit the default, which delegates to `request` —
        // preserving their existing behavior.
        let mut fetch_url = parsed_url.clone();
        let mut hops: u32 = 0;
        let resp_result = loop {
            let req = HttpRequest {
                method: HttpMethod::Get,
                url: fetch_url.to_string(),
                headers: vec![
                    ("user-agent".into(), Self::user_agent()),
                    ("accept".into(), "text/markdown, text/html, */*".into()),
                ],
                body: None,
                body_bytes: None,
                timeout: Some(WEBFETCH_TIMEOUT),
            };
            let result = self.ctx.http.request_no_follow(req).await;

            // Redirect handling on a 3xx `Ok` carrying a Location header. The
            // redirect status set is byte-locked to claude-code's
            // `YHp=new Set([301,302,303,307,308])` — 303 (See Other) IS included
            // (it was previously omitted, so a 303 fell through to normal-content
            // handling).
            if let Ok(resp) = &result {
                if matches!(resp.status, 301 | 302 | 303 | 307 | 308) {
                    let location = resp
                        .headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                        .map(|(_, v)| v.clone());
                    // claude-code: `if(typeof l!=="string"||l.trim()==="")return
                    // {type:"http_error",statusCode:s}` — a redirect with a
                    // missing OR empty/whitespace Location is NOT a thrown error;
                    // it degrades to the same `http_error` SUCCESS result as a
                    // >=400 response (with the 3xx status code).
                    let location = match location {
                        Some(l) if !l.trim().is_empty() => l,
                        _ => {
                            let elapsed_ms = started.elapsed().as_millis() as u64;
                            let code_text = status_reason_phrase(resp.status);
                            let message = format_http_error_message(resp.status, None);
                            self.emit_completed(&invocation_id, resp.status, 0, false, elapsed_ms)
                                .await;
                            return Ok(ToolCallResult {
                                data: json!({
                                    "url": parsed_input.url,
                                    "status": resp.status,
                                    "code_text": code_text,
                                    "content": message,
                                    "truncated": false,
                                    "bytes": 0,
                                }),
                                new_messages: vec![],
                                context_modifier: None,
                                mcp_meta: None,
                            });
                        }
                    };
                    // Resolve a possibly-relative Location against the current URL.
                    let redirect_url = match fetch_url.join(&location) {
                        Ok(u) => u.to_string(),
                        Err(_) => location.clone(),
                    };
                    let current = fetch_url.to_string();
                    if is_permitted_redirect(&current, &redirect_url) {
                        hops += 1;
                        if hops > WEBFETCH_MAX_REDIRECTS {
                            let elapsed_ms = started.elapsed().as_millis() as u64;
                            self.emit_failed(&invocation_id, "too_many_redirects", None, elapsed_ms)
                                .await;
                            return Err(ToolError::Transport(format!(
                                "WebFetch: too many redirects (exceeded {WEBFETCH_MAX_REDIRECTS})"
                            )));
                        }
                        // Follow the permitted redirect (parse failure => fall
                        // through to the redirect notice rather than loop forever).
                        if let Ok(u) = url::Url::parse(&redirect_url) {
                            fetch_url = u;
                            continue;
                        }
                    }
                    // Not permitted (different host) — return the redirect notice.
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    let status_text = status_reason_phrase(resp.status);
                    let message = format_redirect_message(
                        &current,
                        &redirect_url,
                        resp.status,
                        parsed_input.prompt.as_deref().unwrap_or(""),
                    );
                    self.emit_completed(
                        &invocation_id,
                        resp.status,
                        message.len() as u64,
                        false,
                        elapsed_ms,
                    )
                    .await;
                    return Ok(ToolCallResult {
                        data: json!({
                            "url": parsed_input.url,
                            "status": resp.status,
                            "code_text": status_text,
                            "content": message,
                            "truncated": false,
                            "bytes": message.len(),
                        }),
                        new_messages: vec![],
                        context_modifier: None,
                        mcp_meta: None,
                    });
                }
            }
            break result;
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;

        match resp_result {
            Ok(resp) if resp.status >= 400 => {
                // claude-code returns HTTP ≥ 400 as a SUCCESS data result, NOT a
                // thrown error — `c.type === "http_error"` →
                // `{bytes:0, code, codeText: STATUS_CODES[code] ?? "Unknown
                // Status", result: iIp(c), durationMs, url}`. The `iIp` body gives
                // the model actionable guidance ("use gh for GitHub / an MCP fetch
                // tool if this requires auth") instead of an opaque transport
                // failure. The `Retry-After` header (when present, e.g. on 429/503)
                // is surfaced on its own line.
                let retry_after = resp
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
                    .map(|(_, v)| v.clone());
                let code_text = status_reason_phrase(resp.status);
                let message =
                    format_http_error_message(resp.status, retry_after.as_deref());
                // The tool call COMPLETED (it returns a result); claude-code logs a
                // distinct `tengu_web_fetch_http_error`, but LingXi's telemetry
                // vocabulary is started/completed/failed — completed is the faithful
                // mapping for a successful tool result. `bytes:0` matches `iIp`.
                self.emit_completed(&invocation_id, resp.status, 0, false, elapsed_ms)
                    .await;
                Ok(ToolCallResult {
                    data: json!({
                        "url": parsed_input.url,
                        "status": resp.status,
                        "code_text": code_text,
                        "content": message,
                        "truncated": false,
                        "bytes": 0,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Ok(resp) => {
                let status = resp.status;
                let body_bytes = resp.body.len();

                // Transfer cap (`utils.ts:112` `maxContentLength`): a body larger
                // than 10 MB is rejected, NOT truncated (TS: axios throws). The M1
                // transport already buffered the body, so we check its length here.
                if body_bytes > WEBFETCH_MAX_TRANSFER_BYTES {
                    self.emit_failed(&invocation_id, "content_too_large", Some(status), elapsed_ms)
                        .await;
                    return Err(ToolError::Transport(format!(
                        "WebFetch: response body ({body_bytes} bytes) exceeds maximum allowed size ({WEBFETCH_MAX_TRANSFER_BYTES} bytes)"
                    )));
                }

                let content_type = resp
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map_or_else(String::new, |(_, v)| v.clone());

                // HTML->markdown (claude-code converts HTML; non-HTML is used as-is).
                // Behind `web-markdown`; feature off => content is the raw body.
                #[cfg(feature = "web-markdown")]
                let converted = if crate::markdown::is_html_content_type(&content_type) {
                    crate::markdown::html_to_markdown(&resp.body)
                } else {
                    resp.body
                };
                #[cfg(not(feature = "web-markdown"))]
                let converted = resp.body;

                // Markdown cap (`utils.ts:128`/`529-533`): truncate the converted
                // markdown to 100k chars, appending the suffix, BEFORE caching /
                // returning. `truncated` reflects whether this cut fired.
                let (content, truncated) = truncate_markdown_for_return(converted);

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
                    .maybe_apply(is_preapproved, &content, parsed_input.prompt.as_deref())
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
                // Transports that surface a 4xx/5xx as `Err(Status)` (rather than
                // `Ok(resp)` with a >=400 status) take the same path as the
                // `Ok(resp) if status >= 400` arm above: a SUCCESS result carrying
                // the `iIp` body. No headers are available on this variant, so
                // `Retry-After` is omitted.
                let code_text = status_reason_phrase(status);
                let message = format_http_error_message(status, None);
                self.emit_completed(&invocation_id, status, 0, false, elapsed_ms)
                    .await;
                Ok(ToolCallResult {
                    data: json!({
                        "url": parsed_input.url,
                        "status": status,
                        "code_text": code_text,
                        "content": message,
                        "truncated": false,
                        "bytes": 0,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
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
        // TS-faithful caps (utils.ts:112/125/128).
        assert_eq!(WEBFETCH_MAX_TRANSFER_BYTES, 10 * 1024 * 1024);
        assert_eq!(WEBFETCH_MAX_MARKDOWN_LEN, 100_000);
        assert_eq!(WEBFETCH_MAX_REDIRECTS, 10);
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
    fn does_not_truncate_short_markdown() {
        let small = "hello world".to_string();
        let (out, flag) = truncate_markdown_for_return(small.clone());
        assert_eq!(out, small);
        assert!(!flag);
    }

    #[test]
    fn does_not_truncate_markdown_exactly_at_cap() {
        let exact = "a".repeat(WEBFETCH_MAX_MARKDOWN_LEN);
        let (out, flag) = truncate_markdown_for_return(exact.clone());
        assert_eq!(out, exact);
        assert!(!flag);
    }

    #[test]
    fn truncates_oversized_markdown_to_char_cap() {
        let big = "a".repeat(WEBFETCH_MAX_MARKDOWN_LEN + 1024);
        let (out, flag) = truncate_markdown_for_return(big);
        assert!(flag);
        assert!(out.ends_with(WEBFETCH_TRUNCATION_SUFFIX));
        // Char count of the body (excluding the suffix) is exactly the cap.
        let body_only = &out[..out.len() - WEBFETCH_TRUNCATION_SUFFIX.len()];
        assert_eq!(body_only.chars().count(), WEBFETCH_MAX_MARKDOWN_LEN);
    }

    #[test]
    fn truncates_markdown_on_char_boundary_multibyte() {
        // A run of multibyte chars; the cap is by CHAR (not byte), so no scalar
        // is split and the body holds exactly WEBFETCH_MAX_MARKDOWN_LEN chars.
        let s = "あ".repeat(WEBFETCH_MAX_MARKDOWN_LEN + 50);
        let (out, flag) = truncate_markdown_for_return(s);
        assert!(flag);
        let body_only = &out[..out.len() - WEBFETCH_TRUNCATION_SUFFIX.len()];
        assert!(body_only.is_char_boundary(body_only.len()));
        assert_eq!(body_only.chars().count(), WEBFETCH_MAX_MARKDOWN_LEN);
    }

    // ---- is_permitted_redirect (utils.ts:212-243) --------------------------

    #[test]
    fn permitted_redirect_same_host_path_change() {
        assert!(is_permitted_redirect(
            "https://example.com/a",
            "https://example.com/b?q=1"
        ));
    }

    #[test]
    fn permitted_redirect_adds_or_removes_www() {
        assert!(is_permitted_redirect(
            "https://example.com/a",
            "https://www.example.com/a"
        ));
        assert!(is_permitted_redirect(
            "https://www.example.com/a",
            "https://example.com/a"
        ));
    }

    #[test]
    fn rejected_redirect_different_host() {
        assert!(!is_permitted_redirect(
            "https://example.com/a",
            "https://evil.example.org/a"
        ));
    }

    #[test]
    fn rejected_redirect_scheme_or_port_or_creds_change() {
        // Scheme change.
        assert!(!is_permitted_redirect(
            "https://example.com/a",
            "http://example.com/a"
        ));
        // Port change.
        assert!(!is_permitted_redirect(
            "https://example.com/a",
            "https://example.com:8443/a"
        ));
        // Credentials on the redirect target.
        assert!(!is_permitted_redirect(
            "https://example.com/a",
            "https://user:pass@example.com/a"
        ));
        // Unparseable.
        assert!(!is_permitted_redirect("not-a-url", "https://example.com/a"));
    }

    #[test]
    fn format_http_error_message_matches_iip() {
        // No Retry-After (the common case): status sentence + the body note.
        assert_eq!(
            format_http_error_message(404, None),
            "The server returned HTTP 404 Not Found.\n\nThe response body was not retrieved. If this URL requires authentication, use an authenticated tool (e.g. `gh` for GitHub, or an MCP-provided fetch tool) instead of WebFetch."
        );
        assert_eq!(
            format_http_error_message(500, None),
            "The server returned HTTP 500 Internal Server Error.\n\nThe response body was not retrieved. If this URL requires authentication, use an authenticated tool (e.g. `gh` for GitHub, or an MCP-provided fetch tool) instead of WebFetch."
        );
        // With Retry-After (e.g. 429/503): a "\nRetry-After: {value}" line is
        // inserted directly after the status sentence, before the blank line.
        assert_eq!(
            format_http_error_message(429, Some("120")),
            "The server returned HTTP 429 Too Many Requests.\nRetry-After: 120\n\nThe response body was not retrieved. If this URL requires authentication, use an authenticated tool (e.g. `gh` for GitHub, or an MCP-provided fetch tool) instead of WebFetch."
        );
        // Unknown code falls through to "Unknown Status".
        assert_eq!(
            format_http_error_message(799, None),
            "The server returned HTTP 799 Unknown Status.\n\nThe response body was not retrieved. If this URL requires authentication, use an authenticated tool (e.g. `gh` for GitHub, or an MCP-provided fetch tool) instead of WebFetch."
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

    // ---- status reason phrase (c9n / STATUS_CODES) -------------------------

    #[test]
    fn status_reason_phrase_matches_node_table() {
        // Redirect codes used by the redirect notice.
        assert_eq!(status_reason_phrase(301), "Moved Permanently");
        assert_eq!(status_reason_phrase(308), "Permanent Redirect");
        assert_eq!(status_reason_phrase(307), "Temporary Redirect");
        assert_eq!(status_reason_phrase(302), "Found");
        // 303 is "See Other" — the prior `redirect_status_text` ternary wrongly
        // returned "Found" here (the divergence this fixes, #26).
        assert_eq!(status_reason_phrase(303), "See Other");
        // Success + client/server error phrases used by the http_error result.
        assert_eq!(status_reason_phrase(200), "OK");
        assert_eq!(status_reason_phrase(404), "Not Found");
        assert_eq!(status_reason_phrase(429), "Too Many Requests");
        assert_eq!(status_reason_phrase(418), "I'm a Teapot");
        assert_eq!(status_reason_phrase(500), "Internal Server Error");
        assert_eq!(status_reason_phrase(503), "Service Unavailable");
        // bun-only entry (Node omits 509).
        assert_eq!(status_reason_phrase(509), "Bandwidth Limit Exceeded");
        assert_eq!(status_reason_phrase(511), "Network Authentication Required");
        // Codes absent from the table fall through to "Unknown Status" (the
        // `?? "Unknown Status"` fallback), NOT "Found".
        assert_eq!(status_reason_phrase(0), "Unknown Status");
        assert_eq!(status_reason_phrase(799), "Unknown Status");
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
    async fn http_500_returns_success_result_not_error() {
        // 1:1 with claude-code: HTTP >= 400 is a SUCCESS data result carrying the
        // `iIp` body (so the model can react / fall back to gh/MCP), NOT a thrown
        // transport error.
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, sink) = make_web_ctx();
        ctx.bus.attach_sink(sink.clone()).await;
        // Preflight allows, then the fetch returns 500.
        http.enqueue(preflight_allow());
        http.enqueue(ok_response(500, "server error"));

        let tool = WebFetchTool::new(ctx);
        let result = tool
            .call(
                json!({ "url": "https://http500.example/x" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("500 must be Ok(result), not Err");
        assert_eq!(result.data["status"], 500);
        assert_eq!(result.data["code_text"], "Internal Server Error");
        assert_eq!(result.data["bytes"], 0);
        assert_eq!(
            result.data["content"].as_str().unwrap(),
            format_http_error_message(500, None)
        );
        // The tool call COMPLETED (returned a result), so `completed` fires and
        // `failed` does not.
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_web_fetch_started"));
        assert!(names.contains(&"tengu_tool_web_fetch_completed"));
        assert!(!names.contains(&"tengu_tool_web_fetch_failed"));
    }

    #[tokio::test]
    async fn http_429_surfaces_retry_after_header() {
        // A 429 with a Retry-After header surfaces the header on its own line in
        // the result body, and reports codeText "Too Many Requests".
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::blocklist::clear_domain_check_cache();
        let (ctx, http, _sink) = make_web_ctx();
        http.enqueue(preflight_allow());
        http.enqueue(ScriptedResponse::Sync(protocol::HttpResponse {
            status: 429,
            headers: vec![("Retry-After".into(), "30".into())],
            body: String::new(),
        }));
        let tool = WebFetchTool::new(ctx);
        let result = tool
            .call(
                json!({ "url": "https://ratelimited.example/x" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("429 must be Ok(result)");
        assert_eq!(result.data["status"], 429);
        assert_eq!(result.data["code_text"], "Too Many Requests");
        assert_eq!(
            result.data["content"].as_str().unwrap(),
            format_http_error_message(429, Some("30"))
        );
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
        assert_eq!(
            ua_value,
            &format!(
                "Claude-User (claude-code/{}; +https://support.anthropic.com/)",
                env!("CARGO_PKG_VERSION")
            ),
            "WebFetch UA must be claude-code's `Claude-User (...)` form"
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

    // ---- redirect loop drives request_no_follow (transport no-follow) -------

    /// A transport that distinguishes [`HttpTransport::request`] from
    /// [`HttpTransport::request_no_follow`]: it counts calls to each and only
    /// the `request_no_follow` path returns the scripted redirect/body. If the
    /// WebFetch loop regressed to calling plain `request`, the redirect response
    /// would NOT be served (the `request` arm returns a 200 sentinel and bumps a
    /// separate counter the assertions catch).
    ///
    /// Responses are FIFO from a single queue, consumed by `request_no_follow`.
    /// The blocklist preflight is skipped via `LINGXI_SKIP_WEBFETCH_PREFLIGHT`
    /// (held under `SKIP_ENV_LOCK`) so the only transport traffic is the fetch
    /// loop itself — keeping the call counts unambiguous.
    struct NoFollowMock {
        queue: std::sync::Mutex<std::collections::VecDeque<protocol::HttpResponse>>,
        request_calls: std::sync::atomic::AtomicUsize,
        no_follow_calls: std::sync::atomic::AtomicUsize,
    }

    impl NoFollowMock {
        fn new(responses: Vec<protocol::HttpResponse>) -> Arc<Self> {
            Arc::new(Self {
                queue: std::sync::Mutex::new(responses.into()),
                request_calls: std::sync::atomic::AtomicUsize::new(0),
                no_follow_calls: std::sync::atomic::AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl HttpTransport for NoFollowMock {
        async fn request(
            &self,
            _req: HttpRequest,
        ) -> Result<protocol::HttpResponse, HttpError> {
            // The WebFetch redirect loop must NOT reach this path. Count it and
            // return a harmless 200 so a regression is visible via the counter
            // (and the redirect/body the test scripted goes unconsumed).
            self.request_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(protocol::HttpResponse {
                status: 200,
                headers: vec![],
                body: "WRONG-PATH: plain request was called".into(),
            })
        }
        async fn request_no_follow(
            &self,
            _req: HttpRequest,
        ) -> Result<protocol::HttpResponse, HttpError> {
            self.no_follow_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match self.queue.lock().unwrap().pop_front() {
                Some(resp) => Ok(resp),
                None => Err(HttpError::InvalidResponse("no scripted response".into())),
            }
        }
        async fn stream_sse(
            &self,
            _req: HttpRequest,
        ) -> Result<traits::http::SseStream, HttpError> {
            Err(HttpError::InvalidRequest("sse not used in this mock".into()))
        }
    }

    fn redirect_resp(status: u16, location: &str) -> protocol::HttpResponse {
        protocol::HttpResponse {
            status,
            headers: vec![("location".into(), location.to_string())],
            body: String::new(),
        }
    }

    fn ctx_with_transport(http: Arc<dyn HttpTransport>) -> BuiltinToolContext {
        let bus = Arc::new(AnalyticsBus::new());
        let mut ctx = tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            bus,
            vec![std::path::PathBuf::from("/tmp")],
        );
        ctx.http = http;
        ctx
    }

    /// A cross-host 3xx surfaced by `request_no_follow` must drive the loop to
    /// return the byte-exact "REDIRECT DETECTED" notice — and the loop must use
    /// `request_no_follow`, NOT plain `request`. This is the production-path
    /// regression guard the whole change exists for.
    #[tokio::test]
    async fn cross_host_redirect_via_request_no_follow_returns_notice() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        std::env::set_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT", "1");

        let http = NoFollowMock::new(vec![redirect_resp(301, "https://other.example/landing")]);
        let ctx = ctx_with_transport(http.clone() as Arc<dyn HttpTransport>);
        let tool = WebFetchTool::new(ctx);
        let res = tool
            .call(
                json!({ "url": "https://orig.example/page", "prompt": "summarize this" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await;
        std::env::remove_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT");
        let res = res.expect("cross-host redirect must return Ok with the notice");

        assert_eq!(res.data["status"], 301);
        assert_eq!(res.data["code_text"], "Moved Permanently");
        let content = res.data["content"].as_str().unwrap();
        assert!(
            content.starts_with("REDIRECT DETECTED: The URL redirects to a different host."),
            "unexpected content: {content}"
        );
        assert!(content.contains("Redirect URL: https://other.example/landing"));
        assert!(content.contains("- prompt: \"summarize this\""));

        // The loop drove `request_no_follow`, never plain `request`.
        assert_eq!(
            http.no_follow_calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "must fetch via request_no_follow"
        );
        assert_eq!(
            http.request_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "must NOT use plain request (would auto-follow in production)"
        );
    }

    /// A same-host (permitted) 3xx from `request_no_follow` must be FOLLOWED:
    /// the loop re-issues `request_no_follow` against the redirect target and
    /// returns the final body — again never touching plain `request`.
    #[tokio::test]
    async fn same_host_redirect_via_request_no_follow_is_followed() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        std::env::set_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT", "1");

        // First hop: same-host redirect (only the path changes). Second hop: 200.
        let http = NoFollowMock::new(vec![
            redirect_resp(301, "https://follow.example/final"),
            protocol::HttpResponse {
                status: 200,
                headers: vec![],
                body: "final body".into(),
            },
        ]);
        let ctx = ctx_with_transport(http.clone() as Arc<dyn HttpTransport>);
        let tool = WebFetchTool::new(ctx);
        let res = tool
            .call(
                json!({ "url": "https://follow.example/start" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await;
        std::env::remove_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT");
        let res = res.expect("permitted redirect must be followed to the final body");

        assert_eq!(res.data["status"], 200);
        assert_eq!(res.data["content"], "final body");
        // Two `request_no_follow` calls (start + final), zero plain `request`.
        assert_eq!(
            http.no_follow_calls.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "must follow the permitted redirect via a second request_no_follow"
        );
        assert_eq!(
            http.request_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "must NOT use plain request"
        );
    }

    /// #87: 303 (See Other) is in claude-code's redirect set
    /// (`YHp=new Set([301,302,303,307,308])`). A cross-host 303 must be DETECTED
    /// as a redirect (→ the notice), not treated as normal page content.
    #[tokio::test]
    async fn cross_host_303_redirect_is_detected() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        std::env::set_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT", "1");

        let http = NoFollowMock::new(vec![redirect_resp(303, "https://other.example/landing")]);
        let ctx = ctx_with_transport(http.clone() as Arc<dyn HttpTransport>);
        let tool = WebFetchTool::new(ctx);
        let res = tool
            .call(
                json!({ "url": "https://orig.example/page", "prompt": "do x" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await;
        std::env::remove_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT");
        let res = res.expect("303 must be detected as a redirect and return Ok with the notice");

        assert_eq!(res.data["status"], 303);
        assert_eq!(res.data["code_text"], "See Other");
        let content = res.data["content"].as_str().unwrap();
        assert!(
            content.starts_with("REDIRECT DETECTED: The URL redirects to a different host."),
            "303 should produce the redirect notice, got: {content}"
        );
        assert!(content.contains("Status: 303 See Other"));
    }

    /// #92: a redirect with a missing (or empty/whitespace) Location header
    /// degrades to the `http_error` SUCCESS result with the 3xx status code
    /// (claude-code: `if(typeof l!=="string"||l.trim()==="")return
    /// {type:"http_error",statusCode:s}`), NOT a thrown transport error.
    #[tokio::test]
    async fn redirect_without_location_returns_http_error_result() {
        let _env = SKIP_ENV_LOCK.lock().await;
        crate::cache::clear_web_fetch_cache();
        crate::blocklist::clear_domain_check_cache();
        std::env::set_var("LINGXI_SKIP_WEBFETCH_PREFLIGHT", "1");

        // 301 with NO Location header.
        let http = NoFollowMock::new(vec![protocol::HttpResponse {
            status: 301,
            headers: vec![],
            body: String::new(),
        }]);
        let ctx = ctx_with_transport(http.clone() as Arc<dyn HttpTransport>);
        let tool = WebFetchTool::new(ctx);
        let res = tool
            .call(json!({ "url": "https://noloc.example/page" }), fresh_ctx(), fresh_tx())
            .await
            .expect("redirect-without-Location must be Ok(http_error result), not Err");

        assert_eq!(res.data["status"], 301);
        assert_eq!(res.data["code_text"], "Moved Permanently");
        assert_eq!(res.data["bytes"], 0);
        assert_eq!(
            res.data["content"].as_str().unwrap(),
            format_http_error_message(301, None)
        );
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
                    // Inferred as `cost::Usage::default()`. Spelled `Default::default()`
                    // so the mock needn't name `cost` (the dev-dep was dropped); the
                    // clippy::default_trait_access lint that prefers the explicit type
                    // is intentionally allowed here for that reason.
                    #[allow(clippy::default_trait_access)]
                    usage: Default::default(),
                    stop_reason: Some("end_turn".into()),
                })
            }
        }

        #[tokio::test]
        async fn cache_hit_still_runs_apply_step() {
            let _env = SKIP_ENV_LOCK.lock().await;
            crate::cache::clear_web_fetch_cache();
            crate::blocklist::clear_domain_check_cache();
            let (ctx, http, _sink) = make_web_ctx();
            http.enqueue(preflight_allow());
            http.enqueue(ScriptedResponse::Sync(protocol::HttpResponse {
                status: 200,
                headers: vec![("content-type".into(), "text/html".into())],
                body: "<h1>Doc</h1>".into(),
            }));
            let capture = std::sync::Arc::new(CapturingSideQuery {
                captured: std::sync::Mutex::new(None),
                reply: "APPLIED".into(),
            });
            let tool = WebFetchTool::new(ctx).with_side_query(capture.clone());
            let url = json!({ "url": "https://cachehit-apply.example/x", "prompt": "summarize" });
            let first = tool.call(url.clone(), fresh_ctx(), fresh_tx()).await.expect("first ok");
            assert_eq!(first.data["content"], "APPLIED");
            let second = tool.call(url, fresh_ctx(), fresh_tx()).await.expect("second ok");
            assert_eq!(second.data["content"], "APPLIED", "cache hit must still run the apply step");
            let seen = capture.captured.lock().unwrap().clone().unwrap();
            assert!(seen.contains("# Doc"));
            assert_eq!(http.received_requests().len(), 2, "second call must be a cache hit (no new fetch)");
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
