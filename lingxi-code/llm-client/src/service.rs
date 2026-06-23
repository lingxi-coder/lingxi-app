//! Provider-neutral API service: drive the LLM client with full
//! retry/rate-limit/betas.
//!
//! `ApiService` owns the inherent drive loop relocated from the orchestrator's
//! `ProviderApiAdapter` (`orchestrator/src/provider_adapter.rs`). It speaks
//! `protocol::ConversationMessage` + llm-client types and references only
//! `crate::*` + `protocol`/`traits`/`telemetry` — never any orchestrator-internal
//! path. The orchestrator's consumer-trait impls delegate to it 1:1.

use crate::convert::{
    ensure_tool_result_pairing, normalize_messages_for_api, to_llm_messages, to_tool_declarations,
};
use crate::model::betas::{apply_beta_header_with_auth, BetaContext, Endpoint, Provider};
use crate::model::rate_limit::{
    formatted_reset_times_from_headers, parse_retry_after, parse_unified_reset,
    rate_limit_error_message, RateLimitInfo, RawUtilization, SubscriptionContext,
};
use crate::model::retry::{
    next_step_with_backoff, resolve_retry_control_with_settings, DriveStep, ResolveRetryEnv,
    RetryControl, RetryState,
};
use crate::model::telemetry;
use crate::model::user_agent::{user_agent, UserAgentEnv};
use crate::{
    CacheControl, CostEstimator, DefaultLlmClient, LlmError, LlmEvent, LlmRequest, LlmResponse,
    ResponsesWebSocketSession, Transport,
};
use futures::stream::BoxStream;
use protocol::{ContentBlock, ConversationMessage};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Mirror claude-code `getPromptCachingEnabled` (services/api/claude.ts:333).
///
/// Prompt caching is on by default; `DISABLE_PROMPT_CACHING` turns it off
/// globally, and the per-family `DISABLE_PROMPT_CACHING_{HAIKU,SONNET,OPUS}`
/// vars turn it off for a matching model. Truthiness follows TS `isEnvTruthy`
/// (utils/envUtils.ts): only `1`/`true`/`yes`/`on` (case-insensitive) count.
///
/// PARITY-NOTE: TS compares `model` for exact equality with the *configured*
/// small-fast / default-sonnet / default-opus IDs; here we match the family by
/// substring, a close (slightly more lenient) approximation.
/// Shared truthy-env reader (`1`/`true`/`yes`/`on`, case/space-insensitive) —
/// used by the prompt-cache gates.
fn cache_env_truthy(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn prompt_caching_enabled(model: &str) -> bool {
    fn env_truthy(name: &str) -> bool {
        cache_env_truthy(name)
    }
    if env_truthy("DISABLE_PROMPT_CACHING") {
        return false;
    }
    let m = model.to_ascii_lowercase();
    if m.contains("haiku") && env_truthy("DISABLE_PROMPT_CACHING_HAIKU") {
        return false;
    }
    if m.contains("sonnet") && env_truthy("DISABLE_PROMPT_CACHING_SONNET") {
        return false;
    }
    if m.contains("opus") && env_truthy("DISABLE_PROMPT_CACHING_OPUS") {
        return false;
    }
    true
}

/// Collapse a [`ReasoningConfig`] to a numeric budget for telemetry labels.
/// `Adaptive` → 0, `Enabled{b}` → b.
fn reasoning_budget(reasoning: Option<crate::ReasoningConfig>) -> u32 {
    match reasoning {
        Some(crate::ReasoningConfig::Enabled { budget_tokens }) => budget_tokens,
        Some(crate::ReasoningConfig::Adaptive) | None => 0,
    }
}

/// `true` when the named env var is truthy under the strict claude-code
/// allowlist (`1`/`true`/`yes`/`on`). Used for the `CLAUDE_CODE_DISABLE_THINKING`
/// / `CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING` gates.
fn is_thinking_env_disabled(name: &str) -> bool {
    traits::env::is_env_truthy(std::env::var(name).ok().as_deref())
}

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
    decoder: Box<dyn crate::StreamDecoder>,
    frames: Box<dyn crate::FrameStream>,
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

/// Production service: drives `DefaultLlmClient` with full retry/rate-limit/betas.
pub struct ApiService {
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
    /// Forced `tool_choice` for every request this adapter drives, set by
    /// [`Self::with_forced_tool_choice`]. Used by `--json-schema` structured
    /// output to COMPEL the `StructuredOutput` tool (1:1 with claude-code forcing
    /// `tool_choice` to that tool). `None` (the default for every normal turn)
    /// leaves the request's `tool_choice` unset so the model chooses freely.
    forced_tool_choice: Option<crate::ToolChoice>,
    /// Session thinking configuration (claude-code `thinking` intent).
    ///
    /// Default [`ThinkingConfig::Adaptive`] — claude-code sends adaptive thinking
    /// by default for adaptive-capable models. `build_request` resolves this
    /// against the model's thinking predicates + the `CLAUDE_CODE_DISABLE_*`
    /// env gates to produce the `reasoning` field and the coupled `temperature`.
    /// Set via [`Self::with_thinking`].
    thinking: crate::model::thinking::ThinkingConfig,
    /// Identity for the Anthropic `metadata.user_id` field (claude-code
    /// `claude.ts:503-525`). `None` (the default) omits `metadata` entirely.
    /// Set via [`Self::with_request_metadata`]; the composition root supplies
    /// the composed identity string.
    request_metadata: Option<crate::RequestMetadata>,
    /// 1P experimental cache-editing inputs (claude.ts `addCacheBreakpoints`
    /// `newCacheEdits`/`pinnedEdits`, claude.ts:3068-3069). LingXi has no
    /// cached-microcompact scheduler to produce these, so the default is
    /// `None`/empty — the gate-armed `cache_reference`-on-tool_results pass
    /// (the directly-exercised behavior) still runs from `req.messages`. Set
    /// only by [`Self::with_cache_editing_inputs`] (test-only today); wiring a
    /// real producer is residual. See module note.
    cache_editing_inputs: CacheEditingInputs,
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
    /// The Anthropic `request-id` response header (`req_…`) of the most recently
    /// recorded response, captured in [`Self::record_rate_limit_from_headers`]
    /// (the stream connect-success + non-stream header pass). Read via the
    /// `last_request_id()` trait method to stamp the persisted assistant line's
    /// top-level `requestId`. `None` until the first recorded response.
    last_request_id: Mutex<Option<String>>,
    /// Number of budget-consuming retry attempts the most recent drive
    /// performed before its terminal outcome (`RetryState::attempt`). Recorded
    /// on the non-stream success path and at stream connect-success; read via
    /// the `last_retry_count()` trait method (both `OrchestratorApiClient` and
    /// `StreamingApiClient`) so the orchestrator's cost-recording call sites can
    /// pass the real retry count to `CostTracker::record_api_response_v2`
    /// instead of the previous hardcoded `0` (#5 main-loop parity). For the
    /// stream this reflects connect-phase retries only (the value the adapter
    /// knows when it returns the `BoxStream`). `0` until the first drive.
    last_retry_count: Mutex<u32>,
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
    /// response. Recorded on success passes here AND, as of B6-T1, on a
    /// TERMINAL 429 — TS extracts raw utilization from error headers too
    /// (`extractRawUtilization`, `claudeAiLimits.ts:500`). The 429 path stages
    /// the raw snapshot in [`Self::pending_429`] and promotes it into this
    /// cache only when the turn dies on the 429 (via
    /// [`Self::promote_pending_429`]), never on a retried-then-recovered
    /// attempt. Exposed via the `OrchestratorApiClient::last_raw_utilization`
    /// override.
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
    /// 429-attempt state staged until the retry loop declares the error
    /// TERMINAL.
    ///
    /// B6-T1: claude-code updates the limits/raw module state ONLY in the
    /// terminal catch handler `extractQuotaStatusFromError`
    /// (claudeAiLimits.ts:487), never on a retried attempt that later
    /// recovers. [`Self::record_rate_limit_from_429`] writes this slot on
    /// every decoded 429; [`Self::promote_pending_429`] promotes it into
    /// `last_rate_limit` / `last_raw_utilization` only at the decode-terminal
    /// returns of both drive fns. Discarded at drive-entry and on any
    /// subsequent success — so a retried-then-recovered 429 never plants a
    /// rejected snapshot (the prior per-attempt-write divergence, CLOSED).
    pending_429: Mutex<Option<Pending429>>,
    /// Conversation-session scoped OpenAI Responses WebSocket connection/cache.
    ///
    /// The adapter is used by one conversation runtime; mobile already enforces
    /// one in-flight turn. The underlying `llm-client` session still only sends
    /// `previous_response_id` when the new request is a strict compatible
    /// extension of the previous completed request.
    responses_ws_session: tokio::sync::Mutex<ResponsesWebSocketSession>,
}

/// 429-attempt state held until the retry loop declares the error TERMINAL —
/// TS updates module state only in the terminal catch handler
/// (`extractQuotaStatusFromError`, claudeAiLimits.ts:487), never on retried
/// attempts. Promoted by [`ApiService::promote_pending_429`]; discarded
/// on drive-entry and on any subsequent success.
struct Pending429 {
    /// Forced-rejected limits snapshot (`from_429_error_headers`); `None`
    /// when the unified-header limits gate did not pass but raw windows did.
    info: Option<RateLimitInfo>,
    /// Raw per-window utilization from the SAME error headers, computed
    /// UNCONDITIONALLY (`extractRawUtilization`, ts:500 — independent of the
    /// limits gate).
    raw: RawUtilization,
}

/// A previously-pinned cache_edits block plus the user-message index it must be
/// re-inserted at. Mirrors claude-code's `CachedMCPinnedEdits`
/// (`services/api/claude.ts:3057-3060`).
#[derive(Debug, Clone, Default)]
struct PinnedCacheEdits {
    /// Index into the request `messages` (the user message to splice into).
    user_message_index: usize,
    /// The cache_edits delete operations to re-insert at that position.
    edits: Vec<crate::CacheEdit>,
}

/// Cache-editing builder inputs — the `newCacheEdits` + `pinnedEdits` args of
/// claude-code's `addCacheBreakpoints`. Default is empty (no producer wired):
/// only the gate-armed `cache_reference`-on-tool_results pass runs by default.
#[derive(Debug, Clone, Default)]
struct CacheEditingInputs {
    /// New cache_edits delete ops to insert into the last user message and pin.
    new_edits: Vec<crate::CacheEdit>,
    /// Previously-pinned cache_edits to re-insert at their original positions.
    pinned: Vec<PinnedCacheEdits>,
}

impl ApiService {
    /// Construct the service.  Called by Task 10 host constructors.
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
        Self::new_with_estimator(
            client,
            transport,
            subscriber,
            ua,
            version,
            analytics,
            fallback_model,
            None,
        )
    }

    /// Construct the service with an explicit cost estimator.
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

    /// Construct the service with routing overrides from `routing.fallback` /
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
            forced_tool_choice: None,
            thinking: crate::model::thinking::ThinkingConfig::default(),
            request_metadata: None,
            cache_editing_inputs: CacheEditingInputs::default(),
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
            last_request_id: Mutex::new(None),
            last_retry_count: Mutex::new(0),
            last_raw_utilization: Mutex::new(None),
            last_429_message: Mutex::new(None),
            pending_429: Mutex::new(None),
            responses_ws_session: tokio::sync::Mutex::new(ResponsesWebSocketSession::new()),
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

    /// Force a specific `tool_choice` on every request this adapter drives — used
    /// by `--json-schema` structured output to compel the `StructuredOutput`
    /// tool. Builder-style; `None` (the default) leaves tool choice to the model.
    #[must_use]
    pub fn with_forced_tool_choice(mut self, choice: crate::ToolChoice) -> Self {
        self.forced_tool_choice = Some(choice);
        self
    }

    /// Set the session thinking configuration. Builder-style; the default is
    /// [`ThinkingConfig::Adaptive`](crate::model::thinking::ThinkingConfig::Adaptive).
    #[must_use]
    pub fn with_thinking(mut self, thinking: crate::model::thinking::ThinkingConfig) -> Self {
        self.thinking = thinking;
        self
    }

    /// Set the identity for the Anthropic `metadata.user_id` field. Builder-style;
    /// the default is `None` (no `metadata` object emitted).
    #[must_use]
    pub fn with_request_metadata(mut self, metadata: crate::RequestMetadata) -> Self {
        self.request_metadata = Some(metadata);
        self
    }

    /// `getAPIMetadata()` (`services/api/claude.ts:503-528`): build the Anthropic
    /// request `metadata.user_id` value, which claude-code packs as a JSON STRING
    /// `JSON.stringify({...extra, device_id, account_uuid, session_id})`.
    ///
    /// * `extra` = the `CLAUDE_CODE_EXTRA_METADATA` env var when it parses to a
    ///   JSON object (any other value is ignored, mirroring the TS
    ///   debug-log-and-skip — we have no debug log).
    /// * Key order is `extra…, device_id, account_uuid, session_id` (workspace
    ///   `serde_json` `preserve_order`), and a colliding `extra` key keeps its
    ///   first position but takes the canonical value — byte-identical to the JS
    ///   object spread.
    ///
    /// The composition root supplies `device_id` ([`migrations::global_config::
    /// get_or_create_user_id`]), `account_uuid` (the OAuth account UUID, or `""`
    /// — the TS `getOauthAccountInfo()?.accountUuid ?? ''`), and `session_id`
    /// (the main session id, claude-code's `getSessionId()`).
    #[must_use]
    pub fn build_api_metadata_user_id(
        device_id: &str,
        account_uuid: &str,
        session_id: &str,
    ) -> String {
        let mut obj = serde_json::Map::new();
        if let Ok(extra_str) = std::env::var("CLAUDE_CODE_EXTRA_METADATA") {
            if let Ok(serde_json::Value::Object(extra)) =
                serde_json::from_str::<serde_json::Value>(&extra_str)
            {
                for (k, v) in extra {
                    obj.insert(k, v);
                }
            }
        }
        obj.insert(
            "device_id".to_string(),
            serde_json::Value::String(device_id.to_string()),
        );
        obj.insert(
            "account_uuid".to_string(),
            serde_json::Value::String(account_uuid.to_string()),
        );
        obj.insert(
            "session_id".to_string(),
            serde_json::Value::String(session_id.to_string()),
        );
        serde_json::to_string(&serde_json::Value::Object(obj)).unwrap_or_default()
    }

    /// Inject 1P cache-editing inputs (`newCacheEdits` / `pinnedEdits`). Test-only
    /// today — no production producer (cached-microcompact scheduler) is wired, so
    /// the default is empty. Builder-style.
    #[cfg(test)]
    #[must_use]
    fn with_cache_editing_inputs(mut self, inputs: CacheEditingInputs) -> Self {
        self.cache_editing_inputs = inputs;
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
        let Some(slot) = &self.subscription else {
            return self.subscriber;
        };
        let Ok(guard) = slot.read() else {
            return self.subscriber;
        };
        let Some(snap) = guard.as_ref() else {
            return self.subscriber;
        };
        SubscriberState {
            is_subscriber: snap.is_subscriber,
            is_enterprise: snap.subscription_type.as_deref() == Some("enterprise"),
        }
    }

    /// 1P global-cache-scope gate — parity `shouldUseGlobalCacheScope`
    /// (`utils/betas.ts:227-232`): `getAPIProvider() === 'firstParty' &&
    /// !CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`.
    ///
    /// LingXi resolves the concrete provider downstream of this provider-agnostic
    /// request builder and has no GrowthBook rollout bucketing, so the feature is
    /// kept **dormant**: it requires an explicit opt-in env
    /// (`CLAUDE_CODE_GLOBAL_CACHE_SCOPE`) — mirroring the experimental-beta gating
    /// pattern used elsewhere — AND the subscriber (firstParty) signal, AND the
    /// shared experimental-betas kill switch must not be set. Default: off.
    fn should_use_global_cache_scope(&self) -> bool {
        if cache_env_truthy("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS") {
            return false;
        }
        // `firstParty` approximation at this layer: a Claude.ai subscriber (the
        // OAuth/first-party path). Opt-in env arms the otherwise-dormant feature.
        cache_env_truthy("CLAUDE_CODE_GLOBAL_CACHE_SCOPE")
            && self.effective_subscriber().is_subscriber
    }

    /// 1h-TTL gate — parity `should1hCacheTTL` (`services/api/claude.ts:393-434`).
    ///
    /// The TS path is GrowthBook-allowlist + querySource gated (machinery LingXi
    /// lacks) plus a Bedrock env opt-in (`ENABLE_PROMPT_CACHING_1H_BEDROCK`).
    /// Kept **dormant**: honored only via the Bedrock-style explicit opt-in
    /// env `ENABLE_PROMPT_CACHING_1H` (default off), since LingXi has no
    /// querySource allowlist to consult. Folded into the emitted cache_control.
    fn should_1h_cache_ttl(&self) -> bool {
        cache_env_truthy("ENABLE_PROMPT_CACHING_1H")
    }

    /// 1P experimental cache-EDITING gate — parity `useCachedMC`
    /// (`services/api/claude.ts:3067`, passed down from the caller at
    /// claude.ts:1531-1709, where it additionally requires
    /// `getAPIProvider()==='firstParty' && querySource==='repl_main_thread'`).
    ///
    /// LingXi resolves the concrete provider downstream of this provider-agnostic
    /// request builder and has no querySource allowlist, so — exactly like
    /// [`Self::should_use_global_cache_scope`] — the feature is kept **dormant**:
    /// it requires an explicit opt-in env (`CLAUDE_CODE_CACHE_EDITING`), the
    /// shared experimental-betas kill switch must not be set, AND the subscriber
    /// (firstParty) signal must be present. Default: off → no `cache_edits` /
    /// `cache_reference` ever emitted, so 3P traffic is byte-unchanged.
    ///
    /// PARITY-NOTE: the TS `useCachedMC` body also pushes the
    /// `CACHE_EDITING_BETA_HEADER` once-per-session via the `cacheEditingHeaderLatched`
    /// latch (claude.ts:1673) — session-latch machinery LingXi lacks; the
    /// request-builder emission is ported, the beta-header latch is residual.
    fn should_use_cache_editing(&self) -> bool {
        if cache_env_truthy("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS") {
            return false;
        }
        cache_env_truthy("CLAUDE_CODE_CACHE_EDITING") && self.effective_subscriber().is_subscriber
    }

    // ── Shared request build ─────────────────────────────────────────────────

    /// Convert orchestrator-layer inputs into an `LlmRequest`.
    // An internal request-assembler: model + profile + system + msgs + tools +
    // stream + max_tokens are all genuinely distinct inputs (8/7).
    #[allow(clippy::too_many_arguments)]
    fn build_request(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        stream: bool,
        max_tokens: Option<u32>,
    ) -> Result<LlmRequest, LlmError> {
        // Pre-wire pipeline (claude-code order): strip_excess_media →
        // normalizeMessagesForAPI (consecutive-role merge) → ensureToolResultPairing
        // (SEND-time repair of orphaned/missing/duplicate tool_use↔tool_result on
        // resumed/interrupted transcripts; strict no-op on a clean turn).
        let messages = to_llm_messages(ensure_tool_result_pairing(normalize_messages_for_api(
            strip_excess_media(msgs, MAX_MEDIA_PER_REQUEST),
        )))?;
        let tool_decls = to_tool_declarations(tools)?;

        let mut req = LlmRequest::new(model);
        if let Some(p) = profile {
            req = req.with_profile(p);
        }

        // Prompt-cache breakpoints (parity: claude-code getPromptCachingEnabled +
        // buildSystemPromptBlocks + addCacheBreakpoints). Anthropic permits at most
        // 4 ephemeral breakpoints per request; we place at most 3 — up to two on
        // the system blocks (prefix + rest, per splitSysPromptPrefix /
        // buildSystemPromptBlocks) and one on the last content block of the last
        // message — matching the TS baseline (the tools array gets none on the
        // non-global-cache path). The Anthropic codec serializes
        // Some(CacheControl::Ephemeral) as {"type":"ephemeral"}; non-Anthropic
        // codecs ignore the field, so this is a no-op for them.
        let enable_caching = prompt_caching_enabled(model);

        if let Some(s) = system {
            // Split the assembled system string into up-to-3 cache blocks
            // (here ≤2 — the attribution bucket is always empty for LingXi),
            // marking only the org-scoped buckets, per
            // `prompt::split_system_blocks`. Replaces the previous single
            // collapsed block.
            //
            // 1P global-cache path (dormant): when the experimental gate is on
            // we take splitSysPromptPrefix's global mode. LingXi does not yet
            // assemble SYSTEM_PROMPT_DYNAMIC_BOUNDARY into the prompt, so this
            // degenerates to the org default (boundaryIndex===-1 fallthrough) —
            // the scope:'global'/ttl:'1h' serialization is wired and ready but
            // inert until a boundary marker is assembled. See module residual.
            let split_opts = crate::prompt_format::SplitOptions {
                global_scope: self.should_use_global_cache_scope(),
                ttl_1h: self.should_1h_cache_ttl(),
            };
            req.system = crate::prompt_format::split_system_blocks_with(s, enable_caching, split_opts);
        }
        req.messages = messages;

        // Exactly one message-level breakpoint, on the last cache-eligible content
        // block of the last message (claude.ts addCacheBreakpoints markerIndex =
        // len-1). Skip reasoning/redacted blocks (assistantMessageToMessageParam).
        if enable_caching {
            use crate::ContentBlock as LlmContentBlock;
            if let Some(last) = req.messages.last_mut() {
                if let Some(block) = last.content.iter_mut().rev().find(|b| {
                    !matches!(
                        b,
                        LlmContentBlock::Reasoning { .. }
                            | LlmContentBlock::RedactedThinking { .. }
                    )
                }) {
                    match block {
                        LlmContentBlock::Text { cache_control, .. }
                        | LlmContentBlock::ToolResult { cache_control, .. } => {
                            *cache_control = Some(CacheControl::Ephemeral);
                        }
                        // Image / ToolCall / etc.: no cache_control slot — skip.
                        _ => {}
                    }
                }
            }
        }

        // 1P experimental cache-editing pass (claude.ts addCacheBreakpoints,
        // 3108-3208). Gated behind `useCachedMC` (`should_use_cache_editing`):
        // when OFF (the default), this is a no-op and the request is
        // byte-identical to the pre-feature path. When ARMED it (a) re-inserts
        // previously-pinned cache_edits at their original positions, (b) inserts
        // the new cache_edits into the last user message, and (c) stamps
        // `cache_reference` onto every tool_result strictly before the last
        // cache_control marker — all with cross-block delete-ref dedup.
        if self.should_use_cache_editing() {
            apply_cache_editing(
                &mut req.messages,
                enable_caching,
                &self.cache_editing_inputs.new_edits,
                &self.cache_editing_inputs.pinned,
            );
        }

        req.tools = tool_decls; // No tool-array breakpoint (matches TS baseline).
                                // Forced tool choice (e.g. `--json-schema` → `StructuredOutput`). Unset
                                // for every normal turn, so the request carries no `tool_choice` and the
                                // model chooses freely — byte-identical to the pre-feature request.
        if let Some(choice) = &self.forced_tool_choice {
            req.tool_choice = Some(choice.clone());
        }
        req.stream = stream;

        // max_tokens (DIV-3): honor the escalation override (Some) else the
        // model value (claude.ts getMaxOutputTokensForModel). The base path
        // passes None → the model's binary-grounded max-output tokens, NOT the
        // codec's 4096 default.
        req.max_tokens = Some(max_tokens.unwrap_or_else(|| {
            u32::try_from(crate::model::context_window::max_output_tokens_for_model(model))
                .unwrap_or(u32::MAX)
        }));

        // thinking (DIV-1) + temperature (DIV-4), mirroring claude.ts:1596-1630
        // and claude.ts:1693. Computed AFTER max_tokens is known (the fixed-
        // budget cap clamps to max_tokens-1).
        {
            use crate::model::thinking::{
                model_sends_temperature, model_supports_adaptive_thinking,
                model_supports_thinking, ThinkingConfig,
            };
            use crate::ReasoningConfig;

            let has_thinking = self.thinking != ThinkingConfig::Disabled
                && !is_thinking_env_disabled("CLAUDE_CODE_DISABLE_THINKING");

            req.reasoning = if has_thinking && model_supports_thinking(model) {
                if !is_thinking_env_disabled("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING")
                    && model_supports_adaptive_thinking(model)
                {
                    Some(ReasoningConfig::Adaptive)
                } else {
                    let mut budget =
                        crate::model::context_window::max_thinking_tokens_for_model(model);
                    if let ThinkingConfig::Enabled { budget_tokens } = self.thinking {
                        budget = budget_tokens;
                    }
                    // budget_tokens must stay strictly below max_tokens.
                    budget = budget.min(req.max_tokens.unwrap_or(u32::MAX).saturating_sub(1));
                    Some(ReasoningConfig::Enabled {
                        budget_tokens: budget,
                    })
                }
            } else {
                None
            };

            // temperature:1 ONLY when thinking is disabled AND the model is in the
            // `rhn` temperature-gate set (binary @205866168:
            // `!xs && rhn(u) ? temperatureOverride ?? 1 : void 0`). The default
            // opus-4-8 (and 4-7/fable-5/mythos-5/unknowns) are NOT in `rhn` → the
            // field is omitted. The Anthropic codec emits temperature on Some only.
            req.temperature = if !has_thinking && model_sends_temperature(model) {
                Some(1.0)
            } else {
                None
            };
        }

        // metadata.user_id (DIV-2): claude-code always sends it. `None` (no
        // identity wired) omits the object — byte-identical to the prior request.
        req.metadata = self.request_metadata.clone();

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
    /// Build the per-request [`BetaContext`] (the binary's `xLr(model)` inputs)
    /// from the prepared request body: the resolved model id and `speed: "fast"`.
    /// `interactive`/`show_thinking_summaries` use the faithful external-default
    /// (interactive TUI, no summaries) — wiring the live session flags is a
    /// documented follow-up; the dominant interactive path matches the binary.
    fn beta_context(prepared: &crate::PreparedLlmCall) -> BetaContext {
        let model = prepared
            .provider_request
            .body_json
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let fast_mode = prepared
            .provider_request
            .body_json
            .get("speed")
            .and_then(serde_json::Value::as_str)
            == Some("fast");
        // The `effort-2025-11-24` beta gates on the body carrying
        // `output_config.effort`.
        let has_effort = prepared
            .provider_request
            .body_json
            .get("output_config")
            .and_then(|oc| oc.get("effort"))
            .is_some();
        BetaContext::for_model(model)
            .with_fast_mode(fast_mode)
            .with_effort(has_effort)
    }

    fn inject_headers(&self, prepared: &mut crate::PreparedLlmCall, request_id: &str) {
        // Anthropic beta headers are protocol-specific. OpenAI/Gemini/Vertex/
        // Bedrock/Azure routes must not receive Anthropic beta headers.
        if matches!(
            prepared.route.protocol,
            crate::ProtocolFamily::AnthropicMessages
        ) {
            let ctx = Self::beta_context(prepared);
            apply_beta_header_with_auth(
                &mut prepared.provider_request,
                Provider::Anthropic,
                Endpoint::MessagesCreate,
                &ctx,
                self.effective_subscriber().is_subscriber,
            );
        }
        // User-Agent (Task 3).
        prepared.provider_request.headers.insert(
            "user-agent".to_string(),
            user_agent(&self.ua, &self.version),
        );
        // Client-traceable request id (matches api-client header name).
        prepared
            .provider_request
            .headers
            .insert("x-request-id".to_string(), request_id.to_string());
    }

    /// Same as [`inject_headers`] but for the streaming endpoint.
    fn inject_stream_headers(&self, prepared: &mut crate::PreparedLlmCall, request_id: &str) {
        if matches!(
            prepared.route.protocol,
            crate::ProtocolFamily::AnthropicMessages
        ) {
            let ctx = Self::beta_context(prepared);
            apply_beta_header_with_auth(
                &mut prepared.provider_request,
                Provider::Anthropic,
                Endpoint::MessagesCreateStream,
                &ctx,
                self.effective_subscriber().is_subscriber,
            );
        }
        prepared.provider_request.headers.insert(
            "user-agent".to_string(),
            user_agent(&self.ua, &self.version),
        );
        prepared
            .provider_request
            .headers
            .insert("x-request-id".to_string(), request_id.to_string());
    }

    // ── 429 retry-after resolution (reset ladder) ─────────────────────────────

    /// Resolve the 429 retry delay using the server-sent reset ladder:
    /// `retry-after` → `anthropic-ratelimit-unified-reset` → `anthropic-ratelimit-requests-reset` → 1 s.
    fn resolve_retry_after(
        headers: &std::collections::BTreeMap<String, String>,
    ) -> std::time::Duration {
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
    /// `Arc<ApiService>` and call this method directly.  A future task can
    /// wire it through the handle if needed.
    pub fn last_rate_limit_info(&self) -> Option<RateLimitInfo> {
        self.last_rate_limit.lock().unwrap().clone()
    }

    /// Parse rate-limit headers from a 2xx response and update the cached snapshot.
    ///
    /// Emits a `tracing::warn!` when the overage status indicates the account is
    /// at or near exhaustion (`overage_status == "rejected"` or `"allowed_warning"`).
    fn record_rate_limit_from_headers(&self, headers: &std::collections::BTreeMap<String, String>) {
        let hvec: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Capture the Anthropic `request-id` response header (`req_…`) on every
        // recorded response — the SDK's `response._request_id`, which claude-code
        // persists as the assistant line's top-level `requestId`. Prefer the
        // canonical `request-id`, falling back to `x-request-id` (mirrors the
        // transport's `request_id()` helper). `None` clears it when neither is
        // present (so a stale id never leaks onto a later line).
        *self.last_request_id.lock().unwrap() = headers
            .get("request-id")
            .or_else(|| headers.get("x-request-id"))
            .cloned();
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
        // B6-T1: a success also discards any staged-but-unpromoted 429 from an
        // earlier retried attempt (TS resets module state to the success's
        // `status`, never leaving a stale `rejected` behind).
        self.clear_pending_429();
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
    /// 1. the forced-`rejected` limits view is STAGED in [`Self::pending_429`]
    ///    (TS updates its limits state from the error headers with
    ///    `status: 'rejected'`, `errors.ts:482-516`), and
    /// 2. the composed `getRateLimitErrorMessage` copy is cached for the
    ///    orchestrator's terminal-error re-map
    ///    (`OrchestratorError::RateLimitRejected`).
    ///
    /// The RAW per-window utilization is parsed from the SAME error headers
    /// UNCONDITIONALLY — `extractRawUtilization(headersToUse)` runs for ANY
    /// error headers (claudeAiLimits.ts:500), independent of the limits gate —
    /// and staged alongside.
    ///
    /// The staged slot is PROMOTED into the live `last_rate_limit` /
    /// `last_raw_utilization` caches only when the retry loop declares the
    /// error TERMINAL (via [`Self::promote_pending_429`]), mirroring the TS
    /// terminal catch handler `extractQuotaStatusFromError`
    /// (claudeAiLimits.ts:487) — NOT on a retried attempt that later recovers.
    /// This closes the prior per-attempt-write divergence (a
    /// retried-then-recovered 429 no longer plants a rejected snapshot).
    ///
    /// When the 429 yields NEITHER a gated `info` NOR any raw window, the
    /// pending slot is cleared (`None`); the copy slot is likewise cleared
    /// (the generic 429 surface applies) — TS only updates inside the gated
    /// branch.
    fn record_rate_limit_from_429(&self, headers: &std::collections::BTreeMap<String, String>) {
        let hvec: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // `extractRawUtilization(headersToUse)` (claudeAiLimits.ts:500) runs
        // for ANY error headers, independent of the limits gate below.
        let raw = RawUtilization::from_headers(&hvec);
        let info = RateLimitInfo::from_429_error_headers(&hvec);

        // MESSAGE composition ports errors.ts:482-516 (a LOCAL limits object
        // built from the error headers) — kept verbatim; it is NOT staged and
        // is set per-attempt because the terminal error re-map reads it
        // directly (the most-recent-429 slot, cleared on success).
        let composed = info.as_ref().map(|info| {
            // `formatResetTime(…, true)` analogue for both reset headers
            // (`rateLimitMessages.ts:144-148`), formatted at error time.
            let formatted = formatted_reset_times_from_headers(&hvec);
            rate_limit_error_message(
                info,
                &formatted.as_reset_times(),
                SubscriptionContext {
                    is_pro_or_enterprise: self.is_pro_or_enterprise(),
                },
            )
        });
        *self.last_429_message.lock().unwrap() = composed.flatten();

        // Stage the snapshot whenever EITHER the limits gate passed OR raw
        // windows are present. A 429 that yields neither clears the slot.
        let staged = if info.is_some() || raw != RawUtilization::default() {
            Some(Pending429 { info, raw })
        } else {
            None
        };
        *self.pending_429.lock().unwrap() = staged;
    }

    /// Discard any staged 429 snapshot. Called at drive entry and on success so
    /// a retried-then-recovered 429 (or a non-RateLimited terminal that leaves a
    /// staged slot) cannot promote into a LATER drive. Defensive backstop: the
    /// active cross-drive isolation is the per-attempt record stage-or-clear in
    /// [`Self::record_rate_limit_from_429`] (a fresh attempt always overwrites
    /// or clears the slot before the terminal promote runs); this guards against
    /// a future refactor that adds a promote-without-record path.
    fn clear_pending_429(&self) {
        *self.pending_429.lock().expect("pending_429 poisoned") = None;
    }

    /// Promote a staged 429 snapshot into the live caches — the Rust analogue
    /// of the TS terminal catch handler `extractQuotaStatusFromError`
    /// (claudeAiLimits.ts:487-515), which updates module state only when the
    /// turn DIES on a 429.
    ///
    /// `.take()`s [`Self::pending_429`]; when `Some`:
    /// - `info` `Some` → the forced-`rejected` limits snapshot replaces
    ///   `last_rate_limit`;
    /// - `raw != RawUtilization::default()` → the raw per-window snapshot
    ///   replaces `last_raw_utilization`.
    ///
    /// Convention: the EMPTY raw snapshot is NEVER stored (the `last_raw…`
    /// cache and the `emit_raw_utilization_if_changed` seam treat the empty
    /// `{}` as "no windows" and skip it) — a documented divergence from TS,
    /// which assigns `rawUtilization` unconditionally. Idempotent via
    /// `.take()`: a second call after promotion is a no-op.
    fn promote_pending_429(&self) {
        let Some(pending) = self.pending_429.lock().unwrap().take() else {
            return;
        };
        if let Some(info) = pending.info {
            *self.last_rate_limit.lock().unwrap() = Some(info);
        }
        if pending.raw != RawUtilization::default() {
            *self.last_raw_utilization.lock().unwrap() = Some(pending.raw);
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
            LlmError::Overloaded { .. } => Some(529),
            LlmError::Transport { .. }
            | LlmError::StreamInterrupted { .. }
            | LlmError::CostUnavailable { .. }
            | LlmError::UnsupportedCapability { .. } => None,
        }
    }
}

// ── Media capping (stripExcessMediaItems) ─────────────────────────────────────

/// Maximum media items (images + documents) the API accepts per request.
/// Above this we trim oldest-first. Mirrors TS `API_MAX_MEDIA_PER_REQUEST`
/// (apiLimits.ts:94).
const MAX_MEDIA_PER_REQUEST: usize = 100;

/// True when a nested `tool_result.content` block (a raw JSON value, e.g. an MCP
/// image/resource result) is a media item — `type === "image" || "document"`,
/// matching claude-code `isMedia` (`claude.ts:943`).
fn is_media_value(v: &serde_json::Value) -> bool {
    matches!(
        v.get("type").and_then(serde_json::Value::as_str),
        Some("image") | Some("document")
    )
}

/// Count media (image/document) content blocks across all messages, INCLUDING
/// media NESTED inside `tool_result.content` (the `content_blocks` array MCP
/// image/resource results populate). 1:1 with claude-code `stripExcessMediaItems`
/// counting (`claude.ts:961-971`) — top-level media that ignored the nested
/// channel let an MCP-image-heavy transcript silently exceed the API media cap.
fn count_media(msgs: &[ConversationMessage]) -> usize {
    msgs.iter()
        .map(|m| match m {
            ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } => content
                .iter()
                .map(|b| match b {
                    ContentBlock::Image { .. } | ContentBlock::Document { .. } => 1,
                    ContentBlock::ToolResult {
                        content_blocks: Some(blocks),
                        ..
                    } => blocks.iter().filter(|v| is_media_value(v)).count(),
                    _ => 0,
                })
                .sum::<usize>(),
            ConversationMessage::System { .. } => 0,
        })
        .sum()
}

/// Return `msgs` with the OLDEST media items stripped until at most `limit`
/// remain. 1:1 with claude-code `stripExcessMediaItems` (`claude.ts:975-1014`):
/// for each message (oldest-first), strip media NESTED in `tool_result.content`
/// FIRST (the `.map`, `:982-999`), then TOP-LEVEL media (the `.filter`,
/// `:1000-1006`).
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
        // (1) Nested-in-tool_result media first (claude-code `.map`).
        for block in content.iter_mut() {
            if to_remove == 0 {
                break;
            }
            if let ContentBlock::ToolResult {
                content_blocks: Some(blocks),
                ..
            } = block
            {
                blocks.retain(|v| {
                    if to_remove > 0 && is_media_value(v) {
                        to_remove -= 1;
                        false
                    } else {
                        true
                    }
                });
            }
        }
        // (2) Top-level media (claude-code `.filter`).
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

/// Insert `block` into a content array relative to its `tool_result` blocks.
/// 1:1 port of claude-code `insertBlockAfterToolResults`
/// (`utils/contentArray.ts:21-51`):
///   - if any `tool_result` exists, insert after the LAST one; if that lands the
///     inserted block last, append a `{type:'text', text:'.'}` continuation
///     (some APIs reject a prompt ending in non-text content);
///   - otherwise insert before the last block (`max(0, len-1)`).
/// Mutates `content` in place.
fn insert_block_after_tool_results(
    content: &mut Vec<crate::ContentBlock>,
    block: crate::ContentBlock,
) {
    use crate::ContentBlock as Cb;
    let mut last_tool_result_index: isize = -1;
    for (i, item) in content.iter().enumerate() {
        if matches!(item, Cb::ToolResult { .. }) {
            last_tool_result_index = i as isize;
        }
    }
    if last_tool_result_index >= 0 {
        let insert_pos = (last_tool_result_index as usize) + 1;
        content.insert(insert_pos, block);
        // Append a text continuation if the inserted block is now last.
        if insert_pos == content.len() - 1 {
            content.push(Cb::Text {
                text: ".".to_string(),
                cache_control: None,
            });
        }
    } else {
        // No tool_result blocks — insert before the last block.
        let insert_index = content.len().saturating_sub(1);
        content.insert(insert_index, block);
    }
}

/// 1P experimental cache-editing pass — the `useCachedMC` tail of claude-code
/// `addCacheBreakpoints` (`services/api/claude.ts:3108-3208`). The caller gates
/// this behind `should_use_cache_editing`; here we assume it's armed.
///
/// `enable_caching` mirrors the TS `enablePromptCaching` flag (the
/// `cache_reference`-on-tool_results pass at 3164 is additionally gated on it).
/// `new_edits` = `newCacheEdits.edits`; `pinned` = `pinnedEdits`.
fn apply_cache_editing(
    messages: &mut [crate::Message],
    enable_caching: bool,
    new_edits: &[crate::CacheEdit],
    pinned: &[PinnedCacheEdits],
) {
    use crate::ContentBlock as Cb;

    // Track all cache_references being deleted to prevent duplicates across
    // blocks (claude.ts:3112-3125 seenDeleteRefs + deduplicateEdits).
    let mut seen_delete_refs: std::collections::HashSet<String> = std::collections::HashSet::new();
    let dedup = |edits: &[crate::CacheEdit],
                 seen: &mut std::collections::HashSet<String>|
     -> Vec<crate::CacheEdit> {
        edits
            .iter()
            .filter(|e| {
                let crate::CacheEdit::Delete { cache_reference } = e;
                if seen.contains(cache_reference) {
                    false
                } else {
                    seen.insert(cache_reference.clone());
                    true
                }
            })
            .cloned()
            .collect()
    };

    // Re-insert all previously-pinned cache_edits at their original positions
    // (claude.ts:3127-3139). Only when that message is a `user` message.
    for p in pinned {
        if let Some(msg) = messages.get_mut(p.user_message_index) {
            if msg.role == "user" {
                let deduped = dedup(&p.edits, &mut seen_delete_refs);
                if !deduped.is_empty() {
                    insert_block_after_tool_results(
                        &mut msg.content,
                        Cb::CacheEdits { edits: deduped },
                    );
                }
            }
        }
    }

    // Insert new cache_edits into the LAST user message and (in TS) pin them
    // (claude.ts:3141-3162). LingXi has no cross-call pin store, so the pinning
    // side-effect is a residual — the in-request insertion is faithful.
    if !messages.is_empty() {
        let deduped_new = dedup(new_edits, &mut seen_delete_refs);
        if !deduped_new.is_empty() {
            for i in (0..messages.len()).rev() {
                if messages[i].role == "user" {
                    insert_block_after_tool_results(
                        &mut messages[i].content,
                        Cb::CacheEdits { edits: deduped_new },
                    );
                    break;
                }
            }
        }
    }

    // Add cache_reference to tool_result blocks within the cached prefix
    // (claude.ts:3164-3207). Must run AFTER cache_edits insertion since that
    // modifies content arrays.
    if enable_caching {
        // Find the last message containing a cache_control marker.
        let mut last_cc_msg: isize = -1;
        for (i, msg) in messages.iter().enumerate() {
            for block in &msg.content {
                let has_cc = match block {
                    Cb::Text { cache_control, .. } | Cb::ToolResult { cache_control, .. } => {
                        cache_control.is_some()
                    }
                    _ => false,
                };
                if has_cc {
                    last_cc_msg = i as isize;
                }
            }
        }

        // Stamp `cache_reference = tool_use_id` on tool_results in `user`
        // messages STRICTLY before the last cache_control marker. (TS uses strict
        // "before" to avoid edge cases where cache_edits splicing shifts indices;
        // it also clones rather than mutating in place to avoid contaminating
        // blocks reused by non-cache-editing secondary queries — here each
        // request owns its `messages`, so an in-place set is equivalent.)
        if last_cc_msg >= 0 {
            for i in 0..(last_cc_msg as usize) {
                if messages[i].role != "user" {
                    continue;
                }
                for block in &mut messages[i].content {
                    if let Cb::ToolResult {
                        tool_call_id,
                        cache_reference,
                        ..
                    } = block
                    {
                        *cache_reference = Some(tool_call_id.clone());
                    }
                }
            }
        }
    }
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
