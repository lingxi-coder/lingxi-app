//! Scheduling tools: CronCreate, CronDelete, CronList, RemoteTrigger.
//! Extracted in M8-P7. Cross-platform.
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
pub mod cron_delete;
pub mod cron_list;
pub mod remote_trigger;
pub mod schedule_cron;
pub use cron_delete::CronDeleteTool;
pub use cron_list::CronListTool;
pub use remote_trigger::{ClaudeAiAuthProvider, RemoteTriggerTool};
pub use schedule_cron::CronCreateTool;
/// Register the cron scheduling tools against `reg`.
///
/// `RemoteTrigger` is registered WITHOUT an OAuth auth provider (`None`), so its
/// pre-flight "not authenticated" error fires until a host wires one. The
/// desktop composition root calls [`register_all_with_auth`] instead to hand the
/// tool a credential-store-backed [`ClaudeAiAuthProvider`]. Mobile (WIP) uses
/// this no-provider path.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    register_all_with_auth(reg, ctx, None);
}

/// Register the cron scheduling tools, handing `RemoteTrigger` an in-process
/// [`ClaudeAiAuthProvider`] (`Some(..)` on desktop; `None` is equivalent to
/// [`register_all`]).
pub fn register_all_with_auth(
    reg: &mut tool_api::ToolRegistry,
    ctx: tool_api::BuiltinToolContext,
    auth: Option<std::sync::Arc<dyn ClaudeAiAuthProvider>>,
) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(CronCreateTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(CronDeleteTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(CronListTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(RemoteTriggerTool::new(ctx, auth)));
}
