//! Dangerous-permission detection for auto mode.
//!
//! Ports the `isDangerous{Bash,PowerShell,Task}Permission` predicates,
//! `isDangerousClassifierPermission`, and `findDangerousClassifierPermissions`
//! from claude-code `utils/permissions/permissionSetup.ts:94-342`.
//!
//! An allow rule that lets the model run arbitrary code (e.g. `Bash(python:*)`,
//! `PowerShell(iex:*)`, `Agent(*)`) would bypass the auto-mode classifier's
//! safety evaluation. These predicates identify such rules so
//! [`crate::policy::PermissionPolicy::strip_dangerous_for_auto`] can strip them
//! on auto-mode entry and restore them on exit.
//!
//! ## Documented divergences (external build parity)
//! - The `USER_TYPE === 'ant'` `Tmux` special-case in
//!   `isDangerousClassifierPermission` (`permissionSetup.ts:276-279`) is
//!   OMITTED. TS only treats `Tmux` as dangerous for `ant` users; external
//!   builds (and this port) do not.
//! - The ant-only tail of `DANGEROUS_BASH_PATTERNS` is omitted in
//!   [`crate::dangerous_patterns`] (documented there).
//!
//! The exact-shape matcher itself is byte-faithful to TS.

use crate::dangerous_patterns::{
    dangerous_bash_patterns, CROSS_PLATFORM_CODE_EXEC, POWERSHELL_DANGEROUS_PATTERNS,
};
use crate::rule::{
    normalize_legacy_tool_name, PermissionBehavior, PermissionRule, PermissionRuleSource,
    PermissionRuleValue,
};

/// Canonical tool names (claude-code `tools/{BashTool,PowerShellTool}/toolName.ts`,
/// `tools/AgentTool/constants.ts`). Kept as local literals so the predicates
/// don't pull a dependency on the tool crates.
const BASH_TOOL_NAME: &str = "Bash";
const POWERSHELL_TOOL_NAME: &str = "PowerShell";
const AGENT_TOOL_NAME: &str = "Agent";

/// The exact-shape match used by both shell predicates against a single
/// lowercase pattern. 1:1 with the per-pattern body of
/// `isDangerousBashPermission` (`permissionSetup.ts:117-143`):
///
/// - `content == pattern`
/// - `content == "{pattern}:*"`
/// - `content == "{pattern}*"`
/// - `content == "{pattern} *"`
/// - `content.starts_with("{pattern} -") && content.ends_with('*')`
///
/// `content` and `pattern` are both already lowercased by the caller.
fn matches_pattern_shape(content: &str, pattern: &str) -> bool {
    content == pattern
        || content == format!("{pattern}:*")
        || content == format!("{pattern}*")
        || content == format!("{pattern} *")
        || (content.starts_with(&format!("{pattern} -")) && content.ends_with('*'))
}

/// Checks if a Bash permission rule is dangerous for auto mode. 1:1 with
/// `isDangerousBashPermission` (`permissionSetup.ts:94-147`).
///
/// A rule is dangerous if it would auto-allow commands that execute arbitrary
/// code, bypassing the classifier's safety evaluation:
/// 1. Tool-level allow (`Bash` with no content, or `Bash(*)`) — allows ALL
///    commands. (`content` is `None` here because the parser collapses
///    `Bash()`/`Bash(*)` to `rule_content: None`.)
/// 2. Standalone wildcard (`*`).
/// 3. A dangerous interpreter pattern in any of the rule-shape variants.
#[must_use]
pub fn is_dangerous_bash_permission(tool_name: &str, rule_content: &Option<String>) -> bool {
    // Only check Bash rules.
    if tool_name != BASH_TOOL_NAME {
        return false;
    }

    // Tool-level allow (Bash with no content, or Bash(*)) — allows ALL commands.
    let content = match rule_content {
        None => return true,
        Some(c) if c.is_empty() => return true,
        Some(c) => c.trim().to_lowercase(),
    };

    // Standalone wildcard (*) matches everything.
    if content == "*" {
        return true;
    }

    // Check for dangerous patterns with prefix syntax (e.g. "python:*") or
    // wildcard syntax (e.g. "python*").
    for pattern in dangerous_bash_patterns() {
        let lower_pattern = pattern.to_lowercase();
        if matches_pattern_shape(&content, &lower_pattern) {
            return true;
        }
    }

    false
}

/// Checks if a `PowerShell` permission rule is dangerous for auto mode. 1:1 with
/// `isDangerousPowerShellPermission` (`permissionSetup.ts:157-233`).
///
/// `PowerShell` is case-insensitive, so rule content is lowercased before
/// matching. In addition to the bash-style rule-shape variants, this checks the
/// `.exe`-on-first-word variant (`python` → `python.exe`, `npm run` →
/// `npm.exe run`) so a rule like `PowerShell(npm.exe run:*)` matches `npm run`.
#[must_use]
pub fn is_dangerous_powershell_permission(tool_name: &str, rule_content: &Option<String>) -> bool {
    if tool_name != POWERSHELL_TOOL_NAME {
        return false;
    }

    // Tool-level allow (PowerShell with no content, or PowerShell(*)).
    let content = match rule_content {
        None => return true,
        Some(c) if c.is_empty() => return true,
        Some(c) => c.trim().to_lowercase(),
    };

    // Standalone wildcard (*) matches everything.
    if content == "*" {
        return true;
    }

    // PS-specific cmdlet names. CROSS_PLATFORM_CODE_EXEC is shared with bash.
    let patterns = CROSS_PLATFORM_CODE_EXEC
        .iter()
        .chain(POWERSHELL_DANGEROUS_PATTERNS.iter());

    for pattern in patterns {
        // patterns stored lowercase; content lowercased above.
        if matches_pattern_shape(&content, pattern) {
            return true;
        }
        // .exe — goes on the FIRST word. `python` → `python.exe`.
        // `npm run` → `npm.exe run` (npm.exe is the real Windows binary name).
        // A rule like `PowerShell(npm.exe run:*)` needs to match `npm run`.
        let exe = match pattern.find(' ') {
            None => format!("{pattern}.exe"),
            Some(sp) => format!("{}.exe{}", &pattern[..sp], &pattern[sp..]),
        };
        if matches_pattern_shape(&content, &exe) {
            return true;
        }
    }
    false
}

/// Checks if an Agent (sub-agent) permission rule is dangerous for auto mode.
/// 1:1 with `isDangerousTaskPermission` (`permissionSetup.ts:240-245`).
///
/// Any Agent allow rule would auto-approve sub-agent spawns before the auto-mode
/// classifier can evaluate the sub-agent's prompt, defeating delegation-attack
/// prevention. The tool name is normalized (`Task` → `Agent`) first.
#[must_use]
pub fn is_dangerous_task_permission(tool_name: &str, _rule_content: &Option<String>) -> bool {
    normalize_legacy_tool_name(tool_name) == AGENT_TOOL_NAME
}

/// Checks if a permission rule is dangerous for auto mode (the OR of the three
/// shell/agent predicates). 1:1 with `isDangerousClassifierPermission`
/// (`permissionSetup.ts:272-285`), MINUS the `USER_TYPE === 'ant'` `Tmux` case
/// (see module doc).
#[must_use]
pub fn is_dangerous_classifier_permission(tool_name: &str, rule_content: &Option<String>) -> bool {
    // The ant-only `Tmux` special-case (`permissionSetup.ts:276-279`) is omitted
    // for external parity.
    is_dangerous_bash_permission(tool_name, rule_content)
        || is_dangerous_powershell_permission(tool_name, rule_content)
        || is_dangerous_task_permission(tool_name, rule_content)
}

/// Structured info about a dangerous permission found in the loaded rules.
/// 1:1 shape with the TS `DangerousPermissionInfo`
/// (`permissionSetup.ts:258-265`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DangerousPermissionInfo {
    /// The matched rule's value (tool name + optional content).
    pub rule_value: PermissionRuleValue,
    /// Where the rule came from.
    pub source: PermissionRuleSource,
    /// The rule formatted for display, e.g. `"Bash(*)"` or `"Bash(python:*)"`.
    pub rule_display: String,
    /// The source formatted for display (the source variant name in this port;
    /// TS resolves settings sources to a relative file path via
    /// `formatPermissionSource`, which needs the on-disk settings layout — out
    /// of scope for the pure predicate, so the source identifier is surfaced).
    pub source_display: String,
}

/// Finds all dangerous ALLOW permissions among the loaded rules. 1:1 with the
/// settings-rules half of `findDangerousClassifierPermissions`
/// (`permissionSetup.ts:295-321`).
///
/// Divergence from TS: the TS function also takes a `cliAllowedTools: string[]`
/// and parses each `"Tool(content)"` spec (`:322-339`). In this port CLI
/// `--allowed-tools` are already parsed into [`PermissionRule`]s tagged with
/// [`PermissionRuleSource::CliArg`] by the loader, so they arrive through the
/// `rules` slice and the separate string-parsing pass is unnecessary.
#[must_use]
pub fn find_dangerous_classifier_permissions(
    rules: &[PermissionRule],
) -> Vec<DangerousPermissionInfo> {
    let mut dangerous = Vec::new();

    for rule in rules {
        if rule.behavior == PermissionBehavior::Allow
            && is_dangerous_classifier_permission(
                &rule.value.tool_name,
                &rule.value.rule_content,
            )
        {
            // `Bash(python:*)` when content is set, else `Bash(*)` for tool-wide.
            let rule_string = match &rule.value.rule_content {
                Some(content) if !content.is_empty() => {
                    format!("{}({})", rule.value.tool_name, content)
                }
                _ => format!("{}(*)", rule.value.tool_name),
            };
            dangerous.push(DangerousPermissionInfo {
                rule_value: rule.value.clone(),
                source: rule.source,
                rule_display: rule_string,
                source_display: format!("{:?}", rule.source),
            });
        }
    }

    dangerous
}

#[cfg(test)]
mod tests {
    use super::*;

    // Always-`Some` by design: builds the `Some(content)` arg the predicates
    // take, mirroring a `Tool(content)` rule (vs `&None` for a tool-wide rule).
    #[allow(clippy::unnecessary_wraps)]
    fn content(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    // ── isDangerousBashPermission ──

    #[test]
    fn bash_interpreter_prefix_is_dangerous() {
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &content("python:*")));
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &content("node:*")));
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &content("npm run:*")));
    }

    #[test]
    fn bash_safe_command_is_not_dangerous() {
        assert!(!is_dangerous_bash_permission(BASH_TOOL_NAME, &content("ls:*")));
        assert!(!is_dangerous_bash_permission(BASH_TOOL_NAME, &content("git status")));
        assert!(!is_dangerous_bash_permission(BASH_TOOL_NAME, &content("cat foo")));
    }

    #[test]
    fn bash_tool_wide_is_dangerous() {
        // Bash with no content (`Bash`, `Bash()`, `Bash(*)` all parse to None).
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &None));
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &content("")));
    }

    #[test]
    fn bash_standalone_wildcard_is_dangerous() {
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &content("*")));
    }

    #[test]
    fn bash_shape_variants_match() {
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &content("python")));
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &content("python*")));
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &content("python *")));
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &content("python -c*")));
        assert!(is_dangerous_bash_permission(BASH_TOOL_NAME, &content("PYTHON:*"))); // case-insensitive
    }

    #[test]
    fn bash_predicate_ignores_non_bash_tools() {
        assert!(!is_dangerous_bash_permission("PowerShell", &content("python:*")));
        assert!(!is_dangerous_bash_permission("Read", &None));
    }

    #[test]
    fn bash_ant_only_tail_is_omitted() {
        // The ant-only tail (`gh`, `curl`, `git`, `aws`, …) is NOT dangerous in
        // the external build.
        assert!(!is_dangerous_bash_permission(BASH_TOOL_NAME, &content("gh:*")));
        assert!(!is_dangerous_bash_permission(BASH_TOOL_NAME, &content("curl:*")));
        assert!(!is_dangerous_bash_permission(BASH_TOOL_NAME, &content("git:*")));
        assert!(!is_dangerous_bash_permission(BASH_TOOL_NAME, &content("aws:*")));
    }

    // ── isDangerousPowerShellPermission ──

    #[test]
    fn powershell_iex_is_dangerous() {
        assert!(is_dangerous_powershell_permission(POWERSHELL_TOOL_NAME, &content("iex:*")));
        assert!(is_dangerous_powershell_permission(
            POWERSHELL_TOOL_NAME,
            &content("invoke-expression:*")
        ));
        assert!(is_dangerous_powershell_permission(
            POWERSHELL_TOOL_NAME,
            &content("start-process:*")
        ));
    }

    #[test]
    fn powershell_exe_variant_is_dangerous() {
        // `python.exe` and `npm.exe run` variants must match.
        assert!(is_dangerous_powershell_permission(
            POWERSHELL_TOOL_NAME,
            &content("python.exe:*")
        ));
        assert!(is_dangerous_powershell_permission(
            POWERSHELL_TOOL_NAME,
            &content("npm.exe run:*")
        ));
    }

    #[test]
    fn powershell_tool_wide_and_wildcard_are_dangerous() {
        assert!(is_dangerous_powershell_permission(POWERSHELL_TOOL_NAME, &None));
        assert!(is_dangerous_powershell_permission(POWERSHELL_TOOL_NAME, &content("*")));
    }

    #[test]
    fn powershell_safe_command_is_not_dangerous() {
        assert!(!is_dangerous_powershell_permission(
            POWERSHELL_TOOL_NAME,
            &content("get-childitem:*")
        ));
    }

    #[test]
    fn powershell_predicate_ignores_non_ps_tools() {
        assert!(!is_dangerous_powershell_permission("Bash", &content("iex:*")));
    }

    // ── isDangerousTaskPermission ──

    #[test]
    fn agent_any_rule_is_dangerous() {
        assert!(is_dangerous_task_permission(AGENT_TOOL_NAME, &None));
        assert!(is_dangerous_task_permission(AGENT_TOOL_NAME, &content("general-purpose")));
        // Legacy `Task` normalizes to `Agent`.
        assert!(is_dangerous_task_permission("Task", &content("anything")));
    }

    #[test]
    fn non_agent_task_permission_is_not_dangerous() {
        assert!(!is_dangerous_task_permission("Read", &None));
        assert!(!is_dangerous_task_permission("Bash", &content("ls:*")));
    }

    // ── isDangerousClassifierPermission (OR) ──

    #[test]
    fn classifier_predicate_is_or_of_three() {
        assert!(is_dangerous_classifier_permission("Bash", &content("python:*")));
        assert!(is_dangerous_classifier_permission("PowerShell", &content("iex:*")));
        assert!(is_dangerous_classifier_permission("Agent", &content("x")));
        assert!(!is_dangerous_classifier_permission("Read", &None));
        assert!(!is_dangerous_classifier_permission("Bash", &content("ls:*")));
    }

    // ── findDangerousClassifierPermissions ──

    fn allow(tool: &str, content: Option<&str>, source: PermissionRuleSource) -> PermissionRule {
        PermissionRule {
            value: PermissionRuleValue {
                tool_name: tool.to_string(),
                rule_content: content.map(str::to_string),
            },
            behavior: PermissionBehavior::Allow,
            source,
        }
    }

    #[test]
    fn find_collects_only_dangerous_allow_rules() {
        let rules = vec![
            allow("Bash", Some("python:*"), PermissionRuleSource::UserSettings),
            allow("Bash", Some("ls:*"), PermissionRuleSource::UserSettings),
            allow("Read", None, PermissionRuleSource::UserSettings),
            allow("Agent", None, PermissionRuleSource::CliArg),
            // a dangerous DENY rule must NOT be collected (only allow rules).
            PermissionRule {
                value: PermissionRuleValue {
                    tool_name: "Bash".into(),
                    rule_content: None,
                },
                behavior: PermissionBehavior::Deny,
                source: PermissionRuleSource::UserSettings,
            },
        ];
        let found = find_dangerous_classifier_permissions(&rules);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].rule_display, "Bash(python:*)");
        assert_eq!(found[1].rule_display, "Agent(*)");
    }

    #[test]
    fn find_displays_tool_wide_as_star() {
        let rules = vec![allow("Bash", None, PermissionRuleSource::UserSettings)];
        let found = find_dangerous_classifier_permissions(&rules);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule_display, "Bash(*)");
    }
}
