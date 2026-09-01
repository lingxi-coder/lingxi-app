//! Task scheduling and tracking primitives.
//!
//! See spec §6.6 (Tasks subsystem). This crate provides:
//! - The polymorphic [`state::TaskState`] union (10 variants).
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

// `LOCAL_APP_BUILD_WORKFLOWS` (P-1.7 doc'd it as unable to delete this
// const because `engine-mobile`'s `local_app_build_workflow_sets_agree`
// twinned it against `tool_workflow::LOCAL_APP_BUILD_WORKFLOWS`) is gone as
// of P-1.9: this crate's own guards already read a task's typed
// `scope::LocalAppWorkflowTaskScope` instead of a workflow-name string (see
// `registry::TaskRegistry::find_nonterminal_local_app_workflows` and
// `handlers::local_workflow::requires_workspace_lease`), and the ONLY other
// reader was that twin-agreement test. `tool_workflow` now answers "is this
// a Local App build workflow" from a typed field on its own
// `BuiltinWorkflowDescriptor` (`is_local_app_build`), so there is no second
// list left anywhere to twin this crate's (deleted) list against.

pub use handlers::{
    escape_xml, fusion_result_xml, DreamHandler, InProcessTeammateHandler, LocalAgentHandler,
    LocalBashHandler, LocalFusionHandler, LocalWorkflowHandler, MonitorHandler, MonitorMcpHandler,
};
pub use id::{generate_task_id, TaskType};
pub use registry::{
    register_agent_handlers, register_dream_handler, register_fusion_handler,
    register_self_contained_handlers,
};
pub use scope::{LocalAppWorkflowPurpose, LocalAppWorkflowTaskScope, MalformedAppId};
pub use state::*;
pub use task_trait::*;
