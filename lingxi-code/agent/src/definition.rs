//! Static description of an agent (a.k.a. subagent type).
//!
//! An [`AgentDefinition`] is parsed from a markdown frontmatter file or
//! synthesized in code; it carries the tool policy, permission mode, model
//! preference, and worktree requirement that downstream subsystems consult
//! when a new agent is spawned. See spec §10.2.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Self-contained description of an agent type used by
/// [`crate::context::SubagentContext`] and [`crate::pool::StateMachinePool`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Stable agent kind label (e.g. `"general"`, `"reviewer"`, `"fork"`).
    pub agent_type: String,
    /// Free-form guidance shown to the orchestrator about when to dispatch
    /// this agent.
    pub when_to_use: String,
    /// Tool exposure policy resolved by
    /// [`crate::tool_resolver::AgentToolResolver`].
    pub tools: AgentToolPolicy,
    /// Hard upper bound on turns this agent may take before forced shutdown.
    pub max_turns: u32,
    /// Model preference. Resolved against the host's model catalog at spawn.
    pub model: AgentModel,
    /// Permission mode applied to tool calls — see [`AgentPermissionMode`].
    pub permission_mode: AgentPermissionMode,
    /// Origin of the definition (built-in, user file, plugin, ...).
    pub source: AgentSource,
    /// Directory the definition was loaded from. Used to resolve relative
    /// includes (e.g. system-prompt fragments).
    pub base_dir: PathBuf,
    /// Optional system-prompt body to prepend to the per-spawn template.
    pub system_prompt: Option<String>,
    /// MCP servers the agent should connect to.
    pub mcp_servers: Vec<AgentMcpServerSpec>,
    /// Hooks declared inside the agent's frontmatter.
    pub frontmatter_hooks: Vec<hooks::HookDefinition>,
    /// Optional emoji or short icon for the UI.
    pub icon: Option<String>,
    /// Allow-list of tool names appended to the resolver output. Often used
    /// to expose MCP-server tools without expanding `AgentToolPolicy::All`.
    pub allowed_tools: Vec<String>,
    /// Worktree policy (required, optional, none) — see
    /// [`crate::worktree_policy::create_worktree_or_degrade`].
    pub worktree_requirement: Option<WorktreeRequirement>,
}

/// Strategy the [`crate::tool_resolver::AgentToolResolver`] uses to project
/// the parent agent's tool set onto the child.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentToolPolicy {
    /// Inherit every tool from the parent. `use_exact_tools` controls whether
    /// the child receives the parent's exact tool instances or fresh ones.
    All {
        /// `true` when the child must reuse the parent's `Arc<dyn Tool>`
        /// references (e.g. to share state); `false` when it may be re-bound.
        use_exact_tools: bool,
    },
    /// Inherit only the explicitly named tools.
    Explicit(Vec<String>),
    /// Inherit every tool except the explicitly named ones.
    Except(Vec<String>),
}

/// Model resolution strategy for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentModel {
    /// Use whichever model the parent picked.
    Inherit,
    /// Resolve via a logical alias (e.g. `"sonnet"`, `"haiku-fast"`).
    Alias(String),
    /// Use the specific model id verbatim.
    Explicit(String),
}

/// Permission mode applied to tool calls inside the subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentPermissionMode {
    /// Bubble prompts up to the parent agent for approval.
    Bubble,
    /// Run isolated — never prompt; deny if not pre-approved.
    Isolated,
    /// Auto-approve every tool call (sandboxed contexts).
    Auto,
    /// Plan-only mode — read-only tools only.
    Plan,
}

/// Origin of an [`AgentDefinition`]. Used by precedence rules at load time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentSource {
    /// Built into the binary.
    BuiltIn,
    /// Defined by the user (e.g. `~/.config/lingxi/agents/`).
    UserDefined,
    /// Defined inside the current project tree.
    Project,
    /// Provided by a plugin.
    Plugin,
    /// Sourced from policy settings (org-wide).
    PolicySettings,
}

/// Reference to an MCP server an agent should connect to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMcpServerSpec {
    /// Refer to a server already registered by name in the host's
    /// [`mcp::McpRegistry`].
    ByName(String),
    /// Provide a full inline configuration, registered on demand.
    Inline {
        /// Logical name of the inline server.
        name: String,
        /// Connection configuration.
        config: mcp::McpServerConfig,
    },
}

/// Whether the agent needs an isolated git worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeRequirement {
    /// Spawn must fail if a worktree cannot be created.
    Required,
    /// Try to create one; fall back to in-place execution on failure.
    Optional,
    /// Don't bother with worktrees.
    None,
}

impl AgentDefinition {
    /// `true` if this is the special `fork` agent that runs a byte-exact
    /// copy of the parent's prompt cache. See spec §10.4.
    #[must_use]
    pub fn is_fork(&self) -> bool {
        self.agent_type == "fork"
    }
}
