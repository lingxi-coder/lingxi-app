//! Shell permission-rule matching — faithful port of claude-code
//! `src/utils/permissions/shellRuleMatching.ts`.
//!
//! A `Bash(...)`/`PowerShell(...)` rule's CONTENT is parsed into one of three
//! shapes and matched against a (already split + stripped) shell command:
//!
//! - **prefix** — legacy `npm:*` syntax → matches `npm` exactly or any command
//!   starting `npm ` (word boundary).
//! - **wildcard** — contains an unescaped `*` (and does NOT end in `:*`) →
//!   glob-style match where `*` ⇒ `.*`. `\*` matches a literal asterisk and
//!   `\\` a literal backslash.
//! - **exact** — everything else → byte-exact command match.
//!
//! The higher-level deny/ask/allow aggregation over compound commands lives in
//! [`crate::shell_command`]; this module is the pure per-pattern matcher.

use regex::Regex;

/// Null-byte sentinels for escaped wildcard/backslash, mirroring the TS
/// `ESCAPED_STAR_PLACEHOLDER` / `ESCAPED_BACKSLASH_PLACEHOLDER`. Null bytes can
/// never appear in a real command, so they are collision-free placeholders.
const ESCAPED_STAR_PLACEHOLDER: &str = "\u{0}ESCAPED_STAR\u{0}";
const ESCAPED_BACKSLASH_PLACEHOLDER: &str = "\u{0}ESCAPED_BACKSLASH\u{0}";

/// A parsed shell permission rule (claude-code `ShellPermissionRule`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellRule {
    /// Byte-exact command match.
    Exact(String),
    /// Legacy `prefix:*` — matches `prefix` or `prefix <args>`.
    Prefix(String),
    /// Glob pattern with unescaped `*` wildcards.
    Wildcard(String),
}

/// Extract the prefix from legacy `:*` syntax (`"npm:*"` → `Some("npm")`,
/// `"npm"` → `None`). Faithful to `permissionRuleExtractPrefix` (regex
/// `^(.+):\*$` — the prefix is non-empty and greedy).
#[must_use]
pub fn permission_rule_extract_prefix(rule: &str) -> Option<String> {
    let stripped = rule.strip_suffix(":*")?;
    if stripped.is_empty() {
        // `^(.+)` requires at least one char before `:*`.
        return None;
    }
    Some(stripped.to_string())
}

/// Does `pattern` contain an UNESCAPED `*` that is not the legacy `:*` suffix?
/// Faithful to `hasWildcards`: a trailing `:*` is prefix syntax (returns
/// `false`); otherwise a `*` preceded by an even number of backslashes (incl.
/// zero) is unescaped.
#[must_use]
pub fn has_wildcards(pattern: &str) -> bool {
    if pattern.ends_with(":*") {
        return false;
    }
    // A `*` is unescaped iff preceded by an even number of backslashes.
    let mut backslashes = 0usize;
    for c in pattern.chars() {
        match c {
            '\\' => backslashes += 1,
            '*' => {
                if backslashes % 2 == 0 {
                    return true;
                }
                backslashes = 0;
            }
            _ => backslashes = 0,
        }
    }
    false
}

/// Parse a rule-content string into a [`ShellRule`] (claude-code
/// `parsePermissionRule`): legacy `:*` prefix first, then wildcard, else exact.
#[must_use]
pub fn parse_shell_rule(rule: &str) -> ShellRule {
    if let Some(prefix) = permission_rule_extract_prefix(rule) {
        return ShellRule::Prefix(prefix);
    }
    if has_wildcards(rule) {
        return ShellRule::Wildcard(rule.to_string());
    }
    ShellRule::Exact(rule.to_string())
}

/// Match `command` against a wildcard `pattern` where `*` matches any sequence
/// (dotAll — `*` spans embedded newlines), `\*` a literal `*`, and `\\` a
/// literal `\`. Faithful to `matchWildcardPattern`.
///
/// Special case: when the pattern ends in ` *` (space + the ONLY unescaped
/// wildcard), the trailing space-and-args is made optional so `git *` matches
/// both `git add` and bare `git` (aligning with `git:*` prefix semantics).
#[must_use]
pub fn match_wildcard_pattern(pattern: &str, command: &str, case_insensitive: bool) -> bool {
    let trimmed = pattern.trim();

    // Phase 1: replace escape sequences `\*` and `\\` with placeholders so the
    // regex-escaping pass below leaves them alone.
    let mut processed = String::with_capacity(trimmed.len());
    let chars: Vec<char> = trimmed.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let next = chars[i + 1];
            match next {
                '*' => {
                    processed.push_str(ESCAPED_STAR_PLACEHOLDER);
                    i += 2;
                    continue;
                }
                '\\' => {
                    processed.push_str(ESCAPED_BACKSLASH_PLACEHOLDER);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        processed.push(c);
        i += 1;
    }

    // Count unescaped `*` (the placeholders no longer contain a bare `*`).
    let unescaped_star_count = processed.matches('*').count();

    // Phase 2: escape regex metacharacters EXCEPT `*`. The TS set also escapes
    // `'"` but those are not Rust-regex metacharacters (and `\'` is an illegal
    // Rust escape), so we escape only the genuine metacharacters — behaviorally
    // identical since quotes are literals either way.
    let mut escaped = String::with_capacity(processed.len());
    for c in processed.chars() {
        if matches!(
            c,
            '.' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(c);
    }

    // Phase 3: unescaped `*` → `.*`.
    let with_wildcards = escaped.replace('*', ".*");

    // Phase 4: placeholders → literal-regex forms. The placeholders were escaped
    // in phase 2 only via their (absent) metacharacters; the literal text
    // `\u{0}ESCAPED_STAR\u{0}` survives intact, so replace it wholesale.
    let mut regex_pattern = with_wildcards
        .replace(ESCAPED_STAR_PLACEHOLDER, "\\*")
        .replace(ESCAPED_BACKSLASH_PLACEHOLDER, "\\\\");

    // Phase 5: trailing ` *` (now ` .*`) as the sole unescaped wildcard → make
    // the space-and-args optional.
    if regex_pattern.ends_with(" .*") && unescaped_star_count == 1 {
        let base_len = regex_pattern.len() - 3;
        regex_pattern.truncate(base_len);
        regex_pattern.push_str("( .*)?");
    }

    // Phase 6: anchor + dotAll (+ optional case-insensitive) and test.
    let flags = if case_insensitive { "(?si)" } else { "(?s)" };
    let full = format!("^{flags}{regex_pattern}$");
    match Regex::new(&full) {
        Ok(re) => re.is_match(command),
        Err(e) => {
            // A malformed user pattern must not crash the permission check; a
            // pattern that won't compile simply matches nothing.
            tracing::warn!(pattern, error = %e, "unparseable shell wildcard rule; treated as no-match");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_prefix() {
        assert_eq!(permission_rule_extract_prefix("npm:*"), Some("npm".into()));
        assert_eq!(
            permission_rule_extract_prefix("git push:*"),
            Some("git push".into())
        );
        assert_eq!(permission_rule_extract_prefix("npm"), None);
        assert_eq!(permission_rule_extract_prefix(":*"), None); // needs ≥1 char
    }

    #[test]
    fn wildcards_detection() {
        assert!(!has_wildcards("npm:*")); // legacy prefix, not wildcard
        assert!(!has_wildcards("npm")); // no star
        assert!(has_wildcards("git *"));
        assert!(has_wildcards("* run *"));
        assert!(!has_wildcards("foo\\*bar")); // escaped star
        assert!(has_wildcards("foo\\\\*bar")); // escaped backslash then live star
    }

    #[test]
    fn parse_dispatches() {
        assert_eq!(parse_shell_rule("npm:*"), ShellRule::Prefix("npm".into()));
        assert_eq!(
            parse_shell_rule("git *"),
            ShellRule::Wildcard("git *".into())
        );
        assert_eq!(
            parse_shell_rule("npm run build"),
            ShellRule::Exact("npm run build".into())
        );
    }

    #[test]
    fn wildcard_trailing_star_optional_args() {
        // `git *` matches both `git add` and bare `git`.
        assert!(match_wildcard_pattern("git *", "git add", false));
        assert!(match_wildcard_pattern("git *", "git", false));
        assert!(match_wildcard_pattern(
            "git *",
            "git push origin main",
            false
        ));
        assert!(!match_wildcard_pattern("git *", "gitk", false)); // word boundary
    }

    #[test]
    fn wildcard_single_star_trailing_optional() {
        // ONE unescaped star at the end → trailing-optional applies, so
        // `npm run *` matches bare `npm run` (claude-code unescapedStarCount===1).
        assert!(match_wildcard_pattern("npm run *", "npm run build", false));
        assert!(match_wildcard_pattern("npm run *", "npm run", false));
    }

    #[test]
    fn wildcard_multi_star_no_optional() {
        // TWO unescaped stars → trailing-optional rule does NOT apply, so a
        // trailing argument is required after ` run `.
        assert!(match_wildcard_pattern("* run *", "npm run build", false));
        assert!(!match_wildcard_pattern("* run *", "npm run", false));
    }

    #[test]
    fn wildcard_mid_pattern() {
        assert!(match_wildcard_pattern(
            "docker * ps",
            "docker -H x ps",
            false
        ));
        assert!(!match_wildcard_pattern(
            "docker * ps",
            "docker ps now",
            false
        ));
    }

    #[test]
    fn wildcard_literal_star_and_backslash() {
        // `\*` matches a literal asterisk, not a wildcard.
        assert!(match_wildcard_pattern("echo \\*", "echo *", false));
        assert!(!match_wildcard_pattern("echo \\*", "echo hello", false));
    }

    #[test]
    fn wildcard_dotall_spans_newlines() {
        assert!(match_wildcard_pattern("bash *", "bash -c 'a\nb'", false));
    }

    #[test]
    fn wildcard_case_insensitive_opt_in() {
        assert!(match_wildcard_pattern("git *", "GIT add", true));
        assert!(!match_wildcard_pattern("git *", "GIT add", false));
    }

    #[test]
    fn wildcard_escapes_regex_metachars() {
        // `.` in the pattern is a literal dot, not "any char".
        assert!(match_wildcard_pattern("cat a.txt", "cat a.txt", false));
        assert!(!match_wildcard_pattern("cat a.txt", "cat axtxt", false));
    }
}
