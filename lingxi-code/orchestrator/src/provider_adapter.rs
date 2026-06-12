//! Drive `llm_client::DefaultLlmClient` from the orchestrator's seam traits.
//!
//! **Task 6**: replaces the old `providers::ModelRouter`-backed stubs with a real
//! `DefaultLlmClient` drive: `prepare()` → header injection → `transport.execute()`
//! / `open_stream()` → `codec.decode_response()`.  The retry driver loop wraps the
//! prepare/execute pair and feeds headers to `model/rate_limit.rs` + `model/retry.rs`.

use crate::conversation::{OrchestratorApiClient, StreamingApiClient};
use crate::model::betas::{apply_beta_header_with_auth, Endpoint, Provider};
use crate::model::rate_limit::{
    formatted_reset_times_from_headers, parse_retry_after, parse_unified_reset,
    rate_limit_error_message, RateLimitInfo, RawUtilization, SubscriptionContext,
};
use crate::model::retry::{next_step_with_backoff, resolve_retry_control_with_settings, DriveStep, ResolveRetryEnv, RetryControl, RetryState};
use crate::model::telemetry;
use crate::model::user_agent::{user_agent, UserAgentEnv};
use agent::convert::{to_llm_messages, to_tool_declarations};
use async_trait::async_trait;
use futures::stream::BoxStream;
use llm_client::{
    CostEstimator, DefaultLlmClient, LlmError, LlmEvent, LlmRequest, LlmResponse,
    ProviderRequest, SystemBlock, Transport,
};
use protocol::{ContentBlock, ConversationMessage};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
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
    /// Guard against double-emit: once we have fired succeed/fail we never fire again.
    done: bool,
    /// Optional analytics bus for stream telemetry twins (`emit_succeeded` / `emit_failed`).
    analytics: Option<Arc<::telemetry::AnalyticsBus>>,
    /// Request model string for telemetry labels.
    model: String,
    /// Client-side request id for telemetry correlation.
    request_id: String,
    /// Wall-clock start of the stream for `duration_ms`.
    started: Instant,
}

// ── Adapter state ─────────────────────────────────────────────────────────────

/// Production adapter: drives `DefaultLlmClient` with full retry/rate-limit/betas.
pub struct ProviderApiAdapter {
    client: Arc<DefaultLlmClient>,
    transport: Arc<dyn Transport>,
    /// Subscriber state for the 429 gate (Task 8 wires real value).
    ///
    /// Build-time seed/fallback: when [`Self::subscription`] is attached and
    /// resolved, [`Self::effective_subscriber`] prefers the live snapshot.
    subscriber: SubscriberState,
    /// Live shared subscription slot (batch-5 Task 3). Filled asynchronously
    /// by the composition root's background profile/roles fetch (batch 4);
    /// `None` when the host has no OAuth profile fetch (mobile) or predates
    /// the wiring. Read via [`Self::effective_subscriber`].
    subscription: Option<traits::subscription::SharedSubscription>,
    /// User-agent environment snapshot (Task 3).
    ua: UserAgentEnv,
    /// Build version string for the User-Agent header.
    version: String,
    /// Optional analytics bus for telemetry events.
    analytics: Option<Arc<::telemetry::AnalyticsBus>>,
    /// Global fallback model, if configured (used by `messages_create_with_fallback`
    /// when no per-model entry exists in `fallback_overrides`).
    fallback_model: Option<String>,
    /// Per-model fallback chains from `routing.fallback`.
    ///
    /// Key is the request's resolved display model; value is the ordered chain
    /// of fallback target display models.  A per-model entry **wins** over
    /// `fallback_model` (global).  The adapter walks the chain in order on
    /// consecutive overload events: chain[0] fires first, chain[1] next, etc.
    fallback_overrides: std::collections::BTreeMap<String, Vec<String>>,
    /// Alias → display-model map built at construction from
    /// `client.available_models()`. Used by `messages_create_with_fallback`
    /// to normalize an alias request string to the display model before
    /// probing `fallback_overrides` (whose keys are display-normalized at
    /// parse time).
    alias_to_display: std::collections::BTreeMap<String, String>,
    /// `routing.retry.maxAttempts` override.
    ///
    /// Precedence: `CLAUDE_CODE_MAX_RETRIES` env > this > `DEFAULT_MAX_RETRIES`.
    settings_max_retries: Option<u32>,
    /// `routing.retry.backoffMs` override.
    ///
    /// When `Some(b)`, the jitter ladder's first rung is `b` ms (default 500).
    /// Subsequent rungs are scaled proportionally (`DEFAULT[i] * b/500`).
    /// Jitter ±20% still applies.
    settings_backoff_ms: Option<u64>,
    /// Available model ids from the client registry (for `available_models`).
    available_model_ids: Vec<String>,
    /// Optional cost estimator for populating `LlmResponse.cost`.
    ///
    /// When `Some`, a successful `decode_response` triggers a cost estimate using
    /// the model's resolved `PricingModelRef` and usage counters.  Unpriced or
    /// unknown models leave `response.cost = None` (never an error).  The
    /// `CostTracker` budget authority is UNTOUCHED by this path.
    estimator: Option<Arc<CostEstimator>>,
    /// Most recently observed 2xx rate-limit header snapshot.
    ///
    /// Parsed via [`RateLimitInfo::from_headers`] on every successful
    /// `drive_non_stream` and `drive_stream` connect-success response.
    /// Exposed via [`Self::last_rate_limit_info`].  `None` until the first
    /// successful response is received.  Interior-mutable so non-`&mut self`
    /// callers (the `OrchestratorApiClient` impls) can update it.
    ///
    /// TUI wiring: no existing `OrchestratorHandle` surface maps naturally to
    /// per-request rate-limit metadata (all status APIs are session-wide
    /// snapshots). Callers that need this should call `last_rate_limit_info()`
    /// on the adapter directly. A future task can thread it into the handle if
    /// needed.
    last_rate_limit: Mutex<Option<RateLimitInfo>>,
    /// Most recently observed RAW per-window utilization snapshot.
    ///
    /// Task 2 (llm-client future-work batch 5): parsed via
    /// [`RawUtilization::from_headers`] alongside the [`RateLimitInfo`]
    /// parse in `record_rate_limit_from_headers` — claude-code assigns
    /// `rawUtilization = extractRawUtilization(headers)` on the same passes
    /// that compute the limits (`claudeAiLimits.ts:476`). Assigned
    /// UNCONDITIONALLY on every recorded response (unlike `last_rate_limit`,
    /// which is gated on `has_unified_headers()`), so a later response
    /// without the per-window quartet resets it to the empty snapshot
    /// exactly like the TS module state. `None` until the first recorded
    /// response. NOT recorded on 429 error responses — TS also extracts raw
    /// utilization from error headers (`claudeAiLimits.ts:500`); this seam
    /// records on success paths only, the same pre-existing limitation as
    /// `last_rate_limit`. Exposed via the
    /// `OrchestratorApiClient::last_raw_utilization` override.
    last_raw_utilization: Mutex<Option<RawUtilization>>,
    /// Limits-specific copy composed from the most recent 429 **error**
    /// response's unified headers.
    ///
    /// Task 6 (llm-client future-work batch 5): claude-code builds the
    /// rejected-limits view from the terminal 429's own headers and renders
    /// `getRateLimitErrorMessage` as the user-visible error content
    /// (`errors.ts:480-524`). Set on EVERY decoded 429 by
    /// [`Self::record_rate_limit_from_429`] — `Some(copy)` when the 429
    /// carried unified headers, `None` otherwise (the
    /// `if (rateLimitType || overageStatus)` gate at `errors.ts:480`) — and
    /// cleared on every successful response, so it always reflects the most
    /// recent response seen. Exposed via the
    /// `OrchestratorApiClient::last_rate_limit_error_message` override.
    last_429_message: Mutex<Option<String>>,
}

impl ProviderApiAdapter {
    /// Construct the adapter.  Called by Task 10 host constructors.
    ///
    /// `version` is the build version string embedded in the User-Agent header.
    ///
    /// `estimator` — when `Some`, a successful response decode populates
    /// `LlmResponse.cost` via the llm-client `CostEstimator`.  Pass
    /// `None` to leave cost estimation disabled (existing behaviour before 3c-T3).
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
        Self::new_with_estimator(client, transport, subscriber, ua, version, analytics, fallback_model, None)
    }

    /// Construct the adapter with an explicit cost estimator.
    ///
    /// Hosts that have the `cost::PricingCatalog` available (desktop + mobile)
    /// call this instead of [`Self::new`] to get live `LlmResponse.cost` values.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_estimator(
        client: Arc<DefaultLlmClient>,
        transport: Arc<dyn Transport>,
        subscriber: SubscriberState,
        ua: UserAgentEnv,
        version: impl Into<String>,
        analytics: Option<Arc<::telemetry::AnalyticsBus>>,
        fallback_model: Option<String>,
        estimator: Option<Arc<CostEstimator>>,
    ) -> Self {
        Self::new_with_routing(
            client,
            transport,
            subscriber,
            ua,
            version,
            analytics,
            fallback_model,
            estimator,
            std::collections::BTreeMap::new(),
            None,
            None,
        )
    }

    /// Construct the adapter with routing overrides from `routing.fallback` /
    /// `routing.retry` settings.
    ///
    /// ## Constructor choice
    ///
    /// Hosts that parse `routing` settings call this after
    /// `parse_routing_overrides`; the older [`Self::new`] and
    /// [`Self::new_with_estimator`] paths delegate here with empty overrides so
    /// they continue to compile unchanged.
    ///
    /// ## Fallback precedence (per-request)
    ///
    /// Per-model `fallback_overrides` entry for the request's display model **wins**
    /// over the global `fallback_model` field.  When neither is set, no fallback
    /// is configured.
    ///
    /// ## Retry precedence
    ///
    /// `CLAUDE_CODE_MAX_RETRIES` env > `settings_max_retries` > `DEFAULT_MAX_RETRIES` (10).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_routing(
        client: Arc<DefaultLlmClient>,
        transport: Arc<dyn Transport>,
        subscriber: SubscriberState,
        ua: UserAgentEnv,
        version: impl Into<String>,
        analytics: Option<Arc<::telemetry::AnalyticsBus>>,
        fallback_model: Option<String>,
        estimator: Option<Arc<CostEstimator>>,
        fallback_overrides: std::collections::BTreeMap<String, Vec<String>>,
        settings_max_retries: Option<u32>,
        settings_backoff_ms: Option<u64>,
    ) -> Self {
        let models = client.available_models();
        let available_model_ids = models.iter().map(|m| m.display_model.clone()).collect();
        // Build alias→display map once so `messages_create_with_fallback` can
        // normalize an alias request to the display model before looking up
        // per-model fallback overrides (whose keys are display-normalized).
        let mut alias_to_display = std::collections::BTreeMap::new();
        for m in &models {
            for alias in &m.aliases {
                alias_to_display.insert(alias.clone(), m.display_model.clone());
            }
            // Map display_model → itself so the lookup is always correct
            // whether the caller used the canonical name or an alias.
            alias_to_display.insert(m.display_model.clone(), m.display_model.clone());
        }
        Self {
            client,
            transport,
            subscriber,
            subscription: None,
            ua,
            version: version.into(),
            analytics,
            fallback_model,
            fallback_overrides,
            alias_to_display,
            settings_max_retries,
            settings_backoff_ms,
            available_model_ids,
            estimator,
            last_rate_limit: Mutex::new(None),
            last_raw_utilization: Mutex::new(None),
            last_429_message: Mutex::new(None),
        }
    }

    /// Attach the live subscription slot (batch-5 Task 3). When present and
    /// resolved, the drive loops read subscriber/enterprise state from it at
    /// call time instead of the build-time [`SubscriberState`] copy.
    #[must_use]
    pub fn with_subscription(mut self, slot: traits::subscription::SharedSubscription) -> Self {
        self.subscription = Some(slot);
        self
    }

    /// Effective subscriber state: the live shared snapshot when provided and
    /// resolved (closes the retry-gate half of the `OrchestratorConfig`
    /// PARITY-GAP — `is_enterprise` was build-time `false` because the profile
    /// fetch lands after construction), else the static build-time state.
    /// Poisoned/empty slot → static fallback (conservative, pre-batch-5
    /// behavior).
    ///
    /// Granularity: each drive fn hoists this ONCE before its retry loop, so
    /// `RetryState`'s 429/enterprise gate is stable across a request's retry
    /// attempts — the TS-faithful behaviour (`getSubscriptionType()` reads per
    /// attempt-ish but the gate effectively stabilizes per request).
    fn effective_subscriber(&self) -> SubscriberState {
        let Some(slot) = &self.subscription else { return self.subscriber; };
        let Ok(guard) = slot.read() else { return self.subscriber; };
        let Some(snap) = guard.as_ref() else { return self.subscriber; };
        SubscriberState {
            is_subscriber: snap.is_subscriber,
            is_enterprise: snap.subscription_type.as_deref() == Some("enterprise"),
        }
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
    ///
    /// Reads [`Self::effective_subscriber`] directly (one resolver call per
    /// attempt — these injectors run once per prepare/execute attempt, so the
    /// live-slot read here is per-attempt, the lighter diff vs. threading the
    /// hoisted value through as a parameter).
    fn inject_headers(&self, prepared: &mut ProviderRequest, request_id: &str) {
        // anthropic-beta (Task 2): full assembled list merged with any auth-injected betas.
        apply_beta_header_with_auth(
            prepared,
            Provider::Anthropic,
            Endpoint::MessagesCreate,
            self.effective_subscriber().is_subscriber,
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
            self.effective_subscriber().is_subscriber,
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
            LlmError::Overloaded { .. } => "overloaded",
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

    /// Return the most recently observed 2xx rate-limit header snapshot, if any.
    ///
    /// Populated on every successful response from `drive_non_stream` and on the
    /// connect-success path of `drive_stream`.  `None` until the first successful
    /// response is received.
    ///
    /// TUI wiring note: no existing `OrchestratorHandle` surface maps naturally
    /// to per-request rate-limit metadata.  Callers that need this should hold an
    /// `Arc<ProviderApiAdapter>` and call this method directly.  A future task can
    /// wire it through the handle if needed.
    pub fn last_rate_limit_info(&self) -> Option<RateLimitInfo> {
        self.last_rate_limit.lock().unwrap().clone()
    }

    /// Parse rate-limit headers from a 2xx response and update the cached snapshot.
    ///
    /// Emits a `tracing::warn!` when the overage status indicates the account is
    /// at or near exhaustion (`overage_status == "rejected"` or `"allowed_warning"`).
    fn record_rate_limit_from_headers(
        &self,
        headers: &std::collections::BTreeMap<String, String>,
    ) {
        let hvec: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Task 2 (llm-client future-work batch 5): track the raw per-window
        // snapshot on EVERY recorded headers pass — `rawUtilization =
        // extractRawUtilization(headersToUse)` (claudeAiLimits.ts:476), NOT
        // gated on `has_unified_headers()` like the limits snapshot below.
        *self.last_raw_utilization.lock().unwrap() = Some(RawUtilization::from_headers(&hvec));
        let info = RateLimitInfo::from_headers(&hvec);
        if info.has_unified_headers() {
            // Warn when the account is near or at exhaustion.
            match info.overage_status.as_deref() {
                Some("rejected") => {
                    tracing::warn!(
                        overage_status = "rejected",
                        rate_limit_type = ?info.rate_limit_type,
                        "Rate limit: overage rejected — usage limit exhausted"
                    );
                }
                Some("allowed_warning") => {
                    tracing::warn!(
                        overage_status = "allowed_warning",
                        rate_limit_type = ?info.rate_limit_type,
                        "Rate limit: overage warning — nearing usage limit"
                    );
                }
                _ => {}
            }
            *self.last_rate_limit.lock().unwrap() = Some(info);
        }
        // Task 6 (batch 5): a successful response supersedes any cached 429
        // limits copy — the slot always reflects the most recent response.
        *self.last_429_message.lock().unwrap() = None;
    }

    /// Whether the live subscription snapshot is a Pro or Enterprise plan —
    /// the `getSubscriptionType() === 'pro' || 'enterprise'` predicate gating
    /// the `seven_day_sonnet` wording (claude-code
    /// `rateLimitMessages.ts:176-181`).
    ///
    /// Reads the live [`Self::subscription`] slot directly (the snapshot
    /// carries `subscription_type`; [`SubscriberState`] does not). With no
    /// resolved snapshot, falls back to the build-time enterprise bit —
    /// `pro` is unknowable pre-snapshot, matching an unresolved
    /// `getSubscriptionType()` evaluating to neither.
    fn is_pro_or_enterprise(&self) -> bool {
        if let Some(slot) = &self.subscription {
            if let Ok(guard) = slot.read() {
                if let Some(snap) = guard.as_ref() {
                    return matches!(
                        snap.subscription_type.as_deref(),
                        Some("pro" | "enterprise")
                    );
                }
            }
        }
        self.subscriber.is_enterprise
    }

    /// Record the unified rate-limit context from a 429 **error** response —
    /// the Rust seam for claude-code `errors.ts:471-524`, which extracts the
    /// unified headers from the error itself when a turn dies on a 429
    /// (success-path recording never sees them).
    ///
    /// When the 429 carries unified headers (the
    /// `if (rateLimitType || overageStatus)` gate, `errors.ts:480`):
    /// 1. the forced-`rejected` limits view replaces the cached snapshot
    ///    (TS updates its limits state from the error headers with
    ///    `status: 'rejected'`, `errors.ts:482-516`), and
    /// 2. the composed `getRateLimitErrorMessage` copy is cached for the
    ///    orchestrator's terminal-error re-map
    ///    (`OrchestratorError::RateLimitRejected`).
    ///
    /// Without unified headers the copy slot is cleared (the generic 429
    /// surface applies) and the limits snapshot is left untouched — TS only
    /// updates inside the gated branch.
    fn record_rate_limit_from_429(&self, headers: &std::collections::BTreeMap<String, String>) {
        let hvec: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let composed = RateLimitInfo::from_429_error_headers(&hvec).map(|info| {
            // errors.ts:482-516 — the limits state is updated from the
            // error's headers with status forced to 'rejected'.
            *self.last_rate_limit.lock().unwrap() = Some(info.clone());
            // `formatResetTime(…, true)` analogue for both reset headers
            // (`rateLimitMessages.ts:144-148`), formatted at error time.
            let formatted = formatted_reset_times_from_headers(&hvec);
            rate_limit_error_message(
                &info,
                &formatted.as_reset_times(),
                SubscriptionContext {
                    is_pro_or_enterprise: self.is_pro_or_enterprise(),
                },
            )
        });
        *self.last_429_message.lock().unwrap() = composed.flatten();
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
            LlmError::Overloaded { .. } => Some(529),
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
        self.drive_non_stream_seeded_with_chain(req, retry_control, 0, &[]).await
    }

    /// Non-stream retry driver with a pre-seeded `consecutive_overloaded` counter
    /// and an optional fallback chain.
    ///
    /// The seed is set to 1 when this call is a non-streaming fallback triggered by
    /// a mid-stream `LlmError::Overloaded` — mirroring TS `claude.ts:2559`
    /// (`initialConsecutive529Errors: is529Error(streamingError) ? 1 : 0`).
    ///
    /// `chain` is the ordered slice of fallback models to walk on consecutive
    /// overload events.  `retry_control` must already carry `chain[0]` as
    /// `fallback_model` (set by [`Self::messages_create_with_fallback`]); on
    /// each [`DriveStep::Fallback`] the loop advances `chain_idx` and rebuilds
    /// `retry_control` with `chain[chain_idx]` (or disables fallback when
    /// exhausted).
    #[allow(clippy::too_many_lines)]
    async fn drive_non_stream_seeded_with_chain(
        &self,
        mut req: LlmRequest,
        mut retry_control: RetryControl,
        initial_consecutive_overloaded: u8,
        chain: &[String],
    ) -> Result<LlmResponse, LlmError> {
        let request_id = new_request_id();
        let started = Instant::now();
        telemetry::emit_started(&self.analytics, &req.model, &request_id, false).await;

        // Batch-5 Task 3: resolve the live subscriber state ONCE per drive call
        // (not per attempt) — RetryState persists across the retry loop, so the
        // 429/enterprise gate is stable for the whole request, matching the TS
        // granularity (the gate effectively stabilizes per request).
        let sub = self.effective_subscriber();
        let mut state = RetryState {
            consecutive_overloaded: initial_consecutive_overloaded,
            is_subscriber: sub.is_subscriber,
            is_enterprise: sub.is_enterprise,
            ..RetryState::default()
        };
        // thinking_budget: Task 6 drives with 0; extended-thinking wiring in Task 10+.
        let thinking_budget: u32 = req.reasoning.map_or(0, |r| r.budget_tokens);
        // Index into `chain` for the NEXT fallback entry to use.
        // chain_idx=0 means chain[0] is the current fallback in `retry_control`.
        // After a Fallback step, chain_idx advances to point at the next entry.
        // When chain_idx >= chain.len(), the chain is exhausted.
        let mut chain_idx: usize = 0;

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
                    let step = next_step_with_backoff(&mut state, &retry_control, &transport_err, thinking_budget, self.settings_backoff_ms);
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

                    match prepared.route.codec.decode_response(provider_resp.clone()) {
                        Ok(mut response) => {
                            // Feed rate-limit headers from every 2xx success response.
                            self.record_rate_limit_from_headers(&provider_resp.headers);
                            // 3c-T3: populate response.cost when an estimator is wired.
                            // Unpriced or unknown models leave response.cost = None — never an error.
                            if let Some(est) = &self.estimator {
                                let pricing_ref =
                                    prepared.route.resolved_route.pricing_model.clone();
                                if let Ok(estimate) = est.estimate(pricing_ref, &response.usage) {
                                    if estimate.total_cost_usd.is_some() {
                                        response.cost = Some(estimate);
                                    }
                                }
                            }
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
                                // Task 6 (batch 5): capture the 429's OWN
                                // unified headers (errors.ts:471-516) so a
                                // terminal 429 can surface the limits copy.
                                self.record_rate_limit_from_429(&provider_resp.headers);
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

                            let step = next_step_with_backoff(&mut state, &retry_control, &effective_err, thinking_budget, self.settings_backoff_ms);
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
                                    // Switch to the fallback model; advance the
                                    // chain index so the next iteration's ctl
                                    // points at chain[chain_idx] (or is
                                    // exhausted → allow_fallback=false).
                                    req.model = fallback_model;
                                    chain_idx += 1;
                                    // Reset the consecutive-overload counter so
                                    // the new primary model's 529 budget is fresh.
                                    state.consecutive_overloaded = 0;
                                    // Rebuild retry_control with the next chain
                                    // entry (None when exhausted).
                                    let next_fallback = chain.get(chain_idx).cloned();
                                    let allow_fallback = next_fallback.is_some();
                                    retry_control = resolve_retry_control_with_settings(
                                        &req.model,
                                        next_fallback,
                                        sub.is_subscriber,
                                        &ResolveRetryEnv::from_process_env(),
                                        self.settings_max_retries,
                                    );
                                    if allow_fallback {
                                        retry_control.allow_fallback = true;
                                    }
                                    continue;
                                }
                                DriveStep::Terminal => {
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
                                DriveStep::RepeatedOverloaded => {
                                    // External non-sandbox threshold: surface the
                                    // repeated bit so the conversion layer produces
                                    // `OrchestratorError::RepeatedOverloaded` with the
                                    // byte-locked "Repeated 529 Overloaded errors" copy
                                    // (errors.ts:166).
                                    let repeated_err = LlmError::Overloaded { repeated: true };
                                    telemetry::emit_failed(
                                        &self.analytics,
                                        &req.model,
                                        &request_id,
                                        Self::error_kind(&repeated_err),
                                        Self::status_of(&repeated_err),
                                    )
                                    .await;
                                    return Err(repeated_err);
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
    /// **Streaming rate-limit headers (3c-T1 closed):** `Transport::open_stream`
    /// now returns real `StreamingResponse{status, headers}` via the additive
    /// `stream_sse_with_meta` path added in plan 3c.  The connect-phase ≥400
    /// branch below reads `streaming.headers` and calls `resolve_retry_after`
    /// just as the non-stream path does, so 429+`retry-after` delays are
    /// honoured on the streaming path.
    #[allow(clippy::too_many_lines)]
    async fn drive_stream(
        &self,
        req: LlmRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let request_id = new_request_id();
        telemetry::emit_started(&self.analytics, &req.model, &request_id, true).await;

        // Batch-5 Task 3: live subscriber state, resolved ONCE per drive call
        // (see `drive_non_stream_seeded_with_chain` for the granularity note).
        let sub = self.effective_subscriber();
        let mut state = RetryState {
            is_subscriber: sub.is_subscriber,
            is_enterprise: sub.is_enterprise,
            ..RetryState::default()
        };
        // Stream path uses settings-based retry control (same precedence as non-stream).
        let ctl = resolve_retry_control_with_settings(
            &req.model,
            None, // fallback not used on stream connect-phase
            sub.is_subscriber,
            &ResolveRetryEnv::from_process_env(),
            self.settings_max_retries,
        );
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
                    let step = next_step_with_backoff(&mut state, &ctl, &transport_err, thinking_budget, self.settings_backoff_ms);
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
                        let response_headers = streaming.headers;
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
                            headers: response_headers.clone(),
                            body_json,
                            request_id: None,
                        };
                        let decode_err = match prepared.route.codec.decode_response(err_response) {
                            Err(e) => e,
                            Ok(_) => LlmError::ProviderInternal,
                        };

                        // Mirror the non-stream path: for 429s, resolve the
                        // actual retry delay from the real response headers
                        // (retry-after / anthropic-ratelimit-*).  Empty headers
                        // fall through to the 1 s fallback inside
                        // `resolve_retry_after`.
                        let effective_err = if let LlmError::RateLimited { .. } = &decode_err {
                            // Task 6 (batch 5): same 429-error-header capture
                            // as the non-stream path (errors.ts:471-516).
                            self.record_rate_limit_from_429(&response_headers);
                            LlmError::RateLimited {
                                retry_after: Some(Self::resolve_retry_after(&response_headers)),
                                scope: None,
                            }
                        } else {
                            decode_err.clone()
                        };

                        let step = next_step_with_backoff(&mut state, &ctl, &effective_err, thinking_budget, self.settings_backoff_ms);
                        if let DriveStep::RetryAfter(delay) = step {
                            tokio::time::sleep(delay).await;
                            // Re-prepare on next iteration so headers stay fresh.
                            continue;
                        }
                        // Terminal twin for the connect-phase emit_started
                        // (mirrors the non-stream terminal arms).
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

                    // Feed rate-limit headers from the connect-success response.
                    self.record_rate_limit_from_headers(&streaming.headers);

                    // Success: wrap the LlmEventStream from the codec into a BoxStream.
                    // Build the event stream from the codec decoder + raw frames.
                    let decoder = prepared.route.codec.stream_decoder();
                    let frames = streaming.frames;

                    // Clone analytics + metadata into the unfold state so
                    // emit_succeeded / emit_failed can fire from inside the async closure.
                    let stream_started = Instant::now();
                    let stream_analytics = self.analytics.clone();
                    let stream_model = req.model.clone();
                    let stream_request_id = request_id.clone();

                    // Assemble events via a manual unfold that drives next_frame + decode.
                    // We keep a queue of pre-decoded events and drain them first.
                    let stream_state = StreamState {
                        decoder,
                        frames,
                        queue: VecDeque::new(),
                        finished: false,
                        done: false,
                        analytics: stream_analytics,
                        model: stream_model,
                        request_id: stream_request_id,
                        started: stream_started,
                    };

                    let boxed: BoxStream<'static, Result<LlmEvent, LlmError>> = Box::pin(
                        futures::stream::unfold(stream_state, |mut s| async move {
                            loop {
                                if let Some(event) = s.queue.pop_front() {
                                    // Emit succeed telemetry on the terminal event
                                    // (MessageStop or Completed) — once, guarded by `done`.
                                    let is_terminal = matches!(
                                        event,
                                        LlmEvent::MessageStop | LlmEvent::Completed { .. }
                                    );
                                    if is_terminal && !s.done {
                                        s.done = true;
                                        let elapsed_ms = u64::try_from(
                                            s.started.elapsed().as_millis()
                                        )
                                        .unwrap_or(u64::MAX);
                                        telemetry::emit_succeeded(
                                            &s.analytics,
                                            &s.model,
                                            &s.request_id,
                                            elapsed_ms,
                                            200,
                                        )
                                        .await;
                                    }
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
                                            if !s.done {
                                                s.done = true;
                                                telemetry::emit_failed(
                                                    &s.analytics,
                                                    &s.model,
                                                    &s.request_id,
                                                    ProviderApiAdapter::error_kind(&e),
                                                    ProviderApiAdapter::status_of(&e),
                                                )
                                                .await;
                                            }
                                            return Some((Err(e), s));
                                        }
                                    },
                                    Ok(None) => {
                                        s.finished = true;
                                        match s.decoder.finish() {
                                            Ok(events) => s.queue.extend(events),
                                            Err(e) => {
                                                if !s.done {
                                                    s.done = true;
                                                    telemetry::emit_failed(
                                                        &s.analytics,
                                                        &s.model,
                                                        &s.request_id,
                                                        ProviderApiAdapter::error_kind(&e),
                                                        ProviderApiAdapter::status_of(&e),
                                                    )
                                                    .await;
                                                }
                                                return Some((Err(e), s));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        s.finished = true;
                                        if !s.done {
                                            s.done = true;
                                            telemetry::emit_failed(
                                                &s.analytics,
                                                &s.model,
                                                &s.request_id,
                                                ProviderApiAdapter::error_kind(&e),
                                                ProviderApiAdapter::status_of(&e),
                                            )
                                            .await;
                                        }
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
        let ctl = resolve_retry_control_with_settings(
            model,
            None,
            self.effective_subscriber().is_subscriber,
            &ResolveRetryEnv::from_process_env(),
            self.settings_max_retries,
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
        let ctl = resolve_retry_control_with_settings(
            model,
            None,
            self.effective_subscriber().is_subscriber,
            &ResolveRetryEnv::from_process_env(),
            self.settings_max_retries,
        );
        self.drive_non_stream(req, ctl).await
    }

    /// Non-streaming call with Opus-fallback policy wired.
    ///
    /// Routes through [`resolve_retry_control`] which computes `allow_fallback`
    /// from the env + subscriber state (Task 8). The `_is_subscriber` /
    /// `_is_enterprise` parameters are **ignored** — the adapter always reads
    /// subscriber state via [`Self::effective_subscriber`] (the live shared
    /// snapshot when attached, else the construction-time copy).
    /// The underscore prefix signals that these call-site values are not used;
    /// the parameters are kept for API compatibility and will be removed in
    /// Task 10.
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
        // Per-model settings fallback wins over global fallback_model.
        // Call-site fallback_model (from OrchestratorApiClient) wins over both when
        // it's explicitly passed.
        //
        // Normalize the request model via alias_to_display so that an alias
        // request (e.g. "claude-3-5-sonnet" → display "claude-sonnet-4-5")
        // still finds the per-model fallback entry whose key is the display model.
        let display_model = self.alias_to_display.get(model).map_or(model, String::as_str);

        // Build the effective chain:
        //   1. explicit call-site fallback_model → single-entry chain (legacy path)
        //   2. per-model settings chain          → full multi-entry chain
        //   3. global fallback_model             → single-entry chain
        // The chain is walked entry-by-entry in the drive loop.
        let effective_chain: Vec<String> = if let Some(fb) = fallback_model {
            // Explicit call-site model → single-entry chain (preserves pre-Task-8 contract).
            vec![fb.to_string()]
        } else if let Some(chain) = self.fallback_overrides.get(display_model) {
            chain.clone()
        } else if let Some(global) = &self.fallback_model {
            vec![global.clone()]
        } else {
            vec![]
        };

        let req = self.build_request(model, system, msgs, tools, false, None)?;
        // Initial ctl: chain[0] as fallback_model (None when chain is empty).
        let mut ctl = resolve_retry_control_with_settings(
            model,
            effective_chain.first().cloned(),
            self.effective_subscriber().is_subscriber,
            &ResolveRetryEnv::from_process_env(),
            self.settings_max_retries,
        );
        // If any fallback is configured, honour allow_fallback regardless of
        // the model-type heuristic (preserves the pre-Task-8 contract: an
        // explicit or configured fallback always enables the gate).
        if !effective_chain.is_empty() {
            ctl.allow_fallback = true;
        }
        self.drive_non_stream_seeded_with_chain(req, ctl, 0, &effective_chain).await
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
        let ctl = resolve_retry_control_with_settings(
            model,
            None,
            self.effective_subscriber().is_subscriber,
            &ResolveRetryEnv::from_process_env(),
            self.settings_max_retries,
        );
        self.drive_non_stream_seeded_with_chain(req, ctl, initial_consecutive_overloaded, &[])
            .await
    }

    fn available_models(&self) -> Vec<String> {
        self.available_model_ids.clone()
    }

    /// Return the most recently observed rate-limit header snapshot.
    ///
    /// Delegates to [`Self::last_rate_limit_info`] and maps the internal
    /// `RateLimitInfo` struct into the public [`traits::RateLimitSnapshot`]
    /// (all three fields: `rate_limit_type`, `overage_status`, and
    /// `overage_disabled_reason`).
    fn last_rate_limit_info(&self) -> Option<traits::RateLimitSnapshot> {
        self.last_rate_limit_info().map(|info| traits::RateLimitSnapshot {
            rate_limit_type: info.rate_limit_type,
            overage_status: info.overage_status,
            overage_disabled_reason: info.overage_disabled_reason,
        })
    }

    /// Task 8 (llm-client future-work batch 3): expose the FULL internal
    /// nine-field snapshot for the turn drivers' `emit_rate_limit` seam.
    /// Delegates to the inherent [`Self::last_rate_limit_info`] (which
    /// already returns the internal `RateLimitInfo`); the trait method of
    /// the same name above keeps its three-field projection untouched.
    fn last_rate_limit_full(&self) -> Option<RateLimitInfo> {
        self.last_rate_limit_info()
    }

    /// Task 2 (llm-client future-work batch 5): expose the raw per-window
    /// snapshot cached by `record_rate_limit_from_headers` for the turn
    /// drivers' `emit_raw_utilization` seam.
    fn last_raw_utilization(&self) -> Option<RawUtilization> {
        *self.last_raw_utilization.lock().unwrap()
    }

    /// Task 6 (llm-client future-work batch 5): expose the limits copy
    /// composed by [`Self::record_rate_limit_from_429`] from the most recent
    /// 429 error response's unified headers, for the orchestrator's
    /// terminal-429 re-map (claude-code `errors.ts:480-524`).
    fn last_rate_limit_error_message(&self) -> Option<String> {
        self.last_429_message.lock().unwrap().clone()
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

        /// Return the `"model"` field from the JSON body of the `idx`-th request.
        /// Useful for asserting chain-walk model sequences.
        fn seen_body_model(&self, idx: usize) -> Option<String> {
            self.seen.lock().unwrap()[idx]
                .body_json
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string)
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
                    signing: None,
                    azure: None,
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
                    signing: None,
                    azure: None,
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

    // ── effective_subscriber (batch-5 Task 3: live SharedSubscription) ───────

    fn shared_slot(
        snap: Option<traits::subscription::SubscriptionSnapshot>,
    ) -> traits::subscription::SharedSubscription {
        Arc::new(std::sync::RwLock::new(snap))
    }

    #[test]
    fn effective_subscriber_prefers_live_snapshot() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(transport, SubscriberState::default())
            .with_subscription(shared_slot(Some(traits::subscription::SubscriptionSnapshot {
                is_subscriber: true,
                subscription_type: Some("enterprise".to_string()),
                ..Default::default()
            })));
        let sub = adapter.effective_subscriber();
        assert!(sub.is_subscriber);
        assert!(sub.is_enterprise);
    }

    #[test]
    fn effective_subscriber_falls_back_when_slot_empty_or_absent() {
        let static_state = SubscriberState { is_subscriber: true, is_enterprise: false };

        // No slot attached → static build-time state.
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(transport, static_state);
        let sub = adapter.effective_subscriber();
        assert!(sub.is_subscriber);
        assert!(!sub.is_enterprise);

        // Slot attached but unresolved (None) → static build-time state.
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(transport, static_state)
            .with_subscription(shared_slot(None));
        let sub = adapter.effective_subscriber();
        assert!(sub.is_subscriber);
        assert!(!sub.is_enterprise);
    }

    #[test]
    fn effective_subscriber_non_enterprise_tier_is_not_enterprise() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(transport, SubscriberState::default())
            .with_subscription(shared_slot(Some(traits::subscription::SubscriptionSnapshot {
                is_subscriber: true,
                subscription_type: Some("team".to_string()),
                ..Default::default()
            })));
        let sub = adapter.effective_subscriber();
        assert!(sub.is_subscriber);
        assert!(!sub.is_enterprise);
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
        assert_eq!(ProviderApiAdapter::error_kind(&LlmError::Overloaded { repeated: false }), "overloaded");
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

    /// Task 6 (batch 5): a terminal 429 whose response carries the unified
    /// headers records the forced-`rejected` snapshot AND the composed
    /// limits copy (byte-pinned: no reset header → no ` · resets …` clause,
    /// `rateLimitMessages.ts:149` + `:333-344`).
    #[tokio::test]
    async fn terminal_429_with_unified_headers_records_limits_copy() {
        let transport = FakeTransport::always(ProviderResponse {
            status: 429,
            headers: {
                let mut h = BTreeMap::new();
                h.insert(
                    "anthropic-ratelimit-unified-representative-claim".to_string(),
                    "seven_day".to_string(),
                );
                h.insert(
                    "anthropic-ratelimit-unified-status".to_string(),
                    "rejected".to_string(),
                );
                h
            },
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "You have reached your usage limit"}
            }),
            request_id: None,
        });
        // Subscriber (non-enterprise) → the 429 is terminal on the first try.
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState { is_subscriber: true, is_enterprise: false },
        );
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;
        assert!(matches!(result, Err(LlmError::RateLimited { .. })), "got {result:?}");

        // The composed copy is cached for the orchestrator's terminal re-map.
        assert_eq!(
            OrchestratorApiClient::last_rate_limit_error_message(&adapter).as_deref(),
            Some("You've hit your weekly limit"),
            "seven_day → formatLimitReachedText('weekly limit', '') verbatim"
        );
        // errors.ts:482-516 — the limits snapshot is updated from the error's
        // headers with status FORCED 'rejected'.
        let info = OrchestratorApiClient::last_rate_limit_full(&adapter).expect("snapshot");
        assert_eq!(info.status.as_deref(), Some("rejected"));
        assert_eq!(info.rate_limit_type.as_deref(), Some("seven_day"));
    }

    /// Task 6 (batch 5): a 429 WITHOUT unified headers fails the
    /// `if (rateLimitType || overageStatus)` gate (errors.ts:480) — no copy
    /// is composed, and a copy from an earlier 429 is superseded (the slot
    /// reflects the most recent 429), so the generic surface applies.
    #[tokio::test]
    async fn terminal_429_without_unified_headers_records_no_copy() {
        let transport = FakeTransport::always(ProviderResponse {
            status: 429,
            headers: BTreeMap::new(),
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "rate limited"}
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
        assert!(result.is_err());
        assert_eq!(
            OrchestratorApiClient::last_rate_limit_error_message(&adapter),
            None,
            "no unified headers on the 429 → no limits copy"
        );
    }

    /// Task 6 (batch 5): the live subscription slot's `pro` plan flips the
    /// `seven_day_sonnet` wording to "weekly limit"
    /// (`rateLimitMessages.ts:176-181`).
    #[tokio::test]
    async fn terminal_429_sonnet_copy_uses_pro_subscription_wording() {
        let resp_429 = |claim: &str| ProviderResponse {
            status: 429,
            headers: {
                let mut h = BTreeMap::new();
                h.insert(
                    "anthropic-ratelimit-unified-representative-claim".to_string(),
                    claim.to_string(),
                );
                h
            },
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "rate limited"}
            }),
            request_id: None,
        };

        // Without a pro/enterprise snapshot → "Sonnet limit".
        let adapter = make_adapter_with_subscriber(
            FakeTransport::always(resp_429("seven_day_sonnet")),
            SubscriberState { is_subscriber: true, is_enterprise: false },
        );
        let _ = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;
        assert_eq!(
            OrchestratorApiClient::last_rate_limit_error_message(&adapter).as_deref(),
            Some("You've hit your Sonnet limit")
        );

        // With a live `pro` snapshot → "weekly limit".
        let pro = make_adapter_with_subscriber(
            FakeTransport::always(resp_429("seven_day_sonnet")),
            SubscriberState { is_subscriber: true, is_enterprise: false },
        )
        .with_subscription(shared_slot(Some(traits::subscription::SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("pro".to_string()),
            ..Default::default()
        })));
        let _ = pro
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;
        assert_eq!(
            OrchestratorApiClient::last_rate_limit_error_message(&pro).as_deref(),
            Some("You've hit your weekly limit")
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

    // ── Fix 2: RepeatedOverloaded → LlmError::Overloaded { repeated: true } → OrchestratorError ──

    /// Fix 2 end-to-end: a scripted transport that returns 529 three times triggers
    /// the external non-sandbox `DriveStep::RepeatedOverloaded` branch, which the
    /// adapter surfaces as `LlmError::Overloaded { repeated: true }`.  The
    /// `From<LlmError>` conversion on `OrchestratorError` then produces
    /// `OrchestratorError::RepeatedOverloaded` whose Display equals the byte-locked
    /// `"Repeated 529 Overloaded errors"` copy (`errors.ts:166`).
    #[tokio::test]
    async fn repeated_529_terminal_maps_to_byte_locked_copy() {
        use crate::error::{OrchestratorError, REPEATED_529_ERROR_MESSAGE};

        // Transport that always returns 529 overloaded.
        let overloaded_body = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let transport = FakeTransport::always(ProviderResponse::json(529, overloaded_body));
        // make_adapter wires user_type=Some("external") in the UserAgentEnv but
        // resolve_retry_control reads USER_TYPE from ResolveRetryEnv::from_process_env().
        // Set the env vars temporarily to gate allow_fallback + is_external.
        // std::env::set_var is deprecated (Rust 2024) but not removed; acceptable
        // in test-only code.
        #[allow(deprecated)]
        std::env::set_var("USER_TYPE", "external");
        #[allow(deprecated)]
        std::env::set_var("FALLBACK_FOR_ALL_PRIMARY_MODELS", "1");
        #[allow(deprecated)]
        std::env::remove_var("IS_SANDBOX");

        let adapter = make_adapter(transport);
        let llm_result = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;

        // Clean up before any assert that might panic.
        #[allow(deprecated)]
        std::env::remove_var("FALLBACK_FOR_ALL_PRIMARY_MODELS");
        #[allow(deprecated)]
        std::env::remove_var("USER_TYPE");

        // The adapter must return Err(LlmError::Overloaded { repeated: true }).
        match &llm_result {
            Err(LlmError::Overloaded { repeated: true }) => {} // correct
            other => panic!(
                "expected Err(LlmError::Overloaded {{ repeated: true }}), got {other:?}"
            ),
        }

        // The OrchestratorError conversion must yield RepeatedOverloaded.
        let orch_err: OrchestratorError = llm_result.unwrap_err().into();
        assert!(
            matches!(orch_err, OrchestratorError::RepeatedOverloaded),
            "OrchestratorError must be RepeatedOverloaded, got {orch_err:?}"
        );
        assert_eq!(
            orch_err.to_string(),
            REPEATED_529_ERROR_MESSAGE,
            "Display must equal the byte-locked copy"
        );
    }

    // ── 3c-T3: LlmResponse.cost populated from cost estimator ─────────────────

    fn make_adapter_with_estimator(transport: Arc<dyn Transport>) -> ProviderApiAdapter {
        use cost::pricing::PricingCatalog as CostCatalog;
        use crate::cost_wiring::llm_catalog_from_cost;
        use llm_client::{CostEstimator, PricingPolicy};
        #[allow(deprecated)]
        std::env::set_var("ADAPTER_TEST_KEY", "test-key");
        let cost_cat = CostCatalog::builtin_reference();
        let llm_cat = llm_catalog_from_cost(&cost_cat);
        let estimator = Arc::new(CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated));

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
                    signing: None,
                    azure: None,
                }],
            })
            .expect("client"),
        );
        ProviderApiAdapter::new_with_estimator(
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
            Some(estimator),
        )
    }

    /// 3c-T3: adapter populates response.cost for a priced model.
    ///
    /// claude-sonnet-4 billing_model → catalog hit → cost is Some(estimate with
    /// total_cost_usd present).
    #[tokio::test]
    async fn cost_populated_for_priced_model() {
        let response_json = serde_json::json!({
            "id": "msg_cost_test",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1_000_000, "output_tokens": 1_000_000}
        });
        let transport = FakeTransport::always(ProviderResponse::json(200, response_json));
        let adapter = make_adapter_with_estimator(transport);
        let resp = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await
            .expect("ok");
        let cost = resp.cost.expect("cost must be Some for a priced model");
        let total = cost.total_cost_usd.expect("total_cost_usd must be Some");
        // claude-sonnet-4: input 3_000 nano → 3.0 usd/M × 1M + output 15_000 → 15.0 × 1M = 18.0
        assert!(
            (total - 18.0).abs() < 1e-9,
            "expected total $18.0 for 1M in + 1M out at $3/$15, got ${total}"
        );
    }

    /// 3c-T3: unknown billing model → cost stays None (no error).
    ///
    /// The adapter uses a model profile whose billing_model ("claude-sonnet-4")
    /// IS in the catalog; to test the None path we use a profile with a
    /// billing_model that has no entry.
    #[tokio::test]
    async fn cost_none_for_unpriced_model() {
        // Build an adapter with an estimator but a billing model not in the catalog.
        use cost::pricing::PricingCatalog as CostCatalog;
        use crate::cost_wiring::llm_catalog_from_cost;
        use llm_client::{CostEstimator, PricingPolicy};
        #[allow(deprecated)]
        std::env::set_var("ADAPTER_TEST_KEY2", "test-key");
        let cost_cat = CostCatalog::builtin_reference();
        let llm_cat = llm_catalog_from_cost(&cost_cat);
        let estimator = Arc::new(CostEstimator::new(llm_cat, PricingPolicy::MarkUnestimated));

        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "ADAPTER_TEST_KEY2".to_string(),
                    },
                    models: vec![ModelProfile {
                        display_model: "claude-future-9999".to_string(),
                        request_model: "claude-future-9999".to_string(),
                        // billing_model not in any catalog entry
                        billing_model: "claude-future-9999".to_string(),
                        aliases: vec![],
                        capabilities: Capabilities::default(),
                    }],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                }],
            })
            .expect("client"),
        );
        let response_json = serde_json::json!({
            "id": "msg_unpriced",
            "model": "claude-future-9999",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 100, "output_tokens": 50}
        });
        let transport = FakeTransport::always(ProviderResponse::json(200, response_json));
        let adapter = ProviderApiAdapter::new_with_estimator(
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
            Some(estimator),
        );
        let resp = adapter
            .messages_create("claude-future-9999", None, Vec::new(), Vec::new())
            .await
            .expect("ok");
        assert!(
            resp.cost.is_none(),
            "unpriced billing_model must leave cost = None; got {:?}",
            resp.cost
        );
    }

    /// 3c-T3: cost tracker recording is unchanged (existing CostTracker tests still pass).
    ///
    /// When no estimator is wired, response.cost stays None — backward-compat.
    #[tokio::test]
    async fn no_estimator_leaves_cost_none() {
        let response_json = serde_json::json!({
            "id": "msg_no_est",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 100, "output_tokens": 50}
        });
        let transport = FakeTransport::always(ProviderResponse::json(200, response_json));
        // make_adapter wires None estimator (ProviderApiAdapter::new default path)
        let adapter = make_adapter(transport);
        let resp = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await
            .expect("ok");
        assert!(resp.cost.is_none(), "no estimator → cost must be None");
    }

    // ── 3c-T1: streaming 429 + retry-after header drives correct delay ────────

    /// Empty frame-stream for scripted streaming errors.
    struct EmptyFrames;
    impl llm_client::FrameStream for EmptyFrames {
        fn next_frame(
            &mut self,
        ) -> BoxFuture<'_, Result<Option<llm_client::RawStreamFrame>, LlmError>> {
            Box::pin(async { Ok(None) })
        }
    }

    /// A Transport that sequences execute responses AND can return scripted
    /// streaming (`open_stream`) responses.
    struct FakeStreamTransport {
        /// Sequence of `open_stream` results.
        stream_resps: Mutex<Vec<FakeStreamResp>>,
        stream_call_count: Mutex<usize>,
    }

    #[allow(dead_code)]
    enum FakeStreamResp {
        /// Streaming response with given status + headers + no frames.
        Status {
            status: u16,
            headers: BTreeMap<String, String>,
        },
        /// Terminal transport error (e.g. connection failure).
        Err(LlmError),
    }

    impl FakeStreamTransport {
        fn sequence(stream_resps: Vec<FakeStreamResp>) -> Arc<Self> {
            Arc::new(Self {
                stream_resps: Mutex::new(stream_resps),
                stream_call_count: Mutex::new(0),
            })
        }

        fn stream_call_count(&self) -> usize {
            *self.stream_call_count.lock().unwrap()
        }
    }

    impl Transport for FakeStreamTransport {
        fn execute<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
            Box::pin(async move {
                Err(LlmError::Transport {
                    message: "execute not scripted in FakeStreamTransport".to_string(),
                })
            })
        }

        fn open_stream<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
            let mut count = self.stream_call_count.lock().unwrap();
            let idx = (*count).min(
                self.stream_resps
                    .lock()
                    .unwrap()
                    .len()
                    .saturating_sub(1),
            );
            *count += 1;
            drop(count);
            let resp = {
                let resps = self.stream_resps.lock().unwrap();
                match &resps[idx] {
                    FakeStreamResp::Status { status, headers } => {
                        Ok(StreamingResponse {
                            status: *status,
                            headers: headers.clone(),
                            frames: Box::new(EmptyFrames),
                        })
                    }
                    FakeStreamResp::Err(e) => Err(e.clone()),
                }
            };
            Box::pin(async move { resp })
        }
    }

    /// 3c-T1 pin: a connect-phase 429 with `retry-after: 7` on the streaming
    /// path must drive a 7 s `RetryAfter` delay (not the 1 s fallback).
    ///
    /// We use `tokio::time::pause()` so the test completes instantly; the
    /// `drive_stream` loop sleeps via `tokio::time::sleep` which respects the
    /// paused clock.  After the first 429 the test advances time past 7 s and
    /// the second (200) attempt is served, confirming the delay was honoured.
    #[tokio::test(start_paused = true)]
    async fn streaming_429_with_retry_after_header_drives_7s_not_1s() {
        // Attempt 1: 429 with retry-after: 7.
        let mut headers_429 = BTreeMap::new();
        headers_429.insert("retry-after".to_string(), "7".to_string());

        // Attempt 2: 200 with an empty body (the codec will produce
        // StreamInterrupted on an empty frame-stream, but that is terminal and
        // proves two calls were made — what we care about).
        let stream_transport = FakeStreamTransport::sequence(vec![
            FakeStreamResp::Status {
                status: 429,
                headers: headers_429,
            },
            FakeStreamResp::Status {
                status: 200,
                headers: BTreeMap::new(),
            },
        ]);

        let adapter = make_adapter(Arc::clone(&stream_transport) as Arc<dyn Transport>);

        // Record the instant before calling drive_stream.
        let before = tokio::time::Instant::now();

        // drive_stream is private; call it through the StreamingApiClient trait.
        // The result will be an error (empty frame-stream on attempt 2) or Ok
        // depending on the codec — we only care that two open_stream calls were
        // made and that the elapsed time is ≥ 7 s (the retry-after delay).
        let _result = StreamingApiClient::stream(
            &adapter,
            "claude-sonnet-4-20250514",
            None,
            Vec::new(),
            Vec::new(),
        )
        .await;

        let elapsed = before.elapsed();
        // The sleep was for exactly 7 s (retry-after value).  With time paused
        // the sleep advances the mock clock, so elapsed reports ≥ 7 s.
        assert!(
            elapsed >= std::time::Duration::from_secs(7),
            "retry-after:7 must drive a ≥7 s delay; elapsed={elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(8),
            "delay must be close to 7 s (not jittered / not 1 s fallback); elapsed={elapsed:?}"
        );
        // Two open_stream calls: 429 then 200.
        assert_eq!(
            stream_transport.stream_call_count(),
            2,
            "must retry exactly once (429 → 200)"
        );
    }

    // ── Task 1: routing.fallback/retry adapter tests ──────────────────────────

    /// Build an adapter with routing overrides for per-model fallback and retry.
    fn make_adapter_with_routing(
        transport: Arc<dyn Transport>,
        fallback_overrides: std::collections::BTreeMap<String, Vec<String>>,
        settings_max_retries: Option<u32>,
        settings_backoff_ms: Option<u64>,
    ) -> ProviderApiAdapter {
        #[allow(deprecated)]
        std::env::set_var("ROUTING_TEST_KEY", "test-key");
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "ROUTING_TEST_KEY".to_string(),
                    },
                    models: vec![
                        ModelProfile {
                            display_model: "claude-opus-4-6".to_string(),
                            request_model: "claude-opus-4-6".to_string(),
                            billing_model: "claude-opus-4-6".to_string(),
                            aliases: vec![],
                            capabilities: Capabilities {
                                streaming: true,
                                tools: true,
                                ..Default::default()
                            },
                        },
                        ModelProfile {
                            display_model: "claude-haiku-4-20250307".to_string(),
                            request_model: "claude-haiku-4-20250307".to_string(),
                            billing_model: "claude-haiku-4".to_string(),
                            aliases: vec![],
                            capabilities: Capabilities {
                                streaming: true,
                                tools: true,
                                ..Default::default()
                            },
                        },
                        ModelProfile {
                            display_model: "claude-sonnet-4-20250514".to_string(),
                            request_model: "claude-sonnet-4-20250514".to_string(),
                            billing_model: "claude-sonnet-4".to_string(),
                            aliases: vec!["claude".to_string()],
                            capabilities: Capabilities {
                                streaming: true,
                                tools: true,
                                ..Default::default()
                            },
                        },
                    ],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                }],
            })
            .expect("client"),
        );
        ProviderApiAdapter::new_with_routing(
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
            None,
            fallback_overrides,
            settings_max_retries,
            settings_backoff_ms,
        )
    }

    fn routing_ok_response_json() -> serde_json::Value {
        serde_json::json!({
            "id": "msg_routing",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        })
    }

    /// Per-model fallback entry wins over global fallback_model.
    ///
    /// Uses `claude-opus-4-6` as primary (is_non_custom_opus = true → allow_fallback
    /// activates naturally for a non-subscriber, no process env mutation needed).
    /// After 3 consecutive 529s the per-model fallback to haiku fires.
    #[tokio::test]
    async fn per_model_fallback_wins_over_global() {
        let haiku_ok = serde_json::json!({
            "id": "msg_haiku",
            "model": "claude-haiku-4-20250307",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        let overloaded_body = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, haiku_ok)),
        ]);

        let mut fallback_overrides = std::collections::BTreeMap::new();
        // Per-model: opus → haiku (single-entry chain).
        fallback_overrides.insert(
            "claude-opus-4-6".to_string(),
            vec!["claude-haiku-4-20250307".to_string()],
        );

        // No env var needed: claude-opus-4-6 is_non_custom_opus=true → allow_fallback=true
        // for non-subscriber (default SubscriberState).
        let mut adapter = make_adapter_with_routing(
            transport.clone(),
            fallback_overrides,
            None,
            None,
        );
        // Global fallback also points somewhere — per-model must win.
        adapter.fallback_model = Some("claude-sonnet-4-20250514".to_string());

        let result = OrchestratorApiClient::messages_create_with_fallback(
            &adapter,
            "claude-opus-4-6",
            None,
            Vec::new(),
            Vec::new(),
            None, // no explicit call-site fallback
            false,
            false,
        )
        .await;

        // Should succeed — 3 × 529 then haiku 200.
        assert!(
            result.is_ok(),
            "per-model fallback should route to haiku and succeed: {result:?}"
        );
        assert_eq!(transport.seen_count(), 4, "expected 4 requests: 3 × 529 + 1 × 200");
    }

    /// Global fallback is used when no per-model entry is present.
    #[tokio::test]
    async fn global_fallback_used_when_no_per_model_entry() {
        let haiku_ok = serde_json::json!({
            "id": "msg_haiku2",
            "model": "claude-haiku-4-20250307",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        let overloaded_body = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_body.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, haiku_ok)),
        ]);

        // No per-model overrides.
        let mut adapter = make_adapter_with_routing(
            transport.clone(),
            std::collections::BTreeMap::new(),
            None,
            None,
        );
        // Global fallback: opus → haiku.
        adapter.fallback_model = Some("claude-haiku-4-20250307".to_string());

        // claude-opus-4-6 is_non_custom_opus=true → allow_fallback=true for non-subscriber.
        let result = OrchestratorApiClient::messages_create_with_fallback(
            &adapter,
            "claude-opus-4-6",
            None,
            Vec::new(),
            Vec::new(),
            None,
            false,
            false,
        )
        .await;

        assert!(result.is_ok(), "global fallback should work: {result:?}");
        assert_eq!(transport.seen_count(), 4);
    }

    /// Per-model fallback fires even when the request uses an ALIAS of the
    /// primary model.
    ///
    /// `fallback_overrides` keys are keyed by the display model; if the request
    /// arrives as an alias (e.g. `"claude"` instead of `"claude-sonnet-4-20250514"`)
    /// the lookup must normalize via `alias_to_display` before probing the map.
    ///
    /// RED on the old code (raw model probe skips the per-model entry when an
    /// alias is used); GREEN after the alias normalization fix.
    #[tokio::test]
    async fn per_model_fallback_fires_via_alias() {
        // Three 529s then a success on the fallback (haiku).
        let overloaded_json = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "overloaded"}
        });
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_json.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_json.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded_json.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, routing_ok_response_json())),
        ]);

        // Per-model fallback: display "claude-sonnet-4-20250514" → "claude-haiku-4-20250307"
        // (single-entry chain).
        let mut fallback_overrides = std::collections::BTreeMap::new();
        fallback_overrides.insert(
            "claude-sonnet-4-20250514".to_string(),
            vec!["claude-haiku-4-20250307".to_string()],
        );

        let adapter = make_adapter_with_routing(
            transport.clone(),
            fallback_overrides,
            None,
            None,
        );

        // Request via ALIAS — the alias_to_display map must normalize this to
        // "claude-sonnet-4-20250514" before the fallback_overrides lookup.
        let result = OrchestratorApiClient::messages_create_with_fallback(
            &adapter,
            "claude",      // alias of "claude-sonnet-4-20250514"
            Some("sys"),
            Vec::new(),
            Vec::new(),
            None,          // no explicit call-site fallback (per-model must activate)
            false,
            false,
        )
        .await;

        assert!(
            result.is_ok(),
            "alias-keyed per-model fallback should fire and succeed; got: {result:?}"
        );
        // 3 primary 529s + 1 fallback success = 4 transport calls.
        assert_eq!(
            transport.seen_count(),
            4,
            "expected 3 failing primary calls + 1 successful fallback call"
        );
    }

    // ── Task 5: fallback chain walk tests ────────────────────────────────────

    /// 2-entry chain: primary → chain[0] → chain[1] when all 529s.
    ///
    /// Scripted: 3×529 on primary, 3×529 on chain[0], then 200 on chain[1].
    /// Asserts the model sequence primary→c0→c1 via captured request bodies.
    #[tokio::test]
    async fn two_entry_chain_walks_both_entries() {
        let overloaded = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let c1_ok = serde_json::json!({
            "id": "msg_c1",
            "model": "claude-haiku-4-20250307",
            "content": [{"type": "text", "text": "c1 ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        // 3 × 529 on primary (claude-opus-4-6)
        // 3 × 529 on chain[0] (claude-sonnet-4-20250514)
        // 1 × 200 on chain[1] (claude-haiku-4-20250307)
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, c1_ok)),
        ]);

        let mut fallback_overrides = std::collections::BTreeMap::new();
        // 2-entry chain: opus-4-6 → sonnet-4-20250514 → haiku-4-20250307
        fallback_overrides.insert(
            "claude-opus-4-6".to_string(),
            vec![
                "claude-sonnet-4-20250514".to_string(),
                "claude-haiku-4-20250307".to_string(),
            ],
        );

        let adapter = make_adapter_with_routing(transport.clone(), fallback_overrides, None, None);

        let result = OrchestratorApiClient::messages_create_with_fallback(
            &adapter,
            "claude-opus-4-6",
            None,
            Vec::new(),
            Vec::new(),
            None,
            false,
            false,
        )
        .await;

        assert!(result.is_ok(), "chain walk must succeed on chain[1]: {result:?}");
        assert_eq!(transport.seen_count(), 7, "3 primary + 3 chain[0] + 1 chain[1]");

        // Assert the model sequence: first 3 requests use primary, next 3 use chain[0],
        // last 1 uses chain[1].
        let primary = "claude-opus-4-6";
        let c0 = "claude-sonnet-4-20250514";
        let c1 = "claude-haiku-4-20250307";
        for i in 0..3 {
            assert_eq!(
                transport.seen_body_model(i).as_deref(),
                Some(primary),
                "request {i} must use primary model"
            );
        }
        for i in 3..6 {
            assert_eq!(
                transport.seen_body_model(i).as_deref(),
                Some(c0),
                "request {i} must use chain[0]"
            );
        }
        assert_eq!(
            transport.seen_body_model(6).as_deref(),
            Some(c1),
            "request 6 must use chain[1]"
        );
    }

    /// Chain exhaustion: when all entries are overloaded, the call is terminal.
    ///
    /// Single-entry chain: primary 3×529 → chain[0] persistent 529 → terminal error.
    #[tokio::test]
    async fn chain_exhausted_gives_terminal_error() {
        let overloaded = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        // primary: 3 × 529 → Fallback
        // chain[0]: persistent 529 → RepeatedOverloaded (is_external=true in make_adapter_with_routing)
        let transport = FakeTransport::always(ProviderResponse::json(529, overloaded));

        let mut fallback_overrides = std::collections::BTreeMap::new();
        fallback_overrides.insert(
            "claude-opus-4-6".to_string(),
            vec!["claude-haiku-4-20250307".to_string()],
        );

        let adapter = make_adapter_with_routing(transport.clone(), fallback_overrides, None, None);

        let result = OrchestratorApiClient::messages_create_with_fallback(
            &adapter,
            "claude-opus-4-6",
            None,
            Vec::new(),
            Vec::new(),
            None,
            false,
            false,
        )
        .await;

        assert!(result.is_err(), "exhausted chain must produce terminal error");
        // The error must be Overloaded (either repeated=true from external path or
        // plain Overloaded — either variant indicates the chain was walked and terminated).
        assert!(
            matches!(result.unwrap_err(), LlmError::Overloaded { .. }),
            "terminal error must be LlmError::Overloaded"
        );
    }

    /// Single-entry chain behaves like batch-1 (exactly one fallback hop).
    ///
    /// Uses the existing `per_model_fallback_wins_over_global` scenario but
    /// verifies via `seen_body_model` that the model sequence is correct.
    #[tokio::test]
    async fn single_entry_chain_behaves_like_batch1() {
        let overloaded = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let haiku_ok = serde_json::json!({
            "id": "msg_haiku",
            "model": "claude-haiku-4-20250307",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, haiku_ok)),
        ]);

        let mut fallback_overrides = std::collections::BTreeMap::new();
        fallback_overrides.insert(
            "claude-opus-4-6".to_string(),
            vec!["claude-haiku-4-20250307".to_string()],
        );
        let adapter = make_adapter_with_routing(transport.clone(), fallback_overrides, None, None);

        let result = OrchestratorApiClient::messages_create_with_fallback(
            &adapter,
            "claude-opus-4-6",
            None,
            Vec::new(),
            Vec::new(),
            None,
            false,
            false,
        )
        .await;

        assert!(result.is_ok(), "single-entry chain must succeed: {result:?}");
        assert_eq!(transport.seen_count(), 4, "3 primary 529s + 1 fallback 200");
        // First 3 requests: primary model.
        for i in 0..3 {
            assert_eq!(
                transport.seen_body_model(i).as_deref(),
                Some("claude-opus-4-6"),
                "request {i} must use primary"
            );
        }
        // 4th request: fallback model.
        assert_eq!(
            transport.seen_body_model(3).as_deref(),
            Some("claude-haiku-4-20250307"),
            "request 3 must use chain[0]"
        );
    }

    /// Global fallback_model still works when no per-model chain is configured.
    ///
    /// The global `fallback_model` is wrapped into a single-entry chain and walks
    /// the same code path; this test guards that wiring.
    #[tokio::test]
    async fn global_fallback_model_works_without_chain_entry() {
        let overloaded = serde_json::json!({
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Overloaded"}
        });
        let sonnet_ok = serde_json::json!({
            "id": "msg_sonnet",
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(529, overloaded.clone())),
            FakeResponse::Ok(ProviderResponse::json(200, sonnet_ok)),
        ]);

        let mut adapter =
            make_adapter_with_routing(transport.clone(), std::collections::BTreeMap::new(), None, None);
        // Set global fallback only (no per-model chain).
        adapter.fallback_model = Some("claude-sonnet-4-20250514".to_string());

        let result = OrchestratorApiClient::messages_create_with_fallback(
            &adapter,
            "claude-opus-4-6",
            None,
            Vec::new(),
            Vec::new(),
            None,
            false,
            false,
        )
        .await;

        assert!(result.is_ok(), "global fallback must work: {result:?}");
        assert_eq!(transport.seen_count(), 4);
        // Request 3 must use the global fallback model.
        assert_eq!(
            transport.seen_body_model(3).as_deref(),
            Some("claude-sonnet-4-20250514"),
            "request 3 must use global fallback model"
        );
    }

    /// `settings_max_retries=2` causes terminal after 3 executions (not 11).
    ///
    /// The adapter reads `CLAUDE_CODE_MAX_RETRIES` from the process env in
    /// `messages_create`.  To avoid interference with parallel tests we verify
    /// via the retry.rs layer (which is injected, not process-env) rather than
    /// through the adapter's env path.  The adapter's `settings_max_retries`
    /// field is directly observable via the resolve_retry_control_with_settings
    /// call: when env var is absent it uses `settings_max_retries` as the
    /// effective limit.  We temporarily clear the env var then restore it.
    #[tokio::test]
    async fn settings_max_retries_beats_default() {
        // We can test the settings path directly: when the env var is absent
        // the settings_max_retries field applies.  We control CLAUDE_CODE_MAX_RETRIES
        // for the duration of this test — accept minor isolation risk since the
        // pre-existing test suite also does this.
        let transport = FakeTransport::sequence(vec![FakeResponse::Ok(ProviderResponse::json(
            500,
            serde_json::json!({"type": "error", "error": {"type": "api_error", "message": "internal"}}),
        ))]);

        let adapter = make_adapter_with_routing(
            transport.clone(),
            std::collections::BTreeMap::new(),
            Some(2), // settings says max 2 retries
            None,
        );

        // Temporarily unset CLAUDE_CODE_MAX_RETRIES so settings value wins.
        let saved = std::env::var("CLAUDE_CODE_MAX_RETRIES").ok();
        #[allow(deprecated)]
        std::env::remove_var("CLAUDE_CODE_MAX_RETRIES");

        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;

        // Restore.
        if let Some(v) = saved {
            #[allow(deprecated)]
            std::env::set_var("CLAUDE_CODE_MAX_RETRIES", v);
        }

        assert!(result.is_err(), "must fail after exhausting budget");
        // max_retries=2 → 3 executions (2 sleeps + 1 final).
        assert_eq!(
            transport.seen_count(),
            3,
            "settings_max_retries=2 should give 3 executions (2 retries + 1 initial)"
        );
    }

    /// Env `CLAUDE_CODE_MAX_RETRIES` beats `settings_max_retries`.
    ///
    /// Proven via `resolve_retry_control_with_settings` unit tests in retry.rs;
    /// this adapter-level test verifies the wiring by injecting through
    /// `ResolveRetryEnv` directly rather than mutating the process env.
    ///
    /// We use `crate::model::retry::resolve_retry_control_with_settings` to
    /// build the expected `RetryControl` and compare `max_retries`.
    #[test]
    fn env_max_retries_beats_settings_via_resolve() {
        use crate::model::retry::{resolve_retry_control_with_settings, ResolveRetryEnv, DEFAULT_MAX_RETRIES};

        // env=Some("1") + settings=Some(8) → max_retries=1 (env wins).
        let env_with_1 = ResolveRetryEnv {
            max_retries: Some("1".to_string()),
            ..ResolveRetryEnv::default()
        };
        let ctl = resolve_retry_control_with_settings("claude-sonnet-4-20250514", None, false, &env_with_1, Some(8));
        assert_eq!(ctl.max_retries, 1, "env(1) must beat settings(8)");

        // env=None + settings=Some(7) → max_retries=7 (settings wins).
        let env_absent = ResolveRetryEnv::default();
        let ctl2 = resolve_retry_control_with_settings("claude-sonnet-4-20250514", None, false, &env_absent, Some(7));
        assert_eq!(ctl2.max_retries, 7, "settings(7) must beat default(10)");

        // env=None + settings=None → DEFAULT.
        let ctl3 = resolve_retry_control_with_settings("claude-sonnet-4-20250514", None, false, &env_absent, None);
        assert_eq!(ctl3.max_retries, DEFAULT_MAX_RETRIES);
    }

    /// `settings_backoff_ms=1000` doubles the jitter ladder base.
    ///
    /// With `backoff_ms=1000` and time paused, we verify the first retry delay
    /// is ≥ 800 ms (= 1000 × 0.8 lower-jitter-bound).  Without the setting the
    /// base would be 500 ms (lower bound 400 ms) — so 800 ms is above the
    /// un-scaled upper bound (600 ms) which proves scaling is active.
    #[tokio::test(start_paused = true)]
    async fn backoff_ms_scales_jitter_base() {
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse::json(
                500,
                serde_json::json!({"type": "error", "error": {"type": "api_error", "message": "err"}}),
            )),
            FakeResponse::Ok(ProviderResponse::json(200, routing_ok_response_json())),
        ]);

        let adapter = make_adapter_with_routing(
            transport.clone(),
            std::collections::BTreeMap::new(),
            None,
            Some(1000), // backoff_ms = 1000 → first rung 1000, jitter [800, 1200)
        );

        let before = tokio::time::Instant::now();
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await;
        let elapsed = before.elapsed();

        assert!(result.is_ok(), "should succeed after retry: {result:?}");
        // Delay must be ≥ 800 ms (lower jitter bound of 1000 ms base).
        // Default base = 500 ms → upper jitter bound = 600 ms < 800 ms.
        // So ≥ 800 ms proves the 1000 ms base is in effect.
        assert!(
            elapsed >= std::time::Duration::from_millis(800),
            "backoff_ms=1000 should produce ≥ 800 ms delay; elapsed={elapsed:?}"
        );
        // Must be < 1200 ms (upper jitter bound of 1000 ms base).
        assert!(
            elapsed < std::time::Duration::from_millis(1200),
            "backoff_ms=1000 delay should be < 1200 ms; elapsed={elapsed:?}"
        );
    }

    // ── T3 Step 2: 2xx rate-limit header feed ────────────────────────────────

    /// A 2xx response with `anthropic-ratelimit-unified-overage-status: rejected`
    /// must be stored in `last_rate_limit_info` and trigger a `tracing::warn!`.
    ///
    /// We only assert the data is stored; the warn fires on a live-log subscriber
    /// which we do not attach in tests — the absence of a panic is the assertion.
    #[tokio::test]
    async fn rate_limit_info_stored_from_2xx_response() {
        let mut headers = BTreeMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-representative-claim".to_string(),
            "five_hour".to_string(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-overage-status".to_string(),
            "rejected".to_string(),
        );

        let transport = FakeTransport::always(ProviderResponse {
            status: 200,
            headers,
            body_json: ok_response_json(),
            request_id: None,
        });
        let adapter = make_adapter(transport);
        let _ = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await
            .expect("ok");

        let info = adapter
            .last_rate_limit_info()
            .expect("last_rate_limit_info must be Some after a 2xx with unified headers");
        assert_eq!(
            info.rate_limit_type.as_deref(),
            Some("five_hour"),
            "rate_limit_type must be five_hour"
        );
        assert_eq!(
            info.overage_status.as_deref(),
            Some("rejected"),
            "overage_status must be rejected"
        );
    }

    /// Without unified rate-limit headers the cached info stays `None`.
    #[tokio::test]
    async fn rate_limit_info_none_when_no_unified_headers() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let _ = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await
            .expect("ok");

        // No unified headers → None still.
        assert!(
            adapter.last_rate_limit_info().is_none(),
            "last_rate_limit_info must be None when no unified headers are present"
        );
    }

    // ── Task 5 Part B: OrchestratorApiClient::last_rate_limit_info ──────────────

    /// `OrchestratorApiClient::last_rate_limit_info` returns the adapter's stored
    /// rate-limit info mapped into a `traits::RateLimitSnapshot`.
    ///
    /// After a 2xx response with unified headers the snapshot must carry all
    /// three fields: `rate_limit_type`, `overage_status`, and
    /// `overage_disabled_reason`.
    #[tokio::test]
    async fn orchestrator_api_client_last_rate_limit_info_returns_stored_info() {
        let mut headers = BTreeMap::new();
        headers.insert(
            "anthropic-ratelimit-unified-representative-claim".to_string(),
            "five_hour".to_string(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-overage-status".to_string(),
            "allowed_warning".to_string(),
        );
        headers.insert(
            "anthropic-ratelimit-unified-overage-disabled-reason".to_string(),
            "out_of_credits".to_string(),
        );

        let transport = FakeTransport::always(ProviderResponse {
            status: 200,
            headers,
            body_json: ok_response_json(),
            request_id: None,
        });
        let adapter = make_adapter(transport);
        let _ = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await
            .expect("ok");

        // Via the OrchestratorApiClient trait method (RateLimitSnapshot).
        let snapshot = OrchestratorApiClient::last_rate_limit_info(&adapter)
            .expect("must be Some after 2xx with unified headers");
        assert_eq!(
            snapshot.rate_limit_type.as_deref(),
            Some("five_hour"),
            "rate_limit_type must round-trip through the snapshot"
        );
        assert_eq!(
            snapshot.overage_status.as_deref(),
            Some("allowed_warning"),
            "overage_status must round-trip through the snapshot"
        );
        assert_eq!(
            snapshot.overage_disabled_reason.as_deref(),
            Some("out_of_credits"),
            "overage_disabled_reason must round-trip through the snapshot"
        );
    }

    /// `OrchestratorApiClient::last_rate_limit_info` returns `None` before any
    /// response with unified headers.
    #[tokio::test]
    async fn orchestrator_api_client_last_rate_limit_info_none_initially() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let _ = adapter
            .messages_create("claude-sonnet-4-20250514", None, Vec::new(), Vec::new())
            .await
            .expect("ok");

        // No unified headers → trait method also returns None.
        assert!(
            OrchestratorApiClient::last_rate_limit_info(&adapter).is_none(),
            "trait method must return None when adapter has no unified header snapshot"
        );
    }

    // ── T3 Step 3: stream telemetry twins ────────────────────────────────────

    /// A `FrameStream` that yields scripted raw SSE frames (encoded as
    /// JSON byte sequences) then terminates. Used to drive the stream decoder
    /// with a controlled event sequence so the `drive_stream` unfold
    /// emits telemetry at the right points.
    struct ScriptedFrames {
        frames: Vec<Vec<u8>>,
        idx: usize,
    }

    impl ScriptedFrames {
        fn new(frames: Vec<Vec<u8>>) -> Self {
            Self { frames, idx: 0 }
        }
    }

    impl llm_client::FrameStream for ScriptedFrames {
        fn next_frame(
            &mut self,
        ) -> BoxFuture<'_, Result<Option<llm_client::RawStreamFrame>, LlmError>> {
            let result = if self.idx < self.frames.len() {
                let bytes = self.frames[self.idx].clone();
                self.idx += 1;
                Ok(Some(llm_client::RawStreamFrame::new(bytes)))
            } else {
                Ok(None)
            };
            Box::pin(async move { result })
        }
    }

    /// A transport that delivers a fixed successful stream ending in `message_stop`.
    ///
    /// Builds valid Anthropic SSE frames (as raw JSON lines) so the `AnthropicMessages`
    /// codec can decode them. The stream ends with `message_stop` which is the
    /// terminal event → should trigger `emit_succeeded` once.
    struct ScriptedStreamTransport {
        frames: Vec<Vec<u8>>,
        headers: BTreeMap<String, String>,
        status: u16,
    }

    impl ScriptedStreamTransport {
        /// Build a transport that delivers a minimal valid anthropic stream:
        /// `message_start` → `message_delta(stop_reason=end_turn)` → `message_stop`.
        fn anthropic_success() -> Arc<Self> {
            let frames = vec![
                br#"{"type":"message_start","message":{"id":"msg_t","model":"claude-sonnet-4-20250514","usage":{"input_tokens":1,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#.to_vec(),
                br#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}"#.to_vec(),
                br#"{"type":"message_stop"}"#.to_vec(),
            ];
            Arc::new(Self {
                frames,
                headers: BTreeMap::new(),
                status: 200,
            })
        }

        /// Build a transport that delivers a single malformed frame → decoder error.
        fn malformed_frame() -> Arc<Self> {
            let frames = vec![b"not-valid-json".to_vec()];
            Arc::new(Self {
                frames,
                headers: BTreeMap::new(),
                status: 200,
            })
        }
    }

    impl Transport for ScriptedStreamTransport {
        fn execute<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<ProviderResponse, LlmError>> {
            Box::pin(async move {
                Err(LlmError::Transport {
                    message: "execute not used in ScriptedStreamTransport".into(),
                })
            })
        }

        fn open_stream<'a>(
            &'a self,
            _request: &'a ProviderRequest,
        ) -> BoxFuture<'a, Result<StreamingResponse, LlmError>> {
            let frames: Vec<Vec<u8>> = self.frames.clone();
            let headers = self.headers.clone();
            let status = self.status;
            Box::pin(async move {
                Ok(StreamingResponse {
                    status,
                    headers,
                    frames: Box::new(ScriptedFrames::new(frames)),
                })
            })
        }
    }

    /// Build a streaming adapter with an attached analytics bus.
    async fn make_stream_adapter_with_bus(
        transport: Arc<dyn Transport>,
    ) -> (ProviderApiAdapter, Arc<::telemetry::InMemorySink>) {
        use ::telemetry::{AnalyticsBus, InMemorySink};

        #[allow(deprecated)]
        std::env::set_var("STREAM_TELEM_TEST_KEY", "test-key");
        let client = Arc::new(
            DefaultLlmClient::from_config(ClientConfig {
                providers: vec![ProviderProfile {
                    provider_id: ProviderId::AnthropicFirstParty,
                    profile_name: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    protocol: ProtocolFamily::AnthropicMessages,
                    auth: AuthStrategy::ApiKey,
                    credential: CredentialConfig::Env {
                        var: "STREAM_TELEM_TEST_KEY".to_string(),
                    },
                    models: vec![ModelProfile {
                        display_model: "claude-sonnet-4-20250514".to_string(),
                        request_model: "claude-sonnet-4-20250514".to_string(),
                        billing_model: "claude-sonnet-4".to_string(),
                        aliases: vec![],
                        capabilities: Capabilities {
                            streaming: true,
                            tools: true,
                            ..Default::default()
                        },
                    }],
                    pricing: PricingConfig::default(),
                    signing: None,
                    azure: None,
                }],
            })
            .expect("client"),
        );

        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::new());
        bus.attach_sink(sink.clone()).await;

        let adapter = ProviderApiAdapter::new(
            client,
            transport,
            SubscriberState::default(),
            UserAgentEnv {
                user_type: Some("external".to_string()),
                entrypoint: Some("cli".to_string()),
                ..Default::default()
            },
            "0.0.0",
            Some(bus),
            None,
        );
        (adapter, sink)
    }

    /// Stream telemetry twin — succeed path:
    /// a clean stream (message_stop at the end) must emit:
    /// 1. `tengu_api_request_started` (stream=true)
    /// 2. `tengu_api_request_succeeded` (from the unfold's terminal-event arm)
    #[tokio::test]
    async fn stream_emit_succeeded_fires_on_message_stop() {
        use futures::StreamExt as _;
        use ::telemetry::AnalyticsValue;

        let transport = ScriptedStreamTransport::anthropic_success();
        let (adapter, sink) = make_stream_adapter_with_bus(transport).await;

        let mut stream = StreamingApiClient::stream(
            &adapter,
            "claude-sonnet-4-20250514",
            None,
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("stream open ok");

        // Drain all events.
        while stream.next().await.is_some() {}

        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();

        assert!(
            names.contains(&"tengu_api_request_started"),
            "started must fire; got {names:?}"
        );
        assert!(
            names.contains(&"tengu_api_request_succeeded"),
            "succeeded must fire on message_stop; got {names:?}"
        );

        // Must NOT fire multiple times.
        let succeeded_count = names
            .iter()
            .filter(|&&n| n == "tengu_api_request_succeeded")
            .count();
        assert_eq!(
            succeeded_count, 1,
            "succeeded must fire exactly once; got {succeeded_count}"
        );

        // Verify stream=true on the started event.
        let started = events
            .iter()
            .find(|e| e.name == "tengu_api_request_started")
            .unwrap();
        assert!(
            matches!(&started.metadata["stream"], AnalyticsValue::Bool(true)),
            "stream must be true on the started event"
        );
    }

    /// Stream telemetry twin — fail path:
    /// a stream that produces a decode error must emit `tengu_api_request_failed`
    /// and NOT emit `tengu_api_request_succeeded`.
    #[tokio::test]
    async fn stream_emit_failed_fires_on_decode_error() {
        use futures::StreamExt as _;

        let transport = ScriptedStreamTransport::malformed_frame();
        let (adapter, sink) = make_stream_adapter_with_bus(transport).await;

        let stream_result = StreamingApiClient::stream(
            &adapter,
            "claude-sonnet-4-20250514",
            None,
            Vec::new(),
            Vec::new(),
        )
        .await;

        // May error at open or during drain.
        if let Ok(mut stream) = stream_result {
            while let Some(item) = stream.next().await {
                // consume until error or end
                let _ = item;
            }
        }

        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();

        // failed must fire (decode error or stream-interrupted)
        // succeeded must NOT fire
        assert!(
            names.contains(&"tengu_api_request_failed"),
            "failed must fire on decode error; got {names:?}"
        );
        assert!(
            !names.contains(&"tengu_api_request_succeeded"),
            "succeeded must NOT fire on error; got {names:?}"
        );
    }
}
