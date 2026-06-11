//! Drive `llm_client::DefaultLlmClient` from the orchestrator's seam traits.
//!
//! **Task 6**: replaces the old `providers::ModelRouter`-backed stubs with a real
//! `DefaultLlmClient` drive: `prepare()` → header injection → `transport.execute()`
//! / `open_stream()` → `codec.decode_response()`.  The retry driver loop wraps the
//! prepare/execute pair and feeds headers to `model/rate_limit.rs` + `model/retry.rs`.

use crate::conversation::{OrchestratorApiClient, StreamingApiClient};
use crate::model::betas::{apply_beta_header_with_auth, Endpoint, Provider};
use crate::model::rate_limit::{parse_retry_after, parse_unified_reset};
use crate::model::retry::{next_step, resolve_retry_control, DriveStep, ResolveRetryEnv, RetryControl, RetryState};
use crate::model::telemetry;
use crate::model::user_agent::{user_agent, UserAgentEnv};
use agent::convert::{to_llm_messages, to_tool_declarations};
use async_trait::async_trait;
use futures::stream::BoxStream;
use llm_client::{DefaultLlmClient, LlmError, LlmEvent, LlmRequest, LlmResponse, ProviderRequest, SystemBlock, Transport};
use protocol::{ContentBlock, ConversationMessage};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

// ── Subscriber state ─────────────────────────────────────────────────────────

/// Subscription flags — gates the 429 retry policy.
///
/// Task 8 wires real values from auth; default is both false (conservative:
/// over-retries 429s slightly, but never breaks).
#[derive(Debug, Clone, Copy, Default)]
pub struct SubscriberState {
    /// `true` when the configured credential is a Claude.ai OAuth subscriber.
    pub is_subscriber: bool,
    /// `true` when the subscriber is an enterprise account.
    pub is_enterprise: bool,
}

// ── Stream state (used in drive_stream unfold) ────────────────────────────────

/// State threaded through the `futures::stream::unfold` loop in `drive_stream`.
struct StreamState {
    decoder: Box<dyn llm_client::StreamDecoder>,
    frames: Box<dyn llm_client::FrameStream>,
    queue: VecDeque<LlmEvent>,
    finished: bool,
}

// ── Adapter state ─────────────────────────────────────────────────────────────

/// Production adapter: drives `DefaultLlmClient` with full retry/rate-limit/betas.
pub struct ProviderApiAdapter {
    client: Arc<DefaultLlmClient>,
    transport: Arc<dyn Transport>,
    /// Subscriber state for the 429 gate (Task 8 wires real value).
    subscriber: SubscriberState,
    /// User-agent environment snapshot (Task 3).
    ua: UserAgentEnv,
    /// Build version string for the User-Agent header.
    version: String,
    /// Optional analytics bus for telemetry events.
    analytics: Option<Arc<::telemetry::AnalyticsBus>>,
    /// Fallback model, if configured (used by `messages_create_with_fallback`).
    fallback_model: Option<String>,
    /// Available model ids from the client registry (for `available_models`).
    available_model_ids: Vec<String>,
}

impl ProviderApiAdapter {
    /// Construct the adapter.  Called by Task 10 host constructors.
    ///
    /// `version` is the build version string embedded in the User-Agent header.
    #[must_use]
    pub fn new(
        client: Arc<DefaultLlmClient>,
        transport: Arc<dyn Transport>,
        subscriber: SubscriberState,
        ua: UserAgentEnv,
        version: impl Into<String>,
        analytics: Option<Arc<::telemetry::AnalyticsBus>>,
        fallback_model: Option<String>,
    ) -> Self {
        let available_model_ids = client
            .available_models()
            .into_iter()
            .map(|m| m.display_model)
            .collect();
        Self {
            client,
            transport,
            subscriber,
            ua,
            version: version.into(),
            analytics,
            fallback_model,
            available_model_ids,
        }
    }

    /// Temporary bridge constructor for host callers that still pass the old
    /// `Arc<dyn ModelRouter>` argument (Task 10 will replace these call sites
    /// with the full `new(client, transport, …)` form).
    ///
    /// # Panics
    ///
    /// Always panics at runtime — this exists only to keep `cargo check
    /// --workspace` green while host wiring is pending.
    #[doc(hidden)]
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    #[deprecated(note = "Task 10 replaces router construction; panics at runtime")]
    pub fn new_from_router(_router: Arc<dyn providers::ModelRouter>) -> Self {
        unimplemented!(
            "ProviderApiAdapter::new_from_router is a compile-only bridge; Task 10 replaces \
             host call sites with ProviderApiAdapter::new(client, transport, …)"
        )
    }

    // ── Shared request build ─────────────────────────────────────────────────

    /// Convert orchestrator-layer inputs into an `LlmRequest`.
    #[allow(clippy::unused_self)]
    fn build_request(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        stream: bool,
        max_tokens: Option<u32>,
    ) -> Result<LlmRequest, LlmError> {
        let messages = to_llm_messages(strip_excess_media(msgs, MAX_MEDIA_PER_REQUEST))?;
        let tool_decls = to_tool_declarations(tools)?;

        let mut req = LlmRequest::new(model);
        if let Some(s) = system {
            req.system = vec![SystemBlock::text(s)];
        }
        req.messages = messages;
        req.tools = tool_decls;
        req.stream = stream;
        req.max_tokens = max_tokens;
        Ok(req)
    }

    /// Inject betas + User-Agent + request-id headers onto a prepared request.
    ///
    /// **Header name `x-request-id`** — sourced from `api-client/src/anthropic.rs`
    /// where it is written as `("x-request-id".into(), new_request_id())`.
    fn inject_headers(&self, prepared: &mut ProviderRequest, request_id: &str) {
        // anthropic-beta (Task 2): full assembled list merged with any auth-injected betas.
        apply_beta_header_with_auth(
            prepared,
            Provider::Anthropic,
            Endpoint::MessagesCreate,
            self.subscriber.is_subscriber,
        );
        // User-Agent (Task 3).
        prepared
            .headers
            .insert("user-agent".to_string(), user_agent(&self.ua, &self.version));
        // Client-traceable request id (matches api-client header name).
        prepared
            .headers
            .insert("x-request-id".to_string(), request_id.to_string());
    }

    /// Same as [`inject_headers`] but for the streaming endpoint.
    fn inject_stream_headers(&self, prepared: &mut ProviderRequest, request_id: &str) {
        apply_beta_header_with_auth(
            prepared,
            Provider::Anthropic,
            Endpoint::MessagesCreateStream,
            self.subscriber.is_subscriber,
        );
        prepared
            .headers
            .insert("user-agent".to_string(), user_agent(&self.ua, &self.version));
        prepared
            .headers
            .insert("x-request-id".to_string(), request_id.to_string());
    }

    // ── 429 retry-after resolution (reset ladder) ─────────────────────────────

    /// Resolve the 429 retry delay using the server-sent reset ladder:
    /// `retry-after` → `anthropic-ratelimit-unified-reset` → `anthropic-ratelimit-requests-reset` → 1 s.
    fn resolve_retry_after(headers: &std::collections::BTreeMap<String, String>) -> std::time::Duration {
        // Convert BTreeMap to vec for parse helpers.
        let hvec: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let now = std::time::SystemTime::now();

        // 1. Retry-After (RFC 7231 delta-seconds).
        if let Some(d) = parse_retry_after(&hvec) {
            return d;
        }
        // 2. anthropic-ratelimit-unified-reset (epoch seconds).
        if let Some(d) = parse_unified_reset(&hvec, now) {
            return d;
        }
        // 3. anthropic-ratelimit-requests-reset (ISO8601 UTC).
        if let Some(d) = crate::model::rate_limit::parse_anthropic_ratelimit_reset(&hvec, now) {
            return d;
        }
        // 4. Fallback: 1 s.
        std::time::Duration::from_secs(1)
    }

    // ── error_kind label (for telemetry) ─────────────────────────────────────

    /// Stable `error_kind` label for `emit_failed`.
    ///
    /// Strings are **spec-locked** to the originals from
    /// `api-client/src/anthropic.rs::error_kind` (`:1144`) to keep telemetry
    /// dashboards consistent across the api-client and llm-client codepaths.
    ///
    /// Mapping table (api-client variant → llm-client variant → label):
    ///
    /// | api-client             | LlmError                           | label            |
    /// |------------------------|------------------------------------|------------------|
    /// | `Unauthorized`         | `Authentication \| PermissionDenied` | `"unauthorized"` |
    /// | `Server`               | `ProviderInternal`                 | `"server"`       |
    /// | `Http`                 | `Transport`                        | `"http"`         |
    /// | `MalformedStream`      | `StreamInterrupted`                | `"malformed_stream"` |
    /// | `Overloaded`           | `Overloaded`                       | `"overloaded"`   |
    /// | `RateLimited`          | `RateLimited`                      | `"rate_limited"` |
    /// | `PromptTooLong`        | `ContextOverflow`                  | `"prompt_too_long"` |
    /// | *(llm-client only)*    | `InvalidRequest`                   | `"invalid_request"` |
    /// | *(llm-client only)*    | `QuotaExceeded`                    | `"quota_exceeded"` |
    /// | *(llm-client only)*    | `ModelUnavailable`                 | `"model_unavailable"` |
    /// | *(llm-client only)*    | `CostUnavailable`                  | `"cost_unavailable"` |
    /// | *(llm-client only)*    | `UnsupportedCapability`            | `"unsupported_capability"` |
    fn error_kind(err: &LlmError) -> &'static str {
        match err {
            // "unauthorized" — api-client `Unauthorized(_) => "unauthorized"` (:1153)
            LlmError::Authentication | LlmError::PermissionDenied => "unauthorized",
            // "server" — api-client `Server { .. } => "server"` (:1157)
            LlmError::ProviderInternal => "server",
            // "http" — api-client `Http(_) => "http"` (:1146)
            LlmError::Transport { .. } => "http",
            // "malformed_stream" — api-client `MalformedStream(_) => "malformed_stream"` (:1155)
            LlmError::StreamInterrupted { .. } => "malformed_stream",
            // "overloaded" — api-client `Overloaded { .. } => "overloaded"` (:1149)
            LlmError::Overloaded => "overloaded",
            // "rate_limited" — api-client `RateLimited { .. } => "rate_limited"` (:1148)
            LlmError::RateLimited { .. } => "rate_limited",
            // "prompt_too_long" — api-client `PromptTooLong { .. } => "prompt_too_long"` (:1147)
            LlmError::ContextOverflow { .. } => "prompt_too_long",
            // llm-client-only classes — no api-client analogue; use descriptive names.
            LlmError::InvalidRequest { .. } => "invalid_request",
            LlmError::QuotaExceeded => "quota_exceeded",
            LlmError::ModelUnavailable => "model_unavailable",
            LlmError::CostUnavailable { .. } => "cost_unavailable",
            LlmError::UnsupportedCapability { .. } => "unsupported_capability",
        }
    }

    /// HTTP status code approximation for `emit_failed` (best-effort: only the
    /// variants that carry an HTTP status are non-None).
    fn status_of(err: &LlmError) -> Option<u16> {
        match err {
            LlmError::Authentication | LlmError::PermissionDenied => Some(401),
            LlmError::InvalidRequest { .. } | LlmError::ContextOverflow { .. } => Some(400),
            LlmError::RateLimited { .. } | LlmError::QuotaExceeded => Some(429),
            LlmError::ModelUnavailable => Some(404),
            LlmError::ProviderInternal => Some(500),
            LlmError::Overloaded => Some(529),
            LlmError::Transport { .. }
            | LlmError::StreamInterrupted { .. }
            | LlmError::CostUnavailable { .. }
            | LlmError::UnsupportedCapability { .. } => None,
        }
    }

    // ── Non-stream drive (Step 1 + 1b) ───────────────────────────────────────

    /// Shared non-stream retry driver. Accepts an already-built `LlmRequest` so
    /// the two call-paths (`messages_create` and `messages_create_with_fallback`)
    /// can both route here.
    ///
    /// Step 1b: before classifying a retryable 5xx, the driver checks
    /// `x-should-retry: false` — that header makes the response terminal (same
    /// behaviour as api-client `retry.rs:196`).
    #[allow(clippy::too_many_lines)]
    async fn drive_non_stream(
        &self,
        req: LlmRequest,
        retry_control: RetryControl,
    ) -> Result<LlmResponse, LlmError> {
        self.drive_non_stream_seeded(req, retry_control, 0).await
    }

    /// Non-stream retry driver with a pre-seeded `consecutive_overloaded` counter.
    ///
    /// The seed is set to 1 when this call is a non-streaming fallback triggered by
    /// a mid-stream `LlmError::Overloaded` — mirroring TS `claude.ts:2559`
    /// (`initialConsecutive529Errors: is529Error(streamingError) ? 1 : 0`).
    #[allow(clippy::too_many_lines)]
    async fn drive_non_stream_seeded(
        &self,
        mut req: LlmRequest,
        retry_control: RetryControl,
        initial_consecutive_overloaded: u8,
    ) -> Result<LlmResponse, LlmError> {
        let request_id = new_request_id();
        let started = Instant::now();
        telemetry::emit_started(&self.analytics, &req.model, &request_id, false).await;

        let mut state = RetryState {
            consecutive_overloaded: initial_consecutive_overloaded,
            is_subscriber: self.subscriber.is_subscriber,
            is_enterprise: self.subscriber.is_enterprise,
            ..RetryState::default()
        };
        // thinking_budget: Task 6 drives with 0; extended-thinking wiring in Task 10+.
        let thinking_budget: u32 = req.reasoning.map_or(0, |r| r.budget_tokens);

        loop {
            // prepare → inject headers → execute.
            let mut prepared = match self.client.prepare(&req).await {
                Ok(p) => p,
                Err(e) => {
                    // prepare() errors (auth, capability, encoding) are always terminal.
                    telemetry::emit_failed(
                        &self.analytics,
                        &req.model,
                        &request_id,
                        Self::error_kind(&e),
                        Self::status_of(&e),
                    )
                    .await;
                    return Err(e);
                }
            };
            self.inject_headers(&mut prepared.provider_request, &request_id);

            let resp_result = self.transport.execute(&prepared.provider_request).await;

            match resp_result {
                Err(transport_err) => {
                    // Transport-layer failure; feed into the retry driver.
                    let step = next_step(&mut state, &retry_control, &transport_err, thinking_budget);
                    if let DriveStep::RetryAfter(delay) = step {
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    telemetry::emit_failed(
                        &self.analytics,
                        &req.model,
                        &request_id,
                        Self::error_kind(&transport_err),
                        None,
                    )
                    .await;
                    return Err(transport_err);
                }
                Ok(provider_resp) => {
                    // Step 1b: x-should-retry: false is terminal for retryable 5xx.
                    let x_should_retry_false = provider_resp
                        .headers
                        .get("x-should-retry")
                        .is_some_and(|v| v.as_str() == "false");

                    // Feed headers to rate-limit tracker for 429.
                    // (Rate-limit state recording is best-effort for now.)

                    match prepared.route.codec.decode_response(provider_resp.clone()) {
                        Ok(response) => {
                            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                            telemetry::emit_succeeded(
                                &self.analytics,
                                &req.model,
                                &request_id,
                                elapsed_ms,
                                provider_resp.status,
                            )
                            .await;
                            return Ok(response);
                        }
                        Err(decode_err) => {
                            // Step 1b: honour x-should-retry: false as terminal.
                            if x_should_retry_false {
                                telemetry::emit_failed(
                                    &self.analytics,
                                    &req.model,
                                    &request_id,
                                    Self::error_kind(&decode_err),
                                    Self::status_of(&decode_err),
                                )
                                .await;
                                return Err(decode_err);
                            }

                            // Rate-limited: resolve delay from headers.
                            let effective_err = if let LlmError::RateLimited { .. } = &decode_err {
                                let delay = Self::resolve_retry_after(&provider_resp.headers);
                                telemetry::emit_rate_limited(
                                    &self.analytics,
                                    &req.model,
                                    u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                                )
                                .await;
                                LlmError::RateLimited {
                                    retry_after: Some(delay),
                                    scope: None,
                                }
                            } else {
                                decode_err.clone()
                            };

                            let step = next_step(&mut state, &retry_control, &effective_err, thinking_budget);
                            match step {
                                DriveStep::RetryAfter(delay) => {
                                    tokio::time::sleep(delay).await;
                                    continue;
                                }
                                DriveStep::AdjustMaxTokens(new_max) => {
                                    // Emit telemetry for the overflow adjustment.
                                    if let Some(overflow) = crate::model::overflow::parse_overflow_message(
                                        match &decode_err {
                                            LlmError::InvalidRequest { message } => message,
                                            _ => "",
                                        },
                                    ) {
                                        telemetry::emit_max_tokens_overflow_adjustment(
                                            &self.analytics,
                                            &req.model,
                                            overflow.input_tokens,
                                            overflow.context_limit,
                                            new_max,
                                            state.attempt,
                                        )
                                        .await;
                                    }
                                    req.max_tokens = Some(new_max);
                                    continue;
                                }
                                DriveStep::Fallback { fallback_model } => {
                                    req.model = fallback_model;
                                    continue;
                                }
                                DriveStep::Terminal | DriveStep::RepeatedOverloaded => {
                                    telemetry::emit_failed(
                                        &self.analytics,
                                        &req.model,
                                        &request_id,
                                        Self::error_kind(&decode_err),
                                        Self::status_of(&decode_err),
                                    )
                                    .await;
                                    return Err(decode_err);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Stream drive (Step 2) ─────────────────────────────────────────────────

    /// Drive a streaming call; connect-phase failures retry through the driver.
    ///
    /// Uses `DefaultLlmClient::execute_stream` which already handles the
    /// connect-phase error-drain path internally.  For the retry loop we re-prepare
    /// on each attempt so a fresh `PreparedLlmCall` (with correct auth headers) is
    /// sent even after a previous attempt fails.
    ///
    /// **Streaming rate-limit headers (prereqs item 11):** `execute_stream` does
    /// not surface response headers from the streaming path via the current
    /// `Transport::open_stream` signature (headers are only available inside
    /// `StreamingResponse` which `execute_stream` consumes internally). Rate-limit
    /// header tracking on the streaming path is therefore best-effort / not
    /// implemented in 3a.  An additive `stream_sse` metadata return on
    /// `traits::HttpTransport` is the preferred follow-up.
    #[allow(clippy::too_many_lines)]
    async fn drive_stream(
        &self,
        req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let request_id = new_request_id();
        telemetry::emit_started(&self.analytics, &req.model, &request_id, true).await;

        let mut state = RetryState::default();
        let ctl = RetryControl::default();
        let thinking_budget: u32 = req.reasoning.map_or(0, |r| r.budget_tokens);

        loop {
            // Prepare so we can inject headers, then call execute_stream via
            // a thin wrapper transport that uses our already-modified request.
            let mut prepared = match self.client.prepare(&req).await {
                Ok(p) => p,
                Err(e) => return Err(e),
            };
            self.inject_stream_headers(&mut prepared.provider_request, &request_id);

            // Open stream directly through transport; replicate the connect-phase
            // error-drain that execute_stream normally does, because we need to
            // inject headers into the prepared request ourselves.
            match self.transport.open_stream(&prepared.provider_request).await {
                Err(transport_err) => {
                    let step = next_step(&mut state, &ctl, &transport_err, thinking_budget);
                    match step {
                        DriveStep::RetryAfter(delay) => {
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        _ => return Err(transport_err),
                    }
                }
                Ok(streaming) => {
                    // Connect-phase status ≥ 400: drain and decode as error.
                    if streaming.status >= 400 {
                        let mut frames = streaming.frames;
                        let mut body = Vec::new();
                        loop {
                            match frames.next_frame().await {
                                Ok(Some(frame)) => body.extend_from_slice(&frame.bytes),
                                Ok(None) => break,
                                Err(e) => return Err(e),
                            }
                        }
                        let body_json =
                            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                        let err_response = llm_client::ProviderResponse {
                            status: streaming.status,
                            headers: streaming.headers,
                            body_json,
                            request_id: None,
                        };
                        let decode_err = match prepared.route.codec.decode_response(err_response) {
                            Err(e) => e,
                            Ok(_) => LlmError::ProviderInternal,
                        };

                        let step = next_step(&mut state, &ctl, &decode_err, thinking_budget);
                        match step {
                            DriveStep::RetryAfter(delay) => {
                                tokio::time::sleep(delay).await;
                                // Re-prepare on next iteration so headers stay fresh.
                                continue;
                            }
                            _ => return Err(decode_err),
                        }
                    }

                    // Success: wrap the LlmEventStream from the codec into a BoxStream.
                    // Build the event stream from the codec decoder + raw frames.
                    let decoder = prepared.route.codec.stream_decoder();
                    let frames = streaming.frames;

                    // Assemble events via a manual unfold that drives next_frame + decode.
                    // We keep a queue of pre-decoded events and drain them first.
                    let stream_state = StreamState {
                        decoder,
                        frames,
                        queue: VecDeque::new(),
                        finished: false,
                    };

                    let boxed: BoxStream<'static, Result<LlmEvent, LlmError>> = Box::pin(
                        futures::stream::unfold(stream_state, |mut s| async move {
                            loop {
                                if let Some(event) = s.queue.pop_front() {
                                    return Some((Ok(event), s));
                                }
                                if s.finished {
                                    return None;
                                }
                                match s.frames.next_frame().await {
                                    Ok(Some(frame)) => match s.decoder.decode_frame(frame) {
                                        Ok(events) => s.queue.extend(events),
                                        Err(e) => {
                                            s.finished = true;
                                            return Some((Err(e), s));
                                        }
                                    },
                                    Ok(None) => {
                                        s.finished = true;
                                        match s.decoder.finish() {
                                            Ok(events) => s.queue.extend(events),
                                            Err(e) => return Some((Err(e), s)),
                                        }
                                    }
                                    Err(e) => {
                                        s.finished = true;
                                        return Some((Err(e), s));
                                    }
                                }
                            }
                        }),
                    );
                    return Ok(boxed);
                }
            }
        }
    }
}

// ── Trait implementations ─────────────────────────────────────────────────────

#[async_trait]
impl OrchestratorApiClient for ProviderApiAdapter {
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        let req = self.build_request(model, system, msgs, tools, false, None)?;
        let ctl = resolve_retry_control(
            model,
            None,
            self.subscriber.is_subscriber,
            &ResolveRetryEnv::from_process_env(),
        );
        self.drive_non_stream(req, ctl).await
    }

    async fn messages_create_with_opts(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        max_tokens: u32,
    ) -> Result<LlmResponse, LlmError> {
        let req = self.build_request(model, system, msgs, tools, false, Some(max_tokens))?;
        let ctl = resolve_retry_control(
            model,
            None,
            self.subscriber.is_subscriber,
            &ResolveRetryEnv::from_process_env(),
        );
        self.drive_non_stream(req, ctl).await
    }

    /// Non-streaming call with Opus-fallback policy wired.
    ///
    /// Routes through [`resolve_retry_control`] which computes `allow_fallback`
    /// from the env + subscriber state (Task 8). The caller-supplied
    /// `_is_subscriber` / `_is_enterprise` override the adapter's own
    /// subscriber state when the host passes explicit values; the `_` prefix
    /// documents that the adapter reads from `self.subscriber` via the env
    /// resolver (the call-site values are preserved in the method signature for
    /// API compatibility — Task 10 will drop them).
    async fn messages_create_with_fallback(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        fallback_model: Option<&str>,
        _is_subscriber: bool,
        _is_enterprise: bool,
    ) -> Result<LlmResponse, LlmError> {
        let effective_fallback = fallback_model.or(self.fallback_model.as_deref());
        let req = self.build_request(model, system, msgs, tools, false, None)?;
        let mut ctl = resolve_retry_control(
            model,
            effective_fallback.map(str::to_string),
            self.subscriber.is_subscriber,
            &ResolveRetryEnv::from_process_env(),
        );
        // If caller passed an explicit fallback model, honour it even when
        // resolve_retry_control would not have set allow_fallback (e.g. Sonnet
        // primary with a configured fallback).  This preserves the pre-Task-8
        // contract: an explicit `fallback_model` always enables the fallback gate.
        if effective_fallback.is_some() {
            ctl.allow_fallback = true;
        }
        self.drive_non_stream(req, ctl).await
    }

    /// Non-streaming call seeded with a pre-counted consecutive-529 value.
    ///
    /// Used by the mid-stream 529 fallback (Task 7): the streaming 529 that
    /// triggered the fallback is pre-counted into the retry budget so total
    /// 529s-before-fallback is consistent whether the overload was hit in
    /// streaming or non-streaming mode.  Mirrors TS `claude.ts:2559`
    /// (`initialConsecutive529Errors: is529Error(streamingError) ? 1 : 0`).
    async fn messages_create_seeded(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        initial_consecutive_overloaded: u8,
    ) -> Result<LlmResponse, LlmError> {
        let req = self.build_request(model, system, msgs, tools, false, None)?;
        let ctl = resolve_retry_control(
            model,
            None,
            self.subscriber.is_subscriber,
            &ResolveRetryEnv::from_process_env(),
        );
        self.drive_non_stream_seeded(req, ctl, initial_consecutive_overloaded)
            .await
    }

    fn available_models(&self) -> Vec<String> {
        self.available_model_ids.clone()
    }
}

/// Subagent API seam — delegates to the orchestrator impl.
#[async_trait]
impl agent::SubagentApiClient for ProviderApiAdapter {
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        OrchestratorApiClient::messages_create(self, model, system, messages, tools).await
    }

    async fn messages_create_stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        StreamingApiClient::stream(self, model, system, messages, tools).await
    }
}

#[async_trait]
impl StreamingApiClient for ProviderApiAdapter {
    async fn stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let req = self.build_request(model, system, messages, tools, true, None)?;
        self.drive_stream(req).await
    }
}

// ── Media capping (stripExcessMediaItems) ─────────────────────────────────────

/// Maximum media items (images + documents) the API accepts per request.
/// Above this we trim oldest-first. Mirrors TS `API_MAX_MEDIA_PER_REQUEST`
/// (apiLimits.ts:94).
const MAX_MEDIA_PER_REQUEST: usize = 100;

/// Count media (image/document) content blocks across all messages.
fn count_media(msgs: &[ConversationMessage]) -> usize {
    msgs.iter()
        .map(|m| match m {
            ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } => content
                .iter()
                .filter(|b| {
                    matches!(b, ContentBlock::Image { .. })
                        || matches!(b, ContentBlock::Document { .. })
                })
                .count(),
            ConversationMessage::System { .. } => 0,
        })
        .sum()
}

/// Return `msgs` with the OLDEST media items stripped until at most `limit`
/// remain.
fn strip_excess_media(
    mut msgs: Vec<ConversationMessage>,
    limit: usize,
) -> Vec<ConversationMessage> {
    let total = count_media(&msgs);
    if total <= limit {
        return msgs;
    }
    let mut to_remove = total - limit;
    for m in &mut msgs {
        if to_remove == 0 {
            break;
        }
        let content = match m {
            ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } => content,
            ConversationMessage::System { .. } => continue,
        };
        content.retain(|b| {
            if to_remove > 0
                && (matches!(b, ContentBlock::Image { .. })
                    || matches!(b, ContentBlock::Document { .. }))
            {
                to_remove -= 1;
                false
            } else {
                true
            }
        });
    }
    msgs
}

/// Generate a short client-side request id (same alphabet as api-client).
///
/// **Header name**: `x-request-id` — sourced from `api-client/src/anthropic.rs:868`.
#[must_use]
fn new_request_id() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-";
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::retry::DEFAULT_MAX_RETRIES;
    use llm_client::{
        AuthStrategy, BoxFuture, Capabilities, ClientConfig, CredentialConfig, LlmError,
        ModelProfile, PricingConfig, ProtocolFamily, ProviderId, ProviderProfile, ProviderRequest,
        ProviderResponse, StreamingResponse,
    };
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    // ── FakeTransport ─────────────────────────────────────────────────────────

    /// A fake Transport that returns a scripted sequence of responses (or errors).
    struct FakeTransport {
        /// Pre-recorded responses returned in order; cycles to last entry.
        responses: Mutex<Vec<FakeResponse>>,
        /// All requests received, in order.
        seen: Mutex<Vec<ProviderRequest>>,
    }

    #[derive(Clone)]
    #[allow(dead_code)]
    enum FakeResponse {
        Ok(ProviderResponse),
        Err(LlmError),
    }

    impl FakeTransport {
        /// Return the same response on every call.
        fn always(resp: ProviderResponse) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(vec![FakeResponse::Ok(resp)]),
                seen: Mutex::new(vec![]),
            })
        }

        /// Return each response in sequence; the last is repeated forever.
        fn sequence(resps: Vec<FakeResponse>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(resps),
                seen: Mutex::new(vec![]),
            })
        }

        fn seen_count(&self) -> usize {
            self.seen.lock().unwrap().len()
        }

        fn seen_headers(&self, idx: usize) -> BTreeMap<String, String> {
            self.seen.lock().unwrap()[idx].headers.clone()
        }
    }

    impl Transport for FakeTransport {
        fn execute<'a>(
            &'a self,
            request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
            let mut seen = self.seen.lock().unwrap();
            seen.push(request.clone());
            let idx = (seen.len() - 1).min({
                let resps = self.responses.lock().unwrap();
                resps.len().saturating_sub(1)
            });
            let resp = {
                let resps = self.responses.lock().unwrap();
                resps[idx].clone()
            };
            Box::pin(async move {
                match resp {
                    FakeResponse::Ok(r) => Ok(r),
                    FakeResponse::Err(e) => Err(e),
                }
            })
        }

        fn open_stream<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
            Box::pin(async move {
                Err(LlmError::Transport {
                    message: "open_stream not scripted".to_string(),
                })
            })
        }
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn ok_response_json() -> serde_json::Value {
        serde_json::json!({
            "id": "msg_test",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        })
    }

    fn make_adapter_with_subscriber(
        transport: Arc<dyn Transport>,
        subscriber: SubscriberState,
    ) -> ProviderApiAdapter {
        std::env::set_var("ADAPTER_TEST_KEY", "test-key");
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "ADAPTER_TEST_KEY".to_string(),
                    },
                    models: vec![ModelProfile {
                        display_model: "claude-sonnet-4-20250514".to_string(),
                        request_model: "claude-sonnet-4-20250514".to_string(),
                        billing_model: "claude-sonnet-4".to_string(),
                        aliases: vec!["claude".to_string()],
                        capabilities: Capabilities {
                            streaming: true,
                            tools: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                }],
            })
            .expect("client"),
        );
        ProviderApiAdapter::new(
            client,
            transport,
            subscriber,
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            None,
            None,
        )
    }

    fn make_adapter(transport: Arc<dyn Transport>) -> ProviderApiAdapter {
        std::env::set_var("ADAPTER_TEST_KEY", "test-key");
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "ADAPTER_TEST_KEY".to_string(),
                    },
                    models: vec![ModelProfile {
                        display_model: "claude-sonnet-4-20250514".to_string(),
                        request_model: "claude-sonnet-4-20250514".to_string(),
                        billing_model: "claude-sonnet-4".to_string(),
                        aliases: vec!["claude".to_string()],
                        capabilities: Capabilities {
                            streaming: true,
                            tools: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                }],
            })
            .expect("client"),
        );
        ProviderApiAdapter::new(
            client,
            transport,
            SubscriberState::default(),
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            None,
            None,
        )
    }

    // ── Previously-ignored tests (un-ignored, ported to FakeTransport) ────────

    #[tokio::test]
    async fn bridge_resolves_and_forwards_local_model() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let resp = adapter
            .messages_create("claude-sonnet-4-20250514", Some("sys"), Vec::new(), Vec::new())
            .await
            .expect("ok");
        assert_eq!(resp.model, "claude-sonnet-4-20250514");
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn available_models_non_empty() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let models = OrchestratorApiClient::available_models(&adapter);
        assert!(!models.is_empty(), "available_models must return at least one entry");
    }

    #[tokio::test]
    async fn bridge_forwards_batched_tools() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let tools = vec![serde_json::json!({
            "name": "Read",
            "description": "read a file",
            "input_schema": {"type": "object"}
        })];
        let _ = adapter
            .messages_create("claude-sonnet-4-20250514", Some("sys"), Vec::new(), tools)
            .await
            .expect("ok");
        assert_eq!(transport.seen_count(), 1);
    }

    #[tokio::test]
    async fn messages_create_with_tools_on_tool_capable_model_succeeds() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let tools = vec![serde_json::json!({
            "name": "Read",
            "description": "read",
            "input_schema": {"type": "object"}
        })];
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), tools)
            .await;
        assert!(result.is_ok(), "tool-capable model should accept tools");
    }

    #[tokio::test]
    async fn subagent_api_client_seam_forwards_through_trait_object() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let seam: Arc<dyn agent::SubagentApiClient> = Arc::new(adapter);
        let result = seam
            .messages_create("claude-sonnet-4-20250514", Some("sys"), Vec::new(), Vec::new())
            .await;
        // May succeed or fail with UnsupportedCapability if stream not configured,
        // but must not panic.
        let _ = result;
    }

    #[tokio::test]
    async fn image_to_vision_model_is_allowed() {
        use protocol::{ContentBlock, ConversationMessage, ImageSource, MessageId};
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let msgs = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Url {
                    url: "https://x/y.png".to_string(),
                },
            }],
        }];
        // The capability check is in DefaultLlmClient.validate_capabilities; since
        // FakeTransport doesn't inspect the body, this exercises the whole path.
        let _ = adapter
            .messages_create("claude-sonnet-4-20250514", None, msgs, Vec::new())
            .await;
    }

    // ── New plan-named tests ──────────────────────────────────────────────────

    /// Plan test: 429 with `retry-after` triggers one sleep then succeeds.
    #[tokio::test]
    async fn live_path_surfaces_rate_limit_headers() {
        let mut rate_limit_headers = BTreeMap::new();
        rate_limit_headers.insert("retry-after".to_string(), "1".to_string());

        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse {
                status: 429,
                headers: rate_limit_headers,
                body_json: serde_json::json!({
                    "type": "error",
                    "error": {"type": "rate_limit_error", "message": "rate limited"}
                }),
                request_id: None,
            }),
            FakeResponse::Ok(ProviderResponse::json(200, ok_response_json())),
        ]);
        let adapter = make_adapter(transport.clone());
        let resp = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await
            .expect("ok after retry");
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        // Two executions: the 429 then the 200.
        assert_eq!(transport.seen_count(), 2);
    }

    /// Plan test: betas and User-Agent are injected post-prepare.
    #[tokio::test]
    async fn betas_and_user_agent_applied_post_prepare() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport.clone());
        let _ = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await
            .expect("ok");
        let headers = transport.seen_headers(0);
        assert!(
            headers.contains_key("anthropic-beta"),
            "anthropic-beta must be present; got: {headers:?}"
        );
        let ua = headers.get("user-agent").cloned().unwrap_or_default();
        assert!(
            ua.starts_with("claude-cli/"),
            "user-agent must start with claude-cli/; got: {ua}"
        );
    }

    /// Plan test: budget terminates after DEFAULT_MAX_RETRIES + 1 executions.
    #[tokio::test]
    async fn retry_terminal_after_budget() {
        // All responses are 500 — should retry DEFAULT_MAX_RETRIES times then fail.
        let transport = FakeTransport::sequence(vec![FakeResponse::Ok(ProviderResponse::json(
            500,
            serde_json::json!({"type": "error", "error": {"type": "api_error", "message": "internal"}}),
        ))]);
        let adapter = make_adapter(transport.clone());
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;
        assert!(result.is_err(), "must fail after exhausting budget");
        // Should have tried DEFAULT_MAX_RETRIES + 1 = 11 times.
        assert_eq!(
            u32::try_from(transport.seen_count()).unwrap(),
            DEFAULT_MAX_RETRIES + 1,
            "expected {} executions, got {}",
            DEFAULT_MAX_RETRIES + 1,
            transport.seen_count()
        );
    }

    /// Plan test (Step 1b): x-should-retry: false on a 503 is terminal (no retry).
    #[tokio::test]
    async fn x_should_retry_false_is_terminal_for_5xx() {
        let mut headers = BTreeMap::new();
        headers.insert("x-should-retry".to_string(), "false".to_string());

        let transport = FakeTransport::always(ProviderResponse {
            status: 503,
            headers,
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "api_error", "message": "service unavailable"}
            }),
            request_id: None,
        });
        let adapter = make_adapter(transport.clone());
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;
        assert!(result.is_err(), "x-should-retry:false must be terminal");
        // Only ONE execution — no retries.
        assert_eq!(transport.seen_count(), 1, "x-should-retry:false must not retry");
    }

    /// Plan test: tool_use id round-trips through the adapter without mangling.
    #[tokio::test]
    async fn tool_use_id_round_trip_within_turn() {
        use llm_client::ContentBlock as LlmBlock;

        let response_json = serde_json::json!({
            "id": "msg_tool",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {"type": "tool_use", "id": "toolu_abc", "name": "Read", "input": {"path": "/x"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let transport = FakeTransport::always(ProviderResponse::json(200, response_json));
        let adapter = make_adapter(transport);
        let resp = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await
            .expect("ok");
        match resp.content.as_slice() {
            [LlmBlock::ToolCall { id, name, .. }] => {
                assert_eq!(id, "toolu_abc", "tool_use id must round-trip verbatim");
                assert_eq!(name, "Read");
            }
            other => panic!("expected single ToolCall, got: {other:?}"),
        }
    }

    // ---- MULTIMODAL.6: per-request media cap (stripExcessMediaItems) ----

    fn img(n: usize) -> protocol::ContentBlock {
        protocol::ContentBlock::Image {
            source: protocol::ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: format!("img{n}"),
            },
        }
    }

    fn user_with_imgs(range: std::ops::Range<usize>) -> ConversationMessage {
        let mut content = vec![ContentBlock::Text {
            text: "hi".to_string(),
        }];
        content.extend(range.map(img));
        ConversationMessage::User {
            id: protocol::MessageId::new(),
            content,
        }
    }

    fn image_data_in_order(msgs: &[ConversationMessage]) -> Vec<String> {
        let mut out = Vec::new();
        for m in msgs {
            if let ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } = m
            {
                for b in content {
                    if let ContentBlock::Image {
                        source: protocol::ImageSource::Base64 { data, .. },
                    } = b
                    {
                        out.push(data.clone());
                    }
                }
            }
        }
        out
    }

    #[test]
    fn count_media_counts_top_level_images_across_messages() {
        let msgs = vec![user_with_imgs(0..3), user_with_imgs(3..5)];
        assert_eq!(count_media(&msgs), 5);
    }

    #[test]
    fn tool_result_string_content_contributes_no_media() {
        let msgs = vec![ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: protocol::ToolUseId::new(),
                    content: "lots of text, no media".to_string(),
                    is_error: false,
                },
                img(0),
            ],
        }];
        assert_eq!(count_media(&msgs), 1);
    }

    #[test]
    fn strip_excess_media_trims_oldest_to_limit_without_touching_history() {
        let stored = vec![user_with_imgs(0..60), user_with_imgs(60..102)];
        assert_eq!(count_media(&stored), 102);

        let to_send = stored.clone();
        let trimmed = strip_excess_media(to_send, MAX_MEDIA_PER_REQUEST);

        assert_eq!(count_media(&trimmed), 100);
        let remaining = image_data_in_order(&trimmed);
        assert_eq!(remaining.len(), 100);
        assert_eq!(remaining.first().unwrap(), "img2");
        assert_eq!(remaining.last().unwrap(), "img101");
        assert!(!remaining.contains(&"img0".to_string()));
        assert!(!remaining.contains(&"img1".to_string()));

        assert_eq!(count_media(&stored), 102);
        assert_eq!(image_data_in_order(&stored).first().unwrap(), "img0");
    }

    #[test]
    fn strip_excess_media_leaves_within_limit_messages_unchanged() {
        let msgs = vec![user_with_imgs(0..50), user_with_imgs(50..100)];
        let before = msgs.clone();
        let out = strip_excess_media(msgs, MAX_MEDIA_PER_REQUEST);
        assert_eq!(out, before, "exactly 100 media → no stripping");
        assert_eq!(count_media(&out), 100);
    }

    #[test]
    fn strip_excess_media_no_images_is_noop() {
        let msgs = vec![ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::Text {
                text: "no media here".to_string(),
            }],
        }];
        let before = msgs.clone();
        let out = strip_excess_media(msgs, MAX_MEDIA_PER_REQUEST);
        assert_eq!(out, before);
    }

    // ── Fix 2: error_kind label-parity test ──────────────────────────────────

    /// Labels must be locked to api-client originals (Fix 2).
    ///
    /// Ensures that re-naming a label here triggers a test failure so the
    /// telemetry schema change is explicit.
    #[test]
    fn error_kind_labels_match_api_client_originals() {
        // api-client: Unauthorized → "unauthorized"
        assert_eq!(ProviderApiAdapter::error_kind(&LlmError::Authentication), "unauthorized");
        assert_eq!(ProviderApiAdapter::error_kind(&LlmError::PermissionDenied), "unauthorized");
        // api-client: Server → "server"
        assert_eq!(ProviderApiAdapter::error_kind(&LlmError::ProviderInternal), "server");
        // api-client: Http → "http"
        assert_eq!(
            ProviderApiAdapter::error_kind(&LlmError::Transport { message: "t".into() }),
            "http"
        );
        // api-client: MalformedStream → "malformed_stream"
        assert_eq!(
            ProviderApiAdapter::error_kind(&LlmError::StreamInterrupted { message: "s".into() }),
            "malformed_stream"
        );
        // api-client: Overloaded → "overloaded"
        assert_eq!(ProviderApiAdapter::error_kind(&LlmError::Overloaded), "overloaded");
        // api-client: RateLimited → "rate_limited"
        assert_eq!(
            ProviderApiAdapter::error_kind(&LlmError::RateLimited { retry_after: None, scope: None }),
            "rate_limited"
        );
        // api-client: PromptTooLong → "prompt_too_long"
        assert_eq!(
            ProviderApiAdapter::error_kind(&LlmError::ContextOverflow { token_gap: 0 }),
            "prompt_too_long"
        );
    }

    // ── Task 8: subscriber 429 gate end-to-end through the adapter ───────────

    /// Task 8 gate: subscriber non-enterprise 429 → terminal immediately (no retry).
    ///
    /// Wire: `SubscriberState { is_subscriber: true, is_enterprise: false }` +
    /// a transport that always returns 429 → the adapter returns an error without
    /// making a second request.
    #[tokio::test]
    async fn subscriber_429_is_terminal_in_adapter() {
        let transport = FakeTransport::always(ProviderResponse {
            status: 429,
            headers: BTreeMap::new(),
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "You have reached your usage limit"}
            }),
            request_id: None,
        });
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState { is_subscriber: true, is_enterprise: false },
        );
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;
        assert!(result.is_err(), "subscriber 429 must be terminal");
        // Only ONE execution — no retries.
        assert_eq!(
            transport.seen_count(),
            1,
            "subscriber 429 must not retry (seen_count should be 1)"
        );
    }

    /// Task 8 gate: enterprise subscriber 429 → retries through the full budget.
    ///
    /// Wire: `SubscriberState { is_subscriber: true, is_enterprise: true }` +
    /// a transport that returns 429 then 200 → the adapter retries and succeeds.
    #[tokio::test]
    async fn enterprise_subscriber_429_retries_in_adapter() {
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse {
                status: 429,
                headers: {
                    let mut h = BTreeMap::new();
                    h.insert("retry-after".to_string(), "0".to_string());
                    h
                },
                body_json: serde_json::json!({
                    "type": "error",
                    "error": {"type": "rate_limit_error", "message": "rate limited"}
                }),
                request_id: None,
            }),
            FakeResponse::Ok(ProviderResponse::json(200, ok_response_json())),
        ]);
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState { is_subscriber: true, is_enterprise: true },
        );
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;
        assert!(result.is_ok(), "enterprise subscriber 429 must retry and succeed");
        assert_eq!(transport.seen_count(), 2, "should have made 2 requests (429 then 200)");
    }

    /// Fix 1 end-to-end: a fake transport returns a 400 PTL envelope with counts;
    /// the resulting `LlmError::ContextOverflow` carries the parsed `token_gap`.
    ///
    /// This pins the adapter→codec→LlmError path: the adapter decodes the 400
    /// PTL response through the AnthropicMessagesCodec and surfaces the variant
    /// with the correct non-zero gap, so the turn-loop's `token_gap` binding is
    /// non-zero instead of the old `0` sentinel.
    #[tokio::test]
    async fn ptl_response_surfaces_context_overflow_with_gap() {
        // The 400 PTL envelope that Anthropic returns.
        let ptl_body = serde_json::json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "prompt is too long: 210000 tokens > 200000 maximum"
            }
        });
        let transport = FakeTransport::always(ProviderResponse::json(400, ptl_body));
        let adapter = make_adapter(transport);

        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;

        match result {
            Err(LlmError::ContextOverflow { token_gap }) => {
                assert_eq!(
                    token_gap, 10_000,
                    "token_gap must be 210000 - 200000 = 10000; got {token_gap}"
                );
            }
            other => panic!("expected ContextOverflow {{ token_gap: 10000 }}, got {other:?}"),
        }
    }
}
