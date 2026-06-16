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
pub mod prompt;
pub mod status_sink;
pub mod swarm;
pub mod team_file;
pub mod team_registry;
pub mod teammate_pump;
pub mod tool_send_message;
pub mod tool_synthetic_output;
pub mod tool_team_create;
pub mod tool_team_delete;

pub use handle::worker_status_label;
pub use mailbox::{MailboxError, MailboxRouter, MessageSender, TeammateMailbox, TeammateMessage};
pub use mode::CoordinatorMode;
pub use prompt::{coordinator_system_prompt, coordinator_user_context, is_env_truthy};
pub use status_sink::CoordinatorStatusSink;
pub use team_registry::{TeamRegistry, WorkerAgent, WorkerStatus};
pub use teammate_pump::run_teammate_pump;
pub use tool_send_message::{SendMessageTool, SEND_MESSAGE_TOOL_NAME};
pub use tool_synthetic_output::{SyntheticOutputTool, SYNTHETIC_OUTPUT_TOOL_NAME};
pub use tool_team_create::{TeamCreateTool, TEAM_CREATE_TOOL_NAME};
pub use tool_team_delete::{TeamDeleteTool, TEAM_DELETE_TOOL_NAME};
