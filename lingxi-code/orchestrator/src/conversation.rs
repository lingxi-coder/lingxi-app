//! Conversation orchestrator.
//!
//! Drives the v0.6.0 batched turn loop. See module-level docs in `lib.rs`.

use crate::config::OrchestratorConfig;
use crate::error::OrchestratorError;
use crate::test_support::{HookExecutor, PermissionGate};
use crate::token_budget::{check_token_budget, BudgetTracker, TokenBudgetDecision};
use crate::turn_loop::{
    execute_one_turn, execute_one_turn_with_recovery_tracked, surface_prompt_too_long,
    RecoveryState, TurnStepOutcome, MAX_OUTPUT_TOKENS_RECOVERY_LIMIT,
    MAX_OUTPUT_TOKENS_RECOVERY_NUDGE,
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
        self.messages_create(model, profile, system, msgs, tools).await
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
        self.messages_create(model, profile, system, msgs, tools).await
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
        self.messages_create(model, profile, system, msgs, tools).await
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
    ) -> Result<
        futures::stream::BoxStream<'static, Result<LlmEvent, LlmError>>,
        LlmError,
    >;
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
    /// (TS `query.ts:1278`); the turn ends as `StopHookPrevented`.
    Prevent,
}

/// Driver control-flow directive produced by `handle_stop_at_end` (hooks B4) so
/// the three turn drivers (batched / streaming / cancelable) translate the Stop
/// disposition into their own loop mechanics uniformly.
enum StopHookFlow {
    /// Terminate the turn loop, returning this outcome (`emit_end_turn` already
    /// fired inside the helper).
    Terminate(ConversationOutcome),
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
    /// CLAUDE.md hierarchy provider (M5-03). The orchestrator calls
    /// `memory.load(&cwd).await` once per `run_turn` to gather the
    /// memory files spliced into the system prompt.
    pub(crate) memory: Arc<dyn crate::prompt::MemoryHierarchyProvider>,
    /// Working directory used as the root for the env + file-tree +
    /// git-status + memory probes inside `build_system_prompt`. M5-12
    /// CLI will plumb `--cwd`; until then, callers pass the platform
    /// caller's cwd here.
    pub(crate) cwd: std::path::PathBuf,
    /// Optional on-disk JSONL persistence (M5-07). `None` for in-memory
    /// tests; `Some` when the CLI binary wires `~/.claude/projects/.../<uuid>.jsonl`.
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
    /// Cost tracker wired by [`Self::with_cost_tracker`] (M6-06). `None`
    /// when not configured — `snapshot_cost` then falls back to the M5-10
    /// zero-shaped stub. The CLI binary (M6-06 init.rs) always populates
    /// this so production `lingxi-cli` reports real cost; library callers
    /// (e.g. unit tests) may leave it `None`.
    pub(crate) cost_tracker: Option<Arc<cost::CostTracker>>,
    /// Optional analytics bus wired by [`Self::with_analytics_bus`] (M7). When
    /// present (desktop composition root), the live turn loop fires
    /// `tengu_cost_recorded` per recorded API response — 1:1 with claude-code's
    /// `logEvent('tengu_cost_recorded', …)`. `None` for library/test callers,
    /// which then silently skip the emission (the tracker still accrues totals).
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
    /// `~/.claude/agents/` + project `.claude/agents/`.
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
    /// FORK (codex #5 follow-up): the rendered system-prompt bytes the current
    /// turn handed the model, recorded by the turn driver after a successful API
    /// call so a fork-subagent spawn dispatched LATER in the same turn can thread
    /// the exact bytes onto its child (cache-identical prefix, claude
    /// `AgentTool.tsx:622-623` `override.systemPrompt = forkParentSystemPrompt`).
    /// `None` until the first successful turn / when the turn ran with no system
    /// prompt. Read by the fork dispatch path ONLY; no non-fork tool touches it.
    pub(crate) current_turn_system_prompt: Mutex<Option<String>>,
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
    /// here (and to the future staleness guards / Read dedup). Behavior-neutral
    /// for now: the map is populated but nothing reads it yet.
    ///
    /// Intentionally write-only THIS batch (file-tools-remainder Batch B): the
    /// staleness guards (D/E/F) and Read dedup (A) are the first consumers, and
    /// the composition root shares this `Arc` into the file tools'
    /// `BuiltinToolContext`. Allow `dead_code` until those land.
    #[allow(dead_code)]
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
    /// §F: cache of the CONDITIONAL (`paths:`-gated) memory rules, populated the
    /// first time [`Self::conditional_rules_reminder_message`] runs (a `OnceCell`
    /// fill via the same `memory.load(&cwd)` the system prompt uses, then
    /// re-filtered to `globs.is_some()`). Avoids re-walking disk every turn while
    /// still letting lazy activation re-test the cached rules against the latest
    /// `read_file_state`. Empty when the hierarchy has no conditional rules.
    pub(crate) conditional_rules_cache:
        tokio::sync::OnceCell<Vec<crate::prompt::MemoryFile>>,
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
    /// Only consulted when the gate (`CLAUDE_CODE_AGENT_LIST_IN_MESSAGES`) is ON;
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
    /// P0.1 per-turn slot holding the in-flight prefetch handle armed by
    /// [`Self::start_memory_prefetch`] at turn start and consumed by
    /// [`Self::relevant_memory_reminder_message`] before snapshot assembly.
    /// `None` between turns / when no prefetch is wired. Mirrors the
    /// pending-handle slot pattern of the recovery / cache-safe slots.
    pub(crate) pending_memory_prefetch:
        Mutex<Option<memory::prefetch::PendingMemoryPrefetch>>,
    /// P0.1 surfacing dedup: paths already surfaced via the
    /// `relevant_memories` channel this session, so a memory surfaced once is
    /// never re-injected on a later turn. Mirrors [`Self::sent_conditional_rules`]
    /// (TS `loadedNestedMemoryPaths` / the prefetch's per-iteration consume
    /// guard). Distinct from [`Self::read_file_state`], which the SHARED dedup
    /// also consults so a file already loaded as a nested/conditional attachment
    /// (P3.2) is never double-injected here.
    pub(crate) surfaced_memory_paths: Mutex<std::collections::HashSet<std::path::PathBuf>>,
}

impl ConversationOrchestrator {
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
        let session = SessionState::empty(SessionId::new(), config.model.clone());
        Self {
            config,
            api,
            streaming_api,
            tools,
            hooks,
            perms,
            output,
            session: Arc::new(Mutex::new(session)),
            memory,
            cwd,
            jsonl_writer: None,
            last_jsonl_uuid: Mutex::new(None),
            git_branch_cache: Mutex::new(None),
            current_prompt_id: Mutex::new(None),
            should_exit: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cost_tracker: None,
            analytics_bus: None,
            session_started_at: std::time::Instant::now(),
            api_calls_recorded: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            mcp_registry: None,
            hook_registry: None,
            agent_catalog: None,
            compaction: None,
            compaction_tracking: Mutex::new(compaction::AutoCompactTrackingState::default()),
            cache_safe_slot: None,
            current_turn_system_prompt: Mutex::new(None),
            read_file_state: Arc::new(Mutex::new(Vec::new())),
            read_state_map: tool_api::read_file_state::new_read_file_state_map(),
            last_emitted_rate_limit: Mutex::new(None),
            last_emitted_raw_utilization: Mutex::new(None),
            skill_listing: None,
            async_hook_responses: None,
            task_notifications: None,
            conditional_rules_cache: tokio::sync::OnceCell::new(),
            sent_conditional_rules: Mutex::new(std::collections::HashSet::new()),
            sent_skill_names: Mutex::new(std::collections::HashSet::new()),
            sent_agent_names: Mutex::new(std::collections::HashSet::new()),
            memory_prefetch: None,
            pending_memory_prefetch: Mutex::new(None),
            surfaced_memory_paths: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Attach a [`JsonlWriter`] for byte-equivalent session persistence.
    /// Builder-style — used by the CLI binary (M5-12) and integration tests.
    #[must_use]
    pub fn with_jsonl_writer(mut self, writer: Arc<JsonlWriter>) -> Self {
        self.jsonl_writer = Some(writer);
        self
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
    /// `tengu_cost_recorded` per recorded API response (M7). Without this the
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
    pub fn with_memory_prefetch(
        mut self,
        prefetch: Arc<memory::prefetch::MemoryPrefetch>,
    ) -> Self {
        self.memory_prefetch = Some(prefetch);
        self
    }

    /// Whether a memory prefetcher has been wired via
    /// [`Self::with_memory_prefetch`]. (P0.1)
    #[must_use]
    pub fn has_memory_prefetch(&self) -> bool {
        self.memory_prefetch.is_some()
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
        self.mcp_registry
            .as_ref()
            .is_some_and(|r| r.has_oauth())
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
        // `manual` (TS `isAutoCompact ? 'auto' : 'manual'`). Best-effort — a
        // hook failure/Block never aborts compaction.
        self.fire_pre_compact("manual").await;

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
        self.fire_post_compact("manual", summary, tokens_freed).await;

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
    pub(crate) async fn apply_post_compact(
        &self,
        result: compaction::IterationCompactionResult,
        trigger: compaction::CompactTrigger,
        messages_before: u32,
        bytes_before: u64,
    ) -> traits::CompactionSummary {
        // CSM.4: build the TS-faithful compact boundary (`createCompactBoundaryMessage`,
        // the byte-exact `"Conversation compacted"` sentinel) instead of the ad-hoc
        // `[Compacted N → M]` marker, so the TUI scrollback + next-turn system-prompt
        // assembly see the same boundary TS emits. The rich `CompactBoundaryMetadata`
        // has no orchestrator-side consumer yet (no sidecar store / no
        // `get_messages_after_compact_boundary` caller), so it is discarded here; a
        // follow-up that persists it can swap `_metadata` for a real store.
        let (marker, _metadata) =
            compaction::create_compact_boundary(trigger, 0, None, None, None, &[]);
        // COMPACT.1: the boundary marker leads the post-compact history, matching
        // TS `buildPostCompactMessages` order `[boundaryMarker, ...summaryMessages,
        // ...messagesToKeep, ...]` (compact.ts:330). Prepending (not appending) the
        // marker is what lets a `get_messages_after_compact_boundary` consumer treat
        // the summary + kept messages as the content AFTER the boundary, mirroring
        // TS `getMessagesAfterCompactBoundary`.
        let mut history_after = Vec::with_capacity(result.messages.len() + 1);
        history_after.push(marker.clone());
        history_after.extend(result.messages);

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
    pub(crate) async fn maybe_compact_before_call(&self) {
        let Some(compactor) = self.compaction.clone() else {
            // No compactor wired — strict no-op (history untouched).
            return;
        };

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

        let messages_before = u32::try_from(snapshot.len()).unwrap_or(u32::MAX);
        let bytes_before: u64 = snapshot.iter().map(protocol::text_byte_size).sum();

        // hooks compaction lifecycle: PreCompact fires once we have crossed the
        // autocompact threshold and are about to run the summary pass (TS
        // `executePreCompactHooks` BEFORE the summary request, `compact.ts:413`).
        // The proactive trigger is always the `auto` arm. Best-effort — a hook
        // failure/Block never aborts compaction.
        self.fire_pre_compact("auto").await;

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
        let (mut input_tokens, mut output_tokens) = (0u64, 0u64);
        for entry in state.per_model_usage.values() {
            input_tokens = input_tokens.saturating_add(entry.usage.tokens.input);
            output_tokens = output_tokens.saturating_add(entry.usage.tokens.output);
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
        let decision = check_token_budget(
            tracker,
            None,
            self.config.token_budget,
            global_turn_tokens,
        );
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
    /// first turn). `cwd` is taken from `self.cwd`. The `message` payload
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
            msg, session_id, parent_uuid, git_branch, entrypoint, prompt_id, None,
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
    ) -> session::JsonlMessage {
        let (kind, mut inner_message) = match msg {
            ConversationMessage::User { content, .. } => (
                "user",
                serde_json::json!({ "role": "user", "content": content }),
            ),
            ConversationMessage::Assistant { content, .. } => (
                "assistant",
                serde_json::json!({ "role": "assistant", "content": content }),
            ),
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
        session::JsonlMessage {
            message_type: kind.to_string(),
            uuid: msg.id().as_uuid().to_string(),
            parent_uuid,
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
            cwd: self.cwd.to_string_lossy().into_owned(),
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
        let jmsg =
            self.to_jsonl_message(msg, &session_id_str, parent_uuid, git_branch, entrypoint, prompt_id);
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

        let session_id_str = self.session.lock().await.session_id.to_string();
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

    /// Task 6 (llm-client future-work batch 5): re-map a terminal
    /// `RateLimited` error onto the limits-specific copy the API client
    /// composed from the 429's own unified headers (claude-code
    /// `errors.ts:480-524`). Applied by every public turn driver so each
    /// consumer of the error's `Display` (CLI stderr, TUI scrollback,
    /// `client-adapter` `ClientEvent::Error`) sees the
    /// `"You've hit your … limit · resets …"` copy instead of the generic
    /// `"api call failed: rate limited"`. No-op for non-429 errors and when
    /// the 429 carried no unified headers.
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
    async fn emit_terminal_rate_limit_if_changed<T>(
        &self,
        result: &Result<T, OrchestratorError>,
    ) {
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
            Ok(ConversationOutcome::EndTurn { turn_count, .. }
            | ConversationOutcome::StopHookPrevented { turn_count, .. }) => {
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
        let session_id = { self.session.lock().await.session_id };
        HookContext {
            session_id,
            cwd: self.cwd.clone(),
            stop_hook_active,
            ..Default::default()
        }
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
            .execute(HookEvent::UserPromptSubmit { prompt: prompt.to_string() }, ctx)
            .await;
        matches!(agg.decision, Some(hooks::response::HookDecision::Block))
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
        let ctx = self.lifecycle_hook_ctx(stop_hook_active).await;
        let agg = self
            .hooks
            .execute(HookEvent::Stop { reason: reason.to_string() }, ctx)
            .await;
        let disposition = if agg.prevent_continuation {
            StopHookDisposition::Prevent
        } else if matches!(agg.decision, Some(hooks::response::HookDecision::Block))
            && !stop_hook_active
        {
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
            .execute(HookEvent::StopFailure { error: error.to_string() }, ctx)
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
        turn_count: u32,
        final_message_id: MessageId,
    ) -> StopHookFlow {
        if stop_reason == "prompt_too_long" {
            // RECOV.2: this turn ended on an API error — the model never produced
            // a real response, so fire the `StopFailure` hooks (NOT the `Stop`
            // hooks) before ending. 1:1 with TS `query.ts:1262-1264` (the
            // api-error skip guard runs `executeStopFailureHooks(lastMessage)` then
            // returns) and `query.ts:1174/1181` (PTL recovery exhausted). Running
            // the normal `Stop` hooks here would risk the death-spiral TS warns
            // against (error → hook blocking → retry → error → …). The wire
            // `error` is `"invalid_request"`, matching TS
            // `createAssistantAPIErrorMessage({ …, error: 'invalid_request' })`
            // (`query.ts:642-644`).
            self.fire_stop_failure("invalid_request").await;
            return StopHookFlow::FallThrough;
        }
        match self.fire_stop_hooks(stop_reason, *stop_hook_active).await {
            StopHookDisposition::Prevent => {
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn(stop_reason, &cost).await;
                StopHookFlow::Terminate(ConversationOutcome::StopHookPrevented {
                    turn_count,
                    final_message_id,
                })
            }
            StopHookDisposition::Continue(reason) => {
                self.append_stop_hook_feedback(&reason).await;
                *stop_hook_active = true;
                StopHookFlow::LoopAgain
            }
            StopHookDisposition::Pass => StopHookFlow::FallThrough,
        }
    }

    /// Fire the `PreCompact` lifecycle hooks immediately BEFORE a compaction
    /// pass runs (hooks compaction lifecycle, TS `executePreCompactHooks` called
    /// from `services/compact/compact.ts:413` BEFORE the summary request).
    ///
    /// `trigger` is the `auto` / `manual` discriminator carried verbatim into
    /// the wire payload's `trigger` field (TS `compactData.trigger`): `auto` for
    /// the proactive pre-call autocompact and the reactive 413/PTL fallback,
    /// `manual` for an explicit `/compact`. Best-effort: a hook failure (or a
    /// hook returning a `Block` decision) must NEVER abort compaction — we fire
    /// and continue, mirroring how the `PostToolUse` hooks are best-effort
    /// (`turn_loop.rs`). Strict no-op when no `PreCompact` hook is registered.
    ///
    /// DEFERRED (documented divergence, not a parity gap): TS
    /// `executePreCompactHooks` returns `newCustomInstructions` which the caller
    /// merges into the summary prompt (`compact.ts:420`). This port does NOT
    /// thread that back into the summarizer: the `HookEvent::PreCompact` wire
    /// builder hard-codes `custom_instructions: None` (`hooks/executor.rs:755`)
    /// and the compaction seam (`process_iteration` / `process_iteration_tracked`)
    /// accepts no custom-instruction argument, so there is no clean seam to feed
    /// the aggregate's instructions into the summary request. Firing the hook so
    /// it RUNS is the byte-faithful behaviour for the event itself; consuming its
    /// returned instructions is left for a future batch that widens the seam.
    pub(crate) async fn fire_pre_compact(&self, trigger: &str) {
        let ctx = self.lifecycle_hook_ctx(false).await;
        // Best-effort: we deliberately discard the aggregate. A PreCompact hook
        // cannot block compaction (see DEFERRED note re: custom_instructions).
        let _ = self
            .hooks
            .execute(HookEvent::PreCompact { reason: trigger.to_string() }, ctx)
            .await;
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

    pub(crate) async fn fire_post_compact(&self, trigger: &str, summary: String, tokens_freed: u64) {
        // `trigger` is part of the TS PostCompact `matchQuery` but the
        // `HookEvent::PostCompact` wire builder emits an empty `trigger` field
        // (`hooks/executor.rs:768`); accepted here for call-site symmetry with
        // `fire_pre_compact` and forward-compatibility if the payload widens.
        let _ = trigger;
        let ctx = self.lifecycle_hook_ctx(false).await;
        let _ = self
            .hooks
            .execute(HookEvent::PostCompact { summary, tokens_freed }, ctx)
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
        let _ = self
            .hooks
            .execute(
                HookEvent::SessionStart {
                    session_id,
                    source: source.to_string(),
                },
                ctx,
            )
            .await;
    }

    /// Fire the `InstructionsLoaded` hooks once per loaded instruction file at
    /// session startup (hooks lifecycle, TS `executeInstructionsLoadedHooks`
    /// dispatched from the eager `getMemoryFiles` pass — `utils/claudemd.ts:1054-1071`,
    /// `utils/hooks.ts:4335-4369`).
    ///
    /// claude-code fires this fire-and-forget hook for **each** CLAUDE.md /
    /// `CLAUDE.local.md` it splices into context, carrying that file's `file_path`,
    /// `memory_type` (`User` / `Project` / `Local` / `Managed`), and `load_reason`.
    /// The eager session-start pass reports `load_reason: 'session_start'` for every
    /// top-level (parent-less) file (`eagerLoadReason`). The orchestrator's
    /// [`crate::prompt::MemoryHierarchyProvider`] loads the full Managed/User/
    /// Project/Local hierarchy and tags each file with its
    /// [`memory::claude_md::ClaudeMdTier`]; `memory_type` is taken directly from
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
                memory::claude_md::ClaudeMdTier::Managed => {
                    hooks::events::InstructionsMemoryType::Managed
                }
                memory::claude_md::ClaudeMdTier::User => {
                    hooks::events::InstructionsMemoryType::User
                }
                memory::claude_md::ClaudeMdTier::Project => {
                    hooks::events::InstructionsMemoryType::Project
                }
                memory::claude_md::ClaudeMdTier::Local => {
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
        let _ = self
            .hooks
            .execute(
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
        // A3: token-budget continuation bookkeeping. `Some` only when the gate
        // is enabled AND a budget is set; otherwise the budget check is a
        // NO-OP and the loop stops at the first `end_turn` (parity default).
        let mut budget = self.new_budget_tracker();
        let mut global_turn_tokens: u64 = 0;
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
                        .handle_stop_at_end(&stop_reason, &mut stop_hook_active, turn_count, id)
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
            Ok(ConversationOutcome::EndTurn { turn_count, .. }
            | ConversationOutcome::StopHookPrevented { turn_count, .. }) => {
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
        use crate::streaming_loop::{pump_stream_with_executor, ExecutorPump};
        use protocol::ContentBlock;

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
        // A3: token-budget continuation bookkeeping (streaming twin). `Some`
        // only when the gate is enabled AND a budget is set; otherwise the
        // budget check is a NO-OP and the loop stops at the first `end_turn`.
        let mut budget = self.new_budget_tracker();
        let mut global_turn_tokens: u64 = 0;
        let mut turn_count: u32 = 0;
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
            if user_cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn("aborted_streaming", &cost).await;
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
            // reminder — path-gated CLAUDE.md rules that newly activate because a
            // touched file matches their globs. Appended to THIS turn's OUTGOING
            // snapshot only (never `session.history` / JSONL), after the
            // skill-listing reminder and BEFORE the blocking-limit estimate below
            // so its tokens are counted in the prompt size. `None` when no
            // provider / no conditional rules / nothing newly active. See
            // [`Self::conditional_rules_reminder_message`].
            if let Some(reminder) = self.conditional_rules_reminder_message().await {
                snapshot.push(reminder);
            }

            // `agent_listing_delta` (streaming twin): per-turn, transient agent
            // catalog reminder, emitted ONLY when the
            // `CLAUDE_CODE_AGENT_LIST_IN_MESSAGES` gate is ON (default OFF ⇒
            // `None`, keeping the locked streaming fixtures byte-identical and
            // the inline catalog in place). Appended to THIS turn's OUTGOING
            // snapshot only (never `session.history` / JSONL). See
            // [`Self::agent_listing_reminder_message`].
            if let Some(reminder) = self.agent_listing_reminder_message().await {
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
                // RECOV.2 chokepoint: `handle_stop_at_end` fires the `StopFailure`
                // hooks for this `"prompt_too_long"` api-error end; its guard always
                // returns `FallThrough` for that reason (it short-circuits before
                // the `Stop` hooks), so the directive is discarded and the normal
                // end-of-turn tail runs — exactly mirroring the batched path.
                let _ = self
                    .handle_stop_at_end(
                        "prompt_too_long",
                        &mut stop_hook_active,
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
            let mut exec = match &user_cancel {
                Some(token) => crate::streaming_executor::StreamingToolExecutor::new_with_user_cancel(
                    self,
                    token.clone(),
                ),
                None => crate::streaming_executor::StreamingToolExecutor::new(self),
            };

            let stream = self
                .streaming_api
                .stream(
                    &model,
                    model_profile.as_deref(),
                    system_prompt.as_deref(),
                    snapshot,
                    wire_tools.clone(),
                )
                .await
                .map_err(OrchestratorError::Streaming)?;

            // 3. Pump the stream (with mid-stream 529 → non-streaming fallback).
            //
            // Task 7 / claude.ts parity: if the stream errors with `LlmError::Overloaded`
            // after the first event — AND `CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK` is not
            // set — discard the partial accumulation and issue a fresh non-streaming call
            // seeded with `initial_consecutive_overloaded = 1`.  This mirrors
            // `claude.ts:2469-2594` + `withRetry.ts:186` (`initialConsecutive529Errors`).
            //
            // The env gate name is locked byte-for-byte to TS:
            //   `process.env.CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK` (claude.ts:2470)
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
            let pumped = match pump_stream_with_executor(
                stream,
                &self.output,
                ExecutorPump {
                    executor: &mut exec,
                    assistant_id,
                    user_cancel: user_cancel.as_ref(),
                },
            )
            .await
            {
                Ok(p) => p,
                Err(OrchestratorError::Streaming(ref e @ (LlmError::Overloaded { .. } | LlmError::ProviderInternal)))
                    if !is_env_truthy(
                        std::env::var("CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK")
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
                    let (non_stream_snapshot, non_stream_model, non_stream_profile) = {
                        let s = self.session.lock().await;
                        (s.history.clone(), s.model.clone(), s.model_profile.clone())
                    };
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
                Err(other) => return Err(other),
            };
            // A3: accumulate this turn's output tokens (TS `getTurnOutputTokens()`).
            global_turn_tokens = global_turn_tokens.saturating_add(pumped.output_tokens);

            // BILLING: record streaming-turn usage into CostTracker — mirrors the
            // non-streaming path in `turn_loop.rs:375-393`. Uses the same
            // `record_api_response_v2` function + arg semantics: `Duration::ZERO`
            // (adapter doesn't surface per-call wall-clock) and `retries = 0`
            // (retries are swallowed internally by the adapter, same as batch path).
            if let Some(tracker) = self.cost_tracker.as_ref() {
                if let Some(ref usage) = pumped.usage {
                    let cost_usage = crate::cost_wiring::llm_usage_to_cost_usage(usage);
                    let cache_read = usage.billable_tokens.cache_read;
                    let cache_create = usage.billable_tokens.cache_write;
                    let model_ref = crate::cost_wiring::model_ref_from_string(&model);
                    let _cost = tracker
                        .record_api_response_v2(
                            model_ref,
                            cost_usage,
                            std::time::Duration::ZERO,
                            0,    // retries — not yet exposed from the adapter
                            cache_read,
                            cache_create,
                            false, // is_batch_request — streaming is never batch
                            self.analytics_bus.as_ref(), // M7: fire tengu_cost_recorded
                        )
                        .await;
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
            let tool_use_parent_uuids =
                self.persist_assistant_per_block(&assistant_msg).await;
            // Fallback parent (the LAST persisted block's uuid) for any
            // tool_result whose tool_use id is missing from the map (defensive).
            let assistant_uuid = self.last_jsonl_uuid.lock().await.clone();

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

            // DEFERRED-3 / esc-interrupt FIX: "we were aborted during tool calls"
            // (faithful port of claude-code `query.ts:1485` — the `aborted_tools`
            // return). Once the user-interrupt token has fired, the executor above
            // already drained the bare REJECT_MESSAGE `tool_result`s into history
            // (model-visible). The turn MUST now STOP — claude-code returns
            // `aborted_tools` with NO further `callModel`, honoring REJECT_MESSAGE's
            // "STOP what you are doing and wait for the user". Looping into the
            // `Some("tool_use") => continue` arm below would (1) issue a wasted
            // extra round-trip after every ESC-during-tools and (2) let a
            // Block-behavior tool emitted on that continuation actually EXECUTE
            // (`abort_reason_for` returns `None` for Block tools) despite the
            // interrupt — both of which claude-code structurally prevents by
            // returning here first. `None` token (plain `run_turn_streaming`) →
            // never fires → identical to before.
            if user_cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
                let cost = self.snapshot_cost_real().await;
                self.output.emit_end_turn("aborted_tools", &cost).await;
                final_message_id = assistant_id;
                break;
            }

            // 6. Decide loop disposition.
            match pumped.stop_reason.as_deref() {
                Some("end_turn") => {
                    // hooks B4: Stop hooks BEFORE the budget check (streaming
                    // twin; order recovery → stop-hooks → token-budget).
                    match self
                        .handle_stop_at_end(
                            "end_turn",
                            &mut stop_hook_active,
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
                Some("tool_use") if !pumped.tool_uses.is_empty() => continue,
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
                    recovery.max_output_tokens_recovery_count = recovery
                        .max_output_tokens_recovery_count
                        .saturating_add(1);
                    recovery.max_output_tokens_override = None;
                    continue;
                }
                Some(other) => {
                    // max_tokens (recovery exhausted) / stop_sequence /
                    // pause_turn / refusal — terminate the loop with the value
                    // as-is, mirroring claude-code's behavior (claude.ts:2269).
                    let cost = self.snapshot_cost_real().await;
                    self.output.emit_end_turn(other, &cost).await;
                    final_message_id = assistant_id;
                    break;
                }
                None => {
                    // Stream ended without a stop_reason — treat as
                    // end_turn (rare; claude.ts uses the same fallback). The
                    // token-budget check applies here too (A3).
                    // hooks B4: Stop hooks before the budget check (same as the
                    // explicit end_turn arm).
                    match self
                        .handle_stop_at_end(
                            "end_turn",
                            &mut stop_hook_active,
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
        let mut turn_count: u32 = 0;
        // hooks B4: Stop-hook re-entry guard (cancelable twin).
        let mut stop_hook_active = false;
        loop {
            if cancel.is_cancelled() {
                return Ok(TurnOutcome::Cancelled);
            }
            if self.config.max_turns != 0 && turn_count >= self.config.max_turns {
                return Ok(TurnOutcome::MaxTurns);
            }
            turn_count = turn_count.saturating_add(1);

            // Race the API call against the cancellation token.
            let step = tokio::select! {
                r = execute_one_turn(self, system_prompt.as_deref()) => r?,
                () = cancel.cancelled() => return Ok(TurnOutcome::Cancelled),
            };
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
                        .handle_stop_at_end(&stop_reason, &mut stop_hook_active, turn_count, id)
                        .await
                    {
                        StopHookFlow::Terminate(_) => return Ok(TurnOutcome::EndTurn),
                        StopHookFlow::LoopAgain => continue,
                        StopHookFlow::FallThrough => {}
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
            Ok(ConversationOutcome::EndTurn { turn_count, .. }
            | ConversationOutcome::StopHookPrevented { turn_count, .. }) => {
                tracing::info!(event = orch_events::TURN_STREAMING_COMPLETED, turn_count);
                if cancel.is_cancelled() {
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

    /// Build the per-turn system prompt by gathering cwd / git / file
    /// tree / memory / tool-name context and calling
    /// [`crate::prompt::assemble_system_prompt`]. Bypassed when
    /// `OrchestratorConfig::system_prompt_override` is `Some(_)`.
    async fn build_system_prompt(&self) -> String {
        use crate::prompt::{
            assemble_system_prompt_with_style, file_tree, git_status, ActiveOutputStyle,
            SystemPromptContext,
        };

        let cwd = self.cwd.clone();
        let memory_files = self.memory.load(&cwd).await;

        let git = git_status::probe(&cwd);
        let tree = file_tree::probe(&cwd, file_tree::DEFAULT_DEPTH_LIMIT);

        // Tool name extraction: ToolRegistry's `all_names()` is the
        // unfiltered set (builtin + plugin + MCP). M5-03 uses the
        // unfiltered list because the registry's enable-filter requires
        // a `ToolStaticContext` that's only meaningful at dispatch time.
        // tools_block::format sorts alphabetically inside.
        let tool_names: Vec<String> = self.tools.all_names();

        let shell = std::env::var("SHELL")
            .ok()
            .and_then(|s| {
                std::path::Path::new(&s)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "sh".into());

        let ctx = SystemPromptContext {
            cwd,
            platform: std::env::consts::OS.to_string(),
            model: self.config.model.clone(),
            // SYSPROMPT.1: port TS getMarketingNameForModel / getKnowledgeCutoff
            // (`utils/model/model.ts:570`, `constants/prompts.ts:712`) so the
            // model line + cutoff sentence match claude-code instead of being
            // stubbed to None.
            model_marketing_name: crate::prompt::env_meta::marketing_name_for_model(
                &self.config.model,
            )
            .map(String::from),
            knowledge_cutoff: crate::prompt::env_meta::knowledge_cutoff_for_model(&self.config.model)
                .map(String::from),
            shell,
            // SYSPROMPT.1: `uname -sr` (TS getUnameSR) e.g. "Darwin 25.3.0",
            // falling back to "<os> <arch>" on Windows / spawn failure.
            os_version: crate::prompt::env_meta::os_version_string(),
            git_status: git,
            file_tree: tree,
            memory_files,
            tool_names,
        };
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
        });
        assemble_system_prompt_with_style(&ctx, style)
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
        // skills, where the budgeter degrades gracefully.
        let window =
            compaction::context_window::context_window_for_model(&self.config.model, &[]) as usize;
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

    /// The per-turn, transient `agent_listing_delta` reminder, or `None` when
    /// the gate is OFF (the default — keeps the inline-catalog build
    /// byte-identical), no agent catalog is wired, the `Agent` tool is absent
    /// this turn, or no NEW agent type has appeared since the last reminder.
    ///
    /// 1:1 with claude-code's `agent_listing_delta` attachment
    /// (`getAgentListingDeltaAttachment`, attachments.ts:1490-1554 →
    /// `normalizeAttachmentForAPI`'s `'agent_listing_delta'` case,
    /// messages.ts:4194-4215):
    /// - GATE: `shouldInjectAgentListInMessages()` (env
    ///   `CLAUDE_CODE_AGENT_LIST_IN_MESSAGES`, default OFF — see
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
        // Require a wired catalog (DISK agents; built-ins are merged below).
        let catalog = self.agent_catalog.as_ref()?;
        // Gate on the Agent tool being available this turn (attachments.ts:1497).
        // `find_by_name` also matches the legacy `Task` alias.
        if self.tools.find_by_name("Agent").is_none() {
            return None;
        }

        // Merge BUILT-INS first, then the wired catalog (DISK agents only) on
        // top — `agent_catalog` does NOT include built-ins, so we prepend them
        // here. Later-wins precedence means a same-named catalog agent overrides
        // a built-in, matching the inline `AgentTool` prompt's
        // `PoolSubagentSpawner::listing_entries` (built-in < user/project) and TS
        // (`activeAgents` already includes built-ins via `getAgents`).
        let mut defs = agent::builtin_agent_definitions();
        defs.extend(catalog.read().await.iter().cloned());
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
    /// CLAUDE.md rules (`paths:`-globbed) that newly ACTIVATE because a file the
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
        let tools = self.tools.available_tools(&ToolStaticContext::default());
        tool_api::wire::tools_to_wire(
            &tools,
            &PromptOptions {
                include_examples: true,
            },
        )
        .await
    }

    /// Borrow the in-memory session (read-write lock surrogate). Useful for tests.
    #[must_use]
    pub fn session(&self) -> Arc<Mutex<SessionState>> {
        self.session.clone()
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
    }
}

/// Mirror TS `isEnvTruthy` (`utils/envUtils.ts:32`): a value is truthy ONLY
/// when, lowercased and trimmed, it is one of the whitelist members
/// `"1"`, `"true"`, `"yes"`, `"on"`. Absent, empty, and every other value
/// (including `"no"`, `"off"`, `"2"`, `"enabled"`, …) are falsy.
///
/// Locked against the TS helper used at `claude.ts:2470`:
/// `isEnvTruthy(process.env.CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK)`.
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
    ) -> Result<
        futures::stream::BoxStream<'static, Result<LlmEvent, LlmError>>,
        LlmError,
    > {
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
    use llm_client::ContentBlock as LlmContentBlock;
    use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
    use hooks::events::HookEventType;
    use hooks::executor::BuiltinHookHandler;
    use hooks::registry::HookRegistry;
    use hooks::response::{HookDecision, HookOutcome, HookResponse, HookResult};
    use hooks::HookExecutorImpl;
    use protocol::{HookId, HttpRequest, HttpResponse};
    use std::pin::Pin;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;
    use tokio::sync::RwLock;
    use traits::{HttpError, HttpTransport, OutputEvent, RuntimeError, RuntimeSpawner};

    // ---- unused HTTP / Runtime stubs (Builtin hooks never touch them) ----
    struct UnusedHttp;
    #[async_trait]
    impl HttpTransport for UnusedHttp {
        async fn request(&self, _r: HttpRequest) -> Result<HttpResponse, HttpError> {
            Err(HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(
            &self,
            _r: HttpRequest,
        ) -> Result<traits::http::SseStream, HttpError> {
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
        async fn cancel(
            &self,
            _h: &traits::BackgroundTaskHandle,
        ) -> Result<(), RuntimeError> {
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
                    self.log.lock().unwrap().push(format!("StopFailure:{error}"));
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

    /// Seed a history far past the hard blocking limit. The default model
    /// (`claude-opus-4-7`, 200k window) blocks around ~177k tokens; 2M chars ≈
    /// 500k tokens (estimator is chars/4), comfortably over.
    async fn seed_over_blocking_limit(orch: &ConversationOrchestrator) {
        let session = orch.session();
        let mut s = session.lock().await;
        s.history.push(ConversationMessage::user(
            MessageId::new(),
            "x".repeat(2_000_000),
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
        assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }), "{outcome:?}");

        // The stream was NEVER opened — the preempt short-circuited the API call.
        assert!(
            streaming.captured_calls().await.is_empty(),
            "the blocking-limit preempt must NOT open the stream"
        );

        // The byte-exact prompt-too-long message + an EndTurn("prompt_too_long").
        let events = output.snapshot().await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OutputEvent::Text { text } if text == "Prompt is too long")),
            "byte-exact prompt-too-long message must be surfaced; events={events:#?}"
        );
        assert!(
            events.iter().any(
                |e| matches!(e, OutputEvent::EndTurn { stop_reason, .. } if stop_reason == "prompt_too_long")
            ),
            "the turn must end with stop_reason prompt_too_long; events={events:#?}"
        );
    }

    // -------- RECOV.2 — StopFailure fires on an api-error turn-end --------

    #[tokio::test]
    async fn recov2_stop_failure_fires_on_api_error_end_and_stop_does_not() {
        // A history over the blocking limit ⇒ the batched preempt surfaces
        // prompt_too_long, which is an api-error end. `StopFailure` must fire
        // (error == "invalid_request"); the normal `Stop` hooks must NOT.
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
                vec![LlmContentBlock::Text { text: "done".into(), cache_control: None }],
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
            exec_blocking_stop().await,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );

        let outcome = orch.run_turn("go").await.expect("turn ok");
        assert!(matches!(outcome, ConversationOutcome::EndTurn { .. }), "{outcome:?}");
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
            text_of(&orch.output_style_reminder_message().expect("Learning resolves")),
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
        let fs: Arc<dyn traits::FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(dir.path().to_path_buf()));
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

        // OUTGOING snapshot: [user(prompt), reminder] — reminder is the trailing
        // meta user message (TS position).
        let outgoing = api.captured_msgs().await;
        assert_eq!(outgoing.len(), 1, "exactly one batched API call");
        let sent = &outgoing[0];
        assert_eq!(sent.len(), 2, "user prompt + reminder; got {sent:?}");
        assert_eq!(text_of(&sent[0]), "user prompt body");
        assert!(
            is_reminder(&sent[1], EXPLANATORY_REMINDER),
            "trailing message must be the byte-exact reminder; got {:?}",
            sent[1]
        );

        // STORED history: [user(prompt), assistant] — the reminder was NOT pushed.
        let history = orch.session.lock().await.history.clone();
        assert_eq!(history.len(), 2, "user + assistant only; got {history:?}");
        assert!(
            history.iter().all(|m| !is_reminder(m, EXPLANATORY_REMINDER)),
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
        // Byte-identical to the styleless path: the outgoing list is the prompt
        // alone — no extra message.
        assert_eq!(outgoing[0].len(), 1, "no reminder; got {:?}", outgoing[0]);
        assert_eq!(text_of(&outgoing[0][0]), "just the prompt");
    }

    // ----- streaming driver (`run_turn_streaming`) -----

    #[tokio::test]
    async fn streaming_active_style_appends_transient_reminder_not_persisted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let fs: Arc<dyn traits::FileSystem> =
            Arc::new(platform_posix::fs::PosixFileSystem::new(dir.path().to_path_buf()));
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

        // OUTGOING snapshot to the stream: [user(prompt), reminder].
        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 1, "exactly one streaming call");
        let sent = &calls[0].messages;
        assert_eq!(sent.len(), 2, "user prompt + reminder; got {sent:?}");
        assert_eq!(text_of(&sent[0]), "streaming prompt");
        assert!(
            is_reminder(&sent[1], LEARNING_REMINDER),
            "trailing message must be the byte-exact Learning reminder; got {:?}",
            sent[1]
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
        // Byte-identical to the styleless path: the prompt alone, no extra message.
        assert_eq!(
            calls[0].messages.len(),
            1,
            "no reminder; got {:?}",
            calls[0].messages
        );
        assert_eq!(text_of(&calls[0].messages[0]), "only prompt");
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
        content_block_start_text, content_block_start_tool_use, content_block_stop, input_json_delta,
        message_delta_stop, message_start, message_stop, mock_message_response, noop_hook_executor,
        text_delta, MockApiClient, MockOutputStream, MockStreamingApiClient, NoOpPermissionGate,
        StaticMemoryProvider,
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
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| serde_json::json!({ "type": "object", "properties": {} }));
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
                new_messages: vec![],
                context_modifier: Some(modifier),
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
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| serde_json::json!({ "type": "object", "properties": {} }));
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
                new_messages: vec![],
                context_modifier: None,
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
            calls.iter().all(|c| c.model == crate::config::DEFAULT_MODEL),
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
// Env gate: `CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK` (claude.ts:2470)
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

    const DISABLE_FALLBACK_ENV: &str = "CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK";

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
        assert_eq!(stream_calls.len(), 1, "(a) stream must be called exactly once");

        // (b) A fresh non-streaming messages_create_seeded was called.
        let seeds = api.captured_seeds().await;
        assert_eq!(seeds.len(), 1, "(b) messages_create_seeded must be called exactly once");

        // (c) The seed is 1 (the streaming 529 counts toward the consecutive 529 budget).
        assert_eq!(seeds[0], 1, "(c) seed must be 1 for a streaming Overloaded error");

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

    /// Task 7 Step 1 (twin with CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK=1):
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
        assert!(result.is_err(), "error must propagate when fallback is disabled");
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
        assert!(matches!(api, OrchestratorError::ApiCall(LlmError::RateLimited { .. })));
        let stream =
            enrich_rate_limited_error(OrchestratorError::Streaming(rate_limited()), None);
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
                new_messages: vec![],
                context_modifier: None,
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
        assert!(t1.contains("- gamma:"), "turn-1 must contain the new skill: {t1}");
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

    // ── T35: `task-notification` reminder folds in then drains once ──────────

    /// A [`TaskNotificationProvider`] that hands back its fixture exactly once
    /// (the second drain returns empty), mirroring the registry's
    /// take-mark-evict semantics so the consume-once invariant is testable
    /// without a real registry.
    struct OnceTaskNotifications(
        std::sync::Mutex<Vec<traits::task_registry::TaskNotification>>,
    );
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
// - GATE ON (`CLAUDE_CODE_AGENT_LIST_IN_MESSAGES=1`, guarded by a process-wide
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
    use agent::{
        AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
    };
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// `CLAUDE_CODE_AGENT_LIST_IN_MESSAGES` is process-global; serialize the
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
                new_messages: vec![],
                context_modifier: None,
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
    async fn gate_off_default_is_none() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");

        // Even with the Agent tool + a catalog wired, the default gate is OFF.
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "general-purpose",
            "anything",
            AgentToolPolicy::All { use_exact_tools: false },
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog));
        assert!(
            orch.agent_listing_reminder_message().await.is_none(),
            "gate OFF (default) must yield no reminder"
        );
    }

    #[tokio::test]
    async fn gate_on_but_no_catalog_is_none() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES", "1");
        let orch = orch_with(reg_with_agent_tool(), None);
        let got = orch.agent_listing_reminder_message().await;
        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");
        assert!(got.is_none(), "no catalog ⇒ no reminder even when gate ON");
    }

    #[tokio::test]
    async fn gate_on_but_agent_tool_absent_is_none() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES", "1");
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "general-purpose",
            "anything",
            AgentToolPolicy::All { use_exact_tools: false },
        )]));
        // Empty registry — the Agent tool is not present this turn.
        let orch = orch_with(ToolRegistry::new(), Some(catalog));
        let got = orch.agent_listing_reminder_message().await;
        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");
        assert!(got.is_none(), "Agent tool absent ⇒ no reminder");
    }

    #[tokio::test]
    async fn gate_on_turn0_full_listing_with_initial_header() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES", "1");
        // Catalog supplies a custom type; built-ins are merged in too.
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "custom-agent",
            "a project agent",
            AgentToolPolicy::Explicit(vec!["Read".into()]),
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog));

        let msg = orch.agent_listing_reminder_message().await;
        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");
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
        std::env::set_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES", "1");
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "custom-agent",
            "a project agent",
            AgentToolPolicy::All { use_exact_tools: false },
        )]));
        let orch = orch_with(reg_with_agent_tool(), Some(catalog));

        // Turn 0 emits the full listing.
        let t0 = orch.agent_listing_reminder_message().await;
        assert!(t0.is_some(), "turn-0 must emit");
        // Turn 1 with the same catalog ⇒ nothing new ⇒ None.
        let t1 = orch.agent_listing_reminder_message().await;
        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");
        assert!(t1.is_none(), "no new types ⇒ no reminder");
    }

    #[tokio::test]
    async fn gate_on_newly_added_type_emits_delta_with_new_header_only() {
        let _g = AGENT_LIST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES", "1");
        let catalog = Arc::new(tokio::sync::RwLock::new(vec![agent_def(
            "alpha-agent",
            "the alpha agent",
            AgentToolPolicy::All { use_exact_tools: false },
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
        std::env::remove_var("CLAUDE_CODE_AGENT_LIST_IN_MESSAGES");

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
mod conditional_rules_reminder_tests {
    use super::*;
    use crate::prompt::MemoryFile;
    use crate::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use crate::OrchestratorConfig;
    use memory::claude_md::ClaudeMdTier;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::registry::ToolRegistry;

    /// A Project-tier conditional rule living at `<cwd>/.claude/rules/{name}.md`
    /// (so its derived base dir is `<cwd>`) carrying the given `paths:` globs.
    fn project_rule(cwd: &std::path::Path, name: &str, globs: &[&str]) -> MemoryFile {
        MemoryFile {
            path: cwd.join(".claude").join("rules").join(format!("{name}.md")),
            body: format!("BODY OF {name}"),
            is_local_override: false,
            tier: ClaudeMdTier::Project,
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
            text.contains("Contents of /work/repo/.claude/rules/scoped.md:"),
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
            path: cwd.join("CLAUDE.md"),
            body: "always".into(),
            is_local_override: false,
            tier: ClaudeMdTier::Project,
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
            text.contains("Retrieved for possible relevance \u{2014} use only if it actually applies"),
            "idx-0 preamble missing: {text}"
        );
        assert!(text.contains("Memory: /m/a.md:\n\nUSE FD NOT FIND"), "got: {text}");
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
        assert!(!text.contains("/m/seen.md"), "already-read memory leaked: {text}");
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
        let fs: Arc<dyn traits::FileSystem> =
            Arc::new(PosixFileSystem::new(dir.to_path_buf()));
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
        assert_eq!(lines_after_asst.len(), 1, "expected 1 line (the assistant message)");
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
        let user_msg =
            ConversationMessage::user(protocol::MessageId::new(), "user prompt".into());
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
        orch.persist_message_to_jsonl_with_parent(
            &tool_result_msg,
            Some(user_uuid.clone()),
        )
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

        let msg1 =
            ConversationMessage::user(protocol::MessageId::new(), "first message".into());
        orch.persist_message_to_jsonl_with_parent(&msg1, None).await;

        let msg2 =
            ConversationMessage::user(protocol::MessageId::new(), "second message".into());
        orch.persist_message_to_jsonl_with_parent(&msg2, None).await;

        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 2, "expected 2 JSONL lines");
        // First entry: root of chain → parent_uuid is None.
        assert_eq!(lines[0].parent_uuid, None, "first entry must have no parent");
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
        let msg1 =
            ConversationMessage::user(protocol::MessageId::new(), "root".into());
        orch.persist_message_to_jsonl(&msg1).await;
        let lines = read_jsonl(&session_path);
        let root_uuid = lines[0].uuid.clone();

        // 2. Overridden message pointing back to root — simulates a tool result.
        let msg2 =
            ConversationMessage::user(protocol::MessageId::new(), "overridden".into());
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
        let msg3 =
            ConversationMessage::user(protocol::MessageId::new(), "subsequent".into());
        orch.persist_message_to_jsonl(&msg3).await;
        let lines = read_jsonl(&session_path);
        assert_eq!(lines.len(), 3, "expected 3 JSONL lines");
        assert_eq!(
            lines[2].parent_uuid.as_deref(),
            Some(overridden_uuid.as_str()),
            "subsequent non-overridden line must chain off the overridden line"
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

        let map = orch.persist_assistant_per_block(&assistant_msg).await;

        let lines = read_jsonl(&session_path);
        // (c) THREE single-block assistant lines.
        let asst_lines: Vec<&JsonlMessage> =
            lines.iter().filter(|l| l.message_type == "assistant").collect();
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
        assert_eq!(uuids.len(), 3, "the three lines must have distinct top-level uuids");

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
