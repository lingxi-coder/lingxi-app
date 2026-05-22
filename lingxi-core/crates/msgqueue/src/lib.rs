//! Unified priority message queue for engine inputs.
//!
//! See spec §27 (`QueuedCommand`, `MessageQueueManager`, `QueueOperation`).

#![forbid(unsafe_code)]

pub mod operations;
pub mod queue;

pub use operations::QueueOperation;
pub use queue::{
    MessageQueueManager, NotificationMode, QueuePriority, QueueSource, QueuedCommand,
    QueuedCommandContent,
};
