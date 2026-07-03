//! Preview of queued user inputs shown above the composer (plan Phase 5).
//!
//! Modeled on codex-rs `tui/src/bottom_pane/pending_input_preview.rs` (UI
//! architecture pattern only). The `LingXi` data path has no queued-input
//! source yet — no steer queue, no held follow-up drafts — so this component
//! ships as the SEAM: [`BottomPane`](super::BottomPane) already lays it out
//! between the status row and the composer, and it renders/measures as zero
//! rows while empty. Once a queue source appears in `ChatWidget` (plan Phase
//! 6+), wiring is [`PendingInputPreview::set_queued_messages`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::renderable::Renderable;

/// Queued user inputs held while a turn is in progress, previewed above the
/// composer. Empty (zero-height, renders nothing) until a queue source exists.
#[derive(Debug, Default)]
pub struct PendingInputPreview {
    queued_messages: Vec<String>,
}

impl PendingInputPreview {
    /// An empty preview (the current production state: no queue source yet).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether there is nothing to preview.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queued_messages.is_empty()
    }

    /// The queued messages currently previewed, oldest first.
    #[must_use]
    pub fn queued_messages(&self) -> &[String] {
        &self.queued_messages
    }

    /// Replace the previewed queue — the wiring seam for the future
    /// `ChatWidget` queued-input source.
    pub fn set_queued_messages(&mut self, messages: Vec<String>) {
        self.queued_messages = messages;
    }

    /// The preview body: a header plus one row per queued message (first line
    /// only, so a multiline draft never floods the pane).
    fn lines(&self) -> Vec<Line<'static>> {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let mut lines = vec![Line::from(Span::styled("Queued messages:", dim))];
        for message in &self.queued_messages {
            let first = message.lines().next().unwrap_or_default();
            lines.push(Line::from(Span::styled(format!("  ↳ {first}"), dim)));
        }
        lines
    }
}

impl Renderable for PendingInputPreview {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if self.is_empty() {
            return;
        }
        Paragraph::new(self.lines()).render(area, buf);
    }

    /// One header row plus one row per queued message; zero while empty so the
    /// pane's layout is unchanged until a queue source exists.
    fn desired_height(&self, _width: u16) -> u16 {
        if self.is_empty() {
            0
        } else {
            u16::try_from(1 + self.queued_messages.len()).unwrap_or(u16::MAX)
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Position;

    use super::*;

    fn row(buf: &Buffer, y: u16) -> String {
        (buf.area.left()..buf.area.right())
            .map(|x| {
                buf.cell(Position::new(x, y))
                    .map_or(" ", ratatui::buffer::Cell::symbol)
            })
            .collect()
    }

    #[test]
    fn empty_preview_is_zero_height_and_renders_nothing() {
        let preview = PendingInputPreview::new();
        assert!(preview.is_empty());
        assert_eq!(preview.desired_height(80), 0);
        let area = Rect::new(0, 0, 20, 2);
        let mut buf = Buffer::empty(area);
        preview.render(area, &mut buf);
        assert_eq!(row(&buf, 0), " ".repeat(20), "empty preview paints nothing");
        assert_eq!(row(&buf, 1), " ".repeat(20));
    }

    #[test]
    fn queued_messages_render_header_plus_one_row_each() {
        let mut preview = PendingInputPreview::new();
        preview.set_queued_messages(vec![
            "first draft".to_string(),
            "second\nwith a hidden continuation".to_string(),
        ]);
        assert!(!preview.is_empty());
        assert_eq!(preview.queued_messages().len(), 2);
        assert_eq!(preview.desired_height(80), 3, "header + one row per entry");
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        preview.render(area, &mut buf);
        assert!(row(&buf, 0).starts_with("Queued messages:"));
        assert!(row(&buf, 1).starts_with("  ↳ first draft"));
        assert!(
            row(&buf, 2).starts_with("  ↳ second"),
            "only the first line of a multiline draft is previewed: {}",
            row(&buf, 2)
        );
        assert!(!row(&buf, 2).contains("continuation"));
    }

    #[test]
    fn clearing_the_queue_returns_to_the_empty_state() {
        let mut preview = PendingInputPreview::new();
        preview.set_queued_messages(vec!["x".to_string()]);
        assert_eq!(preview.desired_height(80), 2);
        preview.set_queued_messages(Vec::new());
        assert!(preview.is_empty());
        assert_eq!(preview.desired_height(80), 0);
    }
}
