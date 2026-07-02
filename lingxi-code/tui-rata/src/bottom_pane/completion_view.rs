//! The composer completion popup: a small list of candidates shown just above
//! the composer while a `/command` or `@file` token is being typed (ported
//! from the former `palette::CompletionPopup`, plan Phase 4).
//!
//! Presentational + selection only — the app decides how to APPLY the chosen
//! item (a `/command` replaces the whole buffer; an `@file` replaces just the
//! token). Modeled on codex's `bottom_pane` completion popup, anchored above
//! the composer rather than a full-screen modal. Unlike the modal
//! [`crate::bottom_pane::view::BottomPaneView`]s it is NOT stacked: it
//! coexists with the composer (typing keeps filtering), so its state stays
//! with the composer owner (`RataApp` today, `BottomPane` in plan Phase 5).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

/// Rows shown in the popup before it stops growing.
const MAX_ROWS: usize = 6;

/// The authoritative list of slash commands the ratatui backend handles
/// (`app::RataApp::handle_slash`), with one-line descriptions. Keep in sync
/// with the router.
pub const COMMANDS: &[(&str, &str)] = &[
    ("/help", "Show shortcuts and commands"),
    ("/model", "Switch the active model"),
    ("/doctor", "Show diagnostics"),
    ("/mcp", "List MCP servers"),
    ("/hooks", "List hooks"),
    ("/agents", "List agents"),
    ("/vim", "Toggle vim editing mode"),
    ("/clear", "Clear the conversation"),
    ("/exit", "Exit LingXi"),
];

/// One completion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// The text shown in the popup's left column.
    pub label: String,
    /// The text inserted when the item is chosen.
    pub insert: String,
    /// A dim right-column hint (command description, or `""`).
    pub desc: String,
}

/// The commands whose name starts with `prefix` (a `/`-led token), as
/// completion items. Empty when `prefix` is not a command fragment.
#[must_use]
pub fn command_items(prefix: &str) -> Vec<CompletionItem> {
    if !prefix.starts_with('/') {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, desc)| CompletionItem {
            label: (*name).to_string(),
            insert: (*name).to_string(),
            desc: (*desc).to_string(),
        })
        .collect()
}

/// A completion popup over a candidate list.
pub struct CompletionView {
    items: Vec<CompletionItem>,
    selected: usize,
}

impl CompletionView {
    /// Build a popup over `items` (highlight at the top). Returns `None` when
    /// there is nothing to show.
    #[must_use]
    pub fn new(items: Vec<CompletionItem>) -> Option<Self> {
        if items.is_empty() {
            None
        } else {
            Some(Self { items, selected: 0 })
        }
    }

    /// The text the highlighted item inserts.
    #[must_use]
    pub fn selected_insert(&self) -> &str {
        &self.items[self.selected].insert
    }

    /// Highlighted row index (exposed for tests).
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Move the highlight up (clamped).
    pub fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the highlight down (clamped).
    pub fn next(&mut self) {
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
        }
    }

    /// Draw the popup anchored just above `composer` (bordered list, cleared
    /// beneath), rendering into `buf` (`(Rect, &mut Buffer)` contract). Grows
    /// upward from the composer's top edge.
    pub fn render(&self, composer: Rect, buf: &mut Buffer) {
        let rows = self.items.len().min(MAX_ROWS);
        let height = u16::try_from(rows + 2).unwrap_or(u16::MAX);
        let y = composer.y.saturating_sub(height);
        let rect = Rect {
            x: composer.x,
            y,
            width: composer.width,
            height: height.min(composer.y),
        };
        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title("Complete");
        let inner = block.inner(rect);
        block.render(rect, buf);

        let lines: Vec<Line> = self
            .items
            .iter()
            .take(MAX_ROWS)
            .enumerate()
            .map(|(i, item)| {
                let caret = if i == self.selected { "› " } else { "  " };
                let style = if i == self.selected {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                let mut spans = vec![Span::styled(format!("{caret}{}", item.label), style)];
                if !item.desc.is_empty() {
                    spans.push(Span::styled(
                        format!("  {}", item.desc),
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                }
                Line::from(spans)
            })
            .collect();
        Paragraph::new(lines).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_items_filter_by_prefix() {
        // A bare "/" matches every command.
        assert_eq!(command_items("/").len(), COMMANDS.len());
        // "/m" matches /model + /mcp.
        let m = command_items("/m");
        assert!(m.iter().any(|i| i.insert == "/model"));
        assert!(m.iter().any(|i| i.insert == "/mcp"));
        assert!(!m.iter().any(|i| i.insert == "/help"));
        // Non-slash input yields nothing.
        assert!(command_items("model").is_empty());
        assert!(command_items("/zzz").is_empty());
    }

    #[test]
    fn new_is_none_when_empty() {
        assert!(CompletionView::new(Vec::new()).is_none());
        assert!(CompletionView::new(command_items("/h")).is_some());
    }

    #[test]
    fn nav_clamps_and_reports_selected_insert() {
        let mut p = CompletionView::new(command_items("/")).unwrap();
        assert_eq!(p.selected(), 0);
        p.prev(); // clamps at 0
        assert_eq!(p.selected(), 0);
        p.next();
        assert_eq!(p.selected(), 1);
        assert_eq!(p.selected_insert(), COMMANDS[1].0);
    }

    #[test]
    fn render_anchors_above_composer_rect_in_buffer() {
        let p = CompletionView::new(command_items("/m")).unwrap();
        let screen = Rect::new(0, 0, 40, 12);
        // Composer occupies the bottom rows; the popup grows upward from its top.
        let composer = Rect::new(0, 8, 40, 4);
        let mut buf = Buffer::empty(screen);
        p.render(composer, &mut buf);
        let text: String = (screen.top()..screen.bottom())
            .map(|y| {
                (screen.left()..screen.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Complete"), "{text}");
        assert!(text.contains("› /model"), "highlighted match: {text}");
        assert!(text.contains("/mcp"), "{text}");
        // Everything the popup drew sits strictly above the composer rows.
        let composer_rows: String = (composer.top()..screen.bottom())
            .map(|y| {
                (screen.left()..screen.right())
                    .map(|x| {
                        buf.cell(ratatui::layout::Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect();
        assert!(
            composer_rows.trim().is_empty(),
            "popup stays above composer"
        );
    }
}
