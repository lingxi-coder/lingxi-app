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
}
