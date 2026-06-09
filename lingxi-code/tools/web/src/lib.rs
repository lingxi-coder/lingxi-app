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
pub mod blocklist;
pub mod cache;
mod markdown;
pub mod url_safety;
pub mod web_fetch;
pub mod web_search;
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
/// Register the web fetch + search tools against `reg`.
///
/// `side_query`: the small-fast client powering WebFetch's apply step. `None`
/// (mobile/minimal, or the offline registry-snapshot path) => WebFetch returns
/// markdown without the secondary model.
pub fn register_all(
    reg: &mut tool_api::ToolRegistry,
    ctx: tool_api::BuiltinToolContext,
    side_query: Option<std::sync::Arc<dyn sidequery::SideQueryClient>>,
) {
    use std::sync::Arc;
    let web_fetch = match side_query {
        Some(client) => WebFetchTool::new(ctx.clone()).with_side_query(client),
        None => WebFetchTool::new(ctx.clone()),
    };
    reg.register_builtin(Arc::new(web_fetch));
    reg.register_builtin(Arc::new(WebSearchTool::new(ctx)));
}
