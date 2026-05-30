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
// M4-01 telemetry emitters convert `u64` byte/duration counters into the
// `AnalyticsValue::Int(i64)` wire type — values are always well below
// `i64::MAX` (file sizes are capped at 256 KB, durations are ms-scale, line
// counts are user-bounded). The saturating helper used in `lingxi-cost`
// would add noise across 6 emitter files for no observable benefit. Same
// rationale for the `usize → i64` casts in tests and for a few style
// choices (let-else vs match destructure) the per-tool dispatch keeps for
// readability.
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
pub mod tool_invoker_impl;
pub mod tool_trait;

pub use builtin::{
    register_all_builtin_tools, BuiltinToolContext, FileEditTool, FileReadTool, FileWriteTool,
    GlobTool, GrepTool, NotebookEditTool, WebFetchTool, WebSearchTool,
};
pub use context::{ToolUseContext, ToolUseOptions};
pub use dispatcher::{ToolCall, ToolDispatchEvent, ToolDispatcher};
pub use progress::{progress_channel, ToolProgress, ToolProgressReceiver, ToolProgressSender};
pub use registry::ToolRegistry;
pub use result_storage::ToolResultStorage;
pub use tool_invoker_impl::RegistryToolInvoker;
pub use tool_trait::*;
