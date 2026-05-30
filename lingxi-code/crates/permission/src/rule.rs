//! `PermissionRule` — one allow/deny/ask entry, tagged with its source.
//!
//! Rules are evaluated in source-priority order: highest-priority source
//! wins, deny rules beat allow rules at the same priority.

use serde::{Deserialize, Serialize};

/// A single permission rule: the tool it targets, the behavior to apply,
/// and the configuration source it came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionRule {
    /// What the rule matches against.
    pub value: PermissionRuleValue,
    /// What the rule does on match.
    pub behavior: PermissionBehavior,
    /// Where the rule came from (drives priority).
    pub source: PermissionRuleSource,
}

/// The match key for a `PermissionRule`: a tool name plus optional
/// rule-specific content (for example, a bash command substring).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionRuleValue {
    /// Name of the tool this rule applies to (e.g. `"Bash"`, `"Edit"`).
    pub tool_name: String,
    /// Optional rule-specific content used by tool-aware classifiers
    /// (e.g. a bash command pattern). `None` matches the bare tool.
    pub rule_content: Option<String>,
}

/// What a rule does on match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionBehavior {
    /// Allow the call without prompting.
    Allow,
    /// Deny the call.
    Deny,
    /// Ask the user.
    Ask,
}

/// The configuration source a `PermissionRule` came from.
///
/// Priority increases down the list: `Session` overrides everything,
/// `UserSettings` is the lowest. See `priority` for the canonical ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionRuleSource {
    /// `~/.claude/settings.json` (global user config).
    UserSettings,
    /// `.claude/settings.json` checked into the project.
    ProjectSettings,
    /// `.claude/settings.local.json` (gitignored per-clone overrides).
    LocalSettings,
    /// Rules attached to a feature flag.
    FlagSettings,
    /// Managed-policy rules (enterprise pushed).
    PolicySettings,
    /// Rules supplied on the command line.
    CliArg,
    /// Rules emitted by a command (e.g. `/permissions add`).
    Command,
    /// Session-scoped rules (highest priority).
    Session,
}

impl PermissionRuleSource {
    /// Priority order matching claude-code:
    /// `userSettings` → `projectSettings` → `localSettings` → `flagSettings`
    /// → `policySettings` → `cliArg` → `command` → `session`.
    /// Higher index = higher priority.
    #[must_use]
    pub fn priority(self) -> u8 {
        match self {
            Self::UserSettings => 0,
            Self::ProjectSettings => 1,
            Self::LocalSettings => 2,
            Self::FlagSettings => 3,
            Self::PolicySettings => 4,
            Self::CliArg => 5,
            Self::Command => 6,
            Self::Session => 7,
        }
    }
}

impl PermissionRule {
    /// (M6-05) Construct a session-scoped allow rule for the given tool.
    /// Used by `TuiPermissionGate` when the user picks `AllowAlways` in a
    /// permission dialog — the rule lives until the orchestrator session
    /// ends. M7 wires `/permissions` to surface and edit these rules.
    #[must_use]
    pub fn allow_tool_session(tool: &str) -> Self {
        Self {
            value: PermissionRuleValue {
                tool_name: tool.to_string(),
                rule_content: None,
            },
            behavior: PermissionBehavior::Allow,
            source: PermissionRuleSource::Session,
        }
    }

    /// (M6-05) Whether this rule matches the bare tool name with an
    /// `Allow` behavior. Used by the session-rule lookup in
    /// `TuiPermissionGate::check` before the dialog opens.
    #[must_use]
    pub fn matches_tool(&self, tool: &str) -> bool {
        self.value.tool_name == tool
            && self.value.rule_content.is_none()
            && matches!(self.behavior, PermissionBehavior::Allow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_outranks_user_settings() {
        assert!(
            PermissionRuleSource::Session.priority()
                > PermissionRuleSource::UserSettings.priority()
        );
    }

    // M6-05 Task 7: PermissionRule::allow_tool_session + matches_tool.

    #[test]
    fn allow_tool_session_marks_source_session() {
        let r = PermissionRule::allow_tool_session("Bash");
        assert_eq!(r.value.tool_name, "Bash");
        assert!(r.value.rule_content.is_none());
        assert!(matches!(r.behavior, PermissionBehavior::Allow));
        assert!(matches!(r.source, PermissionRuleSource::Session));
    }

    #[test]
    fn matches_tool_returns_true_for_same_tool_name() {
        let r = PermissionRule::allow_tool_session("Bash");
        assert!(r.matches_tool("Bash"));
        assert!(!r.matches_tool("Read"));
    }

    #[test]
    fn matches_tool_returns_false_when_rule_content_is_some() {
        let r = PermissionRule {
            value: PermissionRuleValue {
                tool_name: "Bash".to_string(),
                rule_content: Some("ls".to_string()),
            },
            behavior: PermissionBehavior::Allow,
            source: PermissionRuleSource::Session,
        };
        assert!(!r.matches_tool("Bash"));
    }

    #[test]
    fn matches_tool_returns_false_when_behavior_is_deny() {
        let r = PermissionRule {
            value: PermissionRuleValue {
                tool_name: "Bash".to_string(),
                rule_content: None,
            },
            behavior: PermissionBehavior::Deny,
            source: PermissionRuleSource::Session,
        };
        assert!(!r.matches_tool("Bash"));
    }
}
