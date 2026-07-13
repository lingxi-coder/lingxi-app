//! `SubagentSpawner` — narrow trait abstracting `StateMachinePool::allocate`
//! plus the loop until the spawned subagent reports a terminal event.
//!
//! Concrete impl lives in `lingxi-agent` (production adapter over
//! `StateMachinePool` + `ToolRegistry` + `BudgetEnforcer`). Tests inject a
//! recording mock that captures the parent registry / budget Arcs so the
//! `Arc::ptr_eq` recursion-lock + budget-inheritance assertions can fire.
//!
//! See M4-05 wiring follow-up plan.

use crate::budget::BudgetEnforcerHandle;
use crate::tool_invoker::ToolInvoker;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Locked subagent input passed to [`SubagentSpawner::spawn`].
///
/// Mirrors `AgentToolInput` in `lingxi-tools::builtin::agent` byte-for-byte
/// so the trait surface stays insulated from `lingxi-tools`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentSpawnRequest {
    /// The subagent type to resolve (built-in or user/project catalog). NOT
    /// validated here — the spawner resolves it with claude-code precedence
    /// (catalog overrides built-ins; unknown → `general-purpose`).
    pub subagent_type: String,
    /// Initial prompt seeded into the subagent's first turn.
    pub prompt: String,
    /// Optional context-path files injected as system-tagged messages.
    #[serde(default)]
    pub context_paths: Vec<PathBuf>,
    // ===== AgentTool spawn-surface parity (coordinator batch D2a) =====
    // Additive optional fields mirroring claude-code's `AgentTool` schema
    // (`AgentTool.tsx:82-101`). All `#[serde(default)]`/`Option` so existing
    // call sites and serialized payloads remain valid (frozen-crate rule).
    /// Short (3-5 word) human description of the task (TS `description`,
    /// required in the model-facing schema). Carried for telemetry / display.
    #[serde(default)]
    pub description: Option<String>,
    /// Model-family override (`"sonnet"` | `"opus"` | `"haiku"`). Takes
    /// precedence over the resolved [`crate::…`] agent definition's model
    /// (TS `model`). The spawner maps this onto the agent model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Provider profile name for routing the child's model, e.g. the candidate's
    /// resolved profile; `None` = default/unscoped resolution. When set, the
    /// spawner uses [`Self::model`] verbatim as the explicit wire model and
    /// threads this profile through to the subagent api client so the round-trip
    /// targets the named provider (the dual-LLM dual-PROVIDER routing). When
    /// `None`, model resolution + provider selection are unchanged (the legacy
    /// default-provider path).
    #[serde(default)]
    pub model_profile: Option<String>,
    /// Whether to run the spawned agent in the background (TS `run_in_background`,
    /// `AgentTool.tsx:87` `z.boolean().optional()`). claude treats it as a
    /// boolean predicate (`run_in_background === true`), so it is collapsed to a
    /// plain `bool` (default `false`) at this seam — `None`/absent ⇒ `false` —
    /// rather than carrying an `Option<bool>`. The model-facing schema in
    /// `tools/agent` already declares + parses it; this field stops it being
    /// dropped when the request is built.
    #[serde(default)]
    pub run_in_background: bool,
    /// Name for the spawned agent, making it addressable via `SendMessage`
    /// (TS `name`). Carried through; teammate routing is deferred.
    #[serde(default)]
    pub name: Option<String>,
    /// Team name for spawning (TS `team_name`). Carried through; teammate
    /// routing is deferred.
    #[serde(default)]
    pub team_name: Option<String>,
    /// Permission mode for a spawned teammate (TS `mode`, e.g. `"plan"`).
    /// Carried through; permission-mode application is deferred.
    #[serde(default)]
    pub mode: Option<String>,
    /// Isolation mode (`"worktree"` | `"remote"`, TS `isolation`). Carried
    /// through; worktree/remote isolation behavior is deferred.
    #[serde(default)]
    pub isolation: Option<String>,
    /// Absolute path to run the agent in (TS `cwd`). Carried through; the
    /// cwd-override behavior is deferred.
    #[serde(default)]
    pub cwd: Option<String>,
    // ===== Fork-subagent path (codex #5) =====
    // Populated ONLY on the `AgentTool` fork path (subagent_type omitted + the
    // fork gate ON). For every non-fork spawn they stay `None` and the spawner
    // builds the child's system prompt from the resolved `AgentDefinition` body
    // as before. Both `#[serde(default)]` so existing serialized payloads stay
    // valid (frozen-crate rule).
    /// The byte-exact forked conversation prefix the child replays as its cache
    /// prefix (TS `forkContextMessages = toolUseContext.messages`, threaded as
    /// `buildForkedMessages(prompt, assistantMessage)` output: the cloned parent
    /// assistant message + a user message of placeholder tool_results + the
    /// per-child directive). `AgentTool` owns the parent assistant message, so it
    /// builds these and ships them here; on the fork path the spawner seeds
    /// `prompt_messages = []` and lets the runner replay this prefix verbatim.
    #[serde(default)]
    pub fork_context_messages: Option<Vec<protocol::ConversationMessage>>,
    /// The parent's already-rendered system prompt bytes (TS
    /// `override.systemPrompt = forkParentSystemPrompt`, sourced from
    /// `toolUseContext.renderedSystemPrompt`). When set, the spawner uses these
    /// bytes VERBATIM as the child's system prompt and SKIPS the subagent
    /// `Notes:` trailer (re-appending it would bust the prompt cache). `None`
    /// (non-fork, or the orchestrator has not yet threaded the rendered prompt
    /// onto `ToolUseContext`) keeps the existing body+trailer behavior.
    #[serde(default)]
    pub fork_parent_system_prompt: Option<String>,
    /// Structured-output schema (JSON Schema, serialised as a string) the child
    /// must satisfy: the runner injects a forced `StructuredOutput` tool whose
    /// `input_schema` IS this schema, forces `tool_choice` to it, and returns the
    /// model's tool input as the result (claude-code's workflow `agent({schema})`
    /// — validation happens at the tool-call layer). `None` ⇒ free-form text.
    #[serde(default)]
    pub schema: Option<String>,
    /// Per-spawn thinking-effort override (claude-code workflow `agent({effort})`
    /// — `me={...ie,effort:ae}`): a level string (`"low"`..`"max"`) or an integer
    /// budget. When set, the spawner overrides the resolved agent definition's
    /// `effort`. `None` ⇒ the definition's own effort (frontmatter) stands.
    #[serde(default)]
    pub effort: Option<serde_json::Value>,
    /// Originating `tool_use_id` of the spawning Agent tool call. Threaded into a
    /// BACKGROUND agent's task so its `<task-notification>` carries the
    /// `<tool-use-id>` line (claude-code stamps `toolUseId` on the async task).
    /// `None` for sync spawns / call sites that don't carry it.
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// Per-spawn system-prompt override: replaces the resolved `AgentDefinition`'s
    /// `system_prompt` field before the `Notes:` trailer is appended. Used by the
    /// workflow runtime to substitute the schema-variant xBp prompt when
    /// `agent({schema})` is called without an explicit `agentType`
    /// (claude-code `DBp.getSystemPrompt => xBp`). `None` ⇒ use the definition's own body.
    #[serde(default)]
    pub system_prompt_override: Option<String>,
    /// Per-spawn system-prompt addendum: appended to the fully-rendered system
    /// prompt (after Notes + env block). Used by the workflow runtime to inject
    /// the HBp/IBp NOTE addendum when the caller specifies an explicit `agentType`
    /// (claude-code's appended NOTE for workflow-context agents). `None` ⇒ no addendum.
    #[serde(default)]
    pub system_prompt_addendum: Option<String>,
    /// Per-spawn additional disallowed tools: unioned with the resolved
    /// `AgentDefinition`'s `disallowed_tools` before tool resolution. Used by
    /// the workflow runtime to enforce `{SendUserMessage, Agent, Workflow}` on
    /// user-specified agentType spawns (claude-code's disallow union, §6). Empty ⇒ no extra tools denied.
    #[serde(default)]
    pub additional_disallowed_tools: Vec<String>,
    /// The CHILD's recursion depth = the spawning agent's depth + 1 (claude
    /// `spawnDepth = z6(parentContext) + 1`). The spawner stamps it onto the
    /// child's `SubagentContext.depth`, which the tool-resolver consults to gate
    /// the `Agent` tool at `depth < 5` (claude `e9t = 5`). `#[serde(default)]` ⇒
    /// `0` for legacy/serialized payloads, so a child that deserializes without
    /// it behaves like a top-level spawn (the conservative direction).
    #[serde(default)]
    pub depth: u32,
    /// The parent / main-loop model to resolve this spawn's `AgentModel::Inherit`
    /// + bare-family aliases against (claude-code `AgentTool.tsx:418`
    /// `getAgentModel(selectedAgent.model, toolUseContext.options.mainLoopModel, …)`).
    /// `AgentTool` fills it from `ToolUseContext.options.main_loop_model` — the
    /// LIVE session model at the top level (so a mid-session `/model` switch is
    /// reflected), or the IMMEDIATE parent subagent's resolved model on a NESTED
    /// spawn (claude `runAgent.ts:678` seeds each child's `mainLoopModel:
    /// resolvedAgentModel`). When set (and non-empty) it takes precedence over the
    /// spawner's boot/live `default_model`; `None`/empty ⇒ the spawner's own
    /// default is used (non-`AgentTool` spawn paths / legacy serialized payloads).
    #[serde(default)]
    pub parent_model_override: Option<String>,
}

/// Token-usage rollup returned at the end of a successful spawn.
///
/// Mirrors the primary numeric buckets of claude-code's AgentTool result `usage`
/// object (`agentToolUtils.ts:238-256`): `input_tokens`, `output_tokens`,
/// `cache_creation_input_tokens`, `cache_read_input_tokens`. The legacy
/// [`SubagentUsage::total_tokens`] footer field is retained (existing consumers
/// in `tasks::handlers::local_agent` / `dream` spool a `<usage><total_tokens>`
/// footer from it).
///
/// The nullable sub-objects from claude's schema — `server_tool_use`
/// (`{web_search_requests, web_fetch_requests}`), `service_tier`
/// (`standard|priority|batch`), and `cache_creation`
/// (`{ephemeral_1h_input_tokens, ephemeral_5m_input_tokens}`) — are DEFERRED:
/// the runner's source `llm_client::Usage` has no `web_fetch_requests`,
/// `service_tier`, or 1h/5m cache split to populate them faithfully, so they are
/// intentionally omitted rather than zero-faked. Extending `llm_client::Usage`
/// is the prerequisite for carrying them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentUsage {
    /// Total token count consumed by the subagent's turn(s) — the legacy footer
    /// field consumers already read. When the runner populates this, it holds
    /// the same value as the result-level total (claude `getTokenCountFromUsage`
    /// of the final turn's usage = input + cache_creation + cache_read + output).
    pub total_tokens: u64,
    /// Billable input tokens (claude `usage.input_tokens`).
    pub input_tokens: u64,
    /// Billable output tokens (claude `usage.output_tokens`).
    pub output_tokens: u64,
    /// Cache-creation (write) input tokens (claude `usage.cache_creation_input_tokens`).
    pub cache_creation_input_tokens: u64,
    /// Cache-read input tokens (claude `usage.cache_read_input_tokens`).
    pub cache_read_input_tokens: u64,
}

/// Terminal result of one [`SubagentSpawner::spawn`] call.
///
/// The child pool [`protocol::AgentId`] is carried on EVERY variant (matching
/// the source `agent::runner::SubagentEvent`, which already carries `agent_id`
/// on all of Completed/Failed/Killed). This lets the orchestrator key the
/// `SubagentStart`/`SubagentStop` lifecycle on the REAL child id (claude-code
/// `runAgent.ts:347` real `agentId`) instead of minting a fresh one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubagentResult {
    /// The subagent finished normally.
    Completed {
        /// The real child pool agent id (claude `runAgent.ts:347` `agentId`).
        agent_id: protocol::AgentId,
        /// Free-form JSON payload returned by the subagent.
        content: Value,
        /// Token-usage rollup.
        usage: SubagentUsage,
        /// Number of tool-use blocks the subagent executed across the run
        /// (claude result-level `totalToolUseCount`, `agentToolUtils.ts:233`).
        total_tool_use_count: u64,
        /// Wall-clock duration of the run in milliseconds (claude result-level
        /// `totalDurationMs`, `agentToolUtils.ts:234` / `finalizeAgentTool`).
        total_duration_ms: u64,
        /// Token total = claude `getTokenCountFromUsage` of the FINAL turn's
        /// usage (`input + cache_creation + cache_read + output`; `tokens.ts:46-54`)
        /// — the result-level `totalTokens`, NOT a cross-turn sum. Holds the same
        /// value as [`SubagentUsage::total_tokens`] when the runner populates
        /// both from the final response usage.
        total_tokens: u64,
        /// Number of assistant messages the subagent produced across the run
        /// (claude `agentMessages.length` fed into `tengu_agent_tool_completed`'s
        /// `assistant_message_count`, `agentToolUtils.ts:329`). `0` on the stub
        /// path / when the runner does not surface it.
        #[serde(default)]
        assistant_message_count: u64,
        /// Number of text BLOCKS in the subagent's final response content
        /// (claude `response_char_count: content.length` →
        /// `tengu_agent_tool_completed`, `agentToolUtils.ts:328`). Despite the
        /// field name, claude emits `content.length` — the element count of the
        /// final response's `[{type:'text', text}]` array, NOT a summed char
        /// count. `0` when not surfaced.
        #[serde(default)]
        response_char_count: u64,
        /// The FINAL assistant turn's provider request id (claude
        /// `lastAssistantMessage.requestId`, `agentToolUtils.ts:338`) — used to
        /// emit `tengu_cache_eviction_hint` only when present
        /// (`agentToolUtils.ts:339`). `None` on the stub path / when unavailable
        /// → the cache-eviction hint is skipped, matching claude's truthy guard.
        #[serde(default)]
        last_request_id: Option<String>,
    },
    /// The subagent terminated with an error.
    Failed {
        /// The real child pool agent id (claude `runAgent.ts:347` `agentId`).
        agent_id: protocol::AgentId,
        /// Human-readable reason.
        reason: String,
    },
    /// The subagent was cancelled by the host.
    Killed {
        /// The real child pool agent id (claude `runAgent.ts:347` `agentId`).
        agent_id: protocol::AgentId,
    },
}

/// Failure modes for [`SubagentSpawner::spawn`].
#[derive(Debug, Error)]
pub enum SubagentSpawnError {
    /// The agent pool is at capacity.
    #[error("SubagentSpawner: pool full")]
    PoolFull,
    /// The runtime rejected the spawn.
    #[error("SubagentSpawner: runtime error: {0}")]
    Runtime(String),
    /// Any other internal failure.
    #[error("SubagentSpawner: internal error: {0}")]
    Internal(String),
}

/// Pre-spawn selection metadata for the `tengu_agent_tool_selected` event
/// (claude `AgentTool.tsx:419-428`).
///
/// `AgentTool` needs the resolved agent's `source` / `color` / `model` /
/// `is_built_in` BEFORE the spawn (to emit `tengu_agent_tool_selected`) without
/// a second catalog lookup. The production [`SubagentSpawner`] surfaces it via
/// [`SubagentSpawner::resolve_selection`]; the default impl returns a minimal
/// meta (the `agent_type` echoed, everything else empty/false) so existing
/// impls/tests/mocks are unaffected (frozen-crate rule).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedAgentMeta {
    /// Resolved agent type label (claude `selectedAgent.agentType`).
    pub agent_type: String,
    /// Resolved concrete model id (claude `resolvedAgentModel` =
    /// `getAgentModel(...)`). Empty when no default model is wired.
    pub resolved_model: String,
    /// Origin string mapped to claude's `SettingSource` / `'built-in'` / `'plugin'`
    /// literals (claude `selectedAgent.source`): one of `built-in`, `plugin`,
    /// `userSettings`, `projectSettings`, `localSettings`, `flagSettings`,
    /// `policySettings`. Empty when unresolved.
    pub source: String,
    /// The agent's configured color (claude `selectedAgent.color`), or `None`.
    pub color: Option<String>,
    /// Whether this is a built-in agent (claude `isBuiltInAgent(selectedAgent)`
    /// = `source === 'built-in'`).
    pub is_built_in: bool,
    /// The agent definition's `background` frontmatter flag (claude
    /// `selectedAgent.background`). claude computes
    /// `is_async = run_in_background || selectedAgent.background`
    /// (`AgentTool.tsx:426`), so a `background: true` agent is dispatched async
    /// even when the caller omits `run_in_background`. Defaults to `false`.
    #[serde(default)]
    pub background: bool,
}

/// Result of an ASYNC spawn (claude `AgentTool.tsx:754-764` `async_launched`
/// return).
///
/// `AgentTool`'s async branch returns `{ status:'async_launched', agentId,
/// outputFile, canReadOutputFile, ... }` immediately; the lifecycle runs
/// detached. The production [`SubagentSpawner::spawn_async`] surfaces the real
/// child `agent_id` + the on-disk output path (claude `getTaskOutputPath`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsyncLaunch {
    /// The real child pool agent id (claude `agentBackgroundTask.agentId`).
    pub agent_id: protocol::AgentId,
    /// Absolute path to the agent's on-disk output file (claude
    /// `getTaskOutputPath(agentId)`, `AgentTool.tsx:761`).
    pub output_file: String,
}

/// Inheritance bundle the parent agent hands to a child spawn.
///
/// The recursion-lock + budget-inheritance invariants are asserted in
/// `lingxi-tools` tests via `Arc::ptr_eq` on these trait-object pointers.
#[derive(Clone)]
pub struct SubagentInheritance {
    /// Parent's tool invoker (`Arc<ToolRegistry>` wrapped). The recursion
    /// lock requires the child to reuse this exact `Arc`, NOT a fresh one.
    pub tool_invoker: Arc<dyn ToolInvoker>,
    /// Parent's budget enforcer. The child inherits this `Arc` so budget
    /// charges aggregate across the whole agent tree.
    pub budget: Arc<dyn BudgetEnforcerHandle>,
}

impl std::fmt::Debug for SubagentInheritance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The trait-object handles intentionally have no Debug contract. Keep
        // task-input diagnostics useful without exposing or fabricating their
        // concrete implementations.
        f.debug_struct("SubagentInheritance").finish_non_exhaustive()
    }
}

/// One resolved subagent type, surfaced to `AgentTool` so it can build the
/// dynamic tool prompt (claude-code's `formatAgentLine`,
/// `AgentTool/prompt.ts:43-46`: `- {agentType}: {whenToUse} (Tools: …)`).
///
/// Lives in `traits` (a leaf crate) so `tool-agent` — which must NOT depend on
/// the `agent` engine crate — can render the catalog without a cyclic dep. The
/// concrete [`SubagentSpawner`] impl in `agent` populates it from the resolved
/// built-in + user/project [`AgentDefinition`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentListingEntry {
    /// Agent type label (TS `agentType`).
    pub agent_type: String,
    /// "When to use" guidance (TS `whenToUse`).
    pub when_to_use: String,
    /// Pre-rendered tools description (TS `getToolsDescription`): `All tools`,
    /// `All tools except X, Y`, an explicit `A, B, C`, or `None`.
    pub tools_description: String,
}

/// Format one agent catalog line, the single source of truth for claude-code's
/// `formatAgentLine` (`AgentTool/prompt.ts:43-46`):
/// `- {agentType}: {whenToUse} (Tools: {toolsDescription})`.
///
/// Lives here (a leaf crate) so BOTH the inline tool-prompt path (`tool-agent`)
/// and the `agent_listing_delta` attachment path (`agent` crate → orchestrator)
/// render identical lines without `tool-agent` taking a dep on the heavier
/// `agent` engine crate. The `tools_description` is pre-rendered by the
/// spawner / catalog (TS `getToolsDescription`).
#[must_use]
pub fn format_agent_line(entry: &SubagentListingEntry) -> String {
    format!(
        "- {}: {} (Tools: {})",
        entry.agent_type, entry.when_to_use, entry.tools_description
    )
}

/// Whether the Agent catalog should be conveyed as a per-turn
/// `<system-reminder>` attachment (the `agent_listing_delta` path) instead of
/// embedded inline in the `AgentTool` description.
///
/// Port of claude-code `shouldInjectAgentListInMessages`
/// (`AgentTool/prompt.ts`): honor the `LINGXI_AGENT_LIST_IN_MESSAGES`
/// override (`isEnvTruthy` ⇒ true, `isEnvDefinedFalsy` ⇒ false), else the
/// default. **Default is now ON**: in claude-code v2.1.193 the agent catalog is
/// ALWAYS externalized to the per-turn `<system-reminder>` attachment (the
/// `AgentTool` description carries only the static pointer line — there is no
/// inline-catalog variant left). An explicit
/// `LINGXI_AGENT_LIST_IN_MESSAGES=false` opts back into a LEGACY inline
/// catalog (not a 2.1.193 form), retained only as an escape hatch.
#[must_use]
pub fn should_inject_agent_list_in_messages() -> bool {
    let v = std::env::var("LINGXI_AGENT_LIST_IN_MESSAGES").ok();
    if crate::env::is_env_truthy(v.as_deref()) {
        return true;
    }
    if crate::env::is_env_defined_falsy(v.as_deref()) {
        return false;
    }
    // v2.1.193: catalog is always externalized ⇒ default ON.
    true
}

/// Spawn-a-subagent seam used by `AgentTool`.
#[async_trait]
pub trait SubagentSpawner: Send + Sync {
    /// Allocate a subagent slot, pump its state machine to completion, and
    /// return the terminal [`SubagentResult`].
    ///
    /// `inherit` carries the parent's tool invoker (registry recursion lock)
    /// and budget enforcer; the implementation MUST hand the same `Arc`s on
    /// to the child without cloning the inner value.
    async fn spawn(
        &self,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
    ) -> Result<SubagentResult, SubagentSpawnError>;

    /// Like [`Self::spawn`], but forwards a one-line summary of each nested
    /// subagent step (its tool calls, as they run) to `progress` so the caller
    /// (`AgentTool`) can surface the subagent's work under its `Task` cell.
    ///
    /// Defaulted to [`Self::spawn`] (dropping `progress`) so existing impls and
    /// tests need no change (frozen-crate rule); the production pool spawner
    /// overrides it to pump non-terminal events through. `progress` is a
    /// plain `String` channel — `traits` cannot depend on `tool-api`'s
    /// `ToolProgress` (cycle), so the AgentTool bridges the string into a
    /// `ToolProgress` on its side.
    async fn spawn_with_progress(
        &self,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
        _progress: Option<tokio::sync::mpsc::Sender<String>>,
    ) -> Result<SubagentResult, SubagentSpawnError> {
        self.spawn(request, inherit).await
    }

    /// The resolved subagent catalog (built-ins + any wired user/project
    /// agents), used by `AgentTool` to render its dynamic tool prompt.
    ///
    /// Defaulted to empty so existing impls/tests need no change (frozen-crate
    /// rule). The production [`SubagentSpawner`] overrides this to surface the
    /// real catalog with claude-code's later-wins precedence.
    async fn agent_listing(&self) -> Vec<SubagentListingEntry> {
        Vec::new()
    }

    /// Resolve the `required_mcp_servers` declared by the agent definition that
    /// `subagent_type` resolves to (claude-code `AgentDefinition.requiredMcpServers`,
    /// `loadAgentsDir.ts:122`). `AgentTool` calls this BEFORE the spawn to run the
    /// pre-spawn MCP-servers gate (`AgentTool.tsx:367-409`): an agent that requires
    /// MCP servers is unavailable until those servers are connected + authenticated
    /// (i.e. expose tools).
    ///
    /// Defaulted to empty so existing impls/tests need no change (frozen-crate
    /// rule); the production [`SubagentSpawner`] overrides this to read the
    /// resolved definition's `required_mcp_servers`. An empty result means "no
    /// requirement" → the gate is skipped.
    async fn resolve_required_mcp_servers(&self, _subagent_type: &str) -> Vec<String> {
        Vec::new()
    }

    /// Resolve the pre-spawn selection metadata for the
    /// `tengu_agent_tool_selected` event (claude `AgentTool.tsx:419-428`): the
    /// resolved agent `source`, `color`, concrete `model`, and `is_built_in`
    /// flag — surfaced WITHOUT a second catalog lookup or a spawn.
    ///
    /// Defaulted to a minimal meta (the `subagent_type` echoed, everything else
    /// empty/false) so existing impls/tests need no change (frozen-crate rule);
    /// the production [`SubagentSpawner`] overrides it. `model` is the caller's
    /// optional model-family override (claude `model` schema field).
    async fn resolve_selection(
        &self,
        subagent_type: &str,
        _model: Option<&str>,
    ) -> SelectedAgentMeta {
        SelectedAgentMeta {
            agent_type: subagent_type.to_string(),
            ..SelectedAgentMeta::default()
        }
    }

    /// Launch the subagent ASYNC (claude `run_in_background` /
    /// `selectedAgent.background`, `AgentTool.tsx:686-764`): allocate the slot,
    /// drive its lifecycle DETACHED, and return immediately with the
    /// `async_launched` info.
    ///
    /// Defaulted to a CLEAR error (`SubagentSpawnError::Internal`) so an unwired
    /// async branch surfaces an explicit message rather than silently falling
    /// back to a sync spawn (the task forbids a silent wrong path). The
    /// production [`SubagentSpawner`] overrides it once the disk-output +
    /// notification lifecycle exists.
    async fn spawn_async(
        &self,
        _request: SubagentSpawnRequest,
        _inherit: SubagentInheritance,
    ) -> Result<AsyncLaunch, SubagentSpawnError> {
        Err(SubagentSpawnError::Internal(
            "async subagent spawn (run_in_background) is not wired in this build".to_string(),
        ))
    }

    /// Register `name → agent_id` for `SendMessage` routing of a spawned ASYNC
    /// subagent (claude `AppState.agentNameRegistry.set`, `AgentTool.tsx:704-711`).
    ///
    /// Defaulted to a no-op so existing impls/tests need no change; the
    /// production [`SubagentSpawner`] overrides it to store in its internal
    /// name→id map (or delegate to a wired
    /// [`crate::agent_name_registry::AgentNameRegistry`]). Sync agents are NOT
    /// registered (claude `AgentTool.tsx:700-702`).
    async fn register_name(&self, _name: &str, _agent_id: protocol::AgentId) {}

    /// Resolve a previously-registered async-agent name to its id (the read side
    /// of [`Self::register_name`]). Defaulted to `None`; the production spawner
    /// overrides it so a `SendMessage({ to: name })` resolver can route to a
    /// running async subagent.
    async fn resolve_name(&self, _name: &str) -> Option<protocol::AgentId> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn SubagentSpawner>> = None;
    }

    #[test]
    fn format_agent_line_matches_ts_shape() {
        let entry = SubagentListingEntry {
            agent_type: "Explore".into(),
            when_to_use: "Read-only search agent".into(),
            tools_description: "All tools except Edit, Write".into(),
        };
        assert_eq!(
            format_agent_line(&entry),
            "- Explore: Read-only search agent (Tools: All tools except Edit, Write)"
        );
    }

    /// The gate defaults ON in v2.1.193 (catalog always externalized). Guarded by
    /// a process-wide lock because it mutates a shared env var.
    #[test]
    fn agent_list_gate_default_on_and_env_override() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
        assert!(
            should_inject_agent_list_in_messages(),
            "v2.1.193 default must be ON (catalog externalized)"
        );

        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "1");
        assert!(should_inject_agent_list_in_messages(), "truthy ⇒ ON");

        std::env::set_var("LINGXI_AGENT_LIST_IN_MESSAGES", "false");
        assert!(
            !should_inject_agent_list_in_messages(),
            "explicit defined-falsy ⇒ OFF (legacy inline escape hatch)"
        );

        std::env::remove_var("LINGXI_AGENT_LIST_IN_MESSAGES");
    }
}
