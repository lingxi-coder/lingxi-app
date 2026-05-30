//! Coordinator-mode subsystem.
//!
//! See spec §6.16. The coordinator owns:
//! - the [`CoordinatorMode`] toggle that gates internal tools,
//! - a [`TeamRegistry`] tracking spawned worker agents,
//! - a [`MailboxRouter`] delivering messages between coordinator and workers,
//! - the per-worker [`TeammateMailbox`] inboxes,
//! - swarm-backend trait re-exports for tmux/screen integrations.

#![forbid(unsafe_code)]

pub mod handle;
pub mod internal_tools;
pub mod mailbox;
pub mod mode;
pub mod swarm;
pub mod team_registry;

pub use mailbox::{MailboxError, MailboxRouter, MessageSender, TeammateMailbox, TeammateMessage};
pub use mode::CoordinatorMode;
pub use team_registry::{TeamRegistry, WorkerAgent, WorkerStatus};
