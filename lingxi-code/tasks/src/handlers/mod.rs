//! Per-type task handler stubs.
//!
//! Full implementations land in M2. The M1 stubs exist so dependents can
//! compile against the [`crate::Task`] trait surface.

pub mod dream;
pub mod in_process_teammate;
pub mod local_agent;
pub mod local_bash;
pub mod local_workflow;
pub mod monitor_mcp;
pub mod remote_agent;
