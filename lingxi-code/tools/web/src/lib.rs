//! Web tools: WebFetch, WebSearch. Extracted from the monolith in M8-P7.
//! Cross-platform (HTTP via the injected `HttpTransport`).
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
pub mod web_fetch;
pub mod web_search;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(WebFetchTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(WebSearchTool::new(ctx)));
}
