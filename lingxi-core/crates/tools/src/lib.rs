//! Tool system — `Tool` trait, registry, and concurrency-partitioned dispatcher.
//!
//! M1.4 ships the `Tool` trait + context + error types. Subsequent tasks add
//! registry, dispatcher, progress channel, content replacement, etc.

#![forbid(unsafe_code)]

pub mod content_replacement;
pub mod context;
pub mod dispatcher;
pub mod permissions;
pub mod progress;
pub mod registry;
pub mod result_storage;
pub mod streaming_exec;
pub mod tool_trait;

pub use context::{ToolUseContext, ToolUseOptions};
pub use dispatcher::{ToolCall, ToolDispatchEvent, ToolDispatcher};
pub use progress::{progress_channel, ToolProgress, ToolProgressReceiver, ToolProgressSender};
pub use registry::ToolRegistry;
pub use result_storage::ToolResultStorage;
pub use tool_trait::*;
