//! `WebFetchTool` — fetches a URL via the M1 `HttpTransport` trait, with a
//! 10 MB transfer cap, a 100 000-char markdown cap (applied in the apply step,
//! NOT before caching), an SSRF/abuse gate (length / credentials / single-label
//! host — no scheme rejection, faithful to claude-code `validateURL`), an
//! `http`→`https` upgrade, manual permitted-redirect handling, and the claude-code
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
//! - `WEBFETCH_ALLOWED_SCHEMES = ["https", "http"]` (upgrade-native schemes, NOT
//!   a rejection allow-list — see finding #93)
//! - HTTP error format: `"WebFetch: HTTP {status} from {url}"`
//! - DNS error format: `"WebFetch: cannot resolve {host}"`

use crate::persist::{append_binary_footer, is_binary_content_type};
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

/// Legacy User-Agent prefix (kept for `web_search` + the parity fixture). The
/// live WebFetch `User-Agent` is built by [`WebFetchTool::user_agent`] as
/// `Claude-User (claude-code/{traits::CLAUDE_CODE_VERSION}; +https://support.anthropic.com/)`
/// — see R-V1; this prefix const is NOT the WebFetch header.
/// Spec §7 lock.
pub const WEBFETCH_USER_AGENT_PREFIX: &str = "claude-code-tool/";

/// The two web schemes WebFetch handles natively: `https` is fetched as-is and
/// `http` is upgraded to `https` before the fetch ([`upgrade_to_https`]). This is
/// NOT a rejection allow-list — claude-code's WebFetch never rejects a URL by
/// scheme (see [`validate_url`] / finding #93); a non-http(s) scheme that passes
/// the SSRF gate reaches the transport and fails there. Retained only for
/// documentation/parity-fixture purposes.
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

/// Validate that `url_str` parses and passes the SSRF/abuse gate.
///
/// Returns the parsed `url::Url` on success, or a descriptive error string on
/// failure.
///
/// PARITY (#93): claude-code's WebFetch path (`getURLMarkdownContent`/
/// `validateURL` = `oqa`) does NOT reject by scheme — it only parse-checks,
/// rejects embedded credentials, and rejects a hostname with fewer than two
/// dot-separated labels, then upgrades `http:`→`https:`. A non-http(s) scheme
/// like `ftp://host.tld/` therefore passes `validateURL` (and fails later at
/// transport time). Schemes with no host (`file:///…`, `data:…`) are rejected
/// by the `< 2 labels` host rule, NOT by a scheme allow-list. The `Invalid URL
/// protocol` scheme check belongs to the OS-open path (`uVu`/`Dni`), not
/// WebFetch. The prior `WEBFETCH_ALLOWED_SCHEMES` rejection here was an invented
/// divergence and has been removed.
///
/// [`crate::url_safety::validate_url_safety`] remains the SSRF/abuse gate
/// (length / credentials / single-label host) — faithful to `validateURL`.
///
/// # Errors
/// Returns an error if the URL fails to parse or fails the SSRF/abuse gate.
pub fn validate_url(url_str: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(url_str).map_err(|e| format!("invalid URL: {e}"))?;
    // claude-code `validateURL` SSRF/abuse gate: overlong URL, embedded
    // credentials, single-label/internal hostname (the latter also rejects
    // host-less schemes such as `file:`/`data:`, matching `oqa`).
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

/// Whether the FULL fetched `body` exceeds the 100k-char apply-step cap and would
/// therefore be truncated inside the apply step (`applyPromptToMarkdown` = `l9n`:
/// `t.length > Cut`). PARITY (#88): the body is cached/returned in FULL; the cap
/// only fires inside the apply step, so this is the source of the LingXi-internal
/// `truncated` telemetry/result flag. Measured by `char` count (Unicode scalar),
/// matching the raw fast-path condition `content.length < Cut`.
#[must_use]
pub fn body_exceeds_markdown_cap(body: &str) -> bool {
    body.chars().count() > WEBFETCH_MAX_MARKDOWN_LEN
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
        // claude-code WebFetch User-Agent (v2.1.183: `Claude-User (${tg()}; +...)`):
        // `Claude-User (claude-code/<version>; +https://support.anthropic.com/)`.
        // The `Claude-User (...)` wrapper is how Anthropic web infra recognizes
        // claude-code fetch traffic (distinct from the api-client UA).
        // R-V1: the version is claude-code's VERSION (the parity target,
        // `traits::CLAUDE_CODE_VERSION`), NOT LingXi's CARGO_PKG_VERSION — every
        // WebFetch GET previously sent `claude-code/0.12.0` to Anthropic infra +
        // target servers instead of `claude-code/2.1.183`.
        format!(
            "Claude-User (claude-code/{}; +https://support.anthropic.com/)",
            traits::CLAUDE_CODE_VERSION
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

    /// Produce the tool-visible result body from the FULL fetched `content`,
    /// 1:1 with claude-code's WebFetch `call()` apply decision (#89):
    ///
    /// ```text
    /// let g = isPreapprovedUrl(url);
    /// if (g && contentType.includes("text/markdown") && content.length < Cut)
    ///     result = content;                      // raw fast-path
    /// else
    ///     result = await applyPromptToMarkdown(prompt, content, signal, …, g);
    /// ```
    ///
    /// The apply model is the DEFAULT; the raw body is returned ONLY when the URL
    /// is preapproved AND the content-type is `text/markdown` AND the body is
    /// under the 100k-char cap (`Cut`). The prompt is ALWAYS passed (empty string
    /// when absent — claude-code's `s` is the input prompt, present or `undefined`
    /// interpolated; LingXi models absence as `""`). The 100k truncation lives
    /// INSIDE the apply step ([`Self::apply_prompt`] → `markdown::truncate_markdown`),
    /// NOT before caching/returning (#88).
    ///
    /// LingXi degraded path: when no `side_query` client is wired (mobile/minimal),
    /// there is no apply model, so the raw body is returned regardless. This is a
    /// faithful degradation — claude-code always has a model available.
    #[cfg(feature = "web-markdown")]
    async fn apply_or_raw(
        &self,
        is_preapproved: bool,
        content_type: &str,
        content: &str,
        prompt: Option<&str>,
    ) -> String {
        // Raw fast-path: preapproved + text/markdown + under the 100k char cap.
        if is_preapproved
            && content_type.contains("text/markdown")
            && content.chars().count() < WEBFETCH_MAX_MARKDOWN_LEN
        {
            return content.to_string();
        }
        // Apply step is the default whenever a model (side_query) is wired.
        if let Some(client) = self.side_query.as_ref() {
            return self
                .apply_prompt(client, is_preapproved, content, prompt.unwrap_or(""))
                .await;
        }
        // Degraded path (no model wired): return the raw body.
        content.to_string()
    }

    /// No-op fallback when `web-markdown` is disabled: always the raw body.
    #[cfg(not(feature = "web-markdown"))]
    #[allow(clippy::unused_async)]
    async fn apply_or_raw(
        &self,
        _is_preapproved: bool,
        _content_type: &str,
        content: &str,
        _prompt: Option<&str>,
    ) -> String {
        content.to_string()
    }

    /// Persist the RAW response `body` to a temp file when `content_type` is a
    /// binary type ([`is_binary_content_type`] = `U7r`), 1:1 with the
    /// `if(U7r(a)){…B$e(i,a,A)…}` block in `getURLMarkdownContent`.
    ///
    /// Returns `(persisted_path, persisted_size)` — both `None` for a non-binary
    /// body or on a write failure (matching `if(!("error"in h))`, which silently
    /// skips persistence and the footer on error).
    ///
    /// Temp dir: `<workspace>/.lingxi/tool-results` (the same project-local
    /// tool-results dir other tools use, e.g. `tools/mcp`'s binary-blob persist).
    /// claude-code uses `<config>/<session>/tool-results`; LingXi's tool context
    /// exposes the workspace root, not the config/session root, so the artifact is
    /// written under the project's `.lingxi/tool-results`. (Residual: see report.)
    fn persist_binary(
        &self,
        content_type: &str,
        body: &[u8],
    ) -> (Option<String>, Option<usize>) {
        if !is_binary_content_type(content_type) {
            return (None, None);
        }
        let unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0u128, |d| d.as_millis());
        // A non-crypto uniqueness seed for the 6-char base-36 suffix (matches
        // `Math.random().toString(36).slice(2,8)`'s role: collision-avoidance, not
        // security). Mixes the ns clock with the body length + pointer.
        let seed = {
            let ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0u128, |d| d.as_nanos()) as u64;
            ns ^ (body.len() as u64).rotate_left(17) ^ (body.as_ptr() as u64)
        };
        let stem = crate::persist::persisted_filename(unix_ms, seed);
        let output_dir = self.ctx.workspace.join(branding::DOT_DIR).join("tool-results");
        match crate::persist::persist_binary_content(body, content_type, &stem, &output_dir) {
            crate::persist::PersistResult::Ok { filepath, size } => (Some(filepath), Some(size)),
            crate::persist::PersistResult::Err { .. } => (None, None),
        }
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
        // claude-code zod (binary @201712484):
        // `E.strictObject({url:E.string().url().describe(...),prompt:E.string()...})`
        // — `prompt` is NOT `.optional()`, and `sdk-tools.d.ts`'s `WebFetchInput`
        // confirms `{ url: string; prompt: string }` (both required). So `prompt`
        // is advertised as required. The Rust `WebFetchInput.prompt` stays
        // `Option<String>` with `#[serde(default)]` purely for deserialization
        // tolerance (a missing/legacy payload decodes to `None` rather than
        // erroring); the requirement is enforced at the model-facing schema, as
        // claude-code enforces it at its zod input gate.
        "required": ["url", "prompt"],
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
    async fn prompt(&self, opts: &PromptOptions) -> String {
        // 1:1 with claude-code `CMi(model)` (binary @196852876) — WebFetch DOES
        // have a model-gated `prompt()` (the prior "no separate prompt()" note was
        // STALE vs v2.1.185). `Dh(model)` (the shared `dh_simple_system_prompt`
        // gate, identical to WebSearch) selects the SHORT variant; otherwise the
        // LONG = an `IMPORTANT: WebFetch WILL FAIL…` auth-warning prefix (ending
        // in `\n`) followed by the DESCRIPTION (which itself begins with `\n`, so
        // the join is `access.\n\n- Fetches…`, matching the `…access.\n${GSd}`
        // template, od-verified). The `t` (hasArtifactTool) artifact-exception
        // branch is omitted — LingXi exposes no claude.ai/code/artifact tool, so
        // `t` is always false.
        if tool_api::dh_simple_system_prompt(opts.model.as_deref()) {
            "Fetches a URL, converts the page to markdown, and answers `prompt` against it using a small fast model.\n\n- Fails on authenticated/private URLs \u{2014} use an authenticated MCP tool or `gh` for those instead.\n- HTTP is upgraded to HTTPS. Cross-host redirects are returned to you rather than followed; call again with the redirect URL.\n- Responses are cached for 15 minutes per URL.".to_string()
        } else {
            let description = self
                .description(&Value::Null, &DescriptionOptions {
                    is_non_interactive_session: false,
                })
                .await;
            format!(
                "IMPORTANT: WebFetch WILL FAIL for authenticated or private URLs. Before using this tool, check if the URL points to an authenticated service (e.g. Google Docs, Confluence, Jira, GitHub). If so, look for a specialized MCP tool that provides authenticated access.\n{description}"
            )
        }
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
        // `durationMs` baseline for the result `data` object — claude-code stamps
        // `durationMs: Date.now() - l` (l = the tool-call start) on EVERY path,
        // including cache hits (which is why this is measured here, before the
        // cache check, rather than at the redirect-loop `started` below).
        let call_started = Instant::now();

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
            // PARITY (#88): the cached `content` is the FULL body (claude-code
            // caches `content:p` un-sliced; the 100k cap only fires inside the
            // apply step). The LingXi-internal `truncated` flag is derived from
            // whether the body exceeds the apply-step cap, not from a suffix.
            let truncated = body_exceeds_markdown_cap(&hit.content);
            // Cache hits do no network work, so the reported duration is 0 ms.
            self.emit_completed(&invocation_id, hit.status, hit.bytes as u64, truncated, 0)
                .await;
            // claude-code caches only the body; the apply decision runs on EVERY
            // call (cache hit or miss) — PARITY (#89): apply is the default, raw
            // only for preapproved + text/markdown + under-cap.
            let out_content = self
                .apply_or_raw(
                    is_preapproved,
                    &hit.content_type,
                    &hit.content,
                    parsed_input.prompt.as_deref(),
                )
                .await;
            // Binary footer (#94): re-append on the cache-hit path when the cached
            // entry persisted a binary artifact.
            let out_content = append_binary_footer(
                out_content,
                hit.persisted_path.as_deref(),
                &hit.content_type,
                hit.persisted_size,
                hit.bytes,
            );
            return Ok(ToolCallResult {
                // claude-code WebFetch result `data` (byte-faithful key set + order):
                // `{bytes, code, codeText, result, durationMs, url}` (verified vs the
                // 2.1.191 binary; `result` is the model-facing content, `codeText` is
                // the HTTP reason phrase, `code` the numeric status). NOTE: the
                // LingXi-internal `truncated` flag is NOT part of claude-code's
                // contract and is intentionally omitted.
                data: json!({
                    "bytes": hit.bytes,
                    "code": hit.status,
                    "codeText": status_reason_phrase(hit.status),
                    "result": out_content,
                    "durationMs": call_started.elapsed().as_millis() as u64,
                    "url": parsed_input.url,
                }),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
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
                                    "bytes": 0,
                                    "code": resp.status,
                                    "codeText": code_text,
                                    "result": message,
                                    "durationMs": call_started.elapsed().as_millis() as u64,
                                    "url": parsed_input.url,
                                }),
                                model_content: None,
                                new_messages: vec![],
                                context_modifier: None,
                                is_error: false,
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
                            "bytes": message.len(),
                            "code": resp.status,
                            "codeText": status_text,
                            "result": message,
                            "durationMs": call_started.elapsed().as_millis() as u64,
                            "url": parsed_input.url,
                        }),
                        model_content: None,
                        new_messages: vec![],
                        context_modifier: None,
                        is_error: false,
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
                        "bytes": 0,
                        "code": resp.status,
                        "codeText": code_text,
                        "result": message,
                        "durationMs": call_started.elapsed().as_millis() as u64,
                        "url": parsed_input.url,
                    }),
                    model_content: None,
                    new_messages: vec![],
                    context_modifier: None,
                    is_error: false,
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

                // Binary persist (#94): claude-code's `getURLMarkdownContent`
                // persists the RAW response bytes to a temp file BEFORE the
                // HTML→markdown conversion, whenever the content-type is binary
                // (`U7r`). The artifact is the raw bytes; the cached/returned
                // `content` is still the converted markdown / utf-8 text. On a
                // persist error, the footer is simply skipped (`if(!("error"in
                // h))`).
                //
                // RESIDUAL: the M1 `HttpResponse.body` is a `String` (UTF-8 text),
                // so for a genuinely-binary body the original bytes are already
                // lost to UTF-8 decoding upstream — we persist `body.as_bytes()`,
                // which is faithful for text-ish/over-cap bodies and exercises the
                // full predicate/persist/footer path, but is NOT byte-identical to
                // the wire for true binaries. Full fidelity needs a `Vec<u8>`
                // transport body (a cross-subsystem change — see report).
                let (persisted_path, persisted_size) =
                    self.persist_binary(&content_type, resp.body.as_bytes());

                // HTML->markdown (claude-code converts HTML; non-HTML is used as-is).
                // Behind `web-markdown`; feature off => content is the raw body.
                #[cfg(feature = "web-markdown")]
                let content = if crate::markdown::is_html_content_type(&content_type) {
                    crate::markdown::html_to_markdown(&resp.body)
                } else {
                    resp.body
                };
                #[cfg(not(feature = "web-markdown"))]
                let content = resp.body;

                // PARITY (#88): cache + return the FULL converted body. claude-code
                // caches `content:p` un-sliced — the 100k cap fires ONLY inside the
                // apply step. The LingXi-internal `truncated` flag reflects whether
                // the body exceeds the apply-step cap.
                let truncated = body_exceeds_markdown_cap(&content);

                crate::cache::cache_set(
                    parsed_input.url.clone(),
                    crate::cache::CachedFetch {
                        content: content.clone(),
                        status,
                        content_type: content_type.clone(),
                        bytes: body_bytes,
                        persisted_path: persisted_path.clone(),
                        persisted_size,
                    },
                );
                self.emit_completed(&invocation_id, status, body_bytes as u64, truncated, elapsed_ms)
                    .await;

                // PARITY (#89): apply is the default; raw only for preapproved +
                // text/markdown + under-cap.
                let out_content = self
                    .apply_or_raw(
                        is_preapproved,
                        &content_type,
                        &content,
                        parsed_input.prompt.as_deref(),
                    )
                    .await;
                // Binary footer (#94): append when a binary artifact was persisted.
                let out_content = append_binary_footer(
                    out_content,
                    persisted_path.as_deref(),
                    &content_type,
                    persisted_size,
                    body_bytes,
                );

                Ok(ToolCallResult {
                    data: json!({
                        "bytes": body_bytes,
                        "code": status,
                        "codeText": status_reason_phrase(status),
                        "result": out_content,
                        "durationMs": call_started.elapsed().as_millis() as u64,
                        "url": parsed_input.url,
                    }),
                    model_content: None,
                    new_messages: vec![],
                    context_modifier: None,
                    is_error: false,
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
                        "bytes": 0,
                        "code": status,
                        "codeText": code_text,
                        "result": message,
                        "durationMs": call_started.elapsed().as_millis() as u64,
                        "url": parsed_input.url,
                    }),
                    model_content: None,
                    new_messages: vec![],
                    context_modifier: None,
                    is_error: false,
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
#[path = "web_fetch_test.rs"]
mod web_fetch_test;
