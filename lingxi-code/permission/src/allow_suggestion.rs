//! `allow_suggestion` — narrow the rule persisted when a user picks
//! "always allow" in a permission dialog.
//!
//! ## Why
//! When the user selects `AllowAlways`, the host previously persisted a BARE
//! tool-wide allow ([`PermissionRule::allow_tool_session`], `rule_content: None`)
//! — e.g. one `Bash` approval became `allow: ["Bash"]`, granting EVERY future
//! bash command. claude-code instead narrows the persisted rule to the specific
//! command / path / domain the call used, so the grant is scoped (its permission
//! dialog builds `ruleSuggestions` from the tool input and persists the chosen
//! suggestion, e.g. `Bash(npm run build:*)`, `Edit(/abs/path)`,
//! `WebFetch(domain:example.com)`).
//!
//! ## What this ports
//! The 2.1.211 `KQt` / `$ro` / `MOg` static suggestion algorithm, keyed on the
//! tool + its input:
//! - shell tools (`Bash`/`PowerShell`) → a `Bash(<prefix> *)` WILDCARD rule
//!   (`t9r` appends ` *`, NOT `:*`) where `<prefix>` is EXACTLY TWO tokens
//!   (`$ro`): leading `NAME=value` env assignments are consumed only when every
//!   NAME is in the safe `Jqr` set (else fall back to exact), the first non-env
//!   token must not be an interpreter (`Bro`), and the second token must match
//!   `/^[a-z][a-z0-9]*(-[a-z0-9]+)*$/`. Heredoc (`MOg`) and multiline
//!   (first-line) commands take a special-cased prefix. Anything else falls back
//!   to an EXACT full-command rule (`YYn`, no wildcard) — so `ls` → `Bash(ls)`
//!   (bare only), `cat /etc/hosts` → `Bash(cat /etc/hosts)`, and
//!   `SECRET=x cmd` → `Bash(SECRET=x cmd)` (never widened to `cmd *`);
//! - file-path tools (`Edit`/`Write`/`MultiEdit`/`NotebookEdit`/`Read`/`Glob`)
//!   → `Edit(<path>)` from `file_path` / `notebook_path` / `path`;
//! - `WebFetch` → `WebFetch(domain:<host>)` from the `url` host;
//! - a shell call with NO command, and every other tool (MCP tools, tools with
//!   no narrowable input) → tool-wide, exactly as before.
//!
//! ## Documented residual
//! claude-code's `getCommandPrefix` may ALSO issue an LLM call to refine the
//! prefix; only the byte-faithful STATIC `KQt` path is ported here. Both `:*`
//! and ` *` are prefix wildcards to the matcher (`shell_rule_matching` strips the
//! trailing two chars), so the ` *` bytes match `<prefix>`-anchored commands
//! exactly as `:*` would; the exact-command fallback requires a byte-equal
//! command.

use crate::rule::{PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue};
use crate::shell_command::{command_from_input, is_shell_tool};
use serde_json::Value;
use std::sync::LazyLock;

/// File-path tools whose `AllowAlways` narrows to the touched path.
const FILE_PATH_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit", "Read", "Glob"];

/// Build the [`PermissionRule`] to persist for an `AllowAlways` choice on a call
/// to `tool_name` with `input`. Returns a Session-sourced `Allow` rule whose
/// `rule_content` is narrowed to the command / path / domain the call used, or a
/// bare tool-wide allow when nothing narrowable is present (preserving the prior
/// behavior for MCP tools and inputs without a recognizable command/path/url).
///
/// The returned rule's `source` is [`PermissionRuleSource::Session`] (the
/// session-rule list); the persist layer assigns the durable destination
/// (`settings.local.json`) separately.
#[must_use]
pub fn allow_suggestion(tool_name: &str, input: &Value) -> PermissionRule {
    match narrowed_content(tool_name, input) {
        Some(content) => PermissionRule {
            value: PermissionRuleValue {
                tool_name: tool_name.to_string(),
                rule_content: Some(content),
            },
            behavior: PermissionBehavior::Allow,
            source: PermissionRuleSource::Session,
        },
        // Nothing narrowable → fall back to the prior tool-wide allow.
        None => PermissionRule::allow_tool_session(tool_name),
    }
}

/// Whether a session/allow `rule` matches a live call to `tool_name`/`input`.
///
/// Content-aware analogue of [`PermissionRule::matches_tool`]: a NARROWED
/// `AllowAlways` rule (e.g. `Bash(git commit:*)`, `Edit(/abs/path)`,
/// `WebFetch(domain:example.com)`) skips the dialog ONLY for a matching
/// command / path / domain — not the whole tool. A tool-wide allow
/// (`rule_content: None`) still matches the bare tool, preserving the prior
/// fast-path. Used by the TUI / adapter gates' pre-dialog session-rule lookup so
/// the narrowed rule they now persist is also honored within the session.
#[must_use]
pub fn call_matches_rule(rule: &PermissionRule, tool_name: &str, input: &Value) -> bool {
    if rule.value.tool_name != tool_name || !matches!(rule.behavior, PermissionBehavior::Allow) {
        return false;
    }
    match rule.value.rule_content.as_deref() {
        // Tool-wide allow — matches every call to the tool (legacy behavior).
        None => true,
        Some(content) => {
            if is_shell_tool(tool_name) {
                // Faithful shell allow-matcher: the stored prefix/exact rule must
                // cover the live command (every sub-command).
                command_from_input(input)
                    .is_some_and(|cmd| crate::shell_command::command_fully_allowed(&[content], cmd))
            } else {
                // Path / domain / other: the live call must narrow to the SAME
                // content (exact path / host), which is safe (never broader).
                narrowed_content(tool_name, input).as_deref() == Some(content)
            }
        }
    }
}

/// Compute the narrowed `rule_content` for `tool_name`/`input`, or `None` to keep
/// the call tool-wide.
fn narrowed_content(tool_name: &str, input: &Value) -> Option<String> {
    if is_shell_tool(tool_name) {
        // 2.1.211 `KQt`: for a shell tool with a command, the suggestion is ALWAYS
        // a concrete rule (a `<prefix> *` wildcard or an EXACT full command) —
        // never tool-wide. Only a call with no command at all stays tool-wide.
        return command_from_input(input)
            .filter(|c| !c.trim().is_empty())
            .map(bash_suggestion_content);
    }
    if FILE_PATH_TOOLS.contains(&tool_name) {
        return file_path_content(input);
    }
    if tool_name == "WebFetch" {
        return webfetch_domain(input).map(|host| format!("domain:{host}"));
    }
    None
}

/// The 2.1.211 `KQt` interpreter blocklist `Bro`: if the command's first
/// non-env token is one of these (by basename), no static prefix is derived and
/// the suggestion falls back to the EXACT full command.
const INTERPRETER_BLOCKLIST: &[&str] = &[
    "sh", "bash", "zsh", "fish", "csh", "tcsh", "ksh", "dash", "cmd", "powershell", "pwsh", "env",
    "xargs", "command", "builtin", "noglob", "nice", "stdbuf", "nohup", "timeout", "time", "watch",
    "ionice", "chrt", "setsid", "taskset", "strace", "ltrace", "script", "flock", "unshare",
    "nsenter", "sudo", "doas", "pkexec", "su", "runuser",
];

/// The 2.1.211 `KQt` safe leading-env-assignment allowlist `Jqr`: a leading
/// `NAME=value` prefix keeps the command prefix-narrowable only when every NAME
/// is in this set (else the suggestion falls back to the EXACT full command, so
/// a `SECRET=… cmd` grant never widens to `cmd *`).
const SAFE_ENV_ASSIGNMENTS: &[&str] = &[
    "GOEXPERIMENT",
    "GOOS",
    "GOARCH",
    "CGO_ENABLED",
    "GO111MODULE",
    "RUST_BACKTRACE",
    "RUST_LOG",
    "NODE_ENV",
    "PYTHONUNBUFFERED",
    "PYTHONDONTWRITEBYTECODE",
    "PYTEST_DISABLE_PLUGIN_AUTOLOAD",
    "PYTEST_DEBUG",
    "ANTHROPIC_API_KEY",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_TIME",
    "CHARSET",
    "TERM",
    "COLORTERM",
    "NO_COLOR",
    "FORCE_COLOR",
    "TZ",
    "LS_COLORS",
    "LSCOLORS",
    "GREP_COLOR",
    "GREP_COLORS",
    "GCC_COLORS",
    "TIME_STYLE",
    "BLOCK_SIZE",
    "BLOCKSIZE",
    "COLUMNS",
    "LINES",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "CI",
    "DEBIAN_FRONTEND",
    "GIT_TERMINAL_PROMPT",
];

/// `Fro=/^[A-Za-z_]\w*=/` — a leading `NAME=` env-assignment token.
fn env_assignment_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^[A-Za-z_]\w*=").unwrap());
    &RE
}

/// `/^[a-z][a-z0-9]*(-[a-z0-9]+)*$/` — the shape the SECOND prefix token must
/// have (a lowercase sub-command like `commit`, `run`, `for-each-ref`).
fn subcommand_shape_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$").unwrap());
    &RE
}

/// The full 2.1.211 `KQt` suggestion-content chain for a shell command. Returns
/// the rule_content to persist: a `<prefix> *` wildcard (`t9r`) when a static
/// 2-token prefix can be derived (heredoc `MOg`, multiline first-line, or the
/// general `$ro` case), else the EXACT full command (`YYn`, no wildcard).
fn bash_suggestion_content(command: &str) -> String {
    // Heredoc (`MOg`): a prefix taken from the text BEFORE `<<`.
    if let Some(prefix) = heredoc_prefix(command) {
        return format!("{prefix} *");
    }
    // Multiline (`Nd(e).trim()` = text before the first newline): the whole first
    // line is the prefix, wildcarded.
    if command.contains('\n') {
        let first_line = command.split('\n').next().unwrap_or(command).trim();
        if !first_line.is_empty() {
            return format!("{first_line} *");
        }
    }
    // General 2-token prefix (`$ro`).
    if let Some(prefix) = static_prefix(command) {
        return format!("{prefix} *");
    }
    // Fallback (`YYn`): the exact full command, no wildcard.
    command.to_string()
}

/// 2.1.211 `$ro`: derive the static two-token prefix of `command`, or `None` to
/// fall back to the exact command. Splits on whitespace, consumes leading safe
/// env assignments (aborting if any NAME is unsafe), requires ≥2 remaining
/// tokens, rejects an interpreter first token, and requires the second token to
/// match [`subcommand_shape_re`]. Returns exactly the first two remaining tokens
/// joined by a space.
fn static_prefix(command: &str) -> Option<String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    let mut r = 0;
    while r < tokens.len() && env_assignment_re().is_match(tokens[r]) {
        // `ts(tok,"=")` — the NAME before the first `=`.
        let name = tokens[r].split_once('=').map_or(tokens[r], |(n, _)| n);
        if !SAFE_ENV_ASSIGNMENTS.contains(&name) {
            return None; // unsafe leading env → fall back to exact command.
        }
        r += 1;
    }
    let rest = &tokens[r..];
    if rest.len() < 2 {
        return None;
    }
    // `n[0].split("/").pop()` — the first token's basename.
    let base0 = rest[0].rsplit('/').next().unwrap_or(rest[0]);
    if INTERPRETER_BLOCKLIST.contains(&base0) {
        return None;
    }
    if !subcommand_shape_re().is_match(rest[1]) {
        return None;
    }
    Some(format!("{} {}", rest[0], rest[1]))
}

/// 2.1.211 `MOg`: derive a two-token prefix from the text BEFORE a `<<` heredoc,
/// or `None`. Tries [`static_prefix`] first; else consumes leading safe env
/// assignments and takes up to two of the remaining tokens (WITHOUT the
/// second-token shape check), rejecting an interpreter first token.
fn heredoc_prefix(command: &str) -> Option<String> {
    if !command.contains("<<") {
        return None;
    }
    let idx = command.find("<<")?;
    if idx == 0 {
        return None; // `t<=0` — nothing before `<<`.
    }
    let before = command[..idx].trim();
    if before.is_empty() {
        return None;
    }
    if let Some(prefix) = static_prefix(before) {
        return Some(prefix);
    }
    // Manual env-skip + up-to-two-tokens (no shape check).
    let tokens: Vec<&str> = before.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() && env_assignment_re().is_match(tokens[i]) {
        let name = tokens[i].split_once('=').map_or(tokens[i], |(n, _)| n);
        if !SAFE_ENV_ASSIGNMENTS.contains(&name) {
            return None;
        }
        i += 1;
    }
    if i >= tokens.len() {
        return None;
    }
    let base = tokens[i].rsplit('/').next().unwrap_or(tokens[i]);
    if INTERPRETER_BLOCKLIST.contains(&base) {
        return None;
    }
    let joined = tokens[i..(i + 2).min(tokens.len())].join(" ");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// Pull a file path from a file-tool input (`file_path` for Edit/Write/Read,
/// `notebook_path` for NotebookEdit, `path`/`pattern` for Glob).
fn file_path_content(input: &Value) -> Option<String> {
    for key in ["file_path", "notebook_path", "path", "pattern"] {
        if let Some(s) = input.get(key).and_then(Value::as_str) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Extract the host of a `WebFetch` call's `url` (scheme + userinfo + port +
/// path stripped), e.g. `https://user@Example.COM:8443/x` → `example.com`.
fn webfetch_domain(input: &Value) -> Option<String> {
    let url = input.get("url").and_then(Value::as_str)?;
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    // Authority ends at the first `/`, `?`, or `#`.
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    // Drop any `userinfo@`.
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // Drop a `:port` suffix (IPv6 literals are not narrowed — keep as-is).
    let host = if hostport.starts_with('[') {
        hostport
    } else {
        hostport.split(':').next().unwrap_or(hostport)
    };
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn content(tool: &str, input: Value) -> Option<String> {
        let rule = allow_suggestion(tool, &input);
        rule.value.rule_content
    }

    #[test]
    fn bash_narrows_to_two_token_wildcard_prefix() {
        // `$ro` yields EXACTLY two tokens with the ` *` (space-star) wildcard.
        assert_eq!(
            content("Bash", json!({ "command": "git commit -m \"x\"" })),
            Some("git commit *".into())
        );
        // Two-token cap: `npm run build` → `npm run *` (NOT `npm run build`).
        assert_eq!(
            content("Bash", json!({ "command": "npm run build" })),
            Some("npm run *".into())
        );
        // First two whitespace tokens (does NOT stop at the pipe): `cat f`.
        assert_eq!(
            content("Bash", json!({ "command": "cat f | grep x" })),
            Some("cat f *".into())
        );
    }

    #[test]
    fn bash_unsafe_env_prefix_falls_back_to_exact() {
        // A leading env NAME outside the safe `Jqr` set → EXACT full command,
        // never widened to `<cmd> *`.
        assert_eq!(
            content(
                "Bash",
                json!({ "command": "FOO=1 systemctl restart nginx" })
            ),
            Some("FOO=1 systemctl restart nginx".into())
        );
        // A safe env NAME (`CI`) IS consumed, leaving a two-token prefix.
        assert_eq!(
            content("Bash", json!({ "command": "CI=1 npm test" })),
            Some("npm test *".into())
        );
    }

    #[test]
    fn bash_single_token_and_interpreter_and_path_fall_back_to_exact() {
        // Single token (< 2 non-env tokens) → EXACT bare command (`ls`), NOT `ls *`.
        assert_eq!(content("Bash", json!({ "command": "ls" })), Some("ls".into()));
        // Interpreter first token (`Bro`) → exact.
        assert_eq!(
            content("Bash", json!({ "command": "bash script.sh" })),
            Some("bash script.sh".into())
        );
        // Second token failing the shape regex (a path) → exact.
        assert_eq!(
            content("Bash", json!({ "command": "cat /etc/hosts" })),
            Some("cat /etc/hosts".into())
        );
        // Flag-only command (`-v`) → exact `-v` (still not tool-wide).
        assert_eq!(content("Bash", json!({ "command": "-v" })), Some("-v".into()));
    }

    #[test]
    fn bash_heredoc_and_multiline_prefix() {
        // Heredoc (`MOg`): prefix from the text before `<<`.
        assert_eq!(
            content("Bash", json!({ "command": "cat foo <<EOF\nx\nEOF" })),
            Some("cat foo *".into())
        );
        // Multiline: whole first line + ` *`.
        assert_eq!(
            content("Bash", json!({ "command": "git status\nrm -rf /" })),
            Some("git status *".into())
        );
    }

    #[test]
    fn mobile_shell_narrows_like_bash() {
        // The Android mobile `Shell` tool narrows an AllowAlways exactly like
        // `Bash` — never a tool-wide `Shell` allow.
        assert_eq!(
            content("Shell", json!({ "command": "git status" })),
            Some("git status *".into())
        );
        assert_eq!(
            content("Shell", json!({ "command": "git commit -m \"x\"" })),
            Some("git commit *".into())
        );
        // The persisted ` *` prefix rule matches a covered command, not an
        // unrelated one.
        let rule = allow_suggestion("Shell", &json!({ "command": "git status" }));
        assert!(call_matches_rule(
            &rule,
            "Shell",
            &json!({ "command": "git status -s" })
        ));
        assert!(!call_matches_rule(
            &rule,
            "Shell",
            &json!({ "command": "rm -rf /tmp/x" })
        ));
    }

    #[test]
    fn bash_no_command_stays_tool_wide() {
        // Only a call with NO command (or an all-whitespace command) stays
        // tool-wide; a flag-only command instead persists exactly (see above).
        assert_eq!(content("Bash", json!({ "command": "" })), None);
        assert_eq!(content("Bash", json!({ "command": "   " })), None);
        assert_eq!(content("Bash", json!({})), None);
    }

    #[test]
    fn file_tools_narrow_to_path() {
        assert_eq!(
            content("Edit", json!({ "file_path": "/abs/src/main.rs" })),
            Some("/abs/src/main.rs".into())
        );
        assert_eq!(
            content("NotebookEdit", json!({ "notebook_path": "/n.ipynb" })),
            Some("/n.ipynb".into())
        );
        assert_eq!(content("Write", json!({})), None);
    }

    #[test]
    fn webfetch_narrows_to_host() {
        assert_eq!(
            content(
                "WebFetch",
                json!({ "url": "https://user@Example.COM:8443/a/b?q=1" })
            ),
            Some("domain:example.com".into())
        );
        assert_eq!(
            content("WebFetch", json!({ "url": "http://docs.rs/serde" })),
            Some("domain:docs.rs".into())
        );
        assert_eq!(content("WebFetch", json!({})), None);
    }

    #[test]
    fn unknown_and_mcp_tools_stay_tool_wide() {
        assert_eq!(content("WebSearch", json!({ "query": "x" })), None);
        assert_eq!(content("mcp__server__tool", json!({ "a": 1 })), None);
        // The fallback rule is the bare tool-wide allow.
        let rule = allow_suggestion("mcp__server__tool", &json!({}));
        assert!(rule.value.rule_content.is_none());
        assert_eq!(rule.value.tool_name, "mcp__server__tool");
        assert!(matches!(rule.behavior, PermissionBehavior::Allow));
    }
}
