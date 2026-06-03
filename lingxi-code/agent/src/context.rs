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
    /// agent. (The current spawn path leaves this empty + `AgentToolPolicy::All`.)
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
    /// LIVE in production (the CLI boot path fills the spawner's set-once cell
    /// from the live registry after it is built — breaking the construction
    /// cycle where the spawner sits inside the `BuiltinToolContext` that builds
    /// the registry). The advertised set is the FULL registry, NOT the per-agent
    /// [`crate::definition::AgentToolPolicy`]-filtered set, because the spawn path
    /// loads no real [`crate::definition::AgentDefinition`] yet — it hardcodes
    /// `AgentToolPolicy::All`. The production [`Self::tool_invoker`]
    /// (`RegistryToolInvoker`) does NOT enforce policy either (`find_by_name` +
    /// `tool.call`), so the dispatch-time guard is the runner's allow-list check
    /// on [`Self::allowed_tools`] in [`crate::runner::run_subagent`] — IMPLEMENTED.
    /// What remains DEFERRED is (a) filtering THIS advertised set through
    /// [`crate::tool_resolver::AgentToolResolver`] AND populating `allowed_tools`
    /// from the resolved policy, so a future policy-restricted agent is neither
    /// told about nor able to call a tool outside its policy. Safe today only
    /// because every spawnable agent is `AgentToolPolicy::All` with empty
    /// `allowed_tools`.
    pub tool_schemas: Vec<serde_json::Value>,
    /// Inherited budget enforcer (from `SubagentInheritance::budget`). When
    /// `Some`, the multi-turn loop consults it once per turn and stops with a
    /// budget-exhausted terminal when the cumulative cost is over the limit.
    /// `None` disables budget enforcement (legacy/test contexts).
    pub budget: Option<Arc<dyn traits::budget::BudgetEnforcerHandle>>,
}
