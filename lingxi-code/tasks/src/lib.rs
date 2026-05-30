//! Task scheduling and tracking primitives.
//!
//! See spec §6.6 (Tasks subsystem). This crate provides:
//! - The polymorphic [`state::TaskState`] union (7 variants).
//! - The generic [`task_trait::Task`] handler interface.
//! - [`registry::TaskRegistry`] for tracking running tasks.
//! - [`output_manager::TaskOutputManager`] for sandboxed task spool files.
//! - [`notification::TaskNotificationBuilder`] for XML completion notices.
//! - Per-type handler stubs under [`handlers`] (full impls land in M2).

#![forbid(unsafe_code)]

pub mod cron;
pub mod handle;
pub mod handlers;
pub mod id;
pub mod notification;
pub mod output_manager;
pub mod registry;
pub mod state;
pub mod task_trait;

pub use id::{generate_task_id, TaskType};
pub use state::*;
pub use task_trait::*;
