//! Cross-crate parity test: every tool name from `lingxi-tools::builtin::*`
//! must resolve via `lingxi_permission::tool_default`. Uses `lingxi-tools`
//! as a dev-dep (no build cycle — `lingxi-tools → lingxi-permission` is the
//! real edge; this test-only dep reverses for verification).
//!
//! Source for the constants: `grep -n 'pub const TOOL_NAME\|pub const.*_TOOL_NAME'
//! lingxi-code/crates/tools/src/builtin/*.rs`.

use lingxi_permission::tool_default;
use lingxi_permission::PromptDefault;

#[test]
fn bash_constant_resolves_deny() {
    assert_eq!(
        tool_default(lingxi_tools::builtin::bash::TOOL_NAME),
        PromptDefault::DenyByDefault
    );
}

#[test]
fn read_constant_resolves_allow() {
    assert_eq!(
        tool_default(lingxi_tools::builtin::file_read::TOOL_NAME),
        PromptDefault::AllowByDefault
    );
}

#[test]
fn write_constant_resolves_deny() {
    assert_eq!(
        tool_default(lingxi_tools::builtin::file_write::TOOL_NAME),
        PromptDefault::DenyByDefault
    );
}

#[test]
fn edit_constant_resolves_deny() {
    assert_eq!(
        tool_default(lingxi_tools::builtin::file_edit::TOOL_NAME),
        PromptDefault::DenyByDefault
    );
}

#[test]
fn agent_constant_resolves_allow() {
    assert_eq!(
        tool_default(lingxi_tools::builtin::agent::AGENT_TOOL_NAME),
        PromptDefault::AllowByDefault
    );
    // Legacy alias of Agent
    assert_eq!(
        tool_default(lingxi_tools::builtin::agent::LEGACY_AGENT_TOOL_NAME),
        PromptDefault::AllowByDefault
    );
}

#[test]
fn web_fetch_constant_resolves_deny() {
    assert_eq!(
        tool_default(lingxi_tools::builtin::web_fetch::TOOL_NAME),
        PromptDefault::DenyByDefault
    );
}

#[test]
fn web_search_constant_resolves_deny() {
    assert_eq!(
        tool_default(lingxi_tools::builtin::web_search::TOOL_NAME),
        PromptDefault::DenyByDefault
    );
}

#[test]
fn mcp_constant_resolves_deny() {
    assert_eq!(
        tool_default(lingxi_tools::builtin::mcp::MCP_TOOL_NAME),
        PromptDefault::DenyByDefault
    );
}
