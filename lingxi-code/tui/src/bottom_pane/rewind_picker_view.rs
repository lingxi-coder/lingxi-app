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
//! summarize_up_to | nevermind`. All five are here.
//!
//! ⚠️ The two `summarize*` options are NOT restores and take a different exit:
//! they emit [`ViewOutcome::Summarize`], which does NOT unwind the app loop,
//! because the summarizer runs over the CURRENT conversation. Upstream carries
//! them as list rows with an inline `{type:"input", placeholder:"add context
//! (optional)"}`; this picker cycles scopes rather than listing options, so the
//! two summarize scopes join the cycle and each owns its OWN draft — a context
//! typed against "from here" must not leak into "up to here" (upstream keeps
//! them in separate `onChange` handlers for the same reason).

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
    /// The "add context (optional)" draft for `SummarizeFrom`.
    from_draft: String,
    /// …and the separate one for `SummarizeUpTo`.
    up_to_draft: String,
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
            from_draft: String::new(),
            up_to_draft: String::new(),
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

    /// The cycle order: Both → Code → Conversation → Summarize from →
    /// Summarize up to → Both.
    ///
    /// The three restore scopes keep their existing order and the two
    /// summarize options are appended, matching upstream's list, which also
    /// pushes them after the restore rows.
    const SCOPE_CYCLE: [RewindScope; 5] = [
        RewindScope::CodeAndConversation,
        RewindScope::CodeOnly,
        RewindScope::ConversationOnly,
        RewindScope::SummarizeFrom,
        RewindScope::SummarizeUpTo,
    ];

    fn cycle_scope(&mut self, forward: bool) {
        let len = Self::SCOPE_CYCLE.len();
        let at = Self::SCOPE_CYCLE
            .iter()
            .position(|candidate| *candidate == self.scope)
            .unwrap_or(0);
        let next = if forward {
            (at + 1) % len
        } else {
            (at + len - 1) % len
        };
        self.scope = Self::SCOPE_CYCLE[next];
    }

    /// The draft belonging to the currently selected summarize scope.
    fn active_draft_mut(&mut self) -> Option<&mut String> {
        match self.scope {
            RewindScope::SummarizeFrom => Some(&mut self.from_draft),
            RewindScope::SummarizeUpTo => Some(&mut self.up_to_draft),
            _ => None,
        }
    }

    /// The trimmed context for the selected summarize scope, or `None` when the
    /// draft is empty (`allowEmptySubmitToCancel` ⇒ an empty submit still
    /// selects the option, with no extra context).
    #[must_use]
    pub fn summarize_context(&self) -> Option<String> {
        let draft = match self.scope {
            RewindScope::SummarizeFrom => &self.from_draft,
            RewindScope::SummarizeUpTo => &self.up_to_draft,
            _ => return None,
        };
        let trimmed = draft.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
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
            let title_style = if i == self.selected {
                accent
            } else {
                Style::default()
            };
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
        // `scope_label` already begins with "Restore …"/"Summarize …" (the
        // 2.1.205 scope text), so it is shown directly — no redundant prefix.
        // A summarize scope renders its own draft after `labelValueSeparator`
        // (`": "`), with the placeholder standing in while it is empty
        // (upstream's `showLabelWithValue` + `placeholder`).
        let action = match self.scope {
            RewindScope::SummarizeFrom | RewindScope::SummarizeUpTo => {
                let draft = self.summarize_context();
                format!(
                    "{}: {}",
                    scope_label(self.scope),
                    draft.as_deref().unwrap_or(SUMMARIZE_PLACEHOLDER)
                )
            }
            _ => scope_label(self.scope).to_string(),
        };
        lines.push(Line::from(Span::styled(
            format!("{action}   (Tab to change · Enter to confirm · Esc to cancel)"),
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
        RewindScope::SummarizeFrom => "Summarize from here",
        RewindScope::SummarizeUpTo => "Summarize up to here",
    }
}

/// Upstream's inline-input placeholder for both summarize rows.
const SUMMARIZE_PLACEHOLDER: &str = "add context (optional)";

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
                Some(row) => {
                    let message = row.message_uuid;
                    match self.scope.summarize_direction() {
                        Some(direction) => ViewOutcome::Summarize {
                            message,
                            direction,
                            context: self.summarize_context(),
                        },
                        None => ViewOutcome::Rewind {
                            message,
                            scope: self.scope,
                        },
                    }
                }
                None => ViewOutcome::Cancelled,
            },
            // Typing only reaches a draft while a summarize scope is selected;
            // on a restore scope the picker stays a pure list, as before.
            KeyCode::Backspace => {
                if let Some(draft) = self.active_draft_mut() {
                    draft.pop();
                }
                ViewOutcome::Pending
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(draft) = self.active_draft_mut() {
                    draft.push(c);
                }
                ViewOutcome::Pending
            }
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
    use platform_api::SummarizeDirection;

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

    fn tab_to(view: &mut RewindPickerView, target: RewindScope) {
        for _ in 0..RewindPickerView::SCOPE_CYCLE.len() {
            if view.scope() == target {
                return;
            }
            view.handle_key(press(KeyCode::Tab));
        }
        panic!("{target:?} is not reachable by cycling");
    }

    fn type_text(view: &mut RewindPickerView, text: &str) {
        for c in text.chars() {
            view.handle_key(press(KeyCode::Char(c)));
        }
    }

    /// ⛔ The property that keeps this feature correct: a summarize takes a
    /// DIFFERENT exit from a restore. `Rewind` unwinds the app loop and
    /// re-mounts against a truncated transcript; a summarize needs that
    /// transcript, so it must never travel that path.
    #[test]
    fn a_summarize_scope_never_emits_a_rewind() {
        for (scope, want_direction) in [
            (RewindScope::SummarizeFrom, SummarizeDirection::From),
            (RewindScope::SummarizeUpTo, SummarizeDirection::UpTo),
        ] {
            let rows = vec![row("first"), row("second")];
            let want = rows[1].message_uuid;
            let mut v = view(rows);
            tab_to(&mut v, scope);
            match v.handle_key(press(KeyCode::Enter)) {
                ViewOutcome::Summarize {
                    message,
                    direction,
                    context,
                } => {
                    assert_eq!(message, want);
                    assert_eq!(direction, want_direction);
                    assert_eq!(context, None, "an untouched draft carries no context");
                }
                _ => panic!("expected Summarize for {scope:?}"),
            }
        }
    }

    /// …and the three restore scopes still take the restore exit.
    #[test]
    fn a_restore_scope_still_emits_a_rewind() {
        for scope in [
            RewindScope::CodeAndConversation,
            RewindScope::CodeOnly,
            RewindScope::ConversationOnly,
        ] {
            let mut v = view(vec![row("a")]);
            tab_to(&mut v, scope);
            assert!(
                matches!(v.handle_key(press(KeyCode::Enter)), ViewOutcome::Rewind { .. }),
                "{scope:?} must still restore"
            );
        }
    }

    /// 🚨 Each summarize row owns its OWN draft. Sharing one buffer would send
    /// the context typed for "from here" to an "up to here" summary — a wrong
    /// but entirely plausible summary, with no error anywhere.
    #[test]
    fn the_two_summarize_drafts_do_not_leak_into_each_other() {
        let mut v = view(vec![row("a")]);
        tab_to(&mut v, RewindScope::SummarizeFrom);
        type_text(&mut v, "keep the parser notes");
        assert_eq!(
            v.summarize_context().as_deref(),
            Some("keep the parser notes")
        );

        tab_to(&mut v, RewindScope::SummarizeUpTo);
        assert_eq!(
            v.summarize_context(),
            None,
            "the other row must start empty"
        );
        type_text(&mut v, "drop the logs");
        assert_eq!(v.summarize_context().as_deref(), Some("drop the logs"));

        tab_to(&mut v, RewindScope::SummarizeFrom);
        assert_eq!(
            v.summarize_context().as_deref(),
            Some("keep the parser notes"),
            "the first draft must survive a visit to the second"
        );
    }

    /// Typing belongs to the summarize rows only — on a restore scope the
    /// picker stays a pure list and a stray keystroke must not become context.
    #[test]
    fn typing_on_a_restore_scope_is_not_captured() {
        let mut v = view(vec![row("a")]);
        assert_eq!(v.scope(), RewindScope::CodeAndConversation);
        type_text(&mut v, "hello");
        tab_to(&mut v, RewindScope::SummarizeFrom);
        assert_eq!(
            v.summarize_context(),
            None,
            "keys pressed on a restore scope must not land in a draft"
        );
    }

    /// `allowEmptySubmitToCancel` reads like "an empty submit cancels" and means
    /// the opposite: `onSubmit` selects the option when the value is empty,
    /// and `Vo` then passes `text.trim() || undefined`. So an empty draft
    /// SUMMARIZES with no extra context — it does not cancel.
    #[test]
    fn an_empty_draft_summarizes_rather_than_cancelling() {
        let mut v = view(vec![row("a")]);
        tab_to(&mut v, RewindScope::SummarizeUpTo);
        type_text(&mut v, "   ");
        match v.handle_key(press(KeyCode::Enter)) {
            ViewOutcome::Summarize { context, .. } => assert_eq!(context, None),
            _ => panic!("a blank draft must still summarize"),
        }
    }

    #[test]
    fn backspace_edits_the_active_draft() {
        let mut v = view(vec![row("a")]);
        tab_to(&mut v, RewindScope::SummarizeFrom);
        type_text(&mut v, "abc");
        v.handle_key(press(KeyCode::Backspace));
        assert_eq!(v.summarize_context().as_deref(), Some("ab"));
    }

    /// The cycle reaches all five and returns; Tab and BackTab are inverses.
    #[test]
    fn the_scope_cycle_covers_every_option_in_both_directions() {
        let mut v = view(vec![row("a")]);
        let mut seen = Vec::new();
        for _ in 0..RewindPickerView::SCOPE_CYCLE.len() {
            seen.push(v.scope());
            v.handle_key(press(KeyCode::Tab));
        }
        assert_eq!(seen, RewindPickerView::SCOPE_CYCLE.to_vec());
        assert_eq!(
            v.scope(),
            RewindScope::CodeAndConversation,
            "a full cycle returns to the start"
        );
        v.handle_key(press(KeyCode::BackTab));
        assert_eq!(v.scope(), RewindScope::SummarizeUpTo, "BackTab wraps back");
    }

    #[test]
    fn tab_cycles_the_restore_scope() {
        let mut v = view(vec![row("a")]);
        assert_eq!(v.scope(), RewindScope::CodeAndConversation);
        v.handle_key(press(KeyCode::Tab));
        assert_eq!(v.scope(), RewindScope::CodeOnly);
        v.handle_key(press(KeyCode::Tab));
        assert_eq!(v.scope(), RewindScope::ConversationOnly);
        // The three RESTORE scopes keep this order. The next Tab leaves the
        // restore group for the summarize options rather than wrapping — the
        // cycle is five long now, and the wrap itself is asserted by
        // `the_scope_cycle_covers_every_option_in_both_directions`.
        v.handle_key(press(KeyCode::Tab));
        assert_eq!(v.scope(), RewindScope::SummarizeFrom);
        assert_eq!(
            v.scope().summarize_direction(),
            Some(SummarizeDirection::From),
            "…and it is a summarize option, not a fourth restore scope"
        );
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
        assert!(matches!(
            v.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
    }

    #[test]
    fn empty_state_enter_cancels() {
        let mut v = view(vec![]);
        assert!(matches!(
            v.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Cancelled
        ));
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
