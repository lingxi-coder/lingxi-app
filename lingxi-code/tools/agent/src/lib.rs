//! Agent tool: AgentTool (subagent dispatch). Extracted in M8-P7.
//! Desktop-only. Dispatches via the SubagentSpawner carried in
//! BuiltinToolContext (a `traits` seam), so no dep on the `agent` engine crate.
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
pub mod agent;
pub use agent::AgentTool;

#[cfg(any(test, feature = "agent-test-support"))]
pub mod agent_test_support;

/// Register the agent (subagent dispatch) tools against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(AgentTool::new(ctx)));
}
