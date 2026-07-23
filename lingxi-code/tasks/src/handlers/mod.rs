//! Per-type task handler stubs.
//!
//! Full implementations land in M2. The M1 stubs exist so dependents can
//! compile against the [`crate::Task`] trait surface.

pub mod dream;
pub mod in_process_teammate;
pub mod local_agent;
pub mod local_bash;
pub mod local_workflow;
pub mod monitor;
pub mod monitor_mcp;
pub mod remote_agent;

// The M2 handler implementations are re-exported at the module root so
// callers (and the registration helper) can name them without the per-type
// submodule path.
pub use dream::DreamHandler;
pub use in_process_teammate::{
    DefaultTeammateDefinition, InProcessTeammateHandler, TeammateDefinitionResolver,
};
pub use local_agent::LocalAgentHandler;
pub use local_bash::{LocalBashHandler, NoopStatusSink, TaskStatusSink};
pub use local_workflow::LocalWorkflowHandler;
pub use monitor::MonitorHandler;
pub use monitor_mcp::MonitorMcpHandler;
