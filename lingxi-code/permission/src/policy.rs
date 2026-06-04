//! `PermissionPolicy::authorize` — rule-driven decision + mode fallback.
//!
//! Classifiers and shadow detection are wired in Plan 03 (Tools System).
//! M1.3 evaluates rules in priority order (deny first, then allow) and
//! falls back to the active mode for unmatched calls.

use crate::auto_edit_safety::{check_path_safety_for_auto_edit, AutoEditSafety};
use crate::denial_tracking::DenialTrackingState;
use crate::filesystem::{
    file_tool_kind, input_path_for_tool, path_in_allowed_working_path, path_matches_rule_pattern,
    FileToolKind, FsRoots,
};
use crate::mode::PermissionMode;
use crate::result::{
    PermissionDecisionReason, PermissionMetadata, PermissionPrompt, PermissionResult,
};
use crate::rule::{PermissionBehavior, PermissionRule, PermissionRuleSource};
use crate::shell_command;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Rule sources in DESCENDING priority (highest → lowest), matching D1.
/// `authorize` walks every behavior bucket in this order.
const SOURCES_BY_PRIORITY: [PermissionRuleSource; 8] = [
    PermissionRuleSource::Session,
    PermissionRuleSource::Command,
    PermissionRuleSource::CliArg,
    PermissionRuleSource::PolicySettings,
    PermissionRuleSource::FlagSettings,
    PermissionRuleSource::LocalSettings,
    PermissionRuleSource::ProjectSettings,
    PermissionRuleSource::UserSettings,
];

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
    /// Allow rules stripped out on entry to [`PermissionMode::Auto`] because
    /// they would bypass the auto-mode classifier (`Bash(python:*)`, `Agent(*)`,
    /// `PowerShell(iex:*)`, …). Stashed here by
    /// [`Self::strip_dangerous_for_auto`] and re-added verbatim by
    /// [`Self::restore_dangerous`] when leaving Auto. Empty in every other mode.
    /// 1:1 with the TS `strippedDangerousRules` field on `ToolPermissionContext`.
    pub stripped_dangerous: Vec<PermissionRule>,
    /// Per-stripped-rule `(source, original_index_within_source_bucket)`, kept
    /// parallel to [`Self::stripped_dangerous`] so [`Self::restore_dangerous`]
    /// re-inserts each rule at its original position — making `strip → restore`
    /// an EXACT identity (TS only guarantees set-equality via `applyPermissionUpdate`
    /// re-add; the position record is a Rust-side strengthening, documented).
    stripped_positions: Vec<(PermissionRuleSource, usize)>,
    /// Extra directories (beyond `roots.cwd`) inside which `AcceptEdits` mode
    /// auto-allows safe file edits. 1:1 with the keys of the TS
    /// `ToolPermissionContext.additionalWorkingDirectories` map, which
    /// `allWorkingDirectories` (`filesystem.ts:667-674`) unions with the original
    /// cwd. Empty by default; set via [`Self::with_working_dirs`]. Production
    /// engine wiring is deferred this batch (cwd comes from `roots.cwd`).
    pub additional_working_dirs: Vec<PathBuf>,
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
            stripped_dangerous: Vec::new(),
            stripped_positions: Vec::new(),
            additional_working_dirs: Vec::new(),
        }
    }

    /// Set the extra working directories inside which `AcceptEdits` mode
    /// auto-allows safe edits (beyond `roots.cwd`). Mirrors seeding the keys of
    /// TS `ToolPermissionContext.additionalWorkingDirectories`. Backward-compatible
    /// (default empty); production may leave it unset this batch.
    #[must_use]
    pub fn with_working_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.additional_working_dirs = dirs;
        self
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
    /// Evaluation order (claude-code `checkPermissionsForToolUse` skeleton):
    /// 1. Deny rules, walked from highest to lowest source priority.
    /// 2. Ask rules, same order (NEW — a matching ask rule forces a prompt).
    /// 3. Allow rules, same order.
    /// 4. Mode fallback (`Default`/`Plan`/`AcceptEdits` ask the user,
    ///    `BypassPermissions` allows unless the killswitch is set, `DontAsk`
    ///    denies).
    ///
    /// Rule matching ([`Self::rule_matches`]): TOOL-WIDE rules match by exact
    /// tool name (claude-code `toolMatchesRule`); CONTENT rules for FILE tools
    /// match the input's path via [`crate::filesystem`] grouping (phase 3a);
    /// CONTENT rules for SHELL tools (`Bash`/`PowerShell`) match the command
    /// per [`crate::shell_command`] — deny/ask match if ANY subcommand matches
    /// (aggressive wrapper/env stripping), allow requires EVERY subcommand
    /// covered (3a-bash). Other NON-file content rules stay tool-wide
    /// (`WebFetch` domain matching is a separate deferral). Shell + file content
    /// matching is active only when [`Self::roots`] is set (production always
    /// sets it); without roots the phase-2 tool-wide behavior is preserved.
    ///
    /// The wider `checkRead/checkWritePermissionForTool` allowances
    /// (working-directory auto-allow, `.git`/`.claude` safety asks, path/sed/
    /// mode constraints) are still NOT modeled — `Read`'s mode-ask is
    /// auto-allowed by `PolicyPermissionGate` (read-only default).
    #[must_use]
    pub fn authorize(&self, tool_name: &str, input: &serde_json::Value) -> PermissionResult {
        let sources = SOURCES_BY_PRIORITY;

        // Precedence mirrors claude-code `hasPermissionsToUseToolInner`:
        //   1a tool-wide deny → 1b tool-wide ask → 1c content deny → content ask
        //   → allow → mode.
        // The tool-wide ask SHORT-CIRCUITS before any content deny (a project
        // that asks on all of a tool, yet also denies one command, gets the ask).
        // 1a. Tool-wide deny.
        if let Some(rule) = self.first_match(&self.deny_rules, &sources, tool_name, input, false) {
            return deny_with_rule(rule);
        }
        // 1b. Tool-wide ask.
        if let Some(rule) = self.first_match(&self.ask_rules, &sources, tool_name, input, false) {
            return ask_with_rule(rule, tool_name);
        }
        // 1c. Content deny.
        if let Some(rule) = self.first_match(&self.deny_rules, &sources, tool_name, input, true) {
            return deny_with_rule(rule);
        }
        // Content ask — a matching ask rule prompts (the gate's read-only
        // default may still auto-allow, but the rule is honored).
        if let Some(rule) = self.first_match(&self.ask_rules, &sources, tool_name, input, true) {
            return ask_with_rule(rule, tool_name);
        }
        // 3. Allow. Shell tools need compound aggregation (a single allow rule
        //    matching ONE subcommand must not allow a whole compound command),
        //    so they take a dedicated path rather than the per-rule walk.
        if self.roots.is_some() && shell_command::is_shell_tool(tool_name) {
            if let Some(rule) = self.shell_allow(tool_name, input, &sources) {
                return allow_with_rule(rule);
            }
        } else {
            for src in &sources {
                if let Some(rules) = self.allow_rules.get(src) {
                    if let Some(rule) =
                        rules.iter().find(|r| self.rule_matches(r, tool_name, input))
                    {
                        return allow_with_rule(rule);
                    }
                }
            }
        }
        // 3a. AcceptEdits working-dir auto-allow (claude-code `checkWritePermissionForTool`
        //     step 3, `filesystem.ts:1360-1375`). In `AcceptEdits` mode an EDITOR
        //     tool whose target path (a) passes the auto-edit safety guard
        //     (`checkPathSafetyForAutoEdit`, Batch 2 — runs FIRST at `:1242`+, so
        //     `.git`/`.claude`/dangerous/suspicious-Windows paths fall through to
        //     ask) AND (b) lives inside an allowed working dir (cwd +
        //     `additional_working_dirs`, `allWorkingDirectories` `:667-674`) is
        //     auto-allowed with a `mode` reason. This runs AFTER the deny/ask/allow
        //     walks (so explicit deny/ask rules still win — preserved by ordering)
        //     and BEFORE the generic mode fallback. On any failure the branch is
        //     simply not taken and control falls through to the `AcceptEdits`-mode
        //     ask below. Requires [`Self::roots`] (the working-dir set is derived
        //     from `roots.cwd`).
        if self.mode == PermissionMode::AcceptEdits
            && file_tool_kind(tool_name) == FileToolKind::Editor
        {
            if let Some(roots) = self.roots.as_ref() {
                if let Some(raw_path) = input_path_for_tool(tool_name, input, roots) {
                    // Safety guard runs first (Batch 2). Only a `Safe` verdict may
                    // be auto-allowed; an `Unsafe` path falls through to ask.
                    if check_path_safety_for_auto_edit(&raw_path, roots) == AutoEditSafety::Safe {
                        // Working-dir set = cwd + additional dirs (`allWorkingDirectories`).
                        let mut working_dirs = Vec::with_capacity(1 + self.additional_working_dirs.len());
                        working_dirs.push(roots.cwd.clone());
                        working_dirs.extend(self.additional_working_dirs.iter().cloned());
                        if path_in_allowed_working_path(Path::new(raw_path.as_ref()), &working_dirs, roots) {
                            return allow_with_mode(PermissionMode::AcceptEdits);
                        }
                    }
                }
            }
        }
        // 3b. Plan-mode mutation backstop (claude-code `prepareContextForPlanMode`,
        //     `permissionSetup.ts:1462-1500`). In `Plan` mode, the primary
        //     enforcement is that mutating tools are NOT advertised on the wire
        //     (a conversation-layer concern, out of scope here); this is the
        //     permission-layer backstop. A tool that is NOT on the read-only /
        //     planning-safe allowlist ([`crate::mode_policy::is_plan_safe_tool`],
        //     the external `SAFE_YOLO_ALLOWLISTED_TOOLS` subset) and that no
        //     allow rule matched is treated as a state mutation and ASKED about
        //     (NOT denied — matching TS, the user may approve and thereby exit
        //     plan-mode constraints). Deny/ask rules and explicit allow rules
        //     already won above, so they are preserved. Plan-safe tools fall
        //     through to the generic mode fallback below (and the gate's
        //     read-only auto-allow), keeping their existing path.
        if self.mode == PermissionMode::Plan && !crate::mode_policy::is_plan_safe_tool(tool_name) {
            return ask_plan_mutation(tool_name);
        }
        // 4. Mode fallback.
        match self.mode {
            PermissionMode::BypassPermissions if !self.bypass_killswitch_active => {
                allow_with_mode(PermissionMode::BypassPermissions)
            }
            PermissionMode::DontAsk => deny_with_mode(PermissionMode::DontAsk),
            _ => ask_with_mode(self.mode, tool_name),
        }
    }

    /// Strip every ALLOW rule that would bypass the auto-mode classifier (e.g.
    /// `Bash(python:*)`, `Agent(*)`, `PowerShell(iex:*)`), stashing the removed
    /// rules in [`Self::stripped_dangerous`] so [`Self::restore_dangerous`] can
    /// re-add them verbatim. 1:1 with `stripDangerousPermissionsForAutoMode`
    /// (`permissionSetup.ts:510-553`): the predicate is
    /// [`crate::dangerous_perms::is_dangerous_classifier_permission`].
    ///
    /// Deny/ask rules are never touched (only allow rules can auto-allow an
    /// arbitrary-code command ahead of the classifier). Idempotent in spirit but
    /// NOT a no-op on a fresh strip — call [`Self::restore_dangerous`] before
    /// re-stripping to avoid stacking the stash. (The mode-transition driver
    /// [`Self::set_mode`] guarantees a strip is always paired with a restore.)
    ///
    /// This does NOT change [`Self::authorize`] behavior on its own: Auto mode's
    /// classifier is unwired externally (Batch 6 stub), so stripping is
    /// behavior-neutral until a future wiring batch consumes Auto mode.
    pub fn strip_dangerous_for_auto(&mut self) {
        for (&source, rules) in &mut self.allow_rules {
            // Record each dangerous rule's ORIGINAL index within this bucket, then
            // restore re-inserts in ascending original-index order — which exactly
            // reconstructs the pre-strip vec (a removed slot's gap is re-filled
            // before any later removed slot is, so indices stay valid).
            let mut kept = Vec::with_capacity(rules.len());
            for (orig_idx, rule) in std::mem::take(rules).into_iter().enumerate() {
                if crate::dangerous_perms::is_dangerous_classifier_permission(
                    &rule.value.tool_name,
                    &rule.value.rule_content,
                ) {
                    self.stripped_dangerous.push(rule);
                    self.stripped_positions.push((source, orig_idx));
                } else {
                    kept.push(rule);
                }
            }
            *rules = kept;
        }
    }

    /// Re-add every allow rule previously stashed by
    /// [`Self::strip_dangerous_for_auto`] at its original bucket position, then
    /// clear the stash so a second call is a no-op. 1:1 with
    /// `restoreDangerousPermissions` (`permissionSetup.ts:561-579`). Exact
    /// inverse of a strip: `strip → restore` returns [`Self::allow_rules`] to its
    /// pre-strip contents (positions included — see [`Self::stripped_positions`]).
    pub fn restore_dangerous(&mut self) {
        let rules = std::mem::take(&mut self.stripped_dangerous);
        let positions = std::mem::take(&mut self.stripped_positions);
        // Re-insert in ascending recorded-index order so each rule lands back in
        // the same slot it was removed from (strip recorded indices in this order).
        for (rule, (source, idx)) in rules.into_iter().zip(positions) {
            let bucket = self.allow_rules.entry(source).or_default();
            let at = idx.min(bucket.len());
            bucket.insert(at, rule);
        }
    }

    /// Transition the active mode, running the auto-mode strip/restore
    /// side-effects. 1:1 with the strip/restore arms of `transitionPermissionMode`
    /// (`permissionSetup.ts:597-646`, the `:627-637` block):
    ///
    /// - entering `Auto` (from a non-Auto mode) → [`Self::strip_dangerous_for_auto`];
    /// - leaving `Auto` (to a non-Auto mode) → [`Self::restore_dangerous`].
    ///
    /// `to == from` is a no-op (matches the TS `fromMode === toMode` guard).
    /// The Plan-mode attachment and classifier-gate side-effects from TS are out
    /// of scope here (Plan is Batch 3; the LLM classifier is the stubbed Batch 6
    /// non-goal). Strip/restore is behavior-neutral on [`Self::authorize`] until
    /// Auto's classifier is wired, so this is safe to call now.
    pub fn set_mode(&mut self, to: PermissionMode) {
        let from = self.mode;
        if from == to {
            return;
        }
        if to == PermissionMode::Auto && from != PermissionMode::Auto {
            self.strip_dangerous_for_auto();
        } else if from == PermissionMode::Auto && to != PermissionMode::Auto {
            self.restore_dangerous();
        }
        self.mode = to;
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
            FileToolKind::NonFile => {
                // Shell tools: CONTENT rule matches the command (any-subcommand,
                // aggressive stripping). Correct for deny/ask; allow uses the
                // dedicated `shell_allow` aggregation instead of this per-rule
                // path. Other non-file tools keep tool-wide matching.
                if shell_command::is_shell_tool(tool_name) && rule.value.tool_name == tool_name {
                    let Some(command) = shell_command::command_from_input(input) else {
                        return false;
                    };
                    return shell_command::rule_matches_any_subcommand(pattern, command);
                }
                return rule.value.tool_name == tool_name;
            }
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

    /// First rule in `bucket` (walked highest→lowest source priority) that
    /// applies to this call AND is in the requested tier: `content == false`
    /// selects TOOL-WIDE rules (`rule_content == None`, claude-code
    /// `toolMatchesRule`), `content == true` selects CONTENT rules. Splitting
    /// the tiers lets `authorize` order tool-wide-ask ahead of content-deny as
    /// the TS general checker does.
    fn first_match<'a>(
        &self,
        bucket: &'a HashMap<PermissionRuleSource, Vec<PermissionRule>>,
        sources: &[PermissionRuleSource],
        tool_name: &str,
        input: &serde_json::Value,
        content: bool,
    ) -> Option<&'a PermissionRule> {
        for src in sources {
            if let Some(rules) = bucket.get(src) {
                if let Some(rule) = rules.iter().find(|r| {
                    r.value.rule_content.is_some() == content
                        && self.rule_matches(r, tool_name, input)
                }) {
                    return Some(rule);
                }
            }
        }
        None
    }

    /// Allow decision for a shell tool, with compound-command aggregation.
    ///
    /// 1. A TOOL-WIDE allow rule (`Bash` with no content) allows every command.
    /// 2. Otherwise the command is allowed only if EVERY subcommand is covered
    ///    by some CONTENT allow rule (`Bash(npm install:*)` etc.). Gathering
    ///    rules across all sources matches claude-code, where deny/ask/allow
    ///    precedence is by behavior, not source. The reported rule is the
    ///    highest-priority content allow rule.
    fn shell_allow(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        sources: &[PermissionRuleSource],
    ) -> Option<&PermissionRule> {
        // 1. Tool-wide allow → allow everything.
        for src in sources {
            if let Some(rules) = self.allow_rules.get(src) {
                if let Some(rule) = rules
                    .iter()
                    .find(|r| r.value.tool_name == tool_name && r.value.rule_content.is_none())
                {
                    return Some(rule);
                }
            }
        }
        // 2. Content allow aggregation over the command's subcommands.
        let command = shell_command::command_from_input(input)?;
        let mut content_rules: Vec<&PermissionRule> = Vec::new();
        for src in sources {
            if let Some(rules) = self.allow_rules.get(src) {
                for r in rules {
                    if r.value.tool_name == tool_name && r.value.rule_content.is_some() {
                        content_rules.push(r);
                    }
                }
            }
        }
        if content_rules.is_empty() {
            return None;
        }
        let contents: Vec<&str> = content_rules
            .iter()
            .filter_map(|r| r.value.rule_content.as_deref())
            .collect();
        if shell_command::command_fully_allowed(&contents, command) {
            content_rules.first().copied()
        } else {
            None
        }
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

fn ask_with_rule(rule: &PermissionRule, tool_name: &str) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::MatchedRule { rule: rule.clone() },
        prompt: PermissionPrompt {
            title: format!("Allow {tool_name}?"),
            message: "The agent wants to use this tool (matched an ask rule).".into(),
            options: vec!["Allow once".into(), "Always allow".into(), "Deny".into()],
        },
        pending_classifier_check: None,
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

/// Plan-mode mutation backstop ask: a tool that would mutate state in `Plan`
/// mode. Tagged with [`PermissionMode::Plan`]; the message tells the user that
/// approving exits the plan-mode constraints (matching claude-code, which
/// surfaces a plan-mode mutation as an interactive ask, NOT a hard deny).
fn ask_plan_mutation(tool_name: &str) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::PermissionMode {
            mode: PermissionMode::Plan,
        },
        prompt: PermissionPrompt {
            title: format!("Allow {tool_name}?"),
            message: "Plan mode: this tool would modify state; approve to exit \
                plan-mode constraints."
                .into(),
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

    // ── 3a-bash: shell command content matching ───────────────────────────

    fn bash(cmd: &str) -> serde_json::Value {
        serde_json::json!({ "command": cmd })
    }

    #[test]
    fn bash_deny_rule_matches_only_that_command() {
        // 3a-bash CLOSES the old tool-wide deferral: `Bash(rm:*)` denies `rm`
        // commands but NOT unrelated ones.
        let p = policy_with_roots(
            r#"{ "permissions": { "deny": ["Bash(rm:*)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Bash", &bash("rm -rf /tmp/x")),
            PermissionResult::Deny { .. }
        ));
        // An unrelated command is NOT denied (precise, unlike phase-2 tool-wide).
        assert!(matches!(
            p.authorize("Bash", &bash("echo hi")),
            PermissionResult::Ask { .. }
        ));
    }

    #[test]
    fn bash_deny_not_bypassable_by_compound_or_env() {
        let p = policy_with_roots(
            r#"{ "permissions": { "deny": ["Bash(curl:*)"] } }"#,
            PermissionMode::Default,
        );
        // denied subcommand hidden behind a benign one / a pipe / env prefix
        assert!(matches!(
            p.authorize("Bash", &bash("echo ok && curl evil.com")),
            PermissionResult::Deny { .. }
        ));
        assert!(matches!(
            p.authorize("Bash", &bash("echo x | curl evil.com")),
            PermissionResult::Deny { .. }
        ));
        assert!(matches!(
            p.authorize("Bash", &bash("HTTPS_PROXY=x curl evil.com")),
            PermissionResult::Deny { .. }
        ));
    }

    #[test]
    fn bash_allow_requires_all_subcommands_covered() {
        let p = policy_with_roots(
            r#"{ "permissions": { "allow": ["Bash(echo:*)"] } }"#,
            PermissionMode::Default,
        );
        // single covered subcommand → allow
        assert!(matches!(
            p.authorize("Bash", &bash("echo hi")),
            PermissionResult::Allow { .. }
        ));
        // compound with an UNcovered subcommand → NOT allowed (no over-allow)
        assert!(matches!(
            p.authorize("Bash", &bash("echo ok && rm -rf /")),
            PermissionResult::Ask { .. }
        ));
    }

    #[test]
    fn bash_allow_multiple_rules_cover_compound() {
        let p = policy_with_roots(
            r#"{ "permissions": { "allow": ["Bash(echo:*)", "Bash(ls:*)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Bash", &bash("echo hi && ls -l")),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn bash_toolwide_allow_still_allows_everything() {
        let p = policy_with_roots(
            r#"{ "permissions": { "allow": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Bash", &bash("anything --here")),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn bash_deny_beats_allow_for_same_command() {
        let p = policy_with_roots(
            r#"{ "permissions": { "allow": ["Bash(git:*)"], "deny": ["Bash(git push:*)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Bash", &bash("git push origin main")),
            PermissionResult::Deny { .. }
        ));
        assert!(matches!(
            p.authorize("Bash", &bash("git status")),
            PermissionResult::Allow { .. }
        ));
    }

    // ── ask-rule consultation (was a pre-existing gap across ALL tools) ────

    #[test]
    fn ask_rule_now_consulted_for_tool() {
        // A tool-wide ask rule yields Ask (previously fell through to mode).
        let p = policy_with_roots(
            r#"{ "permissions": { "ask": ["WebFetch"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("WebFetch", &serde_json::json!({ "url": "https://x" })),
            PermissionResult::Ask { .. }
        ));
    }

    #[test]
    fn ask_rule_consulted_for_bash_command() {
        let p = policy_with_roots(
            r#"{ "permissions": { "ask": ["Bash(npm publish:*)"], "allow": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        // ask beats the tool-wide allow (ask walked before allow)
        assert!(matches!(
            p.authorize("Bash", &bash("npm publish --tag beta")),
            PermissionResult::Ask { .. }
        ));
        // a non-publish command still rides the tool-wide allow
        assert!(matches!(
            p.authorize("Bash", &bash("npm test")),
            PermissionResult::Allow { .. }
        ));
    }

    #[test]
    fn toolwide_ask_short_circuits_before_content_deny() {
        // claude-code precedence: a TOOL-WIDE ask rule pre-empts a CONTENT deny
        // rule (1b before 1c). `ask:["Bash"]` + `deny:["Bash(rm:*)"]` → Ask.
        let p = policy_with_roots(
            r#"{ "permissions": { "ask": ["Bash"], "deny": ["Bash(rm:*)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Bash", &bash("rm -rf /")),
            PermissionResult::Ask { .. }
        ));
    }

    #[test]
    fn toolwide_deny_still_beats_toolwide_ask() {
        // 1a before 1b: a tool-wide deny wins over a tool-wide ask.
        let p = policy_with_roots(
            r#"{ "permissions": { "deny": ["Bash"], "ask": ["Bash"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Bash", &bash("ls")),
            PermissionResult::Deny { .. }
        ));
    }

    #[test]
    fn content_deny_beats_content_ask() {
        let p = policy_with_roots(
            r#"{ "permissions": { "deny": ["Bash(rm:*)"], "ask": ["Bash(rm:*)"] } }"#,
            PermissionMode::Default,
        );
        assert!(matches!(
            p.authorize("Bash", &bash("rm x")),
            PermissionResult::Deny { .. }
        ));
    }

    #[test]
    fn ask_rule_reason_is_matched_rule() {
        let p = policy_with_roots(
            r#"{ "permissions": { "ask": ["WebFetch"] } }"#,
            PermissionMode::Default,
        );
        match p.authorize("WebFetch", &serde_json::json!({})) {
            PermissionResult::Ask { reason, .. } => assert!(matches!(
                reason,
                PermissionDecisionReason::MatchedRule { .. }
            )),
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    // ── Batch 3: Plan-mode mutation backstop ──────────────────────────────

    #[test]
    fn plan_mode_asks_on_mutating_tool() {
        // Plan + Edit → Ask tagged with Plan mode (NOT deny), even with no rules.
        let p = PermissionPolicy::new(PermissionMode::Plan);
        match p.authorize("Edit", &edit("/proj/src/x.rs")) {
            PermissionResult::Ask { reason, prompt, .. } => {
                assert!(
                    matches!(
                        reason,
                        PermissionDecisionReason::PermissionMode {
                            mode: PermissionMode::Plan
                        }
                    ),
                    "Plan-mutation ask must be tagged with Plan mode"
                );
                assert!(
                    prompt.message.contains("Plan mode"),
                    "Plan-mutation ask carries the plan-specific message: {}",
                    prompt.message
                );
            }
            other => panic!("expected Ask(Plan), got {other:?}"),
        }
    }

    #[test]
    fn plan_mode_asks_on_bash() {
        // Plan + Bash → Ask (Bash is not plan-safe).
        let p = PermissionPolicy::new(PermissionMode::Plan);
        match p.authorize("Bash", &bash("rm -rf /")) {
            PermissionResult::Ask { reason, .. } => assert!(matches!(
                reason,
                PermissionDecisionReason::PermissionMode {
                    mode: PermissionMode::Plan
                }
            )),
            other => panic!("expected Ask(Plan), got {other:?}"),
        }
    }

    #[test]
    fn plan_mode_does_not_block_plan_safe_tools() {
        // Plan + Read/Grep/Glob → the plan backstop is NOT taken; they fall
        // through to the generic mode fallback (a plain Ask tagged Plan, which
        // the gate later auto-allows since they are read-only). The key
        // assertion is that the decision is NOT the plan-mutation ask: the
        // mode-fallback ask carries the generic message, not the "Plan mode:"
        // backstop message.
        let p = PermissionPolicy::new(PermissionMode::Plan);
        for tool in ["Read", "Grep", "Glob"] {
            match p.authorize(tool, &edit("/proj/src/main.rs")) {
                PermissionResult::Ask { prompt, .. } => assert!(
                    !prompt.message.contains("Plan mode"),
                    "{tool} is plan-safe; must not trip the mutation backstop"
                ),
                other => panic!("expected Ask for plan-safe {tool}, got {other:?}"),
            }
        }
    }

    #[test]
    fn plan_mode_explicit_allow_rule_wins_over_block() {
        // An explicit allow rule on Edit still wins in Plan mode (the allow walk
        // runs before the plan backstop).
        let p = policy_with_roots(
            r#"{ "permissions": { "allow": ["Edit(src/**)"] } }"#,
            PermissionMode::Plan,
        );
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/src/main.rs")),
            PermissionResult::Allow { .. }
        ));
        // …but an Edit outside the allow scope still trips the plan block (Ask).
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/other/x.rs")),
            PermissionResult::Ask { .. }
        ));
    }

    #[test]
    fn plan_mode_deny_rule_still_wins() {
        // A deny rule wins over the plan ask (deny walk precedes the backstop).
        let p = policy_with_roots(
            r#"{ "permissions": { "deny": ["Edit(src/**)"] } }"#,
            PermissionMode::Plan,
        );
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/src/secret.rs")),
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

    // ── Batch 4: auto-mode dangerous-permission strip/restore ─────────────

    fn allow_rule(tool: &str, content: Option<&str>) -> PermissionRule {
        PermissionRule {
            value: PermissionRuleValue {
                tool_name: tool.into(),
                rule_content: content.map(str::to_string),
            },
            behavior: PermissionBehavior::Allow,
            source: PermissionRuleSource::UserSettings,
        }
    }

    fn allow_count(p: &PermissionPolicy) -> usize {
        p.allow_rules.values().flatten().count()
    }

    fn seeded_policy(mode: PermissionMode) -> PermissionPolicy {
        let mut p = PermissionPolicy::new(mode);
        for r in [
            allow_rule("Bash", Some("python:*")), // dangerous
            allow_rule("Bash", Some("ls:*")),     // safe
            allow_rule("Agent", None),            // dangerous
            allow_rule("Read", None),             // safe
        ] {
            p.allow_rules.entry(r.source).or_default().push(r);
        }
        p
    }

    #[test]
    fn strip_removes_only_dangerous_allow_rules_and_stashes_them() {
        let mut p = seeded_policy(PermissionMode::Default);
        assert_eq!(allow_count(&p), 4);
        p.strip_dangerous_for_auto();
        // Two dangerous rules stripped (Bash(python:*) + Agent), two kept.
        assert_eq!(allow_count(&p), 2);
        assert_eq!(p.stripped_dangerous.len(), 2);
        // The kept rules are the safe ones.
        let kept: Vec<_> = p.allow_rules.values().flatten().collect();
        assert!(kept.iter().all(|r| !crate::dangerous_perms::is_dangerous_classifier_permission(
            &r.value.tool_name,
            &r.value.rule_content
        )));
    }

    #[test]
    fn restore_is_exact_inverse_of_strip() {
        let mut p = seeded_policy(PermissionMode::Default);
        let before = p.allow_rules.clone();
        p.strip_dangerous_for_auto();
        p.restore_dangerous();
        assert_eq!(p.allow_rules, before, "strip→restore must be identity");
        assert!(p.stripped_dangerous.is_empty());
    }

    #[test]
    fn second_restore_is_a_noop() {
        let mut p = seeded_policy(PermissionMode::Default);
        p.strip_dangerous_for_auto();
        p.restore_dangerous();
        let after_first = p.allow_rules.clone();
        p.restore_dangerous(); // stash already empty
        assert_eq!(p.allow_rules, after_first);
        assert!(p.stripped_dangerous.is_empty());
    }

    #[test]
    fn set_mode_strips_on_enter_auto_and_restores_on_leave() {
        let mut p = seeded_policy(PermissionMode::Default);
        let before = p.allow_rules.clone();

        p.set_mode(PermissionMode::Auto);
        assert_eq!(p.mode, PermissionMode::Auto);
        assert_eq!(allow_count(&p), 2); // dangerous stripped
        assert_eq!(p.stripped_dangerous.len(), 2);

        p.set_mode(PermissionMode::Default);
        assert_eq!(p.mode, PermissionMode::Default);
        assert_eq!(p.allow_rules, before); // restored
        assert!(p.stripped_dangerous.is_empty());
    }

    #[test]
    fn set_mode_to_same_mode_is_noop() {
        let mut p = seeded_policy(PermissionMode::Auto);
        // Already Auto; transitioning Auto→Auto must NOT strip.
        p.set_mode(PermissionMode::Auto);
        assert_eq!(allow_count(&p), 4);
        assert!(p.stripped_dangerous.is_empty());
    }

    #[test]
    fn set_mode_between_two_non_auto_modes_leaves_rules_untouched() {
        let mut p = seeded_policy(PermissionMode::Default);
        let before = p.allow_rules.clone();
        p.set_mode(PermissionMode::AcceptEdits);
        assert_eq!(p.mode, PermissionMode::AcceptEdits);
        assert_eq!(p.allow_rules, before);
        assert!(p.stripped_dangerous.is_empty());
    }

    #[test]
    fn auto_fallback_asks_for_stripped_tool() {
        // After stripping the `Agent` allow rule on entry to Auto, `Agent` has no
        // remaining allow rule, so Auto (classifier unwired) falls through to ask
        // — strip is behavior-neutral relative to the unwired Auto classifier.
        let mut p = seeded_policy(PermissionMode::Default);
        // Before: the Agent allow rule auto-allows (tool-wide, no roots).
        assert!(matches!(
            p.authorize("Agent", &serde_json::json!({})),
            PermissionResult::Allow { .. }
        ));
        p.set_mode(PermissionMode::Auto);
        // After strip: no Agent allow rule remains → Auto fallback asks.
        assert!(matches!(
            p.authorize("Agent", &serde_json::json!({})),
            PermissionResult::Ask { .. }
        ));
        // Leaving Auto restores it → auto-allow again.
        p.set_mode(PermissionMode::Default);
        assert!(matches!(
            p.authorize("Agent", &serde_json::json!({})),
            PermissionResult::Allow { .. }
        ));
    }

    // ── Batch 1: AcceptEdits working-dir auto-allow for editors ───────────

    fn accept_edits_policy(raw: &str) -> PermissionPolicy {
        let rules = crate::loader::permission_rules_from_settings_json(
            raw,
            PermissionRuleSource::ProjectSettings,
        )
        .unwrap();
        PermissionPolicy::from_rules(PermissionMode::AcceptEdits, rules).with_roots(roots())
    }

    #[test]
    fn accept_edits_auto_allows_editor_inside_cwd() {
        // (a) AcceptEdits + Edit inside cwd → Allow tagged with AcceptEdits mode.
        let p = accept_edits_policy(r#"{ "permissions": {} }"#);
        match p.authorize("Edit", &edit("/proj/src/x.rs")) {
            PermissionResult::Allow { reason, .. } => assert!(
                matches!(
                    reason,
                    PermissionDecisionReason::PermissionMode {
                        mode: PermissionMode::AcceptEdits
                    }
                ),
                "auto-allow must be tagged with AcceptEdits mode, got {reason:?}"
            ),
            other => panic!("expected Allow(AcceptEdits), got {other:?}"),
        }
        // Write / NotebookEdit (other editors) are likewise auto-allowed.
        assert!(matches!(
            p.authorize("Write", &edit("/proj/out/y.rs")),
            PermissionResult::Allow {
                reason: PermissionDecisionReason::PermissionMode {
                    mode: PermissionMode::AcceptEdits
                },
                ..
            }
        ));
        assert!(matches!(
            p.authorize(
                "NotebookEdit",
                &serde_json::json!({ "notebook_path": "/proj/nb.ipynb" })
            ),
            PermissionResult::Allow {
                reason: PermissionDecisionReason::PermissionMode {
                    mode: PermissionMode::AcceptEdits
                },
                ..
            }
        ));
    }

    #[test]
    fn accept_edits_asks_for_editor_outside_cwd() {
        // (b) Edit outside cwd → not auto-allowed → falls through to AcceptEdits
        // mode ask.
        let p = accept_edits_policy(r#"{ "permissions": {} }"#);
        match p.authorize("Edit", &edit("/elsewhere/x.rs")) {
            PermissionResult::Ask { reason, .. } => assert!(matches!(
                reason,
                PermissionDecisionReason::PermissionMode {
                    mode: PermissionMode::AcceptEdits
                }
            )),
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    #[test]
    fn accept_edits_does_not_auto_allow_non_editors() {
        // (c) AcceptEdits must NOT auto-allow Bash / Read / other non-editor
        // tools — those fall through to the AcceptEdits-mode ask.
        let p = accept_edits_policy(r#"{ "permissions": {} }"#);
        // Bash: a non-file tool, even with a command "inside" cwd.
        assert!(matches!(
            p.authorize("Bash", &serde_json::json!({ "command": "rm -rf /proj" })),
            PermissionResult::Ask { .. }
        ));
        // Read: a reader (not an editor) targeting a path inside cwd.
        assert!(matches!(
            p.authorize("Read", &edit("/proj/src/x.rs")),
            PermissionResult::Ask { .. }
        ));
        // Glob (reader) inside cwd is also not auto-allowed.
        assert!(matches!(
            p.authorize("Glob", &serde_json::json!({ "path": "/proj/src" })),
            PermissionResult::Ask { .. }
        ));
    }

    #[test]
    fn accept_edits_content_deny_rule_still_wins() {
        // (d) A content DENY rule on the path beats the AcceptEdits auto-allow
        // (the deny walk precedes the auto-allow branch).
        let p = accept_edits_policy(r#"{ "permissions": { "deny": ["Edit(src/**)"] } }"#);
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/src/secret.rs")),
            PermissionResult::Deny { .. }
        ));
        // A path NOT covered by the deny rule is still auto-allowed.
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/other/ok.rs")),
            PermissionResult::Allow {
                reason: PermissionDecisionReason::PermissionMode {
                    mode: PermissionMode::AcceptEdits
                },
                ..
            }
        ));
    }

    #[test]
    fn accept_edits_safety_blocks_git_config_inside_cwd() {
        // (e) `.git/config` inside cwd → the auto-edit safety guard fails, so the
        // branch is NOT taken and the call falls through to the AcceptEdits ask
        // (never auto-allowed).
        let p = accept_edits_policy(r#"{ "permissions": {} }"#);
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/.git/config")),
            PermissionResult::Ask {
                reason: PermissionDecisionReason::PermissionMode {
                    mode: PermissionMode::AcceptEdits
                },
                ..
            }
        ));
        // `.claude/settings.json` (claude-config) is likewise blocked → ask.
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/.claude/settings.json")),
            PermissionResult::Ask { .. }
        ));
        // …but a path under `.claude/worktrees/` is structural → auto-allowed.
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/.claude/worktrees/x/file.rs")),
            PermissionResult::Allow {
                reason: PermissionDecisionReason::PermissionMode {
                    mode: PermissionMode::AcceptEdits
                },
                ..
            }
        ));
    }

    #[test]
    fn accept_edits_additional_working_dir_is_honored() {
        // An editor inside an ADDITIONAL working dir is auto-allowed.
        let p = accept_edits_policy(r#"{ "permissions": {} }"#)
            .with_working_dirs(vec![PathBuf::from("/extra/work")]);
        assert!(matches!(
            p.authorize("Edit", &edit("/extra/work/file.rs")),
            PermissionResult::Allow {
                reason: PermissionDecisionReason::PermissionMode {
                    mode: PermissionMode::AcceptEdits
                },
                ..
            }
        ));
        // Still outside both cwd and the extra dir → ask.
        assert!(matches!(
            p.authorize("Edit", &edit("/nope/file.rs")),
            PermissionResult::Ask { .. }
        ));
    }

    #[test]
    fn accept_edits_without_roots_falls_through_to_ask() {
        // No roots → the working-dir auto-allow cannot run; AcceptEdits collapses
        // to the mode ask (backward-compatible with the pre-Batch-1 behavior).
        let p = PermissionPolicy::new(PermissionMode::AcceptEdits);
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/src/x.rs")),
            PermissionResult::Ask {
                reason: PermissionDecisionReason::PermissionMode {
                    mode: PermissionMode::AcceptEdits
                },
                ..
            }
        ));
    }

    #[test]
    fn accept_edits_explicit_allow_rule_still_allows() {
        // An explicit allow rule continues to win (allow walk precedes the
        // auto-allow branch) — and still produces an Allow.
        let p = accept_edits_policy(r#"{ "permissions": { "allow": ["Edit(src/**)"] } }"#);
        assert!(matches!(
            p.authorize("Edit", &edit("/proj/src/x.rs")),
            PermissionResult::Allow { .. }
        ));
    }
}
