//! Tool system — `Tool` trait, registry, concurrency-partitioned dispatcher,
//! shared helpers (M4-01), and builtin tool implementations (M4-01..M4-08).
//!
//! M1.4 shipped the `Tool` trait + context + error types. M4-01 adds:
//! - `shared/` — cross-tool helpers (binary detection, path validation,
//!   output truncation) reused by every later sub-plan.
//! - `builtin/` — concrete `Tool` impls. M4-01 lands the 6 foundation tools
//!   (Read, Write, Edit, NotebookEdit, Glob, Grep) plus the
//!   `register_all_builtin_tools(registry, ctx)` entrypoint that future
//!   sub-plans extend.

#![forbid(unsafe_code)]

pub mod builtin;
pub mod content_replacement;
pub mod context;
pub mod dispatcher;
pub mod permissions;
pub mod progress;
pub mod registry;
pub mod result_storage;
pub mod shared;
pub mod streaming_exec;
pub mod tool_trait;

pub use builtin::{
    register_all_builtin_tools, BuiltinToolContext, FileEditTool, FileReadTool, FileWriteTool,
    GlobTool, GrepTool, NotebookEditTool,
};
pub use context::{ToolUseContext, ToolUseOptions};
pub use dispatcher::{ToolCall, ToolDispatchEvent, ToolDispatcher};
pub use progress::{progress_channel, ToolProgress, ToolProgressReceiver, ToolProgressSender};
pub use registry::ToolRegistry;
pub use result_storage::ToolResultStorage;
pub use tool_trait::*;
