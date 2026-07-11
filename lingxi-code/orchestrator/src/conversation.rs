//! Conversation orchestrator.
//!
//! Drives the v0.6.0 batched turn loop. See module-level docs in `lib.rs`.

use crate::config::OrchestratorConfig;
use crate::error::OrchestratorError;
use crate::test_support::{HookExecutor, PermissionGate};
use crate::token_budget::{check_token_budget, BudgetTracker, TokenBudgetDecision};
use crate::turn_loop::{
    call_api_with_ptl_recovery, execute_one_turn_with_recovery_tracked, surface_prompt_too_long,
    surface_rapid_refill_thrashing, PtlCallOutcome, RecoveryState, TurnStepOutcome,
    MALFORMED_TOOL_USE_RETRY_FAILED, MALFORMED_TOOL_USE_RETRY_NUDGE,
    MAX_OUTPUT_TOKENS_RECOVERY_LIMIT, MAX_OUTPUT_TOKENS_RECOVERY_NUDGE, THINKING_ONLY_NUDGE,
};
use async_trait::async_trait;
use engine::SessionState;
use hooks::events::HookEvent;
use hooks::registry::HookContext;
use llm_client::{LlmError, LlmEvent, LlmResponse};
use protocol::{ConversationMessage, MessageId, SessionId};
use session::JsonlWriter;

/// Re-export of the canonical image-source shape (FROZEN in `protocol`) so callers
/// that do NOT depend on the `protocol` crate — notably the desktop bridge's
/// `OrchestratorTurnDriver` — can construct the already-decoded sources handed to
/// [`ConversationOrchestrator::run_turn_streaming_with_cancel_image_sources`].
pub use protocol::ImageSource;
use std::sync::Arc;
use telemetry::tengu::orchestrator as orch_events;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tool_api::registry::ToolRegistry;
use traits::orchestrator::ModelListing;
use traits::OutputStream;

/// Minimal contract the orchestrator needs from the API client.
///
/// Production: [`crate::provider_adapter::ProviderApiAdapter`] (Task 6)
/// drives `llm_client::DefaultLlmClient` into this shape.
/// Tests: `MockApiClient`.
#[async_trait]
pub trait OrchestratorApiClient: Send + Sync {
    /// Non-streaming `messages.create` with optional system prompt.
    ///
    /// `system` is the assembled system prompt (M5-03). `None` is a
    /// no-op (the API call omits the `"system"` key). Callers that
    /// want the assembled LingXi prompt populate it via
    /// `ConversationOrchestrator::build_system_prompt` (private).
    /// Callers with an override populate it from
    /// `OrchestratorConfig::system_prompt_override`. `tools` is the wire
    /// tool-definition array (`{name, description, input_schema}`) advertised
    /// to the model, built via `ConversationOrchestrator::build_wire_tools`
    /// (empty omits the `"tools"` key — same as the streaming `stream`).
    async fn messages_create(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError>;

    /// Non-streaming `messages.create` with an explicit `max_tokens` override
    /// (REC.A1 8k→64k escalation, TS `query.ts:1199-1221`). The turn loop calls
    /// this ONLY when a prior `max_tokens` recovery armed
    /// [`crate::turn_loop::RecoveryState::max_output_tokens_override`]; otherwise
    /// the plain [`Self::messages_create`] is used and this is never invoked.
    ///
    /// The DEFAULT body delegates to [`Self::messages_create`], dropping the
    /// override — so every mock / non-Anthropic impl compiles unchanged and the
    /// escalation is a strict no-op there. Only [`ProviderApiAdapter`]
    /// overrides it to thread `max_tokens` into the provider call.
    async fn messages_create_with_opts(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        _max_tokens: u32,
    ) -> Result<LlmResponse, LlmError> {
        self.messages_create(model, profile, system, msgs, tools)
            .await
    }

    /// Non-streaming `messages.create` with the **Opus-fallback** policy wired
    /// (Opus-fallback batch). Identical to [`Self::messages_create`] except the
    /// caller hands in the configured `fallback_model` (+ the pre-computed
    /// subscription flags `is_subscriber` / `is_enterprise`).
    ///
    /// The DEFAULT body delegates to [`Self::messages_create`], dropping the
    /// fallback args — so every existing impl (mocks, adapter, the
    /// hook-prompt mock) compiles unchanged and behaves byte-identically. Only
    /// [`ProviderApiAdapter`] overrides it to thread the fallback (Task 6).
    /// The turn loop only calls THIS method when `config.fallback_model.is_some()`;
    /// with no fallback configured it stays on `messages_create`, a strict no-op.
    ///
    /// NOTE: `LlmError` has no `FallbackTriggered` variant — that becomes
    /// adapter-internal in Task 6. The turn-loop interception of `FallbackTriggered`
    /// is removed; fallback is handled entirely within `ProviderApiAdapter`.
    #[allow(clippy::too_many_arguments)]
    async fn messages_create_with_fallback(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        _fallback_model: Option<&str>,
        _is_subscriber: bool,
        _is_enterprise: bool,
    ) -> Result<LlmResponse, LlmError> {
        // Default: ignore the fallback args and use the plain seam. Keeps all
        // non-Anthropic impls (and mocks) byte-identical.
        self.messages_create(model, profile, system, msgs, tools)
            .await
    }

    /// Non-streaming `messages.create` with a pre-seeded consecutive-529 counter.
    ///
    /// Used by the mid-stream 529 → non-streaming fallback (Task 7 / claude.ts parity):
    /// when a streaming call fails with `LlmError::Overloaded` after the first event,
    /// the turn loop issues a FRESH non-streaming call seeded with
    /// `initial_consecutive_overloaded = 1` so the retry budget accounts for the
    /// streaming 529 that triggered the fallback (`initialConsecutive529Errors` in
    /// `claude.ts:2559`).
    ///
    /// The DEFAULT body delegates to [`Self::messages_create`], ignoring the seed —
    /// so every mock / non-Anthropic impl compiles unchanged and the seeding is a
    /// strict no-op there (the mock retries from 0, which is conservative / safe).
    /// Only [`crate::provider_adapter::ProviderApiAdapter`] overrides it to thread
    /// `initial_consecutive_overloaded` into the non-stream retry driver.
    async fn messages_create_seeded(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        _initial_consecutive_overloaded: u8,
    ) -> Result<LlmResponse, LlmError> {
        // Default: ignore the seed and use the plain seam. Keeps all
        // non-Anthropic impls (and mocks) byte-identical.
        self.messages_create(model, profile, system, msgs, tools)
            .await
    }

    /// Count the input tokens a `messages.create` for `(model, system, msgs,
    /// tools)` would consume on its resolved route.
    ///
    /// [`ProviderApiAdapter`](crate::provider_adapter::ProviderApiAdapter)
    /// overrides this to call the real `/v1/messages/count_tokens` endpoint on
    /// Anthropic routes (with the `count_tokens` beta) and a byte-length/4
    /// approximation elsewhere (see [`crate::model::count_tokens`]). The default
    /// here is that same byte/4 approximation computed directly from the
    /// conversation text, so mocks and non-routing impls return a sane estimate
    /// without a network call.
    async fn count_tokens(
        &self,
        _model: &str,
        _profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<u64, LlmError> {
        let mut bytes = system.map_or(0u64, |s| s.len() as u64);
        bytes += msgs.iter().map(protocol::text_byte_size).sum::<u64>();
        Ok((bytes / crate::model::count_tokens::APPROX_CHARS_PER_TOKEN).max(1))
    }

    /// Enumerate available `provider/model` ids + `@aliases` for `/model`'s
    /// list mode. Default returns empty so non-routing impls (mocks / the
    /// no-streaming stub) need no override; `ProviderApiAdapter` overrides it
    /// to delegate to the router.
    fn available_models(&self) -> Vec<String> {
        Vec::new()
    }

    /// Richer catalog listing for the grouped `/model` picker. Default returns
    /// empty (mocks / non-routing impls); `ProviderApiAdapter` overrides it.
    fn list_model_listings(&self) -> Vec<ModelListing> {
        Vec::new()
    }

    /// Return the most recently observed rate-limit header snapshot, if any.
    ///
    /// Default returns `None`.  `ProviderApiAdapter` overrides this to delegate
    /// to [`crate::provider_adapter::ProviderApiAdapter::last_rate_limit_info`],
    /// which is populated from every successful 2xx response's headers.
    ///
    /// Returns a [`traits::RateLimitSnapshot`] carrying all three header-derived
    /// fields (`rate_limit_type`, `overage_status`, `overage_disabled_reason`).
    /// Using the public snapshot type avoids leaking the orchestrator-internal
    /// `RateLimitInfo` struct through the trait.
    fn last_rate_limit_info(&self) -> Option<traits::RateLimitSnapshot> {
        None
    }

    /// The Anthropic `request-id` response header (`req_…`) of the most
    /// recently completed call — recorded by the adapter from the stream
    /// connect-success / non-stream response headers (the same pass that records
    /// the rate-limit snapshot). Used to stamp the persisted assistant line's
    /// top-level `requestId` (claude-code's `response._request_id`). Default
    /// `None` for mocks / non-recording impls.
    fn last_request_id(&self) -> Option<String> {
        None
    }

    /// Number of budget-consuming retry attempts the most recent API call
    /// performed before succeeding. Recorded by the adapter from its retry
    /// driver's `RetryState`. Used by the cost-recording call sites to pass the
    /// real retry count to `CostTracker::record_api_response_v2` instead of the
    /// previous hardcoded `0` (#5 main-loop parity). Default `0` for mocks /
    /// non-retrying impls.
    fn last_retry_count(&self) -> u32 {
        0
    }

    /// Return the FULL most recently observed rate-limit header snapshot.
    ///
    /// Task 8 (llm-client future-work batch 3): unlike
    /// [`Self::last_rate_limit_info`] — whose signature is kept untouched and
    /// projects the three-field public `traits::RateLimitSnapshot` — this
    /// returns the orchestrator-internal nine-field
    /// [`crate::model::rate_limit::RateLimitInfo`] so the turn drivers can
    /// forward every unified header value to
    /// `traits::OutputStream::emit_rate_limit`.
    ///
    /// Default returns `None` (mocks / non-Anthropic impls compile
    /// unchanged); `ProviderApiAdapter` overrides it to expose its cached
    /// per-response snapshot.
    fn last_rate_limit_full(&self) -> Option<crate::model::rate_limit::RateLimitInfo> {
        None
    }

    /// Return the most recently observed RAW per-window utilization snapshot.
    ///
    /// Task 2 (llm-client future-work batch 5): the parallel accessor to
    /// [`Self::last_rate_limit_full`] for claude-code's `rawUtilization`
    /// tracking (`extractRawUtilization`, `claudeAiLimits.ts:164-179`) —
    /// per-window 5h/7d values recorded on every unified-headers response,
    /// independent of the warning-gated [`Self::last_rate_limit_full`]
    /// fields.
    ///
    /// Default returns `None` (mocks / non-Anthropic impls compile
    /// unchanged); `ProviderApiAdapter` overrides it to expose the snapshot
    /// cached alongside the rate-limit parse.
    fn last_raw_utilization(&self) -> Option<crate::model::rate_limit::RawUtilization> {
        None
    }

    /// Return the limits-specific copy composed from the most recent 429
    /// **error** response's unified rate-limit headers, if any.
    ///
    /// Task 6 (llm-client future-work batch 5): claude-code builds the
    /// rejected-limits view from the terminal 429's own headers and renders
    /// `getRateLimitErrorMessage` as the user-visible error content
    /// (`errors.ts:480-524`). `ProviderApiAdapter` overrides this to expose
    /// the copy it composed when it decoded the 429 (`None` when the 429
    /// carried no unified headers — the `if (rateLimitType || overageStatus)`
    /// gate at `errors.ts:480`). The public turn drivers consult it to
    /// re-map a terminal `RateLimited` error into
    /// [`OrchestratorError::RateLimitRejected`].
    ///
    /// Default returns `None` (mocks / non-Anthropic impls keep the generic
    /// `"api call failed: rate limited"` surface).
    fn last_rate_limit_error_message(&self) -> Option<String> {
        None
    }

    /// Best-effort startup prewarm for OpenAI Responses WebSocket providers.
    ///
    /// Default is a no-op so non-routing mocks and non-WebSocket clients keep
    /// their existing behavior. Production [`ProviderApiAdapter`] sends the
    /// provided empty-history request with `generate=false`.
    async fn prewarm_responses_websocket(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<(), LlmError> {
        Ok(())
    }

    /// Close any reusable Responses WebSocket session held by this API client.
    async fn close_responses_websocket_session(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Re-map a terminal `RateLimited` turn error onto the limits-specific copy
/// composed from the 429's own headers (claude-code `errors.ts:480-524`):
/// when the turn died on a 429 AND the API client recorded a composed
/// rejected-limits message, the user-visible error becomes that copy
/// ([`OrchestratorError::RateLimitRejected`]); otherwise the error passes
/// through untouched. Covers both wrappers a 429 can ride in on — the
/// batched `ApiCall` and the streaming connect-phase `Streaming`.
fn enrich_rate_limited_error(
    err: OrchestratorError,
    composed: Option<String>,
) -> OrchestratorError {
    let is_rate_limited = matches!(
        &err,
        OrchestratorError::ApiCall(LlmError::RateLimited { .. })
            | OrchestratorError::Streaming(LlmError::RateLimited { .. })
    );
    if !is_rate_limited {
        return err;
    }
    match composed {
        Some(message) => OrchestratorError::RateLimitRejected { message },
        None => err,
    }
}

/// Streaming-API surface used by the orchestrator's streaming turn loop.
///
/// Mirrors [`OrchestratorApiClient`] but returns a typed
/// `BoxStream<'static, Result<LlmEvent, LlmError>>` instead of a
/// single `LlmResponse`. The orchestrator owns the stream and drives
/// it to completion (or `message_stop` / `Completed`).
///
/// Production: [`ProviderApiAdapter`] (Task 6) drives `DefaultLlmClient`
/// directly. Tests: `MockStreamingApiClient` in `test_support_stream.rs`.
#[async_trait]
pub trait StreamingApiClient: Send + Sync {
    /// Open a streaming `messages.create` request. The returned stream
    /// yields wire-decoded `LlmEvent` values until the server emits
    /// `message_stop` or a `Completed` event. The implementation is
    /// responsible for HTTP, SSE chunk buffering, and JSON-decoding the
    /// `data:` lines into typed `LlmEvent` values.
    ///
    /// `profile` — optional provider profile name (e.g. `"github-copilot"`).
    /// Mirrors the `profile` parameter on the batched `messages_create*`
    /// methods so the streaming path can thread `SessionState::model_profile`
    /// through to `build_request` / `DefaultLlmClient::prepare`.
    async fn stream(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<futures::stream::BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError>;

    /// Connect-phase retry count of the most recent `stream` call (the value
    /// the adapter knows when it returns the stream). Used by the streaming
    /// cost-recording site to pass the real retry count to
    /// `CostTracker::record_api_response_v2` instead of `0` (#5 main-loop
    /// parity). Default `0` for mocks / non-retrying impls.
    fn last_retry_count(&self) -> u32 {
        0
    }
}

/// Outcome of a single REPL turn driven by
/// [`ConversationOrchestrator::run_turn_with_cancel`]. (M5-13)
///
/// Distinct from [`ConversationOutcome`] because the REPL needs to react
/// differently to each variant without inspecting the `stop_reason` string:
/// - `EndTurn` → silent, loop back to prompt.
/// - `MaxTurns` → print `[turn ended: reached MAX_TURNS_PER_CONVERSATION]`.
/// - `Cancelled` → print the SIGINT feedback line + loop back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// Model returned `end_turn` (or any other natural stop reason).
    EndTurn,
    /// The orchestrator's `max_turns` limit was reached before `end_turn`.
    MaxTurns,
    /// A `CancellationToken` passed to [`ConversationOrchestrator::run_turn_with_cancel`]
    /// was cancelled mid-turn (SIGINT / external cancel). The orchestrator
    /// unwound the current API call and returned early.
    Cancelled,
}

/// Result of `ConversationOrchestrator::run_turn` on success.
///
/// Only one variant in M5-02; M5-04 may add `Cancelled { ... }` later.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ConversationOutcome {
    /// Model emitted `stop_reason == "end_turn"` after `turn_count` API
    /// calls. `final_message_id` is the id of the final assistant message
    /// appended to the session.
    EndTurn {
        /// Number of API round-trips it took to reach `end_turn`.
        turn_count: u32,
        /// Stable identifier of the final assistant message.
        final_message_id: MessageId,
    },
    /// A `Stop` lifecycle hook requested *preventContinuation* (`continue:false`)
    /// — the turn loop terminated the agent rather than continuing to work
    /// (hooks B4, TS `query.ts:1278`). Distinct from [`Self::EndTurn`] so callers
    /// can tell a hook-forced stop from a natural `end_turn`.
    StopHookPrevented {
        /// Number of API round-trips before the Stop hook forced termination.
        turn_count: u32,
        /// Stable identifier of the final assistant message, if any.
        final_message_id: MessageId,
    },
}

/// Outcome of firing the `Stop` lifecycle hooks at end-of-turn (hooks B4).
///
/// Mirrors the three-way branch in TS `query.ts:1267-1306`: a Stop hook can
/// force termination (`preventContinuation`), ask the agent to keep working
/// (a bare `Block` / exit-2), or pass (no Stop hook, or it allowed the stop).
enum StopHookDisposition {
    /// No Stop hook fired, or it allowed the stop — proceed to the normal
    /// end-of-turn (token-budget check then `emit_end_turn`).
    Pass,
    /// A Stop hook blocked the stop (wants the agent to keep working). The turn
    /// loop appends the carried blocking reason as a meta user message wrapped by
    /// `getStopHookMessage` (TS `utils/hooks.ts:1895`), sets
    /// `stop_hook_active = true`, and runs one more turn step. The re-entry
    /// guard converts a *second* such block into [`Self::Pass`] so a hook that
    /// always blocks cannot loop forever (TS `query.ts:1297`). The carried
    /// `String` is the hook's blocking reason (`blockingError.blockingError`,
    /// TS `query/stopHooks.ts:257-262`), NOT the transcript-only systemMessage.
    Continue(String),
    /// A Stop hook requested `continue: false` — terminate the agent loop
    /// (TS `query.ts:1278`); the turn ends as `StopHookPrevented`. The carried
    /// `String` is the hook's `stopReason` (defaulted to
    /// `"Stop hook prevented continuation"`, TS `query/stopHooks.ts:271`) —
    /// persisted as a `hook_stopped_continuation` meta message before the turn
    /// terminates (FIX C).
    Prevent(String),
}

/// Driver control-flow directive produced by `handle_stop_at_end` (hooks B4) so
/// the three turn drivers (batched / streaming / cancelable) translate the Stop
/// disposition into their own loop mechanics uniformly.
enum StopHookFlow {
    /// Terminate the turn loop, returning this outcome (`emit_end_turn` already
    /// fired inside the helper).
    Terminate(ConversationOutcome),
    /// A Stop hook blocked the turn from ending, but the next turn would exceed
    /// `max_turns`, so end NOW on the max-turns terminal instead of looping
    /// (binary blocking-branch `if(c&&dt>c) … {reason:"max_turns",turnCount:dt}`).
    /// The caller converts this to `OrchestratorError::MaxTurnsReached`
    /// (→ `TurnOutcome::MaxTurns`); the `tengu_stop_hook_block_count`
    /// `{hit_max_turns:true}` event is fired inside the helper before returning.
    TerminateMaxTurns,
    /// A Stop hook asked the agent to keep working — loop one more turn step.
    LoopAgain,
    /// No Stop hook intervened — fall through to the driver's normal end
    /// (token-budget check, then `emit_end_turn` + break).
    FallThrough,
}

/// `getEntrypoint()` (`sessionStorage.ts:1058`) — the CLI entrypoint stamped on
/// every JSONL line. claude-code reads `process.env.CLAUDE_CODE_ENTRYPOINT`
/// (defaulting to `"cli"`); we mirror that (same env the UA builder reads,
/// `model/user_agent.rs:73`) so an embedder can override it, but the parity
/// default is the hardcoded `"cli"`.
fn entrypoint_value() -> String {
    std::env::var("CLAUDE_CODE_ENTRYPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "cli".to_string())
}

/// The top-level api-error envelope fields claude-code stamps on a synthetic
/// assistant line built via `createAssistantAPIErrorMessage` (`ql`/`tc`) or the
/// refusal builder (`fje`). On disk these are SIBLINGS of `message` —
/// `isApiErrorMessage` (always `true`), an optional `error` category string, and
/// an optional `apiErrorStatus` HTTP status — read back by the loader's
/// transcript reconstruction (`ht=Ie.isApiErrorMessage===!0, st=Ie.apiErrorStatus`).
///
/// Field shapes are pinned against the 2.1.195 binary + real transcripts:
/// - `error` is the category passed to the builder; it is OMITTED (not `null`)
///   when the builder is called with no `error:` arg — e.g. the top-level
///   `model_error` catch (`createAssistantAPIErrorMessage({content})`) and the
///   malformed-tool-use terminal (`ql({content})`).
/// - `api_error_status` mirrors `KNn`'s `r.apiErrorStatus=e.status` — set only
///   when the error was an `APIError` with a numeric status; otherwise omitted.
/// - `inner_stop_reason` overrides the synthetic inner `message.stop_reason`.
///   The `ql`/`tc` path leaves it `"stop_sequence"` (the default); the refusal
///   `fje` path keeps `"refusal"` (verified on disk: the sole refusal line
///   carries `stop_reason:"refusal", error:"invalid_request"`).
#[derive(Clone, Debug, Default)]
pub(crate) struct ApiErrorEnvelope {
    pub error: Option<&'static str>,
    pub api_error_status: Option<u16>,
    pub inner_stop_reason: Option<&'static str>,
}

/// Per-request api-error classifier — the port of claude-code's `Flp`/`KNn`
/// (the message + CATEGORY producer and its `apiErrorStatus` decorator). Maps a
/// model/runtime error that escaped the API layer to the top-level api-error
/// envelope (`error` category string + optional `apiErrorStatus`) that the
/// graceful `model_error` catch stamps on the persisted assistant line.
///
/// Recovered from the 2.1.195 binary (`Flp` if-chain + `KNn`):
/// `Flp` returns `ql({content,error:<CATEGORY>})` per branch, and `KNn` adds
/// `r.apiErrorStatus=e.status` ONLY when the error is an `APIError` with a
/// numeric status (otherwise it is OMITTED, not `null`). The category strings
/// (`rate_limit`/`invalid_request`/`server_error`/`authentication_failed`/
/// `model_not_found`/`billing_error`/`unknown`) are byte-lockable file-format
/// values read back by the loader's transcript reconstruction, so they are kept
/// verbatim from the binary.
///
/// MULTI-PROVIDER CARVE-OUT: claude's `Flp` dispatches on `e instanceof $o &&
/// e.status===N` (Anthropic `APIError`). This port instead dispatches on the
/// provider-NEUTRAL [`LlmError`] semantic enum, so the classifier works
/// identically for every provider (an OpenAI/Gemini rate-limit decoded into
/// `LlmError::RateLimited` still maps to `rate_limit`/429). Because the raw HTTP
/// status was already collapsed into the semantic variant, exact per-request
/// status recovery for the long tail is unavailable; each variant maps to its
/// CANONICAL status and the status is OMITTED (`None`) where the port has no
/// confident canonical value — mirroring claude omitting `apiErrorStatus` when
/// the error is not an `APIError`-with-numeric-status (a wrong status would be
/// worse than an omitted one). On-disk 529 lines carry `error:"server_error"`
/// (the `Flp` tail `status>=500` branch), which is authoritative over the `YNn`
/// statusline classifier's `529→"overloaded"`.
///
/// `inner_stop_reason` is always `None` here: the `ql` path leaves the synthetic
/// inner `message.stop_reason` at `"stop_sequence"` (verified on disk).
pub(crate) fn classify_api_error(e: &OrchestratorError) -> ApiErrorEnvelope {
    let (error, api_error_status) = match e {
        OrchestratorError::ApiCall(inner) | OrchestratorError::Streaming(inner) => match inner {
            // 429 family → "rate_limit" (status 429). Carved out before reaching
            // `surface_model_error` for the non-streaming path; kept for totality
            // and exercised only via a non-carved `Streaming` surface.
            LlmError::RateLimited { .. } => (Some("rate_limit"), Some(429)),
            // 529 overload: the ENVELOPE (`Flp` tail `status>=500`) and on-disk
            // 529 lines tag `server_error` — NOT the `YNn` statusline
            // `"overloaded"`. Carved out for the non-streaming path.
            LlmError::Overloaded { .. } => (Some("server_error"), Some(529)),
            // x-api-key / 401 → "authentication_failed".
            LlmError::Authentication => (Some("authentication_failed"), Some(401)),
            // 403 → "authentication_failed".
            LlmError::PermissionDenied => (Some("authentication_failed"), Some(403)),
            // Billing (`Fio`) is an Error-message match in `Flp`, not a status
            // branch → category only, no `apiErrorStatus`.
            LlmError::QuotaExceeded => (Some("billing_error"), None),
            // PTL/context-window (`Nio`/`D9t`) → `ql({error:"invalid_request"})`
            // with NO status set; the port decodes ContextOverflow from the
            // message, so no `APIError` status is available → omit.
            LlmError::ContextOverflow { .. } => (Some("invalid_request"), None),
            // 400 invalid-request family → "invalid_request" (status 400).
            LlmError::InvalidRequest { .. } => (Some("invalid_request"), Some(400)),
            // 404 / bedrock model-id → "model_not_found".
            LlmError::ModelUnavailable => (Some("model_not_found"), Some(404)),
            // `Flp` tail `status>=500` → "server_error".
            LlmError::ProviderInternal => (Some("server_error"), Some(500)),
            // Timeout / transport / connection-lost tail → "server_error", no
            // status (these are not `APIError`-with-numeric-status).
            LlmError::Transport { .. }
            | LlmError::TlsCert { .. }
            | LlmError::StreamInterrupted { .. } => (Some("server_error"), None),
            // Generic `Error` fallthrough in `Flp` → "unknown".
            LlmError::CostUnavailable { .. } | LlmError::UnsupportedCapability { .. } => {
                (Some("unknown"), None)
            }
        },
        // Generic-Error fallthrough (`Flp`: `if(e instanceof $o)→"unknown"`;
        // generic Error → "unknown"). These orchestrator-internal variants never
        // carry a status. `MaxTurnsReached`/`MaxBudgetReached` are handled
        // upstream and never reach `surface_model_error` — dead arms kept for
        // totality.
        OrchestratorError::Internal(_)
        | OrchestratorError::StreamingProtocol(_)
        | OrchestratorError::StreamEndedWithoutStop
        | OrchestratorError::Compaction(_)
        | OrchestratorError::CompactionCancelled
        | OrchestratorError::RepeatedOverloaded
        | OrchestratorError::RateLimitRejected { .. }
        | OrchestratorError::MaxTurnsReached { .. }
        | OrchestratorError::MaxBudgetReached { .. } => (Some("unknown"), None),
    };
    ApiErrorEnvelope {
        error,
        api_error_status,
        inner_stop_reason: None,
    }
}

/// Build the persisted assistant-envelope `usage` value from a normalized
/// [`llm_client::Usage`]. Prefers the raw Anthropic usage object the codec
/// retained on `provider_metadata` (byte-faithful to claude-code's persisted
/// `BetaMessage.usage`); falls back to a reconstruction from the normalized
/// billable buckets only when no raw object is present (unusual).
fn assistant_usage_value(usage: &llm_client::Usage) -> serde_json::Value {
    if usage.provider_metadata.is_object() {
        return usage.provider_metadata.clone();
    }
    let b = &usage.billable_tokens;
    serde_json::json!({
        "input_tokens": b.input,
        "cache_creation_input_tokens": b.cache_write,
        "cache_read_input_tokens": b.cache_read,
        "output_tokens": b.output,
    })
}

/// Map Rust's `std::env::consts::OS` to the node `process.platform` value that
/// claude-code's `# Environment` `Platform:` line emits (`je.platform`). Rust
/// uses `macos`/`windows`; node uses `darwin`/`win32`. Other targets
/// (`linux`, `freebsd`, …) share the same token in both, so they pass through.
fn node_platform_name(rust_os: &str) -> &str {
    match rust_os {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// `getBranch()` (`sessionStorage.ts:1012-1019`) — resolve the cwd's current git
/// branch via `git rev-parse --abbrev-ref HEAD`, or `None` on ANY failure (git
/// missing / not a repo / non-zero exit / empty output). A detached HEAD prints
/// the literal `"HEAD"`; we surface that verbatim (claude-code's `getBranch`
/// returns it too — it does not special-case detached HEAD).
///
/// Uses [`std::process::Command`] (no new dependency), the same shell-git pattern
/// the loader already uses for worktree enumeration. One-shot + cached by the
/// caller, so the synchronous `output()` runs at most once per session.
fn git_branch_for_cwd(cwd: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

/// Count `document` and `image` content blocks across a message list, for the
/// fixed-prefix overflow telemetry (`a3p`'s `documentBlockCount` /
/// `imageBlockCount`, `bin/claude.exe` offset 203004969).
///
/// The binary recurses into `tool_result.content` arrays; in this port a
/// [`protocol::ContentBlock::ToolResult`] carries a flat `String` (it cannot
/// nest image/document blocks), so counting the top-level blocks of each
/// message is the faithful equivalent. Returns `(document_count, image_count)`.
fn count_document_and_image_blocks(messages: &[protocol::ConversationMessage]) -> (u32, u32) {
    let mut documents = 0u32;
    let mut images = 0u32;
    for message in messages {
        let blocks = match message {
            protocol::ConversationMessage::User { content, .. }
            | protocol::ConversationMessage::Assistant { content, .. } => content,
            protocol::ConversationMessage::System { .. } => continue,
        };
        for block in blocks {
            match block {
                protocol::ContentBlock::Document { .. } => documents = documents.saturating_add(1),
                protocol::ContentBlock::Image { .. } => images = images.saturating_add(1),
                _ => {}
            }
        }
    }
    (documents, images)
}

/// Bare text injected as a user message when streaming is cancelled (ESC /
/// SIGINT) BEFORE any tool runs in the current turn. 1:1 with claude-code
/// `messages.ts:207` `INTERRUPT_MESSAGE`.
const INTERRUPT_MESSAGE: &str = "[Request interrupted by user]";

/// Bare text injected as a user message when streaming is cancelled (ESC /
/// SIGINT) DURING tool execution for the current turn. 1:1 with claude-code
/// `messages.ts:208` `INTERRUPT_MESSAGE_FOR_TOOL_USE`.
const INTERRUPT_MESSAGE_FOR_TOOL_USE: &str = "[Request interrupted by user for tool use]";

/// The orchestrator. Owns the session, dispatches tools, drives the loop.
///
/// Construction is via `new(...)` (batched-only) or `new_with_streaming(...)`
/// (both paths). Driven via `run_turn(prompt)` or `run_turn_streaming(prompt)`.
pub struct ConversationOrchestrator {
    pub(crate) config: OrchestratorConfig,
    pub(crate) api: Arc<dyn OrchestratorApiClient>,
    /// Streaming-path API client. Wired by `new_with_streaming`; the
    /// legacy `new` constructor wires a [`NoStreamingApiClient`] stub
    /// that always errors. Both methods share `self.session` so a
    /// caller can mix batched and streaming turns transparently.
    pub(crate) streaming_api: Arc<dyn StreamingApiClient>,
    pub(crate) tools: Arc<ToolRegistry>,
    pub(crate) hooks: Arc<HookExecutor>, // = hooks::HookExecutorImpl (M5-06)
    pub(crate) perms: Arc<dyn PermissionGate>,
    pub(crate) output: Arc<dyn OutputStream>,
    pub(crate) session: Arc<Mutex<SessionState>>,
    /// `queryTracking.chainId` for analytics (claude-code `query.ts:347-358`): a
    /// random uuid grouping a query chain, stamped onto the `queryChainId` field
    /// of `tengu_query_error` / `tengu_auto_compact_*` events. In claude-code a
    /// subagent INHERITS the parent's chainId and increments `depth`; in this
    /// port subagents never run through `ConversationOrchestrator` (the `agent`
    /// crate is a separate path), so every orchestrator IS a top-level chain —
    /// `queryDepth` is always 0 and each orchestrator owns one fresh chainId.
    /// The byte value is a host-minted uuid (shape-parity only — never matches
    /// the binary's per-run uuid).
    pub(crate) query_chain_id: String,
    /// LINGXI.md hierarchy provider (M5-03). The orchestrator calls
    /// `memory.load(&cwd).await` once per `run_turn` to gather the
    /// memory files spliced into the system prompt.
    pub(crate) memory: Arc<dyn crate::prompt::MemoryHierarchyProvider>,
    /// Working directory used as the root for the env + file-tree +
    /// git-status + memory probes inside `build_system_prompt`. M5-12
    /// CLI will plumb `--cwd`; until then, callers pass the platform
    /// caller's cwd here.
    pub(crate) cwd: std::path::PathBuf,
    /// The CURRENT working directory — the session-init `cwd` by default, but
    /// MUTATED when a `cd` inside a Bash call moves the persistent shell cwd
    /// (the desktop composition root shares this exact `Arc` with the
    /// [`crate::OrchestratorCwdChangedFirer`], which writes the new path on every
    /// `CwdChanged` fire). Hook payloads read THIS (not the static `cwd`) so a
    /// PreToolUse/PostToolUse/lifecycle hook sees the post-`cd` directory — 1:1
    /// with claude-code, where every hook reads the single global `getCwd()`
    /// that `cd` mutates (`Shell.ts:409` `setCwdState` → `cwd.ts:19` `getCwd`).
    /// Defaults to a private `Arc` over `cwd` (no firer wired ⇒ never moves ⇒
    /// hooks read the static cwd exactly as before).
    pub(crate) current_cwd: Arc<std::sync::Mutex<std::path::PathBuf>>,
    /// Resolved `$LINGXI_CONFIG_DIR ?? ~/.claude` dir (the claude-home root).
    /// Used by [`Self::computed_transcript_path`] to deterministically derive the
    /// session's transcript path (`<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`,
    /// = claude-code `getTranscriptPathForSession`) for hook payloads when no
    /// `jsonl_writer` is wired — which is the PRODUCTION case (every
    /// `with_jsonl_writer` call site is a test). `None` for library/test callers
    /// that wire neither a writer nor a config home, in which case
    /// `computed_transcript_path` returns an empty path (the prior `""` behavior).
    /// Wired at the composition root via [`Self::with_config_home`].
    pub(crate) config_home: Option<std::path::PathBuf>,
    /// Optional on-disk JSONL persistence (M5-07). `None` for in-memory
    /// tests; `Some` when the CLI binary wires `~/.lingxi/projects/.../<uuid>.jsonl`.
    pub(crate) jsonl_writer: Option<Arc<JsonlWriter>>,
    /// Cached UUID of the last persisted JSONL entry — used to populate
    /// `parentUuid` on the next append. Reset to `None` for fresh sessions.
    pub(crate) last_jsonl_uuid: Mutex<Option<String>>,
    /// Lazily-resolved git branch for the cwd — the parity analog of TS
    /// `getBranch()`, which claude-code calls once per `insertMessageChain`
    /// (`sessionStorage.ts:1012-1019`) and stamps onto every line of that chain.
    /// We resolve it ONCE on first append (`git rev-parse --abbrev-ref HEAD` in
    /// `self.cwd`, reusing the loader's shell-git pattern) and cache the result so
    /// it is not recomputed per-append. `Some(None)` means "resolved, not a repo /
    /// git failed" (→ `gitBranch` omitted, matching TS `undefined`); the outer
    /// `None` means "not yet resolved".
    ///
    /// `Option<Option<_>>` is deliberate (the three-state case clippy's
    /// `option_option` lint explicitly allows): outer `None` = unresolved, inner
    /// `None` = resolved-to-no-branch, inner `Some` = resolved branch. A bare
    /// `Option<String>` could not distinguish "unresolved" from "resolved, no
    /// branch", which would re-shell git on every append for a non-repo cwd.
    #[allow(clippy::option_option)]
    pub(crate) git_branch_cache: Mutex<Option<Option<String>>>,
    /// Stable per-prompt id for the IN-FLIGHT turn — the parity analog of TS
    /// `getPromptId()` (`sessionStorage.ts:1045-1046`), which stamps the same id
    /// on the user prompt line AND every `tool_result` `user` line of that turn.
    /// [`Self::persist_message_to_jsonl`] mints a fresh UUID when it persists a
    /// genuine new user prompt (a `user` message that is NOT a `tool_result`
    /// carrier) and reuses it for the turn's `tool_result` `user` lines; non-`user`
    /// lines never read it. `None` until the first user prompt is persisted.
    pub(crate) current_prompt_id: Mutex<Option<String>>,
    /// Set by [`traits::OrchestratorHandle::request_exit`] (M5-10).
    /// The REPL (M5-13) checks this flag at the start of each iteration
    /// and breaks the loop. Wraps `AtomicBool` so reads are lock-free.
    /// Once `true`, this flag is never cleared (idempotent `/exit`).
    pub(crate) should_exit: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Session-scoped fast-mode toggle (`/fast`). Shared (same `Arc`) with the
    /// request-building `ProviderApiAdapter` (via [`Self::with_fast_mode`]), so
    /// flipping it via the handle's `set_fast_mode` makes the next turn send
    /// `speed:"fast"` when the active model supports it. Wraps `AtomicBool` so
    /// reads are lock-free (mirrors `should_exit`). Defaults to a private
    /// always-`false` flag until the composition root shares one with the
    /// adapter.
    pub(crate) fast_mode: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Finding #80: once-per-session latch for the refusal→fallback-model swap
    /// (claude-code's `refusalFallbackModelLatch`). Set the first time a turn's
    /// response arrives with `stop_reason == "refusal"` AND
    /// [`OrchestratorConfig::refusal_fallback_model`] is `Some`, after the session
    /// model is swapped to the fallback. Once `true`, a subsequent `refusal`
    /// keeps today's terminal behavior (no re-swap), so the fallback fires at
    /// most ONCE per session — matching the binary, where the latch makes the
    /// `mainLoopModel` override sticky.
    pub(crate) refusal_fallback_latched: std::sync::atomic::AtomicBool,
    /// Cost tracker wired by [`Self::with_cost_tracker`] (M6-06). `None`
    /// when not configured — `snapshot_cost` then falls back to the M5-10
    /// zero-shaped stub. The CLI binary (M6-06 init.rs) always populates
    /// this so production `lingxi-cli` reports real cost; library callers
    /// (e.g. unit tests) may leave it `None`.
    pub(crate) cost_tracker: Option<Arc<cost::CostTracker>>,
    /// Optional analytics bus wired by [`Self::with_analytics_bus`] (M7). When
    /// present (desktop composition root), the live turn loop fires
    /// `tengu_api_success` per completed API response — 1:1 with claude-code
    /// 2.1.195's `logEvent('tengu_api_success', …)`. `None` for library/test
    /// callers, which then silently skip the emission (the tracker still accrues
    /// totals).
    pub(crate) analytics_bus: Option<Arc<telemetry::AnalyticsBus>>,
    /// Monotonic timestamp captured at orchestrator construction. Used by
    /// `snapshot_cost` to compute the `session_duration` field of the
    /// returned [`traits::CostSnapshot`]. Stored as `std::time::Instant`
    /// (not `tokio::time::Instant`) so the orchestrator can be constructed
    /// outside a tokio runtime if needed.
    pub(crate) session_started_at: std::time::Instant,
    /// Count of API responses successfully recorded into `cost_tracker`.
    /// Used to populate `CostSnapshot::api_calls`. Lives on the orchestrator
    /// (rather than `cost::CostState`) because `cost::Usage`
    /// does not carry a per-call counter; `ModelUsage::usage.add()` merges
    /// the token totals but not "how many times we recorded".
    pub(crate) api_calls_recorded: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// `tengu_api_success` `timeSinceLastApiCallMs:W` source: ms-since-session-start
    /// of the PREVIOUS successful API call (claude's module-level `G`). `-1` until
    /// the first call. Shared by the streaming + non-streaming emit sites.
    pub(crate) last_api_call_at_ms: std::sync::Arc<std::sync::atomic::AtomicI64>,
    /// MCP registry (M2-02b). `None` when not wired — `list_mcp_servers`
    /// then returns `vec![]`. The CLI binary (M6-07 init.rs) populates
    /// this from `.mcp.json` + `~/.config/lingxi/mcp.json`.
    pub(crate) mcp_registry: Option<Arc<mcp::McpRegistry>>,
    /// Hook registry (M5-06). `None` when not wired — `list_hooks` then
    /// returns `vec![]`. The CLI binary populates from settings + plugin
    /// sources at startup.
    pub(crate) hook_registry: Option<Arc<tokio::sync::RwLock<hooks::HookRegistry>>>,
    /// Subagent catalog (M6-07). `None` when not wired — `list_agents`
    /// then returns `vec![]`. The CLI binary populates from
    /// `~/.lingxi/agents/` + project `.lingxi/agents/`.
    pub(crate) agent_catalog: Option<Arc<tokio::sync::RwLock<Vec<agent::AgentDefinition>>>>,
    /// Compaction engine (M3-05) wired by `with_compaction`. `None` when
    /// not configured — `force_compact` then falls back to the legacy
    /// no-op semantics. The CLI binary (M6-08 init.rs) always populates
    /// this. (M6-08)
    pub(crate) compaction: Option<Arc<compaction::CompactionOrchestrator>>,
    /// Per-conversation autocompact circuit-breaker tracking (In-Loop
    /// Compaction Batch 4). Threaded into
    /// [`compaction::CompactionOrchestrator::process_iteration_tracked`] by
    /// the proactive pre-call trigger (`maybe_compact_before_call`) and the
    /// reactive 413 fallback so the consecutive-failure circuit breaker
    /// (`autoCompact.ts:260-265`) survives across turns. Mirrors the
    /// `autoCompactTracking` object TS threads through `autoCompactIfNeeded`
    /// (`autoCompact.ts:241-351`). A `tokio::sync::Mutex` because the trigger
    /// fires inside `async` turn drivers; uncontended in practice (only the
    /// in-flight turn touches it). Default = zero consecutive failures.
    pub(crate) compaction_tracking: Mutex<compaction::AutoCompactTrackingState>,
    /// The last API response's total *input* token count
    /// (`input_tokens + cache_read_input_tokens + cache_creation_input_tokens`),
    /// recorded by both turn drivers after every successful call. This is the
    /// Rust seam for claude-code's `Xtt(messages)` last-usage snapshot
    /// (`bin/claude.exe` offset 197212071): the fixed-prefix overflow guard
    /// `a3p` (`compaction::compaction_prefix_overflow`) needs the immovable
    /// prefix = `totalInput − messagesEstimate`, and `protocol` carries no
    /// per-message `usage` object to recover it from, so the orchestrator caches
    /// the most recent call's input total here. `0` until the first successful
    /// call — the prefix guard is then a strict no-op (prefix `0 ≤ threshold`).
    pub(crate) last_response_input_tokens: std::sync::atomic::AtomicU64,
    /// Shared output-token pool backing a workflow script's `budget.spent()`.
    /// Every successful main-loop API response adds its output tokens here (in
    /// [`Self::record_response_input_tokens`]); a launched `LocalWorkflowHandler`
    /// is handed this same `Arc` so its subagents add theirs too. `budget.spent()`
    /// then reads the union — the main loop plus every workflow — matching
    /// claude-code's shared per-turn pool (cumulative over the session; exact for
    /// the dominant single-directive case).
    pub(crate) output_token_pool: Arc<std::sync::atomic::AtomicU64>,
    /// Turn-start output baseline (claude-code `xtr`, set by `UAc(e)` each turn):
    /// the cumulative [`Self::output_token_pool`] value captured at the START of
    /// the current turn. A launched workflow's `budget.spent()` reads
    /// `pool - baseline` = `getTurnSpent()` (output spent THIS turn), so prior
    /// turns' output is excluded. Updated by each turn driver as its per-turn
    /// token counter resets to 0.
    pub(crate) turn_start_output_baseline: Arc<std::sync::atomic::AtomicU64>,
    /// Shared cache-safe prompt-prefix slot (In-Loop Compaction Batch 6). When
    /// wired (via [`Self::with_cache_safe_slot`]), the turn drivers write a
    /// [`sidequery::CacheSafeParams`] snapshot after every successful API call
    /// so the forked autocompact summarizer can replay the parent's prefix
    /// verbatim and hit Anthropic's prompt cache (TS `cacheSafeParams`,
    /// `autoCompact.ts:241-326`). `None` in tests and any binary that has not
    /// wired the forked runner — then [`Self::save_cache_safe_params`] is a
    /// strict no-op. The same `Arc` is handed to
    /// [`compaction::Autocompactor::with_forked_runner`] at the composition root
    /// so producer (here) and consumer (the summarizer) share one slot.
    pub(crate) cache_safe_slot: Option<Arc<sidequery::CacheSafeParamsSlot>>,
    /// Passive `<new-diagnostics>` source (the LSP diagnostic registry). When
    /// wired (via [`Self::with_new_diagnostics_source`]), each turn polls it for
    /// LSP diagnostics not yet surfaced and injects them as a transient meta
    /// user message (claude-code's `formatDiagnosticsBlock` flow). `None` when
    /// no LSP servers are configured (the common case) ⇒ no reminder.
    pub(crate) new_diagnostics_source: Option<Arc<dyn traits::NewDiagnosticsSource>>,
    /// FORK (codex #5 follow-up): the rendered system-prompt bytes the current
    /// turn handed the model, recorded by the turn driver after a successful API
    /// call so a fork-subagent spawn dispatched LATER in the same turn can thread
    /// the exact bytes onto its child (cache-identical prefix, claude
    /// `AgentTool.tsx:622-623` `override.systemPrompt = forkParentSystemPrompt`).
    /// `None` until the first successful turn / when the turn ran with no system
    /// prompt. Read by the fork dispatch path ONLY; no non-fork tool touches it.
    pub(crate) current_turn_system_prompt: Mutex<Option<String>>,
    /// `/fork` background-agent spawner. When wired (via
    /// [`Self::with_fork_spawner`], the composition root's
    /// `BackgroundAgentSpawner`), [`OrchestratorHandle::fork_conversation`]
    /// spawns a detached background agent that inherits the conversation.
    /// `None` (tests / non-desktop roots) ⇒ `fork_conversation` fails with a
    /// clear `ActionFailed` rather than panicking. Mirrors the existing
    /// `with_compaction` / `with_cache_safe_slot` Option-field pattern.
    pub(crate) fork_spawner: Option<Arc<dyn traits::subagent_spawn::SubagentSpawner>>,
    /// `/fork` budget enforcer inherited by the spawned background agent
    /// (`SubagentInheritance::budget`). Wired via [`Self::with_fork_budget`].
    /// `None` ⇒ `fork_conversation` fails gracefully.
    pub(crate) fork_budget: Option<Arc<dyn traits::budget::BudgetEnforcerHandle>>,
    /// `/recap` side-query runner — the SAME single-turn
    /// [`sidequery::ForkedAgentRunner`] the autocompact summarizer uses (cloned
    /// from the composition root's `forked_runner` before it moves into the
    /// `Autocompactor`), so recap replays the same cache-safe prefix. Wired via
    /// [`Self::with_recap_runner`]. `None` ⇒ [`OrchestratorHandle::generate_recap`]
    /// fails gracefully. Read-only: recap NEVER writes history/slot (skipTranscript
    /// / skipCacheWrite), unlike `force_compact`.
    pub(crate) recap_runner: Option<Arc<sidequery::ForkedAgentRunner>>,
    /// (`/rewind`) Shared file-history checkpoint store. The SAME
    /// `Arc<session::FileHistory>` the CLI holds (for restore + picker rows). The
    /// turn loop calls `make_snapshot` once per user turn and hands each tool a
    /// `FileHistorySink` view via `ToolUseContext.file_history` so pre-edit
    /// content is backed up. `None` ⇒ no checkpointing (edits untracked).
    pub(crate) file_history: Option<Arc<session::FileHistory>>,
    /// Forced permission decisions keyed by `tool_use_id`, consulted ONCE
    /// (removed on read) by the permission gate in
    /// [`crate::turn_loop::dispatch_tool_uses_tracked`]. Populated transiently by
    /// [`Self::run_orphaned_permission`] right before it re-dispatches an
    /// orphaned tool so the recovered `control_response` decision REPLACES the
    /// interactive gate — twin of claude-code's forced `canUseTool` in
    /// `handleOrphanedPermission` (`queryHelpers.ts:278-284`). Empty on every
    /// normal turn → the gate's behaviour (and the byte-locked turn-loop
    /// fixtures) are unchanged.
    pub(crate) orphan_forced_decisions: Mutex<
        std::collections::HashMap<protocol::ToolUseId, crate::test_support::PermissionDecision>,
    >,
    /// Read-file-state cache backing `/files` (TS `context.readFileState`).
    /// The dispatch loop (`turn_loop::dispatch_tool_uses`) inserts the
    /// absolutized `file_path` of every successful
    /// `Read`/`Edit`/`Write`/`MultiEdit`/`NotebookEdit`, and
    /// [`Self::files_in_context`] returns the keys in insertion order. TS
    /// uses an LRU `FileStateCache` keyed by `normalize(expandPath(file_path))`;
    /// this port only needs the key set for `/files`, so it stores the
    /// absolutized paths in a `Vec`, dedup keeping the FIRST insertion.
    ///
    /// FORCED divergences from the TS LRU (documented, not parity gaps):
    /// - Ordering: TS `keys()` iterates MRU→LRU and a re-`set` promotes the
    ///   key, so re-reading reorders the *middle* of the list (read a,b,c,a →
    ///   TS `[a, c, b]`); this `Vec` keeps first-insertion order (`[a, b, c]`).
    ///   The 2-file re-read case (`a,b,a`) coincides at `[a, b]`. Only the
    ///   display order of re-read files differs — never the set itself.
    /// - The 100-entry LRU eviction is intentionally not reproduced (MVP).
    pub(crate) read_file_state: Arc<Mutex<Vec<std::path::PathBuf>>>,
    /// Richer per-path read-state registry (`path → {content, mtime_ms,
    /// offset, limit}`) — the 1:1 port of claude-code's `readFileState` map
    /// (`FileReadTool.ts:1032`). Kept SEPARATE from the `read_file_state`
    /// `Vec` above, which preserves the existing `/files` ordering semantics.
    /// The orchestrator shares this `Arc` with the file tools' construction-
    /// time `BuiltinToolContext` so a tool's `readFileState.set` is visible
    /// here (and to the future staleness guards / Read dedup).
    ///
    /// CONSUMED post-compact (#59): [`Self::restore_post_compact_attachments`]
    /// snapshots this registry, clears it, and re-attaches the most-recent files
    /// after the compaction boundary (`K2p`/`Pqn`). The composition root shares
    /// this `Arc` into the file tools' `BuiltinToolContext`. The staleness guards
    /// (D/E/F) and Read dedup (A) are later additional consumers.
    pub(crate) read_state_map: tool_api::read_file_state::ReadFileStateMap,
    /// Task 8 (llm-client future-work batch 3): the last rate-limit snapshot
    /// forwarded to [`traits::OutputStream::emit_rate_limit`], for the
    /// emit-on-change dedup in [`Self::emit_rate_limit_if_changed`]. Lives on
    /// the orchestrator (not per-turn loop state) so the dedup spans turns —
    /// an identical snapshot across two `run_turn` calls emits exactly once.
    /// `None` until the first emission.
    pub(crate) last_emitted_rate_limit: Mutex<Option<crate::model::rate_limit::RateLimitInfo>>,
    /// Task 2 (llm-client future-work batch 5): the last RAW per-window
    /// utilization snapshot forwarded to
    /// [`traits::OutputStream::emit_raw_utilization`], for the
    /// emit-on-change dedup in [`Self::emit_raw_utilization_if_changed`].
    /// Same lifetime/placement rationale as
    /// [`Self::last_emitted_rate_limit`]: lives on the orchestrator so the
    /// dedup spans turns. `None` until the first emission.
    pub(crate) last_emitted_raw_utilization:
        Mutex<Option<crate::model::rate_limit::RawUtilization>>,
    /// SKILLLIST.1: supplies the model-invocable skill entries for the per-turn
    /// `skill_listing` reminder (TS `getSkillToolCommands` →
    /// `getSkillListingAttachments`). `None` when not wired (every test + any
    /// binary without a `CommandRegistry`) — then
    /// [`Self::skill_listing_reminder_message`] is a strict no-op, keeping the
    /// locked turn-loop/streaming fixtures byte-identical. The desktop binary
    /// wires a `CommandRegistry`-backed provider at the composition root.
    pub(crate) skill_listing: Option<Arc<dyn crate::prompt::skill_listing::SkillListingProvider>>,
    /// Source of completed background (`async`) hook responses to fold back into
    /// the next turn (claude-code `getAsyncHookResponseAttachments`). `None` ⇒
    /// [`Self::async_hook_response_reminder_message`] is a strict no-op (the
    /// default — keeps fixtures byte-identical). Wired at the desktop
    /// composition root from the `AsyncHookRegistry` completion channel.
    pub(crate) async_hook_responses:
        Option<Arc<dyn crate::prompt::async_hook_response::AsyncHookResponseProvider>>,
    /// T35: source of terminal background tasks (a backgrounded `local_bash` /
    /// `local_agent` / MCP `monitor` …) finished since the last turn, folded back
    /// into the next turn as a `<task-notification>` reminder so the model learns
    /// its async task completed (claude-code's per-task-type `enqueue*Notification`).
    /// `None` ⇒ [`Self::task_notification_reminder_message`] is a strict no-op (the
    /// default — keeps fixtures byte-identical). Wired at the desktop composition
    /// root from the `TaskRegistry`.
    pub(crate) task_notifications:
        Option<Arc<dyn crate::prompt::task_notification::TaskNotificationProvider>>,
    /// Source of the `Stop` / `SubagentStop` hook `background_tasks` +
    /// `session_crons` snapshot (claude-code `Lic(taskRegistry.all())` /
    /// `Mic()`). Consulted ONLY at the `Stop` / `SubagentStop` firings (claude's
    /// `s` = tool-use-context gate), so other lifecycle hooks omit both keys.
    /// `None` ⇒ both fields stay `None` and the keys are omitted — keeping
    /// no-registry builds byte-identical. Wired at the desktop composition root
    /// from the live `TaskRegistry` + cron file.
    pub(crate) stop_hook_snapshot:
        Option<Arc<dyn crate::stop_hook_snapshot::StopHookSnapshotProvider>>,
    /// Mid-turn drain seam: source of queued user input to inject WITHIN a
    /// running streaming turn (claude-code's query.ts mid-turn injection,
    /// ~1570-1580). Empty ⇒ the streaming loop's mid-turn drain is a strict
    /// no-op (the default — keeps the locked fixtures byte-identical). Wired at
    /// the composition root from the `MessageQueueManager` via a msgqueue-backed
    /// adapter, so the orchestrator keeps NO dependency on `msgqueue`. A
    /// [`std::sync::OnceLock`] so it can be set on `&self` AFTER the orchestrator
    /// is shared as an `Arc` (the bridge wires it post-build with its
    /// per-connection queue). See
    /// [`crate::prompt::mid_turn_input::MidTurnInputSource`].
    pub(crate) mid_turn_input:
        std::sync::OnceLock<Arc<dyn crate::prompt::mid_turn_input::MidTurnInputSource>>,
    /// Abort-reason flag shared with the queue adapter so the streaming loop can
    /// distinguish a `Now`-command abort from a user Ctrl+C/ESC interrupt at the
    /// cancel-check points. Unset ⇒ every abort is treated as a user interrupt
    /// (today's behavior). Wired alongside [`Self::mid_turn_input`] at the
    /// composition root. See [`crate::prompt::mid_turn_input::CancelReasonFlag`].
    pub(crate) cancel_reason: std::sync::OnceLock<crate::prompt::mid_turn_input::CancelReasonFlag>,
    /// Finding #73: source of the V2 task list for the per-turn `task_reminder`
    /// (the binary's `B4p` reading `p9(KF())`). `None` ⇒ the V2 reminder renders
    /// with its base text only (no items appended), matching an empty store.
    /// The V1 (`todo_reminder`) path needs no provider — it reads
    /// `session.todos` directly. Wired at the desktop composition root from the
    /// `tool_task::todo_store::TodoStore`.
    pub(crate) todo_reminder_tasks:
        Option<Arc<dyn crate::prompt::todo_reminder::TodoReminderTaskProvider>>,
    /// §F: cache of the CONDITIONAL (`paths:`-gated) memory rules, populated the
    /// first time [`Self::conditional_rules_reminder_message`] runs (a `OnceCell`
    /// fill via the same `memory.load(&cwd)` the system prompt uses, then
    /// re-filtered to `globs.is_some()`). Avoids re-walking disk every turn while
    /// still letting lazy activation re-test the cached rules against the latest
    /// `read_file_state`. Empty when the hierarchy has no conditional rules.
    pub(crate) conditional_rules_cache: tokio::sync::OnceCell<Vec<crate::prompt::MemoryFile>>,
    /// §F sent-tracking ("delta"): the paths of conditional rules already
    /// injected this session, so each rule is rendered ONCE when first activated
    /// and never re-injected on later turns. 1:1 with TS `loadedNestedMemoryPaths`
    /// (attachments.ts:1722-1732 — a non-evicting Set keyed by rule path).
    pub(crate) sent_conditional_rules: Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// SKILLLIST.1 delta: skill names already emitted in a prior turn's
    /// `skill_listing` reminder. Turn-0 emits the FULL listing; later turns emit
    /// ONLY newly-appeared skills (mirrors TS `sentSkillNames` per-agent delta,
    /// attachments.ts:2607/2699). When no new skill appears,
    /// [`Self::skill_listing_reminder_message`] returns `None` (no reminder that
    /// turn). Process-/session-local, exactly like the TS module-scope map.
    pub(crate) sent_skill_names: Mutex<std::collections::HashSet<String>>,
    /// `agent_listing_delta` delta: agent TYPES already announced in a prior
    /// turn's `agent_listing` reminder. Turn-0 (empty set) emits the FULL
    /// listing with the "Available agent types for the Agent tool:" header;
    /// later turns emit ONLY newly-added types with the "New agent types are now
    /// available…" header. 1:1 with TS's transcript-reconstructed `announced`
    /// set (attachments.ts:1524-1530) — kept in memory here (like
    /// [`Self::sent_skill_names`]) rather than rebuilt from prior deltas. When no
    /// new type appears, [`Self::agent_listing_reminder_message`] returns `None`.
    /// Only consulted when the gate (`LINGXI_AGENT_LIST_IN_MESSAGES`) is ON;
    /// inert (never read) in the default OFF build.
    pub(crate) sent_agent_names: Mutex<std::collections::HashSet<String>>,
    /// P0.1: the memory-selector prefetcher, fired at turn start to score +
    /// rank the available memdir set CONCURRENTLY with the main API call (1:1
    /// with claude-code's `tengu_memdir_prefetch_collected` side-channel,
    /// `wAo`/`Y$p`). `None` when no prefetch is wired (every test + any binary
    /// without a memory selector) — then [`Self::start_memory_prefetch`] +
    /// [`Self::relevant_memory_reminder_message`] are strict no-ops, keeping the
    /// surfacing channel inert and the ~4000 locked fixtures byte-identical. The
    /// LingXi gate is purely `memory_prefetch.is_some()` at the composition root
    /// (no new env flag), mirroring claude-code's `tengu_moth_copse`-default-false
    /// gate. Wired (when a real selector lands) via [`Self::with_memory_prefetch`].
    pub(crate) memory_prefetch: Option<Arc<memory::prefetch::MemoryPrefetch>>,
    /// EndConversation (2.1.206) end-request slot, shared with the
    /// [`crate::end_conversation_tool::EndConversationTool`]: raised by the
    /// tool's 2nd consecutive call, read (and consumed) by the turn loop after
    /// tool execution to terminate the conversation. `None` when the feature is
    /// disabled (default) → the turn loop never checks it → byte-identical.
    pub(crate) end_conversation_slot: Option<crate::end_conversation_tool::EndConversationSlot>,
    /// P0.1 per-turn slot holding the in-flight prefetch handle armed by
    /// [`Self::start_memory_prefetch`] at turn start and consumed by
    /// [`Self::relevant_memory_reminder_message`] before snapshot assembly.
    /// `None` between turns / when no prefetch is wired. Mirrors the
    /// pending-handle slot pattern of the recovery / cache-safe slots.
    pub(crate) pending_memory_prefetch: Mutex<Option<memory::prefetch::PendingMemoryPrefetch>>,
    /// P0.1 surfacing dedup: paths already surfaced via the
    /// `relevant_memories` channel this session, so a memory surfaced once is
    /// never re-injected on a later turn. Mirrors [`Self::sent_conditional_rules`]
    /// (TS `loadedNestedMemoryPaths` / the prefetch's per-iteration consume
    /// guard). Distinct from [`Self::read_file_state`], which the SHARED dedup
    /// also consults so a file already loaded as a nested/conditional attachment
    /// (P3.2) is never double-injected here.
    pub(crate) surfaced_memory_paths: Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// EXPERIMENTAL_SKILL_SEARCH: the skill-discovery prefetcher, fired at turn
    /// start to select skills relevant to the turn query CONCURRENTLY with the
    /// main API call + tool execution (1:1 with claude-code's
    /// `startSkillDiscoveryPrefetch`, bundle fn `C1z`:
    /// `B=at1?.startSkillDiscoveryPrefetch(null,V,T)`). `None` when no prefetch is
    /// wired — then [`Self::start_skill_discovery_prefetch`] +
    /// [`Self::skill_discovery_reminder_message`] are strict no-ops, keeping the
    /// channel inert and the locked fixtures byte-identical. The LingXi gate is
    /// purely `skill_discovery_prefetch.is_some()` at the composition root,
    /// mirroring claude-code's `feature('EXPERIMENTAL_SKILL_SEARCH')` (default
    /// false). Wired (flag ON) via [`Self::with_skill_discovery_prefetch`].
    pub(crate) skill_discovery_prefetch: Option<Arc<skill_api::SkillDiscoveryPrefetch>>,
    /// EXPERIMENTAL_SKILL_SEARCH per-turn slot holding the in-flight prefetch
    /// handle armed by [`Self::start_skill_discovery_prefetch`] at turn start and
    /// consumed by [`Self::skill_discovery_reminder_message`] before snapshot
    /// assembly. `None` between turns / when no prefetch is wired. Mirrors
    /// [`Self::pending_memory_prefetch`].
    pub(crate) pending_skill_prefetch: Mutex<Option<skill_api::PendingSkillDiscoveryPrefetch>>,
    /// EXPERIMENTAL_SKILL_SEARCH surfacing dedup: skill NAMES already surfaced via
    /// the `skill_discovery` channel this session, so a skill surfaced once is
    /// never re-injected on a later turn. Analog of [`Self::sent_skill_names`] /
    /// [`Self::surfaced_memory_paths`], keyed on `name` (skill names are not
    /// files, so this does NOT consult `read_file_state`).
    pub(crate) surfaced_skill_names: Mutex<std::collections::HashSet<String>>,
    /// P1 session-memory standalone trigger (§6.5). `None` = inert (no caller
    /// wires it). When wired (via [`Self::with_session_memory`]) AND the
    /// extractor's threshold is crossed, [`Self::maybe_extract_session_memory`]
    /// background-forks a distillation at turn start and writes the per-session
    /// memory file the next session re-loads through the Session-tier memdir scan.
    pub(crate) session_memory: Option<Arc<SessionMemoryHandle>>,
    /// Best-effort startup Responses WebSocket prewarm task.
    ///
    /// Lifecycle operations abort this before clearing or exiting the session so
    /// a stale prewarm cannot later seed `previous_response_id`.
    pub(crate) startup_responses_websocket_prewarm:
        std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Everything [`ConversationOrchestrator::maybe_extract_session_memory`] needs to
/// run a standalone session-memory extraction (§6.5): the threshold-stateful
/// extractor (behind a `Mutex` — `extract` advances its watermark), the forked
/// runner that issues the distillation, the resolved config-home for the write
/// path, and a runtime to background-spawn the fork so it never blocks a turn.
pub struct SessionMemoryHandle {
    /// The threshold-gated extractor; `Mutex` because `extract` is `&mut`.
    pub extractor: Mutex<memory::session_memory::SessionMemoryExtractor>,
    /// Forked-agent runner that issues the distillation off the cache prefix.
    pub runner: Arc<sidequery::ForkedAgentRunner>,
    /// Resolved `$LINGXI_CONFIG_DIR ?? ~/.claude` dir (the write base).
    pub config_home: std::path::PathBuf,
    /// Runtime used to background-spawn the extraction fork.
    pub runtime: Arc<dyn traits::RuntimeSpawner>,
}

/// Find the assistant message carrying a `tool_use` with `tool_use_id` that has
/// NO matching `tool_result` anywhere in `history`, returning a CLONE of that
/// assistant message. 1:1 with claude-code's `findUnresolvedToolUse`
/// (`sessionStorage.ts:4478-4519`): a `tool_result` for the same id (in any user
/// message) means the call already resolved → `None`. The full message is
/// returned (not just the matched block) so the caller can re-emit it as a
/// stream frame, mirroring the TS `yield sdkAssistantMessage`. Used by
/// [`ConversationOrchestrator::run_orphaned_permission`].
fn find_unresolved_tool_use_in_history(
    history: &[protocol::ConversationMessage],
    tool_use_id: &protocol::ToolUseId,
) -> Option<protocol::ConversationMessage> {
    use protocol::{ContentBlock, ConversationMessage};
    // A matching `tool_result` anywhere ⇒ already resolved (bail, like the TS
    // early `return null` when a tool_result block is found).
    let resolved = history.iter().any(|m| match m {
        ConversationMessage::User { content, .. } => content.iter().any(
            |b| matches!(b, ContentBlock::ToolResult { tool_use_id: id, .. } if id == tool_use_id),
        ),
        _ => false,
    });
    if resolved {
        return None;
    }
    history
        .iter()
        .find(|m| match m {
            ConversationMessage::Assistant { content, .. } => content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == tool_use_id)),
            _ => false,
        })
        .cloned()
}

impl ConversationOrchestrator {
    /// `tengu_api_success` `timeSinceLastApiCallMs`: ms since the previous
    /// successful API call, then record this call's timestamp. Returns `None`
    /// on the first call (claude `W=G!==null?Math.max(0,Math.round(M-G)):void 0`).
    #[allow(clippy::cast_sign_loss)]
    pub(crate) fn record_api_call_gap_ms(&self) -> Option<u64> {
        use std::sync::atomic::Ordering;
        let now_ms =
            i64::try_from(self.session_started_at.elapsed().as_millis()).unwrap_or(i64::MAX);
        let prev = self.last_api_call_at_ms.swap(now_ms, Ordering::SeqCst);
        // `prev < 0` is the `-1` sentinel = no prior call → OMIT the field.
        (prev >= 0).then(|| (now_ms - prev).max(0) as u64)
    }

    /// Construct a new orchestrator with a fresh in-memory session and
    /// BOTH batched + streaming API clients wired.
    ///
    /// Argument order: same as `new`, but inserts `streaming_api` right
    /// after `api`. Use this when the streaming path is needed
    /// (`run_turn_streaming`); otherwise `new` is shorter.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new_with_streaming(
        config: OrchestratorConfig,
        api: Arc<dyn OrchestratorApiClient>,
        streaming_api: Arc<dyn StreamingApiClient>,
        tools: Arc<ToolRegistry>,
        hooks: Arc<HookExecutor>,
        perms: Arc<dyn PermissionGate>,
        output: Arc<dyn OutputStream>,
        memory: Arc<dyn crate::prompt::MemoryHierarchyProvider>,
        cwd: std::path::PathBuf,
    ) -> Self {
        // M5-14 Task 10: emit release markers once per process lifetime.
        telemetry::emit_release_markers_once();
        // Publish the session interactivity to the process-global flag (the port's
        // `getIsNonInteractiveSession()` analog) so prompt builders without a
        // `ToolUseContext` — e.g. the `AgentTool` fork gate — see the right mode.
        // `is_non_interactive_session == !interactive_permissions` (turn_loop's
        // own derivation).
        traits::session_flags::set_non_interactive_session(!config.interactive_permissions);
        let session = SessionState::empty(SessionId::new(), config.model.clone());
        Self {
            config,
            api,
            streaming_api,
            query_chain_id: uuid::Uuid::new_v4().to_string(),
            tools,
            hooks,
            perms,
            output,
            session: Arc::new(Mutex::new(session)),
            memory,
            current_cwd: Arc::new(std::sync::Mutex::new(cwd.clone())),
            cwd,
            config_home: None,
            jsonl_writer: None,
            last_jsonl_uuid: Mutex::new(None),
            git_branch_cache: Mutex::new(None),
            current_prompt_id: Mutex::new(None),
            should_exit: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            fast_mode: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            refusal_fallback_latched: std::sync::atomic::AtomicBool::new(false),
            cost_tracker: None,
            analytics_bus: None,
            session_started_at: std::time::Instant::now(),
            api_calls_recorded: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            last_api_call_at_ms: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(-1)),
            mcp_registry: None,
            hook_registry: None,
            agent_catalog: None,
            compaction: None,
            compaction_tracking: Mutex::new(compaction::AutoCompactTrackingState::default()),
            last_response_input_tokens: std::sync::atomic::AtomicU64::new(0),
            output_token_pool: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            turn_start_output_baseline: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache_safe_slot: None,
            new_diagnostics_source: None,
            current_turn_system_prompt: Mutex::new(None),
            fork_spawner: None,
            fork_budget: None,
            recap_runner: None,
            file_history: None,
            orphan_forced_decisions: Mutex::new(std::collections::HashMap::new()),
            read_file_state: Arc::new(Mutex::new(Vec::new())),
            read_state_map: tool_api::read_file_state::new_read_file_state_map(),
            last_emitted_rate_limit: Mutex::new(None),
            last_emitted_raw_utilization: Mutex::new(None),
            skill_listing: None,
            async_hook_responses: None,
            task_notifications: None,
            stop_hook_snapshot: None,
            mid_turn_input: std::sync::OnceLock::new(),
            cancel_reason: std::sync::OnceLock::new(),
            todo_reminder_tasks: None,
            conditional_rules_cache: tokio::sync::OnceCell::new(),
            sent_conditional_rules: Mutex::new(std::collections::HashSet::new()),
            sent_skill_names: Mutex::new(std::collections::HashSet::new()),
            sent_agent_names: Mutex::new(std::collections::HashSet::new()),
            memory_prefetch: None,
            end_conversation_slot: None,
            pending_memory_prefetch: Mutex::new(None),
            surfaced_memory_paths: Mutex::new(std::collections::HashSet::new()),
            skill_discovery_prefetch: None,
            pending_skill_prefetch: Mutex::new(None),
            surfaced_skill_names: Mutex::new(std::collections::HashSet::new()),
            session_memory: None,
            startup_responses_websocket_prewarm: std::sync::Mutex::new(None),
        }
    }

    /// Attach a [`JsonlWriter`] for byte-equivalent session persistence.
    /// Builder-style — used by the CLI binary (M5-12) and integration tests.
    #[must_use]
    pub fn with_jsonl_writer(mut self, writer: Arc<JsonlWriter>) -> Self {
        self.jsonl_writer = Some(writer);
        self
    }

    /// Whether a [`JsonlWriter`] has been wired via [`Self::with_jsonl_writer`].
    /// (M3 cc2.1.198) Probe for the `--no-session-persistence` boot gate: the
    /// composition root leaves the slot `None` so nothing persists to disk.
    #[must_use]
    pub fn has_jsonl_writer(&self) -> bool {
        self.jsonl_writer.is_some()
    }

    /// Seed the JSONL parent-uuid chain pointer so the FIRST append after a
    /// resume chains via `parent_uuid` off the resumed transcript's tail
    /// (matching the M5-07 writer's chain semantics). Used by the CLI's
    /// resume-into-TUI seed alongside adopting the resumed history + id; without
    /// it the first appended message would be a chain orphan (recoverable, but
    /// this keeps the on-disk chain linear).
    pub async fn seed_last_jsonl_uuid(&self, last_uuid: Option<String>) {
        *self.last_jsonl_uuid.lock().await = last_uuid;
    }

    /// Attach the resolved claude-home (`$LINGXI_CONFIG_DIR ?? ~/.claude`) so
    /// hook payloads carry a deterministically-computed `transcript_path` even
    /// when no [`JsonlWriter`] is wired (the production case). Builder-style —
    /// wired at the composition root (`engine-desktop` / `engine-mobile`).
    #[must_use]
    pub fn with_config_home(mut self, config_home: std::path::PathBuf) -> Self {
        self.config_home = Some(config_home);
        self
    }

    /// Share the composition root's mutable-cwd cell — the SAME `Arc` the
    /// [`crate::OrchestratorCwdChangedFirer`] writes on a Bash `cd` — so hook
    /// payloads read the post-`cd` directory. Builder-style; wired at the
    /// desktop composition root. Without it the default private cell (over the
    /// static `cwd`) never moves, so hooks read the init cwd exactly as before.
    #[must_use]
    pub fn with_current_cwd(mut self, cell: Arc<std::sync::Mutex<std::path::PathBuf>>) -> Self {
        self.current_cwd = cell;
        self
    }

    /// The CURRENT working directory for hook payloads (the post-`cd` shell cwd
    /// when a firer is wired, else the static init `cwd`). Clones out of the
    /// shared cell so no lock is held across an await; a poisoned lock falls
    /// back to the static `cwd`.
    pub(crate) fn current_cwd(&self) -> std::path::PathBuf {
        self.current_cwd
            .lock()
            .map_or_else(|_| self.cwd.clone(), |g| g.clone())
    }

    /// Override the orchestrator's session id. Builder-style — used at the
    /// composition root so the boot-canonical session id (also handed to the
    /// leaf hook firers / subagent spawner for a consistent `transcript_path`)
    /// matches the orchestrator's live session. Without this the constructor's
    /// fresh [`SessionId::new`] stands (test/library callers).
    #[must_use]
    pub fn with_session_id(self, session_id: SessionId) -> Self {
        // `session` is behind an `Arc<Mutex>`; this builder runs before any turn
        // (single owner at construction), so a blocking lock is safe and avoids
        // making the builder async.
        {
            let mut s = self
                .session
                .try_lock()
                .expect("with_session_id runs at construction, before any turn holds the lock");
            s.session_id = session_id;
        }
        self
    }

    /// Deterministically derive this session's transcript path from the resolved
    /// claude-home + cwd + session id — the parity analog of claude-code's
    /// `getTranscriptPathForSession(sessionId)` (`utils/sessionStorage.ts:207`),
    /// which `createBaseHookInput` (`utils/hooks.ts:322`) ALWAYS stamps onto every
    /// hook payload. Shape: `<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`
    /// (= `session::jsonl::path::session_path`). The id is formatted as the BARE
    /// uuid (`SessionId::as_uuid`) to match claude-code's `${sessionId}.jsonl` and
    /// the on-disk JSONL filename the writer/loader use — NOT the `sess:`-prefixed
    /// [`std::fmt::Display`] form. Returns an empty path when no `config_home` is
    /// wired (test/library builds that also wire no writer), preserving the prior
    /// `""` hook field for those.
    #[must_use]
    pub(crate) fn computed_transcript_path(&self, session_id: &SessionId) -> std::path::PathBuf {
        match &self.config_home {
            Some(home) => session::jsonl::path::session_path(
                home,
                &self.cwd.to_string_lossy(),
                &session_id.as_uuid().to_string(),
            ),
            None => std::path::PathBuf::new(),
        }
    }

    /// Attach a [`cost::CostTracker`] so `snapshot_cost` returns
    /// real numbers. Without this, `snapshot_cost` keeps the M5-10
    /// zero-shaped stub shape. (M6-06)
    #[must_use]
    pub fn with_cost_tracker(mut self, tracker: Arc<cost::CostTracker>) -> Self {
        self.cost_tracker = Some(tracker);
        self
    }

    /// Whether a [`cost::CostTracker`] has been wired via
    /// [`Self::with_cost_tracker`]. (M6-06)
    #[must_use]
    pub fn has_cost_tracker(&self) -> bool {
        self.cost_tracker.is_some()
    }

    /// Attach an analytics bus so the live turn loop fires
    /// `tengu_api_success` per completed API response (M7). Without this the
    /// cost tracker still accrues totals but emits no analytics event — matching
    /// the pre-M7 behavior (and library/test callers that don't want telemetry).
    /// The desktop composition root passes the SAME bus it gives the provider
    /// adapter (which fires `tengu_api_*`), so all telemetry shares one sink set.
    #[must_use]
    pub fn with_analytics_bus(mut self, bus: Arc<telemetry::AnalyticsBus>) -> Self {
        self.analytics_bus = Some(bus);
        self
    }

    /// Attach an MCP registry so `list_mcp_servers` reports real data.
    /// Without this, the trait method returns `vec![]`. (M6-07)
    #[must_use]
    pub fn with_mcp_registry(mut self, mcp: Arc<mcp::McpRegistry>) -> Self {
        self.mcp_registry = Some(mcp);
        self
    }

    /// Attach a hook registry so `list_hooks` reports real data.
    /// Wrapped in `RwLock` because `HookRegistry::register` is `&mut self`
    /// and the CLI binary may register hooks past the initial load.
    /// (M6-07)
    #[must_use]
    pub fn with_hook_registry(
        mut self,
        hooks: Arc<tokio::sync::RwLock<hooks::HookRegistry>>,
    ) -> Self {
        self.hook_registry = Some(hooks);
        self
    }

    /// Attach an agent catalog so `list_agents` reports real data.
    /// Wrapped in `RwLock` so the CLI binary can append/reload agents
    /// without rebuilding the orchestrator. (M6-07)
    #[must_use]
    pub fn with_agent_catalog(
        mut self,
        agents: Arc<tokio::sync::RwLock<Vec<agent::AgentDefinition>>>,
    ) -> Self {
        self.agent_catalog = Some(agents);
        self
    }

    /// Attach a skill-listing provider so the per-turn `skill_listing`
    /// system-reminder enumerates the model-invocable skills (SKILLLIST.1).
    /// Without this the reminder is never injected (the model cannot discover
    /// skills autonomously). The desktop binary wires a `CommandRegistry`-backed
    /// provider at the composition root.
    #[must_use]
    pub fn with_skill_listing(
        mut self,
        provider: Arc<dyn crate::prompt::skill_listing::SkillListingProvider>,
    ) -> Self {
        self.skill_listing = Some(provider);
        self
    }

    /// Whether a skill-listing provider has been wired via
    /// [`Self::with_skill_listing`]. (SKILLLIST.1)
    #[must_use]
    pub fn has_skill_listing(&self) -> bool {
        self.skill_listing.is_some()
    }

    /// Attach a memory prefetcher so the per-turn `relevant_memories` surfacing
    /// reminder is injected (P0.1). Without this the surfacing channel is a
    /// strict no-op ([`Self::relevant_memory_reminder_message`] returns `None`),
    /// keeping the locked fixtures byte-identical — the LingXi equivalent of
    /// claude-code's `tengu_moth_copse`-default-false gate (here: "is a prefetch
    /// wired at all"). Wired at the composition root once a real
    /// selector-backed prefetch lands.
    #[must_use]
    pub fn with_memory_prefetch(mut self, prefetch: Arc<memory::prefetch::MemoryPrefetch>) -> Self {
        self.memory_prefetch = Some(prefetch);
        self
    }

    /// Wire the EndConversation end-request slot (shared with the tool). The
    /// composition root passes the same `Arc` it gave
    /// [`crate::end_conversation_tool::EndConversationTool::new`]; the turn loop
    /// reads+consumes it after tool execution to terminate the conversation.
    /// Only set when the feature is enabled — `None` keeps the turn loop
    /// byte-identical.
    #[must_use]
    pub fn with_end_conversation_slot(
        mut self,
        slot: crate::end_conversation_tool::EndConversationSlot,
    ) -> Self {
        self.end_conversation_slot = Some(slot);
        self
    }

    /// Whether a memory prefetcher has been wired via
    /// [`Self::with_memory_prefetch`]. (P0.1)
    #[must_use]
    pub fn has_memory_prefetch(&self) -> bool {
        self.memory_prefetch.is_some()
    }

    /// Wire the EXPERIMENTAL_SKILL_SEARCH skill-discovery prefetcher. The
    /// composition root calls this ONLY when the flag is ON (default OFF), so an
    /// unwired build keeps [`Self::start_skill_discovery_prefetch`] +
    /// [`Self::skill_discovery_reminder_message`] strict no-ops and the locked
    /// fixtures byte-identical (the presence of the prefetch IS the gate, exactly
    /// like `with_memory_prefetch`).
    #[must_use]
    pub fn with_skill_discovery_prefetch(
        mut self,
        prefetch: Arc<skill_api::SkillDiscoveryPrefetch>,
    ) -> Self {
        self.skill_discovery_prefetch = Some(prefetch);
        self
    }

    /// Whether a skill-discovery prefetcher has been wired via
    /// [`Self::with_skill_discovery_prefetch`] (EXPERIMENTAL_SKILL_SEARCH).
    #[must_use]
    pub fn has_skill_discovery_prefetch(&self) -> bool {
        self.skill_discovery_prefetch.is_some()
    }

    /// Wire the standalone session-memory extractor (§6.5). `None` (the default)
    /// keeps it inert. The composition root builds the handle (gated, default
    /// off) via [`crate::prompt::build_session_memory_handle`].
    #[must_use]
    pub fn with_session_memory(mut self, handle: Arc<SessionMemoryHandle>) -> Self {
        self.session_memory = Some(handle);
        self
    }

    /// P1 (§6.5): when a session-memory handle is wired AND the extractor's
    /// tool-call threshold is crossed, BACKGROUND-fork a distillation of the
    /// session history and write `<configHome>/agents/session-memory/<id>.md`
    /// (which the next session re-loads via the Session-tier memdir scan). A
    /// strict no-op when no handle is wired, no cache-safe params are available
    /// yet, or the threshold is not crossed — so the locked fixtures stay
    /// byte-identical by default. Never blocks the turn (the fork runs on the
    /// handle's runtime); a failed extraction is swallowed, never surfaced.
    pub(crate) async fn maybe_extract_session_memory(&self) {
        let Some(handle) = self.session_memory.clone() else {
            return;
        };
        // The fork shares the parent's cache-safe prefix; without it (early in a
        // session) skip — a later turn re-checks.
        let Some(slot) = self.cache_safe_slot.as_ref() else {
            return;
        };
        let Some(params) = slot.get_last().await else {
            return;
        };
        // Snapshot history + id without holding the session lock across the fork.
        let (history, session_id) = {
            let s = self.session.lock().await;
            (s.history.clone(), s.session_id)
        };
        let runtime = handle.runtime.clone();
        let _ = runtime
            .spawn(
                "session-memory-extract",
                Box::pin(async move {
                    let mut ex = handle.extractor.lock().await;
                    if !ex.should_extract(&history) {
                        return;
                    }
                    let _ = ex
                        .extract(
                            &handle.runner,
                            params,
                            &session_id.to_string(),
                            &history,
                            &handle.config_home,
                        )
                        .await;
                }),
            )
            .await;
    }

    /// Wire the source of completed background (`async`) hook responses, folded
    /// back into the next turn by [`Self::async_hook_response_reminder_message`]
    /// (claude-code `getAsyncHookResponseAttachments`). Without it that method
    /// is a strict no-op.
    #[must_use]
    pub fn with_async_hook_responses(
        mut self,
        provider: Arc<dyn crate::prompt::async_hook_response::AsyncHookResponseProvider>,
    ) -> Self {
        self.async_hook_responses = Some(provider);
        self
    }

    /// T35: wire the source of terminal background tasks, folded back into the
    /// next turn as a `<task-notification>` reminder by
    /// [`Self::task_notification_reminder_message`] (claude-code's per-task-type
    /// `enqueue*Notification`). Without it that method is a strict no-op.
    #[must_use]
    pub fn with_task_notifications(
        mut self,
        provider: Arc<dyn crate::prompt::task_notification::TaskNotificationProvider>,
    ) -> Self {
        self.task_notifications = Some(provider);
        self
    }

    /// Wire the source of the `Stop` / `SubagentStop` hook `background_tasks` +
    /// `session_crons` snapshot (claude-code `Lic(taskRegistry.all())` /
    /// `Mic()`). Without it both fields stay `None` and the keys are omitted on
    /// every payload (the no-registry default — byte-identical to today).
    #[must_use]
    pub fn with_stop_hook_snapshot(
        mut self,
        provider: Arc<dyn crate::stop_hook_snapshot::StopHookSnapshotProvider>,
    ) -> Self {
        self.stop_hook_snapshot = Some(provider);
        self
    }

    /// Wire the mid-turn input source consulted by the streaming turn loop at
    /// each cancel-check point to drain queued user input WITHIN the running turn
    /// (claude-code's query.ts mid-turn injection). Without it the mid-turn drain
    /// is a strict no-op, so a turn with no source wired is byte-identical to
    /// today. Builder form (fresh construction). See
    /// [`Self::set_mid_turn_input`] for the post-`Arc` (`&self`) form the bridge
    /// uses with its per-connection queue.
    #[must_use]
    pub fn with_mid_turn_input(
        self,
        source: Arc<dyn crate::prompt::mid_turn_input::MidTurnInputSource>,
    ) -> Self {
        self.set_mid_turn_input(source);
        self
    }

    /// Set the mid-turn input source on a SHARED orchestrator (`&self`), so the
    /// bridge can wire its per-connection queue adapter AFTER `engine_desktop::build`
    /// returns the orchestrator as an `Arc`. Set-once: a second call is ignored
    /// (the first wiring wins). See [`Self::with_mid_turn_input`].
    pub fn set_mid_turn_input(
        &self,
        source: Arc<dyn crate::prompt::mid_turn_input::MidTurnInputSource>,
    ) {
        let _ = self.mid_turn_input.set(source);
    }

    /// Wire the abort-reason flag shared with the queue adapter so the streaming
    /// loop can tell a `Now`-command abort from a user Ctrl+C/ESC interrupt.
    /// Without it every abort is labeled a user interrupt (today's behavior).
    /// Builder form. See [`Self::set_cancel_reason`] for the `&self` form.
    #[must_use]
    pub fn with_cancel_reason(self, flag: crate::prompt::mid_turn_input::CancelReasonFlag) -> Self {
        self.set_cancel_reason(flag);
        self
    }

    /// Set the abort-reason flag on a SHARED orchestrator (`&self`). Set-once.
    /// See [`Self::with_cancel_reason`].
    pub fn set_cancel_reason(&self, flag: crate::prompt::mid_turn_input::CancelReasonFlag) {
        let _ = self.cancel_reason.set(flag);
    }

    /// Read the active turn's [`crate::prompt::mid_turn_input::CancelReason`] from
    /// the wired flag, defaulting to `UserInterrupt` when no flag is wired (so an
    /// un-wired turn always takes the user-interrupt branch — today's behavior).
    fn cancel_reason_now(&self) -> crate::prompt::mid_turn_input::CancelReason {
        self.cancel_reason.get().map_or(
            crate::prompt::mid_turn_input::CancelReason::UserInterrupt,
            |f| f.get(),
        )
    }

    /// Mid-turn drain step: pull any queued main-thread, non-slash input from the
    /// wired source and inject it as a META user message so the next sampling
    /// sees it. A strict no-op when no source is wired (the default) or the queue
    /// is empty. Returns `true` if anything was injected (for the caller's
    /// observability — the loop continues regardless). Mirrors claude-code's
    /// `joinPromptValues` + meta-prompt injection at query.ts ~1570-1580.
    async fn drain_mid_turn_input(&self) -> bool {
        let Some(source) = self.mid_turn_input.get() else {
            return false;
        };
        let mut injected = false;
        // Loop so a burst of consecutive enqueues all land before the next call.
        // The production source ([`MsgQueueMidTurnInput`]) is consume-once — it
        // REMOVES the commands it returns each call — so it self-terminates after
        // it has drained the queue. The bound below is a defensive guard against a
        // MALFUNCTIONING source impl (the trait is public; a buggy impl that fails
        // to consume could otherwise return `Some` forever and hang the turn loop):
        // we cap the per-iteration drain at a generous fixed number of batches so a
        // single drain step can never spin unboundedly.
        const MAX_DRAIN_BATCHES: usize = 1024;
        for _ in 0..MAX_DRAIN_BATCHES {
            match source.take_mid_turn_input().await {
                Some(text) => {
                    let wrapped = Self::wrap_mid_turn_user_message(&text);
                    self.inject_meta_user_message(&wrapped).await;
                    injected = true;
                }
                None => break,
            }
        }
        injected
    }

    /// Wrap joined mid-turn user input in the 2.1.206 envelope (`YAt`, binary
    /// @225654300) before injection. The `human` / `auto-continuation` / unset
    /// arm — the only source the port's `MsgQueueMidTurnInput` produces (all
    /// mid-turn input is user-typed) — prefixes `jca` ("The user sent a new
    /// message while you were working:\n") and appends the explainer. Em-dash is
    /// U+2014; "Claude Code" -> "LingXi" per the brand rebrand. (206 dropped the
    /// 201 "IMPORTANT: After completing your current task…" suffix — 0 hits in
    /// 206.) A non-user source would instead use
    /// `"[MESSAGE FROM NON-USER SOURCE - NOT USER INPUT]\n{text}"`, but the port
    /// has no such mid-turn source today.
    #[must_use]
    fn wrap_mid_turn_user_message(text: &str) -> String {
        format!(
            "The user sent a new message while you were working:\n{text}\n\nThis is how LingXi surfaces messages the user sends mid-turn \u{2014} within the running turn, often alongside the next tool result, rather than as a separate conversation turn. Address the message above as you continue this turn."
        )
    }

    /// Finding #73: wire the V2 task source consulted by the per-turn
    /// `task_reminder` ([`Self::todo_reminder_message`], the binary's `B4p`).
    /// Without it the V2 reminder still fires when its counters/gates are met,
    /// but renders with no task items (base text only). The V1 (`todo_reminder`)
    /// path reads `session.todos` directly and needs no provider.
    #[must_use]
    pub fn with_todo_reminder_tasks(
        mut self,
        provider: Arc<dyn crate::prompt::todo_reminder::TodoReminderTaskProvider>,
    ) -> Self {
        self.todo_reminder_tasks = Some(provider);
        self
    }

    /// Whether an MCP registry has been wired via
    /// [`Self::with_mcp_registry`]. (M6-07)
    #[must_use]
    pub fn has_mcp_registry(&self) -> bool {
        self.mcp_registry.is_some()
    }

    /// Whether the wired MCP registry has the OAuth seam injected
    /// ([`mcp::registry::OAuthDeps`] via `McpRegistry::with_oauth`). `false`
    /// when no registry is wired OR the registry lacks OAuth (static-header
    /// fallback only). Lets the desktop composition-root test confirm OAuth is
    /// production-reachable end to end.
    #[must_use]
    pub fn has_mcp_oauth(&self) -> bool {
        self.mcp_registry.as_ref().is_some_and(|r| r.has_oauth())
    }

    /// Whether the wired MCP registry has a Cross-App-Access provider injected
    /// ([`mcp::registry::XaaConfigProvider`] inside [`mcp::registry::OAuthDeps`]).
    /// `false` when no registry/OAuth is wired OR no `xaaIdp` settings were
    /// present (XAA stays opt-in, falling through to the actionable hard-fail).
    /// Lets the desktop composition-root test confirm the XAA config layer is
    /// production-reachable once configured.
    #[must_use]
    pub fn has_mcp_xaa(&self) -> bool {
        self.mcp_registry.as_ref().is_some_and(|r| r.has_xaa())
    }

    /// Whether a hook registry has been wired via
    /// [`Self::with_hook_registry`]. (M6-07)
    #[must_use]
    pub fn has_hook_registry(&self) -> bool {
        self.hook_registry.is_some()
    }

    /// Whether an agent catalog has been wired via
    /// [`Self::with_agent_catalog`]. (M6-07)
    #[must_use]
    pub fn has_agent_catalog(&self) -> bool {
        self.agent_catalog.is_some()
    }

    /// Attach a [`compaction::CompactionOrchestrator`] so
    /// `force_compact` performs real history compaction. Without this,
    /// `force_compact` retains the M5-10 no-op shape. (M6-08)
    #[must_use]
    pub fn with_compaction(mut self, compactor: Arc<compaction::CompactionOrchestrator>) -> Self {
        self.compaction = Some(compactor);
        self
    }

    /// Attach the shared [`sidequery::CacheSafeParamsSlot`] the turn drivers
    /// write after every successful API call (In-Loop Compaction Batch 6).
    ///
    /// Pass the SAME `Arc` that was handed to
    /// [`compaction::Autocompactor::with_forked_runner`] at the composition
    /// root, so the forked autocompact summarizer reads the prefix this
    /// orchestrator produced. Without this the slot stays empty and the
    /// summarizer falls back to a no-cache forked call.
    #[must_use]
    pub fn with_cache_safe_slot(mut self, slot: Arc<sidequery::CacheSafeParamsSlot>) -> Self {
        self.cache_safe_slot = Some(slot);
        self
    }

    /// Attach the `/fork` background-agent spawner (the composition root's
    /// `BackgroundAgentSpawner`), so [`OrchestratorHandle::fork_conversation`]
    /// can spawn a detached background agent. Without it, `/fork` fails with a
    /// clear `ActionFailed`.
    #[must_use]
    pub fn with_fork_spawner(
        mut self,
        spawner: Arc<dyn traits::subagent_spawn::SubagentSpawner>,
    ) -> Self {
        self.fork_spawner = Some(spawner);
        self
    }

    /// Attach the budget enforcer a `/fork`-spawned background agent inherits
    /// (`SubagentInheritance::budget`). Pair with [`Self::with_fork_spawner`].
    #[must_use]
    pub fn with_fork_budget(
        mut self,
        budget: Arc<dyn traits::budget::BudgetEnforcerHandle>,
    ) -> Self {
        self.fork_budget = Some(budget);
        self
    }

    /// Attach the `/recap` side-query runner. Pass the SAME
    /// [`sidequery::ForkedAgentRunner`] `Arc` handed to
    /// [`compaction::Autocompactor::with_forked_runner`] at the composition root
    /// (clone it BEFORE that move) so recap replays the identical cache-safe
    /// prefix the summarizer would. Without it, `/recap` fails gracefully.
    #[must_use]
    pub fn with_recap_runner(mut self, runner: Arc<sidequery::ForkedAgentRunner>) -> Self {
        self.recap_runner = Some(runner);
        self
    }

    /// (`/rewind`) Share the file-history checkpoint store — the SAME
    /// `Arc<session::FileHistory>` the CLI holds for restore + picker rows. The
    /// turn loop then snapshots each turn and tracks tool edits.
    #[must_use]
    pub fn with_file_history(mut self, file_history: Arc<session::FileHistory>) -> Self {
        self.file_history = Some(file_history);
        self
    }

    /// (`/fast`) Share the session's fast-mode flag with this orchestrator — the
    /// SAME `Arc<AtomicBool>` the request-building `ProviderApiAdapter` holds, so
    /// the handle's `set_fast_mode` flip is seen by the adapter on the next
    /// turn. Without it the flag is a private always-`false` default (no request
    /// ever carries `speed`).
    #[must_use]
    pub fn with_fast_mode(mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.fast_mode = flag;
        self
    }

    /// Byte-exact `/recap` prompt (probed from the 2.1.198 binary). Sent as the
    /// single user turn of the isolated recap side query. Kept in lockstep with
    /// the byte-audit copy in `command-core`'s `recap.rs` test.
    pub(crate) const RECAP_PROMPT: &str = "The user stepped away and is coming back. Recap in under 40 words, 1-2 plain sentences, no markdown. Lead with the overall goal and current task, then the one next action. Skip root-cause narrative, fix internals, secondary to-dos, and em-dash tangents.";

    /// Real `/recap` body — a read-only, tool-denied, single-turn side query,
    /// cancelable via a [`CancellationToken`]. HISTORY-INERT by construction:
    /// unlike [`Self::force_compact_with_cancel`], it reuses the
    /// [`sidequery::ForkedAgentRunner`] directly (the SAME single-turn primitive
    /// the autocompact summarizer uses) and NEVER touches `session.history`,
    /// `apply_post_compact`, `save_cache_safe_params`, or the pre/post-compact
    /// hooks. The runner replays `cache_safe_params.fork_context_messages` +
    /// the recap prompt, exposes no tools, and issues exactly one query.
    ///
    /// On cancel returns `Ok(RecapOutcome::Cancelled)` (a fixed-string outcome),
    /// not `Err`. An empty cache-safe slot (no successful turn recorded yet — a
    /// resumed session whose transcript loaded but which has run no live turn)
    /// maps to `Err(ActionFailed)` rather than a panic.
    pub(crate) async fn generate_recap_query(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<traits::RecapOutcome, traits::HandleError> {
        let runner = self
            .recap_runner
            .clone()
            .ok_or_else(|| traits::HandleError::ActionFailed("recap unavailable".into()))?;
        let params = self
            .cache_safe_slot
            .as_ref()
            .ok_or_else(|| traits::HandleError::ActionFailed("recap: no cache-safe slot".into()))?
            .get_last()
            .await
            .ok_or_else(|| {
                traits::HandleError::ActionFailed("recap: no cache-safe params".into())
            })?;

        // Fast-path cancel (deterministic even when the runner completes
        // synchronously, e.g. the stub side-query path), mirroring
        // `force_compact_with_cancel`.
        if cancel.is_cancelled() {
            return Ok(traits::RecapOutcome::Cancelled);
        }

        let req = sidequery::ForkedAgentRequest {
            prompt_messages: vec![ConversationMessage::user(
                MessageId::new(),
                Self::RECAP_PROMPT.to_string(),
            )],
            cache_safe_params: params,
            fork_label: "recap".into(),
            query_source: sidequery::QuerySource::Custom("recap".into()),
            max_output_tokens: Some(256),
        };

        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(traits::RecapOutcome::Cancelled),
            r = runner.run(req) => match r {
                Ok(res) => Ok(traits::RecapOutcome::Text(res.final_text.trim().to_string())),
                Err(e) => Err(traits::HandleError::ActionFailed(e.to_string())),
            }
        }
    }

    /// Byte-faithful `/btw` side-question wrapper, ported verbatim from
    /// claude-code `utils/sideQuestion.ts`: the `<system-reminder>` that turns
    /// the shared context into a one-off, tool-less answer. Prepended (with a
    /// blank line) to the user's question as the single user turn of the
    /// isolated side query.
    pub(crate) const SIDE_QUESTION_SYSTEM_REMINDER: &str = "<system-reminder>This is a side question from the user. You must answer this question directly in a single response.\n\nIMPORTANT CONTEXT:\n- You are a separate, lightweight agent spawned to answer this one question\n- The main agent is NOT interrupted - it continues working independently in the background\n- You share the conversation context but are a completely separate instance\n- Do NOT reference being interrupted or what you were \"previously doing\" - that framing is incorrect\n\nCRITICAL CONSTRAINTS:\n- You have NO tools available - you cannot read files, run commands, search, or take any actions\n- This is a one-off response - there will be no follow-up turns\n- You can ONLY provide information based on what you already know from the conversation context\n- NEVER say things like \"Let me try...\", \"I'll now...\", \"Let me check...\", or promise to take any action\n- If you don't know the answer, say so - do not offer to look it up or investigate\n\nSimply answer the question with the information you have.</system-reminder>";

    /// Real `/btw` body — the SAME read-only, tool-denied, single-turn,
    /// HISTORY-INERT side query as [`Self::generate_recap_query`], differing
    /// ONLY in the prompt (the wrapped side question instead of `RECAP_PROMPT`).
    /// Reuses the SAME `recap_runner` (the single-turn `ForkedAgentRunner`) and
    /// `cache_safe_slot`, so it needs no new composition-root wiring. NEVER
    /// touches `session.history`, cache writes, or the pre/post-compact hooks;
    /// tool-denial is structural (the single-turn runner exposes no tools).
    ///
    /// Empty cache-safe slot (no successful turn yet) maps to `Err(ActionFailed)`
    /// (the TUI renders "Couldn't answer side question: …") rather than a panic —
    /// the same limitation as `/recap` (the LingXi single-turn runner requires a
    /// captured prefix; cc's from-scratch rebuild fallback is not modeled).
    pub(crate) async fn answer_side_question_query(
        &self,
        question: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<traits::RecapOutcome, traits::HandleError> {
        let runner = self
            .recap_runner
            .clone()
            .ok_or_else(|| traits::HandleError::ActionFailed("side question unavailable".into()))?;
        let params = self
            .cache_safe_slot
            .as_ref()
            .ok_or_else(|| {
                traits::HandleError::ActionFailed("side question: no cache-safe slot".into())
            })?
            .get_last()
            .await
            .ok_or_else(|| {
                traits::HandleError::ActionFailed(
                    "side question: no context yet — send a message first".into(),
                )
            })?;

        if cancel.is_cancelled() {
            return Ok(traits::RecapOutcome::Cancelled);
        }

        let wrapped = format!("{}\n\n{}", Self::SIDE_QUESTION_SYSTEM_REMINDER, question);
        let req = sidequery::ForkedAgentRequest {
            prompt_messages: vec![ConversationMessage::user(MessageId::new(), wrapped)],
            cache_safe_params: params,
            fork_label: "side_question".into(),
            query_source: sidequery::QuerySource::Custom("side_question".into()),
            // Uncapped like cc's `runSideQuestion` (maxTurns=1, no maxTokens):
            // `None` → the runner's DEFAULT_FORK_MAX_TOKENS.
            max_output_tokens: None,
        };

        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(traits::RecapOutcome::Cancelled),
            r = runner.run(req) => match r {
                Ok(res) => Ok(traits::RecapOutcome::Text(res.final_text.trim().to_string())),
                Err(e) => Err(traits::HandleError::ActionFailed(e.to_string())),
            }
        }
    }

    /// Wire the passive `<new-diagnostics>` source (the LSP diagnostic registry)
    /// so each turn surfaces newly-reported LSP diagnostics to the model. See
    /// [`Self::new_diagnostics_source`] / [`Self::new_diagnostics_reminder_message`].
    #[must_use]
    pub fn with_new_diagnostics_source(
        mut self,
        source: Arc<dyn traits::NewDiagnosticsSource>,
    ) -> Self {
        self.new_diagnostics_source = Some(source);
        self
    }

    /// Snapshot the cache-safe prompt prefix into the wired slot after a
    /// successful API call (In-Loop Compaction Batch 6).
    ///
    /// Strict no-op when no slot is wired (every test + any binary that has not
    /// wired the forked runner), so it adds zero work — and crucially no history
    /// clone — off the production path. When wired, it stores the CURRENT
    /// `session.history` as `fork_context_messages`: callers invoke this right
    /// after the model call returns successfully but BEFORE appending the
    /// assistant reply, so the snapshot is exactly the message set the model
    /// saw (including any PTL truncation / reactive compaction the call applied).
    ///
    /// `user_context` / `system_context` / `tool_use_options` are not consulted
    /// by the single-turn forked summary path (it exposes no tools and replays
    /// `system_prompt` + `fork_context_messages` verbatim — see
    /// `sidequery::ForkedAgentRunner::run`), so they are filled minimally; only
    /// `system_prompt` and `fork_context_messages` drive the cache hit.
    pub(crate) async fn save_cache_safe_params(&self, system: Option<&str>, model: &str) {
        let Some(slot) = self.cache_safe_slot.as_ref() else {
            return;
        };
        let fork_context_messages = {
            let s = self.session.lock().await;
            s.history.clone()
        };
        slot.save(sidequery::CacheSafeParams {
            system_prompt: system.unwrap_or("").into(),
            user_context: std::collections::HashMap::new(),
            system_context: std::collections::HashMap::new(),
            tool_use_options: tool_api::ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: model.to_string(),
                model_profile: None,
                max_budget_nano_usd: None,
                mcp_clients: Vec::new(),
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            fork_context_messages,
            // Overwritten by the slot on save; the value here is irrelevant.
            generation: 0,
        })
        .await;
    }

    /// FORK (codex #5 follow-up): record the rendered system-prompt bytes this
    /// turn handed the model, so a fork-subagent spawn dispatched later in the
    /// SAME turn can thread them onto its child (cache-identical prefix, claude
    /// `AgentTool.tsx:622-623`). Called by the turn drivers right after a
    /// successful API call (the `save_cache_safe_params` site). `None`/empty
    /// system collapses to `None` (a turn with no system prompt records nothing).
    pub(crate) async fn save_current_turn_system_prompt(&self, system: Option<&str>) {
        *self.current_turn_system_prompt.lock().await =
            system.filter(|s| !s.is_empty()).map(ToString::to_string);
    }

    /// FORK (codex #5 follow-up): the rendered system prompt recorded by the most
    /// recent successful turn ([`Self::save_current_turn_system_prompt`]), or
    /// `None` before the first successful turn / when that turn had no system
    /// prompt. Read by the fork dispatch path to seed each tool's
    /// `ToolUseContext::fork_parent_system_prompt`.
    pub(crate) async fn current_turn_system_prompt(&self) -> Option<String> {
        self.current_turn_system_prompt.lock().await.clone()
    }

    /// Task 8 (llm-client future-work batch 3): forward the API client's
    /// latest unified rate-limit header snapshot to
    /// [`traits::OutputStream::emit_rate_limit`], emitting ONLY when it
    /// differs from the last emitted value (emit-on-change dedup against
    /// [`Self::last_emitted_rate_limit`]).
    ///
    /// Called by the turn drivers after each completed API call — the
    /// batched/cancelable seam in `turn_loop::execute_one_turn_with_recovery_tracked`
    /// and the streaming seam in `try_run_turn_streaming` (both right after
    /// `save_cache_safe_params`, the existing "API call succeeded" point).
    /// `self.api` and `self.streaming_api` are the same `ProviderApiAdapter`
    /// in production, and the adapter records headers on both
    /// `drive_non_stream` and the `drive_stream` connect-success path, so
    /// reading `self.api` covers both drivers.
    ///
    /// A strict no-op when the client has no snapshot (the default
    /// `last_rate_limit_full()` returns `None` — mocks / non-Anthropic
    /// providers), so all pre-existing fixtures see zero extra events.
    pub(crate) async fn emit_rate_limit_if_changed(&self) {
        let Some(info) = self.api.last_rate_limit_full() else {
            return;
        };
        let mut last = self.last_emitted_rate_limit.lock().await;
        if last.as_ref() == Some(&info) {
            return;
        }
        self.output
            .emit_rate_limit(
                info.status.as_deref(),
                info.rate_limit_type.as_deref(),
                info.utilization,
                info.resets_at,
                info.claim_resets_at,
                info.overage_status.as_deref(),
                info.overage_resets_at,
                info.overage_disabled_reason.as_deref(),
                info.fallback_available,
            )
            .await;
        *last = Some(info);
    }

    /// Task 2 (llm-client future-work batch 5): forward the API client's
    /// latest RAW per-window utilization snapshot to
    /// [`traits::OutputStream::emit_raw_utilization`] when it CHANGED since
    /// the last emit and is non-empty.
    ///
    /// Called immediately next to [`Self::emit_rate_limit_if_changed`] at
    /// both turn-driver seams (the batched/cancelable funnel in
    /// `turn_loop::execute_one_turn_with_recovery_tracked` and the streaming
    /// seam in `try_run_turn_streaming`), reading the same
    /// `self.api`-cached snapshot source.
    ///
    /// Documented divergence: TS updates `rawUtilization` unconditionally on
    /// every headers pass (`claudeAiLimits.ts:476` and the 429 path `:500`),
    /// because it is module state polled by `getRawUtilization()`; our event
    /// channel emits on change to avoid spamming the stream — the same
    /// dedupe stance as [`Self::emit_rate_limit_if_changed`]. The EMPTY
    /// snapshot is never emitted (TS clears raw state only via the
    /// subscriber-gating path we don't model, `claudeAiLimits.ts:462`).
    ///
    /// Atomic-window invariant: each window contributes either both `Some`
    /// values or both `None` — guaranteed by construction, since
    /// [`crate::model::rate_limit::RawWindow`] only exists with both fields.
    pub(crate) async fn emit_raw_utilization_if_changed(&self) {
        let Some(raw) = self.api.last_raw_utilization() else {
            return;
        };
        if raw == crate::model::rate_limit::RawUtilization::default() {
            return;
        }
        let mut last = self.last_emitted_raw_utilization.lock().await;
        if last.as_ref() == Some(&raw) {
            return;
        }
        self.output
            .emit_raw_utilization(
                raw.five_hour.map(|w| w.utilization),
                raw.five_hour.map(|w| w.resets_at),
                raw.seven_day.map(|w| w.utilization),
                raw.seven_day.map(|w| w.resets_at),
            )
            .await;
        *last = Some(raw);
    }

    /// Whether a [`compaction::CompactionOrchestrator`] has been
    /// wired via [`Self::with_compaction`]. (M6-08)
    #[must_use]
    pub fn has_compaction(&self) -> bool {
        self.compaction.is_some()
    }

    /// Real `force_compact` body — cancelable via a [`CancellationToken`].
    ///
    /// Behavior:
    /// 1. Snapshot `session.history` (clone — we don't hold the lock
    ///    across `process_iteration`).
    /// 2. Race `compactor.process_iteration(snapshot, 0)` against
    ///    `cancel.cancelled()`. On cancel, drop the future and return
    ///    `HandleError::ActionFailed("compaction cancelled")` —
    ///    history is left untouched.
    /// 3. On success: append a `[Compacted N → M messages]` System
    ///    marker, swap history under the same lock, emit
    ///    `OutputEvent::CompactionCompleted`, return a
    ///    `CompactionSummary` with real numbers.
    ///
    /// When no compactor is wired (`compaction == None`), falls back
    /// to the M5-10 no-op shape — returns the current history length
    /// as both `messages_before` and `messages_after`.
    ///
    /// (M6-08)
    pub async fn force_compact_with_cancel(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<traits::CompactionSummary, traits::HandleError> {
        let Some(compactor) = self.compaction.clone() else {
            // No compactor wired — fall back to the M5-10 no-op shape so
            // pre-M6-08 callers do not break.
            let s = self.session.lock().await;
            let count = u32::try_from(s.history.len()).unwrap_or(u32::MAX);
            return Ok(traits::CompactionSummary {
                messages_before: count,
                messages_after: count,
                bytes_saved: 0,
            });
        };

        // Snapshot history (clone — we don't hold the lock across the
        // network call inside `process_iteration`).
        let history_before = {
            let s = self.session.lock().await;
            s.history.clone()
        };
        let messages_before = u32::try_from(history_before.len()).unwrap_or(u32::MAX);
        let bytes_before: u64 = history_before.iter().map(protocol::text_byte_size).sum();

        // Fast-path: if already cancelled, exit without invoking the
        // compactor. tokio::select! random-polls between ready arms,
        // so this explicit check keeps the cancel-first contract
        // deterministic even when process_iteration completes synchronously
        // (e.g. the M3 stub Autocompactor path).
        if cancel.is_cancelled() {
            return Err(traits::HandleError::ActionFailed(
                "compaction cancelled".into(),
            ));
        }

        // hooks compaction lifecycle: PreCompact fires before the summary pass.
        // This is the explicit `/compact` entry point, so the trigger is
        // `manual` (TS `isAutoCompact ? 'auto' : 'manual'`). TS `VJn`: a
        // blocking PreCompact hook ABORTS the compaction, throwing
        // `"Compaction blocked by PreCompact hook: <blockedBy>"`. We surface the
        // same message as the `/compact` failure result.
        if let Some(detail) = self.fire_pre_compact("manual").await {
            let msg = if detail.is_empty() {
                "Compaction blocked by PreCompact hook".to_string()
            } else {
                format!("Compaction blocked by PreCompact hook: {detail}")
            };
            tracing::warn!("{msg}");
            return Err(traits::HandleError::ActionFailed(msg));
        }

        // Run the 5-layer compactor, racing against the cancel token.
        // process_iteration takes no CancellationToken; drop-on-cancel
        // leaves history untouched because we have not written back.
        // `biased` so the cancel arm wins a tie — preferred when both
        // arms are immediately ready.
        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                return Err(traits::HandleError::ActionFailed(
                    "compaction cancelled".into(),
                ));
            }
            r = compactor.process_iteration(history_before, 0) => r
                .map_err(|e| traits::HandleError::ActionFailed(format!("compaction failed: {e}")))?,
        };

        // hooks compaction lifecycle: capture the summary + freed-token count
        // BEFORE `apply_post_compact` consumes the result, so PostCompact can
        // carry the byte-faithful payload (TS `compactData.compactSummary`).
        let summary = Self::compaction_summary_text(&result);
        let tokens_freed = result.total_tokens_freed;

        // Apply the post-compact transition (boundary marker + history swap +
        // CompactionCompleted emit) via the shared helper reused by the
        // proactive trigger (Batch 4) and the reactive 413 fallback (Batch 5).
        // `bytes_before` was computed from the same `history_before` snapshot.
        let summary_out = self
            .apply_post_compact(
                result,
                compaction::CompactTrigger::Manual,
                messages_before,
                bytes_before,
            )
            .await;

        // PostCompact fires AFTER the compaction transition has been applied
        // (TS `compact.ts:723`). Manual `/compact` ⇒ `manual` trigger.
        // Best-effort — never fails the call.
        self.fire_post_compact("manual", summary, tokens_freed)
            .await;

        Ok(summary_out)
    }

    /// Apply a completed compaction pass to the live session: append the
    /// `[Compacted N → M messages]` boundary marker, swap `session.history`
    /// under the lock, persist the marker to the optional JSONL writer, and
    /// emit [`traits::OutputStream::emit_compaction_completed`].
    ///
    /// Factored out of [`Self::force_compact_with_cancel`] (Batch 4) so the
    /// manual `/compact` path, the proactive pre-call trigger
    /// ([`crate::turn_loop::maybe_compact_before_call`] via
    /// [`Self::maybe_compact_before_call`]), and the reactive 413 fallback
    /// (Batch 5) all replace history identically. Mirrors TS
    /// `buildPostCompactMessages` + the `CompactionCompleted` yield
    /// (`query.ts:528-534`).
    ///
    /// `messages_before` / `bytes_before` are computed by the caller from the
    /// pre-compaction snapshot (the same snapshot fed to the compactor); the
    /// helper does NOT re-read history before swapping because the caller has
    /// not mutated it between snapshot and apply.
    /// Snapshot the read-file-state, clear it, and restore the most-recent
    /// files as post-compact attachment messages.
    ///
    /// #59 / `K2p`+`Pqn` (`bin/claude.exe` offsets 202820676 / 203001477): after
    /// a compaction the read-file-state is cleared (its entries no longer match
    /// the summarized history) and up to
    /// [`compaction::POST_COMPACT_MAX_FILES_TO_RESTORE`] of the most-recently-read
    /// files are re-attached — capped at
    /// [`compaction::POST_COMPACT_MAX_TOKENS_PER_FILE`] each and a running
    /// [`compaction::POST_COMPACT_TOKEN_BUDGET`] total — so the model keeps the
    /// freshest file context across the boundary. The selection/budgeting is the
    /// pure [`compaction::restore_post_compact_files`]; this method supplies the
    /// candidates (from `read_state_map`, the `{content, mtime_ms}` registry) and
    /// renders each survivor as a `<system-reminder>` meta user message.
    ///
    /// SKILL restoration (`Lqn`) is NOT wired here: this orchestrator carries no
    /// invoked-skill registry to source candidates from, so the skill arm of
    /// `K2p` has no data seam yet (documented residual). FILE restoration uses
    /// the SNAPSHOT content the model last saw rather than a fresh disk re-read
    /// (the binary re-reads via `R6n` with `maxTokens:J9p`); the per-file cap is
    /// applied to the snapshot here, which is observably equivalent for an
    /// unchanged file.
    async fn restore_post_compact_attachments(&self) -> Vec<protocol::ConversationMessage> {
        // Snapshot then clear the read-file-state registries (the `eOt` snapshot
        // + `readFileState.clear()` step). Both the rich map and the `/files`
        // Vec are cleared so the post-compact context starts from the restored
        // set only.
        let snapshot: Vec<(std::path::PathBuf, tool_api::read_file_state::ReadFileEntry)> = {
            let mut map = self
                .read_state_map
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            map.drain().collect()
        };
        self.read_file_state.lock().await.clear();

        if snapshot.is_empty() {
            return Vec::new();
        }

        let candidates: Vec<compaction::FileRestoreCandidate> = snapshot
            .into_iter()
            .map(|(path, entry)| compaction::FileRestoreCandidate {
                path,
                content: entry.content,
                timestamp_ms: entry.mtime_ms,
            })
            .collect();

        // `already_attached` is empty: this port does not thread the running
        // attachment set into the boundary builder, so no file is double-counted
        // here (the snapshot is the sole source).
        let restored = compaction::restore_post_compact_files(candidates, &[]);

        restored
            .into_iter()
            .map(|file| {
                // Render the restored file as a `<system-reminder>` meta user
                // message carrying the (per-file-capped) content. Mirrors the
                // `compact_file_reference` / `type:"file"` attachment surfacing a
                // "Referenced file {path}" body with the file content.
                let body = format!(
                    "<system-reminder>\nReferenced file {} (restored after compaction):\n{}\n</system-reminder>",
                    file.path.display(),
                    file.content
                );
                protocol::ConversationMessage::user_meta(protocol::MessageId::new(), body)
            })
            .collect()
    }

    pub(crate) async fn apply_post_compact(
        &self,
        result: compaction::IterationCompactionResult,
        trigger: compaction::CompactTrigger,
        messages_before: u32,
        bytes_before: u64,
    ) -> traits::CompactionSummary {
        // #58: the usage-zeroed verbatim tail the autocompact layer preserved
        // (`messagesToPreserve` → `messagesToKeep`). Empty on the
        // full-replacement path (short conversation / snip-micro-only /
        // under-threshold), keeping the post-compact history byte-identical to
        // before this finding.
        let preserved_tail = result.messages_to_preserve;

        // CSM.4: build the TS-faithful compact boundary (`createCompactBoundaryMessage`,
        // the byte-exact `"Conversation compacted"` sentinel) instead of the ad-hoc
        // `[Compacted N → M]` marker, so the TUI scrollback + next-turn system-prompt
        // assembly see the same boundary TS emits. The rich `CompactBoundaryMetadata`
        // has no orchestrator-side consumer yet (no sidecar store / no
        // `get_messages_after_compact_boundary` caller), so it is discarded here; a
        // follow-up that persists it can swap `_metadata` for a real store.
        //
        // #58: when a tail was preserved, the boundary carries a
        // `preserved_segment` (`WAo`): head = first kept msg, anchor = the LAST
        // summary message (suffix-preserving splice point), tail = last kept msg.
        // The anchor is the last of `result.messages` (the summary set). When the
        // tail is empty, `create_compact_boundary_with_preserved_tail` yields
        // `preserved_segment: None`, identical to the plain constructor.
        let anchor_uuid = if preserved_tail.is_empty() {
            None
        } else {
            result
                .messages
                .last()
                .map(protocol::ConversationMessage::id)
        };
        let (marker, _metadata) = compaction::create_compact_boundary_with_preserved_tail(
            trigger,
            0,
            None,
            None,
            None,
            &[],
            &preserved_tail,
            anchor_uuid.as_ref(),
        );

        // #59: snapshot the read-file-state BEFORE clearing it, then restore the
        // most-recent files as post-compact attachments. Mirrors `Iqn`
        // (`bin/claude.exe` offset 202817825): `let f=eOt(d.readFileState);
        // d.readFileState.clear(); ...; K2p(f,...)`. The restored attachments are
        // appended AFTER the summary, in the `messagesToKeep`/`attachments`
        // position of `buildPostCompactMessages` order
        // `[boundaryMarker, ...summaryMessages, ...attachments, ...]`.
        let restored_attachments = self.restore_post_compact_attachments().await;

        // COMPACT.1 / #58: the boundary marker leads the post-compact history,
        // matching TS `buildPostCompactMessages` / `Iqn` order
        // `[boundaryMarker, ...summaryMessages, ...messagesToKeep, ...attachments,
        // ...hookResults]` (compact.ts:330). The preserved verbatim tail
        // (`messagesToKeep`) rides AFTER the summary and BEFORE the restored
        // attachments. Empty `preserved_tail` ⇒ the order is identical to before
        // (`[marker, ...summary, ...attachments]`).
        let mut history_after = Vec::with_capacity(
            result.messages.len() + 1 + preserved_tail.len() + restored_attachments.len(),
        );
        history_after.push(marker.clone());
        history_after.extend(result.messages);
        // #58: the usage-zeroed verbatim tail (`messagesToKeep`).
        history_after.extend(preserved_tail);
        // Restored file attachments ride after the summary + kept tail (the
        // `attachments` slot in `buildPostCompactMessages`).
        history_after.extend(restored_attachments);

        let messages_after = u32::try_from(history_after.len()).unwrap_or(u32::MAX);
        let bytes_after: u64 = history_after.iter().map(protocol::text_byte_size).sum();
        let bytes_saved = bytes_before.saturating_sub(bytes_after);

        // Swap history under the same lock.
        {
            let mut s = self.session.lock().await;
            s.history = history_after;
        }

        // CSM.4: post-compact module-state reset (TS `resetPostCompactState`).
        // This is a main-thread compact (no subagent `query_source`), so the
        // main-thread resets fire.
        compaction::run_post_compact_cleanup(None);

        // Persist the boundary marker to the optional JSONL writer so a
        // `--resume` of this session sees the compaction transition (the
        // summary user message(s) inside `result.messages` are the compactor's
        // output; the marker is the orchestrator-side boundary). Best-effort —
        // a write failure never fails the turn.
        self.persist_message_to_jsonl(&marker).await;

        // Best-effort emit so the TUI hears about it.
        self.output
            .emit_compaction_completed(messages_before, messages_after, bytes_saved)
            .await;

        traits::CompactionSummary {
            messages_before,
            messages_after,
            bytes_saved,
        }
    }

    /// Proactive pre-call compaction trigger (In-Loop Compaction Batch 4).
    ///
    /// Invoked at the TOP of both the batched
    /// ([`crate::turn_loop::execute_one_turn_with_recovery_tracked`]) and the
    /// streaming turn-driver loop bodies, BEFORE the history snapshot that
    /// feeds the model call — so a proactive compact this turn makes the
    /// subsequent snapshot read the NEW, compacted history. 1:1 with the TS
    /// pre-call pipeline (`query.ts:365-467`, `autoCompactIfNeeded` contract at
    /// `autoCompact.ts:241-351`).
    ///
    /// Behavior:
    /// - **No compactor wired** (`compaction == None`): strict NO-OP — history
    ///   untouched, no event. Keeps the locked turn-loop fixtures behaviour-
    ///   neutral until the CLI wires a real compactor (Batch 6).
    /// - **Under threshold**: strict NO-OP. We gate on
    ///   [`compaction::should_auto_compact`] against the snapshot estimate
    ///   BEFORE calling the orchestrator so that snip/microcompact do not
    ///   silently rewrite history below threshold (the proactive trigger is
    ///   an autocompact gate, not an unconditional snip pass).
    /// - **Over threshold**: run `process_iteration_tracked` threading the
    ///   per-conversation [`Self::compaction_tracking`] circuit-breaker state;
    ///   when `was_compacted`, apply via [`Self::apply_post_compact`] (history
    ///   swap + boundary marker + JSONL persist + `CompactionCompleted` emit).
    ///   The updated `consecutive_failures` is persisted back into
    ///   `compaction_tracking` regardless of success so the breaker survives
    ///   across turns.
    ///
    /// **Recursion note** (parity with the TS `querySource === 'compact'`
    /// recursion guard rationale, `query.ts:365`): the autocompact summarizer
    /// runs as a SEPARATE stateless side-query (`ForkedAgentRunner` /
    /// `SideQueryClient` inside `Autocompactor::compact`) — it does NOT
    /// re-enter `execute_one_turn`/this trigger — so no explicit guard flag is
    /// needed here. Documented to make the absence intentional.
    /// Record the last API response's total input-token count
    /// (`input_tokens + cache_read + cache_creation`) for the fixed-prefix
    /// overflow guard. Called by both turn drivers after every successful call.
    /// Mirrors claude-code's `Xtt` last-usage snapshot (see
    /// [`Self::last_response_input_tokens`]).
    pub(crate) fn record_response_input_tokens(&self, usage: &llm_client::Usage) {
        let total_input = usage
            .billable_tokens
            .input
            .saturating_add(usage.billable_tokens.cache_read)
            .saturating_add(usage.billable_tokens.cache_write);
        self.last_response_input_tokens
            .store(total_input, std::sync::atomic::Ordering::Relaxed);
        // Feed the shared workflow `budget.spent()` pool: this is the single
        // per-response chokepoint both turn drivers call, so adding the
        // response's output tokens here accumulates the main-loop side of the
        // pool. A launched workflow's subagents add their output tokens to the
        // same `Arc`, so `budget.spent()` reads main loop + all workflows.
        self.output_token_pool.fetch_add(
            usage.billable_tokens.output,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// The shared output-token pool backing a launched workflow's
    /// `budget.spent()`. The composition root hands the returned `Arc` to the
    /// `LocalWorkflowHandler` so the script's `spent()` reflects the union of
    /// main-loop and all-workflow output tokens. See [`Self::output_token_pool`].
    #[must_use]
    pub fn output_token_pool(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.output_token_pool.clone()
    }

    /// The turn-start output baseline (claude-code `xtr`) backing a launched
    /// workflow's turn-relative `budget.spent()`. The composition root hands this
    /// `Arc` to the `LocalWorkflowHandler` (snapshotted at workflow spawn). See
    /// [`Self::turn_start_output_baseline`].
    #[must_use]
    pub fn turn_start_output_baseline(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.turn_start_output_baseline.clone()
    }

    /// Fire the `tengu_auto_compact_prefix_overflow` telemetry event for a
    /// detected fixed-prefix overflow.
    ///
    /// 1:1 with claude-code v2.1.183 (`bin/claude.exe` offset 203006250): the
    /// auto path on a non-`null` `a3p` result emits
    /// `j("tengu_auto_compact_prefix_overflow", {...u, wouldHaveBlocked:!0})`
    /// — the spread `...u` carries the overflow descriptor fields, and the
    /// path WARNS but STILL PROCEEDS (the guard never aborts the auto compact;
    /// only the reactive PTL path surfaces `compactionImpossible`).
    async fn fire_prefix_overflow_telemetry(&self, overflow: compaction::PrefixOverflow) {
        let Some(bus) = self.analytics_bus.as_ref() else {
            return;
        };
        #[allow(clippy::cast_possible_wrap)]
        fn int(v: u64) -> telemetry::AnalyticsValue {
            telemetry::AnalyticsValue::Int(i64::try_from(v).unwrap_or(i64::MAX))
        }
        let mut metadata = telemetry::LogEventMetadata::new();
        metadata.insert("prefixTokens".into(), int(overflow.prefix_tokens));
        metadata.insert("thresholdTokens".into(), int(overflow.threshold_tokens));
        metadata.insert("totalInputTokens".into(), int(overflow.total_input_tokens));
        metadata.insert("messagesEstimate".into(), int(overflow.messages_estimate));
        metadata.insert("snipTokensFreed".into(), int(overflow.snip_tokens_freed));
        metadata.insert(
            "documentBlockCount".into(),
            int(u64::from(overflow.document_block_count)),
        );
        metadata.insert(
            "imageBlockCount".into(),
            int(u64::from(overflow.image_block_count)),
        );
        metadata.insert(
            "wouldHaveBlocked".into(),
            telemetry::AnalyticsValue::Bool(true),
        );
        bus.log_event("tengu_auto_compact_prefix_overflow", metadata)
            .await;
    }

    /// Fire the `tengu_auto_compact_rapid_refill_breaker` telemetry event when
    /// the thrashing breaker trips.
    ///
    /// 1:1 with claude-code v2.1.183 (`bin/claude.exe` offset 202942256 /
    /// 203006250): `j("tengu_auto_compact_rapid_refill_breaker",
    /// {consecutiveRapidRefills, turnsSincePreviousCompact, ..., reactive})`.
    /// The reactive arm sets `reactive:!0`; the proactive arm omits it.
    async fn fire_rapid_refill_breaker_telemetry_with_reactive(
        &self,
        consecutive_rapid_refills: u32,
        turns_since_previous_compact: i64,
        reactive: bool,
    ) {
        let Some(bus) = self.analytics_bus.as_ref() else {
            return;
        };
        let mut metadata = telemetry::LogEventMetadata::new();
        metadata.insert(
            "consecutiveRapidRefills".into(),
            telemetry::AnalyticsValue::Int(i64::from(consecutive_rapid_refills)),
        );
        metadata.insert(
            "turnsSincePreviousCompact".into(),
            telemetry::AnalyticsValue::Int(turns_since_previous_compact),
        );
        // queryTracking fields (binary v2.1.193 rapid-refill breaker payload
        // `{…,queryChainId:se,queryDepth:ne.depth}`). `queryDepth` is always 0 in
        // this port (subagents never run through `ConversationOrchestrator`).
        metadata.insert(
            "queryChainId".into(),
            telemetry::AnalyticsValue::String(self.query_chain_id.clone()),
        );
        metadata.insert("queryDepth".into(), telemetry::AnalyticsValue::Int(0));
        if reactive {
            metadata.insert("reactive".into(), telemetry::AnalyticsValue::Bool(true));
        }
        bus.log_event("tengu_auto_compact_rapid_refill_breaker", metadata)
            .await;
    }

    /// Proactive-arm rapid-refill breaker telemetry (no `reactive` flag).
    async fn fire_rapid_refill_breaker_telemetry(
        &self,
        consecutive_rapid_refills: u32,
        turns_since_previous_compact: i64,
    ) {
        self.fire_rapid_refill_breaker_telemetry_with_reactive(
            consecutive_rapid_refills,
            turns_since_previous_compact,
            false,
        )
        .await;
    }

    /// Reactive-arm rapid-refill breaker telemetry (`reactive:true`). Called by
    /// the reactive PTL recovery path in `turn_loop`.
    pub(crate) async fn fire_rapid_refill_breaker_telemetry_reactive(
        &self,
        consecutive_rapid_refills: u32,
        turns_since_previous_compact: i64,
    ) {
        self.fire_rapid_refill_breaker_telemetry_with_reactive(
            consecutive_rapid_refills,
            turns_since_previous_compact,
            true,
        )
        .await;
    }

    /// `tengu_post_autocompact_turn` (binary main loop, offset ~209133741):
    /// fired once per turn that follows an auto-compact, alongside the
    /// `turnCounter++` increment. Payload `{turnId, turnCounter, queryChainId,
    /// queryDepth}`; `queryDepth` is always 0 (subagents never run through
    /// `ConversationOrchestrator`). Strict no-op when no analytics bus is wired.
    async fn fire_post_autocompact_turn(&self, turn_id: &str, turn_counter: u32) {
        let Some(bus) = self.analytics_bus.as_ref() else {
            return;
        };
        let mut metadata = telemetry::LogEventMetadata::new();
        metadata.insert(
            "turnId".into(),
            telemetry::AnalyticsValue::String(turn_id.to_string()),
        );
        metadata.insert(
            "turnCounter".into(),
            telemetry::AnalyticsValue::Int(i64::from(turn_counter)),
        );
        metadata.insert(
            "queryChainId".into(),
            telemetry::AnalyticsValue::String(self.query_chain_id.clone()),
        );
        metadata.insert("queryDepth".into(), telemetry::AnalyticsValue::Int(0));
        bus.log_event(
            telemetry::tengu::orchestrator::POST_AUTOCOMPACT_TURN,
            metadata,
        )
        .await;
    }

    pub(crate) async fn maybe_compact_before_call(&self) {
        let Some(compactor) = self.compaction.clone() else {
            // No compactor wired — strict no-op (history untouched).
            return;
        };

        // #54 per-turn turn-counter increment + `tengu_post_autocompact_turn`
        // emit (binary `if(le?.compacted)le.turnCounter++,G(
        // "tengu_post_autocompact_turn",{turnId,turnCounter,queryChainId,queryDepth})`,
        // offsets 202951666 / ~209133741). Runs on EVERY turn after a compact
        // (regardless of the threshold below) so the rapid-refill window
        // (`turn_counter < RAPID_REFILL_TURN_WINDOW`) measures
        // turns-since-previous-compact correctly. Capture under the lock, then
        // emit after releasing it (the analytics emit is async).
        let post_autocompact = {
            let mut tracking = self.compaction_tracking.lock().await;
            if tracking.compacted {
                tracking.turn_counter = tracking.turn_counter.saturating_add(1);
                Some((tracking.turn_id.clone(), tracking.turn_counter))
            } else {
                None
            }
        };
        if let Some((turn_id, turn_counter)) = post_autocompact {
            self.fire_post_autocompact_turn(&turn_id, turn_counter)
                .await;
        }

        // Snapshot history + estimate tokens WITHOUT holding the lock across
        // the (possibly networked) compaction call.
        let snapshot = {
            let s = self.session.lock().await;
            s.history.clone()
        };
        let estimate = compaction::grouping::estimate_tokens_for_range(&snapshot);

        // Threshold gate: under threshold ⇒ strict no-op. `snip_freed = 0`
        // because we have done no snip work yet at the call site.
        if !compaction::should_auto_compact(estimate, 0, compactor.autocompact_threshold) {
            return;
        }

        // #7 consecutive-failures breaker (`bin/claude.exe` `ewo` step 2): a
        // tripped breaker (`consecutiveFailures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES`)
        // makes compaction a guaranteed no-op, so RETURN HERE — BEFORE the
        // fixed-prefix overflow probe below. The binary's `ewo` runs
        // `if(o?.consecutiveFailures>=Swo) return {wasCompacted:!1}` (Swo=3)
        // ahead of `Wom` (the prefix-overflow probe that emits
        // `tengu_auto_compact_prefix_overflow`), so a breaker-tripped,
        // over-threshold session emits NO prefix-overflow event. Without this
        // early return our code fired a spurious `tengu_auto_compact_prefix_overflow`
        // (`wouldHaveBlocked:true`) every turn for such a session.
        {
            let tracking = self.compaction_tracking.lock().await;
            if tracking.consecutive_failures
                >= compaction::thresholds::MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES
            {
                return;
            }
        }

        // #55 fixed-prefix overflow guard (`a3p`, `bin/claude.exe` offset
        // 203004969). If the immovable prefix (system prompt + tools +
        // attachments = `totalInput − messages`) already exceeds the autocompact
        // threshold, compaction can NEVER bring usage below threshold. Mirror the
        // auto path exactly: WARN + emit `tengu_auto_compact_prefix_overflow`
        // (`wouldHaveBlocked:true`) but STILL PROCEED — the guard is observational
        // on the auto path; only the reactive PTL path surfaces it to the user as
        // `compactionImpossible`. `totalInput` is the last response's input total
        // (the `Xtt` snapshot); `0` until the first call ⇒ a strict no-op.
        let total_input = self
            .last_response_input_tokens
            .load(std::sync::atomic::Ordering::Relaxed);
        let (doc_blocks, img_blocks) = count_document_and_image_blocks(&snapshot);
        if let Some(overflow) = compaction::compaction_prefix_overflow(
            total_input,
            estimate,
            compactor.autocompact_threshold,
            0,
            doc_blocks,
            img_blocks,
        ) {
            tracing::warn!(
                prefix_tokens = overflow.prefix_tokens,
                threshold_tokens = overflow.threshold_tokens,
                "autocompact: fixed prefix ~{} > threshold {} — compaction cannot help",
                overflow.prefix_tokens,
                overflow.threshold_tokens,
            );
            self.fire_prefix_overflow_telemetry(overflow).await;
            // Fall through — the auto path STILL PROCEEDS (parity with the binary).
        }

        let messages_before = u32::try_from(snapshot.len()).unwrap_or(u32::MAX);
        let bytes_before: u64 = snapshot.iter().map(protocol::text_byte_size).sum();

        // hooks compaction lifecycle: PreCompact fires once we have crossed the
        // autocompact threshold and are about to run the summary pass (TS
        // `executePreCompactHooks` BEFORE the summary request, `compact.ts:413`).
        // The proactive trigger is always the `auto` arm. TS precomputed arm: a
        // blocking PreCompact hook logs `Precomputed compact blocked by
        // PreCompact hook: <blockedBy>` and skips compaction (history untouched).
        if let Some(detail) = self.fire_pre_compact("auto").await {
            tracing::warn!("Precomputed compact blocked by PreCompact hook: {detail}");
            return;
        }

        // Run the orchestrator pass under the per-conversation tracking lock so
        // the circuit-breaker state is read + written atomically for this turn.
        let mut tracking = self.compaction_tracking.lock().await;
        let result = match compactor
            .process_iteration_tracked(snapshot, 0, &mut tracking)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // Autocompact failed: `tracking.consecutive_failures` has
                // already been incremented in-place by the orchestrator and is
                // retained (the lock guard writes it back on drop). History is
                // left untouched — proactive compaction is best-effort and must
                // never fail the turn (TS `autoCompactIfNeeded` swallows the
                // error and proceeds with the un-compacted history).
                tracing::warn!(error = %e, "proactive autocompact failed; continuing un-compacted");
                return;
            }
        };

        // #54 rapid-refill (thrashing) breaker (proactive trip): when the
        // breaker tripped, the orchestrator SKIPPED the summarizer (history
        // untouched). Emit `tengu_auto_compact_rapid_refill_breaker` and warn,
        // then return — re-summarizing cannot help (a file/tool output is too
        // large), so the proactive path leaves history alone. Mirrors `Eho`
        // (`bin/claude.exe` offset 203006250).
        if result.rapid_refill_breaker_tripped {
            let consecutive = result.consecutive_rapid_refills;
            // `turnsSincePreviousCompact` = the tracking turn counter (the
            // binary reports `oe?.turnCounter ?? -1`).
            let turns_since = i64::from(tracking.turn_counter);
            // Drop the tracking lock before the async telemetry emit.
            drop(tracking);
            tracing::warn!(
                consecutive_rapid_refills = consecutive,
                turns_since_previous_compact = turns_since,
                "autocompact: rapid-refill breaker tripped — {consecutive} consecutive refills within <{} turns each",
                compaction::RAPID_REFILL_TURN_WINDOW,
            );
            self.fire_rapid_refill_breaker_telemetry(consecutive, turns_since)
                .await;
            return;
        }

        if !result.was_compacted {
            // Snip/micro may have fired but autocompact did not (circuit
            // breaker tripped, or the post-snip estimate fell under threshold).
            // Keep history untouched so the proactive trigger stays a strict
            // no-op whenever autocompact itself did not run — matching the
            // manual-path contract that only the autocompact transition emits a
            // boundary marker.
            return;
        }

        // Drop the tracking guard before the apply so the history-swap lock and
        // the tracking lock are never both held (avoid lock-ordering surprises).
        drop(tracking);

        // hooks compaction lifecycle: capture the summary + freed-token count
        // from the compaction result BEFORE `apply_post_compact` consumes it,
        // so the PostCompact hook can carry the byte-faithful payload (TS
        // `compactData.compactSummary`). The proactive trigger is always the
        // `auto` arm (TS `isAutoCompact`).
        let summary = Self::compaction_summary_text(&result);
        let tokens_freed = result.total_tokens_freed;

        self.apply_post_compact(
            result,
            compaction::CompactTrigger::Auto,
            messages_before,
            bytes_before,
        )
        .await;

        // PostCompact fires AFTER the compaction transition has been applied to
        // the live session (TS `compact.ts:723`). Best-effort — never fails the
        // turn.
        self.fire_post_compact("auto", summary, tokens_freed).await;
    }

    /// Read the current cost state from the wired tracker, if any.
    /// Returns `None` if no tracker was attached. Exposed so future M7
    /// renderers (per-model breakdown view) can access
    /// `CostState.per_model_usage` without going through the leaf-friendly
    /// [`traits::CostSnapshot`] projection. (M6-06)
    pub async fn cost_state(&self) -> Option<cost::CostState> {
        let t = self.cost_tracker.as_ref()?;
        Some(t.snapshot().await)
    }

    /// Seed the wired [`cost::CostTracker`]'s cumulative total from a restored
    /// session (resume). No-op when no tracker is wired. Paired with the CLI
    /// mount's project-config `lastCost`/`lastSessionId` persistence so a
    /// `--resume`d session's footer continues from the prior accumulated cost
    /// instead of resetting to `$0.0000` (claude-code `restoreCostStateForSession`).
    pub async fn restore_session_cost(&self, total_nano_usd: u64) {
        if let Some(tracker) = self.cost_tracker.as_ref() {
            tracker.restore_total_nano_usd(total_nano_usd).await;
        }
    }

    /// Project the wired [`cost::CostTracker`] state onto the
    /// leaf-friendly [`traits::CostSnapshot`]. Used by both the
    /// trait method `snapshot_cost` and the per-turn end-of-turn emitter
    /// (`OutputStream::emit_end_turn`). (M6-06)
    ///
    /// If no tracker is wired, returns a zero-valued snapshot keyed to
    /// the current session id (backward-compat shape).
    pub async fn snapshot_cost_real(&self) -> traits::CostSnapshot {
        let session_id = self.session.lock().await.session_id;
        let Some(tracker) = self.cost_tracker.as_ref() else {
            return traits::CostSnapshot {
                session_id,
                ..traits::CostSnapshot::default()
            };
        };
        let state = tracker.snapshot().await;
        // Sum per-model usage into aggregate token counters. api_calls comes
        // from our own counter because cost::Usage does not carry a
        // per-call count (its `add()` merges token totals only).
        let (mut input_tokens, mut output_tokens, mut cache_read_tokens, mut cache_creation_tokens) =
            (0u64, 0u64, 0u64, 0u64);
        for entry in state.per_model_usage.values() {
            input_tokens = input_tokens.saturating_add(entry.usage.tokens.input);
            output_tokens = output_tokens.saturating_add(entry.usage.tokens.output);
            cache_read_tokens = cache_read_tokens.saturating_add(entry.cache_read_input_tokens);
            cache_creation_tokens =
                cache_creation_tokens.saturating_add(entry.cache_creation_input_tokens);
        }
        let api_calls = self
            .api_calls_recorded
            .load(std::sync::atomic::Ordering::SeqCst);
        #[allow(clippy::cast_precision_loss)]
        let total_usd = (state.total_nano_usd as f64) / 1_000_000_000.0;
        let session_duration = self.session_started_at.elapsed();
        traits::CostSnapshot {
            session_id,
            total_nano_usd: state.total_nano_usd,
            total_tokens: input_tokens.saturating_add(output_tokens),
            total_usd,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            api_calls,
            session_duration,
        }
    }

    /// True when a `max_budget_nano_usd` cost ceiling is set AND the session's
    /// cumulative cost has reached it — 1:1 with claude-code
    /// `getTotalCost() >= maxBudgetUsd` (`QueryEngine.ts:972`). Always false when
    /// no cap is set, OR when no [`cost::CostTracker`] is wired (the cap cannot
    /// be enforced without cost tracking — a headless `--max-budget` run, which
    /// wires the tracker, is the primary consumer). Checked at each turn-loop
    /// iteration so a run stops once it crosses the ceiling.
    async fn over_budget(&self) -> bool {
        let Some(budget) = self.config.max_budget_nano_usd else {
            return false;
        };
        let Some(tracker) = self.cost_tracker.as_ref() else {
            return false;
        };
        tracker.snapshot().await.total_nano_usd >= budget
    }

    /// A3: construct a fresh [`BudgetTracker`] for this turn IFF the
    /// token-budget feature is enabled AND a positive budget is configured.
    ///
    /// Returns `None` (the parity default) when
    /// [`OrchestratorConfig::enable_token_budget`] is `false` or
    /// [`OrchestratorConfig::token_budget`] is `None`/`Some(0)` — in which case
    /// the turn drivers skip the budget check entirely and stop at the first
    /// `end_turn`, preserving the locked turn-loop behaviour.
    fn new_budget_tracker(&self) -> Option<BudgetTracker> {
        if self.config.enable_token_budget && matches!(self.config.token_budget, Some(b) if b > 0) {
            Some(BudgetTracker::new())
        } else {
            None
        }
    }

    /// A3: consult the token budget at a natural end-of-turn.
    ///
    /// Returns `true` if the loop should CONTINUE (a continuation nudge was
    /// injected as a meta user message and the A1 recovery count was reset per
    /// `query.ts:1332`); `false` if the loop should stop (budget off, agent
    /// context, threshold reached, or diminishing returns).
    ///
    /// 1:1 with TS `query.ts:1308-1355`: gated by `feature('TOKEN_BUDGET')`,
    /// drives [`check_token_budget`], and on `continue` appends a meta user
    /// message carrying the byte-exact `getBudgetContinuationMessage` nudge.
    /// The completion telemetry is emitted as a `tracing` event on stop.
    /// Inject a meta nudge as a plain user text message into both the live
    /// session history and the JSONL persistence stream. The protocol carries
    /// no `isMeta` flag (cf. the max-output-tokens / token-budget nudges), so a
    /// meta message is a plain user text message carrying the byte-exact bytes.
    /// Shared by the malformed-tool-use retry (#77) and thinking-only (#78)
    /// continuations, which mirror the same injection pattern as the
    /// max-output-tokens recovery nudge.
    /// Finding #80: refusal → fallback-model swap (claude-code `bin/claude.exe`
    /// offset ~205871579, `vr === "refusal" && rc !== void 0`). When the active
    /// turn's response has `stop_reason == "refusal"`, a
    /// [`OrchestratorConfig::refusal_fallback_model`] is configured, AND the
    /// once-per-session latch ([`Self::refusal_fallback_latched`]) is not yet set:
    ///
    /// 1. set the latch (so the fallback fires at most once per session — the
    ///    binary's `refusalFallbackModelLatch` makes the override sticky);
    /// 2. persistently swap the session model to the fallback (the binary's
    ///    `setAppState mainLoopModel = fallbackModel` + `jT(fallbackModel)` —
    ///    every subsequent turn re-snapshots `session.model`, so the swap sticks);
    /// 3. surface the user-visible warning on the output stream (the binary's
    ///    `type:"system", subtype:"model_refusal_fallback", level:"warning",
    ///    content: Bwn(originalModel, fallbackModel, category)`), reproduced
    ///    byte-exact for the common `category == "other"` shape.
    ///
    /// Returns `true` when the swap happened (the caller must retry/continue the
    /// turn against the fallback model), `false` when no fallback is configured or
    /// the latch is already set (the caller preserves today's terminal behavior).
    pub(crate) async fn maybe_swap_to_refusal_fallback(&self) -> bool {
        let Some(fallback) = self.config.refusal_fallback_model.clone() else {
            return false;
        };
        // Once-per-session latch (refusalFallbackModelLatch analog). `swap` returns
        // the PRIOR value, so the first caller sees `false` and proceeds; any later
        // caller sees `true` and bails → at most one swap per session.
        if self
            .refusal_fallback_latched
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return false;
        }
        // Persistently swap the session model to the fallback.
        let original_model = {
            let mut s = self.session.lock().await;
            let prev = std::mem::replace(&mut s.model, fallback.clone());
            // The fallback model has no associated provider profile (mirrors the
            // overload-fallback re-issue, which passes `profile = None`).
            s.model_profile = None;
            prev
        };
        // User-visible warning. 2.1.206 `VPn(e,t,r)` =
        //   `${f_t(r) ? mmi(e) : hmi(e,r)} Switched to ${Mf(t)}. ${bxr(e)}`
        // for the common `category == "other"` path: `f_t("other")` is false, so
        // `hmi(e,"other")` fires with the generic `$7m` prefix ("This model's
        // safeguards flagged this message. This sometimes happens with safe,
        // normal conversations."); `bxr(e)` is the feedback line. `Mf(t)` = the
        // fallback's MARKETING NAME (byte-verified: 206 uses the friendly name,
        // not the raw id) — resolve it, falling back to the id for an unknown
        // model. (The cyber/bio `mmi(e)` "intentionally broad" variant needs the
        // refusal category routed through here — deferred with the typed
        // model_refusal_fallback system frame.)
        let fallback_display = crate::prompt::env_meta::marketing_name_for_model(&fallback)
            .map(String::from)
            .unwrap_or_else(|| fallback.clone());
        let warning = format!(
            "This model's safeguards flagged this message. \
This sometimes happens with safe, normal conversations. Switched to {fallback_display}. \
Send feedback with /feedback or learn more: https://support.claude.com/en/articles/15363606"
        );
        self.output.emit_text(&warning).await;
        // Success-path analytics — inline event name (NOT a locked telemetry
        // const), so the 347-entry `ALL_EVENT_NAMES` fixture lock is unperturbed.
        tracing::info!(
            event = "tengu_refusal_fallback_triggered",
            original_model = %original_model,
            fallback_model = %fallback,
            trigger = "refusal",
        );
        true
    }

    async fn inject_meta_user_message(&self, text: &str) {
        let msg = ConversationMessage::user(MessageId::new(), text.to_string());
        {
            let mut s = self.session.lock().await;
            s.history.push(msg.clone());
        }
        self.persist_message_to_jsonl(&msg).await;
    }

    async fn maybe_continue_for_budget(
        &self,
        budget: Option<&mut BudgetTracker>,
        recovery: &mut RecoveryState,
        global_turn_tokens: u64,
    ) -> bool {
        // No tracker → feature off / no budget → never continue (parity no-op).
        let Some(tracker) = budget else {
            return false;
        };
        // The orchestrator turn loop has no sub-agent `agentId` concept here
        // (that lives in the agent-spawn path); pass `None`, matching the main
        // query loop where `toolUseContext.agentId` is undefined for the root.
        let decision =
            check_token_budget(tracker, None, self.config.token_budget, global_turn_tokens);
        match decision {
            TokenBudgetDecision::Continue {
                nudge_message,
                continuation_count,
                pct,
                turn_tokens,
                budget,
            } => {
                tracing::info!(
                    event = "token_budget_continuation",
                    continuation_count,
                    pct,
                    turn_tokens,
                    budget,
                    "token budget continuation #{continuation_count}: {pct}% ({turn_tokens} / {budget})"
                );
                // Inject the continuation nudge as a meta user message. The
                // protocol carries no `isMeta` flag, so it is a plain user text
                // message with the byte-exact nudge string.
                let nudge_msg = ConversationMessage::user(MessageId::new(), nudge_message);
                {
                    let mut s = self.session.lock().await;
                    s.history.push(nudge_msg.clone());
                }
                self.persist_message_to_jsonl(&nudge_msg).await;
                // Reset the A1 recovery count on each budget continuation
                // (TS `query.ts:1332` `maxOutputTokensRecoveryCount: 0` +
                // `maxOutputTokensOverride: undefined`; REC.A1: a fresh recovery
                // episode may escalate again).
                recovery.reset_max_output_tokens_recovery();
                true
            }
            TokenBudgetDecision::Stop { completion_event } => {
                if let Some(ev) = completion_event {
                    if ev.diminishing_returns {
                        tracing::info!(
                            event = "token_budget_completed",
                            pct = ev.pct,
                            "token budget early stop: diminishing returns at {}%",
                            ev.pct
                        );
                    }
                    tracing::info!(
                        event = "token_budget_completed",
                        continuation_count = ev.continuation_count,
                        pct = ev.pct,
                        turn_tokens = ev.turn_tokens,
                        budget = ev.budget,
                        diminishing_returns = ev.diminishing_returns,
                        duration_ms = u64::try_from(ev.duration_ms).unwrap_or(u64::MAX),
                    );
                }
                false
            }
        }
    }

    /// Convert an in-memory `ConversationMessage` into a `JsonlMessage`.
    ///
    /// `parent_uuid` is the UUID of the prior persisted entry (None for the
    /// first turn). `cwd` is read from the LIVE `current_cwd()` cell (the
    /// post-`cd` shell cwd; falls back to `self.cwd` when no firer is wired).
    /// The `message` payload
    /// is the Anthropic-shaped inner object: for user/assistant we splat
    /// the content blocks via `serde_json::to_value` of the
    /// `ConversationMessage` and pull out the `content` array.
    ///
    /// Writer-field fidelity (§G gap 4) — mirrors `insertMessageChain`
    /// (`sessionStorage.ts:1039-1064`):
    /// - `git_branch`: the once-per-chain `getBranch()` value (`None` on a
    ///   non-repo), resolved by the caller and threaded in.
    /// - `entrypoint`: `getEntrypoint()` — `"cli"` for this engine (caller-supplied).
    /// - `prompt_id`: `getPromptId()` on `user` lines ONLY; `None` elsewhere. The
    ///   caller passes the in-flight turn's id and we apply it only to `user`.
    /// - `logical_parent_uuid`: compact-boundary back-link. The orchestrator's
    ///   append path is NOT a compaction boundary (compaction replays through a
    ///   separate engine), so this is always `None` here. See the
    ///   `persist_message_to_jsonl` note.
    pub(crate) fn to_jsonl_message(
        &self,
        msg: &ConversationMessage,
        session_id: &str,
        parent_uuid: Option<String>,
        git_branch: Option<String>,
        entrypoint: Option<String>,
        prompt_id: Option<String>,
    ) -> session::JsonlMessage {
        self.to_jsonl_message_with_inner_id(
            msg,
            session_id,
            parent_uuid,
            git_branch,
            entrypoint,
            prompt_id,
            None,
            None,
            None,
            None,
            None,
        )
    }

    /// As [`Self::to_jsonl_message`], but allows stamping a shared inner
    /// `message.id` on the persisted line.
    ///
    /// claude-code's streaming writer emits one JSONL line per
    /// `content_block_stop`, each carrying a DISTINCT top-level `uuid` but the
    /// SAME inner Anthropic `message.id` (the `message_start` message id shared
    /// across all blocks of the turn — `claude.ts:1981, 2192-2203`). That shared
    /// inner id is what the loader's parallel-tool-result recovery groups
    /// siblings by (`loader::message_id` → `loader::recover_orphaned_parallel_tool_results`).
    /// When `inner_message_id` is `Some`, it is injected into the assistant
    /// line's inner `message` object as `"id"`. `None` reproduces the prior
    /// (no inner id) shape exactly.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn to_jsonl_message_with_inner_id(
        &self,
        msg: &ConversationMessage,
        session_id: &str,
        parent_uuid: Option<String>,
        git_branch: Option<String>,
        entrypoint: Option<String>,
        prompt_id: Option<String>,
        inner_message_id: Option<&str>,
        // The real-response persist path supplies the response `model` and the
        // raw Anthropic `usage` object, which makes the assistant line carry the
        // full BetaMessage envelope (`{id,type,role,content,model,stop_reason,
        // stop_sequence,usage}`, matching claude-code + the golden fixtures).
        // Both `None` (the synthetic / user / system path) keeps the prior
        // `{role,content}` inner shape.
        assistant_model: Option<&str>,
        assistant_usage: Option<&serde_json::Value>,
        // The Anthropic `request-id` response header for a REAL assistant line
        // → the top-level `requestId` field (via `extra`). `None` (synthetic /
        // user / system) omits it, matching claude-code's `requestId: undefined`.
        request_id: Option<&str>,
        // When `Some`, this is a synthetic api-error assistant line: stamp the
        // top-level `isApiErrorMessage`/`error`/`apiErrorStatus` envelope fields
        // (via `extra`) and apply any inner `stop_reason` override. `None`
        // (every non-api-error line) leaves the shape exactly as before.
        api_error: Option<&ApiErrorEnvelope>,
    ) -> session::JsonlMessage {
        let (kind, mut inner_message) = match msg {
            ConversationMessage::User { content, .. } => (
                "user",
                serde_json::json!({ "role": "user", "content": content }),
            ),
            ConversationMessage::Assistant {
                content,
                stop_reason,
                ..
            } => {
                let inner = if let Some(model) = assistant_model {
                    // Build in the BetaMessage key order (`id` first); the block
                    // below re-stamps the shared `id` idempotently.
                    let mut m = serde_json::Map::new();
                    if let Some(id) = inner_message_id {
                        m.insert("id".to_string(), serde_json::Value::String(id.to_string()));
                    }
                    m.insert(
                        "type".to_string(),
                        serde_json::Value::String("message".to_string()),
                    );
                    m.insert(
                        "role".to_string(),
                        serde_json::Value::String("assistant".to_string()),
                    );
                    m.insert("content".to_string(), serde_json::json!(content));
                    m.insert(
                        "model".to_string(),
                        serde_json::Value::String(model.to_string()),
                    );
                    m.insert(
                        "stop_reason".to_string(),
                        stop_reason
                            .clone()
                            .map_or(serde_json::Value::Null, serde_json::Value::String),
                    );
                    m.insert("stop_sequence".to_string(), serde_json::Value::Null);
                    m.insert(
                        "usage".to_string(),
                        assistant_usage.cloned().unwrap_or(serde_json::Value::Null),
                    );
                    serde_json::Value::Object(m)
                } else {
                    // SYNTHETIC assistant line (no real response model/usage).
                    // LingXi's only synthetic assistant persist is the terminal
                    // API-error line (conversation.rs ~4399). claude-code builds
                    // these via baseCreateAssistantMessage (`QBl`) →
                    // createAssistantAPIErrorMessage (`tc`), which persists the
                    // synthetic BetaMessage envelope (v2.1.185 binary @205978440):
                    //   {id, container:null, model:"<synthetic>", role:"assistant",
                    //    stop_details:null, stop_reason:"stop_sequence",
                    //    stop_sequence:"", type:"message", content, context_management:null}
                    // `usage` is OMITTED — `tc` calls `QBl` with no `usage` arg,
                    // so `usage:undefined` drops the key under JSON.stringify.
                    // `stop_reason` is hardcoded `"stop_sequence"`; the real
                    // terminal reason lives in the OUTER apiError/error fields,
                    // which claude-code does NOT write into the persisted inner
                    // message (and which LingXi doesn't track). `model` is the
                    // `<synthetic>` sentinel (`WR`). The prior shape was a bare
                    // `{role,content}` — under-specified vs the binary.
                    let mut m = serde_json::Map::new();
                    m.insert(
                        "id".to_string(),
                        serde_json::Value::String(
                            inner_message_id
                                .map_or_else(|| msg.id().as_uuid().to_string(), str::to_string),
                        ),
                    );
                    m.insert("container".to_string(), serde_json::Value::Null);
                    m.insert(
                        "model".to_string(),
                        serde_json::Value::String("<synthetic>".to_string()),
                    );
                    m.insert(
                        "role".to_string(),
                        serde_json::Value::String("assistant".to_string()),
                    );
                    m.insert("stop_details".to_string(), serde_json::Value::Null);
                    m.insert(
                        "stop_reason".to_string(),
                        serde_json::Value::String(
                            // `ql`/`tc` leave the synthetic inner `stop_reason` as
                            // `"stop_sequence"`; the refusal `fje` path overrides
                            // it to `"refusal"` (verified on disk).
                            api_error
                                .and_then(|e| e.inner_stop_reason)
                                .unwrap_or("stop_sequence")
                                .to_string(),
                        ),
                    );
                    m.insert(
                        "stop_sequence".to_string(),
                        serde_json::Value::String(String::new()),
                    );
                    m.insert(
                        "type".to_string(),
                        serde_json::Value::String("message".to_string()),
                    );
                    m.insert("content".to_string(), serde_json::json!(content));
                    m.insert("context_management".to_string(), serde_json::Value::Null);
                    // `stop_reason` from the ConversationMessage is intentionally
                    // not used here (the synthetic envelope hardcodes it).
                    let _ = stop_reason;
                    serde_json::Value::Object(m)
                };
                ("assistant", inner)
            }
            ConversationMessage::System { content, .. } => (
                "system",
                serde_json::json!({ "role": "system", "content": content }),
            ),
        };
        // Stamp the shared inner Anthropic `message.id` on assistant lines so the
        // loader's sibling-grouping (by inner `message.id`) reconstructs the DAG.
        if let (Some(id), Some(obj)) = (inner_message_id, inner_message.as_object_mut()) {
            obj.insert("id".to_string(), serde_json::Value::String(id.to_string()));
        }
        // `promptId` is a USER-line-only field (TS: `type === 'user' ?
        // getPromptId() : undefined`). Drop it on assistant/system lines even
        // when the caller passes one.
        let prompt_id = if kind == "user" { prompt_id } else { None };
        // Use the raw UUID (8-4-4-4-12 lowercase), NOT the `msg.id().to_string()`
        // form which carries the `"msg:"` prefix — that prefix would break the
        // byte-equivalent JSONL schema (see `JsonlMessage::uuid` doc) and the
        // `validate_uuid` regex.
        //
        // `isMeta` is a TOP-LEVEL envelope field in claude-code (a sibling of
        // `message`/`uuid`, emitted at `utils/messages.ts:765,810` and read as an
        // outer field at `session/src/jsonl/title.rs:101`). Emit it ONLY for a
        // meta user message (default-`false` is omitted), so normal lines — and
        // every existing golden fixture — keep their exact byte shape.
        let mut extra = serde_json::Map::new();
        if msg.is_meta() {
            extra.insert("isMeta".to_string(), serde_json::Value::Bool(true));
        }
        // Top-level `requestId` (the Anthropic `request-id` response header) —
        // claude-code persists it on REAL assistant lines only. The caller
        // passes `Some` from the per-block real-response path; `None` (synthetic
        // / user / system) omits it, matching `requestId: undefined`.
        if let Some(rid) = request_id {
            extra.insert(
                "requestId".to_string(),
                serde_json::Value::String(rid.to_string()),
            );
        }
        // Top-level api-error envelope (`createAssistantAPIErrorMessage`/`fje`):
        // `error` (omitted when the builder took no `error:` arg), the always-on
        // `isApiErrorMessage: true`, and `apiErrorStatus` (set only for an
        // `APIError` with a numeric status). On disk these sit between
        // `requestId` and `userType`. These flow through the `extra` channel;
        // `JsonlMessage`'s hand-written `Serialize` (session/jsonl/schema.rs)
        // now places them in claude's EXACT per-kind outer-key order
        // (api-error head: type, uuid, timestamp, message, requestId?, error?,
        // isApiErrorMessage, apiErrorStatus?) — presence + values + ORDER are
        // 1:1. See [`ApiErrorEnvelope`].
        if let Some(ae) = api_error {
            if let Some(cat) = ae.error {
                extra.insert(
                    "error".to_string(),
                    serde_json::Value::String(cat.to_string()),
                );
            }
            extra.insert(
                "isApiErrorMessage".to_string(),
                serde_json::Value::Bool(true),
            );
            if let Some(status) = ae.api_error_status {
                extra.insert(
                    "apiErrorStatus".to_string(),
                    serde_json::Value::Number(status.into()),
                );
            }
        }
        session::JsonlMessage {
            message_type: kind.to_string(),
            uuid: msg.id().as_uuid().to_string(),
            parent_uuid,
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
            // Per-line cwd readback — the LIVE session cwd (advanced by a Bash
            // `cd` via the shared `current_cwd` cell), NOT the static init cwd.
            // 1:1 with claude-code, which stamps `getCwd()` on every persisted
            // line and where `cd` mutates that single global cwd. Falls back to
            // the static `cwd` when no firer is wired (the cell never moves).
            cwd: self.current_cwd().to_string_lossy().into_owned(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            message: inner_message,
            is_sidechain: false,
            user_type: Some("external".to_string()),
            git_branch,
            entrypoint,
            // Plan-slug cache is not wired in this engine — TS reads
            // `getPlanSlugCache().get(sessionId)`, which is `undefined` for any
            // session without a stored plan slug. We have no such cache, so this
            // is always omitted (matches the common TS path).
            slug: None,
            prompt_id,
            // Always `None` from this append path — see the doc comment above and
            // the `persist_message_to_jsonl` note.
            logical_parent_uuid: None,
            extra,
        }
    }

    /// Resolve the cwd's git branch ONCE and cache it — the parity analog of TS
    /// `getBranch()` (`sessionStorage.ts:1012-1019`), which is called per
    /// `insertMessageChain` and stamped on every line. We resolve lazily on the
    /// first append and memoize, so subsequent appends pay nothing.
    ///
    /// Reuses the loader's shell-git pattern (`std::process::Command`, no new
    /// dependency): `git rev-parse --abbrev-ref HEAD` in `self.cwd`. Returns
    /// `None` on ANY failure (git missing, not a repo, non-zero exit, detached
    /// HEAD reporting `"HEAD"`), matching TS's `try { getBranch() } catch {
    /// undefined }` — a `None` is then omitted from the JSONL line.
    async fn resolve_git_branch(&self) -> Option<String> {
        {
            let cache = self.git_branch_cache.lock().await;
            if let Some(resolved) = cache.as_ref() {
                return resolved.clone();
            }
        }
        let resolved = git_branch_for_cwd(&self.cwd);
        *self.git_branch_cache.lock().await = Some(resolved.clone());
        resolved
    }

    /// The stable per-turn `promptId` for `msg` — the parity analog of
    /// `getPromptId()` (`sessionStorage.ts:1045`).
    ///
    /// TS stamps the SAME prompt id on the user prompt line AND every
    /// `tool_result` `user` line of the turn. We reproduce that through the
    /// single append chokepoint: a genuine new user prompt (a `user` message
    /// that is NOT a `tool_result` carrier) MINTS a fresh UUID into
    /// `current_prompt_id`; a `tool_result` `user` line REUSES the cached id; any
    /// non-`user` message returns `None` (the caller / `to_jsonl_message` also
    /// guards this, so the field never lands on assistant/system lines).
    async fn prompt_id_for_message(&self, msg: &ConversationMessage) -> Option<String> {
        let ConversationMessage::User { content, .. } = msg else {
            // Non-user line — no promptId (mirrors `type === 'user' ? … :
            // undefined`). Leave the cached turn id untouched.
            return None;
        };
        let is_tool_result_carrier = content
            .iter()
            .any(|b| matches!(b, protocol::ContentBlock::ToolResult { .. }));
        let mut slot = self.current_prompt_id.lock().await;
        if is_tool_result_carrier {
            // Continuation of the in-flight turn — reuse the current id. If none
            // exists yet (defensive: a tool_result persisted before any prompt),
            // mint one so the field is still populated.
            if slot.is_none() {
                *slot = Some(uuid::Uuid::new_v4().to_string());
            }
        } else {
            // Genuine new user prompt — start a fresh prompt id for this turn.
            *slot = Some(uuid::Uuid::new_v4().to_string());
        }
        slot.clone()
    }

    /// Re-run a tool whose `can_use_tool` permission response was ORPHANED — the
    /// stdio control-plane received a late `control_response` it could not match
    /// to a pending request (a process restart with `--resume`, or a
    /// duplicate/late delivery). 1:1 with claude-code's `handleOrphanedPermission`
    /// (`queryHelpers.ts:224-343`), driven by the `mode:'orphaned-permission'`
    /// command (`print.ts:5291`).
    ///
    /// Locates the unresolved `tool_use` in the LIVE session history — which the
    /// CLI seeds from the transcript on `--resume`, so this mirrors claude-code's
    /// `findUnresolvedToolUse` over the transcript file while staying robust to
    /// compaction (a tool_use trimmed from the active context is not re-run) —
    /// forces the recovered permission `decision` past the gate, runs the tool
    /// through the SAME dispatch the normal turn loop uses, then appends +
    /// persists the resulting `tool_result`, completing the `tool_use →
    /// tool_result` chain.
    ///
    /// Returns `Ok(true)` when this id was handled (executed, OR short-circuited
    /// because its tool is no longer registered), `Ok(false)` when no unresolved
    /// `tool_use` with this id exists (already resolved, or absent). The caller
    /// records the id in its per-toolUseID "handled" set (twin of
    /// `handledOrphanedToolUseIds`) only on `Ok(true)`, so a not-found delivery
    /// can still recover later — matching claude-code (which adds to the set only
    /// when `findUnresolvedToolUse` succeeds).
    pub async fn run_orphaned_permission(
        &self,
        tool_use_id: &protocol::ToolUseId,
        decision: traits::permission_gate::PermissionOutcome,
    ) -> Result<bool, OrchestratorError> {
        use protocol::{ContentBlock, ConversationMessage, MessageId};

        // 1. Locate the orphaned assistant message and confirm its `tool_use` is
        //    UNRESOLVED (no matching `tool_result`) — `findUnresolvedToolUse`
        //    (sessionStorage.ts:4478-4519). Snapshot a clone under the lock.
        let found = {
            let s = self.session.lock().await;
            find_unresolved_tool_use_in_history(&s.history, tool_use_id)
        };
        let Some(assistant_msg) = found else {
            // Already resolved (a tool_result exists) or never present — no-op,
            // exactly like claude-code's `findUnresolvedToolUse → null`.
            return Ok(false);
        };

        // Extract the matching `tool_use` block (queryHelpers.ts:238-251). The
        // lookup guarantees one exists; bail defensively otherwise.
        let Some((name, input, provider_id)) = (match &assistant_msg {
            ConversationMessage::Assistant { content, .. } => {
                content.iter().find_map(|b| match b {
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input,
                        provider_id,
                    } if id == tool_use_id => {
                        Some((name.clone(), input.clone(), provider_id.clone()))
                    }
                    _ => None,
                })
            }
            _ => None,
        }) else {
            return Ok(false);
        };

        // Unknown-tool guard (queryHelpers.ts:256-259 `findToolByName → return`):
        // if the orphaned tool is no longer registered, emit NOTHING and push NO
        // tool_result, but still report recovery (`Ok(true)`) so the caller marks
        // this id handled — the TS sets `hasHandledOrphanedPermission` BEFORE
        // `handleOrphanedPermission` runs, so the gate is consumed even here.
        if self.tools.find_by_name(&name).is_none() {
            return Ok(true);
        }

        // 2. Re-emit the recovered assistant message as a self-contained stream
        //    frame (twin of `yield sdkAssistantMessage`, queryHelpers.ts:314-319)
        //    so a stream-json consumer sees the `tool_use` before its
        //    `tool_result`. Default-no-op sinks (TUI/tests) ignore the
        //    message_start/boundary envelope.
        {
            let model = self.session.lock().await.model.clone();
            let (msg_id, stop_reason) = match &assistant_msg {
                ConversationMessage::Assistant {
                    id, stop_reason, ..
                } => (id.to_string(), stop_reason.clone()),
                _ => (assistant_msg.id().to_string(), None),
            };
            self.output.emit_message_start(&msg_id, &model).await;
            if let ConversationMessage::Assistant { content, .. } = &assistant_msg {
                for b in content {
                    match b {
                        ContentBlock::Text { text } => self.output.emit_text(text).await,
                        ContentBlock::ToolUse {
                            id, name, input, ..
                        } => self.output.emit_tool_call(id, name, input).await,
                        ContentBlock::Thinking {
                            thinking,
                            signature,
                        } => {
                            self.output
                                .emit_thinking(thinking, signature.as_deref())
                                .await;
                        }
                        _ => {}
                    }
                }
            }
            self.output
                .emit_message_boundary(stop_reason.as_deref(), None)
                .await;
        }

        // 4. Apply `updatedInput` on an allow (queryHelpers.ts:262-276): an allow
        //    carries the host's possibly-rewritten input; a deny keeps the
        //    original (it will not run anyway).
        let (forced, final_input) = match decision {
            traits::permission_gate::PermissionOutcome::Allow { updated_input, .. } => (
                crate::test_support::PermissionDecision::Allow,
                updated_input.unwrap_or(input),
            ),
            traits::permission_gate::PermissionOutcome::Deny { reason } => (
                crate::test_support::PermissionDecision::Deny { reason },
                input,
            ),
        };

        // The orphaned assistant message is ALREADY in history (we found it
        // there), so — like claude-code's `alreadyPresent` guard
        // (queryHelpers.ts:299-312) — it is NOT re-pushed or re-persisted; only
        // the new `tool_result` below is appended.

        // 5. Force the recovered decision past the permission gate for this one
        //    `tool_use`, then run it through the SAME dispatch the normal turn
        //    loop uses (the `runTools` analog). The gate consumes (removes) the
        //    forced entry; clear any residue defensively.
        self.orphan_forced_decisions
            .lock()
            .await
            .insert(tool_use_id.clone(), forced);
        let tool_uses = vec![(tool_use_id.clone(), name, final_input, provider_id)];
        let dispatch_result =
            crate::turn_loop::dispatch_tool_uses_tracked(self, &tool_uses, None).await;
        self.orphan_forced_decisions
            .lock()
            .await
            .remove(tool_use_id);
        let (tool_results, _prevent, injected_messages, context_modifiers) = dispatch_result?;

        // 6. Append + persist the `tool_result` user message and any
        //    tool-injected follow-ups — mirroring the batched turn loop's
        //    post-dispatch block (`execute_one_turn`), the `recordTranscript`
        //    per-result twin (queryHelpers.ts:328-332).
        let tool_results_msg = ConversationMessage::User {
            id: MessageId::new(),
            content: tool_results,
            is_meta: false,
        };
        {
            let mut s = self.session.lock().await;
            s.history.push(tool_results_msg.clone());
            for (m, source_id) in &injected_messages {
                s.history.push(m.clone());
                s.injected_message_sources.insert(m.id(), source_id.clone());
            }
        }
        self.persist_message_to_jsonl(&tool_results_msg).await;
        for (m, _source_id) in &injected_messages {
            self.persist_message_to_jsonl(m).await;
        }
        crate::turn_loop::apply_model_context_modifiers(self, context_modifiers).await;

        Ok(true)
    }

    /// Persist a single message to the optional JSONL writer.
    ///
    /// Best-effort: write failures are logged via the telemetry
    /// `tengu_session_corrupted` event but never fail the turn. On
    /// success, emits `tengu_session_appended` and updates the
    /// `last_jsonl_uuid` cache.
    pub(crate) async fn persist_message_to_jsonl(&self, msg: &ConversationMessage) {
        self.persist_message_to_jsonl_with_parent(msg, None).await;
    }

    /// Persist with an optional explicit `parentUuid` override.
    ///
    /// The streaming executor passes the originating assistant message's UUID so
    /// each tool result parents to the assistant that requested it (TS
    /// `sourceToolAssistantUUID`), rather than the linear `last_jsonl_uuid` chain.
    ///
    /// When `parent_override` is `None`, behaves exactly as before (chain off
    /// `last_jsonl_uuid`). In BOTH cases the `last_jsonl_uuid` cache is advanced
    /// to this line's UUID so any subsequent non-overridden line chains correctly.
    pub(crate) async fn persist_message_to_jsonl_with_parent(
        &self,
        msg: &ConversationMessage,
        parent_override: Option<String>,
    ) {
        self.persist_message_to_jsonl_inner(msg, parent_override, None)
            .await;
    }

    /// Persist a synthetic api-error assistant line, stamping the top-level
    /// `isApiErrorMessage`/`error`/`apiErrorStatus` envelope (and any inner
    /// `stop_reason` override) from `env`. 1:1 with claude-code's
    /// `createAssistantAPIErrorMessage` (`ql`/`tc`) and refusal (`fje`) lines.
    pub(crate) async fn persist_api_error_message_to_jsonl(
        &self,
        msg: &ConversationMessage,
        env: ApiErrorEnvelope,
    ) {
        self.persist_message_to_jsonl_inner(msg, None, Some(env))
            .await;
    }

    /// Shared append body for [`Self::persist_message_to_jsonl_with_parent`] and
    /// [`Self::persist_api_error_message_to_jsonl`].
    async fn persist_message_to_jsonl_inner(
        &self,
        msg: &ConversationMessage,
        parent_override: Option<String>,
        api_error: Option<ApiErrorEnvelope>,
    ) {
        let Some(writer) = self.jsonl_writer.as_ref() else {
            return;
        };
        let (session_id_str, parent_uuid) = {
            let session_id = self.session.lock().await.session_id;
            let parent = match parent_override {
                Some(p) => Some(p),
                None => self.last_jsonl_uuid.lock().await.clone(),
            };
            (session_id.to_string(), parent)
        };
        // Writer-field fidelity (§G gap 4):
        // - gitBranch: once-per-session `getBranch()` (cached).
        // - entrypoint: `getEntrypoint()` → `CLAUDE_CODE_ENTRYPOINT` env or "cli"
        //   (mirrors the UA builder, `model/user_agent.rs:73`).
        // - promptId: `getPromptId()` on USER lines. We mint a fresh id when this
        //   is a genuine new user prompt and reuse it for the turn's tool_result
        //   `user` lines, matching TS where `getPromptId()` is stable across a
        //   turn. `to_jsonl_message` drops it on non-user lines.
        let git_branch = self.resolve_git_branch().await;
        let entrypoint = Some(entrypoint_value());
        let prompt_id = self.prompt_id_for_message(msg).await;
        let jmsg = self.to_jsonl_message_with_inner_id(
            msg,
            &session_id_str,
            parent_uuid,
            git_branch,
            entrypoint,
            prompt_id,
            None,
            None,
            None,
            None,
            api_error.as_ref(),
        );
        let uuid_for_chain = jmsg.uuid.clone();
        match writer.append(&jmsg).await {
            Ok(()) => {
                *self.last_jsonl_uuid.lock().await = Some(uuid_for_chain.clone());
                telemetry::emit_session_appended(&session_id_str, &uuid_for_chain);
            }
            Err(e) => {
                tracing::error!(error = %e, "jsonl writer append failed");
                telemetry::emit_session_corrupted(&session_id_str, &e.to_string());
            }
        }
    }

    /// Persist an assistant turn as ONE single-block JSONL line PER content block
    /// (claude-code's per-`content_block_stop` writer — `claude.ts:2171-2211`).
    ///
    /// claude-code builds an `AssistantMessage` at each `content_block_stop` from
    /// a SINGLE content block (`content: normalizeContentFromAPI([contentBlock])`)
    /// with a FRESH top-level `uuid: randomUUID()` but the SAME inner
    /// `message.id` shared across all blocks of the turn. So an assistant turn
    /// `[text, tool_use A, tool_use B]` becomes THREE assistant JSONL lines: one
    /// shared inner `message.id`, three distinct top-level `uuid`s, one block each.
    ///
    /// This is a WRITE-side (transcript) split ONLY — the caller keeps the single
    /// merged `ConversationMessage::Assistant` in `session.history` for
    /// request-building (the Anthropic request needs one assistant turn carrying
    /// all blocks). We mint a fresh [`MessageId`] per block so each line gets a
    /// distinct top-level `uuid` (via [`Self::to_jsonl_message`], whose `uuid`
    /// derives from `msg.id().as_uuid()`), and inject the originating turn's id
    /// (`msg.id().as_uuid()`) as the shared inner `message.id` so the loader's
    /// sibling-grouping reconstructs the DAG.
    ///
    /// Returns a `tool_use_id -> that block's line uuid` map so the caller can
    /// parent EACH `tool_result` to ITS specific `tool_use` line (TS
    /// `sourceToolAssistantUUID`), not one shared per-turn parent. On an
    /// assistant with no `tool_use` blocks the map is empty. The lines chain off
    /// `last_jsonl_uuid` (advancing it per line), so the LAST block's uuid ends
    /// up as `last_jsonl_uuid` and any subsequent non-tool message chains
    /// correctly. An empty-content assistant persists nothing (no line, empty
    /// map) — faithful to streaming, which never emits a zero-block turn.
    pub(crate) async fn persist_assistant_per_block(
        &self,
        msg: &ConversationMessage,
        // Raw Anthropic `usage` object for the BetaMessage envelope (the codec's
        // `Usage::provider_metadata`); `None` writes `usage: null`.
        usage: Option<&serde_json::Value>,
        // The Anthropic `request-id` response header for this turn → the
        // top-level `requestId` on every per-block assistant line. `None` when
        // the adapter recorded no request-id (e.g. a mock that does not surface
        // headers) — the line then omits `requestId`, like claude-code.
        request_id: Option<&str>,
    ) -> std::collections::HashMap<protocol::ToolUseId, String> {
        let mut map: std::collections::HashMap<protocol::ToolUseId, String> =
            std::collections::HashMap::new();
        let ConversationMessage::Assistant {
            id: turn_id,
            content,
            stop_reason,
        } = msg
        else {
            // Defensive: non-assistant messages fall back to the normal single
            // line (no split applies). Should not happen in practice.
            self.persist_message_to_jsonl(msg).await;
            return map;
        };
        let Some(writer) = self.jsonl_writer.as_ref() else {
            return map;
        };
        // Shared inner Anthropic `message.id` for every block of this turn.
        let inner_id = turn_id.as_uuid().to_string();

        let (session_id_str, model) = {
            let s = self.session.lock().await;
            (s.session_id.to_string(), s.model.clone())
        };
        let git_branch = self.resolve_git_branch().await;
        let entrypoint = Some(entrypoint_value());

        for block in content {
            // Build a synthetic SINGLE-block assistant message with a FRESH id so
            // its top-level JSONL `uuid` is distinct per line.
            let single = ConversationMessage::Assistant {
                id: MessageId::new(),
                content: vec![block.clone()],
                stop_reason: stop_reason.clone(),
            };
            let parent_uuid = self.last_jsonl_uuid.lock().await.clone();
            // Assistant lines never carry a promptId (it is a user-only field).
            let jmsg = self.to_jsonl_message_with_inner_id(
                &single,
                &session_id_str,
                parent_uuid,
                git_branch.clone(),
                entrypoint.clone(),
                None,
                Some(&inner_id),
                Some(&model),
                usage,
                request_id,
                None,
            );
            let line_uuid = jmsg.uuid.clone();
            match writer.append(&jmsg).await {
                Ok(()) => {
                    *self.last_jsonl_uuid.lock().await = Some(line_uuid.clone());
                    telemetry::emit_session_appended(&session_id_str, &line_uuid);
                }
                Err(e) => {
                    tracing::error!(error = %e, "jsonl writer append failed");
                    telemetry::emit_session_corrupted(&session_id_str, &e.to_string());
                    // Skip recording this block's uuid in the map — the caller's
                    // fallback (prior single-parent) will be used for any
                    // tool_result that can't find its parent.
                    continue;
                }
            }
            if let protocol::ContentBlock::ToolUse { id, .. } = block {
                map.insert(id.clone(), line_uuid);
            }
        }
        map
    }

    /// Persist a NON-streaming (batched) assistant turn as ONE merged JSONL line
    /// carrying the full BetaMessage envelope — the real `model` (the live
    /// session model, same source [`Self::persist_assistant_per_block`] uses),
    /// the Anthropic `usage` object, and the `requestId`. This is the batched
    /// counterpart of `persist_assistant_per_block`: claude-code's non-streaming
    /// handler (`claude.ts:2571`) emits one merged `AssistantMessage` WITH the
    /// response model/usage, and the batched `run_turn` path must match.
    ///
    /// Previously the batched path used the model-less
    /// [`Self::persist_message_to_jsonl`], so every real `--print` / `--bg`
    /// reply was recorded as `model:"<synthetic>"` with `usage` dropped (cost
    /// lost, telemetry/resume mis-attributed) even though the API call
    /// succeeded. Genuine SYNTHETIC api-error lines still use the model-less
    /// path (`persist_api_error_message_to_jsonl` / `persist_message_to_jsonl`).
    pub(crate) async fn persist_assistant_merged(
        &self,
        msg: &ConversationMessage,
        // Typed response usage → the Anthropic `usage` JSON (via
        // `assistant_usage_value`). `None` writes `usage: null`.
        usage: Option<&llm_client::Usage>,
        request_id: Option<&str>,
    ) {
        let ConversationMessage::Assistant { id: turn_id, .. } = msg else {
            // Defensive: non-assistant messages take the plain single-line path.
            self.persist_message_to_jsonl(msg).await;
            return;
        };
        let Some(writer) = self.jsonl_writer.as_ref() else {
            return;
        };
        let inner_id = turn_id.as_uuid().to_string();
        let (session_id_str, model) = {
            let s = self.session.lock().await;
            (s.session_id.to_string(), s.model.clone())
        };
        let git_branch = self.resolve_git_branch().await;
        let entrypoint = Some(entrypoint_value());
        let parent_uuid = self.last_jsonl_uuid.lock().await.clone();
        let usage_json = usage.map(assistant_usage_value);
        let jmsg = self.to_jsonl_message_with_inner_id(
            msg,
            &session_id_str,
            parent_uuid,
            git_branch,
            entrypoint,
            None,
            Some(&inner_id),
            Some(&model),
            usage_json.as_ref(),
            request_id,
            None,
        );
        let line_uuid = jmsg.uuid.clone();
        match writer.append(&jmsg).await {
            Ok(()) => {
                *self.last_jsonl_uuid.lock().await = Some(line_uuid.clone());
                telemetry::emit_session_appended(&session_id_str, &line_uuid);
            }
            Err(e) => {
                tracing::error!(error = %e, "jsonl writer append failed");
                telemetry::emit_session_corrupted(&session_id_str, &e.to_string());
            }
        }
    }

    /// Construct a new orchestrator with a fresh in-memory session and
    /// the BATCHED API client only — the streaming field is wired with
    /// the [`NoStreamingApiClient`] stub so any `run_turn_streaming`
    /// call surfaces "no streaming client configured" rather than
    /// panicking. M5-02 / M5-03 callers continue to use this signature
    /// unchanged.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        config: OrchestratorConfig,
        api: Arc<dyn OrchestratorApiClient>,
        tools: Arc<ToolRegistry>,
        hooks: Arc<HookExecutor>,
        perms: Arc<dyn PermissionGate>,
        output: Arc<dyn OutputStream>,
        memory: Arc<dyn crate::prompt::MemoryHierarchyProvider>,
        cwd: std::path::PathBuf,
    ) -> Self {
        Self::new_with_streaming(
            config,
            api,
            Arc::new(NoStreamingApiClient),
            tools,
            hooks,
            perms,
            output,
            memory,
            cwd,
        )
    }

    /// Schedule a best-effort startup Responses WebSocket prewarm.
    ///
    /// The prewarm uses the current model/profile, assembled system prompt, and
    /// current tool list with an empty conversation history. A later first turn
    /// can then reuse the returned `response.id` when its request is a strict
    /// extension of this prefix. Failures are intentionally ignored.
    pub fn spawn_startup_responses_websocket_prewarm(self: &Arc<Self>) {
        self.abort_startup_responses_websocket_prewarm();
        let orchestrator = Arc::clone(self);
        let handle = tokio::spawn(async move {
            let (model, profile) = {
                let session = orchestrator.session.lock().await;
                (session.model.clone(), session.model_profile.clone())
            };
            let system = orchestrator.build_system_prompt().await;
            let tools = orchestrator.build_wire_tools().await;
            let _ = orchestrator
                .api
                .prewarm_responses_websocket(
                    &model,
                    profile.as_deref(),
                    Some(&system),
                    Vec::new(),
                    tools,
                )
                .await;
        });
        *self
            .startup_responses_websocket_prewarm
            .lock()
            .expect("startup responses websocket prewarm") = Some(handle);
    }

    /// Abort any pending startup Responses WebSocket prewarm task.
    pub fn abort_startup_responses_websocket_prewarm(&self) {
        if let Some(handle) = self
            .startup_responses_websocket_prewarm
            .lock()
            .expect("startup responses websocket prewarm")
            .take()
        {
            handle.abort();
        }
    }

    /// Task 6 (llm-client future-work batch 5): re-map a terminal
    /// `RateLimited` error onto the limits-specific copy the API client
    /// composed from the 429's own unified headers (claude-code
    /// `errors.ts:480-524`). Applied by every public turn driver so each
    /// consumer of the error's `Display` (CLI stderr, TUI scrollback,
    /// `client-adapter` `ClientEvent::Error`) sees the
    /// `"You've hit your … limit · resets …"` copy instead of the generic
    /// `"api call failed: rate limited"`. No-op for non-429 errors and when
    /// the 429 carried no unified headers.
    /// Text for a graceful `model_error` surface. Verbatim for parity EXCEPT a
    /// `ModelUnavailable` (a provider 404): enrich it with the model id and a
    /// `/model` hint so the user knows WHICH model the provider rejected and how
    /// to switch — the bare "model unavailable" was opaque, especially with a
    /// stale third-party catalog entry (multi-provider UX).
    pub(crate) async fn model_error_text(&self, err: &LlmError) -> String {
        match err {
            LlmError::ModelUnavailable => {
                let model = self.session.lock().await.model.clone();
                format!(
                    "model unavailable: the provider does not serve '{model}' (HTTP 404). Pick another model with /model."
                )
            }
            other => other.to_string(),
        }
    }

    fn enrich_api_error(&self, err: OrchestratorError) -> OrchestratorError {
        enrich_rate_limited_error(err, self.api.last_rate_limit_error_message())
    }

    /// B6-T1: status-change emit parity for a turn that DIED on a rate limit.
    ///
    /// claude-code's terminal catch handler `extractQuotaStatusFromError`
    /// (claudeAiLimits.ts:487) forces the limits to `status='rejected'` and
    /// runs `emitStatusChange` (ts:509-511) ALONGSIDE rendering the terminal
    /// error copy — so the TUI shows the rate-limit banner (+ the T5 overage
    /// notice) next to the assistant error message. By the time a turn driver
    /// surfaces a terminal `RateLimited`, the drive fn has already PROMOTED
    /// the staged 429 into `self.api`'s caches (via
    /// [`crate::provider_adapter::ProviderApiAdapter::promote_pending_429`]),
    /// so these emit-on-change helpers flow the rejected snapshot (+ raw
    /// windows) out as an [`traits::OutputEvent::RateLimit`] (+ `RawUtilization`).
    ///
    /// Gated on the rate-limited discriminant a terminal error carries BEFORE
    /// enrichment — `ApiCall(RateLimited)` (batched) or `Streaming(RateLimited)`
    /// (stream connect-phase) — so a non-429 terminal never emits. Called
    /// AFTER the drive fn returned (promotion done) and BEFORE
    /// [`Self::enrich_api_error`].
    ///
    /// B1 — DOCUMENTED DIVERGENCE (not parity): TS forces `status='rejected'`
    /// and emits even on a HEADERLESS terminal 429 (claudeAiLimits.ts:506-507,
    /// outside the headers block). The Rust `last_rate_limit` has no "bare
    /// rejected, no windows" representation, so a headerless terminal 429
    /// promotes nothing and these emit-on-change helpers are no-ops — the
    /// terminal error copy already conveys the rejection.
    async fn emit_terminal_rate_limit_if_changed<T>(&self, result: &Result<T, OrchestratorError>) {
        if let Err(
            OrchestratorError::ApiCall(LlmError::RateLimited { .. })
            | OrchestratorError::Streaming(LlmError::RateLimited { .. }),
        ) = result
        {
            self.emit_rate_limit_if_changed().await;
            self.emit_raw_utilization_if_changed().await;
        }
    }

    /// Drive one user prompt through the turn loop until `end_turn` or
    /// `max_turns` is exhausted.
    ///
    /// Emits 3 telemetry events:
    /// - [`orch_events::CONVERSATION_STARTED`] at entry
    /// - [`orch_events::CONVERSATION_COMPLETED`] on success
    /// - [`orch_events::CONVERSATION_FAILED`] on error
    pub async fn run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError> {
        tracing::info!(
            event = orch_events::CONVERSATION_STARTED,
            prompt_len = prompt.len()
        );
        let result = self.try_run_turn(prompt).await;
        self.emit_terminal_rate_limit_if_changed(&result).await;
        let result = result.map_err(|e| self.enrich_api_error(e));
        // ConversationOutcome is #[non_exhaustive] so future variants will
        // also log as Completed when the only existing variant is EndTurn.
        match &result {
            Ok(
                ConversationOutcome::EndTurn { turn_count, .. }
                | ConversationOutcome::StopHookPrevented { turn_count, .. },
            ) => {
                tracing::info!(
                    event = orch_events::CONVERSATION_COMPLETED,
                    turn_count = *turn_count
                );
            }
            Err(err) => {
                tracing::error!(
                    event = orch_events::CONVERSATION_FAILED,
                    reason = %err
                );
            }
        }
        result
    }

    /// Internal turn driver (no telemetry — wrapped by `run_turn`).
    /// Build the lifecycle `HookContext` for this conversation (hooks B4),
    /// mirroring the `PreToolUse` context construction in `turn_loop.rs` plus
    /// the B4 additive fields.
    async fn lifecycle_hook_ctx(&self, stop_hook_active: bool) -> HookContext {
        // FIX 2: populate `transcript_path` + `permission_mode` on the lifecycle
        // hook context, matching claude-code `createBaseHookInput` (always sets
        // `transcript_path`, utils/hooks.ts:322) and the plan/default approximation
        // of the session's permission mode. The lifecycle hooks (Stop /
        // UserPromptSubmit / SessionStart / …) thus carry a non-empty
        // `transcript_path` like the tool-use hooks do, instead of serializing `""`.
        //
        // FIX A: the path is the live JSONL writer's path when one is wired
        // (preserves the writer-backed tests) ELSE the deterministically-computed
        // `<config_home>/projects/<sanitize(cwd)>/<uuid>.jsonl` — because in
        // PRODUCTION no writer is wired, so the prior `unwrap_or_default()` left
        // `transcript_path` EMPTY for every lifecycle hook. `computed_transcript_path`
        // is itself `""` only when neither a writer nor a `config_home` is present
        // (library/test builds), preserving the old behavior there.
        let (session_id, plan_mode) = {
            let s = self.session.lock().await;
            (s.session_id, s.plan_mode)
        };
        let transcript_path = self
            .jsonl_writer
            .as_ref()
            .map(|w| w.path().to_path_buf())
            .unwrap_or_else(|| self.computed_transcript_path(&session_id));
        HookContext {
            session_id,
            cwd: self.current_cwd(),
            transcript_path,
            permission_mode: Some(if plan_mode { "plan" } else { "default" }.to_string()),
            stop_hook_active,
            ..Default::default()
        }
    }

    /// Stamp the `Stop` / `SubagentStop` `background_tasks` + `session_crons`
    /// snapshot onto a lifecycle [`HookContext`] (claude-code's `m = s ? {
    /// background_tasks: Lic(s.taskRegistry.all()), session_crons: Mic() } :
    /// undefined`, spread `...m` after `last_assistant_message`).
    ///
    /// Called ONLY at the `Stop` / `SubagentStop` firings — never on other
    /// lifecycle hooks (`UserPromptSubmit`, expansion, …) — mirroring claude's
    /// gate on the tool-use context `s`. When a snapshot provider is wired BOTH
    /// fields become `Some(vec)` (possibly empty `[]`, which claude STILL emits
    /// when `s` is present); when no provider is wired both stay `None` and the
    /// executor omits the keys (claude `m = undefined`), keeping no-registry
    /// builds byte-identical.
    pub(crate) async fn populate_stop_hook_snapshot(&self, ctx: &mut HookContext) {
        if let Some(provider) = self.stop_hook_snapshot.as_ref() {
            ctx.background_tasks = Some(provider.background_tasks().await);
            ctx.session_crons = Some(provider.session_crons().await);
        }
    }

    /// Public lifecycle [`HookContext`] for collaborators that fire engine
    /// hooks OUTSIDE the turn loop — notably the slash-command dispatcher,
    /// which fires `UserPromptExpansion` (#39) at command expansion and needs
    /// this conversation's `session_id` + `cwd` to populate the base hook input
    /// (claude-code minified `vd`). Same shape as the in-loop lifecycle hooks
    /// (`stop_hook_active = false`), so the dispatcher's payload matches the
    /// orchestrator's own lifecycle firings byte-for-byte on the shared fields.
    pub async fn expansion_hook_context(&self) -> HookContext {
        self.lifecycle_hook_ctx(false).await
    }

    /// Fire the `UserPromptSubmit` lifecycle hooks at prompt ingress (hooks B4,
    /// TS `executeUserPromptSubmitHooks` / `query.ts` prompt path). Returns
    /// `true` when a hook returned a `Block` decision, signalling the caller to
    /// ABORT the turn before any API call. Strict no-op (returns `false`) when
    /// no matching hook is registered, so existing flows are unaffected.
    async fn fire_user_prompt_submit(&self, prompt: &str) -> bool {
        let ctx = self.lifecycle_hook_ctx(false).await;
        let agg = self
            .hooks
            .execute(
                HookEvent::UserPromptSubmit {
                    prompt: prompt.to_string(),
                },
                ctx,
            )
            .await;
        matches!(agg.decision, Some(hooks::response::HookDecision::Block))
    }

    /// Fire the `MessageDisplay` hooks at the BEGIN of an assistant-message
    /// stream — the orchestrator twin of claude-code's stream-display state
    /// machine `begin(d)` (BIN off 208862320), which, on each new assistant
    /// message, checks the `MessageDisplay` gate and then initializes the
    /// per-message flush state `o={apiMessageId:d, messageId:randomUUID(),
    /// turnId:r, index:0, ...}`. The hook payload is built by `aAt` (BIN off
    /// 205705090): `{hook_event_name:"MessageDisplay", turn_id:e.turnId,
    /// message_id:e.messageId, index:e.index, final:e.final, delta:e.delta}`.
    ///
    /// LingXi's streaming pump emits text deltas through the `OutputStream`
    /// rather than re-entering a hook-aware flush loop, so we fire ONCE per
    /// assistant message at its begin (the single hook-reachable point that
    /// owns the executor + the pre-allocated assistant id), mirroring the
    /// `begin(d)` initialization with `index:0`, `final:false`, and an empty
    /// `delta` (no delta text has streamed yet at begin). `turn_id` is a fresh
    /// per-turn UUID minted here (claude-code `newTurn(){…; r=randomUUID()}`),
    /// and `message_id` is the assistant message's bare UUID (no `msg:` prefix,
    /// matching the binary's bare `randomUUID()` display id and the inner
    /// Anthropic `message.id` formatting used by `persist_assistant_per_block`).
    ///
    /// Best-effort: the aggregate is discarded so a failing / blocking
    /// `MessageDisplay` hook never affects the turn, and firing is a strict
    /// no-op when no `MessageDisplay` hook is registered.
    async fn fire_message_display(&self, turn_id: &str, assistant_id: MessageId) {
        let ctx = self.lifecycle_hook_ctx(false).await;
        let _ = self
            .hooks
            .execute(
                HookEvent::MessageDisplay {
                    turn_id: turn_id.to_string(),
                    message_id: assistant_id.as_uuid().to_string(),
                    index: 0,
                    is_final: false,
                    delta: String::new(),
                },
                ctx,
            )
            .await;
    }

    /// Fire the `Stop` lifecycle hooks at end-of-turn and classify the result
    /// (hooks B4, TS `handleStopHooks` + `query.ts:1267-1306`).
    ///
    /// `stop_hook_active` carries the re-entry flag into the hook payload AND
    /// gates the continuation guard: a `Block` when already active becomes
    /// [`StopHookDisposition::Pass`] so a perpetually-blocking Stop hook cannot
    /// loop forever. `prevent_continuation` (`continue:false`) always wins →
    /// [`StopHookDisposition::Prevent`]. Strict no-op (`Pass`) when no Stop hook
    /// is registered.
    async fn fire_stop_hooks(&self, reason: &str, stop_hook_active: bool) -> StopHookDisposition {
        tracing::debug!(event = "hook_stop_started", reason, stop_hook_active);
        let mut ctx = self.lifecycle_hook_ctx(stop_hook_active).await;
        // claude-code stamps `background_tasks` + `session_crons` onto the Stop
        // payload whenever the tool-use context is present (`...m`). The
        // orchestrator's main-loop Stop firing always runs inside a tool-use
        // context, so populate the snapshot here (and ONLY here / SubagentStop).
        self.populate_stop_hook_snapshot(&mut ctx).await;
        let agg = self
            .hooks
            .execute(
                HookEvent::Stop {
                    reason: reason.to_string(),
                },
                ctx,
            )
            .await;
        let disposition = if agg.prevent_continuation {
            // FIX C: carry the hook's `stopReason` (parsed into `agg.reason`,
            // `hook_payload.rs:1113`) so `handle_stop_at_end` can persist the
            // `hook_stopped_continuation` meta message. Default matches TS
            // `query/stopHooks.ts:271` (`result.stopReason || 'Stop hook prevented
            // continuation'`).
            let reason = agg
                .reason
                .clone()
                .unwrap_or_else(|| "Stop hook prevented continuation".to_string());
            StopHookDisposition::Prevent(reason)
        } else if matches!(agg.decision, Some(hooks::response::HookDecision::Block)) {
            // #2: ANY Block yields Continue — the consecutive-block CAP is no
            // longer the old `!stop_hook_active` boolean (which let a blocking
            // hook drive exactly ONE extra turn). The binary carries a
            // `stopHookBlockingCount` and only ends after it exceeds
            // `LINGXI_STOP_HOOK_BLOCK_CAP` (default 8); that counter cap is
            // enforced in `handle_stop_at_end`, not here. `stop_hook_active` is
            // still threaded into the hook CONTEXT above so the hook can read it
            // and return success while it is true (the documented escape hatch).
            //
            // Source the continuation from the hook's blocking REASON
            // (`blockingError.blockingError`), NOT the transcript-only
            // `system_messages`. `agg.reason` is `None` when the hook omits a
            // reason, so apply claude-code's default (`utils/hooks.ts:533`,
            // `json.reason || 'Blocked by hook'`).
            let reason = agg
                .reason
                .clone()
                .unwrap_or_else(|| "Blocked by hook".to_string());
            StopHookDisposition::Continue(reason)
        } else {
            StopHookDisposition::Pass
        };
        tracing::debug!(
            event = "hook_stop_completed",
            prevent_continuation = agg.prevent_continuation,
            blocked = matches!(agg.decision, Some(hooks::response::HookDecision::Block)),
        );
        disposition
    }

    /// Fire the `StopFailure` lifecycle hooks when a turn ends on an API error
    /// (RECOV.2, TS `executeStopFailureHooks`, `query.ts:1174/1181/1263`).
    ///
    /// Distinct from [`Self::fire_stop_hooks`]: the model never produced a real
    /// response, so the normal `Stop` hooks are skipped (they would create a
    /// death spiral — error → hook blocking → retry → error → …) and the
    /// `StopFailure` event fires instead. Fire-and-forget + best-effort exactly
    /// like the other lifecycle fires: the aggregate is discarded (TS calls
    /// `executeStopFailureHooks` as `void` — a `StopFailure` hook can neither
    /// block nor continue the turn). Strict no-op when no `StopFailure` hook is
    /// registered. `error` is the api-error discriminator carried verbatim into
    /// the wire payload's `error` field (TS `lastMessage.error`).
    async fn fire_stop_failure(&self, error: &str) {
        tracing::debug!(event = "hook_stop_failure_started", error);
        let ctx = self.lifecycle_hook_ctx(false).await;
        let _ = self
            .hooks
            .execute(
                HookEvent::StopFailure {
                    error: error.to_string(),
                },
                ctx,
            )
            .await;
    }

    /// Fire Stop hooks at a natural end-of-turn arm and translate the
    /// disposition into a driver control-flow directive (hooks B4). Shared by
    /// all three turn drivers. Skips firing (returns `FallThrough`) on the
    /// `prompt_too_long` error surface — the port of the skip-on-API-error guard
    /// (`query.ts:1262`). On `Prevent` it emits the end-turn before terminating
    /// so the cost/UI bookkeeping still fires.
    async fn handle_stop_at_end(
        &self,
        stop_reason: &str,
        stop_hook_active: &mut bool,
        stop_hook_blocking_count: &mut u32,
        turn_count: u32,
        final_message_id: MessageId,
    ) -> StopHookFlow {
        if matches!(
            stop_reason,
            "prompt_too_long" | "blocking_limit" | "rapid_refill_breaker"
        ) {
            // RECOV.2: this turn ended on an API error — the model never produced
            // a real response, so fire the `StopFailure` hooks (NOT the `Stop`
            // hooks) before ending. 1:1 with TS `query.ts:1262-1264` (the
            // api-error skip guard runs `executeStopFailureHooks(lastMessage)` then
            // returns) and `query.ts:1174/1181` (PTL recovery exhausted). Running
            // the normal `Stop` hooks here would risk the death-spiral TS warns
            // against (error → hook blocking → retry → error → …). The wire
            // `error` is `"invalid_request"` for ALL THREE — the proactive
            // blocking-limit preempt and the rapid-refill breaker both surface an
            // assistant message whose api-error field is `invalid_request`
            // (binary `Ol({...,error:"invalid_request"})`), even though their
            // TERMINAL reasons (`blocking_limit` / `rapid_refill_breaker`) differ
            // from the reactive `prompt_too_long`. Matching TS
            // `createAssistantAPIErrorMessage({ …, error: 'invalid_request' })`
            // (`query.ts:642-644`).
            self.fire_stop_failure("invalid_request").await;
            return StopHookFlow::FallThrough;
        }
        match self.fire_stop_hooks(stop_reason, *stop_hook_active).await {
            StopHookDisposition::Prevent(reason) => {
                // FIX C (Stop hook_stopped_continuation): persist the stop-reason
                // meta message (claude `query/stopHooks.ts:269-280`, an isMeta
                // `hook_stopped_continuation` attachment) BEFORE terminating so the
                // transcript records why the Stop hook halted continuation.
                self.append_stop_hook_stopped_continuation(&reason).await;
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn(stop_reason, &cost).await;
                StopHookFlow::Terminate(ConversationOutcome::StopHookPrevented {
                    turn_count,
                    final_message_id,
                })
            }
            StopHookDisposition::Continue(reason) => {
                // #2/#4 consecutive-block cap (binary `let ar=Z+1; if(bo>0&&ar>bo)
                // …return {reason:"completed"}`). `Z` is the carried
                // `stopHookBlockingCount`; `ar` the would-be next count.
                let next_count = stop_hook_blocking_count.saturating_add(1);
                // Binary blocking-branch max-turns check, evaluated BEFORE the
                // block cap (`let dt=ie+1,nn=te+1; if(c&&dt>c) return
                // G("tengu_stop_hook_block_count",{count:nn,hit_max_turns:!0,
                // hit_cap:!1}), yield ei({type:"max_turns_reached",maxTurns:c,
                // turnCount:dt},…), {reason:"max_turns",turnCount:dt}`). `c` is
                // `maxTurns`; `ie` is this turn's count = our (already-incremented)
                // `turn_count`, so the binary's `ie+1>c` is exactly
                // `turn_count >= max_turns`. The port represents the max-turns
                // terminal as `OrchestratorError::MaxTurnsReached`
                // (→ `TurnOutcome::MaxTurns`) rather than a discrete stream event,
                // so we route there via `TerminateMaxTurns` — but we still fire the
                // `hit_max_turns:true` block-count event the binary emits here, and
                // (like the binary, which returns before appending) we end WITHOUT
                // adding the stop-hook feedback message the `LoopAgain` path would.
                if self.config.max_turns != 0 && turn_count >= self.config.max_turns {
                    self.fire_stop_hook_block_count(next_count, true, false)
                        .await;
                    return StopHookFlow::TerminateMaxTurns;
                }
                // `parseInt(process.env.LINGXI_STOP_HOOK_BLOCK_CAP??"",10)`
                // with `Number.isNaN(jr)?8:jr` ⇒ unset / non-numeric → 8. A
                // `cap <= 0` disables the cap (binary `if(bo>0&&…)`), letting a
                // blocking hook drive until the max_turns top-of-loop guard ends it.
                let cap: i64 = std::env::var("LINGXI_STOP_HOOK_BLOCK_CAP")
                    .ok()
                    .and_then(|v| v.trim().parse::<i64>().ok())
                    .unwrap_or(8);
                if cap > 0 && i64::from(next_count) > cap {
                    // Cap exceeded: log `tengu_stop_hook_block_count{hit_cap:true}`,
                    // surface the byte-exact override warning (em-dash U+2014;
                    // binary `yield Dc(…,"warning")`), and END the turn (binary
                    // returns `{reason:"completed"}` ⇒ our `FallThrough` runs the
                    // normal end-of-turn tail). NOTE: `is_subagent` is hard-coded
                    // `false` — the orchestrator does not thread subagent identity
                    // (`Boolean(N.agentId)`); the main-loop value is `false`.
                    self.fire_stop_hook_block_count(next_count, false, true)
                        .await;
                    let warning = format!(
                        "A hook blocked the turn from ending {next_count} consecutive times — overriding and ending turn. For Stop/SubagentStop hooks, check stop_hook_active in the input and return success while it's true. Set LINGXI_STOP_HOOK_BLOCK_CAP to raise this limit."
                    );
                    self.output.emit_text(&warning).await;
                    return StopHookFlow::FallThrough;
                }
                self.append_stop_hook_feedback(&reason).await;
                *stop_hook_active = true;
                *stop_hook_blocking_count = next_count;
                StopHookFlow::LoopAgain
            }
            StopHookDisposition::Pass => {
                // #4: a previously-blocking Stop hook finally let the turn end —
                // log the final consecutive-block count (binary
                // `if(Z>0&&Un.blockingErrors.length===0) W("tengu_stop_hook_block_count",
                // {count:Z,hit_max_turns:!1,hit_cap:!1})`). No-op when the hook
                // never blocked this turn (`count == 0`).
                if *stop_hook_blocking_count > 0 {
                    self.fire_stop_hook_block_count(*stop_hook_blocking_count, false, false)
                        .await;
                }
                StopHookFlow::FallThrough
            }
        }
    }

    /// Fire the `tengu_stop_hook_block_count` analytics event (hooks B4, binary
    /// `bin/claude.exe` offset ~208046100). Emitted in three forms: on the
    /// block-cap end (`hit_cap:true`), on the max-turns-via-stop-hook end
    /// (`hit_max_turns:true`), and when a previously-blocking hook finally lets
    /// the turn end (both `false`). `is_subagent` is hard-coded `false` — the
    /// orchestrator does not model subagent identity (`Boolean(N.agentId)`).
    /// Strict no-op when no analytics bus is wired.
    async fn fire_stop_hook_block_count(&self, count: u32, hit_max_turns: bool, hit_cap: bool) {
        let Some(bus) = self.analytics_bus.as_ref() else {
            return;
        };
        let mut metadata = telemetry::LogEventMetadata::new();
        metadata.insert(
            "count".into(),
            telemetry::AnalyticsValue::Int(i64::from(count)),
        );
        metadata.insert("is_subagent".into(), telemetry::AnalyticsValue::Bool(false));
        metadata.insert(
            "hit_max_turns".into(),
            telemetry::AnalyticsValue::Bool(hit_max_turns),
        );
        metadata.insert("hit_cap".into(), telemetry::AnalyticsValue::Bool(hit_cap));
        bus.log_event("tengu_stop_hook_block_count", metadata).await;
    }

    /// Fire the `PreCompact` lifecycle hooks immediately BEFORE a compaction
    /// pass runs (hooks compaction lifecycle, TS `executePreCompactHooks` called
    /// from `services/compact/compact.ts:413` BEFORE the summary request).
    ///
    /// `trigger` is the `auto` / `manual` discriminator carried verbatim into
    /// the wire payload's `trigger` field (TS `compactData.trigger`): `auto` for
    /// the proactive pre-call autocompact and the reactive 413/PTL fallback,
    /// `manual` for an explicit `/compact`. Strict no-op when no `PreCompact`
    /// hook is registered.
    ///
    /// Returns `Some(blockedBy)` when a PreCompact hook BLOCKED the compaction
    /// (TS `executePreCompactHooks` = `xhe`: `blockedBy` is set when any hook
    /// result has `blocked`), and `None` when compaction should proceed. A block
    /// ABORTS the pass in every path (TS `VJn` throws
    /// `"Compaction blocked by PreCompact hook: <blockedBy>"` on the manual
    /// route; the proactive / reactive routes log and skip). The returned
    /// string is the port's aggregate `reason` (the closest equivalent of TS's
    /// `[cmd]: output` join); callers own the surfacing so the log wording
    /// matches each route (manual / `Precomputed` / `Reactive`).
    ///
    /// DEFERRED (documented divergence, not a parity gap): TS
    /// `executePreCompactHooks` also returns `newCustomInstructions` which the
    /// caller merges into the summary prompt (`compact.ts:420`). This port does
    /// NOT thread that back into the summarizer: the `HookEvent::PreCompact` wire
    /// builder hard-codes `custom_instructions: None` (`hooks/executor.rs:755`)
    /// and the compaction seam (`process_iteration` / `process_iteration_tracked`)
    /// accepts no custom-instruction argument, so there is no clean seam to feed
    /// the aggregate's instructions into the summary request. Honoring the block
    /// is the byte-faithful behaviour; consuming the returned instructions is
    /// left for a future batch that widens the seam.
    pub(crate) async fn fire_pre_compact(&self, trigger: &str) -> Option<String> {
        let ctx = self.lifecycle_hook_ctx(false).await;
        let agg = self
            .hooks
            .execute(
                HookEvent::PreCompact {
                    reason: trigger.to_string(),
                },
                ctx,
            )
            .await;
        // TS `xhe`: a PreCompact hook that BLOCKS aborts compaction. Surface the
        // block detail (`blockedBy`) so the caller can log/throw per its route;
        // `None` = no block → proceed. `reason` is the port's block detail.
        if matches!(agg.decision, Some(hooks::response::HookDecision::Block)) {
            Some(agg.reason.clone().unwrap_or_default())
        } else {
            None
        }
    }

    /// Fire the `PostCompact` lifecycle hooks immediately AFTER a compaction pass
    /// has been applied to the live session (hooks compaction lifecycle, TS
    /// `executePostCompactHooks` called from `services/compact/compact.ts:723`
    /// AFTER the summary is produced).
    ///
    /// `summary` carries the compaction summary text (TS `compactData.compactSummary`
    /// = `getAssistantMessageText(summaryResponse)`); `tokens_freed` is the
    /// approximate reclaimed-token count. Best-effort, exactly like
    /// [`Self::fire_pre_compact`]: the aggregate is discarded so a failing
    /// `PostCompact` hook never breaks the turn. Strict no-op when no
    /// `PostCompact` hook is registered.
    /// Derive the `PostCompact` `summary` payload text from a completed
    /// compaction pass. Mirrors TS `compactData.compactSummary =
    /// getAssistantMessageText(summaryResponse)`: the autocompact layer replaces
    /// history with the summarizer's output message(s)
    /// ([`compaction::IterationCompactionResult::messages`] ==
    /// `summary_messages`), so concatenating their text reconstitutes the
    /// summary the model produced. Empty when the pass produced no text.
    pub(crate) fn compaction_summary_text(
        result: &compaction::IterationCompactionResult,
    ) -> String {
        result
            .messages
            .iter()
            .map(protocol::ConversationMessage::text_content)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) async fn fire_post_compact(
        &self,
        trigger: &str,
        summary: String,
        tokens_freed: u64,
    ) {
        // `trigger` is part of the TS PostCompact `matchQuery` but the
        // `HookEvent::PostCompact` wire builder emits an empty `trigger` field
        // (`hooks/executor.rs:768`); accepted here for call-site symmetry with
        // `fire_pre_compact` and forward-compatibility if the payload widens.
        let _ = trigger;
        let ctx = self.lifecycle_hook_ctx(false).await;
        let _ = self
            .hooks
            .execute(
                HookEvent::PostCompact {
                    summary,
                    tokens_freed,
                },
                ctx,
            )
            .await;
    }

    /// Fire the `SessionStart` lifecycle hooks at session startup (hooks
    /// session lifecycle, TS `SessionStart` event fired by `executeSetupHooks` /
    /// the `SessionStart` path at session startup — `utils/hooks.ts:3876-3881`).
    ///
    /// `source` is the TS `SessionStart` `source` discriminator — one of
    /// `startup` / `resume` / `clear` / `compact`. The host composition root
    /// (`engine-desktop` / `engine-mobile`) owns the session lifecycle (it
    /// constructs the orchestrator), so it calls this ONCE immediately after
    /// `build()` returns a fully-wired runtime, passing `"startup"` for a fresh
    /// session.
    ///
    /// Best-effort, exactly like [`Self::fire_pre_compact`]: the aggregate is
    /// discarded so a failing `SessionStart` hook never breaks boot. Strict no-op
    /// when no `SessionStart` hook is registered. The hook executor reads
    /// `session_id` / `cwd` from the lifecycle [`HookContext`]; the variant's
    /// `session_id` field is filled from the live session for symmetry.
    pub async fn fire_session_start(&self, source: &str) {
        let session_id = { self.session.lock().await.session_id };
        let ctx = self.lifecycle_hook_ctx(false).await;
        let agg = self
            .hooks
            .execute(
                HookEvent::SessionStart {
                    session_id,
                    source: source.to_string(),
                },
                ctx,
            )
            .await;
        // SESSIONSTART.CTX: a `SessionStart` hook's
        // `hookSpecificOutput.additionalContext` becomes a persistent
        // `hook_additional_context` attachment in the conversation —
        // claude-code's `processSessionStartHooks` collects every hook's
        // `additionalContext` and, when non-empty, emits a single
        // `createAttachmentMessage({type:'hook_additional_context', content:
        // additionalContexts, hookName:'SessionStart'})` (`utils/sessionStart.ts:163-172`),
        // rendered by `messages.ts:4117-4128` as a meta user message
        // `wrapInSystemReminder(`${hookName} hook additional context: ${content.join('\n')}`)`
        // with `isMeta:true`.
        //
        // Unlike the per-turn date / output-style reminders (regenerated each
        // turn and never stored), claude-code adds THIS message ONCE at session
        // start and keeps it in the conversation array, so it rides EVERY
        // subsequent turn. We mirror that by pushing it into `s.history`, which
        // is cloned into each turn's outgoing snapshot (`turn_loop.rs` —
        // `s.history.clone()`). `user_meta` (isMeta) is sent to the wire but not
        // persisted to JSONL (matching `createUserMessage({isMeta:true})`); a
        // `resume` re-fires `SessionStart`, so the context is re-added rather
        // than relying on transcript persistence.
        //
        // Strict no-op when no hook emitted `additionalContext` — the aggregate
        // is otherwise discarded exactly as before, so a failing / silent
        // `SessionStart` hook never affects boot.
        if !agg.additional_contexts.is_empty() {
            let body = agg.additional_contexts.join("\n");
            let msg = ConversationMessage::user_meta(
                MessageId::new(),
                format!(
                    "<system-reminder>\nSessionStart hook additional context: {body}\n</system-reminder>"
                ),
            );
            self.session.lock().await.history.push(msg);
        }
    }

    /// Fire the `InstructionsLoaded` hooks once per loaded instruction file at
    /// session startup (hooks lifecycle, TS `executeInstructionsLoadedHooks`
    /// dispatched from the eager `getMemoryFiles` pass — `utils/claudemd.ts:1054-1071`,
    /// `utils/hooks.ts:4335-4369`).
    ///
    /// claude-code fires this fire-and-forget hook for **each** LINGXI.md /
    /// `LINGXI.local.md` it splices into context, carrying that file's `file_path`,
    /// `memory_type` (`User` / `Project` / `Local` / `Managed`), and `load_reason`.
    /// The eager session-start pass reports `load_reason: 'session_start'` for every
    /// top-level (parent-less) file (`eagerLoadReason`). The orchestrator's
    /// [`crate::prompt::MemoryHierarchyProvider`] loads the full Managed/User/
    /// Project/Local hierarchy and tags each file with its
    /// [`memory::lingxi_md::LingxiMdTier`]; `memory_type` is taken directly from
    /// that tier (so an enterprise-`Managed` file is reported as `Managed`).
    ///
    /// Conditional (`paths:`-gated) rules are filtered out of the eager set by
    /// the provider, so every file fired here is unconditional and top-level ⇒
    /// `load_reason = session_start` with `globs = None`.
    ///
    /// `trigger_file_path` / `parent_file_path` are omitted — the eager pass
    /// carries no lazy-trigger or `@include`-parent metadata (those wire fields
    /// are `.optional()` and elided when absent, matching the TS session-start
    /// fire).
    ///
    /// The orchestrator already owns the memory provider AND the hook registry, so
    /// it loads memory ONCE here (the same `memory.load(&cwd)` the system-prompt
    /// assembler uses) and fires from a single point — no cross-crate seam. The
    /// host composition root calls this ONCE immediately after [`Self::fire_session_start`].
    ///
    /// Best-effort, exactly like [`Self::fire_session_start`]: each hook aggregate
    /// is discarded so a failing `InstructionsLoaded` hook never breaks boot, and
    /// it is a strict no-op when no `InstructionsLoaded` hook is registered.
    pub async fn fire_instructions_loaded(&self) {
        let cwd = self.cwd.clone();
        let memory_files = self.memory.load(&cwd).await;
        if memory_files.is_empty() {
            return;
        }
        for file in memory_files {
            // §F: `load()` now also returns conditional (`paths:`-gated) rules.
            // Those are NOT eagerly loaded, so they must not fire a
            // `session_start` `InstructionsLoaded` event here — claude-code fires
            // them at lazy-activation time with `load_reason: 'path_glob_match'`
            // (`memoryFilesToAttachments`, attachments.ts:1754-1769). Skip them
            // so the eager fire stays unconditional-only.
            if file.globs.is_some() {
                continue;
            }
            // `memory_type` is taken straight from the file's tier (claude-code
            // fires `file.type`, claudemd.ts:1058-1062), so the Managed tier is
            // reported faithfully rather than misclassified as Project.
            let memory_type = match file.tier {
                memory::lingxi_md::LingxiMdTier::Managed => {
                    hooks::events::InstructionsMemoryType::Managed
                }
                memory::lingxi_md::LingxiMdTier::User => {
                    hooks::events::InstructionsMemoryType::User
                }
                memory::lingxi_md::LingxiMdTier::Project => {
                    hooks::events::InstructionsMemoryType::Project
                }
                memory::lingxi_md::LingxiMdTier::Local => {
                    hooks::events::InstructionsMemoryType::Local
                }
            };
            // A fresh per-file `HookContext` (the executor reads `session_id` /
            // `cwd` from it); `lifecycle_hook_ctx` re-locks the session each call,
            // matching the other lifecycle fires.
            let ctx = self.lifecycle_hook_ctx(false).await;
            let _ = self
                .hooks
                .execute(
                    HookEvent::InstructionsLoaded {
                        file_path: file.path,
                        memory_type,
                        // Top-level eager session-start load (no `@include` parent).
                        load_reason: hooks::events::InstructionsLoadReason::SessionStart,
                        // Always `None` here — conditional (`globs.is_some()`)
                        // rules were skipped above; only unconditional files reach
                        // this fire.
                        globs: file.globs,
                        trigger_file_path: None,
                        parent_file_path: None,
                    },
                    ctx,
                )
                .await;
        }
    }

    /// Fire the `SessionEnd` lifecycle hooks at session teardown (hooks session
    /// lifecycle, TS `executeSessionEndHooks` — `utils/hooks.ts:4097-4117`).
    ///
    /// `reason` is the TS `SessionEnd` `reason` (`ExitReason`) discriminator
    /// (e.g. `clear` / `logout` / `prompt_input_exit` / `other`). The host
    /// composition root owns teardown; it calls this at an explicit session-end
    /// seam when one exists. Best-effort like [`Self::fire_session_start`] — a
    /// failing hook never breaks teardown, and it is a strict no-op when no
    /// `SessionEnd` hook is registered.
    pub async fn fire_session_end(&self, reason: &str) {
        let session_id = { self.session.lock().await.session_id };
        let ctx = self.lifecycle_hook_ctx(false).await;
        // Route through the SessionEnd *batch-deadline* path (claude-code `lje`
        // → `cH({signal: AbortSignal.timeout(Wqt())})`): the whole SessionEnd
        // hook batch is capped by a single shutdown budget
        // (`LINGXI_SESSIONEND_HOOKS_TIMEOUT_MS`, else `max(1500,
        // min(maxPerHookMs, 60000))`), so a slow / hung teardown hook cannot
        // stall session exit — NOT the generic 10-minute per-hook `execute`.
        let _ = self
            .hooks
            .execute_session_end(
                HookEvent::SessionEnd {
                    session_id,
                    reason: reason.to_string(),
                },
                ctx,
            )
            .await;
    }

    /// Whether any registered hook subscribes to the `Notification` event.
    ///
    /// Cheap gate read for callers that want to avoid arming a fire path with
    /// no subscriber — e.g. the CLI repl's idle-prompt timer only arms when
    /// this is `true`, mirroring how `engine-desktop`'s settings watcher only
    /// arms the `ConfigChange` fire when a subscriber exists. A `true` here
    /// reports event-type subscription only (a declared matcher may still
    /// filter the hook out at fire time), which is exactly what the gate needs.
    pub async fn has_notification_hook(&self) -> bool {
        self.hooks
            .has_hooks_for(&hooks::events::HookEventType::Notification)
            .await
    }

    /// Fire the `Notification` lifecycle hooks (hooks runtime lifecycle, TS
    /// `sendNotification` → `executeNotificationHooks`). `message` is the wire
    /// `NotificationPayload.message`; `notification_type` is the byte-faithful
    /// `notification_type` discriminator (e.g. `idle_prompt`) — it FEEDS the
    /// `HookEvent::Notification { kind }` field, which the executor copies
    /// verbatim into `NotificationPayload.notification_type`
    /// (`hooks/executor.rs` Notification arm).
    ///
    /// The canonical caller is the REPL idle watcher: claude-code fires
    /// `sendNotification({ message: "Claude is waiting for your input",
    /// notificationType: "idle_prompt" })` once the repl has been idle for
    /// `messageIdleNotifThresholdMs` after the last response
    /// (`screens/REPL.tsx:3930-3940`).
    ///
    /// Best-effort, exactly like [`Self::fire_session_end`]: the aggregate is
    /// discarded so a failing or blocking `Notification` hook can NEVER affect
    /// the caller (the repl input loop), and it is a strict no-op when no
    /// `Notification` hook is registered (the `execute` matcher returns an
    /// empty set → default aggregate, no process spawned).
    pub async fn fire_notification(&self, message: &str, notification_type: &str) {
        let ctx = self.lifecycle_hook_ctx(false).await;
        let _ = self
            .hooks
            .execute(
                HookEvent::Notification {
                    message: message.to_string(),
                    kind: notification_type.to_string(),
                },
                ctx,
            )
            .await;
    }

    /// Fire the `ConfigChange` hooks when a settings / skills file mutated on
    /// disk (hooks runtime lifecycle, TS `executeConfigChangeHooks` —
    /// `utils/hooks.ts:4214`, dispatched from the settings watcher
    /// `utils/settings/changeDetector.ts:285-297`).
    ///
    /// claude-code's `changeDetector` watches the settings files via
    /// `fs.watchFile` polling and, on every detected change, fires this hook
    /// with the `source` (which settings layer / skills) and the changed
    /// `file_path` BEFORE applying the change to the live session. The host
    /// composition root (`engine-desktop`) owns the watcher; this method is the
    /// single fire seam it calls per change event — the live settings RELOAD is
    /// a separate concern handled (or not) by the composition root.
    ///
    /// `source` maps 1:1 onto claude-code's `ConfigChangeSource`
    /// (`user_settings` / `project_settings` / `local_settings` /
    /// `policy_settings` / `skills`); `file_path` is the absolute path that
    /// changed (TS always passes the path, so this is `Some` in production —
    /// the field stays `Option` because the wire schema marks it `.optional()`).
    ///
    /// Best-effort, exactly like [`Self::fire_session_start`]: the aggregate is
    /// discarded so a failing or blocking `ConfigChange` hook never breaks the
    /// watcher loop, and it is a strict no-op when no `ConfigChange` hook is
    /// registered (the common case). The hook executor reads `session_id` /
    /// `cwd` from the lifecycle [`HookContext`].
    pub async fn fire_config_change(
        &self,
        source: hooks::events::ConfigChangeSource,
        file_path: Option<std::path::PathBuf>,
    ) {
        let ctx = self.lifecycle_hook_ctx(false).await;
        let _ = self
            .hooks
            .execute(HookEvent::ConfigChange { source, file_path }, ctx)
            .await;
    }

    /// Append a Stop hook's blocking reason as a *meta* user message so the
    /// model sees the hook feedback on the continued turn. Mirrors claude-code
    /// `query/stopHooks.ts:257-262`: each blocking error becomes
    /// `createUserMessage({ content: getStopHookMessage(blockingError), isMeta: true })`,
    /// where `getStopHookMessage` = `Stop hook feedback:\n${blockingError}`
    /// (`utils/hooks.ts:1895`). The `isMeta` flag hides it from the user-facing
    /// UI while keeping it in the API stream. Best-effort persist, like the
    /// other meta appends.
    async fn append_stop_hook_feedback(&self, reason: &str) {
        let content = format!("Stop hook feedback:\n{reason}");
        let msg = ConversationMessage::user_meta(MessageId::new(), content);
        {
            let mut s = self.session.lock().await;
            s.history.push(msg.clone());
        }
        self.persist_message_to_jsonl(&msg).await;
    }

    /// Append a Stop hook's `preventContinuation` stop-reason as a *meta* user
    /// message so the transcript records why continuation was halted. Mirrors
    /// claude-code `query/stopHooks.ts:269-280`: a `hook_stopped_continuation`
    /// attachment (hookName `Stop`) rendered as
    /// `<system-reminder>\nStop hook stopped continuation: {stopReason}\n</system-reminder>`
    /// (isMeta, `utils/messages.ts:4130-4137`). Best-effort persist, exactly like
    /// [`Self::append_stop_hook_feedback`].
    async fn append_stop_hook_stopped_continuation(&self, reason: &str) {
        let content =
            format!("<system-reminder>\nStop hook stopped continuation: {reason}\n</system-reminder>");
        let msg = ConversationMessage::user_meta(MessageId::new(), content);
        {
            let mut s = self.session.lock().await;
            s.history.push(msg.clone());
        }
        self.persist_message_to_jsonl(&msg).await;
    }

    async fn try_run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError> {
        // 0. Build the system prompt for THIS turn. Override always wins.
        let system_prompt: Option<String> = match &self.config.system_prompt_override {
            Some(custom) => Some(custom.clone()),
            None => Some(self.build_system_prompt().await),
        };

        // 1. Append the user prompt to session history.
        let user_msg = ConversationMessage::user(MessageId::new(), prompt.to_string());
        {
            let mut s = self.session.lock().await;
            s.history.push(user_msg.clone());
        }
        self.persist_message_to_jsonl(&user_msg).await;

        // hooks B4: fire UserPromptSubmit. A Block decision aborts the turn
        // BEFORE any API call (TS prompt-ingress hook). No-op when unregistered.
        if self.fire_user_prompt_submit(prompt).await {
            return Ok(ConversationOutcome::StopHookPrevented {
                turn_count: 0,
                final_message_id: user_msg.id(),
            });
        }

        // 2. Turn-by-turn driver.
        // A1: per-conversation max_output_tokens recovery bookkeeping carried
        // across turn-steps (the 3-retry limit is consecutive).
        let mut recovery = RecoveryState::default();
        // hooks B4: Stop-hook re-entry guard. Set true after a Stop hook blocks
        // and we loop once more; a second block then passes (no infinite loop).
        let mut stop_hook_active = false;
        // #2 consecutive Stop-hook block counter (binary `stopHookBlockingCount`):
        // bumped per block; ends the turn via the cap once it would exceed
        // LINGXI_STOP_HOOK_BLOCK_CAP (default 8). Fresh per turn-driver run.
        let mut stop_hook_blocking_count: u32 = 0;
        // A3: token-budget continuation bookkeeping. `Some` only when the gate
        // is enabled AND a budget is set; otherwise the budget check is a
        // NO-OP and the loop stops at the first `end_turn` (parity default).
        let mut budget = self.new_budget_tracker();
        let mut global_turn_tokens: u64 = 0;
        // Turn-start output baseline (claude-code `xtr` via `UAc(e)`): snapshot the
        // cumulative pool as this turn begins, so a workflow launched this turn
        // reads `budget.spent()` = `pool - baseline` (output spent THIS turn).
        self.turn_start_output_baseline.store(
            self.output_token_pool
                .load(std::sync::atomic::Ordering::Relaxed),
            std::sync::atomic::Ordering::Relaxed,
        );
        let mut turn_count: u32 = 0;
        let final_message_id;
        loop {
            if self.config.max_turns != 0 && turn_count >= self.config.max_turns {
                return Err(OrchestratorError::MaxTurnsReached {
                    max_turns: self.config.max_turns,
                });
            }
            if self.over_budget().await {
                return Err(OrchestratorError::MaxBudgetReached {
                    budget_nano_usd: self.config.max_budget_nano_usd.unwrap_or(0),
                });
            }
            turn_count = turn_count.saturating_add(1);

            let (step, output_tokens) = execute_one_turn_with_recovery_tracked(
                self,
                system_prompt.as_deref(),
                Some(&mut recovery),
            )
            .await?;
            // A3: accumulate the running per-turn output tokens (TS
            // `getTurnOutputTokens()`). No-op for accounting when budget is off.
            global_turn_tokens = global_turn_tokens.saturating_add(output_tokens);
            match step {
                TurnStepOutcome::Continue => continue,
                TurnStepOutcome::Ended {
                    final_message_id: id,
                    stop_reason,
                } => {
                    // hooks B4: fire Stop hooks BEFORE the token-budget check
                    // (order: recovery → stop-hooks → token-budget, TS
                    // `query.ts:1262-1308`).
                    match self
                        .handle_stop_at_end(
                            &stop_reason,
                            &mut stop_hook_active,
                            &mut stop_hook_blocking_count,
                            turn_count,
                            id,
                        )
                        .await
                    {
                        StopHookFlow::Terminate(outcome) => return Ok(outcome),
                        StopHookFlow::TerminateMaxTurns => {
                            return Err(OrchestratorError::MaxTurnsReached {
                                max_turns: self.config.max_turns,
                            });
                        }
                        StopHookFlow::LoopAgain => {
                            // RECOV.4: a Stop hook forced the loop to continue —
                            // reset the max_output_tokens recovery bookkeeping so the
                            // continued turn starts a fresh escalation episode (TS
                            // `query.ts:1291` sets `maxOutputTokensRecoveryCount: 0`
                            // + `maxOutputTokensOverride: undefined` on the
                            // stop-hook-blocking continuation).
                            recovery.reset_max_output_tokens_recovery();
                            continue;
                        }
                        StopHookFlow::FallThrough => {}
                    }
                    // A3: at a natural end-of-turn, consult the token budget. If
                    // it says `continue`, inject the meta nudge, reset the A1
                    // recovery count (per `query.ts:1332`), and loop again
                    // instead of breaking. When budget is off this is a no-op.
                    if self
                        .maybe_continue_for_budget(
                            budget.as_mut(),
                            &mut recovery,
                            global_turn_tokens,
                        )
                        .await
                    {
                        continue;
                    }
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn(&stop_reason, &cost).await;
                    final_message_id = id;
                    break;
                }
            }
        }

        Ok(ConversationOutcome::EndTurn {
            turn_count,
            final_message_id,
        })
    }

    /// Drive one user prompt through the STREAMING turn loop. Mirrors
    /// the contract of [`Self::run_turn`] but consumes SSE events as
    /// they arrive (per-token `OutputStream::emit_text`) and dispatches
    /// `tool_use` blocks the moment their `content_block_stop` event is
    /// received.
    ///
    /// Emits 2 streaming-specific telemetry events at the boundaries:
    /// - [`orch_events::TURN_STREAMING_STARTED`] at entry.
    /// - [`orch_events::TURN_STREAMING_COMPLETED`] after success.
    ///
    /// On error, the existing [`orch_events::CONVERSATION_FAILED`] is
    /// reused (no new error event in M5-04).
    pub async fn run_turn_streaming(
        &self,
        prompt: &str,
    ) -> Result<ConversationOutcome, OrchestratorError> {
        tracing::info!(
            event = orch_events::TURN_STREAMING_STARTED,
            prompt_len = prompt.len()
        );
        // DEFERRED-3: the plain (non-cancelable) streaming entry has no granular
        // user-interrupt token → `None` (behaviour byte-identical to before).
        let result = self.try_run_turn_streaming(prompt, Vec::new(), None).await;
        self.emit_terminal_rate_limit_if_changed(&result).await;
        let result = result.map_err(|e| self.enrich_api_error(e));
        match &result {
            Ok(
                ConversationOutcome::EndTurn { turn_count, .. }
                | ConversationOutcome::StopHookPrevented { turn_count, .. },
            ) => {
                tracing::info!(
                    event = orch_events::TURN_STREAMING_COMPLETED,
                    turn_count = *turn_count
                );
            }
            Err(err) => {
                tracing::error!(
                    event = orch_events::CONVERSATION_FAILED,
                    reason = %err
                );
            }
        }
        result
    }

    /// Internal streaming turn driver (no telemetry — wrapped by
    /// `run_turn_streaming`).
    #[allow(clippy::too_many_lines)]
    async fn try_run_turn_streaming(
        &self,
        prompt: &str,
        images: Vec<protocol::ImageSource>,
        // DEFERRED-3: the turn's USER-interrupt token (ESC / new message). `Some`
        // only on the cancelable streaming entry; `None` for the plain
        // `run_turn_streaming` (no granular interrupt). When fired mid-tools, the
        // `StreamingToolExecutor` rejects in-flight/queued Cancel-behavior tools
        // with the bare REJECT_MESSAGE, PERSISTS those results, and ends the turn
        // gracefully (no whole-turn drop) — mirroring claude-code's
        // `StreamingToolExecutor` user_interrupted path.
        user_cancel: Option<CancellationToken>,
    ) -> Result<ConversationOutcome, OrchestratorError> {
        use crate::streaming_loop::ExecutorPump;
        use protocol::ContentBlock;

        // Startup Responses WebSocket prewarm is strictly opportunistic. A real
        // user turn must never wait for an in-flight `generate=false` request to
        // finish before it can open its own stream.
        self.abort_startup_responses_websocket_prewarm();

        // 0. Build the system prompt for THIS turn. Override always wins.
        let system_prompt: Option<String> = match &self.config.system_prompt_override {
            Some(custom) => Some(custom.clone()),
            None => Some(self.build_system_prompt().await),
        };

        // 1. Append the user prompt (+ any pasted images) to session history.
        // `images` arrives already decoded (path-based callers ran `load_images`
        // first; the bridge converts inline `ImageRefDto`s straight to sources).
        let user_msg =
            ConversationMessage::user_with_images(MessageId::new(), prompt.to_string(), images);
        {
            let mut s = self.session.lock().await;
            s.history.push(user_msg.clone());
        }
        self.persist_message_to_jsonl(&user_msg).await;

        // (/rewind) Snapshot the pre-turn file state IN MEMORY, keyed by this
        // user message, so `track_edit` (fired by Edit/Write/NotebookEdit during
        // the turn) records each file's pre-edit backup into it. The POPULATED
        // record is persisted to the transcript at TURN END (see below) — NOT
        // here: at turn start the backup map is empty (no edits yet), and
        // persisting it now would leave disk-based restore (`rewind_from_disk`,
        // which runs after the TUI unwinds and rebuilds the index from these
        // lines) with nothing to restore.
        let file_history_msg_id = user_msg.id().as_uuid();
        if let Some(fh) = &self.file_history {
            fh.make_snapshot(file_history_msg_id).await;
        }

        // hooks B4: UserPromptSubmit (streaming twin). A Block aborts before the
        // first stream is opened. No-op when unregistered.
        if self.fire_user_prompt_submit(prompt).await {
            return Ok(ConversationOutcome::StopHookPrevented {
                turn_count: 0,
                final_message_id: user_msg.id(),
            });
        }

        // Build the wire tool definitions once for the conversation (stable
        // across turns; see `build_wire_tools`). Cloned into each turn's stream.
        let wire_tools = self.build_wire_tools().await;

        // A1: per-conversation max_output_tokens recovery bookkeeping (streaming
        // twin of the batched driver). Carried across turn-steps so the 3-retry
        // limit is consecutive.
        let mut recovery = RecoveryState::default();
        // hooks B4: Stop-hook re-entry guard (streaming twin).
        let mut stop_hook_active = false;
        // #2 consecutive Stop-hook block counter (binary `stopHookBlockingCount`):
        // bumped per block; ends the turn via the cap once it would exceed
        // LINGXI_STOP_HOOK_BLOCK_CAP (default 8). Fresh per turn-driver run.
        let mut stop_hook_blocking_count: u32 = 0;
        // A3: token-budget continuation bookkeeping (streaming twin). `Some`
        // only when the gate is enabled AND a budget is set; otherwise the
        // budget check is a NO-OP and the loop stops at the first `end_turn`.
        let mut budget = self.new_budget_tracker();
        let mut global_turn_tokens: u64 = 0;
        // Turn-start output baseline (claude-code `xtr` via `UAc(e)`): snapshot the
        // cumulative pool as this turn begins, so a workflow launched this turn
        // reads `budget.spent()` = `pool - baseline` (output spent THIS turn).
        self.turn_start_output_baseline.store(
            self.output_token_pool
                .load(std::sync::atomic::Ordering::Relaxed),
            std::sync::atomic::Ordering::Relaxed,
        );
        let mut turn_count: u32 = 0;
        // Malformed-tool-use retry guard (claude-code `transition.reason ===
        // "malformed_tool_use_retry"`): set after the FIRST `tool_use`
        // stop_reason that produced zero tool_use blocks, so the SECOND such
        // failure terminates instead of looping forever. Persists across
        // turn-steps within this invocation.
        let mut malformed_tool_use_retried = false;
        // Thinking-only nudge guard (claude-code `thinkingOnlyNudged`): set
        // after the once-per-turn nudge on an `end_turn`/`stop_sequence`
        // response with no visible text, so a still-empty continuation ends
        // normally instead of nudging again.
        let mut thinking_only_nudged = false;
        let final_message_id;
        // DEFERRED-3 / esc-interrupt FIX: id of the most recent persisted message
        // (the user prompt until the first assistant message lands, then each
        // turn's assistant id). The top-of-loop user-interrupt guard reports it as
        // the turn's `final_message_id` when it stops a turn before the next model
        // call (claude-code `aborted_streaming` — query.ts:1015).
        let mut last_message_id = user_msg.id();
        loop {
            if self.config.max_turns != 0 && turn_count >= self.config.max_turns {
                return Err(OrchestratorError::MaxTurnsReached {
                    max_turns: self.config.max_turns,
                });
            }
            if self.over_budget().await {
                return Err(OrchestratorError::MaxBudgetReached {
                    budget_nano_usd: self.config.max_budget_nano_usd.unwrap_or(0),
                });
            }
            turn_count = turn_count.saturating_add(1);

            // MID-TURN DRAIN (claude-code query.ts ~1570-1580): BEFORE the
            // top-of-loop cancel guard, pull any queued main-thread, non-slash
            // user input and inject it as a meta user message so this iteration's
            // model call sees it. A strict no-op when no source is wired (the
            // default) — the locked streaming fixtures are unaffected. Drained
            // BEFORE the cancel check so the injected input is in history even if
            // the very next thing observed is a `Now`-driven cancellation. A
            // mid-turn drain pulls `Next`+`Now` text; the separate `Now`-abort
            // branch below handles the urgent-command-aborts-the-turn UX.
            //
            // ORDERING IS LOAD-BEARING — DO NOT REORDER past the cancel guard
            // below. The drain MUST run before the cancel check on EVERY iteration
            // so that any `Next`/`Now` text enqueued during the previous
            // iteration's streaming is folded into history before this iteration
            // can observe the (possibly already-cancelled) token and break. The
            // abort-reason flag read by the cancel guard is set at ENQUEUE time
            // (by the queue adapter, before it fires the token) and reset at TURN
            // START (driver `reset()`), never by this drain — so it is monotonic
            // within a turn and the guard never reads a stale value regardless of
            // when the drain consumes. A `Now`-priority command is intentionally
            // NOT mid-turn-injected here; it aborts the turn and is run by the
            // between-turn drain, so leaving it queued past this point is correct.
            self.drain_mid_turn_input().await;

            // DEFERRED-3 / esc-interrupt FIX: top-of-loop user-interrupt guard
            // (faithful port of claude-code `query.ts:1015` — the `aborted_streaming`
            // return). If the user-interrupt token is already set when we reach the
            // top of an iteration — a pre-cancel, or an abort that fired during the
            // previous iteration's streaming BEFORE any tool ran — we must STOP
            // BEFORE issuing the next `callModel`. claude-code consumes any
            // remaining streaming results then returns `aborted_streaming` with no
            // further sampling; here the previous iteration already drained its
            // results into history (the post-tools guard) or there were none, so we
            // simply break. This is the structural barrier that prevents a
            // Block-behavior tool on a post-interrupt continuation from ever
            // executing. `None` token → never fires → identical to before.
            if user_cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn("aborted_streaming", &cost).await;
                // NOW-ABORT disambiguation: when the cancellation was driven by a
                // `Now`-priority enqueue (not a user Ctrl+C/ESC), the urgent
                // command IS the "interruption" and will be run next by the
                // between-turn drain — so DON'T inject the user-interrupt message
                // (which would mislabel the abort and pollute context). For a
                // plain user interrupt (the default when no reason flag is wired)
                // behavior is byte-identical to before: inject the message.
                // claude-code `query.ts:1046-1050`: `createUserInterruptionMessage`.
                if self.cancel_reason_now()
                    != crate::prompt::mid_turn_input::CancelReason::QueueNowCommand
                {
                    self.inject_meta_user_message(INTERRUPT_MESSAGE).await;
                }
                final_message_id = last_message_id;
                break;
            }

            // P0.1 (streaming twin): arm the memory-selector prefetch CONCURRENTLY
            // with this turn (claude-code `wAo`). Fired here at turn start so the
            // in-flight handle is ready when `relevant_memory_reminder_message`
            // awaits it below, before the blocking-limit estimate. A strict no-op
            // when no prefetch is wired, keeping the locked streaming fixtures
            // byte-identical. See [`Self::start_memory_prefetch`].
            self.start_memory_prefetch().await;
            // EXPERIMENTAL_SKILL_SEARCH (streaming twin): arm the skill-discovery
            // prefetch CONCURRENTLY with this turn (claude-code
            // `startSkillDiscoveryPrefetch`). A strict no-op when no prefetch is
            // wired (default OFF), keeping the locked streaming fixtures
            // byte-identical. See [`Self::start_skill_discovery_prefetch`].
            self.start_skill_discovery_prefetch().await;
            // P1 (§6.5): background-fork a session-memory extraction if the
            // tool-call threshold has crossed (inert unless wired + enabled).
            self.maybe_extract_session_memory().await;

            // In-Loop Compaction Batch 4 (streaming twin): proactively
            // snip+micro+autocompact BEFORE snapshotting history for the
            // stream, so a long conversation self-compacts mid-turn. A strict
            // no-op when no compactor is wired or the history is under
            // threshold, so the locked streaming fixtures are unaffected. After
            // a proactive compact the snapshot below reads the NEW history.
            //
            // Batch 5 streaming-PTL DIVERGENCE: the reactive 413/prompt-too-long
            // recovery loop is applied to the BATCHED path only. On the
            // streaming path a 413 surfaces as a stream error through
            // `OrchestratorError::Streaming`; threading `ApiError::PromptTooLong`
            // out of the SSE plumbing cleanly is deferred (the TS B5 test plan
            // targets the batched `messages_create`). The proactive B4 trigger
            // above still shrinks the prompt before the call, which is the
            // common case; the reactive tail is a documented close divergence.
            self.maybe_compact_before_call().await;

            // 2. Open the stream for this turn.
            let (mut snapshot, model, model_profile) = {
                let s = self.session.lock().await;
                (s.history.clone(), s.model.clone(), s.model_profile.clone())
            };

            // R-P1c/R-P1d (streaming twin): PREPEND the leading `additionalContext`
            // meta message (`# claudeMd` / `# userEmail` / `# currentDate`) to THIS
            // turn's OUTGOING snapshot only (never `session.history` / JSONL). 1:1
            // with claude-code `A6n(re, userContext)`, which prepends the meta
            // message at every `callModel`. Recomputed each turn, never accumulates.
            // `currentDate` is always present, so this is `Some(_)` whenever a
            // LINGXI.md / email / date is sourceable (i.e. always for the date).
            if let Some(ctx_msg) = self.additional_context_message().await {
                snapshot.insert(0, ctx_msg);
            }

            // OUTSTYLE.3 (streaming twin): per-turn, transient output-style
            // reminder. Appended to THIS turn's OUTGOING snapshot only — never to
            // `session.history` / JSONL — so it is recomputed each turn and never
            // accumulates (TS recomputes attachments each turn). Injected BEFORE
            // the blocking-limit estimate below so the reminder's tokens are
            // counted in the prompt size, matching the model input. Trailing
            // position mirrors TS; `None` for the default style ⇒ no extra
            // message, keeping the locked streaming fixtures byte-identical. See
            // [`Self::output_style_reminder_message`].
            if let Some(reminder) = self.output_style_reminder_message() {
                snapshot.push(reminder);
            }

            // SKILLLIST.1 (streaming twin): per-turn, transient `skill_listing`
            // reminder so the model can discover skills. Appended to THIS turn's
            // OUTGOING snapshot only (never to `session.history` / JSONL), after
            // the output-style reminder and BEFORE the blocking-limit estimate
            // below so its tokens are counted in the prompt size. `None` when no
            // provider is wired / no skills / the Skill tool is absent. See
            // [`Self::skill_listing_reminder_message`].
            if let Some(reminder) = self.skill_listing_reminder_message().await {
                snapshot.push(reminder);
            }

            // §F (streaming twin): per-turn, transient `conditional_rules`
            // reminder — path-gated LINGXI.md rules that newly activate because a
            // touched file matches their globs. Appended to THIS turn's OUTGOING
            // snapshot only (never `session.history` / JSONL), after the
            // skill-listing reminder and BEFORE the blocking-limit estimate below
            // so its tokens are counted in the prompt size. `None` when no
            // provider / no conditional rules / nothing newly active. See
            // [`Self::conditional_rules_reminder_message`].
            if let Some(reminder) = self.conditional_rules_reminder_message().await {
                snapshot.push(reminder);
            }

            // `<new-diagnostics>` (streaming twin — #3 main-loop parity):
            // per-turn, transient reminder of newly-reported LSP diagnostics not
            // yet surfaced (claude-code `formatDiagnosticsBlock`). Appended to
            // THIS turn's OUTGOING snapshot only (never `session.history` /
            // JSONL), after the conditional-rules reminder and before the
            // agent-listing reminder — identical position to the batched twin
            // (`turn_loop.rs`). claude-code has ONE main loop, so both LingXi
            // twins must inject this reminder. `None` when no LSP source is wired
            // (no servers) or no new diagnostics, keeping the locked streaming
            // fixtures byte-identical. See
            // [`Self::new_diagnostics_reminder_message`].
            if let Some(reminder) = self.new_diagnostics_reminder_message().await {
                snapshot.push(reminder);
            }

            // `agent_listing_delta` (streaming twin): per-turn, transient agent
            // catalog reminder, emitted ONLY when the
            // `LINGXI_AGENT_LIST_IN_MESSAGES` gate is ON (default OFF ⇒
            // `None`, keeping the locked streaming fixtures byte-identical and
            // the inline catalog in place). Appended to THIS turn's OUTGOING
            // snapshot only (never `session.history` / JSONL). See
            // [`Self::agent_listing_reminder_message`].
            if let Some(reminder) = self.agent_listing_reminder_message().await {
                snapshot.push(reminder);
            }

            // Finding #73 (streaming twin): per-turn, transient `todo_reminder`
            // (V1) / `task_reminder` (V2) reminder. Same gates as the batched
            // twin (killswitch / tool-present / Brief-absent / non-empty history
            // / both counters at threshold). Body emitted RAW (no
            // `<system-reminder>` wrap, matching `Ln({content:r,isMeta:!0})`).
            // Placed after the agent-listing reminder and before the async-hook
            // reminder, mirroring the binary `ytl` order (`todo_reminders` in the
            // core `A` array, before the main-only `async_hook_responses`).
            // Appended to THIS turn's OUTGOING snapshot only (never
            // `session.history` / JSONL). `None` keeps the locked streaming
            // fixtures byte-identical. See [`Self::todo_reminder_message`].
            if let Some(reminder) = self.todo_reminder_message().await {
                snapshot.push(reminder);
            }

            // async_hook_response (streaming twin): fold completed background
            // (`async`) hook responses into THIS turn's OUTGOING snapshot only
            // (never `session.history` / JSONL), drained consume-once. `None`
            // when no source is wired / nothing completed since the last turn.
            // See [`Self::async_hook_response_reminder_message`].
            if let Some(reminder) = self.async_hook_response_reminder_message().await {
                snapshot.push(reminder);
            }

            // T35: fold the terminal background tasks finished since the last
            // turn into THIS turn's OUTGOING snapshot only (never
            // `session.history` / JSONL), drained consume-once so each completion
            // surfaces exactly one `<task-notification>`. `None` when no registry
            // is wired / nothing finished. See
            // [`Self::task_notification_reminder_message`].
            if let Some(reminder) = self.task_notification_reminder_message().await {
                snapshot.push(reminder);
            }

            // P0.1 (streaming twin): per-turn, transient `relevant_memories`
            // SURFACING reminder — the memory-selector/prefetch result rendered
            // as one `<system-reminder>` meta user message. Appended to THIS
            // turn's OUTGOING snapshot only (never `session.history` / JSONL),
            // after the async-hook reminder and BEFORE the blocking-limit estimate
            // below so its tokens are counted in the prompt size. Awaits the
            // prefetch armed by `start_memory_prefetch` at turn start. `None` when
            // no prefetch is wired / empty result / everything already injected.
            // See [`Self::relevant_memory_reminder_message`].
            if let Some(reminder) = self.relevant_memory_reminder_message().await {
                snapshot.push(reminder);
            }

            // EXPERIMENTAL_SKILL_SEARCH (streaming twin): per-turn, transient
            // `skill_discovery` SURFACING reminder, collected AFTER the memory
            // consume above (bundle order: memory consume → `collectSkill...`).
            // Appended to THIS turn's OUTGOING snapshot only. `None` when no
            // prefetch is wired (default OFF) / empty / everything already
            // surfaced. See [`Self::skill_discovery_reminder_message`].
            if let Some(reminder) = self.skill_discovery_reminder_message().await {
                snapshot.push(reminder);
            }

            // RECOV.1: blocking-limit preempt — the streaming twin of the batched
            // `call_api_with_ptl_recovery` step (1) (TS `query.ts:592-648`). If the
            // pre-call prompt is already at the hard blocking limit
            // (`token_usage >= effective_window − MANUAL_COMPACT_BUFFER_TOKENS`),
            // surface the byte-exact `PROMPT_TOO_LONG_ERROR_MESSAGE` and END the
            // turn WITHOUT opening the stream — mirroring the batched path (which
            // returns `PtlCallOutcome::PromptTooLong` ⇒ ends with stop_reason
            // `"prompt_too_long"`). Same window math as the batched path: `betas`
            // is `&[]` (the orchestrator does not thread the per-request beta set
            // here) and `auto_compact_enabled = true` for this always-on port. A
            // strict no-op below the limit, so the locked streaming fixtures are
            // unaffected.
            let warning = compaction::calculate_token_warning_state(
                compaction::grouping::estimate_tokens_for_range(&snapshot),
                &model,
                &[],
                true,
            );
            if warning.is_at_blocking_limit {
                tracing::warn!(
                    model = %model,
                    "prompt at blocking limit — preempting before stream"
                );
                let id = surface_prompt_too_long(self).await;
                // PROACTIVE preempt ⇒ terminal reason `"blocking_limit"` (distinct
                // from the reactive-exhausted `prompt_too_long`), mirroring the
                // batched path's `PtlCallOutcome::BlockingLimit`. RECOV.2 chokepoint:
                // `handle_stop_at_end` treats `"blocking_limit"` as an api-error end
                // (fires `StopFailure`, skips the normal `Stop` hooks, returns
                // `FallThrough`), so the directive is discarded and the normal
                // end-of-turn tail runs — exactly mirroring the batched path.
                let _ = self
                    .handle_stop_at_end(
                        "blocking_limit",
                        &mut stop_hook_active,
                        &mut stop_hook_blocking_count,
                        turn_count,
                        id,
                    )
                    .await;
                if self
                    .maybe_continue_for_budget(budget.as_mut(), &mut recovery, global_turn_tokens)
                    .await
                {
                    continue;
                }
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn("blocking_limit", &cost).await;
                final_message_id = id;
                break;
            }

            // Mid-stream tool dispatch (claude-code `query.ts:562` + `837-844`):
            // create the executor + pre-allocate this turn's assistant id BEFORE
            // opening the stream, so each `tool_use` block can be `add_tool`'d and
            // dispatched the moment it streams in (instead of collecting all
            // tool_uses and only starting them after the stream ends). This is
            // BYTE-EQUIVALENT: only WHEN tools start changes. The per-block
            // assistant JSONL persistence + per-result drain/persist still run
            // post-stream below (see `pump_stream_with_executor`'s contract).
            //
            // DEFERRED-3: hand the executor the turn's user-interrupt token (if
            // any) so it can reject in-flight/queued Cancel-behavior tools with the
            // REJECT_MESSAGE; `None` → identical to before.
            let assistant_id = MessageId::new();

            // hooks #39: MessageDisplay fires at the BEGIN of this assistant
            // message's stream (claude-code `begin(d)`, BIN off 208862320),
            // mirroring its `o={apiMessageId:d, messageId:randomUUID(),
            // turnId:r, index:0, …}` initialization. The `turn_id` is a fresh
            // per-turn UUID (`newTurn(){…; r=randomUUID()}`). Best-effort +
            // no-op when unregistered, so existing flows are byte-identical.
            let turn_id = uuid::Uuid::new_v4().to_string();
            self.fire_message_display(&turn_id, assistant_id).await;

            let mut exec = match &user_cancel {
                Some(token) => {
                    crate::streaming_executor::StreamingToolExecutor::new_with_user_cancel(
                        self,
                        token.clone(),
                    )
                }
                None => crate::streaming_executor::StreamingToolExecutor::new(self),
            };

            // #5: wall-clock from stream-open through pump completion (incl. any
            // 529→non-stream fallback) so the CostTracker records a REAL duration
            // instead of `Duration::ZERO`. Paired with
            // `self.streaming_api.last_retry_count()` at the billing site below.
            let api_call_started = std::time::Instant::now();

            // tengu_api_success `messageCount:n` / `messageTokens:r`: capture from
            // the OUTGOING snapshot BEFORE it is moved into `.stream(...)`.
            let api_success_message_count = u32::try_from(snapshot.len()).unwrap_or(u32::MAX);
            let api_success_message_tokens =
                compaction::grouping::estimate_tokens_for_range(&snapshot);

            // Either an open stream to pump, or a turn already RECOVERED from a
            // connect-phase prompt-too-long (#1, see the ContextOverflow arm).
            enum OpenOutcome {
                Stream(futures::stream::BoxStream<'static, Result<LlmEvent, LlmError>>),
                Recovered(crate::streaming_loop::PumpedTurn),
            }

            let opened = match self
                .streaming_api
                .stream(
                    &model,
                    model_profile.as_deref(),
                    system_prompt.as_deref(),
                    snapshot,
                    wire_tools.clone(),
                )
                .await
            {
                Ok(s) => OpenOutcome::Stream(s),
                // #1 (main-loop parity): a connect-phase 413 / prompt-too-long
                // surfaces HERE as `LlmError::ContextOverflow` — the adapter's
                // `drive_stream` returns `Err` on connect status >= 400, so it
                // never reaches the pump. The proactive blocking-limit preempt
                // above undershot the server's own limit, so recover REACTIVELY
                // via the SAME helper the batched path uses
                // (`call_api_with_ptl_recovery`): truncate-head xN -> one full
                // compact -> retry. On success we replay the recovered
                // non-streaming response exactly like the 529 fallback below; on
                // exhaustion we end the turn with the byte-exact prompt_too_long /
                // rapid_refill copy — identical to the proactive preempt and the
                // batched path. NOT gated on
                // `LINGXI_DISABLE_NONSTREAMING_FALLBACK` (that flag governs
                // the 529 overload fallback; PTL recovery is the always-on batched
                // behavior). Previously a streaming 413 bubbled as a hard
                // `OrchestratorError::Streaming` error (documented divergence).
                Err(LlmError::ContextOverflow { .. }) => {
                    // Re-snapshot history — the per-turn `snapshot` was MOVED into
                    // the failed `stream()` call. Prepend the additional-context
                    // meta message, like every `callModel` (claude-code `A6n`).
                    let (mut recov_snapshot, recov_model, recov_profile) = {
                        let s = self.session.lock().await;
                        (s.history.clone(), s.model.clone(), s.model_profile.clone())
                    };
                    if let Some(ctx_msg) = self.additional_context_message().await {
                        recov_snapshot.insert(0, ctx_msg);
                    }
                    match call_api_with_ptl_recovery(
                        self,
                        system_prompt.as_deref(),
                        &recov_model,
                        recov_profile.as_deref(),
                        recov_snapshot,
                        wire_tools.clone(),
                        None,
                    )
                    .await?
                    {
                        PtlCallOutcome::Response(resp) => {
                            // Replay the recovered non-streaming response exactly
                            // like the 529 fallback below: emit text live, rebuild
                            // a fresh executor, register its tool_uses, and flow on
                            // as the turn's `pumped` result.
                            let pumped_from_recovery = llm_response_to_pumped_turn(&resp);
                            for blk in &pumped_from_recovery.assistant_blocks {
                                if let ContentBlock::Text { text } = blk {
                                    self.output.emit_text(text).await;
                                }
                            }
                            exec = match &user_cancel {
                                Some(token) => {
                                    crate::streaming_executor::StreamingToolExecutor::new_with_user_cancel(
                                        self,
                                        token.clone(),
                                    )
                                }
                                None => crate::streaming_executor::StreamingToolExecutor::new(self),
                            };
                            for tu in &pumped_from_recovery.tool_uses {
                                exec.add_tool(
                                    tu.id.clone(),
                                    tu.name.clone(),
                                    tu.input.clone(),
                                    tu.provider_id.clone(),
                                    assistant_id,
                                );
                            }
                            OpenOutcome::Recovered(pumped_from_recovery)
                        }
                        PtlCallOutcome::PromptTooLong => {
                            // Reactive recovery exhausted — end with the reactive
                            // terminal reason `prompt_too_long` (`query.ts:1175`).
                            let id = surface_prompt_too_long(self).await;
                            let _ = self
                                .handle_stop_at_end(
                                    "prompt_too_long",
                                    &mut stop_hook_active,
                                    &mut stop_hook_blocking_count,
                                    turn_count,
                                    id,
                                )
                                .await;
                            if self
                                .maybe_continue_for_budget(
                                    budget.as_mut(),
                                    &mut recovery,
                                    global_turn_tokens,
                                )
                                .await
                            {
                                continue;
                            }
                            let cost = self.snapshot_cost_real().await;
                            self.output.emit_end_turn("prompt_too_long", &cost).await;
                            final_message_id = id;
                            break;
                        }
                        PtlCallOutcome::BlockingLimit => {
                            // The recovery's own PROACTIVE step-1 preempt fired on
                            // the re-snapshotted history ⇒ terminal `"blocking_limit"`
                            // (distinct from reactive-exhausted `prompt_too_long`),
                            // mirroring the batched path's `BlockingLimit` arm.
                            let id = surface_prompt_too_long(self).await;
                            let _ = self
                                .handle_stop_at_end(
                                    "blocking_limit",
                                    &mut stop_hook_active,
                                    &mut stop_hook_blocking_count,
                                    turn_count,
                                    id,
                                )
                                .await;
                            if self
                                .maybe_continue_for_budget(
                                    budget.as_mut(),
                                    &mut recovery,
                                    global_turn_tokens,
                                )
                                .await
                            {
                                continue;
                            }
                            let cost = self.snapshot_cost_real().await;
                            self.output.emit_end_turn("blocking_limit", &cost).await;
                            final_message_id = id;
                            break;
                        }
                        PtlCallOutcome::RapidRefillBreaker => {
                            // #54 reactive trip — surface the thrashing message and
                            // end with the terminal reason `rapid_refill_breaker`
                            // (the MESSAGE still carries api-error `invalid_request`),
                            // mirroring the batched path.
                            let id = surface_rapid_refill_thrashing(self).await;
                            let _ = self
                                .handle_stop_at_end(
                                    "rapid_refill_breaker",
                                    &mut stop_hook_active,
                                    &mut stop_hook_blocking_count,
                                    turn_count,
                                    id,
                                )
                                .await;
                            let cost = self.snapshot_cost_real().await;
                            self.output
                                .emit_end_turn("rapid_refill_breaker", &cost)
                                .await;
                            final_message_id = id;
                            break;
                        }
                    }
                }
                // #10: a connect-phase RateLimited/Overloaded keeps its dedicated
                // downstream handling — propagate as a hard error.
                Err(e @ (LlmError::RateLimited { .. } | LlmError::Overloaded { .. })) => {
                    return Err(OrchestratorError::Streaming(e))
                }
                // #10: any other connect-phase model/runtime error ends the turn
                // GRACEFULLY as `model_error` (faithful port of the `query.ts`
                // catch) — surface the raw error text as an api-error assistant
                // message instead of bubbling a hard error. No assistant message
                // was persisted this turn, so no orphaned tool_use to repair.
                Err(other) => {
                    // Classify the typed connect-phase error (`Flp`/`KNn`) before
                    // consuming it for the verbatim message text. Wrap into the
                    // `Streaming` variant — this is the connect-phase streaming
                    // surface — so the classifier sees the inner `LlmError`.
                    let env = classify_api_error(&OrchestratorError::Streaming(other.clone()));
                    let id =
                        crate::turn_loop::surface_model_error(self, &self.model_error_text(&other).await, env).await;
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn("model_error", &cost).await;
                    final_message_id = id;
                    break;
                }
            };

            // 3. Pump the stream (with mid-stream 529 → non-streaming fallback).
            //
            // Task 7 / claude.ts parity: if the stream errors with `LlmError::Overloaded`
            // after the first event — AND `LINGXI_DISABLE_NONSTREAMING_FALLBACK` is not
            // set — discard the partial accumulation and issue a fresh non-streaming call
            // seeded with `initial_consecutive_overloaded = 1`.  This mirrors
            // `claude.ts:2469-2594` + `withRetry.ts:186` (`initialConsecutive529Errors`).
            //
            // The env gate name is locked byte-for-byte to TS:
            //   `process.env.LINGXI_DISABLE_NONSTREAMING_FALLBACK` (claude.ts:2470)
            // Truthiness follows `isEnvTruthy` (non-empty, non-"false", non-"0").
            //
            // M1 parity note (Task 7 review): TS yields partial deltas LIVE to callers
            // as they arrive (claude.ts:2210 `yield m` fires inside the for-await loop,
            // at each `content_block_stop`).  Our path likewise dispatches text deltas live
            // via `event_router.rs` → `output.emit_text` for each `TextDelta`, so partial
            // output DOES reach callers before the fallback fires.  This matches TS: both
            // implementations dispatch partial output live, then dispatch the fallback-only
            // output after the non-streaming call completes.  The PERSISTED assistant message
            // (and the final `ConversationOutcome`) contains ONLY the fallback blocks —
            // `pumped_from_fallback.assistant_blocks` — not the discarded partial stream
            // fragments, which is correct: the partial stream never reached `content_block_stop`
            // for its text block, so no completed block was accumulated.
            // #1: a connect-phase prompt-too-long already recovered above (its
            // recovered non-streaming response was replayed) skips the pump; an
            // open stream is pumped as before.
            let pumped = match opened {
                OpenOutcome::Recovered(pumped_from_recovery) => pumped_from_recovery,
                OpenOutcome::Stream(first_stream) => {
                // cc 2.1.198 mid-response transient retry (`query.ts` stream
                // loop @219649648): on a transient network drop (ECONNRESET /
                // connection closed / reset) OR a watchdog idle-timeout, re-open
                // and re-pump the SAME streaming request with backoff — but ONLY
                // while `!real_content_started` (binary `!Hr`). Because a
                // `tool_use` block STARTING flips `real_content_started`, this
                // guard also guarantees NO tool has been dispatched, so a
                // non-idempotent tool is never re-run. The failed pump left
                // `session.history` untouched and (by the guard) the executor
                // clean, so the retry reuses `exec` and re-snapshots history.
                let mut cur_stream = first_stream;
                let mut mid_stream_retries: u32 = 0;
                let pump_outcome: Result<crate::streaming_loop::PumpedTurn, OrchestratorError> =
                    loop {
                        match crate::streaming_loop::pump_stream_with_executor_tracked(
                            cur_stream,
                            &self.output,
                            ExecutorPump {
                                executor: &mut exec,
                                assistant_id,
                                user_cancel: user_cancel.as_ref(),
                            },
                        )
                        .await
                        {
                            Ok(p) => break Ok(p),
                            Err(f)
                                if crate::streaming_loop::is_transient_mid_stream(&f.error)
                                    && !f.real_content_started
                                    && mid_stream_retries
                                        < crate::streaming_loop::mid_stream_retry_cap(&f.error) =>
                            {
                                mid_stream_retries += 1;
                                // Exponential backoff + jitter (binary `sle`).
                                let base = llm_client::model::retry::scaled_base_delay_ms(
                                    u8::try_from(mid_stream_retries - 1).unwrap_or(u8::MAX),
                                    None,
                                );
                                tokio::time::sleep(llm_client::model::retry::jittered_delay(base))
                                    .await;
                                tracing::warn!(
                                    attempt = mid_stream_retries,
                                    "mid-response transient stream error — retrying streaming request"
                                );
                                // Re-snapshot history (+ additional context) for
                                // the retry — same pattern as the 529 fallback.
                                let (mut re_snapshot, re_model, re_profile) = {
                                    let s = self.session.lock().await;
                                    (s.history.clone(), s.model.clone(), s.model_profile.clone())
                                };
                                if let Some(ctx_msg) = self.additional_context_message().await {
                                    re_snapshot.insert(0, ctx_msg);
                                }
                                match self
                                    .streaming_api
                                    .stream(
                                        &re_model,
                                        re_profile.as_deref(),
                                        system_prompt.as_deref(),
                                        re_snapshot,
                                        wire_tools.clone(),
                                    )
                                    .await
                                {
                                    Ok(s) => {
                                        cur_stream = s;
                                        continue;
                                    }
                                    // Re-open failed: surface as the terminal
                                    // pump error for the arms below.
                                    Err(e) => break Err(OrchestratorError::Streaming(e)),
                                }
                            }
                            Err(f) => break Err(f.error),
                        }
                    };
                match pump_outcome {
                Ok(p) => p,
                Err(OrchestratorError::Streaming(
                    ref e @ (LlmError::Overloaded { .. } | LlmError::ProviderInternal),
                )) if !is_env_truthy(
                    std::env::var("LINGXI_DISABLE_NONSTREAMING_FALLBACK")
                        .as_deref()
                        .ok(),
                ) =>
                {
                    // Seed: a streaming overload counts as 1 toward the consecutive
                    // 529 budget (LlmError::Overloaded = 529).  Other in-band errors
                    // (e.g. ProviderInternal) seed 0 — matching TS
                    // `is529Error(streamingError) ? 1 : 0` (claude.ts:2559).
                    let seed: u8 = u8::from(matches!(e, LlmError::Overloaded { .. }));

                    // Re-snapshot history for the non-streaming call (the partial
                    // stream never touched session.history, so it is still the same
                    // snapshot we used for the stream — no reset needed).
                    let (mut non_stream_snapshot, non_stream_model, non_stream_profile) = {
                        let s = self.session.lock().await;
                        (s.history.clone(), s.model.clone(), s.model_profile.clone())
                    };
                    // R-P1c/R-P1d: claude-code's `A6n` prepends the additional-
                    // context meta message on EVERY `callModel`, including this
                    // non-streaming fallback. Prepend it to the re-snapshot too.
                    if let Some(ctx_msg) = self.additional_context_message().await {
                        non_stream_snapshot.insert(0, ctx_msg);
                    }
                    let tools_for_fallback = wire_tools.clone();

                    let resp = self
                        .api
                        .messages_create_seeded(
                            &non_stream_model,
                            non_stream_profile.as_deref(),
                            system_prompt.as_deref(),
                            non_stream_snapshot,
                            tools_for_fallback,
                            seed,
                        )
                        .await
                        .map_err(OrchestratorError::ApiCall)?;

                    // Convert LlmResponse → PumpedTurn so the rest of the streaming
                    // turn loop can proceed identically.
                    let pumped_from_fallback = llm_response_to_pumped_turn(&resp);

                    // Emit text blocks from the non-streaming response to the output
                    // stream, mirroring the batched path (turn_loop.rs step 4:
                    // `orch.output.emit_text(text).await`).  In the normal streaming
                    // path `pump_stream` calls `dispatch_event` → `emit_text` for each
                    // `TextDelta`; the non-streaming path has no SSE events, so we
                    // replicate the whole-body emit here.
                    for blk in &pumped_from_fallback.assistant_blocks {
                        if let ContentBlock::Text { text } = blk {
                            self.output.emit_text(text).await;
                        }
                    }

                    // claude-code `query.ts:733-740`: discard the partial
                    // streaming attempt's executor (its tool_uses have stale ids
                    // and would orphan against the fallback response) and replace
                    // it with a fresh one. Dropping the old executor cancels any
                    // in-flight tool futures it had started mid-stream. The fresh
                    // executor's tools are registered from the FALLBACK response's
                    // tool_uses by the post-stream drive loop below (this is the
                    // ONLY path that still `add_tool`s after the stream — the
                    // normal path registers mid-stream).
                    exec = match &user_cancel {
                        Some(token) => {
                            crate::streaming_executor::StreamingToolExecutor::new_with_user_cancel(
                                self,
                                token.clone(),
                            )
                        }
                        None => crate::streaming_executor::StreamingToolExecutor::new(self),
                    };
                    for tu in &pumped_from_fallback.tool_uses {
                        exec.add_tool(
                            tu.id.clone(),
                            tu.name.clone(),
                            tu.input.clone(),
                            tu.provider_id.clone(),
                            assistant_id,
                        );
                    }

                    pumped_from_fallback
                }
                // #10: RateLimited/Overloaded/RepeatedOverloaded keep dedicated
                // downstream handling — propagate.
                Err(e) if crate::turn_loop::is_carveout_propagated(&e) => return Err(e),
                // #10: any other mid-stream model/runtime error (e.g. Transport)
                // ends the turn GRACEFULLY as `model_error` (faithful port of the
                // `query.ts` catch) rather than bubbling a hard error / phantom
                // interrupt. The assistant message for this turn is persisted only
                // AFTER a successful pump, so the errored pump left no orphaned
                // tool_use to repair (TS `yieldMissingToolResultBlocks` no-op here).
                Err(other) => {
                    // Classify the typed mid-stream error (`Flp`/`KNn`) into the
                    // api-error envelope; the message text stays verbatim.
                    let env = classify_api_error(&other);
                    let id =
                        crate::turn_loop::surface_model_error(self, &other.to_string(), env)
                            .await;
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn("model_error", &cost).await;
                    final_message_id = id;
                    break;
                }
                }
                }
            };
            // A3: accumulate this turn's output tokens (TS `getTurnOutputTokens()`).
            global_turn_tokens = global_turn_tokens.saturating_add(pumped.output_tokens);

            // BILLING: record streaming-turn usage into CostTracker — mirrors the
            // non-streaming path in `turn_loop.rs`. #5 (main-loop parity): pass
            // the REAL wall-clock duration (stream-open → pump completion) and
            // the REAL connect-phase retry count (`last_retry_count()`) instead
            // of the previous hardcoded `Duration::ZERO` / `0`.
            if let Some(ref usage) = pumped.usage {
                // #55: cache this response's total input tokens (the `Xtt`
                // last-usage snapshot) for the fixed-prefix overflow guard.
                self.record_response_input_tokens(usage);
            }
            if let Some(tracker) = self.cost_tracker.as_ref() {
                if let Some(ref usage) = pumped.usage {
                    let cost_usage = crate::cost_wiring::llm_usage_to_cost_usage(usage);
                    let cache_read = usage.billable_tokens.cache_read;
                    let cache_create = usage.billable_tokens.cache_write;
                    let model_ref =
                        crate::cost_wiring::model_ref_from_string(&model, model_profile.as_deref());
                    let elapsed = api_call_started.elapsed();
                    let retries = self.streaming_api.last_retry_count();
                    let cost_for_this_call = tracker
                        .record_api_response_v2(
                            model_ref.clone(),
                            cost_usage,
                            elapsed,
                            retries,
                            cache_read,
                            cache_create,
                            false, // is_batch_request — streaming is never batch
                            self.analytics_bus.as_ref(),
                        )
                        .await;
                    // strict-parity (2.1.195): fire `tengu_api_success` on the
                    // streaming per-request success path (claude
                    // `j("tengu_api_success", {...})`). `tengu_cost_recorded`
                    // was port-only and dropped.
                    if let Some(bus) = self.analytics_bus.as_ref() {
                        #[allow(clippy::cast_possible_truncation)]
                        let dur_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
                        cost::emit_api_success(
                            bus,
                            &cost::ApiSuccessFields {
                                model: model.clone(),
                                input_tokens: usage.billable_tokens.input,
                                output_tokens: usage.billable_tokens.output,
                                cached_input_tokens: cache_read,
                                uncached_input_tokens: cache_create,
                                duration_ms: dur_ms,
                                duration_ms_including_retries: dur_ms,
                                attempt: retries + 1,
                                cost_nano_usd: cost_for_this_call,
                                provider: crate::cost_wiring::provider_tag(&model_ref.provider),
                                stop_reason: pumped.stop_reason.clone(),
                                request_id: self.api.last_request_id(),
                                message_count: api_success_message_count,
                                message_tokens: api_success_message_tokens,
                                // The 529 non-streaming fallback flows through this
                                // SAME emit; threading a real flag through
                                // `PumpedTurn` is deferred, so `false` uniformly
                                // (documented close divergence vs claude `m`).
                                did_fall_back_to_non_streaming: false,
                                is_non_interactive_session:
                                    traits::session_flags::is_non_interactive_session(),
                                print: traits::session_flags::is_non_interactive_session(),
                                is_tty: false,
                                query_source: "user".into(),
                                permission_mode: if self.session.lock().await.plan_mode {
                                    "plan"
                                } else {
                                    "default"
                                }
                                .to_string(),
                                ttft_ms: None,
                                fast_mode: usage.speed.as_deref() == Some("fast"),
                                time_since_last_api_call_ms: self.record_api_call_gap_ms(),
                            },
                        )
                        .await;
                    }
                    self.api_calls_recorded
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }

            // In-Loop Compaction Batch 6: snapshot the cache-safe prompt prefix
            // after a successful stream (streaming twin of the batched save).
            // `session.history` here equals the streamed snapshot — the streaming
            // path does not mutate history mid-call — taken before the assistant
            // reply is appended below. Strict no-op when no slot is wired.
            self.save_cache_safe_params(system_prompt.as_deref(), &model)
                .await;

            // Task 8 (llm-client future-work batch 3): the streamed call (or
            // its non-streaming 529 fallback) completed — forward the
            // adapter's unified rate-limit snapshot when it changed since the
            // last emission. `self.api` is the same `ProviderApiAdapter` as
            // `self.streaming_api` in production; the adapter records headers
            // on the `drive_stream` connect-success path too.
            self.emit_rate_limit_if_changed().await;
            // Task 2 (batch 5): same seam, raw per-window snapshot.
            self.emit_raw_utilization_if_changed().await;

            // 4. Assemble + append the assistant message.
            // `assistant_id` was pre-allocated before the stream (mid-stream
            // dispatch hands it to the executor as each tool registers).
            let mut blocks: Vec<ContentBlock> = pumped.assistant_blocks.clone();
            for t in &pumped.tool_uses {
                blocks.push(ContentBlock::ToolUse {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    input: t.input.clone(),
                    provider_id: t.provider_id.clone(),
                });
            }
            let assistant_msg = ConversationMessage::Assistant {
                id: assistant_id,
                content: blocks,
                stop_reason: pumped.stop_reason.clone(),
            };
            {
                let mut s = self.session.lock().await;
                s.history.push(assistant_msg.clone());
            }

            // Finding #73 (streaming twin): advance the per-turn todo/task
            // reminder counters for THIS assistant turn, then reset
            // `turns_since_last_todo_write` if this turn invoked the variant's
            // "recent use" tool (TodoWrite / TaskCreate / TaskUpdate). Bump THEN
            // reset so a TodoWrite turn lands at 0 (matching the binary scan that
            // excludes the TodoWrite message itself). Mirrors the batched twin.
            self.bump_reminder_turn_counters().await;
            let invoked_tool_names: Vec<String> =
                pumped.tool_uses.iter().map(|t| t.name.clone()).collect();
            self.note_todo_reminder_tool_call(&invoked_tool_names).await;

            // DEFERRED-3: advance the interrupt-guard's reported final id to this
            // turn's assistant message.
            last_message_id = assistant_id;
            // WRITE-side per-block split (claude.ts:2171-2211): persist the turn
            // as one single-block assistant JSONL line per content block, sharing
            // the turn's inner `message.id` with distinct top-level uuids, and
            // capture each `tool_use`'s line uuid so its `tool_result` parents to
            // ITS line (TS `sourceToolAssistantUUID`) — NOT one shared per-turn
            // parent. The in-memory `s.history` above stays the single merged
            // assistant message (the Anthropic request needs all blocks in one
            // assistant turn).
            // Raw Anthropic `usage` object for the persisted BetaMessage envelope
            // (the streaming codec retains it on `Usage::provider_metadata`).
            let assistant_usage = pumped.usage.as_ref().map(assistant_usage_value);
            // The Anthropic `request-id` response header for THIS turn → the
            // top-level `requestId` on each persisted assistant line. The
            // adapter records it from the stream connect-success / non-stream
            // headers (the same `record_rate_limit_from_headers` pass), so it is
            // the just-completed call's id here. `self.api` is the same adapter
            // as `self.streaming_api` in production; the fallback non-stream call
            // records it on `self.api` too.
            let request_id = self.api.last_request_id();
            // stream-json P1: signal the message boundary to the output sink so
            // `StreamJsonStream` can flush its accumulated assistant frame.
            self.output
                .emit_message_boundary(pumped.stop_reason.as_deref(), request_id.as_deref())
                .await;
            let tool_use_parent_uuids = self
                .persist_assistant_per_block(
                    &assistant_msg,
                    assistant_usage.as_ref(),
                    request_id.as_deref(),
                )
                .await;
            // Fallback parent (the LAST persisted block's uuid) for any
            // tool_result whose tool_use id is missing from the map (defensive).
            let assistant_uuid = self.last_jsonl_uuid.lock().await.clone();

            // #5 aborted_streaming vs aborted_tools disambiguation (faithful port
            // of claude-code's TWO distinct abort checkpoints): query.ts:1015 runs
            // RIGHT AFTER `callModel`, BEFORE the tool-completion drive — an abort
            // observed there is `aborted_streaming` + `createUserInterruptionMessage({toolUse:false})`
            // (`[Request interrupted by user]`). query.ts:1485 runs AFTER the tool
            // drive — an abort observed only there is `aborted_tools` +
            // `createUserInterruptionMessage({toolUse:true})`
            // (`[Request interrupted by user for tool use]`). Capture the
            // checkpoint-1015 state HERE (before the drive); the single post-drive
            // abort check below picks the reason/message from it. The common
            // "interrupt during thinking/streaming" case (incl. no-tool responses)
            // lands as `aborted_streaming`, not the previous mislabel
            // `aborted_tools`. `None` token (plain `run_turn_streaming`) → always
            // false → byte-identical to before. The drive loop below still flushes
            // synthetic REJECT_MESSAGE tool_results (the executor was built with the
            // cancel token, so pending/queued tools reject rather than execute) —
            // matching ref's `getRemainingResults()` at the 1015 path.
            let aborted_during_stream = user_cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled);

            // 5. Drive tools through the StreamingToolExecutor (faithful port of
            //    claude-code's `StreamingToolExecutor` + `query.ts:826-862`).
            //    Each tool runs the same hook + permission + registry pipeline
            //    (via `dispatch_tool_uses_tracked` per tool) under concurrency
            //    control, and EACH result is persisted as its OWN `user` message
            //    parented to the originating assistant (per-result,
            //    assistant-parented topology), in RECEIVED order — diverging from
            //    the old single-batched-user-message shape and matching the TS
            //    `sessionStorage` `sourceToolAssistantUUID → parentUuid` mapping.
            if !pumped.tool_uses.is_empty() {
                // The executor (`exec`) was created BEFORE the stream and its
                // tools were registered + dispatched MID-STREAM by
                // `pump_stream_with_executor` (claude-code `query.ts:837-844`);
                // on the non-streaming 529-fallback path it was rebuilt above and
                // its tools added from the fallback response. Here we only DRIVE
                // it to completion + persist — `add_tool` no longer happens
                // post-stream on the normal path.
                //
                // Drive to completion, persisting each result IN RECEIVED ORDER
                // as its own user message parented to the originating assistant.
                let mut all_modifiers: Vec<tool_api::ContextModifier> = Vec::new();
                loop {
                    // (no-op unless a Bash sibling errored / the turn discarded)
                    exec.apply_abort_to_pending();
                    exec.process_queue();
                    // persist whatever just completed, in order
                    for drained in exec.take_newly_completed() {
                        // Parent this tool_result to ITS tool_use's per-block
                        // assistant line uuid (TS `sourceToolAssistantUUID`),
                        // falling back to the turn's last assistant block uuid if
                        // the id isn't in the map (defensive — e.g. an append that
                        // failed and was skipped above).
                        let parent_uuid = match &drained.block {
                            ContentBlock::ToolResult { tool_use_id, .. } => tool_use_parent_uuids
                                .get(tool_use_id)
                                .cloned()
                                .or_else(|| assistant_uuid.clone()),
                            _ => assistant_uuid.clone(),
                        };
                        let user_msg = ConversationMessage::User {
                            id: MessageId::new(),
                            content: vec![drained.block],
                            is_meta: false,
                        };
                        {
                            let mut s = self.session.lock().await;
                            s.history.push(user_msg.clone());
                        }
                        self.persist_message_to_jsonl_with_parent(&user_msg, parent_uuid)
                            .await;
                        // SKILLEXEC.3 (streaming): replay tool-injected
                        // `new_messages` (the Skill tool's expanded prompt) right
                        // after the tool_result, recording each injected message's
                        // id → originating tool_use_id into the in-memory
                        // `injected_message_sources` side-table (faithful port of
                        // TS `sourceToolUseID`). These chain normally (NOT a parent
                        // override) — matching the batched path — so the JSONL
                        // bytes stay byte-identical. Empty for every non-skill tool
                        // → strict no-op.
                        for (m, tool_use_id) in drained.injected {
                            {
                                let mut s = self.session.lock().await;
                                s.history.push(m.clone());
                                s.injected_message_sources.insert(m.id(), tool_use_id);
                            }
                            self.persist_message_to_jsonl(&m).await;
                        }
                        all_modifiers.extend(drained.modifiers);
                    }
                    // Guaranteed-progress shape (mirrors the test-only
                    // `run_to_completion`): an empty in-flight set after
                    // `process_queue` means no Queued tool remains startable
                    // (process_queue starts any runnable one) and nothing is
                    // executing — so every tool is done + drained above. Break
                    // here, else drain one future below. Never spins: each
                    // iteration either breaks or `.await`s a completion.
                    if exec.inflight_is_empty() {
                        break;
                    }
                    exec.drain_one().await;
                }
                // SKILLEXEC.3 (model scope, streaming twin): fold this turn's
                // `context_modifier`s and switch `session.model` if a skill
                // declared a `model:` override. Applied AFTER all results +
                // injected messages, so there is no race on `session.model`.
                // Empty for every non-`model:` tool → strict no-op.
                crate::turn_loop::apply_model_context_modifiers(self, all_modifiers).await;
            }

            // DEFERRED-3 / esc-interrupt FIX: "we were aborted" — the single
            // post-drive abort checkpoint. Once the user-interrupt token has fired,
            // the executor above already drained the bare REJECT_MESSAGE
            // `tool_result`s into history (model-visible). The turn MUST now STOP —
            // claude-code returns with NO further `callModel`, honoring
            // REJECT_MESSAGE's "STOP what you are doing and wait for the user".
            // Looping into the `Some("tool_use") => continue` arm below would (1)
            // issue a wasted extra round-trip after every ESC and (2) let a
            // Block-behavior tool emitted on that continuation actually EXECUTE
            // (`abort_reason_for` returns `None` for Block tools) despite the
            // interrupt — both of which claude-code structurally prevents by
            // returning here first. `None` token (plain `run_turn_streaming`) →
            // never fires → identical to before.
            //
            // #5: the terminal reason + interrupt message depend on WHICH ref
            // checkpoint observed the abort (captured in `aborted_during_stream`
            // before the drive): an abort already set when the stream ended is
            // `aborted_streaming` / `[Request interrupted by user]` (query.ts:1015,
            // `toolUse:false`); an abort that fired only DURING the tool drive is
            // `aborted_tools` / `[Request interrupted by user for tool use]`
            // (query.ts:1485, `toolUse:true`).
            if user_cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                let cost = self.snapshot_cost_real().await;
                let (abort_reason, interrupt_message) = if aborted_during_stream {
                    ("aborted_streaming", INTERRUPT_MESSAGE)
                } else {
                    ("aborted_tools", INTERRUPT_MESSAGE_FOR_TOOL_USE)
                };
                self.output.emit_end_turn(abort_reason, &cost).await;
                // NOW-ABORT disambiguation: a `Now`-driven cancellation means the
                // urgent queued command will run next via the between-turn drain —
                // DON'T inject the user-interrupt message. For a plain user
                // interrupt (the default with no reason flag wired) inject as
                // before. claude-code `query.ts:1046-1050`/`1501-1505`:
                // `createUserInterruptionMessage`.
                if self.cancel_reason_now()
                    != crate::prompt::mid_turn_input::CancelReason::QueueNowCommand
                {
                    self.inject_meta_user_message(interrupt_message).await;
                }
                final_message_id = assistant_id;
                break;
            }

            // #78 nudge guard `!Pt(ce)` (streaming twin): suppress the
            // thinking-only nudge during a StructuredOutput exchange. Computed
            // before the match (a match guard cannot `.await` the session lock);
            // reused by all three streaming nudge sites (end_turn / stop_sequence
            // / missing). The current assistant response is already in `history`.
            let prior_structured_output = {
                let s = self.session.lock().await;
                crate::turn_loop::prior_assistant_used_structured_output(&s.history)
            };

            // 6. Decide loop disposition.
            match pumped.stop_reason.as_deref() {
                // #1 needsFollowUp gate (claude-code `query.ts:554-558`/`832-835`/
                // `1062`): continuation is keyed on tool-block PRESENCE, NOT the raw
                // `stop_reason` string (the ref notes `stop_reason == "tool_use"` is
                // "unreliable"). Any response that dispatched tool_use blocks runs
                // the tools (already driven above) AND continues — feeding the
                // tool_results back — regardless of whether the stop_reason was
                // `tool_use`, `end_turn`, `stop_sequence`, or a truncated
                // `max_tokens` that still carried a complete tool block. Fires only
                // when tools were dispatched; a withheld `max_output_tokens`
                // response carries NO tool_uses and falls through to the recovery/
                // terminal arms below. Subsumes the former
                // `Some("tool_use") if !pumped.tool_uses.is_empty()` arm.
                _ if !pumped.tool_uses.is_empty() => {
                    // EndConversation (2.1.206, streaming twin): a 2nd
                    // consecutive EndConversation call raised the shared
                    // end-request slot during tool dispatch above. Consume it;
                    // if raised, surface the end message and terminate instead
                    // of continuing. Default-OFF (no slot wired) → strict no-op
                    // → byte-identical to before.
                    if self
                        .end_conversation_slot
                        .as_ref()
                        .is_some_and(|s| s.swap(false, std::sync::atomic::Ordering::SeqCst))
                    {
                        self.output
                            .emit_text(
                                crate::prompt::end_conversation::END_CONVERSATION_ENDED_MESSAGE,
                            )
                            .await;
                        return Ok(ConversationOutcome::EndTurn {
                            turn_count,
                            final_message_id: assistant_id,
                        });
                    }
                    continue;
                }
                Some("end_turn") => {
                    // #78 thinking-only nudge (claude-code `bin/claude.exe`
                    // offset ~202946760): an `end_turn` response with no visible
                    // text gets ONE nudge to produce user-visible output. This
                    // fires BEFORE the Stop hooks (binary order: malformed →
                    // thinking-only → stop-hooks → budget), so a thinking-only
                    // turn re-prompts the model without first running Stop hooks.
                    // The `a !== "compact" && !GRe(a)` source guard is satisfied
                    // unconditionally here (compact subturns run in a separate
                    // code path — `CompactionOrchestrator` — never this loop), and
                    // `!isApiErrorMessage` holds because API errors are caught as
                    // `Err(..)` upstream of this match. Once nudged, a still-empty
                    // continuation falls through to the normal end.
                    if !thinking_only_nudged
                        && !pumped_has_visible_text(&pumped.assistant_blocks)
                        && !prior_structured_output
                    {
                        self.inject_meta_user_message(THINKING_ONLY_NUDGE).await;
                        thinking_only_nudged = true;
                        continue;
                    }
                    // hooks B4: Stop hooks BEFORE the budget check (streaming
                    // twin; order recovery → stop-hooks → token-budget).
                    match self
                        .handle_stop_at_end(
                            "end_turn",
                            &mut stop_hook_active,
                            &mut stop_hook_blocking_count,
                            turn_count,
                            assistant_id,
                        )
                        .await
                    {
                        StopHookFlow::Terminate(outcome) => return Ok(outcome),
                        StopHookFlow::TerminateMaxTurns => {
                            return Err(OrchestratorError::MaxTurnsReached {
                                max_turns: self.config.max_turns,
                            });
                        }
                        StopHookFlow::LoopAgain => {
                            // RECOV.4: a Stop hook forced the loop to continue —
                            // reset the max_output_tokens recovery bookkeeping so the
                            // continued turn starts a fresh escalation episode (TS
                            // `query.ts:1291` sets `maxOutputTokensRecoveryCount: 0`
                            // + `maxOutputTokensOverride: undefined` on the
                            // stop-hook-blocking continuation).
                            recovery.reset_max_output_tokens_recovery();
                            continue;
                        }
                        StopHookFlow::FallThrough => {}
                    }
                    // A3: token-budget continuation (streaming twin). On a
                    // natural end, consult the budget; on `continue`, inject the
                    // meta nudge, reset the A1 recovery count, and loop again.
                    if self
                        .maybe_continue_for_budget(
                            budget.as_mut(),
                            &mut recovery,
                            global_turn_tokens,
                        )
                        .await
                    {
                        continue;
                    }
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn("end_turn", &cost).await;
                    final_message_id = assistant_id;
                    break;
                }
                // #77 malformed-tool-use retry (claude-code `bin/claude.exe`
                // offset ~202945837): `stop_reason == "tool_use"` but the
                // assistant produced ZERO tool_use blocks (a malformed /
                // leaked-invoke response). On the FIRST such failure, inject the
                // byte-exact meta retry nudge, reset the max-output-tokens
                // recovery bookkeeping (TS resets `maxOutputTokensRecoveryCount:
                // 0` + `hasAttemptedReactiveCompact: false`), arm the guard, and
                // loop. On the SECOND (`malformed_tool_use_retried` already set),
                // surface the non-meta terminal message and end the turn. The
                // `!isApiErrorMessage` guard holds (API errors are caught
                // upstream as `Err(..)`). Default build keeps the clean-retry
                // feature flag (`PZa()`) OFF, so we do NOT tombstone the leaked
                // assistant blocks and use the non-clean-retry nudge string.
                Some("tool_use") => {
                    if malformed_tool_use_retried {
                        // Second failure → terminal NON-meta message, complete.
                        // Binary `ql(...)`→`mcc({isApiErrorMessage:!0})`: an
                        // ASSISTANT api-error message (`role:"assistant",
                        // stop_reason:"stop_sequence", stop_details:null`) appended
                        // after the malformed assistant response (two assistants in
                        // a row, matching the binary). Shape mirrors
                        // `surface_model_error`. (Was a USER message.)
                        self.output.emit_text(MALFORMED_TOOL_USE_RETRY_FAILED).await;
                        let failed_msg = ConversationMessage::Assistant {
                            id: MessageId::new(),
                            content: vec![ContentBlock::Text {
                                text: MALFORMED_TOOL_USE_RETRY_FAILED.to_string(),
                            }],
                            stop_reason: Some("stop_sequence".to_string()),
                        };
                        {
                            let mut s = self.session.lock().await;
                            s.history.push(failed_msg.clone());
                        }
                        self.persist_message_to_jsonl(&failed_msg).await;
                        let cost = self.snapshot_cost_real().await;
                        self.output.emit_end_turn("end_turn", &cost).await;
                        final_message_id = assistant_id;
                        break;
                    }
                    self.inject_meta_user_message(MALFORMED_TOOL_USE_RETRY_NUDGE)
                        .await;
                    // TS resets the recovery counters on the retry transition so
                    // the continued turn starts a fresh max-output-tokens
                    // escalation episode.
                    recovery.reset_max_output_tokens_recovery();
                    malformed_tool_use_retried = true;
                    continue;
                }
                // A1: intercept `max_tokens` BEFORE the generic terminal arm.
                // While recovery is not exhausted, inject the byte-exact meta
                // nudge user message, increment the counter, and Continue
                // (TS `query.ts:1223-1252`). On exhaustion, fall through to the
                // generic terminal below (end with stop_reason `max_tokens`).
                Some("max_tokens")
                    if recovery.max_output_tokens_recovery_count
                        < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT =>
                {
                    // The nudge is a plain user text message carrying the
                    // byte-exact string (the protocol has no `isMeta` flag).
                    let nudge_msg = ConversationMessage::user(
                        MessageId::new(),
                        MAX_OUTPUT_TOKENS_RECOVERY_NUDGE.to_string(),
                    );
                    {
                        let mut s = self.session.lock().await;
                        s.history.push(nudge_msg.clone());
                    }
                    self.persist_message_to_jsonl(&nudge_msg).await;
                    recovery.max_output_tokens_recovery_count =
                        recovery.max_output_tokens_recovery_count.saturating_add(1);
                    recovery.max_output_tokens_override = None;
                    continue;
                }
                // #78 thinking-only nudge for `stop_sequence` (claude-code
                // groups `end_turn` and `stop_sequence` under one guard). A
                // `stop_sequence` response with no visible text gets the same
                // once-per-turn nudge before terminating. Intercepted ahead of
                // the generic terminal arm; once nudged it falls through.
                Some("stop_sequence")
                    if !thinking_only_nudged
                        && !pumped_has_visible_text(&pumped.assistant_blocks)
                        && !prior_structured_output =>
                {
                    self.inject_meta_user_message(THINKING_ONLY_NUDGE).await;
                    thinking_only_nudged = true;
                    continue;
                }
                // Finding #80 (streaming twin): a `refusal` response swaps to the
                // configured `refusalFallbackModel` ONCE per session and retries.
                // Intercepted ahead of the generic terminal arm; when no fallback
                // is configured (or the latch is already set) it falls through to
                // the terminal `Some(other)` arm below, byte-identical to before.
                Some("refusal") if self.maybe_swap_to_refusal_fallback().await => {
                    continue;
                }
                Some(other) => {
                    // max_tokens (recovery exhausted) / stop_sequence (visible
                    // text or already nudged) / pause_turn / refusal (no fallback
                    // configured / already latched) — terminate the loop with the
                    // value as-is, mirroring claude-code's behavior (claude.ts:2269).
                    //
                    // First surface the byte-locked user-visible `API Error: …`
                    // assistant message claude-code emits for the terminal
                    // stop_reasons it reports as errors (`claude.ts:2266`
                    // max_tokens [recovery exhausted], `:2279`
                    // model_context_window_exceeded). A strict no-op for every
                    // other terminal (stop_sequence / pause_turn /
                    // refusal-without-fallback), so those end byte-identically to
                    // before. Mirrors `surface_prompt_too_long` (persist a new
                    // assistant message carrying the error text + the originating
                    // stop_reason, then emit it).
                    // Build the byte-locked `API Error: …` via the shared
                    // [`crate::turn_loop::terminal_api_error_text`] (the batched
                    // twin uses the SAME builder, so both paths surface identical
                    // text). `None` for stop_sequence / pause_turn / refusal-
                    // without-fallback's other terminals → no message, end as-is.
                    let api_error: Option<String> = {
                        let model = self.session.lock().await.model.clone();
                        let request_id = self.api.last_request_id();
                        crate::turn_loop::terminal_api_error_text(
                            &model,
                            self.config.interactive_permissions,
                            other,
                            request_id.as_deref(),
                            pumped.stop_details.as_ref(),
                        )
                    };
                    if let Some(text) = api_error {
                        let err_msg = ConversationMessage::Assistant {
                            id: MessageId::new(),
                            content: vec![ContentBlock::Text { text: text.clone() }],
                            stop_reason: Some(other.to_string()),
                        };
                        self.session.lock().await.history.push(err_msg.clone());
                        self.persist_message_to_jsonl(&err_msg).await;
                        self.output.emit_text(&text).await;
                    }
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn(other, &cost).await;
                    final_message_id = assistant_id;
                    break;
                }
                None => {
                    // Stream ended without a stop_reason — treat as
                    // end_turn (rare; claude.ts uses the same fallback). The
                    // token-budget check applies here too (A3).
                    // #78: a missing stop_reason is treated as `end_turn`
                    // (claude-code `stop_reason ?? <default>`), so the
                    // thinking-only nudge applies here on the same terms and,
                    // like the `end_turn` arm, fires BEFORE the Stop hooks.
                    if !thinking_only_nudged
                        && !pumped_has_visible_text(&pumped.assistant_blocks)
                        && !prior_structured_output
                    {
                        self.inject_meta_user_message(THINKING_ONLY_NUDGE).await;
                        thinking_only_nudged = true;
                        continue;
                    }
                    // hooks B4: Stop hooks before the budget check (same as the
                    // explicit end_turn arm).
                    match self
                        .handle_stop_at_end(
                            "end_turn",
                            &mut stop_hook_active,
                            &mut stop_hook_blocking_count,
                            turn_count,
                            assistant_id,
                        )
                        .await
                    {
                        StopHookFlow::Terminate(outcome) => return Ok(outcome),
                        StopHookFlow::LoopAgain => {
                            // RECOV.4: a Stop hook forced the loop to continue —
                            // reset the max_output_tokens recovery bookkeeping so the
                            // continued turn starts a fresh escalation episode (TS
                            // `query.ts:1291` sets `maxOutputTokensRecoveryCount: 0`
                            // + `maxOutputTokensOverride: undefined` on the
                            // stop-hook-blocking continuation).
                            recovery.reset_max_output_tokens_recovery();
                            continue;
                        }
                        StopHookFlow::TerminateMaxTurns => {
                            return Err(OrchestratorError::MaxTurnsReached {
                                max_turns: self.config.max_turns,
                            });
                        }
                        StopHookFlow::FallThrough => {}
                    }
                    if self
                        .maybe_continue_for_budget(
                            budget.as_mut(),
                            &mut recovery,
                            global_turn_tokens,
                        )
                        .await
                    {
                        continue;
                    }
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn("end_turn", &cost).await;
                    final_message_id = assistant_id;
                    break;
                }
            }
        }

        // (/rewind) Persist THIS turn's now-populated file-history snapshot to the
        // transcript — `track_edit` filled its backup map (pre-edit content of
        // every file Edit/Write/NotebookEdit touched) during the turn above.
        // Restore (`rewind_from_disk`) and `--resume` rebuild the index from
        // these lines, so the record must carry the backups, not the empty map
        // it had at turn start. Persisted unconditionally (an edit-free turn
        // still records a restore point for conversation-only rewind).
        if let Some(fh) = &self.file_history {
            if let (Some(record), Some(writer)) =
                (fh.snapshot_record(file_history_msg_id), &self.jsonl_writer)
            {
                let session_uuid = self.session.lock().await.session_id.as_uuid().to_string();
                let line = session::file_history::snapshot_line_json(&session_uuid, &record);
                let _ = writer.append_file_history_snapshot(&line).await;
            }
        }

        Ok(ConversationOutcome::EndTurn {
            turn_count,
            final_message_id,
        })
    }

    /// Drive one user prompt through a REPL turn until `end_turn`,
    /// `max_turns`, or the given `cancel` token fires.
    ///
    /// Unlike [`Self::run_turn`] this method:
    /// - returns [`TurnOutcome`] so the REPL can distinguish natural end /
    ///   max-turns / cancellation.
    /// - takes a [`CancellationToken`] that fires on Ctrl+C (SIGINT); the
    ///   orchestrator checks it before each API round-trip.
    /// - does NOT close the session — the session accumulates messages across
    ///   REPL turns; the REPL persists via the JSONL writer when it exits.
    ///
    /// Telemetry: emits `CONVERSATION_STARTED` at entry; delegates to the
    /// same turn-loop body as `run_turn` (via `try_run_turn_cancelable`).
    pub async fn run_turn_with_cancel(
        &self,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, OrchestratorError> {
        tracing::info!(
            event = orch_events::CONVERSATION_STARTED,
            prompt_len = prompt.len()
        );
        let result = self.try_run_turn_cancelable(prompt, cancel).await;
        self.emit_terminal_rate_limit_if_changed(&result).await;
        result.map_err(|e| self.enrich_api_error(e))
    }

    /// Internal implementation of the REPL turn loop with cancellation.
    async fn try_run_turn_cancelable(
        &self,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, OrchestratorError> {
        // 0. Build the system prompt (same as non-cancelable path).
        let system_prompt: Option<String> = match &self.config.system_prompt_override {
            Some(custom) => Some(custom.clone()),
            None => Some(self.build_system_prompt().await),
        };

        // 1. Append the user prompt to session history.
        let user_msg = ConversationMessage::user(MessageId::new(), prompt.to_string());
        {
            let mut s = self.session.lock().await;
            s.history.push(user_msg.clone());
        }
        self.persist_message_to_jsonl(&user_msg).await;

        // hooks B4: UserPromptSubmit (cancelable REPL twin). A Block aborts the
        // turn before any API call. No-op when unregistered.
        if self.fire_user_prompt_submit(prompt).await {
            return Ok(TurnOutcome::EndTurn);
        }

        // 2. Turn-by-turn loop — check cancel before each API call.
        //
        // #2 (main-loop parity): the cancelable driver is recovery- AND
        // budget-aware, identical to the non-cancelable [`Self::run_turn`]
        // batched loop, except each API round-trip is raced against `cancel`.
        // The legacy no-recovery shim ([`execute_one_turn`]) is no longer used
        // here: a `max_tokens` stop_reason now drives the A1 multi-turn recovery
        // nudge (and exhaustion-ends) exactly as the main batched path does,
        // rather than legacy-continuing without the nudge.
        let mut recovery = RecoveryState::default();
        // hooks B4: Stop-hook re-entry guard (cancelable twin).
        let mut stop_hook_active = false;
        // #2 consecutive Stop-hook block counter (binary `stopHookBlockingCount`):
        // bumped per block; ends the turn via the cap once it would exceed
        // LINGXI_STOP_HOOK_BLOCK_CAP (default 8). Fresh per turn-driver run.
        let mut stop_hook_blocking_count: u32 = 0;
        // A3: token-budget continuation bookkeeping (no-op unless gated + set).
        let mut budget = self.new_budget_tracker();
        let mut global_turn_tokens: u64 = 0;
        // Turn-start output baseline (claude-code `xtr` via `UAc(e)`): snapshot
        // the cumulative pool as this turn begins, so a workflow launched this
        // turn reads `budget.spent()` = output spent THIS turn.
        self.turn_start_output_baseline.store(
            self.output_token_pool
                .load(std::sync::atomic::Ordering::Relaxed),
            std::sync::atomic::Ordering::Relaxed,
        );
        let mut turn_count: u32 = 0;
        loop {
            if cancel.is_cancelled() {
                // claude-code `query.ts:1046-1050`: inject the non-tool-use
                // interrupt message on a loop-top pre-cancel (ESC fired before
                // we even called the model this iteration). NOW-ABORT
                // disambiguation: skip the message when the cancel was a
                // `Now`-command (the urgent command runs next); default behavior
                // (no reason flag wired) is byte-identical to before.
                if self.cancel_reason_now()
                    != crate::prompt::mid_turn_input::CancelReason::QueueNowCommand
                {
                    self.inject_meta_user_message(INTERRUPT_MESSAGE).await;
                }
                return Ok(TurnOutcome::Cancelled);
            }
            if self.config.max_turns != 0 && turn_count >= self.config.max_turns {
                return Ok(TurnOutcome::MaxTurns);
            }
            if self.over_budget().await {
                return Err(OrchestratorError::MaxBudgetReached {
                    budget_nano_usd: self.config.max_budget_nano_usd.unwrap_or(0),
                });
            }
            turn_count = turn_count.saturating_add(1);

            // Race the recovery-aware API turn-step against the cancellation
            // token. The `_tracked` variant returns this step's output-token
            // count for the A3 budget accumulation, mirroring `run_turn`.
            let (step, output_tokens) = tokio::select! {
                r = execute_one_turn_with_recovery_tracked(
                    self,
                    system_prompt.as_deref(),
                    Some(&mut recovery),
                ) => r?,
                () = cancel.cancelled() => {
                    // claude-code `query.ts:1046-1050`: inject the non-tool-use
                    // interrupt message when the cancel fires mid-API-call
                    // (model was in-flight, no tool_use blocks produced yet).
                    // NOW-ABORT disambiguation: skip the message for a
                    // `Now`-command abort (default behavior unchanged).
                    if self.cancel_reason_now()
                        != crate::prompt::mid_turn_input::CancelReason::QueueNowCommand
                    {
                        self.inject_meta_user_message(INTERRUPT_MESSAGE).await;
                    }
                    return Ok(TurnOutcome::Cancelled);
                }
            };
            // A3: accumulate the running per-turn output tokens (TS
            // `getTurnOutputTokens()`). No-op for accounting when budget is off.
            global_turn_tokens = global_turn_tokens.saturating_add(output_tokens);
            match step {
                TurnStepOutcome::Continue => continue,
                TurnStepOutcome::Ended {
                    stop_reason,
                    final_message_id: id,
                } => {
                    // hooks B4: Stop hooks (cancelable twin). `TurnOutcome` does
                    // not distinguish StopHookPrevented from EndTurn, so both the
                    // Terminate and FallThrough dispositions end the REPL turn as
                    // EndTurn; only `LoopAgain` (a Stop hook asking to keep
                    // working) loops. `handle_stop_at_end` already emits the
                    // end-turn on Terminate, so we don't re-emit there.
                    match self
                        .handle_stop_at_end(
                            &stop_reason,
                            &mut stop_hook_active,
                            &mut stop_hook_blocking_count,
                            turn_count,
                            id,
                        )
                        .await
                    {
                        StopHookFlow::Terminate(_) => return Ok(TurnOutcome::EndTurn),
                        // Binary blocking-branch max-turns end — mirror this fn's
                        // own top-of-loop guard, which returns `TurnOutcome::MaxTurns`.
                        StopHookFlow::TerminateMaxTurns => return Ok(TurnOutcome::MaxTurns),
                        StopHookFlow::LoopAgain => {
                            // RECOV.4: a Stop hook forced the loop to continue —
                            // reset the max_output_tokens recovery bookkeeping so
                            // the continued turn starts a fresh escalation episode
                            // (TS `query.ts:1291`), matching `run_turn`.
                            recovery.reset_max_output_tokens_recovery();
                            continue;
                        }
                        StopHookFlow::FallThrough => {}
                    }
                    // A3: at a natural end-of-turn, consult the token budget. If
                    // it says continue, inject the meta nudge, reset the A1
                    // recovery count, and loop again instead of ending. When the
                    // budget is off this is a no-op (parity default).
                    if self
                        .maybe_continue_for_budget(
                            budget.as_mut(),
                            &mut recovery,
                            global_turn_tokens,
                        )
                        .await
                    {
                        continue;
                    }
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn(&stop_reason, &cost).await;
                    return Ok(TurnOutcome::EndTurn);
                }
            }
        }
    }

    /// Read the `should_exit` flag set by `/exit` (M5-10 / M5-13).
    ///
    /// The REPL checks this after each dispatch and breaks the loop if
    /// `true`. The flag is set via
    /// [`traits::OrchestratorHandle::request_exit`]; once set it
    /// never resets (idempotent `/exit`).
    pub fn current_should_exit(&self) -> bool {
        self.should_exit.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Streaming twin of [`Self::run_turn_with_cancel`] (M6-03).
    ///
    /// DEFERRED-3: the `cancel` token is a GRANULAR user-interrupt (ESC / new
    /// message), threaded INTO [`Self::try_run_turn_streaming`] rather than raced
    /// against it. When it fires mid-tools the `StreamingToolExecutor` rejects
    /// in-flight/queued Cancel-behavior tools with the bare REJECT_MESSAGE,
    /// PERSISTS those `tool_result`s, and the turn ends gracefully:
    /// - natural completion, token never fired → `TurnOutcome::EndTurn`.
    /// - token fired (the turn finished gracefully with the interrupted results
    ///   recorded in history) → `TurnOutcome::Cancelled`.
    /// - a pre-cancelled token → `TurnOutcome::Cancelled` immediately (no API call).
    /// - `OrchestratorError::MaxTurnsReached` → `TurnOutcome::MaxTurns`.
    /// - any other API/streaming error → propagated as `Err`.
    ///
    /// This is the entry point the M6 TUI calls. M5-13 stdio REPL keeps
    /// using `run_turn_with_cancel` (batched) until M6 makes streaming
    /// the default.
    pub async fn run_turn_streaming_with_cancel(
        &self,
        prompt: &str,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, OrchestratorError> {
        self.run_turn_streaming_with_cancel_images(prompt, &[], cancel)
            .await
    }

    /// As [`Self::run_turn_streaming_with_cancel`], but carrying pasted image
    /// file paths that are loaded + base64-encoded into `ContentBlock::Image`
    /// blocks on the outgoing user message (TUI paste→image). A failed image
    /// read aborts the turn with `Err` before any API call.
    pub async fn run_turn_streaming_with_cancel_images(
        &self,
        prompt: &str,
        image_paths: &[std::path::PathBuf],
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, OrchestratorError> {
        // Decode the pasted PATHS into canonical sources first, then hand off to
        // the already-decoded entry below — so the path-based and bridge (inline
        // base64) flows share ONE cancel race + ONE turn core. A failed image read
        // aborts the turn with `Err` before any API call (unchanged).
        let images = Self::load_images(image_paths)?;
        self.run_turn_streaming_with_cancel_image_sources(prompt, images, cancel)
            .await
    }

    /// As [`Self::run_turn_streaming_with_cancel_images`], but taking
    /// ALREADY-DECODED [`protocol::ImageSource`]s instead of file paths.
    ///
    /// This is the entry the desktop bridge adapter drives: pasted/attached images
    /// arrive over the wire as inline base64 (`ImageRefDto`) and are converted
    /// straight to [`protocol::ImageSource::Base64`] with NO temp-file round-trip.
    /// The path-based entry above decodes its paths via [`Self::load_images`] then
    /// delegates here, so both paths share this cancel race and the single
    /// [`Self::try_run_turn_streaming`] core — which appends the images to the
    /// outgoing user message via [`ConversationMessage::user_with_images`]. With an
    /// empty `images` vector this is byte-identical to the text-only streaming path.
    pub async fn run_turn_streaming_with_cancel_image_sources(
        &self,
        prompt: &str,
        images: Vec<protocol::ImageSource>,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, OrchestratorError> {
        tracing::info!(
            event = orch_events::TURN_STREAMING_STARTED,
            prompt_len = prompt.len()
        );
        if cancel.is_cancelled() {
            self.abort_startup_responses_websocket_prewarm();
            if let Err(err) = self.api.close_responses_websocket_session().await {
                tracing::warn!(
                    error = %err,
                    "failed to close responses websocket session after pre-cancelled streaming turn"
                );
            }
            return Ok(TurnOutcome::Cancelled);
        }
        // DEFERRED-3: GRANULAR user-ESC interrupt — do NOT race-drop the turn.
        // Previously this `select!`'d `try_run_turn_streaming` against
        // `cancel.cancelled()` and on cancel DROPPED the whole turn future, so
        // in-flight tools vanished and no result reached the model. Now the token
        // is threaded INTO the turn core: the `StreamingToolExecutor` substitutes
        // the bare REJECT_MESSAGE for in-flight/queued Cancel-behavior tools,
        // PERSISTS those `tool_result`s (model-visible), and the streaming loop
        // FINISHES gracefully — mirroring claude-code's `StreamingToolExecutor`
        // user_interrupted path where the interrupted results become part of the
        // transcript. We then map the outcome to `Cancelled` for the TUI when the
        // token fired (so it still renders "interrupted"), else `EndTurn`. A turn
        // running only Block-behavior tools (or no tools) runs to its natural end
        // and is still reported `Cancelled` here — faithful: claude-code only
        // aborts Cancel-behavior tools; Block tools / the stream finish.
        let r = self
            .try_run_turn_streaming(prompt, images, Some(cancel.clone()))
            .await;
        match r {
            Ok(
                ConversationOutcome::EndTurn { turn_count, .. }
                | ConversationOutcome::StopHookPrevented { turn_count, .. },
            ) => {
                tracing::info!(event = orch_events::TURN_STREAMING_COMPLETED, turn_count);
                if cancel.is_cancelled() {
                    self.abort_startup_responses_websocket_prewarm();
                    if let Err(err) = self.api.close_responses_websocket_session().await {
                        tracing::warn!(
                            error = %err,
                            "failed to close responses websocket session after cancelled streaming turn"
                        );
                    }
                    Ok(TurnOutcome::Cancelled)
                } else {
                    Ok(TurnOutcome::EndTurn)
                }
            }
            Err(OrchestratorError::MaxTurnsReached { .. }) => Ok(TurnOutcome::MaxTurns),
            Err(e) => {
                // B6-T1: status-change emit parity — fire the emit-on-change
                // helpers for a terminal rate-limited error (the drive fn
                // already promoted the staged 429), BEFORE enrichment. Same
                // discriminant + B1 divergence as
                // `emit_terminal_rate_limit_if_changed`.
                if matches!(
                    e,
                    OrchestratorError::ApiCall(LlmError::RateLimited { .. })
                        | OrchestratorError::Streaming(LlmError::RateLimited { .. })
                ) {
                    self.emit_rate_limit_if_changed().await;
                    self.emit_raw_utilization_if_changed().await;
                }
                Err(self.enrich_api_error(e))
            }
        }
    }

    /// Load + base64-encode each pasted image path into an [`ImageSource`].
    fn load_images(
        image_paths: &[std::path::PathBuf],
    ) -> Result<Vec<protocol::ImageSource>, OrchestratorError> {
        image_paths
            .iter()
            .map(|p| crate::image_input::load_image_source(p))
            .collect()
    }

    /// Assemble the system prompt this orchestrator would send on the next
    /// turn, WITHOUT running a turn (no API call, no message mutation).
    ///
    /// Honors the same `system_prompt_override` bypass as [`Self::run_turn`]:
    /// returns the override verbatim when set, otherwise the freshly assembled
    /// prompt (cwd / git / file-tree / **memory** / tool-name context). This is
    /// a read-only introspection seam — it lets a host/composition-root test
    /// prove that its injected [`crate::prompt::MemoryHierarchyProvider`]
    /// (e.g. a controlled `StaticMemoryProvider`, or the production
    /// `real_provider()`) actually reaches the system prompt, without a live
    /// model round-trip.
    pub async fn assemble_system_prompt_preview(&self) -> String {
        match &self.config.system_prompt_override {
            Some(p) => p.clone(),
            None => self.build_system_prompt().await,
        }
    }

    /// Read-only introspection seam for the leading additional-context
    /// `<system-reminder>` meta (R-P1): `claudeMd` / `userEmail` / `currentDate`
    /// live HERE now, not in the system prompt. Returns the meta's text, or
    /// `None` when nothing is sourceable. Companion to
    /// [`Self::assemble_system_prompt_preview`] — lets a host/composition-root
    /// test prove an injected memory provider reaches the additional-context
    /// message without a live model round-trip.
    pub async fn additional_context_preview(&self) -> Option<String> {
        self.additional_context_message()
            .await
            .and_then(|m| match m {
                ConversationMessage::User { content, .. } => {
                    content.into_iter().find_map(|b| match b {
                        protocol::ContentBlock::Text { text } => Some(text),
                        _ => None,
                    })
                }
                _ => None,
            })
    }

    /// Build the per-turn system prompt by gathering cwd / git / file
    /// tree / memory / tool-name context and calling
    /// [`crate::prompt::assemble_system_prompt`]. Bypassed when
    /// `OrchestratorConfig::system_prompt_override` is `Some(_)`.
    /// Build the per-turn [`crate::prompt::SystemPromptContext`] (cwd / env /
    /// git / tools / memory / …). Shared by [`Self::build_system_prompt`] and
    /// [`Self::additional_context_message`] — the latter re-emits the env block
    /// in the first user message when `--exclude-dynamic-system-prompt-sections`
    /// is set, so the construction (and its env-field probes) lives in ONE place.
    async fn build_prompt_context(&self) -> crate::prompt::SystemPromptContext {
        use crate::prompt::{file_tree, git_status, SystemPromptContext};

        let cwd = self.cwd.clone();
        // The `<env>` model-identity line ("You are powered by the model named
        // …") must reflect the CURRENT model, not the launch model. `/model`
        // switches update `session.model` (see `OrchestratorHandle::switch_model`
        // → `SessionState::model`), while `config.model` stays frozen at
        // startup. Reading `config.model` here froze the injected identity, so a
        // switched-to model (e.g. Fable 5) still saw "You are Opus 4.8" in its
        // system prompt and reported the stale identity. The outgoing REQUEST
        // model already re-snapshots `session.model` each turn; this aligns the
        // prompt identity with it. Locked briefly and released — every
        // `build_system_prompt` caller builds the prompt BEFORE taking the
        // session lock, so there is no reentrancy.
        let model = self.session.lock().await.model.clone();
        let memory_files = self.memory.load(&cwd).await;

        let git = git_status::probe(&cwd);
        let tree = file_tree::probe(&cwd, file_tree::DEFAULT_DEPTH_LIMIT);

        // Tool name extraction: ToolRegistry's `all_names()` is the
        // unfiltered set (builtin + plugin + MCP). M5-03 uses the
        // unfiltered list because the registry's enable-filter requires
        // a `ToolStaticContext` that's only meaningful at dispatch time.
        // tools_block::format sorts alphabetically inside.
        let tool_names: Vec<String> = self.tools.all_names();

        // Port of claude-code `tIo` (binary offset ~205825700):
        //   let e = process.env.SHELL || "unknown",
        //       t = e.includes("zsh") ? "zsh" : e.includes("bash") ? "bash" : e;
        // i.e. the RAW $SHELL collapsed to "zsh"/"bash" by SUBSTRING (not the
        // path basename), else the raw full $SHELL value verbatim; "unknown"
        // when $SHELL is unset/empty. (`env_block` prepends the "Shell: "
        // literal that `tIo` carries in its return value.) RESIDUAL #53b: the
        // win32 PowerShell-primary branches (`Su()`/`tN()` availability probes)
        // are not ported — LingXi has no PowerShell/Bash-tool probe at this
        // site, so Windows falls through to `t` (tIo's `Shell: ${t}` else-arm).
        let shell = {
            let raw = std::env::var("SHELL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".into());
            if raw.contains("zsh") {
                "zsh".into()
            } else if raw.contains("bash") {
                "bash".into()
            } else {
                raw
            }
        };

        // DIV-1: worktree detection — `hf()!==null` in claude-code. Detect a
        // worktree by checking for the `gitdir` file that git creates in worktree
        // checkouts (a file rather than a directory at .git). Computed before `cwd`
        // is moved into the context struct below.
        let in_worktree = cwd.join(".git").is_file();

        SystemPromptContext {
            cwd,
            // `Platform: ${je.platform}` — claude-code emits the node
            // `process.platform` value (`darwin`/`linux`/`win32`), NOT Rust's
            // `std::env::consts::OS` (`macos`/`linux`/`windows`). Map the two
            // divergent names so the env line is byte-exact.
            platform: node_platform_name(std::env::consts::OS).to_string(),
            // SYSPROMPT.1: port TS getMarketingNameForModel / getKnowledgeCutoff
            // (`utils/model/model.ts:570`, `constants/prompts.ts:712`) so the
            // model line + cutoff sentence match claude-code instead of being
            // stubbed to None. Sourced from the LIVE `session.model` (above) so
            // `/model` switches take effect for the identity block.
            //
            // NON-Claude fallback: `marketing_name_for_model` only names Claude
            // ids, so a switched-to non-Claude model (deepseek/gemini/…) got the
            // weak id-only "powered by the model {id}." form. Fall back to the
            // catalog display name so EVERY turn's identity line uses the strong
            // "powered by the model named {name}." form with the CURRENT model.
            // Gated on non-Claude so Claude ids keep byte-parity (an unknown
            // Claude id stays id-only exactly like claude-code).
            model_marketing_name: crate::prompt::env_meta::marketing_name_for_model(&model)
                .map(String::from)
                .or_else(|| {
                    (!model.to_ascii_lowercase().contains("claude"))
                        .then(|| crate::provider_adapter::display_name_for_model(&model))
                        .flatten()
                }),
            knowledge_cutoff: crate::prompt::env_meta::knowledge_cutoff_for_model(&model)
                .map(String::from),
            model,
            shell,
            // SYSPROMPT.1: `uname -sr` (TS getUnameSR) e.g. "Darwin 25.3.0",
            // falling back to "<os> <arch>" on Windows / spawn failure.
            os_version: crate::prompt::env_meta::os_version_string(),
            git_status: git,
            in_worktree,
            file_tree: tree,
            memory_files,
            tool_names,
            // `nz()` non-empty: at least one model-invocable prompt skill
            // exists. Sourced from the attached skill-listing provider (same
            // source as the per-turn skill reminder); `None` provider ⇒ false.
            skills_available: match &self.skill_listing {
                Some(provider) => !provider.skill_entries().await.is_empty(),
                None => false,
            },
            // `# Memory` section gate (claude-code `tengu_moth_copse`, default
            // OFF): the memory feature is active iff a memory prefetch is wired
            // (`memory_prefetch.is_some()`), and the section points the model at
            // exactly the user memdir the prefetch scans. `None` ⇒ section
            // omitted (byte-identical to the pre-memory prompt).
            memory_dir: self
                .memory_prefetch
                .as_ref()
                .and_then(|p| p.user_memdir())
                .map(std::path::Path::to_path_buf),
            // `--exclude-dynamic-system-prompt-sections`: when set, `assemble`
            // OMITS the env block from the system prompt (it is re-emitted in the
            // first-user-message context reminder via `env_reminder_section`).
            exclude_dynamic_sections: self.config.exclude_dynamic_system_prompt_sections,
        }
    }

    /// Assemble the full system-prompt STRING from [`Self::build_prompt_context`].
    /// Bypassed when `OrchestratorConfig::system_prompt_override` is `Some(_)`.
    async fn build_system_prompt(&self) -> String {
        use crate::prompt::{assemble_system_prompt_with_style, git_status, ActiveOutputStyle};
        let ctx = self.build_prompt_context().await;
        // OUTSTYLE.2/.3: when a non-default output style is active — a builtin
        // OR a custom disk style discovered under `output_style_dirs` — inject
        // its `# Output Style: <name>` section (TS getOutputStyleSection). A
        // `None`/`"default"`/unknown style resolves to `None`, leaving the prompt
        // byte-identical to the styleless path (empty `output_style_dirs` ⇒
        // builtin-only, as before).
        let resolved = outputstyles::resolve_output_style(
            self.config.output_style.as_deref(),
            &self.config.output_style_dirs,
        );
        let style = resolved.as_ref().map(|r| ActiveOutputStyle {
            name: r.name.as_str(),
            prompt: r.prompt.as_str(),
            keep_coding_instructions: r.keep_coding_instructions,
        });
        let mut prompt = assemble_system_prompt_with_style(&ctx, style);

        // R-P1c: append the `gitStatus` system-prompt attachment as a trailing
        // dynamic block. claude-code threads the `systemContext` (whose only
        // relevant key is `gitStatus`) into the system prompt via
        // `WZa(systemPromptArray, systemContext)` → one extra `string[]` member
        // `gitStatus: <value>` joined with `\n\n`, fed to `getSystemPrompt`.
        // LingXi concatenates the system prompt into one string, so the block is
        // appended here with the same blank-line boundary. `None` when cwd is not
        // a git repo (claude-code omits the key, so nothing is appended).
        //
        // `--exclude-dynamic-system-prompt-sections`: gitStatus is a per-machine,
        // commit-volatile section, so it is also OMITTED from the system prompt
        // when the flag is set (claude empties systemContext). Leaving it in would
        // churn the prompt-cache key on every commit — exactly what the flag
        // prevents. (claude drops gitStatus entirely; it is NOT re-emitted in the
        // user message.)
        if !self.config.exclude_dynamic_system_prompt_sections {
            if let Some(block) = git_status::render_git_status_block(&self.cwd) {
                prompt.push_str("\n\n");
                prompt.push_str(&block);
            }
        }
        prompt
    }

    /// R-P1c/R-P1d: the leading `additionalContext` (`# claudeMd` / `# userEmail`
    /// / `# currentDate`) meta user message, or `None` when nothing is sourceable.
    ///
    /// 1:1 with claude-code `A6n(messages, userContext)` (binary offset
    /// ~205838418): when the `userContext` object is non-empty it PREPENDS one
    /// `isMeta` user message whose body is
    /// ```text
    /// <system-reminder>
    /// As you answer the user's questions, you can use the following context:
    /// # {key}
    /// {value}
    /// …                          (one `# {key}\n{value}` per entry, joined by `\n`)
    ///
    ///       IMPORTANT: this context may or may not be relevant to your tasks. You should not respond to this context unless it is highly relevant to your task.
    /// </system-reminder>
    /// ```
    /// (the IMPORTANT line is indented by exactly six spaces).
    ///
    /// The `userContext` keys, in claude-code insertion order (`pS`,
    /// binary offset ~197202100): `claudeMd` (the assembled LINGXI.md memory
    /// block — [`memory_block::format`]), `userEmail`
    /// (`The user's email address is {email}.`, only when configured), and
    /// `currentDate` (`Today's date is {YYYY-MM-DD}.`, always present). The
    /// `attachedProject` key (CLAUDE_PROJECT_TOOL) is not modelled.
    ///
    /// NOTE: `gitStatus` is NOT here — it belongs to the SEPARATE `systemContext`
    /// that claude-code folds into the SYSTEM PROMPT (see `build_system_prompt`),
    /// not this `userContext` message.
    ///
    /// Like the per-turn reminders, this is recomputed and prepended to the
    /// OUTGOING snapshot each turn (claude-code calls `A6n` on every `callModel`);
    /// it is never persisted to `session.history` / JSONL.
    pub(crate) async fn additional_context_message(&self) -> Option<ConversationMessage> {
        // `claudeMd` value = the assembled memory block (preamble + `Contents
        // of …:` blocks). Empty when no LINGXI.md files are loaded.
        let memory_files = self.memory.load(&self.cwd).await;
        let lingxi_md = crate::prompt::memory_block::format(&memory_files);

        // Build the entries in claude-code insertion order; each is `# key\nvalue`.
        let mut entries: Vec<String> = Vec::with_capacity(4);
        // `--exclude-dynamic-system-prompt-sections`: the per-machine env block
        // (cwd / env / git / OS / shell) is OMITTED from the static system prompt
        // (see `assemble_system_prompt_with_style`) and re-emitted HERE in the
        // first-user-message context reminder, so the system prompt stays
        // identical across machines (prompt-cache reuse) while the model still
        // sees the env. Built from the SAME `build_prompt_context` the system
        // prompt uses. (`false` ⟶ skipped, byte-identical to before this flag.)
        //
        // claude-code: "Only applies with the default system prompt (ignored with
        // --system-prompt)." A custom system prompt already bypasses the static
        // env block, so re-emitting it here under `--system-prompt` would leak
        // per-machine env into the first user message that the oracle never sends.
        // Gate the env-block re-emission on the absence of a system-prompt override
        // so the exclude-dynamic flag is a complete no-op when a custom prompt is
        // active. (The claudeMd / userEmail / currentDate entries below stay
        // unconditional — they are unrelated to this flag.)
        if self.config.exclude_dynamic_system_prompt_sections
            && self.config.system_prompt_override.is_none()
        {
            let ctx = self.build_prompt_context().await;
            let env = crate::prompt::env_block::format(&ctx);
            // `env_block::format` already begins with its own `# Environment\n`
            // heading, so key the entry as `Environment` and strip that leading
            // heading — the userContext renderer prepends `# {key}\n`, and a raw
            // `# env\n{env}` would DOUBLE the heading (`# env\n# Environment\n…`).
            let body = env.strip_prefix("# Environment\n").unwrap_or(&env).trim();
            if !body.is_empty() {
                entries.push(format!("# Environment\n{body}"));
            }
        }
        if !lingxi_md.is_empty() {
            entries.push(format!("# claudeMd\n{lingxi_md}"));
        }
        if let Some(email) = self
            .config
            .user_email
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            entries.push(format!("# userEmail\nThe user's email address is {email}."));
        }
        // `currentDate` is unconditional in claude-code (`currentDate: WNi(bRe())`).
        entries.push(format!(
            "# currentDate\nToday's date is {}.",
            crate::prompt::env_meta::current_date_string()
        ));

        // `A6n` returns the messages unchanged when the context object is empty.
        // `currentDate` is always present, so `entries` is never empty — but keep
        // the guard for faithfulness to the `Object.entries(t).length===0` check.
        if entries.is_empty() {
            return None;
        }

        let body = entries.join("\n");
        // NOTE: the IMPORTANT line is indented by EXACTLY six spaces (claude-code
        // `A6n`). Those spaces must NOT sit at the start of a continued (`\`)
        // string line — Rust's line-continuation strips leading whitespace — so
        // the `\n\n      IMPORTANT` segment is written without a preceding `\`.
        let important = "      IMPORTANT: this context may or may not be relevant to your tasks. \
You should not respond to this context unless it is highly relevant to your task.";
        let content = format!(
            "<system-reminder>\n\
As you answer the user's questions, you can use the following context:\n\
{body}\n\n{important}\n</system-reminder>\n"
        );
        // claude-code `A6n` sets `isMeta:!0` on this message. It is sent to the
        // wire (the wire conversion does not drop meta user messages) but never
        // persisted to JSONL (it is only prepended to the OUTGOING snapshot).
        Some(ConversationMessage::user_meta(MessageId::new(), content))
    }

    /// OUTSTYLE.3: the byte-exact per-turn output-style reminder, or `None` when
    /// the default style is active.
    ///
    /// 1:1 with claude-code's `output_style` attachment. On EVERY turn where
    /// `settings.outputStyle != 'default'`, claude-code injects a meta user
    /// message into the model's input: `getOutputStyleAttachment`
    /// (`attachments.ts:1597-1612`) → `normalizeAttachmentForAPI`'s
    /// `'output_style'` case (`messages.ts:3797-3811`), which wraps
    /// `` `${outputStyle.name} output style is active. Remember to follow the
    /// specific guidelines for this style.` `` via `wrapInSystemReminder`
    /// (`messages.ts:3097-3099`, literally `` `<system-reminder>\n${content}\n</system-reminder>` ``).
    /// `outputStyle.name` is the builtin's `OUTPUT_STYLE_CONFIG[style].name`
    /// (`"Explanatory"` / `"Learning"`), here the resolved
    /// [`outputstyles::BuiltinOutputStyle::name`].
    ///
    /// Returns `None` for the `None`/`"default"`/unknown style (the same gate as
    /// the system-prompt section above), so the styleless path stays
    /// byte-identical and the locked turn-loop + streaming fixtures stay green.
    ///
    /// The protocol has no `isMeta` flag, so — exactly like the A1 "resume
    /// directly" nudge ([`crate::turn_loop::MAX_OUTPUT_TOKENS_RECOVERY_NUDGE`]) —
    /// the reminder is a plain user-text [`ConversationMessage`] carrying the
    /// byte-exact string. The fresh [`MessageId`] is irrelevant: callers append
    /// this ONLY to the per-turn outgoing message snapshot, never to
    /// `session.history` nor JSONL, so it is TRANSIENT and never accumulates
    /// (TS recomputes the attachment each turn — see `query.ts` mid-turn
    /// `getAttachmentMessages`). Position mirrors TS: the caller appends it as a
    /// trailing meta user message after the user prompt / tool-results
    /// (`processTextPrompt` returns `[userMessage, ...attachmentMessages]`;
    /// `query.ts:1580-1590` pushes the attachment after `toolResults`).
    pub(crate) fn output_style_reminder_message(&self) -> Option<ConversationMessage> {
        let resolved = outputstyles::resolve_output_style(
            self.config.output_style.as_deref(),
            &self.config.output_style_dirs,
        )?;
        let content = format!(
            "<system-reminder>\n{} output style is active. \
             Remember to follow the specific guidelines for this style.\n</system-reminder>",
            resolved.name
        );
        Some(ConversationMessage::user(MessageId::new(), content))
    }

    /// SKILLLIST.1: the per-turn, transient `skill_listing` reminder, or `None`
    /// when no provider is wired, the `Skill` tool is absent this turn, or there
    /// are no model-invocable skills.
    ///
    /// 1:1 with claude-code's `skill_listing` attachment: `getSkillToolCommands`
    /// (`commands.ts:565`) selects the eligible skills, `formatCommandsWithinBudget`
    /// (`SkillTool/prompt.ts`) renders them within a ~1%-of-context char budget,
    /// and `normalizeAttachmentForAPI`'s `'skill_listing'` case
    /// (`messages.ts:3728-3738`) wraps the body in a `<system-reminder>` meta
    /// user message: `"The following skills are available for use with the Skill
    /// tool:\n\n{listing}"`. The Skill-tool gate mirrors `attachments.ts:2668`.
    ///
    /// Like the OUTSTYLE.3 reminder, the message is appended ONLY to the per-turn
    /// outgoing snapshot (never to `session.history` / JSONL), so it is recomputed
    /// each turn and never accumulates. `None` keeps the styleless/skilless path
    /// byte-identical and the locked fixtures green.
    ///
    /// DELTA (SKILLLIST.1): turn-0 emits the FULL listing; each later turn emits
    /// ONLY skills that have NOT appeared in a prior turn's reminder, tracked via
    /// [`Self::sent_skill_names`]. When no new skill appears, returns `None` (no
    /// reminder that turn). 1:1 with TS `sentSkillNames` (attachments.ts:2607,
    /// 2699): the budgeter still runs over the delta subset, so the rendered
    /// bytes match what TS would send for that turn's new-skill set.
    pub(crate) async fn skill_listing_reminder_message(&self) -> Option<ConversationMessage> {
        let provider = self.skill_listing.as_ref()?;
        // Gate on the Skill tool being available this turn (attachments.ts:2668).
        if self.tools.find_by_name("Skill").is_none() {
            return None;
        }
        let entries = provider.skill_entries().await;

        // DELTA: keep only skills not yet sent this session, then record them as
        // sent. Turn 0 keeps everything (the set is empty); subsequent turns keep
        // only newly-appeared names. An empty delta ⇒ no reminder this turn.
        let new_entries: Vec<crate::prompt::skill_listing::SkillListingEntry> = {
            let mut sent = self.sent_skill_names.lock().await;
            let delta: Vec<_> = entries
                .into_iter()
                .filter(|e| !sent.contains(&e.name))
                .collect();
            for e in &delta {
                sent.insert(e.name.clone());
            }
            delta
        };
        if new_entries.is_empty() {
            return None;
        }

        // ~1% of the active model's context window (TS getCharBudget). Resolved
        // with no betas — the small 200k↔1M budget delta only matters past ~30
        // skills, where the budgeter degrades gracefully. Read the LIVE model
        // (mutated by /model + resume), not the frozen boot `config.model`, so a
        // switch across a 200k↔1M window boundary re-sizes the budget correctly
        // (mirrors `build_prompt_context`).
        let model = self.session.lock().await.model.clone();
        let window =
            compaction::context_window::context_window_for_model(&model, &[]) as usize;
        let content = crate::prompt::skill_listing::render_reminder(&new_entries, Some(window))?;
        Some(ConversationMessage::user(MessageId::new(), content))
    }

    /// The per-turn, transient `async_hook_response` reminder, or `None` when no
    /// source is wired or no background (`async`) hook has completed since the
    /// last turn.
    ///
    /// 1:1 with claude-code's `async_hook_response` attachment
    /// (`getAsyncHookResponseAttachments`, attachments.ts:3464 →
    /// `normalizeAttachmentForAPI`, messages.ts:4026): drains the completed
    /// background-hook responses (CONSUME-ONCE — TS `removeDeliveredAsyncHooks`)
    /// and wraps their `system_message` text (which already folds in any
    /// `additionalContext`) in one `<system-reminder>` meta user message. Like
    /// the skill-/agent-listing reminders it is appended ONLY to the per-turn
    /// OUTGOING snapshot, never `session.history` / JSONL, so it never
    /// accumulates. No delta set is needed — draining the source IS the dedup.
    pub(crate) async fn async_hook_response_reminder_message(&self) -> Option<ConversationMessage> {
        let provider = self.async_hook_responses.as_ref()?;
        let responses = provider.take_pending_responses().await;
        let content = crate::prompt::async_hook_response::render_reminder(&responses)?;
        Some(ConversationMessage::user(MessageId::new(), content))
    }

    /// T35: the per-turn, transient `task-notification` reminder, or `None` when
    /// no source is wired or no background task finished since the last turn.
    ///
    /// Mirrors [`Self::async_hook_response_reminder_message`]: drains the
    /// registry's terminal-not-notified tasks (CONSUME-ONCE — the registry marks
    /// each `notified` + evicts on drain) and renders their `<task-notification>`
    /// blocks (claude-code's per-task-type `enqueue*Notification` formats) inside
    /// one `<system-reminder>` meta user message. Appended ONLY to the per-turn
    /// OUTGOING snapshot, never `session.history` / JSONL, so it never
    /// accumulates. No delta set is needed — draining the registry IS the dedup.
    pub(crate) async fn task_notification_reminder_message(&self) -> Option<ConversationMessage> {
        let provider = self.task_notifications.as_ref()?;
        let notifications = provider.take_pending_task_notifications().await;
        let content = crate::prompt::task_notification::render_reminder(&notifications)?;
        Some(ConversationMessage::user(MessageId::new(), content))
    }

    /// Finding #73: the per-turn `todo_reminder` (V1) / `task_reminder` (V2)
    /// meta user message, or `None` when not eligible this turn.
    ///
    /// 1:1 with the binary's producer `()=>TE()?B4p(o,t):M4p(o,t)` (`ytl`,
    /// offset ~203087213). [`tool_task::reminder::select_mode`] picks V1 vs V2
    /// (`TE()`). For the selected variant this mirrors the `M4p`/`B4p` gates:
    /// 1. killswitch `wgo()!=="off"`;
    /// 2. the `Brief` tool (`rjn`/`SendUserMessage`) is ABSENT (present ⇒ skip);
    /// 3. the relevant tool is PRESENT this turn — `TodoWrite` (V1) /
    ///    `TaskUpdate` (V2);
    /// 4. the history is non-empty (`!e||e.length===0 ⇒ []`);
    /// 5. BOTH counters reach their thresholds (`turns_since_last_todo_write >=
    ///    TURNS_SINCE_WRITE && turns_since_last_reminder >= TURNS_BETWEEN_REMINDERS`).
    ///
    /// On fire it renders the RAW body (no `<system-reminder>` wrapper —
    /// `Ln({content:r,isMeta:!0})`) and RESETS `turns_since_last_reminder` to
    /// `0`. V1 reads `session.todos`; V2 reads the wired
    /// [`crate::prompt::todo_reminder::TodoReminderTaskProvider`] (no provider ⇒
    /// base text only, an empty store). Appended ONLY to the per-turn OUTGOING
    /// snapshot (never `session.history` / JSONL) so it never accumulates.
    ///
    /// COUNTER NOTE: the binary recomputes the counters by scanning the message
    /// log for the last `TodoWrite`/`Task` tool_use and the last reminder
    /// ATTACHMENT. This engine never persists the reminder attachment, so the
    /// counters are tracked as explicit `SessionState` fields, incremented once
    /// per assistant turn (`bump_reminder_turn_counters`) and reset on the
    /// relevant tool call (`note_todo_reminder_tool_call`).
    pub(crate) async fn todo_reminder_message(&self) -> Option<ConversationMessage> {
        // (1) killswitch.
        if tool_task::reminder::is_killswitched() {
            return None;
        }
        // (2) Brief (`SendUserMessage`/`Brief`) present ⇒ skip (both variants).
        if self.tools.find_by_name("SendUserMessage").is_some() {
            return None;
        }

        let mode = tool_task::reminder::select_mode();

        // (3) tool-presence gate + (4) non-empty history + (5) counters, all
        // read under one session lock so the snapshot is consistent. We reset
        // `turns_since_last_reminder` here (inside the lock) iff we fire.
        let mut s = self.session.lock().await;

        // (4) empty history ⇒ no reminder.
        if s.history.is_empty() {
            return None;
        }
        // (5) both thresholds.
        if s.turns_since_last_todo_write < tool_task::reminder::TURNS_SINCE_WRITE
            || s.turns_since_last_reminder < tool_task::reminder::TURNS_BETWEEN_REMINDERS
        {
            return None;
        }

        match mode {
            tool_task::reminder::ReminderMode::V1Todo => {
                // (3) TodoWrite must be present this turn.
                if self.tools.find_by_name("TodoWrite").is_none() {
                    return None;
                }
                let items: Vec<(engine::TodoState, String)> = s
                    .todos
                    .iter()
                    .map(|t| (t.status, t.content.clone()))
                    .collect();
                s.turns_since_last_reminder = 0;
                drop(s);
                let content = tool_task::reminder::render_v1(&items);
                Some(ConversationMessage::user(MessageId::new(), content))
            }
            tool_task::reminder::ReminderMode::V2Task => {
                // (3) TaskUpdate must be present this turn.
                if self.tools.find_by_name("TaskUpdate").is_none() {
                    return None;
                }
                s.turns_since_last_reminder = 0;
                drop(s);
                // Read the V2 task store outside the session lock.
                let items: Vec<(String, engine::TodoState, String)> =
                    match &self.todo_reminder_tasks {
                        Some(provider) => provider
                            .task_items()
                            .await
                            .into_iter()
                            .map(|t| (t.id, t.status, t.subject))
                            .collect(),
                        None => Vec::new(),
                    };
                let content = tool_task::reminder::render_v2(&items);
                Some(ConversationMessage::user(MessageId::new(), content))
            }
        }
    }

    /// Finding #73: increment BOTH reminder counters by one assistant turn.
    /// Called once per assistant turn (after the API response is processed) on
    /// both the batched and streaming paths, mirroring the binary's per-
    /// assistant-message counting in `L4p`/`N4p`.
    pub(crate) async fn bump_reminder_turn_counters(&self) {
        let mut s = self.session.lock().await;
        s.turns_since_last_todo_write = s.turns_since_last_todo_write.saturating_add(1);
        s.turns_since_last_reminder = s.turns_since_last_reminder.saturating_add(1);
    }

    /// Finding #73: reset `turns_since_last_todo_write` to `0` when this turn's
    /// assistant response invoked the variant's "recent use" tool — `TodoWrite`
    /// (V1) or `TaskCreate`/`TaskUpdate` (V2). Mirrors `L4p`/`N4p` finding the
    /// last such tool_use in the message log (which zeroes their `r` counter).
    /// `tool_names` is the set of tool names invoked in the assistant turn.
    pub(crate) async fn note_todo_reminder_tool_call(&self, tool_names: &[String]) {
        let resets = match tool_task::reminder::select_mode() {
            tool_task::reminder::ReminderMode::V1Todo => {
                tool_names.iter().any(|n| n == "TodoWrite")
            }
            tool_task::reminder::ReminderMode::V2Task => tool_names
                .iter()
                .any(|n| n == "TaskCreate" || n == "TaskUpdate"),
        };
        if resets {
            let mut s = self.session.lock().await;
            s.turns_since_last_todo_write = 0;
        }
    }

    /// The per-turn, transient `agent_listing_delta` reminder, or `None` when
    /// the gate is OFF (the default — keeps the inline-catalog build
    /// byte-identical), the `Agent` tool is absent this turn, or no NEW agent
    /// type has appeared since the last reminder. A wired DISK catalog is NOT
    /// required — built-ins are always announced (binary `aLe` uses
    /// `activeAgents`, which includes built-ins).
    ///
    /// 1:1 with claude-code's `agent_listing_delta` attachment
    /// (`getAgentListingDeltaAttachment`, attachments.ts:1490-1554 →
    /// `normalizeAttachmentForAPI`'s `'agent_listing_delta'` case,
    /// messages.ts:4194-4215):
    /// - GATE: `shouldInjectAgentListInMessages()` (env
    ///   `LINGXI_AGENT_LIST_IN_MESSAGES`, default OFF — see
    ///   [`agent::should_inject_agent_list_in_messages`]). When ON, `AgentTool`'s
    ///   description drops the inline catalog for a static pointer line and the
    ///   catalog is conveyed here instead, so the tool-schema prompt cache stops
    ///   busting on every MCP/plugin/permission-driven catalog change.
    /// - TOOL GATE: skip when the `Agent` tool is not in the registry this turn
    ///   (attachments.ts:1497-1501) — the listing would be unactionable.
    /// - ENTRIES: the merged built-ins + catalog listing via
    ///   [`agent::agent_listing_entries`] (later-wins precedence, sorted), the
    ///   same source of truth the inline prompt uses.
    /// - DELTA: emit lines only for types NOT yet announced
    ///   ([`Self::sent_agent_names`]); `is_initial` = the set was empty BEFORE
    ///   this turn (TS `announced.size === 0`). An empty delta ⇒ `None`.
    /// - RENDER: `<system-reminder>\n{header}\n{lines}\n</system-reminder>` with
    ///   the `is_initial`-conditional header (messages.ts:4197-4199), wrapped as
    ///   a meta user message.
    ///
    /// Like the skill-listing + conditional-rules reminders, the message is
    /// appended ONLY to the per-turn OUTGOING snapshot (never `session.history` /
    /// JSONL), so it is recomputed each turn and never accumulates.
    ///
    /// DOCUMENTED DEFERRAL vs TS: the `removedTypes` branch (an agent type that
    /// DISAPPEARS, messages.ts:4202-4206) and the subscription-conditioned
    /// "launch multiple agents concurrently" note (`showConcurrencyNote`,
    /// messages.ts:4207-4211) are omitted. The Rust catalog is wired once at boot
    /// and does not shrink mid-session (no live `/reload-plugins` removal path),
    /// and the concurrency note is subscription-gated (no subscription signal
    /// here) and secondary; the inline path keeps its own existing note.
    pub(crate) async fn agent_listing_reminder_message(&self) -> Option<ConversationMessage> {
        // GATE: off by default (no GrowthBook in Rust) ⇒ no reminder, inline
        // catalog stays byte-identical.
        if !agent::should_inject_agent_list_in_messages() {
            return None;
        }
        // Gate on the Agent tool being available this turn (attachments.ts:1497).
        // `find_by_name` also matches the legacy `Task` alias. This is the ONLY
        // structural gate in the binary's `aLe` — it does NOT gate on a wired
        // DISK catalog (see below).
        if self.tools.find_by_name("Agent").is_none() {
            return None;
        }

        // Merge BUILT-INS first, then the wired DISK catalog (if any) on top.
        // Built-ins are ALWAYS part of the listing — the binary's `aLe` builds
        // the delta from `activeAgents` (= built-ins + user/project agents via
        // `getAgents`), so a session with NO disk catalog still announces the
        // built-in agents. (Previously this early-returned when `agent_catalog`
        // was unset, suppressing built-ins entirely under the gate — a divergence
        // from `aLe`.) Later-wins precedence: a same-named catalog agent overrides
        // a built-in, matching the inline `AgentTool` prompt's
        // `PoolSubagentSpawner::listing_entries` (built-in < user/project).
        let mut defs = agent::builtin_agent_definitions();
        if let Some(catalog) = self.agent_catalog.as_ref() {
            defs.extend(catalog.read().await.iter().cloned());
        }
        let entries = agent::agent_listing_entries(&defs);

        // DELTA: keep only types not yet announced, then record them as sent.
        // `is_initial` is captured BEFORE inserting (TS `announced.size === 0`).
        let (is_initial, new_entries): (bool, Vec<traits::subagent_spawn::SubagentListingEntry>) = {
            let mut sent = self.sent_agent_names.lock().await;
            let is_initial = sent.is_empty();
            let delta: Vec<_> = entries
                .into_iter()
                .filter(|e| !sent.contains(&e.agent_type))
                .collect();
            for e in &delta {
                sent.insert(e.agent_type.clone());
            }
            (is_initial, delta)
        };
        if new_entries.is_empty() {
            return None;
        }

        // RENDER: header (is_initial-conditional) + one formatAgentLine per new
        // type, wrapped in a single `<system-reminder>` (messages.ts:4194-4214).
        let header = if is_initial {
            "Available agent types for the Agent tool:"
        } else {
            "New agent types are now available for the Agent tool:"
        };
        let lines = new_entries
            .iter()
            .map(agent::format_agent_line)
            .collect::<Vec<_>>()
            .join("\n");
        let content = format!("<system-reminder>\n{header}\n{lines}\n</system-reminder>");
        Some(ConversationMessage::user(MessageId::new(), content))
    }

    /// §F: the per-turn, transient `conditional_rules` reminder — path-gated
    /// LINGXI.md rules (`paths:`-globbed) that newly ACTIVATE because a file the
    /// session has touched this run matches their globs. Returns `None` when no
    /// memory provider is wired, the hierarchy has no conditional rules, or no
    /// newly-activated rule exists this turn.
    ///
    /// 1:1 with claude-code `processConditionedMdRules` (claudemd.ts:1354-1397)
    /// fed through the `nested_memory` render seam (messages.ts:3700-3707):
    ///
    /// 1. CACHE: the first call loads the full hierarchy (`memory.load(&cwd)` —
    ///    the same call the system prompt uses) and caches the `globs.is_some()`
    ///    subset in [`Self::conditional_rules_cache`]. Later turns reuse the cache
    ///    — no disk re-walk — and only re-test it against the latest touched set.
    /// 2. MATCH: for each cached rule and each touched file in
    ///    [`Self::read_file_state`] (the absolutized Read/Edit/Write/… paths),
    ///    [`crate::prompt::conditional_rules::rule_matches_touched_file`] derives
    ///    the rule's base dir (Project → parent-of-`.claude`; else `cwd`),
    ///    relativizes + guards the touched path, and gitignore-tests it against
    ///    the rule's globs. A rule with ANY matching touched file is ACTIVE.
    /// 3. DELTA: a rule already in [`Self::sent_conditional_rules`] is skipped
    ///    (TS `loadedNestedMemoryPaths`), so each rule injects ONCE. Newly-active
    ///    rules are recorded as sent and rendered.
    /// 4. RENDER: each newly-active rule becomes a bare `Contents of {path}:` body
    ///    wrapped in `<system-reminder>` (messages.ts `nested_memory`), joined by
    ///    a blank line into one meta user message (TS pushes one wrapped message
    ///    per rule; concatenation here is byte-equivalent for a single rule and a
    ///    faithful grouping for several).
    ///
    /// Appended ONLY to the per-turn outgoing snapshot (never `session.history` /
    /// JSONL), exactly like the skill-listing + output-style reminders.
    /// Per-turn, transient `<new-diagnostics>` reminder — newly-reported LSP
    /// diagnostics not yet surfaced to the model (claude-code's
    /// `formatDiagnosticsBlock` flow). `None` when no LSP source is wired (no
    /// servers ⇒ the common case) or there are no new diagnostics. The block is
    /// already wrapped in its own `<new-diagnostics>` tag (NOT `<system-reminder>`),
    /// so it is injected as a bare meta user message, appended ONLY to the
    /// outgoing snapshot (never `session.history` / JSONL).
    pub(crate) async fn new_diagnostics_reminder_message(&self) -> Option<ConversationMessage> {
        let block = self
            .new_diagnostics_source
            .as_ref()?
            .take_new_diagnostics_block()
            .await?;
        Some(ConversationMessage::user(MessageId::new(), block))
    }

    pub(crate) async fn conditional_rules_reminder_message(&self) -> Option<ConversationMessage> {
        // (1) CACHE — fill once from the same memory load the system prompt uses.
        let cwd = self.cwd.clone();
        let rules = self
            .conditional_rules_cache
            .get_or_init(|| async {
                self.memory
                    .load(&cwd)
                    .await
                    .into_iter()
                    .filter(|f| f.globs.is_some())
                    .collect::<Vec<_>>()
            })
            .await;
        if rules.is_empty() {
            return None;
        }

        // Snapshot the touched files (absolutized Read/Edit/Write/… paths).
        let touched: Vec<std::path::PathBuf> = self.read_file_state.lock().await.clone();
        if touched.is_empty() {
            return None;
        }

        // (2)+(3) MATCH + DELTA — collect newly-active rules not yet sent.
        let mut newly_active: Vec<&crate::prompt::MemoryFile> = Vec::new();
        {
            let mut sent = self.sent_conditional_rules.lock().await;
            for rule in rules {
                if sent.contains(&rule.path) {
                    continue; // already injected this session
                }
                let active = touched.iter().any(|t| {
                    crate::prompt::conditional_rules::rule_matches_touched_file(rule, t, &cwd)
                });
                if active {
                    sent.insert(rule.path.clone());
                    newly_active.push(rule);
                }
            }
        }
        if newly_active.is_empty() {
            return None;
        }

        // (4) RENDER — one `<system-reminder>` block per rule, joined by a blank
        // line into a single meta user message.
        let content = newly_active
            .iter()
            .map(|r| crate::prompt::conditional_rules::render_reminder(r))
            .collect::<Vec<_>>()
            .join("\n\n");
        Some(ConversationMessage::user(MessageId::new(), content))
    }

    /// P0.1: arm the memory-selector prefetch for THIS turn, firing it
    /// CONCURRENTLY with the main API call (claude-code's `wAo` prefetch
    /// side-channel). Called at the START of each turn in BOTH drivers, BEFORE
    /// the snapshot is assembled, so the in-flight handle is ready for
    /// [`Self::relevant_memory_reminder_message`] to await. A strict no-op when
    /// no prefetch is wired ([`Self::memory_prefetch`] is `None`) — then the slot
    /// stays empty and the surfacing reminder is `None`, keeping the locked
    /// fixtures byte-identical.
    ///
    /// The prefetch query is the latest NON-meta user-message text in the
    /// session history (mirroring TS `e.findLast(m => m.type==="user" &&
    /// !m.isMeta)` in `wAo`). The memdir directory is derived from the cwd; the
    /// stub prefetch ignores both for now (it resolves to an empty set) so this
    /// is inert by default.
    pub(crate) async fn start_memory_prefetch(&self) {
        let Some(prefetch) = self.memory_prefetch.as_ref() else {
            return; // no prefetch wired ⇒ surfacing channel stays inert
        };
        // Latest non-meta user message = the turn query (TS findLast user/!meta).
        // Meta messages (the transient reminders we append) carry no user intent,
        // but they never enter `session.history`, so a plain last-user scan over
        // history is faithful here.
        let query = {
            let s = self.session.lock().await;
            s.history
                .iter()
                .rev()
                .find(|m| matches!(m.role(), protocol::MessageRole::User))
                .map(ConversationMessage::text_content)
                .unwrap_or_default()
        };
        let pending = prefetch.start(query, self.cwd.clone()).await;
        *self.pending_memory_prefetch.lock().await = Some(pending);
    }

    /// P0.1: the per-turn, transient `relevant_memories` SURFACING reminder — the
    /// memory-selector/prefetch result rendered as a single `<system-reminder>`
    /// meta user message. Returns `None` when no prefetch was armed this turn
    /// ([`Self::start_memory_prefetch`] left the slot empty / no prefetch wired),
    /// the prefetch resolved to an empty set, or every surfaced memory was
    /// already injected (the SHARED dedup below).
    ///
    /// 1:1 with claude-code v2.1.181's `relevant_memories` attachment
    /// (`normalizeAttachmentForAPI` case `"relevant_memories"`, messages.ts —
    /// see [`memory::surfacing::render_surfacing_block`] for the exact shape):
    /// the em-dash idx-0 preamble + per-memory `Memory: {path}:` header (with a
    /// `>1`-day staleness prefix) wrapped in one `<system-reminder>` envelope.
    ///
    /// SHARED DEDUP: a memory is skipped when its path is in EITHER
    /// [`Self::surfaced_memory_paths`] (already surfaced a prior turn) OR
    /// [`Self::read_file_state`] (already loaded as a nested/conditional P3.2
    /// attachment OR read by a file tool) — so a file can never be double-injected
    /// across the surfacing + nested channels. Surfaced paths are recorded so each
    /// memory injects ONCE (TS prefetch consume-once + `loadedNestedMemoryPaths`).
    ///
    /// Like every other per-turn reminder, the message is appended ONLY to the
    /// per-turn OUTGOING snapshot (never `session.history` / JSONL), so it is
    /// recomputed each turn and never accumulates.
    pub(crate) async fn relevant_memory_reminder_message(&self) -> Option<ConversationMessage> {
        // Consume the in-flight prefetch handle armed at turn start. `None` ⇒ no
        // prefetch wired / not armed ⇒ no surfacing this turn.
        let pending = self.pending_memory_prefetch.lock().await.take()?;
        let surfaced = pending.take().await;
        if surfaced.is_empty() {
            return None;
        }

        // SHARED DEDUP — skip any memory already surfaced this session OR already
        // loaded as a nested/conditional attachment / tool read (`read_file_state`).
        let already_read: std::collections::HashSet<std::path::PathBuf> =
            self.read_file_state.lock().await.iter().cloned().collect();
        let fresh: Vec<memory::surfacing::SurfacedMemory> = {
            let mut surfaced_set = self.surfaced_memory_paths.lock().await;
            let mut out = Vec::new();
            for m in surfaced {
                if surfaced_set.contains(&m.path) || already_read.contains(&m.path) {
                    continue; // double-injection guard
                }
                surfaced_set.insert(m.path.clone());
                out.push(m);
            }
            out
        };
        if fresh.is_empty() {
            return None;
        }

        let content = memory::surfacing::render_surfacing_block(&fresh);
        Some(ConversationMessage::user(MessageId::new(), content))
    }

    /// EXPERIMENTAL_SKILL_SEARCH: arm the skill-discovery prefetch CONCURRENTLY
    /// with this turn (1:1 with claude-code `startSkillDiscoveryPrefetch`, bundle
    /// fn `C1z`: `B=at1?.startSkillDiscoveryPrefetch(null,V,T)` at iteration top).
    /// A strict no-op when no prefetch is wired ([`Self::skill_discovery_prefetch`]
    /// is `None`) — then the slot stays empty and the surfacing reminder is `None`,
    /// keeping the locked fixtures byte-identical.
    ///
    /// The prefetch query is the latest non-meta user-message text (same scan as
    /// [`Self::start_memory_prefetch`], mirroring TS `findLast(user/!meta)`). The
    /// per-iteration `findWritePivot` guard (`query.ts:323` — discovery only fires
    /// on write-pivot iterations) is computed from the most recent assistant
    /// message's requested tools (see [`skill_api::find_write_pivot`],
    /// [RECONSTRUCTED]); on a non-write iteration the prefetch ships empty.
    pub(crate) async fn start_skill_discovery_prefetch(&self) {
        let Some(prefetch) = self.skill_discovery_prefetch.as_ref() else {
            return; // no prefetch wired ⇒ discovery channel stays inert
        };
        // Latest non-meta user message = the turn query (TS findLast user/!meta),
        // and the most recent assistant message's tool names for the write-pivot
        // predicate — both read in one history lock.
        let (query, last_assistant_tools) = {
            let s = self.session.lock().await;
            let query = s
                .history
                .iter()
                .rev()
                .find(|m| matches!(m.role(), protocol::MessageRole::User))
                .map(ConversationMessage::text_content)
                .unwrap_or_default();
            let last_assistant_tools = s
                .history
                .iter()
                .rev()
                .find(|m| matches!(m.role(), protocol::MessageRole::Assistant))
                .map(|m| {
                    m.tool_calls()
                        .into_iter()
                        .filter_map(|b| match b {
                            protocol::ContentBlock::ToolUse { name, .. } => Some(name.clone()),
                            _ => None,
                        })
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default();
            (query, last_assistant_tools)
        };
        let is_write_pivot = skill_api::find_write_pivot(&last_assistant_tools);
        let pending = prefetch.start(query, is_write_pivot).await;
        *self.pending_skill_prefetch.lock().await = Some(pending);
    }

    /// EXPERIMENTAL_SKILL_SEARCH: the per-turn, transient `skill_discovery`
    /// SURFACING reminder — the prefetch result rendered as a single
    /// `<system-reminder>` meta user message (1:1 with claude-code's
    /// `collectSkillDiscoveryPrefetch` → `skill_discovery` attachment,
    /// `messages.ts:3506-3519`). Returns `None` when no prefetch was armed this
    /// turn, the prefetch resolved to an empty set, or every discovered skill was
    /// already surfaced.
    ///
    /// Emits the `hidden_by_main_turn` telemetry field (`query.ts:1617`): `true`
    /// when the prefetch resolved BEFORE collection (it hid under the main turn's
    /// streaming + tool execution; expected >98%). Peeked via
    /// [`skill_api::PendingSkillDiscoveryPrefetch::is_ready`] before the consuming
    /// `take`.
    ///
    /// DEDUP: a skill is skipped when its `name` is in
    /// [`Self::surfaced_skill_names`] (already surfaced a prior turn). Keyed on
    /// `name` (skill names are not files, so — unlike the memory channel — this
    /// does NOT consult `read_file_state`). Surfaced names are recorded so each
    /// skill injects ONCE. Like every other per-turn reminder, the message is
    /// appended ONLY to the per-turn OUTGOING snapshot (never `session.history` /
    /// JSONL).
    pub(crate) async fn skill_discovery_reminder_message(&self) -> Option<ConversationMessage> {
        // Consume the in-flight prefetch handle armed at turn start.
        let pending = self.pending_skill_prefetch.lock().await.take()?;

        // hidden_by_main_turn — peek readiness BEFORE the consuming take.
        let hidden = pending.is_ready();
        telemetry::emit_skill_discovery_collected(hidden);

        let skills = pending.take().await;
        if skills.is_empty() {
            return None;
        }

        // DEDUP by name — skip any skill already surfaced this session.
        let fresh: Vec<skill_api::DiscoveredSkill> = {
            let mut surfaced = self.surfaced_skill_names.lock().await;
            let mut out = Vec::new();
            for s in skills {
                if surfaced.contains(&s.name) {
                    continue; // already surfaced a prior turn
                }
                surfaced.insert(s.name.clone());
                out.push(s);
            }
            out
        };

        // render returns None on empty (TS `return []`).
        let content = skill_api::render_skill_discovery_block(&fresh)?;
        Some(ConversationMessage::user(MessageId::new(), content))
    }

    /// Build the wire `tools` array for a turn from the registry's enabled tool
    /// set, serialized via [`tool_api::wire::tools_to_wire`] to the
    /// `{name, description, input_schema}` shape claude-code sends
    /// (`utils/api.ts:169-178`). Used by both the batched ([`execute_one_turn`])
    /// and streaming ([`Self::run_turn_streaming`]) paths.
    ///
    /// `ToolStaticContext::default()` (no feature flags) mirrors the system
    /// prompt's enable-filter punt; `include_examples: true` requests the full
    /// tool prompt as the `description`. Recomputed per turn (no session-level
    /// `toolSchemaCache` analog yet). The wire order is parity-fixed by
    /// [`available_tools`](tool_api::ToolRegistry::available_tools): builtins
    /// `locale_cmp`-sorted as a contiguous prefix, then MCP / LSP / plugin
    /// tools `locale_cmp`-sorted — matching claude-code's `assembleToolPool` /
    /// `mergeAndFilterTools` (`tools.ts:345-367`, `utils/toolPool.ts:65-70`),
    /// which sort with `name.localeCompare`. `tools_to_wire` preserves that
    /// order. A session-level cache is a recommended follow-up.
    ///
    /// [`execute_one_turn`]: crate::turn_loop::execute_one_turn
    pub(crate) async fn build_wire_tools(&self) -> Vec<serde_json::Value> {
        use tool_api::tool_trait::{PromptOptions, ToolStaticContext};
        let mut tools = self.tools.available_tools(&ToolStaticContext::default());
        // Tool-wide deny filter (claude-code `filterToolsByDenyRules`,
        // `tools.ts:307-310`): strip every tool a TOOL-WIDE deny rule blankets,
        // BEFORE the model sees it, using the SAME matcher the runtime check uses
        // (`tool_wide_name_matches` — exact name OR an `mcp__server` prefix that
        // covers all `mcp__server__tool` of that server). The names come from the
        // permission gate; the default gate (no rule layer) returns an EMPTY list,
        // so with zero deny rules `tools` is untouched and the wire bytes are
        // byte-identical to before (regression-safe). Content deny rules
        // (`Bash(rm:*)`, `WebFetch(domain:x)`) are NOT in this list — they deny
        // specific calls, not the tool, so the tool stays advertised.
        let denied = self.perms.tool_wide_deny_names().await;
        if !denied.is_empty() {
            tools.retain(|t| {
                !denied
                    .iter()
                    .any(|d| permission::tool_wide_name_matches(d, t.name()))
            });
        }
        // claude-code builds the wire `tools` array with `prompt({model})`; the
        // session model gates model-dependent tool prompts (TodoWrite's
        // `Xla(model)=Dh(model)?FWd:UWd`). Snapshot it from the live session.
        let (model, model_profile) = {
            let s = self.session.lock().await;
            (s.model.clone(), s.model_profile.clone())
        };
        tool_api::wire::tools_to_wire(
            &tools,
            &PromptOptions {
                include_examples: true,
                model: Some(model),
                model_profile,
            },
        )
        .await
    }

    /// Borrow the in-memory session (read-write lock surrogate). Useful for tests.
    #[must_use]
    pub fn session(&self) -> Arc<Mutex<SessionState>> {
        self.session.clone()
    }

    /// Return all registered tool names (alphabetical, same order as the
    /// system-prompt listing). Used by stream-json `system/init` to populate
    /// the `tools` array — mirrors `self.tools.all_names()` but through a
    /// public seam that does not expose the `ToolRegistry` internals.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.all_names()
    }

    /// The session's DEFAULT main-loop model — the boot-configured model the
    /// stream-json `set_model` control_request resolves `"default"` (or an
    /// absent model) back to (claude-code `getDefaultMainLoopModel()`), so a
    /// client can revert a prior `set_model` override.
    #[must_use]
    pub fn default_model(&self) -> String {
        self.config.model.clone()
    }

    /// Apply a LIVE session permission-mode change (stream-json
    /// `set_permission_mode` control_request). Delegates to the gate's
    /// [`traits::PermissionGate::set_permission_mode`]; only the enforcing
    /// `PolicyPermissionGate` actually mutates (other gates no-op). Returns the
    /// gate's validation error string on an invalid / disallowed mode.
    pub async fn set_permission_mode(&self, mode: &str) -> Result<(), String> {
        self.perms.set_permission_mode(mode).await
    }
}

// ── Task 7 helpers ──────────────────────────────────────────────────────────

/// Convert an `LlmResponse` (from the non-streaming fallback call) into the
/// same [`crate::streaming_loop::PumpedTurn`] shape the streaming loop uses,
/// so the remainder of the streaming turn handler works unchanged.
///
/// Mirrors the batched path's `translate_response_blocks` call: content blocks
/// are translated to `protocol::ContentBlock`; `ToolCall` blocks additionally
/// populate the `tool_uses` vec so the concurrent dispatch runs exactly as in a
/// real stream.
/// Whether the turn's assistant blocks contain any user-visible text, mirroring
/// claude-code's thinking-only guard predicate (`bin/claude.exe` offset
/// ~202946760):
/// `ie.some(msg => msg.content.some(b => b.type === "text" && b.text.trim().length > 0))`.
///
/// A `false` return = a thinking-only (or otherwise text-empty) response. Only
/// [`protocol::ContentBlock::Text`] blocks with a non-whitespace body count;
/// `Thinking`, `ToolUse`, etc. are not "visible output" for this gate. (Tool
/// uses live in `PumpedTurn::tool_uses`, not `assistant_blocks`, and this gate
/// only fires on `end_turn`/`stop_sequence` where no `tool_use` is present.)
fn pumped_has_visible_text(blocks: &[protocol::ContentBlock]) -> bool {
    use protocol::ContentBlock;
    blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Text { text } if !text.trim().is_empty()))
}

fn llm_response_to_pumped_turn(resp: &LlmResponse) -> crate::streaming_loop::PumpedTurn {
    use crate::streaming_loop::{ObservedToolUse, PumpedTurn};
    use crate::turn_loop::translate_response_blocks;
    use protocol::ContentBlock;

    let output_tokens = resp.usage.billable_tokens.output;
    let stop_reason = resp.stop_reason.clone();
    // BILLING: carry the full usage so the streaming turn loop records it
    // in CostTracker. The non-streaming fallback issues a real messages_create
    // call (seeded) whose response includes the authoritative usage.
    let usage = Some(resp.usage.clone());

    // Translate LlmResponse content → protocol::ContentBlock (same as batched path).
    // Then split into (assistant_blocks, tool_uses): text/thinking go into
    // assistant_blocks; ToolUse blocks go into tool_uses for the concurrent dispatch.
    // The streaming loop step 4 re-assembles them into the assistant message by
    // appending ToolUse blocks from tool_uses — so we must NOT include ToolUse in
    // assistant_blocks (or they appear twice in the assistant message).
    let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
    let mut tool_uses: Vec<ObservedToolUse> = Vec::new();

    for blk in translate_response_blocks(&resp.content) {
        match blk {
            ContentBlock::ToolUse {
                id,
                ref name,
                ref input,
                ref provider_id,
            } => {
                tool_uses.push(ObservedToolUse {
                    id,
                    name: name.clone(),
                    input: input.clone(),
                    provider_id: provider_id.clone(),
                });
                // Not pushed to assistant_blocks — step 4 appends ToolUse from tool_uses.
            }
            other => {
                assistant_blocks.push(other);
            }
        }
    }

    PumpedTurn {
        assistant_blocks,
        tool_uses,
        stop_reason,
        output_tokens,
        usage,
        // Non-streaming fallback: carry the response's refusal stop_details so
        // the terminal refusal arm gets the cyber/bio variant.
        stop_details: resp.stop_details.clone(),
    }
}

/// Mirror TS `isEnvTruthy` (`utils/envUtils.ts:32`): a value is truthy ONLY
/// when, lowercased and trimmed, it is one of the whitelist members
/// `"1"`, `"true"`, `"yes"`, `"on"`. Absent, empty, and every other value
/// (including `"no"`, `"off"`, `"2"`, `"enabled"`, …) are falsy.
///
/// Locked against the TS helper used at `claude.ts:2470`:
/// `isEnvTruthy(process.env.LINGXI_DISABLE_NONSTREAMING_FALLBACK)`.
fn is_env_truthy(val: Option<&str>) -> bool {
    match val {
        None => false,
        Some(v) => matches!(v.to_lowercase().trim(), "1" | "true" | "yes" | "on"),
    }
}

// NOTE: `AnthropicProviderAdapter` and `AnthropicProviderStreamingAdapter`
// were removed in Task 5 — they drove `api_client::AnthropicProvider` directly.
// The live path is now `ProviderApiAdapter` (provider_adapter.rs), retargeted
// in Task 6 to drive `llm_client::DefaultLlmClient`. (3b deletes api-client.)

/// Internal no-op streaming client used by [`ConversationOrchestrator::new`]
/// when the caller doesn't supply a streaming transport. Every call to
/// `stream` returns `LlmError::Transport("no streaming client configured")`.
/// Wired in Task 12 when the legacy `new()` constructor delegates to
/// `new_with_streaming(..., NoStreamingApiClient, ...)`.
#[allow(dead_code)]
pub(crate) struct NoStreamingApiClient;

#[async_trait]
impl StreamingApiClient for NoStreamingApiClient {
    async fn stream(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<futures::stream::BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        Err(LlmError::Transport {
            message: "no streaming client configured".into(),
        })
    }
}

// ============================================================================
// Turn-recovery behaviors (RECOV.1 / RECOV.2 / RECOV.4)
// ============================================================================
//
// In-file integration tests for the three turn-driver recovery behaviors ported
// from claude-code `query.ts`:
//   - RECOV.1 — the streaming driver's blocking-limit preempt
//     (`query.ts:592-648`): a prompt already at the hard blocking limit ends the
//     turn with the byte-exact prompt-too-long message WITHOUT opening the stream.
//   - RECOV.2 — `StopFailure` hooks fire on an api-error turn-end
//     (`query.ts:1174/1181/1263`); the normal `Stop` hooks do NOT.
//   - RECOV.4 — a Stop-hook blocking continuation resets the
//     `max_output_tokens` recovery budget (`query.ts:1291`).
#[cfg(test)]
mod turn_recovery_tests {
    use super::*;
    use crate::test_support::{
        content_block_start_text, content_block_stop, message_delta_stop, message_start,
        message_stop, mock_message_response, noop_hook_executor, text_delta, MockApiClient,
        MockOutputStream, MockStreamingApiClient, NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
    use hooks::events::HookEventType;
    use hooks::executor::BuiltinHookHandler;
    use hooks::registry::HookRegistry;
    use hooks::response::{HookDecision, HookOutcome, HookResponse, HookResult};
    use hooks::HookExecutorImpl;
    use llm_client::ContentBlock as LlmContentBlock;
    use protocol::{HookId, HttpRequest, HttpResponse};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;
    use tokio::sync::{Notify, RwLock};
    use traits::{HttpError, HttpTransport, OutputEvent, RuntimeError, RuntimeSpawner};

    async fn wait_for_prewarm_capture(
        api: &MockApiClient,
    ) -> Vec<crate::test_support::MockPrewarmCall> {
        for _ in 0..50 {
            let captured = api.captured_prewarm().await;
            if !captured.is_empty() {
                return captured;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        api.captured_prewarm().await
    }

    struct BlockingPrewarmApiClient {
        active: Arc<AtomicBool>,
        started: Notify,
    }

    impl BlockingPrewarmApiClient {
        fn new() -> Self {
            Self {
                active: Arc::new(AtomicBool::new(false)),
                started: Notify::new(),
            }
        }

        async fn wait_started(&self) {
            loop {
                let notified = self.started.notified();
                if self.active.load(Ordering::SeqCst) {
                    return;
                }
                tokio::time::timeout(Duration::from_secs(1), notified)
                    .await
                    .expect("startup prewarm should start");
            }
        }

        async fn wait_inactive(&self) {
            for _ in 0..50 {
                if !self.active.load(Ordering::SeqCst) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("startup prewarm should have been aborted");
        }
    }

    struct ActivePrewarmGuard(Arc<AtomicBool>);

    impl Drop for ActivePrewarmGuard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl OrchestratorApiClient for BlockingPrewarmApiClient {
        async fn messages_create(
            &self,
            _model: &str,
            _profile: Option<&str>,
            _system: Option<&str>,
            _msgs: Vec<ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<LlmResponse, LlmError> {
            Err(LlmError::Transport {
                message: "blocking prewarm api does not serve messages_create".into(),
            })
        }

        async fn prewarm_responses_websocket(
            &self,
            _model: &str,
            _profile: Option<&str>,
            _system: Option<&str>,
            _messages: Vec<ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<(), LlmError> {
            self.active.store(true, Ordering::SeqCst);
            self.started.notify_waiters();
            let _guard = ActivePrewarmGuard(self.active.clone());
            std::future::pending::<()>().await;
            Ok(())
        }
    }

    // ---- unused HTTP / Runtime stubs (Builtin hooks never touch them) ----
    struct UnusedHttp;
    #[async_trait]
    impl HttpTransport for UnusedHttp {
        async fn request(&self, _r: HttpRequest) -> Result<HttpResponse, HttpError> {
            Err(HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(&self, _r: HttpRequest) -> Result<traits::http::SseStream, HttpError> {
            Err(HttpError::InvalidRequest("unused".into()))
        }
    }
    struct UnusedRuntime;
    #[async_trait]
    impl RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _n: &str,
            _t: Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, RuntimeError> {
            Err(RuntimeError::Internal("unused".into()))
        }
        async fn sleep(&self, _d: Duration) {}
        async fn cancel(&self, _h: &traits::BackgroundTaskHandle) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    /// Records every `Stop` / `StopFailure` lifecycle event it sees as
    /// `"Stop:<reason>"` / `"StopFailure:<error>"`. Pass-through (no decision).
    struct RecordingLifecycleHandler {
        log: Arc<StdMutex<Vec<String>>>,
    }
    #[async_trait]
    impl BuiltinHookHandler for RecordingLifecycleHandler {
        fn id(&self) -> &str {
            "rec-lifecycle"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            match event {
                HookEvent::Stop { reason } => {
                    self.log.lock().unwrap().push(format!("Stop:{reason}"));
                }
                HookEvent::StopFailure { error } => {
                    self.log
                        .lock()
                        .unwrap()
                        .push(format!("StopFailure:{error}"));
                }
                _ => {}
            }
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response: None,
            }
        }
    }

    /// Stop hook that ALWAYS blocks (asks the agent to keep working). The
    /// re-entry guard converts a SECOND block (when `stop_hook_active`) into a
    /// pass so the loop cannot spin forever.
    struct BlockingStopHandler;
    #[async_trait]
    impl BuiltinHookHandler for BlockingStopHandler {
        fn id(&self) -> &str {
            "block-stop"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            let response = matches!(event, HookEvent::Stop { .. }).then(|| HookResponse {
                decision: Some(HookDecision::Block),
                reason: Some("keep going".into()),
                system_message: Some("[stop-hook] please continue".into()),
                ..Default::default()
            });
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response,
            }
        }
    }

    /// `PreCompact` hook that ALWAYS blocks — exercises the compaction abort
    /// (TS `xhe` sets `blockedBy` from the blocked result; `VJn` / proactive /
    /// reactive all honor it by aborting the pass).
    struct BlockingPreCompactHandler;
    #[async_trait]
    impl BuiltinHookHandler for BlockingPreCompactHandler {
        fn id(&self) -> &str {
            "block-precompact"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            let response = matches!(event, HookEvent::PreCompact { .. }).then(|| HookResponse {
                decision: Some(HookDecision::Block),
                reason: Some("[guard] compaction not allowed".into()),
                ..Default::default()
            });
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response,
            }
        }
    }

    /// `SessionStart` hook that emits `hookSpecificOutput.additionalContext`
    /// (`Some`) or nothing (`None`) — exercises the SESSIONSTART.CTX consumption.
    struct SessionStartCtxHandler {
        ctx: Option<String>,
    }
    #[async_trait]
    impl BuiltinHookHandler for SessionStartCtxHandler {
        fn id(&self) -> &str {
            "sess-ctx"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            let response = matches!(event, HookEvent::SessionStart { .. }).then(|| HookResponse {
                additional_context: self.ctx.clone(),
                ..Default::default()
            });
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response,
            }
        }
    }

    async fn exec_session_start_ctx(ctx: Option<String>) -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry
            .write()
            .await
            .register(builtin_hook("sess-ctx", HookEventType::SessionStart));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(SessionStartCtxHandler { ctx }));
        Arc::new(exec)
    }

    fn builtin_hook(handler_id: &str, event_type: HookEventType) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: handler_id.into(),
            events: vec![event_type],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: handler_id.into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    async fn exec_recording(
        log: Arc<StdMutex<Vec<String>>>,
        events: &[HookEventType],
    ) -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        {
            let mut r = registry.write().await;
            for ev in events {
                r.register(builtin_hook("rec-lifecycle", ev.clone()));
            }
        }
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(RecordingLifecycleHandler { log }));
        Arc::new(exec)
    }

    async fn exec_blocking_stop() -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry
            .write()
            .await
            .register(builtin_hook("block-stop", HookEventType::Stop));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(BlockingStopHandler));
        Arc::new(exec)
    }

    async fn exec_blocking_pre_compact() -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry
            .write()
            .await
            .register(builtin_hook("block-precompact", HookEventType::PreCompact));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(BlockingPreCompactHandler));
        Arc::new(exec)
    }

    fn compact_orch(hooks: Arc<HookExecutorImpl>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            hooks,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
    }

    // A blocking PreCompact hook aborts compaction in every route (TS `xhe` /
    // `VJn`): `fire_pre_compact` surfaces the block detail so the caller can
    // throw ("Compaction blocked by PreCompact hook: …") / log + skip.
    #[tokio::test]
    async fn pre_compact_block_returns_detail_else_none() {
        let blocked = compact_orch(exec_blocking_pre_compact().await)
            .fire_pre_compact("manual")
            .await;
        assert_eq!(
            blocked.as_deref(),
            Some("[guard] compaction not allowed"),
            "a blocking PreCompact hook must surface its blockedBy detail"
        );

        // No PreCompact hook registered → None → compaction proceeds unchanged.
        let proceed = compact_orch(noop_hook_executor())
            .fire_pre_compact("auto")
            .await;
        assert_eq!(proceed, None, "no block → compaction proceeds");
    }

    /// A Stop hook that blocks EXACTLY ONCE, then passes. Used by tests that need
    /// precisely one stop-hook continuation, isolated from the consecutive-block
    /// CAP (`LINGXI_STOP_HOOK_BLOCK_CAP`, default 8): a block-every-time hook
    /// would now drive up to 8 continuations, so a test asserting a single
    /// continuation must bound the blocking deterministically.
    struct BlockOnceStopHandler {
        blocked: std::sync::atomic::AtomicBool,
    }
    #[async_trait]
    impl BuiltinHookHandler for BlockOnceStopHandler {
        fn id(&self) -> &str {
            "block-stop"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            let first = matches!(event, HookEvent::Stop { .. })
                && !self.blocked.swap(true, std::sync::atomic::Ordering::SeqCst);
            let response = first.then(|| HookResponse {
                decision: Some(HookDecision::Block),
                reason: Some("keep going".into()),
                system_message: Some("[stop-hook] please continue".into()),
                ..Default::default()
            });
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response,
            }
        }
    }

    async fn exec_block_once_stop() -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry
            .write()
            .await
            .register(builtin_hook("block-stop", HookEventType::Stop));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(BlockOnceStopHandler {
            blocked: std::sync::atomic::AtomicBool::new(false),
        }));
        Arc::new(exec)
    }

    /// A Stop hook that requests `continue:false` (preventContinuation) with a
    /// fixed `stopReason` — terminates the agent loop (FIX C).
    struct PreventStopHandler {
        reason: Option<String>,
    }
    #[async_trait]
    impl BuiltinHookHandler for PreventStopHandler {
        fn id(&self) -> &str {
            "prevent-stop"
        }
        async fn handle(&self, event: &HookEvent, _ctx: &HookContext) -> HookResult {
            let response = matches!(event, HookEvent::Stop { .. }).then(|| HookResponse {
                prevent_continuation: true,
                reason: self.reason.clone(),
                ..Default::default()
            });
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response,
            }
        }
    }

    async fn exec_prevent_stop(reason: Option<String>) -> Arc<HookExecutorImpl> {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        registry
            .write()
            .await
            .register(builtin_hook("prevent-stop", HookEventType::Stop));
        let mut exec =
            HookExecutorImpl::new(registry, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(PreventStopHandler { reason }));
        Arc::new(exec)
    }

    /// Seed a history far past the hard blocking limit. The default model
    /// (`claude-opus-4-8`) is natively 1M as of 2.1.198 (M1b), so the
    /// blocking limit sits just under 1M tokens; 8M chars ≈ 2M tokens
    /// (estimator is chars/4), comfortably over.
    async fn seed_over_blocking_limit(orch: &ConversationOrchestrator) {
        let session = orch.session();
        let mut s = session.lock().await;
        s.history.push(ConversationMessage::user(
            MessageId::new(),
            "x".repeat(8_000_000),
        ));
    }

    // -------- RECOV.1 — streaming blocking-limit preempt --------

    #[tokio::test]
    async fn recov1_streaming_blocking_limit_preempts_before_opening_stream() {
        // One valid end_turn turn is scripted; if the preempt regresses the
        // stream opens (captured_calls == 1) and the prompt-too-long text is
        // absent — both asserted against below.
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "should not be reached"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]));
        let output = Arc::new(MockOutputStream::new());
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        seed_over_blocking_limit(&orch).await;

        let outcome = orch
            .run_turn_streaming("go")
            .await
            .expect("turn ends without a hard error");
        assert!(
            matches!(outcome, ConversationOutcome::EndTurn { .. }),
            "{outcome:?}"
        );

        // The stream was NEVER opened — the preempt short-circuited the API call.
        assert!(
            streaming.captured_calls().await.is_empty(),
            "the blocking-limit preempt must NOT open the stream"
        );

        // The byte-exact prompt-too-long message + an EndTurn("blocking_limit").
        // The PROACTIVE blocking-limit preempt ends with the DISTINCT terminal
        // reason `blocking_limit` (the binary keeps it separate from the
        // reactive-exhausted `prompt_too_long`); the surfaced message text is
        // still the byte-exact "Prompt is too long".
        let events = output.snapshot().await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OutputEvent::Text { text } if text == "Prompt is too long")),
            "byte-exact prompt-too-long message must be surfaced; events={events:#?}"
        );
        assert!(
            events.iter().any(
                |e| matches!(e, OutputEvent::EndTurn { stop_reason, .. } if stop_reason == "blocking_limit")
            ),
            "the proactive preempt must end with stop_reason blocking_limit; events={events:#?}"
        );
    }

    // -------- terminal stop-reason API errors (claude.ts:2266-2292) --------

    #[tokio::test]
    async fn terminal_model_context_window_exceeded_surfaces_api_error() {
        // `model_context_window_exceeded` has no recovery path, so it hits the
        // terminal arm directly and must surface claude-code's byte-locked
        // API-error message (`claude.ts:2279`) before ending the turn.
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "partial answer"),
            content_block_stop(0),
            message_delta_stop("model_context_window_exceeded"),
            message_stop(),
        ]]));
        let output = Arc::new(MockOutputStream::new());
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.run_turn_streaming("go").await.expect("turn ends");

        let events = output.snapshot().await;
        assert!(
            events.iter().any(|e| matches!(e, OutputEvent::Text { text }
                if text == "API Error: The model has reached its context window limit.")),
            "byte-exact context-window-exceeded API error must be surfaced; events={events:#?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OutputEvent::EndTurn { stop_reason, .. }
                if stop_reason == "model_context_window_exceeded")),
            "the turn must end with stop_reason model_context_window_exceeded; events={events:#?}"
        );
    }

    #[tokio::test]
    async fn terminal_refusal_without_fallback_surfaces_safety_message() {
        // A `refusal` with no `refusalFallbackModel` configured hits the terminal
        // arm (the swap arm `continue`s only when a fallback is set), so it must
        // surface claude-code's byte-locked `U2e` refusal message — the model-label
        // branch (resolved via `marketing_name_for_model`), non-interactive suffix
        // (`interactive_permissions` defaults false).
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m", "claude-opus-4-8"),
            content_block_start_text(0),
            text_delta(0, "partial"),
            content_block_stop(0),
            message_delta_stop("refusal"),
            message_stop(),
        ]]));
        let output = Arc::new(MockOutputStream::new());
        let mut cfg = OrchestratorConfig::default();
        cfg.model = "claude-opus-4-8".to_string();
        assert!(
            cfg.refusal_fallback_model.is_none(),
            "default config must have no refusal fallback (else the swap arm runs)"
        );
        let orch = ConversationOrchestrator::new_with_streaming(
            cfg,
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.run_turn_streaming("go").await.expect("turn ends");

        let events = output.snapshot().await;
        let expected = "API Error: Opus 4.8's safeguards flagged this message (https://www.anthropic.com/legal/aup). This sometimes happens with safe, normal conversations. LingXi can't respond to this request with Opus 4.8.\n\nTry rephrasing the request in a new session or change your model.\n\nLearn more: https://support.claude.com/en/articles/15363606";
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OutputEvent::Text { text } if text == expected)),
            "byte-exact U2e refusal message must be surfaced; events={events:#?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OutputEvent::EndTurn { stop_reason, .. }
                if stop_reason == "refusal")),
            "the turn must end with stop_reason refusal; events={events:#?}"
        );
    }

    // -------- RECOV.2 — StopFailure fires on an api-error turn-end --------

    #[tokio::test]
    async fn recov2_stop_failure_fires_on_api_error_end_and_stop_does_not() {
        // A history over the blocking limit ⇒ the batched proactive preempt ends
        // with terminal reason `blocking_limit`, which is an api-error end (the
        // surfaced message's api-error field is `invalid_request`). `StopFailure`
        // must fire (error == "invalid_request"); the normal `Stop` hooks must NOT.
        let log = Arc::new(StdMutex::new(Vec::<String>::new()));
        let hooks = exec_recording(
            log.clone(),
            &[HookEventType::Stop, HookEventType::StopFailure],
        )
        .await;
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            // Never called — the blocking-limit preempt fires before the API call.
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            hooks,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        seed_over_blocking_limit(&orch).await;

        orch.run_turn("go")
            .await
            .expect("turn ends without a hard error");

        let seen = log.lock().unwrap().clone();
        assert!(
            seen.iter().any(|s| s == "StopFailure:invalid_request"),
            "StopFailure must fire on the api-error end with error=invalid_request: {seen:?}"
        );
        assert!(
            !seen.iter().any(|s| s.starts_with("Stop:")),
            "the normal Stop hooks must NOT fire on an api-error end: {seen:?}"
        );
    }

    #[tokio::test]
    async fn startup_responses_websocket_prewarm_uses_current_model_profile_system_and_empty_history(
    ) {
        let api = Arc::new(MockApiClient::new(vec![]));
        let orch = Arc::new(ConversationOrchestrator::new(
            OrchestratorConfig {
                model: "gpt-5".to_string(),
                ..OrchestratorConfig::default()
            },
            api.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        ));
        {
            let mut session = orch.session.lock().await;
            session.model_profile = Some("openai".to_string());
        }

        orch.spawn_startup_responses_websocket_prewarm();

        let captured = wait_for_prewarm_capture(&api).await;
        assert_eq!(captured.len(), 1);
        let call = &captured[0];
        assert_eq!(call.model, "gpt-5");
        assert_eq!(call.profile.as_deref(), Some("openai"));
        assert!(
            call.messages.is_empty(),
            "startup prewarm uses empty history"
        );
        assert!(
            call.system
                .as_deref()
                .is_some_and(|system| !system.is_empty()),
            "startup prewarm must use the assembled system prompt"
        );
    }

    #[tokio::test]
    async fn system_prompt_model_identity_follows_switch_model() {
        // Regression (reported): /model switched the ROUTED model, but the <env>
        // identity line ("You are powered by the model named …") stayed frozen
        // at config.model, so a switched-to model (e.g. Fable 5) still saw — and
        // reported — the launch model's identity (Opus 4.8). The prompt identity
        // must track the LIVE session.model that switch_model updates.
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig {
                model: "claude-opus-4-8".to_string(),
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let before = orch.build_system_prompt().await;
        assert!(
            before.contains(
                "powered by the model named Opus 4.8. The exact model ID is claude-opus-4-8"
            ),
            "launch identity present: {before}"
        );

        <ConversationOrchestrator as traits::OrchestratorHandle>::switch_model(
            &orch,
            "claude-fable-5",
            None,
        )
        .await
        .expect("switch_model");

        let after = orch.build_system_prompt().await;
        assert!(
            after.contains(
                "powered by the model named Fable 5. The exact model ID is claude-fable-5"
            ),
            "identity follows the switch: {after}"
        );
        // The stale identity LINE must be gone. (The static "most recent Claude
        // models … Opus 4.8" catalog sentence is model-independent and stays —
        // so assert on the identity line, not the bare "Opus 4.8" substring.)
        assert!(
            !after.contains("powered by the model named Opus 4.8"),
            "the stale identity line must be gone after switching: {after}"
        );
    }

    #[tokio::test]
    async fn non_claude_switch_uses_the_named_identity_form_not_id_only() {
        // A switched-to NON-Claude model must still get the strong "powered by
        // the model named {name}" form each turn (via the catalog display name),
        // not the weak id-only "the model {id}." — so its current identity is
        // asserted clearly. (Also: no Claude-catalog contamination.)
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig {
                model: "claude-opus-4-8".to_string(),
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        <ConversationOrchestrator as traits::OrchestratorHandle>::switch_model(
            &orch,
            "deepseek-v4-pro",
            Some("deepseek"),
        )
        .await
        .expect("switch_model");

        let sp = orch.build_system_prompt().await;
        assert!(
            sp.contains(" - You are powered by the model named ")
                && sp.contains("The exact model ID is deepseek-v4-pro."),
            "non-Claude model uses the named form with its exact id: {sp}"
        );
        assert!(
            !sp.contains(" - You are powered by the model deepseek-v4-pro."),
            "must NOT use the weak id-only fallback: {sp}"
        );
        // The prior fix: no Claude model-catalog line for a non-Claude model.
        assert!(
            !sp.contains("claude-fable-5"),
            "no Claude catalog contamination for a non-Claude model: {sp}"
        );
    }

    #[tokio::test]
    async fn streaming_turn_aborts_pending_startup_prewarm_before_opening_stream() {
        let api = Arc::new(BlockingPrewarmApiClient::new());
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "ok"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]));
        let output = Arc::new(MockOutputStream::new());
        let orch = Arc::new(ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            api.clone(),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output,
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        ));

        orch.spawn_startup_responses_websocket_prewarm();
        api.wait_started().await;

        let outcome = tokio::time::timeout(Duration::from_secs(1), orch.run_turn_streaming("go"))
            .await
            .expect("streaming turn must not wait for startup prewarm")
            .expect("streaming turn completes");

        assert!(
            matches!(outcome, ConversationOutcome::EndTurn { .. }),
            "{outcome:?}"
        );
        api.wait_inactive().await;
        assert!(
            orch.startup_responses_websocket_prewarm
                .lock()
                .expect("startup responses websocket prewarm")
                .is_none(),
            "turn start must clear the pending startup prewarm handle"
        );
        assert_eq!(
            streaming.captured_calls().await.len(),
            1,
            "the real streaming turn should still open normally"
        );
    }

    #[tokio::test]
    async fn clear_session_aborts_startup_prewarm_and_closes_responses_websocket_session() {
        let api = Arc::new(MockApiClient::new(vec![]));
        let orch = Arc::new(ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        ));

        orch.spawn_startup_responses_websocket_prewarm();
        <ConversationOrchestrator as traits::OrchestratorHandle>::clear_session(&*orch)
            .await
            .expect("clear session");

        assert_eq!(api.close_responses_ws_count().await, 1);
    }

    // -------- SESSIONSTART.CTX — SessionStart additionalContext consumption ----

    #[tokio::test]
    async fn session_start_additional_context_becomes_persistent_meta_history_message() {
        // A `SessionStart` hook that emits `hookSpecificOutput.additionalContext`
        // must surface it as a persistent `hook_additional_context` meta message
        // in the conversation history (claude-code `processSessionStartHooks`,
        // `sessionStart.ts:163-172` → `messages.ts:4117-4128`), so it rides every
        // subsequent turn. The bytes are the exact `wrapInSystemReminder`
        // (`hookName` = `SessionStart`, multi-line content preserved).
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            exec_session_start_ctx(Some("Project: lingxi\nBranch: main".into())).await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.fire_session_start("startup").await;

        let history = orch.session().lock().await.history.clone();
        assert_eq!(
            history.len(),
            1,
            "exactly one hook_additional_context message; got {history:?}"
        );
        let body = match &history[0] {
            ConversationMessage::User { content, .. } => content
                .iter()
                .filter_map(|b| match b {
                    protocol::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            other => panic!("expected a user meta message; got {other:?}"),
        };
        assert_eq!(
            body,
            "<system-reminder>\nSessionStart hook additional context: Project: lingxi\nBranch: main\n</system-reminder>",
            "exact hook_additional_context bytes (hookName=SessionStart, content joined by \\n)"
        );
    }

    #[tokio::test]
    async fn session_start_without_additional_context_pushes_nothing() {
        // Strict no-op: a SessionStart hook that emits no additionalContext leaves
        // the history untouched (the aggregate is discarded exactly as before).
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            exec_session_start_ctx(None).await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.fire_session_start("startup").await;

        assert!(
            orch.session().lock().await.history.is_empty(),
            "no additionalContext ⇒ nothing pushed to history"
        );
    }

    // -------- RECOV.4 — recovery budget reset on stop-hook continuation -----

    #[tokio::test]
    async fn recov4_stop_hook_continuation_resets_max_output_tokens_recovery() {
        // Script: max_tokens, max_tokens, end_turn, then max_tokens ×4.
        // With the reset on the stop-hook continuation, the post-continuation
        // episode gets a FRESH budget of MAX_OUTPUT_TOKENS_RECOVERY_LIMIT (3)
        // nudges, so the loop makes exactly 7 API calls and injects 5 nudges.
        // WITHOUT the reset the carried count (2) would exhaust after only 2
        // more calls (5 total, 3 nudges).
        let mt = || {
            mock_message_response(
                vec![LlmContentBlock::Text {
                    text: "partial".into(),
                    cache_control: None,
                }],
                Some("max_tokens"),
            )
        };
        let et = || {
            mock_message_response(
                vec![LlmContentBlock::Text {
                    text: "done".into(),
                    cache_control: None,
                }],
                Some("end_turn"),
            )
        };
        let api = Arc::new(MockApiClient::new(vec![
            mt(),
            mt(),
            et(),
            mt(),
            mt(),
            mt(),
            mt(),
        ]));
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api.clone(),
            Arc::new(ToolRegistry::new()),
            // Block-ONCE: this test isolates the recovery-reset on a SINGLE
            // stop-hook continuation. A block-every-time hook would now (post
            // #2 cap-counter) also block the final recovery-exhaustion end and
            // drive further continuations up to LINGXI_STOP_HOOK_BLOCK_CAP
            // (default 8), exhausting the scripted responses.
            exec_block_once_stop().await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let outcome = orch.run_turn("go").await.expect("turn ok");
        assert!(
            matches!(outcome, ConversationOutcome::EndTurn { .. }),
            "{outcome:?}"
        );
        assert_eq!(
            api.captured_msgs().await.len(),
            7,
            "the stop-hook continuation must reset the recovery budget (fresh 3 nudges ⇒ 7 API calls)"
        );
        let nudges = orch
            .session()
            .lock()
            .await
            .history
            .iter()
            .filter(|m| m.text_content() == MAX_OUTPUT_TOKENS_RECOVERY_NUDGE)
            .count();
        assert_eq!(
            nudges, 5,
            "5 recovery nudges expected across the two episodes (2 before + 3 after the reset)"
        );
    }

    // -------- FIX C — Stop hook_stopped_continuation meta message -----------

    #[tokio::test]
    async fn fix_c_stop_prevent_continuation_persists_stopped_message() {
        // Parity with claude-code `query/stopHooks.ts:269-280` — a Stop hook's
        // `continue:false` (preventContinuation) yields a
        // `hook_stopped_continuation` attachment (hookName `Stop`), rendered as
        // an isMeta `<system-reminder>\nStop hook stopped continuation:
        // {stopReason}\n</system-reminder>` user message before the turn
        // terminates. Script a single end_turn, then let the Stop hook prevent
        // continuation.
        let et = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![et])),
            Arc::new(ToolRegistry::new()),
            exec_prevent_stop(Some("STOP-CONTINUATION".into())).await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let outcome = orch.run_turn("go").await.expect("turn ok");
        assert!(
            matches!(outcome, ConversationOutcome::StopHookPrevented { .. }),
            "continue:false must terminate as StopHookPrevented, got {outcome:?}"
        );

        // The exact meta message is appended to history (persisted via the same
        // `persist_message_to_jsonl` path as `append_stop_hook_feedback`).
        let session = orch.session();
        let s = session.lock().await;
        let found = s.history.iter().any(|m| {
            m.text_content()
                == "<system-reminder>\nStop hook stopped continuation: STOP-CONTINUATION\n</system-reminder>"
        });
        assert!(
            found,
            "the Stop hook_stopped_continuation meta message must be in history: {:#?}",
            s.history
                .iter()
                .map(protocol::ConversationMessage::text_content)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn fix_c_stop_prevent_continuation_default_reason() {
        // No `stopReason` → claude's default `'Stop hook prevented continuation'`
        // (`query/stopHooks.ts:271`).
        let et = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![et])),
            Arc::new(ToolRegistry::new()),
            exec_prevent_stop(None).await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        orch.run_turn("go").await.expect("turn ok");

        let session = orch.session();
        let s = session.lock().await;
        let found = s.history.iter().any(|m| {
            m.text_content()
                == "<system-reminder>\nStop hook stopped continuation: Stop hook prevented continuation\n</system-reminder>"
        });
        assert!(found, "default stopReason must be used");
    }
}

// ============================================================================
// OUTSTYLE.3: per-turn, transient output-style reminder.
//
// Proves the byte-exact `<system-reminder>` meta user message is appended to
// EACH turn's OUTGOING model input when a non-default output style is active,
// on BOTH turn drivers (batched `run_turn` + streaming `run_turn_streaming`),
// and that it is NEVER persisted to `session.history` nor the JSONL transcript
// (transient — never accumulates). With the default style the outgoing message
// list is byte-identical (no extra message), keeping the locked parity fixtures
// green.
// ============================================================================
#[cfg(test)]
mod output_style_reminder_tests {
    use super::*;
    use crate::test_support::{
        content_block_start_text, content_block_stop, message_delta_stop, message_start,
        message_stop, mock_message_response, noop_hook_executor, text_delta, MockApiClient,
        MockOutputStream, MockStreamingApiClient, NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use llm_client::ContentBlock as LlmContentBlock;
    use protocol::ContentBlock;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// The byte-exact reminder text for the `Explanatory` builtin — 1:1 with TS
    /// `wrapInSystemReminder(`${name} output style is active. …`)`
    /// (`messages.ts:3097-3099` + `3805-3810`).
    const EXPLANATORY_REMINDER: &str = "<system-reminder>\nExplanatory output style is active. \
         Remember to follow the specific guidelines for this style.\n</system-reminder>";
    const LEARNING_REMINDER: &str = "<system-reminder>\nLearning output style is active. \
         Remember to follow the specific guidelines for this style.\n</system-reminder>";

    /// Config with a non-default builtin output style active.
    fn config_with_style(style: &str) -> OrchestratorConfig {
        OrchestratorConfig {
            output_style: Some(style.to_string()),
            ..OrchestratorConfig::default()
        }
    }

    /// Concatenated text of a message's text blocks (for substring checks).
    fn text_of(msg: &ConversationMessage) -> String {
        match msg {
            ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } => content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            ConversationMessage::System { content, .. } => content.clone(),
        }
    }

    fn is_reminder(msg: &ConversationMessage, expected: &str) -> bool {
        matches!(msg, ConversationMessage::User { .. }) && text_of(msg) == expected
    }

    /// True when `msg` is the leading `additionalContext` (`# claudeMd` /
    /// `# userEmail` / `# currentDate`) meta message prepended each turn
    /// (R-P1c/R-P1d). With `StaticMemoryProvider::empty()` and no `user_email`
    /// it carries only the always-present `# currentDate` entry.
    fn is_additional_context(msg: &ConversationMessage) -> bool {
        matches!(msg, ConversationMessage::User { .. })
            && text_of(msg).starts_with(
                "<system-reminder>\nAs you answer the user's questions, you can use the following context:",
            )
    }

    // ----- direct unit coverage of the reminder builder -----

    #[test]
    fn builder_emits_byte_exact_explanatory_reminder() {
        let orch = ConversationOrchestrator::new(
            config_with_style("Explanatory"),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        let msg = orch
            .output_style_reminder_message()
            .expect("Explanatory resolves to a reminder");
        assert!(matches!(msg, ConversationMessage::User { .. }));
        assert_eq!(text_of(&msg), EXPLANATORY_REMINDER);
        // Spell out the literal bytes once so a drift in the helper const is caught.
        assert_eq!(
            text_of(&msg),
            "<system-reminder>\nExplanatory output style is active. Remember to follow the specific guidelines for this style.\n</system-reminder>"
        );
    }

    #[test]
    fn builder_emits_byte_exact_learning_reminder() {
        let orch = ConversationOrchestrator::new(
            config_with_style("Learning"),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        assert_eq!(
            text_of(
                &orch
                    .output_style_reminder_message()
                    .expect("Learning resolves")
            ),
            LEARNING_REMINDER
        );
    }

    #[test]
    fn builder_returns_none_for_default_and_unknown_styles() {
        for style in [None, Some("default"), Some(""), Some("Nonexistent")] {
            let cfg = OrchestratorConfig {
                output_style: style.map(str::to_string),
                ..OrchestratorConfig::default()
            };
            let orch = ConversationOrchestrator::new(
                cfg,
                Arc::new(MockApiClient::new(vec![])),
                Arc::new(ToolRegistry::new()),
                noop_hook_executor(),
                Arc::new(NoOpPermissionGate),
                Arc::new(MockOutputStream::new()),
                Arc::new(StaticMemoryProvider::empty()),
                std::env::temp_dir(),
            );
            assert!(
                orch.output_style_reminder_message().is_none(),
                "style {style:?} must not produce a reminder"
            );
        }
    }

    // ----- batched driver (`run_turn` / `execute_one_turn`) -----

    #[tokio::test]
    async fn batched_active_style_appends_transient_reminder_not_persisted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let fs: Arc<dyn traits::FileSystem> = Arc::new(platform_posix::fs::PosixFileSystem::new(
            dir.path().to_path_buf(),
        ));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
            session_path.clone(),
            fs,
        ));

        let resp = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "assistant body".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let api = Arc::new(MockApiClient::new(vec![resp]));
        let orch = ConversationOrchestrator::new(
            config_with_style("Explanatory"),
            api.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            dir.path().to_path_buf(),
        )
        .with_jsonl_writer(writer);

        orch.run_turn("user prompt body").await.expect("turn");

        // OUTGOING snapshot: [additionalContext(meta), user(prompt), reminder] —
        // the leading additional-context meta message (R-P1c/d) prepends the
        // user prompt; the output-style reminder trails it (TS position).
        let outgoing = api.captured_msgs().await;
        assert_eq!(outgoing.len(), 1, "exactly one batched API call");
        let sent = &outgoing[0];
        assert_eq!(
            sent.len(),
            3,
            "additionalContext + prompt + reminder; got {sent:?}"
        );
        assert!(
            is_additional_context(&sent[0]),
            "leading meta; got {:?}",
            sent[0]
        );
        assert_eq!(text_of(&sent[1]), "user prompt body");
        assert!(
            is_reminder(&sent[2], EXPLANATORY_REMINDER),
            "trailing message must be the byte-exact reminder; got {:?}",
            sent[2]
        );

        // STORED history: [user(prompt), assistant] — the reminder was NOT pushed.
        let history = orch.session.lock().await.history.clone();
        assert_eq!(history.len(), 2, "user + assistant only; got {history:?}");
        assert!(
            history
                .iter()
                .all(|m| !is_reminder(m, EXPLANATORY_REMINDER)),
            "the reminder must never enter stored history; got {history:?}"
        );
        assert_eq!(text_of(&history[0]), "user prompt body");
        assert_eq!(text_of(&history[1]), "assistant body");

        // JSONL transcript: user + assistant only, reminder text absent.
        let on_disk = std::fs::read_to_string(&session_path).expect("read jsonl");
        assert!(on_disk.contains("user prompt body"));
        assert!(on_disk.contains("assistant body"));
        assert!(
            !on_disk.contains("output style is active"),
            "the reminder must never be persisted to JSONL; file:\n{on_disk}"
        );
    }

    #[tokio::test]
    async fn batched_default_style_sends_no_reminder() {
        let resp = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "body".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let api = Arc::new(MockApiClient::new(vec![resp]));
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(), // output_style: None
            api.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        orch.run_turn("just the prompt").await.expect("turn");

        let outgoing = api.captured_msgs().await;
        assert_eq!(outgoing.len(), 1);
        // No output-style reminder; the only prepended message is the leading
        // additional-context meta (always present via `# currentDate`).
        assert_eq!(
            outgoing[0].len(),
            2,
            "additionalContext + prompt; got {:?}",
            outgoing[0]
        );
        assert!(
            is_additional_context(&outgoing[0][0]),
            "leading meta; got {:?}",
            outgoing[0][0]
        );
        assert_eq!(text_of(&outgoing[0][1]), "just the prompt");
        assert!(
            !outgoing[0]
                .iter()
                .any(|m| is_reminder(m, EXPLANATORY_REMINDER) || is_reminder(m, LEARNING_REMINDER)),
            "no output-style reminder on the default path; got {:?}",
            outgoing[0]
        );
    }

    // ----- streaming driver (`run_turn_streaming`) -----

    #[tokio::test]
    async fn streaming_active_style_appends_transient_reminder_not_persisted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let fs: Arc<dyn traits::FileSystem> = Arc::new(platform_posix::fs::PosixFileSystem::new(
            dir.path().to_path_buf(),
        ));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
            session_path.clone(),
            fs,
        ));

        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m1", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "streamed body"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]));
        let orch = ConversationOrchestrator::new_with_streaming(
            config_with_style("Learning"),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            dir.path().to_path_buf(),
        )
        .with_jsonl_writer(writer);

        orch.run_turn_streaming("streaming prompt")
            .await
            .expect("streaming turn");

        // OUTGOING snapshot to the stream: [additionalContext(meta), user(prompt),
        // reminder].
        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 1, "exactly one streaming call");
        let sent = &calls[0].messages;
        assert_eq!(
            sent.len(),
            3,
            "additionalContext + prompt + reminder; got {sent:?}"
        );
        assert!(
            is_additional_context(&sent[0]),
            "leading meta; got {:?}",
            sent[0]
        );
        assert_eq!(text_of(&sent[1]), "streaming prompt");
        assert!(
            is_reminder(&sent[2], LEARNING_REMINDER),
            "trailing message must be the byte-exact Learning reminder; got {:?}",
            sent[2]
        );

        // STORED history: reminder absent.
        let history = orch.session.lock().await.history.clone();
        assert!(
            history.iter().all(|m| !is_reminder(m, LEARNING_REMINDER)),
            "the reminder must never enter stored history; got {history:?}"
        );
        assert_eq!(text_of(&history[0]), "streaming prompt");

        // JSONL transcript: reminder text absent.
        let on_disk = std::fs::read_to_string(&session_path).expect("read jsonl");
        assert!(on_disk.contains("streaming prompt"));
        assert!(
            !on_disk.contains("output style is active"),
            "the reminder must never be persisted to JSONL; file:\n{on_disk}"
        );
    }

    #[tokio::test]
    async fn streaming_default_style_sends_no_reminder() {
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
            message_start("m1", "claude-opus-4-7"),
            content_block_start_text(0),
            text_delta(0, "body"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(), // output_style: None
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        orch.run_turn_streaming("only prompt")
            .await
            .expect("streaming turn");

        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 1);
        // No output-style reminder; only the leading additional-context meta
        // (always present via `# currentDate`) prepends the prompt.
        assert_eq!(
            calls[0].messages.len(),
            2,
            "additionalContext + prompt; got {:?}",
            calls[0].messages
        );
        assert!(
            is_additional_context(&calls[0].messages[0]),
            "leading meta; got {:?}",
            calls[0].messages[0]
        );
        assert_eq!(text_of(&calls[0].messages[1]), "only prompt");
    }
}

// ============================================================================
// R-P1c/R-P1d: the leading `additionalContext` (`# claudeMd` / `# userEmail` /
// `# currentDate`) meta message — byte-lock against claude-code `A6n`.
// ============================================================================
#[cfg(test)]
mod additional_context_tests {
    use super::*;
    use crate::prompt::MemoryFile;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use protocol::ContentBlock;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    fn orch_with(
        memory: Arc<StaticMemoryProvider>,
        email: Option<&str>,
    ) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig {
                user_email: email.map(str::to_string),
                ..OrchestratorConfig::default()
            },
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            memory,
            std::env::temp_dir(),
        )
    }

    fn text(msg: &ConversationMessage) -> String {
        match msg {
            ConversationMessage::User { content, .. } => content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect(),
            _ => String::new(),
        }
    }

    #[tokio::test]
    async fn all_three_keys_byte_exact_order_and_wrapper() {
        // claudeMd + userEmail present; currentDate always present. Insertion
        // order (claude-code `pS`): claudeMd, userEmail, currentDate.
        let mem = Arc::new(StaticMemoryProvider::with_files(vec![MemoryFile {
            path: std::path::PathBuf::from("/proj/LINGXI.md"),
            body: "MD BODY".into(),
            is_local_override: false,
            tier: memory::lingxi_md::LingxiMdTier::Project,
            globs: None,
        }]));
        let orch = orch_with(mem, Some("u@example.com"));
        let msg = orch.additional_context_message().await.expect("present");
        // It is a META user message (claude-code `isMeta:!0`).
        assert!(msg.is_meta(), "additionalContext must be isMeta");
        let body = text(&msg);

        // Exact wrapper: opens with the header line, closes with the IMPORTANT
        // line indented by 6 spaces + the closing tag + trailing LF.
        assert!(body.starts_with(
            "<system-reminder>\nAs you answer the user's questions, you can use the following context:\n"
        ));
        assert!(body.ends_with(
            "\n\n      IMPORTANT: this context may or may not be relevant to your tasks. \
You should not respond to this context unless it is highly relevant to your task.\n</system-reminder>\n"
        ));

        // Keys in order, each `# key\nvalue`, joined by `\n`.
        let i_md = body.find("# claudeMd\n").expect("claudeMd key");
        let i_email = body.find("# userEmail\n").expect("userEmail key");
        let i_date = body.find("# currentDate\n").expect("currentDate key");
        assert!(
            i_md < i_email && i_email < i_date,
            "key order claudeMd<userEmail<currentDate"
        );

        // claudeMd value = the assembled memory block (preamble + Contents).
        assert!(body.contains("# claudeMd\nCodebase and user instructions are shown below."));
        assert!(body.contains("Contents of /proj/LINGXI.md"));
        assert!(body.contains("MD BODY"));
        // userEmail value.
        assert!(body.contains("# userEmail\nThe user's email address is u@example.com."));
        // currentDate value (ISO local date).
        let today = crate::prompt::env_meta::current_date_string();
        assert!(body.contains(&format!("# currentDate\nToday's date is {today}.")));
    }

    #[tokio::test]
    async fn omits_lingxi_md_and_email_when_absent_keeps_date() {
        // Empty memory + no email → only `# currentDate` remains.
        let orch = orch_with(Arc::new(StaticMemoryProvider::empty()), None);
        let msg = orch
            .additional_context_message()
            .await
            .expect("date always present");
        let body = text(&msg);
        assert!(!body.contains("# claudeMd"));
        assert!(!body.contains("# userEmail"));
        assert!(body.contains("# currentDate\nToday's date is "));
        // The body between the header and the IMPORTANT line is exactly the one
        // currentDate entry (no stray blank lines from empty entries).
        let today = crate::prompt::env_meta::current_date_string();
        let expected = format!(
            "<system-reminder>\n\
As you answer the user's questions, you can use the following context:\n\
# currentDate\nToday's date is {today}.\n\
\n      IMPORTANT: this context may or may not be relevant to your tasks. \
You should not respond to this context unless it is highly relevant to your task.\n\
</system-reminder>\n"
        );
        assert_eq!(body, expected, "single-key wrapper byte-lock");
    }

    #[tokio::test]
    async fn empty_email_string_is_treated_as_absent() {
        // `user_email: Some("")` (or whitespace) is filtered, matching the
        // `...email&&{userEmail:…}` spread + LingXi's non-empty guard.
        let orch = orch_with(Arc::new(StaticMemoryProvider::empty()), Some("   "));
        let body = text(&orch.additional_context_message().await.expect("date"));
        assert!(!body.contains("# userEmail"));
    }
}

// ============================================================================
// SKILLEXEC.3 (model scope): a tool's `context_modifier` switches the session's
// main-loop model, applied POST-BATCH on BOTH turn drivers (batched `run_turn`
// + streaming `run_turn_streaming`). A tool that returns NO modifier (every
// existing tool + skills WITHOUT a `model:` frontmatter) leaves `session.model`
// untouched — byte-identical, keeping the locked turn-loop/streaming fixtures
// green.
// ============================================================================
#[cfg(test)]
mod skill_model_override_tests {
    use super::*;
    use crate::test_support::{
        content_block_start_text, content_block_start_tool_use, content_block_stop,
        input_json_delta, message_delta_stop, message_start, message_stop, mock_message_response,
        noop_hook_executor, text_delta, MockApiClient, MockOutputStream, MockStreamingApiClient,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use llm_client::ContentBlock as LlmContentBlock;
    use protocol::ToolUseId;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        ContextModifier, DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError,
        ToolStaticContext, ValidationError,
    };

    /// The model the [`ModelSwitchTool`] switches the session to.
    const SWITCHED_MODEL: &str = "claude-opus-4-9-zzz";

    /// A tool that succeeds AND returns a `context_modifier` setting the turn's
    /// `main_loop_model` to [`SWITCHED_MODEL`] — the orchestrator-side twin of a
    /// Skill tool with a `model:` frontmatter. Sets the model directly (the
    /// skill-specific `[1m]`-resolution logic is unit-tested in the skill crate).
    struct ModelSwitchTool;
    #[async_trait]
    impl Tool for ModelSwitchTool {
        fn name(&self) -> &str {
            "ModelSwitch"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(
                || serde_json::json!({ "type": "object", "properties": {} }),
            );
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "model-switch".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            let modifier: ContextModifier = Box::new(|mut ctx: ToolUseContext| {
                ctx.options.main_loop_model = SWITCHED_MODEL.to_string();
                ctx
            });
            Ok(ToolCallResult {
                data: serde_json::json!({
                    "content": "TOOL-RESULT",
                    "model_content": "Launching skill: switcher",
                }),
                model_content: None,
                new_messages: vec![],
                context_modifier: Some(modifier),
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// A tool with NO `context_modifier` (the byte-identical baseline — like
    /// every existing tool).
    struct PlainTool;
    #[async_trait]
    impl Tool for PlainTool {
        fn name(&self) -> &str {
            "Plain"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(
                || serde_json::json!({ "type": "object", "properties": {} }),
            );
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "plain".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: serde_json::json!({ "content": "PLAIN-RESULT" }),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    fn registry_with(tool: Arc<dyn Tool>) -> Arc<ToolRegistry> {
        let mut reg = ToolRegistry::new();
        reg.register_builtin(tool);
        Arc::new(reg)
    }

    // ----- batched driver (`run_turn`) -----

    #[tokio::test]
    async fn batched_skill_model_override_switches_session_model() {
        let tu = ToolUseId::new();
        let resp1 = mock_message_response(
            vec![LlmContentBlock::ToolCall {
                id: tu.to_string(),
                name: "ModelSwitch".into(),
                input: serde_json::json!({}),
            }],
            Some("tool_use"),
        );
        let resp2 = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![resp1, resp2])),
            registry_with(Arc::new(ModelSwitchTool)),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        // Precondition: the session boots on the default model.
        assert_eq!(
            orch.session.lock().await.model,
            crate::config::DEFAULT_MODEL
        );

        orch.run_turn("switch please").await.expect("turn");

        // POST-BATCH the override took effect; the NEXT turn's API call reads it.
        assert_eq!(orch.session.lock().await.model, SWITCHED_MODEL);
    }

    #[tokio::test]
    async fn batched_no_modifier_leaves_session_model_untouched() {
        // Byte-identical guard: a tool with NO context_modifier must not move
        // `session.model`.
        let tu = ToolUseId::new();
        let resp1 = mock_message_response(
            vec![LlmContentBlock::ToolCall {
                id: tu.to_string(),
                name: "Plain".into(),
                input: serde_json::json!({}),
            }],
            Some("tool_use"),
        );
        let resp2 = mock_message_response(
            vec![LlmContentBlock::Text {
                text: "done".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        );
        let orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![resp1, resp2])),
            registry_with(Arc::new(PlainTool)),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.run_turn("no switch").await.expect("turn");
        assert_eq!(
            orch.session.lock().await.model,
            crate::config::DEFAULT_MODEL,
            "no context_modifier → session.model unchanged (byte-identical)"
        );
    }

    // ----- streaming driver (`run_turn_streaming`) -----

    #[tokio::test]
    async fn streaming_skill_model_override_switches_session_and_next_call() {
        let tu = ToolUseId::new();
        let turn1 = vec![
            message_start("m1", crate::config::DEFAULT_MODEL),
            content_block_start_tool_use(0, tu.clone(), "ModelSwitch"),
            input_json_delta(0, "{}"),
            content_block_stop(0),
            message_delta_stop("tool_use"),
            message_stop(),
        ];
        let turn2 = vec![
            message_start("m2", SWITCHED_MODEL),
            content_block_start_text(0),
            text_delta(0, "done"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ];
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![turn1, turn2]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            registry_with(Arc::new(ModelSwitchTool)),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        orch.run_turn_streaming("switch please")
            .await
            .expect("streaming turn");

        // session.model switched POST-BATCH...
        assert_eq!(orch.session.lock().await.model, SWITCHED_MODEL);
        // ...and the NEXT (second) streaming call used the switched model, while
        // the first used the boot default.
        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 2, "two streaming calls (tool turn + end turn)");
        assert_eq!(calls[0].model, crate::config::DEFAULT_MODEL);
        assert_eq!(
            calls[1].model, SWITCHED_MODEL,
            "the NEXT API call must use the switched model"
        );
    }

    #[tokio::test]
    async fn streaming_no_modifier_leaves_session_model_untouched() {
        // Byte-identical guard on the streaming path.
        let tu = ToolUseId::new();
        let turn1 = vec![
            message_start("m1", crate::config::DEFAULT_MODEL),
            content_block_start_tool_use(0, tu.clone(), "Plain"),
            input_json_delta(0, "{}"),
            content_block_stop(0),
            message_delta_stop("tool_use"),
            message_stop(),
        ];
        let turn2 = vec![
            message_start("m2", crate::config::DEFAULT_MODEL),
            content_block_start_text(0),
            text_delta(0, "done"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ];
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![turn1, turn2]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            registry_with(Arc::new(PlainTool)),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        orch.run_turn_streaming("no switch")
            .await
            .expect("streaming turn");
        assert_eq!(
            orch.session.lock().await.model,
            crate::config::DEFAULT_MODEL,
            "no context_modifier → session.model unchanged on the streaming path"
        );
        let calls = streaming.captured_calls().await;
        assert!(
            calls
                .iter()
                .all(|c| c.model == crate::config::DEFAULT_MODEL),
            "every streaming call used the unchanged default model"
        );
    }

    /// Regression guard for the streaming-profile gap: when `session.model_profile`
    /// is set (e.g. `"github-copilot"`) the INITIAL streaming `.stream()` call
    /// must carry the profile, not `None`.  Mirrors the batched
    /// `build_request_sets_profile_when_provided` test in `provider_adapter.rs`.
    #[tokio::test]
    async fn streaming_threads_model_profile_to_stream_call() {
        let turn = vec![
            message_start("m1", crate::config::DEFAULT_MODEL),
            content_block_start_text(0),
            text_delta(0, "hello"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ];
        let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![turn]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            streaming.clone(),
            registry_with(Arc::new(PlainTool)),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        // Set model_profile on the session directly (mirrors what switch_model does).
        {
            let mut s = orch.session.lock().await;
            s.model_profile = Some("github-copilot".to_string());
        }

        orch.run_turn_streaming("hello")
            .await
            .expect("streaming turn");

        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 1, "one streaming call");
        assert_eq!(
            calls[0].profile.as_deref(),
            Some("github-copilot"),
            "streaming path must thread session.model_profile through to the stream() call"
        );
    }
}

// ============================================================================
// Task 7: mid-stream 529 → non-streaming fallback tests
//
// Parity: `claude.ts:2469-2594`, `withRetry.ts:141,186`
// Env gate: `LINGXI_DISABLE_NONSTREAMING_FALLBACK` (claude.ts:2470)
// Error copy: `errors.ts:166` REPEATED_529_ERROR_MESSAGE = "Repeated 529 Overloaded errors"
// ============================================================================
#[cfg(test)]
mod task7_midstream_fallback_tests {
    use super::*;
    use crate::test_support::{
        content_block_start_text, message_start, mock_message_response, noop_hook_executor,
        text_delta, MockApiClient, MockOutputStream, MockStreamingApiClient, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use llm_client::ContentBlock as LlmContentBlock;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    const DISABLE_FALLBACK_ENV: &str = "LINGXI_DISABLE_NONSTREAMING_FALLBACK";

    /// Serializes the two midstream tests that read/write `DISABLE_FALLBACK_ENV`.
    ///
    /// `std::env::set_var` / `remove_var` are not thread-safe when other threads
    /// read the same variable concurrently.  Tokio runs `#[tokio::test]` functions
    /// in the same process and may schedule them in parallel; holding this lock for
    /// the duration of each test makes the pair race-free without any new crate dep.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Build a one-ContentBlockStart-then-Err(Overloaded) stream: the first
    /// event is yielded successfully (proving partial events arrived), then the
    /// stream errors with `LlmError::Overloaded`.
    fn one_event_then_overloaded() -> Vec<Result<llm_client::LlmEvent, llm_client::LlmError>> {
        vec![
            Ok(message_start("m1", "claude-opus-4-7")),
            Ok(content_block_start_text(0)),
            Ok(text_delta(0, "partial")),
            Err(llm_client::LlmError::Overloaded { repeated: false }),
        ]
    }

    /// Build an `end_turn` non-streaming response for the fallback.
    fn fallback_response() -> llm_client::LlmResponse {
        mock_message_response(
            vec![LlmContentBlock::Text {
                text: "fallback body".into(),
                cache_control: None,
            }],
            Some("end_turn"),
        )
    }

    /// Task 7 Step 1 (a)(b)(c)(d):
    /// A stream that yields one ContentBlockStart then Err(Overloaded):
    /// (a) the stream is NOT replayed (streaming_api called exactly once),
    /// (b) a fresh non-streaming `messages_create_seeded` is issued,
    /// (c) the seed is 1 (streaming 529 counts toward the budget),
    /// (d) the final response is built from the non-streaming reply only.
    ///
    /// Parity: claude.ts:2551-2594, withRetry.ts:186
    /// (`initialConsecutive529Errors: is529Error(streamingError) ? 1 : 0`)
    #[tokio::test]
    async fn midstream_529_triggers_nonstreaming_fallback() {
        // Serialize with the sibling test that also reads/writes DISABLE_FALLBACK_ENV.
        // `set_var`/`remove_var` are not thread-safe; the (tokio) mutex makes the
        // pair race-free without a new crate dependency, and its guard is safe to
        // hold across the .await points below.
        let _guard = ENV_LOCK.lock().await;
        // Ensure fallback is ENABLED for this test.
        std::env::remove_var(DISABLE_FALLBACK_ENV);

        let streaming = Arc::new(MockStreamingApiClient::with_fallible_turns(vec![
            one_event_then_overloaded(),
        ]));
        let api = Arc::new(MockApiClient::new(vec![fallback_response()]));
        let output = Arc::new(MockOutputStream::new());
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            api.clone(),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            output.clone(),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let outcome = orch
            .run_turn_streaming("hello")
            .await
            .expect("turn must succeed via fallback");

        // (a) Stream was called exactly once — NOT replayed.
        let stream_calls = streaming.captured_calls().await;
        assert_eq!(
            stream_calls.len(),
            1,
            "(a) stream must be called exactly once"
        );

        // (b) A fresh non-streaming messages_create_seeded was called.
        let seeds = api.captured_seeds().await;
        assert_eq!(
            seeds.len(),
            1,
            "(b) messages_create_seeded must be called exactly once"
        );

        // (c) The seed is 1 (the streaming 529 counts toward the consecutive 529 budget).
        assert_eq!(
            seeds[0], 1,
            "(c) seed must be 1 for a streaming Overloaded error"
        );

        // (d) The final turn outcome is built from the non-streaming reply only.
        // The output must contain "fallback body" (from the non-streaming response),
        // NOT just "partial" (the partial stream events are discarded).
        let events = output.snapshot().await;
        let texts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                traits::OutputEvent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            texts.contains(&"fallback body"),
            "(d) output must contain the fallback body; texts={texts:?}"
        );
        // The turn must have ended with end_turn (not an error).
        assert!(
            matches!(outcome, ConversationOutcome::EndTurn { .. }),
            "outcome must be EndTurn after non-streaming fallback; got {outcome:?}"
        );

        // M1 (Task 7 review): the PERSISTED assistant message must contain ONLY the
        // fallback body, not the partial streaming fragments.  TS yields deltas live
        // (claude.ts:2210 `yield m` fires inside the for-await loop at each
        // `content_block_stop`), so partial output reaching callers before the fallback
        // is parity — but the final persisted turn must reflect ONLY the fallback result.
        let session_arc = orch.session();
        let session_guard = session_arc.lock().await;
        let final_assistant = session_guard
            .history
            .iter()
            .filter_map(|msg| match msg {
                ConversationMessage::Assistant { content, .. } => Some(content),
                _ => None,
            })
            .last()
            .expect("session must contain at least one assistant message");
        let persisted_texts: Vec<&str> = final_assistant
            .iter()
            .filter_map(|blk| match blk {
                protocol::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            persisted_texts,
            vec!["fallback body"],
            "M1: final persisted assistant message must contain ONLY the fallback body; \
             got {persisted_texts:?}"
        );
    }

    /// Task 7 Step 1 (twin with LINGXI_DISABLE_NONSTREAMING_FALLBACK=1):
    /// When the env gate is set, the streaming error propagates instead of
    /// triggering the non-streaming fallback.
    ///
    /// Parity: claude.ts:2476-2501 (disableFallback branch).
    #[tokio::test]
    async fn midstream_529_propagates_when_fallback_disabled() {
        // Serialize with the sibling test that also reads/writes DISABLE_FALLBACK_ENV.
        // `set_var`/`remove_var` are not thread-safe; the (tokio) mutex makes the
        // pair race-free without a new crate dependency, and its guard is safe to
        // hold across the .await points below.
        let _guard = ENV_LOCK.lock().await;
        // Set the disable flag for this test.
        std::env::set_var(DISABLE_FALLBACK_ENV, "1");

        let streaming = Arc::new(MockStreamingApiClient::with_fallible_turns(vec![
            one_event_then_overloaded(),
        ]));
        let api = Arc::new(MockApiClient::new(vec![]));
        let orch = ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            api.clone(),
            streaming.clone(),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let result = orch.run_turn_streaming("hello").await;

        // Restore env BEFORE assertions to avoid leaking even on panic.
        std::env::remove_var(DISABLE_FALLBACK_ENV);

        // The error MUST propagate — no fallback.
        assert!(
            result.is_err(),
            "error must propagate when fallback is disabled"
        );
        // No non-streaming call was made.
        assert!(
            api.captured_seeds().await.is_empty(),
            "messages_create_seeded must NOT be called when fallback is disabled"
        );
        // The stream was called exactly once.
        assert_eq!(
            streaming.captured_calls().await.len(),
            1,
            "stream was called exactly once"
        );
    }

    /// `is_env_truthy` covers the exact semantics of TS `isEnvTruthy`
    /// (`utils/envUtils.ts:32`): truthy ONLY for the whitelist
    /// `1`/`true`/`yes`/`on`, case-insensitive and trimmed; everything
    /// else (including `no`/`off`/`2`/`enabled`/arbitrary strings) is
    /// falsy.
    #[test]
    fn is_env_truthy_matches_ts_semantics() {
        // Not set → not truthy.
        assert!(!is_env_truthy(None));
        // Empty → not truthy.
        assert!(!is_env_truthy(Some("")));
        // "false" → not truthy.
        assert!(!is_env_truthy(Some("false")));
        // "0" → not truthy.
        assert!(!is_env_truthy(Some("0")));
        // Whitelist members → truthy.
        assert!(is_env_truthy(Some("1")));
        assert!(is_env_truthy(Some("true")));
        assert!(is_env_truthy(Some("yes")));
        assert!(is_env_truthy(Some("on")));
        // Case-insensitive + trimmed.
        assert!(is_env_truthy(Some("ON")));
        assert!(is_env_truthy(Some(" TRUE ")));
        assert!(is_env_truthy(Some("Yes")));
        // Non-whitelist values → NOT truthy (strict whitelist).
        assert!(!is_env_truthy(Some("no")));
        assert!(!is_env_truthy(Some("off")));
        assert!(!is_env_truthy(Some("2")));
        assert!(!is_env_truthy(Some("enabled")));
        assert!(!is_env_truthy(Some("disable")));
        assert!(!is_env_truthy(Some("random")));
    }
}

/// Task 6 (llm-client future-work batch 5): the terminal-429 limits-copy
/// re-map (`enrich_rate_limited_error`). The integration test
/// (`tests/rate_limit_terminal_429_test.rs`) drives the batched `ApiCall`
/// wrapper end-to-end; these cover the `Streaming` wrapper and the
/// pass-through arms directly.
#[cfg(test)]
mod enrich_rate_limited_error_tests {
    use super::*;

    fn rate_limited() -> LlmError {
        LlmError::RateLimited {
            retry_after: None,
            scope: None,
        }
    }

    /// A streaming connect-phase 429 (the wrapper `try_run_turn_streaming`
    /// produces) re-maps onto the composed copy too.
    #[test]
    fn streaming_429_with_copy_maps_to_rate_limit_rejected() {
        let err = enrich_rate_limited_error(
            OrchestratorError::Streaming(rate_limited()),
            Some("You've hit your weekly limit · resets 3pm".to_string()),
        );
        assert!(
            matches!(err, OrchestratorError::RateLimitRejected { .. }),
            "got {err:?}"
        );
        assert_eq!(err.to_string(), "You've hit your weekly limit · resets 3pm");
    }

    /// No composed copy → both wrappers pass through untouched.
    #[test]
    fn rate_limited_without_copy_passes_through() {
        let api = enrich_rate_limited_error(OrchestratorError::ApiCall(rate_limited()), None);
        assert!(matches!(
            api,
            OrchestratorError::ApiCall(LlmError::RateLimited { .. })
        ));
        let stream = enrich_rate_limited_error(OrchestratorError::Streaming(rate_limited()), None);
        assert!(matches!(
            stream,
            OrchestratorError::Streaming(LlmError::RateLimited { .. })
        ));
    }

    /// A non-429 error never consults the copy — even when one is cached.
    #[test]
    fn non_rate_limited_ignores_copy() {
        let err = enrich_rate_limited_error(
            OrchestratorError::StreamEndedWithoutStop,
            Some("You've hit your weekly limit".to_string()),
        );
        assert!(matches!(err, OrchestratorError::StreamEndedWithoutStop));
    }
}

// ============================================================================
// SKILLLIST.1: per-turn, transient `skill_listing` reminder.
//
// Proves the orchestrator method: returns the rendered `<system-reminder>` when
// a provider is wired AND the `Skill` tool is present this turn; returns `None`
// when no provider is wired, or when the `Skill` tool is absent (so we never
// advertise skills the model can't invoke). The byte-level formatting is covered
// in `prompt::skill_listing::tests`.
// ============================================================================
#[cfg(test)]
mod skill_listing_reminder_tests {
    use super::*;
    use crate::prompt::skill_listing::{SkillListingEntry, SkillListingProvider};
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// Static skill-listing fixture.
    struct FixtureSkills(Vec<SkillListingEntry>);
    #[async_trait]
    impl SkillListingProvider for FixtureSkills {
        async fn skill_entries(&self) -> Vec<SkillListingEntry> {
            self.0.clone()
        }
    }

    /// Minimal tool whose only meaningful behavior is its name — used to put a
    /// `Skill`-named tool (or not) into the registry for the gate test.
    struct NamedTool(&'static str);
    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(
                || serde_json::json!({ "type": "object", "properties": {} }),
            );
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other { reason: "t".into() },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            String::new()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: serde_json::json!({}),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    fn orch_with(
        tools: ToolRegistry,
        provider: Option<Arc<dyn SkillListingProvider>>,
    ) -> ConversationOrchestrator {
        let api = Arc::new(MockApiClient::new(vec![]));
        let mut orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api,
            Arc::new(tools),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        if let Some(p) = provider {
            orch = orch.with_skill_listing(p);
        }
        orch
    }

    fn fixture() -> Arc<dyn SkillListingProvider> {
        Arc::new(FixtureSkills(vec![SkillListingEntry {
            name: "debug".into(),
            description: "Debug a failing test".into(),
            when_to_use: None,
            is_bundled: false,
        }]))
    }

    #[tokio::test]
    async fn reminder_present_when_provider_and_skill_tool_wired() {
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(NamedTool("Skill")));
        let orch = orch_with(reg, Some(fixture()));
        let msg = orch
            .skill_listing_reminder_message()
            .await
            .expect("reminder present");
        let text = msg.text_content();
        assert!(text.starts_with("<system-reminder>"), "got: {text}");
        assert!(text.contains("The following skills are available for use with the Skill tool:"));
        assert!(text.contains("- debug: Debug a failing test"));
    }

    #[tokio::test]
    async fn no_reminder_when_skill_tool_absent() {
        // Provider wired, but the Skill tool is not in the registry this turn.
        let orch = orch_with(ToolRegistry::new(), Some(fixture()));
        assert!(orch.skill_listing_reminder_message().await.is_none());
    }

    #[tokio::test]
    async fn no_reminder_when_provider_absent() {
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(NamedTool("Skill")));
        let orch = orch_with(reg, None);
        assert!(orch.skill_listing_reminder_message().await.is_none());
    }

    // ── SKILLLIST.1 delta (sent-tracking) ──────────────────────────────────

    /// A skill provider whose entry set can change between turns (shared
    /// `Arc<Mutex<…>>`), to exercise the "new skill appears later" delta path.
    struct MutableSkills(std::sync::Arc<std::sync::Mutex<Vec<SkillListingEntry>>>);
    #[async_trait]
    impl SkillListingProvider for MutableSkills {
        async fn skill_entries(&self) -> Vec<SkillListingEntry> {
            self.0.lock().unwrap().clone()
        }
    }

    fn skill(name: &str) -> SkillListingEntry {
        SkillListingEntry {
            name: name.into(),
            description: format!("desc for {name}"),
            when_to_use: None,
            is_bundled: false,
        }
    }

    #[tokio::test]
    async fn skill_listing_delta_turn0_full_then_none_when_no_new() {
        // Turn 0 emits the FULL listing; a later turn with the SAME skills (no
        // new names) emits nothing (None).
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(NamedTool("Skill")));
        let orch = orch_with(
            reg,
            Some(Arc::new(FixtureSkills(vec![skill("alpha"), skill("beta")]))),
        );

        // Turn 0: both skills present.
        let t0 = orch
            .skill_listing_reminder_message()
            .await
            .expect("turn-0 full listing");
        let t0 = t0.text_content();
        assert!(t0.contains("- alpha:"), "turn-0 missing alpha: {t0}");
        assert!(t0.contains("- beta:"), "turn-0 missing beta: {t0}");

        // Turn 1: no NEW skills since both were already sent → None.
        assert!(
            orch.skill_listing_reminder_message().await.is_none(),
            "turn-1 must emit nothing when no new skill appeared"
        );
    }

    #[tokio::test]
    async fn skill_listing_delta_emits_only_new_skill_on_later_turn() {
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(NamedTool("Skill")));
        let shared = std::sync::Arc::new(std::sync::Mutex::new(vec![skill("alpha")]));
        let orch = orch_with(reg, Some(Arc::new(MutableSkills(shared.clone()))));

        // Turn 0: only `alpha`.
        let t0 = orch
            .skill_listing_reminder_message()
            .await
            .expect("turn-0")
            .text_content();
        assert!(t0.contains("- alpha:"));
        assert!(!t0.contains("- gamma:"));

        // A new skill `gamma` appears.
        shared.lock().unwrap().push(skill("gamma"));

        // Turn 1: ONLY the new `gamma` is emitted (alpha was already sent).
        let t1 = orch
            .skill_listing_reminder_message()
            .await
            .expect("turn-1 new-only")
            .text_content();
        assert!(
            t1.contains("- gamma:"),
            "turn-1 must contain the new skill: {t1}"
        );
        assert!(
            !t1.contains("- alpha:"),
            "turn-1 must NOT re-emit the already-sent skill: {t1}"
        );
    }

    struct OnceAsyncResponses(std::sync::Mutex<Vec<String>>);
    #[async_trait::async_trait]
    impl crate::prompt::async_hook_response::AsyncHookResponseProvider for OnceAsyncResponses {
        async fn take_pending_responses(&self) -> Vec<String> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    #[tokio::test]
    async fn async_hook_response_reminder_folds_in_then_drains_once() {
        let reg = ToolRegistry::new();
        let orch = orch_with(reg, None).with_async_hook_responses(Arc::new(OnceAsyncResponses(
            std::sync::Mutex::new(vec!["ran background lints: clean".to_string()]),
        )));
        // Turn 0: the completed background-hook response is folded in, wrapped.
        let t0 = orch
            .async_hook_response_reminder_message()
            .await
            .expect("turn-0 async hook response")
            .text_content();
        assert!(t0.contains("<system-reminder>"), "must be wrapped: {t0}");
        assert!(
            t0.contains("ran background lints: clean"),
            "must carry the hook's system_message: {t0}"
        );
        // Turn 1: consume-once — the delivered response must NOT re-appear.
        assert!(
            orch.async_hook_response_reminder_message().await.is_none(),
            "a delivered async-hook response must be drained, not repeated"
        );
    }

    #[tokio::test]
    async fn async_hook_response_reminder_none_without_provider() {
        let reg = ToolRegistry::new();
        let orch = orch_with(reg, None);
        assert!(
            orch.async_hook_response_reminder_message().await.is_none(),
            "no provider wired ⇒ strict no-op"
        );
    }

    // ── hook-bg-fields: Stop / SubagentStop background_tasks + session_crons ──

    /// A [`StopHookSnapshotProvider`] that returns fixed fixtures, so the
    /// orchestrator's `populate_stop_hook_snapshot` wiring is testable without a
    /// live registry / cron file.
    struct FixtureStopSnapshot {
        tasks: Vec<hooks::HookBackgroundTask>,
        crons: Vec<hooks::HookSessionCron>,
    }
    #[async_trait::async_trait]
    impl crate::stop_hook_snapshot::StopHookSnapshotProvider for FixtureStopSnapshot {
        async fn background_tasks(&self) -> Vec<hooks::HookBackgroundTask> {
            self.tasks.clone()
        }
        async fn session_crons(&self) -> Vec<hooks::HookSessionCron> {
            self.crons.clone()
        }
    }

    #[tokio::test]
    async fn populate_stop_hook_snapshot_stamps_both_arrays_when_wired() {
        use hooks::{HookBackgroundTask, HookSessionCron};
        let reg = ToolRegistry::new();
        let orch = orch_with(reg, None).with_stop_hook_snapshot(Arc::new(FixtureStopSnapshot {
            tasks: vec![HookBackgroundTask {
                id: "b1".into(),
                r#type: "shell".into(),
                status: "running".into(),
                description: "build".into(),
                command: Some("cargo build".into()),
                agent_type: None,
                server: None,
                tool: None,
                name: None,
            }],
            crons: vec![HookSessionCron {
                id: "c1".into(),
                schedule: "* * * * *".into(),
                recurring: true,
                prompt: "hi".into(),
            }],
        }));
        // A Stop-firing context (the only path that populates the snapshot)
        // carries BOTH arrays, populated, after the snapshot helper runs.
        let mut ctx = orch.lifecycle_hook_ctx(false).await;
        // Before population the lifecycle ctx leaves both fields None (the
        // default — a non-Stop lifecycle hook omits the keys).
        assert!(ctx.background_tasks.is_none());
        assert!(ctx.session_crons.is_none());
        orch.populate_stop_hook_snapshot(&mut ctx).await;
        let bg = ctx.background_tasks.expect("background_tasks populated");
        assert_eq!(bg.len(), 1);
        assert_eq!(bg[0].id, "b1");
        assert_eq!(bg[0].r#type, "shell");
        let crons = ctx.session_crons.expect("session_crons populated");
        assert_eq!(crons.len(), 1);
        assert_eq!(crons[0].id, "c1");
        assert!(crons[0].recurring);
    }

    #[tokio::test]
    async fn populate_stop_hook_snapshot_noop_without_provider() {
        let reg = ToolRegistry::new();
        let orch = orch_with(reg, None);
        let mut ctx = orch.lifecycle_hook_ctx(false).await;
        orch.populate_stop_hook_snapshot(&mut ctx).await;
        // No provider wired ⇒ both fields stay None ⇒ the executor omits the
        // keys (claude `m = undefined`), byte-identical to the pre-feature build.
        assert!(ctx.background_tasks.is_none());
        assert!(ctx.session_crons.is_none());
    }

    // ── T35: `task-notification` reminder folds in then drains once ──────────

    /// A [`TaskNotificationProvider`] that hands back its fixture exactly once
    /// (the second drain returns empty), mirroring the registry's
    /// take-mark-evict semantics so the consume-once invariant is testable
    /// without a real registry.
    struct OnceTaskNotifications(std::sync::Mutex<Vec<traits::task_registry::TaskNotification>>);
    #[async_trait::async_trait]
    impl crate::prompt::task_notification::TaskNotificationProvider for OnceTaskNotifications {
        async fn take_pending_task_notifications(
            &self,
        ) -> Vec<traits::task_registry::TaskNotification> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    #[tokio::test]
    async fn task_notification_reminder_folds_in_then_drains_once() {
        let reg = ToolRegistry::new();
        // One terminal `local_bash` task — the minimal faithful surface.
        let bash = traits::task_registry::TaskNotification {
            task_id: "b12345678".into(),
            task_type: "local_bash".into(),
            status: "completed".into(),
            description: "run tests".into(),
            tool_use_id: None,
            output_path: Some("/tmp/tasks/b12345678.output".into()),
            exit_code: Some(0),
            error: None,
            result: None,
            usage: None,
        };
        let orch = orch_with(reg, None).with_task_notifications(Arc::new(OnceTaskNotifications(
            std::sync::Mutex::new(vec![bash]),
        )));
        // Turn 0: the terminal task is folded in as a byte-faithful
        // `<task-notification>` inside one `<system-reminder>`.
        let t0 = orch
            .task_notification_reminder_message()
            .await
            .expect("turn-0 task notification")
            .text_content();
        assert_eq!(
            t0,
            "<system-reminder>\n\
<task-notification>\n\
<task-id>b12345678</task-id>\n\
<output-file>/tmp/tasks/b12345678.output</output-file>\n\
<status>completed</status>\n\
<summary>Background command \"run tests\" completed (exit code 0)</summary>\n\
</task-notification>\n\
</system-reminder>"
        );
        // Turn 1: consume-once — the notified+evicted task must NOT re-appear.
        assert!(
            orch.task_notification_reminder_message().await.is_none(),
            "a delivered task notification must be drained, not repeated"
        );
    }

    #[tokio::test]
    async fn task_notification_reminder_none_without_provider() {
        let reg = ToolRegistry::new();
        let orch = orch_with(reg, None);
        assert!(
            orch.task_notification_reminder_message().await.is_none(),
            "no provider wired ⇒ strict no-op"
        );
    }
}

// ── `agent_listing_delta`: per-turn, transient agent catalog reminder ─────────
//
// Proves [`ConversationOrchestrator::agent_listing_reminder_message`]:
// - GATE OFF (default): always `None`, and the inline `AgentTool` prompt is
//   unchanged (asserted in `tool-agent` — here we just confirm the orchestrator
//   side stays silent).
// - GATE ON (`LINGXI_AGENT_LIST_IN_MESSAGES=1`, guarded by a process-wide
//   lock): turn-0 full listing + "Available agent types for the Agent tool:"
//   header; a later turn with no new types ⇒ `None`; a newly-added type ⇒ a
//   delta with the "New agent types are now available…" header and ONLY the new
//   line. Also gated on the `Agent` tool's presence + a wired catalog.
#[cfg(test)]
mod agent_listing_reminder_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use agent::{AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy};
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// `LINGXI_AGENT_LIST_IN_MESSAGES` is process-global; serialize the
    /// gate-sensitive tests (every one removes/sets the var under this lock).
    static AGENT_LIST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Minimal tool whose only meaningful behavior is its name — used to put an
    /// `Agent`-named tool (or not) into the registry for the gate test.
    struct NamedTool(&'static str);
    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(
                || serde_json::json!({ "type": "object", "properties": {} }),
            );
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other { reason: "t".into() },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            String::new()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: serde_json::json!({}),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    fn agent_def(agent_type: &str, when_to_use: &str, tools: AgentToolPolicy) -> AgentDefinition {
        AgentDefinition {
            agent_type: agent_type.into(),
            when_to_use: when_to_use.into(),
            tools,
            max_turns: 1,
            model: AgentModel::Inherit,
            permission_mode: AgentPermissionMode::Bubble,
            source: AgentSource::BuiltIn,
            base_dir: "/tmp".into(),
            system_prompt: None,
            mcp_servers: vec![],
            frontmatter_hooks: vec![],
            icon: None,
            allowed_tools: vec![],
            worktree_requirement: None,
            disallowed_tools: vec![],
            skills: vec![],
            required_mcp_servers: vec![],
            background: false,
            isolation: None,
            memory: None,
            effort: None,
            initial_prompt: None,
            color: None,
        }
    }

    /// Build an orchestrator with the given tools + optional agent catalog.
    fn orch_with(
        tools: ToolRegistry,
        catalog: Option<Arc<tokio::sync::RwLock<Vec<AgentDefinition>>>>,
    ) -> ConversationOrchestrator {
        let api = Arc::new(MockApiClient::new(vec![]));
        let mut orch = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api,
            Arc::new(tools),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        if let Some(c) = catalog {
            orch = orch.with_agent_catalog(c);
        }
        orch
    }

    fn reg_with_agent_tool() -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register_builtin(Arc::new(NamedTool("Agent")));
        reg
    }

    #[tokio::test]
    async fn gate_explicit_off_is_none() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // v2.1.193 default is ON (catalog externalized); the LEGACY inline path
        // (explicit `=false`) keeps the catalog in the description, so the
        // orchestrator emits no reminder.
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "false");

        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "general-purpose",
            "anything",
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog));
        let got = orch.agent_listing_reminder_message().await;
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        assert!(
            got.is_none(),
            "explicit gate OFF ⇒ no reminder (inline path)"
        );
    }

    #[tokio::test]
    async fn gate_on_no_catalog_still_announces_builtins() {
        // Binary `aLe` builds the delta from `activeAgents` (built-ins +
        // user/project), gating ONLY on the Agent tool's presence — NOT on a
        // wired DISK catalog. So a session with the gate ON, the Agent tool
        // present, and NO disk catalog still announces the BUILT-IN agents
        // (e.g. general-purpose). (Previously this early-returned `None`,
        // suppressing built-ins under the gate — a divergence from `aLe`.)
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        let orch = orch_with(reg_with_agent_tool(), None);
        let got = orch.agent_listing_reminder_message().await;
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        let text = got
            .expect("built-ins must be announced even with no disk catalog")
            .text_content();
        assert!(text.starts_with("<system-reminder>"), "got: {text}");
        assert!(
            text.contains("Available agent types for the Agent tool:"),
            "turn-0 initial header expected; got: {text}"
        );
        assert!(
            text.contains("- general-purpose:"),
            "built-ins must be listed with no disk catalog; got: {text}"
        );
    }

    #[tokio::test]
    async fn gate_on_but_agent_tool_absent_is_none() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "general-purpose",
            "anything",
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
        )]));
        // Empty registry — the Agent tool is not present this turn.
        let orch = orch_with(ToolRegistry::new(), Some(catalog));
        let got = orch.agent_listing_reminder_message().await;
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        assert!(got.is_none(), "Agent tool absent ⇒ no reminder");
    }

    #[tokio::test]
    async fn gate_on_turn0_full_listing_with_initial_header() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        // Catalog supplies a custom type; built-ins are merged in too.
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "custom-agent",
            "a project agent",
            AgentToolPolicy::Explicit(vec!["Read".into()]),
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog));

        let msg = orch.agent_listing_reminder_message().await;
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        let text = msg.expect("turn-0 reminder present").text_content();

        assert!(text.starts_with("<system-reminder>"), "got: {text}");
        assert!(text.ends_with("</system-reminder>"), "got: {text}");
        assert!(
            text.contains("Available agent types for the Agent tool:"),
            "turn-0 must use the is_initial header; got: {text}"
        );
        // formatAgentLine for the catalog entry.
        assert!(
            text.contains("- custom-agent: a project agent (Tools: Read)"),
            "missing catalog line; got: {text}"
        );
        // Built-ins are merged in (e.g. general-purpose).
        assert!(
            text.contains("- general-purpose:"),
            "built-ins must be merged into the listing; got: {text}"
        );
    }

    #[tokio::test]
    async fn gate_on_later_turn_no_new_types_is_none() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "custom-agent",
            "a project agent",
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog));

        // Turn 0 emits the full listing.
        let t0 = orch.agent_listing_reminder_message().await;
        assert!(t0.is_some(), "turn-0 must emit");
        // Turn 1 with the same catalog ⇒ nothing new ⇒ None.
        let t1 = orch.agent_listing_reminder_message().await;
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        assert!(t1.is_none(), "no new types ⇒ no reminder");
    }

    #[tokio::test]
    async fn gate_on_newly_added_type_emits_delta_with_new_header_only() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "alpha-agent",
            "the alpha agent",
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog.clone()));

        // Turn 0: full listing (contains alpha-agent + built-ins).
        let t0 = orch
            .agent_listing_reminder_message()
            .await
            .expect("turn-0")
            .text_content();
        assert!(t0.contains("- alpha-agent:"));
        assert!(!t0.contains("- gamma-agent:"));

        // A brand-new agent type appears in the catalog.
        catalog.write().await.push(agent_def(
            "gamma-agent",
            "the gamma agent",
            AgentToolPolicy::Explicit(vec!["Read".into(), "Edit".into()]),
        ));

        // Turn 1: ONLY the new type, with the "New agent types…" header.
        let t1 = orch
            .agent_listing_reminder_message()
            .await
            .expect("turn-1 delta")
            .text_content();
        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");

        assert!(
            t1.contains("New agent types are now available for the Agent tool:"),
            "delta must use the non-initial header; got: {t1}"
        );
        assert!(
            !t1.contains("Available agent types for the Agent tool:"),
            "delta must NOT use the is_initial header; got: {t1}"
        );
        assert!(
            t1.contains("- gamma-agent: the gamma agent (Tools: Read, Edit)"),
            "delta must contain the new agent line; got: {t1}"
        );
        assert!(
            !t1.contains("- alpha-agent:"),
            "delta must NOT re-emit an already-announced type; got: {t1}"
        );
    }
}

// ── §F: per-turn, transient `conditional_rules` reminder ──────────────────────
//
// Mirrors the `skill_listing_reminder_tests` template: a `StaticMemoryProvider`
// fixture supplies conditional (`paths:`-gated) `MemoryFile`s, the touched-file
// set is seeded directly into `read_file_state`, and
// `conditional_rules_reminder_message` is asserted to inject the matching rule
// once (with sent-tracking dedup) and skip non-matching / already-sent rules.
#[cfg(test)]
mod new_diagnostics_reminder_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    struct MockDiag(Option<String>);
    #[async_trait::async_trait]
    impl traits::NewDiagnosticsSource for MockDiag {
        async fn take_new_diagnostics_block(&self) -> Option<String> {
            self.0.clone()
        }
    }

    fn orch_with_diag(
        source: Option<Arc<dyn traits::NewDiagnosticsSource>>,
    ) -> ConversationOrchestrator {
        let o = ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            std::path::PathBuf::from("/work"),
        );
        match source {
            Some(s) => o.with_new_diagnostics_source(s),
            None => o,
        }
    }

    #[tokio::test]
    async fn injects_block_when_source_has_new_diagnostics() {
        let block = "<new-diagnostics>The following new diagnostic issues were detected:\n\nx.rs:\n  \u{2718} [Line 1:1] boom</new-diagnostics>";
        let orch = orch_with_diag(Some(Arc::new(MockDiag(Some(block.to_string())))));
        let msg = orch
            .new_diagnostics_reminder_message()
            .await
            .expect("a block is injected");
        assert_eq!(msg.text_content(), block);
    }

    #[tokio::test]
    async fn no_reminder_without_source_or_when_empty() {
        // No source wired (the common no-LSP case).
        assert!(orch_with_diag(None)
            .new_diagnostics_reminder_message()
            .await
            .is_none());
        // Source wired but nothing new.
        assert!(orch_with_diag(Some(Arc::new(MockDiag(None))))
            .new_diagnostics_reminder_message()
            .await
            .is_none());
    }
}

#[cfg(test)]
mod conditional_rules_reminder_tests {
    use super::*;
    use crate::prompt::MemoryFile;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use memory::lingxi_md::LingxiMdTier;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// A Project-tier conditional rule living at `<cwd>/.lingxi/rules/{name}.md`
    /// (so its derived base dir is `<cwd>`) carrying the given `paths:` globs.
    fn project_rule(cwd: &std::path::Path, name: &str, globs: &[&str]) -> MemoryFile {
        MemoryFile {
            path: cwd.join(".lingxi").join("rules").join(format!("{name}.md")),
            body: format!("BODY OF {name}"),
            is_local_override: false,
            tier: LingxiMdTier::Project,
            globs: Some(globs.iter().map(|s| (*s).to_string()).collect()),
        }
    }

    /// Build an orchestrator whose memory provider returns `rules` and whose cwd
    /// is `cwd`. Conditional rules need no Skill tool / skill provider.
    fn orch_with_rules(cwd: PathBuf, rules: Vec<MemoryFile>) -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(rules)),
            cwd,
        )
    }

    async fn push_touched(orch: &ConversationOrchestrator, path: &std::path::Path) {
        orch.read_file_state.lock().await.push(path.to_path_buf());
    }

    #[tokio::test]
    async fn matching_touched_file_injects_rule() {
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(cwd.clone(), vec![project_rule(&cwd, "scoped", &["src"])]);
        // A touched file under `src/` matches `paths: src/**`.
        push_touched(&orch, &cwd.join("src/x.rs")).await;

        let msg = orch
            .conditional_rules_reminder_message()
            .await
            .expect("matching rule must be injected");
        let text = msg.text_content();
        assert!(text.starts_with("<system-reminder>"), "got: {text}");
        assert!(
            text.contains("Contents of /work/repo/.lingxi/rules/scoped.md:"),
            "got: {text}"
        );
        assert!(text.contains("BODY OF scoped"), "got: {text}");
        // It is a BARE nested-memory render — no eager-block preamble.
        assert!(!text.contains("Codebase and user instructions"));
    }

    #[tokio::test]
    async fn non_matching_touched_file_does_not_inject() {
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(cwd.clone(), vec![project_rule(&cwd, "scoped", &["src"])]);
        // `docs/y.md` does NOT match `paths: src/**`.
        push_touched(&orch, &cwd.join("docs/y.md")).await;
        assert!(
            orch.conditional_rules_reminder_message().await.is_none(),
            "a non-matching touched file must not activate the rule"
        );
    }

    #[tokio::test]
    async fn rule_injected_once_then_not_reinjected() {
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(cwd.clone(), vec![project_rule(&cwd, "scoped", &["src"])]);
        push_touched(&orch, &cwd.join("src/x.rs")).await;

        // Turn 0: injected.
        assert!(
            orch.conditional_rules_reminder_message().await.is_some(),
            "first activation must inject"
        );
        // Turn 1: the same file is still touched, but the rule was already sent →
        // not re-injected (sent-tracking dedup).
        assert!(
            orch.conditional_rules_reminder_message().await.is_none(),
            "an already-sent rule must not be re-injected"
        );
    }

    #[tokio::test]
    async fn no_touched_file_yields_none() {
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(cwd.clone(), vec![project_rule(&cwd, "scoped", &["src"])]);
        // read_file_state empty → no rule can match.
        assert!(orch.conditional_rules_reminder_message().await.is_none());
    }

    #[tokio::test]
    async fn no_conditional_rules_yields_none() {
        // Provider returns an unconditional file only (globs == None): the
        // conditional cache is empty, so the reminder is a strict no-op even with
        // a touched file present.
        let cwd = PathBuf::from("/work/repo");
        let unconditional = MemoryFile {
            path: cwd.join("LINGXI.md"),
            body: "always".into(),
            is_local_override: false,
            tier: LingxiMdTier::Project,
            globs: None,
        };
        let orch = orch_with_rules(cwd.clone(), vec![unconditional]);
        push_touched(&orch, &cwd.join("src/x.rs")).await;
        assert!(orch.conditional_rules_reminder_message().await.is_none());
    }

    #[tokio::test]
    async fn newly_matching_rule_injected_on_later_turn() {
        // Two rules; only one matches initially. After a second file is touched,
        // the second rule activates and is injected (delta across turns).
        let cwd = PathBuf::from("/work/repo");
        let orch = orch_with_rules(
            cwd.clone(),
            vec![
                project_rule(&cwd, "src-rule", &["src"]),
                project_rule(&cwd, "docs-rule", &["docs"]),
            ],
        );
        push_touched(&orch, &cwd.join("src/a.rs")).await;
        let t0 = orch
            .conditional_rules_reminder_message()
            .await
            .expect("src-rule active")
            .text_content();
        assert!(t0.contains("src-rule.md"));
        assert!(!t0.contains("docs-rule.md"));

        // Now touch a docs file → docs-rule newly activates; src-rule already sent.
        push_touched(&orch, &cwd.join("docs/readme.md")).await;
        let t1 = orch
            .conditional_rules_reminder_message()
            .await
            .expect("docs-rule newly active")
            .text_content();
        assert!(t1.contains("docs-rule.md"), "got: {t1}");
        assert!(
            !t1.contains("src-rule.md"),
            "already-sent src-rule must not re-inject: {t1}"
        );
    }
}

// P0.1: `relevant_memory_reminder_message` SURFACING tests.
//
// A `MemoryPrefetch::with_fixed_result` (seeded surfaced set) is wired via
// `with_memory_prefetch`; `start_memory_prefetch` arms the per-turn handle and
// the reminder is asserted to render the `relevant_memories` shape, dedup against
// both `surfaced_memory_paths` (across turns) and `read_file_state` (the SHARED
// P3.2 nested-channel guard), and stay a strict no-op when no prefetch is wired.
#[cfg(test)]
mod relevant_memory_reminder_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use memory::surfacing::SurfacedMemory;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::SystemTime;
    use tool_api::registry::ToolRegistry;

    /// A runtime that actually RUNS the spawned future on the current tokio
    /// runtime, so the prefetch's one-shot send fires (the shared
    /// `noop_hook_executor` `UnusedRuntime` errors instead, which would leave the
    /// channel unresolved). Cancel/sleep are no-ops — the prefetch task is
    /// instantaneous.
    struct InlineRuntime;
    #[async_trait]
    impl traits::RuntimeSpawner for InlineRuntime {
        async fn spawn(
            &self,
            name: &str,
            task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            tokio::spawn(task);
            Ok(traits::BackgroundTaskHandle {
                task_name: name.to_string(),
                task_id: 0,
            })
        }
        async fn sleep(&self, _d: std::time::Duration) {}
        async fn cancel(
            &self,
            _h: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    fn mem(path: &str, content: &str, age_days: u64) -> SurfacedMemory {
        SurfacedMemory {
            path: PathBuf::from(path),
            content: content.into(),
            age_days,
            mtime: SystemTime::UNIX_EPOCH,
        }
    }

    /// Build an orchestrator with NO prefetch wired (surfacing inert).
    fn orch_bare() -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        )
    }

    #[tokio::test]
    async fn maybe_extract_session_memory_is_noop_without_handle() {
        // The inert default: no session-memory handle wired (and no cache slot)
        // ⇒ a strict no-op (no panic, nothing spawned), so the locked fixtures
        // stay byte-identical. The enabled path's extract+write is covered by
        // `memory::session_memory` tests; the composition-root wiring is gated
        // behind `LINGXI_SESSION_MEMORY` (default off).
        let orch = orch_bare();
        assert!(orch.session_memory.is_none());
        orch.maybe_extract_session_memory().await;
        assert!(orch.session_memory.is_none());
    }

    /// Build an orchestrator whose prefetch resolves to `seed`.
    fn orch_with_seed(seed: Vec<SurfacedMemory>) -> ConversationOrchestrator {
        let runtime: Arc<dyn traits::RuntimeSpawner> = Arc::new(InlineRuntime);
        let prefetch = Arc::new(memory::prefetch::MemoryPrefetch::with_fixed_result(
            runtime, seed,
        ));
        orch_bare().with_memory_prefetch(prefetch)
    }

    #[tokio::test]
    async fn no_prefetch_wired_yields_none() {
        let orch = orch_bare();
        // Without arming, and with no prefetch, the reminder is a strict no-op.
        orch.start_memory_prefetch().await;
        assert!(orch.relevant_memory_reminder_message().await.is_none());
        assert!(!orch.has_memory_prefetch());
    }

    #[tokio::test]
    async fn empty_prefetch_result_yields_none() {
        let orch = orch_with_seed(vec![]);
        orch.start_memory_prefetch().await;
        assert!(orch.relevant_memory_reminder_message().await.is_none());
    }

    #[tokio::test]
    async fn not_armed_yields_none() {
        // A wired prefetch that was never armed this turn (slot empty) ⇒ None.
        let orch = orch_with_seed(vec![mem("/m/a.md", "A", 0)]);
        assert!(orch.relevant_memory_reminder_message().await.is_none());
    }

    #[tokio::test]
    async fn seeded_prefetch_renders_relevant_memories_block() {
        let orch = orch_with_seed(vec![mem("/m/a.md", "USE FD NOT FIND", 0)]);
        orch.start_memory_prefetch().await;
        let msg = orch
            .relevant_memory_reminder_message()
            .await
            .expect("seeded prefetch must surface");
        let text = msg.text_content();
        assert!(text.starts_with("<system-reminder>\n"), "got: {text}");
        assert!(
            text.contains(
                "Retrieved for possible relevance \u{2014} use only if it actually applies"
            ),
            "idx-0 preamble missing: {text}"
        );
        assert!(
            text.contains("Memory: /m/a.md:\n\nUSE FD NOT FIND"),
            "got: {text}"
        );
        assert!(text.ends_with("\n</system-reminder>"), "got: {text}");
    }

    #[tokio::test]
    async fn surfaced_once_then_not_reinjected_across_turns() {
        let orch = orch_with_seed(vec![mem("/m/a.md", "A", 0)]);
        // Turn 0: surfaced.
        orch.start_memory_prefetch().await;
        assert!(
            orch.relevant_memory_reminder_message().await.is_some(),
            "first surfacing must inject"
        );
        // Turn 1: same memory ⇒ already in surfaced_memory_paths ⇒ no re-inject.
        orch.start_memory_prefetch().await;
        assert!(
            orch.relevant_memory_reminder_message().await.is_none(),
            "an already-surfaced memory must not be re-injected"
        );
    }

    #[tokio::test]
    async fn shared_dedup_skips_memory_already_in_read_file_state() {
        // A memory whose path was already loaded as a nested/conditional (P3.2)
        // attachment / tool read (present in read_file_state) must NOT be
        // double-injected via the surfacing channel.
        let path = PathBuf::from("/m/a.md");
        let orch = orch_with_seed(vec![mem("/m/a.md", "A", 0)]);
        orch.read_file_state.lock().await.push(path.clone());
        orch.start_memory_prefetch().await;
        assert!(
            orch.relevant_memory_reminder_message().await.is_none(),
            "a path already in read_file_state must not be surfaced"
        );
    }

    #[tokio::test]
    async fn partial_dedup_surfaces_only_fresh_memories() {
        // Two memories; one already read. Only the fresh one surfaces, and it
        // carries the idx-0 preamble (it is the first RENDERED memory).
        let orch = orch_with_seed(vec![
            mem("/m/seen.md", "SEEN", 0),
            mem("/m/new.md", "NEW", 0),
        ]);
        orch.read_file_state
            .lock()
            .await
            .push(PathBuf::from("/m/seen.md"));
        orch.start_memory_prefetch().await;
        let text = orch
            .relevant_memory_reminder_message()
            .await
            .expect("the fresh memory must surface")
            .text_content();
        assert!(text.contains("Memory: /m/new.md:\n\nNEW"), "got: {text}");
        assert!(
            !text.contains("/m/seen.md"),
            "already-read memory leaked: {text}"
        );
    }
}

// ── EXPERIMENTAL_SKILL_SEARCH skill-discovery surfacing (default OFF) ─────────
#[cfg(test)]
mod skill_discovery_reminder_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use protocol::ContentBlock;
    use skill_api::DiscoveredSkill;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// A runtime that actually RUNS the spawned future so the one-shot resolves.
    struct InlineRuntime;
    #[async_trait]
    impl traits::RuntimeSpawner for InlineRuntime {
        async fn spawn(
            &self,
            name: &str,
            task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            tokio::spawn(task);
            Ok(traits::BackgroundTaskHandle {
                task_name: name.to_string(),
                task_id: 0,
            })
        }
        async fn sleep(&self, _d: std::time::Duration) {}
        async fn cancel(
            &self,
            _h: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    fn skill(name: &str, description: &str) -> DiscoveredSkill {
        DiscoveredSkill {
            name: name.into(),
            description: description.into(),
            short_id: None,
        }
    }

    /// Build an orchestrator with NO skill prefetch wired (channel inert).
    fn orch_bare() -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        )
    }

    /// Build an orchestrator whose skill prefetch resolves to `seed`.
    fn orch_with_seed(seed: Vec<DiscoveredSkill>) -> ConversationOrchestrator {
        let runtime: Arc<dyn traits::RuntimeSpawner> = Arc::new(InlineRuntime);
        let prefetch = Arc::new(skill_api::SkillDiscoveryPrefetch::with_fixed_result(
            runtime, seed,
        ));
        orch_bare().with_skill_discovery_prefetch(prefetch)
    }

    /// Push an assistant message that requested an `Edit` so `find_write_pivot`
    /// reports a write pivot and the prefetch fires.
    async fn push_write_pivot(orch: &ConversationOrchestrator) {
        let msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id: protocol::ToolUseId::new(),
                name: "Edit".into(),
                input: serde_json::json!({}),
                provider_id: None,
            }],
            stop_reason: None,
        };
        orch.session.lock().await.history.push(msg);
    }

    // (D.8) Flag OFF (no prefetch wired) = zero change: both calls are strict
    // no-ops and `has_skill_discovery_prefetch()` is false.
    #[tokio::test]
    async fn no_prefetch_wired_is_inert() {
        let orch = orch_bare();
        push_write_pivot(&orch).await;
        orch.start_skill_discovery_prefetch().await;
        assert!(orch.skill_discovery_reminder_message().await.is_none());
        assert!(!orch.has_skill_discovery_prefetch());
    }

    // (D.9) Flag ON = byte-exact attachment injected.
    #[tokio::test]
    async fn seeded_prefetch_renders_skill_discovery_block() {
        let orch = orch_with_seed(vec![
            skill("git-commit", "Commit staged changes"),
            skill("rebase", "Interactive rebase helper"),
        ]);
        assert!(orch.has_skill_discovery_prefetch());
        push_write_pivot(&orch).await;
        orch.start_skill_discovery_prefetch().await;
        let text = orch
            .skill_discovery_reminder_message()
            .await
            .expect("seeded prefetch must surface")
            .text_content();
        assert_eq!(
            text,
            "<system-reminder>\n\
             Skills relevant to your task:\n\n\
             - git-commit: Commit staged changes\n\
             - rebase: Interactive rebase helper\n\n\
             These skills encode project-specific conventions. \
             Invoke via Skill(\"<name>\") for complete instructions.\n\
             </system-reminder>"
        );
    }

    // (D.4 integration) Non-write iteration (no write-pivot tool) ⇒ inert even
    // with a seeded prefetch.
    #[tokio::test]
    async fn non_write_pivot_is_inert() {
        let orch = orch_with_seed(vec![skill("a", "da")]);
        // No assistant tool-use in history ⇒ find_write_pivot == false.
        orch.start_skill_discovery_prefetch().await;
        assert!(
            orch.skill_discovery_reminder_message().await.is_none(),
            "non-write iteration must surface nothing"
        );
    }

    // Empty result ⇒ None.
    #[tokio::test]
    async fn empty_result_yields_none() {
        let orch = orch_with_seed(vec![]);
        push_write_pivot(&orch).await;
        orch.start_skill_discovery_prefetch().await;
        assert!(orch.skill_discovery_reminder_message().await.is_none());
    }

    // Not armed (slot empty) ⇒ None.
    #[tokio::test]
    async fn not_armed_yields_none() {
        let orch = orch_with_seed(vec![skill("a", "da")]);
        assert!(orch.skill_discovery_reminder_message().await.is_none());
    }

    // (D.10) Dedup across turns: same skill armed turn N and N+1 ⇒ injects once.
    #[tokio::test]
    async fn surfaced_once_then_not_reinjected_across_turns() {
        let orch = orch_with_seed(vec![skill("a", "da")]);
        push_write_pivot(&orch).await;
        // Turn 0: surfaced.
        orch.start_skill_discovery_prefetch().await;
        assert!(
            orch.skill_discovery_reminder_message().await.is_some(),
            "first surfacing must inject"
        );
        // Turn 1: same skill ⇒ already in surfaced_skill_names ⇒ no re-inject.
        orch.start_skill_discovery_prefetch().await;
        assert!(
            orch.skill_discovery_reminder_message().await.is_none(),
            "an already-surfaced skill must not be re-injected"
        );
    }

    // Partial dedup: only the fresh skill surfaces on turn N+1.
    #[tokio::test]
    async fn partial_dedup_surfaces_only_fresh_skills() {
        let orch = orch_with_seed(vec![skill("seen", "ds")]);
        push_write_pivot(&orch).await;
        orch.start_skill_discovery_prefetch().await;
        assert!(orch.skill_discovery_reminder_message().await.is_some());

        // Re-seed the SAME prefetch slot is not possible (fixed_result is fixed);
        // instead assert the surfaced_skill_names set recorded "seen".
        assert!(orch.surfaced_skill_names.lock().await.contains("seen"));
    }
}

// ── Finding #80: refusal → fallback-model swap (maybe_swap_to_refusal_fallback) ──
#[cfg(test)]
mod refusal_fallback_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// Build an orchestrator whose `refusal_fallback_model` is `cfg_fallback`,
    /// returning the orchestrator + a clone of its `MockOutputStream` (shares the
    /// same event buffer) so the test can inspect emitted warnings.
    fn orch_with_refusal_fallback(
        cfg_fallback: Option<&str>,
    ) -> (ConversationOrchestrator, MockOutputStream) {
        let out = MockOutputStream::new();
        let cfg = OrchestratorConfig {
            refusal_fallback_model: cfg_fallback.map(str::to_string),
            ..OrchestratorConfig::default()
        };
        let orch = ConversationOrchestrator::new(
            cfg,
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(out.clone()),
            Arc::new(StaticMemoryProvider::with_files(vec![])),
            PathBuf::from("/work/repo"),
        );
        (orch, out)
    }

    #[tokio::test]
    async fn no_fallback_configured_is_a_strict_noop() {
        let (orch, out) = orch_with_refusal_fallback(None);
        let before = orch.session.lock().await.model.clone();
        assert!(
            !orch.maybe_swap_to_refusal_fallback().await,
            "no fallback → false"
        );
        assert_eq!(
            orch.session.lock().await.model,
            before,
            "model must NOT change"
        );
        assert!(out.text_events().await.is_empty(), "no warning emitted");
        assert!(
            !orch
                .refusal_fallback_latched
                .load(std::sync::atomic::Ordering::SeqCst),
            "latch must stay unset when nothing was configured"
        );
    }

    #[tokio::test]
    async fn swaps_once_then_latches() {
        let (orch, _out) = orch_with_refusal_fallback(Some("claude-sonnet-4-6"));
        // First refusal → swap.
        assert!(
            orch.maybe_swap_to_refusal_fallback().await,
            "first call swaps"
        );
        assert_eq!(
            orch.session.lock().await.model,
            "claude-sonnet-4-6",
            "session model must be swapped to the fallback"
        );
        assert!(
            orch.session.lock().await.model_profile.is_none(),
            "fallback model carries no provider profile"
        );
        // Second refusal → latched, no re-swap, terminal behavior preserved.
        assert!(
            !orch.maybe_swap_to_refusal_fallback().await,
            "second call is latched (no re-swap)"
        );
        assert_eq!(
            orch.session.lock().await.model,
            "claude-sonnet-4-6",
            "model unchanged on the second (latched) call"
        );
    }

    #[tokio::test]
    async fn emits_byte_exact_warning_on_swap() {
        let (orch, out) = orch_with_refusal_fallback(Some("claude-sonnet-4-6"));
        assert!(orch.maybe_swap_to_refusal_fallback().await);
        let texts = out.text_events().await;
        assert_eq!(texts.len(), 1, "exactly one warning emitted");
        // Byte-exact reproduction of 2.1.206 `VPn` for category == "other":
        // the generic `$7m`/`hmi` prefix, then "Switched to {Mf(fallback)}" — the
        // fallback's MARKETING NAME ("Sonnet 4.6"), not the raw id — then `bxr`.
        assert_eq!(
            texts[0],
            "This model's safeguards flagged this message. \
This sometimes happens with safe, normal conversations. Switched to Sonnet 4.6. \
Send feedback with /feedback or learn more: https://support.claude.com/en/articles/15363606"
        );
    }
}

// ── `persist_message_to_jsonl_with_parent`: explicit parentUuid override ──────
//
// Proves that the streaming executor can parent each tool-result user message to
// the assistant message that REQUESTED the tool (TS `sourceToolAssistantUUID`),
// rather than the linear `last_jsonl_uuid` chain, by calling
// `persist_message_to_jsonl_with_parent(msg, Some(assistant_uuid))`.
//
// Also proves the `None` path (default chain) is byte-identical to the old
// `persist_message_to_jsonl` behaviour.
#[cfg(test)]
mod persist_with_parent_tests {
    use super::*;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use platform_posix::fs::PosixFileSystem;
    use session::jsonl::schema::JsonlMessage;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// Build an orchestrator wired with a `JsonlWriter` backed by `path`.
    fn orch_with_writer(
        dir: &std::path::Path,
        path: std::path::PathBuf,
    ) -> ConversationOrchestrator {
        let fs: Arc<dyn traits::FileSystem> = Arc::new(PosixFileSystem::new(dir.to_path_buf()));
        let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(path, fs));
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            dir.to_path_buf(),
        )
        .with_jsonl_writer(writer)
    }

    /// Read all JSONL lines back from disk and deserialize.
    fn read_jsonl(path: &std::path::Path) -> Vec<JsonlMessage> {
        let raw = std::fs::read_to_string(path).expect("read jsonl");
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<JsonlMessage>(l).expect("deserialize jsonl line"))
            .collect()
    }

    // ── test 1: explicit parent_override ─────────────────────────────────────

    #[tokio::test]
    async fn tool_result_parents_to_explicit_assistant_uuid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        // Persist an assistant message first (linear chain — no override).
        let asst_msg = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "I will call a tool".into(),
            }],
            stop_reason: Some("tool_use".into()),
        };
        orch.persist_message_to_jsonl(&asst_msg).await;

        // Capture the assistant line's uuid from disk.
        let lines_after_asst = read_jsonl(&session_path);
        assert_eq!(
            lines_after_asst.len(),
            1,
            "expected 1 line (the assistant message)"
        );
        let assistant_uuid = lines_after_asst[0].uuid.clone();

        // Persist a tool-result user message via the override variant, passing
        // the assistant's uuid explicitly — simulates streaming executor parenting.
        let tool_result_msg =
            ConversationMessage::user(protocol::MessageId::new(), "tool result body".into());
        orch.persist_message_to_jsonl_with_parent(&tool_result_msg, Some(assistant_uuid.clone()))
            .await;

        // Read back both lines.
        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 2, "expected 2 lines (assistant + tool_result)");
        let tool_result_line = &lines[1];

        // THE KEY ASSERTION: the tool-result line's parentUuid must equal the
        // assistant's uuid, NOT the prior last_jsonl_uuid (which also happens to
        // be the assistant uuid here, but the next test distinguishes them).
        assert_eq!(
            tool_result_line.parent_uuid.as_deref(),
            Some(assistant_uuid.as_str()),
            "tool_result parentUuid must equal the explicit assistant uuid override"
        );
    }

    // ── test 2: override bypasses last_jsonl_uuid ─────────────────────────────
    //
    // Three messages: user → assistant → tool_result(override=user_uuid).
    // Without the override, the tool_result would parent to the assistant.
    // With the override it must parent to the user uuid instead, proving the
    // override takes effect independent of what `last_jsonl_uuid` holds.

    #[tokio::test]
    async fn override_bypasses_last_jsonl_uuid_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        // 1. Persist a user message (no override).
        let user_msg = ConversationMessage::user(protocol::MessageId::new(), "user prompt".into());
        orch.persist_message_to_jsonl(&user_msg).await;
        let lines = read_jsonl(&session_path);
        let user_uuid = lines[0].uuid.clone();

        // 2. Persist an assistant message (no override → chains off user).
        let asst_msg = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "ok calling tool".into(),
            }],
            stop_reason: Some("tool_use".into()),
        };
        orch.persist_message_to_jsonl(&asst_msg).await;
        let lines = read_jsonl(&session_path);
        assert_eq!(lines[1].parent_uuid.as_deref(), Some(user_uuid.as_str()));
        let _asst_uuid = lines[1].uuid.clone();

        // 3. Persist a tool-result user message with an EXPLICIT override pointing
        //    back to the user_uuid (unusual, but proves the override wins over
        //    last_jsonl_uuid which currently holds the assistant uuid).
        let tool_result_msg =
            ConversationMessage::user(protocol::MessageId::new(), "tool result".into());
        orch.persist_message_to_jsonl_with_parent(&tool_result_msg, Some(user_uuid.clone()))
            .await;

        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 3, "expected 3 lines");
        assert_eq!(
            lines[2].parent_uuid.as_deref(),
            Some(user_uuid.as_str()),
            "override must win over last_jsonl_uuid (which holds the assistant uuid)"
        );
    }

    // ── test 3: None path advances last_jsonl_uuid (regression) ──────────────
    //
    // Proves `persist_message_to_jsonl_with_parent(msg, None)` is byte-identical
    // to the old `persist_message_to_jsonl`: two messages with None form a
    // monotonic chain where msg2.parentUuid == msg1.uuid.

    #[tokio::test]
    async fn none_override_chains_off_last_jsonl_uuid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        let msg1 = ConversationMessage::user(protocol::MessageId::new(), "first message".into());
        orch.persist_message_to_jsonl_with_parent(&msg1, None).await;

        let msg2 = ConversationMessage::user(protocol::MessageId::new(), "second message".into());
        orch.persist_message_to_jsonl_with_parent(&msg2, None).await;

        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 2, "expected 2 JSONL lines");
        // First entry: root of chain → parent_uuid is None.
        assert_eq!(
            lines[0].parent_uuid, None,
            "first entry must have no parent"
        );
        // Second entry: must chain off the first.
        assert_eq!(
            lines[1].parent_uuid.as_deref(),
            Some(lines[0].uuid.as_str()),
            "second entry parentUuid must equal first entry uuid (linear chain)"
        );
    }

    // ── test 4: last_jsonl_uuid advances after override ──────────────────────
    //
    // After an overridden persist, `last_jsonl_uuid` is still advanced to the
    // newly-persisted line's uuid. A subsequent non-overridden line must chain
    // off the overridden line (not off whatever the override pointed to).

    #[tokio::test]
    async fn last_jsonl_uuid_advances_after_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        // 1. First message (no override) — root.
        let msg1 = ConversationMessage::user(protocol::MessageId::new(), "root".into());
        orch.persist_message_to_jsonl(&msg1).await;
        let lines = read_jsonl(&session_path);
        let root_uuid = lines[0].uuid.clone();

        // 2. Overridden message pointing back to root — simulates a tool result.
        let msg2 = ConversationMessage::user(protocol::MessageId::new(), "overridden".into());
        orch.persist_message_to_jsonl_with_parent(&msg2, Some(root_uuid.clone()))
            .await;
        let lines = read_jsonl(&session_path);
        let overridden_uuid = lines[1].uuid.clone();
        // Verify the override took effect.
        assert_eq!(
            lines[1].parent_uuid.as_deref(),
            Some(root_uuid.as_str()),
            "overridden line must parent to root, not to itself"
        );

        // 3. Third message (no override) — must chain off msg2 (the overridden line),
        //    not off msg1 (root). This confirms last_jsonl_uuid was advanced.
        let msg3 = ConversationMessage::user(protocol::MessageId::new(), "subsequent".into());
        orch.persist_message_to_jsonl(&msg3).await;
        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 3, "expected 3 JSONL lines");
        assert_eq!(
            lines[2].parent_uuid.as_deref(),
            Some(overridden_uuid.as_str()),
            "subsequent non-overridden line must chain off the overridden line"
        );
    }

    #[tokio::test]
    async fn assistant_envelope_carries_full_betamessage_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let orch = orch_with_writer(dir.path(), dir.path().join("s.jsonl"));
        let msg = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text { text: "hi".into() }],
            stop_reason: Some("end_turn".into()),
        };
        let usage = serde_json::json!({ "input_tokens": 5, "output_tokens": 3 });

        // Real path (model + usage supplied) → full BetaMessage envelope, in the
        // claude-code / golden-fixture key order.
        let jmsg = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            Some("inner-abc"),
            Some("claude-opus-4-8"),
            Some(&usage),
            Some("req_test123"),
            None,
        );
        // The real-response path stamps the top-level `requestId` (via `extra`).
        assert_eq!(
            jmsg.extra.get("requestId").and_then(|v| v.as_str()),
            Some("req_test123"),
            "real assistant line carries the top-level requestId"
        );
        let inner = jmsg.message.as_object().expect("inner is an object");
        let keys: Vec<&str> = inner.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "id",
                "type",
                "role",
                "content",
                "model",
                "stop_reason",
                "stop_sequence",
                "usage"
            ],
            "BetaMessage envelope key order"
        );
        assert_eq!(inner["id"], serde_json::json!("inner-abc"));
        assert_eq!(inner["type"], serde_json::json!("message"));
        assert_eq!(inner["role"], serde_json::json!("assistant"));
        assert_eq!(inner["model"], serde_json::json!("claude-opus-4-8"));
        assert_eq!(inner["stop_reason"], serde_json::json!("end_turn"));
        assert_eq!(inner["stop_sequence"], serde_json::Value::Null);
        assert_eq!(inner["usage"], usage);

        // Synthetic path (no model/usage) → the synthetic BetaMessage envelope
        // (baseCreateAssistantMessage `QBl` → createAssistantAPIErrorMessage `tc`,
        // binary @205978440): model "<synthetic>", `usage` OMITTED, `stop_reason`
        // hardcoded "stop_sequence" (NOT the message's own "end_turn"), with
        // container/stop_details/context_management = null.
        let plain = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            Some("inner-abc"),
            None,
            None,
            None,
            None,
        );
        // No request_id supplied → no top-level `requestId` (the synthetic case).
        assert!(
            !plain.extra.contains_key("requestId"),
            "synthetic line omits requestId"
        );
        let pinner = plain.message.as_object().unwrap();
        let pkeys: Vec<&str> = pinner.keys().map(String::as_str).collect();
        assert_eq!(
            pkeys,
            vec![
                "id",
                "container",
                "model",
                "role",
                "stop_details",
                "stop_reason",
                "stop_sequence",
                "type",
                "content",
                "context_management"
            ],
            "synthetic BetaMessage envelope key order"
        );
        assert_eq!(pinner["id"], serde_json::json!("inner-abc"));
        assert_eq!(pinner["model"], serde_json::json!("<synthetic>"));
        assert_eq!(pinner["container"], serde_json::Value::Null);
        assert_eq!(pinner["role"], serde_json::json!("assistant"));
        assert_eq!(pinner["stop_details"], serde_json::Value::Null);
        // Hardcoded "stop_sequence", NOT the message's own "end_turn".
        assert_eq!(pinner["stop_reason"], serde_json::json!("stop_sequence"));
        assert_eq!(pinner["stop_sequence"], serde_json::json!(""));
        assert_eq!(pinner["type"], serde_json::json!("message"));
        assert_eq!(pinner["context_management"], serde_json::Value::Null);
        // `usage` is omitted (tc calls QBl without a usage arg).
        assert!(
            !pinner.contains_key("usage"),
            "synthetic envelope must omit usage"
        );
        assert_eq!(pinner["content"][0]["type"], serde_json::json!("text"));
        assert_eq!(pinner["content"][0]["text"], serde_json::json!("hi"));
    }

    #[tokio::test]
    async fn synthetic_api_error_envelope_stamps_top_level_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let orch = orch_with_writer(dir.path(), dir.path().join("s.jsonl"));
        let msg = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "API Error: boom".into(),
            }],
            stop_reason: Some("model_error".into()),
        };

        // 1. No-category builder (top-level `model_error` catch / malformed
        //    terminal): `isApiErrorMessage:true`, `error`/`apiErrorStatus` OMITTED,
        //    inner `stop_reason` stays `"stop_sequence"`.
        let bare = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&ApiErrorEnvelope::default()),
        );
        assert_eq!(
            bare.extra.get("isApiErrorMessage"),
            Some(&serde_json::Value::Bool(true)),
            "isApiErrorMessage is always stamped"
        );
        assert!(!bare.extra.contains_key("error"), "no error category");
        assert!(
            !bare.extra.contains_key("apiErrorStatus"),
            "no apiErrorStatus"
        );
        assert_eq!(
            bare.message["stop_reason"],
            serde_json::json!("stop_sequence"),
            "no-override keeps the synthetic stop_sequence"
        );

        // 2. `max_output_tokens` category (max_tokens / context-window cap).
        let cap = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&ApiErrorEnvelope {
                error: Some("max_output_tokens"),
                api_error_status: None,
                inner_stop_reason: None,
            }),
        );
        assert_eq!(
            cap.extra.get("error").and_then(|v| v.as_str()),
            Some("max_output_tokens")
        );
        assert_eq!(
            cap.extra.get("isApiErrorMessage"),
            Some(&serde_json::Value::Bool(true))
        );

        // 3. Refusal: `error:"invalid_request"` + inner `stop_reason:"refusal"`.
        let refusal = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&ApiErrorEnvelope {
                error: Some("invalid_request"),
                api_error_status: None,
                inner_stop_reason: Some("refusal"),
            }),
        );
        assert_eq!(
            refusal.extra.get("error").and_then(|v| v.as_str()),
            Some("invalid_request")
        );
        assert_eq!(
            refusal.message["stop_reason"],
            serde_json::json!("refusal"),
            "refusal overrides the inner stop_reason"
        );

        // 4. With an HTTP status → `apiErrorStatus` is a JSON number.
        let with_status = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&ApiErrorEnvelope {
                error: Some("rate_limit"),
                api_error_status: Some(429),
                inner_stop_reason: None,
            }),
        );
        assert_eq!(
            with_status.extra.get("apiErrorStatus"),
            Some(&serde_json::Value::Number(429.into()))
        );

        // 5. A normal (non-api-error) assistant line stamps NOTHING.
        let normal = orch.to_jsonl_message(&msg, "sess", None, None, None, None);
        assert!(!normal.extra.contains_key("isApiErrorMessage"));
        assert!(!normal.extra.contains_key("error"));
    }

    // ── per-request api-error classifier (`Flp`/`KNn`) ────────────────────────

    /// `classify_api_error` maps each typed [`LlmError`] semantic variant to the
    /// canonical (category, status) pair recovered from the 2.1.195 `Flp`/`KNn`
    /// classifier + the on-disk transcript aggregate. Status is OMITTED (`None`)
    /// where the port has no confident canonical HTTP status (mirrors claude
    /// omitting `apiErrorStatus` when the error is not an `APIError`-with-status).
    #[test]
    fn classify_api_error_maps_llm_variants_to_category_and_status() {
        use llm_client::LlmError;
        let cases: Vec<(LlmError, Option<&'static str>, Option<u16>)> = vec![
            (
                LlmError::RateLimited {
                    retry_after: None,
                    scope: None,
                },
                Some("rate_limit"),
                Some(429),
            ),
            // On-disk 529 lines tag `server_error` (NOT the `YNn` statusline
            // `"overloaded"`).
            (
                LlmError::Overloaded { repeated: false },
                Some("server_error"),
                Some(529),
            ),
            (
                LlmError::Authentication,
                Some("authentication_failed"),
                Some(401),
            ),
            (
                LlmError::PermissionDenied,
                Some("authentication_failed"),
                Some(403),
            ),
            // Billing is an Error-message match in `Flp`, not a status branch.
            (LlmError::QuotaExceeded, Some("billing_error"), None),
            // PTL/context-window: `invalid_request` with NO status.
            (
                LlmError::ContextOverflow { token_gap: 12 },
                Some("invalid_request"),
                None,
            ),
            (
                LlmError::InvalidRequest {
                    message: "bad".into(),
                },
                Some("invalid_request"),
                Some(400),
            ),
            (
                LlmError::ModelUnavailable,
                Some("model_not_found"),
                Some(404),
            ),
            (LlmError::ProviderInternal, Some("server_error"), Some(500)),
            // Timeout/transport tail → `server_error`, no status.
            (
                LlmError::Transport {
                    message: "t".into(),
                },
                Some("server_error"),
                None,
            ),
            (
                LlmError::StreamInterrupted {
                    message: "s".into(),
                },
                Some("server_error"),
                None,
            ),
            // Generic `Error` fallthrough → `unknown`.
            (
                LlmError::CostUnavailable {
                    message: "c".into(),
                },
                Some("unknown"),
                None,
            ),
            (
                LlmError::UnsupportedCapability {
                    capability: "x".into(),
                },
                Some("unknown"),
                None,
            ),
        ];
        for (inner, cat, status) in cases {
            // Both wrapping variants classify identically.
            for wrapped in [
                OrchestratorError::ApiCall(inner.clone()),
                OrchestratorError::Streaming(inner.clone()),
            ] {
                let env = classify_api_error(&wrapped);
                assert_eq!(env.error, cat, "category for {inner:?}");
                assert_eq!(env.api_error_status, status, "status for {inner:?}");
                // The `ql` path never overrides the inner stop_reason.
                assert_eq!(
                    env.inner_stop_reason, None,
                    "inner stop_reason for {inner:?}"
                );
            }
        }
    }

    /// Orchestrator-internal / generic-Error variants fall through to `unknown`
    /// with NO status (the `Flp` generic-`Error` tail).
    #[test]
    fn classify_api_error_generic_variants_are_unknown_no_status() {
        for e in [
            OrchestratorError::Internal("boom".into()),
            OrchestratorError::StreamingProtocol("bad".into()),
            OrchestratorError::StreamEndedWithoutStop,
            OrchestratorError::RepeatedOverloaded,
            OrchestratorError::MaxTurnsReached { max_turns: 30 },
            OrchestratorError::MaxBudgetReached {
                budget_nano_usd: 5_000_000_000,
            },
        ] {
            let env = classify_api_error(&e);
            assert_eq!(env.error, Some("unknown"), "{e:?}");
            assert_eq!(env.api_error_status, None, "{e:?}");
            assert_eq!(env.inner_stop_reason, None, "{e:?}");
        }
    }

    /// End-to-end: a classified envelope drives the persisted JSONL line's
    /// top-level `error`/`isApiErrorMessage`/`apiErrorStatus` fields with
    /// presence + values 1:1 with the classifier output.
    #[test]
    fn classified_envelope_stamps_jsonl_top_level_fields() {
        use llm_client::LlmError;
        let dir = tempfile::tempdir().expect("tempdir");
        let orch = orch_with_writer(dir.path(), dir.path().join("s.jsonl"));
        let msg = ConversationMessage::Assistant {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::Text {
                text: "invalid request: bad".into(),
            }],
            stop_reason: Some("model_error".into()),
        };

        let env = classify_api_error(&OrchestratorError::ApiCall(LlmError::InvalidRequest {
            message: "bad".into(),
        }));
        let line = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&env),
        );
        assert_eq!(
            line.extra.get("error").and_then(|v| v.as_str()),
            Some("invalid_request")
        );
        assert_eq!(
            line.extra.get("isApiErrorMessage"),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            line.extra.get("apiErrorStatus"),
            Some(&serde_json::Value::Number(400.into()))
        );

        // A no-status category (server_error from transport) OMITS apiErrorStatus.
        let env2 = classify_api_error(&OrchestratorError::ApiCall(LlmError::Transport {
            message: "t".into(),
        }));
        let line2 = orch.to_jsonl_message_with_inner_id(
            &msg,
            "sess",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&env2),
        );
        assert_eq!(
            line2.extra.get("error").and_then(|v| v.as_str()),
            Some("server_error")
        );
        assert!(
            !line2.extra.contains_key("apiErrorStatus"),
            "no-status category must omit apiErrorStatus"
        );
    }

    // ── transcript per-line cwd reflects the LIVE (post-`cd`) session cwd ──────

    #[tokio::test]
    async fn transcript_line_cwd_tracks_live_cwd_after_cd() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Share a `current_cwd` cell with the orchestrator — the same `Arc` the
        // desktop composition root hands to `OrchestratorCwdChangedFirer`, which
        // a Bash `cd` mutates. Start it at the init cwd.
        let init_cwd = dir.path().to_path_buf();
        let cell = Arc::new(std::sync::Mutex::new(init_cwd.clone()));
        let orch =
            orch_with_writer(dir.path(), dir.path().join("s.jsonl")).with_current_cwd(cell.clone());

        // First persisted line is stamped with the init cwd.
        let m1 = ConversationMessage::user(protocol::MessageId::new(), "before cd".into());
        let line1 = orch.to_jsonl_message(&m1, "sess", None, None, None, None);
        assert_eq!(
            line1.cwd,
            init_cwd.to_string_lossy(),
            "pre-`cd` line carries the init cwd"
        );

        // Simulate a Bash `cd` advancing the shared cell (what the CwdChanged
        // firer does on every `cd`).
        let new_cwd = dir.path().join("subdir");
        *cell.lock().unwrap() = new_cwd.clone();

        // The NEXT persisted line must reflect the advanced cwd, not the init.
        let m2 = ConversationMessage::user(protocol::MessageId::new(), "after cd".into());
        let line2 = orch.to_jsonl_message(&m2, "sess", None, None, None, None);
        assert_eq!(
            line2.cwd,
            new_cwd.to_string_lossy(),
            "post-`cd` line must carry the advanced live cwd, not the init cwd"
        );
        assert_ne!(
            line2.cwd, line1.cwd,
            "the cwd readback must move with the live session cwd"
        );
    }

    // ── test 5: per-content_block_stop single-block assistant lines ───────────
    //
    // claude.ts:2171-2211: a streaming assistant turn emits ONE JSONL line per
    // content block — same inner `message.id`, distinct top-level `uuid`, one
    // block each. sessionStorage.ts:1028: each tool_result parents to ITS
    // tool_use's line uuid (`sourceToolAssistantUUID`), NOT a shared per-turn
    // parent.
    //
    // This drives `persist_assistant_per_block` directly: an assistant turn with
    // content [text, tool_use A, tool_use B] must persist THREE single-block
    // assistant lines that (a) share one inner `message.id`, (b) have three
    // DISTINCT top-level uuids, (c) carry exactly one block each; then a
    // tool_result for A parents to A's line uuid and a tool_result for B parents
    // to B's line uuid.
    #[tokio::test]
    async fn assistant_turn_persists_one_line_per_content_block_with_per_tool_reparenting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let orch = orch_with_writer(dir.path(), session_path.clone());

        let id_a = protocol::ToolUseId::from("toolu_A");
        let id_b = protocol::ToolUseId::from("toolu_B");

        let assistant_id = protocol::MessageId::new();
        let assistant_msg = ConversationMessage::Assistant {
            id: assistant_id,
            content: vec![
                protocol::ContentBlock::Text {
                    text: "let me call two tools".into(),
                },
                protocol::ContentBlock::ToolUse {
                    id: id_a.clone(),
                    name: "Alpha".into(),
                    input: serde_json::json!({}),
                    provider_id: None,
                },
                protocol::ContentBlock::ToolUse {
                    id: id_b.clone(),
                    name: "Bravo".into(),
                    input: serde_json::json!({}),
                    provider_id: None,
                },
            ],
            stop_reason: Some("tool_use".into()),
        };

        let map = orch
            .persist_assistant_per_block(&assistant_msg, None, None)
            .await;

        let lines = read_jsonl(&session_path);
        // (c) THREE single-block assistant lines.
        let asst_lines: Vec<&JsonlMessage> = lines
            .iter()
            .filter(|l| l.message_type == "assistant")
            .collect();
        assert_eq!(
            asst_lines.len(),
            3,
            "expected 3 single-block assistant lines (one per content block), got {}",
            asst_lines.len()
        );
        for (i, l) in asst_lines.iter().enumerate() {
            let blocks = l
                .message
                .get("content")
                .and_then(|c| c.as_array())
                .unwrap_or_else(|| panic!("line {i} content must be an array"));
            assert_eq!(blocks.len(), 1, "line {i} must carry exactly one block");
        }

        // (a) all three share ONE inner `message.id`.
        let inner_ids: Vec<&str> = asst_lines
            .iter()
            .map(|l| {
                l.message
                    .get("id")
                    .and_then(|v| v.as_str())
                    .expect("inner message.id present")
            })
            .collect();
        assert_eq!(
            inner_ids[0], inner_ids[1],
            "all blocks must share the same inner message.id"
        );
        assert_eq!(inner_ids[1], inner_ids[2]);
        assert_eq!(
            inner_ids[0],
            assistant_id.as_uuid().to_string(),
            "shared inner message.id must be the turn's logical id"
        );

        // (b) three DISTINCT top-level uuids.
        let uuids: std::collections::HashSet<&str> =
            asst_lines.iter().map(|l| l.uuid.as_str()).collect();
        assert_eq!(
            uuids.len(),
            3,
            "the three lines must have distinct top-level uuids"
        );

        // map must hold A and B -> their respective line uuids (text block none).
        let a_uuid = map.get(&id_a).expect("A in map").clone();
        let b_uuid = map.get(&id_b).expect("B in map").clone();
        assert_ne!(a_uuid, b_uuid, "A and B must map to different line uuids");

        // Persist a tool_result for A and for B; each must parent to ITS line.
        let tr_a = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: id_a.clone(),
                content: "result-A".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
        };
        orch.persist_message_to_jsonl_with_parent(&tr_a, Some(a_uuid.clone()))
            .await;
        let tr_b = ConversationMessage::User {
            id: protocol::MessageId::new(),
            content: vec![protocol::ContentBlock::ToolResult {
                tool_use_id: id_b.clone(),
                content: "result-B".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
        };
        orch.persist_message_to_jsonl_with_parent(&tr_b, Some(b_uuid.clone()))
            .await;

        let lines = read_jsonl(&session_path);
        let tr_lines: Vec<&JsonlMessage> = lines
            .iter()
            .filter(|l| {
                l.message_type == "user"
                    && serde_json::to_string(&l.message)
                        .map(|s| s.contains("tool_result"))
                        .unwrap_or(false)
            })
            .collect();
        assert_eq!(tr_lines.len(), 2, "expected 2 tool_result user lines");
        // tr_a parents to A's line; tr_b parents to B's line — NOT one shared parent.
        assert_eq!(
            tr_lines[0].parent_uuid.as_deref(),
            Some(a_uuid.as_str()),
            "tool_result A must parent to A's tool_use line uuid"
        );
        assert_eq!(
            tr_lines[1].parent_uuid.as_deref(),
            Some(b_uuid.as_str()),
            "tool_result B must parent to B's tool_use line uuid"
        );
        assert_ne!(
            tr_lines[0].parent_uuid, tr_lines[1].parent_uuid,
            "the two tool_results must NOT share one parent (per-tool reparenting)"
        );
    }
}

#[cfg(test)]
mod prefix_overflow_block_count_tests {
    use super::count_document_and_image_blocks;
    use protocol::{ContentBlock, ConversationMessage, DocumentSource, ImageSource, MessageId};

    fn image_block() -> ContentBlock {
        ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "AAAA".into(),
            },
        }
    }

    fn document_block() -> ContentBlock {
        ContentBlock::Document {
            source: DocumentSource::Base64 {
                media_type: "application/pdf".into(),
                data: "AAAA".into(),
            },
        }
    }

    #[test]
    fn counts_documents_and_images_across_user_and_assistant() {
        // #55 a3p documentBlockCount / imageBlockCount.
        let msgs = vec![
            ConversationMessage::User {
                id: MessageId::new(),
                content: vec![
                    ContentBlock::Text { text: "hi".into() },
                    image_block(),
                    document_block(),
                ],
                is_meta: false,
            },
            ConversationMessage::Assistant {
                id: MessageId::new(),
                content: vec![image_block()],
                stop_reason: None,
            },
            // System messages carry a flat string — never counted.
            ConversationMessage::System {
                id: MessageId::new(),
                content: "system".into(),
            },
        ];
        let (docs, imgs) = count_document_and_image_blocks(&msgs);
        assert_eq!(docs, 1, "one document block across the messages");
        assert_eq!(imgs, 2, "two image blocks across the messages");
    }

    #[test]
    fn counts_zero_when_no_media_blocks() {
        let msgs = vec![ConversationMessage::user(
            MessageId::new(),
            "plain text".into(),
        )];
        assert_eq!(count_document_and_image_blocks(&msgs), (0, 0));
    }
}

// #78: unit coverage for the streaming-path "visible output" predicate. The
// streaming driver's thinking-only nudge (`conversation.rs` `Some("end_turn")`
// / `Some("stop_sequence")` / `None` arms) gates on this exact function; the
// batched twin's branch transitions are covered in
// `turn_loop::malformed_and_thinking_only_tests`.
#[cfg(test)]
mod pumped_visible_text_tests {
    use super::pumped_has_visible_text;
    use protocol::ContentBlock;

    fn text(s: &str) -> ContentBlock {
        ContentBlock::Text {
            text: s.to_string(),
        }
    }
    fn thinking(s: &str) -> ContentBlock {
        ContentBlock::Thinking {
            thinking: s.to_string(),
            signature: None,
        }
    }

    #[test]
    fn empty_blocks_have_no_visible_text() {
        assert!(!pumped_has_visible_text(&[]));
    }

    #[test]
    fn thinking_only_has_no_visible_text() {
        assert!(!pumped_has_visible_text(&[thinking("reasoning")]));
    }

    #[test]
    fn whitespace_only_text_is_not_visible() {
        assert!(!pumped_has_visible_text(&[text("   \n\t ")]));
    }

    #[test]
    fn non_empty_text_is_visible() {
        assert!(pumped_has_visible_text(&[text("hello")]));
    }

    #[test]
    fn thinking_plus_real_text_is_visible() {
        assert!(pumped_has_visible_text(&[
            thinking("reasoning"),
            text("answer")
        ]));
    }
}

// ============================================================================
// Finding #73: per-turn `todo_reminder` (V1) / `task_reminder` (V2).
//
// Proves [`ConversationOrchestrator::todo_reminder_message`] +
// [`ConversationOrchestrator::bump_reminder_turn_counters`] +
// [`ConversationOrchestrator::note_todo_reminder_tool_call`]:
// - counters increment once per turn and reset on the relevant tool call;
// - the reminder fires only when BOTH counters reach the thresholds, the
//   relevant tool is present, the Brief tool is absent, history is non-empty,
//   and the killswitch is not "off";
// - the body is byte-exact (V1 with/without items; V2 with items) and emitted
//   RAW (no `<system-reminder>` wrapper).
// The byte-level renderer is additionally covered in `tool_task::reminder::tests`.
// ============================================================================
#[cfg(test)]
mod todo_reminder_tests {
    use super::*;
    use crate::prompt::todo_reminder::{TaskReminderItem, TodoReminderTaskProvider};
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use engine::{TodoItem, TodoState};
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// `std::env::set_var`/`remove_var` are not thread-safe; serialize the
    /// env-mutating tests (selection + killswitch) behind this lock.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Minimal name-only tool for the tool-presence gates.
    struct NamedTool(&'static str);
    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> = once_cell::sync::Lazy::new(
                || serde_json::json!({ "type": "object", "properties": {} }),
            );
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other { reason: "t".into() },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            String::new()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: serde_json::json!({}),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// Static V2 task source.
    struct StaticTasks(Vec<TaskReminderItem>);
    #[async_trait]
    impl TodoReminderTaskProvider for StaticTasks {
        async fn task_items(&self) -> Vec<TaskReminderItem> {
            self.0.clone()
        }
    }

    fn orch_with(tools: ToolRegistry) -> ConversationOrchestrator {
        let api = Arc::new(MockApiClient::new(vec![]));
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            api,
            Arc::new(tools),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        )
    }

    fn reg_with(names: &[&'static str]) -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        for n in names {
            reg.register_builtin(Arc::new(NamedTool(n)));
        }
        reg
    }

    /// Make the session non-empty (the binary `!e||e.length===0 ⇒ []` gate) and
    /// optionally arm both counters at their thresholds.
    async fn prime_session(orch: &ConversationOrchestrator, write_c: u32, reminder_c: u32) {
        let mut s = orch.session.lock().await;
        s.history.push(ConversationMessage::user(
            MessageId::new(),
            "hi".to_string(),
        ));
        s.turns_since_last_todo_write = write_c;
        s.turns_since_last_reminder = reminder_c;
    }

    // ── counters ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn bump_increments_both_counters_each_turn() {
        let orch = orch_with(reg_with(&["TodoWrite"]));
        orch.bump_reminder_turn_counters().await;
        orch.bump_reminder_turn_counters().await;
        let s = orch.session.lock().await;
        assert_eq!(s.turns_since_last_todo_write, 2);
        assert_eq!(s.turns_since_last_reminder, 2);
    }

    #[tokio::test]
    async fn note_tool_call_resets_write_counter_on_todowrite_v1() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("LINGXI_ENABLE_TASKS", "off"); // ⇒ V1 selected
        let orch = orch_with(reg_with(&["TodoWrite"]));
        {
            let mut s = orch.session.lock().await;
            s.turns_since_last_todo_write = 7;
            s.turns_since_last_reminder = 7;
        }
        orch.note_todo_reminder_tool_call(&["TodoWrite".to_string()])
            .await;
        {
            let s = orch.session.lock().await;
            assert_eq!(
                s.turns_since_last_todo_write, 0,
                "TodoWrite resets write ctr"
            );
            assert_eq!(s.turns_since_last_reminder, 7, "reminder ctr untouched");
        }
        // A non-qualifying tool (Read) does NOT reset.
        {
            let mut s = orch.session.lock().await;
            s.turns_since_last_todo_write = 5;
        }
        orch.note_todo_reminder_tool_call(&["Read".to_string()])
            .await;
        assert_eq!(orch.session.lock().await.turns_since_last_todo_write, 5);
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn note_tool_call_resets_on_taskupdate_v2_default() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_ENABLE_TASKS"); // default ⇒ V2
        let orch = orch_with(reg_with(&["TaskUpdate"]));
        {
            let mut s = orch.session.lock().await;
            s.turns_since_last_todo_write = 9;
        }
        // V2 resets on TaskCreate or TaskUpdate, NOT on TodoWrite.
        orch.note_todo_reminder_tool_call(&["TodoWrite".to_string()])
            .await;
        assert_eq!(
            orch.session.lock().await.turns_since_last_todo_write,
            9,
            "V2 mode does not reset on TodoWrite"
        );
        orch.note_todo_reminder_tool_call(&["TaskUpdate".to_string()])
            .await;
        assert_eq!(orch.session.lock().await.turns_since_last_todo_write, 0);
    }

    // ── firing gates ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn no_fire_below_threshold() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::set_var("LINGXI_ENABLE_TASKS", "off"); // V1
        let orch = orch_with(reg_with(&["TodoWrite"]));
        prime_session(&orch, 9, 10).await; // write ctr one short
        assert!(orch.todo_reminder_message().await.is_none());
        // Now both at threshold ⇒ fires.
        {
            let mut s = orch.session.lock().await;
            s.turns_since_last_todo_write = 10;
        }
        assert!(orch.todo_reminder_message().await.is_some());
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn no_fire_when_history_empty() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("LINGXI_ENABLE_TASKS", "off");
        let orch = orch_with(reg_with(&["TodoWrite"]));
        // counters armed but NO history.
        {
            let mut s = orch.session.lock().await;
            s.turns_since_last_todo_write = 10;
            s.turns_since_last_reminder = 10;
        }
        assert!(
            orch.todo_reminder_message().await.is_none(),
            "empty history suppresses the reminder"
        );
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn no_fire_when_tool_absent() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("LINGXI_ENABLE_TASKS", "off"); // V1 needs TodoWrite
        let orch = orch_with(reg_with(&["Read"])); // no TodoWrite
        prime_session(&orch, 10, 10).await;
        assert!(orch.todo_reminder_message().await.is_none());
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn no_fire_when_brief_present() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("LINGXI_ENABLE_TASKS", "off");
        // TodoWrite present AND Brief (SendUserMessage) present ⇒ skip.
        let orch = orch_with(reg_with(&["TodoWrite", "SendUserMessage"]));
        prime_session(&orch, 10, 10).await;
        assert!(orch.todo_reminder_message().await.is_none());
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn killswitch_off_suppresses() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("LINGXI_ENABLE_TASKS", "off");
        std::env::set_var("LINGXI_TODO_REMINDER_MODE", "off");
        let orch = orch_with(reg_with(&["TodoWrite"]));
        prime_session(&orch, 10, 10).await;
        assert!(
            orch.todo_reminder_message().await.is_none(),
            "killswitch \"off\" suppresses the reminder"
        );
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    // ── exact text + reminder-counter reset on fire ──────────────────────────

    #[tokio::test]
    async fn v1_fires_with_exact_text_no_items_and_resets_reminder_ctr() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::set_var("LINGXI_ENABLE_TASKS", "off"); // V1
        let orch = orch_with(reg_with(&["TodoWrite"]));
        prime_session(&orch, 10, 10).await;
        let msg = orch.todo_reminder_message().await.expect("fires");
        // RAW body — NOT wrapped in <system-reminder>.
        assert_eq!(
            msg.text_content(),
            "The TodoWrite tool hasn't been used recently. If you're working on tasks that would benefit from tracking progress, consider using the TodoWrite tool to track progress. Also consider cleaning up the todo list if has become stale and no longer matches what you are working on. Only use it if it's relevant to the current work. This is just a gentle reminder - ignore if not applicable.\n"
        );
        // The reminder counter reset to 0 on fire.
        assert_eq!(orch.session.lock().await.turns_since_last_reminder, 0);
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn v1_fires_with_items_byte_exact() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::set_var("LINGXI_ENABLE_TASKS", "off"); // V1
        let orch = orch_with(reg_with(&["TodoWrite"]));
        prime_session(&orch, 10, 10).await;
        {
            let mut s = orch.session.lock().await;
            s.todos.push(TodoItem {
                id: "t1".into(),
                content: "first".into(),
                status: TodoState::Pending,
                active_form: "Doing first".into(),
            });
            s.todos.push(TodoItem {
                id: "t2".into(),
                content: "second".into(),
                status: TodoState::InProgress,
                active_form: "Doing second".into(),
            });
        }
        let msg = orch.todo_reminder_message().await.expect("fires");
        assert!(msg.text_content().ends_with(
            "\n\nHere are the existing contents of your todo list:\n\n[1. [pending] first\n2. [in_progress] second]"
        ), "got: {:?}", msg.text_content());
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn v2_fires_with_items_from_provider_byte_exact() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::remove_var("LINGXI_ENABLE_TASKS"); // default ⇒ V2
        let orch = orch_with(reg_with(&["TaskUpdate"])).with_todo_reminder_tasks(Arc::new(
            StaticTasks(vec![
                TaskReminderItem {
                    id: "1".into(),
                    status: TodoState::Completed,
                    subject: "alpha".into(),
                },
                TaskReminderItem {
                    id: "2".into(),
                    status: TodoState::Pending,
                    subject: "beta".into(),
                },
            ]),
        ));
        prime_session(&orch, 10, 10).await;
        let msg = orch.todo_reminder_message().await.expect("fires");
        let text = msg.text_content();
        assert!(
            text.starts_with("The task tools haven't been used recently."),
            "got: {text}"
        );
        assert!(
            text.ends_with(
                "\n\nHere are the existing tasks:\n\n#1. [completed] alpha\n#2. [pending] beta"
            ),
            "got: {text:?}"
        );
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }

    #[tokio::test]
    async fn v2_fires_base_only_without_provider() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("LINGXI_TODO_REMINDER_MODE");
        std::env::remove_var("LINGXI_ENABLE_TASKS"); // V2
        let orch = orch_with(reg_with(&["TaskUpdate"])); // no task provider
        prime_session(&orch, 10, 10).await;
        let msg = orch.todo_reminder_message().await.expect("fires");
        assert_eq!(
            msg.text_content(),
            "The task tools haven't been used recently. If you're working on tasks that would benefit from tracking progress, consider using TaskCreate to add new tasks and TaskUpdate to update task status (set to in_progress when starting, completed when done). Also consider cleaning up the task list if it has become stale. Only use these if relevant to the current work. This is just a gentle reminder - ignore if not applicable.\n"
        );
        std::env::remove_var("LINGXI_ENABLE_TASKS");
    }
}
