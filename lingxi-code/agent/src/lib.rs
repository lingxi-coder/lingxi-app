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

mod accumulator;
pub mod api;
pub mod convert;
pub mod builtins;
pub mod catalog;
pub mod color_manager;
pub mod context;
pub mod definition;
pub mod display;
pub mod fork;
pub mod handle;
pub mod model_resolution;
pub mod multi_dispatch;
pub mod permission_mode;
pub mod pool;
pub mod runner;
pub mod tool_resolver;
pub mod transcript;
pub mod worktree_policy;

pub use api::SubagentApiClient;
pub use builtins::builtin_agent_definitions;
pub use catalog::{load_agents_from_dirs, parse_agent_markdown, AgentLoadError};
pub use color_manager::AgentColorManager;
pub use context::SubagentContext;
pub use definition::*;
pub use display::AgentDisplay;
pub use handle::{agent_listing_entries, tools_description, PoolSubagentSpawner};
// `agent_listing_delta` shared surface: the ONE `formatAgentLine` and the
// `shouldInjectAgentListInMessages` gate live in the leaf `traits` crate (so
// `tool-agent` can reach them without depending on this engine crate); re-export
// them here under the `agent::` path the orchestrator + callers use.
pub use traits::subagent_spawn::{format_agent_line, should_inject_agent_list_in_messages};
pub use model_resolution::resolve_agent_model;
pub use multi_dispatch::{MultiAgentDispatcher, MultiAgentSpawnSpec};
pub use pool::{StateMachinePool, StateMachineSlot};
pub use runner::SubagentEvent;
pub use tool_resolver::AgentToolResolver;
pub use worktree_policy::create_worktree_or_degrade;
