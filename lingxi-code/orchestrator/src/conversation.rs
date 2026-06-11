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
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        _max_tokens: u32,
    ) -> Result<LlmResponse, LlmError> {
        self.messages_create(model, system, msgs, tools).await
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
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        _fallback_model: Option<&str>,
        _is_subscriber: bool,
        _is_enterprise: bool,
    ) -> Result<LlmResponse, LlmError> {
        // Default: ignore the fallback args and use the plain seam. Keeps all
        // non-Anthropic impls (and mocks) byte-identical.
        self.messages_create(model, system, msgs, tools).await
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
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
        _initial_consecutive_overloaded: u8,
    ) -> Result<LlmResponse, LlmError> {
        // Default: ignore the seed and use the plain seam. Keeps all
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
    async fn stream(
        &self,
        model: &str,
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
        let result = self.try_run_turn_streaming(prompt, Vec::new()).await;
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
    ) -> Result<ConversationOutcome, OrchestratorError> {
        use crate::streaming_loop::{dispatch_tool_uses_concurrent, pump_stream};
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
            let (mut snapshot, model) = {
                let s = self.session.lock().await;
                (s.history.clone(), s.model.clone())
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
            let pumped = match pump_stream(stream, &self.output).await {
                Ok(p) => p,
                Err(OrchestratorError::Streaming(ref e @ (LlmError::Overloaded | LlmError::ProviderInternal)))
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
                    let seed: u8 = u8::from(matches!(e, LlmError::Overloaded));

                    // Re-snapshot history for the non-streaming call (the partial
                    // stream never touched session.history, so it is still the same
                    // snapshot we used for the stream — no reset needed).
                    let (non_stream_snapshot, non_stream_model) = {
                        let s = self.session.lock().await;
                        (s.history.clone(), s.model.clone())
                    };
                    let tools_for_fallback = wire_tools.clone();

                    let resp = self
                        .api
                        .messages_create_seeded(
                            &non_stream_model,
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

                    pumped_from_fallback
                }
                Err(other) => return Err(other),
            };
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
                let (results, injected_messages, context_modifiers) =
                    dispatch_tool_uses_concurrent(self, &pumped.tool_uses).await?;
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
                // SKILLEXEC.3 (streaming): replay any tool-injected `new_messages`
                // (the Skill tool's expanded prompt) into history right after the
                // tool_result, mirroring the batched turn loop. Empty for every
                // non-skill tool → a strict no-op (history/JSONL byte-identical).
                //
                // Also record each injected message's id → originating
                // tool_use_id into the in-memory `injected_message_sources`
                // side-table (faithful port of TS `sourceToolUseID`;
                // `#[serde(skip)]` so it never reaches the JSONL wire). The
                // tool_use_id is deliberately NOT passed to
                // `persist_message_to_jsonl` — TS does not persist
                // `sourceToolUseID`, so the JSONL bytes stay byte-identical.
                for (m, tool_use_id) in &injected_messages {
                    {
                        let mut s = self.session.lock().await;
                        s.history.push(m.clone());
                        s.injected_message_sources.insert(m.id(), *tool_use_id);
                    }
                    self.persist_message_to_jsonl(m).await;
                }
                // SKILLEXEC.3 (model scope, streaming twin): fold this batch's
                // `context_modifier`s and switch `session.model` if a skill
                // declared a `model:` override. Applied AFTER `injected_messages`
                // and POST-BATCH (the concurrent dispatch already joined), so
                // there is no race on `session.model` between concurrent tools.
                // Empty for every non-`model:` tool → strict no-op (session.model
                // untouched → byte-identical streaming fixtures).
                crate::turn_loop::apply_model_context_modifiers(self, context_modifiers).await;
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
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(TurnOutcome::Cancelled),
            r = self.try_run_turn_streaming(prompt, images) => match r {
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
        // OUTSTYLE.2: when a non-default output style is active, inject its
        // `# Output Style: <name>` section (TS getOutputStyleSection). A
        // `None`/`"default"`/unknown style resolves to `None`, leaving the
        // prompt byte-identical to the styleless path.
        let builtin = outputstyles::resolve_builtin_output_style(self.config.output_style.as_deref());
        let style = builtin.map(|b| ActiveOutputStyle {
            name: b.name,
            prompt: b.prompt,
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
        let builtin =
            outputstyles::resolve_builtin_output_style(self.config.output_style.as_deref())?;
        let content = format!(
            "<system-reminder>\n{} output style is active. \
             Remember to follow the specific guidelines for this style.\n</system-reminder>",
            builtin.name
        );
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
            ContentBlock::ToolUse { id, ref name, ref input } => {
                tool_uses.push(ObservedToolUse {
                    id,
                    name: name.clone(),
                    input: input.clone(),
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
    }
}

/// Mirror TS `isEnvTruthy` (utils/env.ts): a value is truthy when it is present,
/// non-empty, and not equal to `"false"` or `"0"`.
///
/// Locked against the TS helper used at `claude.ts:2470`:
/// `isEnvTruthy(process.env.CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK)`.
fn is_env_truthy(val: Option<&str>) -> bool {
    match val {
        None | Some("" | "false" | "0") => false,
        Some(_) => true,
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
                id: tu.as_uuid().to_string(),
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
                id: tu.as_uuid().to_string(),
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
            content_block_start_tool_use(0, tu, "ModelSwitch"),
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
            content_block_start_tool_use(0, tu, "Plain"),
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
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Build a one-ContentBlockStart-then-Err(Overloaded) stream: the first
    /// event is yielded successfully (proving partial events arrived), then the
    /// stream errors with `LlmError::Overloaded`.
    fn one_event_then_overloaded() -> Vec<Result<llm_client::LlmEvent, llm_client::LlmError>> {
        vec![
            Ok(message_start("m1", "claude-opus-4-7")),
            Ok(content_block_start_text(0)),
            Ok(text_delta(0, "partial")),
            Err(llm_client::LlmError::Overloaded),
        ]
    }

    /// Build an end_turn non-streaming response for the fallback.
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
        // `set_var`/`remove_var` are not thread-safe; the mutex makes the pair race-free
        // without introducing a new crate dependency (serial_test or similar).
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        use traits::OutputEvent;
        let texts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                OutputEvent::Text { text } => Some(text.as_str()),
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
        // `set_var`/`remove_var` are not thread-safe; the mutex makes the pair race-free
        // without introducing a new crate dependency (serial_test or similar).
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    /// `is_env_truthy` covers the exact semantics of TS `isEnvTruthy`:
    /// absent / empty / "false" / "0" → not truthy; anything else → truthy.
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
        // Non-empty, non-false, non-zero → truthy.
        assert!(is_env_truthy(Some("1")));
        assert!(is_env_truthy(Some("true")));
        assert!(is_env_truthy(Some("yes")));
    }
}
