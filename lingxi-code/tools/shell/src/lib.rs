//! Shell execution tools: Bash, PowerShell, REPL.
//!
//! Extracted from the `tools` monolith in M8-P5. Desktop-only (they need
//! process spawning + a sandbox); the mobile composition root never
//! registers them. Each takes a `tool_api::BuiltinToolContext`; `register_all`
//! wires all three. `shared` holds the ANSI-stripping helper used by all
//! three; cross-tool helpers live in `tool_api::util`.

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

pub mod bash;
pub mod command_semantics;
pub mod powershell;
pub mod repl;
pub mod shared;

pub use bash::BashTool;
pub use powershell::PowerShellTool;
pub use repl::REPLTool;

/// Register Bash, PowerShell, and REPL tools against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(BashTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(PowerShellTool::new(ctx.clone())));
    reg.register_builtin(Arc::new(REPLTool::new(ctx)));
}
