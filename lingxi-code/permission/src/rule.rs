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

impl PermissionRuleValue {
    /// Parse a permission-rule string (`"Tool"` or `"Tool(content)"`) into a
    /// [`PermissionRuleValue`]. 1:1 with claude-code
    /// `permissionRuleValueFromString` (`utils/permissions/permissionRuleParser.ts`):
    ///
    /// - Splits on the FIRST unescaped `(` and requires the LAST unescaped `)`
    ///   to be the final char; otherwise the whole string is a bare tool name.
    /// - Empty (`"Tool()"`) or wildcard (`"Tool(*)"`) content collapses to a
    ///   tool-wide rule (`rule_content: None`).
    /// - Content is unescaped (`\(`→`(`, `\)`→`)`, `\\`→`\`).
    /// - The tool name is run through [`normalize_legacy_tool_name`].
    #[must_use]
    pub fn from_rule_string(rule: &str) -> Self {
        let chars: Vec<char> = rule.chars().collect();
        let bare = || Self {
            tool_name: normalize_legacy_tool_name(rule),
            rule_content: None,
        };
        let Some(open) = find_first_unescaped(&chars, '(') else {
            return bare();
        };
        let Some(close) = find_last_unescaped(&chars, ')') else {
            return bare();
        };
        // Malformed / content-after-close → treat the whole string as a tool name.
        if close <= open || close != chars.len() - 1 {
            return bare();
        }
        let tool_name: String = chars[..open].iter().collect();
        if tool_name.is_empty() {
            return bare();
        }
        let raw_content: String = chars[open + 1..close].iter().collect();
        // `Tool()` / `Tool(*)` are tool-wide.
        if raw_content.is_empty() || raw_content == "*" {
            return Self {
                tool_name: normalize_legacy_tool_name(&tool_name),
                rule_content: None,
            };
        }
        Self {
            tool_name: normalize_legacy_tool_name(&tool_name),
            rule_content: Some(unescape_rule_content(&raw_content)),
        }
    }

    /// Serialize back to the `"Tool"` / `"Tool(content)"` rule-string form
    /// (1:1 with claude-code `permissionRuleValueToString`; content parens are
    /// escaped). Round-trips with [`Self::from_rule_string`].
    #[must_use]
    pub fn to_rule_string(&self) -> String {
        match self.rule_content.as_deref() {
            None | Some("") => self.tool_name.clone(),
            Some(content) => format!("{}({})", self.tool_name, escape_rule_content(content)),
        }
    }
}

/// Map a legacy tool name to its canonical name (claude-code
/// `LEGACY_TOOL_NAME_ALIASES`, external/non-ant build — the KAIROS-gated
/// `Brief` alias is omitted). Applied on parse so rules/hooks resolve to the
/// current name.
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

/// Escape rule content for `Tool(content)` storage (claude-code
/// `escapeRuleContent`). Order matters: backslashes first, then parens.
fn escape_rule_content(content: &str) -> String {
    content
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Reverse of [`escape_rule_content`] (claude-code `unescapeRuleContent`):
/// parens first, then backslashes.
fn unescape_rule_content(content: &str) -> String {
    content
        .replace("\\(", "(")
        .replace("\\)", ")")
        .replace("\\\\", "\\")
}

/// Index of the FIRST `target` char preceded by an EVEN number of backslashes
/// (i.e. not escaped). claude-code `findFirstUnescapedChar`.
fn find_first_unescaped(chars: &[char], target: char) -> Option<usize> {
    (0..chars.len()).find(|&i| chars[i] == target && is_unescaped(chars, i))
}

/// Index of the LAST unescaped `target` char. claude-code `findLastUnescapedChar`.
fn find_last_unescaped(chars: &[char], target: char) -> Option<usize> {
    (0..chars.len())
        .rev()
        .find(|&i| chars[i] == target && is_unescaped(chars, i))
}

/// `true` if the char at `i` is preceded by an even number of `\`.
fn is_unescaped(chars: &[char], i: usize) -> bool {
    let mut backslashes = 0usize;
    let mut j = i;
    while j > 0 && chars[j - 1] == '\\' {
        backslashes += 1;
        j -= 1;
    }
    backslashes % 2 == 0
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
    /// `~/.lingxi/settings.json` (global user config).
    UserSettings,
    /// `.lingxi/settings.json` checked into the project.
    ProjectSettings,
    /// `.lingxi/settings.local.json` (gitignored per-clone overrides).
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
    /// Citation precedence matching claude-code's `Szn` walk
    /// (`userSettings` → `projectSettings` → `localSettings` → `flagSettings`
    /// → `policySettings` → `cliArg` → `command` → `session`), where the FIRST
    /// source in the walk wins citation. Encoded so that **higher index = higher
    /// precedence** (`userSettings` highest) — #35 (previously `session` was
    /// highest, the reverse of claude-code). NOTE: citation only; the deny-wins
    /// DECISION is behavior-first and independent of this rank.
    #[must_use]
    pub fn priority(self) -> u8 {
        match self {
            Self::Session => 0,
            Self::Command => 1,
            Self::CliArg => 2,
            Self::PolicySettings => 3,
            Self::FlagSettings => 4,
            Self::LocalSettings => 5,
            Self::ProjectSettings => 6,
            Self::UserSettings => 7,
        }
    }

    /// The RAW claude-code `SettingSource` identifier string for this source.
    ///
    /// This is the camelCase token claude-code interpolates verbatim into rule
    /// citations such as the `AgentTypeError` deny message
    /// `… from ${rule.source}.` — distinct from the human display strings in
    /// [`crate::shadow::format_source`] (`"user settings"` etc.). Byte-locked to
    /// claude-code's `SettingSource` union.
    #[must_use]
    pub fn claude_settings_source(self) -> &'static str {
        match self {
            Self::UserSettings => "userSettings",
            Self::ProjectSettings => "projectSettings",
            Self::LocalSettings => "localSettings",
            Self::FlagSettings => "flagSettings",
            Self::PolicySettings => "policySettings",
            Self::CliArg => "cliArg",
            Self::Command => "command",
            Self::Session => "session",
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
    fn user_settings_outrank_session() {
        // #35: claude-code's Szn cites userSettings before session, so
        // userSettings has the higher citation precedence (NOT session).
        assert!(
            PermissionRuleSource::UserSettings.priority()
                > PermissionRuleSource::Session.priority()
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

    // ── rule-string parser (1:1 with claude-code permissionRuleParser.ts) ──

    fn parse(s: &str) -> PermissionRuleValue {
        PermissionRuleValue::from_rule_string(s)
    }

    #[test]
    fn parse_bare_tool_name() {
        let v = parse("Bash");
        assert_eq!(v.tool_name, "Bash");
        assert!(v.rule_content.is_none());
    }

    #[test]
    fn parse_tool_with_content() {
        let v = parse("Bash(npm install)");
        assert_eq!(v.tool_name, "Bash");
        assert_eq!(v.rule_content.as_deref(), Some("npm install"));
    }

    #[test]
    fn parse_empty_and_wildcard_collapse_to_tool_wide() {
        assert!(parse("Bash()").rule_content.is_none());
        assert!(parse("Bash(*)").rule_content.is_none());
        assert_eq!(parse("Bash(*)").tool_name, "Bash");
    }

    #[test]
    fn parse_unescapes_escaped_parens() {
        // claude-code doc example: Bash(python -c "print\(1\)")
        let v = parse(r#"Bash(python -c "print\(1\)")"#);
        assert_eq!(v.tool_name, "Bash");
        assert_eq!(v.rule_content.as_deref(), Some(r#"python -c "print(1)""#));
    }

    #[test]
    fn parse_malformed_treated_as_tool_name() {
        // No close paren, content after close, or empty tool name → bare name.
        assert_eq!(parse("Bash(npm").tool_name, "Bash(npm");
        assert!(parse("Bash(npm").rule_content.is_none());
        assert_eq!(parse("Bash(x)y").tool_name, "Bash(x)y");
        assert_eq!(parse("(foo)").tool_name, "(foo)");
    }

    #[test]
    fn parse_normalizes_legacy_tool_names() {
        assert_eq!(parse("Task").tool_name, "Agent");
        assert_eq!(parse("Task(general-purpose)").tool_name, "Agent");
        assert_eq!(parse("KillShell").tool_name, "TaskStop");
        assert_eq!(parse("AgentOutputTool").tool_name, "TaskOutput");
        assert_eq!(parse("BashOutputTool").tool_name, "TaskOutput");
    }

    #[test]
    fn to_rule_string_round_trips() {
        for s in [
            "Bash",
            "Bash(npm install)",
            "Edit(src/**)",
            "WebFetch(domain:example.com)",
        ] {
            assert_eq!(parse(s).to_rule_string(), s, "round-trip {s}");
        }
        // Parens in content are escaped on the way out and survive a re-parse.
        let v = parse(r#"Bash(echo "a\(b\)")"#);
        let s = v.to_rule_string();
        assert_eq!(s, r#"Bash(echo "a\(b\)")"#);
        assert_eq!(parse(&s).rule_content, v.rule_content);
        // None / empty content serialize to the bare tool name.
        assert_eq!(
            PermissionRuleValue {
                tool_name: "Read".into(),
                rule_content: None
            }
            .to_rule_string(),
            "Read"
        );
    }
}
