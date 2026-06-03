//! Per-spawn runtime context for a subagent.
//!
//! [`SubagentContext`] is the bundle of data the host hands to a
//! [`crate::pool::StateMachinePool`] slot at allocation time. It carries
//! identity, prompt material, tool exposure, optional worktree handle, and
//! the bridges into the various cross-cutting subsystems (memory, MCP,
//! transcripts, content replacement). See spec §10.3.

use crate::definition::AgentDefinition;
use crate::display::AgentDisplay;
use memory::snapshot::AgentMemorySnapshot;
use protocol::{AgentId, ConversationMessage, McpConnectionId};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tool_api::content_replacement::ContentReplacementState;
use traits::WorktreeHandle;

/// All the state required to drive one subagent run.
///
/// Cloning is cheap: heavy state (replacement, memory snapshot) is wrapped
/// in [`Arc`] so callers can hand the context to background tasks without
/// re-allocating.
#[derive(Clone)]
pub struct SubagentContext {
    /// Stable identifier for this spawn — every event carries this id.
    pub agent_id: AgentId,
    /// Parent agent id, when this agent was dispatched by another agent.
    pub parent_agent_id: Option<AgentId>,
    /// Static definition that gave rise to this spawn.
    pub agent_definition: AgentDefinition,
    /// Initial conversation messages seeded into the agent's state machine.
    pub prompt_messages: Vec<ConversationMessage>,
    /// For fork-mode runs, the byte-exact context to splice from the parent.
    pub fork_context_messages: Option<Vec<ConversationMessage>>,
    /// Tool names the agent is allowed to call. Computed by
    /// [`crate::tool_resolver::AgentToolResolver`].
    ///
    /// Also the dispatch-time enforcement key: [`crate::runner::run_subagent`]
    /// refuses any `tool_use` whose name is not in a NON-EMPTY list (before
    /// invoking the inherited tool invoker). EMPTY = no restriction (the guard
    /// is skipped) — NOT "no tools allowed"; an empty resolver result therefore
    /// means unrestricted, so a caller wiring this from a real policy must
    /// produce the full resolved set, never an empty list, for a locked-down
    /// agent. The production spawner populates this from
    /// [`crate::tool_resolver::AgentToolResolver`] (the same resolved set it
    /// serializes into [`Self::tool_schemas`]), so the advertised set and the
    /// allow-list stay in lock-step. The policy comes from the spawn-resolved
    /// [`crate::definition::AgentDefinition`] (built-in or user/project), so a
    /// read-only agent (e.g. `Explore`/`Plan`, `AgentToolPolicy::Except` of the
    /// write tools) genuinely narrows this list; a `general-purpose` agent
    /// (`All`) keeps every registered tool name.
    pub allowed_tools: Vec<String>,
    /// Optional worktree the agent runs inside.
    pub worktree_handle: Option<WorktreeHandle>,
    /// `true` when the agent runs asynchronously (e.g. background scan).
    pub is_async: bool,
    /// `true` for a long-lived, message-driven teammate: after a turn-set ends
    /// with a terminal stop the runner parks awaiting the next inbound
    /// [`engine::Event::UserMessage`] instead of returning. Terminates only on
    /// [`engine::Event::UserExit`] / [`engine::Event::UserInterrupt`] or when
    /// the inbound event channel closes. Distinct from [`Self::is_async`],
    /// which only governs background-vs-foreground scheduling.
    pub persistent: bool,
    /// Whether the agent may surface permission prompts to the user.
    pub can_show_permission_prompts: bool,
    /// MCP connections the agent should attach to.
    pub mcp_clients: Vec<McpConnectionId>,
    /// Directory under which this agent writes its transcript.
    pub transcript_subdir: PathBuf,
    /// Pre-rendered system prompt (post template + frontmatter expansion).
    pub rendered_system_prompt: Option<Arc<str>>,
    /// Shared content-replacement state (e.g. file mention expansion). Wrapped
    /// in a `Mutex` so the tool layer can mutate it across awaits.
    pub content_replacement_state: Option<Arc<Mutex<ContentReplacementState>>>,
    /// Snapshot of the memory tier visible to this agent type.
    pub agent_memory: Option<AgentMemorySnapshot>,
    /// UI display configuration (color, icon).
    pub display: AgentDisplay,
    /// Model API seam used by the multi-turn [`crate::runner::run_subagent`]
    /// loop. `None` keeps the legacy stub behavior (no real API calls) for
    /// back-compat with callers that haven't wired an API client yet.
    pub api_client: Option<Arc<dyn crate::api::SubagentApiClient>>,
    /// Tool dispatch seam inherited from the parent via
    /// [`traits::subagent_spawn::SubagentInheritance`]. `None` means the agent
    /// cannot dispatch tools — a `tool_use` in that state surfaces a failure.
    pub tool_invoker: Option<Arc<dyn traits::ToolInvoker>>,
    /// Wire tool definitions (`{name, description, input_schema}`) advertised to
    /// the model on every round-trip of the multi-turn loop — the streaming
    /// analog of the orchestrator's own `tools` array. Built by the spawner via
    /// [`tool_api::wire::tools_to_wire`]. Empty means the subagent calls the
    /// model with no tools (so it cannot emit `tool_use`).
    ///
    /// LIVE in production, resolved PER-SPAWN: the spawner holds the live tool
    /// registry (filled after it is built — breaking the construction cycle where
    /// the spawner sits inside the `BuiltinToolContext` that builds the registry)
    /// and, at each spawn, runs [`crate::tool_resolver::AgentToolResolver`] over
    /// the registry's `available_tools` per the child's
    /// [`crate::definition::AgentToolPolicy`], serializing the result here AND
    /// recording the resolved names into [`Self::allowed_tools`]. So the
    /// advertised set and the dispatch allow-list narrow TOGETHER, and
    /// resolution reflects the registry's state at spawn time rather than a
    /// one-time serialized snapshot taken at boot. (The shared `Arc<ToolRegistry>`
    /// is immutable once handed to the spawner, so this picks up boot-time
    /// registry state, not live post-boot mutation.) The production
    /// [`Self::tool_invoker`] (`RegistryToolInvoker`) does not enforce policy
    /// itself (`find_by_name` + `tool.call`), so the runner's allow-list check on
    /// `allowed_tools` is the dispatch-time guard. The spawn path now resolves a
    /// REAL [`crate::definition::AgentDefinition`] per `subagent_type` (the 6
    /// built-ins from [`crate::builtins`], overridden by the file catalog), so
    /// the policy is per-agent: a read-only agent narrows this advertised set
    /// AND `allowed_tools` together, while `general-purpose` keeps the full set.
    pub tool_schemas: Vec<serde_json::Value>,
    /// Inherited budget enforcer (from `SubagentInheritance::budget`). When
    /// `Some`, the multi-turn loop consults it once per turn and stops with a
    /// budget-exhausted terminal when the cumulative cost is over the limit.
    /// `None` disables budget enforcement (legacy/test contexts).
    pub budget: Option<Arc<dyn traits::budget::BudgetEnforcerHandle>>,
}
