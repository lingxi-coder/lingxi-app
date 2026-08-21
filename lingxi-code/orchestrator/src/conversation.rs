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
use protocol::{ConversationMessage, HookId, MessageId, SessionId};
use session::JsonlWriter;
use std::time::Duration;

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
use tool_api::ToolRegistryView as _;
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

    /// Non-streaming `messages.create` carrying a `context_hint` offer.
    ///
    /// The DEFAULT body delegates to [`Self::messages_create`], DROPPING the
    /// hint — so every mock and non-Anthropic impl compiles unchanged and the
    /// negotiation is a strict no-op there. Only [`ProviderApiAdapter`]
    /// overrides it. Same shape as [`Self::messages_create_with_opts`] and for
    /// the same reason: this trait has 13 implementors and is extended by
    /// defaulted methods, never by signature changes.
    ///
    /// The turn loop calls this ONLY when the context-hint controller is active
    /// (gated off by default); otherwise it stays on [`Self::messages_create`].
    async fn messages_create_with_context_hint(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        _context_hint: Option<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        self.messages_create(model, profile, system, msgs, tools)
            .await
    }

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

    /// Return an exact provider count when available. The default is `None`
    /// so mocks and providers without Anthropic's count endpoint select their
    /// caller-specific fallback instead of receiving the text-only estimate.
    async fn count_tokens_exact(
        &self,
        _model: &str,
        _profile: Option<&str>,
        _system: Option<&str>,
        _msgs: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<Option<u64>, LlmError> {
        Ok(None)
    }

    /// Host-validated beta additions applied to provider requests. The
    /// orchestrator consumes these for the same context-window/compaction
    /// calculations; default clients have none.
    fn active_betas(&self) -> Vec<String> {
        Vec::new()
    }

    /// Enumerate available `provider/model` ids + `@aliases` for `/model`'s
    /// list mode. Default returns empty so non-routing impls (mocks / the
    /// no-streaming stub) need no override; `ProviderApiAdapter` overrides it
    /// to delegate to the router.
    fn available_models(&self) -> Vec<String> {
        Vec::new()
    }

    /// Replace the thinking policy used for subsequent provider requests.
    /// Implementations without a mutable request layer may keep the default
    /// no-op; the production provider adapter overrides it.
    fn set_thinking_config(&self, _thinking: llm_client::model::thinking::ThinkingConfig) {}

    /// Replace the main-loop effort used for subsequent provider requests.
    /// `None` clears the live override.
    fn set_effort(&self, _effort: Option<serde_json::Value>) {}

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
    /// A session-scoped `/goal` Stop Prompt hook blocked natural completion.
    /// Unlike a generic Stop-hook block, this bypasses the dedicated
    /// stop-hook block-cap logic; the normal turn-loop boundaries (cancel,
    /// hard budget, max_turns at loop top) remain the only escapes.
    GoalContinue(String),
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

const GOAL_PROMPT_TIMEOUT_SECS: u64 = 30;
const GOAL_STOP_HOOK_NAME: &str = "__session_goal_stop";
const GOAL_STOP_HOOK_PRIORITY: i32 = 1_000_000;

fn goal_stop_hook_prompt(condition: &str) -> String {
    format!(
        "Evaluate whether the active session goal has been fully met.\nGoal condition:\n{condition}\nUse this Stop hook payload JSON as the current stop state:\n$ARGUMENTS"
    )
}

fn strip_goal_prompt_block_reason(reason: &str) -> String {
    if let Some(stripped) = reason.strip_prefix('[') {
        if let Some((_, tail)) = stripped.split_once("]: ") {
            return tail.to_string();
        }
    }
    reason.to_string()
}

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
    // TRUE status first, canonical table second.
    //
    // This is the half of the carve-out documented above that is now closed.
    // Provider decoders store the SDK's `${status} ${body}` text (see
    // `providers::api_error_message`), so a real 422/424/409 is recoverable
    // instead of being flattened to its variant's canonical status. The table
    // below still runs whenever no prefix is present — every variant that
    // carries no message, and every `InvalidRequest` raised by internal
    // validation rather than a provider decode.
    //
    // Still provider-NEUTRAL: the prefix is written by whichever provider
    // decoded the response, so this does not reintroduce an Anthropic-only path.
    let parsed_status = match e {
        OrchestratorError::ApiCall(inner) | OrchestratorError::Streaming(inner) => {
            inner.http_status()
        }
        _ => None,
    };
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
            LlmError::Authentication { .. } => (Some("authentication_failed"), Some(401)),
            // 403 → "authentication_failed".
            LlmError::PermissionDenied { .. } => (Some("authentication_failed"), Some(403)),
            // Dead OAuth session (`e instanceof qQt`) → the oracle renders it
            // with `yu({error:"authentication_failed"})` and passes NO status:
            // the refresh call failed against the IdP, so there is no
            // `APIError` status to carry. Omit rather than invent a 401.
            LlmError::OAuthRefreshDead => (Some("authentication_failed"), None),
            // Billing (`Fio`) is an Error-message match in `Flp`, not a status
            // branch → category only, no `apiErrorStatus`.
            LlmError::QuotaExceeded => (Some("billing_error"), None),
            // PTL/context-window (`Nio`/`D9t`) → `ql({error:"invalid_request"})`
            // with NO status set; the port decodes ContextOverflow from the
            // message, so no `APIError` status is available → omit.
            LlmError::ContextOverflow { .. } => (Some("invalid_request"), None),
            // 413 request-too-large (`su({content:$Vi(),error:"invalid_request",
            // errorDetails:`request_too_large: …`})`, 2.1.212) → the SAME
            // `invalid_request` category as the context-window branch; the
            // handler passes no `apiErrorStatus` on this `su` call → omit.
            LlmError::RequestTooLarge => (Some("invalid_request"), None),
            // 400 invalid-request family → "invalid_request" (status 400).
            LlmError::InvalidRequest { .. } => (Some("invalid_request"), Some(400)),
            // 404 / bedrock model-id → "model_not_found".
            LlmError::ModelUnavailable => (Some("model_not_found"), Some(404)),
            // `Flp` tail `status>=500` → "server_error".
            LlmError::ProviderInternal => (Some("server_error"), Some(500)),
            // Timeout / transport / connection-lost tail → "server_error", no
            // status (these are not `APIError`-with-numeric-status).
            LlmError::Transport { .. }
            | LlmError::TransportTimeout { .. }
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
        | OrchestratorError::PermissionAbort { .. }
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
        api_error_status: parsed_status.or(api_error_status),
        inner_stop_reason: None,
    }
}

/// claude-code 2.1.212 `Sji` — the maximum request body size (32 MiB). A 413
/// whose message does NOT mention the context window means accumulated
/// image/attachment bytes pushed the raw request past this limit.
pub(crate) const MAX_REQUEST_BYTES: u64 = 33_554_432;

/// The byte-exact `$Vi()` "Request too large" notice claude-code 2.1.212 renders
/// for a 413 that is NOT a context-window overflow. `Ua(Sji)` formats
/// [`MAX_REQUEST_BYTES`] (32 MiB) as `32MB` (`toFixed(1)` then trailing `.0`
/// stripped). The tail differs by interactivity (`un()===!Ht.isInteractive`):
/// a non-interactive (print) session gets the generic advice; an interactive
/// (TUI) session gets the `/compact` + double-esc actions.
pub(crate) fn request_too_large_notice(interactive: bool) -> String {
    debug_assert_eq!(MAX_REQUEST_BYTES, 32 * 1024 * 1024);
    let head = "Request too large (max 32MB). Accumulated images and attachments in the conversation pushed the request over the limit.";
    if interactive {
        format!("{head} Run /compact, or double press esc to go back and remove attachments.")
    } else {
        format!("{head} Remove older images or compact the conversation.")
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

/// Conservative provider gate for Anthropic's dynamic-tool-loading beta.
/// Claude models routed through an Anthropic-family profile are eligible;
/// Haiku is the upstream unsupported-model denylist. Project-specific
/// non-Anthropic providers retain the complete inline tool list.
fn tool_search_supported_for_request(model: &str, profile: Option<&str>) -> bool {
    let model = model.to_ascii_lowercase();
    if model.contains("haiku") {
        return false;
    }
    profile.map_or_else(
        // Claude uses a negative capability test: every current/future model is
        // assumed to support tool_reference unless it matches the unsupported
        // Haiku pattern. A missing profile is the built-in first-party route,
        // not evidence that the model name must contain the word "claude".
        || true,
        |profile| {
            let profile = profile.to_ascii_lowercase();
            profile.contains("anthropic")
                || profile.contains("bedrock")
                || profile.contains("vertex-claude")
                || profile.contains("foundry")
        },
    )
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

/// Clone a message for durable persistence, replacing explicitly ephemeral tool
/// images with their non-sensitive summary. The live in-memory message remains
/// untouched and still carries its image content blocks to the current model.
fn redact_ephemeral_tool_result_images(
    message: &protocol::ConversationMessage,
) -> protocol::ConversationMessage {
    let mut sanitized = message.clone();
    let blocks = match &mut sanitized {
        protocol::ConversationMessage::User { content, .. }
        | protocol::ConversationMessage::Assistant { content, .. } => content,
        protocol::ConversationMessage::System { .. } => return sanitized,
    };
    for block in blocks {
        let protocol::ContentBlock::ToolResult {
            content,
            content_blocks,
            ..
        } = block
        else {
            continue;
        };
        let Ok(marker) = serde_json::from_str::<serde_json::Value>(content) else {
            continue;
        };
        if marker
            .get("_lingxi_ephemeral")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        {
            continue;
        }
        *content = marker
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Ephemeral tool image omitted from session persistence.")
            .to_string();
        *content_blocks = None;
    }
    sanitized
}

#[cfg(test)]
#[path = "conversation_test.rs"]
mod conversation_test;

/// Bare text injected as a user message when streaming is cancelled (ESC /
/// SIGINT) DURING tool execution for the current turn. 1:1 with claude-code
/// `messages.ts:208` `INTERRUPT_MESSAGE_FOR_TOOL_USE`.
const INTERRUPT_MESSAGE_FOR_TOOL_USE: &str = "[Request interrupted by user for tool use]";

/// Folded outcome of the `PreCompact` lifecycle hooks.
pub(crate) struct PreCompactHookOutcome {
    /// Blocking reason, when a hook rejected compaction.
    pub(crate) blocked_by: Option<String>,
    /// Successful hook stdout appended to the summary prompt.
    pub(crate) additional_instructions: Option<String>,
}

fn merge_compact_instructions(primary: Option<&str>, additional: Option<&str>) -> Option<String> {
    let parts: Vec<&str> = [primary, additional]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    // Claude `mio`: caller focus and successful PreCompact stdout are separate
    // instruction paragraphs, joined by a blank line.
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// The orchestrator. Owns the session, dispatches tools, drives the loop.
///
/// Construction is via `new(...)` (batched-only) or `new_with_streaming(...)`
/// (both paths). Driven via `run_turn(prompt)` or `run_turn_streaming(prompt)`.
/// Snapshot of the `--agent`-adopted main-thread agent (claude-code
/// `mainThreadAgentDefinition` reduced to the fields LingXi applies on the MAIN
/// conversation loop). Set once at startup by the composition root; see
/// [`ConversationOrchestrator::main_thread_agent`].
#[derive(Debug, Clone)]
pub(crate) struct MainThreadAgentState {
    /// The agent's stable `agentType` (claude-code `mainThreadAgentType` /
    /// `MB()`), threaded into every main-thread lifecycle hook payload.
    pub(crate) agent_type: String,
    /// The agent's system-prompt body (claude-code `agentDef.getSystemPrompt()`)
    /// — becomes the main-loop system prompt on every query unless
    /// `--system-prompt` (`overrideSystemPrompt`) is set. `None` for an agent
    /// that declares no prompt (the assembled default prompt is then used).
    pub(crate) system_prompt: Option<String>,
    /// The agent's `tools:` frontmatter policy (claude-code `agentDef.tools`).
    /// Filters the advertised main-loop tool pool via [`Self::build_wire_tools`]
    /// — the `HJ(agentDef,to,!1,!0)` port with `n=true`, which keeps everything
    /// on [`AgentToolPolicy::All`] (no `tools:` field) and narrows to the named
    /// tools on [`AgentToolPolicy::Explicit`].
    pub(crate) tool_policy: agent::AgentToolPolicy,
    /// The agent's per-definition `disallowedTools` (claude-code
    /// `agentDef.disallowedTools`) — subtracted from the advertised pool BEFORE
    /// the `tool_policy` projection (base tool name, `(rule)` stripped).
    pub(crate) disallowed_tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WireToolSchemaCacheKey {
    tool_names: Vec<String>,
    model: String,
    model_profile: Option<String>,
}

#[derive(Debug, Clone)]
struct WireToolSchemaCache {
    key: WireToolSchemaCacheKey,
    wire: Vec<serde_json::Value>,
}

/// `date_change` (cc `Cop`) per-conversation state.
///
/// The oracle keeps two things: `LGe = Vr(wcs)`, the local date memoized when
/// the session started (the same date the env-context `currentDate` entry was
/// first built from), and the delivered `date_change` attachment itself.
/// Reminders are outgoing-only, so the delivered date is carried here and must
/// survive compaction; otherwise a long-lived session would receive the same
/// midnight reminder again after every compact. The whole struct is re-seeded
/// One `tool_result` SDK frame held until the collection point releases it.
#[derive(Debug, Clone)]
pub(crate) struct PendingToolFrame {
    pub(crate) tool: String,
    /// The model-facing text dispatch buffered. Compared against the released
    /// content to detect a substitution.
    pub(crate) model_text: String,
    pub(crate) result: serde_json::Value,
    pub(crate) denial_kind: Option<String>,
}

/// only when the live `SessionId` changes (`/clear` mints a new one; in-place
/// resume adopts the named one).
#[derive(Debug, Default)]
pub(crate) struct DateChangeState {
    /// Session this state belongs to; `None` until the first producer run.
    session_id: Option<protocol::SessionId>,
    /// `LGe()` — the local date memoized at session start.
    session_date: String,
    /// `newDate` of the reminder last DELIVERED to the model in this session.
    delivered_date: Option<String>,
}

const TOOL_TOKEN_COUNT_OVERHEAD: u64 = 500;

/// Host-approved app-specific instructions appended after the immutable
/// platform/runtime prompt layers. This is intentionally not
/// `system_prompt_override`, which would replace the security prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppAgentPromptProfile {
    /// Monotonic host-approved profile revision.
    pub revision: u64,
    /// Bounded app-specific instructions.
    pub instructions: String,
}

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
    /// Serializes user, queued, and async-hook re-wake turns. A background hook
    /// may finish while a user turn is still streaming; waiting here makes its
    /// re-wake the next turn instead of racing two model loops over one history.
    turn_gate: Mutex<()>,
    /// Live main-loop effort. Unlike `config.effort`, this can change through
    /// stream-json control requests and in-place resume.
    pub(crate) current_effort: std::sync::RwLock<Option<String>>,
    /// Provider-neutral live reasoning selection for subsequent requests.
    pub(crate) current_reasoning_selection: std::sync::RwLock<traits::ReasoningSelection>,
    /// Whether the live effort came from an explicit launch/control choice.
    /// Hot resume may inherit transcript effort only while this is false.
    pub(crate) current_effort_explicit: std::sync::atomic::AtomicBool,
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
    /// Task 5 (worktree 206 session-cwd plumbing): the SAME switchable cwd
    /// cell the tool layer swaps on `EnterWorktree`/`ExitWorktree`
    /// ([`tool_api::SessionCwd`], Task 1). The system prompt's `# Environment`
    /// `Primary working directory:` line, its trailing gitStatus block, and the
    /// per-turn `additional_context_message`/memory-prefetch cwd all read
    /// THIS (via [`Self::build_prompt_context`] et al.), so they re-derive from
    /// the post-swap worktree instead of the frozen boot `cwd` above.
    ///
    /// Defaults to a private, never-swapped `SessionCwd` over the constructor's
    /// `cwd` (see [`ConversationOrchestrator::new_with_streaming`]), so a caller
    /// that never wires [`Self::with_session_cwd`] behaves exactly as before —
    /// the INERT INVARIANT this plan depends on. Wired at the desktop/mobile
    /// composition roots to the SAME `Arc` handed to `BuiltinToolContext`.
    pub(crate) session_cwd: Arc<tool_api::SessionCwd>,
    /// Guest→host hop for prompt probes when the session cwd is a
    /// mobile-linux guest path — see [`Self::with_prompt_probe_cwd_resolver`].
    /// `None` everywhere but the mobile host.
    pub(crate) prompt_probe_cwd_resolver:
        Option<Arc<dyn Fn(&std::path::Path) -> std::path::PathBuf + Send + Sync>>,
    /// Fixed, engine-owned mobile runtime reminder prepended to real main-loop
    /// model requests. The message is rendered once when the mobile composition
    /// root wires it, then cloned with the same id and bytes for every retry and
    /// turn. It stays outside the system prompt so an explicit system-prompt
    /// override remains byte-exact, and outside session history so it is never
    /// persisted or duplicated by resume/compaction.
    pub(crate) mobile_runtime_environment_message: Option<ConversationMessage>,
    /// Typed mobile environment retained so the mutable guest cwd can be
    /// rendered per request as a separate second message. Stable host/tool
    /// facts remain frozen in `mobile_runtime_environment_message`.
    pub(crate) mobile_runtime_environment:
        Option<traits::mobile_runtime_environment::MobileRuntimeEnvironment>,
    /// Optional mobile host-path to guest-path mapping for live cwd updates.
    pub(crate) mobile_workspace_cwd_resolver:
        Option<Arc<dyn Fn(&std::path::Path) -> Option<String> + Send + Sync>>,
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
    /// `/goal` workspace-trust gate.
    pub(crate) workspace_trusted: bool,
    /// `/goal` hook-policy gate.
    pub(crate) hooks_restricted: bool,
    /// Optional on-disk JSONL persistence (M5-07). `None` for in-memory
    /// tests; `Some` when the CLI binary wires `~/.lingxi/projects/.../<uuid>.jsonl`.
    pub(crate) jsonl_writer: Option<Arc<JsonlWriter>>,
    /// Cached UUID of the last persisted JSONL entry — used to populate
    /// `parentUuid` on the next append. Reset to `None` for fresh sessions.
    pub(crate) last_jsonl_uuid: Mutex<Option<String>>,
    /// `tool_use_id` → claude's message-level `toolDenialKind`, recorded when a
    /// tool is denied and consumed when its `tool_result` user line is
    /// persisted.
    ///
    /// A side table rather than a field on the message because
    /// `ConversationMessage` is shared with the model wire, where the kind has
    /// no place — the same reason `injected_message_sources` is kept beside the
    /// history rather than on the message. Entries are removed on use, so a
    /// denial stamps exactly one line.
    pub(crate) tool_denial_kinds: Mutex<std::collections::HashMap<String, String>>,
    /// Buffered `tool_result` SDK frames, keyed by `tool_use_id`.
    ///
    /// `None` = emit as soon as the tool finishes (the batched driver, which
    /// dispatches in received order anyway, so completion order IS received
    /// order there). `Some` = the STREAMING driver is active: frames are held
    /// here and released by the collection point.
    ///
    /// claude-code builds its SDK frames from the message stream, which the
    /// streaming executor has already reordered into received order and in
    /// which a cancelled tool's real outcome has already been replaced by the
    /// synthetic. LingXi emits at dispatch time instead, which is completion
    /// order, and emits the REAL result of a tool whose outcome is about to be
    /// discarded — while a queued-then-cancelled tool emits nothing at all.
    /// Buffering here and releasing at the collection point closes both.
    pub(crate) tool_frames: Mutex<Option<std::collections::HashMap<String, PendingToolFrame>>>,
    /// `tool_use_id` → claude's message-level `toolUseResult` — the tool's RAW
    /// STRUCTURED result (`se.data`, 2.1.220 BIN off **235420375**) on success,
    /// or the plain string `` `Error: ${message}` `` on failure/denial
    /// (BIN off **235424595**). Recorded at dispatch, consumed when the
    /// `tool_result` user line is persisted.
    ///
    /// Same side-table rationale as [`Self::tool_denial_kinds`]: the value has
    /// no place on the model wire, so it cannot ride on
    /// `ConversationMessage`.
    ///
    /// RESIDUAL (deliberate): claude suppresses this field for SUBAGENT tool
    /// results (`n.agentId && !preserveToolUseResults && …` in the same
    /// expression). LingXi's `ConversationOrchestrator` has no `agent_id` —
    /// subagents never run through it, so every orchestrator is a depth-0 main
    /// chain, where claude writes the field on 17 280 / 17 280 real 2.1.220
    /// lines. Porting the gate would mean inventing a field.
    pub(crate) tool_use_results: Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// `tool_use_id` → claude's message-level `mcpMeta`, a TOP-LEVEL sibling of
    /// `toolUseResult` (never nested inside it). On the main chain
    /// `Uks(agentId, meta)` (2.1.220 BIN off **232969604**) returns the MCP
    /// server's meta verbatim when `agentId` is absent.
    pub(crate) tool_use_mcp_meta: Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// `tool_use_id` → claude's `sourceToolAssistantUUID`: the uuid of the
    /// ASSISTANT transcript line that carried this `tool_use` block
    /// (`sourceToolAssistantUUID: i.uuid` at every producer site). claude's
    /// writer `insertMessageChain` (BIN off **237862200**) then derives
    /// `parentUuid` FROM this field, which is why the two are equal on all
    /// 96 794 real 2.1.220 lines that carry it.
    pub(crate) tool_source_assistant_uuids: Mutex<std::collections::HashMap<String, String>>,
    /// `tool_use_id` → hook `attachment` PAYLOADS produced while dispatching
    /// that tool, awaiting persistence.
    ///
    /// claude yields a hook attachment INTO the message stream, so
    /// `insertMessageChain` writes it after the `tool_result` it follows.
    /// LingXi's `dispatch_tool_uses_tracked` runs the hooks but does not
    /// persist anything — the driver persists the tool_result afterwards — so
    /// the payloads are parked here and flushed by
    /// [`Self::flush_hook_attachments`] immediately after that tool's
    /// `tool_result` line lands, preserving claude's chain order. Keyed by
    /// tool so a concurrent streaming batch cannot interleave one tool's
    /// attachments behind another's result.
    pub(crate) pending_hook_attachments:
        Mutex<std::collections::HashMap<String, Vec<serde_json::Value>>>,
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
    /// Models already routed to this session's refusal cascade. Consumed by
    /// the stage resolver so a chain never loops back onto a model that has
    /// already refused — the bound that lets a multi-hop chain terminate
    /// without relying on the once-per-session latch.
    pub(crate) refusal_tried_models: Mutex<Vec<String>>,
    /// The refusal episode's accumulating notice: hops fold into one notice
    /// describing where the session ended up, rather than each hop announcing a
    /// model the cascade may already have left.
    pub(crate) refusal_episode: Mutex<crate::refusal_notice::RefusalEpisode>,
    /// The collapse queue in front of the notice stream. Holds a provisional
    /// notice and drops it when a later one supersedes it, counting the
    /// collapse for `tengu_refusal_fallback_notice_collapsed`.
    pub(crate) refusal_notice_queue: Mutex<crate::refusal_notice::NoticeQueue>,
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
    /// Monotonic timestamp captured at the start of the current session. Used by
    /// `snapshot_cost` to compute the `session_duration` field of the
    /// returned [`traits::CostSnapshot`]. Stored as `std::time::Instant`
    /// (not `tokio::time::Instant`) so the orchestrator can be constructed
    /// outside a tokio runtime if needed.
    pub(crate) session_started_at: std::sync::Mutex<std::time::Instant>,
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
    /// Live plugin output-style registry. Disk/builtin styles remain sourced
    /// from [`OrchestratorConfig`]; this optional registry makes plugin reloads
    /// visible to prompt assembly without rebuilding the orchestrator.
    pub(crate) output_style_registry:
        Option<Arc<tokio::sync::RwLock<outputstyles::OutputStyleRegistry>>>,
    /// Session-level cache for the expensive prompt/schema serialization of
    /// the post-filter tool pool. Dynamic per-turn marks (`strict`,
    /// `defer_loading`) are applied to a clone after cache lookup.
    wire_tool_schema_cache: Mutex<Option<WireToolSchemaCache>>,
    /// Exact deferred-schema token counts, memoized by the same stable schema
    /// key as `wire_tool_schema_cache`. `None` is cached too: unsupported
    /// providers must not retry a doomed count endpoint every turn.
    deferred_tool_token_cache:
        Mutex<std::collections::HashMap<WireToolSchemaCacheKey, Option<u64>>>,
    /// Subagent catalog (M6-07). `None` when not wired — `list_agents`
    /// then returns `vec![]`. The CLI binary populates from
    /// `~/.lingxi/agents/` + project `.lingxi/agents/`.
    pub(crate) agent_catalog: Option<Arc<tokio::sync::RwLock<Vec<agent::AgentDefinition>>>>,
    /// Main-thread agent adopted via `--agent` (claude-code
    /// `Pt.mainThreadAgentType` / `mainThreadAgentDefinition`, read per query
    /// through `MB()`). `Some` once the composition root resolves `--agent` to a
    /// catalog hit and calls [`Self::set_main_thread_agent`]; `None` otherwise.
    /// Interior-mutable because the binary applies the agent to live session
    /// state AFTER the REPL is constructed — the flag is resolved against the
    /// FINAL catalog (dir + `--agents` + plugin agents) once the orchestrator is
    /// already `Arc`-wrapped. Consumed by [`Self::effective_system_prompt`] (the
    /// agent's prompt becomes the main-loop system prompt, `--system-prompt`
    /// still winning), [`Self::build_wire_tools`] (its `tools:` / `disallowedTools`
    /// frontmatter narrows the advertised tool pool, claude `HJ(agentDef,to,!1,!0)`),
    /// and [`Self::lifecycle_hook_ctx`] (its `agentType` rides every main-thread
    /// lifecycle hook payload, claude-code `wf`/`MVe` `?? MB()`). The agent's
    /// `model` is applied eagerly to the session by [`Self::set_main_thread_agent`],
    /// not stored here.
    pub(crate) main_thread_agent: tokio::sync::RwLock<Option<MainThreadAgentState>>,
    /// Registry bucket that owns the active main-thread agent's frontmatter
    /// hooks. Unlike subagent buckets this normally lives for the session, but
    /// hot resume must replace it so hooks from the previous session cannot
    /// leak into the resumed one.
    pub(crate) main_thread_agent_hook_id: Mutex<Option<protocol::AgentId>>,
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
    /// Running `compactMetadata.cumulativeDroppedTokens` value for this live
    /// session. Claude derives it from prior compact-boundary metadata; the
    /// protocol history intentionally projects that metadata out, so the
    /// orchestrator keeps the equivalent scalar directly.
    pub(crate) compaction_cumulative_dropped_tokens: std::sync::atomic::AtomicU64,
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
    /// The last API response's OUTPUT token count, recorded alongside
    /// [`Self::last_response_input_tokens`] by
    /// [`Self::record_response_input_tokens`].
    ///
    /// Together the two reconstruct claude-code's `hoe(messages)` (2.1.238
    /// @294688350) — the LAST assistant message's
    /// `input + cache_creation + cache_read + output` — which the
    /// `total_tokens_reminder` producer `D3T` (@296556375) feeds to the padded
    /// countdown. `protocol` carries no per-message `usage` object, so the
    /// orchestrator caches the scalar the same way the PTL prefix guard already
    /// caches the input total. `0` until the first successful call.
    pub(crate) last_response_output_tokens: std::sync::atomic::AtomicU64,
    /// `Z3f` (2.1.238 @292021xxx) — the per-agent padded-countdown ledger
    /// behind the `total_tokens_reminder`. Keyed by agent id (`"main"` for this
    /// orchestrator, which is always depth-0). A `std::sync::Mutex` because the
    /// reminder is computed inside the async turn drivers but never held across
    /// an await.
    pub(crate) total_tokens_ledger:
        std::sync::Mutex<crate::prompt::total_tokens::TotalTokensLedger>,
    /// `history.len()` captured at each `silent_turn_reminder` emission.
    ///
    /// The oracle keeps its emitted attachments IN the message list, so `Ezm`
    /// (@296525002) counts `remindersInStretch` by walking them. LingXi's
    /// per-turn reminders are transient (outgoing snapshot only, never
    /// `session.history` / JSONL), so the emission POSITIONS are recorded here
    /// and spliced back into the walk by
    /// [`crate::prompt::silent_turn::scan_silent_stretch`].
    pub(crate) silent_turn_reminder_marks: std::sync::Mutex<Vec<usize>>,
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
    /// 2.1.212 `/fork` (`vAd`) background-session forker. When wired (via
    /// [`Self::with_bg_session_forker`], the CLI composition root's
    /// `CliBgSessionForker`), [`OrchestratorHandle::fork_to_background_session`]
    /// snapshots the live conversation into a NEW background session (the
    /// `--bg`/daemon session-copy path) and returns the system line for the live
    /// session. `None` (tests / non-desktop roots) ⇒ that handle method fails
    /// with a clear `ActionFailed`. Mirrors the `fork_spawner`/`fork_budget`
    /// optional-seam pattern above.
    pub(crate) bg_session_forker: Option<Arc<dyn traits::bg_session_forker::BgSessionForker>>,
    /// Host-owned live catalog reconciler used by `register_repo_root`.
    ///
    /// The orchestrator admits the root into the sandbox and MCP root set
    /// first; the desktop composition root then refreshes the registries it
    /// exclusively owns.
    pub(crate) repo_root_reloader: Option<Arc<dyn traits::RepoRootReloader>>,
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
    /// The ONE per-session read-state registry (`path → {content, mtime_ms,
    /// offset, limit}`) — the 1:1 port of claude-code's single
    /// `context.readFileState` LRU (`FileReadTool.ts:1032`). This is the sole
    /// source of truth for every read-state consumer. Model-context consumers
    /// (`/files`, conditional-rule matching, relevant-memory dedup, and
    /// post-compact restore) read the MODEL-VISIBLE subset; the Read dedup +
    /// staleness guards inside the file tools consume the full shared map,
    /// including non-model host seed snapshots.
    /// The composition root creates ONE map, passes a clone into the file
    /// tools' `BuiltinToolContext`, and shares the SAME `Arc` here via
    /// [`Self::with_read_state_map`] (P1-06), so a tool's `readFileState.set`
    /// (Read/Edit/Write/…) is visible here — 1:1 with claude-code's single
    /// per-session map on the `ToolUseContext`. Tests / binaries without a
    /// composition root keep the constructor's fresh default (unshared, but
    /// harmless — nothing populates it, so consumers observe an empty map).
    ///
    /// CONSUMED post-compact (#59): [`Self::restore_post_compact_attachments`]
    /// snapshots this registry, clears it, and re-attaches the most-recent files
    /// after the compaction boundary (`K2p`/`Pqn`). The staleness guards
    /// (D/E/F) and Read dedup (A) are additional in-tool consumers of the SAME
    /// shared map.
    pub(crate) read_state_map: tool_api::read_file_state::ReadFileStateMap,
    /// Skill contents carried by each post-compact `invoked_skills` message,
    /// keyed by the message's stable identity.
    ///
    /// A skill body is arbitrary Markdown and may legally contain the renderer's
    /// `\n\n---\n\n` separator. Keeping the structured association here avoids
    /// reverse-parsing model-visible text when a later compaction deduplicates
    /// attachments that survived in its preserved tail.
    pub(crate) post_compact_skill_attachments:
        std::sync::Mutex<std::collections::HashMap<MessageId, Vec<String>>>,
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
    /// first time [`Self::conditional_rules_reminder_message`] runs (filled via
    /// the same `memory.load(&cwd)` the system prompt uses, then re-filtered to
    /// `globs.is_some()`). Avoids re-walking disk every turn while still
    /// letting lazy activation re-test the cached rules against the latest
    /// `read_file_state`. `None` = not yet filled OR invalidated; an empty
    /// `Vec` (once filled) means the hierarchy has no conditional rules.
    ///
    /// Task 5 (worktree 206 session-cwd plumbing): this is CWD-DEPENDENT
    /// cached state — the one genuine cache this port keeps keyed by cwd (the
    /// env block / gitStatus / `additional_context_message` all re-derive
    /// fresh every turn instead, so they need no invalidation, only a live cwd
    /// source — see [`Self::session_cwd`]). A plain `std::sync::Mutex` (not
    /// the prior `tokio::sync::OnceCell`) so [`Self::with_session_cwd`] can
    /// register a synchronous [`tool_api::SessionCwd::set_on_swap`] callback
    /// that clears it (`*cache.lock() = None`) on every `EnterWorktree`/
    /// `ExitWorktree` swap, forcing the next turn to re-walk disk under the
    /// new cwd instead of replaying the pre-swap directory's rules forever.
    pub(crate) conditional_rules_cache:
        Arc<std::sync::Mutex<Option<Vec<crate::prompt::MemoryFile>>>>,
    /// §F sent-tracking ("delta"): the paths of conditional rules already
    /// injected this session, so each rule is rendered ONCE when first activated
    /// and never re-injected on later turns. 1:1 with TS `loadedNestedMemoryPaths`
    /// (attachments.ts:1722-1732 — a non-evicting Set keyed by rule path).
    pub(crate) sent_conditional_rules: Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// Nested-memory sent-tracking — 1:1 with the oracle's
    /// `loadedNestedMemoryPaths` as `k$o` (@237714543) uses it:
    /// `if(t.loadedNestedMemoryPaths?.[i.path])continue`. Session-lifetime and
    /// non-evicting, so each discovered memory file is surfaced ONCE.
    ///
    /// This is the ONLY dedup state the feature keeps.
    /// [`crate::prompt::nested_memory::discover`] is deliberately stateless
    /// (the oracle's `seen` is per-call), so a `LINGXI.md` written mid-session
    /// is still found — it is this set, not the walk, that stops re-sending.
    ///
    /// NOT cleared on a worktree swap, unlike
    /// [`Self::conditional_rules_cache`]: that is a CACHE (stale after a swap),
    /// while this is a record of what the model has already been told, which a
    /// change of cwd does not undo.
    pub(crate) sent_nested_memory: Mutex<std::collections::HashSet<std::path::PathBuf>>,
    /// Test-only override for the two filesystem roots nested-memory discovery
    /// needs: `(home, managed_dir)`. `None` (production) resolves them exactly
    /// as `RealMemoryHierarchyProvider::load` does — `dirs::home_dir()` and
    /// `hierarchy::managed_path()`.
    ///
    /// Discovery probes `<home>/<config-dir>/rules` on every call, so without
    /// an override a test would read the developer's REAL user memory and its
    /// result would depend on the machine it ran on. Mirrors
    /// `StaticMemoryProvider`'s role for the eager block. Set via
    /// [`Self::with_nested_memory_roots`].
    pub(crate) nested_memory_roots: Option<(std::path::PathBuf, Option<std::path::PathBuf>)>,
    /// SKILLLIST.1 delta: skill names already emitted in a prior turn's
    /// `skill_listing` reminder. Turn-0 emits the FULL listing; later turns emit
    /// ONLY newly-appeared skills (mirrors TS `sentSkillNames` per-agent delta,
    /// attachments.ts:2607/2699). When no new skill appears,
    /// [`Self::skill_listing_reminder_message`] returns `None` (no reminder that
    /// turn). Process-/session-local, exactly like the TS module-scope map.
    pub(crate) sent_skill_names: Mutex<std::collections::HashSet<String>>,
    /// `date_change` (cc `Cop`) per-session state. See [`DateChangeState`].
    pub(crate) date_change: std::sync::Mutex<DateChangeState>,
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
    /// [`Self::relevant_memory_reminder_messages`] are strict no-ops, keeping the
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
    /// [`Self::relevant_memory_reminder_messages`] before snapshot assembly.
    /// `None` between turns / when no prefetch is wired. Mirrors the
    /// pending-handle slot pattern of the recovery / cache-safe slots.
    pub(crate) pending_memory_prefetch: Mutex<Option<memory::prefetch::PendingMemoryPrefetch>>,
    /// P0.1 surfacing dedup: paths already surfaced via the
    /// `relevant_memories` channel this session, so a memory surfaced once is
    /// never re-injected on a later turn. Mirrors [`Self::sent_conditional_rules`]
    /// (TS `loadedNestedMemoryPaths` / the prefetch's per-iteration consume
    /// guard). Distinct from [`Self::read_state_map`], which the SHARED dedup
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
    /// User-approved app Agent Profile applied additively to the next turn.
    pub(crate) app_agent_prompt_profile: std::sync::RwLock<Option<AppAgentPromptProfile>>,
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

/// Recursively convert SDK snake_case compact-metadata keys to the JSONL
/// lowerCamelCase spelling. Values and already-camelCase keys are preserved.
fn camelize_json_keys(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .into_iter()
                .map(|(key, value)| {
                    let mut parts = key.split('_');
                    let mut camel = parts.next().unwrap_or_default().to_string();
                    for part in parts {
                        let mut chars = part.chars();
                        if let Some(first) = chars.next() {
                            camel.extend(first.to_uppercase());
                            camel.extend(chars);
                        }
                    }
                    (camel, camelize_json_keys(value))
                })
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(camelize_json_keys).collect())
        }
        scalar => scalar,
    }
}

impl ConversationOrchestrator {
    /// `tengu_api_success` `timeSinceLastApiCallMs`: ms since the previous
    /// successful API call, then record this call's timestamp. Returns `None`
    /// on the first call (claude `W=G!==null?Math.max(0,Math.round(M-G)):void 0`).
    #[allow(clippy::cast_sign_loss)]
    pub(crate) fn record_api_call_gap_ms(&self) -> Option<u64> {
        use std::sync::atomic::Ordering;
        let now_ms = i64::try_from(
            self.session_started_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .elapsed()
                .as_millis(),
        )
        .unwrap_or(i64::MAX);
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
        // `is_non_interactive_session == !interactive_session`: interactive CLI
        // prompt/request semantics are distinct from the permission-gate
        // transport, so TUI/REPL sessions can carry interactive guidance even
        // when `interactive_permissions` remains on the headless default.
        traits::session_flags::set_non_interactive_session(!config.interactive_session);
        // Publish the session-scoped tool-search gate (Claude Code `$U()`) so the
        // request builder branches `tool_reference` normalization on the SESSION
        // decision, not on whether a given request's toolset carries a
        // `ToolSearch` declaration. This init value uses no resolved profile yet
        // (the CLI resolves it post-construction via `switch_model`); the
        // per-request assembly refreshes it once the model/profile are known, so
        // any side query fired before the first main-loop turn still sees the
        // session's mode+provider decision rather than the bare `false` default.
        traits::session_flags::set_tool_search_enabled(
            tools.deferral().mode().is_enabled()
                && tool_search_supported_for_request(&config.model, None),
        );
        let session = SessionState::empty(SessionId::new(), config.model.clone());
        let current_effort = config.effort.clone();
        let current_reasoning_selection = current_effort
            .as_ref()
            .map(|effort| traits::ReasoningSelection::Level { id: effort.clone() })
            .unwrap_or(traits::ReasoningSelection::Automatic);
        let current_effort_explicit = current_effort.is_some();
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
            turn_gate: Mutex::new(()),
            current_effort: std::sync::RwLock::new(current_effort),
            current_reasoning_selection: std::sync::RwLock::new(current_reasoning_selection),
            current_effort_explicit: std::sync::atomic::AtomicBool::new(current_effort_explicit),
            memory,
            current_cwd: Arc::new(std::sync::Mutex::new(cwd.clone())),
            session_cwd: tool_api::SessionCwd::new(cwd.clone(), vec![cwd.clone()]),
            prompt_probe_cwd_resolver: None,
            mobile_runtime_environment_message: None,
            mobile_runtime_environment: None,
            mobile_workspace_cwd_resolver: None,
            cwd,
            config_home: None,
            workspace_trusted: true,
            hooks_restricted: false,
            jsonl_writer: None,
            last_jsonl_uuid: Mutex::new(None),
            tool_denial_kinds: Mutex::new(std::collections::HashMap::new()),
            tool_frames: Mutex::new(None),
            tool_use_results: Mutex::new(std::collections::HashMap::new()),
            tool_use_mcp_meta: Mutex::new(std::collections::HashMap::new()),
            tool_source_assistant_uuids: Mutex::new(std::collections::HashMap::new()),
            pending_hook_attachments: Mutex::new(std::collections::HashMap::new()),
            git_branch_cache: Mutex::new(None),
            current_prompt_id: Mutex::new(None),
            should_exit: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            fast_mode: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            refusal_fallback_latched: std::sync::atomic::AtomicBool::new(false),
            refusal_tried_models: Mutex::new(Vec::new()),
            refusal_episode: Mutex::new(crate::refusal_notice::RefusalEpisode::default()),
            refusal_notice_queue: Mutex::new(crate::refusal_notice::NoticeQueue::new()),
            cost_tracker: None,
            analytics_bus: None,
            session_started_at: std::sync::Mutex::new(std::time::Instant::now()),
            api_calls_recorded: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            last_api_call_at_ms: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(-1)),
            mcp_registry: None,
            hook_registry: None,
            output_style_registry: None,
            wire_tool_schema_cache: Mutex::new(None),
            deferred_tool_token_cache: Mutex::new(std::collections::HashMap::new()),
            agent_catalog: None,
            main_thread_agent: tokio::sync::RwLock::new(None),
            main_thread_agent_hook_id: Mutex::new(None),
            compaction: None,
            compaction_tracking: Mutex::new(compaction::AutoCompactTrackingState::default()),
            compaction_cumulative_dropped_tokens: std::sync::atomic::AtomicU64::new(0),
            last_response_input_tokens: std::sync::atomic::AtomicU64::new(0),
            last_response_output_tokens: std::sync::atomic::AtomicU64::new(0),
            total_tokens_ledger: std::sync::Mutex::new(
                crate::prompt::total_tokens::TotalTokensLedger::default(),
            ),
            silent_turn_reminder_marks: std::sync::Mutex::new(Vec::new()),
            output_token_pool: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            turn_start_output_baseline: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache_safe_slot: None,
            new_diagnostics_source: None,
            current_turn_system_prompt: Mutex::new(None),
            fork_spawner: None,
            fork_budget: None,
            bg_session_forker: None,
            repo_root_reloader: None,
            recap_runner: None,
            file_history: None,
            orphan_forced_decisions: Mutex::new(std::collections::HashMap::new()),
            read_state_map: tool_api::read_file_state::new_read_file_state_map(),
            post_compact_skill_attachments: std::sync::Mutex::new(std::collections::HashMap::new()),
            last_emitted_rate_limit: Mutex::new(None),
            last_emitted_raw_utilization: Mutex::new(None),
            skill_listing: None,
            async_hook_responses: None,
            task_notifications: None,
            stop_hook_snapshot: None,
            mid_turn_input: std::sync::OnceLock::new(),
            cancel_reason: std::sync::OnceLock::new(),
            todo_reminder_tasks: None,
            conditional_rules_cache: Arc::new(std::sync::Mutex::new(None)),
            sent_conditional_rules: Mutex::new(std::collections::HashSet::new()),
            sent_nested_memory: Mutex::new(std::collections::HashSet::new()),
            nested_memory_roots: None,
            sent_skill_names: Mutex::new(std::collections::HashSet::new()),
            date_change: std::sync::Mutex::new(DateChangeState::default()),
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
            app_agent_prompt_profile: std::sync::RwLock::new(None),
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

    /// Share the per-session read-file-state registry (claude-code's
    /// `readFileState` map) with the file tools. Builder-style — the
    /// composition root creates ONE
    /// [`tool_api::read_file_state::ReadFileStateMap`], passes a clone into the
    /// file tools' [`tool_api::BuiltinToolContext`], and hands the SAME `Arc`
    /// here, so a tool's `readFileState.set` (Read/Edit/Write/NotebookEdit) is
    /// visible to the orchestrator's post-compact restore, `/files`, and the
    /// staleness consumers — 1:1 with claude-code's single per-session map on
    /// the `ToolUseContext` (P1-06). Overwrites the fresh, unshared default the
    /// constructor allocated. Wired at the desktop + mobile composition roots;
    /// tests that need a live registry can call this with a map they also seed.
    #[must_use]
    pub fn with_read_state_map(mut self, map: tool_api::read_file_state::ReadFileStateMap) -> Self {
        self.read_state_map = map;
        self
    }

    /// Seed the live read-state cache from an SDK host's `seed_read_state`
    /// control request.
    ///
    /// Claude Code accepts the host snapshot only when the file is at most
    /// 10 MiB and its floor-truncated on-disk mtime is no newer than the
    /// supplied mtime. Failures are intentionally swallowed by the control
    /// protocol; the boolean is exposed solely so callers and tests can observe
    /// whether an entry was installed.
    pub async fn seed_read_state_from_host(&self, path: &str, host_mtime_ms: f64) -> bool {
        const MAX_SEED_BYTES: u64 = 10 * 1024 * 1024;

        let requested = std::path::PathBuf::from(path);
        let absolute = if requested.is_absolute() {
            requested
        } else {
            self.session_cwd.cwd().join(requested)
        };
        let absolute = crate::turn_loop::normalize_lexically(&absolute);

        let Ok(metadata) = tokio::fs::metadata(&absolute).await else {
            return false;
        };
        if metadata.len() > MAX_SEED_BYTES {
            return false;
        }
        let Ok(modified) = metadata.modified() else {
            return false;
        };
        let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH) else {
            return false;
        };
        let mtime_ms = since_epoch.as_millis().min(i64::MAX as u128) as i64;
        if (mtime_ms as f64) > host_mtime_ms.floor() {
            return false;
        }
        let Ok(content) = tokio::fs::read_to_string(&absolute).await else {
            return false;
        };
        let content = content
            .strip_prefix('\u{feff}')
            .unwrap_or(&content)
            .replace("\r\n", "\n")
            .replace('\r', "\n");

        tool_api::read_file_state::set_with_model_context(
            &self.read_state_map,
            absolute,
            tool_api::read_file_state::ReadFileEntry {
                content,
                mtime_ms,
                offset: None,
                limit: None,
                // The host explicitly marks this content as not present in the
                // model context. `from_read = false` preserves that invariant:
                // the next Read must not deduplicate against this seed.
                from_read: false,
                seeded_from_context: false,
                is_partial_view: false,
            },
            false,
        );
        true
    }

    /// Seed the loaded memory files (LINGXI.md + `.lingxi/rules/**`) into the
    /// shared read-state registry — the port of claude-code's startup loop
    /// `xCt` (2.1.220 @245883373):
    ///
    /// ```text
    /// for(let Fr of yt){
    ///   if(tHt(Fr.path))continue;
    ///   let jn=MLu(Fr),Ao;
    ///   try{Ao=jn?FQ(Fr.path):Date.now()}catch{Ao=Date.now()}
    ///   vM.current.set(Fr.path,{
    ///     content: Fr.contentDiffersFromDisk?Fr.rawContent??Fr.content:X9(Fr.content),
    ///     timestamp: Ao, offset:void 0, limit:void 0,
    ///     isPartialView: Fr.contentDiffersFromDisk,
    ///     seededFromContext: jn,
    ///     ...!jn&&{contentNotInModelContext:!0},
    ///     keepContent:!0}), …}
    /// ```
    ///
    /// This is what makes the Read tool's seeded dedup branch
    /// (`tool_file::read`, @235741459) reachable: the model already received
    /// these files' bodies in the system prompt's memory block, so re-`Read`ing
    /// one returns `FILE_UNCHANGED_SEEDED_PREFIX` instead of a second copy.
    ///
    /// Per-file decisions, each matching the oracle:
    /// - **Skip when already present.** `!readFileState.has(path)` (the site-2
    ///   guard @237715046). Uses the NON-promoting
    ///   [`tool_api::read_file_state::ReadFileStateLru::contains`] so seeding
    ///   never reshuffles LRU order, and so a file the model genuinely `Read`
    ///   is never overwritten by a seed. This is also what makes the
    ///   `reason = Compact` re-entry into
    ///   [`Self::fire_instructions_loaded_with_reason`] idempotent.
    /// - **`seeded_from_context = MLu(f)`** =
    ///   [`crate::prompt::memory_block::is_rendered_into_context`], derived from
    ///   the renderer so the two cannot drift. A `paths:`-gated conditional rule
    ///   is NOT rendered, so it seeds with `false` — its content is NOT in
    ///   context and must never dedup.
    /// - **`mtime_ms`** = the on-disk mtime for a rendered file (`FQ(path)`),
    ///   `Date.now()` otherwise. ANY stat error falls back to `now` — the
    ///   oracle wraps the whole thing in `try{…}catch{Ao=Date.now()}`, so this
    ///   never propagates and never panics.
    /// - **`content`**. LingXi-local adaptation, deliberate: the oracle
    ///   normalizes the non-differing branch with `X9` (BOM strip + CRLF→LF),
    ///   but LingXi's `Read` stores `decode_utf8_strict`
    ///   (`tools/file/src/shared.rs` — BOM strip ONLY, no CRLF collapse) and
    ///   `edit.rs` feeds that same raw-decoded form to the staleness
    ///   comparator. Collapsing CRLF here would make every CRLF memory file
    ///   fail `check_read_before_write`'s content-equality fallback forever.
    ///   The oracle's asymmetry IS preserved: normalize only on the
    ///   `!differs` branch; store `raw_content` verbatim on the `differs`
    ///   branch.
    /// - **`in_model_context`** carries the `...!jn && {contentNotInModelContext:!0}`
    ///   spread: LingXi's existing LRU-slot flag is that field's inverted
    ///   analog, already consumed by `model_context_keys()` /
    ///   `drain_model_context()`, so no entry-level twin is introduced.
    ///
    /// # Divergence (reason)
    /// The oracle also skips sentinel paths via `tHt` (`"<policyHelper>"`
    /// @226886661 / `"<managed-settings>"` @230811638). LingXi's loader emits
    /// only real walked filesystem paths — verified, no analog exists — so the
    /// skip is omitted rather than approximated with a `starts_with('<')`
    /// heuristic, which would be WIDER than the oracle.
    ///
    /// The oracle's AutoMem eviction pass (@245883373-655: delete seeded
    /// entries once memory-stores mode latches on) likewise has no LingXi
    /// analog — `ARe()` / `CLAUDE_MEMORY_STORES` is unported and there is no
    /// `AutoMem` tier. No latch is invented here.
    async fn seed_memory_read_state(&self, files: &[crate::prompt::MemoryFile]) {
        for f in files {
            // The registry is keyed by CANONICAL paths: `FileReadTool` looks up
            // `canonicalize_and_validate(..)`'s output. The memory hierarchy's
            // `f.path` is built from the orchestrator's cwd verbatim, symlinks
            // and all — on macOS a `/var/...` cwd resolves to `/private/var/...`
            // — so seeding under the raw path silently never matches and the
            // dedup simply never fires. Canonicalize once here and use it for
            // BOTH the `has` guard and the key, or the two halves disagree.
            //
            // Falls back to the raw path if canonicalization fails; the file was
            // just read by the loader, so that is close to unreachable, and the
            // fallback is no worse than not seeding at all.
            let key = tokio::fs::canonicalize(&f.path)
                .await
                .unwrap_or_else(|_| f.path.clone());
            // `!readFileState.has(path)` — never clobber a real Read, never
            // MRU-promote (see `contains`' doc).
            if self
                .read_state_map
                .lock()
                .is_ok_and(|guard| guard.contains(&key))
            {
                continue;
            }
            let in_context = crate::prompt::memory_block::is_rendered_into_context(f);
            let now_ms = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis()),
            )
            .unwrap_or(i64::MAX);
            // `try{Ao = jn ? FQ(Fr.path) : Date.now()}catch{Ao = Date.now()}`.
            let mtime_ms = if in_context {
                match tokio::fs::metadata(&f.path)
                    .await
                    .and_then(|m| m.modified())
                {
                    Ok(t) => tool_api::read_file_state::mtime_ms_floor(t),
                    Err(_) => now_ms,
                }
            } else {
                now_ms
            };
            // `contentDiffersFromDisk ? (rawContent ?? content) : X9(content)`,
            // with LingXi's BOM-strip-only normalization (see the doc above).
            let content = if f.content_differs_from_disk {
                f.raw_content.clone()
            } else {
                f.raw_content
                    .strip_prefix('\u{feff}')
                    .unwrap_or(&f.raw_content)
                    .to_string()
            };
            tool_api::read_file_state::set_with_model_context(
                &self.read_state_map,
                key,
                tool_api::read_file_state::ReadFileEntry {
                    content,
                    mtime_ms,
                    offset: None,
                    limit: None,
                    // A seed is not a Read; the non-seeded dedup branch must
                    // never fire off it.
                    from_read: false,
                    seeded_from_context: in_context,
                    is_partial_view: f.content_differs_from_disk,
                },
                // ALWAYS false — deliberately NOT `in_context`.
                //
                // Two different axes that an earlier draft conflated:
                //   * `seeded_from_context` (above) is the oracle's `jn`/`MLu`
                //     and gates ONLY the seeded dedup stub.
                //   * this flag is LingXi's post-compact RESTORE set
                //     (`drain_model_context` -> `restore_post_compact_attachments`)
                //     and the "files touched this turn" input to
                //     `conditional_rules_reminder_message`.
                //
                // A memory file is re-injected by the SYSTEM PROMPT on every
                // turn, so enrolling it here would re-attach LINGXI.md after
                // every compaction and report it as touched. It belongs with the
                // host-seeded snapshots the drain doc already excludes.
                false,
            );
        }
    }

    /// Seed the JSONL parent-uuid chain pointer so the FIRST append after a
    /// resume chains via `parent_uuid` off the resumed transcript's tail
    /// (matching the M5-07 writer's chain semantics). Used by the CLI's
    /// resume-into-TUI seed alongside adopting the resumed history + id; without
    /// it the first appended message would be a chain orphan (recoverable, but
    /// this keeps the on-disk chain linear).
    /// Emit a `tool_result` SDK frame, or buffer it when the streaming driver
    /// has ordering active. Every dispatch-side emission goes through here.
    pub(crate) async fn emit_tool_result_frame(
        &self,
        id: &protocol::ToolUseId,
        tool: &str,
        model_text: &str,
        result: &serde_json::Value,
        denial_kind: Option<&str>,
    ) {
        if let Some(buf) = self.tool_frames.lock().await.as_mut() {
            buf.insert(
                id.to_string(),
                PendingToolFrame {
                    tool: tool.to_string(),
                    model_text: model_text.to_string(),
                    result: result.clone(),
                    denial_kind: denial_kind.map(str::to_string),
                },
            );
            return;
        }
        match denial_kind {
            Some(kind) => {
                self.output
                    .emit_tool_result_denied(id, tool, model_text, result, kind)
                    .await;
            }
            None => {
                self.output
                    .emit_tool_result(id, tool, model_text, result)
                    .await
            }
        }
    }

    /// Turn frame buffering on for the streaming driver, and off again.
    pub(crate) async fn set_tool_frame_buffering(&self, on: bool) {
        let mut slot = self.tool_frames.lock().await;
        *slot = on.then(std::collections::HashMap::new);
    }

    /// Release one buffered frame, in the CALLER's order.
    ///
    /// `content` is the block's FINAL model-facing text, so a synthetic that
    /// replaced a cancelled tool's real outcome wins over whatever the dispatch
    /// buffered. A tool that never dispatched (queued, then cancelled) has no
    /// buffered frame and still gets one, which is the case that previously
    /// emitted nothing at all.
    pub(crate) async fn release_tool_frame(
        &self,
        id: &protocol::ToolUseId,
        content: &str,
        is_error: bool,
    ) {
        let pending = self
            .tool_frames
            .lock()
            .await
            .as_mut()
            .and_then(|b| b.remove(&id.to_string()));
        let (tool, result, denial_kind) = match pending {
            // A substitution replaced the model-facing text, so the buffered
            // payload describes an outcome that was DISCARDED. claude-code's
            // synthetic carries a synthetic `toolUseResult` too, so the real
            // one must not reach the SDK.
            Some(p) if p.model_text != content => (
                p.tool,
                serde_json::json!({ "error": content }),
                p.denial_kind,
            ),
            Some(p) => (p.tool, p.result, p.denial_kind),
            // Never dispatched: synthesize the payload the dispatch would have
            // carried, matching the shape used by every other error result.
            None => (String::new(), serde_json::json!({ "error": content }), None),
        };
        let result = if is_error && !result.is_object() {
            serde_json::json!({ "error": content })
        } else {
            result
        };
        match denial_kind {
            Some(kind) => {
                self.output
                    .emit_tool_result_denied(id, &tool, content, &result, &kind)
                    .await;
            }
            None => {
                self.output
                    .emit_tool_result(id, &tool, content, &result)
                    .await
            }
        }
    }

    /// Record the `toolDenialKind` for a tool that was denied rather than run,
    /// so its `tool_result` user line carries the provenance when persisted.
    ///
    /// Values are claude's: `user-rejected`, `permission-rule`,
    /// `automode-blocked`, `automode-unavailable`, `automode-parsing-error`,
    /// plus the abort kinds `cancelled` / `interrupted`.
    pub(crate) async fn record_tool_denial_kind(&self, id: &protocol::ToolUseId, kind: &str) {
        self.tool_denial_kinds
            .lock()
            .await
            .insert(id.to_string(), kind.to_string());
    }

    /// Drop a recorded kind whose block never reaches persistence.
    ///
    /// Needed because `take_tool_denial_kind` REMOVES on read: when the
    /// streaming executor substitutes a synthetic result for a tool that
    /// already recorded a kind at dispatch, the recorded kind must either be
    /// rewritten to the synthetic's own kind or dropped, or it would both stamp
    /// the wrong provenance and leak an entry for the rest of the session.
    pub(crate) async fn remove_tool_denial_kind(&self, id: &protocol::ToolUseId) {
        self.tool_denial_kinds.lock().await.remove(&id.to_string());
    }

    /// Take the recorded kind for a message carrying EXACTLY ONE `tool_result`.
    ///
    /// The single-block guard is claude's own (`Tpr`): a user message with zero
    /// or several tool_results cannot attribute one message-level kind, so it
    /// gets none. Taking (rather than reading) keeps a denial from stamping a
    /// second line if the same result were ever persisted twice.
    async fn take_tool_denial_kind(&self, msg: &ConversationMessage) -> Option<String> {
        let only = Self::sole_tool_result_id(msg)?;
        self.tool_denial_kinds.lock().await.remove(&only)
    }

    /// claude's `Tpr` guard, factored out so every tool-result head key shares
    /// ONE definition: the id of the message's `tool_result` block when it
    /// carries EXACTLY ONE, else `None`.
    fn sole_tool_result_id(msg: &ConversationMessage) -> Option<String> {
        let ConversationMessage::User { content, .. } = msg else {
            return None;
        };
        let mut results = content.iter().filter_map(|b| match b {
            protocol::ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id),
            _ => None,
        });
        match (results.next(), results.next()) {
            (Some(only), None) => Some(only.to_string()),
            _ => None,
        }
    }

    /// Record a tool's `toolUseResult` payload for its `tool_result` user line.
    ///
    /// `data` is claude's `se.data` on success (2.1.220 BIN off 235420375) —
    /// the RAW structured result, not the model-facing string — or the plain
    /// string `` `Error: ${message}` `` on the error/denial arms
    /// (BIN off 235424595 / 235400200 / 232972524 / …).
    pub(crate) async fn record_tool_use_result(
        &self,
        id: &protocol::ToolUseId,
        data: serde_json::Value,
    ) {
        self.tool_use_results
            .lock()
            .await
            .insert(id.to_string(), data);
    }

    /// Record an MCP tool's `mcpMeta` for its `tool_result` user line
    /// (2.1.220 BIN off 232969604 — verbatim on the main chain).
    pub(crate) async fn record_tool_use_mcp_meta(
        &self,
        id: &protocol::ToolUseId,
        meta: serde_json::Value,
    ) {
        self.tool_use_mcp_meta
            .lock()
            .await
            .insert(id.to_string(), meta);
    }

    /// Record the ASSISTANT line uuid that carried this `tool_use` block —
    /// claude's `sourceToolAssistantUUID`.
    pub(crate) async fn record_source_tool_assistant_uuid(
        &self,
        id: &protocol::ToolUseId,
        assistant_line_uuid: String,
    ) {
        self.tool_source_assistant_uuids
            .lock()
            .await
            .insert(id.to_string(), assistant_line_uuid);
    }

    /// Queue one hook `attachment` payload produced while dispatching `id`.
    ///
    /// Flushed by [`Self::flush_hook_attachments`] right after that tool's
    /// `tool_result` line is written, which is where claude's own stream order
    /// puts it.
    pub(crate) async fn queue_hook_attachment(
        &self,
        id: &protocol::ToolUseId,
        payload: serde_json::Value,
    ) {
        self.pending_hook_attachments
            .lock()
            .await
            .entry(id.to_string())
            .or_default()
            .push(payload);
    }

    /// Remove and return the payloads queued for `id`, in production order.
    pub(crate) async fn take_queued_hook_attachments(
        &self,
        id: &protocol::ToolUseId,
    ) -> Vec<serde_json::Value> {
        self.pending_hook_attachments
            .lock()
            .await
            .remove(&id.to_string())
            .unwrap_or_default()
    }

    /// Persist (and drain) every attachment queued for `id`.
    pub(crate) async fn flush_hook_attachments(&self, id: &protocol::ToolUseId) {
        for payload in self.take_queued_hook_attachments(id).await {
            self.persist_hook_attachment_to_jsonl(payload).await;
        }
    }

    /// Take the recorded `toolUseResult` under the same single-block guard.
    async fn take_tool_use_result(&self, msg: &ConversationMessage) -> Option<serde_json::Value> {
        let only = Self::sole_tool_result_id(msg)?;
        self.tool_use_results.lock().await.remove(&only)
    }

    /// Take the recorded `mcpMeta` under the same single-block guard.
    async fn take_tool_use_mcp_meta(&self, msg: &ConversationMessage) -> Option<serde_json::Value> {
        let only = Self::sole_tool_result_id(msg)?;
        self.tool_use_mcp_meta.lock().await.remove(&only)
    }

    /// Take the recorded `sourceToolAssistantUUID` under the same guard.
    ///
    /// Emitted ONLY on a map HIT: `run_turn`'s parent derivation falls back to
    /// the turn's last assistant block uuid when the id is missing (a
    /// defensive, in-practice-unreachable branch), and writing a uuid that is
    /// not the tool_use's own line would be worse than omitting the key.
    async fn take_source_tool_assistant_uuid(&self, msg: &ConversationMessage) -> Option<String> {
        let only = Self::sole_tool_result_id(msg)?;
        self.tool_source_assistant_uuids.lock().await.remove(&only)
    }

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

    /// Wire the resolved workspace-trust state for `/goal`.
    #[must_use]
    pub fn with_workspace_trusted(mut self, trusted: bool) -> Self {
        self.workspace_trusted = trusted;
        self
    }

    /// Wire the resolved hook-policy restriction state for `/goal`.
    #[must_use]
    pub fn with_hooks_restricted(mut self, restricted: bool) -> Self {
        self.hooks_restricted = restricted;
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

    /// Share the composition root's switchable [`tool_api::SessionCwd`] — the
    /// SAME `Arc` handed to `BuiltinToolContext`, which `EnterWorktree`/
    /// `ExitWorktree` (Task 6-8 of the worktree-206 plan) swap. Builder-style;
    /// wired at the desktop/mobile composition roots.
    ///
    /// Registers a synchronous `set_on_swap` callback (Task 5) that clears
    /// [`Self::conditional_rules_cache`] — the one genuine CWD-keyed cache this
    /// port keeps — on every swap, so the next turn re-walks the memory
    /// hierarchy under the new cwd instead of replaying the pre-swap
    /// directory's conditional rules for the rest of the session. Every other
    /// per-turn cwd-dependent section (env block, gitStatus,
    /// `additional_context_message`) is recomputed from scratch each turn, so
    /// switching them onto this live cell (done in [`Self::build_prompt_context`]
    /// and friends) is enough on its own — no cache to clear there.
    ///
    /// Without this call the default private `SessionCwd` (over the
    /// constructor's static `cwd`, never swapped) stands, so every cwd-derived
    /// section reads the boot cwd exactly as before — the INERT INVARIANT.
    #[must_use]
    pub fn with_session_cwd(mut self, session_cwd: Arc<tool_api::SessionCwd>) -> Self {
        let cache = Arc::clone(&self.conditional_rules_cache);
        session_cwd.set_on_swap(Box::new(move |_new_cwd| {
            if let Ok(mut guard) = cache.lock() {
                *guard = None;
            }
        }));
        self.session_cwd = session_cwd;
        self
    }

    /// PathAtlas S3 (mobile-linux): resolve the model-visible session cwd to
    /// the HOST directory the prompt probes should run against.
    ///
    /// When the session cwd is a guest path (`/workspace/<id>`), the env
    /// block must DISPLAY it verbatim while the memory hierarchy, git-status
    /// probe, and file tree must read the host directory that backs it. This
    /// resolver performs that guest→host hop in [`Self::build_prompt_context`];
    /// unset (every desktop caller), probes run on the session cwd itself —
    /// byte-identical to before.
    #[must_use]
    pub fn with_prompt_probe_cwd_resolver(
        mut self,
        resolver: Arc<dyn Fn(&std::path::Path) -> std::path::PathBuf + Send + Sync>,
    ) -> Self {
        self.prompt_probe_cwd_resolver = Some(resolver);
        self
    }

    /// Attach the stable mobile host/tool-runtime snapshot for this engine.
    ///
    /// Rendering happens exactly once here. The snapshot describes the stable
    /// configured runtime; registered tool schemas remain authoritative for
    /// each agent's effective `Shell` availability.
    #[must_use]
    pub fn with_mobile_runtime_environment(
        mut self,
        environment: traits::mobile_runtime_environment::MobileRuntimeEnvironment,
    ) -> Self {
        self.mobile_runtime_environment_message = Some(ConversationMessage::user_meta(
            MessageId::new(),
            environment.render_system_reminder(),
        ));
        self.mobile_runtime_environment = Some(environment);
        self
    }

    /// Attach the model-visible mobile cwd mapper. Host-native backing paths
    /// that are not covered by a guest mount are omitted by the environment's
    /// sanitizer rather than exposed to the model.
    #[must_use]
    pub fn with_mobile_workspace_cwd_resolver(
        mut self,
        resolver: Arc<dyn Fn(&std::path::Path) -> Option<String> + Send + Sync>,
    ) -> Self {
        self.mobile_workspace_cwd_resolver = Some(resolver);
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

    /// Attach the host's live plugin output-style registry.
    #[must_use]
    pub fn with_output_style_registry(
        mut self,
        styles: Arc<tokio::sync::RwLock<outputstyles::OutputStyleRegistry>>,
    ) -> Self {
        self.output_style_registry = Some(styles);
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
    /// strict no-op ([`Self::relevant_memory_reminder_messages`] returns empty),
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
    /// wired source and inject it as a plain (non-meta) user message so the next
    /// sampling sees it — CC 2.1.207's `queued_command` guard leaves plain human
    /// input non-meta (`r!==void 0&&!Ree(r)||e.isMeta` → `{}`). A strict no-op
    /// when no source is wired (the default) or the queue
    /// is empty. Returns `true` if anything was injected (for the caller's
    /// observability — the loop continues regardless). Mirrors claude-code's
    /// `joinPromptValues` + meta-prompt injection at query.ts ~1570-1580.
    async fn drain_mid_turn_input(&self) -> bool {
        let mut injected = self.drain_peer_inbox(true).await;
        let Some(source) = self.mid_turn_input.get() else {
            return injected;
        };
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
                    self.inject_user_message(&wrapped).await;
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

    /// Attach the 2.1.212 `/fork` (`vAd`) background-session forker (the CLI
    /// composition root's `CliBgSessionForker`), so
    /// [`OrchestratorHandle::fork_to_background_session`] can copy the live
    /// conversation into a new background session. Without it, that `/fork`
    /// variant fails with a clear `ActionFailed`.
    #[must_use]
    pub fn with_bg_session_forker(
        mut self,
        forker: Arc<dyn traits::bg_session_forker::BgSessionForker>,
    ) -> Self {
        self.bg_session_forker = Some(forker);
        self
    }

    /// Attach the composition-root catalog reconciler used by
    /// [`Self::register_repo_root`].
    #[must_use]
    pub fn with_repo_root_reloader(mut self, reloader: Arc<dyn traits::RepoRootReloader>) -> Self {
        self.repo_root_reloader = Some(reloader);
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

    /// Claude Code 2.1.217's `rename_generate_name` prompt. The response is a
    /// JSON object with a single `name` field; this side query is history-inert
    /// and tool-less, sharing the same fork runner as `/recap`.
    pub(crate) const SESSION_NAME_PROMPT: &str = "Generate a short kebab-case name (2-4 words) that captures the main topic of this conversation. Use lowercase words separated by hyphens. Examples: \"fix-login-bug\", \"add-auth-feature\", \"refactor-api-client\", \"debug-test-failures\". Return JSON with a \"name\" field.";

    pub(crate) async fn generate_session_name_query(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Option<String>, traits::HandleError> {
        let runner = self.recap_runner.clone().ok_or_else(|| {
            traits::HandleError::ActionFailed("session name generation unavailable".into())
        })?;
        let Some(params) = self
            .cache_safe_slot
            .as_ref()
            .ok_or_else(|| {
                traits::HandleError::ActionFailed("session name generation unavailable".into())
            })?
            .get_last()
            .await
        else {
            return Ok(None);
        };

        if cancel.is_cancelled() {
            return Ok(None);
        }
        let req = sidequery::ForkedAgentRequest {
            prompt_messages: vec![ConversationMessage::user(
                MessageId::new(),
                Self::SESSION_NAME_PROMPT.to_string(),
            )],
            cache_safe_params: params,
            fork_label: "rename".into(),
            query_source: sidequery::QuerySource::Custom("rename_generate_name".into()),
            max_output_tokens: Some(128),
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(None),
            result = runner.run(req) => match result {
                Ok(result) => parse_generated_session_name(&result.final_text)
                    .map(Some)
                    .ok_or_else(|| traits::HandleError::ActionFailed(
                        "session name response did not contain a non-empty name".into()
                    )),
                Err(error) => Err(traits::HandleError::ActionFailed(error.to_string())),
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
        let (fork_context_messages, session_id) = {
            let s = self.session.lock().await;
            (s.history.clone(), s.session_id)
        };
        let transcript_path = self
            .jsonl_writer
            .as_ref()
            .map(|writer| writer.active_path())
            .unwrap_or_else(|| self.computed_transcript_path(&session_id));
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
            transcript_path: (!transcript_path.as_os_str().is_empty()).then_some(transcript_path),
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
                info.upgrade_paths.as_deref(),
                info.credits_required,
            )
            .await;
        *last = Some(info);
    }

    /// Task 2 (llm-client future-work batch 5): forward the API client's
    /// latest RAW per-window utilization snapshot to
    /// [`traits::OutputStream::emit_raw_utilization`] when it CHANGED since
    /// the last emit. The empty snapshot is significant: it clears a
    /// previously-rendered utilization window in downstream clients.
    ///
    /// Called immediately next to [`Self::emit_rate_limit_if_changed`] at
    /// both turn-driver seams (the batched/cancelable funnel in
    /// `turn_loop::execute_one_turn_with_recovery_tracked` and the streaming
    /// seam in `try_run_turn_streaming`), reading the same
    /// `self.api`-cached snapshot source.
    ///
    /// TS updates `rawUtilization` unconditionally on every headers pass
    /// (`claudeAiLimits.ts:476` and the 429 path `:500`) because it is module
    /// state polled by `getRawUtilization()`. Our event channel emits only on
    /// change to avoid spamming the stream, while still forwarding the empty
    /// snapshot as `(None, None, None, None)` so event-driven clients observe
    /// the same state transition.
    ///
    /// Atomic-window invariant: each window contributes either both `Some`
    /// values or both `None` — guaranteed by construction, since
    /// [`crate::model::rate_limit::RawWindow`] only exists with both fields.
    pub(crate) async fn emit_raw_utilization_if_changed(&self) {
        let Some(raw) = self.api.last_raw_utilization() else {
            return;
        };
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
    /// When no compactor is wired (`compaction == None`), returns an explicit
    /// error. Manual compaction must never report success without a real model
    /// summary and history transition.
    ///
    /// (M6-08)
    pub async fn force_compact_with_cancel(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<traits::CompactionSummary, traits::HandleError> {
        self.force_compact_with_instructions_and_cancel(None, cancel)
            .await
    }

    /// Manual `/compact` with optional focus text and cooperative cancellation.
    pub async fn force_compact_with_instructions_and_cancel(
        &self,
        custom_instructions: Option<&str>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<traits::CompactionSummary, traits::HandleError> {
        let Some(compactor) = self.compaction.clone() else {
            return Err(traits::HandleError::ActionFailed(
                "compaction unavailable".into(),
            ));
        };

        // Snapshot history (clone — we don't hold the lock across the
        // network call inside `process_iteration`).
        let (history_before, model) = {
            let s = self.session.lock().await;
            (s.history.clone(), s.model.clone())
        };
        let messages_before = u32::try_from(history_before.len()).unwrap_or(u32::MAX);
        let bytes_before: u64 = history_before.iter().map(protocol::text_byte_size).sum();
        // Capture the token estimate before `history_before` is consumed by
        // `process_iteration` — used for the boundary `preTokens`.
        let pre_tokens_estimate = compaction::grouping::estimate_tokens_for_range(&history_before);

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

        // Binary-verified guard ordering: only an EMPTY history short-circuits
        // before hooks (`if(n.length===0) throw Error("No messages to
        // compact")`, thrown by /compact's caller before `Juy`). A short-but-
        // non-empty conversation still fires PreCompact hooks and flashes the
        // compacting status — it fails LATER inside the group compactor
        // (`Nto`'s `too_few_groups` → `Not enough messages to compact.`),
        // which the port surfaces via `process_forced`'s
        // `CompactionError::NotEnoughMessages` mapping below.
        if history_before.is_empty() {
            return Err(traits::HandleError::ActionFailed(
                "No messages to compact".into(),
            ));
        }

        // Claude starts manual-compaction timing before PreCompact hooks and
        // freezes durationMs after attachment/SessionStart restoration.
        let compact_started = std::time::Instant::now();

        // Once a real pass begins, emit status before PreCompact hooks so slow
        // hooks are visible too (`Juy` emits `sdk_status: compacting` first).
        self.output.emit_compaction_started().await;

        // hooks compaction lifecycle: PreCompact fires before the summary pass.
        // This is the explicit `/compact` entry point, so the trigger is
        // `manual` (TS `isAutoCompact ? 'auto' : 'manual'`). TS `VJn`: a
        // blocking PreCompact hook ABORTS the compaction, throwing
        // `"Compaction blocked by PreCompact hook: <blockedBy>"`. We surface the
        // same message as the `/compact` failure result.
        let pre_compact = self.fire_pre_compact("manual", custom_instructions).await;
        if let Some(detail) = pre_compact.blocked_by {
            let msg = if detail.is_empty() {
                "Compaction blocked by PreCompact hook".to_string()
            } else {
                format!("Compaction blocked by PreCompact hook: {detail}")
            };
            tracing::warn!("{msg}");
            return Err(traits::HandleError::ActionFailed(msg));
        }

        let merged_instructions = merge_compact_instructions(
            custom_instructions,
            pre_compact.additional_instructions.as_deref(),
        );

        // A resumed session may not have made a live API call yet, leaving the
        // shared cache-safe slot empty. Seed it from the current system prompt
        // and live history so manual compaction works immediately after resume.
        // Autocompactor replaces the slot's potentially stale message clone with
        // `history_before`'s selected prefix before issuing the request.
        let system_prompt = self.effective_system_prompt().await;
        self.save_cache_safe_params(Some(&system_prompt), &model)
            .await;

        // Run the 5-layer compactor, racing against the cancel token.
        // process_iteration takes no CancellationToken; drop-on-cancel
        // leaves history untouched because we have not written back.
        // `biased` so the cancel arm wins a tie — preferred when both
        // arms are immediately ready.
        // The API-duration clock starts HERE (summarizer round-trip only) —
        // separate from `compact_started` (pre-hooks), which feeds the
        // boundary's durationMs.
        let api_started = std::time::Instant::now();
        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                return Err(traits::HandleError::ActionFailed(
                    "compaction cancelled".into(),
                ));
            }
            r = compactor.process_forced(history_before, merged_instructions.as_deref()) => r
                .map_err(|e| match e {
                    // TS throws `Error(GJn)` when the summarizer's PTL-retry loop
                    // exhausts (nothing safe left to drop) — the port models that
                    // as `MaxRetriesExceeded`. Surface the byte-exact GJn message
                    // rather than the generic "compaction failed: …".
                    compaction::autocompact::CompactionError::MaxRetriesExceeded => {
                        traits::HandleError::ActionFailed(
                            compaction::ptl_retry::COMPACTION_CONVERSATION_TOO_LONG.to_string(),
                        )
                    }
                    compaction::autocompact::CompactionError::NotEnoughMessages => {
                        traits::HandleError::ActionFailed(
                            "Not enough messages to compact.".to_string(),
                        )
                    }
                    other => {
                        traits::HandleError::ActionFailed(format!("compaction failed: {other}"))
                    }
                })?,
        };
        // CC re-checks `signal.aborted` between compaction phases: an Esc that
        // lands while the summarizer response was already resolving must still
        // abort BEFORE the post-compact transition (file re-reads, SessionStart
        // hooks, history swap) — otherwise the cancelled task swaps history out
        // from under a prompt the user has since submitted.
        if cancel.is_cancelled() {
            return Err(traits::HandleError::ActionFailed(
                "compaction cancelled".into(),
            ));
        }
        // API duration = the summarizer round-trip only. `compact_started`
        // (above, pre-hooks) feeds the boundary's user-visible durationMs;
        // feeding it here would fold PreCompact hook wall-time into
        // /cost's total_api_duration_ms.
        let compact_duration = api_started.elapsed();
        self.record_compaction_usage(&result, compact_duration)
            .await;

        // hooks compaction lifecycle: capture the summary + freed-token count
        // BEFORE `apply_post_compact` consumes the result, so PostCompact can
        // carry the byte-faithful payload (TS `compactData.compactSummary`).
        let summary = Self::compaction_summary_text(&result);
        let tokens_freed = result.total_tokens_freed;

        // Apply the post-compact transition (boundary marker + history swap +
        // CompactionCompleted emit) via the shared helper reused by the
        // proactive trigger (Batch 4) and the reactive 413 fallback (Batch 5).
        // `bytes_before` / `pre_tokens_estimate` were computed from the same
        // `history_before` snapshot (consumed by `process_iteration` above).
        let Some(summary_out) = self
            .apply_post_compact(
                result,
                compaction::CompactTrigger::Manual,
                pre_tokens_estimate,
                messages_before,
                bytes_before,
                compact_started,
                Some(&cancel),
            )
            .await
        else {
            // Esc landed during the post-compact tail (file re-reads /
            // SessionStart hooks): the swap was skipped, history is untouched.
            return Err(traits::HandleError::ActionFailed(
                "compaction cancelled".into(),
            ));
        };

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
    /// freshest file context across the boundary. Selection is the pure
    /// [`compaction::select_post_compact_files`]; each survivor is then RE-READ
    /// from disk (the byte-faithful `eRg`/`XQn` behaviour — see below) before
    /// [`compaction::budget_post_compact_files`] budgets the fresh contents, and
    /// each survivor is rendered as a `<system-reminder>` meta user message.
    ///
    /// P2-12 / `eRg` (`bin/claude.exe` offset ~91938880): the binary re-reads
    /// each selected file at compact time via `XQn(filename,
    /// {...ctx,fileReadingLimits:{maxTokens:z0g}}, "…_success", "…_error",
    /// "compact")` rather than reusing the stale `readFileState` snapshot. A
    /// successful re-read fires `tengu_post_compact_file_restore_success` (empty
    /// payload, `N(r,{})`) and attaches the FRESH content; an unreadable/deleted
    /// file fires `tengu_post_compact_file_restore_error` (`N(n,{})`) and is
    /// dropped — the model never sees stale/since-deleted content. This differs
    /// observably from the snapshot only when a file changed or was deleted after
    /// its last read.
    ///
    /// SKILL restoration (`rRg`/`kGo`) IS wired here (P2-12): the Skill tool
    /// records each invocation in the process-global
    /// [`compaction::invoked_skills`] registry (`zSr`), and after the file arm we
    /// [`compaction::invoked_skills::filter_for_agent`] the main-thread rows
    /// (`agentId = None`), run [`compaction::restore_post_compact_skills`]
    /// (`rRg`: `invokedAt` DESC, per-skill truncate 5000, budget 25000, registry
    /// write-back on truncation/overflow), and emit the survivors as ONE `isMeta`
    /// user message in the byte-faithful `invoked_skills` attachment shape
    /// (`render_invoked_skills_attachment`). The registry outlives the compaction
    /// (never cleared by `run_post_compact_cleanup`), so a skill invoked before
    /// compaction re-enters the model's context afterwards.
    ///
    async fn restore_post_compact_attachments_against(
        &self,
        boundary_context: &[protocol::ConversationMessage],
    ) -> Vec<protocol::ConversationMessage> {
        // Snapshot then clear the MODEL-VISIBLE portion of the ONE read-file-
        // state registry (the `eOt` snapshot + `readFileState.clear()` step),
        // so the post-compact context starts from the restored set only.
        // Host-seeded snapshots stay cached for staleness/dedup because the
        // model never saw them and therefore they must not be restored.
        let snapshot: Vec<(std::path::PathBuf, tool_api::read_file_state::ReadFileEntry)> = {
            let mut map = self
                .read_state_map
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            map.drain_model_context()
        };

        // The two arms are independent: skills restore from the process-global
        // registry even when no file was read this session, so we do NOT early-
        // return on an empty file snapshot.
        let mut out: Vec<protocol::ConversationMessage> = Vec::new();

        if !snapshot.is_empty() {
            let already_attached = Self::post_compact_attached_file_paths(boundary_context);
            let session_id = self.session.lock().await.session_id;
            let plan_file = std::path::PathBuf::from(Self::plan_file_path(
                &session_id,
                &self.cwd,
                self.config.plans_directory.as_deref(),
            ));
            out.extend(
                self.restore_post_compact_files_arm(snapshot, &already_attached, &plan_file)
                    .await,
            );
        }

        // ── SKILL restoration (`rRg`) ──
        // Source candidates from the process-global invoked-skill registry
        // (`kGo` on the main thread, `agentId = None`), budget them (`rRg`), and
        // emit the survivors as ONE `isMeta` user message in the byte-faithful
        // `invoked_skills` attachment shape. Preserve the binary's LQn
        // deduplication against both plain message bodies and skill content
        // that already survived in an earlier invoked-skills attachment.
        let skill_candidates = compaction::invoked_skills::filter_for_agent(None);
        let already_attached_skills = self.post_compact_attached_skill_contents(boundary_context);
        let restored_skills =
            compaction::restore_post_compact_skills(skill_candidates, &already_attached_skills);
        if let Some(body) = compaction::render_invoked_skills_attachment(&restored_skills) {
            let message_id = protocol::MessageId::new();
            let contents = restored_skills
                .iter()
                .map(|skill| skill.content.clone())
                .collect();
            self.post_compact_skill_attachments
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(message_id, contents);
            out.push(protocol::ConversationMessage::user_meta(message_id, body));
        }

        out
    }

    fn post_compact_attached_file_paths(
        messages: &[protocol::ConversationMessage],
    ) -> Vec<std::path::PathBuf> {
        messages
            .iter()
            .flat_map(|message| {
                message
                    .text_content()
                    .lines()
                    .filter_map(|line| {
                        line.strip_prefix("Referenced file ").map(|rest| {
                            let path = rest
                                .split_once(" (restored after compaction):")
                                .map_or(rest, |(path, _)| path);
                            crate::turn_loop::normalize_lexically(std::path::Path::new(path))
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn post_compact_attached_skill_contents(
        &self,
        messages: &[protocol::ConversationMessage],
    ) -> Vec<compaction::AttachedSkillContent> {
        let surviving_ids = messages
            .iter()
            .map(protocol::ConversationMessage::id)
            .collect::<std::collections::HashSet<_>>();
        let mut known_attachments = self
            .post_compact_skill_attachments
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        known_attachments.retain(|message_id, _| surviving_ids.contains(message_id));

        let mut attached = Vec::new();
        for message in messages {
            let body = message.text_content();
            attached.push(compaction::AttachedSkillContent::Body(body.clone()));
            if let Some(contents) = known_attachments.get(&message.id()) {
                attached.extend(
                    contents
                        .iter()
                        .cloned()
                        .map(compaction::AttachedSkillContent::Attachment),
                );
            }
        }
        attached
    }

    /// The FILE restoration arm of [`Self::restore_post_compact_attachments`]
    /// (`eRg`/`XQn`): select recent files, RE-READ each from disk, fire the
    /// per-file restore telemetry, budget the fresh contents, and render each as
    /// a `<system-reminder>` meta user message. Split out so the skill arm can
    /// run even when the file snapshot is empty.
    async fn restore_post_compact_files_arm(
        &self,
        snapshot: Vec<(std::path::PathBuf, tool_api::read_file_state::ReadFileEntry)>,
        already_attached: &[std::path::PathBuf],
        plan_file: &std::path::Path,
    ) -> Vec<protocol::ConversationMessage> {
        let candidates: Vec<compaction::FileRestoreCandidate> = snapshot
            .into_iter()
            .map(|(path, entry)| compaction::FileRestoreCandidate {
                path,
                content: entry.content,
                timestamp_ms: entry.mtime_ms,
            })
            .collect();

        // Selection half of `eRg`: filter the plan file (`aRg`) and files that
        // already survived in the preserved boundary context, then sort mtime
        // DESC and take the top five.
        let plan_file = crate::turn_loop::normalize_lexically(plan_file);
        let candidates = candidates
            .into_iter()
            .filter(|candidate| {
                let path = crate::turn_loop::normalize_lexically(&candidate.path);
                path != plan_file && !already_attached.iter().any(|attached| attached == &path)
            })
            .collect();
        let selected = compaction::select_post_compact_files(candidates, &[]);

        // RE-READ each selected file from disk (`XQn`), firing the restore
        // telemetry per file. A file that changed since its last read yields the
        // FRESH content; a deleted/unreadable file is dropped (never restoring the
        // stale snapshot content the model would otherwise have carried across the
        // boundary).
        let mut fresh: Vec<compaction::FileRestoreCandidate> = Vec::with_capacity(selected.len());
        for candidate in selected {
            match tokio::fs::read_to_string(&candidate.path).await {
                Ok(content) => {
                    self.fire_post_compact_file_restore(true).await;
                    fresh.push(compaction::FileRestoreCandidate {
                        content,
                        ..candidate
                    });
                }
                Err(_) => {
                    // Unreadable/deleted at compact time → drop; `XQn` returns
                    // null and the file is filtered out of the attachment set.
                    self.fire_post_compact_file_restore(false).await;
                }
            }
        }

        // Budgeting half of `eRg`: per-file cap (maxTokens 5000) + running budget
        // (50000) over the FRESH re-read contents, preserving the DESC order.
        let restored = compaction::budget_post_compact_files(fresh);

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

    /// Fire the post-compact file-restore telemetry — `N(r,{})` / `N(n,{})` in
    /// the binary's `XQn`: an EMPTY payload, one event per re-read attempt.
    ///
    /// `true` → `tengu_post_compact_file_restore_success` (the file re-read
    /// cleanly); `false` → `tengu_post_compact_file_restore_error` (unreadable /
    /// deleted). No-op when no analytics bus is wired (library/test callers),
    /// mirroring the other `fire_*` compaction telemetry helpers.
    async fn fire_post_compact_file_restore(&self, success: bool) {
        let Some(bus) = self.analytics_bus.as_ref() else {
            return;
        };
        let event = if success {
            "tengu_post_compact_file_restore_success"
        } else {
            "tengu_post_compact_file_restore_error"
        };
        // Empty metadata — the binary fires `N(r,{})` / `N(n,{})` with no fields.
        bus.log_event(event, telemetry::LogEventMetadata::new())
            .await;
    }

    /// `cancel`: the manual `/compact` path threads its Esc token so an abort
    /// that lands during this tail (file re-reads, SessionStart hooks) still
    /// skips the history swap — `None` (auto/reactive callers, which have no
    /// user-cancellable surface) never returns `None`.
    pub(crate) async fn apply_post_compact(
        &self,
        result: compaction::IterationCompactionResult,
        trigger: compaction::CompactTrigger,
        pre_tokens_estimate: u64,
        messages_before: u32,
        bytes_before: u64,
        compact_started: std::time::Instant,
        cancel: Option<&tokio_util::sync::CancellationToken>,
    ) -> Option<traits::CompactionSummary> {
        // Preserve the transcript-only summary before `result.messages` is
        // consumed into the replacement history. The TUI carries this on the
        // compact boundary so Ctrl-O can reveal the same summary sent to the
        // continuation turn.
        let visible_summary = Self::compaction_summary_text(&result);
        // Modern automatic/reactive compaction (`kio`) stamps boundary
        // duration before attachment restoration. Manual full compaction
        // (`hio`) stamps it afterwards. Preserve that observable distinction.
        let auto_duration_ms = (trigger == compaction::CompactTrigger::Auto)
            .then(|| u64::try_from(compact_started.elapsed().as_millis()).unwrap_or(u64::MAX));
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
        // is persisted below as the boundary line's `compactMetadata` (P1-05), so a
        // cold `--resume` reconstructs the post-compact transition.
        //
        // #58: when a tail was preserved, the boundary carries a
        // `preserved_segment` (`WAo`): head = first kept msg, anchor = the LAST
        // summary message (suffix-preserving splice point), tail = last kept msg —
        // plus the loader's `preserved_messages` re-splice list (`E$_`). The
        // anchor is the last of `result.messages` (the summary set). When the
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
        // RV6 (parity 2.1.208): the full auto/manual compaction path — the only
        // one this port implements — builds its boundary via the 3-arg
        // `U6r(trigger, preTokens, lastUuid)`, leaving `messagesSummarized` (and
        // `userContext`) undefined, so `JSON.stringify` drops both fields. CC only
        // sets `messagesSummarized:f.length` on the 5-arg message-selector
        // (`up_to`/`from`) path (`tHu`), which LingXi has no feature for. Passing
        // `None` here keeps the persisted boundary byte-identical to a real
        // 2.1.208 transcript (no extra field, and no spurious "Summarized N
        // messages" TUI line when a genuine CC client cold-resumes it).
        // `preCompactDiscoveredTools` carries the ToolSearch-loaded set so a cold
        // `--resume` re-marks them loaded (empty ⇒ field omitted on the wire).
        let discovered_tools = self.tools.deferral().loaded_tool_names();
        let (mut marker, mut metadata) = compaction::create_compact_boundary_with_preserved_tail(
            trigger,
            pre_tokens_estimate,
            None,
            None,
            None,
            &discovered_tools,
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
        let restored_attachments = self
            .restore_post_compact_attachments_against(&preserved_tail)
            .await;

        // Claude Code 2.1.212 `gio(...)` builds the post-compact attachment
        // set before `executePostCompactHooks`: reload instruction files with
        // `load_reason:"compact"`, then run `SessionStart(source:"compact")`
        // and append its model-facing hook results after the restored files /
        // skills. Run the module-state cleanup first so the reload observes a
        // fresh post-compact state rather than the pre-compact caches.
        compaction::run_post_compact_cleanup(None);
        self.fire_instructions_loaded_with_reason(hooks::events::InstructionsLoadReason::Compact)
            .await;
        let session_start_messages = self.collect_session_start_messages("compact").await;

        // COMPACT.1 / #58: the boundary marker leads the post-compact history,
        // matching TS `buildPostCompactMessages` / `Iqn` order
        // `[boundaryMarker, ...summaryMessages, ...messagesToKeep, ...attachments,
        // ...hookResults]` (compact.ts:330). The preserved verbatim tail
        // (`messagesToKeep`) rides AFTER the summary and BEFORE the restored
        // attachments. Empty `preserved_tail` ⇒ the order is identical to before
        // (`[marker, ...summary, ...attachments]`).
        let mut history_after = Vec::with_capacity(
            result.messages.len()
                + 1
                + preserved_tail.len()
                + restored_attachments.len()
                + session_start_messages.len(),
        );
        let tail_preserved = !preserved_tail.is_empty();
        history_after.push(marker.clone());
        history_after.extend(result.messages.iter().cloned());
        // #58: the usage-zeroed verbatim tail (`messagesToKeep`).
        history_after.extend(preserved_tail);
        // Restored file attachments ride after the summary + kept tail (the
        // `attachments` slot in `buildPostCompactMessages`).
        history_after.extend(restored_attachments.iter().cloned());
        // `SessionStart(source:"compact")` hook results are the final
        // `hookResults` slot in Claude's `buildPostCompactMessages` order.
        history_after.extend(session_start_messages.iter().cloned());

        // Claude mutates the boundary metadata only after the complete
        // post-compact message set has been assembled. This includes the
        // boundary, summary, preserved tail, restored attachments, and
        // SessionStart hook results.
        let post_tokens = compaction::grouping::estimate_tokens_for_range(&history_after);
        metadata.post_tokens = Some(post_tokens);
        metadata.duration_ms = Some(auto_duration_ms.unwrap_or_else(|| {
            u64::try_from(compact_started.elapsed().as_millis()).unwrap_or(u64::MAX)
        }));
        let dropped_this_pass = pre_tokens_estimate.saturating_sub(post_tokens);
        // LAST cancel checkpoint (CC re-checks `signal.aborted` between
        // phases): everything above is side-effect-free w.r.t. session state,
        // so an Esc that landed during the attachment re-reads / SessionStart
        // hooks aborts here — before the cumulative counter, the history swap,
        // the JSONL persist, and the CompactionCompleted emit.
        if cancel.is_some_and(tokio_util::sync::CancellationToken::is_cancelled) {
            return None;
        }
        let previous_dropped = self
            .compaction_cumulative_dropped_tokens
            .fetch_add(dropped_this_pass, std::sync::atomic::Ordering::Relaxed);
        metadata.cumulative_dropped_tokens =
            Some(previous_dropped.saturating_add(dropped_this_pass));
        let pre_boundary_last_uuid = self.last_jsonl_uuid.lock().await.clone();
        metadata.logical_parent_uuid = pre_boundary_last_uuid.clone();

        let messages_after = u32::try_from(history_after.len()).unwrap_or(u32::MAX);
        let bytes_after: u64 = history_after.iter().map(protocol::text_byte_size).sum();
        let bytes_saved = bytes_before.saturating_sub(bytes_after);

        // Swap history under the same lock.
        {
            let mut s = self.session.lock().await;
            metadata.active_goal = s
                .active_goal
                .as_ref()
                .map(compaction::compact_active_goal_from_engine);
            debug_assert!(marker.set_compact_metadata(metadata.clone()));
            history_after[0] = marker.clone();
            s.history = history_after;
        }
        // Relevant-memory and skill reminders live only in outgoing request
        // snapshots. Once compaction discards those snapshots, they may surface
        // again; restored file attachments remain deduplicated by
        // `read_state_map`. Drop in-flight pre-compact queries as well so stale
        // selections cannot be injected against the new history.
        self.surfaced_memory_paths.lock().await.clear();
        self.surfaced_skill_names.lock().await.clear();
        *self.pending_memory_prefetch.lock().await = None;
        *self.pending_skill_prefetch.lock().await = None;
        // P1-05: persist the full compaction transition (claude 2.1.207
        // `insertMessageChain` + the compact flow), so a cold `--resume`
        // reconstructs exactly the post-compact state. Best-effort — a write
        // failure never fails the turn.
        //
        // 1. The boundary line: `parentUuid: null` (chain reset — a tip→root
        //    walk stops here) with the real parent stashed in
        //    `logicalParentUuid`, plus the flattened `subtype:"compact_boundary"`
        //    / `content` / `level:"info"` / `compactMetadata` envelope.
        // 2. The summary user line(s) (`isCompactSummary` +
        //    `isVisibleInTranscriptOnly`), chained off the boundary.
        // 3. The preserved verbatim tail is NOT rewritten — its lines are
        //    already on disk (claude doesn't rewrite them either); the
        //    boundary's `preservedSegment`/`preservedMessages` metadata carries
        //    the loader's re-splice info. The chain pointer is reset to the
        //    tail's LAST on-disk line so subsequent lines parent off the kept
        //    tail, exactly like claude (whose writer chains off the in-memory
        //    array `[boundary, ...summary, ...messagesToKeep, ...]`, skipping
        //    already-persisted members).
        // 4. Restored file attachments, chained after the tail (the
        //    `attachments` slot of `buildPostCompactMessages`).
        self.persist_compact_boundary_to_jsonl(&marker, &metadata)
            .await;
        for m in &result.messages {
            self.persist_compact_summary_to_jsonl(m).await;
        }
        if tail_preserved {
            if let Some(tail_last) = pre_boundary_last_uuid {
                *self.last_jsonl_uuid.lock().await = Some(tail_last);
            }
        }
        for m in &restored_attachments {
            self.persist_message_to_jsonl(m).await;
        }
        for m in &session_start_messages {
            self.persist_message_to_jsonl(m).await;
        }

        // Best-effort emit so the TUI hears about it.
        self.output
            .emit_compaction_completed(
                messages_before,
                messages_after,
                bytes_saved,
                &visible_summary,
            )
            .await;

        Some(traits::CompactionSummary {
            messages_before,
            messages_after,
            bytes_saved,
        })
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
        // `hoe(messages)` (2.1.238 @294688350) sums the last assistant usage
        // INCLUDING output tokens; the `total_tokens_reminder` needs that total,
        // so cache the output half here at the same chokepoint.
        self.last_response_output_tokens
            .store(
                usage.billable_tokens.output,
                std::sync::atomic::Ordering::Relaxed,
            );
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

    /// Record the real LLM usage incurred by a successful summary side-query.
    /// Compaction is an API call and contributes to Claude Code's session cost
    /// and API-duration totals even though it is not a normal conversation turn.
    pub(crate) async fn record_compaction_usage(
        &self,
        result: &compaction::IterationCompactionResult,
        duration: std::time::Duration,
    ) {
        let (Some(tracker), Some(usage), Some(model)) = (
            self.cost_tracker.as_ref(),
            result.compaction_usage,
            result.compaction_model.as_deref(),
        ) else {
            return;
        };
        let model_ref = crate::cost_wiring::model_ref_from_string(model, None);
        tracker
            .record_api_response_v2(
                model_ref,
                usage,
                duration,
                0,
                usage.tokens.cache_read,
                usage.tokens.cache_write,
                false,
                self.analytics_bus.as_ref(),
            )
            .await;
        self.api_calls_recorded
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        let (snapshot, last_assistant_at) = {
            let s = self.session.lock().await;
            (s.history.clone(), s.message_timing.last_assistant_at)
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
        let compact_started = std::time::Instant::now();
        let pre_compact = self.fire_pre_compact("auto", None).await;
        if let Some(detail) = pre_compact.blocked_by {
            tracing::warn!("Precomputed compact blocked by PreCompact hook: {detail}");
            return;
        }

        // Signal the UI that compaction has started so it can show a
        // spinner / "Compacting…" while the summarizer runs.
        self.output.emit_compaction_started().await;

        // Run the orchestrator pass under the per-conversation tracking lock so
        // the circuit-breaker state is read + written atomically for this turn.
        let mut tracking = self.compaction_tracking.lock().await;
        // API duration = the summarizer pass only; `compact_started` (above,
        // pre-hooks) is the boundary durationMs clock.
        let api_started = std::time::Instant::now();
        let result = match compactor
            .process_iteration_tracked_with_instructions_and_timing(
                snapshot,
                0,
                &mut tracking,
                pre_compact.additional_instructions.as_deref(),
                last_assistant_at,
                std::time::SystemTime::now(),
            )
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
        let compact_duration = api_started.elapsed();

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
        self.record_compaction_usage(&result, compact_duration)
            .await;

        // hooks compaction lifecycle: capture the summary + freed-token count
        // from the compaction result BEFORE `apply_post_compact` consumes it,
        // so the PostCompact hook can carry the byte-faithful payload (TS
        // `compactData.compactSummary`). The proactive trigger is always the
        // `auto` arm (TS `isAutoCompact`).
        let summary = Self::compaction_summary_text(&result);
        let tokens_freed = result.total_tokens_freed;

        // `cancel: None` — the proactive trigger has no user-cancellable
        // surface, so the apply is infallible.
        self.apply_post_compact(
            result,
            compaction::CompactTrigger::Auto,
            estimate,
            messages_before,
            bytes_before,
            compact_started,
            None,
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
        let session_duration = self
            .session_started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed();
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
            api_duration: std::time::Duration::from_millis(state.total_api_duration_ms),
            code_lines_added: state.total_lines_added,
            code_lines_removed: state.total_lines_removed,
            by_model: state
                .per_model_usage
                .values()
                .map(|mu| traits::orchestrator::ModelUsageRow {
                    model: mu.model_ref.model.clone(),
                    // (cc 2.1.218) `n.provider=n_(r)` — the serving provider,
                    // pre-stringified so the transport row stays cost-free.
                    provider: Some(mu.model_ref.provider.usage_wire_name()),
                    total_nano_usd: mu.cost_nano_usd,
                    input_tokens: mu.usage.tokens.input,
                    output_tokens: mu.usage.tokens.output,
                    cache_read_input_tokens: mu.cache_read_input_tokens,
                    cache_creation_input_tokens: mu.cache_creation_input_tokens,
                })
                .collect(),
            unknown_models: !state.unpriced_models.is_empty(),
            current_usage: state.last_usage.map(|usage| traits::CurrentUsageSnapshot {
                input_tokens: usage.tokens.input,
                output_tokens: usage.tokens.output,
                cache_read_input_tokens: state.last_cache_read_input_tokens,
                cache_creation_input_tokens: state.last_cache_creation_input_tokens,
            }),
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
    /// The continuation nudge is injected as a META user message
    /// ([`Self::inject_meta_user_message`] / [`ConversationMessage::user_meta`])
    /// into both the live session history and the JSONL persistence stream,
    /// where it persists with top-level `isMeta:true`. The same META injection
    /// pattern is shared by the malformed-tool-use retry (#77), thinking-only
    /// (#78), and max-output-tokens recovery nudges.
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
    /// The user-visible refusal-fallback line (2.1.206 `VPn(e,t,r)` for the
    /// common `category == "other"` path: the generic `$7m` prefix, then
    /// `Switched to {marketing name}`, then the feedback line).
    ///
    /// Lives here rather than inline so the emit site and the tests read the
    /// same bytes. The cyber/bio "intentionally broad" variant still needs the
    /// refusal category routed through — see the typed notice's
    /// `api_refusal_category`, which now carries it.
    pub(crate) fn refusal_warning_text(fallback: &str) -> String {
        let display = crate::prompt::env_meta::marketing_name_for_model(fallback)
            .map(String::from)
            .unwrap_or_else(|| fallback.to_string());
        format!(
            "This model's safeguards flagged this message. \
This sometimes happens with safe, normal conversations. Switched to {display}. \
Send feedback with /feedback or learn more: https://support.claude.com/en/articles/15363606"
        )
    }

    pub(crate) async fn maybe_swap_to_refusal_fallback(&self) -> bool {
        // The CASCADE: an ordered chain of models, each tried as the previous
        // one refuses. An empty chain falls back to the historical single
        // `refusal_fallback_model`, which is exactly a one-element chain — so
        // the default path is byte-identical to before the cascade existed.
        let chain: Vec<String> = if self.config.refusal_fallback_chain.is_empty() {
            self.config
                .refusal_fallback_model
                .clone()
                .into_iter()
                .collect()
        } else {
            self.config.refusal_fallback_chain.clone()
        };
        if chain.is_empty() {
            return false;
        }
        // Models already tried THIS EPISODE, so a cascade cannot loop back onto
        // one that has already refused.
        let tried = self.refusal_tried_models.lock().await.clone();
        let route = crate::refusal_cascade::route_refusal(
            &crate::refusal_cascade::RouteInputs {
                chain: Some(&chain),
                armed_fallback_model: None,
                armed_target_is_refusing_model: false,
                catch_all_enabled: false,
            },
            |stage| {
                // A stage is reachable when it has not already been routed to
                // this episode. Claude's exclusion is exactly `triedModels`,
                // which resets with the session — deliberately NOT "differs
                // from the current model": after a hop the current model IS the
                // previous fallback, and excluding it would make a cleared
                // session unable to route to that model again.
                (!tried.iter().any(|m| m == stage)).then(|| stage.to_string())
            },
        );
        // Report every stage the walk passed over. A chain that silently
        // degraded to its last entry is otherwise indistinguishable from one
        // that worked first try.
        for report in crate::refusal_cascade::decline_reports(&route) {
            tracing::info!(
                event = "tengu_refusal_fallback_route_declined",
                reason = report.as_str(),
            );
        }
        let crate::refusal_cascade::RefusalRoute::Category { stage, .. } = route else {
            return false;
        };
        // What the cascade still has left. A hop with stages remaining may be
        // superseded, so its notice is provisional.
        let stage_remaining = stage.remaining_chain;
        let fallback = stage.model;
        // Once-per-session latch (refusalFallbackModelLatch analog) — applies
        // only to a SINGLE-hop chain, which is the historical shape. A real
        // cascade is bounded by the chain instead: each hop is consumed by
        // `tried`, so the walk terminates on its own without needing the latch
        // to cap it.
        if chain.len() <= 1
            && self
                .refusal_fallback_latched
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return false;
        }
        self.refusal_tried_models
            .lock()
            .await
            .push(fallback.clone());
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
        // Route the notice through the episode accumulator and the collapse
        // queue rather than emitting it directly. A hop that a LATER hop
        // supersedes must not reach the user: "switched to X" stops being true
        // the moment the cascade moves on from X. So an intermediate hop is
        // held PROVISIONALLY and folded into the notice that finally settles,
        // which reports how many hops it collapsed.
        let more_hops_possible = !stage_remaining.is_empty();
        let notice_uuid = uuid::Uuid::new_v4().to_string();
        let emitted = {
            let mut episode = self.refusal_episode.lock().await;
            episode.merge(crate::refusal_notice::RefusalNotice {
                uuid: notice_uuid.clone(),
                origin_model: original_model.clone(),
                serving_model: fallback.clone(),
                ..crate::refusal_notice::RefusalNotice::default()
            });
            let taken = if more_hops_possible {
                episode.take_provisional(&notice_uuid)
            } else {
                episode.settle()
            };
            drop(episode);
            match taken {
                Some(notice) => self
                    .refusal_notice_queue
                    .lock()
                    .await
                    .accept(notice, more_hops_possible),
                None => Vec::new(),
            }
        };
        for e in emitted {
            if e.suppressed_count > 0 {
                tracing::info!(
                    event = "tengu_refusal_fallback_notice_collapsed",
                    suppressed_count = e.suppressed_count,
                    emitted_via = e.emitted_via.as_str(),
                );
            }
            self.output
                .emit_text(&Self::refusal_warning_text(&e.banner.serving_model))
                .await;
        }
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

    /// Inject an engine META user message (`isMeta:true`) into both the live
    /// session history and the JSONL persistence stream — the recovery /
    /// continuation nudges (thinking-only, malformed-tool retry) that claude-code
    /// creates via `createUserMessage({ …, isMeta: true })`. Persisting stamps the
    /// top-level `isMeta:true` envelope flag (see `to_jsonl_message`), so these
    /// lines are skipped by title / first-prompt / fork-name / visible-count
    /// extraction, exactly as in CC 2.1.207.
    async fn inject_meta_user_message(&self, text: &str) {
        self.inject_user_text(text, true).await;
    }

    /// Inject a PLAIN (non-meta) user text message. Used for interrupt markers
    /// (`[Request interrupted by user]`) and mid-turn drained HUMAN input, which
    /// CC 2.1.207 persists WITHOUT `isMeta` — interrupt lines are built with no
    /// `isMeta` field, and queued human input is non-meta per the `queued_command`
    /// guard (`r!==void 0&&!Ree(r)||e.isMeta` → `{}` for plain human input).
    async fn inject_user_message(&self, text: &str) {
        self.inject_user_text(text, false).await;
    }

    /// Drain accepted cross-session inbox lines into history as meta user-role
    /// `<cross-session-message>` envelopes (2.1.232 `isMeta:!0`). Policy is
    /// applied at receive; this only injects already-accepted bodies.
    pub(crate) async fn drain_peer_inbox(&self, mid_turn: bool) -> bool {
        let reminders = traits::live_sessions::take_accepted_peer_reminders(mid_turn);
        if reminders.is_empty() {
            return false;
        }
        for body in reminders {
            // Peer / receipt text is never user intent (2.1.232 `isMeta:!0`).
            self.inject_user_text(&body, true).await;
        }
        true
    }

    async fn inject_user_text(&self, text: &str, is_meta: bool) {
        let msg = if is_meta {
            ConversationMessage::user_meta(MessageId::new(), text.to_string())
        } else {
            ConversationMessage::user(MessageId::new(), text.to_string())
        };
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
                // Inject the continuation nudge as a META user message
                // (`createUserMessage({content: nudgeMessage, isMeta: true})`,
                // query.ts:1327). It persists with top-level `isMeta:true` and is
                // skipped by title / first-prompt / visible-count extraction.
                let nudge_msg = ConversationMessage::user_meta(MessageId::new(), nudge_message);
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
        if let Some(contents) = self
            .post_compact_skill_attachments
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&msg.id())
            .cloned()
        {
            extra.insert(
                "invokedSkillContents".to_string(),
                serde_json::json!(contents),
            );
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
        // Top-level `effort` (2.1.212): the session's resolved reasoning-effort
        // LEVEL string. claude-code spreads `...effort!==void 0&&{effort}` (the
        // `Y4n(effort).level`) as the last field of the in-memory assistant
        // message object, which persists verbatim into the transcript record —
        // so it lands on REAL assistant lines only, right after `timestamp` and
        // before the `userType`/`cwd` trailer (the serializer places it there).
        // Gated on a REAL response (`assistant_model.is_some()`) so synthetic
        // api-error assistant lines — which claude builds via a different builder
        // with no effort — stay byte-identical. `None` effort omits the field,
        // matching claude's `!==void 0` guard.
        if kind == "assistant" && assistant_model.is_some() {
            if let Some(effort) = self
                .current_effort
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                extra.insert(
                    "effort".to_string(),
                    serde_json::Value::String(effort.clone()),
                );
            }
            let selection = self
                .current_reasoning_selection
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if !matches!(selection, traits::ReasoningSelection::Automatic) {
                if let Ok(value) = serde_json::to_value(selection) {
                    extra.insert("reasoningSelection".to_string(), value);
                }
            }
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
        let (forced, final_input, permission_updates) = match decision {
            traits::permission_gate::PermissionOutcome::Allow {
                updated_input,
                permission_updates,
                decision_classification: _,
            } => (
                crate::test_support::PermissionDecision::Allow,
                updated_input.unwrap_or(input),
                permission_updates,
            ),
            traits::permission_gate::PermissionOutcome::Deny { reason } => (
                crate::test_support::PermissionDecision::Deny { reason },
                input,
                Vec::new(),
            ),
        };
        if !permission_updates.is_empty() {
            self.perms.apply_permission_updates(&permission_updates);
            self.perms
                .persist_permission_updates(&permission_updates)
                .await;
        }

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
            is_compact_summary: false,
            is_visible_in_transcript_only: false,
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
        // O3: this recovery path dispatches exactly one tool — flush its hook
        // attachment lines after its tool_result, and skip the ephemeral
        // renderings (see the batched driver in `turn_loop.rs`).
        self.flush_hook_attachments(tool_use_id).await;
        for (m, _source_id) in &injected_messages {
            if m.is_meta() {
                continue;
            }
            self.persist_message_to_jsonl(m).await;
        }
        crate::turn_loop::apply_model_context_modifiers(self, context_modifiers).await;

        Ok(true)
    }

    /// Persist a single message to the optional JSONL writer.
    ///
    /// Best-effort: write failures are logged via the telemetry
    /// `tengu_session_corrupted` event and emit one sanitized user-visible
    /// warning per session, but never fail the turn. On success, emits
    /// `tengu_session_appended` and updates the `last_jsonl_uuid` cache.
    pub(crate) async fn persist_message_to_jsonl(&self, msg: &ConversationMessage) {
        self.persist_message_to_jsonl_with_parent(msg, None).await;
    }

    /// Append an SDK/stream-json supplied assistant or system history entry to
    /// the live session and its transcript. This is intentionally not routed
    /// through a model turn: the next user message observes the seeded history
    /// exactly once and input ordering remains owned by the caller.
    pub async fn append_external_history_message(&self, msg: ConversationMessage) {
        let compact_metadata = match &msg {
            ConversationMessage::System {
                subtype: Some(subtype),
                compact_metadata: Some(metadata),
                ..
            } if subtype == "compact_boundary" => Some(metadata.clone()),
            _ => None,
        };
        {
            let mut session = self.session.lock().await;
            session.history.push(msg.clone());
        }
        if let Some(metadata) = compact_metadata {
            self.persist_compact_boundary_to_jsonl(&msg, &metadata)
                .await;
        } else {
            self.persist_message_to_jsonl(&msg).await;
        }
    }

    /// Append an SDK/stream-json compact boundary with its structured metadata.
    ///
    /// Claude's SDK uses snake_case inside `compact_metadata`, while the JSONL
    /// transcript stores camelCase `compactMetadata`. Keeping this path
    /// separate from generic system-history append prevents informational
    /// system frames from masquerading as model context and preserves cold
    /// resume semantics for externally replayed compacted sessions.
    pub async fn append_external_compact_boundary(
        &self,
        mut marker: ConversationMessage,
        sdk_compact_metadata: serde_json::Value,
    ) {
        let compact_metadata = camelize_json_keys(sdk_compact_metadata);
        if let Ok(typed_metadata) =
            serde_json::from_value::<protocol::CompactBoundaryMetadata>(compact_metadata.clone())
        {
            let _ = marker.set_compact_metadata(typed_metadata);
        }
        {
            let mut session = self.session.lock().await;
            session.history.push(marker.clone());
        }
        self.persist_compact_boundary_jsonl_value(&marker, compact_metadata)
            .await;
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
        // Android Computer Use screenshots must reach the current model but
        // must not be written to the durable JSONL transcript. The tool marks
        // only those results with `_lingxi_ephemeral`; every existing desktop
        // and mobile result remains byte-identical.
        let sanitized = redact_ephemeral_tool_result_images(msg);
        self.persist_message_to_jsonl_inner(&sanitized, parent_override, None, false)
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
        self.persist_message_to_jsonl_inner(msg, None, Some(env), false)
            .await;
    }

    /// Persist a compaction summary user line, stamping the top-level
    /// `isVisibleInTranscriptOnly: true` + `isCompactSummary: true` envelope
    /// flags. 1:1 with claude 2.1.207's summary persist
    /// (`$r({content: v9r(...), isCompactSummary: !0,
    /// isVisibleInTranscriptOnly: !0})`, round-tripped by the writer's
    /// `...f.isCompactSummary===!0&&{isCompactSummary:!0}`); the line chains
    /// off `last_jsonl_uuid` (the compact-boundary line).
    pub(crate) async fn persist_compact_summary_to_jsonl(&self, msg: &ConversationMessage) {
        self.persist_message_to_jsonl_inner(msg, None, None, true)
            .await;
    }

    /// Record a best-effort transcript write failure and surface one sanitized
    /// warning per session. The raw error remains confined to logs/telemetry;
    /// it can contain host paths or backend details and must never cross the
    /// user-facing output seam.
    async fn record_transcript_append_failure(
        &self,
        session_id: &str,
        operation: &'static str,
        error: &(impl std::fmt::Display + ?Sized),
    ) {
        // CC 2.1.218 logs + emits telemetry on a transcript-append failure but
        // shows NO user-visible notice — the port's `TRANSCRIPT_PERSISTENCE_WARNING`
        // system notice was an invented surface. Keep the log + telemetry only.
        tracing::error!(error = %error, operation, "jsonl writer append failed");
        telemetry::emit_session_corrupted(session_id, &error.to_string());
    }

    /// Persist the compact-boundary system line (P1-05) — claude 2.1.207's
    /// `insertMessageChain` chain reset: the boundary gets `parentUuid: null`
    /// (`CC(d)` ⇒ `{parentUuid: p?null:f, logicalParentUuid: p?i:void 0}`) with
    /// the real parent (the last pre-compact on-disk line) stashed in
    /// `logicalParentUuid`, plus the flattened system envelope
    /// (`subtype:"compact_boundary"`, `content:"Conversation compacted"`,
    /// `level:"info"`, camelCase `compactMetadata`) and NO inner `message`.
    /// Best-effort like every other JSONL append; advances `last_jsonl_uuid`
    /// to the boundary's uuid on success so the summary line chains off it.
    async fn persist_compact_boundary_to_jsonl(
        &self,
        marker: &ConversationMessage,
        metadata: &compaction::CompactBoundaryMetadata,
    ) {
        // `compactMetadata` wire value: CompactBoundaryMetadata serializes
        // claude's camelCase keys; `logicalParentUuid` is a TOP-LEVEL line
        // field (`x9r` spreads it as a message-level sibling), never a
        // `compactMetadata` member — strip it defensively.
        let marker_metadata = match marker {
            ConversationMessage::System {
                compact_metadata: Some(compact_metadata),
                ..
            } => compact_metadata,
            _ => metadata,
        };
        debug_assert_eq!(marker_metadata, metadata);
        let mut compact_metadata =
            serde_json::to_value(marker_metadata).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = compact_metadata.as_object_mut() {
            obj.remove("logicalParentUuid");
        }
        self.persist_compact_boundary_jsonl_value(marker, compact_metadata)
            .await;
    }

    /// Shared compact-boundary JSONL writer for locally generated and SDK
    /// replayed boundaries. The marker resets the physical chain and retains
    /// the previous tail in `logicalParentUuid`, matching Claude's chain model.
    async fn persist_compact_boundary_jsonl_value(
        &self,
        marker: &ConversationMessage,
        compact_metadata: serde_json::Value,
    ) {
        let Some(writer) = self.jsonl_writer.as_ref() else {
            return;
        };
        let session_id_str = self.session.lock().await.session_id.to_string();
        let logical_parent = self.last_jsonl_uuid.lock().await.clone();
        let git_branch = self.resolve_git_branch().await;
        let content = match marker {
            ConversationMessage::System { content, .. } => content.clone(),
            _ => compaction::BOUNDARY_CONTENT.to_string(),
        };
        let mut extra = serde_json::Map::new();
        extra.insert(
            "subtype".to_string(),
            serde_json::Value::String("compact_boundary".to_string()),
        );
        extra.insert("content".to_string(), serde_json::Value::String(content));
        extra.insert(
            "level".to_string(),
            serde_json::Value::String("info".to_string()),
        );
        extra.insert("compactMetadata".to_string(), compact_metadata);

        let jmsg = session::JsonlMessage {
            message_type: "system".to_string(),
            uuid: marker.id().as_uuid().to_string(),
            parent_uuid: None,
            session_id: session_id_str.clone(),
            timestamp: chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
            cwd: self.current_cwd().to_string_lossy().into_owned(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            // Boundary lines carry NO inner `message` (the schema's
            // compact-boundary arm skips the field entirely).
            message: serde_json::Value::Null,
            is_sidechain: false,
            user_type: Some("external".to_string()),
            git_branch,
            entrypoint: Some(entrypoint_value()),
            slug: None,
            prompt_id: None,
            logical_parent_uuid: logical_parent,
            extra,
        };
        let uuid_for_chain = jmsg.uuid.clone();
        match writer.append(&jmsg).await {
            Ok(()) => {
                *self.last_jsonl_uuid.lock().await = Some(uuid_for_chain.clone());
                telemetry::emit_session_appended(&session_id_str, &uuid_for_chain);
            }
            Err(e) => {
                self.record_transcript_append_failure(&session_id_str, "compact_boundary", &e)
                    .await;
            }
        }
    }

    /// Persist ONE hook-run `attachment` transcript line.
    ///
    /// claude-code writes exactly one `type:"attachment"` line per hook run
    /// (26 048 such records mined from real 2.1.220 transcripts under
    /// `~/.claude/projects`). The outer envelope puts the payload BEFORE the
    /// discriminator — `parentUuid, isSidechain, attachment, type, uuid,
    /// timestamp, …trailer` — and carries NO inner `message`; that ordering is
    /// implemented by the attachment arm of
    /// [`session::jsonl::schema::JsonlMessage`]'s hand-written `Serialize`.
    ///
    /// `payload` is the value built by [`hooks::attachment`] (`hook_success` /
    /// `hook_non_blocking_error` / `hook_cancelled`). Best-effort like every
    /// other JSONL append; advances `last_jsonl_uuid` on success so the next
    /// line chains off it.
    pub async fn persist_hook_attachment_to_jsonl(&self, payload: serde_json::Value) {
        let Some(writer) = self.jsonl_writer.as_ref() else {
            return;
        };
        let session_id_str = self.session.lock().await.session_id.to_string();
        let parent_uuid = self.last_jsonl_uuid.lock().await.clone();
        let git_branch = self.resolve_git_branch().await;
        let mut extra = serde_json::Map::new();
        extra.insert("attachment".to_string(), payload);

        let jmsg = session::JsonlMessage {
            message_type: "attachment".to_string(),
            uuid: uuid::Uuid::new_v4().to_string(),
            parent_uuid,
            session_id: session_id_str.clone(),
            timestamp: chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
            cwd: self.current_cwd().to_string_lossy().into_owned(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            // Attachment lines carry NO inner `message`.
            message: serde_json::Value::Null,
            is_sidechain: false,
            user_type: Some("external".to_string()),
            git_branch,
            entrypoint: Some(entrypoint_value()),
            slug: None,
            prompt_id: None,
            logical_parent_uuid: None,
            extra,
        };
        let line_uuid = jmsg.uuid.clone();
        match writer.append(&jmsg).await {
            Ok(()) => {
                *self.last_jsonl_uuid.lock().await = Some(line_uuid.clone());
                telemetry::emit_session_appended(&session_id_str, &line_uuid);
            }
            Err(e) => {
                self.record_transcript_append_failure(&session_id_str, "hook_attachment", &e)
                    .await;
            }
        }
    }

    /// Atomically persist oversized hook output below this session's
    /// root-confined `tool-results` directory and return the attachment copy.
    pub(crate) async fn persist_large_hook_output(&self, text: &str) -> Option<String> {
        let config_home = self.config_home.clone()?;
        let (session_uuid, cwd) = {
            let session = self.session.lock().await;
            (
                session.session_id.as_uuid().to_string(),
                self.current_cwd().to_string_lossy().into_owned(),
            )
        };
        let relative = std::path::PathBuf::from("projects")
            .join(session::jsonl::path::project_dir_name(&cwd))
            .join(session_uuid)
            .join("tool-results")
            .join(format!("hook-{}.txt", uuid::Uuid::new_v4()));
        let absolute = config_home.join(&relative);
        let bytes = text.as_bytes().to_vec();
        let write = tokio::task::spawn_blocking(move || {
            traits::rooted_fs::atomic_write(
                &config_home,
                &relative,
                &bytes,
                traits::AtomicWriteOptions {
                    overwrite: false,
                    ..traits::AtomicWriteOptions::default()
                },
            )
        })
        .await;
        match write {
            Ok(Ok(())) => Some(format!("(Full output saved to: {})", absolute.display())),
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "failed to persist oversized hook output");
                None
            }
            Err(error) => {
                tracing::warn!(error = %error, "oversized hook output writer task failed");
                None
            }
        }
    }

    /// Persist the latest active-goal snapshot as a transcript metadata line.
    ///
    /// This keeps `/goal` resumable on non-compacted transcripts; compact
    /// boundaries also snapshot the active goal in `compactMetadata` so a later
    /// compaction cannot summarize away the only copy.
    pub(crate) async fn persist_active_goal_state_to_jsonl(
        &self,
        active_goal: Option<&engine::session::ActiveGoalState>,
    ) {
        let status = if active_goal.is_some() {
            traits::GoalStatusKind::Set
        } else {
            traits::GoalStatusKind::Cleared
        };
        self.persist_goal_status_attachment(status, active_goal)
            .await;
    }

    async fn persist_goal_status_attachment(
        &self,
        status: traits::GoalStatusKind,
        active_goal: Option<&engine::session::ActiveGoalState>,
    ) {
        let Some(goal) = active_goal else {
            return;
        };
        let total_tokens = self.snapshot_cost_real().await.total_tokens;
        let duration_ms = std::time::SystemTime::now()
            .duration_since(goal.set_at)
            .unwrap_or_default()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let snapshot = traits::ActiveGoalSnapshot {
            condition: goal.condition.clone(),
            set_at: goal.set_at,
            last_reason: goal.last_reason.clone(),
            iterations: goal.iterations,
            tokens_at_start: goal.tokens_at_start,
        };
        let attachment = traits::GoalStatusAttachment {
            kind: "goal_status".to_string(),
            status,
            condition: goal.condition.clone(),
            iterations: goal.iterations,
            duration_ms,
            tokens: total_tokens.saturating_sub(goal.tokens_at_start),
            last_reason: goal.last_reason.clone(),
            goal_state: matches!(status, traits::GoalStatusKind::Set).then_some(snapshot),
        };
        match serde_json::to_value(attachment) {
            Ok(value) => self.persist_hook_attachment_to_jsonl(value).await,
            Err(error) => tracing::warn!(%error, "failed to encode goal status attachment"),
        }
    }

    pub(crate) async fn upsert_active_goal_stop_hook_for_session(
        &self,
        session_id: SessionId,
        condition: &str,
    ) {
        let hook = hooks::HookDefinition {
            id: protocol::HookId::new(),
            name: GOAL_STOP_HOOK_NAME.to_string(),
            events: vec![hooks::HookEventType::Stop],
            if_condition: None,
            executor: hooks::HookExecutor::Prompt {
                prompt: goal_stop_hook_prompt(condition),
                model: None,
                continue_on_block: true,
            },
            source: hooks::HookSource::Session,
            blocking: true,
            timeout: Some(Duration::from_secs(GOAL_PROMPT_TIMEOUT_SECS)),
            priority: GOAL_STOP_HOOK_PRIORITY,
            once: false,
            status_message: None,
            async_rewake: false,
            async_timeout: None,
            rewake_message: None,
        };
        let _ = self
            .hooks
            .upsert_session_named_hook(session_id, GOAL_STOP_HOOK_NAME.to_string(), hook)
            .await;
    }

    pub(crate) async fn remove_active_goal_stop_hook_for_session(
        &self,
        session_id: SessionId,
    ) -> bool {
        self.hooks
            .remove_session_named_hook(session_id, GOAL_STOP_HOOK_NAME)
            .await
            .is_some()
    }

    pub async fn sync_active_goal_stop_hook_for_current_state(&self) {
        let (session_id, active_goal) = {
            let s = self.session.lock().await;
            (s.session_id, s.active_goal.clone())
        };
        match active_goal {
            Some(goal) => {
                self.upsert_active_goal_stop_hook_for_session(session_id, &goal.condition)
                    .await;
            }
            None => {
                let _ = self
                    .remove_active_goal_stop_hook_for_session(session_id)
                    .await;
            }
        }
    }

    pub(crate) async fn clear_active_goal_state_and_hook(
        &self,
    ) -> Option<traits::ActiveGoalSnapshot> {
        self.finish_active_goal_state_and_hook(traits::GoalStatusKind::Cleared)
            .await
    }

    async fn finish_active_goal_state_and_hook(
        &self,
        status: traits::GoalStatusKind,
    ) -> Option<traits::ActiveGoalSnapshot> {
        let (session_id, goal, cleared) = {
            let mut s = self.session.lock().await;
            let goal = s.active_goal.take();
            let cleared = goal.as_ref().map(|goal| traits::ActiveGoalSnapshot {
                condition: goal.condition.clone(),
                set_at: goal.set_at,
                last_reason: goal.last_reason.clone(),
                iterations: goal.iterations,
                tokens_at_start: goal.tokens_at_start,
            });
            (s.session_id, goal, cleared)
        };
        if let Some(goal) = goal.as_ref() {
            self.persist_goal_status_attachment(status, Some(goal))
                .await;
        }
        let _ = self
            .remove_active_goal_stop_hook_for_session(session_id)
            .await;
        cleared
    }

    /// Shared append body for [`Self::persist_message_to_jsonl_with_parent`],
    /// [`Self::persist_api_error_message_to_jsonl`] and
    /// [`Self::persist_compact_summary_to_jsonl`].
    async fn persist_message_to_jsonl_inner(
        &self,
        msg: &ConversationMessage,
        parent_override: Option<String>,
        api_error: Option<ApiErrorEnvelope>,
        compact_summary: bool,
    ) {
        self.note_assistant_commit(msg).await;
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
        let mut jmsg = self.to_jsonl_message_with_inner_id(
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
        // Compaction summary user line: stamp the top-level envelope flags in
        // claude's on-disk order (`isVisibleInTranscriptOnly` before
        // `isCompactSummary`, between `message` and `uuid` — the schema's user
        // arm emits them there).
        // Denial provenance: claude stamps `toolDenialKind` on the tool_result
        // user line for a tool that was denied rather than run. The schema's
        // tool-result head places it after `timestamp` and before the common
        // trailer; the exactly-one-tool_result guard lives in
        // `take_tool_denial_kind`.
        //
        // O1: the same line also carries `toolUseResult` (the tool's raw
        // structured result / the `Error: …` string), the MCP `mcpMeta`
        // sibling, and `sourceToolAssistantUUID` (the assistant line that
        // carried the `tool_use`). All four share the single-block guard and
        // are emitted in `TOOL_RESULT_HEAD_EXTRA` order regardless of the
        // order they are inserted here.
        if let Some(result) = self.take_tool_use_result(msg).await {
            jmsg.extra.insert("toolUseResult".to_string(), result);
        }
        if let Some(kind) = self.take_tool_denial_kind(msg).await {
            jmsg.extra.insert(
                "toolDenialKind".to_string(),
                serde_json::Value::String(kind),
            );
        }
        if let Some(meta) = self.take_tool_use_mcp_meta(msg).await {
            jmsg.extra.insert("mcpMeta".to_string(), meta);
        }
        if let Some(src) = self.take_source_tool_assistant_uuid(msg).await {
            jmsg.extra.insert(
                "sourceToolAssistantUUID".to_string(),
                serde_json::Value::String(src),
            );
        }
        if msg.is_visible_in_transcript_only() || compact_summary {
            jmsg.extra.insert(
                "isVisibleInTranscriptOnly".to_string(),
                serde_json::Value::Bool(true),
            );
        }
        if msg.is_compact_summary() || compact_summary {
            jmsg.extra.insert(
                "isCompactSummary".to_string(),
                serde_json::Value::Bool(true),
            );
        }
        let uuid_for_chain = jmsg.uuid.clone();
        match writer.append(&jmsg).await {
            Ok(()) => {
                *self.last_jsonl_uuid.lock().await = Some(uuid_for_chain.clone());
                telemetry::emit_session_appended(&session_id_str, &uuid_for_chain);
            }
            Err(e) => {
                self.record_transcript_append_failure(&session_id_str, "message", &e)
                    .await;
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
        self.note_assistant_commit(msg).await;
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

        let (session_id_str, model, model_profile) = {
            let s = self.session.lock().await;
            (
                s.session_id.to_string(),
                s.model.clone(),
                s.model_profile.clone(),
            )
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
            let mut jmsg = self.to_jsonl_message_with_inner_id(
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
            jmsg.extra.insert(
                "modelProfile".to_string(),
                model_profile
                    .as_ref()
                    .map_or(serde_json::Value::Null, |profile| {
                        serde_json::Value::String(profile.clone())
                    }),
            );
            let line_uuid = jmsg.uuid.clone();
            match writer.append(&jmsg).await {
                Ok(()) => {
                    *self.last_jsonl_uuid.lock().await = Some(line_uuid.clone());
                    telemetry::emit_session_appended(&session_id_str, &line_uuid);
                }
                Err(e) => {
                    self.record_transcript_append_failure(&session_id_str, "assistant_block", &e)
                        .await;
                    // Skip recording this block's uuid in the map — the caller's
                    // fallback (prior single-parent) will be used for any
                    // tool_result that can't find its parent.
                    continue;
                }
            }
            if let protocol::ContentBlock::ToolUse { id, .. } = block {
                // O1: the SAME uuid claude stamps as `sourceToolAssistantUUID`
                // on this tool's `tool_result` user line (and from which its
                // `parentUuid` is derived — BIN off 237862200). Recording it
                // here keeps both turn-loop paths correct without touching any
                // call site.
                self.record_source_tool_assistant_uuid(id, line_uuid.clone())
                    .await;
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
        self.note_assistant_commit(msg).await;
        let ConversationMessage::Assistant { id: turn_id, .. } = msg else {
            // Defensive: non-assistant messages take the plain single-line path.
            self.persist_message_to_jsonl(msg).await;
            return;
        };
        let Some(writer) = self.jsonl_writer.as_ref() else {
            return;
        };
        let inner_id = turn_id.as_uuid().to_string();
        let (session_id_str, model, model_profile) = {
            let s = self.session.lock().await;
            (
                s.session_id.to_string(),
                s.model.clone(),
                s.model_profile.clone(),
            )
        };
        let git_branch = self.resolve_git_branch().await;
        let entrypoint = Some(entrypoint_value());
        let parent_uuid = self.last_jsonl_uuid.lock().await.clone();
        let usage_json = usage.map(assistant_usage_value);
        let mut jmsg = self.to_jsonl_message_with_inner_id(
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
        jmsg.extra.insert(
            "modelProfile".to_string(),
            model_profile.map_or(serde_json::Value::Null, serde_json::Value::String),
        );
        let line_uuid = jmsg.uuid.clone();
        match writer.append(&jmsg).await {
            Ok(()) => {
                *self.last_jsonl_uuid.lock().await = Some(line_uuid.clone());
                telemetry::emit_session_appended(&session_id_str, &line_uuid);
                // O1: the batched path writes ONE merged line, so every
                // `tool_use` block in it shares that line's uuid as its
                // `sourceToolAssistantUUID`.
                if let ConversationMessage::Assistant { content, .. } = msg {
                    for block in content {
                        if let protocol::ContentBlock::ToolUse { id, .. } = block {
                            self.record_source_tool_assistant_uuid(id, line_uuid.clone())
                                .await;
                        }
                    }
                }
            }
            Err(e) => {
                self.record_transcript_append_failure(&session_id_str, "assistant_merged", &e)
                    .await;
            }
        }
    }

    /// Update the out-of-band assistant timestamp without changing message wire
    /// shape. Every production assistant commit passes through one of the JSONL
    /// persistence seams, including writer-less runtimes.
    async fn note_assistant_commit(&self, msg: &ConversationMessage) {
        if matches!(msg, ConversationMessage::Assistant { .. }) {
            self.session.lock().await.message_timing.last_assistant_at =
                Some(std::time::SystemTime::now());
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
            // 413 request-too-large (accumulated images/attachments): render the
            // byte-exact `$Vi()` notice instead of the opaque "request too large".
            LlmError::RequestTooLarge => request_too_large_notice(self.prompt_is_interactive()),
            // Billing (`Flp`: `yu({content:LYr,error:"billing_error"})`) and
            // prompt-too-long (`content:Jq`) both render BARE — no `API Error:`
            // prefix. Both used to fall through to `Display`, i.e. the words
            // "quota exceeded" and "context overflow".
            LlmError::QuotaExceeded => crate::api_error_copy::CREDIT_BALANCE_TOO_LOW.to_string(),
            // Dead OAuth session (`e instanceof qQt`): the IdP REJECTED the
            // stored refresh token, so no retry can help and only a fresh
            // sign-in will. Keyed on the variant, mirroring the oracle's
            // instanceof check rather than sniffing message text.
            LlmError::OAuthRefreshDead => {
                crate::api_error_copy::oauth_refresh_dead_text(self.prompt_is_interactive())
                    .to_string()
            }
            // Revoked OAuth token (`Uke`): a 403 whose message names it. Checked
            // BEFORE the x-api-key branch, matching the oracle's order, and
            // split on interactivity because a non-interactive caller cannot
            // run the auth command.
            //
            // The non-interactive half names a product, so it takes the LIVE
            // provider profile: this renderer is shared by every provider, and
            // the oracle's hardcoded "Claude" would be wrong for a session
            // routed elsewhere.
            LlmError::Authentication { .. } | LlmError::PermissionDenied { .. }
                if crate::api_error_copy::is_oauth_revoked(
                    err.http_status(),
                    err.provider_message().unwrap_or_default(),
                ) =>
            {
                let profile = self.session.lock().await.model_profile.clone();
                crate::api_error_copy::oauth_revoked_text(
                    self.prompt_is_interactive(),
                    profile.as_deref(),
                )
            }
            // Org policy turned the SUBSCRIPTION path off (`de_()` → `ce_`, a
            // 401/403 naming it). The oracle checks this BEFORE the API-key
            // disablement below, and the two are mirror images: this one sends
            // the user to an API key, that one sends them to sign-in. Ordering
            // them wrongly would hand a blocked user the remedy their org just
            // disabled.
            LlmError::Authentication { .. } | LlmError::PermissionDenied { .. }
                if crate::api_error_copy::is_oauth_org_not_allowed(
                    err.http_status(),
                    err.provider_message().unwrap_or_default(),
                ) =>
            {
                crate::api_error_copy::OAUTH_ORG_NOT_ALLOWED.to_string()
            }
            // Org policy turned API-key auth off (403 naming it). Checked
            // before the generic credential branch, and names the specific
            // thing THIS user has to unset.
            LlmError::Authentication { .. } | LlmError::PermissionDenied { .. }
                if crate::api_error_copy::is_api_key_auth_disabled(
                    err.http_status(),
                    err.provider_message().unwrap_or_default(),
                ) =>
            {
                let profile = self.session.lock().await.model_profile.clone();
                crate::api_error_copy::api_key_auth_disabled_text(
                    &self.config.credential_origin,
                    self.config.has_oauth_token,
                    profile.as_deref(),
                )
            }
            // Credential rejection. The oracle gates this on the MESSAGE naming
            // `x-api-key`, not on the status, then splits on where the key came
            // from: an env var or `apiKeyHelper` gets "fix that", everything
            // else gets "/login" — because /login cannot fix an external key.
            //
            // A 401/403 that does NOT name the header falls through to the
            // variant's own text, matching the oracle's outer `if`.
            LlmError::Authentication { .. } | LlmError::PermissionDenied { .. }
                if crate::api_error_copy::mentions_api_key_header(
                    err.provider_message().unwrap_or_default(),
                ) =>
            {
                // A cloud-hosted route names ITS credential problem instead —
                // "run gcloud auth ..." is useful advice, "/login" is not. The
                // 401-vs-other split inside is only decidable because the status
                // now survives in the message.
                // `if(qOu()) return UOu` comes FIRST in the oracle: on a
                // remote session the failure is reported as possibly transient
                // before anything looks at the credential's source.
                if crate::api_error_copy::is_remote_session() {
                    return crate::api_error_copy::AUTH_TRANSIENT.to_string();
                }
                // `xn()==="gateway"` comes next in the oracle. It IS reachable:
                // `Mt.gatewayAuth` is bootstrapped from the environment
                // (`CLAUDE_CODE_USE_GATEWAY` + `ANTHROPIC_BASE_URL` +
                // `ANTHROPIC_AUTH_TOKEN`), so the route resolves without any
                // runtime credential object. When a gateway fronts the provider
                // and IT cannot authenticate upstream, no credential the user
                // holds is at fault — say so instead of sending them to /connect.
                let route = self
                    .config
                    .error_route
                    .clone()
                    .unwrap_or_else(crate::api_error_copy::ErrorRouteTag::from_env);
                if matches!(route, crate::api_error_copy::ErrorRouteTag::Gateway) {
                    return crate::api_error_copy::GATEWAY_UPSTREAM_AUTH_FAILED.to_string();
                }
                crate::api_error_copy::cloud_credential_text(&route, err.http_status())
                    .unwrap_or_else(|| {
                        crate::api_error_copy::credential_rejected_text(
                            &self.config.credential_origin,
                        )
                        .to_string()
                    })
            }
            // TERMINAL 401/403 arm (@230607344). Every specific auth branch
            // above declined, so the oracle still renders an `API Error:` line
            // carrying the provider detail rather than falling through to the
            // variant's bare `Display` ("authentication failed" /
            // "permission denied"), which is what this port used to show.
            LlmError::Authentication { .. } | LlmError::PermissionDenied { .. } => {
                crate::api_error_copy::auth_failed_fallback(
                    self.prompt_is_interactive(),
                    // Oracle: `let i = sir(e)` — the SAME normalizer the retry
                    // banner uses, so both surfaces agree.
                    &llm_client::error_display_text(err),
                )
            }
            LlmError::ContextOverflow { .. } => crate::api_error_copy::PROMPT_TOO_LONG.to_string(),
            // 429: the oracle renders `API Error: Request rejected (429) · …`,
            // pulling the detail out of the JSON body the decoder stringified
            // into the message. This used to fall through to `Display`, which is
            // the bare words "rate limited".
            //
            // Two first-party variants are NOT selected here and are documented
            // as such in `api_error_copy`: the `Server is temporarily limiting
            // requests` label and the `hpo()` status-page/gateway suffix both
            // need provider-route plumbing this layer does not have. The
            // fallback clause only shows when the body carries no detail, which
            // a real 429 does.
            LlmError::RateLimited { .. } => {
                let raw = err.to_string();
                let source = self.api.last_rate_limit_error_message().unwrap_or(raw);
                if crate::api_error_copy::is_long_context_credit_message(&source) {
                    crate::api_error_copy::usage_credits_required_for_1m_context(false)
                } else {
                    // `let p = i ? le_ : "Request rejected (429)"`.
                    //
                    // INFERRED, not read off `eir`'s body: `i = eir(ii())` gates
                    // three branches here — the 1M-credits clamp, the
                    // overage-disabled-reason lookup, and this label — all of
                    // which are subscription concepts, so it reads as "this is a
                    // claude.ai subscriber". `is_subscriber` is LingXi's
                    // equivalent and is already threaded from the composition
                    // root. The distinction matters: telling a subscriber their
                    // request was "rejected" implies they hit their own limit,
                    // which is exactly what `le_` exists to deny.
                    let label = if self.config.is_subscriber {
                        crate::api_error_copy::SERVER_LIMITING
                    } else {
                        crate::api_error_copy::REQUEST_REJECTED_429
                    };
                    crate::api_error_copy::rate_limited_text(
                        &source,
                        label,
                        &crate::api_error_copy::capacity_fallback(Some(
                            &self
                                .config
                                .error_route
                                .clone()
                                .unwrap_or_else(crate::api_error_copy::ErrorRouteTag::from_env),
                        )),
                    )
                }
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
        let _turn_guard = self.turn_gate.lock().await;
        tracing::info!(
            event = orch_events::CONVERSATION_STARTED,
            prompt_len = prompt.len()
        );
        let result = traits::session_flags::scope_non_interactive_session(
            !self.prompt_is_interactive(),
            self.try_run_turn(prompt),
        )
        .await;
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
        // `transcript_path`, utils/hooks.ts:322) and the live permission mode.
        // The lifecycle hooks (Stop /
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
        let (session_id, plan_mode, last_assistant_message) = {
            let s = self.session.lock().await;
            (
                s.session_id,
                s.plan_mode,
                Self::last_assistant_message_for_hooks(&s.history),
            )
        };
        let transcript_path = self
            .jsonl_writer
            .as_ref()
            .map(|w| w.path().to_path_buf())
            .unwrap_or_else(|| self.computed_transcript_path(&session_id));
        // LingXi keeps `/plan` as session state, while Shift+Tab/control
        // requests mutate the enforcing permission gate. Prefer the explicit
        // plan state when active; otherwise report the gate's authoritative
        // live wire id instead of collapsing every non-plan mode to `default`.
        let permission_mode = if plan_mode {
            "plan".to_string()
        } else {
            self.permission_mode()
                .unwrap_or_else(|| "default".to_string())
        };
        // Main-thread lifecycle hooks (SessionStart / UserPromptSubmit / Stop /
        // expansion) carry the adopted `--agent`'s `agentType` — claude-code's
        // base hook-input builder `wf` uses `r?.agentType ?? MB()`, and these
        // firings have no tool-use context `r`, so they fall through to `MB()`
        // (the main-thread agent type). `None` when no `--agent` was applied.
        let agent_type = self.main_thread_agent_type().await;
        HookContext {
            session_id,
            cwd: self.current_cwd(),
            transcript_path,
            permission_mode: Some(permission_mode),
            stop_hook_active,
            last_assistant_message,
            agent_type,
            ..Default::default()
        }
    }

    fn last_assistant_message_for_hooks(
        history: &[protocol::ConversationMessage],
    ) -> Option<String> {
        let assistant = history.iter().rev().find_map(|message| match message {
            protocol::ConversationMessage::Assistant { content, .. } => Some(content),
            _ => None,
        })?;
        let joined = assistant
            .iter()
            .filter_map(|block| match block {
                protocol::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let trimmed = joined.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
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
    ///
    /// Two `hookSpecificOutput` fields are applied here (P2-04), matching the
    /// binary's prompt-hook switch:
    /// - `sessionTitle` renames the session via the same custom-title write
    ///   path as `/rename` (claude `kje(title,"hook")`), applied for any
    ///   outcome (not only on block).
    /// - On a `Block`, a warning message is rendered to the output stream —
    ///   claude `dPs`/`Tc(...,"warning",void 0,!0)`:
    ///   `"UserPromptSubmit operation blocked by hook:\n{reason}"`, and unless
    ///   the hook set `suppressOriginalPrompt`, `"\n\nOriginal prompt: {prompt}"`
    ///   is appended. The reason defaults to `"Blocked by hook"` when the hook
    ///   omits one (`e.reason||"Blocked by hook"`). The message is display-only
    ///   (isMeta warning) — the turn still aborts (`shouldQuery:!1`).
    async fn fire_user_prompt_submit(&self, prompt: &str, message_id: MessageId) -> bool {
        // The JSONL append immediately before this seam minted the stable
        // per-turn prompt id. Emit once for all batched/streaming/cancelable
        // prompt paths before hooks can block the API call.
        let prompt_id = self
            .current_prompt_id
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| message_id.as_uuid().to_string());
        telemetry::otel::emit_user_prompt_log(
            prompt,
            &prompt_id,
            &message_id.as_uuid().to_string(),
        );
        self.append_ultracode_attachments(prompt).await;
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

        // `hookSpecificOutput.sessionTitle` — rename the session (claude
        // applies it regardless of the block outcome). Best-effort: no writer
        // wired (library/test callers) ⇒ silent no-op, an empty title is
        // ignored.
        if let Some(title) = agg.session_title.as_deref() {
            if !title.is_empty() {
                if let Some(writer) = self.jsonl_writer.as_ref() {
                    let session_id = self.session.lock().await.session_id;
                    if let Err(error) = writer
                        .append_custom_title(&session_id.as_uuid().to_string(), title)
                        .await
                    {
                        self.record_transcript_append_failure(
                            &session_id.to_string(),
                            "hook_custom_title",
                            &error,
                        )
                        .await;
                    }
                }
            }
        }

        let blocked = matches!(agg.decision, Some(hooks::response::HookDecision::Block));
        if blocked {
            let reason = agg
                .reason
                .clone()
                .unwrap_or_else(|| "Blocked by hook".to_string());
            let base = format!("UserPromptSubmit operation blocked by hook:\n{reason}");
            let warning = if agg.suppress_original_prompt {
                base
            } else {
                format!("{base}\n\nOriginal prompt: {prompt}")
            };
            self.output.emit_text(&warning).await;
        }
        blocked
    }

    /// Advance the session-scoped Ultracode state at the one prompt-ingress
    /// seam shared by batched, streaming, and cancelable drivers.
    async fn append_ultracode_attachments(&self, prompt: &str) {
        use tool_api::tool_trait::ToolStaticContext;
        use tool_workflow::{UltracodeConfig, UltracodeGate, UltracodeState};

        let workflows_enabled = self
            .tools
            .available_tools(&ToolStaticContext::default())
            .iter()
            .any(|tool| tool.name() == tool_workflow::TOOL_NAME);
        let effort = self
            .current_effort
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let is_meta_turn = prompt.trim_start().starts_with('/');
        let attachments = {
            let mut session = self.session.lock().await;
            let mut state = UltracodeState {
                active: session.ultracode_active,
                non_meta_turns_since_reminder: session.ultracode_non_meta_turns_since_reminder,
            };
            let attachments = state.advance(
                UltracodeGate {
                    model: &session.model,
                    effort: effort.as_deref(),
                    workflows_enabled,
                },
                UltracodeConfig {
                    feature_flag_cadence: self.config.ultracode_feature_flag_cadence,
                    product_default_cadence: self.config.ultracode_product_default_cadence,
                    keyword_trigger_enabled: self.config.workflow_keyword_trigger_enabled,
                },
                prompt,
                is_meta_turn,
            );
            session.ultracode_active = state.active;
            session.ultracode_non_meta_turns_since_reminder = state.non_meta_turns_since_reminder;
            attachments
        };

        for attachment in attachments {
            let message = ConversationMessage::user_meta(
                MessageId::new(),
                format!("<system-reminder>\n{}\n</system-reminder>", attachment.text),
            );
            self.session.lock().await.history.push(message.clone());
            self.persist_message_to_jsonl(&message).await;
        }
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

    /// Fire the `MessageDisplay` completed-message pass and return the hook's
    /// display override, if any — the orchestrator twin of claude-code's `Qff`
    /// (BIN off 229876575). After an assistant message's text blocks finalize,
    /// claude-code fires `MessageDisplay` once with `{turnId, messageId:
    /// randomUUID(), index:0, final:!0, delta:<joined text>}` and folds the last
    /// `displayContent` any hook returns into the message's separate
    /// `displayedMessageContent` (the stored `message.content` is NEVER touched —
    /// "Display-only"). On a hook exception it logs
    /// `MessageDisplay hook failed for completed message; emitting original text:`
    /// and displays the original.
    ///
    /// `message_id` is a fresh per-pass UUID (claude-code `zfn.randomUUID()`),
    /// distinct from the assistant message's own id. Returns `Some(text)` when a
    /// hook supplied `displayContent`, else `None` (caller displays the original).
    /// Best-effort: a failing / blocking hook yields `None`, never affecting the
    /// stored message or the turn.
    async fn fire_message_display_completed(&self, turn_id: &str, text: &str) -> Option<String> {
        let ctx = self.lifecycle_hook_ctx(false).await;
        let agg = self
            .hooks
            .execute(
                HookEvent::MessageDisplay {
                    turn_id: turn_id.to_string(),
                    message_id: uuid::Uuid::new_v4().to_string(),
                    index: 0,
                    is_final: true,
                    delta: text.to_string(),
                },
                ctx,
            )
            .await;
        // claude-code's `try { … } catch(c) { log; return original }` — a
        // `MessageDisplay` hook that errored (non-zero exit / transport failure /
        // timeout) never overrides the display: log the byte-exact fallback and
        // fall through to the original text.
        if agg.display_content.is_none()
            && agg.all_results.iter().any(|(_, r)| {
                matches!(
                    r.outcome,
                    hooks::response::HookOutcome::Error | hooks::response::HookOutcome::Timeout
                )
            })
        {
            tracing::warn!(
                "MessageDisplay hook failed for completed message; emitting original text: {text}"
            );
        }
        agg.display_content
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
        let goal_hook_id = self
            .hooks
            .get_session_named_hook(ctx.session_id, GOAL_STOP_HOOK_NAME)
            .await
            .map(|hook| hook.id);
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
        let goal_disposition = self.goal_stop_hook_disposition(goal_hook_id, &agg).await;
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
        } else if let Some(disposition) = goal_disposition {
            disposition
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

    async fn goal_stop_hook_disposition(
        &self,
        goal_hook_id: Option<HookId>,
        agg: &hooks::AggregateHookResult,
    ) -> Option<StopHookDisposition> {
        let goal_hook_id = goal_hook_id?;
        let Some((_, result)) = agg
            .all_results
            .iter()
            .find(|(hook_id, _)| *hook_id == goal_hook_id)
        else {
            self.record_goal_evaluation(Some("Goal completion hook did not run".to_string()), true)
                .await;
            return Some(StopHookDisposition::GoalContinue(
                "Goal completion hook did not run".to_string(),
            ));
        };

        match result.outcome {
            hooks::HookOutcome::Success => {
                if let Some(response) = &result.response {
                    if matches!(response.decision, Some(hooks::HookDecision::Block)) {
                        let reason = response
                            .reason
                            .as_deref()
                            .map(strip_goal_prompt_block_reason)
                            .filter(|reason| !reason.is_empty())
                            .unwrap_or_else(|| "Goal not met yet".to_string());
                        self.record_goal_evaluation(Some(reason.clone()), true)
                            .await;
                        return Some(StopHookDisposition::GoalContinue(reason));
                    }
                }
                // The terminal `achieved` attachment below carries the updated
                // iteration count; avoid writing a redundant intermediate
                // `set` attachment for the same successful evaluation.
                self.record_goal_evaluation(None, false).await;
                let _ = self
                    .finish_active_goal_state_and_hook(traits::GoalStatusKind::Achieved)
                    .await;
                None
            }
            hooks::HookOutcome::Timeout
            | hooks::HookOutcome::Error
            | hooks::HookOutcome::Cancelled => {
                let reason = if !result.stderr.is_empty() {
                    result.stderr.clone()
                } else if !result.stdout.is_empty() {
                    result.stdout.clone()
                } else {
                    "Goal completion check failed".to_string()
                };
                self.record_goal_evaluation(Some(reason.clone()), true)
                    .await;
                Some(StopHookDisposition::GoalContinue(reason))
            }
        }
    }

    async fn record_goal_evaluation(&self, last_reason: Option<String>, persist_active: bool) {
        let snapshot = {
            let mut session = self.session.lock().await;
            let Some(goal) = session.active_goal.as_mut() else {
                return;
            };
            goal.iterations = goal.iterations.saturating_add(1);
            goal.last_reason = last_reason;
            goal.clone()
        };
        if persist_active {
            self.persist_active_goal_state_to_jsonl(Some(&snapshot))
                .await;
        }
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
            StopHookDisposition::GoalContinue(reason) => {
                *stop_hook_active = false;
                *stop_hook_blocking_count = 0;
                self.append_stop_hook_feedback(&reason).await;
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
    /// Returns both a possible blocking reason and the successful hooks' stdout.
    /// A block ABORTS the pass in every path (TS `VJn` throws
    /// `"Compaction blocked by PreCompact hook: <blockedBy>"` on the manual
    /// route; the proactive / reactive routes log and skip). The returned
    /// string is the port's aggregate `reason` (the closest equivalent of TS's
    /// `[cmd]: output` join); callers own the surfacing so the log wording
    /// matches each route (manual / `Precomputed` / `Reactive`).
    ///
    /// `custom_instructions` is the caller's current `/compact <focus>` text and
    /// is exposed in the hook payload. Exit-code-0 stdout is returned in hook
    /// order and appended to the same summary prompt.
    pub(crate) async fn fire_pre_compact(
        &self,
        trigger: &str,
        custom_instructions: Option<&str>,
    ) -> PreCompactHookOutcome {
        let ctx = self.lifecycle_hook_ctx(false).await;
        let agg = self
            .hooks
            .execute(
                HookEvent::PreCompact {
                    reason: trigger.to_string(),
                    custom_instructions: custom_instructions.map(str::to_owned),
                },
                ctx,
            )
            .await;
        let blocked_by = matches!(agg.decision, Some(hooks::response::HookDecision::Block))
            .then(|| agg.reason.clone().unwrap_or_default());
        let stdout: Vec<&str> = agg
            .all_results
            .iter()
            .filter(|(_, result)| result.outcome == hooks::response::HookOutcome::Success)
            .map(|(_, result)| result.stdout.trim())
            .filter(|text| !text.is_empty())
            .collect();
        PreCompactHookOutcome {
            blocked_by,
            additional_instructions: (!stdout.is_empty()).then(|| stdout.join("\n")),
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
        // `trigger` (`manual` / `auto`) is threaded onto the `PostCompact`
        // event so it becomes the TS `matchQuery` (claude `getMatchingHooks`
        // `i = r.trigger`) AND rides the wire payload — a hook matcher of
        // `"manual"` / `"auto"` now filters correctly.
        let ctx = self.lifecycle_hook_ctx(false).await;
        let _ = self
            .hooks
            .execute(
                HookEvent::PostCompact {
                    summary,
                    tokens_freed,
                    trigger: trigger.to_string(),
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
    /// Fire the `SessionStart` hooks once and return the folded aggregate.
    async fn run_session_start_hooks(&self, source: &str) -> hooks::response::AggregateHookResult {
        let session_id = { self.session.lock().await.session_id };
        let ctx = self.lifecycle_hook_ctx(false).await;
        self.hooks
            .execute(
                HookEvent::SessionStart {
                    session_id,
                    source: source.to_string(),
                },
                ctx,
            )
            .await
    }

    async fn collect_session_start_messages(&self, source: &str) -> Vec<ConversationMessage> {
        let agg = self.run_session_start_hooks(source).await;
        Self::session_start_context_messages(&agg)
    }

    /// Build the model-facing `hook_additional_context` meta message(s) from a
    /// folded `SessionStart` aggregate (empty when no hook emitted context).
    fn session_start_context_messages(
        agg: &hooks::response::AggregateHookResult,
    ) -> Vec<ConversationMessage> {
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
        if agg.additional_contexts.is_empty() {
            return Vec::new();
        }
        let body = agg.additional_contexts.join("\n");
        vec![ConversationMessage::user_meta(
            MessageId::new(),
            format!(
                "<system-reminder>\nSessionStart hook additional context: {body}\n</system-reminder>"
            ),
        )]
    }

    /// Fire SessionStart and append its model-facing additional context, then
    /// honor a `SessionStart` hook's `initialUserMessage` (injected as a non-meta
    /// user prompt — claude-code `if(p.initialUserMessage)$os=p.initialUserMessage`)
    /// and return the folded hook aggregate so the composition root can apply
    /// host-owned follow-up actions such as `reloadSkills`.
    pub async fn fire_session_start(&self, source: &str) -> hooks::response::AggregateHookResult {
        let agg = self.run_session_start_hooks(source).await;
        let messages = Self::session_start_context_messages(&agg);
        if !messages.is_empty() {
            // O3: the PERSISTED record is a `hook_additional_context`
            // ATTACHMENT line (2.1.220 BIN off 232675554). Note the three
            // LITERALS at that site — `hookName:"SessionStart"` (BARE, NOT
            // `SessionStart:{source}` as the hook-RUN attachments use) and
            // `toolUseID:"SessionStart"` (a literal, NOT a uuid). Confirmed by
            // 129 real 2.1.220 records.
            //
            // The `user_meta` message below is the EPHEMERAL model rendering
            // (`zr({content: Ww(…), isMeta:true})`, renderer BIN off
            // 238107100); it only enters `history` and is never persisted from
            // here, so this attachment is the sole on-disk record.
            self.persist_hook_attachment_to_jsonl(hooks::additional_context_attachment(
                "SessionStart",
                "SessionStart",
                "SessionStart",
                &agg.additional_contexts,
            ))
            .await;
            self.session.lock().await.history.extend(messages);
        }
        // `initialUserMessage` → seed the initial user prompt (claude-code `$os`).
        if let Some(initial) = &agg.initial_user_message {
            self.inject_user_message(initial).await;
        }
        agg
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
        self.fire_instructions_loaded_with_reason(
            hooks::events::InstructionsLoadReason::SessionStart,
        )
        .await;
    }

    async fn fire_instructions_loaded_with_reason(
        &self,
        load_reason: hooks::events::InstructionsLoadReason,
    ) {
        let cwd = self.cwd.clone();
        let memory_files = self.memory.load(&cwd).await;
        if memory_files.is_empty() {
            return;
        }
        // Seed the read-state registry BEFORE (and OUTSIDE) the hook loop.
        // Outside is load-bearing: the loop's `if file.globs.is_some() {
        // continue; }` skips conditional rules for the InstructionsLoaded fire,
        // but the oracle's `xCt` seeds them too — with `seededFromContext:
        // false` — so folding this into the loop would silently drop them.
        // Re-entry with `load_reason = Compact` is safe: `seed_memory_read_state`
        // skips every path already present.
        self.seed_memory_read_state(&memory_files).await;
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
                        // Top-level eager load (no `@include` parent). Session
                        // boot uses `session_start`; post-compact reload uses
                        // `compact`, matching Claude's memory-cache reload cause.
                        load_reason,
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

    /// Fire the `DirectoryAdded` hook after a working directory is added
    /// mid-session (2.1.219).
    ///
    /// `source` is both a payload field and the MATCHER QUERY — claude-code
    /// `a$t` dispatches with `matchQuery: t`, so a hook `matcher` is tested
    /// against `slash_command` / `register_repo_root`, not against the path. A
    /// matcher written against the directory would silently never fire.
    ///
    /// Best-effort, like the other lifecycle fires: a failing or absent hook
    /// must not undo a directory the user successfully added.
    pub async fn fire_directory_added(
        &self,
        directory: &str,
        source: &str,
    ) -> traits::DirectoryAddedHookSummary {
        let ctx = self.lifecycle_hook_ctx(false).await;
        let aggregate = self
            .hooks
            .execute(
                HookEvent::DirectoryAdded {
                    directory: directory.to_string(),
                    source: source.to_string(),
                },
                ctx,
            )
            .await;

        let mut failure_count = 0u32;
        for (hook_id, result) in &aggregate.all_results {
            if result.outcome != hooks::response::HookOutcome::Success {
                failure_count = failure_count.saturating_add(1);
                tracing::error!(
                    hook_id = %hook_id,
                    outcome = ?result.outcome,
                    stdout = %result.stdout,
                    stderr = %result.stderr,
                    "DirectoryAdded hook failed"
                );
            }
        }

        const PER_MESSAGE_LIMIT: usize = 4 * 1024;
        const TOTAL_LIMIT: usize = 16 * 1024;
        let mut remaining = TOTAL_LIMIT;
        let mut context_messages = Vec::new();
        for message in aggregate
            .system_messages
            .iter()
            .chain(aggregate.additional_contexts.iter())
        {
            if remaining == 0 {
                break;
            }
            let cap = PER_MESSAGE_LIMIT.min(remaining);
            let mut used = 0usize;
            let bounded: String = message
                .chars()
                .take_while(|ch| {
                    let width = ch.len_utf8();
                    if used.saturating_add(width) > cap {
                        false
                    } else {
                        used += width;
                        true
                    }
                })
                .collect();
            remaining = remaining.saturating_sub(bounded.len());
            if !bounded.is_empty() {
                context_messages.push(bounded);
            }
        }
        if failure_count > 0 {
            context_messages.push(format!(
                "{failure_count} DirectoryAdded hook(s) failed; output is in the debug log, not shown here"
            ));
        }

        if !context_messages.is_empty() {
            let body = context_messages
                .iter()
                .map(|message| format!("DirectoryAdded hook: {message}"))
                .collect::<Vec<_>>()
                .join("\n");
            let message = ConversationMessage::user_meta(
                MessageId::new(),
                format!("<system-reminder>\n{body}\n</system-reminder>"),
            );
            self.session.lock().await.history.push(message.clone());
            self.persist_message_to_jsonl(&message).await;
        }

        traits::DirectoryAddedHookSummary {
            failure_count,
            context_messages,
        }
    }

    /// Register a canonical additional repository root for the live session.
    ///
    /// The trusted-root cell is updated before MCP/sandbox consumers and before
    /// `DirectoryAdded` hooks, so a hook observing the event cannot race the
    /// permission boundary it was told had changed.
    pub async fn register_repo_root(
        &self,
        request: traits::RegisterRepoRootRequest,
    ) -> Result<traits::RegisterRepoRootOutcome, traits::HandleError> {
        let current = self.session_cwd.cwd();
        let raw = std::path::PathBuf::from(request.path.trim());
        let candidate = if raw.is_absolute() {
            raw
        } else {
            current.join(raw)
        };
        let canonical = std::fs::canonicalize(&candidate).map_err(|_| {
            traits::HandleError::ActionFailed(
                "register_repo_root: target is not a directory".into(),
            )
        })?;
        if !canonical.is_dir() {
            return Err(traits::HandleError::ActionFailed(
                "register_repo_root: target is not a directory".into(),
            ));
        }

        let current_scope = current
            .parent()
            .and_then(|path| std::fs::canonicalize(path).ok())
            .unwrap_or_else(|| current.clone());
        let home_scope = dirs::home_dir().and_then(|path| std::fs::canonicalize(path).ok());
        if !canonical.starts_with(&current_scope)
            && !home_scope
                .as_ref()
                .is_some_and(|home| canonical.starts_with(home))
        {
            return Err(traits::HandleError::ActionFailed(
                "register_repo_root: target is outside the allowed registration scope".into(),
            ));
        }

        // Sandbox/file permission refresh FIRST.
        if !self.session_cwd.add_trusted_dir(canonical.clone()) {
            return Err(traits::HandleError::ActionFailed(
                "register_repo_root: target is already a registered working directory".into(),
            ));
        }
        // Then refresh MCP roots; hooks run only after both live consumers see
        // the new directory.
        if let Some(registry) = &self.mcp_registry {
            registry.add_root(canonical.clone());
            registry.notify_roots_list_changed_all().await;
        }

        let directory = canonical.to_string_lossy().into_owned();
        let hooks = self
            .fire_directory_added(&directory, "register_repo_root")
            .await;

        if request.reload_claude_md {
            self.fire_instructions_loaded_with_reason(
                hooks::events::InstructionsLoadReason::NestedTraversal,
            )
            .await;
        }
        let reload = if request.reload_skills || request.reload_plugins {
            if let Some(reloader) = &self.repo_root_reloader {
                reloader
                    .reload(traits::RepoRootReloadRequest {
                        root: canonical.clone(),
                        reload_skills: request.reload_skills,
                        reload_plugins: request.reload_plugins,
                    })
                    .await
            } else {
                traits::RepoRootReloadOutcome {
                    errors: vec![
                        "catalog reload unavailable in this runtime; repository root was registered"
                            .to_string(),
                    ],
                    ..traits::RepoRootReloadOutcome::default()
                }
            }
        } else {
            traits::RepoRootReloadOutcome::default()
        };
        for error in &reload.errors {
            tracing::warn!(%error, root = %canonical.display(), "register_repo_root reload failed");
        }

        Ok(traits::RegisterRepoRootOutcome {
            directory: canonical,
            added: true,
            hooks,
            skills_reloaded: reload.skills_reloaded,
            plugins_reloaded: reload.plugins_reloaded,
            reload_errors: reload.errors,
        })
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
        // O2: the PERSISTED record is a `hook_stopped_continuation` attachment
        // (BIN off 233101239): `Va({type:"hook_stopped_continuation",
        // message:N, hookName:"Stop", toolUseID:H, hookEvent:"Stop"})`, where
        // `N = B.stopReason||"Stop hook prevented continuation"` and `H` is the
        // Stop dispatch's `hook-${randomUUID()}` — the SAME id shape the
        // dispatch's `hook_additional_context` record uses (BIN off 233240830).
        //
        // The derived meta message is kept in live API history but is not written
        // as a second JSONL row. Cold resume performs the same normalization from
        // this attachment, so the on-disk transcript has one source of truth.
        self.persist_hook_attachment_to_jsonl(hooks::stopped_continuation_attachment(
            &hooks::HookAttachmentIdentity {
                hook_name: "Stop".to_string(),
                hook_event: "Stop".to_string(),
                tool_use_id: format!("hook-{}", protocol::HookId::new().as_uuid()),
            },
            reason,
        ))
        .await;
        let content = format!(
            "<system-reminder>\nStop hook stopped continuation: {reason}\n</system-reminder>"
        );
        let msg = ConversationMessage::user_meta(MessageId::new(), content);
        {
            let mut s = self.session.lock().await;
            s.history.push(msg);
        }
    }

    async fn try_run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError> {
        // 0. Build the system prompt for THIS turn.
        // claude-code `nre` precedence: `--system-prompt` (override) wins; else
        // the `--agent`-adopted main-thread agent's prompt; else the default.
        let system_prompt: Option<String> = Some(self.effective_system_prompt().await);

        // A side query left unfinished when the previous user turn ended was
        // keyed to that previous prompt. Never surface it against new intent.
        self.discard_stale_prefetches().await;

        // 1. Append the user prompt to session history.
        let user_msg = ConversationMessage::user(MessageId::new(), prompt.to_string());
        {
            let mut s = self.session.lock().await;
            s.history.push(user_msg.clone());
        }
        self.persist_message_to_jsonl(&user_msg).await;

        // hooks B4: fire UserPromptSubmit. A Block decision aborts the turn
        // BEFORE any API call (TS prompt-ingress hook). No-op when unregistered.
        if self.fire_user_prompt_submit(prompt, user_msg.id()).await {
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
            // Streaming twin already drains here (query.ts ~1570). Claude Code
            // has one main loop; the batched print path must consume mid-turn
            // input before max_turns / budget so a queued message is not dropped.
            self.drain_mid_turn_input().await;
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
                    if stop_reason == "end_turn"
                        && self
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
        let _turn_guard = self.turn_gate.lock().await;
        tracing::info!(
            event = orch_events::TURN_STREAMING_STARTED,
            prompt_len = prompt.len()
        );
        // DEFERRED-3: the plain (non-cancelable) streaming entry has no granular
        // user-interrupt token → `None` (behaviour byte-identical to before).
        let result = traits::session_flags::scope_non_interactive_session(
            !self.prompt_is_interactive(),
            self.try_run_turn_streaming(prompt, Vec::new(), None, None, false),
        )
        .await;
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

    /// Start a model turn solely to deliver completed async-hook responses.
    ///
    /// No synthetic human prompt is added to history or JSONL. The completed
    /// hook buffer contributes the transient `async_hook_response` meta user
    /// message during request assembly, so the provider still receives a valid
    /// user boundary while the transcript remains faithful.
    pub async fn run_async_hook_rewake(&self) -> Result<TurnOutcome, OrchestratorError> {
        let _turn_guard = self.turn_gate.lock().await;
        self.output.emit_turn_started().await;
        let result = traits::session_flags::scope_non_interactive_session(
            !self.prompt_is_interactive(),
            self.try_run_turn_streaming("", Vec::new(), None, None, true),
        )
        .await;
        self.emit_terminal_rate_limit_if_changed(&result).await;
        match result {
            Ok(
                ConversationOutcome::EndTurn { .. } | ConversationOutcome::StopHookPrevented { .. },
            ) => Ok(TurnOutcome::EndTurn),
            Err(OrchestratorError::MaxTurnsReached { .. }) => Ok(TurnOutcome::MaxTurns),
            Err(error) => {
                let error = self.enrich_api_error(error);
                self.output
                    .emit_system_notice(&error.to_string(), true)
                    .await;
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn("error", &cost).await;
                Err(error)
            }
        }
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
        message_id: Option<MessageId>,
        transient_rewake: bool,
    ) -> Result<ConversationOutcome, OrchestratorError> {
        use crate::streaming_loop::ExecutorPump;
        use protocol::ContentBlock;

        // Startup Responses WebSocket prewarm is strictly opportunistic. A real
        // user turn must never wait for an in-flight `generate=false` request to
        // finish before it can open its own stream.
        self.abort_startup_responses_websocket_prewarm();

        // 0. Build the system prompt for THIS turn.
        // claude-code `nre` precedence: `--system-prompt` (override) wins; else
        // the `--agent`-adopted main-thread agent's prompt; else the default.
        let system_prompt: Option<String> = Some(self.effective_system_prompt().await);

        // A side query left unfinished when the previous user turn ended was
        // keyed to that previous prompt. Never surface it against new intent.
        self.discard_stale_prefetches().await;

        // 1. Append the user prompt (+ any pasted images) to session history.
        // `images` arrives already decoded (path-based callers ran `load_images`
        // first; the bridge converts inline `ImageRefDto`s straight to sources).
        let user_msg = ConversationMessage::user_with_images(
            message_id.unwrap_or_default(),
            prompt.to_string(),
            images,
        );
        let prior_message_id = {
            let mut s = self.session.lock().await;
            let prior = s.history.last().map(ConversationMessage::id);
            if !transient_rewake {
                s.history.push(user_msg.clone());
            }
            prior
        };
        if !transient_rewake {
            self.persist_message_to_jsonl(&user_msg).await;
        }

        // (/rewind) Snapshot the pre-turn file state IN MEMORY, keyed by this
        // user message, so `track_edit` (fired by Edit/Write/NotebookEdit during
        // the turn) records each file's pre-edit backup into it. The POPULATED
        // record is persisted to the transcript at TURN END (see below) — NOT
        // here: at turn start the backup map is empty (no edits yet), and
        // persisting it now would leave disk-based restore (`rewind_from_disk`,
        // which runs after the TUI unwinds and rebuilds the index from these
        // lines) with nothing to restore.
        let file_history_msg_id = (!transient_rewake).then(|| user_msg.id().as_uuid());
        if let (Some(fh), Some(message_id)) = (&self.file_history, file_history_msg_id) {
            fh.make_snapshot(message_id).await;
        }

        // hooks B4: UserPromptSubmit (streaming twin). A Block aborts before the
        // first stream is opened. No-op when unregistered.
        if !transient_rewake && self.fire_user_prompt_submit(prompt, user_msg.id()).await {
            return Ok(ConversationOutcome::StopHookPrevented {
                turn_count: 0,
                final_message_id: user_msg.id(),
            });
        }

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
        let mut last_message_id = prior_message_id.unwrap_or_else(|| user_msg.id());
        loop {
            // MID-TURN DRAIN (claude-code query.ts ~1570-1580): drain BEFORE
            // every terminal top-of-loop guard, including `max_turns`. A message
            // can arrive while the previous model/tool step is running; returning
            // for the cap before this consume-once source is polled would discard
            // it. Claude Code 2.1.205 preserves that message when `--max-turns`
            // ends the turn, so inject it into the persisted history first.
            // With no source wired this remains a strict no-op.
            self.drain_mid_turn_input().await;

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
                    self.inject_user_message(INTERRUPT_MESSAGE).await;
                }
                final_message_id = last_message_id;
                break;
            }

            // P0.1 (streaming twin): arm the memory-selector prefetch CONCURRENTLY
            // with this turn (claude-code `wAo`). Fired here at turn start so the
            // in-flight handle is ready when `relevant_memory_reminder_messages`
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
            // The connect-phase streaming 413 path below reuses the same
            // reactive truncate/compact recovery loop as the batched path.
            self.maybe_compact_before_call().await;
            // 2.1.232: accepted peer inbox → user-role `<cross-session-message>`
            // before the outgoing snapshot is cloned from history.
            let _ = self.drain_peer_inbox(false).await;

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
            self.prepend_leading_context(&mut snapshot).await;

            // Per-turn TRANSIENT reminders are collected here rather than
            // pushed straight onto `snapshot`, because `snapshot` is MOVED into
            // `stream()` and every recovery path below rebuilds it from
            // `session.history`. Each of these advances session state when it
            // is computed (sent-sets, delta trackers, consume-once drains), so
            // recomputing them on a retry would return `None` and the reminder
            // would be silently lost for the rest of the session. Computed
            // ONCE, re-appended on every re-snapshot — the same discipline
            // `deferred_reminder` and `date_change_reminder` already follow.
            let mut turn_reminders: Vec<ConversationMessage> = Vec::new();

            // OUTSTYLE.3 (streaming twin): per-turn, transient output-style
            // reminder. Appended to THIS turn's OUTGOING snapshot only — never to
            // `session.history` / JSONL — so it is recomputed each turn and never
            // accumulates (TS recomputes attachments each turn). Injected BEFORE
            // the blocking-limit estimate below so the reminder's tokens are
            // counted in the prompt size, matching the model input. Trailing
            // position mirrors TS; `None` for the default style ⇒ no extra
            // message, keeping the locked streaming fixtures byte-identical. See
            // [`Self::output_style_reminder_message`].
            if let Some(reminder) = self.output_style_reminder_message().await {
                turn_reminders.push(reminder);
            }

            // PLANMODE (streaming twin): per-turn, transient `plan_mode` reminder
            // (206 `xEg` attachment). Emitted ONLY while `session.plan_mode` is
            // active (after `EnterPlanMode`), so the DEFAULT build stays
            // byte-identical and the locked streaming fixtures still pass. Placed
            // right after the output-style reminder and BEFORE the skill-listing
            // reminder — identical position to the batched twin (`turn_loop.rs`)
            // and to 206 `KJn` (`l=await xEg(t)` spread before the invoked-skills
            // bodies and tool/mcp deltas). claude-code has ONE main loop, so both
            // LingXi twins must inject this reminder. Appended to THIS turn's
            // OUTGOING snapshot only (never `session.history` / JSONL) and BEFORE
            // the blocking-limit estimate below so its tokens are counted in the
            // prompt size. See [`Self::plan_mode_reminder_message`].
            if let Some(reminder) = self.plan_mode_reminder_message().await {
                turn_reminders.push(reminder);
            }

            // SKILLLIST.1 (streaming twin): per-turn, transient `skill_listing`
            // reminder so the model can discover skills. Appended to THIS turn's
            // OUTGOING snapshot only (never to `session.history` / JSONL), after
            // the output-style reminder and BEFORE the blocking-limit estimate
            // below so its tokens are counted in the prompt size. `None` when no
            // provider is wired / no skills / the Skill tool is absent. See
            // [`Self::skill_listing_reminder_message`].
            if let Some(reminder) = self.skill_listing_reminder_message().await {
                turn_reminders.push(reminder);
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
                turn_reminders.push(reminder);
            }

            // Nested memory (streaming twin): the LINGXI.md governing the
            // directory of a touched file. Appended to THIS turn's OUTGOING
            // snapshot only (never `session.history` / JSONL), directly after
            // the conditional-rules reminder and BEFORE the blocking-limit
            // estimate below so its tokens are counted in the prompt size —
            // identical position to the batched twin (`turn_loop.rs`).
            // claude-code has ONE main loop, so both LingXi twins must inject
            // it. See [`Self::nested_memory_reminder_message`].
            if let Some(reminder) = self.nested_memory_reminder_message().await {
                turn_reminders.push(reminder);
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
                turn_reminders.push(reminder);
            }

            // `agent_listing_delta` (streaming twin): per-turn, transient agent
            // catalog reminder, emitted ONLY when the
            // `LINGXI_AGENT_LIST_IN_MESSAGES` gate is ON (default OFF ⇒
            // `None`, keeping the locked streaming fixtures byte-identical and
            // the inline catalog in place). Appended to THIS turn's OUTGOING
            // snapshot only (never `session.history` / JSONL). See
            // [`Self::agent_listing_reminder_message`].
            if let Some(reminder) = self.agent_listing_reminder_message().await {
                turn_reminders.push(reminder);
            }

            // Finding #73 (streaming twin): per-turn, transient `todo_reminder`
            // (V1) / `task_reminder` (V2) reminder. Same gates as the batched
            // twin (killswitch / tool-present / Brief-absent / non-empty history
            // / both counters at threshold). The body is wrapped in a
            // `<system-reminder>` envelope and marked `isMeta`, matching the
            // oracle's `Zy([kn({content:o,isMeta:!0})])` (2.1.238 @296690005).
            // Placed after the agent-listing reminder and before the async-hook
            // reminder, mirroring the binary `ytl` order (`todo_reminders` in the
            // core `A` array, before the main-only `async_hook_responses`).
            // Appended to THIS turn's OUTGOING snapshot only (never
            // `session.history` / JSONL). `None` keeps the locked streaming
            // fixtures byte-identical. See [`Self::todo_reminder_message`].
            if let Some(reminder) = self.todo_reminder_message().await {
                turn_reminders.push(reminder);
            }

            // async_hook_response (streaming twin): fold completed background
            // (`async`) hook responses into THIS turn's OUTGOING snapshot only
            // (never `session.history` / JSONL), drained consume-once. `None`
            // when no source is wired / nothing completed since the last turn.
            // See [`Self::async_hook_response_reminder_message`].
            if let Some(reminder) = self.async_hook_response_reminder_message().await {
                turn_reminders.push(reminder);
            }

            // T35: fold the terminal background tasks finished since the last
            // turn into THIS turn's OUTGOING snapshot only (never
            // `session.history` / JSONL), drained consume-once so each completion
            // surfaces exactly one `<task-notification>`. `None` when no registry
            // is wired / nothing finished. See
            // [`Self::task_notification_reminder_message`].
            if let Some(reminder) = self.task_notification_reminder_message().await {
                turn_reminders.push(reminder);
            }

            // P0.1 (streaming twin): per-turn, transient `relevant_memories`
            // SURFACING reminders — the memory-selector/prefetch result rendered
            // as one meta user message per surfaced memory. Appended to THIS
            // turn's OUTGOING snapshot only (never `session.history` / JSONL),
            // after the async-hook reminder and BEFORE the blocking-limit estimate
            // below so its tokens are counted in the prompt size. Awaits the
            // prefetch armed by `start_memory_prefetch` at turn start. Empty when
            // no prefetch is wired / empty result / everything already injected.
            // See [`Self::relevant_memory_reminder_messages`].
            turn_reminders.extend(self.relevant_memory_reminder_messages().await);

            // EXPERIMENTAL_SKILL_SEARCH (streaming twin): per-turn, transient
            // `skill_discovery` SURFACING reminder, collected AFTER the memory
            // consume above (bundle order: memory consume → `collectSkill...`).
            // Appended to THIS turn's OUTGOING snapshot only. `None` when no
            // prefetch is wired (default OFF) / empty / everything already
            // surfaced. See [`Self::skill_discovery_reminder_message`].
            if let Some(reminder) = self.skill_discovery_reminder_message().await {
                turn_reminders.push(reminder);
            }

            // `silent_turn_reminder` (streaming twin, NEW in 2.1.238): nudge the
            // model to say something when it has worked several turns in a row
            // in silence. Producer `K4T` @296525255, gate @296520120 (main agent,
            // tool-round continuation only, model capability / env). The port has
            // no model-capability table, so the DEFAULT IS OFF (`None`) and the
            // locked streaming fixtures stay byte-identical. Positioned late in
            // the batch, mirroring the oracle's fan-out order
            // (`…critical_system_reminder, silent_turn_reminder,
            // total_tokens_reminder, budget_usd`). Appended to THIS turn's
            // OUTGOING snapshot only. See [`Self::silent_turn_reminder_message`].
            if let Some(reminder) = self.silent_turn_reminder_message().await {
                turn_reminders.push(reminder);
            }

            // `total_tokens_reminder` (streaming twin): the
            // `<total_tokens>N tokens left</total_tokens>` budget block. Producer
            // `D3T` @296556375; the oracle emits it after every tool-result batch
            // and (with `totalTokensReminderAfterUserTurn`) after each regular
            // user prompt. DEFAULT OFF in the port — see the divergence note on
            // [`crate::prompt::total_tokens`] — so this is a strict no-op unless
            // `CLAUDE_CODE_TOTAL_TOKENS_REMINDER` is set. Placed directly after
            // the silent-turn reminder, matching the oracle's fan-out order.
            // Appended to THIS turn's OUTGOING snapshot only. See
            // [`Self::total_tokens_reminder_message`].
            if let Some(reminder) = self.total_tokens_reminder_message().await {
                turn_reminders.push(reminder);
            }

            // Rebuild on every model step: a ToolSearch result marks schemas as
            // discovered, so the immediately following request must include
            // those schemas with `defer_loading:true`. The accompanying
            // `deferred_tools_delta` reminder is transient and prepended exactly
            // once per outgoing request. `deferred_tools_reminder_message`
            // ADVANCES the announced-set tracking, so compute it ONCE per model
            // step here and reuse this value on every retry/fallback re-snapshot
            // below (each of which rebuilds the SAME step's request).
            snapshot.extend(turn_reminders.iter().cloned());

            let wire_tools = self.build_wire_tools().await;
            let deferred_reminder = self.deferred_tools_reminder_message();
            if let Some(reminder) = deferred_reminder.clone() {
                self.prepend_transient_leading_context(&mut snapshot, reminder);
            }

            // `date_change` (streaming twin): sessions crossing local midnight
            // tell the model the new date once. Prepended AFTER the deferred
            // insert so the final order is [date_change, deferred_tools_delta,
            // …] — matching the oracle attachment batch order (`Ky("date_change")`
            // before `Ky("deferred_tools_delta")`). Computed ONCE per model
            // step and reused by the retry/fallback re-snapshots below, exactly
            // like `deferred_reminder`; the dedupe is committed only once the
            // stream actually opens (below the blocking-limit preempt). `None`
            // (the overwhelmingly common same-date case) keeps the locked
            // streaming fixtures byte-identical. See
            // [`Self::date_change_reminder_message`].
            let date_change_reminder =
                self.date_change_reminder_message(self.session.lock().await.session_id);
            if let Some(reminder) = date_change_reminder.clone() {
                self.prepend_transient_leading_context(&mut snapshot, reminder);
            }

            // RECOV.1: blocking-limit preempt — the streaming twin of the batched
            // `call_api_with_ptl_recovery` step (1) (TS `query.ts:592-648`). If the
            // pre-call prompt is already at the hard blocking limit
            // (`token_usage >= effective_window − MANUAL_COMPACT_BUFFER_TOKENS`),
            // surface the byte-exact `PROMPT_TOO_LONG_ERROR_MESSAGE` and END the
            // turn WITHOUT opening the stream — mirroring the batched path (which
            // returns `PtlCallOutcome::PromptTooLong` ⇒ ends with stop_reason
            // `"prompt_too_long"`). Use the exact same custom beta set as the
            // provider request so context-1m sessions are not preempted at the
            // default 200k boundary.
            let active_betas = self.api.active_betas();
            let warning = compaction::calculate_token_warning_state(
                compaction::grouping::estimate_tokens_for_range(&snapshot),
                &model,
                &active_betas,
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
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn("blocking_limit", &cost).await;
                final_message_id = id;
                break;
            }
            // Past the preempt: this step's snapshot WILL be sent, so the
            // `date_change` reminder it carries counts as delivered.
            self.commit_date_change_reminder();

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

            // P1-04 (cc 2.1.199 partial-stream finalize): set by the pump error
            // arm when a completed partial is finalized in place. Drives the
            // "API Error: … may be incomplete." notice (surfaced after the partial
            // is persisted) and the terminal turn-end below. Reset per turn.
            let mut partial_finalize: Option<crate::streaming_loop::PartialFinalizeCause> = None;
            let mut partial_finalize_notice_id: Option<MessageId> = None;

            // hooks #39: MessageDisplay fires at the BEGIN of this assistant
            // message's stream (claude-code `begin(d)`, BIN off 208862320),
            // mirroring its `o={apiMessageId:d, messageId:randomUUID(),
            // turnId:r, index:0, …}` initialization. The `turn_id` is a fresh
            // per-turn UUID (`newTurn(){…; r=randomUUID()}`). Best-effort +
            // no-op when unregistered, so existing flows are byte-identical.
            let turn_id = uuid::Uuid::new_v4().to_string();
            self.fire_message_display(&turn_id, assistant_id).await;

            // P2-04 (MessageDisplay `displayContent`): a registered
            // `MessageDisplay` hook makes the live pump SUPPRESS per-token text
            // deltas so the completed-message pass below renders the full
            // (possibly hook-substituted) text once — faithful to claude-code
            // `Qff` (BIN off 229876575), whose live render flows through the
            // display flush, not raw deltas. `false` (no hook) ⇒ byte-identical
            // live streaming. Cheap subscription gate (declared-event only).
            let display_hook_active = self
                .hooks
                .has_hooks_for(&hooks::events::HookEventType::MessageDisplay)
                .await;

            let mut exec = match &user_cancel {
                Some(token) => {
                    crate::streaming_executor::StreamingToolExecutor::new_with_user_cancel(
                        self,
                        token.clone(),
                    )
                }
                None => crate::streaming_executor::StreamingToolExecutor::new(self),
            };
            // Hold `tool_result` frames until the collection point below can
            // release them in RECEIVED order, with a cancelled tool's synthetic
            // already substituted. Off again after the drive, so any later
            // emission on this orchestrator goes straight out.
            self.set_tool_frame_buffering(true).await;

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
            let mut did_fall_back_to_non_streaming = false;

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
                    self.reattach_outgoing_context(
                        &mut recov_snapshot,
                        deferred_reminder.as_ref(),
                        date_change_reminder.as_ref(),
                        &turn_reminders,
                    )
                    .await;
                    match call_api_with_ptl_recovery(
                        self,
                        system_prompt.as_deref(),
                        &recov_model,
                        recov_profile.as_deref(),
                        recov_snapshot,
                        wire_tools.clone(),
                        None,
                        deferred_reminder.clone(),
                        date_change_reminder.clone(),
                        &turn_reminders,
                    )
                    .await?
                    {
                        PtlCallOutcome::Response(resp) => {
                            // Replay the recovered non-streaming response exactly
                            // like the 529 fallback below: emit text live, rebuild
                            // a fresh executor, register its tool_uses, and flow on
                            // as the turn's `pumped` result.
                            let pumped_from_recovery = llm_response_to_pumped_turn(&resp);
                            // P2-04: when a `MessageDisplay` hook is active the
                            // completed-message pass below is the single on-screen
                            // render (with `displayContent` substitution) — skip the
                            // direct whole-body emit here to avoid double display.
                            if !display_hook_active {
                                for blk in &pumped_from_recovery.assistant_blocks {
                                    if let ContentBlock::Text { text } = blk {
                                        self.output.emit_text(text).await;
                                    }
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
                    let id = crate::turn_loop::surface_model_error(
                        self,
                        &self.model_error_text(&other).await,
                        env,
                    )
                    .await;
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn("model_error", &cost).await;
                    final_message_id = id;
                    break;
                }
            };

            // 3. Pump the stream (with mid-stream 529 → non-streaming fallback OR
            //    the cc 2.1.199 partial-finalize, whichever applies).
            //
            // Task 7 / claude.ts parity: if the stream errors with `LlmError::Overloaded`
            // after the first event — AND `LINGXI_DISABLE_NONSTREAMING_FALLBACK` is not
            // set — AND no content block completed yet — issue a fresh non-streaming call
            // seeded with `initial_consecutive_overloaded = 1`.  This mirrors
            // `claude.ts:2469-2594` + `withRetry.ts:186` (`initialConsecutive529Errors`).
            //
            // The env gate name is locked byte-for-byte to TS:
            //   `process.env.LINGXI_DISABLE_NONSTREAMING_FALLBACK` (claude.ts:2470)
            // Truthiness follows `isEnvTruthy` (non-empty, non-"false", non-"0").
            //
            // P1-04 (cc 2.1.199, binary-verified): once a REAL content block has
            // completed — or a transport close leaves visible text whose stop
            // frame was lost — the partial is no longer discarded. The
            // `partial_has_output` arm in the `match pump_outcome` below finalizes
            // it in place (synthesized stop_reason + usage + `tengu_streaming_partial_finalized`),
            // persists the streamed blocks, and appends the "API Error: … may be
            // incomplete." notice — instead of the pre-2.1.199 discard-and-refetch.
            // The non-streaming fallback therefore fires only when the stream
            // produced no recoverable output. Provider errors before the first
            // completed block retain cc's `_r`-length behavior.
            // In both cases partial deltas already reached callers LIVE via
            // `event_router.rs` → `output.emit_text` at each `TextDelta`
            // (claude.ts:2210 `yield m`).
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
                    let pump_outcome: Result<
                        crate::streaming_loop::PumpedTurn,
                        crate::streaming_loop::PumpFailure,
                    > = loop {
                        match crate::streaming_loop::pump_stream_with_executor_tracked(
                            cur_stream,
                            &self.output,
                            ExecutorPump {
                                executor: &mut exec,
                                assistant_id,
                                user_cancel: user_cancel.as_ref(),
                                suppress_live_text: display_hook_active,
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
                                    mid_stream_retries - 1,
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
                                self.reattach_outgoing_context(
                                    &mut re_snapshot,
                                    deferred_reminder.as_ref(),
                                    date_change_reminder.as_ref(),
                                    &turn_reminders,
                                )
                                .await;
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
                                    // pump error for the arms below. No partial to
                                    // finalize here — the retry only fires while
                                    // `!real_content_started`, so nothing real was
                                    // yielded on the attempt we are abandoning.
                                    Err(e) => {
                                        break Err(crate::streaming_loop::PumpFailure {
                                            error: OrchestratorError::Streaming(e),
                                            real_content_started: false,
                                            partial: crate::streaming_loop::PumpedTurn::default(),
                                        })
                                    }
                                }
                            }
                            Err(f) => break Err(f),
                        }
                    };
                    match pump_outcome {
                        Ok(p) => p,
                        // P1-04 (cc 2.1.199 partial-stream finalize, binary-verified): a
                        // finalize-class mid-stream error (server/overloaded/api error,
                        // watchdog stall, or connection close) that landed after useful
                        // output is not discarded. The already-streamed
                        // partial is finalized in place — persisted with a synthesized
                        // `stop_reason` (`tool_use` if any tool_use else `end_turn`) +
                        // usage — `tengu_streaming_partial_finalized` fires, and a byte-exact
                        // "API Error: … The response above may be incomplete." notice is
                        // surfaced after it (see the notice + terminal sites below, gated on
                        // `partial_finalize`). This runs BEFORE the 529 non-streaming
                        // fallback so a completed-partial 529 keeps its streamed output
                        // instead of re-fetching; a 529 that erred before any block
                        // completed (no output) falls through to the fallback as before.
                        Err(f)
                            if crate::streaming_loop::partial_has_output(&f.partial)
                                && crate::streaming_loop::partial_finalize_cause(&f.error)
                                    .is_some() =>
                        {
                            let cause = crate::streaming_loop::partial_finalize_cause(&f.error)
                                .expect("finalize cause present (guarded above)");
                            let mut partial = f.partial;
                            // cc `gm=vd?"tool_use":"end_turn"`: a dispatched tool_use makes
                            // this a tool turn, else a natural end.
                            let synthesized_stop_reason = if partial.tool_uses.is_empty() {
                                "end_turn"
                            } else {
                                "tool_use"
                            };
                            partial.stop_reason = Some(synthesized_stop_reason.to_string());
                            // cc `_r.length`: one yielded message per completed content block.
                            let blocks_yielded =
                                partial.assistant_blocks.len() + partial.tool_uses.len();
                            if let Some(bus) = self.analytics_bus.as_ref() {
                                let mut md = telemetry::LogEventMetadata::new();
                                md.insert(
                                    "model".into(),
                                    telemetry::AnalyticsValue::String(model.clone()),
                                );
                                md.insert(
                                    "blocks_yielded".into(),
                                    telemetry::AnalyticsValue::Int(
                                        i64::try_from(blocks_yielded).unwrap_or(i64::MAX),
                                    ),
                                );
                                // has_output is always true on this arm (partial_has_output).
                                md.insert(
                                    "has_output".into(),
                                    telemetry::AnalyticsValue::Bool(true),
                                );
                                md.insert(
                                    "synthesized_stop_reason".into(),
                                    telemetry::AnalyticsValue::String(
                                        synthesized_stop_reason.to_string(),
                                    ),
                                );
                                md.insert(
                                    "cause".into(),
                                    telemetry::AnalyticsValue::String(cause.as_str().to_string()),
                                );
                                if let Some(rid) = self.api.last_request_id() {
                                    md.insert(
                                        "request_id".into(),
                                        telemetry::AnalyticsValue::String(rid),
                                    );
                                }
                                bus.log_event("tengu_streaming_partial_finalized", md).await;
                            }
                            // Arm the notice + terminal-end sites below; the partial flows
                            // through the normal billing/persist/tool-drive path first.
                            partial_finalize = Some(cause);
                            partial
                        }
                        Err(f)
                            if matches!(
                                f.error,
                                OrchestratorError::Streaming(
                                    LlmError::Overloaded { .. } | LlmError::ProviderInternal
                                )
                            ) && !is_env_truthy(
                                std::env::var("LINGXI_DISABLE_NONSTREAMING_FALLBACK")
                                    .as_deref()
                                    .ok(),
                            ) =>
                        {
                            did_fall_back_to_non_streaming = true;
                            // Seed: a streaming overload counts as 1 toward the consecutive
                            // 529 budget (LlmError::Overloaded = 529).  Other in-band errors
                            // (e.g. ProviderInternal) seed 0 — matching TS
                            // `is529Error(streamingError) ? 1 : 0` (claude.ts:2559).
                            let seed: u8 = u8::from(matches!(
                                f.error,
                                OrchestratorError::Streaming(LlmError::Overloaded { .. })
                            ));

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
                            self.reattach_outgoing_context(
                                &mut non_stream_snapshot,
                                deferred_reminder.as_ref(),
                                date_change_reminder.as_ref(),
                                &turn_reminders,
                            )
                            .await;
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
                            // P2-04: suppressed when a `MessageDisplay` hook is active — the
                            // completed-message pass renders the (possibly substituted) text
                            // once, so skip the direct emit to avoid double display.
                            for blk in &pumped_from_fallback.assistant_blocks {
                                match blk {
                                    ContentBlock::Text { text } if !display_hook_active => {
                                        self.output.emit_text(text).await;
                                    }
                                    ContentBlock::Thinking {
                                        thinking,
                                        signature,
                                    } => {
                                        self.output
                                            .emit_thinking(thinking, signature.as_deref())
                                            .await;
                                    }
                                    ContentBlock::RedactedThinking { data } => {
                                        self.output.emit_redacted_thinking(data).await;
                                    }
                                    _ => {}
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
                        Err(f) if crate::turn_loop::is_carveout_propagated(&f.error) => {
                            return Err(f.error)
                        }
                        // #10: any other mid-stream model/runtime error (e.g. Transport)
                        // ends the turn GRACEFULLY as `model_error` (faithful port of the
                        // `query.ts` catch) rather than bubbling a hard error / phantom
                        // interrupt. Reached when the partial finalize above did NOT apply —
                        // either no recoverable output existed before the error (for
                        // example, an incomplete tool block) or the error is not a
                        // finalize class. The assistant message for a partial-with-real-
                        // -output turn is persisted by the finalize arm above; here nothing
                        // was persisted (TS `yieldMissingToolResultBlocks` no-op).
                        Err(f) => {
                            let other = f.error;
                            // Classify the typed mid-stream error (`Flp`/`KNn`) into the
                            // api-error envelope; the message text stays verbatim.
                            let env = classify_api_error(&other);
                            let id = crate::turn_loop::surface_model_error(
                                self,
                                &other.to_string(),
                                env,
                            )
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
                    //
                    // P1-04: a partial-stream finalize is NOT a per-request success —
                    // cc records cost (`Ae+=zhe`, kept above) but does NOT emit
                    // `tengu_api_success` (it already fired `tengu_streaming_partial_finalized`).
                    // Skip the success emit when this turn was finalized from a partial.
                    if let Some(bus) = self
                        .analytics_bus
                        .as_ref()
                        .filter(|_| partial_finalize.is_none())
                    {
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
                                did_fall_back_to_non_streaming,
                                is_non_interactive_session: !self.prompt_is_interactive(),
                                print: !self.prompt_is_interactive(),
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

            // P2-04 (MessageDisplay `displayContent`): completed-message pass.
            // Once the assistant text blocks are finalized, fire `MessageDisplay`
            // with `final:true` + the joined visible text and render the result
            // ON SCREEN — substituting the hook's `displayContent` when present,
            // else the original text. The stored `assistant_msg` / JSONL keep the
            // ORIGINAL content (claude-code stores the override on a separate
            // `displayedMessageContent`, never `message.content`). Gated on a
            // registered `MessageDisplay` hook — when active the live per-token
            // deltas were suppressed in the pump, so this is the single on-screen
            // render; when inactive this whole block is skipped (byte-identical).
            if display_hook_active {
                let joined: String = pumped
                    .assistant_blocks
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => text.as_str(),
                        _ => "",
                    })
                    .collect();
                // claude-code `Qff`: skip firing entirely when the joined text is
                // empty (`if(s==="")return i`).
                if !joined.is_empty() {
                    let on_screen = self
                        .fire_message_display_completed(&turn_id, &joined)
                        .await
                        .unwrap_or(joined);
                    self.output.emit_text(&on_screen).await;
                }
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

            // P1-04 (cc 2.1.199): after the finalized partial assistant is
            // persisted, yield the byte-exact incomplete-response notice as its own
            // api-error assistant message — cc yields `tu({content:…, error:"server_error"})`
            // RIGHT AFTER the patched partial and BEFORE any tool_results run. The
            // notice's api-error category is hardcoded `server_error` regardless of
            // the underlying finalize cause (cc `error:"server_error"`). Persisted
            // without the `tengu_query_error` telemetry (that fires only from the
            // top-level `model_error` catch, not this finalize path).
            if let Some(cause) = partial_finalize {
                let env = ApiErrorEnvelope {
                    error: Some("server_error"),
                    api_error_status: None,
                    inner_stop_reason: None,
                };
                partial_finalize_notice_id = Some(
                    crate::turn_loop::surface_api_error_notice(
                        self,
                        cause.incomplete_notice(),
                        env,
                    )
                    .await,
                );
            }

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
                    // A queued tool cancelled here gets the SAME
                    // `user_interrupted` synthetic `drain_one` substitutes, and
                    // claude-code stamps that message `user-rejected`
                    // (`createSyntheticErrorMessage`, 2.1.220 @232972524), so
                    // record the kind for the persisted tool_result line.
                    for (id, reason) in exec.apply_abort_to_pending() {
                        if reason == crate::streaming_executor::AbortReason::UserInterrupted {
                            self.record_tool_denial_kind(&id, "user-rejected").await;
                        }
                        // O1: the synthetic that survives carries claude's own
                        // short `toolUseResult` literal, not the block's text.
                        self.record_tool_use_result(
                            &id,
                            crate::streaming_executor::synthetic_tool_use_result(reason),
                        )
                        .await;
                    }
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
                        // Release this tool's SDK frame HERE — `take_newly_completed`
                        // yields in received order, and `drained.block` is the
                        // post-substitution content, so a cancelled tool reports
                        // its synthetic rather than the real outcome the executor
                        // discarded. A queued-then-cancelled tool never dispatched
                        // and so has no buffered frame; it gets one from here,
                        // where previously it got none at all.
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                            ..
                        } = &drained.block
                        {
                            self.release_tool_frame(tool_use_id, content, *is_error)
                                .await;
                        }
                        // O3: keep this result's tool id so its hook
                        // `attachment` lines can be flushed immediately after
                        // its tool_result — claude's stream order.
                        let drained_tool_use_id = match &drained.block {
                            ContentBlock::ToolResult { tool_use_id, .. } => {
                                Some(tool_use_id.clone())
                            }
                            _ => None,
                        };
                        let user_msg = ConversationMessage::User {
                            id: MessageId::new(),
                            content: vec![drained.block],
                            is_meta: false,
                            is_compact_summary: false,
                            is_visible_in_transcript_only: false,
                        };
                        {
                            let mut s = self.session.lock().await;
                            s.history.push(user_msg.clone());
                        }
                        // `bash_output_audience_note` (NEW in 2.1.238, gate `kpm`
                        // @294267076, emission @294300924): a Bash result whose
                        // stdout is longer than the few lines the user's terminal
                        // shows gets a one-line note telling the model the user
                        // did NOT see it. The oracle pushes it into the message
                        // STREAM right after the `tool_result` line, so it lands
                        // in history + JSONL like any other attachment message.
                        // Gated on the model capability
                        // `bash_output_audience_note` / the
                        // `CLAUDE_CODE_BASH_OUTPUT_AUDIENCE_NOTE` env var; the
                        // port has no capability table ⇒ DEFAULT OFF ⇒ strict
                        // no-op, so the locked streaming fixtures are unaffected.
                        //
                        // Computed BEFORE the persist below: the JSONL writer
                        // CONSUMES the recorded `toolUseResult` (it moves into
                        // the line's `toolUseResult` field), and the gate needs
                        // that payload's `stdout`.
                        let audience_note = match &drained_tool_use_id {
                            Some(id) => self.bash_output_audience_note_message(id).await,
                            None => None,
                        };
                        self.persist_message_to_jsonl_with_parent(&user_msg, parent_uuid)
                            .await;
                        if let Some(note) = audience_note {
                            {
                                let mut s = self.session.lock().await;
                                s.history.push(note.clone());
                            }
                            self.persist_message_to_jsonl(&note).await;
                        }
                        if let Some(id) = &drained_tool_use_id {
                            self.flush_hook_attachments(id).await;
                        }
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
                            let is_ephemeral_rendering = m.is_meta();
                            {
                                let mut s = self.session.lock().await;
                                s.history.push(m.clone());
                                s.injected_message_sources.insert(m.id(), tool_use_id);
                            }
                            // O3: an `is_meta` injected message is the
                            // EPHEMERAL rendering of the attachment flushed
                            // above — persisting it would duplicate the record.
                            if is_ephemeral_rendering {
                                continue;
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
            // Drive finished: stop holding frames.
            self.set_tool_frame_buffering(false).await;

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
                    self.inject_user_message(interrupt_message).await;
                }
                final_message_id = assistant_id;
                break;
            }

            // P1-04 (cc 2.1.199): a finalized partial ends the turn once its
            // dispatched tools have drained (above) and the incomplete-response
            // notice has been surfaced — cc `break e`s out of the stream loop after
            // yielding the notice; it does NOT re-enter the continuation logic. We
            // terminate here (reason `model_error`, matching the api-error catch)
            // rather than looping on the synthesized `tool_use`/`end_turn`, so the
            // user sees the partial + notice and can retry. The synthesized
            // stop_reason still rides on the persisted partial assistant line
            // (patched above) for resume fidelity.
            if partial_finalize.is_some() {
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn("model_error", &cost).await;
                final_message_id = partial_finalize_notice_id.unwrap_or(assistant_id);
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
                        final_message_id = failed_msg.id();
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
                    // The nudge is a META user message carrying the byte-exact
                    // string — CC 2.1.207 builds it via `createUserMessage({…,
                    // isMeta:!0})`, so it persists with top-level `isMeta:true`.
                    let nudge_msg = ConversationMessage::user_meta(
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
                            self.prompt_is_interactive(),
                            other,
                            request_id.as_deref(),
                            pumped.stop_details.as_ref(),
                        )
                    };
                    let surfaced_id = if let Some(text) = api_error {
                        let err_msg = ConversationMessage::Assistant {
                            id: MessageId::new(),
                            content: vec![ContentBlock::Text { text: text.clone() }],
                            stop_reason: Some(other.to_string()),
                        };
                        self.session.lock().await.history.push(err_msg.clone());
                        self.persist_message_to_jsonl(&err_msg).await;
                        self.output.emit_text(&text).await;
                        Some(err_msg.id())
                    } else {
                        None
                    };
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn(other, &cost).await;
                    final_message_id = surfaced_id.unwrap_or(assistant_id);
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
        if let (Some(fh), Some(file_history_msg_id)) = (&self.file_history, file_history_msg_id) {
            if let (Some(record), Some(writer)) =
                (fh.snapshot_record(file_history_msg_id), &self.jsonl_writer)
            {
                let session_id = self.session.lock().await.session_id;
                let session_uuid = session_id.as_uuid().to_string();
                let line = session::file_history::snapshot_line_json(&session_uuid, &record);
                if let Err(error) = writer.append_file_history_snapshot(&line).await {
                    self.record_transcript_append_failure(
                        &session_id.to_string(),
                        "file_history_snapshot",
                        &error,
                    )
                    .await;
                }
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
        let _turn_guard = self.turn_gate.lock().await;
        tracing::info!(
            event = orch_events::CONVERSATION_STARTED,
            prompt_len = prompt.len()
        );
        let result = traits::session_flags::scope_non_interactive_session(
            !self.prompt_is_interactive(),
            self.try_run_turn_cancelable(prompt, cancel),
        )
        .await;
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
        // claude-code `nre` precedence: `--system-prompt` (override) wins; else the
        // `--agent` main-thread agent's prompt; else the default (`build_system_prompt`).
        let system_prompt: Option<String> = Some(self.effective_system_prompt().await);

        // A side query left unfinished when the previous user turn ended was
        // keyed to that previous prompt. Never surface it against new intent.
        self.discard_stale_prefetches().await;

        // 1. Append the user prompt to session history.
        let user_msg = ConversationMessage::user(MessageId::new(), prompt.to_string());
        {
            let mut s = self.session.lock().await;
            s.history.push(user_msg.clone());
        }
        self.persist_message_to_jsonl(&user_msg).await;

        // hooks B4: UserPromptSubmit (cancelable REPL twin). A Block aborts the
        // turn before any API call. No-op when unregistered.
        if self.fire_user_prompt_submit(prompt, user_msg.id()).await {
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
                    self.inject_user_message(INTERRUPT_MESSAGE).await;
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
                        self.inject_user_message(INTERRUPT_MESSAGE).await;
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
                    //
                    // Gate on `end_turn` (matching `run_turn`): unlike the
                    // streaming twin, which is structurally in the hardcoded
                    // `"end_turn"` branch, this driver carries the live
                    // `stop_reason`, so a TERMINAL end (blocking_limit /
                    // prompt_too_long) must NOT trigger budget continuation.
                    if stop_reason == "end_turn"
                        && self
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
        self.run_turn_streaming_with_cancel_images_and_message_id(prompt, &[], cancel, None)
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
        self.run_turn_streaming_with_cancel_images_and_message_id(prompt, image_paths, cancel, None)
            .await
    }

    /// As [`Self::run_turn_streaming_with_cancel_images`], but allows the
    /// caller to pin the persisted user-message UUID.
    pub async fn run_turn_streaming_with_cancel_images_and_message_id(
        &self,
        prompt: &str,
        image_paths: &[std::path::PathBuf],
        cancel: CancellationToken,
        message_id: Option<MessageId>,
    ) -> Result<TurnOutcome, OrchestratorError> {
        // Decode the pasted PATHS into canonical sources first, then hand off to
        // the already-decoded entry below — so the path-based and bridge (inline
        // base64) flows share ONE cancel race + ONE turn core. A failed image read
        // aborts the turn with `Err` before any API call (unchanged).
        let images = Self::load_images(image_paths)?;
        self.run_turn_streaming_with_cancel_image_sources_and_message_id(
            prompt, images, cancel, message_id,
        )
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
        self.run_turn_streaming_with_cancel_image_sources_and_message_id(
            prompt, images, cancel, None,
        )
        .await
    }

    /// As [`Self::run_turn_streaming_with_cancel_image_sources`], but allows the
    /// caller to pin the persisted user-message UUID.
    pub async fn run_turn_streaming_with_cancel_image_sources_and_message_id(
        &self,
        prompt: &str,
        images: Vec<protocol::ImageSource>,
        cancel: CancellationToken,
        message_id: Option<MessageId>,
    ) -> Result<TurnOutcome, OrchestratorError> {
        let _turn_guard = self.turn_gate.lock().await;
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
        let r = traits::session_flags::scope_non_interactive_session(
            !self.prompt_is_interactive(),
            self.try_run_turn_streaming(prompt, images, Some(cancel.clone()), message_id, false),
        )
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

    /// Whether the current in-memory session history already contains this raw
    /// top-level JSONL UUID. Structured stdin replay uses this to suppress
    /// duplicate user turns across process restarts / reattach flows.
    pub async fn session_contains_message_uuid(&self, raw_uuid: &str) -> bool {
        let Some(id) = MessageId::parse_prefixed(raw_uuid) else {
            return false;
        };
        let session = self.session.lock().await;
        session.history.iter().any(|message| message.id() == id)
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
        self.effective_system_prompt().await
    }

    /// Adopt a `--agent`-resolved definition for the MAIN conversation loop
    /// (claude-code `bde(agentDef.agentType)` + `mainThreadAgentDefinition`).
    /// Called ONCE at startup by the composition root when `--agent` resolves to
    /// a catalog hit — the resolution runs against the FINAL agent catalog after
    /// this orchestrator is already `Arc`-wrapped, so the seam is interior-mutable.
    /// After this, `agent_type` rides every main-thread lifecycle hook payload,
    /// `system_prompt` (when `Some`) becomes the main-loop system prompt on every
    /// query — `--system-prompt` (`system_prompt_override`) still winning — and
    /// `tool_policy` / `disallowed_tools` narrow the advertised tool pool (claude
    /// `HJ(agentDef,to,!1,!0)`).
    ///
    /// `model_override` is the agent's frontmatter `model` ALREADY resolved to a
    /// concrete wire id and gated by the caller (claude-code
    /// `if(!userSpecifiedModel&&y.model&&y.model!=="inherit"){jb(Zo(y.model))}` —
    /// the `!userSpecifiedModel` / `!=="inherit"` checks live at the composition
    /// root, which owns `--model`). When `Some`, it replaces the session model
    /// (profile cleared: agent frontmatter carries a bare id, no provider profile).
    pub async fn set_main_thread_agent(
        &self,
        agent_type: String,
        system_prompt: Option<String>,
        tool_policy: agent::AgentToolPolicy,
        disallowed_tools: Vec<String>,
        model_override: Option<String>,
    ) {
        if let Some(model) = model_override {
            let mut s = self.session.lock().await;
            s.model = model;
            s.model_profile = None;
        }
        *self.main_thread_agent.write().await = Some(MainThreadAgentState {
            agent_type,
            system_prompt,
            tool_policy,
            disallowed_tools,
        });
    }

    /// Replace the frontmatter-hook bucket owned by the main-thread agent.
    /// Normal startup installs it once; hot resume calls this again so the
    /// previous session's hooks are removed before the resumed agent's hooks
    /// become visible.
    pub async fn replace_main_thread_agent_hooks(&self, definitions: &[hooks::HookDefinition]) {
        let previous = self.main_thread_agent_hook_id.lock().await.take();
        if let Some(previous) = previous {
            self.hooks.clear_agent_hooks(previous).await;
        }
        if definitions.is_empty() {
            return;
        }
        let agent_id = protocol::AgentId::new();
        self.hooks
            .register_agent_hooks(agent_id, definitions, false)
            .await;
        *self.main_thread_agent_hook_id.lock().await = Some(agent_id);
    }

    /// Restore the main-thread agent selected by a resumed session. Prefer the
    /// immutable, integrity-checked transcript snapshot; legacy transcripts
    /// fall back to the current live catalog by agent type. A missing or
    /// unresolvable selection explicitly restores default behavior instead of
    /// retaining state from the session that was previously mounted.
    pub(crate) async fn restore_main_thread_agent_from_resume(
        &self,
        wanted: Option<String>,
        snapshot: Option<serde_json::Value>,
    ) {
        let mut resolved = match (wanted.as_deref(), snapshot) {
            (Some(wanted), Some(value)) => serde_json::from_value::<agent::AgentDefinition>(value)
                .ok()
                .filter(|definition| definition.agent_type == wanted),
            _ => None,
        };

        if resolved.is_none() {
            if let (Some(wanted), Some(catalog)) = (wanted.as_deref(), self.agent_catalog.as_ref())
            {
                let catalog = catalog.read().await;
                // CC 2.1.218 resolves the resumed agentType by EXACT equality
                // only — no bare-name/suffix fallback (that belongs to the
                // `--agent` startup surface, not resume).
                resolved = catalog
                    .iter()
                    .find(|definition| definition.agent_type == wanted)
                    .cloned();
            }
        }

        match resolved {
            Some(definition) => {
                // (cc 2.1.218 `mvo`) ORIGIN TRUST — the RESUME surface. The
                // resumed `agent-setting` snapshot carries the definition's
                // `frontmatter_hooks` verbatim, so without this check a
                // definition whose hooks were correctly REFUSED at `--agent`
                // time would be silently installed on the next resume of that
                // session. Evaluated BEFORE the fields are moved below.
                let hooks_trusted =
                    agent::hooks_trust::agent_hooks_origin_trusted(&definition, &self.cwd);
                if !hooks_trusted {
                    agent::hooks_trust::report_untrusted_hooks(
                        &definition,
                        &self.cwd,
                        agent::hooks_trust::HooksTrustSurface::MainThread,
                        false,
                    );
                }
                // (gap218 #43 / cc 2.1.218 `NQe`) Adopt the resumed agent's
                // frontmatter `model`, resolved to a wire id — the hot-resume twin
                // of the composition root's COLD-resume gate. Applied ONLY when the
                // user did NOT pass `--model` (`apply_resumed_agent_model` is
                // `!default_model_explicit`, set at the root) AND the agent declares
                // a concrete model (`AgentModel != Inherit`, oracle `i.model &&
                // i.model!=="inherit"`); an explicit `--model` is never overridden.
                // `resolve_user_specified_model` is `Zo` (alias → wire id).
                // Evaluated BEFORE `definition.*` moves into the call below.
                let model_override = if self.config.apply_resumed_agent_model {
                    match &definition.model {
                        agent::AgentModel::Alias(spec) | agent::AgentModel::Explicit(spec) => {
                            Some(agent::model_resolution::resolve_user_specified_model(spec))
                        }
                        agent::AgentModel::Inherit => None,
                    }
                } else {
                    None
                };
                self.set_main_thread_agent(
                    definition.agent_type,
                    definition.system_prompt,
                    definition.tools,
                    definition.disallowed_tools,
                    model_override,
                )
                .await;
                if hooks_trusted {
                    self.replace_main_thread_agent_hooks(&definition.frontmatter_hooks)
                        .await;
                } else {
                    // `QEt`'s untrusted arm ends in `b1r(void 0)` — it CLEARS the
                    // main-thread bucket rather than leaving it alone. Skipping
                    // the clear would strand the PREVIOUS session's hooks: an
                    // in-place resume from a trusted folder A into an untrusted
                    // session B would keep A's hook commands firing under B.
                    self.replace_main_thread_agent_hooks(&[]).await;
                }
            }
            None => {
                if let Some(wanted) = wanted {
                    tracing::warn!(
                        "Resumed session had agent \"{wanted}\" but it is no longer available. Using default behavior."
                    );
                }
                *self.main_thread_agent.write().await = None;
                self.replace_main_thread_agent_hooks(&[]).await;
            }
        }
    }

    /// The adopted main-thread agent's `agentType` (claude-code `MB()`), or
    /// `None` when no `--agent` was applied. Threaded into main-thread lifecycle
    /// hook payloads.
    pub(crate) async fn main_thread_agent_type(&self) -> Option<String> {
        self.main_thread_agent
            .read()
            .await
            .as_ref()
            .map(|a| a.agent_type.clone())
    }

    /// The system prompt for the next query, applying claude-code `nre`
    /// precedence: `overrideSystemPrompt` (`--system-prompt`) wins; else the
    /// adopted main-thread agent's prompt (`mainThreadAgentDefinition`
    /// `.getSystemPrompt()`); else the freshly assembled default.
    pub(crate) async fn effective_system_prompt(&self) -> String {
        if let Some(custom) = &self.config.system_prompt_override {
            return custom.clone();
        }
        let main_thread_prompt = self
            .main_thread_agent
            .read()
            .await
            .as_ref()
            .and_then(|agent| agent.system_prompt.clone());
        let mut prompt = match main_thread_prompt {
            Some(prompt) => prompt,
            None => self.build_system_prompt().await,
        };
        if let Ok(profile) = self.app_agent_prompt_profile.read() {
            if let Some(profile) = profile.as_ref() {
                if !profile.instructions.trim().is_empty() {
                    prompt.push_str("\n\n# App Agent Profile\n");
                    prompt.push_str(&format!("Revision: {}\n", profile.revision));
                    prompt.push_str(&profile.instructions);
                }
            }
        }
        prompt
    }

    /// Install a host-approved app Agent Profile as an additive prompt layer.
    /// The next turn observes it; the current in-flight request keeps its
    /// already-built prompt.
    pub fn set_app_agent_prompt_profile(
        &self,
        revision: u64,
        instructions: String,
    ) -> Result<(), String> {
        if instructions.len() > 32 * 1024 {
            return Err("app Agent Profile exceeds 32 KiB".into());
        }
        let mut profile = self
            .app_agent_prompt_profile
            .write()
            .map_err(|_| "app Agent Profile lock is poisoned".to_string())?;
        if profile
            .as_ref()
            .is_some_and(|current| revision < current.revision)
        {
            return Err("app Agent Profile revision moved backwards".into());
        }
        *profile = Some(AppAgentPromptProfile {
            revision,
            instructions,
        });
        Ok(())
    }

    /// Remove the app-specific additive prompt layer when an app session is
    /// closed or the host switches back to the ordinary conversation.
    pub fn clear_app_agent_prompt_profile(&self) -> Result<(), String> {
        self.app_agent_prompt_profile
            .write()
            .map_err(|_| "app Agent Profile lock is poisoned".to_string())?
            .take();
        Ok(())
    }

    /// Read-only introspection seam for the ordinary per-turn additional
    /// context (`claudeMd` / `userEmail` / `currentDate`). Companion to
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

    /// Prepend the fixed runtime context followed by the ordinary per-turn
    /// additional context. Keeping this in one helper prevents retry paths from
    /// drifting in ordering or accidentally dropping the mobile snapshot.
    pub(crate) async fn prepend_leading_context(&self, messages: &mut Vec<ConversationMessage>) {
        if let Some(ctx_msg) = self.additional_context_message().await {
            messages.insert(0, ctx_msg);
        }
        if let Some(workspace) = self.mobile_workspace_environment_message() {
            messages.insert(0, workspace);
        }
        if let Some(runtime) = self.mobile_runtime_environment_message().await {
            messages.insert(0, runtime);
        }
    }

    fn mobile_workspace_environment_message(&self) -> Option<ConversationMessage> {
        let environment = self.mobile_runtime_environment.as_ref()?;
        let cwd = self.session_cwd.cwd();
        let model_cwd = match &self.mobile_workspace_cwd_resolver {
            Some(resolver) => resolver(&cwd),
            None => Some(cwd.to_string_lossy().into_owned()),
        };
        let reminder = environment.render_workspace_system_reminder(model_cwd.as_deref())?;
        Some(ConversationMessage::user_meta(MessageId::new(), reminder))
    }

    pub(crate) fn prompt_is_interactive(&self) -> bool {
        self.mobile_runtime_environment.as_ref().map_or(
            self.config.interactive_session,
            |environment| {
                !matches!(
                    environment.host.launch_mode,
                    traits::MobileLaunchMode::ScheduledHeadless
                )
            },
        )
    }

    /// Prepend a transient call-scoped reminder without displacing the fixed
    /// mobile runtime snapshot from index zero.
    ///
    /// Desktop callers retain the historical index-zero behavior. Mobile
    /// callers place date/deferred-tool deltas immediately after the fixed
    /// runtime reminder, keeping that cache-stable prefix in one position on
    /// initial, retry, and fallback requests.
    pub(crate) fn prepend_transient_leading_context(
        &self,
        messages: &mut Vec<ConversationMessage>,
        reminder: ConversationMessage,
    ) {
        let mut index = usize::from(messages.first().is_some_and(|message| {
            Self::is_mobile_runtime_environment_message(message)
                || self
                    .mobile_runtime_environment_message
                    .as_ref()
                    .is_some_and(|runtime| runtime == message)
        }));
        if self.mobile_runtime_environment.is_some()
            && messages.get(index).is_some_and(|message| {
                matches!(message, ConversationMessage::User { content, .. } if content.iter().any(
                    |block| matches!(block, protocol::ContentBlock::Text { text } if text.starts_with("<system-reminder>\nMobile workspace context"))
                ))
            })
        {
            index += 1;
        }
        messages.insert(index, reminder);
    }

    /// Reattach all call-scoped context after rebuilding a request from raw
    /// session history (for example after a context-overflow retry).
    pub(crate) async fn reattach_outgoing_context(
        &self,
        messages: &mut Vec<ConversationMessage>,
        deferred_tools_reminder: Option<&ConversationMessage>,
        date_change_reminder: Option<&ConversationMessage>,
        turn_reminders: &[ConversationMessage],
    ) {
        self.prepend_leading_context(messages).await;
        if let Some(reminder) = deferred_tools_reminder {
            self.prepend_transient_leading_context(messages, reminder.clone());
        }
        if let Some(reminder) = date_change_reminder {
            self.prepend_transient_leading_context(messages, reminder.clone());
        }
        messages.extend(turn_reminders.iter().cloned());
    }

    /// Read-only test/host preview of the fixed runtime reminder.
    pub async fn mobile_runtime_environment_preview(&self) -> Option<String> {
        self.mobile_runtime_environment_message()
            .await
            .and_then(|message| Self::text_content(&message))
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

        // Task 5 (worktree 206 session-cwd plumbing): read the LIVE
        // `self.session_cwd` — the SAME cell `EnterWorktree`/`ExitWorktree`
        // swap on the tool side — not the frozen `self.cwd`. The env block's
        // `Primary working directory:` line, the file tree, the git-status
        // probe, and the memory hierarchy below all derive from `cwd`, so this
        // one substitution re-derives the ENTIRE prompt context from the
        // post-swap worktree every turn. `self.session_cwd` defaults to a
        // private, never-swapped cell equal to `self.cwd` when
        // `with_session_cwd` was never called, so this is byte-identical to
        // before for every caller that doesn't wire it (INERT INVARIANT).
        let cwd = self.session_cwd.cwd();
        // PathAtlas S3: when the session cwd is a mobile-linux guest path,
        // the probes below (memory hierarchy, git status, file tree,
        // worktree check) must read the HOST directory backing it while the
        // env block's `Primary working directory:` keeps displaying the
        // guest path the model actually uses. Desktop never sets the
        // resolver, so `probe_cwd == cwd` there — byte-identical.
        let probe_cwd = match &self.prompt_probe_cwd_resolver {
            Some(resolver) => resolver(&cwd),
            None => cwd.clone(),
        };
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
        let memory_files = self.memory.load(&probe_cwd).await;

        let git = git_status::probe(&probe_cwd);
        let tree = file_tree::probe(&probe_cwd, file_tree::DEFAULT_DEPTH_LIMIT);

        // Tool name extraction: ToolRegistry's `all_names()` is the
        // unfiltered set (builtin + plugin + MCP). M5-03 uses the
        // unfiltered list because the registry's enable-filter requires
        // a `ToolStaticContext` that's only meaningful at dispatch time.
        // tools_block::format sorts alphabetically inside.
        let tool_names: Vec<String> = self.tools.all_names();

        let shell = crate::prompt::env_meta::detect_shell();

        // DIV-1: worktree detection — `hf()!==null` in claude-code. Detect a
        // worktree by checking for the `gitdir` file that git creates in worktree
        // checkouts (a file rather than a directory at .git). Computed before `cwd`
        // is moved into the context struct below.
        let in_worktree = probe_cwd.join(".git").is_file();

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
            // A scheduled mobile runtime shares a process with the foreground
            // conversation, so `interactive_session` intentionally remains
            // process-interactive. The typed per-orchestrator launch mode is
            // authoritative for prompt guidance and avoids advertising `!`
            // commands or other live-UI actions to a headless Cron run.
            is_interactive: self.prompt_is_interactive(),
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

    async fn resolve_active_output_style(&self) -> Option<outputstyles::ResolvedOutputStyle> {
        if let Some(registry) = &self.output_style_registry {
            if let Some(style) = registry
                .read()
                .await
                .resolve(self.config.output_style.as_deref())
            {
                return Some(style);
            }
        }
        outputstyles::resolve_output_style(
            self.config.output_style.as_deref(),
            &self.config.output_style_dirs,
        )
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
        let resolved = self.resolve_active_output_style().await;
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
            // Task 5 (worktree 206 session-cwd plumbing): read `ctx.cwd` (the
            // SAME live cwd `build_prompt_context` already resolved through
            // `self.session_cwd`), not the frozen `self.cwd` — otherwise this
            // trailing gitStatus block would report the boot directory's git
            // status while the env block above it already shows the swapped
            // worktree, an inconsistent prompt.
            if let Some(block) = git_status::render_git_status_block(&ctx.cwd) {
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
        //
        // Task 5 (worktree 206 session-cwd plumbing): read the LIVE
        // `self.session_cwd.cwd()`, not the frozen `self.cwd` — this reminder
        // is already recomputed fresh every turn (no cache), but reading the
        // frozen field would still show the pre-swap directory's LINGXI.md
        // files after `EnterWorktree`.
        let memory_files = self.memory.load(&self.session_cwd.cwd()).await;
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
        // `currentDate` is unconditional, but its date is session-memoized.
        // Midnight rollover is communicated exclusively by `date_change`; the
        // leading cacheable context entry must stay byte-stable.
        let session_id = self.session.lock().await.session_id;
        let session_date = self.session_start_date(session_id);
        entries.push(format!("# currentDate\nToday's date is {}.", session_date));

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
    /// The reminder is a plain user-text [`ConversationMessage`] carrying the
    /// byte-exact string. The fresh [`MessageId`] is irrelevant: callers append
    /// this ONLY to the per-turn outgoing message snapshot, never to
    /// `session.history` nor JSONL, so it is TRANSIENT and never accumulates —
    /// its `isMeta` state is therefore immaterial (nothing persists it)
    /// (TS recomputes the attachment each turn — see `query.ts` mid-turn
    /// `getAttachmentMessages`). Position mirrors TS: the caller appends it as a
    /// trailing meta user message after the user prompt / tool-results
    /// (`processTextPrompt` returns `[userMessage, ...attachmentMessages]`;
    /// `query.ts:1580-1590` pushes the attachment after `toolResults`).
    pub(crate) async fn output_style_reminder_message(&self) -> Option<ConversationMessage> {
        let resolved = self.resolve_active_output_style().await?;
        let content = format!(
            "<system-reminder>\n{} output style is active. \
             Remember to follow the specific guidelines for this style.\n</system-reminder>",
            resolved.name
        );
        Some(ConversationMessage::user_meta(MessageId::new(), content))
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
    /// The per-turn, transient plan-mode reminder (206 `plan_mode` attachment,
    /// builder `xEg`), or `None` when plan mode is not active.
    ///
    /// 1:1 with the binary's `xEg(t)`: gated on `permissionMode === "plan"`
    /// (here `session.plan_mode`), it returns the `{type:"plan_mode",
    /// reminderType, isSubAgent, planFilePath, planExists, ...customInstructions}`
    /// attachment which `KJn` assembles (`l=await xEg(t)`) BEFORE the
    /// invoked-skills bodies (`REg`) and the tool/mcp deltas (`gYt`/`YJn`), i.e.
    /// before the skill-listing reminder in the port's per-turn sequence.
    ///
    /// `reminderType` is `"full"` (206 `LU_`) on the FIRST plan-mode turn and
    /// `"sparse"` (206 `MU_`) thereafter — tracked by `plan_reminder_shown`,
    /// which is reset to `false` on plan-mode ENTRY. `isSubAgent` is ALWAYS
    /// `false` here: subagents never run through `ConversationOrchestrator`
    /// (every orchestrator is a depth-0 main thread), so the `NU_` variant is
    /// unreachable via this path. `customInstructions` stays `None` (wired by a
    /// later unit). Appended ONLY to the per-turn OUTGOING snapshot (never
    /// `session.history` / JSONL) so it never accumulates; `None` keeps the
    /// locked turn-loop fixtures byte-identical (default: plan mode OFF).
    pub(crate) async fn plan_mode_reminder_message(&self) -> Option<ConversationMessage> {
        let (path, exists, sparse) = {
            let mut s = self.session.lock().await;
            if !s.plan_mode {
                return None;
            }
            let path = Self::plan_file_path(
                &s.session_id,
                // 206 `Ct()` = the original project root (session-init cwd), NOT
                // the post-`cd` shell cwd.
                &self.cwd,
                self.config.plans_directory.as_deref(),
            );
            let exists = std::path::Path::new(&path).exists();
            // "full" on the first plan-mode turn (206 reminderType), "sparse"
            // after. Read-then-arm under the lock so concurrent turns can't both
            // render "full".
            let sparse = s.plan_reminder_shown;
            s.plan_reminder_shown = true;
            (path, exists, sparse)
        };
        let params = crate::prompt::plan_reminder::PlanReminderParams {
            plan_file_path: &path,
            plan_exists: exists,
            // C5: `--plan-mode-instructions` custom workflow body (borrows from
            // `self.config`, which outlives `params`; the session guard is already
            // dropped). `None` ⇒ the default 5-phase reminder.
            custom_instructions: self.config.plan_mode_instructions.as_deref(),
            is_subagent: false,
            reminder_type_sparse: sparse,
        };
        // All three plan-mode renderers (`M5T` full / `L5T` sparse / `H5T`
        // subagent) return through the batch wrapper `Zy` (2.1.238 @296675470),
        // which maps `NT` = `` `<system-reminder>\n${e}\n</system-reminder>` ``
        // (@296673554) over every message and marks it `isMeta:!0`. The body
        // renderer stays pure (its byte-exact unit tests pin the bare body); the
        // envelope is applied here, exactly as the other per-turn reminders do.
        let body = crate::prompt::plan_reminder::render_plan_mode_reminder(&params);
        let content = format!("<system-reminder>\n{body}\n</system-reminder>");
        Some(ConversationMessage::user_meta(MessageId::new(), content))
    }

    /// Resolve this session's plan file path (206 `ON(agentId)` →
    /// `<plansDir>/<slug>.md`). The plans directory is resolved by
    /// [`Self::plans_dir`] (206 `iT`): the `plansDirectory` settings override
    /// (relative to the project root, with a within-root containment check) when
    /// present, else the default `<config-home>/plans/`. The slug is the
    /// session's UUID (206's slug is likewise session-specific — exact bytes are
    /// not observable, the structure is). Uses the bare UUID (not the `sess:`
    /// display form) so the filename has no `:` separator, matching
    /// `computed_transcript_path`.
    pub(crate) fn plan_file_path(
        session_id: &SessionId,
        project_root: &std::path::Path,
        plans_directory: Option<&str>,
    ) -> String {
        Self::plans_dir(project_root, plans_directory)
            .join(format!("{}.md", session_id.as_uuid()))
            .to_string_lossy()
            .into_owned()
    }

    /// Resolve the plans DIRECTORY — 1:1 with the binary's `iT`:
    ///
    /// ```js
    /// iT=Or(function(){
    ///   let r=Wn().plansDirectory;
    ///   if(r){
    ///     let n=Ct(),o=Pne.resolve(n,r);
    ///     if(W5_(o,n))return o;
    ///     C(`plansDirectory must be within project root: ${r}`,{level:"error"})
    ///   }
    ///   return KPp()  // Pne.join(mn(),"plans")
    /// })
    /// ```
    ///
    /// When `plansDirectory` is set: resolve it against the project root
    /// (absolute values are used verbatim, `path.resolve` semantics), normalize
    /// `.`/`..` lexically, and accept it only if it is WITHIN the project root
    /// (`W5_`'s primary check `o === n || o.startsWith(n + sep)`) and passes
    /// the hardened protected-dir / same-repo-root checks. On rejection, fall
    /// through to the default `<config-home>/plans/`.
    fn plans_dir(
        project_root: &std::path::Path,
        plans_directory: Option<&str>,
    ) -> std::path::PathBuf {
        if let Some(r) = plans_directory.filter(|s| !s.is_empty()) {
            // `path.resolve(project_root, r)`: absolute `r` wins; else join.
            let candidate = if std::path::Path::new(r).is_absolute() {
                std::path::PathBuf::from(r)
            } else {
                project_root.join(r)
            };
            let resolved = crate::turn_loop::normalize_lexically(&candidate);
            let root = crate::turn_loop::normalize_lexically(project_root);
            // `W5_` primary: `o === n || o.startsWith(n + sep)` — component-wise
            // prefix containment on the normalized paths (so a `../escape` that
            // popped above `root` is rejected).
            if confined_path_components(&root, &resolved).is_some() {
                if plans_dir_passes_hardening(project_root, &root, &resolved) {
                    return resolved;
                }
                tracing::warn!("plansDirectory rejected by hardening guard: {r}");
                return Self::default_plans_dir();
            }
            tracing::error!("plansDirectory must be within project root: {r}");
        }
        Self::default_plans_dir()
    }

    /// The default plans directory — `<config-home>/plans/`, rebranding 206's
    /// `~/.claude/plans/` to `$LINGXI_CONFIG_DIR ?? ~/.lingxi`
    /// (`memory::lingxi_md::user_config_dir`).
    fn default_plans_dir() -> std::path::PathBuf {
        let config_home = dirs::home_dir()
            .map(|h| memory::lingxi_md::user_config_dir(&h))
            .unwrap_or_else(|| {
                // No home: honor an explicit `$LINGXI_CONFIG_DIR`, else cwd-relative.
                std::env::var_os(branding::CONFIG_DIR_ENV).map_or_else(
                    || std::path::PathBuf::from(".lingxi"),
                    std::path::PathBuf::from,
                )
            });
        config_home.join("plans")
    }

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
            compaction::context_window::context_window_for_model(&model, &self.api.active_betas())
                as usize;
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
        Some(ConversationMessage::user_meta(MessageId::new(), content))
    }

    /// T35: the per-turn, transient `task-notification` reminder, or `None` when
    /// no source is wired or no background task finished since the last turn.
    ///
    /// Mirrors [`Self::async_hook_response_reminder_message`]: drains the
    /// registry's terminal-not-notified tasks (CONSUME-ONCE — the registry marks
    /// each `notified` + evicts on drain) and renders their `<task-notification>`
    /// blocks (claude-code's per-task-type `enqueue*Notification` formats) inside
    /// one `<system-reminder>` meta user message, stamped at the front with the
    /// `NON_USER_INPUT_HEADER` provenance header (claude-code's `v6r`, applied to
    /// every `task-notification`-origin user message so the model never treats a
    /// machine-generated completion as user consent). Appended ONLY to the
    /// per-turn OUTGOING snapshot, never `session.history` / JSONL, so it never
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
    /// On fire it renders the body inside a `<system-reminder>` envelope as a
    /// META user message — the oracle's `Zy([kn({content:o,isMeta:!0})])`
    /// (2.1.238 @296690005 / @296690634, `Zy` @296675470 mapping `NT`
    /// @296673554) — and RESETS `turns_since_last_reminder` to
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
                // `case"todo_reminder"` returns `Zy([kn({content:o,isMeta:!0})])`
                // (2.1.238 @296690005), i.e. the body wrapped by `NT` =
                // `` `<system-reminder>\n${e}\n</system-reminder>` `` and marked
                // meta. The body renderer stays pure (byte-locked in
                // `tool_task::reminder`); the envelope is applied here.
                let body = tool_task::reminder::render_v1(&items);
                let content = format!("<system-reminder>\n{body}\n</system-reminder>");
                Some(ConversationMessage::user_meta(MessageId::new(), content))
            }
            tool_task::reminder::ReminderMode::V2Task => {
                // (3) TaskUpdate must be present this turn.
                if self.tools.find_by_name("TaskUpdate").is_none() {
                    return None;
                }
                let session_id = s.session_id;
                s.turns_since_last_reminder = 0;
                drop(s);
                // Read the V2 task store outside the session lock.
                let items: Vec<(String, engine::TodoState, String)> =
                    match &self.todo_reminder_tasks {
                        Some(provider) => provider
                            .task_items(session_id)
                            .await
                            .into_iter()
                            .map(|t| (t.id, t.status, t.subject))
                            .collect(),
                        None => Vec::new(),
                    };
                // `case"task_reminder"` — same `Zy([kn({…,isMeta:!0})])` envelope
                // as the V1 branch (2.1.238 @296690634).
                let body = tool_task::reminder::render_v2(&items);
                let content = format!("<system-reminder>\n{body}\n</system-reminder>");
                Some(ConversationMessage::user_meta(MessageId::new(), content))
            }
        }
    }

    /// `date_change` (cc `Cop` + renderer `date_change:` in the attachment
    /// table): a session that crosses local midnight tells the model the new
    /// date once per changed date. Producer logic 1:1 —
    /// `wcs()` = local `YYYY-MM-DD` ([`crate::prompt::env_meta::current_date_string`]),
    /// `LGe()` = the memoized session-start date; equal ⇒ no attachment, and an
    /// already-DELIVERED reminder for the same `newDate` dedupes.
    ///
    /// PURE — the dedupe is advanced by [`Self::commit_date_change_reminder`]
    /// once the request carrying the reminder has actually been issued. The
    /// oracle can latch on produce because it materialises the attachment as a
    /// real message (`Va(c,o)`) and pushes it into the message array BEFORE the
    /// call, so its dedupe reads the same fact it delivered; the port's
    /// reminder lives only in the outgoing snapshot, so a step that ends before
    /// the call (blocking-limit preempt, stream error, abort) must not consume
    /// it.
    ///
    /// Rendered through `pm([zr({content, isMeta:!0})])` = `<system-reminder>`
    /// wrap + meta user message, appended to THIS turn's OUTGOING snapshot only
    /// (never `session.history` / JSONL).
    pub(crate) fn date_change_reminder_message(
        &self,
        session_id: protocol::SessionId,
    ) -> Option<ConversationMessage> {
        let today = crate::prompt::env_meta::current_date_string();
        let session_date = self.session_start_date(session_id);
        let state = self
            .date_change
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if session_date == today || state.delivered_date.as_deref() == Some(today.as_str()) {
            return None;
        }
        drop(state);
        // Byte-exact reminder body (2.1.238 renderer @296739637, string-table
        // copy @256746832), wrapped by `NT`:
        // `<system-reminder>\n{e}\n</system-reminder>`. The tail sentence was
        // rewritten upstream between 2.1.220 ("DO NOT mention this to the user
        // explicitly because they are already aware.") and 2.1.238; the dash is
        // U+2014.
        let content = format!(
            "<system-reminder>\nThe date has changed. Today's date is now {today}. \
No need to announce the new date \u{2014} the user's own clock shows it.\n</system-reminder>"
        );
        Some(ConversationMessage::user_meta(MessageId::new(), content))
    }

    /// Does THIS model step continue a tool round rather than follow a fresh
    /// user prompt?
    ///
    /// The oracle's attachment fan-out (@296520120) distinguishes the two with
    /// `e === null` (no new prompt was handed to `getAttachments`) plus
    /// `!s?.isRegularUserPrompt`. LingXi's turn drivers re-enter the same
    /// assembly for both cases, so the discriminator is recovered from the
    /// history tail: a step that follows tool execution ends on a user line
    /// carrying `tool_result` blocks (claude's `sxl`, @296542062).
    pub(crate) fn step_follows_tool_results(history: &[ConversationMessage]) -> bool {
        matches!(
            history.last(),
            Some(ConversationMessage::User { content, .. })
                if content
                    .iter()
                    .any(|b| matches!(b, protocol::ContentBlock::ToolResult { .. }))
        )
    }

    /// The per-turn, transient `silent_turn_reminder` (2.1.238, producer `K4T`
    /// @296525255), or `None` when the gate is off or the stretch is too short.
    ///
    /// Gate, 1:1 with the fan-out condition @296520120:
    /// `p && e===null && !s?.isRegularUserPrompt && !CDt() && u3m(model)` —
    /// main agent only (every `ConversationOrchestrator` is depth-0, so `p` is
    /// always true), only on a tool-round continuation, and only when the
    /// capability/env gate is on. `CDt()` is the focus/brief-transcript view
    /// mode, which LingXi does not have ⇒ always `false` ⇒ never suppresses.
    ///
    /// `u3m` (@296477528) consults `CLAUDE_CODE_SILENT_TURN_REMINDER` and then
    /// the model capability table; the port has no capability table, so the
    /// DEFAULT IS OFF and this method is a strict no-op for stock sessions —
    /// the locked streaming fixtures stay byte-identical.
    ///
    /// On fire the body is wrapped in the usual `<system-reminder>` envelope
    /// (renderer @296738727: `[kn({content:NT(e.text),isMeta:!0})]`) and the
    /// emission position is recorded so `Ezm`'s `remindersInStretch` can be
    /// reconstructed on later turns. Appended to THIS turn's OUTGOING snapshot
    /// only — never `session.history` / JSONL.
    pub(crate) async fn silent_turn_reminder_message(&self) -> Option<ConversationMessage> {
        if !crate::prompt::silent_turn::is_enabled() {
            return None;
        }
        let history = {
            let s = self.session.lock().await;
            s.history.clone()
        };
        if !Self::step_follows_tool_results(&history) {
            return None;
        }
        let mut marks = self
            .silent_turn_reminder_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stretch = crate::prompt::silent_turn::scan_silent_stretch(&history, marks.as_slice());
        if !crate::prompt::silent_turn::should_emit(
            stretch,
            crate::prompt::silent_turn::turns_between_reminders(),
        ) {
            return None;
        }
        marks.push(history.len());
        drop(marks);
        let body = crate::prompt::silent_turn::reminder_text();
        let content = format!("<system-reminder>\n{body}\n</system-reminder>");
        Some(ConversationMessage::user_meta(MessageId::new(), content))
    }

    /// The per-turn, transient `total_tokens_reminder` (producer `D3T`
    /// @296556375), or `None` when the mode resolves to `off`.
    ///
    /// ```js
    /// let i=srt(); if(i==="off")return[];
    /// let s=n??"main", a=hoe(t), l=RYn.of(e);
    /// if(o) l.reanchorTaskBudget(s,a);
    /// let c = i==="countdown" ? OR(r,Ox())-a : i==="padded-countdown" ? uOi()-l.cumulativeUsed(s,a) : 0;
    /// return [{type:"total_tokens_reminder", text:dOi(i,c)}]
    /// ```
    ///
    /// The fan-out only calls `D3T` when the step continues a tool round
    /// (`e===null`) or when a regular user prompt arrived AND
    /// `totalTokensReminderAfterUserTurn` is on — the latter also being the
    /// `reanchor` flag. `hoe(messages)` (@294688350) is the LAST assistant
    /// message's `input + cache_creation + cache_read + output`, cached here as
    /// [`Self::last_response_input_tokens`] + [`Self::last_response_output_tokens`].
    ///
    /// **Default OFF in the port** — see the divergence note on
    /// [`crate::prompt::total_tokens`]. Stock sessions get `None`, so the
    /// locked streaming fixtures stay byte-identical.
    pub(crate) async fn total_tokens_reminder_message(&self) -> Option<ConversationMessage> {
        use crate::prompt::total_tokens as tt;
        let mode = tt::resolve_mode(None);
        if mode == tt::TotalTokensMode::Off {
            return None;
        }
        let (history_tail_is_tool_results, model) = {
            let s = self.session.lock().await;
            (
                Self::step_follows_tool_results(&s.history),
                s.model.clone(),
            )
        };
        let reanchor = !history_tail_is_tool_results && tt::after_user_turn(None);
        if !history_tail_is_tool_results && !reanchor {
            return None;
        }
        let used = i64::try_from(
            self.last_response_input_tokens
                .load(std::sync::atomic::Ordering::Relaxed)
                .saturating_add(
                    self.last_response_output_tokens
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
        )
        .unwrap_or(i64::MAX);
        let context_window = i64::try_from(compaction::effective_context_window_size(
            &model,
            &self.api.active_betas(),
        ))
        .unwrap_or(i64::MAX);
        let budget = tt::resolve_budget(None);
        let body = {
            let mut ledger = self
                .total_tokens_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if reanchor {
                ledger.reanchor_task_budget("main", used);
            }
            let remaining =
                tt::remaining_tokens(mode, &mut ledger, "main", used, context_window, budget);
            tt::format_total_tokens(mode, remaining)
        };
        // Renderer @296738663: `[kn({content:NT(e.text),isMeta:!0})]`.
        let content = format!("<system-reminder>\n{body}\n</system-reminder>");
        Some(ConversationMessage::user_meta(MessageId::new(), content))
    }

    /// The `bash_output_audience_note` attachment (2.1.238, gate `kpm`
    /// @294267076, emission @294300924), or `None` when the gate says no.
    ///
    /// Unlike the per-turn reminders this one belongs to the message STREAM: it
    /// follows the `tool_result` line for a Bash call whose stdout is longer
    /// than the few lines the user's terminal showed. Gated on the model
    /// capability `bash_output_audience_note` / the
    /// `CLAUDE_CODE_BASH_OUTPUT_AUDIENCE_NOTE` env var, which the port has no
    /// capability table for ⇒ **default OFF**.
    pub(crate) async fn bash_output_audience_note_message(
        &self,
        tool_use_id: &protocol::ToolUseId,
    ) -> Option<ConversationMessage> {
        if !crate::prompt::bash_output_note::is_enabled() {
            return None;
        }
        let tool_name = self.tool_name_for_use_id(tool_use_id).await?;
        let data = self
            .tool_use_results
            .lock()
            .await
            .get(&tool_use_id.to_string())
            .cloned();
        if !crate::prompt::bash_output_note::should_attach(
            &tool_name,
            data.as_ref(),
            self.config.interactive_session,
        ) {
            return None;
        }
        let content = format!(
            "<system-reminder>\n{}\n</system-reminder>",
            crate::prompt::bash_output_note::BASH_OUTPUT_AUDIENCE_NOTE
        );
        Some(ConversationMessage::user_meta(MessageId::new(), content))
    }

    /// The tool name that issued `tool_use_id`, recovered from the assistant
    /// line that carried the `tool_use` block.
    async fn tool_name_for_use_id(&self, tool_use_id: &protocol::ToolUseId) -> Option<String> {
        let s = self.session.lock().await;
        s.history.iter().rev().find_map(|msg| {
            let ConversationMessage::Assistant { content, .. } = msg else {
                return None;
            };
            content.iter().find_map(|b| match b {
                protocol::ContentBlock::ToolUse { id, name, .. } if id == tool_use_id => {
                    Some(name.clone())
                }
                _ => None,
            })
        })
    }

    /// Return the local date memoized for `session_id`, seeding it exactly once.
    ///
    /// Both the leading `# currentDate` context and the midnight reminder use
    /// this producer, so call order cannot create two independent date memos.
    fn session_start_date(&self, session_id: protocol::SessionId) -> String {
        let today = crate::prompt::env_meta::current_date_string();
        let mut state = self
            .date_change
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.session_id != Some(session_id) {
            *state = DateChangeState {
                session_id: Some(session_id),
                session_date: today,
                delivered_date: None,
            };
        }
        state.session_date.clone()
    }

    /// Mark the current local date's `date_change` reminder as DELIVERED — the
    /// commit half of [`Self::date_change_reminder_message`]. Called once the
    /// request carrying this turn's outgoing snapshot has actually been issued.
    pub(crate) fn commit_date_change_reminder(&self) {
        self.date_change
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .delivered_date = Some(crate::prompt::env_meta::current_date_string());
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
    ///    [`Self::read_state_map`] (the tools' live-cwd absolutized paths),
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
        // (1) CACHE — fill once from the same memory load the system prompt
        // uses. Task 5 (worktree 206 session-cwd plumbing): `cwd` is the LIVE
        // `self.session_cwd` (not the frozen `self.cwd`), and
        // `conditional_rules_cache` is reset to `None` by the `set_on_swap`
        // callback [`Self::with_session_cwd`] registers, so a worktree swap
        // forces this to re-walk disk under the NEW cwd instead of replaying
        // the pre-swap directory's rule set for the rest of the session.
        let cwd = self.session_cwd.cwd();
        let cached: Option<Vec<crate::prompt::MemoryFile>> =
            self.conditional_rules_cache.lock().unwrap().clone();
        let rules: Vec<crate::prompt::MemoryFile> = match cached {
            Some(rules) => rules,
            None => {
                let loaded = self
                    .memory
                    .load(&cwd)
                    .await
                    .into_iter()
                    .filter(|f| f.globs.is_some())
                    .collect::<Vec<_>>();
                *self.conditional_rules_cache.lock().unwrap() = Some(loaded.clone());
                loaded
            }
        };
        if rules.is_empty() {
            return None;
        }

        // Snapshot the touched files from the shared read-state registry (the
        // tools' live-cwd absolutized Read/Edit/Write/… paths).
        let touched: Vec<std::path::PathBuf> = self
            .read_state_map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .model_context_keys();
        if touched.is_empty() {
            return None;
        }

        // (2)+(3) MATCH + DELTA — collect newly-active rules not yet sent.
        let mut newly_active: Vec<&crate::prompt::MemoryFile> = Vec::new();
        {
            let mut sent = self.sent_conditional_rules.lock().await;
            for rule in &rules {
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

    /// Hermetic override for the roots [`Self::nested_memory_reminder_message`]
    /// probes. See [`Self::nested_memory_roots`].
    #[must_use]
    pub fn with_nested_memory_roots(
        mut self,
        home: std::path::PathBuf,
        managed_dir: Option<std::path::PathBuf>,
    ) -> Self {
        self.nested_memory_roots = Some((home, managed_dir));
        self
    }

    /// The per-turn NESTED MEMORY reminder: the `LINGXI.md` (and matching
    /// `paths:`-gated rules) governing the directories of files the session has
    /// TOUCHED. Guidance that lives next to the code reaches the model when the
    /// model reaches the code.
    ///
    /// 1:1 with claude-code `k$o` (@237714543) driven by `Rop` (@237715260):
    ///
    /// ```js
    /// for(let i of e){
    ///   if(t.loadedNestedMemoryPaths?.[i.path])continue;
    ///   if(!t.readFileState.has(i.path)){ n.push({type:"nested_memory",…});
    ///     t.loadedNestedMemoryPaths[i.path]=!0;
    ///     t.readFileState.set(i.path,{…,seededFromContext:!0,keepContent:!0}) }}
    /// ```
    ///
    /// 1. DISCOVER — [`crate::prompt::nested_memory::discover`] per touched
    ///    file. Stateless by design; see [`Self::sent_nested_memory`].
    /// 2. SKIP — anything already sent (`loadedNestedMemoryPaths`), already
    ///    claimed by [`Self::conditional_rules_reminder_message`], or already in
    ///    `read_file_state` (the model has the real thing).
    /// 3. SEED — [`Self::seed_nested_memory_read_state`], so the next `Read` of
    ///    a surfaced file returns the dedup stub instead of the bytes again.
    /// 4. RENDER — [`crate::prompt::conditional_rules::render_reminder`], the
    ///    same bare `Contents of {path}:` shape the oracle's `nested_memory`
    ///    attachment renders to.
    ///
    /// MUTATES the sent-set, so it must be called at most ONCE per outgoing
    /// model step — the same constraint every reminder in this family carries.
    ///
    /// # Divergence (reason)
    /// `Rop` opens with `if(!zK(e,r.toolPermissionContext))return n` — a
    /// read-permission check on the TRIGGER file. LingXi's permission context
    /// is not plumbed to this layer, and the trigger is by construction a file
    /// a tool already read, so the gate would be a no-op here. Not invented.
    ///
    /// `pub` (unlike its `pub(crate)` siblings) only so `test-harness` can drive
    /// it against a real `FileReadTool`: `orchestrator` does not depend on
    /// `tool-file`, so the end-to-end seed-then-dedup proof cannot live here.
    /// Both turn drivers are still the only production callers.
    pub async fn nested_memory_reminder_message(&self) -> Option<ConversationMessage> {
        // Same env kill-switch the eager loader honors (`Rop`'s
        // `CLAUDE_CODE_DISABLE_CLAUDE_MDS` guard). ANY non-empty value disables.
        if std::env::var_os("LINGXI_DISABLE_LINGXI_MDS").is_some_and(|v| !v.is_empty()) {
            return None;
        }
        let touched: Vec<std::path::PathBuf> = self
            .read_state_map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .model_context_keys();
        if touched.is_empty() {
            return None;
        }
        let (home, managed) = match &self.nested_memory_roots {
            Some((home, managed)) => (home.clone(), managed.clone()),
            None => (
                dirs::home_dir()?,
                Some(memory::lingxi_md::hierarchy::managed_path()),
            ),
        };
        // CANONICAL cwd, not the raw one. `split_ancestors` decides "is the
        // touched file under cwd" with a prefix test, and the two sides reach
        // it in different forms: `read_state_map` keys are whatever
        // `canonicalize_and_validate` produced, while `session_cwd` is whatever
        // the user launched in. On macOS that is `/private/var/...` against
        // `/var/...`, so every touched file looks OUTSIDE cwd and nothing is
        // ever discovered. The oracle has no such split (its `Li` is purely
        // lexical, so both halves agree); LingXi has to normalize on ONE side,
        // and cwd is the side that makes every derived path match the registry
        // the seed writes to. Falls back to the raw cwd if it does not exist.
        let cwd = tokio::fs::canonicalize(self.session_cwd.cwd())
            .await
            .unwrap_or_else(|_| self.session_cwd.cwd());

        let mut surfaced: Vec<crate::prompt::MemoryFile> = Vec::new();
        {
            let mut sent = self.sent_nested_memory.lock().await;
            let mut sent_rules = self.sent_conditional_rules.lock().await;
            for trigger in &touched {
                for f in
                    crate::prompt::nested_memory::discover(trigger, &cwd, &home, managed.as_deref())
                {
                    if sent.contains(&f.path) {
                        continue;
                    }
                    // A `paths:`-gated rule is owned by BOTH mechanisms; the
                    // shared set means whichever reaches the model first wins
                    // and the other stands down. Unconditional memory files
                    // never enter this set — conditional rules is not their
                    // owner and marking them would be a lie.
                    if f.globs.is_some() && sent_rules.contains(&f.path) {
                        continue;
                    }
                    // `!t.readFileState.has(i.path)`, canonical-keyed like the
                    // registry itself. Note the oracle does NOT mark such a
                    // path as loaded — it stays in `readFileState` forever, so
                    // it stays skipped either way.
                    let key = tokio::fs::canonicalize(&f.path)
                        .await
                        .unwrap_or_else(|_| f.path.clone());
                    if self
                        .read_state_map
                        .lock()
                        .is_ok_and(|guard| guard.contains(&key))
                    {
                        continue;
                    }
                    sent.insert(f.path.clone());
                    if f.globs.is_some() {
                        sent_rules.insert(f.path.clone());
                    }
                    surfaced.push(f);
                }
            }
        }
        if surfaced.is_empty() {
            return None;
        }
        // The oracle's `k$o` returns RECORDS, not text — the reminder is one
        // rendering of them and the UI attachment line is the other. The port
        // originally took only the text half, so the attachment cells the TUI
        // already knows how to draw had no producer. `displayPath` is the
        // oracle's `relative(cwd, path)`.
        for file in &surfaced {
            let display_path = file
                .path
                .strip_prefix(&cwd)
                .unwrap_or(&file.path)
                .display()
                .to_string();
            self.output
                .emit_attachment(traits::AttachmentKind::NestedMemory { display_path })
                .await;
        }
        self.seed_nested_memory_read_state(&surfaced).await;
        let content = surfaced
            .iter()
            .map(crate::prompt::conditional_rules::render_reminder)
            .collect::<Vec<_>>()
            .join("\n\n");
        Some(ConversationMessage::user(MessageId::new(), content))
    }

    /// Seed `read_file_state` for files surfaced as NESTED MEMORY — `k$o`'s
    /// `readFileState.set` half.
    ///
    /// Deliberately NOT [`Self::seed_memory_read_state`], which ports the
    /// EAGER-block seeding site (`xCt` @245883373). The two sites disagree on
    /// two fields, and the difference is load-bearing:
    ///
    /// | | eager (`xCt`) | nested (`k$o`) |
    /// |---|---|---|
    /// | `seededFromContext` | `MLu(file)` — only if actually rendered | `!0` always |
    /// | `timestamp` | mtime when rendered, else `Date.now()` | mtime always |
    ///
    /// A conditional rule is NOT in the eager block, so the eager site would
    /// seed it `seeded_from_context:false` — and the dedup stub would never
    /// fire for exactly the files this reminder just put in the context.
    async fn seed_nested_memory_read_state(&self, files: &[crate::prompt::MemoryFile]) {
        for f in files {
            // Canonical key, lexical render — the fork that already shipped one
            // silent bug: `FileReadTool` looks up `canonicalize_and_validate`'s
            // output, so seeding under the raw path never matches on a macOS
            // `/var` -> `/private/var` cwd.
            let key = tokio::fs::canonicalize(&f.path)
                .await
                .unwrap_or_else(|_| f.path.clone());
            let now_ms = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis()),
            )
            .unwrap_or(i64::MAX);
            // `try{s=FQ(i.path)}catch{s=Date.now()}` — unconditional mtime.
            let mtime_ms = match tokio::fs::metadata(&f.path)
                .await
                .and_then(|m| m.modified())
            {
                Ok(t) => tool_api::read_file_state::mtime_ms_floor(t),
                Err(_) => now_ms,
            };
            let content = if f.content_differs_from_disk {
                f.raw_content.clone()
            } else {
                f.raw_content
                    .strip_prefix('\u{feff}')
                    .unwrap_or(&f.raw_content)
                    .to_string()
            };
            tool_api::read_file_state::set_with_model_context(
                &self.read_state_map,
                key,
                tool_api::read_file_state::ReadFileEntry {
                    content,
                    mtime_ms,
                    offset: None,
                    limit: None,
                    from_read: false,
                    seeded_from_context: true,
                    is_partial_view: f.content_differs_from_disk,
                },
                // ALWAYS false, for the reason spelled out on
                // `seed_memory_read_state`, plus one specific to this site: the
                // touched-file set is this reminder's own INPUT, so enrolling a
                // surfaced memory file would make it a trigger for the next
                // turn's discovery — a feedback loop walking its own ancestors.
                false,
            );
        }
    }

    /// P0.1: arm the memory-selector prefetch for THIS turn, firing it
    /// CONCURRENTLY with the main API call (claude-code's `wAo` prefetch
    /// side-channel). Called at the START of each turn in BOTH drivers, BEFORE
    /// the snapshot is assembled, so the in-flight handle is ready for
    /// [`Self::relevant_memory_reminder_messages`] to await. A strict no-op when
    /// no prefetch is wired ([`Self::memory_prefetch`] is `None`) — then the slot
    /// stays empty and the surfacing reminder list is empty, keeping the locked
    /// fixtures byte-identical.
    ///
    /// The prefetch query is the latest NON-meta user-message text in the
    /// session history (mirroring TS `e.findLast(m => m.type==="user" &&
    /// !m.isMeta)` in `wAo`). The memdir directory is derived from the cwd; the
    /// stub prefetch ignores both for now (it resolves to an empty set) so this
    /// is inert by default.
    async fn discard_stale_prefetches(&self) {
        *self.pending_memory_prefetch.lock().await = None;
        *self.pending_skill_prefetch.lock().await = None;
    }

    pub(crate) async fn start_memory_prefetch(&self) {
        let Some(prefetch) = self.memory_prefetch.as_ref() else {
            return; // no prefetch wired ⇒ surfacing channel stays inert
        };
        // Keep at most one in-flight selector. A slow result continues under
        // the current model call instead of being overwritten or queueing
        // unbounded side queries.
        if self.pending_memory_prefetch.lock().await.is_some() {
            return;
        }
        // Latest REAL user message = the turn query. Compact summaries,
        // transcript-only rows, Stop-hook feedback, and tool-result user rows
        // are synthetic context rather than user intent.
        let query = {
            let s = self.session.lock().await;
            s.history
                .iter()
                .rev()
                .find_map(|message| match message {
                    ConversationMessage::User {
                        content,
                        is_meta: false,
                        is_compact_summary: false,
                        is_visible_in_transcript_only: false,
                        ..
                    } => {
                        let text = content
                            .iter()
                            .filter_map(|block| match block {
                                protocol::ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        (!text.is_empty()).then_some(text)
                    }
                    _ => None,
                })
                .unwrap_or_default()
        };
        // Task 5 (worktree 206 session-cwd plumbing): the live cwd, so a future
        // non-stub prefetch derives the memdir from the post-swap worktree, not
        // the frozen boot cwd. Currently inert (the stub prefetch ignores its
        // cwd argument), so this is a no-behavior-change correctness fix.
        let pending = prefetch.start(query, self.session_cwd.cwd()).await;
        *self.pending_memory_prefetch.lock().await = Some(pending);
    }

    /// P0.1: the per-turn, transient `relevant_memories` SURFACING reminders — the
    /// memory-selector/prefetch result rendered as one meta user message per
    /// surfaced memory. Returns an empty list when no prefetch was armed this turn
    /// ([`Self::start_memory_prefetch`] left the slot empty / no prefetch wired),
    /// the prefetch resolved to an empty set, or every surfaced memory was
    /// already injected (the SHARED dedup below).
    ///
    /// 1:1 with claude-code v2.1.181's `relevant_memories` attachment
    /// (`normalizeAttachmentForAPI` case `"relevant_memories"`, messages.ts —
    /// see [`memory::surfacing::render_surfacing_messages`] for the exact shape):
    /// the em-dash idx-0 preamble + per-memory `Memory: {path}:` header (with a
    /// `>1`-day staleness prefix), preserving each memory's message boundary.
    ///
    /// SHARED DEDUP: a memory is skipped when its path is in EITHER
    /// [`Self::surfaced_memory_paths`] (already surfaced a prior turn) OR
    /// [`Self::read_state_map`] (already loaded as a nested/conditional P3.2
    /// attachment OR read by a file tool) — so a file can never be double-injected
    /// across the surfacing + nested channels. Surfaced paths are recorded so each
    /// memory injects ONCE (TS prefetch consume-once + `loadedNestedMemoryPaths`).
    ///
    /// Like every other per-turn reminder, the message is appended ONLY to the
    /// per-turn OUTGOING snapshot (never `session.history` / JSONL), so it is
    /// recomputed each turn and never accumulates.
    pub(crate) async fn relevant_memory_reminder_messages(&self) -> Vec<ConversationMessage> {
        // Never await an unresolved side query on the model-call critical path.
        // Leave it in the slot so it can run concurrently with this iteration
        // and be collected by a later one.
        let pending = {
            let mut slot = self.pending_memory_prefetch.lock().await;
            let Some(pending) = slot.take() else {
                return Vec::new();
            };
            if !pending.is_ready() {
                *slot = Some(pending);
                return Vec::new();
            }
            pending
        };
        let surfaced = pending.take().await;
        if surfaced.is_empty() {
            return Vec::new();
        }

        // SHARED DEDUP — skip any memory already surfaced this session OR already
        // loaded as a nested/conditional attachment / tool read (`read_state_map`).
        let already_read: std::collections::HashSet<std::path::PathBuf> = self
            .read_state_map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .model_context_keys()
            .into_iter()
            .collect();
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
            return Vec::new();
        }

        memory::surfacing::render_surfacing_messages(&fresh)
            .into_iter()
            .map(|content| ConversationMessage::user_meta(MessageId::new(), content))
            .collect()
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
        // One bounded in-flight discovery. A slow result remains eligible for a
        // later iteration and never queues another side query behind it.
        if self.pending_skill_prefetch.lock().await.is_some() {
            return;
        }
        // Latest non-meta user message = the turn query (TS findLast user/!meta),
        // and the most recent assistant message's tool names for the write-pivot
        // predicate — both read in one history lock.
        let (query, last_assistant_tools) = {
            let s = self.session.lock().await;
            let query = s
                .history
                .iter()
                .rev()
                .find_map(|message| match message {
                    ConversationMessage::User {
                        content,
                        is_meta: false,
                        is_compact_summary: false,
                        is_visible_in_transcript_only: false,
                        ..
                    } => {
                        let text = content
                            .iter()
                            .filter_map(|block| match block {
                                protocol::ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        (!text.is_empty()).then_some(text)
                    }
                    _ => None,
                })
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
        let pending = {
            let mut slot = self.pending_skill_prefetch.lock().await;
            let pending = slot.take()?;
            // A side query that has not hidden under available work must not
            // delay the API call. Keep it alive and try again next iteration.
            if !pending.is_ready() {
                *slot = Some(pending);
                telemetry::emit_skill_discovery_collected(false);
                return None;
            }
            pending
        };
        telemetry::emit_skill_discovery_collected(true);

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
    /// tool prompt as the `description`. The post-filter base wire schemas are
    /// cached per session by `(tool names, model, model_profile)`; per-turn
    /// dynamic fields are still applied after cloning the cached base. The wire
    /// order is parity-fixed by
    /// [`available_tools`](tool_api::ToolRegistry::available_tools): builtins
    /// `locale_cmp`-sorted as a contiguous prefix, then MCP / LSP / plugin
    /// tools `locale_cmp`-sorted — matching claude-code's `assembleToolPool` /
    /// `mergeAndFilterTools` (`tools.ts:345-367`, `utils/toolPool.ts:65-70`),
    /// which sort with `name.localeCompare`. `tools_to_wire` preserves that
    /// order. A session-level cache is a recommended follow-up.
    ///
    /// [`execute_one_turn`]: crate::turn_loop::execute_one_turn
    pub(crate) async fn build_wire_tools(&self) -> Vec<serde_json::Value> {
        use tool_api::tool_trait::PromptOptions;
        let tools = self.filtered_available_tools().await;
        // claude-code builds the wire `tools` array with `prompt({model})`; the
        // session model gates model-dependent tool prompts (TodoWrite's
        // `Xla(model)=Dh(model)?FWd:UWd`). Snapshot it from the live session.
        let (model, model_profile) = {
            let s = self.session.lock().await;
            (s.model.clone(), s.model_profile.clone())
        };
        let cache_key = WireToolSchemaCacheKey {
            tool_names: tools.iter().map(|t| t.name().to_string()).collect(),
            model: model.clone(),
            model_profile: model_profile.clone(),
        };
        let mut wire = {
            let cached = self.wire_tool_schema_cache.lock().await.clone();
            if let Some(cached) = cached.filter(|entry| entry.key == cache_key) {
                cached.wire
            } else {
                let wire = tool_api::wire::tools_to_wire(
                    &tools,
                    &PromptOptions {
                        include_examples: true,
                        model: Some(model.clone()),
                        model_profile: model_profile.clone(),
                    },
                )
                .await;
                *self.wire_tool_schema_cache.lock().await = Some(WireToolSchemaCache {
                    key: cache_key.clone(),
                    wire: wire.clone(),
                });
                wire
            }
        };
        // Structured-output strict mode (claude-code `tengu_structured_output_strict`
        // + `strictInputJSONSchema`): when the flag is on, mark the forced
        // `StructuredOutput` tool `strict` so the Anthropic codec sends its
        // schema in strict form. Default-OFF ⇒ no tool is marked ⇒ wire bytes
        // unchanged. Provider-gated downstream (only the Anthropic codec acts on
        // `strict`).
        if telemetry::flag_bool("tengu_structured_output_strict", false) {
            for t in &mut wire {
                if t.get("name").and_then(serde_json::Value::as_str)
                    == Some(crate::structured_output::STRUCTURED_OUTPUT_TOOL_NAME)
                {
                    if let Some(obj) = t.as_object_mut() {
                        obj.insert("strict".to_string(), serde_json::Value::Bool(true));
                    }
                }
            }
        }
        let tool_search_present = tools.iter().any(|tool| tool.name() == "ToolSearch");
        let has_deferred_candidates = tools.iter().any(|tool| {
            self.tools
                .deferral()
                .wants_defer(tool.name(), tool.should_defer())
        });
        // Keep ToolSearch available while an MCP server is still connecting,
        // even if the current catalog has no deferred definitions yet. Claude
        // does this so a model can retry discovery after the pending server
        // publishes its tools instead of permanently losing ToolSearch for the
        // turn/session.
        let has_pending_mcp_servers = if has_deferred_candidates {
            false
        } else if let Some(registry) = &self.mcp_registry {
            !registry.servers_pending().await.is_empty()
        } else {
            false
        };
        let request_supported = tool_search_present
            && (has_deferred_candidates || has_pending_mcp_servers)
            && tool_search_supported_for_request(&model, model_profile.as_deref());
        self.tools
            .deferral()
            .set_request_supported(request_supported);
        // Refresh the session-scoped tool-search gate (Claude Code `$U()`) for the
        // request builder now that the model/profile are resolved. Unlike
        // `request_supported`, `$U()` depends ONLY on the session mode + provider
        // support — never on a present `ToolSearch` tool or deferred candidates —
        // so side queries assembled with an empty toolset take the same
        // normalization branch as the main loop.
        traits::session_flags::set_tool_search_enabled(
            self.tools.deferral().mode().is_enabled()
                && tool_search_supported_for_request(&model, model_profile.as_deref()),
        );
        let complete_wire = wire.clone();
        // Tool Search (2.1.216): omit undiscovered deferred definitions and
        // stamp discovered definitions with `defer_loading: true`. The shared
        // `DeferralState` is owned by the registry and restored on resume, so a
        // prior ToolSearch result remains available without exposing the rest
        // of the deferred catalog.
        let context_window =
            compaction::context_window::context_window_for_model(&model, &self.api.active_betas());
        let exact_deferred_tokens = if self.tools.deferral().auto_percentage().is_some()
            && request_supported
            && has_deferred_candidates
        {
            let cached = self
                .deferred_tool_token_cache
                .lock()
                .await
                .get(&cache_key)
                .copied();
            if let Some(cached) = cached {
                cached
            } else {
                let deferred_names: std::collections::HashSet<&str> = tools
                    .iter()
                    .filter(|tool| {
                        self.tools
                            .deferral()
                            .wants_defer(tool.name(), tool.should_defer())
                    })
                    .map(|tool| tool.name())
                    .collect();
                let deferred_wire: Vec<serde_json::Value> = complete_wire
                    .iter()
                    .filter(|tool| {
                        tool.get("name")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|name| deferred_names.contains(name))
                    })
                    .cloned()
                    .collect();
                // Claude subtracts the fixed request/tool envelope from the
                // exact count. A zero response means the endpoint is
                // unavailable and selects the character fallback.
                let counted = match self
                    .api
                    .count_tokens_exact(
                        &model,
                        model_profile.as_deref(),
                        None,
                        Vec::new(),
                        deferred_wire,
                    )
                    .await
                {
                    Ok(Some(total)) if total != 0 => {
                        Some(total.saturating_sub(TOOL_TOKEN_COUNT_OVERHEAD))
                    }
                    Ok(_) | Err(_) => None,
                };
                self.deferred_tool_token_cache
                    .lock()
                    .await
                    .insert(cache_key.clone(), counted);
                counted
            }
        } else {
            None
        };
        tool_api::wire::apply_defer_loading_with_context_and_tokens(
            &mut wire,
            &tools,
            self.tools.deferral(),
            context_window,
            exact_deferred_tokens,
        );
        // Auto mode learns whether it is active only after the schemas are
        // serialized and measured. Publish the complete deferred candidate
        // view (including descriptions), then hide ToolSearch itself whenever
        // the resolved request does not support the beta.
        self.tools
            .refresh_tool_search_view_from_wire(&complete_wire);
        if !self.tools.deferral().is_enabled() {
            wire.retain(|tool| {
                tool.get("name").and_then(serde_json::Value::as_str) != Some("ToolSearch")
            });
        }
        wire
    }

    /// `deferred_tools_delta` reminder for THIS outgoing model step, prepended to
    /// the transient snapshot and never persisted.
    ///
    /// Claude Code (`A1s` + the `deferred_tools_delta` attachment renderer)
    /// announces the searchable deferred set as a `<system-reminder>` only when
    /// that set has CHANGED since the prior request — listing the newly-available
    /// names, re-appearing names (MCP reconnect), and removed names — rather than
    /// repeating the whole catalog every turn. The announced / ever-added state
    /// is tracked in the session's [`DeferralState`].
    ///
    /// MUTATES the delta tracking (via `compute_deferred_delta`), so it must be
    /// called at most ONCE per outgoing model step. Callers that rebuild the
    /// snapshot for a retry of the SAME step (streaming recovery / non-streaming
    /// fallback) reuse the value computed here instead of re-invoking it.
    pub(crate) fn deferred_tools_reminder_message(&self) -> Option<ConversationMessage> {
        if !self.tools.deferral().is_enabled() {
            return None;
        }
        // Currently-deferred (undiscovered) set = oracle `g`: the searchable
        // view minus tools already loaded this session. The view is the
        // `wants_defer` candidate set (which still includes loaded tools), so
        // subtract the loaded names to obtain the `should_defer` set.
        let loaded: std::collections::HashSet<String> = self
            .tools
            .deferral()
            .loaded_tool_names()
            .into_iter()
            .collect();
        let mut current: Vec<String> = self
            .tools
            .tool_search_view()
            .entries()
            .into_iter()
            .map(|entry| entry.name)
            .filter(|name| !loaded.contains(name))
            .collect();
        current.sort();
        current.dedup();
        let delta = self.tools.deferral().compute_deferred_delta(&current);
        let body = delta.render_reminder()?;
        Some(ConversationMessage::user_meta(MessageId::new(), body))
    }

    /// `getTools`' (`iJ`) 2.1.238 tail block — the path that puts
    /// `WaitForMcpServers` in front of the model while MCP servers are still
    /// connecting:
    ///
    /// ```text
    /// if(eZf()&&!l.some((c)=>il(c,y0))&&!l.some((c)=>il(c,Qze)))l=[...l,...Ohe([Sdl],e)];
    /// ```
    ///
    /// (`cc-238.js @230759940`; `y0="ToolSearch"`, `Qze="WaitForMcpServers"`,
    /// `Ohe` = the deny-rule filter, `eZf(){return bdl(b7e()??[]).length>0}` =
    /// "at least one MCP client is `type === "pending"`"). 2.1.220's `d6` has no
    /// such block; the TOOL itself is not new (2.1.220 registers it too).
    ///
    /// The composite is: `WaitForMcpServers` is advertised iff at least one MCP
    /// server is still pending AND `ToolSearch` is not in the final list.
    ///
    /// The first half is `Sdl.isEnabled`'s `bdl(t).length>0` leg, delivered
    /// through the registry's synchronous pending mirror (refreshed here, read
    /// by `WaitForMcpServersTool::is_enabled` so `available_tools` still sorts
    /// the tool into the builtin prefix at its `locale_cmp` position). The
    /// second half is the tail block's `!l.some((c)=>il(c,y0))` guard, applied
    /// as a removal because `is_enabled` cannot see the rest of the list.
    ///
    /// `Ohe`'s deny filter runs over the whole list in the caller, so a deny
    /// rule naming `WaitForMcpServers` still removes it — no separate pass is
    /// needed for the appended tool the way the oracle needs `Ohe([Sdl],e)`.
    fn apply_wait_for_mcp_servers_gate(
        tools: &mut Vec<std::sync::Arc<dyn tool_api::tool_trait::Tool>>,
    ) {
        // Name literals rather than `tool_mcp::…::WAIT_FOR_MCP_SERVERS_TOOL_NAME`
        // / `tool_meta::tool_search::TOOL_SEARCH_TOOL_NAME`: `orchestrator` does
        // not (and should not) depend on the tool crates. Pinned by
        // `tool_mcp::wait_for_mcp_servers::tests::name_is_byte_exact`.
        const WAIT_FOR_MCP_SERVERS: &str = "WaitForMcpServers";
        const TOOL_SEARCH: &str = "ToolSearch";

        if !tools.iter().any(|t| t.name() == WAIT_FOR_MCP_SERVERS) {
            return;
        }
        if tools.iter().any(|t| t.name() == TOOL_SEARCH) {
            tools.retain(|t| t.name() != WAIT_FOR_MCP_SERVERS);
        }
    }

    async fn filtered_available_tools(
        &self,
    ) -> Vec<std::sync::Arc<dyn tool_api::tool_trait::Tool>> {
        use tool_api::tool_trait::ToolStaticContext;

        // `eZf()` — refresh the registry's synchronous pending-server mirror
        // before the enable-filter runs, so `WaitForMcpServersTool::is_enabled`
        // observes the live state. See `apply_wait_for_mcp_servers_gate`.
        if let Some(registry) = self.mcp_registry.as_ref() {
            registry.refresh_pending_servers().await;
        }
        let mut tools = self.tools.available_tools(&ToolStaticContext::default());
        let denied = self.perms.tool_wide_deny_names().await;
        if !denied.is_empty() {
            tools.retain(|t| {
                !denied
                    .iter()
                    .any(|d| permission::tool_wide_name_matches(d, t.name()))
            });
        }
        Self::apply_wait_for_mcp_servers_gate(&mut tools);
        {
            let guard = self.main_thread_agent.read().await;
            if let Some(agent) = guard.as_ref() {
                if !agent.disallowed_tools.is_empty() {
                    let def_denied: std::collections::HashSet<&str> = agent
                        .disallowed_tools
                        .iter()
                        .map(|spec| spec.split('(').next().unwrap_or(spec).trim())
                        .collect();
                    tools.retain(|t| !def_denied.contains(t.name()));
                }
                match &agent.tool_policy {
                    agent::AgentToolPolicy::All { .. } => {}
                    agent::AgentToolPolicy::Explicit(names) => {
                        tools.retain(|t| names.iter().any(|n| n == t.name()));
                    }
                    agent::AgentToolPolicy::Except(names) => {
                        tools.retain(|t| !names.iter().any(|n| n == t.name()));
                    }
                }
            }
        }
        tools
    }

    async fn mobile_runtime_environment_message(&self) -> Option<ConversationMessage> {
        if let Some(message) = &self.mobile_runtime_environment_message {
            return Some(message.clone());
        }
        let environment = self.mobile_runtime_environment.as_ref()?;
        Some(ConversationMessage::user_meta(
            MessageId::new(),
            environment.render_system_reminder(),
        ))
    }

    fn text_content(message: &ConversationMessage) -> Option<String> {
        match message {
            ConversationMessage::User { content, .. } => content.iter().find_map(|block| {
                if let protocol::ContentBlock::Text { text } = block {
                    Some(text.clone())
                } else {
                    None
                }
            }),
            _ => None,
        }
    }

    fn is_mobile_runtime_environment_message(message: &ConversationMessage) -> bool {
        Self::text_content(message).is_some_and(|text| {
            text.starts_with("<system-reminder>\nMobile runtime environment (version ")
        })
    }

    /// Return the live, policy-filtered tool catalog in MCP's 2025-06-18
    /// `tools/list` shape. This deliberately reuses the normal model-facing
    /// schema builder so tool enablement, permission-wide denies, active-agent
    /// restrictions, descriptions, and ordering cannot drift between hosts.
    pub async fn mcp_tool_definitions(&self) -> Vec<serde_json::Value> {
        self.build_wire_tools()
            .await
            .into_iter()
            .filter_map(|wire| {
                let object = wire.as_object()?;
                let mut tool = serde_json::Map::new();
                tool.insert("name".into(), object.get("name")?.clone());
                if let Some(description) = object.get("description") {
                    tool.insert("description".into(), description.clone());
                }
                tool.insert(
                    "inputSchema".into(),
                    object
                        .get("input_schema")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
                );
                Some(serde_json::Value::Object(tool))
            })
            .collect()
    }

    /// Execute one host-originated tool request through the same dispatcher as
    /// a model-originated tool use: schema validation, PreToolUse hooks,
    /// permission policy, sandbox-backed tool execution, PostToolUse hooks, and
    /// result shaping. `None` identifies an unknown tool before dispatch.
    pub async fn call_tool_from_host(
        &self,
        tool_use_id: protocol::ToolUseId,
        name: String,
        input: serde_json::Value,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<Option<protocol::ContentBlock>, OrchestratorError> {
        if self.tools.find_by_name(&name).is_none() {
            return Ok(None);
        }
        let tool_uses = vec![(tool_use_id, name, input, None)];
        let (mut results, _prevent_continuation, _injected, modifiers) =
            crate::turn_loop::dispatch_tool_uses_tracked(self, &tool_uses, cancel).await?;
        crate::turn_loop::apply_model_context_modifiers(self, modifiers).await;
        // O3: this host-driven path writes NO transcript line at all (the block
        // is handed back to the caller), so a queued hook attachment would be a
        // chain orphan. Drain it rather than leaving the entry in the map for
        // the life of the session.
        let _ = self.take_queued_hook_attachments(&tool_uses[0].0).await;
        Ok(results.pop())
    }

    /// Borrow the in-memory session (read-write lock surrogate). Useful for tests.
    #[must_use]
    pub fn session(&self) -> Arc<Mutex<SessionState>> {
        self.session.clone()
    }

    /// Snapshot the live conversation history for an embedding host that owns
    /// a separate durable session store.
    pub async fn snapshot_history(&self) -> Vec<ConversationMessage> {
        self.session.lock().await.history.clone()
    }

    /// Restore history before the first turn of a newly constructed
    /// orchestrator. The session id and all host-owned runtime state remain
    /// local to this orchestrator; only the ordered message transcript is
    /// adopted.
    pub async fn restore_history(&self, history: Vec<ConversationMessage>) -> Result<(), String> {
        let mut session = self.session.lock().await;
        if !session.history.is_empty() {
            return Err("cannot restore Agent history after a turn has started".into());
        }
        session.history = history;
        Ok(())
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

    /// Update the thinking policy for the next API request.
    pub fn set_thinking_config(&self, thinking: llm_client::model::thinking::ThinkingConfig) {
        self.api.set_thinking_config(thinking);
    }

    /// Update how the attached output sink presents subsequent thinking blocks.
    pub fn set_thinking_display(&self, mode: Option<&str>) {
        self.output.set_thinking_display(mode);
    }

    /// Update the effort carried by the next API request and by subsequently
    /// persisted assistant transcript rows.
    pub fn set_effort(&self, effort: Option<String>) {
        self.current_effort_explicit
            .store(true, std::sync::atomic::Ordering::Release);
        *self
            .current_reasoning_selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = effort
            .clone()
            .map(|id| traits::ReasoningSelection::Level { id })
            .unwrap_or(traits::ReasoningSelection::Automatic);
        self.apply_effort(effort);
    }

    /// Restore transcript effort only when no launch/control override owns the
    /// live value. Unlike [`Self::set_effort`], inheritance deliberately does
    /// not pin the value, so a later resume can adopt or clear it again.
    pub(crate) fn restore_effort_from_resume(
        &self,
        model: &str,
        provider_id: Option<&str>,
        effort: Option<String>,
    ) {
        if self
            .current_effort_explicit
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let selection = effort
            .clone()
            .map(|id| traits::ReasoningSelection::Level { id })
            .unwrap_or(traits::ReasoningSelection::Automatic);
        let (validated, thinking, provider_effort, legacy_effort) =
            Self::reasoning_request_state(model, provider_id, &selection);
        *self
            .current_reasoning_selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = validated;
        *self
            .current_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = legacy_effort;
        self.api.set_thinking_config(thinking);
        self.api.set_effort(provider_effort);
    }

    /// Restore a structured transcript selection without claiming it as an
    /// explicit live override. Older transcripts continue through
    /// `restore_effort_from_resume`; newer rows retain toggles and budgets.
    pub(crate) fn restore_reasoning_selection_from_resume(
        &self,
        model: &str,
        provider_id: Option<&str>,
        selection: traits::ReasoningSelection,
    ) {
        if self
            .current_effort_explicit
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let (validated, thinking, provider_effort, legacy_effort) =
            Self::reasoning_request_state(model, provider_id, &selection);
        *self
            .current_reasoning_selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = validated;
        *self
            .current_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = legacy_effort;
        self.api.set_thinking_config(thinking);
        self.api.set_effort(provider_effort);
    }

    fn apply_effort(&self, effort: Option<String>) {
        *self
            .current_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = effort.clone();
        self.api.set_effort(effort.map(serde_json::Value::String));
    }

    #[must_use]
    pub fn current_reasoning_selection(&self) -> traits::ReasoningSelection {
        self.current_reasoning_selection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Project the llm-client's request-facing capability registry into the
    /// provider-neutral controls DTO. Keeping this conversion at the
    /// orchestrator seam means the same catalog that validates/encodes a
    /// request also drives the mobile UI; unknown/custom profiles remain
    /// Auto-only because they have no verified adapter contract.
    fn reasoning_spec_for_model(
        model: &str,
        provider_id: Option<&str>,
    ) -> traits::ReasoningControlSpec {
        // A missing profile is the legacy/builtin route for known Anthropic
        // models. Infer it only from the shared model-capability registry;
        // arbitrary or custom ids remain Auto-only instead of inheriting
        // Anthropic controls by name.
        let inferred_provider = provider_id.or_else(|| {
            traits::model_capabilities::has_capability(
                model,
                traits::model_capabilities::ModelCapability::Effort,
            )
            .then_some("builtin")
        });
        let (protocol, base_url) = match inferred_provider.unwrap_or_default() {
            "anthropic" | "builtin" => (
                llm_client::ProtocolFamily::AnthropicMessages,
                "https://api.anthropic.com",
            ),
            "openai" => (
                llm_client::ProtocolFamily::OpenAiResponses,
                "https://api.openai.com/v1",
            ),
            "openai-chatgpt" => (
                llm_client::ProtocolFamily::OpenAiResponses,
                "https://chatgpt.com/backend-api/codex",
            ),
            "gemini" => (
                llm_client::ProtocolFamily::GeminiGenerateContent,
                "https://generativelanguage.googleapis.com/v1beta",
            ),
            "deepseek" => (
                llm_client::ProtocolFamily::OpenAiChat,
                "https://api.deepseek.com",
            ),
            "kimi" => (
                llm_client::ProtocolFamily::OpenAiChat,
                "https://api.moonshot.cn/v1",
            ),
            "kimi-code" => (
                llm_client::ProtocolFamily::OpenAiChat,
                "https://api.kimi.com/coding/v1",
            ),
            "openrouter" => (
                llm_client::ProtocolFamily::OpenAiChat,
                "https://openrouter.ai/api/v1",
            ),
            _ => return traits::reasoning_control_spec_for_model(model, inferred_provider),
        };

        let raw = llm_client::reasoning_controls::reasoning_control_spec(
            llm_client::reasoning_controls::ReasoningTarget {
                profile_name: inferred_provider,
                protocol: &protocol,
                base_url,
                model,
            },
        );
        let mandatory = raw
            .mandatory_selection
            .as_ref()
            .map(|selection| match selection {
                llm_client::reasoning_controls::ReasoningSelection::Automatic => {
                    traits::ReasoningSelection::Automatic
                }
                llm_client::reasoning_controls::ReasoningSelection::Disabled => {
                    traits::ReasoningSelection::Disabled
                }
                llm_client::reasoning_controls::ReasoningSelection::Enabled => {
                    traits::ReasoningSelection::Enabled
                }
                llm_client::reasoning_controls::ReasoningSelection::Level(id) => {
                    traits::ReasoningSelection::Level { id: id.clone() }
                }
                llm_client::reasoning_controls::ReasoningSelection::TokenBudget(tokens) => {
                    traits::ReasoningSelection::TokenBudget {
                        tokens: u64::from(*tokens),
                    }
                }
            });

        let mut available = Vec::new();
        if let Some(mandatory) = &mandatory {
            available.push(mandatory.clone());
        } else {
            available.push(traits::ReasoningSelection::Automatic);
            if raw.can_disable {
                available.push(traits::ReasoningSelection::Disabled);
            }
            if raw.can_enable {
                available.push(traits::ReasoningSelection::Enabled);
            }
            available.extend(
                raw.levels
                    .iter()
                    .cloned()
                    .map(|id| traits::ReasoningSelection::Level { id }),
            );
        }

        let auto_only = available.len() == 1
            && matches!(
                available.first(),
                Some(traits::ReasoningSelection::Automatic)
            )
            && raw.token_budget.is_none();
        traits::ReasoningControlSpec {
            available,
            selections_persistable: mandatory.is_none(),
            budget_range: raw.token_budget.map(|range| traits::ReasoningBudgetRange {
                min_tokens: range.min,
                max_tokens: range.max,
                supports_dynamic: false,
                supports_disabled: raw.can_disable,
            }),
            provider_default: mandatory.unwrap_or(traits::ReasoningSelection::Automatic),
            forced: raw.mandatory_selection.is_some(),
            modifiable: raw.mandatory_selection.is_none() && !auto_only,
            disabled_reason: if raw.mandatory_selection.is_some() {
                Some("reasoning_required".to_string())
            } else if auto_only {
                Some("reasoning_unavailable".to_string())
            } else {
                None
            },
        }
    }

    fn validate_reasoning_selection(
        selection: &traits::ReasoningSelection,
        model: &str,
        provider_id: Option<&str>,
    ) -> traits::ReasoningSelection {
        let spec = Self::reasoning_spec_for_model(model, provider_id);
        let supported = match selection {
            traits::ReasoningSelection::Automatic => true,
            traits::ReasoningSelection::TokenBudget { tokens } => {
                spec.budget_range.as_ref().is_some_and(|range| {
                    (*tokens >= u64::from(range.min_tokens)
                        && *tokens <= u64::from(range.max_tokens))
                        || (range.supports_disabled && *tokens == 0)
                })
            }
            other => spec.available.iter().any(|candidate| candidate == other),
        };
        if spec.forced && !spec.modifiable {
            spec.provider_default
        } else if supported {
            selection.clone()
        } else {
            traits::ReasoningSelection::Automatic
        }
    }

    fn reasoning_request_state(
        model: &str,
        provider_id: Option<&str>,
        selection: &traits::ReasoningSelection,
    ) -> (
        traits::ReasoningSelection,
        llm_client::model::thinking::ThinkingConfig,
        Option<serde_json::Value>,
        Option<String>,
    ) {
        use llm_client::model::thinking::ThinkingConfig;
        let validated = Self::validate_reasoning_selection(selection, model, provider_id);
        let effort_level = |id: &str| Some(serde_json::Value::String(id.to_string()));
        let legacy = |id: &str| Some(id.to_string());
        let provider_id = provider_id.or_else(|| {
            traits::model_capabilities::has_capability(
                model,
                traits::model_capabilities::ModelCapability::Effort,
            )
            .then_some("builtin")
        });
        let provider_id = provider_id.unwrap_or_default();
        let model_lc = model.to_ascii_lowercase();

        match provider_id {
            "anthropic" | "builtin" => match &validated {
                traits::ReasoningSelection::Automatic => {
                    (validated, ThinkingConfig::Automatic, None, None)
                }
                traits::ReasoningSelection::Disabled => {
                    (validated, ThinkingConfig::Disabled, None, None)
                }
                traits::ReasoningSelection::TokenBudget { tokens } => (
                    validated.clone(),
                    ThinkingConfig::Enabled {
                        budget_tokens: (*tokens).try_into().unwrap_or(u32::MAX),
                    },
                    None,
                    None,
                ),
                traits::ReasoningSelection::Level { id } => (
                    validated.clone(),
                    ThinkingConfig::Adaptive,
                    effort_level(id),
                    legacy(id),
                ),
                traits::ReasoningSelection::Enabled => {
                    (validated, ThinkingConfig::Adaptive, None, None)
                }
            },
            "openai" | "openai-chatgpt" => match &validated {
                traits::ReasoningSelection::Automatic => {
                    (validated, ThinkingConfig::Automatic, None, None)
                }
                traits::ReasoningSelection::Level { id } => (
                    validated.clone(),
                    ThinkingConfig::Adaptive,
                    effort_level(id),
                    legacy(id),
                ),
                traits::ReasoningSelection::Disabled => {
                    // Responses API uses the explicit `none` effort value to
                    // distinguish a user-off override from provider Auto.
                    (
                        validated,
                        ThinkingConfig::Disabled,
                        effort_level("none"),
                        None,
                    )
                }
                traits::ReasoningSelection::Enabled => {
                    (validated, ThinkingConfig::Adaptive, None, None)
                }
                traits::ReasoningSelection::TokenBudget { .. } => {
                    (validated, ThinkingConfig::Adaptive, None, None)
                }
            },
            "gemini" => {
                if model_lc.starts_with("gemini-3.") {
                    match &validated {
                        traits::ReasoningSelection::Automatic => {
                            (validated, ThinkingConfig::Automatic, None, None)
                        }
                        traits::ReasoningSelection::Level { id } => (
                            validated.clone(),
                            ThinkingConfig::Adaptive,
                            effort_level(id),
                            legacy(id),
                        ),
                        traits::ReasoningSelection::Disabled => {
                            (validated, ThinkingConfig::Disabled, None, None)
                        }
                        traits::ReasoningSelection::Enabled => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                        traits::ReasoningSelection::TokenBudget { .. } => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                    }
                } else {
                    match &validated {
                        traits::ReasoningSelection::Automatic => {
                            (validated, ThinkingConfig::Automatic, None, None)
                        }
                        traits::ReasoningSelection::TokenBudget { tokens } => (
                            validated.clone(),
                            ThinkingConfig::Enabled {
                                budget_tokens: (*tokens).try_into().unwrap_or(u32::MAX),
                            },
                            None,
                            None,
                        ),
                        traits::ReasoningSelection::Disabled => {
                            (validated, ThinkingConfig::Disabled, None, None)
                        }
                        traits::ReasoningSelection::Enabled => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                        traits::ReasoningSelection::Level { .. } => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                    }
                }
            }
            "deepseek" => match &validated {
                traits::ReasoningSelection::Automatic => {
                    (validated, ThinkingConfig::Automatic, None, None)
                }
                traits::ReasoningSelection::Disabled => (
                    validated,
                    ThinkingConfig::Disabled,
                    effort_level("off"),
                    None,
                ),
                traits::ReasoningSelection::Level { id } => (
                    validated.clone(),
                    ThinkingConfig::Adaptive,
                    effort_level(id),
                    legacy(id),
                ),
                traits::ReasoningSelection::Enabled => {
                    (validated, ThinkingConfig::Adaptive, None, None)
                }
                traits::ReasoningSelection::TokenBudget { .. } => {
                    (validated, ThinkingConfig::Adaptive, None, None)
                }
            },
            "kimi" | "kimi-code" => {
                if matches!(model_lc.as_str(), "kimi-k3" | "k3" | "k3-256k") {
                    match &validated {
                        traits::ReasoningSelection::Automatic => {
                            (validated, ThinkingConfig::Automatic, None, None)
                        }
                        traits::ReasoningSelection::Level { id } => (
                            validated.clone(),
                            ThinkingConfig::Adaptive,
                            effort_level(id),
                            legacy(id),
                        ),
                        traits::ReasoningSelection::Disabled => {
                            (validated, ThinkingConfig::Disabled, None, None)
                        }
                        traits::ReasoningSelection::Enabled => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                        traits::ReasoningSelection::TokenBudget { .. } => {
                            (validated, ThinkingConfig::Adaptive, None, None)
                        }
                    }
                } else {
                    match &validated {
                        traits::ReasoningSelection::Automatic => {
                            (validated, ThinkingConfig::Automatic, None, None)
                        }
                        traits::ReasoningSelection::Disabled => (
                            validated,
                            ThinkingConfig::Disabled,
                            effort_level("off"),
                            None,
                        ),
                        traits::ReasoningSelection::Enabled => (
                            validated,
                            ThinkingConfig::Adaptive,
                            effort_level("on"),
                            None,
                        ),
                        traits::ReasoningSelection::Level { id } => (
                            validated.clone(),
                            ThinkingConfig::Adaptive,
                            effort_level(id),
                            legacy(id),
                        ),
                        traits::ReasoningSelection::TokenBudget { tokens } => (
                            validated.clone(),
                            ThinkingConfig::Enabled {
                                budget_tokens: (*tokens).try_into().unwrap_or(u32::MAX),
                            },
                            None,
                            None,
                        ),
                    }
                }
            }
            _ => match &validated {
                traits::ReasoningSelection::Automatic => {
                    (validated, ThinkingConfig::Automatic, None, None)
                }
                _ => (validated, ThinkingConfig::Adaptive, None, None),
            },
        }
    }

    pub fn set_reasoning_selection_for_model(
        &self,
        model: &str,
        provider_id: Option<&str>,
        selection: traits::ReasoningSelection,
    ) -> traits::ReasoningSelection {
        self.current_effort_explicit
            .store(true, std::sync::atomic::Ordering::Release);
        let (validated, thinking, effort, legacy_effort) =
            Self::reasoning_request_state(model, provider_id, &selection);
        *self
            .current_reasoning_selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = validated.clone();
        *self
            .current_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = legacy_effort.clone();
        self.api.set_thinking_config(thinking);
        self.api.set_effort(effort);
        validated
    }

    /// Seed a persisted new-session default without marking it as an explicit
    /// session override.  This lets resume restore a transcript's selection
    /// while still applying the user's default to a genuinely new session.
    pub fn initialize_reasoning_selection_for_model(
        &self,
        model: &str,
        provider_id: Option<&str>,
        selection: traits::ReasoningSelection,
    ) -> traits::ReasoningSelection {
        let (validated, thinking, effort, legacy_effort) =
            Self::reasoning_request_state(model, provider_id, &selection);
        self.current_effort_explicit
            .store(false, std::sync::atomic::Ordering::Release);
        *self
            .current_reasoning_selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = validated.clone();
        *self
            .current_effort
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = legacy_effort;
        self.api.set_thinking_config(thinking);
        self.api.set_effort(effort);
        validated
    }

    #[must_use]
    pub fn conversation_controls_for_model(
        &self,
        model: &str,
        provider_id: Option<&str>,
    ) -> traits::ConversationControls {
        let reasoning_spec = Self::reasoning_spec_for_model(model, provider_id);
        let requested_reasoning = self.current_reasoning_selection();
        let effective_reasoning =
            Self::validate_reasoning_selection(&requested_reasoning, model, provider_id);
        let requested_permission = self
            .permission_mode()
            .unwrap_or_else(|| "default".to_string());
        let effective_permission = requested_permission.clone();
        let permission_modes = [
            "default",
            "acceptEdits",
            "plan",
            "auto",
            "dontAsk",
            "bypassPermissions",
        ]
        .into_iter()
        .map(|mode| {
            let unavailable =
                mode == "bypassPermissions" && !self.perms.can_request_bypass_permissions();
            traits::PermissionModeAvailability {
                mode: mode.to_string(),
                available: !unavailable,
                disabled_reason: unavailable.then(|| "not_yet_available".to_string()),
            }
        })
        .collect();
        traits::ConversationControls {
            model_reference: traits::qualified_model_ref(model, provider_id),
            permission: traits::PermissionControlState {
                requested: requested_permission,
                effective: effective_permission,
                modes: permission_modes,
            },
            requested_reasoning_selection: requested_reasoning,
            effective_reasoning_selection: effective_reasoning,
            reasoning_spec,
        }
    }

    /// Apply a LIVE session permission-mode change (stream-json
    /// `set_permission_mode` control_request). Delegates to the gate's
    /// [`traits::PermissionGate::set_permission_mode`]; only the enforcing
    /// `PolicyPermissionGate` actually mutates (other gates no-op). Returns the
    /// gate's validation error string on an invalid / disallowed mode.
    pub async fn set_permission_mode(&self, mode: &str) -> Result<(), String> {
        self.perms.set_permission_mode(mode).await
    }

    /// Apply or clear the LIVE per-MCP-server permission-mode override
    /// (stream-json `set_mcp_permission_mode_override` control_request).
    pub async fn set_mcp_permission_mode_override(
        &self,
        server_name: &str,
        mode: Option<&str>,
    ) -> Result<(), String> {
        self.perms
            .set_mcp_permission_mode_override(server_name, mode)
            .await
    }

    /// Return the enforcing gate's live permission-mode wire id.
    #[must_use]
    pub fn permission_mode(&self) -> Option<String> {
        self.perms.permission_mode()
    }
}

const PROTECTED_PLANS_DIR_COMPONENTS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".bzr",
    ".jj",
    ".sl",
    ".claude",
    ".lingxi",
    ".cargo",
    "node_modules",
];

fn normalize_guard_component(component: &std::ffi::OsStr) -> Option<String> {
    let text = component
        .to_str()?
        .trim_end_matches(['.', ' '])
        .to_ascii_lowercase();
    (!text.is_empty()).then_some(text)
}

fn path_relative_components(
    root: &std::path::Path,
    candidate: &std::path::Path,
    case_insensitive: bool,
) -> Option<Vec<std::ffi::OsString>> {
    let mut candidate_components = candidate.components();
    for root_component in root.components() {
        let candidate_component = candidate_components.next()?;
        let equal = if case_insensitive {
            root_component
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&candidate_component.as_os_str().to_string_lossy())
        } else {
            root_component == candidate_component
        };
        if !equal {
            return None;
        }
    }
    Some(
        candidate_components
            .map(|component| component.as_os_str().to_os_string())
            .collect(),
    )
}

fn confined_path_components(
    root: &std::path::Path,
    candidate: &std::path::Path,
) -> Option<Vec<std::ffi::OsString>> {
    path_relative_components(root, candidate, cfg!(windows))
}

fn plans_dir_has_protected_component(root: &std::path::Path, candidate: &std::path::Path) -> bool {
    confined_path_components(root, candidate).is_none_or(|components| {
        components.iter().any(|name| {
            normalize_guard_component(name).is_some_and(|name| {
                PROTECTED_PLANS_DIR_COMPONENTS
                    .iter()
                    .any(|protected| name == *protected)
            })
        })
    })
}

fn deepest_existing_ancestor(path: &std::path::Path) -> Option<&std::path::Path> {
    path.ancestors().find(|ancestor| ancestor.exists())
}

fn metadata_is_link_like(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    false
}

fn has_symlink_component_between(
    root: &std::path::Path,
    existing: &std::path::Path,
) -> Option<bool> {
    let mut current = root.to_path_buf();
    for component in confined_path_components(root, existing)? {
        current.push(component);
        let meta = std::fs::symlink_metadata(&current).ok()?;
        if metadata_is_link_like(&meta) {
            return Some(true);
        }
    }
    Some(false)
}

fn nearest_repo_root(path: &std::path::Path) -> Option<std::path::PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .and_then(|ancestor| std::fs::canonicalize(ancestor).ok())
}

fn plans_dir_passes_hardening(
    project_root: &std::path::Path,
    normalized_root: &std::path::Path,
    normalized_candidate: &std::path::Path,
) -> bool {
    if plans_dir_has_protected_component(normalized_root, normalized_candidate) {
        return false;
    }

    let canonical_root = match std::fs::canonicalize(project_root) {
        Ok(path) => path,
        Err(_) => return false,
    };
    let existing = match deepest_existing_ancestor(normalized_candidate) {
        Some(path) => path,
        None => return false,
    };
    if !existing.is_dir() {
        return false;
    }
    if has_symlink_component_between(normalized_root, existing) != Some(false) {
        return false;
    }

    let canonical_existing = match std::fs::canonicalize(existing) {
        Ok(path) => path,
        Err(_) => return false,
    };
    if confined_path_components(&canonical_root, &canonical_existing).is_none() {
        return false;
    }

    let project_repo_root = nearest_repo_root(&canonical_root);
    let candidate_repo_root = nearest_repo_root(&canonical_existing);
    match (project_repo_root, candidate_repo_root) {
        (Some(project), Some(candidate)) => project == candidate,
        (None, None) => true,
        _ => false,
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

fn parse_generated_session_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let candidate = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|body| body.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let object = serde_json::from_str::<serde_json::Value>(candidate)
        .ok()
        .or_else(|| {
            let start = candidate.find('{')?;
            let end = candidate.rfind('}')?;
            serde_json::from_str(&candidate[start..=end]).ok()
        })?;
    object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
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
