//! LSP tool. Extracted in M8-P7. Desktop-only (spawns language servers).
//! The impl module is `lsp_tool` (not `lsp`) to avoid colliding with the
//! extern `lsp` crate this tool depends on.
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
pub mod lsp_tool;
pub use lsp_tool::LSPTool;
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(LSPTool::new(ctx)));
}
