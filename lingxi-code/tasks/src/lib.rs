//! Task scheduling and tracking primitives.
//!
//! See spec §6.6 (Tasks subsystem). This crate provides:
//! - The polymorphic [`state::TaskState`] union (9 variants).
//! - The generic [`task_trait::Task`] handler interface.
//! - [`registry::TaskRegistry`] for tracking running tasks.
//! - [`output_manager::TaskOutputManager`] for sandboxed task spool files.
//! - Per-type task handlers under [`handlers`].

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

/// Every workflow that BUILDS a local app, and therefore must take the app's
/// workspace lease and be seen by the delete guard.
///
/// A LIST, not a single name: the drawn-surface workflow (`local-canvas-build`)
/// is a sibling of the routed one, so keying either guard on `"local-app-build"`
/// alone let a canvas app be DELETED WHILE ITS BUILD WAS RUNNING -- with
/// nothing failing, because a guard that does not recognise the workflow simply
/// finds no reason to object.
///
/// ⚠️ `tool-workflow` keeps its own copy for a different question (which builds
/// honour the configured `workflowModel`). The two crates share no natural home
/// -- their only common dependencies are the QuickJS runtime and `traits` --
/// so `engine-mobile`'s `local_app_build_workflow_sets_agree` test depends on
/// both and pins them equal. Add a third build workflow and that test fails
/// until BOTH lists know about it.
pub const LOCAL_APP_BUILD_WORKFLOWS: &[&str] = &["local-app-build", "local-canvas-build"];

pub use handlers::{
    DreamHandler, InProcessTeammateHandler, LocalAgentHandler, LocalBashHandler,
    LocalWorkflowHandler, MonitorHandler, MonitorMcpHandler,
};
pub use id::{generate_task_id, TaskType};
pub use registry::{
    register_agent_handlers, register_dream_handler, register_self_contained_handlers,
};
pub use state::*;
pub use task_trait::*;
