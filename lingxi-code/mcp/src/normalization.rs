//! Pure MCP name-normalization — 1:1 port of claude-code
//! `src/services/mcp/normalization.ts` (a standalone no-dependency module).
//!
//! Server names become the `<server>` token in the `mcp__<server>__<tool>`
//! tool full-name, which the Anthropic API requires to match
//! `^[a-zA-Z0-9_-]{1,64}$`. A server named `my.server` or `My Server` would
//! otherwise produce an invalid tool name and break EVERY tool on that server.
//!
//! ## Wiring status
//!
//! Applied at two boundaries:
//!
//! - the [`crate::client::McpClient`] FQN boundary (build in `list_tools`,
//!   strip in `call_tool`) so the round-trip is self-consistent for
//!   invalid-char names;
//! - the [`crate::registry::McpRegistry::connect`] rewrite site, which stamps
//!   the normalized `<server>` token into each discovered tool's `full_name`
//!   (the posix transport emits an EMPTY token, deferring the rewrite here),
//!   and the [`crate::registry::McpRegistry::get_client`] /
//!   [`crate::registry::McpRegistry::get_config`] normalize-match
//!   (`normalize(stored_key) == arg`, mirroring claude-code's
//!   `normalizeNameForMCP(client.name) === serverName`), which keeps the raw
//!   `config.name` for `/mcp` display while resolving a model-supplied
//!   normalized token.
//!
//! Individual `mcp__server__tool` entries ARE advertised to the model: the
//! composition root builds a per-tool `tool_mcp::MCPTool` for every discovered
//! tool (`tool_mcp::build_registered_mcp_tools`) and registers them into the
//! wire `ToolRegistry` (`engine-desktop` `register_mcp_tools`), so
//! `ConversationOrchestrator::build_wire_tools` serializes each tool under its
//! normalized FQN. The model-facing FQN normalizes BOTH the server AND the tool
//! segment (matching claude-code `buildMcpToolName`, `client.ts:1768`); the raw
//! wire tool name is kept on [`traits::McpToolDto::tool_name`] and recovered for
//! dispatch by [`crate::registry::McpRegistry::resolve_wire_tool_name`].

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
