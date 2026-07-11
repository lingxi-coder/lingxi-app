//! `/rewind` (aliases `/checkpoint`, `/undo`): an interactive restore-point
//! picker rendered IN THE BOTTOM PANE (a full-frame [`BottomPaneView`], same
//! contract as [`super::resume_picker_view::ResumePickerView`]). The list is the
//! per-turn checkpoints captured this session — one row per user message that
//! has a file-history snapshot and/or is a conversation rewind target. The user
//! picks a row and a RESTORE SCOPE (both / code-only / conversation-only, cycled
//! with Tab or ←/→), then `Enter` emits [`ViewOutcome::Rewind`] carrying the
//! target message uuid + scope.
//!
//! Unlike the off-loop effect views, the "conversation" and "both" scopes UNWIND
//! the app loop (the owner returns `AppExit::Rewind`) so the session is re-mounted
//! in-process against the truncated transcript — reusing the `/resume` +
//! `/branch` `mount_resumed_tui` re-mount seam. The "code-only" scope is a pure
//! filesystem side-effect run off-loop by the CLI (mirrors claude-code's
//! `fileHistoryRewind`), but is routed through the SAME unwind + re-mount for
//! uniformity (a code-only restore re-mounts the SAME session id, so no
//! transcript truncation happens — only files are rewound before the re-mount).
//!
//! Faithful to `claude-code/src/components/MessageSelector.tsx`, whose
//! `RestoreOption` union is `both | conversation | code | summarize |
//! summarize_up_to | nevermind`. This first cut ports the three concrete
//! restore scopes; the two `summarize*` options depend on compaction wiring and
//! are deferred (see the recipe's design decisions).

use std::any::Any;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tui_core::theme::Theme;
use uuid::Uuid;

use crate::bottom_pane::view::{BottomPaneView, RewindScope, ViewOutcome};
use crate::renderable::Renderable;

/// One selectable restore point: a past user message that has a captured
/// file-history checkpoint and/or serves as a conversation rewind target. Built
/// by the CLI loader from the session's checkpoint index (see the recipe's
/// `session::file_history` seam).
#[derive(Debug, Clone)]
pub struct RewindRow {
    /// The user message uuid this checkpoint is keyed on — the argument the
    /// restore paths (`fileHistoryRewind` code restore + transcript truncation)
    /// both take.
    pub message_uuid: Uuid,
    /// A one-line preview of the user prompt (already truncated by the loader).
    pub preview: String,
    /// A dim relative-time label (`"3 minutes ago"`), prebuilt by the loader so
    /// this view stays free of the session/time formatting deps.
    pub timestamp_label: String,
    /// Whether restoring to this point would change any file on disk
    /// (claude-code `fileHistoryHasAnyChanges`). Drives the ` · code changes`
    /// hint.
    pub has_code_changes: bool,
}

/// The `/rewind` picker view.
pub struct RewindPickerView {
    /// Restore points, oldest first; the last row is the most recent checkpoint.
    rows: Vec<RewindRow>,
    /// Currently highlighted row index (starts at the most recent).
    selected: usize,
    /// The restore scope the confirm will use (cycled with Tab / ←/→).
    scope: RewindScope,
    /// Active render palette.
    theme: Theme,
}

impl RewindPickerView {
    /// Build the picker over `rows` (checkpoint index, oldest first). Selects the
    /// most recent row and defaults to the `CodeAndConversation` scope (claude's
    /// default `both`).
    #[must_use]
    pub fn new(rows: Vec<RewindRow>, theme: Theme) -> Self {
        let selected = rows.len().saturating_sub(1);
        Self {
            rows,
            selected,
            scope: RewindScope::CodeAndConversation,
            theme,
        }
    }

    /// Test/inspection access to the highlighted row.
    #[must_use]
    pub fn selected_row(&self) -> Option<&RewindRow> {
        self.rows.get(self.selected)
    }

    /// The scope the confirm would apply.
    #[must_use]
    pub fn scope(&self) -> RewindScope {
        self.scope
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.rows.len() {
            self.selected += 1;
        }
    }

    /// Cycle the restore scope: Both → Code → Conversation → Both.
    fn cycle_scope(&mut self, forward: bool) {
        self.scope = match (self.scope, forward) {
            (RewindScope::CodeAndConversation, true) => RewindScope::CodeOnly,
            (RewindScope::CodeOnly, true) => RewindScope::ConversationOnly,
            (RewindScope::ConversationOnly, true) => RewindScope::CodeAndConversation,
            (RewindScope::CodeAndConversation, false) => RewindScope::ConversationOnly,
            (RewindScope::CodeOnly, false) => RewindScope::CodeAndConversation,
            (RewindScope::ConversationOnly, false) => RewindScope::CodeOnly,
        };
    }

    /// The rendered body lines (header + rows + footer), reused by `render` and
    /// `desired_height` so the two never drift.
    fn lines(&self) -> Vec<Line<'static>> {
        let dim = Style::default().fg(crate::style_adapter::to_ratatui(self.theme.dim));
        let accent = Style::default()
            .fg(crate::style_adapter::to_ratatui(self.theme.suggestion))
            .add_modifier(Modifier::BOLD);

        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(Span::styled("Rewind", accent)));
        lines.push(Line::from(Span::styled(
            "Restore the code and/or conversation to a previous point".to_string(),
            dim,
        )));
        lines.push(Line::from(String::new()));

        if self.rows.is_empty() {
            lines.push(Line::from(Span::styled(
                "No checkpoints captured yet in this session".to_string(),
                dim,
            )));
            return lines;
        }

        for (i, row) in self.rows.iter().enumerate() {
            let marker = if i == self.selected { "> " } else { "  " };
            let title_style = if i == self.selected { accent } else { Style::default() };
            lines.push(Line::from(vec![
                Span::styled(marker.to_string(), title_style),
                Span::styled(row.preview.clone(), title_style),
            ]));
            let mut meta = row.timestamp_label.clone();
            if row.has_code_changes {
                meta.push_str(" \u{00b7} code changes");
            }
            lines.push(Line::from(Span::styled(format!("    {meta}"), dim)));
        }

        lines.push(Line::from(String::new()));
        // `scope_label` already begins with "Restore …" (the 2.1.205 scope text),
        // so it is shown directly — no redundant "Restore: " prefix.
        lines.push(Line::from(Span::styled(
            format!(
                "{}   (Tab to change · Enter to confirm · Esc to cancel)",
                scope_label(self.scope)
            ),
            dim,
        )));
        lines
    }

    /// Scroll offset keeping the selected row visible in a `viewport`-tall area.
    fn scroll_offset(&self, total: u16, viewport: u16) -> u16 {
        if viewport == 0 || total <= viewport {
            return 0;
        }
        let max_scroll = total - viewport;
        // 3 header lines, then 2 lines per row; reveal the selected title + meta.
        let selected_line = 3u16.saturating_add(u16::try_from(self.selected).unwrap_or(0) * 2);
        selected_line
            .saturating_add(2)
            .saturating_sub(viewport)
            .min(max_scroll)
    }
}

/// Human label for the confirm footer.
fn scope_label(scope: RewindScope) -> &'static str {
    match scope {
        RewindScope::CodeAndConversation => "Restore code and conversation",
        RewindScope::CodeOnly => "Restore code",
        RewindScope::ConversationOnly => "Restore conversation",
    }
}

impl Renderable for RewindPickerView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::new().borders(Borders::ALL);
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

impl BottomPaneView for RewindPickerView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        match key.code {
            KeyCode::Esc => ViewOutcome::Cancelled,
            KeyCode::Up => {
                self.move_up();
                ViewOutcome::Pending
            }
            KeyCode::Down => {
                self.move_down();
                ViewOutcome::Pending
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_up();
                ViewOutcome::Pending
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_down();
                ViewOutcome::Pending
            }
            KeyCode::Tab | KeyCode::Right => {
                self.cycle_scope(true);
                ViewOutcome::Pending
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.cycle_scope(false);
                ViewOutcome::Pending
            }
            KeyCode::Enter => match self.rows.get(self.selected) {
                Some(row) => ViewOutcome::Rewind {
                    message: row.message_uuid,
                    scope: self.scope,
                },
                None => ViewOutcome::Cancelled,
            },
            _ => ViewOutcome::Pending,
        }
    }

    /// A full-frame picker owns the whole viewport (same contract as
    /// [`super::resume_picker_view::ResumePickerView`]).
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
    use crossterm::event::KeyModifiers;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn row(preview: &str) -> RewindRow {
        RewindRow {
            message_uuid: Uuid::new_v4(),
            preview: preview.to_string(),
            timestamp_label: "1 minute ago".to_string(),
            has_code_changes: true,
        }
    }

    fn view(rows: Vec<RewindRow>) -> RewindPickerView {
        RewindPickerView::new(rows, Theme::dark())
    }

    #[test]
    fn enter_on_selected_emits_rewind_for_that_uuid_and_scope() {
        let rows = vec![row("first prompt"), row("second prompt")];
        let want = rows[1].message_uuid; // most recent = default selection
        let mut v = view(rows);
        match v.handle_key(press(KeyCode::Enter)) {
            ViewOutcome::Rewind { message, scope } => {
                assert_eq!(message, want);
                assert_eq!(scope, RewindScope::CodeAndConversation);
            }
            _ => panic!("expected Rewind"),
        }
    }

    #[test]
    fn tab_cycles_the_restore_scope() {
        let mut v = view(vec![row("a")]);
        assert_eq!(v.scope(), RewindScope::CodeAndConversation);
        v.handle_key(press(KeyCode::Tab));
        assert_eq!(v.scope(), RewindScope::CodeOnly);
        v.handle_key(press(KeyCode::Tab));
        assert_eq!(v.scope(), RewindScope::ConversationOnly);
        v.handle_key(press(KeyCode::Tab));
        assert_eq!(v.scope(), RewindScope::CodeAndConversation);
    }

    #[test]
    fn up_down_moves_selection_and_clamps() {
        let rows = vec![row("a"), row("b"), row("c")];
        let mut v = view(rows);
        assert_eq!(v.selected, 2);
        v.handle_key(press(KeyCode::Up));
        assert_eq!(v.selected, 1);
        v.handle_key(press(KeyCode::Down));
        assert_eq!(v.selected, 2);
        v.handle_key(press(KeyCode::Down));
        assert_eq!(v.selected, 2);
    }

    #[test]
    fn esc_cancels() {
        let mut v = view(vec![row("a")]);
        assert!(matches!(v.handle_key(press(KeyCode::Esc)), ViewOutcome::Cancelled));
    }

    #[test]
    fn empty_state_enter_cancels() {
        let mut v = view(vec![]);
        assert!(matches!(v.handle_key(press(KeyCode::Enter)), ViewOutcome::Cancelled));
    }

    #[test]
    fn full_frame_contract_suppresses_status_line() {
        assert!(!view(vec![row("a")]).wants_status_line());
    }

    #[test]
    fn renders_preview_and_scope_footer() {
        let v = view(vec![row("rewind me please")]);
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
        assert!(text.contains("rewind me please"), "{text}");
        // Footer shows the scope label directly (no doubled "Restore: Restore …").
        assert!(text.contains("Restore code and conversation"), "{text}");
        assert!(!text.contains("Restore: Restore"), "{text}");
    }
}

