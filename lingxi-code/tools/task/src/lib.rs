//! Task lifecycle tools: TaskCreate/Get/List/Update/Stop/Output + TodoWrite.
//!
//! Extracted from the `tools` monolith in M8-P7. Cross-platform (they
//! dispatch through `TaskRegistryHandle` carried in `BuiltinToolContext`).

#![forbid(unsafe_code)]
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

pub mod monitor;
pub mod reminder;
pub mod task;
pub mod todo_store;
pub mod todo_write;

pub use monitor::MonitorTool;
pub use task::{
    TaskCreateTool, TaskGetTool, TaskListTool, TaskOutputTool, TaskStopTool, TaskUpdateTool,
};
pub use todo_write::TodoWriteTool;

/// Register the 6 task tools + TodoWrite against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(TaskCreateTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(TaskGetTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(TaskListTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(TaskUpdateTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(TaskStopTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(TaskOutputTool::new(ctx.clone())));
    // PARITY: the `Monitor` tool (binary `EVp`). Registered always; isEnabled
    // (flag `tengu_amber_sentinel`, default off) gates exposure — invisible to the
    // model by default, like the shipped binary.
    reg.register_builtin(Arc::new(MonitorTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(TodoWriteTool::new(ctx)));
}
