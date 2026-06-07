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
//! The `if`-condition (permission-rule content) matcher is ported as
//! [`matches_if_condition`] — a faithful port of `prepareIfConditionMatcher`
//! (`utils/hooks.ts:1390-1422`) composed with each tool's
//! `preparePermissionMatcher` closure. It reuses the permission crate's
//! `PermissionRuleValue::from_rule_string` + per-tool content matchers, and is
//! wired alongside the tool-name matcher in
//! [`crate::registry::HookRegistry::match_event`].

use permission::shell_command::{command_from_input, rule_matches_any_subcommand};
use permission::shell_rule_matching::match_wildcard_pattern;
use permission::PermissionRuleValue;
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

/// Returns `true` if a hook's `if`-condition `if_condition` (a permission-rule
/// string such as `"Bash(git push:*)"`) matches the current tool invocation.
///
/// Byte-faithful port of `prepareIfConditionMatcher`'s returned closure
/// (`utils/hooks.ts:1404-1421`) composed with each tool's
/// `preparePermissionMatcher` (`BashTool.tsx:445`, `FileEditTool.ts:122`,
/// `FileReadTool.ts:395`, `FileWriteTool.ts:132`, `GlobTool.ts:91`,
/// `GrepTool.ts:198`):
///
/// 1. Parse `if_condition` into `{ tool_name, rule_content }` via
///    [`PermissionRuleValue::from_rule_string`] (`permissionRuleValueFromString`).
/// 2. If the rule's (legacy-normalized) tool name differs from the event's
///    (legacy-normalized) `tool_name` → no match.
/// 3. If the rule carries no content (bare `Tool`, or `Tool()` / `Tool(*)`) →
///    match — tool-name agreement alone (`if (!parsed.ruleContent) return true`).
/// 4. Otherwise dispatch to the tool's content matcher. Only the six tools that
///    implement `preparePermissionMatcher` in claude-code can match a content
///    rule; for every other tool (and an input missing the matched field) the
///    TS `patternMatcher` is `undefined`, so the closure returns `false`.
///
/// `tool_name` is the event's tool (canonicalized internally);
/// `tool_input` is the tool's raw JSON input (`hookInput.tool_input`).
///
/// ## Documented divergences (reused permission-crate matchers; safe direction)
/// The per-tool content match reuses the permission crate's matchers rather than
/// re-deriving each tool's closure:
/// - **Bash** uses [`rule_matches_any_subcommand`] — the crate's deny-like
///   "fires if ANY subcommand matches" aggregation, which is exactly the
///   semantics the TS Bash matcher targets ("compound commands must fire the
///   hook if ANY subcommand matches"). It strips safe wrappers / env-var
///   prefixes and uses the quote-aware delimiter splitter (the crate's
///   documented stand-in for tree-sitter `parseForSecurity`); all such
///   differences make the hook fire MORE readily (the deny-safe direction the
///   TS comment intends), never less, and the `prefix` / `wildcard` / exact
///   match rules are identical.
/// - **File / Glob / Grep** use [`match_wildcard_pattern`] (the port of
///   `matchWildcardPattern`) against the raw `file_path` / search `pattern`
///   string — byte-identical to TS (anchored wildcard, NOT a root-relative
///   gitignore glob).
#[must_use]
pub fn matches_if_condition(
    if_condition: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> bool {
    let parsed = PermissionRuleValue::from_rule_string(if_condition);
    // `from_rule_string` already legacy-normalizes the parsed tool name; the
    // event tool name is normalized here so the comparison matches TS's
    // `normalizeLegacyToolName(parsed.toolName) === normalizeLegacyToolName(toolName)`.
    let canonical_tool = normalize_legacy_tool_name(tool_name);
    if parsed.tool_name != canonical_tool {
        return false;
    }
    // Bare tool name / `Tool()` / `Tool(*)` → tool-name agreement is enough.
    let Some(content) = parsed.rule_content.as_deref() else {
        return true;
    };
    matches_content_rule(&canonical_tool, content, tool_input)
}

/// The per-tool content matcher — the body of each tool's
/// `preparePermissionMatcher` closure. `tool_name` is already
/// legacy-normalized; `content` is the rule's parenthesized content.
fn matches_content_rule(tool_name: &str, content: &str, input: &serde_json::Value) -> bool {
    match tool_name {
        // BashTool: split into subcommands, fire if ANY matches the content
        // (prefix / wildcard / exact). Missing `command` field ⇒ TS input
        // schema parse fails ⇒ `patternMatcher` undefined ⇒ false.
        "Bash" => {
            command_from_input(input).is_some_and(|cmd| rule_matches_any_subcommand(content, cmd))
        }
        // File tools match the content as a wildcard against the RAW `file_path`
        // string (`matchWildcardPattern(pattern, file_path)`).
        "Edit" | "Write" | "Read" => {
            string_field(input, "file_path").is_some_and(|p| match_wildcard_pattern(content, p, false))
        }
        // Glob / Grep match against the SEARCH `pattern` field, not a path.
        "Glob" | "Grep" => {
            string_field(input, "pattern").is_some_and(|p| match_wildcard_pattern(content, p, false))
        }
        // Every other tool (PowerShell, NotebookEdit, MCP, …) has no
        // `preparePermissionMatcher` in claude-code, so a content rule cannot
        // match (TS `patternMatcher` is `undefined` → the closure returns false).
        _ => false,
    }
}

/// Extract a string field from a tool's JSON input, or `None` when absent /
/// non-string (mirrors a TS input-schema parse failure for that field).
fn string_field<'a>(input: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    input.get(field).and_then(serde_json::Value::as_str)
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

    // ── `if`-condition matcher (prepareIfConditionMatcher + per-tool) ──────────

    use serde_json::json;

    #[test]
    fn if_bash_prefix_rule_fires_on_matching_command() {
        let input = json!({ "command": "git push origin main" });
        assert!(matches_if_condition("Bash(git push:*)", "Bash", &input));
    }

    #[test]
    fn if_bash_prefix_rule_does_not_fire_on_non_matching_command() {
        let input = json!({ "command": "git status" });
        assert!(!matches_if_condition("Bash(git push:*)", "Bash", &input));
        // A different command entirely.
        let ls = json!({ "command": "ls -la" });
        assert!(!matches_if_condition("Bash(git push:*)", "Bash", &ls));
    }

    #[test]
    fn if_bash_fires_when_any_subcommand_matches() {
        // Deny-like aggregation: a compound command fires the hook if ANY
        // subcommand matches (BashTool.tsx comment).
        let input = json!({ "command": "echo ok && git push" });
        assert!(matches_if_condition("Bash(git push:*)", "Bash", &input));
        // None of the subcommands match → no fire.
        let benign = json!({ "command": "echo ok && ls" });
        assert!(!matches_if_condition("Bash(git push:*)", "Bash", &benign));
    }

    #[test]
    fn if_bash_wildcard_rule() {
        let input = json!({ "command": "git commit -m x" });
        assert!(matches_if_condition("Bash(git *)", "Bash", &input));
        // word boundary: `git *` must not match `gitk`.
        let gitk = json!({ "command": "gitk" });
        assert!(!matches_if_condition("Bash(git *)", "Bash", &gitk));
    }

    #[test]
    fn if_bare_tool_name_matches_on_tool_agreement_alone() {
        // No rule content → matches purely on the tool name (TS `!parsed.ruleContent`).
        let input = json!({ "command": "rm -rf /" });
        assert!(matches_if_condition("Bash", "Bash", &input));
        // `Bash()` and `Bash(*)` collapse to tool-wide too.
        assert!(matches_if_condition("Bash()", "Bash", &input));
        assert!(matches_if_condition("Bash(*)", "Bash", &input));
    }

    #[test]
    fn if_tool_name_mismatch_does_not_fire() {
        // The rule's tool name must equal the event's tool name.
        let input = json!({ "command": "git push" });
        assert!(!matches_if_condition("Bash(git push:*)", "Write", &input));
        // Bare-tool rule for a different tool also fails.
        assert!(!matches_if_condition("Read", "Bash", &input));
    }

    #[test]
    fn if_legacy_tool_name_normalizes_both_sides() {
        // Legacy `Task` rule normalizes to canonical `Agent`; event tool `Agent`
        // matches. (Bare tool name → tool-agreement match.)
        let input = json!({ "description": "x" });
        assert!(matches_if_condition("Task", "Agent", &input));
        assert!(matches_if_condition("Task(*)", "Agent", &input));
    }

    #[test]
    fn if_malformed_rule_does_not_fire() {
        // Unbalanced paren: `from_rule_string` treats the WHOLE string as a bare
        // tool name (`"Bash(git push:*"`), which != "Bash" → no fire. Mirrors TS
        // `permissionRuleValueFromString` producing toolName === the whole string.
        let input = json!({ "command": "git push" });
        assert!(!matches_if_condition("Bash(git push:*", "Bash", &input));
        // Content after the close paren → bare tool name `"Bash(x)y"` != "Bash".
        assert!(!matches_if_condition("Bash(x)y", "Bash", &input));
    }

    #[test]
    fn if_file_tool_wildcards_match_raw_path() {
        // matchWildcardPattern against the raw file_path (anchored wildcard).
        let input = json!({ "file_path": "/proj/src/main.rs" });
        assert!(matches_if_condition("Edit(*main.rs)", "Edit", &input));
        assert!(matches_if_condition("Edit(/proj/src/*)", "Edit", &input));
        assert!(matches_if_condition("Write(/proj/*)", "Write", &input));
        assert!(matches_if_condition("Read(/proj/src/main.rs)", "Read", &input));
        // Non-matching path.
        assert!(!matches_if_condition("Edit(*.py)", "Edit", &input));
        // A bare unanchored relative pattern does NOT match an absolute path
        // (faithful to anchored `^src/.*$`).
        assert!(!matches_if_condition("Edit(src/*)", "Edit", &input));
    }

    #[test]
    fn if_glob_grep_match_search_pattern_not_path() {
        // Glob/Grep match the rule against the SEARCH `pattern` field.
        let glob = json!({ "pattern": "**/*.rs", "path": "/proj" });
        assert!(matches_if_condition("Glob(**/*.rs)", "Glob", &glob));
        let grep = json!({ "pattern": "TODO", "path": "/proj" });
        assert!(matches_if_condition("Grep(TODO)", "Grep", &grep));
        assert!(!matches_if_condition("Grep(FIXME)", "Grep", &grep));
    }

    #[test]
    fn if_content_rule_on_unmatchable_tool_does_not_fire() {
        // PowerShell / NotebookEdit have no preparePermissionMatcher in TS, so a
        // CONTENT rule never matches (patternMatcher undefined → false). The bare
        // tool-name form still matches on tool agreement.
        let ps = json!({ "command": "Get-ChildItem" });
        assert!(!matches_if_condition("PowerShell(Get-ChildItem:*)", "PowerShell", &ps));
        assert!(matches_if_condition("PowerShell", "PowerShell", &ps));
        let nb = json!({ "notebook_path": "/a.ipynb" });
        assert!(!matches_if_condition("NotebookEdit(/a.ipynb)", "NotebookEdit", &nb));
        assert!(matches_if_condition("NotebookEdit", "NotebookEdit", &nb));
    }

    #[test]
    fn if_content_rule_with_missing_input_field_does_not_fire() {
        // Missing the matched field ⇒ TS input-schema parse fails ⇒ undefined
        // matcher ⇒ false. (A bare tool-name rule still matches.)
        let empty = json!({});
        assert!(!matches_if_condition("Bash(git push:*)", "Bash", &empty));
        assert!(!matches_if_condition("Edit(*.rs)", "Edit", &empty));
        assert!(matches_if_condition("Bash", "Bash", &empty));
    }
}
