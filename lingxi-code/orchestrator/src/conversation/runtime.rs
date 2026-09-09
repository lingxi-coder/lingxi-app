//! Cohesive session-memory runtime state and bounded persistence helpers.

use super::*;

/// Transcript persistence and the side tables consumed while serializing a turn.
pub(crate) struct TranscriptStore {
    /// Per-session provider recovery ownership, replaced when switching sessions.
    pub(crate) thinking_recovery:
        std::sync::RwLock<llm_client::thinking_scope::ThinkingRecoveryScope>,
    /// Optional on-disk JSONL persistence (M5-07). `None` for in-memory
    /// tests; `Some` when the CLI binary wires `~/.lingxi/projects/.../<uuid>.jsonl`.
    pub(crate) jsonl_writer: Option<Arc<JsonlWriter>>,
    /// Cached UUID of the last persisted JSONL entry — used to populate
    /// `parentUuid` on the next append. Reset to `None` for fresh sessions.
    pub(crate) last_jsonl_uuid: Arc<Mutex<Option<String>>>,
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
    /// `tool_use_id` → a successful tool result's turn-end request, consumed by
    /// the turn drivers only after the matching `tool_result` has been
    /// persisted and its post-result hooks/attachments have run.
    pub(crate) pending_tool_result_turn_end:
        Mutex<std::collections::HashMap<String, tool_api::tool_trait::ToolResultTurnEnd>>,
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
    /// Skill contents carried by each post-compact `invoked_skills` message,
    /// keyed by the message's stable identity.
    ///
    /// A skill body is arbitrary Markdown and may legally contain the renderer's
    /// `\n\n---\n\n` separator. Keeping the structured association here avoids
    /// reverse-parsing model-visible text when a later compaction deduplicates
    /// attachments that survived in its preserved tail.
    pub(crate) post_compact_skill_attachments:
        std::sync::Mutex<std::collections::HashMap<MessageId, Vec<String>>>,
}

impl TranscriptStore {
    pub(crate) fn new() -> Self {
        Self {
            thinking_recovery: std::sync::RwLock::new(Default::default()),
            jsonl_writer: None,
            last_jsonl_uuid: Arc::new(Mutex::new(None)),
            tool_denial_kinds: Mutex::new(std::collections::HashMap::new()),
            tool_frames: Mutex::new(None),
            tool_use_results: Mutex::new(std::collections::HashMap::new()),
            tool_use_mcp_meta: Mutex::new(std::collections::HashMap::new()),
            pending_tool_result_turn_end: Mutex::new(std::collections::HashMap::new()),
            tool_source_assistant_uuids: Mutex::new(std::collections::HashMap::new()),
            pending_hook_attachments: Mutex::new(std::collections::HashMap::new()),
            git_branch_cache: Mutex::new(None),
            post_compact_skill_attachments: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub(crate) async fn reset_session_scoped(&self) {
        *self
            .thinking_recovery
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Default::default();
        self.tool_denial_kinds.lock().await.clear();
        *self.tool_frames.lock().await = None;
        self.tool_use_results.lock().await.clear();
        self.tool_use_mcp_meta.lock().await.clear();
        self.pending_tool_result_turn_end.lock().await.clear();
        self.tool_source_assistant_uuids.lock().await.clear();
        self.pending_hook_attachments.lock().await.clear();
        *self.git_branch_cache.lock().await = None;
        self.post_compact_skill_attachments
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

/// State owned by system-prompt assembly and per-turn reminder pipelines.
pub(crate) struct PromptRuntime {
    /// Frozen gitStatus probe + rendered attachment for this conversation.
    /// Claude documents gitStatus as a start-of-conversation snapshot that does
    /// not update, including after a worktree/session-cwd swap.
    pub(crate) git_status_snapshot: Mutex<Option<GitStatusSnapshot>>,
    /// Stable per-prompt id for the IN-FLIGHT turn — the parity analog of TS
    /// `getPromptId()` (`sessionStorage.ts:1045-1046`), which stamps the same id
    /// on the user prompt line AND every `tool_result` `user` line of that turn.
    /// [`Self::persist_message_to_jsonl`] mints a fresh UUID when it persists a
    /// genuine new user prompt (a `user` message that is NOT a `tool_result`
    /// carrier) and reuses it for the turn's `tool_result` `user` lines; non-`user`
    /// lines never read it. `None` until the first user prompt is persisted.
    pub(crate) current_prompt_id: Mutex<Option<String>>,
    /// Live plugin output-style registry. Disk/builtin styles remain sourced
    /// from [`OrchestratorConfig`]; this optional registry makes plugin reloads
    /// visible to prompt assembly without rebuilding the orchestrator.
    pub(crate) output_style_registry:
        Option<Arc<tokio::sync::RwLock<outputstyles::OutputStyleRegistry>>>,
    /// Session-level cache for the expensive prompt/schema serialization of
    /// the post-filter tool pool. Dynamic per-turn marks (`strict`,
    /// `defer_loading`) are applied to a clone after cache lookup.
    pub(super) wire_tool_schema_cache: Mutex<Option<WireToolSchemaCache>>,
    /// Exact deferred-schema token counts, memoized by the same stable schema
    /// key as `wire_tool_schema_cache`. `None` is cached too: unsupported
    /// providers must not retry a doomed count endpoint every turn.
    pub(super) deferred_tool_token_cache:
        Mutex<std::collections::HashMap<WireToolSchemaCacheKey, Option<u64>>>,
    /// `history.len()` captured at each `silent_turn_reminder` emission.
    ///
    /// The oracle keeps its emitted attachments IN the message list, so `Ezm`
    /// (@296525002) counts `remindersInStretch` by walking them. LingXi's
    /// per-turn reminders are transient (outgoing snapshot only, never
    /// `session.history` / JSONL), so the emission POSITIONS are recorded here
    /// and spliced back into the walk by
    /// [`crate::prompt::silent_turn::scan_silent_stretch`].
    pub(crate) silent_turn_reminder_marks: std::sync::Mutex<Vec<usize>>,
    /// REM-14 `pendingMemoryUpdates` — the oracle's app-state queue
    /// (`jzm(e)` @296554545: `let t=e.getAppState().pendingMemoryUpdates;
    /// if(t.length===0)return[]; e.setAppState(…clear…)`), drained
    /// consume-once by [`Self::memory_update_reminder_messages`].
    ///
    /// Filled by [`Self::task_notification_reminder_messages`] when it drains a
    /// TERMINAL `dream` (background memory consolidation) task — the port's
    /// dream handler is a forked subagent whose completion only surfaces through
    /// the task registry, so that drain is the one place the signal exists.
    /// Capped at [`Self::MAX_PENDING_MEMORY_UPDATES`], oldest dropped: the
    /// BATCHED turn driver (`turn_loop.rs`) calls the notification drain but not
    /// the memory-update drain, so on that driver the queue must not grow.
    pub(crate) pending_memory_updates:
        std::sync::Mutex<Vec<crate::prompt::memory_update::PendingMemoryUpdate>>,
    /// REM-10: `history.len()` captured at each `tool_search_usage_reminder`
    /// emission. The oracle finds the previous reminder as an ATTACHMENT row in
    /// the message list (`R3T` @296552671); LingXi's per-turn reminders never
    /// enter `session.history`, so the emission points are recorded here — the
    /// same technique [`Self::silent_turn_reminder_marks`] uses.
    pub(crate) tool_search_reminder_marks: std::sync::Mutex<Vec<usize>>,
    /// Floor-truncated wall clock (ms) of the last memory-directory scan — the
    /// lower bound for "which memdir files did the consolidation rewrite".
    /// Seeded at construction so the first dream only reports files it touched.
    pub(crate) last_memory_scan_ms: std::sync::atomic::AtomicI64,
    /// Passive `<new-diagnostics>` source (the LSP diagnostic registry). When
    /// wired (via [`Self::with_new_diagnostics_source`]), each turn polls it for
    /// LSP diagnostics not yet surfaced and injects them as a transient meta
    /// user message (claude-code's `formatDiagnosticsBlock` flow). `None` when
    /// no LSP servers are configured (the common case) ⇒ no reminder.
    pub(crate) new_diagnostics_source: Option<Arc<dyn platform_api::NewDiagnosticsSource>>,
    /// FORK (codex #5 follow-up): the rendered system-prompt bytes the current
    /// turn handed the model, recorded by the turn driver after a successful API
    /// call so a fork-subagent spawn dispatched LATER in the same turn can thread
    /// the exact bytes onto its child (cache-identical prefix, claude
    /// `AgentTool.tsx:622-623` `override.systemPrompt = forkParentSystemPrompt`).
    /// `None` until the first successful turn / when the turn ran with no system
    /// prompt. Read by the fork dispatch path ONLY; no non-fork tool touches it.
    pub(crate) current_turn_system_prompt: Mutex<Option<String>>,
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
    /// `None` ⇒ [`Self::task_notification_reminder_messages`] is a strict no-op (the
    /// default — keeps fixtures byte-identical). Wired at the desktop composition
    /// root from the `TaskRegistry`.
    pub(crate) task_notifications:
        Option<Arc<dyn crate::prompt::task_notification::TaskNotificationProvider>>,
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
    /// `plan_mode` attachment cadence (2.1.238 `X4T` @296525982 + `txl`
    /// @296558044). See [`PlanReminderCadence`].
    pub(crate) plan_reminder_cadence: Mutex<PlanReminderCadence>,
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
    /// User-approved app Agent Profile applied additively to the next turn.
    pub(crate) app_agent_prompt_profile: std::sync::RwLock<Option<AppAgentPromptProfile>>,
    /// Optional carved-slate snapshot of the static prompt and inline tool
    /// descriptions. Dynamic system context remains live per request.
    pub(crate) prompt_snapshot: Mutex<Option<platform_api::PromptSnapshot>>,
    /// True while the mounted session came from resume. Missing snapshots on
    /// resumed sessions must remain missing rather than being created by the
    /// next turn.
    pub(crate) prompt_snapshot_resume: std::sync::atomic::AtomicBool,
}

impl PromptRuntime {
    pub(crate) fn new() -> Self {
        Self {
            git_status_snapshot: Mutex::new(None),
            current_prompt_id: Mutex::new(None),
            output_style_registry: None,
            wire_tool_schema_cache: Mutex::new(None),
            deferred_tool_token_cache: Mutex::new(std::collections::HashMap::new()),
            silent_turn_reminder_marks: std::sync::Mutex::new(Vec::new()),
            pending_memory_updates: std::sync::Mutex::new(Vec::new()),
            tool_search_reminder_marks: std::sync::Mutex::new(Vec::new()),
            last_memory_scan_ms: std::sync::atomic::AtomicI64::new(
                tool_api::read_file_state::mtime_ms_floor(std::time::SystemTime::now()),
            ),
            new_diagnostics_source: None,
            current_turn_system_prompt: Mutex::new(None),
            read_state_map: tool_api::read_file_state::new_read_file_state_map(),
            skill_listing: None,
            async_hook_responses: None,
            task_notifications: None,
            todo_reminder_tasks: None,
            conditional_rules_cache: Arc::new(std::sync::Mutex::new(None)),
            sent_conditional_rules: Mutex::new(std::collections::HashSet::new()),
            sent_nested_memory: Mutex::new(std::collections::HashSet::new()),
            nested_memory_roots: None,
            sent_skill_names: Mutex::new(std::collections::HashSet::new()),
            plan_reminder_cadence: Mutex::new(PlanReminderCadence::default()),
            date_change: std::sync::Mutex::new(DateChangeState::default()),
            sent_agent_names: Mutex::new(std::collections::HashSet::new()),
            memory_prefetch: None,
            pending_memory_prefetch: Mutex::new(None),
            surfaced_memory_paths: Mutex::new(std::collections::HashSet::new()),
            skill_discovery_prefetch: None,
            pending_skill_prefetch: Mutex::new(None),
            surfaced_skill_names: Mutex::new(std::collections::HashSet::new()),
            app_agent_prompt_profile: std::sync::RwLock::new(None),
            prompt_snapshot: Mutex::new(None),
            prompt_snapshot_resume: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Clear the prompt-owned read registry at its original session-reset stage.
    pub(crate) fn reset_read_state(&self) {
        self.read_state_map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .for_each(drop);
    }

    /// Clear every prompt-owned value whose lifetime is one conversation.
    pub(crate) async fn reset_session_scoped(&self) {
        *self.git_status_snapshot.lock().await = None;
        *self.current_prompt_id.lock().await = None;
        self.silent_turn_reminder_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.pending_memory_updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.tool_search_reminder_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.last_memory_scan_ms.store(
            tool_api::read_file_state::mtime_ms_floor(std::time::SystemTime::now()),
            std::sync::atomic::Ordering::Relaxed,
        );
        *self
            .conditional_rules_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.sent_conditional_rules.lock().await.clear();
        self.sent_nested_memory.lock().await.clear();
        self.sent_skill_names.lock().await.clear();
        self.sent_agent_names.lock().await.clear();
        self.surfaced_memory_paths.lock().await.clear();
        self.surfaced_skill_names.lock().await.clear();
        *self.plan_reminder_cadence.lock().await = PlanReminderCadence::default();
        *self
            .date_change
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = DateChangeState::default();
        *self.current_turn_system_prompt.lock().await = None;
        *self.pending_memory_prefetch.lock().await = None;
        *self.pending_skill_prefetch.lock().await = None;
        *self.prompt_snapshot.lock().await = None;
        self.prompt_snapshot_resume
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod prompt_runtime_tests {
    use super::*;

    #[tokio::test]
    async fn session_reset_clears_every_prompt_owned_conversation_state() {
        let runtime = PromptRuntime::new();
        *runtime.git_status_snapshot.lock().await = Some(GitStatusSnapshot {
            probe: None,
            block: Some("old git status".to_string()),
        });
        *runtime.current_prompt_id.lock().await = Some("old-prompt".to_string());
        runtime
            .silent_turn_reminder_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(3);
        runtime
            .pending_memory_updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(crate::prompt::memory_update::PendingMemoryUpdate {
                source: crate::prompt::memory_update::MemoryUpdateSource::Dream,
                summary: "old memory update".to_string(),
            });
        runtime
            .tool_search_reminder_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(5);
        runtime
            .last_memory_scan_ms
            .store(0, std::sync::atomic::Ordering::Relaxed);
        *runtime.conditional_rules_cache.lock().unwrap() = Some(Vec::new());
        runtime
            .plan_reminder_cadence
            .lock()
            .await
            .attachments_emitted = 4;
        runtime
            .date_change
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .delivered_date = Some("2026-08-24".to_string());
        *runtime.prompt_snapshot.lock().await = Some(platform_api::PromptSnapshot {
            system_prompt: vec!["stale".to_string()],
            ..Default::default()
        });
        runtime
            .prompt_snapshot_resume
            .store(true, std::sync::atomic::Ordering::Release);

        runtime.reset_session_scoped().await;

        assert!(runtime.git_status_snapshot.lock().await.is_none());
        assert!(runtime.current_prompt_id.lock().await.is_none());
        assert!(runtime
            .silent_turn_reminder_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
        assert!(runtime
            .pending_memory_updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
        assert!(runtime
            .tool_search_reminder_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
        assert!(
            runtime
                .last_memory_scan_ms
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0
        );
        assert!(runtime.conditional_rules_cache.lock().unwrap().is_none());
        assert_eq!(
            runtime
                .plan_reminder_cadence
                .lock()
                .await
                .attachments_emitted,
            0
        );
        assert!(runtime
            .date_change
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .delivered_date
            .is_none());
        assert!(runtime.prompt_snapshot.lock().await.is_none());
        assert!(!runtime
            .prompt_snapshot_resume
            .load(std::sync::atomic::Ordering::Acquire));
    }
}

/// State shared by manual/reactive compaction and token accounting.
pub(crate) struct CompactionRuntime {
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
    /// P1 session-memory standalone trigger (§6.5). `None` = inert (no caller
    /// wires it). When wired (via [`Self::with_session_memory`]) AND the
    /// extractor's threshold is crossed, [`Self::maybe_extract_session_memory`]
    /// background-forks a distillation at turn start and writes the per-session
    /// memory file the next session re-loads through the Session-tier memdir scan.
    pub(crate) session_memory: Option<Arc<SessionMemoryHandle>>,
}

impl CompactionRuntime {
    pub(crate) fn new() -> Self {
        Self {
            compaction: None,
            compaction_tracking: Mutex::new(compaction::AutoCompactTrackingState::default()),
            compaction_cumulative_dropped_tokens: std::sync::atomic::AtomicU64::new(0),
            last_response_input_tokens: std::sync::atomic::AtomicU64::new(0),
            last_response_output_tokens: std::sync::atomic::AtomicU64::new(0),
            total_tokens_ledger: std::sync::Mutex::new(
                crate::prompt::total_tokens::TotalTokensLedger::default(),
            ),
            output_token_pool: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            turn_start_output_baseline: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            session_memory: None,
        }
    }

    /// Reset compaction engines before model accounting, matching legacy order.
    pub(crate) async fn reset_context_collapse_and_session_memory(&self) {
        if let Some(compactor) = self.compaction.as_ref() {
            let _ = compactor.context_collapse.reset("session_boundary");
        }
        if let Some(handle) = self.session_memory.as_ref() {
            let mut extractor = handle.extractor.lock().await;
            handle
                .generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            extractor.reset();
        }
    }

    /// Clear token counters after model accounting and before refusal state.
    pub(crate) fn reset_token_accounting(&self) {
        self.last_response_input_tokens
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.last_response_output_tokens
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.output_token_pool
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.turn_start_output_baseline
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// State owned by lifecycle hooks, goal checks, and main-thread agents.
pub(crate) struct LifecycleRuntime {
    /// Hook registry (M5-06). `None` when not wired — `list_hooks` then
    /// returns `vec![]`. The CLI binary populates from settings + plugin
    /// sources at startup.
    pub(crate) hook_registry: Option<Arc<tokio::sync::RwLock<hooks::HookRegistry>>>,
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
    /// REM-09 goal check-in deferral bookkeeping (`deferredSince` /
    /// `checkinCount` / `lastDeferralPassAt` on the oracle's `activeGoal`).
    /// Session-scoped and deliberately not persisted on
    /// `lingxi_core::session::ActiveGoalState`.
    pub(crate) goal_checkin: Arc<std::sync::Mutex<crate::prompt::goal_checkin::GoalDeferralState>>,
    /// Idle background timer for `/goal` check-ins. Armed only while a goal is
    /// actively deferred by background work and canceled as soon as that
    /// stretch ends.
    pub(crate) goal_checkin_idle_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Whether the idle goal-checkin loop is currently live.
    pub(crate) goal_checkin_idle_running: Arc<std::sync::atomic::AtomicBool>,
    /// Monotonic generation for idle-loop ownership; prevents a canceled/exiting
    /// older loop from clearing the running bit for a newer loop.
    pub(crate) goal_checkin_idle_generation: Arc<std::sync::atomic::AtomicU64>,
    /// Source of the `Stop` / `SubagentStop` hook `background_tasks` +
    /// `session_crons` snapshot (claude-code `Lic(taskRegistry.all())` /
    /// `Mic()`). Consulted ONLY at the `Stop` / `SubagentStop` firings (claude's
    /// `s` = tool-use-context gate), so other lifecycle hooks omit both keys.
    /// `None` ⇒ both fields stay `None` and the keys are omitted — keeping
    /// no-registry builds byte-identical. Wired at the desktop composition root
    /// from the live `TaskRegistry` + cron file.
    pub(crate) stop_hook_snapshot:
        Option<Arc<dyn crate::stop_hook_snapshot::StopHookSnapshotProvider>>,
    /// Best-effort startup Responses WebSocket prewarm task.
    ///
    /// Lifecycle operations abort this before clearing or exiting the session so
    /// a stale prewarm cannot later seed `previous_response_id`.
    pub(crate) startup_responses_websocket_prewarm:
        std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl LifecycleRuntime {
    pub(crate) fn new() -> Self {
        Self {
            hook_registry: None,
            agent_catalog: None,
            main_thread_agent: tokio::sync::RwLock::new(None),
            main_thread_agent_hook_id: Mutex::new(None),
            goal_checkin: Arc::new(std::sync::Mutex::new(
                crate::prompt::goal_checkin::GoalDeferralState::default(),
            )),
            goal_checkin_idle_task: std::sync::Mutex::new(None),
            goal_checkin_idle_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            goal_checkin_idle_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            stop_hook_snapshot: None,
            startup_responses_websocket_prewarm: std::sync::Mutex::new(None),
        }
    }
}

/// State owned by model controls, usage accounting, and fallback handling.
pub(crate) struct ModelRuntime {
    /// Live main-loop effort. Unlike `config.effort`, this can change through
    /// stream-json control requests and in-place resume.
    pub(crate) current_effort: std::sync::RwLock<Option<String>>,
    /// Provider-neutral live reasoning selection for subsequent requests.
    pub(crate) current_reasoning_selection: std::sync::RwLock<platform_api::ReasoningSelection>,
    /// Whether the live effort came from an explicit launch/control choice.
    /// Hot resume may inherit transcript effort only while this is false.
    pub(crate) current_effort_explicit: std::sync::atomic::AtomicBool,
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
    /// returned [`platform_api::CostSnapshot`]. Stored as `std::time::Instant`
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
    /// Shared pre-call seam for batched/streaming outgoing request rewrites.
    ///
    /// Vision delegation plugs in here: it can persist `MediaAnalysis` into
    /// session history before the main call, rewrite the outgoing snapshot, and
    /// return a retry-safe rewriter so reconnect/fallback/PTL paths keep the
    /// same transformed request without mutating `session.history`.
    pub(crate) model_call_preparer: Option<Arc<dyn ModelCallPreparer>>,
    /// Task 8 (llm-client future-work batch 3): the last rate-limit snapshot
    /// forwarded to [`platform_api::OutputStream::emit_rate_limit`], for the
    /// emit-on-change dedup in [`Self::emit_rate_limit_if_changed`]. Lives on
    /// the orchestrator (not per-turn loop state) so the dedup spans turns —
    /// an identical snapshot across two `run_turn` calls emits exactly once.
    /// `None` until the first emission.
    pub(crate) last_emitted_rate_limit: Mutex<Option<crate::model::rate_limit::RateLimitInfo>>,
    /// Task 2 (llm-client future-work batch 5): the last RAW per-window
    /// utilization snapshot forwarded to
    /// [`platform_api::OutputStream::emit_raw_utilization`], for the
    /// emit-on-change dedup in [`Self::emit_raw_utilization_if_changed`].
    /// Same lifetime/placement rationale as
    /// [`Self::last_emitted_rate_limit`]: lives on the orchestrator so the
    /// dedup spans turns. `None` until the first emission.
    pub(crate) last_emitted_raw_utilization:
        Mutex<Option<crate::model::rate_limit::RawUtilization>>,
}

impl ModelRuntime {
    pub(crate) fn new(
        current_effort: Option<String>,
        current_reasoning_selection: platform_api::ReasoningSelection,
        current_effort_explicit: bool,
    ) -> Self {
        Self {
            current_effort: std::sync::RwLock::new(current_effort),
            current_reasoning_selection: std::sync::RwLock::new(current_reasoning_selection),
            current_effort_explicit: std::sync::atomic::AtomicBool::new(current_effort_explicit),
            refusal_fallback_latched: std::sync::atomic::AtomicBool::new(false),
            refusal_tried_models: Mutex::new(Vec::new()),
            refusal_episode: Mutex::new(crate::refusal_notice::RefusalEpisode::default()),
            refusal_notice_queue: Mutex::new(crate::refusal_notice::NoticeQueue::new()),
            cost_tracker: None,
            analytics_bus: None,
            session_started_at: std::sync::Mutex::new(std::time::Instant::now()),
            api_calls_recorded: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            last_api_call_at_ms: Arc::new(std::sync::atomic::AtomicI64::new(-1)),
            last_emitted_rate_limit: Mutex::new(None),
            last_emitted_raw_utilization: Mutex::new(None),
            cache_safe_slot: None,
            model_call_preparer: None,
        }
    }

    /// Reset cost and API timing between compaction-engine and token resets.
    pub(crate) async fn reset_cost_and_api_accounting(&self) {
        self.api_calls_recorded
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.last_api_call_at_ms
            .store(-1, std::sync::atomic::Ordering::SeqCst);
        *self
            .session_started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = std::time::Instant::now();
    }

    /// Move the active cost projection to the session that has just been
    /// mounted. Existing session cells remain available to scoped background
    /// work and are never reset here.
    pub(crate) async fn switch_cost_session(&self, session_id: protocol::SessionId) {
        if let Some(tracker) = self.cost_tracker.as_ref() {
            tracker.switch_session(session_id).await;
        }
        self.api_calls_recorded
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.last_api_call_at_ms
            .store(-1, std::sync::atomic::Ordering::SeqCst);
        *self
            .session_started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = std::time::Instant::now();
    }

    /// Reset refusal routing after compaction token accounting is cleared.
    pub(crate) fn reset_refusal_fallback(&self) {
        self.refusal_fallback_latched
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.refusal_tried_models
            .try_lock()
            .map(|mut v| v.clear())
            .ok();
    }
}

/// Everything [`ConversationOrchestrator::maybe_extract_session_memory`] needs to
/// run a standalone session-memory extraction (§6.5): the threshold-stateful
/// extractor (behind a `Mutex` for short decision/commit sections), the forked
/// runner that issues the distillation, the resolved config-home for the write
/// path, and a runtime to background-spawn the fork so it never blocks a turn.
pub struct SessionMemoryHandle {
    /// The threshold-gated extractor. Never hold this across a side-query
    /// await; extraction uses a decision/run/validated-commit sequence.
    pub extractor: Mutex<memory::session_memory::SessionMemoryExtractor>,
    /// Forked-agent runner that issues the distillation off the cache prefix.
    pub runner: Arc<sidequery::ForkedAgentRunner>,
    /// Resolved `$LINGXI_CONFIG_DIR ?? ~/.claude` dir (the write base).
    pub config_home: std::path::PathBuf,
    /// Runtime used to background-spawn the extraction fork.
    pub runtime: Arc<dyn platform_api::RuntimeSpawner>,
    /// One extraction at a time per conversation. The flag is claimed before
    /// spawning so scheduler reordering cannot let an older history snapshot
    /// run after a newer extraction and move the watermark backwards.
    pub(crate) in_flight: std::sync::atomic::AtomicBool,
    /// Invalidates extraction tasks captured before `/clear` or in-place
    /// resume. A stale task checks this after acquiring the extractor lock, so
    /// the reset either waits for its commit or makes it exit without mutation.
    pub(crate) generation: std::sync::atomic::AtomicU64,
}

/// Keep the parent's cacheable prefix byte-identical while appending messages
/// that landed after the successful API call which produced that prefix. A
/// prefix mismatch (for example after a history rewrite) falls back to the live
/// history: correctness takes precedence over a cache hit.
pub(super) fn extend_session_memory_fork_context(
    fork_context_messages: &mut Vec<ConversationMessage>,
    history: &[ConversationMessage],
) {
    if history.starts_with(fork_context_messages) {
        let cached_len = fork_context_messages.len();
        fork_context_messages.extend_from_slice(&history[cached_len..]);
    } else {
        *fork_context_messages = history.to_vec();
    }
}

/// Owns the extraction reservation until the background task has finished (or
/// the parent future is dropped before the task is spawned). Keeping the handle
/// in the guard closes the cancellation window between the CAS and `spawn`.
pub(super) struct SessionMemoryInFlightReset(pub(super) Arc<SessionMemoryHandle>);

impl Drop for SessionMemoryInFlightReset {
    fn drop(&mut self) {
        self.0
            .in_flight
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct BoundedUtf8Read {
    pub(super) content: String,
    pub(super) truncated: bool,
}

/// Read at most `max_bytes` without allocating for the complete file, plus one
/// look-ahead byte so the caller can distinguish an exact-fit file from a
/// truncated prefix. Invalid UTF-8 in the retained prefix remains an error,
/// matching `read_to_string`; only an incomplete scalar at a bounded-read edge
/// is discarded.
pub(super) async fn read_utf8_prefix(
    path: &std::path::Path,
    max_bytes: usize,
    max_chars: usize,
) -> std::io::Result<BoundedUtf8Read> {
    use tokio::io::AsyncReadExt as _;

    let file = tokio::fs::File::open(path).await?;
    let read_limit = max_bytes.saturating_add(1);
    let mut bytes = Vec::with_capacity(read_limit.min(64 * 1024));
    file.take(u64::try_from(read_limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .await?;
    let mut truncated = bytes.len() > max_bytes;
    bytes.truncate(max_bytes);
    let mut text = match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_string(),
        Err(error) if error.error_len().is_none() => {
            truncated = true;
            bytes.truncate(error.valid_up_to());
            String::from_utf8(bytes)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?
        }
        Err(error) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
    };
    // Claude's limit is JavaScript `String.length` (UTF-16 code units), not
    // Rust scalar count. Never split a scalar, and count astral characters as
    // two units just like the model-facing implementation.
    let mut utf16_units = 0usize;
    let mut truncate_at = None;
    for (byte_index, ch) in text.char_indices() {
        let units = ch.len_utf16();
        if utf16_units.saturating_add(units) > max_chars {
            truncate_at = Some(byte_index);
            break;
        }
        utf16_units = utf16_units.saturating_add(units);
    }
    if let Some(byte_index) = truncate_at {
        text.truncate(byte_index);
        truncated = true;
    }
    Ok(BoundedUtf8Read {
        content: text,
        truncated,
    })
}

/// Model-visible body for Claude Code's `compact_file_reference` attachment.
///
/// Byte-exact with the 2.1.246 attachment renderer (`F7r`) after substituting
/// its escaped filename and the active Read tool name.
pub(super) fn compact_file_reference_body(path: &std::path::Path, read_tool_name: &str) -> String {
    let path = crate::prompt::sanitize::escape_reminder_path(&path.to_string_lossy());
    format!(
        "Note: {path} was read before the last conversation was summarized, but the contents are too large to include. Use {read_tool_name} tool if you need to access it."
    )
}

/// Find the assistant message carrying a `tool_use` with `tool_use_id` that has
/// NO matching `tool_result` anywhere in `history`, returning a CLONE of that
/// assistant message. 1:1 with claude-code's `findUnresolvedToolUse`
/// (`sessionStorage.ts:4478-4519`): a `tool_result` for the same id (in any user
/// message) means the call already resolved → `None`. The full message is
/// returned (not just the matched block) so the caller can re-emit it as a
/// stream frame, mirroring the TS `yield sdkAssistantMessage`. Used by
/// [`ConversationOrchestrator::run_orphaned_permission`].
pub(super) fn find_unresolved_tool_use_in_history(
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
pub(super) fn camelize_json_keys(value: serde_json::Value) -> serde_json::Value {
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
