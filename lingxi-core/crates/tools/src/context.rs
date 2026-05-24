//! Per-invocation tool context.
//!
//! [`ToolUseContext`] carries everything a tool needs about the surrounding
//! call: conversation history, the active tool-use id, agent id, options,
//! and shared state (content replacement, file-state cache in Plan 10).

use crate::content_replacement::ContentReplacementState;
use crate::registry::ToolRegistry;
use lingxi_core::SessionState;
use lingxi_protocol::{AgentId, McpConnectionId, ToolUseId};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Per-invocation context passed to every [`crate::Tool`] method that needs
/// to reason about the current call (validation, permission, execution).
///
/// Cloning is cheap: most fields are `Arc`-shared or `Copy`. The file-state
/// cache slot lands in Plan 10.
#[derive(Clone)]
pub struct ToolUseContext {
    /// Static per-call options (debug flags, budget, system-prompt overrides).
    pub options: ToolUseOptions,
    /// Conversation history up to (but not including) the current call.
    pub messages: Vec<lingxi_protocol::ConversationMessage>,
    /// The tool-use id assigned by the model, if this call is bound to one.
    pub tool_use_id: Option<ToolUseId>,
    /// The agent that issued this call, if known.
    pub agent_id: Option<AgentId>,
    /// Shared content-replacement state. Populated in Task 3.
    pub content_replacement_state: Option<Arc<Mutex<ContentReplacementState>>>,
    /// Mutable session state (M4-04). Tools that mutate the conversation
    /// (`TodoWrite`, `EnterPlanMode`, `ExitPlanMode`) acquire the `Mutex`
    /// before reading or writing. `None` for legacy call sites that have not
    /// wired a session yet; M4-04 tools surface a clear error in that case.
    pub session: Option<Arc<Mutex<SessionState>>>,
    /// Parent's tool registry — `AgentTool` clones this `Arc` into the child
    /// subagent's invocation context so the recursion lock (parent and child
    /// share the same `Arc<ToolRegistry>`) is asserted via `Arc::ptr_eq` in
    /// M4-05 wiring tests. `None` for top-level invocations that have no
    /// parent agent yet.
    pub subagent_registry: Option<Arc<ToolRegistry>>,
    // File state cache wired in Plan 10.
}

/// Per-call options that travel inside [`ToolUseContext`].
#[derive(Clone)]
pub struct ToolUseOptions {
    /// Emit debug-level traces from this call.
    pub debug: bool,
    /// Emit verbose informational traces from this call.
    pub verbose: bool,
    /// Identifier of the model driving the main agent loop.
    pub main_loop_model: String,
    /// Optional cost budget for this call, in nano-USD.
    pub max_budget_nano_usd: Option<u64>,
    /// MCP server connections available for this call.
    pub mcp_clients: Vec<McpConnectionId>,
    /// Whether the agent is running in a non-interactive session.
    pub is_non_interactive_session: bool,
    /// Optional system prompt override (replaces the default).
    pub custom_system_prompt: Option<String>,
    /// Optional system prompt addendum (appended to the default).
    pub append_system_prompt: Option<String>,
}
