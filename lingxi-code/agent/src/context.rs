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
use tools::content_replacement::ContentReplacementState;
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
    pub allowed_tools: Vec<String>,
    /// Optional worktree the agent runs inside.
    pub worktree_handle: Option<WorktreeHandle>,
    /// `true` when the agent runs asynchronously (e.g. background scan).
    pub is_async: bool,
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
}
