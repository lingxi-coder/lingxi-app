//! Anthropic provider — builds API requests and maps SSE events to engine
//! events.
//!
//! This module only constructs request shapes and decodes individual SSE
//! payloads; all network I/O is delegated to the `HttpTransport` trait
//! (wired in Tasks 16–18).

use crate::oauth_hook::{current_hook, OAuthRefreshHook, TokenHash};
use crate::overflow::{adjusted_max_tokens, parse_max_tokens_overflow, Overflow};
use crate::rate_limit::{parse_anthropic_ratelimit_reset, parse_retry_after, parse_unified_reset};
use crate::retry::{
    with_retry_ctl, RetryControl, DEFAULT_BASE_DELAYS_MS, DEFAULT_RETRY_BUDGET,
};
use crate::types::{MessageResponse, StreamEvent};
use crate::ApiError;
use protocol::{ConversationMessage, HttpMethod, HttpRequest};
use serde_json::Value;
use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;
use traits::HttpTransport;

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
    /// via `telemetry::AnalyticsBus`. When `None` (test-mode default),
    /// emission is silently skipped — matches M3-01 / M3-02 convention.
    bus: Option<Arc<::telemetry::AnalyticsBus>>,
    /// Optional cost tracker; when present, every successful 200 response
    /// records cost via `tracker.record_api_response_v2(...)` and (if the
    /// provider also has a `bus`) emits `tengu_cost_recorded`.
    cost_tracker: Option<Arc<cost::CostTracker>>,
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
    pub fn with_bus(mut self, bus: Arc<::telemetry::AnalyticsBus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Attach a cost tracker so successful 200 responses record cost.
    /// If the provider also has an
    /// [`AnalyticsBus`](::telemetry::AnalyticsBus) attached via
    /// [`Self::with_bus`], the tracker uses it to emit `tengu_cost_recorded`
    /// per spec §4 Flow B lines 364-376. Without a bus, the cost state
    /// still updates but no event fires.
    #[must_use]
    pub fn with_cost_tracker(mut self, tracker: Arc<cost::CostTracker>) -> Self {
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

    /// Build the JSON body for a non-streaming `POST /v1/messages` request.
    ///
    /// Produces `{"model","max_tokens","messages"}` and, mirroring
    /// [`Self::messages_create_stream`] and the [`crate::types::MessageRequest`]
    /// serde rules (`skip_serializing_if`): sets `body["system"]` only when
    /// `system.is_some()`, `body["tools"]` only when `!tools.is_empty()`, and
    /// `body["temperature"]` only when `temperature.is_some()`. Wire keys are
    /// the exact Anthropic names `"max_tokens"` / `"tools"` / `"temperature"`.
    fn build_messages_body(
        model: &str,
        system: Option<&str>,
        msgs: &[ConversationMessage],
        max_tokens: u32,
        tools: &[Value],
        temperature: Option<f32>,
    ) -> Value {
        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": msgs,
        });
        if let Some(s) = system {
            body["system"] = Value::String(s.to_string());
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }
        if let Some(t) = temperature {
            body["temperature"] = serde_json::json!(t);
        }
        // CACHE.1 + CACHE.2 — stamp Anthropic prompt-cache breakpoints at the
        // wire boundary (no-op + byte-identical when caching is disabled).
        apply_prompt_caching(&mut body, model);
        body
    }

    /// Non-streaming `POST /v1/messages` with retry + rate-limit + OAuth-hook
    /// middleware. Legacy 4-arg entrypoint preserved byte-identically for all
    /// existing callers: it forwards `max_tokens = 4096`, no tools, and no
    /// temperature. Richer callers (provider / sidequery) use
    /// [`Self::messages_create_non_stream_with_opts`] to carry the real
    /// `max_tokens` / `tools` / `temperature`.
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
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        transport: &T,
    ) -> Result<MessageResponse, ApiError> {
        self.messages_create_non_stream_with_opts(
            model,
            system,
            msgs,
            4096,
            Vec::new(),
            None,
            transport,
        )
        .await
    }

    /// Non-streaming `POST /v1/messages` carrying the full request options
    /// (`max_tokens` / `tools` / `temperature`). Same retry + rate-limit +
    /// OAuth-hook + cost middleware as [`Self::messages_create_non_stream`];
    /// only the body construction differs (it threads the supplied options via
    /// [`Self::build_messages_body`] instead of hard-coding 4096 / no-tools /
    /// no-temperature). The thin legacy wrapper above forwards the historical
    /// defaults so existing callers and integration tests stay byte-identical.
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
    // `msgs` / `tools` are taken by value to match the owned-`Vec` public
    // shape of `messages_create_non_stream` / `messages_create_stream` (and
    // every caller hands over an owned `Vec`); the body builder borrows them
    // as slices, so the by-value is intentional for API symmetry.
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    pub async fn messages_create_non_stream_with_opts<T: HttpTransport>(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        max_tokens: u32,
        tools: Vec<Value>,
        temperature: Option<f32>,
        transport: &T,
    ) -> Result<MessageResponse, ApiError> {
        // Behaviour-neutral for existing callers: no fallback model and the
        // consecutive-529 gate disabled (`RetryControl::default()` → the loop
        // reduces to the budget-only path). The fallback-aware entrypoint is
        // `messages_create_non_stream_with_fallback`.
        self.messages_create_non_stream_with_fallback(
            model,
            system,
            msgs,
            max_tokens,
            tools,
            temperature,
            None,
            false,
            false,
            transport,
        )
        .await
    }

    /// Non-streaming `POST /v1/messages` with the **consecutive-529 / Opus
    /// model-fallback** policy (Batch 2). Same retry + rate-limit + OAuth-hook +
    /// cost middleware as [`Self::messages_create_non_stream_with_opts`]; the
    /// only addition is a [`crate::retry::RetryControl`] threaded into the retry
    /// loop.
    ///
    /// 1:1 with claude-code `withRetry.ts:326-365`. After
    /// [`crate::retry::MAX_529_RETRIES`] (3) consecutive 529s on a non-custom
    /// Opus primary model — and the user is **not** a Claude.ai subscriber, OR
    /// `FALLBACK_FOR_ALL_PRIMARY_MODELS` is set — the loop stops retrying:
    ///
    /// * if `fallback_model` is `Some` → returns [`ApiError::FallbackTriggered`]
    ///   so the **orchestrator turn loop** can re-issue against the fallback
    ///   model (the orchestrator wiring is a separate, out-of-crate batch; this
    ///   api-client method only *surfaces* the signal — claude-code re-issues
    ///   via `query.ts`, not inside `withRetry`);
    /// * else if `USER_TYPE === 'external'` and `IS_SANDBOX` is unset → returns
    ///   [`ApiError::Overloaded`] `{ repeated: true }` (byte-locked
    ///   `Repeated 529 Overloaded errors`).
    ///
    /// The fallback gate is read once per request from the environment:
    /// `FALLBACK_FOR_ALL_PRIMARY_MODELS`, `USER_TYPE`, `IS_SANDBOX`
    /// (see [`resolve_retry_control`]). `is_subscriber` is resolved by the caller
    /// (Batch 6 OAuth subscription resolution): `engine_desktop::build` derives it
    /// from the OAuth token scopes via `anthropic_oauth::subscription_from_scopes`
    /// and threads it through `OrchestratorConfig`; non-OAuth sessions and tests
    /// pass `false`, so an external non-subscriber on an Opus model opens the gate
    /// exactly as TS does.
    ///
    /// `is_subscriber` / `is_enterprise` are the **pre-computed** subscription
    /// flags that gate whether a 429 is retryable: claude-code `withRetry.ts:767-769`
    /// only retries a 429 when `!isClaudeAISubscriber() || isEnterpriseSubscriber()`.
    /// A non-enterprise Claude.ai subscriber's 429 reset is hours away, so the
    /// 429 is treated as **terminal** (the loop returns the error instead of
    /// sleeping + retrying). The flags are resolved by the caller (subscription
    /// state lives outside api-client) and handed in — same seam as
    /// `allow_fallback`/`is_subscriber` on the consecutive-529 path.
    ///
    /// # Errors
    /// See [`ApiError`] — adds [`ApiError::FallbackTriggered`] /
    /// [`ApiError::Overloaded`] `{ repeated: true }` from the 529 gate, and
    /// surfaces a terminal `Server { status: 429, .. }` when the subscriber gate
    /// forbids retrying the 429.
    #[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
    pub async fn messages_create_non_stream_with_fallback<T: HttpTransport>(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        max_tokens: u32,
        tools: Vec<Value>,
        temperature: Option<f32>,
        fallback_model: Option<String>,
        is_subscriber: bool,
        is_enterprise: bool,
        transport: &T,
    ) -> Result<MessageResponse, ApiError> {
        let request_id = new_request_id();
        let started = std::time::Instant::now();
        telemetry::emit_started(&self.bus, model, &request_id, false).await;

        let body = Self::build_messages_body(model, system, &msgs, max_tokens, &tools, temperature);

        // claude-code withRetry.ts:767-769 — a 429 is retryable ONLY when the
        // user is not a Claude.ai subscriber, OR is an enterprise subscriber
        // (enterprise typically uses PAYG, not the hours-away rate-limit reset).
        // Pre-compute the gate once and thread it into the 429 caller path; when
        // `false`, the 429 is terminal rather than sleep-and-retry.
        let retry_429_allowed = !is_subscriber || is_enterprise;

        let ctl = resolve_retry_control(model, fallback_model, is_subscriber);
        let resp_result = self
            .drive_retry_loop_with_429(&body, &ctl, retry_429_allowed, transport)
            .await;
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
        let mr = cost::ModelRef {
            provider: cost::ProviderId::Anthropic,
            model: model.to_string(),
        };
        let usage = cost::Usage {
            tokens: cost::TokenUsage {
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

    /// Run the retry loop. A 429 is handled in-closure (parse
    /// `Retry-After` / `anthropic-ratelimit-requests-reset`, sleep, then return
    /// a synthesised retryable so `with_retry` re-attempts — the rate-limit
    /// handshake; see Batch 4 for the subscriber gate).
    ///
    /// A **400** `max_tokens` context-overflow error
    /// is also handled in-closure (Batch 5): the three numbers are parsed
    /// ([`crate::overflow::parse_max_tokens_overflow`]), a safe `max_tokens` is
    /// recomputed ([`crate::overflow::adjusted_max_tokens`], floor 3000 with a
    /// 1000 safety buffer, accounting for the thinking budget), the shared
    /// request body's `max_tokens` field is mutated in place, and the request is
    /// re-issued **within the same attempt** — mirroring the TS `continue`
    /// (`withRetry.ts:425`), which re-loops WITHOUT consuming a normal retry
    /// slot. If the reshrink is impossible (`available < 3000`) the original 400
    /// is surfaced unchanged (TS `throw error`, `:404`).
    ///
    /// **Every other status, including a real 529 or a streamed
    /// `overloaded_error` body, is passed through verbatim** so
    /// [`crate::retry::classify_retryable`] sees the real status/body and can
    /// tag it `Overloaded`. The supplied [`crate::retry::RetryControl`] carries
    /// the consecutive-529 / Opus-fallback policy (Batch 2); a default control
    /// (no fallback, gate disabled) reduces this to the budget-only loop.
    /// Returns the underlying `with_retry_ctl` outcome.
    async fn drive_retry_loop_with_429<T: HttpTransport>(
        &self,
        body: &Value,
        ctl: &crate::retry::RetryControl,
        retry_429_allowed: bool,
        transport: &T,
    ) -> Result<protocol::HttpResponse, ApiError> {
        let bus_for_loop = self.bus.clone();
        let model_for_loop = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        // Extended-thinking budget threaded into the overflow reshrink (0 when
        // thinking is disabled / absent), read from `thinking.budget_tokens` —
        // the wire field the Messages API uses. Mirrors TS `retryContext`
        // thinking config (`withRetry.ts:408-410`).
        let thinking_budget = body
            .get("thinking")
            .and_then(|t| {
                if t.get("type").and_then(Value::as_str) == Some("enabled") {
                    t.get("budget_tokens").and_then(Value::as_u64)
                } else {
                    None
                }
            })
            .unwrap_or(0);
        // Shared, mutable body so an overflow reshrink performed during one
        // attempt persists into subsequent attempts (the 2nd attempt sees the
        // reduced `max_tokens`). `tokio::sync::Mutex` because the closure is
        // async and the lock is held across an `.await`.
        let shared_body = Arc::new(tokio::sync::Mutex::new(body.clone()));
        // Captures the byte-faithful user-facing rate-limit message built from
        // the terminal-429 response headers. The generic loop's `Server` error
        // drops the response headers, so the message must be produced inside the
        // closure (where the headers live) and stashed here for the caller.
        let terminal_429_message: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));
        let outcome = with_retry_ctl(DEFAULT_RETRY_BUDGET, DEFAULT_BASE_DELAYS_MS, ctl, |attempt| {
            let bus = bus_for_loop.clone();
            let model_s = model_for_loop.clone();
            let shared_body = Arc::clone(&shared_body);
            let terminal_429_message = Arc::clone(&terminal_429_message);
            async move {
                // Snapshot the (possibly already-reshrunk) body for this attempt.
                let mut current = shared_body.lock().await.clone();
                let req = self.build_request_with_betas(&current, None);
                let resp = transport.request(req).await?;
                // Only 429 is intercepted: sleep on the rate-limit window, then
                // return a synthesised retryable (a 503) to trigger one more
                // attempt. We deliberately do NOT touch 529 / overloaded-body
                // responses here — passing the real status through lets the
                // classifier recognise the transient-capacity (overloaded)
                // error instead of mis-bucketing it.
                if resp.status == 429 {
                    // TS withRetry.ts:767-769: a 429 is retryable ONLY when
                    // `!isClaudeAISubscriber() || isEnterpriseSubscriber()`. The
                    // gate is pre-computed by the caller and threaded in as
                    // `retry_429_allowed`. When it is `false` (a non-enterprise
                    // Claude.ai subscriber whose reset is hours away), the 429 is
                    // TERMINAL: build the byte-faithful user-facing message from
                    // the response headers (TS `getRateLimitErrorMessage`, the
                    // 429 branch of `getAssistantMessageFromError`, errors.ts:519),
                    // stash it for the caller, then pass the real 429 through so
                    // the generic loop classifies it `Fallthrough` →
                    // `Server { status: 429, .. }`. The caller swaps in the
                    // stashed message instead of sleeping + retrying.
                    if !retry_429_allowed {
                        let info = crate::rate_limit::RateLimitInfo::from_headers(&resp.headers);
                        if info.has_unified_headers() {
                            // Render the reset clauses from the real reset
                            // timestamps (`anthropic-ratelimit-unified-reset` /
                            // `…-overage-reset`) via the `formatResetTime` port,
                            // matching claude-code `getLimitReachedText`. `formatted`
                            // owns the strings; `ResetTimes` borrows from it.
                            let formatted =
                                crate::rate_limit::formatted_reset_times_from_headers(&resp.headers);
                            if let Some(msg) = crate::rate_limit::rate_limit_error_message(
                                &info,
                                &formatted.as_reset_times(),
                                crate::rate_limit::SubscriptionContext::default(),
                            ) {
                                *terminal_429_message.lock().unwrap() = Some(msg);
                            }
                        }
                        return Ok(resp);
                    }
                    handle_429(&resp.headers, &bus, &model_s).await;
                    return Ok(protocol::HttpResponse {
                        status: 503,
                        headers: Vec::new(),
                        body: String::new(),
                    });
                }
                // 400 `max_tokens` context overflow: reshrink `max_tokens` and
                // re-issue WITHIN this attempt (TS `continue`, withRetry.ts:425).
                // The API will not overflow again after a correct reshrink, so a
                // single in-attempt re-issue is bounded; if the re-issued
                // request somehow returns the same overflow, the generic loop
                // re-classifies it and the original 400 surfaces.
                if let Some(overflow) = parse_max_tokens_overflow(resp.status, &resp.body) {
                    if let Some(adjusted) = adjusted_max_tokens(overflow, thinking_budget) {
                        // Mutate the shared body so later attempts also see the
                        // reduced cap, then re-issue immediately.
                        {
                            let mut guard = shared_body.lock().await;
                            guard["max_tokens"] = Value::from(adjusted);
                            current = guard.clone();
                        }
                        tracing::warn!(
                            target: "lingxi::api_client::overflow",
                            input_tokens = overflow.input_tokens,
                            context_limit = overflow.context_limit,
                            adjusted_max_tokens = adjusted,
                            "max_tokens context overflow; reshrinking and retrying"
                        );
                        // Telemetry parity: `tengu_max_tokens_context_overflow_adjustment`
                        // (withRetry.ts:418) is emitted via the bus, not byte-compared.
                        emit_max_tokens_overflow_adjustment(
                            &bus,
                            &model_s,
                            overflow,
                            adjusted,
                            attempt,
                        )
                        .await;
                        let req = self.build_request_with_betas(&current, None);
                        return transport.request(req).await.map_err(Into::into);
                    }
                    // available < 3000 → cannot reshrink; surface the original
                    // 400 unchanged (TS `throw error`, withRetry.ts:404). The
                    // generic loop classifies this 400 as Terminal (its body no
                    // longer reshrinkable here) → ApiError::Server { 400, .. }.
                }
                Ok(resp)
            }
        })
        .await;

        // Subscriber-gated terminal 429: the closure stashed the byte-faithful
        // user-facing rate-limit message. Swap it into the `Server { 429 }` body
        // so the caller surfaces the exact claude-code message (the 429 branch
        // of `getAssistantMessageFromError`) instead of the raw response body.
        if let Err(ApiError::Server { status: 429, .. }) = &outcome {
            if let Some(msg) = terminal_429_message.lock().unwrap().take() {
                return Err(ApiError::Server {
                    status: 429,
                    body: msg,
                });
            }
        }
        outcome
    }

    /// Map the retry-loop outcome into the public response: parse 2xx bodies,
    /// drive the 401 → refresh path, and pass other errors through.
    async fn resolve_outcome<T: HttpTransport>(
        &self,
        resp_result: Result<protocol::HttpResponse, ApiError>,
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
            // Reclassify a prompt-too-long rejection (HTTP 413, or any non-2xx
            // whose body says "prompt is too long" — Anthropic returns it as a
            // 400) into the typed `ApiError::PromptTooLong` the orchestrator's
            // reactive PTL-recovery loop matches on. Without this the variant is
            // never constructed and recovery is dead code (TS classifies these
            // 400/413 bodies — `errors.ts:62-118`).
            Err(other) => Err(crate::prompt_too_long::reclassify_prompt_too_long(other)),
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
        // Pass the SHA-256 of the token that authenticated the request that just
        // 401'd (the `x-api-key`/Bearer value in `self.api_key`) as
        // `prev_token_hash`, NOT an all-zero sentinel. This makes the OAuth
        // hook's single-flight double-check behave like claude-code
        // `handleOAuth401ErrorImpl(failedAccessToken)` (auth.ts:1373-1391):
        //   * stored token still equals the failed one (not rotated) → hashes
        //     match → the hook performs a real HTTP refresh + we retry with the
        //     fresh token (TS `checkAndRefreshOAuthTokenIfNeeded(0, true)`);
        //   * another task already rotated it → hashes differ → the hook returns
        //     the rotated token without a redundant HTTP refresh (TS
        //     "recovered from keychain").
        // The old `TokenHash([0u8; 32])` always mismatched the (SHA-256, never
        // zero) stored hash, so the hook returned the SAME already-rejected
        // token and the retry 401'd again — the OAUTHREF.1 gap. When OAuth is
        // not the active mode the registered hook is `NoOpOAuthHook`, which
        // ignores this argument, so the change is a no-op outside the OAuth path.
        let result = match hook.refresh(self.failed_token_hash()).await {
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

    /// SHA-256 of the token that authenticated the request that just got a 401
    /// (the `x-api-key`/Bearer value held in `self.api_key`). Fed to the OAuth
    /// hook as `prev_token_hash` so its single-flight double-check can tell
    /// whether the token was already rotated by another task.
    ///
    /// Uses the SAME hashing scheme as `anthropic_oauth::TokenInfo::token_hash`
    /// (raw SHA-256 over the access-token bytes), so the value is byte-for-byte
    /// comparable against the hook's stored `TokenHash`. We re-implement it here
    /// rather than reuse that method because api-client cannot depend on
    /// anthropic-oauth (it would be a dependency cycle — anthropic-oauth depends
    /// on api-client for the `OAuthRefreshHook` trait).
    fn failed_token_hash(&self) -> TokenHash {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.api_key.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        TokenHash(digest)
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

    fn bearer_to_header(&self, token: &protocol::Secret<String>) -> String {
        let _ = self; // suppress dead-code lint when impl is empty.
        token.expose_secret().clone()
    }

    /// Streaming `POST /v1/messages` — opens the SSE channel and yields
    /// wire-decoded [`StreamEvent`] values until the server emits
    /// `message_stop` (or the transport errors).
    ///
    /// Unlike [`Self::messages_create_non_stream`], retry / 401-refresh /
    /// 429-handling are NOT performed here — streaming connections that
    /// drop mid-flight surface as `ApiError::Http(_)` and the caller
    /// must decide. The first-connection handshake (i.e. the response
    /// status line) IS subject to the transport's own connect timeout.
    ///
    /// `tools` is the wire-format tool schema array. In M5-04 callers
    /// pass `Vec::new()`; M5-09 wires the real tools list.
    ///
    /// # Errors
    /// * Connect-time failures surface as [`ApiError::Http`].
    /// * Each subsequent decode failure surfaces as
    ///   [`ApiError::MalformedStream`] inside the stream's items.
    pub async fn messages_create_stream<T: HttpTransport + Send + Sync + 'static>(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<Value>,
        transport: Arc<T>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<crate::types::StreamEvent, ApiError>>,
        ApiError,
    > {
        use futures::stream::StreamExt;

        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": 4096u32,
            "messages": msgs,
        });
        if let Some(s) = system {
            body["system"] = Value::String(s.to_string());
        }
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools);
        }
        // CACHE.1 + CACHE.2 — same wire-boundary cache stamping as the
        // non-streaming builder (shared helper keeps both paths in lockstep).
        apply_prompt_caching(&mut body, model);

        let req = self.build_streaming_request(&body);
        let wire_stream = transport.stream_sse(req).await?;
        let typed = wire_stream.map(|item| match item {
            Ok(sse) => serde_json::from_str::<crate::types::StreamEvent>(&sse.data).map_err(|e| {
                ApiError::MalformedStream(format!(
                    "StreamEvent decode failed: {e}: data={}",
                    sse.data
                ))
            }),
            Err(e) => Err(ApiError::Http(e)),
        });
        Ok(typed.boxed())
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
    bus: &Option<Arc<::telemetry::AnalyticsBus>>,
    model: &str,
) {
    let now = SystemTime::now();
    // Delay source preference (claude-code): explicit retry-after, then the
    // unified-reset window (Max/Pro 5-hr limits), then the per-requests reset,
    // else 1s.
    let sleep = parse_retry_after(headers)
        .or_else(|| parse_unified_reset(headers, now))
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

/// Emit the `tengu_max_tokens_context_overflow_adjustment` telemetry event for
/// a parsed overflow + its recomputed cap. Thin wrapper over the `telemetry`
/// module helper so the closure in `drive_retry_loop_with_429` stays terse.
async fn emit_max_tokens_overflow_adjustment(
    bus: &Option<Arc<::telemetry::AnalyticsBus>>,
    model: &str,
    overflow: Overflow,
    adjusted: u32,
    attempt: u8,
) {
    telemetry::emit_max_tokens_overflow_adjustment(
        bus,
        model,
        overflow.input_tokens,
        overflow.context_limit,
        adjusted,
        attempt,
    )
    .await;
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
        ApiError::Overloaded { .. } => "overloaded",
        ApiError::FallbackTriggered { .. } => "fallback_triggered",
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

/// Build the [`RetryControl`] for a request from the environment + model.
///
/// Ports the gate expression at claude-code `withRetry.ts:331-332` plus the
/// external/sandbox checks at `:354-355`:
///
/// * `allow_fallback` = `FALLBACK_FOR_ALL_PRIMARY_MODELS` is set to any
///   non-empty value (TS raw truthy `||`, NOT `isEnvTruthy`), **OR**
///   (`!is_subscriber && is_non_custom_opus(model)`).
/// * `is_external` = `USER_TYPE === 'external'` (exact match).
/// * `is_sandbox` = `IS_SANDBOX` is present (TS `!process.env.IS_SANDBOX` —
///   any value, including empty, counts as sandboxed).
///
/// `fallback_model` is carried through verbatim; the [`RetryControl::default`]
/// `max_529_retries` ([`crate::retry::MAX_529_RETRIES`] = 3) is used.
///
/// Env is read once per request (not cached) to honour mid-process overrides in
/// tests; the values are tiny and the read is off the hot path.
fn resolve_retry_control(
    model: &str,
    fallback_model: Option<String>,
    is_subscriber: bool,
) -> RetryControl {
    // TS raw `process.env.FALLBACK_FOR_ALL_PRIMARY_MODELS ||` — truthy means a
    // non-empty string. An empty string is falsy in JS, so match that.
    let fallback_for_all = std::env::var("FALLBACK_FOR_ALL_PRIMARY_MODELS")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let allow_fallback =
        fallback_for_all || (!is_subscriber && crate::opus::is_non_custom_opus(model));
    let is_external = std::env::var("USER_TYPE").as_deref() == Ok("external");
    // TS `!process.env.IS_SANDBOX` — present (defined) is sandboxed, regardless
    // of value. `var()` returns Ok for any defined value including empty.
    let is_sandbox = std::env::var("IS_SANDBOX").is_ok();
    RetryControl {
        fallback_model,
        primary_model: model.to_string(),
        allow_fallback,
        is_external,
        is_sandbox,
        ..RetryControl::default()
    }
}

/// CACHE.1 + CACHE.2 — post-process the serialized request `Value` to add
/// Anthropic prompt-cache breakpoints at the wire boundary. The frozen
/// `protocol::ContentBlock` carries no `cache_control` field, so the markers
/// are stamped onto the serialized JSON instead of the typed DTOs.
///
/// Ports claude-code:
/// * `buildSystemPromptBlocks` (claude.ts:3213-3237) — the `system` field is
///   ALWAYS sent as an array of `{type:'text', text}` blocks, and with caching
///   enabled each (non-null-scope) block carries `cache_control`. Here the
///   whole system prompt is a single block, so it becomes one text block.
/// * `addCacheBreakpoints` (claude.ts:3089-3106) — stamps `cache_control` onto
///   the LAST content block of the marker message. We implement the
///   `skipCacheWrite=false` base case, so the marker is the LAST message.
/// * `userMessageToMessageParam` / `assistantMessageToMessageParam`
///   (claude.ts:588-674) — a string `content` becomes a single text block, and
///   for the assistant path the trailing `thinking` / `redacted_thinking`
///   (and connector-text) blocks are EXCLUDED from being the cache target.
///
/// Only the BASE marker `{ "type": "ephemeral" }` is applied. The CACHE.3
/// `ttl` / `scope` / `cache_reference` extensions are out of scope (and the
/// frozen `ContentBlock` cannot carry a `cache_reference`). When caching is
/// disabled the body is left byte-identical to today (system stays a String).
fn apply_prompt_caching(body: &mut Value, model: &str) {
    if !prompt_caching_enabled(model) {
        return;
    }

    // CACHE.1 — `system`: String → [ { type:text, text, cache_control } ].
    // An absent/null/already-array system is left untouched (TS only ever
    // emits the blocks it actually has).
    if let Some(Value::String(s)) = body.get("system") {
        let text = s.clone();
        body["system"] = serde_json::json!([
            { "type": "text", "text": text, "cache_control": ephemeral() }
        ]);
    }

    // CACHE.2 — the marker message is the LAST element of `messages`.
    if let Some(Value::Array(messages)) = body.get_mut("messages") {
        if let Some(marker) = messages.last_mut() {
            stamp_marker_message(marker);
        }
    }
}

/// Stamp `cache_control: {type:ephemeral}` onto the last *eligible* content
/// block of a single (serialized) message. String content is first wrapped in
/// a one-element text block, matching `userMessageToMessageParam` /
/// `assistantMessageToMessageParam` (claude.ts:595-607 / 640-652).
fn stamp_marker_message(marker: &mut Value) {
    let is_assistant = marker.get("role").and_then(Value::as_str) == Some("assistant");
    match marker.get_mut("content") {
        // String content → wrap into a single text block carrying the marker.
        Some(content) if content.is_string() => {
            let text = content.as_str().unwrap_or_default().to_string();
            *content = serde_json::json!([
                { "type": "text", "text": text, "cache_control": ephemeral() }
            ]);
        }
        // Array content → stamp the last eligible block in place.
        Some(Value::Array(blocks)) => {
            if let Some(i) = last_eligible_block_index(blocks, is_assistant) {
                if let Some(obj) = blocks[i].as_object_mut() {
                    obj.insert("cache_control".to_string(), ephemeral());
                }
            }
        }
        _ => {}
    }
}

/// Index of the content block that should receive the `cache_control` marker.
///
/// * user / non-assistant role → the LAST block (TS stamps it unconditionally,
///   claude.ts:611-618).
/// * assistant role → the last block that is NOT `thinking` /
///   `redacted_thinking` (claude.ts:658-661 excludes those — and connector-text,
///   which has no representable equivalent in the frozen `ContentBlock`). If
///   every block is excluded, returns `None` (nothing is stamped).
fn last_eligible_block_index(blocks: &[Value], is_assistant: bool) -> Option<usize> {
    if blocks.is_empty() {
        return None;
    }
    if !is_assistant {
        return Some(blocks.len() - 1);
    }
    blocks.iter().rposition(|b| {
        !matches!(
            b.get("type").and_then(Value::as_str),
            Some("thinking" | "redacted_thinking")
        )
    })
}

/// The base prompt-cache marker value `{ "type": "ephemeral" }`.
fn ephemeral() -> Value {
    serde_json::json!({ "type": "ephemeral" })
}

/// Port of claude-code `getPromptCachingEnabled` (claude.ts:333-356).
///
/// Caching defaults ON. The global `DISABLE_PROMPT_CACHING` gate (parsed with
/// `isEnvTruthy` semantics) turns it off for ALL models. The model-specific
/// gates (`DISABLE_PROMPT_CACHING_HAIKU` / `_SONNET` / `_OPUS`) require the
/// resolved small-fast / default-sonnet / default-opus model ids, whose
/// resolvers (`getSmallFastModel` / `getDefaultSonnetModel` /
/// `getDefaultOpusModel`) are NOT reachable from `api-client` — they live in
/// the config layer. Those three gates are a deliberate follow-up; only the
/// global gate is implemented here (it dominates real usage).
fn prompt_caching_enabled(_model: &str) -> bool {
    !env_truthy("DISABLE_PROMPT_CACHING")
}

/// Port of `isEnvTruthy` (envUtils.ts:32-37): lowercase + trim, then `true`
/// iff the value is one of `1` / `true` / `yes` / `on`. An unset var is falsy.
fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Helpers that emit the four `tengu_api_*` telemetry events through the
/// optional `AnalyticsBus`. Each helper short-circuits when the bus is `None`
/// so test setups that don't attach a sink pay no cost.
///
/// Payload keys are spec-locked (see spec §7 telemetry table):
/// * `model`, `request_id`, `error_kind` use the [`Verified`] newtype.
/// * `status_code` becomes `AnalyticsValue::None` when absent, never omitted.
mod telemetry {
    use std::sync::Arc;
    use telemetry::{AnalyticsBus, AnalyticsValue, LogEventMetadata, Verified};

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

    /// Emit `tengu_max_tokens_context_overflow_adjustment` (claude-code
    /// `withRetry.ts:418`) with the parsed input/limit and the recomputed cap.
    /// The bus payload is not byte-compared (Batch 5 fidelity note), but the
    /// event name and field keys mirror the TS `logEvent` call.
    #[allow(
        clippy::cast_possible_wrap,
        reason = "token counts and attempt index fit in i64 for all realistic deployments"
    )]
    pub async fn emit_max_tokens_overflow_adjustment(
        bus: &Option<Arc<AnalyticsBus>>,
        model: &str,
        input_tokens: u64,
        context_limit: u64,
        adjusted_max_tokens: u32,
        attempt: u8,
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
            "inputTokens".into(),
            AnalyticsValue::Int(input_tokens as i64),
        );
        m.insert(
            "contextLimit".into(),
            AnalyticsValue::Int(context_limit as i64),
        );
        m.insert(
            "adjustedMaxTokens".into(),
            AnalyticsValue::Int(i64::from(adjusted_max_tokens)),
        );
        m.insert("attempt".into(), AnalyticsValue::Int(i64::from(attempt)));
        bus.log_event("tengu_max_tokens_context_overflow_adjustment", m)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::HttpMethod;

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

/// OAUTHREF.1 regression coverage for the reactive-401 → OAuth-refresh path.
///
/// These tests drive `messages_create_non_stream` against a scripted transport
/// (`401` then `200`) and a hook that reproduces `RefreshDriver::refresh`'s
/// single-flight double-check, asserting that `refresh_and_retry_401` now feeds
/// the hook the hash of the *failed* token instead of an all-zero sentinel:
///   * unrotated token → the hashes match → a real HTTP refresh fires;
///   * already-rotated token → the hashes differ → no redundant refresh.
#[cfg(test)]
mod reactive_401_refresh_tests {
    use super::AnthropicProvider;
    use crate::oauth_hook::{BearerToken, OAuthHookError, OAuthRefreshHook, TokenHash};
    use protocol::{
        ContentBlock, ConversationMessage, HttpRequest, HttpResponse, MessageId, Secret,
    };
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::{Arc, Mutex};
    use traits::http::SseStream;
    use traits::{HttpError, HttpTransport};

    const OK_BODY: &str = r#"{"id":"msg_01","model":"claude-opus-4-6","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}"#;

    /// SHA-256 over the token bytes — identical scheme to
    /// `AnthropicProvider::failed_token_hash` and
    /// `anthropic_oauth::TokenInfo::token_hash`, so the mock hook compares the
    /// same way `RefreshDriver` does.
    fn sha256_token(token: &str) -> TokenHash {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(token.as_bytes());
        let digest: [u8; 32] = h.finalize().into();
        TokenHash(digest)
    }

    fn make_msgs() -> Vec<ConversationMessage> {
        vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }]
    }

    /// Scripted transport: pops a `(status, body)` per request and records the
    /// `authorization` header it saw (so the retry's bearer can be asserted).
    struct ScriptedTransport {
        responses: Mutex<VecDeque<(u16, String)>>,
        seen_auth: Mutex<Vec<Option<String>>>,
    }

    impl ScriptedTransport {
        fn new(responses: Vec<(u16, String)>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                seen_auth: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl HttpTransport for ScriptedTransport {
        async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
            let auth = req
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                .map(|(_, v)| v.clone());
            self.seen_auth.lock().unwrap().push(auth);
            let (status, body) = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted transport ran out of responses");
            Ok(HttpResponse {
                status,
                headers: vec![],
                body,
            })
        }

        async fn stream_sse(&self, _req: HttpRequest) -> Result<SseStream, HttpError> {
            unimplemented!("streaming is not exercised by the reactive-401 tests")
        }
    }

    /// Reproduces `RefreshDriver::refresh`'s double-check-after-acquire: hash the
    /// stored token and compare against `prev`. Equal → perform a (simulated)
    /// HTTP refresh and rotate; different → another task already rotated, so
    /// return the stored token without a redundant refresh.
    struct DoubleCheckHook {
        stored: tokio::sync::Mutex<String>,
        next: String,
        http_refreshes: AtomicU8,
    }

    #[async_trait::async_trait]
    impl OAuthRefreshHook for DoubleCheckHook {
        async fn refresh(&self, prev: TokenHash) -> Result<BearerToken, OAuthHookError> {
            let mut stored = self.stored.lock().await;
            if sha256_token(&stored) != prev {
                // Already rotated by another task → no HTTP call (TS
                // "recovered from keychain", auth.ts:1385-1388).
                return Ok(BearerToken(Secret::new(stored.clone())));
            }
            // Hash matches the failed token → force a real HTTP refresh + rotate
            // (TS `checkAndRefreshOAuthTokenIfNeeded(0, true)`).
            self.http_refreshes.fetch_add(1, Ordering::SeqCst);
            *stored = self.next.clone();
            Ok(BearerToken(Secret::new(stored.clone())))
        }
    }

    #[tokio::test]
    async fn unrotated_token_forces_http_refresh_then_retries() {
        let hook = Arc::new(DoubleCheckHook {
            stored: tokio::sync::Mutex::new("expired-oauth-token".to_string()),
            next: "fresh-oauth-token".to_string(),
            http_refreshes: AtomicU8::new(0),
        });
        // `api_key` is the still-current OAuth token the rejected request used.
        let provider =
            AnthropicProvider::new("expired-oauth-token", None).with_oauth_hook(hook.clone());
        let transport = ScriptedTransport::new(vec![
            (401, "token expired".to_string()),
            (200, OK_BODY.to_string()),
        ]);

        let r = provider
            .messages_create_non_stream("claude-opus-4-6", None, make_msgs(), &transport)
            .await;

        assert!(r.is_ok(), "401 → refresh → 200 must succeed: {r:?}");
        // Exactly one REAL HTTP refresh — proving we no longer no-op return the
        // already-rejected token (the OAUTHREF.1 bug).
        assert_eq!(hook.http_refreshes.load(Ordering::SeqCst), 1);
        let seen = transport.seen_auth.lock().unwrap();
        assert_eq!(seen.len(), 2, "one 401 request + one retry");
        assert_eq!(seen[0], None, "initial request uses x-api-key (no bearer)");
        assert_eq!(
            seen[1].as_deref(),
            Some("Bearer fresh-oauth-token"),
            "retry must carry the freshly-refreshed bearer",
        );
    }

    #[tokio::test]
    async fn already_rotated_token_skips_redundant_refresh() {
        // Another task rotated the stored token before this 401 was handled.
        let hook = Arc::new(DoubleCheckHook {
            stored: tokio::sync::Mutex::new("rotated-by-another-task".to_string()),
            next: "unused".to_string(),
            http_refreshes: AtomicU8::new(0),
        });
        let provider =
            AnthropicProvider::new("expired-oauth-token", None).with_oauth_hook(hook.clone());
        let transport = ScriptedTransport::new(vec![
            (401, "token expired".to_string()),
            (200, OK_BODY.to_string()),
        ]);

        let r = provider
            .messages_create_non_stream("claude-opus-4-6", None, make_msgs(), &transport)
            .await;

        assert!(r.is_ok(), "must recover with the already-rotated token: {r:?}");
        // The double-check short-circuited — no redundant HTTP refresh.
        assert_eq!(hook.http_refreshes.load(Ordering::SeqCst), 0);
        let seen = transport.seen_auth.lock().unwrap();
        assert_eq!(
            seen[1].as_deref(),
            Some("Bearer rotated-by-another-task"),
            "retry uses the token another task already rotated to",
        );
    }
}

/// CACHE.1 + CACHE.2 — request-body shape tests for prompt caching. They assert
/// the SERIALIZED wire body (not the typed DTOs): with caching on, `system`
/// becomes an array of one text block carrying `cache_control:{type:ephemeral}`
/// and the marker (last) message's last eligible block carries the same; with
/// `DISABLE_PROMPT_CACHING` set the body is byte-identical to the pre-CACHE
/// output. Both the non-streaming (`build_messages_body`) and streaming
/// (`messages_create_stream`) builders are exercised.
#[cfg(test)]
// These tests serialize on ENV_LOCK and intentionally hold the guard across the
// async request call so DISABLE_PROMPT_CACHING stays set for the whole turn.
#[allow(clippy::await_holding_lock)]
mod prompt_caching_tests {
    use super::{
        apply_prompt_caching, last_eligible_block_index, prompt_caching_enabled, AnthropicProvider,
    };
    use protocol::{ContentBlock, ConversationMessage, HttpRequest, HttpResponse, MessageId};
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};
    use traits::http::SseStream;
    use traits::{HttpError, HttpTransport};

    // DISABLE_PROMPT_CACHING is process-global; serialize the env-sensitive
    // tests so a `set_var` in one can't race the default-on assertions in
    // another running concurrently in the same test binary.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn user_msgs() -> Vec<ConversationMessage> {
        vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        }]
    }

    fn assert_ephemeral(v: &Value) {
        assert_eq!(v["cache_control"]["type"], "ephemeral", "block: {v}");
    }

    /// Capturing transport: records the request body it is handed and returns an
    /// empty SSE stream. The streaming builder sends the request inside
    /// `stream_sse`, so the body is captured even though the stream is dropped.
    struct CapturingTransport {
        body: Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl HttpTransport for CapturingTransport {
        async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
            unimplemented!("non-streaming path is not exercised by these tests")
        }
        async fn stream_sse(&self, req: HttpRequest) -> Result<SseStream, HttpError> {
            *self.body.lock().unwrap() = req.body.clone();
            let stream: SseStream = Box::pin(futures::stream::empty());
            Ok(stream)
        }
    }

    // (a) Non-streaming builder, caching ON by default.
    #[test]
    fn non_stream_enabled_stamps_system_and_last_block() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("DISABLE_PROMPT_CACHING");

        let msgs = user_msgs();
        let body =
            AnthropicProvider::build_messages_body("claude-x", Some("SYS"), &msgs, 4096, &[], None);

        // CACHE.1: system is an array of one text block with cache_control.
        let sys = &body["system"];
        assert!(sys.is_array(), "system must be an array, got {sys}");
        assert_eq!(sys[0]["type"], "text");
        assert_eq!(sys[0]["text"], "SYS");
        assert_ephemeral(&sys[0]);

        // CACHE.2: the marker (last) message's last block carries cache_control.
        let last_block = &body["messages"][0]["content"][0];
        assert_eq!(last_block["type"], "text");
        assert_ephemeral(last_block);
    }

    // (a') Streaming builder, caching ON by default.
    #[tokio::test]
    async fn stream_enabled_stamps_system_and_last_block() {
        let body_str = {
            let _g = ENV_LOCK.lock().unwrap();
            std::env::remove_var("DISABLE_PROMPT_CACHING");

            let provider = AnthropicProvider::new("k", None);
            let transport = Arc::new(CapturingTransport {
                body: Mutex::new(None),
            });
            let _stream = provider
                .messages_create_stream(
                    "claude-x",
                    Some("SYS"),
                    user_msgs(),
                    Vec::new(),
                    transport.clone(),
                )
                .await
                .expect("stream handshake");
            let captured = transport
                .body
                .lock()
                .unwrap()
                .clone()
                .expect("request body captured");
            captured
        };

        let body: Value = serde_json::from_str(&body_str).unwrap();
        assert_eq!(body["stream"], true, "streaming flag still set");
        let sys = &body["system"];
        assert!(sys.is_array(), "system must be an array, got {sys}");
        assert_eq!(sys[0]["text"], "SYS");
        assert_ephemeral(&sys[0]);
        assert_ephemeral(&body["messages"][0]["content"][0]);
    }

    // (b) DISABLE_PROMPT_CACHING set → byte-identical to the pre-CACHE body.
    #[test]
    fn disabled_is_byte_identical_non_stream() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("DISABLE_PROMPT_CACHING", "1");

        let msgs = user_msgs();
        let actual =
            AnthropicProvider::build_messages_body("claude-x", Some("SYS"), &msgs, 4096, &[], None);

        // Reconstruct exactly what the builder produced before CACHE.1/CACHE.2.
        let mut expected = json!({
            "model": "claude-x",
            "max_tokens": 4096u32,
            "messages": &msgs,
        });
        expected["system"] = Value::String("SYS".into());

        std::env::remove_var("DISABLE_PROMPT_CACHING");

        assert_eq!(actual, expected, "disabled path must be byte-identical");
        assert!(actual["system"].is_string(), "system stays a plain string");
        assert!(
            !actual.to_string().contains("cache_control"),
            "no cache_control anywhere when disabled",
        );
    }

    // (b') Streaming disabled → system stays a plain string, no cache_control.
    #[tokio::test]
    async fn disabled_is_byte_identical_stream() {
        let body_str = {
            let _g = ENV_LOCK.lock().unwrap();
            std::env::set_var("DISABLE_PROMPT_CACHING", "yes");
            let provider = AnthropicProvider::new("k", None);
            let transport = Arc::new(CapturingTransport {
                body: Mutex::new(None),
            });
            let _ = provider
                .messages_create_stream(
                    "claude-x",
                    Some("SYS"),
                    user_msgs(),
                    Vec::new(),
                    transport.clone(),
                )
                .await
                .expect("stream handshake");
            let b = transport
                .body
                .lock()
                .unwrap()
                .clone()
                .expect("request body captured");
            std::env::remove_var("DISABLE_PROMPT_CACHING");
            b
        };
        let body: Value = serde_json::from_str(&body_str).unwrap();
        assert!(body["system"].is_string(), "system stays a plain string");
        assert!(
            !body_str.contains("cache_control"),
            "no cache_control when disabled",
        );
    }

    // (c) Assistant marker ending in a thinking block → cache_control lands on
    // the last NON-thinking block (the trailing thinking block is excluded).
    #[test]
    fn assistant_trailing_thinking_is_excluded() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("DISABLE_PROMPT_CACHING");

        let msgs = vec![ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![
                ContentBlock::Text {
                    text: "answer".into(),
                },
                ContentBlock::Thinking {
                    thinking: "reasoning".into(),
                    signature: None,
                },
            ],
            stop_reason: None,
        }];
        let body =
            AnthropicProvider::build_messages_body("claude-x", Some("SYS"), &msgs, 4096, &[], None);

        let content = &body["messages"][0]["content"];
        // text block (index 0) receives the marker...
        assert_eq!(content[0]["type"], "text");
        assert_ephemeral(&content[0]);
        // ...the trailing thinking block (index 1) does NOT.
        assert_eq!(content[1]["type"], "thinking");
        assert!(
            content[1].get("cache_control").is_none(),
            "trailing thinking block must be excluded",
        );
    }

    // (c') redacted_thinking is also excluded. It is not constructible via the
    // frozen ContentBlock enum, so exercise the helper on a synthetic wire body.
    #[test]
    fn redacted_thinking_is_excluded() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("DISABLE_PROMPT_CACHING");

        let mut body = json!({
            "model": "claude-x",
            "messages": [{
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "a" },
                    { "type": "redacted_thinking", "data": "xx" }
                ]
            }]
        });
        apply_prompt_caching(&mut body, "claude-x");
        let content = &body["messages"][0]["content"];
        assert_ephemeral(&content[0]);
        assert!(content[1].get("cache_control").is_none());
    }

    // String content (e.g. a degenerate string-content message) is wrapped into
    // a single text block, matching userMessageToMessageParam's string branch.
    #[test]
    fn string_content_marker_is_wrapped() {
        let mut body = json!({
            "messages": [ { "role": "user", "content": "raw string" } ]
        });
        super::stamp_marker_message(&mut body["messages"][0]);
        let content = &body["messages"][0]["content"];
        assert!(content.is_array(), "string content must be wrapped: {content}");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "raw string");
        assert_ephemeral(&content[0]);
    }

    // Unit coverage for the eligibility walk-back used by CACHE.2.
    #[test]
    fn eligibility_index_rules() {
        let user = vec![json!({"type":"text"}), json!({"type":"text"})];
        assert_eq!(last_eligible_block_index(&user, false), Some(1));

        let asst_trailing_text = vec![json!({"type":"thinking"}), json!({"type":"text"})];
        assert_eq!(last_eligible_block_index(&asst_trailing_text, true), Some(1));

        let asst_trailing_thinking = vec![json!({"type":"text"}), json!({"type":"thinking"})];
        assert_eq!(last_eligible_block_index(&asst_trailing_thinking, true), Some(0));

        let all_excluded = vec![
            json!({"type":"thinking"}),
            json!({"type":"redacted_thinking"}),
        ];
        assert_eq!(last_eligible_block_index(&all_excluded, true), None);

        let empty: &[Value] = &[];
        assert_eq!(last_eligible_block_index(empty, false), None);
    }

    // The enable gate reads DISABLE_PROMPT_CACHING with isEnvTruthy semantics.
    #[test]
    fn enable_gate_reads_disable_env() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        assert!(prompt_caching_enabled("claude-x"));
        std::env::set_var("DISABLE_PROMPT_CACHING", "TRUE"); // case-insensitive
        assert!(!prompt_caching_enabled("claude-x"));
        std::env::set_var("DISABLE_PROMPT_CACHING", "0"); // not in the truthy set
        assert!(prompt_caching_enabled("claude-x"));
        std::env::remove_var("DISABLE_PROMPT_CACHING");
    }
}
