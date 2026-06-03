//! `PermissionPolicy::authorize` — rule-driven decision + mode fallback.
//!
//! Classifiers and shadow detection are wired in Plan 03 (Tools System).
//! M1.3 evaluates rules in priority order (deny first, then allow) and
//! falls back to the active mode for unmatched calls.

use crate::denial_tracking::DenialTrackingState;
use crate::filesystem::{file_tool_kind, input_path_for_tool, path_matches_rule_pattern, FsRoots, FileToolKind};
use crate::mode::PermissionMode;
use crate::result::{
    PermissionDecisionReason, PermissionMetadata, PermissionPrompt, PermissionResult,
};
use crate::rule::{PermissionBehavior, PermissionRule, PermissionRuleSource};
use std::collections::HashMap;
use std::sync::Mutex;

/// Rule-driven authorization policy.
///
/// Holds three rule buckets (allow / deny / ask) keyed by source, the active
/// mode, denial-tracking state (populated in Plan 03), and a flag that gates
/// `BypassPermissions` to defend against rogue automation.
pub struct PermissionPolicy {
    /// Active mode (drives fallback when no rule matches).
    pub mode: PermissionMode,
    /// Allow rules grouped by source.
    pub allow_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    /// Deny rules grouped by source.
    pub deny_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    /// Ask rules grouped by source.
    pub ask_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    /// Consecutive-denial counters (Plan 03 populates this).
    pub denial_tracking: Mutex<DenialTrackingState>,
    /// Killswitch that overrides `BypassPermissions` back to `Ask`.
    pub bypass_killswitch_active: bool,
    /// Filesystem roots for per-tool file-path CONTENT matching (phase 3a).
    /// `None` preserves the phase-2 tool-wide behavior (content ignored,
    /// matched by exact tool name); production sets this via [`Self::with_roots`]
    /// so `Edit(src/**)` / `Read(./secrets/**)` match the input path.
    pub roots: Option<FsRoots>,
}

impl PermissionPolicy {
    /// Build a fresh policy with no rules and the given mode.
    #[must_use]
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            allow_rules: HashMap::new(),
            deny_rules: HashMap::new(),
            ask_rules: HashMap::new(),
            denial_tracking: Mutex::new(DenialTrackingState::default()),
            bypass_killswitch_active: false,
            roots: None,
        }
    }

    /// Enable phase-3a file-path content matching by supplying the filesystem
    /// roots a rule's [`PermissionRuleSource`] resolves against. Without this,
    /// content rules for file tools fall back to phase-2 tool-wide matching.
    #[must_use]
    pub fn with_roots(mut self, roots: FsRoots) -> Self {
        // Relative-path matching is meaningless against a relative cwd: every
        // absolute tool path would resolve `../…` and silently stop matching.
        // Production always supplies an absolute cwd (`std::env::current_dir`).
        debug_assert!(
            roots.cwd.is_absolute(),
            "FsRoots.cwd must be absolute for correct file-path matching"
        );
        self.roots = Some(roots);
        self
    }

    /// Build a policy with `mode`, bucketing `rules` into the allow/deny/ask
    /// maps keyed by source. The foundation for enforcement: the loaded rules
    /// (e.g. from [`crate::loader::permission_rules_from_settings_json`]) land
    /// in the same buckets [`Self::authorize`] already evaluates. Wiring a gate
    /// over the resulting policy is a later phase.
    #[must_use]
    pub fn from_rules(mode: PermissionMode, rules: impl IntoIterator<Item = PermissionRule>) -> Self {
        let mut policy = Self::new(mode);
        for rule in rules {
            let bucket = match rule.behavior {
                PermissionBehavior::Allow => &mut policy.allow_rules,
                PermissionBehavior::Deny => &mut policy.deny_rules,
                PermissionBehavior::Ask => &mut policy.ask_rules,
            };
            bucket.entry(rule.source).or_default().push(rule);
        }
        policy
    }

    /// Resolve a tool call to a [`PermissionResult`].
    ///
    /// Evaluation order:
    /// 1. Deny rules, walked from highest to lowest source priority.
    /// 2. Allow rules, same order.
    /// 3. Mode fallback (`Default`/`Plan`/`AcceptEdits` ask the user,
    ///    `BypassPermissions` allows unless the killswitch is set, `DontAsk`
    ///    denies).
    ///
    /// Rule matching ([`Self::rule_matches`]): TOOL-WIDE rules match by exact
    /// tool name (claude-code `toolMatchesRule`); CONTENT rules for FILE tools
    /// match the input's path via [`crate::filesystem`] grouping (`Edit`→all
    /// editors, `Read`→all readers, edit-allow⇒read-allow) when [`Self::roots`]
    /// is set (phase 3a). Content rules for NON-file tools keep phase-2
    /// tool-wide matching (`Bash`/`WebFetch` content matching is the separate
    /// 3a-bash deferral). NOTE: `ask_rules` are not consulted here (a
    /// pre-existing gap across ALL tools — unmatched calls fall to the mode);
    /// and the wider `checkRead/checkWritePermissionForTool` allowances
    /// (working-directory auto-allow, internal/plan/scratchpad paths, `.git`/
    /// `.claude` safety asks, UNC checks) are NOT modeled — `Read`'s mode-ask
    /// is auto-allowed by `PolicyPermissionGate` (read-only default), which
    /// compensates for the missing working-dir allow.
    #[must_use]
    pub fn authorize(&self, tool_name: &str, input: &serde_json::Value) -> PermissionResult {
        // Source order matches D1 priority (highest → lowest).
        let sources = [
            PermissionRuleSource::Session,
            PermissionRuleSource::Command,
            PermissionRuleSource::CliArg,
            PermissionRuleSource::PolicySettings,
            PermissionRuleSource::FlagSettings,
            PermissionRuleSource::LocalSettings,
            PermissionRuleSource::ProjectSettings,
            PermissionRuleSource::UserSettings,
        ];

        // Deny first.
        for src in &sources {
            if let Some(rules) = self.deny_rules.get(src) {
                if let Some(rule) = rules.iter().find(|r| self.rule_matches(r, tool_name, input)) {
                    return deny_with_rule(rule);
                }
            }
        }
        // Then allow.
        for src in &sources {
            if let Some(rules) = self.allow_rules.get(src) {
                if let Some(rule) = rules.iter().find(|r| self.rule_matches(r, tool_name, input)) {
                    return allow_with_rule(rule);
                }
            }
        }
        // Mode fallback.
        match self.mode {
            PermissionMode::BypassPermissions if !self.bypass_killswitch_active => {
                allow_with_mode(PermissionMode::BypassPermissions)
            }
            PermissionMode::DontAsk => deny_with_mode(PermissionMode::DontAsk),
            _ => ask_with_mode(self.mode, tool_name),
        }
    }

    /// Does `rule` apply to a call of `tool_name` with `input`?
    ///
    /// - No [`Self::roots`] → phase-2 verbatim: exact tool-name match (content
    ///   ignored). Preserves all pre-3a behavior and tests.
    /// - TOOL-WIDE rule (`rule_content == None`) → exact tool-name match
    ///   (claude-code `toolMatchesRule`; MCP server-level wildcard is not
    ///   modeled — phase-2 parity).
    /// - CONTENT rule on a NON-file tool → exact tool-name match (3a-bash
    ///   deferral: `Bash`/`WebFetch` content still matches tool-wide).
    /// - CONTENT rule on a file tool → group + path match:
    ///   - editor tool consults `Edit`-named rules;
    ///   - reader tool consults `Read`-named rules, plus `Edit`-named rules
    ///     with `Allow` behavior (edit-allow ⇒ read-allow). An `Edit`-named
    ///     DENY rule never blocks a read (claude-code `checkRead` only consults
    ///     `read` deny rules) — the `behavior == Allow` clause enforces this
    ///     because deny rules are only ever evaluated from the deny bucket.
    fn rule_matches(&self, rule: &PermissionRule, tool_name: &str, input: &serde_json::Value) -> bool {
        let Some(roots) = self.roots.as_ref() else {
            return rule.value.tool_name == tool_name;
        };
        let Some(pattern) = rule.value.rule_content.as_deref() else {
            // Tool-wide rule → exact tool-name match.
            return rule.value.tool_name == tool_name;
        };
        let group_ok = match file_tool_kind(tool_name) {
            FileToolKind::NonFile => return rule.value.tool_name == tool_name,
            FileToolKind::Editor => rule.value.tool_name == "Edit",
            FileToolKind::Reader => {
                rule.value.tool_name == "Read"
                    || (rule.value.tool_name == "Edit"
                        && matches!(rule.behavior, PermissionBehavior::Allow))
            }
        };
        if !group_ok {
            return false;
        }
        let Some(path) = input_path_for_tool(tool_name, input, roots) else {
            return false;
        };
        path_matches_rule_pattern(&path, pattern, rule.source, roots)
    }
}

fn allow_with_rule(rule: &PermissionRule) -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::MatchedRule { rule: rule.clone() },
        updated_input: None,
        update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}

fn deny_with_rule(rule: &PermissionRule) -> PermissionResult {
    PermissionResult::Deny {
        reason: PermissionDecisionReason::MatchedRule { rule: rule.clone() },
        explanation: None,
        metadata: PermissionMetadata::default(),
    }
}

fn allow_with_mode(mode: PermissionMode) -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::PermissionMode { mode },
        updated_input: None,
        update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}

fn deny_with_mode(mode: PermissionMode) -> PermissionResult {
    PermissionResult::Deny {
        reason: PermissionDecisionReason::PermissionMode { mode },
        explanation: None,
        metadata: PermissionMetadata::default(),
    }
}

fn ask_with_mode(mode: PermissionMode, tool_name: &str) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::PermissionMode { mode },
        prompt: PermissionPrompt {
            title: format!("Allow {tool_name}?"),
            message: "The agent wants to use this tool.".into(),
            options: vec!["Allow once".into(), "Always allow".into(), "Deny".into()],
        },
        pending_classifier_check: None,
        metadata: PermissionMetadata::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::{PermissionBehavior, PermissionRuleValue};

    #[test]
    fn default_mode_asks_for_unknown_tool() {
        let p = PermissionPolicy::new(PermissionMode::Default);
        let r = p.authorize("Bash", &serde_json::json!({}));
        assert!(matches!(r, PermissionResult::Ask { .. }));
    }

    #[test]
    fn deny_rule_wins_over_allow() {
        let mut p = PermissionPolicy::new(PermissionMode::Default);
        p.allow_rules
            .entry(PermissionRuleSource::UserSettings)
            .or_default()
            .push(PermissionRule {
                value: PermissionRuleValue {
                    tool_name: "Bash".into(),
                    rule_content: None,
                },
                behavior: PermissionBehavior::Allow,
                source: PermissionRuleSource::UserSettings,
            });
        p.deny_rules
            .entry(PermissionRuleSource::ProjectSettings)
            .or_default()
            .push(PermissionRule {
                value: PermissionRuleValue {
                    tool_name: "Bash".into(),
                    rule_content: None,
                },
                behavior: PermissionBehavior::Deny,
                source: PermissionRuleSource::ProjectSettings,
            });
        let r = p.authorize("Bash", &serde_json::json!({}));
        assert!(matches!(r, PermissionResult::Deny { .. }));
    }

    #[test]
    fn dontask_denies_unmatched() {
        let p = PermissionPolicy::new(PermissionMode::DontAsk);
        assert!(matches!(
            p.authorize("Bash", &serde_json::json!({})),
            PermissionResult::Deny { .. }
        ));
    }

    #[test]
    fn bypass_allows_unmatched() {
        let p = PermissionPolicy::new(PermissionMode::BypassPermissions);
        assert!(matches!(
            p.authorize("Bash", &serde_json::json!({})),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn from_rules_buckets_by_behavior_and_authorizes() {
        // The loader → policy → authorize foundation: a deny rule lands in the
        // deny bucket and wins; an allow rule lands in the allow bucket.
        let rules = crate::loader::permission_rules_from_settings_json(
            r#"{ "permissions": { "allow": ["Read"], "deny": ["Bash"], "ask": ["WebFetch"] } }"#,
            PermissionRuleSource::UserSettings,
        )
        .unwrap();
        let p = PermissionPolicy::from_rules(PermissionMode::Default, rules);
        assert_eq!(p.allow_rules.values().flatten().count(), 1);
        assert_eq!(p.deny_rules.values().flatten().count(), 1);
        assert_eq!(p.ask_rules.values().flatten().count(), 1);
        // Bash is denied by rule (tool-wide), Read allowed, WebFetch falls to
        // its ask rule, an unmatched tool falls to the Default-mode ask.
        assert!(matches!(
            p.authorize("Bash", &serde_json::json!({})),
            PermissionResult::Deny { .. }
        ));
        assert!(matches!(
            p.authorize("Read", &serde_json::json!({})),
            PermissionResult::Allow { .. }
        ));
        assert!(matches!(
            p.authorize("Other", &serde_json::json!({})),
            PermissionResult::Ask { .. }
        ));
    }

    // ── phase 3a: file-path content matching ──────────────────────────────

    use crate::filesystem::FsRoots;
    use std::path::PathBuf;

    fn roots() -> FsRoots {
        FsRoots {
            cwd: PathBuf::from("/proj"),
            home: Some(PathBuf::from("/home/u")),
            claude_home: PathBuf::from("/home/u/.claude"),
        }
    }

    fn policy_with_roots(raw: &str, mode: PermissionMode) -> PermissionPolicy {
        let rules = crate::loader::permission_rules_from_settings_json(
            raw,
            PermissionRuleSource::ProjectSettings,
        )
        .unwrap();
        PermissionPolicy::from_rules(mode, rules).with_roots(roots())
    }

    fn edit(path: &str) -> serde_json::Value {
        serde_json::json!({ "file_path": path })
    }

    #[test]
    fn content_allow_rule_matches_only_matching_path() {
        let p = policy_with_roots(
            r#"{ "permissions": { "allow": ["Edit(src/**)"] } }"#,
            PermissionMode::Default,
        );
        // Edit inside src → allowed by rule.
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/src/main.rs")),
            PermissionResult::Allow { .. }
        ));
        // Edit outside src → no rule match → falls to Default-mode ask.
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/tests/x.rs")),
            PermissionResult::Ask { .. }
        ));
    }

    #[test]
    fn content_deny_rule_denies_only_matching_path() {
        let p = policy_with_roots(
            r#"{ "permissions": { "deny": ["Read(./secrets/**)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Read", &edit("/proj/secrets/key.pem")),
            PermissionResult::Deny { .. }
        ));
        // A read elsewhere is NOT denied (precise, unlike phase-2 tool-wide).
        assert!(matches!(
            p.authorize("Read", &edit("/proj/src/main.rs")),
            PermissionResult::Ask { .. }
        ));
    }

    #[test]
    fn edit_rule_groups_to_all_editors() {
        // An `Edit(...)` deny rule must apply to Write / NotebookEdit too.
        // (cwd-relative pattern so the test isolates grouping, not root
        // resolution — `/etc/**` would anchor to the project root, not `/etc`.)
        let p = policy_with_roots(
            r#"{ "permissions": { "deny": ["Edit(build/**)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Write", &edit("/proj/build/out.o")),
            PermissionResult::Deny { .. }
        ));
        assert!(matches!(
            p.authorize("NotebookEdit", &serde_json::json!({ "notebook_path": "/proj/build/x.ipynb" })),
            PermissionResult::Deny { .. }
        ));
    }

    #[test]
    fn edit_allow_implies_read_allow() {
        // An `Edit(src/**)` ALLOW rule also permits reading src/**.
        let p = policy_with_roots(
            r#"{ "permissions": { "allow": ["Edit(src/**)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Read", &edit("/proj/src/main.rs")),
            PermissionResult::Allow { .. }
        ));
        // Grep (a reader, search root inside src) is likewise allowed.
        assert!(matches!(
            p.authorize("Grep", &serde_json::json!({ "path": "/proj/src" })),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn edit_deny_does_not_block_reads() {
        // claude-code `checkRead` only consults READ deny rules — an edit-deny
        // never blocks a read.
        let p = policy_with_roots(
            r#"{ "permissions": { "deny": ["Edit(src/**)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Read", &edit("/proj/src/main.rs")),
            PermissionResult::Ask { .. }
        ));
        // …but it DOES block the editing tools.
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/src/main.rs")),
            PermissionResult::Deny { .. }
        ));
    }

    #[test]
    fn deny_beats_allow_at_path_level() {
        let p = policy_with_roots(
            r#"{ "permissions": { "allow": ["Edit(src/**)"], "deny": ["Edit(src/secret.rs)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/src/secret.rs")),
            PermissionResult::Deny { .. }
        ));
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/src/ok.rs")),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn without_roots_content_rule_matches_tool_wide_phase2() {
        // No roots → phase-2 behavior: a content rule matches the tool name
        // regardless of path (content ignored).
        let rules = crate::loader::permission_rules_from_settings_json(
            r#"{ "permissions": { "allow": ["Edit(src/**)"] } }"#,
            PermissionRuleSource::ProjectSettings,
        )
        .unwrap();
        let p = PermissionPolicy::from_rules(PermissionMode::Default, rules);
        // Any Edit path is allowed (over-broad — the documented phase-2 limit).
        assert!(matches!(
            p.authorize("Edit", &edit("/anywhere/x.rs")),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn non_file_content_rule_still_matches_tool_wide() {
        // Bash content matching is the 3a-bash deferral: `Bash(npm run *)`
        // matches the Bash tool regardless of args, even with roots set.
        let p = policy_with_roots(
            r#"{ "permissions": { "deny": ["Bash(rm -rf /)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Bash", &serde_json::json!({ "command": "echo hi" })),
            PermissionResult::Deny { .. }
        ));
    }

    #[test]
    fn user_settings_content_rule_roots_at_claude_home() {
        // `/x/**` in a USER-settings rule resolves against ~/.claude, not cwd.
        let rules = crate::loader::permission_rules_from_settings_json(
            r#"{ "permissions": { "deny": ["Read(/agents/**)"] } }"#,
            PermissionRuleSource::UserSettings,
        )
        .unwrap();
        let p = PermissionPolicy::from_rules(PermissionMode::Default, rules).with_roots(roots());
        assert!(matches!(
            p.authorize("Read", &edit("/home/u/.claude/agents/foo.md")),
            PermissionResult::Deny { .. }
        ));
        // Same relative path under cwd is NOT denied (different root).
        assert!(matches!(
            p.authorize("Read", &edit("/proj/agents/foo.md")),
            PermissionResult::Ask { .. }
        ));
    }
}
