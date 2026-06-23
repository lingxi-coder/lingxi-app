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
//! A faithful STATIC structural narrowing keyed on the tool + its input:
//! - shell tools (`Bash`/`PowerShell`) → `Bash(<command-root>:*)` where the root
//!   is the leading run of non-flag tokens of the first sub-command (binary +
//!   sub-commands + leading positional args, stopping at the first `-flag`),
//!   unwrapping `sudo`/`env`/leading `VAR=val` assignments first;
//! - file-path tools (`Edit`/`Write`/`MultiEdit`/`NotebookEdit`/`Read`/`Glob`)
//!   → `Edit(<path>)` from `file_path` / `notebook_path` / `path`;
//! - `WebFetch` → `WebFetch(domain:<host>)` from the `url` host;
//! - everything else (MCP tools, tools with no narrowable input) → tool-wide,
//!   exactly as before.
//!
//! ## Documented residual
//! claude-code's Bash prefix is partly MODEL-assisted (`getCommandPrefix` issues
//! an LLM call to pick the safest prefix and may return the exact command or a
//! shorter prefix). This static extractor errs toward a NARROWER prefix than the
//! model would (it keeps leading positional args), so the persisted grant is
//! never BROADER than claude-code's — only occasionally tighter. The model-driven
//! refinement is the residual; the structural narrowing (vs. whole-tool) is the
//! parity fix.

use crate::rule::{PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue};
use crate::shell_command::{
    command_from_input, is_shell_tool, split_command, strip_all_leading_env_vars,
    strip_safe_wrappers,
};
use serde_json::Value;

/// File-path tools whose `AllowAlways` narrows to the touched path.
const FILE_PATH_TOOLS: &[&str] = &[
    "Edit",
    "Write",
    "MultiEdit",
    "NotebookEdit",
    "Read",
    "Glob",
];

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
        return command_from_input(input).and_then(bash_command_root).map(|root| format!("{root}:*"));
    }
    if FILE_PATH_TOOLS.contains(&tool_name) {
        return file_path_content(input);
    }
    if tool_name == "WebFetch" {
        return webfetch_domain(input).map(|host| format!("domain:{host}"));
    }
    None
}

/// Extract the static command root of the FIRST sub-command: the leading run of
/// tokens that are not `-flags`, after unwrapping `sudo`/`env`-style wrappers and
/// stripping leading `VAR=value` assignments. Returns `None` for an empty /
/// flag-only command (→ tool-wide fallback).
fn bash_command_root(command: &str) -> Option<String> {
    // Unwrap safe wrappers (sudo/env/timeout/...), then take the first
    // sub-command before any `&&`/`|`/`;` operator (quote-aware split).
    let unwrapped = strip_safe_wrappers(command);
    let first = split_command(&unwrapped)
        .into_iter()
        .next()
        .unwrap_or(unwrapped);
    // Drop leading `VAR=val` assignments so `FOO=1 git status` → `git status`.
    let first = strip_all_leading_env_vars(&first, None);

    let mut root: Vec<&str> = Vec::new();
    for tok in first.split_whitespace() {
        // Stop at the first flag or any stray operator token.
        if tok.starts_with('-') || matches!(tok, "|" | "&&" | "||" | ";" | ">" | "<" | "&") {
            break;
        }
        root.push(tok);
    }
    if root.is_empty() {
        None
    } else {
        Some(root.join(" "))
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
    fn bash_narrows_to_command_root_prefix() {
        assert_eq!(
            content("Bash", json!({ "command": "git commit -m \"x\"" })),
            Some("git commit:*".into())
        );
        assert_eq!(
            content("Bash", json!({ "command": "npm run build" })),
            Some("npm run build:*".into())
        );
        // Leading env assignments are stripped (`sudo` is NOT a safe wrapper in
        // claude-code, so it is kept verbatim — only timeout/time/nice/stdbuf/
        // nohup unwrap).
        assert_eq!(
            content("Bash", json!({ "command": "FOO=1 systemctl restart nginx" })),
            Some("systemctl restart nginx:*".into())
        );
        // First sub-command only (before the pipe).
        assert_eq!(
            content("Bash", json!({ "command": "cat f | grep x" })),
            Some("cat f:*".into())
        );
    }

    #[test]
    fn bash_with_no_root_falls_back_tool_wide() {
        // A flag-only / empty command yields a tool-wide allow.
        assert_eq!(content("Bash", json!({ "command": "-v" })), None);
        assert_eq!(content("Bash", json!({ "command": "" })), None);
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
            content("WebFetch", json!({ "url": "https://user@Example.COM:8443/a/b?q=1" })),
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
