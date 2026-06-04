//! Pure MCP name-normalization — 1:1 port of claude-code
//! `src/services/mcp/normalization.ts` (a standalone no-dependency module).
//!
//! Server names become the `<server>` token in the `mcp__<server>__<tool>`
//! tool full-name, which the Anthropic API requires to match
//! `^[a-zA-Z0-9_-]{1,64}$`. A server named `my.server` or `My Server` would
//! otherwise produce an invalid tool name and break EVERY tool on that server.
//!
//! ## Wiring status (important)
//! This is applied at the [`crate::client::McpClient`] FQN boundary (build in
//! `list_tools`, strip in `call_tool`) so that round-trip is self-consistent
//! for invalid-char names. The BROADER MCP-tool-to-model path is still UNWIRED
//! in production and is the real prerequisite gap (separate batch):
//! - the agent's wire tool list (`ConversationOrchestrator::build_wire_tools`)
//!   serializes only the `ToolRegistry` (builtins + the generic `MCPTool`
//!   meta-tool) — individual `mcp__server__tool` entries are NOT advertised;
//! - `McpRegistry::register_client` has NO production caller, so
//!   `get_client`/the `MCPTool` dispatch path returns `None` at runtime;
//! - the posix transport emits an EMPTY `<server>` token (`platforms/posix/
//!   src/mcp.rs`, "rewritten later") and no rewrite site exists yet.
//! When that path is wired, ALSO normalize the rewrite site AND make
//! `get_client`/`get_config` match by `normalize(stored_key) == arg` (claude-code
//! `normalizeNameForMCP(client.name) === serverName`) to complete the round-trip,
//! keeping the raw `config.name` for `/mcp` display.

/// Claude.ai server names are prefixed with this string.
const CLAUDEAI_SERVER_PREFIX: &str = "claude.ai ";

/// Normalize a server name to the API pattern `^[a-zA-Z0-9_-]{1,64}$` by
/// replacing every invalid character (including `.` and spaces) with `_`.
///
/// For claude.ai servers (names starting with `"claude.ai "`), additionally
/// collapse runs of `_` to a single `_` and strip leading/trailing `_`, so the
/// normalized name can't interfere with the `__` delimiter in tool full-names.
///
/// Faithful to `normalizeNameForMCP`. NOTE: like claude-code, this does NOT
/// truncate to 64 chars — the `{1,64}` length bound is the API's, not enforced
/// here.
#[must_use]
pub fn normalize_name_for_mcp(name: &str) -> String {
    let mut normalized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.starts_with(CLAUDEAI_SERVER_PREFIX) {
        normalized = collapse_and_trim_underscores(&normalized);
    }
    normalized
}

/// `replace(/_+/g, '_').replace(/^_|_$/g, '')` — collapse runs of `_` to one,
/// then strip a single leading and trailing `_`.
fn collapse_and_trim_underscores(s: &str) -> String {
    let mut collapsed = String::with_capacity(s.len());
    let mut prev_underscore = false;
    for c in s.chars() {
        if c == '_' {
            if !prev_underscore {
                collapsed.push('_');
            }
            prev_underscore = true;
        } else {
            collapsed.push(c);
            prev_underscore = false;
        }
    }
    // `^_|_$` strips at most ONE leading and ONE trailing underscore (the JS
    // regex without the `g` flag replaces a single match of each alternative).
    let trimmed = collapsed
        .strip_prefix('_')
        .unwrap_or(&collapsed)
        .to_string();
    trimmed.strip_suffix('_').unwrap_or(&trimmed).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_invalid_chars_with_underscore() {
        assert_eq!(normalize_name_for_mcp("my.server"), "my_server");
        assert_eq!(normalize_name_for_mcp("My Server"), "My_Server");
        assert_eq!(normalize_name_for_mcp("a/b:c@d"), "a_b_c_d");
    }

    #[test]
    fn keeps_valid_chars() {
        assert_eq!(normalize_name_for_mcp("filesystem"), "filesystem");
        assert_eq!(normalize_name_for_mcp("my-server_1"), "my-server_1");
        assert_eq!(normalize_name_for_mcp("MOCK"), "MOCK");
    }

    #[test]
    fn claudeai_prefix_collapses_and_trims() {
        // "claude.ai foo" → replace → "claude_ai_foo" (no collapse needed)
        assert_eq!(normalize_name_for_mcp("claude.ai foo"), "claude_ai_foo");
        // dots+spaces produce adjacent underscores that collapse, and a leading
        // invalid char would produce a leading underscore that is trimmed.
        assert_eq!(normalize_name_for_mcp("claude.ai .a..b "), "claude_ai_a_b");
    }

    #[test]
    fn non_claudeai_does_not_collapse() {
        // Without the claude.ai prefix, adjacent underscores are PRESERVED
        // (faithful: only claude.ai names collapse).
        assert_eq!(normalize_name_for_mcp("a..b"), "a__b");
        assert_eq!(normalize_name_for_mcp(".lead"), "_lead");
    }

    #[test]
    fn idempotent_on_already_valid() {
        let n = normalize_name_for_mcp("my.server");
        assert_eq!(normalize_name_for_mcp(&n), n);
    }
}
