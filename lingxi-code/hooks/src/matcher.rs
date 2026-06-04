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

/// Maps a legacy tool name to its canonical name.
///
/// In claude-code this consults `LEGACY_TOOL_NAME_ALIASES`
/// (`permissionRuleParser.ts:21-33`), e.g. `Task → Agent`. Those aliases have
/// NOT been ported into the Rust tree yet, so per the B3 spec this is a
/// **no-op pass-through**: the input name is returned unchanged. The function
/// exists so the call sites mirror the TS structure 1:1; when the alias table
/// lands, only this body changes.
#[must_use]
pub fn normalize_legacy_tool_name(name: &str) -> String {
    name.to_string()
}

/// Returns the legacy aliases that map to `canonical_name`.
///
/// Mirrors `getLegacyToolNames` (`permissionRuleParser.ts:35-41`). Because the
/// alias table is not yet ported (see [`normalize_legacy_tool_name`]), this
/// always returns an empty list — so the regex branch's legacy fallback loop
/// in [`matches_pattern`] iterates over nothing, identical to running the TS
/// with an empty `LEGACY_TOOL_NAME_ALIASES`.
#[must_use]
pub fn get_legacy_tool_names(_canonical_name: &str) -> Vec<String> {
    Vec::new()
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
    // the canonical name. (Empty until the alias table is ported.)
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
    fn legacy_normalize_is_passthrough() {
        // No-op pass-through until alias table lands.
        assert_eq!(normalize_legacy_tool_name("Task"), "Task");
        assert_eq!(normalize_legacy_tool_name("Write"), "Write");
        assert!(get_legacy_tool_names("Agent").is_empty());
        // Because normalization is a no-op, a simple "Task" matcher matches
        // exactly the literal "Task" query and nothing else.
        assert!(matches_pattern("Task", "Task"));
        assert!(!matches_pattern("Agent", "Task"));
    }
}
