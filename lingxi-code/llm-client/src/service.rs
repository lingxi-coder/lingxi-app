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
}
