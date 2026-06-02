//! Meta tools: Config, ToolSearch. Extracted in M8-P7. Cross-platform.
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
pub mod config;
pub mod tool_search;
pub use config::ConfigTool;
pub use tool_search::ToolSearchTool;
/// Register the meta tools (tool search) against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(ConfigTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(ToolSearchTool::new(ctx)));
}
