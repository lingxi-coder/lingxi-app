//! Tool system — builtin `Tool` implementations, concurrency-partitioned
//! dispatcher, shared helpers, and the `register_all_builtin_tools` entry
//! point.
//!
//! M8-P3: the abstract surface (`Tool` trait, `ToolUseContext`,
//! `ToolRegistry`, progress, content-replacement) moved to the `tool-api`
//! crate. This crate re-exports it so existing `use tools::…` and
//! `crate::tool_trait::…` paths keep resolving; the builtin tools +
//! `register_all_builtin_tools` stay here until P5/P7 split them into
//! per-category crates.

#![forbid(unsafe_code)]
// M4-01 telemetry emitters convert `u64` byte/duration counters into the
// `AnalyticsValue::Int(i64)` wire type — values are always well below
// `i64::MAX` (file sizes are capped at 256 KB, durations are ms-scale, line
// counts are user-bounded). Same rationale for the `usize → i64` casts in
// tests and a few style choices the per-tool dispatch keeps for readability.
#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::match_wildcard_for_single_variants,
    clippy::single_match_else,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    clippy::format_collect,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::manual_let_else
)]

// Re-export the abstract surface that moved to `tool-api` in P3, including
// the module paths so `crate::tool_trait::…`, `crate::registry::…`, etc.
// keep resolving from the builtin/dispatcher files.
pub use tool_api::tool_trait::*;
pub use tool_api::{content_replacement, context, progress, registry, tool_trait};
pub use tool_api::{
    progress_channel, ContentReplacementState, ToolProgress, ToolProgressReceiver,
    ToolProgressSender, ToolRegistry, ToolUseContext, ToolUseOptions,
};

// Builtin tools + supporting machinery stay in this crate.
pub mod builtin;
pub mod dispatcher;
pub mod permissions;
pub mod result_storage;
pub mod shared;
pub mod streaming_exec;
pub mod tool_invoker_impl;

pub use builtin::{
    register_all_builtin_tools, BuiltinToolContext, FileEditTool, FileReadTool, FileWriteTool,
    GlobTool, GrepTool, NotebookEditTool, WebFetchTool, WebSearchTool,
};
pub use dispatcher::{ToolCall, ToolDispatchEvent, ToolDispatcher};
pub use result_storage::ToolResultStorage;
pub use tool_invoker_impl::RegistryToolInvoker;
