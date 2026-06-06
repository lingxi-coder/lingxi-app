//! Conversation orchestrator.
//!
//! Drives the v0.6.0 batched turn loop. See module-level docs in `lib.rs`.

use crate::config::OrchestratorConfig;
use crate::error::OrchestratorError;
use crate::test_support::{HookExecutor, PermissionGate};
use crate::token_budget::{check_token_budget, BudgetTracker, TokenBudgetDecision};
use crate::turn_loop::{
    execute_one_turn, execute_one_turn_with_recovery_tracked, RecoveryState, TurnStepOutcome,
    MAX_OUTPUT_TOKENS_RECOVERY_LIMIT, MAX_OUTPUT_TOKENS_RECOVERY_NUDGE,
};
use api_client::{types::MessageResponse, AnthropicProvider, ApiError};
use async_trait::async_trait;
use engine::SessionState;
use hooks::events::HookEvent;
use hooks::registry::HookContext;
use protocol::{ConversationMessage, MessageId, SessionId};
use session::JsonlWriter;
use std::sync::Arc;
use telemetry::tengu::orchestrator as orch_events;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tool_api::registry::ToolRegistry;
use traits::{HttpTransport, OutputStream};

/// Minimal contract the orchestrator needs from the API client.
///
/// Production: `AnthropicProviderAdapter` wraps `AnthropicProvider` +
/// `HttpTransport` into this shape. Tests: `MockApiClient`.
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
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<MessageResponse, ApiError>;

    /// Non-streaming `messages.create` with the **Opus-fallback** policy wired
    /// (Opus-fallback batch). Identical to [`Self::messages_create`] except the
    /// caller hands in the configured `fallback_model` (+ the pre-computed
    /// subscription flags `is_subscriber` / `is_enterprise`) so the api-client
    /// can surface [`ApiError::FallbackTriggered`] after
    /// [`api_client::MAX_529_RETRIES`] consecutive 529s on a non-custom Opus
    /// primary model (1:1 with claude-code `withRetry.ts:326-365`).
    ///
    /// The DEFAULT body delegates to [`Self::messages_create`], dropping the
    /// fallback args — so every existing impl (mocks, the router adapter, the
    /// hook-prompt mock) compiles unchanged and behaves byte-identically. Only
    /// [`AnthropicProviderAdapter`] overrides it to thread the fallback into
    /// `AnthropicProvider::messages_create_non_stream_with_fallback`. The turn
    /// loop only calls THIS method when `config.fallback_model.is_some()`; with
    /// no fallback configured it stays on `messages_create`, a strict no-op.
    #[allow(clippy::too_many_arguments)]
    async fn messages_create_with_fallback(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        _fallback_model: Option<&str>,
        _is_subscriber: bool,
        _is_enterprise: bool,
    ) -> Result<MessageResponse, ApiError> {
        // Default: ignore the fallback args and use the plain seam. Keeps all
        // non-Anthropic impls (and mocks) byte-identical.
        self.messages_create(model, system, msgs, tools).await
    }

    /// Enumerate available `provider/model` ids + `@aliases` for `/model`'s
    /// list mode. Default returns empty so non-routing impls (mocks / the
    /// no-streaming stub) need no override; `ProviderApiAdapter` overrides it
    /// to delegate to the router.
    fn available_models(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Streaming-API surface used by the orchestrator's streaming turn loop.
///
/// Mirrors [`OrchestratorApiClient`] but returns a typed
/// `BoxStream<'static, Result<StreamEvent, ApiError>>` instead of a
/// single `MessageResponse`. The orchestrator owns the stream and drives
/// it to completion (or `message_stop`).
///
/// Production: `AnthropicProviderStreamingAdapter` (added in Task 11)
/// wraps `AnthropicProvider::messages_create_stream` + a transport.
/// Tests: `MockStreamingApiClient` in `test_support_stream.rs`.
#[async_trait]
pub trait StreamingApiClient: Send + Sync {
    /// Open a streaming `messages.create` request. The returned stream
    /// yields wire-decoded `StreamEvent` values until the server emits
    /// `message_stop`. The implementation is responsible for HTTP, SSE
    /// chunk buffering, and JSON-decoding the `data:` lines into typed
    /// `StreamEvent` values.
    async fn stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<api_client::types::StreamEvent, ApiError>>,
        ApiError,
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
    /// loop appends the carried messages as a meta user message, sets
    /// `stop_hook_active = true`, and runs one more turn step. The re-entry
    /// guard converts a *second* such block into [`Self::Pass`] so a hook that
    /// always blocks cannot loop forever (TS `query.ts:1297`).
    Continue(Vec<String>),
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
            should_exit: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cost_tracker: None,
            session_started_at: std::time::Instant::now(),
            api_calls_recorded: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            mcp_registry: None,
            hook_registry: None,
            agent_catalog: None,
            compaction: None,
            compaction_tracking: Mutex::new(compaction::AutoCompactTrackingState::default()),
            cache_safe_slot: None,
            read_file_state: Arc::new(Mutex::new(Vec::new())),
            read_state_map: tool_api::read_file_state::new_read_file_state_map(),
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

    /// Whether an MCP registry has been wired via
    /// [`Self::with_mcp_registry`]. (M6-07)
    #[must_use]
    pub fn has_mcp_registry(&self) -> bool {
        self.mcp_registry.is_some()
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
            .apply_post_compact(result, messages_before, bytes_before)
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
        messages_before: u32,
        bytes_before: u64,
    ) -> traits::CompactionSummary {
        let mut history_after = result.messages;
        // Append the boundary marker so the TUI scrollback and the next
        // turn's system-prompt assembly see the compaction transition.
        let n_after_summary = history_after.len();
        let marker = ConversationMessage::System {
            id: MessageId::new(),
            content: format!("[Compacted {messages_before} → {n_after_summary} messages]"),
        };
        history_after.push(marker.clone());

        let messages_after = u32::try_from(history_after.len()).unwrap_or(u32::MAX);
        let bytes_after: u64 = history_after.iter().map(protocol::text_byte_size).sum();
        let bytes_saved = bytes_before.saturating_sub(bytes_after);

        // Swap history under the same lock.
        {
            let mut s = self.session.lock().await;
            s.history = history_after;
        }

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

        self.apply_post_compact(result, messages_before, bytes_before)
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
                // (TS `query.ts:1332` `maxOutputTokensRecoveryCount: 0`).
                recovery.max_output_tokens_recovery_count = 0;
                recovery.max_output_tokens_override = None;
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
    pub(crate) fn to_jsonl_message(
        &self,
        msg: &ConversationMessage,
        session_id: &str,
        parent_uuid: Option<String>,
    ) -> session::JsonlMessage {
        let (kind, inner_message) = match msg {
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
        // Use the raw UUID (8-4-4-4-12 lowercase), NOT the `msg.id().to_string()`
        // form which carries the `"msg:"` prefix — that prefix would break the
        // byte-equivalent JSONL schema (see `JsonlMessage::uuid` doc) and the
        // `validate_uuid` regex.
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
            git_branch: None,
            extra: serde_json::Map::new(),
        }
    }

    /// Persist a single message to the optional JSONL writer.
    ///
    /// Best-effort: write failures are logged via the telemetry
    /// `tengu_session_corrupted` event but never fail the turn. On
    /// success, emits `tengu_session_appended` and updates the
    /// `last_jsonl_uuid` cache.
    pub(crate) async fn persist_message_to_jsonl(&self, msg: &ConversationMessage) {
        let Some(writer) = self.jsonl_writer.as_ref() else {
            return;
        };
        let (session_id_str, parent_uuid) = {
            let session_id = self.session.lock().await.session_id;
            let parent = self.last_jsonl_uuid.lock().await.clone();
            (session_id.to_string(), parent)
        };
        let jmsg = self.to_jsonl_message(msg, &session_id_str, parent_uuid);
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
            StopHookDisposition::Continue(agg.system_messages)
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
            StopHookDisposition::Continue(msgs) => {
                self.append_stop_hook_messages(&msgs).await;
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
    /// [`crate::prompt::MemoryHierarchyProvider`] loads exactly that top-level
    /// User/Project/Local hierarchy (no `@include` parents, no enterprise-`Managed`
    /// tier), so every file fired here is top-level ⇒ `load_reason = session_start`,
    /// with `memory_type` derived from the loaded file:
    ///
    /// - `is_local_override` ⇒ `Local` (a `CLAUDE.local.md`),
    /// - path under `~/.claude` ⇒ `User` (user-global `CLAUDE.md`),
    /// - otherwise ⇒ `Project` (a repo `CLAUDE.md`).
    ///
    /// `globs` / `trigger_file_path` / `parent_file_path` are omitted — the
    /// hierarchy provider carries no `paths:`-frontmatter, lazy-trigger, or
    /// `@include`-parent metadata (those wire fields are `.optional()` and elided
    /// when absent, matching the TS session-start fire).
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
        let home = dirs::home_dir();
        for file in memory_files {
            let memory_type = if file.is_local_override {
                hooks::events::InstructionsMemoryType::Local
            } else if home
                .as_ref()
                .is_some_and(|h| file.path.starts_with(h.join(".claude")))
            {
                hooks::events::InstructionsMemoryType::User
            } else {
                hooks::events::InstructionsMemoryType::Project
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
                        globs: None,
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

    /// Append a Stop hook's blocking messages as a meta user message so the
    /// model sees the hook feedback on the continued turn (TS appends the
    /// blocking reason). Best-effort persist, like the other meta appends.
    async fn append_stop_hook_messages(&self, messages: &[String]) {
        if messages.is_empty() {
            return;
        }
        let combined = messages.join("\n");
        let msg = ConversationMessage::user(MessageId::new(), combined);
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
            if turn_count >= self.config.max_turns {
                return Err(OrchestratorError::MaxTurnsReached {
                    max_turns: self.config.max_turns,
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
                        StopHookFlow::LoopAgain => continue,
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
        let result = self.try_run_turn_streaming(prompt, &[]).await;
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
        image_paths: &[std::path::PathBuf],
    ) -> Result<ConversationOutcome, OrchestratorError> {
        use crate::streaming_loop::{dispatch_tool_uses_concurrent, pump_stream};
        use protocol::ContentBlock;

        // 0. Build the system prompt for THIS turn. Override always wins.
        let system_prompt: Option<String> = match &self.config.system_prompt_override {
            Some(custom) => Some(custom.clone()),
            None => Some(self.build_system_prompt().await),
        };

        // 1. Append the user prompt (+ any pasted images) to session history.
        let images = Self::load_images(image_paths)?;
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
        loop {
            if turn_count >= self.config.max_turns {
                return Err(OrchestratorError::MaxTurnsReached {
                    max_turns: self.config.max_turns,
                });
            }
            turn_count = turn_count.saturating_add(1);

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
            let (snapshot, model) = {
                let s = self.session.lock().await;
                (s.history.clone(), s.model.clone())
            };
            let stream = self
                .streaming_api
                .stream(
                    &model,
                    system_prompt.as_deref(),
                    snapshot,
                    wire_tools.clone(),
                )
                .await
                .map_err(OrchestratorError::Streaming)?;

            // 3. Pump the stream.
            let pumped = pump_stream(stream, &self.output).await?;
            // A3: accumulate this turn's output tokens (TS `getTurnOutputTokens()`).
            global_turn_tokens = global_turn_tokens.saturating_add(pumped.output_tokens);

            // In-Loop Compaction Batch 6: snapshot the cache-safe prompt prefix
            // after a successful stream (streaming twin of the batched save).
            // `session.history` here equals the streamed snapshot — the streaming
            // path does not mutate history mid-call — taken before the assistant
            // reply is appended below. Strict no-op when no slot is wired.
            self.save_cache_safe_params(system_prompt.as_deref(), &model)
                .await;

            // 4. Assemble + append the assistant message.
            let assistant_id = MessageId::new();
            let mut blocks: Vec<ContentBlock> = pumped.assistant_blocks.clone();
            for t in &pumped.tool_uses {
                blocks.push(ContentBlock::ToolUse {
                    id: t.id,
                    name: t.name.clone(),
                    input: t.input.clone(),
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
            self.persist_message_to_jsonl(&assistant_msg).await;

            // 5. Dispatch tools concurrently (M5-04 Task 13). Each
            //    tool runs the same hook + permission + registry +
            //    hook pipeline as the batched path; futures::join_all
            //    polls them on the current task so I/O overlaps.
            //    ToolResult blocks come back in ORIGINAL stream order
            //    (sorted by the dispatch helper) so the assistant ↔
            //    user message correlation stays deterministic; the
            //    OutputStream events still fire in completion order.
            if !pumped.tool_uses.is_empty() {
                let results = dispatch_tool_uses_concurrent(self, &pumped.tool_uses).await?;
                let user_id = MessageId::new();
                let tool_results_msg = ConversationMessage::User {
                    id: user_id,
                    content: results,
                };
                {
                    let mut s = self.session.lock().await;
                    s.history.push(tool_results_msg.clone());
                }
                self.persist_message_to_jsonl(&tool_results_msg).await;
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
                        StopHookFlow::LoopAgain => continue,
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
                        StopHookFlow::LoopAgain => continue,
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
        self.try_run_turn_cancelable(prompt, cancel).await
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
            if turn_count >= self.config.max_turns {
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
    /// Race [`Self::try_run_turn_streaming`] against the `cancel` token:
    /// - natural completion (`ConversationOutcome::EndTurn`) → `TurnOutcome::EndTurn`.
    /// - `cancel.cancelled()` fires → `TurnOutcome::Cancelled` (the SSE
    ///   stream is dropped, which closes the HTTP request and flushes any
    ///   already-buffered `emit_text` calls to the output sink).
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
        tracing::info!(
            event = orch_events::TURN_STREAMING_STARTED,
            prompt_len = prompt.len()
        );
        if cancel.is_cancelled() {
            return Ok(TurnOutcome::Cancelled);
        }
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(TurnOutcome::Cancelled),
            r = self.try_run_turn_streaming(prompt, image_paths) => match r {
                Ok(ConversationOutcome::EndTurn { turn_count, .. }
                | ConversationOutcome::StopHookPrevented { turn_count, .. }) => {
                    tracing::info!(
                        event = orch_events::TURN_STREAMING_COMPLETED,
                        turn_count
                    );
                    Ok(TurnOutcome::EndTurn)
                }
                Err(OrchestratorError::MaxTurnsReached { .. }) => {
                    Ok(TurnOutcome::MaxTurns)
                }
                Err(e) => Err(e),
            },
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
        use crate::prompt::{assemble_system_prompt, file_tree, git_status, SystemPromptContext};

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
            model_marketing_name: None, // M5-12 CLI fills this when known.
            knowledge_cutoff: None,     // M5-12 CLI fills this when known.
            shell,
            os_version: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            git_status: git,
            file_tree: tree,
            memory_files,
            tool_names,
        };
        assemble_system_prompt(&ctx)
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
    /// `toolSchemaCache` analog yet) — `tools_to_wire` sorts by name so the
    /// bytes stay deterministic across turns despite the registry's `HashMap`
    /// MCP/plugin partitions. A session-level cache is a recommended follow-up.
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

/// Production adapter: wraps `AnthropicProvider` + an `HttpTransport` into
/// the `OrchestratorApiClient` shape.
///
/// Concrete type so callers can construct without knowing the transport
/// type parameter (the constructor takes `Arc<dyn OrchestratorApiClient>`).
pub struct AnthropicProviderAdapter<T: HttpTransport + Send + Sync + 'static> {
    provider: AnthropicProvider,
    transport: Arc<T>,
}

impl<T: HttpTransport + Send + Sync + 'static> AnthropicProviderAdapter<T> {
    /// Construct from an existing provider + transport.
    #[must_use]
    pub fn new(provider: AnthropicProvider, transport: Arc<T>) -> Self {
        Self {
            provider,
            transport,
        }
    }
}

#[async_trait]
impl<T: HttpTransport + Send + Sync + 'static> OrchestratorApiClient
    for AnthropicProviderAdapter<T>
{
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<MessageResponse, ApiError> {
        // Thread `tools` through `_with_opts`, keeping this legacy adapter's
        // historical `max_tokens = 4096` / no-temperature defaults.
        self.provider
            .messages_create_non_stream_with_opts(
                model,
                system,
                msgs,
                4096,
                tools,
                None,
                self.transport.as_ref(),
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn messages_create_with_fallback(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        fallback_model: Option<&str>,
        is_subscriber: bool,
        is_enterprise: bool,
    ) -> Result<MessageResponse, ApiError> {
        // Thread the configured fallback + subscription flags into the
        // fallback-aware provider seam. Same `max_tokens = 4096` / no-temperature
        // defaults as `messages_create` above; when `fallback_model` is `None`
        // this is byte-identical to `messages_create` (the consecutive-529 gate
        // stays closed). `is_subscriber` / `is_enterprise` are the documented
        // stub `false` until OAuth subscription resolution lands (matching the
        // api-client `messages_create_non_stream_with_fallback` ship-with-stub
        // note); the orchestrator does not yet resolve subscription state.
        self.provider
            .messages_create_non_stream_with_fallback(
                model,
                system,
                msgs,
                4096,
                tools,
                None,
                fallback_model.map(str::to_owned),
                is_subscriber,
                is_enterprise,
                self.transport.as_ref(),
            )
            .await
    }
}

/// Production adapter: wraps `AnthropicProvider` + an `HttpTransport`
/// into the `StreamingApiClient` shape.
///
/// Mirrors [`AnthropicProviderAdapter`] but for the streaming endpoint.
/// The provider is held in an `Arc` so the adapter can be cloned cheaply
/// when the caller wants to share one provider across both the batched
/// and streaming paths.
pub struct AnthropicProviderStreamingAdapter<T: HttpTransport + Send + Sync + 'static> {
    provider: Arc<AnthropicProvider>,
    transport: Arc<T>,
}

impl<T: HttpTransport + Send + Sync + 'static> AnthropicProviderStreamingAdapter<T> {
    /// Construct from an existing provider + transport.
    #[must_use]
    pub fn new(provider: Arc<AnthropicProvider>, transport: Arc<T>) -> Self {
        Self {
            provider,
            transport,
        }
    }
}

#[async_trait]
impl<T: HttpTransport + Send + Sync + 'static> StreamingApiClient
    for AnthropicProviderStreamingAdapter<T>
{
    async fn stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<api_client::types::StreamEvent, ApiError>>,
        ApiError,
    > {
        self.provider
            .messages_create_stream(model, system, messages, tools, self.transport.clone())
            .await
    }
}

/// Internal no-op streaming client used by [`ConversationOrchestrator::new`]
/// when the caller doesn't supply a streaming transport. Every call to
/// `stream` returns `ApiError::Http(HttpError::Connection("no streaming
/// client configured"))`. Wired in Task 12 when the legacy `new()`
/// constructor delegates to `new_with_streaming(..., NoStreamingApiClient,
/// ...)`.
#[allow(dead_code)]
pub(crate) struct NoStreamingApiClient;

#[async_trait]
impl StreamingApiClient for NoStreamingApiClient {
    async fn stream(
        &self,
        _model: &str,
        _system: Option<&str>,
        _messages: Vec<ConversationMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<api_client::types::StreamEvent, ApiError>>,
        ApiError,
    > {
        Err(ApiError::Http(traits::HttpError::Connection(
            "no streaming client configured".into(),
        )))
    }
}
