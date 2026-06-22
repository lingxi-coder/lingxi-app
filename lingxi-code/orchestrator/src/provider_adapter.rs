//! Drive `llm_client::DefaultLlmClient` from the orchestrator's seam traits.
//!
//! **Task 6**: replaces the old `providers::ModelRouter`-backed stubs with a real
//! `DefaultLlmClient` drive: `prepare()` → header injection → `transport.execute()`
//! / `open_stream()` → `codec.decode_response()`.  The retry driver loop wraps the
//! prepare/execute pair and feeds headers to `model/rate_limit.rs` + `model/retry.rs`.

use crate::conversation::{OrchestratorApiClient, StreamingApiClient};
use crate::model::betas::{apply_beta_header_with_auth, BetaContext, Endpoint, Provider};
use crate::model::rate_limit::{
    formatted_reset_times_from_headers, parse_retry_after, parse_unified_reset,
    rate_limit_error_message, RateLimitInfo, RawUtilization, SubscriptionContext,
};
use crate::model::retry::{next_step_with_backoff, resolve_retry_control_with_settings, DriveStep, ResolveRetryEnv, RetryControl, RetryState};
use crate::model::telemetry;
use crate::model::user_agent::{user_agent, UserAgentEnv};
use agent::convert::{
    ensure_tool_result_pairing, normalize_messages_for_api, to_llm_messages, to_tool_declarations,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use llm_client::{
    CacheControl, CostEstimator, DefaultLlmClient, LlmError, LlmEvent, LlmRequest, LlmResponse,
    ProviderRequest, Transport,
};
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
fn reasoning_budget(reasoning: Option<llm_client::ReasoningConfig>) -> u32 {
    match reasoning {
        Some(llm_client::ReasoningConfig::Enabled { budget_tokens }) => budget_tokens,
        Some(llm_client::ReasoningConfig::Adaptive) | None => 0,
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
    /// Forced `tool_choice` for every request this adapter drives, set by
    /// [`Self::with_forced_tool_choice`]. Used by `--json-schema` structured
    /// output to COMPEL the `StructuredOutput` tool (1:1 with claude-code forcing
    /// `tool_choice` to that tool). `None` (the default for every normal turn)
    /// leaves the request's `tool_choice` unset so the model chooses freely.
    forced_tool_choice: Option<llm_client::ToolChoice>,
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
    request_metadata: Option<llm_client::RequestMetadata>,
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
}

/// 429-attempt state held until the retry loop declares the error TERMINAL —
/// TS updates module state only in the terminal catch handler
/// (`extractQuotaStatusFromError`, claudeAiLimits.ts:487), never on retried
/// attempts. Promoted by [`ProviderApiAdapter::promote_pending_429`]; discarded
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
    edits: Vec<llm_client::CacheEdit>,
}

/// Cache-editing builder inputs — the `newCacheEdits` + `pinnedEdits` args of
/// claude-code's `addCacheBreakpoints`. Default is empty (no producer wired):
/// only the gate-armed `cache_reference`-on-tool_results pass runs by default.
#[derive(Debug, Clone, Default)]
struct CacheEditingInputs {
    /// New cache_edits delete ops to insert into the last user message and pin.
    new_edits: Vec<llm_client::CacheEdit>,
    /// Previously-pinned cache_edits to re-insert at their original positions.
    pinned: Vec<PinnedCacheEdits>,
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
            last_raw_utilization: Mutex::new(None),
            last_429_message: Mutex::new(None),
            pending_429: Mutex::new(None),
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
    pub fn with_forced_tool_choice(mut self, choice: llm_client::ToolChoice) -> Self {
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
    pub fn with_request_metadata(mut self, metadata: llm_client::RequestMetadata) -> Self {
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
        let Some(slot) = &self.subscription else { return self.subscriber; };
        let Ok(guard) = slot.read() else { return self.subscriber; };
        let Some(snap) = guard.as_ref() else { return self.subscriber; };
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
        cache_env_truthy("CLAUDE_CODE_CACHE_EDITING")
            && self.effective_subscriber().is_subscriber
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
            let split_opts = crate::prompt::SplitOptions {
                global_scope: self.should_use_global_cache_scope(),
                ttl_1h: self.should_1h_cache_ttl(),
            };
            req.system = crate::prompt::split_system_blocks_with(s, enable_caching, split_opts);
        }
        req.messages = messages;

        // Exactly one message-level breakpoint, on the last cache-eligible content
        // block of the last message (claude.ts addCacheBreakpoints markerIndex =
        // len-1). Skip reasoning/redacted blocks (assistantMessageToMessageParam).
        if enable_caching {
            use llm_client::ContentBlock as LlmContentBlock;
            if let Some(last) = req.messages.last_mut() {
                if let Some(block) = last.content.iter_mut().rev().find(|b| {
                    !matches!(
                        b,
                        LlmContentBlock::Reasoning { .. } | LlmContentBlock::RedactedThinking { .. }
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
            u32::try_from(compaction::max_output_tokens_for_model(model)).unwrap_or(u32::MAX)
        }));

        // thinking (DIV-1) + temperature (DIV-4), mirroring claude.ts:1596-1630
        // and claude.ts:1693. Computed AFTER max_tokens is known (the fixed-
        // budget cap clamps to max_tokens-1).
        {
            use crate::model::thinking::{
                model_supports_adaptive_thinking, model_supports_thinking, ThinkingConfig,
            };
            use llm_client::ReasoningConfig;

            let has_thinking = self.thinking != ThinkingConfig::Disabled
                && !is_thinking_env_disabled("CLAUDE_CODE_DISABLE_THINKING");

            req.reasoning = if has_thinking && model_supports_thinking(model) {
                if !is_thinking_env_disabled("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING")
                    && model_supports_adaptive_thinking(model)
                {
                    Some(ReasoningConfig::Adaptive)
                } else {
                    let mut budget = compaction::max_thinking_tokens_for_model(model);
                    if let ThinkingConfig::Enabled { budget_tokens } = self.thinking {
                        budget = budget_tokens;
                    }
                    // budget_tokens must stay strictly below max_tokens.
                    budget = budget.min(req.max_tokens.unwrap_or(u32::MAX).saturating_sub(1));
                    Some(ReasoningConfig::Enabled { budget_tokens: budget })
                }
            } else {
                None
            };

            // temperature:1 ONLY when thinking is disabled (claude.ts:1693). The
            // Anthropic codec emits temperature conditionally on Some.
            req.temperature = if has_thinking { None } else { Some(1.0) };
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
    fn beta_context(prepared: &ProviderRequest) -> BetaContext {
        let model = prepared
            .body_json
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let fast_mode = prepared
            .body_json
            .get("speed")
            .and_then(serde_json::Value::as_str)
            == Some("fast");
        BetaContext::for_model(model).with_fast_mode(fast_mode)
    }

    fn inject_headers(&self, prepared: &mut ProviderRequest, request_id: &str) {
        // anthropic-beta: per-model gated set (e5/xLr port) merged with any
        // auth-injected betas.
        let ctx = Self::beta_context(prepared);
        apply_beta_header_with_auth(
            prepared,
            Provider::Anthropic,
            Endpoint::MessagesCreate,
            &ctx,
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
        let ctx = Self::beta_context(prepared);
        apply_beta_header_with_auth(
            prepared,
            Provider::Anthropic,
            Endpoint::MessagesCreateStream,
            &ctx,
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

        // B6-T1: discard any 429 snapshot staged by a PRIOR drive (whose
        // terminal was non-rate-limited, so it never promoted) — TS module
        // state for the terminal catch handler is per-error, never carried
        // across calls.
        self.clear_pending_429();

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
        // thinking_budget for telemetry: Adaptive → 0, Enabled{b} → b.
        let thinking_budget: u32 = reasoning_budget(req.reasoning);
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
                                    // B6-T1: the turn DIES here — promote the
                                    // 429 snapshot staged this attempt into the
                                    // live caches (the TS terminal catch handler
                                    // `extractQuotaStatusFromError`,
                                    // claudeAiLimits.ts:487). Gated on the
                                    // RateLimited discriminant so a non-429
                                    // terminal never promotes a stale slot.
                                    if matches!(decode_err, LlmError::RateLimited { .. }) {
                                        self.promote_pending_429();
                                    }
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

        // B6-T1: discard any 429 snapshot staged by a PRIOR drive (see the
        // non-stream drive fn) — per-error state, never carried across calls.
        self.clear_pending_429();

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
        let thinking_budget: u32 = reasoning_budget(req.reasoning);

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
                        // B6-T1: the stream connect DIES here — promote the
                        // 429 snapshot staged this attempt (TS terminal catch
                        // handler, claudeAiLimits.ts:487). Gated on the
                        // RateLimited discriminant so a non-429 terminal never
                        // promotes a stale slot.
                        if matches!(decode_err, LlmError::RateLimited { .. }) {
                            self.promote_pending_429();
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
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        let req = self.build_request(model, profile, system, msgs, tools, false, None)?;
        let ctl = resolve_retry_control_with_settings(
            model,
            None,
            self.effective_subscriber().is_subscriber,
            &ResolveRetryEnv::from_process_env(),
            self.settings_max_retries,
        );
        self.drive_non_stream(req, ctl).await
    }

    async fn count_tokens(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<u64, LlmError> {
        // Build the same non-streaming request shape `messages_create` sends,
        // then delegate to the count_tokens facade: the real
        // `/v1/messages/count_tokens` endpoint (with the `count_tokens` beta) on
        // Anthropic routes, byte-length/4 approximation elsewhere.
        let req = self.build_request(model, profile, system, msgs, tools, false, None)?;
        crate::model::count_tokens::count_tokens(self.client.as_ref(), self.transport.as_ref(), &req)
            .await
    }

    async fn messages_create_with_opts(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        max_tokens: u32,
    ) -> Result<LlmResponse, LlmError> {
        let req = self.build_request(model, profile, system, msgs, tools, false, Some(max_tokens))?;
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
        profile: Option<&str>,
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

        // Primary request uses the passed profile; fallback requests use None
        // (the fallback config string has no associated profile).
        let req = self.build_request(model, profile, system, msgs, tools, false, None)?;
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
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        initial_consecutive_overloaded: u8,
    ) -> Result<LlmResponse, LlmError> {
        let req = self.build_request(model, profile, system, msgs, tools, false, None)?;
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

    fn list_model_listings(&self) -> Vec<traits::orchestrator::ModelListing> {
        catalog_model_listings()
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

    fn last_request_id(&self) -> Option<String> {
        self.last_request_id.lock().unwrap().clone()
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

/// Build the grouped-picker listing from the static llm-client catalog.
fn catalog_model_listings() -> Vec<traits::orchestrator::ModelListing> {
    let catalog = llm_client::builtin_presets();
    let Ok(registry) = llm_client::ModelRegistry::from_config(llm_client::ClientConfig {
        providers: catalog.providers,
    }) else {
        return Vec::new();
    };
    registry
        .available_models()
        .into_iter()
        .map(|m| traits::orchestrator::ModelListing {
            display_model: m.display_model,
            request_model: m.request_model,
            provider_label: provider_label(&m.profile_name).to_string(),
            provider_id: m.profile_name,
        })
        .collect()
}

/// Human provider header for a catalog profile name.
fn provider_label(profile_name: &str) -> &str {
    match profile_name {
        "openrouter" => "OpenRouter",
        "deepseek" => "DeepSeek",
        "glm-coding" => "GLM (coding)",
        "zai" => "Z.AI",
        "openai" => "OpenAI",
        "openai-chatgpt" => "OpenAI (ChatGPT login)",
        "github-copilot" => "GitHub Copilot",
        other => other,
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
        // Subagent calls don't carry a provider profile; pass None so
        // llm-client resolves unscoped (default behaviour).
        OrchestratorApiClient::messages_create(self, model, None, system, messages, tools).await
    }

    async fn messages_create_stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        // Subagent calls don't carry a provider profile; pass None so
        // llm-client resolves unscoped (default behaviour).
        StreamingApiClient::stream(self, model, None, system, messages, tools).await
    }

    async fn messages_create_stream_forced(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        forced_tool: Option<&str>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        // Structured-output path: build the request and force `tool_choice` to
        // the named tool so the model must emit a matching structured call.
        let mut req = self.build_request(model, None, system, messages, tools, true, None)?;
        if let Some(name) = forced_tool {
            req.tool_choice = Some(llm_client::ToolChoice::Tool {
                name: name.to_string(),
            });
        }
        self.drive_stream(req).await
    }
}

#[async_trait]
impl StreamingApiClient for ProviderApiAdapter {
    async fn stream(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let req = self.build_request(model, profile, system, messages, tools, true, None)?;
        self.drive_stream(req).await
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
    content: &mut Vec<llm_client::ContentBlock>,
    block: llm_client::ContentBlock,
) {
    use llm_client::ContentBlock as Cb;
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
    messages: &mut [llm_client::Message],
    enable_caching: bool,
    new_edits: &[llm_client::CacheEdit],
    pinned: &[PinnedCacheEdits],
) {
    use llm_client::ContentBlock as Cb;

    // Track all cache_references being deleted to prevent duplicates across
    // blocks (claude.ts:3112-3125 seenDeleteRefs + deduplicateEdits).
    let mut seen_delete_refs: std::collections::HashSet<String> = std::collections::HashSet::new();
    let dedup = |edits: &[llm_client::CacheEdit],
                 seen: &mut std::collections::HashSet<String>|
     -> Vec<llm_client::CacheEdit> {
        edits
            .iter()
            .filter(|e| {
                let llm_client::CacheEdit::Delete { cache_reference } = e;
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
                    if let Cb::ToolResult { tool_call_id, cache_reference, .. } = block {
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
                            reasoning: true,
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
                            reasoning: true,
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

    // ── Prompt-cache breakpoints (CACHE.1) ──────────────────────────────────

    // Serializes the two prompt-cache tests: one mutates DISABLE_PROMPT_CACHING
    // (process-global), so the default-on assertion in the other must not run
    // concurrently. Lock poison is benign here — recover the guard.
    static CACHE_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn text_user_msg(s: &str) -> ConversationMessage {
        ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::Text { text: s.to_string() }],
            is_meta: false,
        }
    }

    /// `last_request_id()` captures the Anthropic `request-id` response header
    /// (falling back to `x-request-id`) on every recorded headers pass, and
    /// clears when neither is present (no stale leak onto a later line). This is
    /// the slot the persisted assistant line's top-level `requestId` reads from.
    #[test]
    fn last_request_id_captures_request_id_header() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        assert_eq!(OrchestratorApiClient::last_request_id(&adapter), None);

        let mut h = std::collections::BTreeMap::new();
        h.insert("request-id".to_string(), "req_011abc".to_string());
        adapter.record_rate_limit_from_headers(&h);
        assert_eq!(
            OrchestratorApiClient::last_request_id(&adapter),
            Some("req_011abc".to_string())
        );

        // `x-request-id` fallback when the canonical header is absent.
        let mut h2 = std::collections::BTreeMap::new();
        h2.insert("x-request-id".to_string(), "req_xfallback".to_string());
        adapter.record_rate_limit_from_headers(&h2);
        assert_eq!(
            OrchestratorApiClient::last_request_id(&adapter),
            Some("req_xfallback".to_string())
        );

        // Neither header → cleared (a later response without a request-id does
        // not inherit the previous one).
        adapter.record_rate_limit_from_headers(&std::collections::BTreeMap::new());
        assert_eq!(OrchestratorApiClient::last_request_id(&adapter), None);
    }

    #[test]
    fn build_request_splits_system_into_org_blocks_by_default() {
        use crate::prompt::locked_templates::{HEADER, SECTION_SEP};
        use llm_client::ContentBlock as LlmContentBlock;
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        // A HEADER-prefixed assembled-shape system prompt splits into prefix +
        // rest (splitSysPromptPrefix default mode), both org-scoped.
        let system = format!("{HEADER}{SECTION_SEP}rest body here");
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some(&system),
                vec![text_user_msg("hello")],
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        // (a) two system blocks: prefix (HEADER) + rest, each org-scoped → each
        // carries an ephemeral breakpoint (the attribution block is never
        // emitted, so 2 not 3).
        assert_eq!(req.system.len(), 2);
        assert_eq!(req.system[0].text, HEADER);
        assert_eq!(req.system[0].cache_control, Some(CacheControl::Ephemeral));
        assert_eq!(req.system[1].text, "rest body here");
        assert_eq!(req.system[1].cache_control, Some(CacheControl::Ephemeral));
        // (c) the last message's last block carries the one message breakpoint.
        let last = req.messages.last().expect("a message");
        match last.content.last().expect("a content block") {
            LlmContentBlock::Text { cache_control, .. } => {
                assert_eq!(*cache_control, Some(CacheControl::Ephemeral));
            }
            other => panic!("expected trailing text block, got {other:?}"),
        }
    }

    #[test]
    fn build_request_omits_cache_breakpoints_when_disabled() {
        use crate::prompt::locked_templates::{HEADER, SECTION_SEP};
        use llm_client::ContentBlock as LlmContentBlock;
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DISABLE_PROMPT_CACHING", "1");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let system = format!("{HEADER}{SECTION_SEP}rest body here");
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some(&system),
                vec![text_user_msg("hello")],
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        // Still split into 2 blocks, but none carry a breakpoint.
        assert_eq!(req.system.len(), 2);
        assert_eq!(req.system[0].cache_control, None);
        assert_eq!(req.system[1].cache_control, None);
        let last = req.messages.last().expect("a message");
        match last.content.last().expect("a content block") {
            LlmContentBlock::Text { cache_control, .. } => assert_eq!(*cache_control, None),
            other => panic!("expected trailing text block, got {other:?}"),
        }
    }

    #[test]
    fn build_request_global_cache_gate_dormant_by_default() {
        // Without the opt-in env, the global gate is off even for a subscriber:
        // a boundary-bearing prompt still splits org-default (2 blocks).
        use crate::prompt::locked_templates::{HEADER, SECTION_SEP};
        use crate::prompt::SYSTEM_PROMPT_DYNAMIC_BOUNDARY;
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        std::env::remove_var("CLAUDE_CODE_GLOBAL_CACHE_SCOPE");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(
            transport,
            SubscriberState { is_subscriber: true, is_enterprise: false },
        );
        let system = format!(
            "{HEADER}{SECTION_SEP}static{SECTION_SEP}{SYSTEM_PROMPT_DYNAMIC_BOUNDARY}{SECTION_SEP}dynamic"
        );
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some(&system),
                vec![text_user_msg("hi")],
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        // Gate off → org default (no scope:global block, marker left inline).
        assert_eq!(req.system.len(), 2);
        assert_eq!(req.system[0].cache_control, Some(CacheControl::Ephemeral));
    }

    #[test]
    fn build_request_global_cache_gate_armed_marks_global_static() {
        // With the opt-in env + subscriber, the 1P global path activates and the
        // static block carries scope:global while prefix/dynamic are uncached.
        use crate::prompt::locked_templates::{HEADER, SECTION_SEP};
        use crate::prompt::SYSTEM_PROMPT_DYNAMIC_BOUNDARY;
        use llm_client::CacheScope;
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        std::env::remove_var("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS");
        std::env::set_var("CLAUDE_CODE_GLOBAL_CACHE_SCOPE", "1");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter_with_subscriber(
            transport,
            SubscriberState { is_subscriber: true, is_enterprise: false },
        );
        let system = format!(
            "{HEADER}{SECTION_SEP}static{SECTION_SEP}{SYSTEM_PROMPT_DYNAMIC_BOUNDARY}{SECTION_SEP}dynamic"
        );
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some(&system),
                vec![text_user_msg("hi")],
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        std::env::remove_var("CLAUDE_CODE_GLOBAL_CACHE_SCOPE");
        assert_eq!(req.system.len(), 3);
        assert_eq!(req.system[0].text, HEADER);
        assert_eq!(req.system[0].cache_control, None); // prefix uncached
        assert_eq!(req.system[1].text, "static");
        assert_eq!(
            req.system[1].cache_control,
            Some(CacheControl::EphemeralScoped { scope: Some(CacheScope::Global), ttl_1h: false })
        );
        assert_eq!(req.system[2].text, "dynamic");
        assert_eq!(req.system[2].cache_control, None); // dynamic uncached
    }

    // ── 1P cache-EDITING (cache_edits / cache_reference, RESIDUAL 4) ───────────

    /// A multi-message conversation: an assistant tool_use, a user tool_result,
    /// an assistant text, then a trailing user text. Only the trailing message
    /// carries the cache_control marker, so the tool_result (in an earlier user
    /// message) is strictly within the cached prefix.
    fn tool_result_conversation() -> Vec<ConversationMessage> {
        use protocol::{ContentBlock as PB, ConversationMessage as CM, MessageId, ToolUseId};
        let tool_id = ToolUseId::new();
        vec![
            CM::Assistant {
                id: MessageId::new(),
                content: vec![PB::ToolUse {
                    id: tool_id.clone(),
                    name: "Read".to_string(),
                    input: serde_json::json!({"path": "/x"}),
                    provider_id: Some("toolu_abc".to_string()),
                }],
                stop_reason: None,
            },
            CM::User {
                id: MessageId::new(),
                content: vec![PB::ToolResult {
                    tool_use_id: tool_id,
                    content: "file body".to_string(),
                    is_error: false,
                    provider_tool_use_id: Some("toolu_abc".to_string()),
                    content_blocks: None,
                }],
                is_meta: false,
            },
            CM::Assistant {
                id: MessageId::new(),
                content: vec![PB::Text { text: "ok".to_string() }],
                stop_reason: None,
            },
            text_user_msg("continue"),
        ]
    }

    #[test]
    fn build_request_cache_editing_dormant_by_default() {
        // Gate OFF (default): no cache_reference on tool_results, no cache_edits
        // block — byte-identical to the pre-feature request.
        use llm_client::ContentBlock as LlmContentBlock;
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        std::env::remove_var("CLAUDE_CODE_CACHE_EDITING");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        // Even a subscriber + injected edits must stay inert without the env.
        let adapter = make_adapter_with_subscriber(
            transport,
            SubscriberState { is_subscriber: true, is_enterprise: false },
        )
        .with_cache_editing_inputs(CacheEditingInputs {
            new_edits: vec![llm_client::CacheEdit::Delete {
                cache_reference: "toolu_zzz".to_string(),
            }],
            pinned: vec![],
        });
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some("sys"),
                tool_result_conversation(),
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        // No cache_edits block anywhere.
        for m in &req.messages {
            for b in &m.content {
                assert!(
                    !matches!(b, LlmContentBlock::CacheEdits { .. }),
                    "no cache_edits block on the default path"
                );
                if let LlmContentBlock::ToolResult { cache_reference, .. } = b {
                    assert_eq!(*cache_reference, None, "no cache_reference by default");
                }
            }
        }
    }

    #[test]
    fn build_request_cache_editing_armed_stamps_refs_and_inserts_block() {
        // Gate ARMED (subscriber + opt-in env): tool_results before the marker
        // get cache_reference=tool_use_id, and injected new+pinned cache_edits
        // are inserted with cross-block delete-ref dedup.
        use llm_client::{CacheEdit, ContentBlock as LlmContentBlock};
        let _guard = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISABLE_PROMPT_CACHING");
        std::env::remove_var("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS");
        std::env::set_var("CLAUDE_CODE_CACHE_EDITING", "1");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        // pinned (pos 1, the tool_result user msg) deletes ref "dup" + "p1";
        // new (last user msg) deletes "dup" (collapsed by dedup) + "n1".
        let adapter = make_adapter_with_subscriber(
            transport,
            SubscriberState { is_subscriber: true, is_enterprise: false },
        )
        .with_cache_editing_inputs(CacheEditingInputs {
            new_edits: vec![
                CacheEdit::Delete { cache_reference: "dup".to_string() },
                CacheEdit::Delete { cache_reference: "n1".to_string() },
            ],
            pinned: vec![PinnedCacheEdits {
                user_message_index: 1,
                edits: vec![
                    CacheEdit::Delete { cache_reference: "dup".to_string() },
                    CacheEdit::Delete { cache_reference: "p1".to_string() },
                ],
            }],
        });
        let req = adapter
            .build_request(
                "claude-sonnet-4-20250514",
                None,
                Some("sys"),
                tool_result_conversation(),
                vec![],
                false,
                Some(1024),
            )
            .expect("build_request");
        std::env::remove_var("CLAUDE_CODE_CACHE_EDITING");

        // (a) cache_reference stamped on the tool_result (it precedes the marker).
        let mut stamped = 0;
        for m in &req.messages {
            for b in &m.content {
                if let LlmContentBlock::ToolResult { tool_call_id, cache_reference, .. } = b {
                    assert_eq!(cache_reference.as_deref(), Some(tool_call_id.as_str()));
                    stamped += 1;
                }
            }
        }
        assert_eq!(stamped, 1, "exactly one tool_result stamped");

        // (b) collect every cache_edits delete ref across the whole request.
        let mut refs: Vec<String> = vec![];
        for m in &req.messages {
            for b in &m.content {
                if let LlmContentBlock::CacheEdits { edits } = b {
                    for e in edits {
                        let CacheEdit::Delete { cache_reference } = e;
                        refs.push(cache_reference.clone());
                    }
                }
            }
        }
        refs.sort();
        // dedup: "dup" appears once (pinned wins, new collapses), plus p1 + n1.
        assert_eq!(refs, vec!["dup".to_string(), "n1".to_string(), "p1".to_string()]);

        // (c) the pinned block landed in the tool_result user message, spliced
        // immediately AFTER the tool_result block.
        let pinned_msg = &req.messages[1];
        let tr_pos = pinned_msg
            .content
            .iter()
            .position(|b| matches!(b, LlmContentBlock::ToolResult { .. }))
            .expect("tool_result present");
        assert!(matches!(
            pinned_msg.content[tr_pos + 1],
            LlmContentBlock::CacheEdits { .. }
        ));
    }

    // ── build_request profile threading (Unit B Task 5) ──────────────────────

    #[test]
    fn build_request_sets_profile_when_provided() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let req = adapter
            .build_request(
                "gpt-5.2",
                Some("github-copilot"),
                None,
                vec![],
                vec![],
                false,
                None,
            )
            .expect("build_request with profile");
        assert_eq!(req.model, "gpt-5.2", "model must be preserved verbatim");
        assert_eq!(
            req.profile.as_deref(),
            Some("github-copilot"),
            "profile must be threaded into LlmRequest"
        );
    }

    #[test]
    fn build_request_leaves_profile_none_when_not_provided() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let req = adapter
            .build_request(
                "claude-opus-4-7",
                None,
                None,
                vec![],
                vec![],
                false,
                None,
            )
            .expect("build_request without profile");
        assert_eq!(req.model, "claude-opus-4-7", "model must be preserved verbatim");
        assert!(req.profile.is_none(), "profile must be None when not passed");
    }

    // ── build_request thinking / temperature / max_tokens (DIV-1/3/4) ────────

    // Serialize the env-touching thinking tests: they mutate process-global
    // CLAUDE_CODE_DISABLE_THINKING. A module-level mutex keeps them from racing
    // each other (and is poison-tolerant).
    static THINKING_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_thinking_env() {
        std::env::remove_var("CLAUDE_CODE_DISABLE_THINKING");
        std::env::remove_var("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING");
    }

    #[test]
    fn build_request_adaptive_models_get_adaptive_no_temperature_model_max_tokens() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport); // default ThinkingConfig::Adaptive

        // (model, expected model-max-output-tokens) — binary YCe (v2.1.183):
        // opus-4-8 / fable-5 → 64k default; sonnet-4-6 → 32k default.
        for (model, expected_max) in [
            ("claude-opus-4-8", 64_000u32),
            ("claude-sonnet-4-6", 32_000),
            ("claude-fable-5", 64_000),
        ] {
            let req = adapter
                .build_request(model, None, None, vec![], vec![], false, None)
                .expect("build_request");
            assert_eq!(
                req.reasoning,
                Some(llm_client::ReasoningConfig::Adaptive),
                "{model} → adaptive"
            );
            assert!(req.temperature.is_none(), "{model} → no temperature when thinking on");
            assert_eq!(req.max_tokens, Some(expected_max), "{model} → model max_tokens");
        }
        clear_thinking_env();
    }

    #[test]
    fn build_request_non_adaptive_model_gets_fixed_budget() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport); // default Adaptive intent, but model disallows adaptive

        // haiku-4-5 supports thinking but NOT adaptive → Enabled{upperLimit-1}.
        // compaction max output for haiku-4-5 = (32_000, 64_000) → budget 63_999,
        // clamped to max_tokens(32_000)-1 = 31_999.
        let req = adapter
            .build_request("claude-haiku-4-5", None, None, vec![], vec![], false, None)
            .expect("build_request");
        assert_eq!(req.max_tokens, Some(32_000));
        assert_eq!(
            req.reasoning,
            Some(llm_client::ReasoningConfig::Enabled { budget_tokens: 31_999 }),
            "haiku-4-5 → fixed budget clamped to max_tokens-1"
        );
        assert!(req.temperature.is_none(), "thinking on → no temperature");
        clear_thinking_env();
    }

    #[test]
    fn build_request_disable_thinking_env_drops_reasoning_sets_temperature() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        std::env::set_var("CLAUDE_CODE_DISABLE_THINKING", "1");
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);

        let req = adapter
            .build_request("claude-opus-4-8", None, None, vec![], vec![], false, None)
            .expect("build_request");
        assert!(req.reasoning.is_none(), "thinking disabled → no reasoning");
        assert_eq!(req.temperature, Some(1.0), "thinking disabled → temperature 1");
        // max_tokens still the model value (binary YCe: opus-4-8 → 64k).
        assert_eq!(req.max_tokens, Some(64_000));
        clear_thinking_env();
    }

    #[test]
    fn build_request_explicit_max_tokens_override_wins() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        // Escalation override (Some) honored verbatim.
        let req = adapter
            .build_request("claude-opus-4-8", None, None, vec![], vec![], false, Some(7_777))
            .expect("build_request");
        assert_eq!(req.max_tokens, Some(7_777));
        clear_thinking_env();
    }

    #[test]
    fn build_request_thinking_disabled_config_drops_reasoning() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport)
            .with_thinking(crate::model::thinking::ThinkingConfig::Disabled);
        let req = adapter
            .build_request("claude-opus-4-8", None, None, vec![], vec![], false, None)
            .expect("build_request");
        assert!(req.reasoning.is_none(), "ThinkingConfig::Disabled → no reasoning");
        assert_eq!(req.temperature, Some(1.0));
        clear_thinking_env();
    }

    #[test]
    fn build_request_metadata_threaded_when_set() {
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_thinking_env();
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport).with_request_metadata(llm_client::RequestMetadata {
            user_id: "{\"session_id\":\"s1\"}".to_string(),
        });
        let req = adapter
            .build_request("claude-opus-4-8", None, None, vec![], vec![], false, None)
            .expect("build_request");
        assert_eq!(
            req.metadata,
            Some(llm_client::RequestMetadata { user_id: "{\"session_id\":\"s1\"}".to_string() })
        );

        // Default adapter → no metadata.
        let bare = make_adapter(FakeTransport::always(ProviderResponse::json(
            200,
            ok_response_json(),
        )))
        .build_request("claude-opus-4-8", None, None, vec![], vec![], false, None)
        .expect("build_request");
        assert!(bare.metadata.is_none());
        clear_thinking_env();
    }

    #[test]
    fn build_api_metadata_user_id_shapes_and_orders() {
        // Serialize env access (CLAUDE_CODE_EXTRA_METADATA) with the other
        // env-mutating tests in this module.
        let _g = THINKING_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CLAUDE_CODE_EXTRA_METADATA");
        // No extra → exactly the three canonical keys, in order, compact JSON.
        assert_eq!(
            ProviderApiAdapter::build_api_metadata_user_id("dev123", "acct-9", "sess-1"),
            r#"{"device_id":"dev123","account_uuid":"acct-9","session_id":"sess-1"}"#
        );
        // Empty account_uuid (the `?? ''` branch) still emits the key.
        assert_eq!(
            ProviderApiAdapter::build_api_metadata_user_id("d", "", "s"),
            r#"{"device_id":"d","account_uuid":"","session_id":"s"}"#
        );
        // Valid extra object is spread FIRST; a colliding key keeps its first
        // position but takes the canonical value (JS `{...extra, device_id,…}`).
        std::env::set_var(
            "CLAUDE_CODE_EXTRA_METADATA",
            r#"{"team":"core","device_id":"override"}"#,
        );
        assert_eq!(
            ProviderApiAdapter::build_api_metadata_user_id("dev", "acct", "sess"),
            r#"{"team":"core","device_id":"dev","account_uuid":"acct","session_id":"sess"}"#
        );
        // Invalid extra (not a JSON object) is ignored.
        std::env::set_var("CLAUDE_CODE_EXTRA_METADATA", "not json");
        assert_eq!(
            ProviderApiAdapter::build_api_metadata_user_id("d", "a", "s"),
            r#"{"device_id":"d","account_uuid":"a","session_id":"s"}"#
        );
        std::env::remove_var("CLAUDE_CODE_EXTRA_METADATA");
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
            .messages_create("claude-sonnet-4-20250514", None, Some("sys"), Vec::new(), Vec::new())
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

    #[test]
    fn list_model_listings_exposes_catalog() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);
        let listings = OrchestratorApiClient::list_model_listings(&adapter);
        // The static llm-client catalog (openrouter + deepseek + glm-coding +
        // zai + github-copilot) yields well over 100 model rows.
        assert!(
            listings.len() >= 100,
            "expected >=100 catalog listings, got {}",
            listings.len()
        );
        // The four catalog providers appear with their hand-authored labels.
        let label_for = |id: &str| -> Option<String> {
            listings
                .iter()
                .find(|l| l.provider_id == id)
                .map(|l| l.provider_label.clone())
        };
        assert_eq!(label_for("openrouter").as_deref(), Some("OpenRouter"));
        assert_eq!(label_for("deepseek").as_deref(), Some("DeepSeek"));
        assert_eq!(label_for("glm-coding").as_deref(), Some("GLM (coding)"));
        assert_eq!(label_for("github-copilot").as_deref(), Some("GitHub Copilot"));
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
            .messages_create("claude-sonnet-4-20250514", None, Some("sys"), Vec::new(), tools)
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), tools)
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
            is_meta: false,
        }];
        // The capability check is in DefaultLlmClient.validate_capabilities; since
        // FakeTransport doesn't inspect the body, this exercises the whole path.
        let _ = adapter
            .messages_create("claude-sonnet-4-20250514", None, None, msgs, Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            is_meta: false,
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
                    provider_tool_use_id: None,
                    content_blocks: None,
                },
                img(0),
            ],
            is_meta: false,
        }];
        assert_eq!(count_media(&msgs), 1);
    }

    #[test]
    fn count_media_includes_nested_tool_result_media() {
        // An MCP image result populates `content_blocks` with `{"type":"image"}`
        // values; these MUST count toward the media cap (claude.ts:965-969), or an
        // image-heavy MCP transcript silently exceeds the API limit and 400s.
        let msgs = vec![ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: protocol::ToolUseId::new(),
                content: "see images".to_string(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: Some(vec![
                    serde_json::json!({"type": "text", "text": "x"}),
                    serde_json::json!({"type": "image", "source": {"data": "AAA"}}),
                    serde_json::json!({"type": "image", "source": {"data": "BBB"}}),
                ]),
            }],
            is_meta: false,
        }];
        assert_eq!(count_media(&msgs), 2, "two nested image blocks must count");
    }

    #[test]
    fn strip_excess_media_strips_nested_tool_result_media() {
        // Over the cap, nested tool_result media is stripped oldest-first
        // (claude.ts:982-999), leaving the text + the most-recent nested image.
        let msgs = vec![ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: protocol::ToolUseId::new(),
                content: "imgs".to_string(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: Some(vec![
                    serde_json::json!({"type": "image", "source": {"data": "a"}}),
                    serde_json::json!({"type": "image", "source": {"data": "b"}}),
                    serde_json::json!({"type": "image", "source": {"data": "c"}}),
                    serde_json::json!({"type": "text", "text": "keep"}),
                ]),
            }],
            is_meta: false,
        }];
        let stripped = strip_excess_media(msgs, 1);
        assert_eq!(count_media(&stripped), 1, "nested media trimmed to the limit");
        // The text block and the newest image survive.
        let ConversationMessage::User { content, .. } = &stripped[0] else {
            panic!("user message");
        };
        let ContentBlock::ToolResult { content_blocks: Some(blocks), .. } = &content[0] else {
            panic!("tool_result with content_blocks");
        };
        assert_eq!(blocks.len(), 2, "one image + the text remain; got {blocks:?}");
        assert!(blocks.iter().any(|v| v.get("type").and_then(|t| t.as_str()) == Some("text")));
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
            is_meta: false,
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
            .await;
        assert!(result.is_err(), "subscriber 429 must be terminal");
        // Only ONE execution — no retries.
        assert_eq!(
            transport.seen_count(),
            1,
            "subscriber 429 must not retry (seen_count should be 1)"
        );
    }

    /// Directly guards [`ProviderApiAdapter::clear_pending_429`]: a staged 429
    /// snapshot must be discarded so a subsequent promote writes NOTHING. This
    /// goes RED iff `clear_pending_429`'s body is emptied (a no-op clear leaves
    /// the slot `Some`, so promote would copy A's `rejected` snapshot into
    /// `last_rate_limit`). The active cross-drive isolation is the per-attempt
    /// record stage-or-clear; this test is the credibility guard for the
    /// defensive drive-entry / success backstop.
    #[test]
    fn clear_pending_429_discards_staged_snapshot() {
        let transport = FakeTransport::always(ProviderResponse::json(200, ok_response_json()));
        let adapter = make_adapter(transport);

        // Stage: a 429 carrying a representative-claim → pending = Some(info).
        let headers = {
            let mut h = std::collections::BTreeMap::new();
            h.insert(
                "anthropic-ratelimit-unified-representative-claim".to_string(),
                "seven_day".to_string(),
            );
            h.insert(
                "anthropic-ratelimit-unified-status".to_string(),
                "rejected".to_string(),
            );
            h
        };
        adapter.record_rate_limit_from_429(&headers);
        // Sanity: the slot is genuinely staged before we clear it.
        assert!(
            adapter.pending_429.lock().unwrap().is_some(),
            "precondition: record_rate_limit_from_429 must stage a snapshot"
        );

        // Clear, then a terminal promote: with the slot emptied, promote is a
        // no-op and `last_rate_limit` stays None.
        adapter.clear_pending_429();
        adapter.promote_pending_429();

        assert_eq!(
            OrchestratorApiClient::last_rate_limit_full(&adapter),
            None,
            "clear_pending_429 must discard the staged snapshot so promote writes nothing"
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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

    // ── B6-T1: terminal-only 429 state promotion (pending slot) ──────────────
    //
    // claude-code updates the limits/raw module state ONLY in the terminal
    // catch handler `extractQuotaStatusFromError` (claudeAiLimits.ts:487),
    // never on a retried attempt that later recovers. The Rust seam stages
    // each 429 attempt's snapshot in a `pending_429` slot and promotes it into
    // the live caches only when the retry loop declares the error TERMINAL —
    // and discards it on drive-entry and on any subsequent success.

    /// A 429 carrying BOTH the unified limits headers (gate passes) AND the
    /// per-window quartet (raw non-empty) that exhausts the retry budget
    /// promotes BOTH: the forced-`rejected` limits snapshot
    /// (`last_rate_limit_full`) and the raw per-window utilization
    /// (`last_raw_utilization`) — `extractRawUtilization` runs on the SAME
    /// error headers (claudeAiLimits.ts:500).
    #[tokio::test]
    async fn terminal_429_promotes_snapshot_and_raw() {
        let headers = {
            let mut h = BTreeMap::new();
            // Limits gate (from_429_error_headers → Some).
            h.insert(
                "anthropic-ratelimit-unified-representative-claim".to_string(),
                "seven_day".to_string(),
            );
            // Per-window quartet → raw non-empty.
            h.insert(
                "anthropic-ratelimit-unified-5h-utilization".to_string(),
                "0.42".to_string(),
            );
            h.insert(
                "anthropic-ratelimit-unified-5h-reset".to_string(),
                "1750000005".to_string(),
            );
            h.insert(
                "anthropic-ratelimit-unified-7d-utilization".to_string(),
                "0.77".to_string(),
            );
            h.insert(
                "anthropic-ratelimit-unified-7d-reset".to_string(),
                "1750000007".to_string(),
            );
            h
        };
        let transport = FakeTransport::always(ProviderResponse {
            status: 429,
            headers,
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "rate limited"}
            }),
            request_id: None,
        });
        // Subscriber (non-enterprise) → the 429 is terminal on the first try.
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState { is_subscriber: true, is_enterprise: false },
        );
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
            .await;
        assert!(matches!(result, Err(LlmError::RateLimited { .. })), "got {result:?}");

        // Promoted at the terminal: rejected limits snapshot.
        let info = OrchestratorApiClient::last_rate_limit_full(&adapter).expect("snapshot");
        assert_eq!(info.status.as_deref(), Some("rejected"));
        assert_eq!(info.rate_limit_type.as_deref(), Some("seven_day"));

        // Promoted at the terminal: raw per-window utilization from the SAME
        // error headers (extractRawUtilization, ts:500).
        let raw = OrchestratorApiClient::last_raw_utilization(&adapter).expect("raw");
        assert_eq!(raw.five_hour.map(|w| w.utilization), Some(0.42));
        assert_eq!(raw.five_hour.map(|w| w.resets_at), Some(1_750_000_005));
        assert_eq!(raw.seven_day.map(|w| w.utilization), Some(0.77));
        assert_eq!(raw.seven_day.map(|w| w.resets_at), Some(1_750_000_007));
    }

    /// A 429-with-headers that is RETRIED and then RECOVERS on a 200 must NOT
    /// leave the rejected snapshot behind. This verifies the SUCCESS PATH:
    /// after recovery, `last_rate_limit` reflects the 200 (here headerless →
    /// `None`), NOT the retried 429 — because the limits cache is only written
    /// at the terminal promote, which never fires on a recovered turn. (The
    /// success-clear of the pending SLOT itself is guarded directly by
    /// `clear_pending_429_discards_staged_snapshot`, not here.)
    #[tokio::test]
    async fn retried_429_does_not_update_limits_snapshot() {
        let headers_429 = {
            let mut h = BTreeMap::new();
            h.insert(
                "anthropic-ratelimit-unified-representative-claim".to_string(),
                "seven_day".to_string(),
            );
            h.insert(
                "anthropic-ratelimit-unified-status".to_string(),
                "rejected".to_string(),
            );
            // retry-after 0 → no real sleep.
            h.insert("retry-after".to_string(), "0".to_string());
            h
        };
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(ProviderResponse {
                status: 429,
                headers: headers_429,
                body_json: serde_json::json!({
                    "type": "error",
                    "error": {"type": "rate_limit_error", "message": "rate limited"}
                }),
                request_id: None,
            }),
            // Recovery: a 200 with NO unified headers.
            FakeResponse::Ok(ProviderResponse::json(200, ok_response_json())),
        ]);
        // Enterprise subscriber → the 429 is retried, then the 200 recovers.
        let adapter = make_adapter_with_subscriber(
            transport.clone(),
            SubscriberState { is_subscriber: true, is_enterprise: true },
        );
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
            .await;
        assert!(result.is_ok(), "429 then 200 must recover: {result:?}");

        // The success seam cleared the limits cache (the 200 carried no unified
        // headers); the retried 429's rejected snapshot was NEVER promoted.
        assert_eq!(
            OrchestratorApiClient::last_rate_limit_full(&adapter),
            None,
            "retried-then-recovered 429 must not plant a rejected snapshot"
        );
        // The 429 message copy was likewise cleared on success.
        assert_eq!(
            OrchestratorApiClient::last_rate_limit_error_message(&adapter),
            None,
        );
    }

    /// Cross-drive isolation, asserted on the OBSERVABLE promoted snapshot: a
    /// pending 429 staged by an EARLIER drive must never survive into a LATER
    /// drive's terminal promote. Drive A = `[429-with-headers(seven_day),
    /// 400-invalid-request terminal]` — A stages a `seven_day` snapshot but the
    /// 400 is non-RateLimited, so it never promotes and the slot is orphaned.
    /// Drive B ends on a TERMINAL 429 carrying its OWN fresh headers
    /// (`five_hour`, DIFFERENT from A's) → B promotes B's snapshot. The promoted
    /// `last_rate_limit_full()` must read `five_hour` (DRIVE B), proving A's
    /// orphaned `seven_day` slot did NOT leak in.
    ///
    /// The ACTIVE isolation mechanism this asserts is the per-attempt record
    /// stage-or-clear in `record_rate_limit_from_429` (drive B's first attempt
    /// overwrites the slot with B's snapshot before the terminal promote runs).
    /// The drive-entry reset is a defensive backstop, not what this test
    /// exercises — `clear_pending_429_discards_staged_snapshot` guards that
    /// directly. This test is non-vacuous: it fails if promotion ever reads a
    /// stale slot (B's snapshot would be wrong, or `seven_day` would surface).
    #[tokio::test]
    async fn stale_pending_429_not_promoted_across_drives() {
        // Drive A's 429: representative-claim = seven_day. Staged then orphaned.
        let resp_429_a = ProviderResponse {
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
                h.insert("retry-after".to_string(), "0".to_string());
                h
            },
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "rate limited"}
            }),
            request_id: None,
        };
        // A plain 400 invalid_request → terminal, NON-rate-limited (no promote).
        let resp_400 = ProviderResponse {
            status: 400,
            headers: BTreeMap::new(),
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "invalid_request_error", "message": "bad request"}
            }),
            request_id: None,
        };
        // Drive B's 429: representative-claim = five_hour (DIFFERENT from A).
        // Terminal here → B promotes B's OWN fresh snapshot.
        let resp_429_b = ProviderResponse {
            status: 429,
            headers: {
                let mut h = BTreeMap::new();
                h.insert(
                    "anthropic-ratelimit-unified-representative-claim".to_string(),
                    "five_hour".to_string(),
                );
                h.insert(
                    "anthropic-ratelimit-unified-status".to_string(),
                    "rejected".to_string(),
                );
                h.insert("retry-after".to_string(), "0".to_string());
                h
            },
            body_json: serde_json::json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "rate limited"}
            }),
            request_id: None,
        };
        // Drive A consumes idx 0,1; Drive B consumes idx 2,3 (global cursor).
        let transport = FakeTransport::sequence(vec![
            FakeResponse::Ok(resp_429_a),
            FakeResponse::Ok(resp_400),
            FakeResponse::Ok(resp_429_b.clone()),
            FakeResponse::Ok(resp_429_b),
        ]);
        // Non-subscriber (429 is retryable), settings_max_retries=1 so each
        // drive retries exactly once then terminates; backoff 0 → no sleeps.
        let adapter = make_adapter_with_routing(
            transport.clone(),
            std::collections::BTreeMap::new(),
            Some(1),
            Some(0),
        );

        // Drive A: 429(seven_day) (retried, pending set) → 400 (terminal,
        // non-RateLimited → no promotion). Pending lingers with A's snapshot.
        let a = adapter
            .messages_create("claude-haiku-4-20250307", None, None, Vec::new(), Vec::new())
            .await;
        assert!(
            matches!(a, Err(LlmError::InvalidRequest { .. })),
            "drive A must die on the 400, got {a:?}"
        );

        // Drive B: 429(five_hour) (retried, pending OVERWRITTEN with B's
        // snapshot) → 429(five_hour) terminal → promotes B's snapshot.
        let b = adapter
            .messages_create("claude-haiku-4-20250307", None, None, Vec::new(), Vec::new())
            .await;
        assert!(
            matches!(b, Err(LlmError::RateLimited { .. })),
            "drive B must die on its terminal 429, got {b:?}"
        );

        // The promoted snapshot must be DRIVE B's (five_hour), proving drive A's
        // orphaned seven_day slot did not survive into B's promote.
        let info = OrchestratorApiClient::last_rate_limit_full(&adapter)
            .expect("drive B promotes its own snapshot");
        assert_eq!(
            info.rate_limit_type.as_deref(),
            Some("five_hour"),
            "promoted snapshot must reflect DRIVE B (five_hour), not A's stale seven_day"
        );
        assert_eq!(info.status.as_deref(), Some("rejected"));
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
                            reasoning: true,
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
                        capabilities: Capabilities {
                            reasoning: true,
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
            .messages_create("claude-future-9999", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
                                reasoning: true,
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
                                reasoning: true,
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
                                reasoning: true,
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
            None,
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            Some(1000), // backoff_ms = 1000 → first rung 1000, additive jitter [1000, 1250)
        );

        let before = tokio::time::Instant::now();
        let result = adapter
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
            .await;
        let elapsed = before.elapsed();

        assert!(result.is_ok(), "should succeed after retry: {result:?}");
        // Additive jitter (binary `sle`: base + rand(0,0.25)·base) → base 1000
        // gives [1000, 1250). Default base 500 → [500, 625), upper 625 < 800.
        // So ≥ 800 ms proves the 1000 ms base (not the default 500) is in effect.
        assert!(
            elapsed >= std::time::Duration::from_millis(800),
            "backoff_ms=1000 should produce ≥ 800 ms delay; elapsed={elapsed:?}"
        );
        // Must be < 1250 ms (upper additive-jitter bound: 1000 + 0.25·1000).
        assert!(
            elapsed < std::time::Duration::from_millis(1250),
            "backoff_ms=1000 delay should be < 1250 ms; elapsed={elapsed:?}"
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
            .await
            .expect("ok");

        // No unified headers → None still.
        assert!(
            adapter.last_rate_limit_info().is_none(),
            "last_rate_limit_info must be None when no unified headers are present"
        );
    }

    // ── OrchestratorApiClient::count_tokens ─────────────────────────────────────

    /// The adapter override drives the real `/v1/messages/count_tokens` endpoint
    /// on an Anthropic route: it sends one request to the count_tokens URL
    /// carrying the `count_tokens` beta header and returns the decoded
    /// `input_tokens` from the response.
    #[tokio::test]
    async fn count_tokens_through_adapter_hits_anthropic_endpoint_with_beta() {
        let transport = FakeTransport::always(ProviderResponse {
            status: 200,
            headers: BTreeMap::new(),
            body_json: serde_json::json!({ "input_tokens": 2095 }),
            request_id: None,
        });
        let adapter = make_adapter(transport.clone());

        let count = OrchestratorApiClient::count_tokens(
            &adapter,
            "claude-sonnet-4-20250514",
            None,
            Some("you are helpful"),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("count_tokens ok");

        assert_eq!(count, 2095, "decoded input_tokens from the count_tokens response");
        assert_eq!(transport.seen_count(), 1, "exactly one count_tokens request sent");

        let url = transport.seen.lock().unwrap()[0].url.clone();
        assert!(
            url.ends_with("/v1/messages/count_tokens"),
            "must route to the count_tokens endpoint; url={url}"
        );
        let expected_beta = crate::model::betas::assemble_beta_header(
            crate::model::betas::Provider::Anthropic,
            crate::model::betas::Endpoint::CountTokens,
            &crate::model::betas::BetaContext::for_model("claude-sonnet-4-20250514"),
        );
        assert_eq!(
            transport.seen_headers(0).get("anthropic-beta").map(String::as_str),
            Some(expected_beta.as_str()),
            "anthropic-beta header must equal assemble_beta_header(Anthropic, CountTokens)"
        );
    }

    /// The trait default (used by mocks / non-routing impls) is the byte/4
    /// approximation over the conversation text: `(system + msg text) / 4`,
    /// floored at 1.
    #[tokio::test]
    async fn count_tokens_default_impl_is_byte_over_four_approximation() {
        let mock = crate::test_support::MockApiClient::new(vec![]);
        // system = 8 bytes; one user message of 40 bytes → (8 + 40) / 4 = 12.
        let msgs = vec![protocol::ConversationMessage::user(
            protocol::MessageId::new(),
            "1234567890123456789012345678901234567890".to_string(),
        )];
        let count = OrchestratorApiClient::count_tokens(
            &mock,
            "any-model",
            None,
            Some("12345678"),
            msgs,
            Vec::new(),
        )
        .await
        .expect("default count_tokens ok");
        assert_eq!(count, 12, "(8 system + 40 user) / 4 = 12 tokens");
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
            .messages_create("claude-sonnet-4-20250514", None, None, Vec::new(), Vec::new())
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
                            reasoning: true,
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
