//! Port of `getToolSearchOrReadInfo` (`utils/collapseReadSearch.ts:143`).
//!
//! The fullscreen-only branches (`isFullscreenEnvEnabled()`) — Snip / ToolSearch
//! silent-absorb, MCP server names, and the non-search Bash "Ran N bash
//! commands" category — plus the memory-write branch are gated OFF in the
//! default inline TUI, matching claude-code's inline default (design doc §7), so
//! they are omitted here. Add them behind a real fullscreen env check when a
//! fullscreen surface lands.

use serde_json::Value;
use tool_shell::search_read::{is_search_or_read_command, ReadSearchKind};

/// Whether the fullscreen surface is enabled (claude-code
/// `isFullscreenEnvEnabled()`). The inline TUI is always inline.
const FULLSCREEN_ENABLED: bool = false;

/// Classification of one tool use (`SearchOrReadResult` in the reference).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOrReadResult {
    /// Folds into a collapsed group (does not break the run).
    pub is_collapsible: bool,
    /// Search (Grep/Glob or bash grep/rg/find).
    pub is_search: bool,
    /// Read (Read or bash cat/head/tail).
    pub is_read: bool,
    /// Directory listing (bash ls/tree/du).
    pub is_list: bool,
    /// REPL wrapper — absorbed with no count.
    pub is_repl: bool,
    /// Meta-op absorbed with no count, visible only in verbose.
    pub is_absorbed_silently: bool,
    /// Fullscreen-only: a non-search/read Bash command ("Ran N bash commands").
    pub is_bash: bool,
}

/// The `command` string of a Bash tool input, if present.
fn bash_command(input: &Value) -> Option<&str> {
    input.get("command").and_then(Value::as_str)
}

/// Classify one tool use by name + raw JSON input (`getToolSearchOrReadInfo`).
#[must_use]
pub fn classify(tool: &str, input: &Value) -> SearchOrReadResult {
    // REPL is absorbed silently — its inner tool calls flow through separately
    // as regular Read/Grep/Bash uses. The wrapper contributes no count and does
    // not break the group.
    if tool == "REPL" {
        return SearchOrReadResult {
            is_collapsible: true,
            is_repl: true,
            is_absorbed_silently: true,
            ..Default::default()
        };
    }
    let base: ReadSearchKind = is_search_or_read_command(tool, bash_command(input));
    let core = base.is_collapsible();
    // Under fullscreen, a non-search/read Bash command is its own collapsible
    // category ("Ran N bash commands") instead of breaking the group.
    let is_bash = FULLSCREEN_ENABLED && !core && tool == "Bash";
    SearchOrReadResult {
        is_collapsible: core || is_bash,
        is_search: base.is_search,
        is_read: base.is_read,
        is_list: base.is_list,
        is_bash,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_grep_glob_collapse() {
        assert!(classify("Read", &json!({"file_path": "a.rs"})).is_read);
        assert!(classify("Grep", &json!({"pattern": "x"})).is_search);
        assert!(classify("Glob", &json!({"pattern": "*.rs"})).is_search);
    }

    #[test]
    fn repl_is_silent_absorb() {
        let r = classify("REPL", &json!({}));
        assert!(r.is_collapsible && r.is_repl && r.is_absorbed_silently);
    }

    #[test]
    fn bash_cat_reads_bash_rm_breaks() {
        assert!(classify("Bash", &json!({"command": "cat a.rs"})).is_read);
        assert!(!classify("Bash", &json!({"command": "rm a.rs"})).is_collapsible);
    }

    #[test]
    fn edit_write_are_not_collapsible() {
        assert!(!classify("Edit", &json!({"file_path": "a"})).is_collapsible);
        assert!(!classify("Write", &json!({"file_path": "a"})).is_collapsible);
    }
}
