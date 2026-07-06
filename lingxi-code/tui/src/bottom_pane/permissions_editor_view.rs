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
//! persists off-loop and reports back via `TurnEvent::SystemNotice`); the
//! in-view buckets update optimistically so the change is visible immediately.

use std::any::Any;

use crossterm::cursor::SetCursorStyle;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use permission::{
    permission_rules_from_settings_json, PermissionBehavior, PermissionPaths, PermissionRule,
    PermissionRuleSource, PermissionRuleValue, PermissionUpdateDestination,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, PermissionAction, ViewOutcome};
use crate::renderable::Renderable;

/// A read-only snapshot of the permission rules across the three writable
/// settings files (user / project / local), used to seed the editor. Built at
/// startup into a shared slot (like the `/web` config snapshot) and re-read
/// after each edit so the next `/permissions` open reflects the latest state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionsSnapshot {
    /// Every rule projected from the user/project/local settings files, each
    /// tagged with its [`PermissionRuleSource`].
    pub rules: Vec<PermissionRule>,
}

impl PermissionsSnapshot {
    /// Read the three writable settings files (user / project / local) and
    /// project their `permissions.{allow,deny,ask}` arrays into rules. Tiny
    /// local files, so a synchronous read is fine. Best-effort: a missing /
    /// unreadable / malformed file contributes no rules (never panics).
    #[must_use]
    pub fn load(paths: &PermissionPaths) -> Self {
        let mut rules = Vec::new();
        for (dest, source) in [
            (
                PermissionUpdateDestination::UserSettings,
                PermissionRuleSource::UserSettings,
            ),
            (
                PermissionUpdateDestination::ProjectSettings,
                PermissionRuleSource::ProjectSettings,
            ),
            (
                PermissionUpdateDestination::LocalSettings,
                PermissionRuleSource::LocalSettings,
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
        }
        Self { rules }
    }
}

/// One of the three rule buckets the editor tabs across.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermTab {
    /// `permissions.allow` — auto-approved calls.
    Allow,
    /// `permissions.ask` — always-prompt calls.
    Ask,
    /// `permissions.deny` — always-blocked calls.
    Deny,
}

impl PermTab {
    /// Tab order (also the render order of the tab bar).
    const ALL: [PermTab; 3] = [PermTab::Allow, PermTab::Ask, PermTab::Deny];

    /// The behavior this tab edits.
    #[must_use]
    pub fn behavior(self) -> PermissionBehavior {
        match self {
            PermTab::Allow => PermissionBehavior::Allow,
            PermTab::Ask => PermissionBehavior::Ask,
            PermTab::Deny => PermissionBehavior::Deny,
        }
    }

    /// The tab's title.
    #[must_use]
    fn title(self) -> &'static str {
        match self {
            PermTab::Allow => "Allow",
            PermTab::Ask => "Ask",
            PermTab::Deny => "Deny",
        }
    }

    fn index(self) -> usize {
        match self {
            PermTab::Allow => 0,
            PermTab::Ask => 1,
            PermTab::Deny => 2,
        }
    }
}

/// Map a rule's source to the settings destination it persists to, or `None`
/// when the source is not user-editable via this editor (managed policy,
/// flags, session, CLI, command-injected). Read-only rows return `None`.
#[must_use]
fn source_to_destination(source: PermissionRuleSource) -> Option<PermissionUpdateDestination> {
    match source {
        PermissionRuleSource::UserSettings => Some(PermissionUpdateDestination::UserSettings),
        PermissionRuleSource::ProjectSettings => Some(PermissionUpdateDestination::ProjectSettings),
        PermissionRuleSource::LocalSettings => Some(PermissionUpdateDestination::LocalSettings),
        _ => None,
    }
}

/// The source an added rule is tagged with, for its destination file.
#[must_use]
fn destination_to_source(dest: PermissionUpdateDestination) -> PermissionRuleSource {
    match dest {
        PermissionUpdateDestination::UserSettings => PermissionRuleSource::UserSettings,
        PermissionUpdateDestination::ProjectSettings => PermissionRuleSource::ProjectSettings,
        PermissionUpdateDestination::LocalSettings => PermissionRuleSource::LocalSettings,
        PermissionUpdateDestination::Session => PermissionRuleSource::Session,
        PermissionUpdateDestination::CliArg => PermissionRuleSource::CliArg,
    }
}

/// Short human label for a rule's source (the dim `From …` column). Managed /
/// non-editable sources read read-only.
#[must_use]
fn source_label(source: PermissionRuleSource) -> &'static str {
    match source {
        PermissionRuleSource::UserSettings => "user",
        PermissionRuleSource::ProjectSettings => "project",
        PermissionRuleSource::LocalSettings => "local",
        PermissionRuleSource::FlagSettings => "flag",
        PermissionRuleSource::PolicySettings => "managed",
        PermissionRuleSource::CliArg => "cli",
        PermissionRuleSource::Command => "command",
        PermissionRuleSource::Session => "session",
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

/// Pure state for the `/permissions` editor: the working rule set (mutated
/// optimistically on add/remove), the active tab, the selected row, the
/// type-to-add input buffer, the add destination, and any in-flight
/// remove-confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionsEditorState {
    /// All rules across sources; add/remove mutate this optimistically so the
    /// list reflects the edit before the async persist lands.
    rules: Vec<PermissionRule>,
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
        let behavior = self.tab.behavior();
        self.rules
            .iter()
            .filter(|r| r.behavior == behavior)
            .collect()
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
        let n = self.rows_for_tab().len();
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

    /// Optimistically add a rule to the working set (deduped on
    /// behavior+source+wire-string), matching what the async persist will do.
    fn add_rule(&mut self, rule_str: &str, behavior: PermissionBehavior, source: PermissionRuleSource) {
        let value = PermissionRuleValue::from_rule_string(rule_str);
        let wire = value.to_rule_string();
        let already = self.rules.iter().any(|r| {
            r.behavior == behavior
                && r.source == source
                && r.value.to_rule_string() == wire
        });
        if !already {
            self.rules.push(PermissionRule {
                value,
                behavior,
                source,
            });
        }
    }

    /// Optimistically remove `rule` from the working set.
    fn remove_rule(&mut self, rule: &PermissionRule) {
        let wire = rule.value.to_rule_string();
        self.rules.retain(|r| {
            !(r.behavior == rule.behavior
                && r.source == rule.source
                && r.value.to_rule_string() == wire)
        });
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
                let action = PermEditorOutcome::Remove {
                    rule: rule.value.to_rule_string(),
                    behavior: rule.behavior,
                    dest,
                };
                state.remove_rule(&rule);
                state.clamp_selected();
                return action;
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                state.pending_remove = None;
                return PermEditorOutcome::Stay;
            }
            _ => return PermEditorOutcome::Stay,
        }
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
                let behavior = state.tab.behavior();
                let dest = state.dest;
                state.add_rule(&rule, behavior, destination_to_source(dest));
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

    /// The centered dialog rect, shared by [`Renderable::render`] and
    /// [`Renderable::cursor_pos`].
    fn block_rect(&self, area: Rect) -> Rect {
        let rows = self.state.rows_for_tab().len().max(1);
        // tab bar + rows + input + footer (+ optional confirm line) + padding.
        let content_rows = rows + 4 + usize::from(self.state.is_confirming_remove());
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
}

impl Renderable for PermissionsEditorView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let rect = self.block_rect(area);
        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title("Permissions");
        let inner = block.inner(rect);
        block.render(rect, buf);

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

        // Rows.
        let rows = self.state.rows_for_tab();
        if rows.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (no rules)",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            for (i, rule) in rows.iter().enumerate() {
                let removable = source_to_destination(rule.source).is_some();
                let marker = if i == self.state.selected { "❯ " } else { "  " };
                let mut style = if i == self.state.selected {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                if !removable {
                    style = style.add_modifier(Modifier::DIM);
                }
                let managed = if removable { "" } else { " (read-only)" };
                lines.push(Line::from(Span::styled(
                    format!(
                        "{marker}{}    [{}]{managed}",
                        rule.value.to_rule_string(),
                        source_label(rule.source),
                    ),
                    style,
                )));
            }
        }

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
        let rows = self.state.rows_for_tab().len().max(1);
        let content_rows = rows + 4 + usize::from(self.state.is_confirming_remove());
        u16::try_from(content_rows + 2).unwrap_or(u16::MAX).max(6)
    }

    /// Claim a bar cursor at the end of the input value (the input line), so
    /// typing a new rule shows the caret in the field.
    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let inner = Block::new()
            .borders(Borders::ALL)
            .inner(self.block_rect(area));
        if inner.width == 0 || inner.height < 2 {
            return None;
        }
        // The input line is the second-to-last inner row (footer is last).
        let input_y = inner.bottom().saturating_sub(2);
        let prefix_cols =
            u16::try_from(input_prefix(self.state.dest).chars().count()).unwrap_or(0);
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

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn rule(spec: &str, behavior: PermissionBehavior, source: PermissionRuleSource) -> PermissionRule {
        PermissionRule {
            value: PermissionRuleValue::from_rule_string(spec),
            behavior,
            source,
        }
    }

    fn snapshot() -> PermissionsSnapshot {
        PermissionsSnapshot {
            rules: vec![
                rule("Read", PermissionBehavior::Allow, PermissionRuleSource::LocalSettings),
                rule(
                    "Edit(src/**)",
                    PermissionBehavior::Allow,
                    PermissionRuleSource::ProjectSettings,
                ),
                rule(
                    "Bash(rm:*)",
                    PermissionBehavior::Deny,
                    PermissionRuleSource::LocalSettings,
                ),
            ],
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
        assert_eq!(handle_perm_key(&mut s, press(KeyCode::Right)), PermEditorOutcome::Stay);
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
    fn typing_then_enter_adds_a_rule_and_keeps_the_editor_open() {
        let mut s = state();
        for c in "Bash(npm:*)".chars() {
            assert_eq!(handle_perm_key(&mut s, press(KeyCode::Char(c))), PermEditorOutcome::Stay);
        }
        assert_eq!(s.input(), "Bash(npm:*)");
        let outcome = handle_perm_key(&mut s, press(KeyCode::Enter));
        assert_eq!(
            outcome,
            PermEditorOutcome::Add {
                rule: "Bash(npm:*)".to_string(),
                behavior: PermissionBehavior::Allow,
                dest: PermissionUpdateDestination::LocalSettings,
            }
        );
        // Buffer cleared, and the rule appears optimistically in the Allow tab.
        assert_eq!(s.input(), "");
        assert!(s
            .rows_for_tab()
            .iter()
            .any(|r| r.value.to_rule_string() == "Bash(npm:*)"));
    }

    #[test]
    fn ctrl_s_cycles_the_add_destination() {
        let mut s = state();
        assert_eq!(s.destination(), PermissionUpdateDestination::LocalSettings);
        let _ = handle_perm_key(&mut s, ctrl('s'));
        assert_eq!(s.destination(), PermissionUpdateDestination::ProjectSettings);
        let _ = handle_perm_key(&mut s, ctrl('s'));
        assert_eq!(s.destination(), PermissionUpdateDestination::UserSettings);
        let _ = handle_perm_key(&mut s, ctrl('s'));
        assert_eq!(s.destination(), PermissionUpdateDestination::LocalSettings);
    }

    #[test]
    fn enter_confirms_then_removes_the_selected_rule() {
        let mut s = state();
        // First Enter (empty buffer) arms confirmation.
        assert_eq!(handle_perm_key(&mut s, press(KeyCode::Enter)), PermEditorOutcome::Stay);
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
        // Optimistically gone; selection clamps to the remaining row.
        assert!(!s.is_confirming_remove());
        let rows: Vec<String> = s
            .rows_for_tab()
            .iter()
            .map(|r| r.value.to_rule_string())
            .collect();
        assert_eq!(rows, vec!["Edit(src/**)".to_string()]);
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
                PermissionRuleSource::PolicySettings,
            )],
        });
        // Enter over a managed row arms nothing.
        assert_eq!(handle_perm_key(&mut s, press(KeyCode::Enter)), PermEditorOutcome::Stay);
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
        assert_eq!(handle_perm_key(&mut s, press(KeyCode::Esc)), PermEditorOutcome::Stay);
        assert_eq!(s.input(), "");
        // Esc with an empty buffer closes.
        assert_eq!(handle_perm_key(&mut s, press(KeyCode::Esc)), PermEditorOutcome::Cancel);
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
}
