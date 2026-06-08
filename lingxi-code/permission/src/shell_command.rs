//! Shell command ↔ permission-rule evaluation — faithful port of the CORE of
//! claude-code `src/tools/BashTool/bashPermissions.ts`
//! (`filterRulesByContentsMatchingInput` + the compound aggregation in
//! `bashToolCheckPermission` / `bashToolHasPermission`).
//!
//! A shell command is split into subcommands (on `&&`, `||`, `;`, `;;`, `|`,
//! quote-aware) and each subcommand is matched against rule CONTENT via
//! [`crate::shell_rule_matching`]. The two aggregations differ by design:
//!
//! - **deny / ask** — the command matches a rule if ANY subcommand matches,
//!   after AGGRESSIVE stripping (safe wrappers + ALL leading env-var prefixes).
//!   A denied command must stay denied even when wrapped (`FOO=bar denied`,
//!   `timeout 5 denied`, `echo ok && denied`). (claude-code `stripAllEnvVars:
//!   true`.)
//! - **allow** — the command is allowed only if EVERY subcommand is covered by
//!   some allow rule, after SAFE-LIST stripping only. One allow rule matching
//!   one subcommand of `echo ok && rm -rf /` must NOT allow the whole command.
//!
//! ## Deferred (documented)
//! Tree-sitter AST parsing (we use the quote-aware delimiter splitter — the
//! analogue of claude-code `splitCommand_DEPRECATED`), `checkPathConstraints`
//! (redirection-target / `cd` path validation, heredoc extraction), sed/mode/
//! read-only validation, and the ANT-only env-var safelist. Output-redirection
//! stripping is best-effort here (used only to let `Bash(python:*)` match
//! `python x.py > out`); full quote-aware redirection parsing lives with the
//! deferred path-constraint check. None of the deferrals can WEAKEN a deny —
//! they only make matching more conservative (fall through to ask).
//!
//! ## Intentional divergence (safe direction)
//! `strip_all_leading_env_vars`' bare-value class admits `$`/backtick, whereas
//! claude-code's `ENV_VAR_PATTERN` excludes them (a documented low-priority TS
//! gap). The effect is the deny/ask path strips `FOO=$(x) denied` down to
//! `denied` and still denies it — STRICTER than TS, never a bypass. Kept
//! deliberately rather than replicating the TS weakness in a safety boundary.

use crate::shell_rule_matching::{parse_shell_rule, ShellRule};
use regex::Regex;
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// Extract the `command` string from a shell tool's JSON input.
#[must_use]
pub fn command_from_input(input: &serde_json::Value) -> Option<&str> {
    input.get("command").and_then(serde_json::Value::as_str)
}

/// Is this a shell tool whose content rules are command patterns?
#[must_use]
pub fn is_shell_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Bash" | "PowerShell")
}

/// Split `command` into subcommands on the shell list separators
/// `&&`, `||`, `;;`, `;`, `|` and a bare newline — quote- and escape-aware so
/// separators inside `'...'` / `"..."` or after `\` are NOT split points. The
/// analogue of claude-code `splitCommand_DEPRECATED` (likewise quote-aware and
/// newline-splitting via its tokenizer). Background `&` is NOT a separator
/// (matches `COMMAND_LIST_SEPARATORS`).
///
/// A bash newline is a command separator UNLESS it is a `\`-continuation. The
/// escape arm below consumes `\<newline>` as a pair (odd backslash =
/// continuation → joined), while a bare or even-backslash-preceded `\n` reaches
/// the separator arm — reproducing claude-code's continuation rule. Lines that
/// are entirely `#` comments are dropped (claude-code `stripCommentLines`).
#[must_use]
pub fn split_command(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    while i < chars.len() {
        let c = chars[i];
        if in_single {
            cur.push(c);
            if c == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            cur.push(c);
            if c == '\\' && i + 1 < chars.len() {
                // In double quotes a backslash escapes the next char; keep both.
                cur.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        match c {
            '\\' if i + 1 < chars.len() => {
                cur.push(c);
                cur.push(chars[i + 1]);
                i += 2;
                continue;
            }
            '\'' => {
                in_single = true;
                cur.push(c);
            }
            '"' => {
                in_double = true;
                cur.push(c);
            }
            _ => {
                // Two-char separators first.
                let two = if i + 1 < chars.len() {
                    Some((c, chars[i + 1]))
                } else {
                    None
                };
                match two {
                    Some(('&', '&') | ('|', '|') | (';', ';')) => {
                        parts.push(std::mem::take(&mut cur));
                        i += 2;
                        continue;
                    }
                    _ if c == ';' || c == '|' || c == '\n' => {
                        parts.push(std::mem::take(&mut cur));
                        i += 1;
                        continue;
                    }
                    _ => cur.push(c),
                }
            }
        }
        i += 1;
    }
    parts.push(cur);
    parts
        .into_iter()
        .map(|s| s.trim().to_string())
        // Drop empties and full-line `#` comments (claude-code `stripCommentLines`).
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .collect()
}

/// Best-effort strip of output redirections (`> f`, `>> f`, `2>&1`, `2> f`,
/// `&> f`) so `Bash(python:*)` matches `python x.py > out.txt`. Simplified
/// relative to claude-code's quote-aware/heredoc extractor (that machinery is
/// part of the deferred path-constraint check); only affects whether an ALLOW
/// rule matches, never a deny.
///
/// Also reused by the 2c bash-safety layer ([`crate::policy`]) to feed each
/// subcommand to the injection validators with its redirect already stripped —
/// matching claude-code, where `splitCommand` yields redirect-stripped
/// subcommands before `bashCommandIsSafe` runs and `checkPathConstraints`
/// validates the redirect target separately.
pub(crate) fn strip_output_redirections(cmd: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // `N>&M`, `N>>`/`N>` followed by an optional target token, `&>`/`&>>`.
        Regex::new(r"\s*(?:[0-9]*>&[0-9-]+|&>>?|[0-9]*>>?)\s*[^\s|&;<>]*").unwrap()
    });
    re.replace_all(cmd.trim(), "").trim().to_string()
}

/// One pass of safe-wrapper stripping (`timeout N`, `nice -n N`, `nohup`,
/// `time`, `sudo --`, `sudo -E … --`, `env --`). Returns `Some(stripped)` if a
/// wrapper was removed. Faithful subset of claude-code `stripSafeWrappers`
/// (the parts that matter for rule matching; the full GNU-flag enumeration is
/// not needed to keep deny safe).
fn strip_safe_wrappers(cmd: &str) -> Option<String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    // sudo -E [KEY=val ...] -- REST  → "KEY=val ... REST" (env-vars retained so
    // the env stripper can take a later pass).
    if tokens.len() >= 4 && tokens[0] == "sudo" && tokens[1] == "-E" {
        let mut i = 2;
        while i < tokens.len() && tokens[i] != "--" {
            if !tokens[i].contains('=') {
                break;
            }
            i += 1;
        }
        if i < tokens.len() && tokens[i] == "--" && i + 1 < tokens.len() {
            let mut rest: Vec<&str> = Vec::new();
            rest.extend_from_slice(&tokens[2..i]);
            rest.extend_from_slice(&tokens[i + 1..]);
            return Some(rest.join(" "));
        }
    }
    // sudo -- REST  /  env -- REST
    if tokens.len() >= 3 && (tokens[0] == "sudo" || tokens[0] == "env") && tokens[1] == "--" {
        return Some(tokens[2..].join(" "));
    }
    // nohup REST  /  time REST
    if tokens.len() >= 2 && (tokens[0] == "nohup" || tokens[0] == "time") {
        return Some(tokens[1..].join(" "));
    }
    // timeout N REST  (optionally `timeout --signal=… N REST` reduced to numeric)
    if tokens.len() >= 3 && tokens[0] == "timeout" && is_numeric(tokens[1]) {
        return Some(tokens[2..].join(" "));
    }
    // nice -n N REST
    if tokens.len() >= 4 && tokens[0] == "nice" && tokens[1] == "-n" && is_numeric(tokens[2]) {
        return Some(tokens[3..].join(" "));
    }
    // nice REST  (no explicit increment)
    if tokens.len() >= 2 && tokens[0] == "nice" && tokens[1] != "-n" {
        return Some(tokens[1..].join(" "));
    }
    None
}

fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Strip ALL leading `KEY=value` env-var assignments (`FOO=bar denied` →
/// `denied`). Used only for deny/ask aggressive matching so a denied command
/// cannot be hidden behind an arbitrary env prefix. Returns `Some` if anything
/// was stripped.
fn strip_all_leading_env_vars(cmd: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    // KEY or KEY[idx], optional `+=`/`=`, then a (quoted or bare) value, then
    // mandatory whitespace. Mirrors the shape of claude-code's ENV_VAR_PATTERN.
    let re = RE.get_or_init(|| {
        Regex::new(r#"^([A-Za-z_][A-Za-z0-9_]*(?:\[[^\]]*\])?)\+?=(?:'[^'\n\r]*'|"(?:\\.|[^"\\\n\r])*"|[^ \t\n\r;|&()<>'"]*)[ \t]+"#).unwrap()
    });
    let mut s = cmd.to_string();
    let mut stripped_any = false;
    while let Some(m) = re.find(&s) {
        let end = m.end();
        s = s[end..].to_string();
        stripped_any = true;
    }
    if stripped_any {
        Some(s.trim().to_string())
    } else {
        None
    }
}

/// Build the set of candidate strings a single subcommand should be matched
/// against: the subcommand itself (redirections stripped) plus fixed-point
/// application of safe-wrapper stripping and — when `aggressive_env` — leading
/// env-var stripping. Mirrors `filterRulesByContentsMatchingInput`'s
/// `commandsToTry` construction.
#[must_use]
fn candidates(subcommand: &str, aggressive_env: bool) -> Vec<String> {
    // Seed with BOTH the original (redirections preserved, so an exact rule like
    // `Bash(cat x 2>&1)` can match) and the redirection-stripped form (so
    // `Bash(python:*)` matches `python x.py > out`). Mirrors claude-code's
    // two-element `commandsForMatching` (bashPermissions.ts:798-801).
    let trimmed = subcommand.trim().to_string();
    let stripped = strip_output_redirections(subcommand);
    let mut out: Vec<String> = vec![trimmed];
    let mut seen: BTreeSet<String> = out.iter().cloned().collect();
    if seen.insert(stripped.clone()) {
        out.push(stripped);
    }
    let mut start = 0;
    while start < out.len() {
        let end = out.len();
        for idx in start..end {
            let c = out[idx].clone();
            if let Some(w) = strip_safe_wrappers(&c) {
                if seen.insert(w.clone()) {
                    out.push(w);
                }
            }
            if aggressive_env {
                if let Some(e) = strip_all_leading_env_vars(&c) {
                    if seen.insert(e.clone()) {
                        out.push(e);
                    }
                }
            }
        }
        start = end;
    }
    out
}

/// Does a parsed rule match a single candidate subcommand? Faithful to the
/// per-rule arm of `filterRulesByContentsMatchingInput`.
///
/// `guard_compound` reproduces claude-code's `isCompoundCommand` second-line
/// defense (bashPermissions.ts:884-893, 923-928): for ALLOW matching a
/// prefix/wildcard rule must NOT match a candidate that is itself still
/// compound (a backslash-escaped operator can survive the first split), so a
/// hidden uncovered subcommand can't be smuggled past `command_fully_allowed`.
/// Deny/ask pass `false` (claude-code `skipCompoundCheck: true`) — a denied
/// command stays denied even when compounded.
fn rule_matches_candidate(rule: &ShellRule, candidate: &str, guard_compound: bool) -> bool {
    match rule {
        ShellRule::Exact(s) => candidate == s,
        ShellRule::Prefix(prefix) => {
            if guard_compound && split_command(candidate).len() > 1 {
                return false;
            }
            prefix_matches(prefix, candidate)
        }
        ShellRule::Wildcard(pattern) => {
            if guard_compound && split_command(candidate).len() > 1 {
                return false;
            }
            crate::shell_rule_matching::match_wildcard_pattern(pattern, candidate, false)
        }
    }
}

/// `prefix:*` word-boundary match: the candidate equals the prefix, starts with
/// `prefix ` (space boundary, so `ls:*` does NOT match `lsof`), or is the same
/// under a bare `xargs ` invocation (`Bash(grep:*)` matches `xargs grep p`).
fn prefix_matches(prefix: &str, candidate: &str) -> bool {
    if candidate == prefix || candidate.starts_with(&format!("{prefix} ")) {
        return true;
    }
    let xargs = format!("xargs {prefix}");
    candidate == xargs || candidate.starts_with(&format!("{xargs} "))
}

/// DENY/ASK aggregation: does `rule_content` match ANY subcommand of `command`
/// (with aggressive env/wrapper stripping)? A denied command stays denied even
/// when wrapped or compounded.
#[must_use]
pub fn rule_matches_any_subcommand(rule_content: &str, command: &str) -> bool {
    let rule = parse_shell_rule(rule_content);
    for sub in split_command(command) {
        for cand in candidates(&sub, true) {
            // deny/ask: no compound guard (skipCompoundCheck) — stay denied.
            if rule_matches_candidate(&rule, &cand, false) {
                return true;
            }
        }
    }
    false
}

/// ALLOW aggregation: is EVERY subcommand of `command` covered by at least one
/// of `allow_contents` (safe-list stripping only)? Returns `false` for an empty
/// command or when any subcommand is uncovered.
#[must_use]
pub fn command_fully_allowed(allow_contents: &[&str], command: &str) -> bool {
    let subs = split_command(command);
    if subs.is_empty() {
        return false;
    }
    let rules: Vec<ShellRule> = allow_contents.iter().map(|c| parse_shell_rule(c)).collect();
    subs.iter().all(|sub| {
        let cands = candidates(sub, false);
        rules.iter().any(|rule| {
            // allow: guard against a still-compound candidate over-covering.
            cands
                .iter()
                .any(|cand| rule_matches_candidate(rule, cand, true))
        })
    })
}

/// EXACT-mode ALLOW match: does the FULL trimmed command (NOT split into
/// subcommands) exactly match one of `allow_contents`? Faithful to claude-code
/// `bashToolCheckExactMatchPermission` → `matchingRulesForInput(..., 'exact')` →
/// `filterRulesByContentsMatchingInput(..., 'exact')` for the ALLOW bucket
/// (bashPermissions.ts:870-934, the `matchMode === 'exact'` arm):
///
/// - an `exact`-type rule matches iff `rule.command === candidate`;
/// - a `prefix`-type rule (`Bash(foo:*)`) matches iff `rule.prefix === candidate`
///   (the full command equals the bare prefix, no trailing args — the
///   `case 'exact': return bashRule.prefix === cmdToMatch` arm);
/// - a `wildcard`-type rule NEVER matches in exact mode (the documented
///   "SECURITY FIX: In exact match mode, wildcards must NOT match" arm).
///
/// The candidate set is the whole command + redirection-stripped + safe-wrapper
/// fixed-point (`candidates(command, false)`), i.e. the same `commandsForMatching`
/// / `commandsToTry` construction TS uses in exact mode WITHOUT `stripAllEnvVars`
/// (allow rules never strip arbitrary env prefixes). No subcommand split and no
/// compound guard — exact mode matches the unparsed command string.
#[must_use]
pub fn command_exact_allowed(allow_contents: &[&str], command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    let cands = candidates(trimmed, false);
    allow_contents.iter().any(|content| {
        let rule = parse_shell_rule(content);
        cands.iter().any(|cand| match &rule {
            // Exact rule: full-string equality (TS `bashRule.command === cmdToMatch`).
            ShellRule::Exact(s) => cand == s,
            // Prefix rule in exact mode: only the bare prefix with no args
            // (TS `bashRule.prefix === cmdToMatch`).
            ShellRule::Prefix(prefix) => cand == prefix,
            // Wildcard never matches in exact mode (TS returns false).
            ShellRule::Wildcard(_) => false,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_separators_quote_aware() {
        assert_eq!(split_command("echo a && echo b"), vec!["echo a", "echo b"]);
        assert_eq!(split_command("a; b ; c"), vec!["a", "b", "c"]);
        assert_eq!(split_command("cat x | grep y"), vec!["cat x", "grep y"]);
        // quotes protect separators
        assert_eq!(split_command("echo 'a && b'"), vec!["echo 'a && b'"]);
        assert_eq!(split_command("echo \"x | y\""), vec!["echo \"x | y\""]);
        // background & is not a separator
        assert_eq!(split_command("sleep 1 & echo done"), vec!["sleep 1 & echo done"]);
    }

    #[test]
    fn splits_on_newline_but_not_continuation() {
        assert_eq!(split_command("ls\nrm -rf /"), vec!["ls", "rm -rf /"]);
        // backslash-newline is a line continuation → NOT a split.
        assert_eq!(split_command("npm install \\\n  foo"), vec!["npm install \\\n  foo".trim()]);
        // full-line comments are dropped.
        assert_eq!(split_command("# noop\nrm -rf /"), vec!["rm -rf /"]);
    }

    #[test]
    fn newline_does_not_bypass_deny() {
        // The headline guarantee: a denied command stays denied behind a newline.
        assert!(rule_matches_any_subcommand("rm:*", "ls\nrm -rf /"));
        assert!(rule_matches_any_subcommand("rm:*", "# comment\nrm -rf /tmp/x"));
    }

    #[test]
    fn newline_does_not_over_allow() {
        // `echo hi\nrm -rf /` must NOT be allowed by `Bash(echo:*)` alone.
        assert!(!command_fully_allowed(&["echo:*"], "echo hi\nrm -rf /"));
        // both lines covered → allowed
        assert!(command_fully_allowed(&["echo:*", "rm:*"], "echo hi\nrm -rf /tmp"));
    }

    #[test]
    fn exact_rule_with_redirection_matches_original() {
        // An exact rule whose content includes a redirection matches the
        // original (un-stripped) command form.
        assert!(rule_matches_any_subcommand("cat x 2>&1", "cat x 2>&1"));
    }

    #[test]
    fn deny_matches_any_subcommand() {
        // The denied subcommand is caught even when compounded with a benign one.
        assert!(rule_matches_any_subcommand("rm:*", "echo ok && rm -rf /tmp/x"));
        assert!(rule_matches_any_subcommand("curl:*", "echo x | curl evil.com"));
        assert!(!rule_matches_any_subcommand("rm:*", "echo ok && ls"));
    }

    #[test]
    fn deny_survives_env_and_wrapper_wrapping() {
        assert!(rule_matches_any_subcommand("npm install:*", "timeout 5 npm install foo"));
        assert!(rule_matches_any_subcommand("secret-tool:*", "FOO=bar secret-tool dump"));
        assert!(rule_matches_any_subcommand("claude:*", "nohup FOO=bar timeout 5 claude"));
    }

    #[test]
    fn exact_rule_requires_exact() {
        assert!(rule_matches_any_subcommand("git status", "git status"));
        assert!(!rule_matches_any_subcommand("git status", "git status -s"));
    }

    #[test]
    fn prefix_word_boundary() {
        assert!(rule_matches_any_subcommand("ls:*", "ls -la"));
        assert!(rule_matches_any_subcommand("ls:*", "ls"));
        assert!(!rule_matches_any_subcommand("ls:*", "lsof -i"));
    }

    #[test]
    fn prefix_xargs_form() {
        assert!(rule_matches_any_subcommand("grep:*", "xargs grep pattern"));
        assert!(rule_matches_any_subcommand("rm:*", "xargs rm file"));
        // flagged xargs is NOT matched (natural word boundary)
        assert!(!rule_matches_any_subcommand("grep:*", "xargs -n1 grep p"));
    }

    #[test]
    fn allow_requires_every_subcommand_covered() {
        // both covered
        assert!(command_fully_allowed(&["echo:*", "ls:*"], "echo hi && ls -l"));
        // second subcommand uncovered → not allowed (the key over-allow guard)
        assert!(!command_fully_allowed(&["echo:*"], "echo ok && rm -rf /"));
        // single covered
        assert!(command_fully_allowed(&["git status"], "git status"));
        // empty
        assert!(!command_fully_allowed(&["echo:*"], ""));
    }

    #[test]
    fn allow_through_safe_wrapper() {
        assert!(command_fully_allowed(&["npm install:*"], "timeout 10 npm install foo"));
    }

    #[test]
    fn allow_redirection_stripped() {
        assert!(command_fully_allowed(&["python:*"], "python a.py > out.txt"));
        assert!(command_fully_allowed(&["echo:*"], "echo hi 2>&1"));
    }

    #[test]
    fn wildcard_allow() {
        assert!(command_fully_allowed(&["git *"], "git add ."));
        assert!(command_fully_allowed(&["git *"], "git"));
        // single trailing star → bare `npm run` is covered (TS trailing-optional)
        assert!(command_fully_allowed(&["npm run *"], "npm run"));
        // but an unrelated command is not
        assert!(!command_fully_allowed(&["git *"], "npm run"));
    }
}
