//! `/plugin` (aliases `/plugins`, `/marketplace`): an interactive plugin
//! manager, rendered as a full-frame [`BottomPaneView`] (modeled on the
//! `/resume` picker [`crate::bottom_pane::resume_picker_view`]). Lists every
//! installed plugin with its enabled/disabled state and toggles the on-disk
//! `settings.enabledPlugins` allowlist via an off-loop
//! [`ViewOutcome::RunPluginAction`] (the owner runs the CLI
//! `plugin_settings::run_enable`/`run_disable` seam and refreshes the shared
//! snapshot). Like the `/permissions` editor
//! ([`crate::bottom_pane::permissions_editor_view`]) the list is NOT mutated
//! optimistically: a toggle reflects on the NEXT `/plugin` open (after the
//! write lands + the snapshot refreshes), and the outcome is reported through
//! `TurnEvent::SystemNotice` — the editor never shows a change that failed to
//! persist.
//!
//! Faithful-scope note: claude-code's `/plugin` (`commands/plugin/index.tsx` →
//! `PluginSettings.tsx`) is a full marketplace browser (install / uninstall /
//! add-marketplace). This first cut covers the enable/disable toggle over
//! already-installed plugins — the 90% case and the only action that is a pure
//! settings write. Install/marketplace network flows are a tracked follow-up
//! (each needs an additional `PluginAction` variant wired to the CLI
//! `plugin_install` / `plugin_marketplace` seams). Toggles take effect NEXT
//! session (or after `/reload-plugins`, once the live-refresh engine seam
//! lands) — matching claude-code's `needsRefresh` model where `/plugin` sets a
//! flag the user later activates.

use std::any::Any;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::bottom_pane::view::{BottomPaneView, PluginAction, ViewOutcome};
use crate::renderable::Renderable;

/// One installed plugin in the manager list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRow {
    /// The id `run_enable`/`run_disable` accept. The bare manifest name is
    /// used (the settings seam's `resolve_id` maps a bare name — or a
    /// `name@marketplace` id — to its canonical allowlist key).
    pub id: String,
    /// The plugin's manifest display name.
    pub name: String,
    /// Manifest version (may be empty).
    pub version: String,
    /// Manifest description (may be empty).
    pub description: String,
    /// Whether `settings.enabledPlugins` currently enables this plugin in any
    /// writable scope.
    pub enabled: bool,
}

/// A read-only snapshot of installed plugins used to seed the manager view.
/// Built OFF the render thread (the CLI resolves `discover_recorded_plugins`
/// joined with the merged `enabledPlugins` map) into a shared slot and re-read
/// after each toggle so the next `/plugin` open reflects the change. Plain
/// data — the loader lives in the CLI (`apps/cli/src/mode.rs`) because the
/// `tui` crate does not depend on the `plugin` crate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginsSnapshot {
    /// Installed plugins, in display order (the CLI sorts by name).
    pub plugins: Vec<PluginRow>,
}

/// The `/plugin` interactive manager view.
pub struct PluginsView {
    snapshot: PluginsSnapshot,
    selected: usize,
}

impl PluginsView {
    /// Build the manager over `snapshot` (already resolved by the CLI). Selects
    /// the first row.
    #[must_use]
    pub fn new(snapshot: PluginsSnapshot) -> Self {
        Self {
            snapshot,
            selected: 0,
        }
    }

    /// Test/inspection access to the seed snapshot.
    #[must_use]
    pub fn snapshot(&self) -> &PluginsSnapshot {
        &self.snapshot
    }

    /// The currently-selected row index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        let last = self.snapshot.plugins.len().saturating_sub(1);
        if self.selected < last {
            self.selected += 1;
        }
    }

    /// The action for the selected row: Enable a disabled plugin, Disable an
    /// enabled one. `None` when the list is empty.
    fn toggle_selected(&self) -> Option<PluginAction> {
        let row = self.snapshot.plugins.get(self.selected)?;
        Some(if row.enabled {
            PluginAction::Disable { id: row.id.clone() }
        } else {
            PluginAction::Enable { id: row.id.clone() }
        })
    }

    /// The rendered body lines (header + one/two lines per plugin + footer).
    fn lines(&self) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(Span::styled(
            "Manage Plugins",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        if self.snapshot.plugins.is_empty() {
            lines.push(Line::from(Span::styled(
                "No plugins installed. Use `lingxi-cli plugin install <name>` to add one.",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            for (i, row) in self.snapshot.plugins.iter().enumerate() {
                let marker = if row.enabled { "[x]" } else { "[ ]" };
                let marker_style = if row.enabled {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().add_modifier(Modifier::DIM)
                };
                let mut name = row.name.clone();
                if !row.version.is_empty() {
                    name.push_str(&format!(" v{}", row.version));
                }
                let name_style = if i == self.selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::styled(marker, marker_style),
                    Span::raw(" "),
                    Span::styled(name, name_style),
                ]));
                if !row.description.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("    {}", row.description),
                        Style::default().add_modifier(Modifier::DIM),
                    )));
                }
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "\u{2191}/\u{2193} select \u{00b7} space/enter toggle \u{00b7} esc close  (changes apply after restart or /reload-plugins)",
            Style::default().add_modifier(Modifier::DIM),
        )));
        lines
    }

    /// Vertical scroll offset keeping the selected row visible in a
    /// `viewport`-tall inner area (same shape as the `/resume` picker's).
    fn scroll_offset(&self, total: u16, viewport: u16) -> u16 {
        if viewport == 0 || total <= viewport {
            return 0;
        }
        let max_scroll = total - viewport;
        // Header is 2 lines; each plugin is 1-2 lines. Bias toward the
        // selection so a low pick scrolls into view (approximate but bounded).
        let selected = u16::try_from(self.selected).unwrap_or(0);
        selected
            .saturating_add(3)
            .saturating_sub(viewport)
            .min(max_scroll)
    }
}

impl Renderable for PluginsView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::new().borders(Borders::ALL).title(" Plugins ");
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let lines = self.lines();
        let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let scroll = self.scroll_offset(total, inner.height);
        Paragraph::new(lines).scroll((scroll, 0)).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        u16::try_from(self.lines().len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
    }
}

impl BottomPaneView for PluginsView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                ViewOutcome::Pending
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                ViewOutcome::Pending
            }
            KeyCode::Char(' ') | KeyCode::Enter => self
                .toggle_selected()
                .map_or(ViewOutcome::Pending, ViewOutcome::RunPluginAction),
            KeyCode::Esc => ViewOutcome::Cancelled,
            _ => ViewOutcome::Pending,
        }
    }

    /// A full-frame manager owns the whole viewport: no status row, no composer
    /// beneath (same contract as the `/resume` picker).
    fn wants_status_line(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row(name: &str, enabled: bool) -> PluginRow {
        PluginRow {
            id: name.to_string(),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: format!("{name} description"),
            enabled,
        }
    }

    fn view(rows: Vec<PluginRow>) -> PluginsView {
        PluginsView::new(PluginsSnapshot { plugins: rows })
    }

    #[test]
    fn enter_on_enabled_row_emits_disable() {
        let mut v = view(vec![row("weather", true)]);
        match v.handle_key(press(KeyCode::Enter)) {
            ViewOutcome::RunPluginAction(PluginAction::Disable { id }) => {
                assert_eq!(id, "weather");
            }
            other => panic!("expected Disable, got a different outcome: {:?}", matches!(other, ViewOutcome::Pending)),
        }
    }

    #[test]
    fn space_on_disabled_row_emits_enable() {
        let mut v = view(vec![row("weather", false)]);
        assert!(matches!(
            v.handle_key(press(KeyCode::Char(' '))),
            ViewOutcome::RunPluginAction(PluginAction::Enable { .. })
        ));
    }

    #[test]
    fn down_then_toggle_targets_the_second_row() {
        let mut v = view(vec![row("a", false), row("b", true)]);
        v.handle_key(press(KeyCode::Down));
        match v.handle_key(press(KeyCode::Enter)) {
            ViewOutcome::RunPluginAction(PluginAction::Disable { id }) => assert_eq!(id, "b"),
            _ => panic!("expected Disable for row b"),
        }
    }

    #[test]
    fn navigation_clamps_at_both_ends() {
        let mut v = view(vec![row("a", false), row("b", false)]);
        v.handle_key(press(KeyCode::Up));
        assert_eq!(v.selected(), 0);
        v.handle_key(press(KeyCode::Down));
        v.handle_key(press(KeyCode::Down));
        assert_eq!(v.selected(), 1);
    }

    #[test]
    fn esc_cancels() {
        let mut v = view(vec![row("a", true)]);
        assert!(matches!(v.handle_key(press(KeyCode::Esc)), ViewOutcome::Cancelled));
    }

    #[test]
    fn empty_list_toggle_is_pending() {
        let mut v = view(vec![]);
        assert!(matches!(v.handle_key(press(KeyCode::Enter)), ViewOutcome::Pending));
    }

    #[test]
    fn full_frame_suppresses_status_line() {
        assert!(!view(vec![row("a", true)]).wants_status_line());
    }

    #[test]
    fn renders_plugin_name_and_marker() {
        let v = view(vec![row("weather", true)]);
        let area = Rect::new(0, 0, 60, 12);
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
        assert!(text.contains("weather"), "{text}");
        assert!(text.contains("Manage Plugins"), "{text}");
    }
}

