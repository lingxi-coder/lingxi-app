//! Scheduling tools: ScheduleCron, RemoteTrigger. Extracted in M8-P7. Cross-platform.
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
pub mod remote_trigger;
pub mod schedule_cron;
pub use remote_trigger::RemoteTriggerTool;
pub use schedule_cron::ScheduleCronTool;
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(ScheduleCronTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(RemoteTriggerTool::new(ctx)));
}
