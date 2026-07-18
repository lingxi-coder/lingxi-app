//! MCP tools: MCPTool, McpAuth, ListMcpResources, ReadMcpResource. Extracted
//! in M8-P7. Desktop-only. Impl module is `mcp_tool` (not `mcp`) to avoid
//! colliding with the extern `mcp` crate dep.
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
pub mod auto_background;
pub mod large_output;
pub mod mcp_tool;
pub mod transform_result;
pub use large_output::process_mcp_result;
pub use mcp_tool::{
    build_registered_mcp_tools, ListMcpResourcesTool, MCPTool, McpAuthTool, ReadMcpResourceTool,
};
pub use transform_result::transform_result_content;
/// Register the MCP tools (call, list/read resources, auth) against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(MCPTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(McpAuthTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(ListMcpResourcesTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(ReadMcpResourceTool::new(ctx)));
}
