//! Hook matcher — byte-faithful port of `matchesPattern` (claude-code
//! `src/utils/hooks.ts:1346-1381`) and the legacy tool-name helpers it relies
//! on (`src/utils/permissions/permissionRuleParser.ts:31-41`).
//!
//! A hook's `matcher` (the `Pre/PostToolUse` tool-name pattern, e.g.
//! `"Write|Edit"`, `"^Bash$"`, `".*"`) gates whether the hook fires for a
//! given tool invocation. [`matches_pattern`] reproduces the three TS branches
//! exactly:
//!
//! 1. `*` / empty → match everything (short-circuit `true`).
//! 2. `^[A-Za-z0-9_|]+$` → a "simple" pattern with no regex metacharacters
//!    other than `|`: an exact match, or — when it contains `|` — a match
//!    against any pipe-separated segment (each `trim()`-med + legacy-normalized).
//! 3. Otherwise → treat the pattern as an anchored-or-not regex. An invalid
//!    regex logs and returns `false` (never panics), exactly like the TS
//!    `try { new RegExp(...) } catch { … return false }`.
//!
//! The `if`-condition (permission-rule content) matching is intentionally NOT
//! ported here — see [`crate::registry::HookRegistry::match_event`] for the gap
//! note. This module is the MATCHER-only half of B3.

use regex::Regex;

/// Maps a legacy tool name to its canonical name — `normalizeLegacyToolName`
/// (`permissionRuleParser.ts:21-33`): `LEGACY_TOOL_NAME_ALIASES[name] ?? name`.
///
/// The external (non-ant) alias table mirrors the canonical copy in
/// `permission::rule::normalize_legacy_tool_name` (kept in sync; both port the
/// same TS source). The KAIROS-gated `Brief → SendUserMessage` alias is omitted
/// in external builds, matching `permission`.
#[must_use]
pub fn normalize_legacy_tool_name(name: &str) -> String {
    match name {
        "Task" => "Agent",
        "KillShell" => "TaskStop",
        "AgentOutputTool" | "BashOutputTool" => "TaskOutput",
        other => other,
    }
    .to_string()
}

/// Returns the legacy aliases that map to `canonical_name` — the reverse of
/// [`normalize_legacy_tool_name`], mirroring `getLegacyToolNames`
/// (`permissionRuleParser.ts:35-41`): every legacy key whose canonical value
/// equals `canonical_name`, in `LEGACY_TOOL_NAME_ALIASES` insertion order. Used
/// by [`matches_pattern`]'s regex branch so a pattern like `^Task$` still
/// matches the canonical tool `Agent`.
#[must_use]
pub fn get_legacy_tool_names(canonical_name: &str) -> Vec<String> {
    match canonical_name {
        "Agent" => vec!["Task".to_string()],
        "TaskStop" => vec!["KillShell".to_string()],
        "TaskOutput" => vec!["AgentOutputTool".to_string(), "BashOutputTool".to_string()],
        _ => Vec::new(),
    }
}

/// Returns `true` if `matcher` matches `match_query`.
///
/// Byte-faithful port of `matchesPattern` (`src/utils/hooks.ts:1346-1381`).
///
/// * `match_query` — the value derived from the event (the tool name for
///   tool events).
/// * `matcher` — the hook's declared pattern.
#[must_use]
pub fn matches_pattern(match_query: &str, matcher: &str) -> bool {
    // TS: `if (!matcher || matcher === '*') return true`
    if matcher.is_empty() || matcher == "*" {
        return true;
    }

    // TS: `if (/^[a-zA-Z0-9_|]+$/.test(matcher))` — a simple string or
    // pipe-separated list with no regex specials other than `|`.
    if is_simple_pattern(matcher) {
        // TS: `if (matcher.includes('|'))` — pipe-separated exact matches.
        if matcher.contains('|') {
            return matcher
                .split('|')
                .map(|p| normalize_legacy_tool_name(p.trim()))
                .any(|p| p == match_query);
        }
        // TS: `return matchQuery === normalizeLegacyToolName(matcher)`
        return match_query == normalize_legacy_tool_name(matcher);
    }

    // TS: otherwise treat as regex.
    let Ok(regex) = Regex::new(matcher) else {
        // TS: `catch { logForDebugging(...); return false }` — an invalid
        // regex never panics, it just logs and fails to match.
        tracing::debug!(matcher = %matcher, "Invalid regex pattern in hook matcher");
        return false;
    };
    if regex.is_match(match_query) {
        return true;
    }
    // TS: also test against legacy names so patterns like "^Task$" still match
    // the canonical name (e.g. query "Agent" → legacy ["Task"]).
    for legacy_name in get_legacy_tool_names(match_query) {
        if regex.is_match(&legacy_name) {
            return true;
        }
    }
    false
}

/// True when every byte of `s` is in `[A-Za-z0-9_|]` and `s` is non-empty —
/// the Rust equivalent of the JS `/^[a-zA-Z0-9_|]+$/` test.
fn is_simple_pattern(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'|')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_and_empty_match_everything() {
        // `*` and empty short-circuit to true regardless of query.
        assert!(matches_pattern("Write", "*"));
        assert!(matches_pattern("Bash", "*"));
        assert!(matches_pattern("anything", ""));
        assert!(matches_pattern("", ""));
    }

    #[test]
    fn simple_exact_match() {
        assert!(matches_pattern("Write", "Write"));
        assert!(!matches_pattern("Bash", "Write"));
        assert!(!matches_pattern("Writer", "Write"));
        assert!(!matches_pattern("Writ", "Write"));
    }

    #[test]
    fn pipe_separated_exact_match() {
        assert!(matches_pattern("Write", "Write|Edit"));
        assert!(matches_pattern("Edit", "Write|Edit"));
        assert!(!matches_pattern("Bash", "Write|Edit"));
        // Three-way pipe list.
        assert!(matches_pattern("Read", "Write|Edit|Read"));
        // A matcher containing spaces is NOT a simple pattern (`^[A-Za-z0-9_|]+$`
        // rejects the space), so it falls through to the regex branch — same as
        // TS. There it is the alternation `Write ` | ` Edit`, neither of which
        // is a substring of the bare query "Edit", so it does not match.
        assert!(!matches_pattern("Edit", "Write | Edit"));
    }

    #[test]
    fn regex_branch_anchored_and_unanchored() {
        // `^Bash.*` is not a simple pattern (contains `^` and `.` and `*`),
        // so it falls through to the regex branch.
        assert!(matches_pattern("Bash", "^Bash.*"));
        assert!(matches_pattern("BashOutput", "^Bash.*"));
        assert!(!matches_pattern("MultiBash", "^Bash.*"));
        // A pattern with a `.` is NOT simple, so it hits the regex branch and
        // matches unanchored (substring), like JS `RegExp.test`.
        assert!(matches_pattern("MultiBash", "Bash.*"));
        assert!(matches_pattern("BashX", "Bash.*"));
        // But a plain "Bash" is a SIMPLE pattern → exact match, not substring:
        // it must NOT match "MultiBash".
        assert!(!matches_pattern("MultiBash", "Bash"));
    }

    #[test]
    fn regex_anchored_full_word() {
        assert!(matches_pattern("Task", "^Task$"));
        assert!(!matches_pattern("TaskOutput", "^Task$"));
        assert!(!matches_pattern("MyTask", "^Task$"));
    }

    #[test]
    fn invalid_regex_returns_false_no_panic() {
        // Unbalanced bracket / paren — invalid regex must NOT panic.
        assert!(!matches_pattern("Bash", "^Bash[")); // contains `[` -> regex branch
        assert!(!matches_pattern("Bash", "(unclosed"));
        assert!(!matches_pattern("Bash", "a{2,1}")); // invalid quantifier range
    }

    #[test]
    fn legacy_aliases_resolve_to_canonical() {
        // Forward map (permissionRuleParser.ts LEGACY_TOOL_NAME_ALIASES).
        assert_eq!(normalize_legacy_tool_name("Task"), "Agent");
        assert_eq!(normalize_legacy_tool_name("KillShell"), "TaskStop");
        assert_eq!(normalize_legacy_tool_name("AgentOutputTool"), "TaskOutput");
        assert_eq!(normalize_legacy_tool_name("BashOutputTool"), "TaskOutput");
        // Non-legacy names pass through unchanged.
        assert_eq!(normalize_legacy_tool_name("Write"), "Write");
        assert_eq!(normalize_legacy_tool_name("Agent"), "Agent");
        // Reverse map (insertion order matters for the two-alias TaskOutput).
        assert_eq!(get_legacy_tool_names("Agent"), vec!["Task".to_string()]);
        assert_eq!(get_legacy_tool_names("TaskStop"), vec!["KillShell".to_string()]);
        assert_eq!(
            get_legacy_tool_names("TaskOutput"),
            vec!["AgentOutputTool".to_string(), "BashOutputTool".to_string()]
        );
        assert!(get_legacy_tool_names("Write").is_empty());
    }

    #[test]
    fn legacy_matcher_resolves_to_canonical_tool() {
        // A simple "Task" matcher now matches the canonical tool "Agent"
        // (matcher normalizes to "Agent"; matchQuery is the canonical name).
        assert!(matches_pattern("Agent", "Task"));
        // Pipe lists normalize each side.
        assert!(matches_pattern("TaskStop", "KillShell|Write"));
        // The regex branch falls back to the query's legacy names, so "^Task$"
        // matches the canonical "Agent".
        assert!(matches_pattern("Agent", "^Task$"));
        // Parity: the matcher is normalized but the query is not, so a literal
        // legacy "Task" query (a tool name that no longer exists) does NOT match
        // a "Task" matcher — `"Task" === normalizeLegacyToolName("Task") ("Agent")`
        // is false in TS too.
        assert!(!matches_pattern("Task", "Task"));
    }
}
