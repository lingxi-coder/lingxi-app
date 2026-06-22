//! Task scheduling and tracking primitives.
//!
//! See spec §6.6 (Tasks subsystem). This crate provides:
//! - The polymorphic [`state::TaskState`] union (7 variants).
//! - The generic [`task_trait::Task`] handler interface.
//! - [`registry::TaskRegistry`] for tracking running tasks.
//! - [`output_manager::TaskOutputManager`] for sandboxed task spool files.
//! - Per-type handler stubs under [`handlers`] (full impls land in M2).

#![forbid(unsafe_code)]

pub mod cron;
pub mod handle;
pub mod handlers;
pub mod id;
pub mod output_manager;
pub mod registry;
pub mod registry_status_sink;
pub mod state;
pub mod task_trait;

pub use handlers::{
    DreamHandler, InProcessTeammateHandler, LocalAgentHandler, LocalBashHandler,
    LocalWorkflowHandler, MonitorMcpHandler,
};
pub use id::{generate_task_id, TaskType};
pub use registry::{
    register_agent_handlers, register_dream_handler, register_self_contained_handlers,
};
pub use state::*;
pub use task_trait::*;
