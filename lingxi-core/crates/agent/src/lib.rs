//! Agent and subagent runtime for `LingXi` Core.
//!
//! Owns the [`pool::StateMachinePool`] (effect-delegated, sibling-slot model
//! rather than nested state machines), the cross-system
//! [`context::SubagentContext`] builder, [`multi_dispatch::MultiAgentDispatcher`]
//! for parallel spawn, the [`color_manager::AgentColorManager`], the
//! [`tool_resolver::AgentToolResolver`], and the worktree degradation policy.
//!
//! See spec §10 for the architecture overview.

#![forbid(unsafe_code)]

pub mod color_manager;
pub mod context;
pub mod definition;
pub mod display;
pub mod fork;
pub mod multi_dispatch;
pub mod permission_mode;
pub mod pool;
pub mod runner;
pub mod tool_resolver;
pub mod transcript;
pub mod worktree_policy;

pub use color_manager::AgentColorManager;
pub use context::SubagentContext;
pub use definition::*;
pub use display::AgentDisplay;
pub use multi_dispatch::{MultiAgentDispatcher, MultiAgentSpawnSpec};
pub use pool::{StateMachinePool, StateMachineSlot};
pub use runner::SubagentEvent;
pub use tool_resolver::AgentToolResolver;
pub use worktree_policy::create_worktree_or_degrade;
