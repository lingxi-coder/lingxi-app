//! UI / interaction tools: AskUserQuestion, Brief, SendMessage, Sleep,
//! StructuredOutput (the `SyntheticOutputTool` struct, wire name
//! `StructuredOutput`). Extracted in M8-P7. Cross-platform.
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
pub mod ask_user_question;
pub mod brief;
pub mod send_message;
pub mod sleep;
pub mod synthetic_output;
pub use ask_user_question::AskUserQuestionTool;
pub use brief::BriefTool;
pub use send_message::SendMessageTool;
pub use sleep::SleepTool;
pub use synthetic_output::SyntheticOutputTool;
/// Register the UI tools against `reg` (the full set, including the builtin
/// `SendMessage`). This is the default-session path.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    register_with_options(reg, ctx, true);
}

/// Register the UI tools EXCEPT the builtin `SendMessage`.
///
/// A coordinator-capable session registers the richer
/// `coordinator::tool_send_message::SendMessageTool` IN PLACE OF this builtin
/// (it carries the swarm routing surface — broadcast / name-resolution /
/// shutdown + plan-approval handshake). Because the registry's `find_by_name`
/// is builtin-first, both must not be present or the earlier one would
/// silently shadow the later — so the engine skips this builtin in coordinator
/// mode, mirroring how `tool_team::register_all` is skipped for `TeamCreate` /
/// `TeamDelete` (see `engine-desktop::register_desktop_tools`).
pub fn register_all_except_send_message(
    reg: &mut tool_api::ToolRegistry,
    ctx: tool_api::BuiltinToolContext,
) {
    register_with_options(reg, ctx, false);
}

fn register_with_options(
    reg: &mut tool_api::ToolRegistry,
    ctx: tool_api::BuiltinToolContext,
    include_send_message: bool,
) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(SleepTool::new(ctx.clone())));
    if include_send_message {
        reg.register_builtin(Arc::new(SendMessageTool::new(ctx.clone())));
    }
    reg.register_builtin(Arc::new(AskUserQuestionTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(BriefTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(SyntheticOutputTool::new(ctx)));
}
