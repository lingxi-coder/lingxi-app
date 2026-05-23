//! Anthropic provider — builds API requests and maps SSE events to engine
//! events.
//!
//! This module only constructs request shapes and decodes individual SSE
//! payloads; all network I/O is delegated to the `HttpTransport` trait
//! (wired in Tasks 16–18).

use crate::oauth_hook::{OAuthRefreshHook, TokenHash, current_hook};
use crate::rate_limit::{
    format_rate_limited_msg, parse_anthropic_ratelimit_reset, parse_retry_after,
};
use crate::retry::{DEFAULT_BASE_DELAYS_MS, DEFAULT_RETRY_BUDGET, with_retry};
use crate::types::{MessageResponse, StreamEvent};
use crate::ApiError;
use lingxi_protocol::{ConversationMessage, HttpMethod, HttpRequest};
use lingxi_traits::HttpTransport;
use serde_json::Value;
use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

/// Default Anthropic API base URL. Override via [`AnthropicProvider::new`].
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Value sent in the `anthropic-version` header on every request.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// User-Agent value sent on every Anthropic API request. Spec §7 wire identifier.
///
/// Format: `claude-cli/<CARGO_PKG_VERSION> (external, cli)`. Locked byte-for-byte
/// against claude-code @ 6a25909. The `<version>` is the api-client crate's
/// `CARGO_PKG_VERSION` at compile time.
#[must_use]
pub fn user_agent() -> String {
    format!("claude-cli/{} (external, cli)", env!("CARGO_PKG_VERSION"))
}

/// Generate a short opaque request ID for telemetry tagging. URL-safe alphanumeric.
///
/// Format: 16 chars `[a-zA-Z0-9-]`. Backed by `rand::thread_rng()` so each
/// request gets an independent ID; we don't need cryptographic uniqueness
/// here — only enough to disambiguate concurrent requests in event logs.
#[must_use]
pub fn new_request_id() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-";
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

/// Provider that builds Anthropic Messages API requests and parses
/// streaming events.
///
/// The `api_key` is stored as a plain `String` for Plan 1; Plan 2 swaps in
/// `secrets::SecretBox<String>`. The custom [`fmt::Debug`] impl redacts the
/// key in all current diagnostic output.
pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    /// Optional per-provider OAuth hook override. When `None`, falls back to
    /// the process-global registration via `oauth_hook::current_hook()`.
    oauth_hook: Option<Arc<dyn OAuthRefreshHook>>,
}

impl fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .field(
                "oauth_hook",
                &self.oauth_hook.as_ref().map(|_| "<dyn OAuthRefreshHook>"),
            )
            .finish()
    }
}

impl AnthropicProvider {
    /// Construct a new provider. Passing `None` for `base_url` uses
    /// [`DEFAULT_BASE_URL`].
    #[must_use]
    pub fn new(api_key: impl Into<String>, base_url: Option<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            oauth_hook: None,
        }
    }

    /// Override the OAuth refresh hook for this provider instance. Useful in
    /// tests; production wiring usually relies on the process-global
    /// `register_oauth_hook(...)`.
    #[must_use]
    pub fn with_oauth_hook(mut self, hook: Arc<dyn OAuthRefreshHook>) -> Self {
        self.oauth_hook = Some(hook);
        self
    }

    fn effective_hook(&self) -> Arc<dyn OAuthRefreshHook> {
        self.oauth_hook.clone().unwrap_or_else(current_hook)
    }

    /// Build a non-streaming `POST /v1/messages` request. The caller owns
    /// the JSON body; this method only attaches headers and metadata.
    #[must_use]
    pub fn build_request(&self, body: &Value) -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Post,
            url: format!("{}/v1/messages", self.base_url),
            headers: vec![
                ("x-api-key".into(), self.api_key.clone()),
                ("anthropic-version".into(), ANTHROPIC_VERSION.into()),
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "application/json".into()),
            ],
            body: Some(body.to_string()),
            timeout: Some(std::time::Duration::from_secs(120)),
        }
    }

    /// Build a streaming variant of [`Self::build_request`]: sets
    /// `stream: true` in the JSON body and swaps the `accept` header to
    /// `text/event-stream`.
    ///
    /// # Panics
    ///
    /// The two internal `expect` calls assume `body` round-trips through
    /// `serde_json` (it just came from `to_string`) and that the `accept`
    /// header set above is present. Both invariants hold by construction.
    #[must_use]
    pub fn build_streaming_request(&self, body: &Value) -> HttpRequest {
        let mut req = self.build_request(body);
        // Re-parse the body we just serialised so we can flip `stream: true`.
        // `unwrap` is safe: it was produced by `serde_json::Value::to_string`
        // a few lines above, which always emits valid JSON.
        let mut body_val: Value =
            serde_json::from_str(req.body.as_ref().expect("build_request always sets a body"))
                .expect("body was just serialised from a Value");
        body_val["stream"] = Value::Bool(true);
        req.body = Some(body_val.to_string());
        if let Some((_, v)) = req.headers.iter_mut().find(|(k, _)| k == "accept") {
            *v = "text/event-stream".to_string();
        }
        req
    }

    /// Parse a single SSE event payload into a [`StreamEvent`]. Used by the
    /// streaming consumer when iterating over events produced by
    /// `HttpTransport::stream_sse`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ApiError::MalformedStream`] when the payload is not
    /// valid JSON for the [`StreamEvent`] enum.
    pub fn parse_stream_event(data: &str) -> Result<StreamEvent, crate::ApiError> {
        serde_json::from_str::<StreamEvent>(data)
            .map_err(|e| crate::ApiError::MalformedStream(e.to_string()))
    }

    /// Non-streaming `POST /v1/messages` with retry + rate-limit + OAuth-hook
    /// middleware.
    ///
    /// Spec §4 Flow B. Retry budget = 3 attempts (500ms / 1s / 2s ± 20% jitter).
    /// On 401, calls the OAuth hook ONCE per request; on a second 401 the
    /// [`ApiError::Unauthorized`] propagates without further refresh attempts.
    /// On 429, parses Retry-After / anthropic-ratelimit-requests-reset and
    /// sleeps before counting another retry.
    ///
    /// # Errors
    /// See [`ApiError`] for the full failure taxonomy.
    pub async fn messages_create_non_stream<T: HttpTransport>(
        &self,
        model: &str,
        msgs: Vec<ConversationMessage>,
        transport: &T,
    ) -> Result<MessageResponse, ApiError> {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 4096u32,
            "messages": msgs,
        });
        // Bearer-token override populated only after a successful 401-driven
        // refresh; the first attempt always uses the constructor-supplied
        // x-api-key. `with_retry` does not see the override variable — it is
        // consumed only on the post-refresh manual retry below.
        let bearer_token: Option<String> = None;

        let hook = self.effective_hook();
        let resp = with_retry(DEFAULT_RETRY_BUDGET, DEFAULT_BASE_DELAYS_MS, |_attempt| {
            let token_override = bearer_token.clone();
            let body = body.clone();
            async move {
                let req = self.build_request_with_betas(&body, token_override.as_deref());
                transport.request(req).await
            }
        })
        .await;

        match resp {
            Ok(http_resp) => serde_json::from_str::<MessageResponse>(&http_resp.body)
                .map_err(|e| ApiError::MalformedStream(e.to_string())),
            Err(ApiError::Server {
                status: 401,
                body: server_body,
            }) => {
                // Drive one refresh + retry, then surface the next outcome verbatim.
                let result = match hook.refresh(TokenHash([0u8; 32])).await {
                    Ok(crate::BearerToken(token)) => {
                        let bearer = self.bearer_to_header(&token);
                        let bearer_opt = Some(bearer);
                        // Retry exactly once with the new token (no further refresh):
                        let req = self.build_request_with_betas(&body, bearer_opt.as_deref());
                        let resp2 = transport.request(req).await.map_err(ApiError::Http)?;
                        if resp2.status == 200 {
                            serde_json::from_str::<MessageResponse>(&resp2.body)
                                .map_err(|e| ApiError::MalformedStream(e.to_string()))
                        } else if resp2.status == 401 {
                            Err(ApiError::Unauthorized(resp2.body))
                        } else {
                            Err(ApiError::Server {
                                status: resp2.status,
                                body: resp2.body,
                            })
                        }
                    }
                    Err(crate::OAuthHookError::TokenStale) => {
                        Err(ApiError::OAuthHook(crate::OAuthHookError::TokenStale))
                    }
                    Err(other) => Err(ApiError::OAuthHook(other)),
                };
                // Note: server_body is intentionally dropped on the happy path
                // — the spec §5 says the user-facing string for AuthExhausted
                // comes from the second 401 (or the hook error), not the first.
                result.map_err(|e| {
                    if matches!(e, ApiError::Server { .. }) {
                        // Map 4xx other than 401 into Unauthorized for parity.
                        ApiError::Unauthorized(server_body.clone())
                    } else {
                        e
                    }
                })
            }
            Err(other) => Err(other),
        }
    }

    /// Build a base HTTP request with `anthropic-beta`, `user-agent`, and
    /// (if provided) `authorization: Bearer ...` headers attached.
    fn build_request_with_betas(
        &self,
        body: &Value,
        bearer_override: Option<&str>,
    ) -> HttpRequest {
        use crate::betas::{Endpoint, Provider, assemble_beta_header};
        let mut req = self.build_request(body);
        // Attach beta header (Anthropic / MessagesCreate non-stream by default).
        let beta = assemble_beta_header(Provider::Anthropic, Endpoint::MessagesCreate);
        if !beta.is_empty() {
            req.headers.push(("anthropic-beta".into(), beta));
        }
        req.headers.push(("user-agent".into(), user_agent()));
        // Spec §7: X-Request-Id is set per call for telemetry correlation.
        req.headers
            .push(("x-request-id".into(), new_request_id()));
        // Default timeout for non-stream messages.create is 600s (spec §7);
        // override what `build_request` set (120s).
        req.timeout = Some(std::time::Duration::from_secs(600));
        if let Some(token) = bearer_override {
            // Replace x-api-key with Bearer auth (M3-04 token-based flow).
            req.headers
                .retain(|(k, _)| !k.eq_ignore_ascii_case("x-api-key"));
            req.headers
                .push(("authorization".into(), format!("Bearer {token}")));
        }
        req
    }

    fn bearer_to_header(&self, token: &lingxi_protocol::Secret<String>) -> String {
        let _ = self; // suppress dead-code lint when impl is empty.
        token.expose_secret().clone()
    }
}

/// Provider parameter for [`AnthropicProvider::count_tokens`]. Different
/// providers gate different model families per spec §7. M3-03 implements
/// three:
/// * `Anthropic` — accepts any model.
/// * `Vertex` — restricted to `claude-3*` and `claude-opus*` families
///   (matches the `VERTEX_COUNT_TOKENS_ALLOWED` beta whitelist's model
///   coverage).
/// * `Bedrock` — same whitelist as Vertex (claude-code parity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountTokensProvider {
    /// Anthropic (api.anthropic.com); no model restriction.
    Anthropic,
    /// Vertex AI; only `claude-3*` and `claude-opus*` accepted.
    Vertex,
    /// AWS Bedrock; same restriction as Vertex.
    Bedrock,
}

impl CountTokensProvider {
    fn name(self) -> &'static str {
        match self {
            CountTokensProvider::Anthropic => "anthropic",
            CountTokensProvider::Vertex => "vertex",
            CountTokensProvider::Bedrock => "bedrock",
        }
    }

    fn allows_model(self, model: &str) -> bool {
        match self {
            CountTokensProvider::Anthropic => true,
            CountTokensProvider::Vertex | CountTokensProvider::Bedrock => {
                model.starts_with("claude-3") || model.starts_with("claude-opus")
            }
        }
    }
}

/// Decoded body of a `count_tokens` response. Anthropic's wire shape is
/// `{"input_tokens": N}`; that's all we need.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CountTokensResponse {
    /// Number of input tokens the prompt consumes.
    pub input_tokens: u64,
}

impl AnthropicProvider {
    /// `POST /v1/messages/count_tokens` with provider-specific model whitelist.
    ///
    /// Default timeout is 30s per spec §7 line 664.
    ///
    /// # Errors
    /// * [`ApiError::UnsupportedModel`] if the provider doesn't permit `model`.
    /// * Otherwise the same failure modes as `messages_create_non_stream`.
    pub async fn count_tokens<T: HttpTransport>(
        &self,
        model: &str,
        msgs: Vec<ConversationMessage>,
        provider: CountTokensProvider,
        transport: &T,
    ) -> Result<CountTokensResponse, ApiError> {
        if !provider.allows_model(model) {
            return Err(ApiError::UnsupportedModel {
                model: model.into(),
                provider: provider.name(),
            });
        }
        let body = serde_json::json!({
            "model": model,
            "messages": msgs,
        });
        let req = self.build_count_tokens_request(&body, provider);
        let resp = transport.request(req).await.map_err(ApiError::Http)?;
        if resp.status != 200 {
            return Err(ApiError::Server {
                status: resp.status,
                body: resp.body,
            });
        }
        serde_json::from_str::<CountTokensResponse>(&resp.body)
            .map_err(|e| ApiError::MalformedStream(e.to_string()))
    }

    fn build_count_tokens_request(
        &self,
        body: &Value,
        provider: CountTokensProvider,
    ) -> HttpRequest {
        use crate::betas::{Endpoint, Provider as BetaProvider, assemble_beta_header};
        let beta_provider = match provider {
            CountTokensProvider::Anthropic => BetaProvider::Anthropic,
            CountTokensProvider::Vertex => BetaProvider::Vertex,
            CountTokensProvider::Bedrock => BetaProvider::Bedrock,
        };
        let mut headers = vec![
            ("x-api-key".into(), self.api_key.clone()),
            ("anthropic-version".into(), ANTHROPIC_VERSION.into()),
            ("content-type".into(), "application/json".into()),
            ("accept".into(), "application/json".into()),
            ("user-agent".into(), user_agent()),
        ];
        let beta = assemble_beta_header(beta_provider, Endpoint::CountTokens);
        if !beta.is_empty() {
            headers.push(("anthropic-beta".into(), beta));
        }
        HttpRequest {
            method: HttpMethod::Post,
            url: format!("{}/v1/messages/count_tokens", self.base_url),
            headers,
            body: Some(body.to_string()),
            timeout: Some(std::time::Duration::from_secs(30)),
        }
    }
}

/// Sleep helper for 429 responses. Parses `Retry-After` /
/// `anthropic-ratelimit-requests-reset`, falling back to 1s. Hooked up
/// end-to-end in Task 9 (rate-limit integration); kept here so the rate-limit
/// + telemetry wiring is co-located with the request middleware.
#[allow(dead_code, reason = "wired into messages_create_non_stream in Task 9")]
async fn handle_rate_limit(headers: &[(String, String)]) {
    let now = SystemTime::now();
    let sleep = parse_retry_after(headers)
        .or_else(|| parse_anthropic_ratelimit_reset(headers, now))
        .unwrap_or(std::time::Duration::from_secs(1));
    tracing::warn!(
        target: "lingxi::api_client::rate_limit",
        secs = sleep.as_secs(),
        msg = %format_rate_limited_msg(sleep.as_secs()),
        "429 received; sleeping"
    );
    tokio::time::sleep(sleep).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_protocol::HttpMethod;

    #[test]
    fn build_request_includes_auth_and_version_headers() {
        let provider = AnthropicProvider::new("sk-ant-test", None);
        let body = serde_json::json!({"model": "claude-opus-4-6"});
        let req = provider.build_request(&body);
        let header_keys: Vec<&str> = req.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(header_keys.contains(&"x-api-key"));
        assert!(header_keys.contains(&"anthropic-version"));
        assert!(header_keys.contains(&"content-type"));
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn build_request_redacts_api_key_in_debug() {
        let provider = AnthropicProvider::new("sk-ant-secret", None);
        let s = format!("{provider:?}");
        assert!(!s.contains("sk-ant-secret"), "api key leaked: {s}");
    }

    #[test]
    fn user_agent_format_is_byte_locked() {
        let ua = crate::anthropic::user_agent();
        let expected = format!(
            "claude-cli/{} (external, cli)",
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(ua, expected);
        // Also smoke: the literal substring must be present so we catch
        // accidental rewrites that swap the parenthetical.
        assert!(ua.contains("(external, cli)"));
    }

    #[test]
    fn new_request_id_is_non_empty_and_url_safe() {
        let id = crate::anthropic::new_request_id();
        assert!(!id.is_empty());
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }
}
