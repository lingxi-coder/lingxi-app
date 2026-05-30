//! UI / interaction tools: AskUserQuestion, Brief, SendMessage, Sleep,
//! SyntheticOutput. Extracted in M8-P7. Cross-platform.
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
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(SleepTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(SendMessageTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(AskUserQuestionTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(BriefTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(SyntheticOutputTool::new(ctx)));
}
