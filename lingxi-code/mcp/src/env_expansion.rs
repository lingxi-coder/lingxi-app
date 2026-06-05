//! `${VAR}` environment-variable expansion for MCP server configs.
//!
//! 1:1 port of claude-code's `services/mcp/envExpansion.ts`
//! (`expandEnvVarsInString`, lines 10-38): expand `${VAR}` and
//! `${VAR:-default}` occurrences in a string, tracking the names of any
//! variables that are neither set in the environment nor given a default.
//!
//! Faithful to the TS regex `/\$\{([^}]+)\}/g`:
//! - Only `${...}` with NON-EMPTY content (`[^}]+`) is a match; a literal
//!   `${}` is left verbatim (the regex requires at least one inner char).
//! - The captured content is split on the FIRST `:-` into `(name, default)`
//!   (TS `split(':-', 2)` — only the first separator splits, so `:-` inside a
//!   default value is preserved).
//! - A set variable expands to its value; otherwise a present default is used;
//!   otherwise the variable name is recorded in `missing_vars` and the WHOLE
//!   `${...}` match is left LITERAL in the output (TS returns `match`).
//!
//! Pure functions — the only side effect is reading the process environment
//! (mirroring TS `process.env[varName]`).

/// Result of expanding `${...}` references in a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvExpansion {
    /// The input with every resolvable `${...}` reference substituted.
    pub expanded: String,
    /// Names of `${VAR}` references that were neither set nor defaulted. Each
    /// such reference is left LITERAL in [`Self::expanded`]. Order and
    /// duplicates mirror the TS `missingVars` array (one entry per occurrence).
    pub missing_vars: Vec<String>,
}

/// Look up `name` in the process environment. Mirrors `process.env[varName]`
/// (an unset variable is `undefined` → `None` here).
fn env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Expand `${VAR}` / `${VAR:-default}` references in `value`.
///
/// 1:1 with claude-code `expandEnvVarsInString` (`envExpansion.ts:10-38`).
/// See the module docs for the exact matching semantics. Reads the process
/// environment via [`env_lookup`].
#[must_use]
pub fn expand_env_vars_in_string(value: &str) -> EnvExpansion {
    expand_env_vars_with(value, env_lookup)
}

/// [`expand_env_vars_in_string`] with an injected `lookup` (used by tests to
/// avoid mutating the shared process environment). `lookup` plays the role of
/// `process.env[varName]`: `Some(v)` for a set variable, `None` for unset.
fn expand_env_vars_with(value: &str, lookup: impl Fn(&str) -> Option<String>) -> EnvExpansion {
    let mut missing_vars: Vec<String> = Vec::new();

    // Fast path: no `${` at all → no-op (the TS `.replace` would also leave
    // the string untouched, but skipping the scan keeps the common case cheap).
    if !value.contains("${") {
        return EnvExpansion {
            expanded: value.to_string(),
            missing_vars,
        };
    }

    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0usize;
    while i < bytes.len() {
        // A match starts at `${` followed by at least one non-`}` byte and a
        // closing `}` (regex `\$\{([^}]+)\}`). Anything else copies verbatim.
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(close_rel) = find_close_brace(&bytes[i + 2..]) {
                // `close_rel` is the offset of `}` within the slice after `${`.
                // Non-empty content is guaranteed because `find_close_brace`
                // requires at least one byte before the `}` (the `[^}]+`).
                let content_start = i + 2;
                let content_end = content_start + close_rel;
                let content = &value[content_start..content_end];
                let full_match = &value[i..=content_end]; // includes `${` and `}`.

                out.push_str(&resolve(content, full_match, &lookup, &mut missing_vars));
                i = content_end + 1; // continue past the closing `}`.
                continue;
            }
        }
        // Not a match — copy this byte. `value` is UTF-8 and we only special-
        // case ASCII `$`/`{`/`}`, so byte indexing never splits a code point.
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&value[i..i + ch_len]);
        i += ch_len;
    }

    EnvExpansion {
        expanded: out,
        missing_vars,
    }
}

/// Resolve one `${content}` occurrence. `full_match` is the verbatim
/// `${...}` slice (returned when the variable is missing, mirroring the TS
/// `return match`). Records missing variables into `missing`.
fn resolve(
    content: &str,
    full_match: &str,
    lookup: &impl Fn(&str) -> Option<String>,
    missing: &mut Vec<String>,
) -> String {
    // TS: `const [varName, defaultValue] = varContent.split(':-', 2)`.
    // Split on the FIRST `:-` only (preserving `:-` inside the default).
    let (var_name, default_value) = match content.find(":-") {
        Some(idx) => (&content[..idx], Some(&content[idx + 2..])),
        None => (content, None),
    };

    if let Some(env_value) = lookup(var_name) {
        return env_value;
    }
    if let Some(default_value) = default_value {
        return default_value.to_string();
    }

    // Missing and no default: record + leave the literal `${...}` in place.
    missing.push(var_name.to_string());
    full_match.to_string()
}

/// Offset of the first `}` byte in `s`, requiring at least one byte before it
/// (so the captured content is non-empty, matching `[^}]+`). Returns `None`
/// when there is no `}` or the `}` is the very first byte (`${}` → no match).
fn find_close_brace(s: &[u8]) -> Option<usize> {
    let idx = s.iter().position(|&b| b == b'}')?;
    if idx == 0 {
        // Empty content (`${}`) — the `[^}]+` requires ≥1 char, so NOT a match.
        None
    } else {
        Some(idx)
    }
}

/// Length in bytes of the UTF-8 code point that starts with `first_byte`.
fn utf8_len(first_byte: u8) -> usize {
    match first_byte {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build a `lookup` closure over a fixed map (avoids touching the shared
    /// process environment, which is global + racy under parallel tests).
    fn map_lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    fn expand(value: &str, pairs: &[(&str, &str)]) -> EnvExpansion {
        expand_env_vars_with(value, map_lookup(pairs))
    }

    #[test]
    fn var_set_is_substituted() {
        let r = expand("hello ${NAME}", &[("NAME", "world")]);
        assert_eq!(r.expanded, "hello world");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn var_unset_no_default_is_left_literal_and_reported() {
        let r = expand("x=${MISSING}", &[]);
        // The `${...}` is left verbatim (TS returns `match`)…
        assert_eq!(r.expanded, "x=${MISSING}");
        // …and the missing name is reported.
        assert_eq!(r.missing_vars, vec!["MISSING".to_string()]);
    }

    #[test]
    fn default_used_when_var_unset() {
        let r = expand("port=${PORT:-8080}", &[]);
        assert_eq!(r.expanded, "port=8080");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn default_ignored_when_var_set() {
        let r = expand("port=${PORT:-8080}", &[("PORT", "9000")]);
        assert_eq!(r.expanded, "port=9000");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn empty_default_is_honored() {
        // `${VAR:-}` with an unset VAR expands to the empty string (a present
        // default, even if empty, wins over the missing-var path).
        let r = expand("a${VAR:-}b", &[]);
        assert_eq!(r.expanded, "ab");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn default_preserves_embedded_separator() {
        // `split(':-', 2)` keeps `:-` inside the default value intact.
        let r = expand("${VAR:-a:-b}", &[]);
        assert_eq!(r.expanded, "a:-b");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn multiple_refs_in_one_string() {
        let r = expand(
            "${A}/${B}/${C:-def}/${MISSING}",
            &[("A", "1"), ("B", "2")],
        );
        assert_eq!(r.expanded, "1/2/def/${MISSING}");
        assert_eq!(r.missing_vars, vec!["MISSING".to_string()]);
    }

    #[test]
    fn missing_vars_records_one_entry_per_occurrence() {
        let r = expand("${X} ${X}", &[]);
        assert_eq!(r.expanded, "${X} ${X}");
        assert_eq!(r.missing_vars, vec!["X".to_string(), "X".to_string()]);
    }

    #[test]
    fn no_op_when_no_dollar_brace() {
        let r = expand("plain string with $ and { } separately", &[("A", "x")]);
        assert_eq!(r.expanded, "plain string with $ and { } separately");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn empty_braces_are_not_a_match() {
        // `${}` has empty content (`[^}]+` requires ≥1 char) → left verbatim,
        // not reported as missing.
        let r = expand("a${}b", &[]);
        assert_eq!(r.expanded, "a${}b");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn unterminated_brace_is_left_verbatim() {
        // No closing `}` → not a match.
        let r = expand("a${UNCLOSED", &[("UNCLOSED", "x")]);
        assert_eq!(r.expanded, "a${UNCLOSED");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn unicode_around_refs_is_preserved() {
        let r = expand("café ${NAME} ☕", &[("NAME", "noir")]);
        assert_eq!(r.expanded, "café noir ☕");
        assert!(r.missing_vars.is_empty());
    }

    #[test]
    fn real_process_env_path_is_used() {
        // Smoke-test the public entry point against the real environment. A
        // never-set name must be left literal + reported.
        let r = expand_env_vars_in_string("${LINGXI_DEFINITELY_UNSET_VAR_XYZ}");
        assert_eq!(r.expanded, "${LINGXI_DEFINITELY_UNSET_VAR_XYZ}");
        assert_eq!(
            r.missing_vars,
            vec!["LINGXI_DEFINITELY_UNSET_VAR_XYZ".to_string()]
        );
    }
}
