//! `/permissions`: an interactive allow/ask/deny rule editor, backed by the
//! same settings write-back the "always allow" dialog uses
//! ([`permission::persist_permission_update`] /
//! [`permission::remove_permission_update`]).
//!
//! Ported in spirit from claude-code `components/permissions/rules/
//! PermissionRuleList.tsx`: a tabbed list (allow/ask/deny) of the current
//! rules with their source; managed (`policySettings`) rows are shown
//! read-only and cannot be removed. Adding types a rule string and picks a
//! destination (User/Project/Local); removing confirms then deletes.
//!
//! The state + key handling ([`PermissionsEditorState`], [`handle_perm_key`])
//! are pure and terminal-free — only the [`Renderable`]/[`BottomPaneView`]
//! impls touch a buffer — so navigation/add/remove are unit-testable exactly
//! like [`crate::resume::ResumeState`]. An add/remove is emitted as a
//! [`ViewOutcome::RunPermissionAction`] and the editor STAYS open (the owner
//! persists off-loop and reports back via `TurnEvent::SystemNotice`). The
//! in-view list is deliberately NOT mutated optimistically: a rule only
//! appears/disappears once the write lands and the owner refreshes the shared
//! snapshot (reflected on the next `/permissions` open), so the editor never
//! shows a change that failed to persist — the only signal of the outcome is
//! the `SystemNotice`.

use std::any::Any;
use std::path::Path;

use crossterm::cursor::SetCursorStyle;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use permission::{
    permission_rules_from_settings_json, PermissionBehavior, PermissionPaths, PermissionRule,
    PermissionRuleSource, PermissionUpdateDestination,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, PermissionAction, ViewOutcome};
use crate::renderable::Renderable;

/// A read-only snapshot of the permission rules used to seed the editor: the
/// three writable settings files (user / project / local), PLUS the enterprise
/// managed (policy) tier, which is shown read-only. Built at startup into a
/// shared slot and re-read after each edit so
/// the next `/permissions` open reflects the latest state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionsSnapshot {
    /// Every rule projected from the user/project/local settings files and the
    /// managed tier, each tagged with its [`PermissionRuleSource`].
    pub rules: Vec<PermissionRule>,
    /// The auto-mode classifier configuration is intentionally separate from
    /// `rules`: these entries are not `permissions.allow/ask/deny` rules.
    pub auto_mode: AutoModeSnapshot,
}

/// One of the four auto-mode classifier configuration buckets shown by the
/// `/permissions` Auto mode tab. The names match the `autoMode` settings keys
/// and the 2.1.251 UI labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutoModeCategory {
    /// Rules that are soft-approved by the classifier.
    SoftAllow,
    /// Rules that require a user confirmation unless another rule applies.
    SoftDeny,
    /// Rules that the classifier always rejects.
    HardDeny,
    /// Environment facts used to interpret classifier rules.
    Environment,
}

impl AutoModeCategory {
    /// Category order used by the Auto mode tab.
    pub const ALL: [Self; 4] = [
        Self::SoftAllow,
        Self::SoftDeny,
        Self::HardDeny,
        Self::Environment,
    ];

    fn index(self) -> usize {
        match self {
            Self::SoftAllow => 0,
            Self::SoftDeny => 1,
            Self::HardDeny => 2,
            Self::Environment => 3,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::SoftAllow => "allow",
            Self::SoftDeny => "soft_deny",
            Self::HardDeny => "hard_deny",
            Self::Environment => "environment",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::SoftAllow => "Soft allow",
            Self::SoftDeny => "Soft deny",
            Self::HardDeny => "Hard deny",
            Self::Environment => "Environment",
        }
    }

    /// Number of shipped rules represented by the built-in row.
    fn builtin_count(self) -> usize {
        match self {
            Self::SoftAllow => permission::auto_mode_defaults::DEFAULT_ALLOW_LABELS.len(),
            Self::SoftDeny => permission::auto_mode_defaults::DEFAULT_SOFT_DENY_LABELS.len(),
            Self::HardDeny => permission::auto_mode_defaults::DEFAULT_HARD_DENY_LABELS.len(),
            Self::Environment => permission::auto_mode_defaults::DEFAULT_ENVIRONMENT.len(),
        }
    }
}

/// One user/project/local/managed auto-mode entry. It is separate from
/// [`PermissionRule`] because auto-mode strings are classifier prose, not
/// tool permission patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoModeRule {
    /// Which `autoMode` array owns the entry.
    pub category: AutoModeCategory,
    /// The raw sentence/path fact as stored in settings.
    pub value: String,
    /// Settings tier that supplied this entry.
    pub source: PermissionRuleSource,
}

/// Read-only projection of the effective auto-mode classifier configuration.
///
/// The built-in rows are represented by `builtin_enabled` rather than copied
/// into `entries`; this preserves the `$defaults` sentinel semantics and keeps
/// the UI from presenting shipped defaults as user-authored rules. A configured
/// rule array without `$defaults` intentionally marks that built-in bucket off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoModeSnapshot {
    entries: Vec<AutoModeRule>,
    builtin_enabled: [bool; 4],
    configured: [bool; 4],
    unrecognized: bool,
}

impl Default for AutoModeSnapshot {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            builtin_enabled: [true; 4],
            configured: [false; 4],
            unrecognized: false,
        }
    }
}

impl AutoModeSnapshot {
    /// The custom entries in a classifier category, preserving source order.
    #[must_use]
    pub fn entries_for_category(&self, category: AutoModeCategory) -> Vec<&AutoModeRule> {
        self.entries
            .iter()
            .filter(|entry| entry.category == category)
            .collect()
    }

    /// Whether this category still includes its shipped built-in rules.
    #[must_use]
    pub fn builtin_enabled(&self, category: AutoModeCategory) -> bool {
        self.builtin_enabled[category.index()]
    }

    /// Whether a settings source explicitly configured this category.
    #[must_use]
    pub fn is_configured(&self, category: AutoModeCategory) -> bool {
        self.configured[category.index()]
    }

    /// Whether an auto-mode settings object contained an unrecognized shape.
    #[must_use]
    pub fn has_unrecognized_entries(&self) -> bool {
        self.unrecognized
    }

    /// Number of rows the Auto mode tab renders (four built-in summary rows
    /// plus custom entries and, when needed, one malformed-config warning).
    fn row_count(&self) -> usize {
        4 + self.entries.len() + usize::from(self.unrecognized)
    }

    fn append_settings_json(&mut self, raw: &str, source: PermissionRuleSource) {
        let Ok(root) = serde_json::from_str::<serde_json::Value>(raw) else {
            return;
        };
        let Some(auto_mode) = root.get("autoMode") else {
            return;
        };
        let Some(auto_mode) = auto_mode.as_object() else {
            self.unrecognized = true;
            return;
        };

        for category in AutoModeCategory::ALL {
            let Some(value) = auto_mode.get(category.key()) else {
                continue;
            };
            self.configured[category.index()] = true;
            let Some(entries) = value.as_array() else {
                self.unrecognized = true;
                continue;
            };
            let uses_defaults = category != AutoModeCategory::Environment
                && entries.iter().any(|entry| {
                    entry.as_str() == Some(permission::auto_mode_setup::AUTO_MODE_DEFAULTS_SENTINEL)
                });
            // A configured category without `$defaults` replaces the shipped
            // rules. Once any source opts out, do not claim the built-ins are
            // enabled in the effective summary row.
            if category != AutoModeCategory::Environment && !uses_defaults {
                self.builtin_enabled[category.index()] = false;
            }
            if category == AutoModeCategory::Environment {
                self.builtin_enabled[category.index()] = false;
            }
            for entry in entries {
                let Some(value) = entry.as_str() else {
                    self.unrecognized = true;
                    continue;
                };
                if value == permission::auto_mode_setup::AUTO_MODE_DEFAULTS_SENTINEL {
                    continue;
                }
                self.entries.push(AutoModeRule {
                    category,
                    value: value.to_string(),
                    source,
                });
            }
        }
    }
}

impl PermissionsSnapshot {
    /// Read the three writable settings files (user / project / local) AND the
    /// enterprise-managed (policy) tier, projecting their
    /// `permissions.{allow,deny,ask}` arrays into rules. Tiny local files, so a
    /// synchronous read is fine. Best-effort: a missing / unreadable / malformed
    /// file contributes no rules (never panics).
    #[must_use]
    pub fn load(paths: &PermissionPaths) -> Self {
        let mut rules = Vec::new();
        let mut auto_mode = AutoModeSnapshot::default();
        for (dest, source) in [
            (
                PermissionUpdateDestination::UserSettings,
                PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::User),
            ),
            (
                PermissionUpdateDestination::ProjectSettings,
                PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Project),
            ),
            (
                PermissionUpdateDestination::LocalSettings,
                PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Local),
            ),
        ] {
            let Some(path) = paths.destination_path(dest) else {
                continue;
            };
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Ok(mut projected) = permission_rules_from_settings_json(&raw, source) {
                rules.append(&mut projected);
            }
            auto_mode.append_settings_json(&raw, source);
        }
        // Managed (policy) tier: enterprise-managed deny/allow/ask rules the user
        // cannot edit. Loading them here makes the editor's read-only handling
        // real (managed rows render dimmed + "(read-only)" and cannot be removed)
        // AND — the reason it matters — makes managed DENY rules VISIBLE, so a
        // user can see why an Allow they'd add is silently overridden by policy.
        // `tui` already depends on `memory`, whose `managed_path()` resolves the
        // same managed dir the engine's settings watcher reads
        // (`<managed>/managed-settings.json` + `managed-settings.d/*.json`); no
        // new plumbing. Best-effort: on most machines the dir is absent → nothing.
        let managed_dir = memory::lingxi_md::hierarchy::managed_path();
        Self::append_managed_rules(&mut rules, &managed_dir);
        Self::append_managed_auto_mode(&mut auto_mode, &managed_dir);
        Self { rules, auto_mode }
    }

    /// Project the managed (policy) settings tier under `managed_dir` into rules
    /// tagged [`PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Managed)`] (rendered read-only).
    /// Mirrors the engine settings watcher's managed tier: the base
    /// `managed-settings.json` first, then every `*.json` under
    /// `managed-settings.d/` in alphabetical order (dotfiles skipped). Split out
    /// (taking the dir explicitly) so it is hermetically unit-testable against a
    /// temp dir without touching the absolute system managed path.
    fn append_managed_rules(rules: &mut Vec<PermissionRule>, managed_dir: &Path) {
        fn read_into(path: &Path, rules: &mut Vec<PermissionRule>) {
            if let Ok(raw) = std::fs::read_to_string(path) {
                if let Ok(mut projected) = permission_rules_from_settings_json(
                    &raw,
                    PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Managed),
                ) {
                    rules.append(&mut projected);
                }
            }
        }
        read_into(&managed_dir.join("managed-settings.json"), rules);
        let drop_in = managed_dir.join("managed-settings.d");
        if let Ok(rd) = std::fs::read_dir(&drop_in) {
            let mut names: Vec<std::ffi::OsString> = rd
                .flatten()
                .map(|e| e.file_name())
                .filter(|n| {
                    let s = n.to_string_lossy();
                    s.ends_with(".json") && !s.starts_with('.')
                })
                .collect();
            names.sort();
            for name in names {
                read_into(&drop_in.join(name), rules);
            }
        }
    }

    /// Project managed `autoMode` blocks into the read-only Auto mode tab.
    /// Uses the same base-file + alphabetical drop-in traversal as managed
    /// permission rules, keeping policy entries visibly distinct from user
    /// settings and never treating them as writable permission rules.
    fn append_managed_auto_mode(snapshot: &mut AutoModeSnapshot, managed_dir: &Path) {
        fn read_into(path: &Path, snapshot: &mut AutoModeSnapshot) {
            if let Ok(raw) = std::fs::read_to_string(path) {
                snapshot.append_settings_json(
                    &raw,
                    PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Managed),
                );
            }
        }
        read_into(&managed_dir.join("managed-settings.json"), snapshot);
        let drop_in = managed_dir.join("managed-settings.d");
        if let Ok(rd) = std::fs::read_dir(&drop_in) {
            let mut names: Vec<std::ffi::OsString> = rd
                .flatten()
                .map(|e| e.file_name())
                .filter(|n| {
                    let s = n.to_string_lossy();
                    s.ends_with(".json") && !s.starts_with('.')
                })
                .collect();
            names.sort();
            for name in names {
                read_into(&drop_in.join(name), snapshot);
            }
        }
    }
}

/// One of the rule buckets the editor tabs across. `Auto` is a separate
/// classifier configuration surface; it is deliberately not a
/// [`PermissionBehavior`] bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermTab {
    /// `permissions.allow` — auto-approved calls.
    Allow,
    /// `permissions.ask` — always-prompt calls.
    Ask,
    /// `permissions.deny` — always-blocked calls.
    Deny,
    /// `autoMode.{environment,allow,soft_deny,hard_deny}` classifier config.
    Auto,
}

impl PermTab {
    /// Tab order (also the render order of the tab bar).
    pub const ALL: [PermTab; 4] = [PermTab::Allow, PermTab::Ask, PermTab::Deny, PermTab::Auto];

    /// The behavior this tab edits.
    #[must_use]
    pub fn behavior(self) -> Option<PermissionBehavior> {
        match self {
            PermTab::Allow => Some(PermissionBehavior::Allow),
            PermTab::Ask => Some(PermissionBehavior::Ask),
            PermTab::Deny => Some(PermissionBehavior::Deny),
            PermTab::Auto => None,
        }
    }

    /// The tab's title.
    #[must_use]
    fn title(self) -> &'static str {
        match self {
            PermTab::Allow => "Allow",
            PermTab::Ask => "Ask",
            PermTab::Deny => "Deny",
            PermTab::Auto => "Auto mode",
        }
    }

    fn index(self) -> usize {
        match self {
            PermTab::Allow => 0,
            PermTab::Ask => 1,
            PermTab::Deny => 2,
            PermTab::Auto => 3,
        }
    }

    fn is_auto(self) -> bool {
        self == Self::Auto
    }
}

/// Map a rule's source to the settings destination it persists to, or `None`
/// when the source is not user-editable via this editor (managed policy,
/// flags, session, CLI, command-injected). Read-only rows return `None`.
#[must_use]
fn source_to_destination(source: PermissionRuleSource) -> Option<PermissionUpdateDestination> {
    match source {
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::User) => {
            Some(PermissionUpdateDestination::UserSettings)
        }
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Project) => {
            Some(PermissionUpdateDestination::ProjectSettings)
        }
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Local) => {
            Some(PermissionUpdateDestination::LocalSettings)
        }
        _ => None,
    }
}

/// Short human label for a rule's source (the dim `From …` column). Managed /
/// non-editable sources read read-only.
#[must_use]
fn source_label(source: PermissionRuleSource) -> &'static str {
    match source {
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::User) => "user",
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Project) => "project",
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Local) => "local",
        PermissionRuleSource::FlagSettings => "flag",
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Managed) => "managed",
        PermissionRuleSource::CliArg => "cli",
        PermissionRuleSource::Command => "command",
        PermissionRuleSource::Session => "session",
        PermissionRuleSource::ToolsNarrowing => "tools-narrowing",
        PermissionRuleSource::McpServerPolicy => "mcp-policy",
    }
}

/// Human-readable source label for an auto-mode entry. The classifier's own
/// UI calls these settings tiers out in prose (for example, "from user
/// settings"), rather than presenting them as ordinary permission buckets.
#[must_use]
fn auto_source_label(source: PermissionRuleSource) -> &'static str {
    match source {
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::User) => "user settings",
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Project) => {
            "project settings"
        }
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Local) => {
            "local settings"
        }
        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Managed) => {
            "managed policy"
        }
        _ => source_label(source),
    }
}

/// Short label for an add destination (the `→ …` hint on the input line).
#[must_use]
fn destination_label(dest: PermissionUpdateDestination) -> &'static str {
    match dest {
        PermissionUpdateDestination::UserSettings => "user",
        PermissionUpdateDestination::ProjectSettings => "project",
        PermissionUpdateDestination::LocalSettings => "local",
        PermissionUpdateDestination::Session => "session",
        PermissionUpdateDestination::CliArg => "cli",
    }
}

/// Pure state for the `/permissions` editor: the working rule set (a read-only
/// projection of the current settings — NOT mutated by add/remove), the active
/// tab, the selected row, the type-to-add input buffer, the add destination,
/// and any in-flight remove-confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionsEditorState {
    /// All rules across sources, seeded from the snapshot. Add/remove do NOT
    /// mutate this: the change is only reflected after the async persist lands
    /// and the shared snapshot is refreshed (next `/permissions` open), so the
    /// list never shows a change that failed to write.
    rules: Vec<PermissionRule>,
    /// The independent auto-mode classifier projection shown by the Auto tab.
    auto_mode: AutoModeSnapshot,
    /// The active bucket tab.
    tab: PermTab,
    /// Index into the CURRENT tab's rows of the highlighted row.
    selected: usize,
    /// Type-to-add buffer (a rule string like `"Bash(npm:*)"`).
    input: String,
    /// Destination an added rule is written to (User/Project/Local). Cycled
    /// with `Ctrl+S`; defaults to `LocalSettings` (the "always allow"
    /// precedent).
    dest: PermissionUpdateDestination,
    /// When `Some(idx)`, a remove of the row at `idx` (in the current tab) is
    /// awaiting confirmation (`Enter`/`y` confirms, `Esc`/`n` cancels).
    pending_remove: Option<usize>,
}

impl PermissionsEditorState {
    /// Seed the editor from a snapshot. Selects the first row of the Allow tab.
    #[must_use]
    pub fn new(snapshot: PermissionsSnapshot) -> Self {
        Self {
            rules: snapshot.rules,
            auto_mode: snapshot.auto_mode,
            tab: PermTab::Allow,
            selected: 0,
            input: String::new(),
            dest: PermissionUpdateDestination::LocalSettings,
            pending_remove: None,
        }
    }

    /// The active tab.
    #[must_use]
    pub fn tab(&self) -> PermTab {
        self.tab
    }

    /// The active add destination.
    #[must_use]
    pub fn destination(&self) -> PermissionUpdateDestination {
        self.dest
    }

    /// The current type-to-add buffer.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The rows shown under the current tab, in insertion order.
    #[must_use]
    pub fn rows_for_tab(&self) -> Vec<&PermissionRule> {
        let Some(behavior) = self.tab.behavior() else {
            return Vec::new();
        };
        self.rules
            .iter()
            .filter(|r| r.behavior == behavior)
            .collect()
    }

    /// All custom auto-mode entries, flattened in the same category order used
    /// for rendering. Built-in summary rows are not returned because they are
    /// classifier defaults, not editable rules.
    #[must_use]
    pub fn auto_mode_entries(&self) -> Vec<&AutoModeRule> {
        AutoModeCategory::ALL
            .into_iter()
            .flat_map(|category| self.auto_mode.entries_for_category(category))
            .collect()
    }

    /// Read-only auto-mode snapshot used by rendering and tests.
    #[must_use]
    pub fn auto_mode(&self) -> &AutoModeSnapshot {
        &self.auto_mode
    }

    /// The currently selected rule (in the active tab), if any.
    #[must_use]
    pub fn selected_rule(&self) -> Option<&PermissionRule> {
        self.rows_for_tab().get(self.selected).copied()
    }

    /// Whether a remove-confirmation is currently pending.
    #[must_use]
    pub fn is_confirming_remove(&self) -> bool {
        self.pending_remove.is_some()
    }

    fn next_tab(&mut self) {
        let idx = (self.tab.index() + 1) % PermTab::ALL.len();
        self.tab = PermTab::ALL[idx];
        self.selected = 0;
        self.pending_remove = None;
    }

    fn prev_tab(&mut self) {
        let len = PermTab::ALL.len();
        let idx = (self.tab.index() + len - 1) % len;
        self.tab = PermTab::ALL[idx];
        self.selected = 0;
        self.pending_remove = None;
    }

    fn cycle_dest(&mut self) {
        self.dest = match self.dest {
            PermissionUpdateDestination::LocalSettings => {
                PermissionUpdateDestination::ProjectSettings
            }
            PermissionUpdateDestination::ProjectSettings => {
                PermissionUpdateDestination::UserSettings
            }
            // From any other value (including User), wrap back to Local.
            _ => PermissionUpdateDestination::LocalSettings,
        };
    }

    fn clamp_selected(&mut self) {
        let n = if self.tab.is_auto() {
            self.auto_mode.row_count()
        } else {
            self.rows_for_tab().len()
        };
        if n == 0 {
            self.selected = 0;
        } else if self.selected >= n {
            self.selected = n - 1;
        }
    }

    /// The selected row's (index, cloned rule, destination) when it is a
    /// user-editable (removable) row; `None` for managed/read-only rows.
    fn selected_removable(&self) -> Option<(usize, PermissionRule, PermissionUpdateDestination)> {
        let rule = self.selected_rule()?.clone();
        let dest = source_to_destination(rule.source)?;
        Some((self.selected, rule, dest))
    }
}

/// What [`handle_perm_key`] tells the view to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermEditorOutcome {
    /// Keep the editor open (navigation, typing, confirm toggle, inert key).
    Stay,
    /// Persist an added rule, then keep the editor open.
    Add {
        /// The typed rule string.
        rule: String,
        /// Which bucket it goes into.
        behavior: PermissionBehavior,
        /// Destination settings file.
        dest: PermissionUpdateDestination,
    },
    /// Persist a rule removal, then keep the editor open.
    Remove {
        /// The rule string removed.
        rule: String,
        /// Which bucket it came from.
        behavior: PermissionBehavior,
        /// Destination settings file it came from.
        dest: PermissionUpdateDestination,
    },
    /// Close the editor (idle `Esc`).
    Cancel,
}

/// Pure key handler for the `/permissions` editor.
///
/// - `←`/`→`/`Tab`/`BackTab` → switch tab (Allow/Ask/Deny).
/// - `Up`/`Down` → move the row selection (clamped).
/// - printable char → type into the add buffer.
/// - `Backspace` → delete the last add-buffer char.
/// - `Ctrl+S` → cycle the add destination (Local → Project → User).
/// - `Enter` with a non-empty buffer → **Add** the rule (buffer cleared).
/// - `Enter`/`Delete` on empty buffer over a removable row → arm a
///   remove-confirmation; a second `Enter`/`y` confirms (**Remove**),
///   `Esc`/`n` cancels. Managed (read-only) rows arm nothing.
/// - `Esc` clears a non-empty buffer, else closes the editor.
#[must_use]
pub fn handle_perm_key(state: &mut PermissionsEditorState, key: KeyEvent) -> PermEditorOutcome {
    // Remove-confirmation owns the keyboard until resolved.
    if let Some(idx) = state.pending_remove {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                state.pending_remove = None;
                let rows = state.rows_for_tab();
                let Some(rule) = rows.get(idx).map(|r| (*r).clone()) else {
                    return PermEditorOutcome::Stay;
                };
                let Some(dest) = source_to_destination(rule.source) else {
                    return PermEditorOutcome::Stay;
                };
                // Emit the remove ACTION but do NOT drop the row locally: the
                // list only changes once the async persist confirms + the shared
                // snapshot refreshes (next open), so a failed write never shows a
                // phantom removal. Selection may now point past the (still
                // present) rows only after a real refresh, so clamp defensively.
                state.clamp_selected();
                return PermEditorOutcome::Remove {
                    rule: rule.value.to_rule_string(),
                    behavior: rule.behavior,
                    dest,
                };
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                state.pending_remove = None;
                return PermEditorOutcome::Stay;
            }
            _ => return PermEditorOutcome::Stay,
        }
    }

    // Auto mode has a distinct classifier configuration model. This editor
    // intentionally exposes it as a read-only projection until the dedicated
    // `/auto-mode-setup` review/persist flow can be embedded here; in
    // particular, never route typed text into `permissions.allow/ask/deny`.
    if state.tab.is_auto() {
        return match key.code {
            KeyCode::Left | KeyCode::BackTab => {
                state.prev_tab();
                PermEditorOutcome::Stay
            }
            KeyCode::Right | KeyCode::Tab => {
                state.next_tab();
                PermEditorOutcome::Stay
            }
            KeyCode::Up => {
                state.selected = state.selected.saturating_sub(1);
                PermEditorOutcome::Stay
            }
            KeyCode::Down => {
                let n = state.auto_mode.row_count();
                if n > 0 {
                    state.selected = (state.selected + 1).min(n - 1);
                }
                PermEditorOutcome::Stay
            }
            KeyCode::Esc => PermEditorOutcome::Cancel,
            _ => PermEditorOutcome::Stay,
        };
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Left | KeyCode::BackTab => {
            state.prev_tab();
            PermEditorOutcome::Stay
        }
        KeyCode::Right | KeyCode::Tab => {
            state.next_tab();
            PermEditorOutcome::Stay
        }
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            PermEditorOutcome::Stay
        }
        KeyCode::Down => {
            let n = state.rows_for_tab().len();
            if n > 0 {
                state.selected = (state.selected + 1).min(n - 1);
            }
            PermEditorOutcome::Stay
        }
        // Ctrl+S cycles the add destination (must precede the typing arm).
        KeyCode::Char('s' | 'S') if ctrl => {
            state.cycle_dest();
            PermEditorOutcome::Stay
        }
        KeyCode::Enter => {
            let rule = state.input.trim().to_string();
            if !rule.is_empty() {
                let Some(behavior) = state.tab.behavior() else {
                    return PermEditorOutcome::Stay;
                };
                let dest = state.dest;
                // Emit the add ACTION and clear the buffer, but do NOT insert the
                // row locally: it appears only after the persist lands + the
                // shared snapshot refreshes (next open), so a failed write never
                // shows a phantom rule.
                state.input.clear();
                return PermEditorOutcome::Add {
                    rule,
                    behavior,
                    dest,
                };
            }
            // Empty buffer: arm a remove-confirmation on a removable row.
            if let Some((idx, _rule, _dest)) = state.selected_removable() {
                state.pending_remove = Some(idx);
            }
            PermEditorOutcome::Stay
        }
        KeyCode::Delete => {
            if let Some((idx, _rule, _dest)) = state.selected_removable() {
                state.pending_remove = Some(idx);
            }
            PermEditorOutcome::Stay
        }
        KeyCode::Backspace => {
            state.input.pop();
            PermEditorOutcome::Stay
        }
        KeyCode::Esc => {
            if state.input.is_empty() {
                PermEditorOutcome::Cancel
            } else {
                state.input.clear();
                PermEditorOutcome::Stay
            }
        }
        KeyCode::Char(c)
            if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
        {
            state.input.push(c);
            PermEditorOutcome::Stay
        }
        _ => PermEditorOutcome::Stay,
    }
}

/// Column where the type-to-add value (and text cursor) begins on the input
/// line, after the `"New rule → local: "` prefix. Recomputed in `render`.
fn input_prefix(dest: PermissionUpdateDestination) -> String {
    format!("New rule → {}: ", destination_label(dest))
}

/// The `/permissions` interactive rule editor view.
pub struct PermissionsEditorView {
    state: PermissionsEditorState,
}

impl PermissionsEditorView {
    /// Build the editor over a seed snapshot.
    #[must_use]
    pub fn new(snapshot: PermissionsSnapshot) -> Self {
        Self {
            state: PermissionsEditorState::new(snapshot),
        }
    }

    /// Test/inspection access to the pure state.
    #[must_use]
    pub fn state(&self) -> &PermissionsEditorState {
        &self.state
    }

    /// Fixed chrome rows that are ALWAYS on screen, framing the (windowed) rule
    /// list: tab bar (1) + input line (1) + footer (1) + the optional
    /// remove-confirm line (0/1). The rule rows share whatever inner height is
    /// left.
    fn chrome_rows(&self) -> usize {
        3 + usize::from(self.state.is_confirming_remove())
    }

    /// The number of rule-list lines rendered (the actual rows, or the single
    /// `(no rules)` placeholder line when the tab is empty).
    fn rule_line_count(&self) -> usize {
        self.state.rows_for_tab().len().max(1)
    }

    /// Total content rows for the active tab, including the Auto mode
    /// classifier's explanatory header and read-only guidance.
    fn content_rows(&self) -> usize {
        if self.state.tab.is_auto() {
            2 + self.state.auto_mode.row_count() + 1
        } else {
            self.rule_line_count() + self.chrome_rows()
        }
    }

    /// The centered dialog rect, shared by [`Renderable::render`] and
    /// [`Renderable::cursor_pos`].
    fn block_rect(&self, area: Rect) -> Rect {
        // tab bar + rule rows + input + footer (+ optional confirm line): the
        // rendered lines fill the inner area EXACTLY (no phantom padding row), so
        // the input line lands at a position `cursor_pos` can reproduce.
        let content_rows = self.content_rows();
        let width = u16::try_from(60usize)
            .unwrap_or(60)
            .min(area.width.saturating_sub(4))
            .max(30);
        let height = u16::try_from(content_rows + 2)
            .unwrap_or(u16::MAX)
            .min(area.height)
            .max(6);
        centered_rect(width, height, area)
    }

    /// Window the rule rows around `selected` so the selected row and the fixed
    /// chrome are always visible when the inner area is shorter than the full
    /// list. Returns `(start, visible)`: render `rule_lines[start..start+visible]`.
    /// Mirrors [`crate::bottom_pane::resume_picker_view`]'s scroll approach.
    fn rule_window(&self, inner_height: u16) -> (usize, usize) {
        let total = self.rule_line_count();
        let inner = inner_height as usize;
        // Rows get whatever the chrome leaves; keep at least one so the selected
        // row is never fully hidden (a tiny overflow is clipped by the paragraph).
        let visible = inner.saturating_sub(self.chrome_rows()).max(1).min(total);
        let sel = self.state.selected.min(total.saturating_sub(1));
        // Keep `selected` in the window: scroll just enough to reveal it, then
        // clamp so the window never runs past the end of the list.
        let start = if sel < visible { 0 } else { sel + 1 - visible };
        let start = start.min(total.saturating_sub(visible));
        (start, visible)
    }

    /// Render the classifier-specific Auto mode tab. This is a read-only
    /// projection: the existing permission action channel only understands
    /// `permissions.allow/ask/deny`, so typing or deleting here must never
    /// silently persist a classifier sentence in one of those arrays.
    fn render_auto_mode(&self, inner: Rect, buf: &mut Buffer) {
        let mut lines = vec![
            Line::from(
                "Extra rules for the auto mode classifier. Rules are plain sentences; edit with /auto-mode-setup.",
            ),
            Line::from(Span::styled(
                "⌕ Search…",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ];
        let mut row_index = 0usize;
        for category in AutoModeCategory::ALL {
            let entries = self.state.auto_mode.entries_for_category(category);
            let builtins = if self.state.auto_mode.builtin_enabled(category) {
                "[x]"
            } else {
                "[ ]"
            };
            let status = if category == AutoModeCategory::Environment {
                if self.state.auto_mode.is_configured(category) {
                    let source = entries
                        .first()
                        .map_or("settings", |entry| auto_source_label(entry.source));
                    format!("Replaces the built-in default · from {source}")
                } else {
                    format!("{builtins} Built-in rules · {}", category.builtin_count())
                }
            } else {
                let suffix = if self.state.auto_mode.builtin_enabled(category) {
                    String::new()
                } else {
                    " · off".to_string()
                };
                format!(
                    "{builtins} Built-in rules · {}{suffix}",
                    category.builtin_count()
                )
            };
            let summary_style = if row_index == self.state.selected {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("{:<18}{}", category.title(), status),
                summary_style,
            )));
            row_index += 1;

            for entry in entries {
                let style = if row_index == self.state.selected {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(
                    format!(
                        "{:<18}{}    [from {}]",
                        category.title(),
                        entry.value,
                        auto_source_label(entry.source),
                    ),
                    style,
                )));
                row_index += 1;
            }
        }

        if self.state.auto_mode.has_unrecognized_entries() {
            let style = if row_index == self.state.selected {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().add_modifier(Modifier::DIM)
            };
            lines.push(Line::from(Span::styled(
                "(some autoMode entries are not recognized; edit the settings file carefully)",
                style,
            )));
        }
        lines.push(Line::from(Span::styled(
            "Read-only here · /auto-mode-setup opens the reviewed editor · `lingxi-cli auto-mode config` prints effective JSON",
            Style::default().add_modifier(Modifier::DIM),
        )));
        Paragraph::new(lines).render(inner, buf);
    }
}

impl Renderable for PermissionsEditorView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let rect = self.block_rect(area);
        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title("Permissions");
        let inner = block.inner(rect);
        block.render(rect, buf);

        if self.state.tab.is_auto() {
            self.render_auto_mode(inner, buf);
            return;
        }

        let mut lines: Vec<Line> = Vec::new();

        // Tab bar: Allow | Ask | Deny, active tab reversed/bold.
        let mut tab_spans: Vec<Span> = Vec::new();
        for (i, tab) in PermTab::ALL.iter().enumerate() {
            if i > 0 {
                tab_spans.push(Span::raw("  "));
            }
            let style = if *tab == self.state.tab {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().add_modifier(Modifier::DIM)
            };
            tab_spans.push(Span::styled(format!(" {} ", tab.title()), style));
        }
        lines.push(Line::from(tab_spans));

        // Rule rows (or the `(no rules)` placeholder). Build the full list first,
        // then window it so the selected row + fixed chrome stay visible when the
        // pane is height-clamped shorter than the list (finding #5: no windowing
        // previously clipped the selected row/input/footer off-screen).
        let rows = self.state.rows_for_tab();
        let mut rule_lines: Vec<Line> = Vec::new();
        if rows.is_empty() {
            rule_lines.push(Line::from(Span::styled(
                "  (no rules)",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            for (i, rule) in rows.iter().enumerate() {
                let removable = source_to_destination(rule.source).is_some();
                let marker = if i == self.state.selected {
                    "❯ "
                } else {
                    "  "
                };
                let mut style = if i == self.state.selected {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                if !removable {
                    style = style.add_modifier(Modifier::DIM);
                }
                let managed = if removable { "" } else { " (read-only)" };
                rule_lines.push(Line::from(Span::styled(
                    format!(
                        "{marker}{}    [{}]{managed}",
                        rule.value.to_rule_string(),
                        source_label(rule.source),
                    ),
                    style,
                )));
            }
        }
        let (start, visible) = self.rule_window(inner.height);
        let end = start.saturating_add(visible).min(rule_lines.len());
        lines.extend(rule_lines[start..end].iter().cloned());

        // Confirm line.
        if self.state.is_confirming_remove() {
            if let Some(rule) = self.state.selected_rule() {
                lines.push(Line::from(Span::styled(
                    format!("Remove {}? (y/n)", rule.value.to_rule_string()),
                    Style::default().add_modifier(Modifier::BOLD),
                )));
            }
        }

        // Input line.
        lines.push(Line::from(format!(
            "{}{}",
            input_prefix(self.state.dest),
            self.state.input
        )));

        // Footer.
        lines.push(Line::from(Span::styled(
            "type to add · Enter add/remove · ←/→ tabs · Del remove · Ctrl+S dest · Esc close",
            Style::default().add_modifier(Modifier::DIM),
        )));

        Paragraph::new(lines).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        let content_rows = self.content_rows();
        u16::try_from(content_rows + 2).unwrap_or(u16::MAX).max(6)
    }

    /// Claim a bar cursor at the end of the input value (the input line), so
    /// typing a new rule shows the caret in the field.
    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if self.state.tab.is_auto() {
            return None;
        }
        let inner = Block::new()
            .borders(Borders::ALL)
            .inner(self.block_rect(area));
        if inner.width == 0 || inner.height < 2 {
            return None;
        }
        // Compute the input line's row from the SAME layout `render` uses
        // (finding #7): tab bar (1) + the windowed rule rows + the optional
        // confirm line. `render` never left a phantom padding row, so this lands
        // on the input line (not the footer).
        let (_, visible) = self.rule_window(inner.height);
        let confirm = u16::from(self.state.is_confirming_remove());
        let input_y = inner
            .top()
            .saturating_add(1)
            .saturating_add(u16::try_from(visible).unwrap_or(0))
            .saturating_add(confirm)
            .min(inner.bottom().saturating_sub(1));
        let prefix_cols = u16::try_from(input_prefix(self.state.dest).chars().count()).unwrap_or(0);
        let typed = u16::try_from(self.state.input.chars().count()).unwrap_or(u16::MAX);
        let x = inner
            .x
            .saturating_add(prefix_cols)
            .saturating_add(typed)
            .min(inner.right().saturating_sub(1));
        Some((x, input_y))
    }

    fn cursor_style(&self, _area: Rect) -> SetCursorStyle {
        SetCursorStyle::SteadyBar
    }
}

impl BottomPaneView for PermissionsEditorView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match handle_perm_key(&mut self.state, key) {
            PermEditorOutcome::Stay => ViewOutcome::Pending,
            PermEditorOutcome::Cancel => ViewOutcome::Cancelled,
            PermEditorOutcome::Add {
                rule,
                behavior,
                dest,
            } => ViewOutcome::RunPermissionAction(PermissionAction::Add {
                rule,
                behavior,
                dest,
            }),
            PermEditorOutcome::Remove {
                rule,
                behavior,
                dest,
            } => ViewOutcome::RunPermissionAction(PermissionAction::Remove {
                rule,
                behavior,
                dest,
            }),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The editor body no longer constructs `PermissionRuleValue` (add/remove are
    // pure ACTIONS now — no optimistic mutation), so the type is test-only.
    use permission::PermissionRuleValue;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn rule(
        spec: &str,
        behavior: PermissionBehavior,
        source: PermissionRuleSource,
    ) -> PermissionRule {
        PermissionRule {
            value: PermissionRuleValue::from_rule_string(spec),
            behavior,
            source,
        }
    }

    fn snapshot() -> PermissionsSnapshot {
        PermissionsSnapshot {
            rules: vec![
                rule(
                    "Read",
                    PermissionBehavior::Allow,
                    PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Local),
                ),
                rule(
                    "Edit(src/**)",
                    PermissionBehavior::Allow,
                    PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Project),
                ),
                rule(
                    "Bash(rm:*)",
                    PermissionBehavior::Deny,
                    PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Local),
                ),
            ],
            auto_mode: AutoModeSnapshot::default(),
        }
    }

    fn state() -> PermissionsEditorState {
        PermissionsEditorState::new(snapshot())
    }

    #[test]
    fn opens_on_allow_tab_with_its_rows() {
        let s = state();
        assert_eq!(s.tab(), PermTab::Allow);
        let rows: Vec<String> = s
            .rows_for_tab()
            .iter()
            .map(|r| r.value.to_rule_string())
            .collect();
        assert_eq!(rows, vec!["Read".to_string(), "Edit(src/**)".to_string()]);
    }

    #[test]
    fn tab_navigation_switches_buckets_and_resets_selection() {
        let mut s = state();
        s.selected = 1;
        // → Ask (empty), → Deny.
        assert_eq!(
            handle_perm_key(&mut s, press(KeyCode::Right)),
            PermEditorOutcome::Stay
        );
        assert_eq!(s.tab(), PermTab::Ask);
        assert_eq!(s.selected, 0, "selection resets on tab switch");
        assert!(s.rows_for_tab().is_empty());
        let _ = handle_perm_key(&mut s, press(KeyCode::Right));
        assert_eq!(s.tab(), PermTab::Deny);
        let rows: Vec<String> = s
            .rows_for_tab()
            .iter()
            .map(|r| r.value.to_rule_string())
            .collect();
        assert_eq!(rows, vec!["Bash(rm:*)".to_string()]);
        // BackTab wraps back to Ask.
        let _ = handle_perm_key(&mut s, KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(s.tab(), PermTab::Ask);
    }

    #[test]
    fn down_up_move_selection_clamped() {
        let mut s = state();
        assert_eq!(s.selected, 0);
        let _ = handle_perm_key(&mut s, press(KeyCode::Down));
        assert_eq!(s.selected, 1);
        // Clamp at the last row.
        let _ = handle_perm_key(&mut s, press(KeyCode::Down));
        assert_eq!(s.selected, 1);
        let _ = handle_perm_key(&mut s, press(KeyCode::Up));
        assert_eq!(s.selected, 0);
        let _ = handle_perm_key(&mut s, press(KeyCode::Up));
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn typing_then_enter_emits_add_action_without_mutating_the_list() {
        let mut s = state();
        let before = s.rows_for_tab().len();
        for c in "Bash(npm:*)".chars() {
            assert_eq!(
                handle_perm_key(&mut s, press(KeyCode::Char(c))),
                PermEditorOutcome::Stay
            );
        }
        assert_eq!(s.input(), "Bash(npm:*)");
        let outcome = handle_perm_key(&mut s, press(KeyCode::Enter));
        // The ADD is emitted as an action for the owner to persist off-loop…
        assert_eq!(
            outcome,
            PermEditorOutcome::Add {
                rule: "Bash(npm:*)".to_string(),
                behavior: PermissionBehavior::Allow,
                dest: PermissionUpdateDestination::LocalSettings,
            }
        );
        // …the buffer is cleared, but the list is NOT optimistically mutated: the
        // rule only appears after the write lands + the snapshot refreshes, so a
        // failed write never shows a phantom rule (finding #4).
        assert_eq!(s.input(), "");
        assert_eq!(
            s.rows_for_tab().len(),
            before,
            "list not mutated optimistically"
        );
        assert!(!s
            .rows_for_tab()
            .iter()
            .any(|r| r.value.to_rule_string() == "Bash(npm:*)"));
    }

    #[test]
    fn ctrl_s_cycles_the_add_destination() {
        let mut s = state();
        assert_eq!(s.destination(), PermissionUpdateDestination::LocalSettings);
        let _ = handle_perm_key(&mut s, ctrl('s'));
        assert_eq!(
            s.destination(),
            PermissionUpdateDestination::ProjectSettings
        );
        let _ = handle_perm_key(&mut s, ctrl('s'));
        assert_eq!(s.destination(), PermissionUpdateDestination::UserSettings);
        let _ = handle_perm_key(&mut s, ctrl('s'));
        assert_eq!(s.destination(), PermissionUpdateDestination::LocalSettings);
    }

    #[test]
    fn enter_confirms_then_emits_remove_action_without_mutating_the_list() {
        let mut s = state();
        // First Enter (empty buffer) arms confirmation.
        assert_eq!(
            handle_perm_key(&mut s, press(KeyCode::Enter)),
            PermEditorOutcome::Stay
        );
        assert!(s.is_confirming_remove());
        // Second Enter confirms → Remove(Read, allow, local).
        let outcome = handle_perm_key(&mut s, press(KeyCode::Enter));
        assert_eq!(
            outcome,
            PermEditorOutcome::Remove {
                rule: "Read".to_string(),
                behavior: PermissionBehavior::Allow,
                dest: PermissionUpdateDestination::LocalSettings,
            }
        );
        // Confirmation cleared, but the row is NOT dropped locally: it disappears
        // only after the write lands + the snapshot refreshes, so a failed write
        // never shows a phantom removal (finding #4).
        assert!(!s.is_confirming_remove());
        let rows: Vec<String> = s
            .rows_for_tab()
            .iter()
            .map(|r| r.value.to_rule_string())
            .collect();
        assert_eq!(
            rows,
            vec!["Read".to_string(), "Edit(src/**)".to_string()],
            "list unchanged until persist confirms"
        );
    }

    #[test]
    fn delete_arms_and_esc_cancels_the_confirmation() {
        let mut s = state();
        let _ = handle_perm_key(&mut s, press(KeyCode::Delete));
        assert!(s.is_confirming_remove());
        let _ = handle_perm_key(&mut s, press(KeyCode::Esc));
        assert!(!s.is_confirming_remove());
        // No rule removed.
        assert_eq!(s.rows_for_tab().len(), 2);
    }

    #[test]
    fn managed_rows_are_read_only_and_emit_no_remove() {
        let mut s = PermissionsEditorState::new(PermissionsSnapshot {
            rules: vec![rule(
                "Bash(curl:*)",
                PermissionBehavior::Allow,
                PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Managed),
            )],
            auto_mode: AutoModeSnapshot::default(),
        });
        // Enter over a managed row arms nothing.
        assert_eq!(
            handle_perm_key(&mut s, press(KeyCode::Enter)),
            PermEditorOutcome::Stay
        );
        assert!(!s.is_confirming_remove());
        // Delete over a managed row also does nothing.
        let _ = handle_perm_key(&mut s, press(KeyCode::Delete));
        assert!(!s.is_confirming_remove());
        assert_eq!(s.rows_for_tab().len(), 1, "managed rule not removed");
    }

    #[test]
    fn esc_clears_buffer_then_closes() {
        let mut s = state();
        for c in "Read".chars() {
            let _ = handle_perm_key(&mut s, press(KeyCode::Char(c)));
        }
        // Esc with a non-empty buffer clears it (stays open).
        assert_eq!(
            handle_perm_key(&mut s, press(KeyCode::Esc)),
            PermEditorOutcome::Stay
        );
        assert_eq!(s.input(), "");
        // Esc with an empty buffer closes.
        assert_eq!(
            handle_perm_key(&mut s, press(KeyCode::Esc)),
            PermEditorOutcome::Cancel
        );
    }

    #[test]
    fn view_maps_add_to_a_run_permission_action_outcome() {
        let mut v = PermissionsEditorView::new(snapshot());
        for c in "Write".chars() {
            v.handle_key(press(KeyCode::Char(c)));
        }
        let outcome = v.handle_key(press(KeyCode::Enter));
        match outcome {
            ViewOutcome::RunPermissionAction(PermissionAction::Add {
                rule,
                behavior,
                dest,
            }) => {
                assert_eq!(rule, "Write");
                assert_eq!(behavior, PermissionBehavior::Allow);
                assert_eq!(dest, PermissionUpdateDestination::LocalSettings);
            }
            _ => panic!("expected RunPermissionAction(Add)"),
        }
    }

    #[test]
    fn render_shows_tabs_rows_and_footer() {
        let v = PermissionsEditorView::new(snapshot());
        let area = Rect::new(0, 0, 72, 16);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text: String = (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Permissions"), "{text}");
        assert!(text.contains("Allow"), "{text}");
        assert!(text.contains("Read"), "{text}");
        assert!(text.contains("New rule"), "{text}");
    }

    /// Flatten the rendered buffer into newline-joined rows.
    fn buffer_text(area: Rect, buf: &Buffer) -> String {
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The first buffer row whose text contains `needle`.
    fn row_containing(area: Rect, buf: &Buffer, needle: &str) -> Option<u16> {
        (area.top()..area.bottom()).find(|&y| {
            let row: String = (area.left()..area.right())
                .map(|x| {
                    buf.cell(ratatui::layout::Position::new(x, y))
                        .map_or(" ", ratatui::buffer::Cell::symbol)
                })
                .collect();
            row.contains(needle)
        })
    }

    fn many_allow_rules(n: usize) -> PermissionsSnapshot {
        PermissionsSnapshot {
            rules: (0..n)
                .map(|i| {
                    rule(
                        &format!("Bash(cmd{i:02}:*)"),
                        PermissionBehavior::Allow,
                        PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Local),
                    )
                })
                .collect(),
            auto_mode: AutoModeSnapshot::default(),
        }
    }

    // FIX #3: managed (policySettings) rules load from the managed dir, tagged
    // read-only.
    #[test]
    fn managed_policy_rules_load_read_only_from_the_managed_dir() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        // Unique temp dir (no tempfile dep): pid + atomic counter avoids the
        // parallel-test `now_millis()` collision trap.
        let dir = std::env::temp_dir().join(format!(
            "lingxi-perm-managed-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("managed-settings.json"),
            r#"{"permissions":{"deny":["Bash(curl:*)"],"allow":["Read"]}}"#,
        )
        .unwrap();
        let drop_in = dir.join("managed-settings.d");
        std::fs::create_dir_all(&drop_in).unwrap();
        std::fs::write(
            drop_in.join("10-extra.json"),
            r#"{"permissions":{"deny":["Write(/etc/**)"]}}"#,
        )
        .unwrap();
        // A dotfile drop-in is skipped (matches the settings watcher).
        std::fs::write(
            drop_in.join(".ignored.json"),
            r#"{"permissions":{"deny":["ShouldNotLoad"]}}"#,
        )
        .unwrap();

        let mut rules = Vec::new();
        PermissionsSnapshot::append_managed_rules(&mut rules, &dir);

        // Every managed rule is tagged PolicySettings → rendered read-only.
        assert!(rules.iter().all(|r| r.source
            == PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Managed)));
        let strs: Vec<String> = rules.iter().map(|r| r.value.to_rule_string()).collect();
        assert!(strs.contains(&"Bash(curl:*)".to_string()), "{strs:?}");
        assert!(strs.contains(&"Read".to_string()), "{strs:?}");
        assert!(
            strs.contains(&"Write(/etc/**)".to_string()),
            "drop-in loaded: {strs:?}"
        );
        assert!(
            !strs.contains(&"ShouldNotLoad".to_string()),
            "dotfile skipped: {strs:?}"
        );

        // A PolicySettings rule has no writable destination, so the editor's
        // read-only handling (no remove) stays active for it.
        assert!(source_to_destination(PermissionRuleSource::Settings(
            lingxi_core::types::SettingsScope::Managed
        ))
        .is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // FIX #5: a rule list taller than the height-clamped pane windows around the
    // selection so the selected row + chrome stay on screen.
    #[test]
    fn render_windows_the_selected_row_when_height_clamped() {
        let mut v = PermissionsEditorView::new(many_allow_rules(20));
        // Select the last row.
        for _ in 0..19 {
            v.handle_key(press(KeyCode::Down));
        }
        assert_eq!(v.state().selected, 19);
        // A short pane: the 20 rule rows cannot all fit.
        let area = Rect::new(0, 0, 72, 10);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text = buffer_text(area, &buf);
        // The selected row is windowed into view even though it's far down…
        assert!(
            text.contains("Bash(cmd19:*)"),
            "selected row must be visible:\n{text}"
        );
        // …and the fixed chrome (input line + footer) is not clipped off-screen.
        assert!(text.contains("New rule"), "input line visible:\n{text}");
        assert!(text.contains("type to add"), "footer visible:\n{text}");
        // A far-away top row scrolls off when clamped.
        assert!(
            !text.contains("Bash(cmd00:*)"),
            "top row scrolled off:\n{text}"
        );
    }

    // FIX #7: the text cursor sits on the input line, not the footer — across a
    // roomy pane, a clamped/windowed pane, and while confirming a remove (which
    // adds a chrome line above the input).
    #[test]
    fn cursor_row_matches_the_input_line_row() {
        // Roomy pane.
        let roomy = PermissionsEditorView::new(snapshot());
        // Clamped pane (windowed rule list) with the selection at the end.
        let mut clamped = PermissionsEditorView::new(many_allow_rules(20));
        for _ in 0..19 {
            clamped.handle_key(press(KeyCode::Down));
        }
        // Confirming a remove (an extra confirm chrome line is present).
        let mut confirming = PermissionsEditorView::new(snapshot());
        confirming.handle_key(press(KeyCode::Enter));
        assert!(confirming.state().is_confirming_remove());

        let cases = [
            (roomy, Rect::new(0, 0, 72, 16)),
            (clamped, Rect::new(0, 0, 72, 10)),
            (confirming, Rect::new(0, 0, 72, 16)),
        ];
        for (v, area) in cases {
            let (_, cy) = v.cursor_pos(area).expect("cursor claimed");
            let mut buf = Buffer::empty(area);
            v.render(area, &mut buf);
            let input_row = row_containing(area, &buf, "New rule").expect("input line rendered");
            assert_eq!(cy, input_row, "cursor on input line, not footer");
        }
    }

    #[test]
    fn auto_mode_snapshot_keeps_classifier_categories_and_defaults_separate() {
        let mut snapshot = AutoModeSnapshot::default();
        assert!(snapshot.builtin_enabled(AutoModeCategory::SoftAllow));
        assert!(snapshot.builtin_enabled(AutoModeCategory::Environment));
        snapshot.append_settings_json(
            r#"{
                "autoMode": {
                    "allow": ["custom allow"],
                    "soft_deny": ["$defaults", "custom soft deny"],
                    "hard_deny": ["$defaults", "custom hard deny"],
                    "environment": ["custom environment"]
                },
                "permissions": {"allow": ["Read"]}
            }"#,
            PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::User),
        );

        assert!(!snapshot.builtin_enabled(AutoModeCategory::SoftAllow));
        assert!(snapshot.builtin_enabled(AutoModeCategory::SoftDeny));
        assert!(snapshot.builtin_enabled(AutoModeCategory::HardDeny));
        assert!(!snapshot.builtin_enabled(AutoModeCategory::Environment));
        assert_eq!(
            snapshot
                .entries_for_category(AutoModeCategory::SoftAllow)
                .iter()
                .map(|entry| entry.value.as_str())
                .collect::<Vec<_>>(),
            vec!["custom allow"]
        );
        assert_eq!(
            snapshot
                .entries_for_category(AutoModeCategory::SoftDeny)
                .iter()
                .map(|entry| entry.value.as_str())
                .collect::<Vec<_>>(),
            vec!["custom soft deny"]
        );
        assert_eq!(snapshot.row_count(), 8);
    }

    #[test]
    fn auto_mode_tab_is_last_and_navigation_does_not_change_permission_buckets() {
        assert_eq!(
            PermTab::ALL,
            [PermTab::Allow, PermTab::Ask, PermTab::Deny, PermTab::Auto]
        );
        let mut s = state();
        for _ in 0..3 {
            assert_eq!(
                handle_perm_key(&mut s, press(KeyCode::Right)),
                PermEditorOutcome::Stay
            );
        }
        assert_eq!(s.tab(), PermTab::Auto);
        assert!(s.rows_for_tab().is_empty());
        assert!(s.auto_mode_entries().is_empty());
        let _ = handle_perm_key(&mut s, press(KeyCode::Right));
        assert_eq!(s.tab(), PermTab::Allow, "Auto wraps to Allow");
    }

    #[test]
    fn auto_mode_tab_is_read_only_and_never_emits_permission_actions() {
        let mut auto_mode = AutoModeSnapshot::default();
        auto_mode.append_settings_json(
            r#"{"autoMode":{"allow":["classifier sentence"]}}"#,
            PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Local),
        );
        let mut s = PermissionsEditorState::new(PermissionsSnapshot {
            rules: vec![rule(
                "Read",
                PermissionBehavior::Allow,
                PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Local),
            )],
            auto_mode,
        });
        for _ in 0..3 {
            let _ = handle_perm_key(&mut s, press(KeyCode::Right));
        }
        assert_eq!(s.tab(), PermTab::Auto);
        assert_eq!(s.auto_mode_entries().len(), 1);
        // Printable input, Enter, and Delete are inert in Auto mode. In
        // particular, the classifier sentence never appears in permissions
        // rows and no ordinary PermissionAction can be emitted.
        assert_eq!(
            handle_perm_key(&mut s, press(KeyCode::Char('x'))),
            PermEditorOutcome::Stay
        );
        assert!(s.input().is_empty());
        assert_eq!(
            handle_perm_key(&mut s, press(KeyCode::Enter)),
            PermEditorOutcome::Stay
        );
        assert!(!s.is_confirming_remove());
        assert_eq!(
            handle_perm_key(&mut s, press(KeyCode::Delete)),
            PermEditorOutcome::Stay
        );
        assert!(!s.is_confirming_remove());
    }

    #[test]
    fn auto_mode_render_shows_builtin_counts_custom_rules_and_sources() {
        let mut auto_mode = AutoModeSnapshot::default();
        auto_mode.append_settings_json(
            r#"{"autoMode":{"allow":["$defaults","custom classifier rule"],"environment":["**Org**: internal"]}}"#,
            PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Project),
        );
        let mut v = PermissionsEditorView::new(PermissionsSnapshot {
            rules: Vec::new(),
            auto_mode,
        });
        for _ in 0..3 {
            v.handle_key(press(KeyCode::Right));
        }
        let area = Rect::new(0, 0, 120, 24);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let text = buffer_text(area, &buf);
        assert!(
            text.contains("Extra rules for the auto mode classifier"),
            "{text}"
        );
        assert!(text.contains("Soft allow"), "{text}");
        assert!(text.contains("Built-in rules · 17"), "{text}");
        assert!(text.contains("custom classifier rule"), "{text}");
        // The fixed-width dialog can clip the tail of the full source label;
        // assert the visible, source-specific prefix and the model value.
        assert!(text.contains("from project"), "{text}");
        assert_eq!(
            v.state()
                .auto_mode()
                .entries_for_category(AutoModeCategory::SoftAllow)[0]
                .source,
            PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Project)
        );
        assert!(text.contains("Environment"), "{text}");
        assert!(text.contains("Replaces the built-in default"), "{text}");
        assert!(text.contains("Read-only here"), "{text}");
        assert!(
            !text.contains("New rule"),
            "Auto must not show ordinary input: {text}"
        );
    }

    #[test]
    fn permissions_snapshot_loads_auto_mode_from_user_and_local_sources() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "lingxi-perm-auto-load-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let cwd = root.join("cwd");
        std::fs::create_dir_all(home.join(branding::DOT_DIR)).unwrap();
        std::fs::create_dir_all(cwd.join(branding::DOT_DIR)).unwrap();
        std::fs::write(
            home.join("settings.json"),
            r#"{"autoMode":{"allow":["$defaults","user classifier"]}}"#,
        )
        .unwrap();
        std::fs::write(
            cwd.join(branding::DOT_DIR).join("settings.local.json"),
            r#"{"autoMode":{"soft_deny":["local classifier"]}}"#,
        )
        .unwrap();
        let snapshot = PermissionsSnapshot::load(&PermissionPaths {
            lingxi_home: home,
            cwd: cwd.clone(),
        });
        let user = snapshot
            .auto_mode
            .entries_for_category(AutoModeCategory::SoftAllow);
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].value, "user classifier");
        assert_eq!(
            user[0].source,
            PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::User)
        );
        let local = snapshot
            .auto_mode
            .entries_for_category(AutoModeCategory::SoftDeny);
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].value, "local classifier");
        assert_eq!(
            local[0].source,
            PermissionRuleSource::Settings(lingxi_core::types::SettingsScope::Local)
        );
        assert!(snapshot
            .auto_mode
            .builtin_enabled(AutoModeCategory::SoftAllow));
        assert!(!snapshot
            .auto_mode
            .builtin_enabled(AutoModeCategory::SoftDeny));
        let _ = std::fs::remove_dir_all(root);
    }
}
