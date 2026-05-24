//! Anthropic provider — builds API requests and maps SSE events to engine
//! events.
//!
//! This module only constructs request shapes and decodes individual SSE
//! payloads; all network I/O is delegated to the `HttpTransport` trait
//! (wired in Tasks 16–18).

use crate::oauth_hook::{current_hook, OAuthRefreshHook, TokenHash};
use crate::rate_limit::{parse_anthropic_ratelimit_reset, parse_retry_after};
use crate::retry::{with_retry, DEFAULT_BASE_DELAYS_MS, DEFAULT_RETRY_BUDGET};
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
    /// Optional analytics bus. When `Some`, `tengu_api_*` events are emitted
    /// via `lingxi_telemetry::AnalyticsBus`. When `None` (test-mode default),
    /// emission is silently skipped — matches M3-01 / M3-02 convention.
    bus: Option<Arc<lingxi_telemetry::AnalyticsBus>>,
    /// Optional cost tracker; when present, every successful 200 response
    /// records cost via `tracker.record_api_response_v2(...)` and (if the
    /// provider also has a `bus`) emits `tengu_cost_recorded`.
    cost_tracker: Option<Arc<lingxi_cost::CostTracker>>,
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
            .field("bus", &self.bus.as_ref().map(|_| "<AnalyticsBus>"))
            .field(
                "cost_tracker",
                &if self.cost_tracker.is_some() {
                    "<set>"
                } else {
                    "<unset>"
                },
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
            bus: None,
            cost_tracker: None,
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

    /// Attach an `AnalyticsBus` so middleware emits `tengu_api_*` events.
    /// Without a bus, events are silently skipped (test-mode default).
    #[must_use]
    pub fn with_bus(mut self, bus: Arc<lingxi_telemetry::AnalyticsBus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Attach a cost tracker so successful 200 responses record cost.
    /// If the provider also has an
    /// [`AnalyticsBus`](lingxi_telemetry::AnalyticsBus) attached via
    /// [`Self::with_bus`], the tracker uses it to emit `tengu_cost_recorded`
    /// per spec §4 Flow B lines 364-376. Without a bus, the cost state
    /// still updates but no event fires.
    #[must_use]
    pub fn with_cost_tracker(mut self, tracker: Arc<lingxi_cost::CostTracker>) -> Self {
        self.cost_tracker = Some(tracker);
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
    /// sleeps before counting another retry. Emits four `tengu_api_*`
    /// telemetry events through the optional `AnalyticsBus`.
    ///
    /// # Errors
    /// See [`ApiError`] for the full failure taxonomy.
    pub async fn messages_create_non_stream<T: HttpTransport>(
        &self,
        model: &str,
        msgs: Vec<ConversationMessage>,
        transport: &T,
    ) -> Result<MessageResponse, ApiError> {
        let request_id = new_request_id();
        let started = std::time::Instant::now();
        telemetry::emit_started(&self.bus, model, &request_id, false).await;

        let body = serde_json::json!({
            "model": model,
            "max_tokens": 4096u32,
            "messages": msgs,
        });

        let resp_result = self.drive_retry_loop_with_429(&body, transport).await;
        let outcome = self
            .resolve_outcome(resp_result, &body, model, &request_id, transport)
            .await;

        self.emit_terminal_event(&outcome, model, &request_id, started)
            .await;

        // M3-05 §6 Flow B: after `tengu_api_request_succeeded` fires, record
        // cost via the optional tracker. Ordering is locked — this MUST
        // happen after the success event so `tengu_cost_recorded` (emitted
        // inside `record_api_response_v2`) lands strictly after.
        if let Ok(ref message_response) = outcome {
            self.record_cost_for_response(message_response, model, started.elapsed())
                .await;
        }

        outcome
    }

    /// Record cost for a successful 200 response via the optional tracker.
    /// No-op when `self.cost_tracker` is `None`.
    ///
    /// Mapping (per M3-05 §6):
    /// * `model` is the user-requested model (NOT `MessageResponse.model`,
    ///   which may differ).
    /// * `Usage` folds `cache_read_input_tokens` / `cache_creation_input_tokens`
    ///   into `tokens.cache_read` / `tokens.cache_write` so the calculator
    ///   applies the cache token rates, AND forwards them as separate args
    ///   for the event payload.
    /// * `retries = 0` — M3-03's `with_retry` doesn't surface the attempt
    ///   count; consistent with M1's `record_accumulates_cost` semantics.
    /// * `is_batch_request = false` ALWAYS in M3 (M4 brings batches).
    /// * The provider's own `bus` is forwarded so `tengu_cost_recorded`
    ///   fires on the same sink as `tengu_api_request_succeeded`.
    async fn record_cost_for_response(
        &self,
        message_response: &MessageResponse,
        model: &str,
        elapsed: std::time::Duration,
    ) {
        let Some(tracker) = &self.cost_tracker else {
            return;
        };
        let mr = lingxi_cost::ModelRef {
            provider: lingxi_cost::ProviderId::Anthropic,
            model: model.to_string(),
        };
        let usage = lingxi_cost::Usage {
            tokens: lingxi_cost::TokenUsage {
                input: message_response.usage.input_tokens,
                output: message_response.usage.output_tokens,
                cache_read: message_response.usage.cache_read_input_tokens,
                cache_write: message_response.usage.cache_creation_input_tokens,
                reasoning_output: 0,
            },
            server_tool_use: None,
            speed: None,
        };
        let _ = tracker
            .record_api_response_v2(
                mr,
                usage,
                elapsed,
                0, // retries — M3-03's with_retry doesn't surface the count
                message_response.usage.cache_read_input_tokens,
                message_response.usage.cache_creation_input_tokens,
                false, // is_batch_request — ALWAYS false in M3
                self.bus.as_ref(),
            )
            .await;
    }

    /// Run the retry loop, treating 429 as a synthesised 503 after sleeping
    /// for the parsed `Retry-After` / `anthropic-ratelimit-requests-reset`
    /// delay. Returns the underlying `with_retry` outcome.
    async fn drive_retry_loop_with_429<T: HttpTransport>(
        &self,
        body: &Value,
        transport: &T,
    ) -> Result<lingxi_protocol::HttpResponse, ApiError> {
        let bus_for_loop = self.bus.clone();
        let model_for_loop = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        with_retry(DEFAULT_RETRY_BUDGET, DEFAULT_BASE_DELAYS_MS, |_attempt| {
            let body = body.clone();
            let bus = bus_for_loop.clone();
            let model_s = model_for_loop.clone();
            async move {
                let req = self.build_request_with_betas(&body, None);
                let resp = transport.request(req).await?;
                if resp.status == 429 {
                    handle_429(&resp.headers, &bus, &model_s).await;
                    return Ok(lingxi_protocol::HttpResponse {
                        status: 503,
                        headers: Vec::new(),
                        body: String::new(),
                    });
                }
                Ok(resp)
            }
        })
        .await
    }

    /// Map the retry-loop outcome into the public response: parse 2xx bodies,
    /// drive the 401 → refresh path, and pass other errors through.
    async fn resolve_outcome<T: HttpTransport>(
        &self,
        resp_result: Result<lingxi_protocol::HttpResponse, ApiError>,
        body: &Value,
        model: &str,
        request_id: &str,
        transport: &T,
    ) -> Result<MessageResponse, ApiError> {
        match resp_result {
            Ok(http_resp) => serde_json::from_str::<MessageResponse>(&http_resp.body)
                .map_err(|e| ApiError::MalformedStream(e.to_string())),
            Err(ApiError::Server {
                status: 401,
                body: server_body,
            }) => {
                self.refresh_and_retry_401(body, server_body, transport)
                    .await
            }
            Err(other) => Err(other),
        }
        .inspect_err(|e| {
            tracing::debug!(
                target: "lingxi::api_client",
                request_id = %request_id,
                model = %model,
                kind = error_kind(e),
                "request terminated with error"
            );
        })
    }

    /// 401 refresh path: call the OAuth hook, retry once with the new bearer,
    /// then surface whatever the retry produced (or wrap stray 4xx as
    /// `Unauthorized` for spec parity).
    async fn refresh_and_retry_401<T: HttpTransport>(
        &self,
        body: &Value,
        server_body: String,
        transport: &T,
    ) -> Result<MessageResponse, ApiError> {
        let hook = self.effective_hook();
        let result = match hook.refresh(TokenHash([0u8; 32])).await {
            Ok(crate::BearerToken(token)) => {
                let bearer = self.bearer_to_header(&token);
                let req = self.build_request_with_betas(body, Some(&bearer));
                match transport.request(req).await {
                    Ok(resp2) if resp2.status == 200 => {
                        serde_json::from_str::<MessageResponse>(&resp2.body)
                            .map_err(|e| ApiError::MalformedStream(e.to_string()))
                    }
                    Ok(resp2) if resp2.status == 401 => Err(ApiError::Unauthorized(resp2.body)),
                    Ok(resp2) => Err(ApiError::Server {
                        status: resp2.status,
                        body: resp2.body,
                    }),
                    Err(e) => Err(ApiError::Http(e)),
                }
            }
            Err(crate::OAuthHookError::TokenStale) => {
                Err(ApiError::OAuthHook(crate::OAuthHookError::TokenStale))
            }
            Err(other) => Err(ApiError::OAuthHook(other)),
        };
        result.map_err(|e| {
            if matches!(e, ApiError::Server { .. }) {
                ApiError::Unauthorized(server_body.clone())
            } else {
                e
            }
        })
    }

    /// Emit the terminal telemetry event — either `tengu_api_request_succeeded`
    /// (with the wall-clock `duration_ms`) or `tengu_api_request_failed`.
    async fn emit_terminal_event(
        &self,
        outcome: &Result<MessageResponse, ApiError>,
        model: &str,
        request_id: &str,
        started: std::time::Instant,
    ) {
        match outcome {
            Ok(_) => {
                telemetry::emit_succeeded(
                    &self.bus,
                    model,
                    request_id,
                    duration_ms_clamped(started.elapsed()),
                    200,
                )
                .await;
            }
            Err(e) => {
                telemetry::emit_failed(&self.bus, model, request_id, error_kind(e), status_of(e))
                    .await;
            }
        }
    }

    /// Build a base HTTP request with `anthropic-beta`, `user-agent`, and
    /// (if provided) `authorization: Bearer ...` headers attached.
    fn build_request_with_betas(&self, body: &Value, bearer_override: Option<&str>) -> HttpRequest {
        use crate::betas::{assemble_beta_header, Endpoint, Provider};
        let mut req = self.build_request(body);
        // Attach beta header (Anthropic / MessagesCreate non-stream by default).
        let beta = assemble_beta_header(Provider::Anthropic, Endpoint::MessagesCreate);
        if !beta.is_empty() {
            req.headers.push(("anthropic-beta".into(), beta));
        }
        req.headers.push(("user-agent".into(), user_agent()));
        // Spec §7: X-Request-Id is set per call for telemetry correlation.
        req.headers.push(("x-request-id".into(), new_request_id()));
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
        use crate::betas::{assemble_beta_header, Endpoint, Provider as BetaProvider};
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

/// Parse the rate-limit headers, emit the `tengu_api_rate_limited` event, and
/// sleep for the resolved delay (defaulting to 1s when no header was sent).
async fn handle_429(
    headers: &[(String, String)],
    bus: &Option<Arc<lingxi_telemetry::AnalyticsBus>>,
    model: &str,
) {
    let now = SystemTime::now();
    let sleep = parse_retry_after(headers)
        .or_else(|| parse_anthropic_ratelimit_reset(headers, now))
        .unwrap_or(std::time::Duration::from_secs(1));
    telemetry::emit_rate_limited(bus, model, duration_ms_clamped(sleep)).await;
    tracing::warn!(
        target: "lingxi::api_client::rate_limit",
        secs = sleep.as_secs(),
        "429 received; sleeping for {}s",
        sleep.as_secs()
    );
    tokio::time::sleep(sleep).await;
}

/// Convert a `Duration` to a `u64` millisecond count, saturating at `u64::MAX`.
/// Used for telemetry payloads where ms-resolution `i64` is the wire shape.
fn duration_ms_clamped(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Map an [`ApiError`] to the stable `error_kind` label emitted in the
/// `tengu_api_request_failed` telemetry event. Strings are spec-locked
/// vocabulary; do not edit without a matching schema update.
fn error_kind(e: &ApiError) -> &'static str {
    match e {
        ApiError::Http(_) => "http",
        ApiError::PromptTooLong { .. } => "prompt_too_long",
        ApiError::RateLimited { .. } => "rate_limited",
        ApiError::Unauthorized(_) => "unauthorized",
        ApiError::MalformedStream(_) => "malformed_stream",
        ApiError::UnexpectedStreamEnd => "stream_end",
        ApiError::Server { .. } => "server",
        ApiError::RetryExhausted { .. } => "retry_exhausted",
        ApiError::UnsupportedModel { .. } => "unsupported_model",
        ApiError::OAuthHook(_) => "oauth_hook",
    }
}

/// Extract the HTTP status code, if any, from an [`ApiError`].
fn status_of(e: &ApiError) -> Option<u16> {
    match e {
        ApiError::Server { status, .. } => Some(*status),
        ApiError::RetryExhausted { last_status } => *last_status,
        _ => None,
    }
}

/// Helpers that emit the four `tengu_api_*` telemetry events through the
/// optional `AnalyticsBus`. Each helper short-circuits when the bus is `None`
/// so test setups that don't attach a sink pay no cost.
///
/// Payload keys are spec-locked (see spec §7 telemetry table):
/// * `model`, `request_id`, `error_kind` use the [`Verified`] newtype.
/// * `status_code` becomes `AnalyticsValue::None` when absent, never omitted.
mod telemetry {
    use lingxi_telemetry::{AnalyticsBus, AnalyticsValue, LogEventMetadata, Verified};
    use std::sync::Arc;

    #[allow(
        clippy::cast_possible_wrap,
        reason = "duration_ms / retry_after_ms fit in i64 for all realistic deployments"
    )]
    pub async fn emit_started(
        bus: &Option<Arc<AnalyticsBus>>,
        model: &str,
        request_id: &str,
        stream: bool,
    ) {
        let Some(bus) = bus else { return };
        let mut m = LogEventMetadata::new();
        m.insert(
            "model".into(),
            AnalyticsValue::String(
                Verified::assert_safe(model.to_string())
                    .as_str()
                    .to_string(),
            ),
        );
        m.insert(
            "request_id".into(),
            AnalyticsValue::String(
                Verified::assert_safe(request_id.to_string())
                    .as_str()
                    .to_string(),
            ),
        );
        m.insert("stream".into(), AnalyticsValue::Bool(stream));
        bus.log_event("tengu_api_request_started", m).await;
    }

    #[allow(
        clippy::cast_possible_wrap,
        reason = "duration_ms fits in i64 for all realistic deployments"
    )]
    pub async fn emit_succeeded(
        bus: &Option<Arc<AnalyticsBus>>,
        model: &str,
        request_id: &str,
        duration_ms: u64,
        status: u16,
    ) {
        let Some(bus) = bus else { return };
        let mut m = LogEventMetadata::new();
        m.insert(
            "model".into(),
            AnalyticsValue::String(
                Verified::assert_safe(model.to_string())
                    .as_str()
                    .to_string(),
            ),
        );
        m.insert(
            "request_id".into(),
            AnalyticsValue::String(
                Verified::assert_safe(request_id.to_string())
                    .as_str()
                    .to_string(),
            ),
        );
        m.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        m.insert("status".into(), AnalyticsValue::Int(i64::from(status)));
        bus.log_event("tengu_api_request_succeeded", m).await;
    }

    pub async fn emit_failed(
        bus: &Option<Arc<AnalyticsBus>>,
        model: &str,
        request_id: &str,
        error_kind: &str,
        status_code: Option<u16>,
    ) {
        let Some(bus) = bus else { return };
        let mut m = LogEventMetadata::new();
        m.insert(
            "model".into(),
            AnalyticsValue::String(
                Verified::assert_safe(model.to_string())
                    .as_str()
                    .to_string(),
            ),
        );
        m.insert(
            "request_id".into(),
            AnalyticsValue::String(
                Verified::assert_safe(request_id.to_string())
                    .as_str()
                    .to_string(),
            ),
        );
        m.insert(
            "error_kind".into(),
            AnalyticsValue::String(
                Verified::assert_safe(error_kind.to_string())
                    .as_str()
                    .to_string(),
            ),
        );
        m.insert(
            "status_code".into(),
            match status_code {
                Some(s) => AnalyticsValue::Int(i64::from(s)),
                None => AnalyticsValue::None,
            },
        );
        bus.log_event("tengu_api_request_failed", m).await;
    }

    #[allow(
        clippy::cast_possible_wrap,
        reason = "retry_after_ms fits in i64 for all realistic deployments"
    )]
    pub async fn emit_rate_limited(
        bus: &Option<Arc<AnalyticsBus>>,
        model: &str,
        retry_after_ms: u64,
    ) {
        let Some(bus) = bus else { return };
        let mut m = LogEventMetadata::new();
        m.insert(
            "model".into(),
            AnalyticsValue::String(
                Verified::assert_safe(model.to_string())
                    .as_str()
                    .to_string(),
            ),
        );
        m.insert(
            "retry_after_ms".into(),
            AnalyticsValue::Int(retry_after_ms as i64),
        );
        bus.log_event("tengu_api_rate_limited", m).await;
    }
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
        let expected = format!("claude-cli/{} (external, cli)", env!("CARGO_PKG_VERSION"));
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
