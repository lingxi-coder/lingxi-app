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
pub mod scope;
pub mod state;
pub mod task_trait;

/// The two workflow names that build a local app.
///
/// ⚠️ **No longer the authority for the workspace lease or the App delete
/// guard.** Both used to key off membership in this list (a caller-supplied
/// `workflow_id` string), which let any custom workflow that happened to
/// reuse one of these names collect the same authority as the real build --
/// see design §8.1 and [`crate::scope::LocalAppWorkflowTaskScope`], which
/// replaces that name check. [`crate::handlers::local_workflow`]'s `spawn`
/// and [`crate::registry::TaskRegistry::find_nonterminal_local_app_workflows`]
/// now read a task's typed `scope` field instead and no longer reference
/// this array.
///
/// **Still here, unused by this crate's own guards, for one reason:**
/// `tool-workflow` keeps an independent copy of this same list for a
/// different question (which builds honour the configured `workflowModel`
/// default -- design §18 Phase -1 step 9, not yet migrated), and
/// `engine-mobile`'s `local_app_build_workflow_sets_agree` test (outside
/// this crate, outside this task's owned files) asserts the two arrays are
/// byte-for-byte equal. Deleting this `pub const` is therefore a breaking
/// change to a file this task does not own; closing it needs either that
/// test's removal/rewrite or `tool-workflow`'s own migration (design §18
/// Phase -1 step 9), neither of which belongs here. Do not read this array
/// for lease/delete-guard purposes again -- read `scope` instead.
pub const LOCAL_APP_BUILD_WORKFLOWS: &[&str] = &["local-app-build", "local-canvas-build"];

pub use handlers::{
    DreamHandler, InProcessTeammateHandler, LocalAgentHandler, LocalBashHandler,
    LocalWorkflowHandler, MonitorHandler, MonitorMcpHandler,
};
pub use id::{generate_task_id, TaskType};
pub use registry::{
    register_agent_handlers, register_dream_handler, register_self_contained_handlers,
};
pub use scope::{LocalAppWorkflowPurpose, LocalAppWorkflowTaskScope, MalformedAppId};
pub use state::*;
pub use task_trait::*;
