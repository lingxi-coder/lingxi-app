//! Per-tool default Y/N decisions for the interactive permission prompt.
//!
//! Source-of-truth table — see M5-05 plan §"Tool default Y/N table" for the
//! claude-code references that justify each row. Unknown tool names default
//! to [`PromptDefault::DenyByDefault`] (fail-closed).
//!
//! Aggregate: 22 `DenyByDefault` (destructive / external side-effects),
//! 19 `AllowByDefault` (read-only or agent-local). Total = 41 known tools
//! + one synthetic `<unknown>` fallback.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::gate::PromptDefault;

static TOOL_DEFAULTS: OnceLock<HashMap<&'static str, PromptDefault>> = OnceLock::new();

fn init_defaults() -> HashMap<&'static str, PromptDefault> {
    use PromptDefault::{AllowByDefault, DenyByDefault};
    let mut m: HashMap<&'static str, PromptDefault> = HashMap::with_capacity(41);

    // Allow-by-default tools ([Y/n]) — 19 entries.
    m.insert("Agent", AllowByDefault);
    m.insert("AskUserQuestion", AllowByDefault);
    m.insert("Brief", AllowByDefault);
    m.insert("Config", AllowByDefault);
    m.insert("EnterPlanMode", AllowByDefault);
    m.insert("ExitPlanMode", AllowByDefault);
    m.insert("Glob", AllowByDefault);
    m.insert("Grep", AllowByDefault);
    m.insert("LSP", AllowByDefault);
    m.insert("Read", AllowByDefault);
    m.insert("Skill", AllowByDefault);
    m.insert("Sleep", AllowByDefault);
    m.insert("StructuredOutput", AllowByDefault);
    m.insert("Task", AllowByDefault); // legacy alias of Agent
    m.insert("TaskGet", AllowByDefault);
    m.insert("TaskList", AllowByDefault);
    m.insert("TaskOutput", AllowByDefault);
    m.insert("TodoWrite", AllowByDefault);
    m.insert("ToolSearch", AllowByDefault);

    // Deny-by-default tools ([y/N]) — 22 entries.
    m.insert("Bash", DenyByDefault);
    m.insert("Edit", DenyByDefault);
    m.insert("EnterWorktree", DenyByDefault);
    m.insert("ExitWorktree", DenyByDefault);
    m.insert("ListMcpResourcesTool", DenyByDefault);
    m.insert("MCP", DenyByDefault);
    m.insert("McpAuth", DenyByDefault);
    m.insert("NotebookEdit", DenyByDefault);
    m.insert("PowerShell", DenyByDefault);
    m.insert("REPL", DenyByDefault);
    m.insert("ReadMcpResourceTool", DenyByDefault);
    m.insert("RemoteTrigger", DenyByDefault);
    m.insert("ScheduleCron", DenyByDefault);
    m.insert("SendMessage", DenyByDefault);
    m.insert("TaskCreate", DenyByDefault);
    m.insert("TaskStop", DenyByDefault);
    m.insert("TaskUpdate", DenyByDefault);
    m.insert("TeamCreate", DenyByDefault);
    m.insert("TeamDelete", DenyByDefault);
    m.insert("WebFetch", DenyByDefault);
    m.insert("WebSearch", DenyByDefault);
    m.insert("Write", DenyByDefault);

    debug_assert_eq!(m.len(), 41, "tool defaults table must list all 41 tools");
    m
}

/// Look up the default Y/N decision for a tool name. Unknown tools → Deny.
#[must_use]
pub fn tool_default(name: &str) -> PromptDefault {
    TOOL_DEFAULTS
        .get_or_init(init_defaults)
        .get(name)
        .copied()
        .unwrap_or(PromptDefault::DenyByDefault)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_is_allow_by_default() {
        assert_eq!(tool_default("Read"), PromptDefault::AllowByDefault);
    }

    #[test]
    fn bash_is_deny_by_default() {
        assert_eq!(tool_default("Bash"), PromptDefault::DenyByDefault);
    }

    #[test]
    fn agent_is_allow_by_default() {
        assert_eq!(tool_default("Agent"), PromptDefault::AllowByDefault);
    }

    #[test]
    fn write_edit_notebook_are_deny() {
        assert_eq!(tool_default("Write"), PromptDefault::DenyByDefault);
        assert_eq!(tool_default("Edit"), PromptDefault::DenyByDefault);
        assert_eq!(tool_default("NotebookEdit"), PromptDefault::DenyByDefault);
    }

    #[test]
    fn web_tools_are_deny() {
        assert_eq!(tool_default("WebFetch"), PromptDefault::DenyByDefault);
        assert_eq!(tool_default("WebSearch"), PromptDefault::DenyByDefault);
    }

    #[test]
    fn mcp_tools_are_deny() {
        assert_eq!(tool_default("MCP"), PromptDefault::DenyByDefault);
        assert_eq!(tool_default("McpAuth"), PromptDefault::DenyByDefault);
        assert_eq!(
            tool_default("ListMcpResourcesTool"),
            PromptDefault::DenyByDefault
        );
        assert_eq!(
            tool_default("ReadMcpResourceTool"),
            PromptDefault::DenyByDefault
        );
    }

    #[test]
    fn read_only_tools_are_allow() {
        assert_eq!(tool_default("Glob"), PromptDefault::AllowByDefault);
        assert_eq!(tool_default("Grep"), PromptDefault::AllowByDefault);
        assert_eq!(tool_default("LSP"), PromptDefault::AllowByDefault);
    }

    #[test]
    fn unknown_tool_defaults_to_deny() {
        assert_eq!(tool_default("DoesNotExist"), PromptDefault::DenyByDefault);
        assert_eq!(tool_default(""), PromptDefault::DenyByDefault);
    }

    #[test]
    fn table_size_is_41() {
        let m = init_defaults();
        assert_eq!(m.len(), 41);
    }
}
