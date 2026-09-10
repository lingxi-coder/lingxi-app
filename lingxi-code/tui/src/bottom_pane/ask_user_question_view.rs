//! The `AskUserQuestion` selection widget: a bottom-pane view that walks the
//! user through 1-4 multiple-choice questions and resolves the answer map.
//!
//! This is the live TUI counterpart of the headless
//! `tool_ui::ask_user_question` resolvers (plan GAP 3). Where an interactive
//! `askUserQuestionTimeout=never` prompt previously surfaced a `ToolError`
//! (the foundation's `DefaultTimeoutResolver` block path), a host resolver now
//! constructs an [`AskUserQuestionExchange`], pushes this view, and awaits the
//! user's REAL selections on the exchange's one-shot channel — exactly the
//! round-trip [`crate::bottom_pane::permission_view::PermissionView`] performs
//! for a permission prompt.
//!
//! Interaction (per question, walked in order):
//! - `↑`/`↓` (and `Tab`/`BackTab`) move the highlight, clamped at the edges
//!   (the deliberate `LingXi` no-wrap navigation, matching `DialogView`).
//! - Single-select: `Enter` (or a `1`-`9` number shortcut) picks the
//!   highlighted/numbered option and advances to the next question.
//! - Multi-select: `Space` (or a number) toggles the highlighted/numbered
//!   option's checkmark; `Enter` confirms the whole set and advances. An empty
//!   set falls back to the highlighted option so a question is never answered
//!   blank.
//! - On the LAST question, confirm resolves the whole answer map (question text
//!   → chosen label, multi-select labels `", "`-joined) through `resp_tx` and
//!   reports [`ViewOutcome::Accepted`].
//! - `Esc` cancels: `resp_tx` is dropped unsent, which the host resolver maps
//!   to a skip/error the way a dropped permission channel maps to `Deny`.
//!
//! When the exchange carries a timeout (`60s`/`5m`/`10m`), a dim
//! "auto-continue in {N}s · any key to stay" countdown line renders beneath the
//! options; the first key press disarms it ("press any key to stay") while
//! still acting normally. The live idle-tick that fires the auto-advance when
//! the window elapses is wired by the app loop (see [`Self::auto_submit`] +
//! [`Self::remaining_secs`]); the afk telemetry
//! (`tengu_ask_user_question_afk_*`) is the remaining step.

use std::any::Any;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossterm::cursor::SetCursorStyle;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tokio::sync::oneshot;
use tui_core::ask_user_question_bridge::{
    join_answer_labels, AskQuestion, AskUserQuestionExchange, ASK_MIDDOT,
};
use unicode_width::UnicodeWidthStr;

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, ViewAction, ViewOutcome};
use crate::renderable::Renderable;

/// Per-question selection state.
struct QuestionState {
    /// The highlighted (focused) option index.
    highlighted: usize,
    /// Checkmarks, one per option — only meaningful for `multi_select`.
    checked: Vec<bool>,
    /// Whether the synthetic free-text "Other" row is selected.
    other_selected: bool,
    /// The free-text value captured for the synthetic "Other" row.
    other_text: String,
    /// Whether the view is currently editing the free-text value.
    editing_other: bool,
}

impl QuestionState {
    fn new(option_count: usize) -> Self {
        Self {
            highlighted: 0,
            checked: vec![false; option_count],
            other_selected: false,
            other_text: String::new(),
            editing_other: false,
        }
    }

    fn pop_other_char(&mut self) {
        self.other_text.pop();
    }
}

/// The live `AskUserQuestion` selection widget.
pub struct AskUserQuestionView {
    /// The questions being walked (immutable prompt data).
    questions: Vec<AskQuestion>,
    /// Per-question selection state (parallel to `questions`).
    states: Vec<QuestionState>,
    /// Which question is active.
    current: usize,
    /// Answers committed for already-confirmed questions (question → label(s)).
    answers: HashMap<String, String>,
    /// One-shot reply channel, consumed by the final submit.
    resp_tx: Option<oneshot::Sender<HashMap<String, String>>>,
    /// The auto-continue idle window; `None` ⇒ `never` (no countdown).
    timeout: Option<Duration>,
    /// When the prompt appeared, for the countdown remaining-time math.
    started: Instant,
    /// Whether the auto-continue countdown is still armed. Set false on the
    /// first key press ("press any key to stay").
    armed: bool,
}

impl AskUserQuestionView {
    const OTHER_LABEL: &str = "Other";

    /// Whether the asker that opened this dialog has already unwound (the
    /// future holding the one-shot receiving half was dropped). Same finding-4
    /// predicate as [`crate::bottom_pane::permission_view::PermissionView::is_asker_gone`];
    /// read by [`crate::bottom_pane::ViewStack::drop_abandoned_prompts`] to
    /// remove a dialog that went ownerless while it was already open.
    #[must_use]
    pub fn is_asker_gone(&self) -> bool {
        self.resp_tx
            .as_ref()
            .is_some_and(oneshot::Sender::is_closed)
    }

    /// Build the widget for `exchange` (questions + optional timeout + the
    /// answer channel).
    #[must_use]
    pub fn new(exchange: AskUserQuestionExchange) -> Self {
        let AskUserQuestionExchange {
            questions,
            timeout_secs,
            resp_tx,
        } = exchange;
        let states = questions
            .iter()
            .map(|q| QuestionState::new(q.options.len()))
            .collect();
        let timeout = timeout_secs.map(Duration::from_secs);
        Self {
            questions,
            states,
            current: 0,
            answers: HashMap::new(),
            resp_tx: Some(resp_tx),
            timeout,
            started: Instant::now(),
            armed: timeout.is_some(),
        }
    }

    /// The active question.
    fn question(&self) -> &AskQuestion {
        &self.questions[self.current]
    }

    fn state(&self) -> &QuestionState {
        &self.states[self.current]
    }

    fn state_mut(&mut self) -> &mut QuestionState {
        &mut self.states[self.current]
    }

    fn option_row_count(&self) -> usize {
        self.question().options.len() + 1
    }

    fn other_index(&self) -> usize {
        self.question().options.len()
    }

    fn editing_other(&self) -> bool {
        self.state().editing_other
    }

    fn other_answer_text(&self) -> Option<String> {
        let trimmed = self.state().other_text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    fn open_other_editor(&mut self) -> ViewOutcome {
        let other_index = self.other_index();
        let multi = self.question().multi_select;
        let st = self.state_mut();
        st.highlighted = other_index;
        st.editing_other = true;
        if multi {
            st.other_selected = true;
        }
        ViewOutcome::Pending
    }

    fn confirm_other_editor(&mut self) -> ViewOutcome {
        let multi = self.question().multi_select;
        {
            let st = self.state_mut();
            if st.other_text.trim().is_empty() {
                return ViewOutcome::Pending;
            }
            st.editing_other = false;
            st.other_selected = true;
        }
        if multi {
            ViewOutcome::Pending
        } else {
            self.confirm_and_advance()
        }
    }

    /// Seconds remaining before auto-continue at `now` (`None` when there is no
    /// armed timeout). Saturates at zero.
    #[must_use]
    pub fn remaining_secs_at(&self, now: Instant) -> Option<u64> {
        if !self.armed {
            return None;
        }
        let window = self.timeout?;
        let elapsed = now.saturating_duration_since(self.started);
        Some(window.saturating_sub(elapsed).as_secs())
    }

    /// Seconds remaining now (convenience over [`Self::remaining_secs_at`]).
    #[must_use]
    pub fn remaining_secs(&self) -> Option<u64> {
        self.remaining_secs_at(Instant::now())
    }

    /// Whether the armed idle window has elapsed at `now` (the app loop's
    /// signal to call [`Self::auto_submit`]).
    #[must_use]
    pub fn is_expired_at(&self, now: Instant) -> bool {
        match (self.armed, self.timeout) {
            (true, Some(window)) => now.saturating_duration_since(self.started) >= window,
            _ => false,
        }
    }

    /// The dim countdown line, when the timeout is still armed. Byte-locked to
    /// the oracle template `"auto-continue in ",i,"s · any key to stay"`.
    #[must_use]
    pub fn countdown_line(remaining_secs: u64) -> String {
        format!("auto-continue in {remaining_secs}s {ASK_MIDDOT} any key to stay")
    }

    /// The committed answer for the active question given its selection state:
    /// the highlighted label for single-select, or the `", "`-joined checked
    /// labels for multi-select (falling back to the highlighted label when
    /// nothing is checked, so a question is never answered blank).
    fn active_answer(&self) -> String {
        let q = self.question();
        let st = self.state();
        if q.multi_select {
            let picked: Vec<String> = q
                .options
                .iter()
                .zip(&st.checked)
                .filter_map(|(opt, on)| on.then(|| opt.label.clone()))
                .collect();
            let mut picked = picked;
            if st.other_selected {
                if let Some(other) = self.other_answer_text() {
                    picked.push(other);
                }
            }
            if picked.is_empty() {
                if st.highlighted == self.other_index() {
                    self.other_answer_text()
                        .unwrap_or_else(|| q.options[0].label.clone())
                } else {
                    q.options[st.highlighted].label.clone()
                }
            } else {
                join_answer_labels(&picked)
            }
        } else if st.highlighted == self.other_index() {
            self.other_answer_text()
                .unwrap_or_else(|| q.options[0].label.clone())
        } else {
            q.options[st.highlighted].label.clone()
        }
    }

    /// Commit the active question's answer and advance. Returns the final
    /// [`ViewOutcome`]: `Pending` while more questions remain, or `Accepted`
    /// once the last question is confirmed (which also resolves `resp_tx`).
    fn confirm_and_advance(&mut self) -> ViewOutcome {
        let answer = self.active_answer();
        self.answers
            .insert(self.question().question.clone(), answer);
        if self.current + 1 < self.questions.len() {
            self.current += 1;
            ViewOutcome::Pending
        } else {
            self.submit()
        }
    }

    /// Deliver the collected answer map through the one-shot channel (first
    /// resolution only) and report acceptance.
    fn submit(&mut self) -> ViewOutcome {
        if let Some(tx) = self.resp_tx.take() {
            let _ = tx.send(std::mem::take(&mut self.answers));
        }
        ViewOutcome::Accepted(ViewAction::Selected(self.current))
    }

    /// Auto-continue with the answers the user actually CONFIRMED (the
    /// idle-timeout afk path). Unanswered questions are left out rather than
    /// back-filled from their current highlight — see the body. Called by the
    /// app loop when [`Self::is_expired_at`].
    pub fn auto_submit(&mut self) -> ViewOutcome {
        // afk idle-timeout: resolve with ONLY the questions the user already
        // CONFIRMED (present in `self.answers`). Do NOT fabricate answers for
        // unanswered questions by back-filling their current highlight / first
        // option — the timeout must never invent a selection the user never
        // made. (The `q.options[0]` fallback stays only on the explicit-Enter
        // `confirm_and_advance` path.)
        self.submit()
    }

    /// Toggle the checkmark at `idx` (multi-select only).
    fn toggle(&mut self, idx: usize) {
        if let Some(slot) = self.states[self.current].checked.get_mut(idx) {
            *slot = !*slot;
        }
    }

    fn append_other_text(&mut self, text: &str) {
        let normalized = text.trim_end_matches(['\n', '\r']);
        if normalized.is_empty() {
            return;
        }
        self.state_mut().other_text.push_str(normalized);
    }
}

impl AskUserQuestionView {
    /// The rendered body lines for the active question (header chip, question
    /// text, option rows, then the countdown line when armed).
    fn body_lines(&self, remaining: Option<u64>) -> Vec<Line<'static>> {
        let q = self.question();
        let st = self.state();
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Header chip + progress ("[Library] 1/2") then the question text.
        let progress = if self.questions.len() > 1 {
            format!(" {}/{}", self.current + 1, self.questions.len())
        } else {
            String::new()
        };
        lines.push(Line::from(Span::styled(
            format!("[{}]{progress}", q.header),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(q.question.clone()));
        lines.push(Line::from(""));

        if st.editing_other {
            lines.push(Line::from("Custom response"));
            lines.push(Line::from(Span::styled(
                format!("Other: {}", st.other_text),
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Type a custom response · Enter confirm · Esc back",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            for (i, opt) in q.options.iter().enumerate() {
                let focused = i == st.highlighted;
                let marker = if focused { "›" } else { " " };
                let checkbox = if q.multi_select {
                    if st.checked[i] {
                        "[x] "
                    } else {
                        "[ ] "
                    }
                } else {
                    ""
                };
                let style = if focused {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(
                    format!("{marker} {checkbox}{}", opt.label),
                    style,
                )));
            }

            let focused = st.highlighted == self.other_index();
            let marker = if focused { "›" } else { " " };
            let checkbox = if q.multi_select {
                if st.other_selected {
                    "[x] "
                } else {
                    "[ ] "
                }
            } else {
                ""
            };
            let other_label = if st.other_text.is_empty() {
                Self::OTHER_LABEL.to_string()
            } else {
                format!("{}: {}", Self::OTHER_LABEL, st.other_text)
            };
            let style = if focused {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("{marker} {checkbox}{other_label}"),
                style,
            )));
        }

        if let Some(secs) = remaining {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                Self::countdown_line(secs),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        lines
    }

    /// Modal box height: body rows + border chrome.
    fn modal_height(&self) -> u16 {
        let mut rows = if self.editing_other() {
            7
        } else {
            3 + self.option_row_count()
        };
        if self.armed && self.timeout.is_some() {
            rows += 2;
        }
        u16::try_from(rows + 2).unwrap_or(u16::MAX)
    }
}

impl Renderable for AskUserQuestionView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let remaining = self.remaining_secs_at(Instant::now());
        let lines = self.body_lines(remaining);

        let content_w = lines
            .iter()
            .map(ratatui::text::Line::width)
            .max()
            .unwrap_or(20);
        let width = u16::try_from(content_w + 4)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(20);
        let height = self.modal_height().min(area.height);
        let rect = centered_rect(width, height, area);

        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title("Select an option");
        let inner = block.inner(rect);
        block.render(rect, buf);
        Paragraph::new(lines).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        self.modal_height()
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.editing_other() {
            return None;
        }
        let rect = centered_rect(
            u16::try_from(
                self.body_lines(self.remaining_secs_at(Instant::now()))
                    .iter()
                    .map(Line::width)
                    .max()
                    .unwrap_or(20)
                    + 4,
            )
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(20),
            self.modal_height().min(area.height),
            area,
        );
        let inner = Block::new().borders(Borders::ALL).inner(rect);
        if inner.width == 0 || inner.height < 4 {
            return None;
        }
        let typed = u16::try_from(self.state().other_text.as_str().width()).unwrap_or(u16::MAX);
        let x = inner
            .x
            .saturating_add(7)
            .saturating_add(typed)
            .min(inner.right().saturating_sub(1));
        Some((x, inner.y + 3))
    }

    fn cursor_style(&self, _area: Rect) -> SetCursorStyle {
        if self.editing_other() {
            SetCursorStyle::SteadyBar
        } else {
            SetCursorStyle::DefaultUserShape
        }
    }
}

impl BottomPaneView for AskUserQuestionView {
    fn handle_tick(&mut self, now: Instant) -> ViewOutcome {
        if self.is_expired_at(now) {
            self.auto_submit()
        } else {
            ViewOutcome::Pending
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        // "press any key to stay": the first key disarms the countdown, then is
        // still processed normally.
        self.armed = false;

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            // Ctrl chords stay swallowed by the modal (locked keyboard
            // ownership, matching the permission view).
            return ViewOutcome::Pending;
        }
        if self.editing_other() {
            return match key.code {
                KeyCode::Enter => self.confirm_other_editor(),
                KeyCode::Esc => {
                    let st = self.state_mut();
                    st.editing_other = false;
                    // Opening the multi-select editor tentatively selects the
                    // synthetic row. Esc with no text is a rollback, not a
                    // valid empty custom answer; otherwise a later Enter on a
                    // fixed option could submit the literal label `Other`.
                    if st.other_text.trim().is_empty() {
                        st.other_selected = false;
                    }
                    ViewOutcome::Pending
                }
                KeyCode::Backspace => {
                    self.state_mut().pop_other_char();
                    ViewOutcome::Pending
                }
                KeyCode::Char(c) => {
                    self.state_mut().other_text.push(c);
                    ViewOutcome::Pending
                }
                _ => ViewOutcome::Pending,
            };
        }
        let multi = self.question().multi_select;
        let opt_count = self.option_row_count();

        match key.code {
            KeyCode::Up | KeyCode::BackTab => {
                let st = &mut self.states[self.current];
                st.highlighted = st.highlighted.saturating_sub(1);
                ViewOutcome::Pending
            }
            KeyCode::Down | KeyCode::Tab => {
                let st = &mut self.states[self.current];
                if st.highlighted + 1 < opt_count {
                    st.highlighted += 1;
                }
                ViewOutcome::Pending
            }
            // Space toggles the highlighted option (multi-select only).
            KeyCode::Char(' ') if multi => {
                let idx = self.states[self.current].highlighted;
                if idx == self.other_index() {
                    let next = !self.state().other_selected;
                    self.state_mut().other_selected = next;
                    if next {
                        self.open_other_editor()
                    } else {
                        ViewOutcome::Pending
                    }
                } else {
                    self.toggle(idx);
                    ViewOutcome::Pending
                }
            }
            // Number shortcuts: multi-select toggles that option; single-select
            // picks it and advances (permission-view parity).
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as usize) - ('1' as usize);
                if idx >= opt_count {
                    return ViewOutcome::Pending;
                }
                if multi {
                    if idx == self.other_index() {
                        self.state_mut().other_selected = true;
                        self.open_other_editor()
                    } else {
                        self.toggle(idx);
                        ViewOutcome::Pending
                    }
                } else {
                    self.states[self.current].highlighted = idx;
                    if idx == self.other_index() && self.other_answer_text().is_none() {
                        self.open_other_editor()
                    } else {
                        self.confirm_and_advance()
                    }
                }
            }
            KeyCode::Enter => {
                if self.state().highlighted == self.other_index()
                    && self.other_answer_text().is_none()
                    && (!multi
                        || self.state().other_selected
                        || self.state().checked.iter().all(|on| !on))
                {
                    self.open_other_editor()
                } else {
                    self.confirm_and_advance()
                }
            }
            // Esc cancels: drop resp_tx unsent (host maps a closed channel to a
            // skip/error, mirroring the permission gate's dropped-channel deny).
            KeyCode::Esc => {
                self.resp_tx = None;
                ViewOutcome::Cancelled
            }
            _ => ViewOutcome::Pending,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn handle_paste(&mut self, text: &str) -> ViewOutcome {
        if self.editing_other() {
            self.append_other_text(text);
        }
        ViewOutcome::Pending
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;
    use ratatui::layout::Position;
    use tui_core::ask_user_question_bridge::{AskOption, AskQuestion};

    use super::*;

    fn q(question: &str, header: &str, labels: &[&str], multi: bool) -> AskQuestion {
        AskQuestion {
            question: question.to_string(),
            header: header.to_string(),
            options: labels
                .iter()
                .map(|l| AskOption::new(*l, format!("desc {l}")))
                .collect(),
            multi_select: multi,
        }
    }

    fn exchange(
        questions: Vec<AskQuestion>,
        timeout_secs: Option<u64>,
    ) -> (
        AskUserQuestionView,
        oneshot::Receiver<HashMap<String, String>>,
    ) {
        let (resp_tx, resp_rx) = oneshot::channel();
        let view = AskUserQuestionView::new(AskUserQuestionExchange {
            questions,
            timeout_secs,
            resp_tx,
        });
        (view, resp_rx)
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn buffer_text(view: &AskUserQuestionView, area: Rect) -> String {
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ===== rendering =====

    #[test]
    fn renders_header_question_and_option_rows() {
        let (view, _rx) = exchange(
            vec![q("Which library?", "Library", &["Chrono", "Time"], false)],
            None,
        );
        let text = buffer_text(&view, Rect::new(0, 0, 60, view.desired_height(60)));
        assert!(text.contains("[Library]"), "header chip: {text}");
        assert!(text.contains("Which library?"), "{text}");
        assert!(text.contains("› Chrono"), "highlight marker: {text}");
        assert!(text.contains("Time"), "{text}");
        // Single-question prompt shows no progress counter.
        assert!(!text.contains("1/1"), "{text}");
    }

    #[test]
    fn multiselect_rows_render_checkboxes() {
        let (view, _rx) = exchange(
            vec![q("Which features?", "Features", &["A", "B", "C"], true)],
            None,
        );
        let text = buffer_text(&view, Rect::new(0, 0, 60, view.desired_height(60)));
        assert!(text.contains("[ ] A"), "unchecked box: {text}");
        assert!(text.contains("Other"), "synthetic other row: {text}");
    }

    #[test]
    fn multi_question_shows_progress_counter() {
        let (view, _rx) = exchange(
            vec![
                q("Q1?", "H1", &["A", "B"], false),
                q("Q2?", "H2", &["C", "D"], false),
            ],
            None,
        );
        let text = buffer_text(&view, Rect::new(0, 0, 60, view.desired_height(60)));
        assert!(text.contains("1/2"), "progress counter: {text}");
    }

    // ===== single-select navigation + submit =====

    #[test]
    fn arrow_navigation_clamps_at_edges() {
        let (mut view, _rx) = exchange(vec![q("Q?", "H", &["A", "B", "C"], false)], None);
        assert_eq!(view.states[0].highlighted, 0);
        view.handle_key(press(KeyCode::Up)); // clamp at 0
        assert_eq!(view.states[0].highlighted, 0);
        view.handle_key(press(KeyCode::Down));
        view.handle_key(press(KeyCode::Down));
        view.handle_key(press(KeyCode::Down)); // synthetic Other row
        view.handle_key(press(KeyCode::Down)); // clamp at last
        assert_eq!(view.states[0].highlighted, 3);
    }

    #[test]
    fn enter_submits_highlighted_single_answer() {
        let (mut view, rx) = exchange(vec![q("Pick?", "H", &["Alpha", "Beta"], false)], None);
        view.handle_key(press(KeyCode::Down)); // highlight Beta
        let outcome = view.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ViewOutcome::Accepted(ViewAction::Selected(_))
        ));
        let answers = rx.blocking_recv().expect("submitted");
        assert_eq!(answers.get("Pick?").map(String::as_str), Some("Beta"));
    }

    #[test]
    fn number_shortcut_picks_and_submits_single_select() {
        let (mut view, rx) = exchange(
            vec![q("Pick?", "H", &["Alpha", "Beta", "Gamma"], false)],
            None,
        );
        let outcome = view.handle_key(press(KeyCode::Char('3')));
        assert!(matches!(
            outcome,
            ViewOutcome::Accepted(ViewAction::Selected(_))
        ));
        let answers = rx.blocking_recv().expect("submitted");
        assert_eq!(answers.get("Pick?").map(String::as_str), Some("Gamma"));
    }

    #[test]
    fn single_select_other_accepts_paste_and_unicode() {
        let (mut view, rx) = exchange(vec![q("Pick?", "H", &["Alpha", "Beta"], false)], None);
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('3'))),
            ViewOutcome::Pending
        ));
        let area = Rect::new(0, 0, 70, view.desired_height(70));
        assert!(
            view.cursor_pos(area).is_some(),
            "other editor claims cursor"
        );
        assert!(matches!(
            view.handle_paste("自由输入"),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            view.handle_paste(" paste\n"),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Accepted(_)
        ));
        let answers = rx.blocking_recv().expect("submitted");
        assert_eq!(
            answers.get("Pick?").map(String::as_str),
            Some("自由输入 paste")
        );
    }

    #[test]
    fn other_editor_cursor_uses_terminal_cell_width() {
        let area = Rect::new(0, 0, 100, 20);
        let (mut ascii, _rx) = exchange(vec![q("Pick?", "H", &["A", "B"], false)], None);
        ascii.handle_key(press(KeyCode::Char('3')));
        ascii.handle_paste("x");
        let ascii_x = ascii.cursor_pos(area).expect("ASCII cursor").0;

        let (mut wide, _rx) = exchange(vec![q("Pick?", "H", &["A", "B"], false)], None);
        wide.handle_key(press(KeyCode::Char('3')));
        wide.handle_paste("中");
        let wide_x = wide.cursor_pos(area).expect("wide cursor").0;

        assert_eq!(wide_x, ascii_x + 1, "CJK occupies two terminal cells");
    }

    #[test]
    fn out_of_range_number_is_ignored() {
        let (mut view, _rx) = exchange(vec![q("Q?", "H", &["A", "B"], false)], None);
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('9'))),
            ViewOutcome::Pending
        ));
        assert!(view.resp_tx.is_some(), "no submit on out-of-range number");
    }

    // ===== multi-select state machine =====

    #[test]
    fn space_toggles_checkmarks_and_enter_comma_joins() {
        let (mut view, rx) = exchange(vec![q("Which?", "H", &["A", "B", "C"], true)], None);
        // Check A (highlight 0), move to C (highlight 2), check C.
        view.handle_key(press(KeyCode::Char(' ')));
        assert!(view.states[0].checked[0]);
        view.handle_key(press(KeyCode::Down));
        view.handle_key(press(KeyCode::Down));
        view.handle_key(press(KeyCode::Char(' ')));
        assert!(view.states[0].checked[2]);
        // Toggle A off then on again to exercise the toggle.
        view.handle_key(press(KeyCode::Up));
        view.handle_key(press(KeyCode::Up));
        view.handle_key(press(KeyCode::Char(' '))); // A off
        assert!(!view.states[0].checked[0]);
        view.handle_key(press(KeyCode::Char(' '))); // A on
        let outcome = view.handle_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ViewOutcome::Accepted(_)));
        let answers = rx.blocking_recv().expect("submitted");
        // Labels join in OPTION order (A then C), not selection order.
        assert_eq!(answers.get("Which?").map(String::as_str), Some("A, C"));
    }

    #[test]
    fn number_toggles_in_multi_select_without_submitting() {
        let (mut view, _rx) = exchange(vec![q("Which?", "H", &["A", "B", "C"], true)], None);
        // A number toggles (does not submit) in multi-select.
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('2'))),
            ViewOutcome::Pending
        ));
        assert!(view.states[0].checked[1]);
        assert!(view.resp_tx.is_some(), "multi-select number never submits");
    }

    #[test]
    fn multi_select_other_can_mix_with_fixed_options() {
        let (mut view, rx) = exchange(vec![q("Which?", "H", &["A", "B"], true)], None);
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('1'))),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            view.handle_key(press(KeyCode::Char('3'))),
            ViewOutcome::Pending
        ));
        assert!(matches!(view.handle_paste("自定义"), ViewOutcome::Pending));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Pending
        ));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Accepted(_)
        ));
        let answers = rx.blocking_recv().expect("submitted");
        assert_eq!(answers.get("Which?").map(String::as_str), Some("A, 自定义"));
    }

    #[test]
    fn multi_select_other_escape_rolls_back_empty_selection() {
        let (mut view, rx) = exchange(vec![q("Which?", "H", &["A", "B"], true)], None);
        view.handle_key(press(KeyCode::Char('1'))); // select A
        view.handle_key(press(KeyCode::Char('3'))); // open Other editor
        assert!(view.states[0].other_selected);

        view.handle_key(press(KeyCode::Esc));
        assert!(!view.states[0].editing_other);
        assert!(
            !view.states[0].other_selected,
            "empty custom response must not remain selected"
        );

        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Accepted(_)
        ));
        let answers = rx.blocking_recv().expect("submitted");
        assert_eq!(answers.get("Which?").map(String::as_str), Some("A"));
    }

    #[test]
    fn multi_select_empty_falls_back_to_highlighted() {
        let (mut view, rx) = exchange(vec![q("Which?", "H", &["A", "B"], true)], None);
        // No checkmarks; Enter falls back to the highlighted option.
        view.handle_key(press(KeyCode::Enter));
        let answers = rx.blocking_recv().expect("submitted");
        assert_eq!(answers.get("Which?").map(String::as_str), Some("A"));
    }

    // ===== multi-question walk =====

    #[test]
    fn walks_all_questions_before_submitting() {
        let (mut view, rx) = exchange(
            vec![
                q("Q1?", "H1", &["A1", "B1"], false),
                q("Q2?", "H2", &["A2", "B2"], false),
            ],
            None,
        );
        // First Enter confirms Q1 and advances (still Pending, no submit yet).
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Pending
        ));
        assert_eq!(view.current, 1, "advanced to Q2");
        assert!(view.resp_tx.is_some(), "not submitted after first question");
        // Second Enter (Q2 → B2 via Down) submits the whole map.
        view.handle_key(press(KeyCode::Down));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Accepted(_)
        ));
        let answers = rx.blocking_recv().expect("submitted");
        assert_eq!(answers.get("Q1?").map(String::as_str), Some("A1"));
        assert_eq!(answers.get("Q2?").map(String::as_str), Some("B2"));
    }

    // ===== cancel =====

    #[test]
    fn esc_cancels_and_drops_the_channel() {
        let (mut view, rx) = exchange(vec![q("Q?", "H", &["A", "B"], false)], None);
        assert!(matches!(
            view.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
        assert!(
            rx.blocking_recv().is_err(),
            "channel closed unsent on cancel"
        );
    }

    #[test]
    fn dropping_the_view_unresolved_closes_the_channel() {
        let (view, rx) = exchange(vec![q("Q?", "H", &["A", "B"], false)], None);
        drop(view);
        assert!(rx.blocking_recv().is_err());
    }

    #[test]
    fn ctrl_chords_are_swallowed() {
        let (mut view, _rx) = exchange(vec![q("Q?", "H", &["A", "B"], false)], None);
        assert!(matches!(
            view.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            ViewOutcome::Pending
        ));
        assert!(view.resp_tx.is_some());
    }

    // ===== countdown =====

    #[test]
    fn countdown_line_is_byte_locked() {
        assert_eq!(
            AskUserQuestionView::countdown_line(42),
            "auto-continue in 42s · any key to stay"
        );
        assert_eq!(
            AskUserQuestionView::countdown_line(0),
            "auto-continue in 0s · any key to stay"
        );
    }

    #[test]
    fn countdown_renders_when_timeout_armed() {
        let (view, _rx) = exchange(vec![q("Q?", "H", &["A", "B"], false)], Some(60));
        let text = buffer_text(&view, Rect::new(0, 0, 60, view.desired_height(60)));
        assert!(text.contains("auto-continue in"), "countdown shown: {text}");
        assert!(text.contains("any key to stay"), "{text}");
    }

    #[test]
    fn no_countdown_when_timeout_never() {
        let (view, _rx) = exchange(vec![q("Q?", "H", &["A", "B"], false)], None);
        assert_eq!(view.remaining_secs(), None);
        let text = buffer_text(&view, Rect::new(0, 0, 60, view.desired_height(60)));
        assert!(!text.contains("auto-continue"), "no countdown: {text}");
    }

    #[test]
    fn first_key_disarms_the_countdown() {
        let (mut view, _rx) = exchange(vec![q("Q?", "H", &["A", "B"], false)], Some(60));
        assert!(view.remaining_secs().is_some(), "armed at start");
        // Any key (here a navigation key) disarms per "press any key to stay".
        view.handle_key(press(KeyCode::Down));
        assert_eq!(view.remaining_secs(), None, "disarmed after first key");
        // …and the key still acted (navigation moved).
        assert_eq!(view.states[0].highlighted, 1);
    }

    #[test]
    fn remaining_secs_counts_down_and_expires() {
        let (view, _rx) = exchange(vec![q("Q?", "H", &["A", "B"], false)], Some(60));
        // Simulate 59s elapsed: 1s remaining, not yet expired.
        let almost = view.started + Duration::from_secs(59);
        assert_eq!(view.remaining_secs_at(almost), Some(1));
        assert!(!view.is_expired_at(almost));
        // 60s elapsed: expired.
        let done = view.started + Duration::from_secs(60);
        assert_eq!(view.remaining_secs_at(done), Some(0));
        assert!(view.is_expired_at(done));
    }

    #[test]
    fn auto_submit_never_fabricates_unanswered_questions() {
        let (mut view, rx) = exchange(
            vec![
                q("Q1?", "H1", &["A1", "B1"], false),
                q("Q2?", "H2", &["A2", "B2"], false),
            ],
            Some(60),
        );
        // The afk timeout fires with NOTHING confirmed → no answers are
        // fabricated from the default highlights.
        let outcome = view.auto_submit();
        assert!(matches!(outcome, ViewOutcome::Accepted(_)));
        let answers = rx.blocking_recv().expect("auto-submitted");
        assert!(
            answers.is_empty(),
            "auto_submit must not invent selections the user never confirmed"
        );
    }

    #[test]
    fn auto_submit_resolves_only_confirmed_answers() {
        let (mut view, rx) = exchange(
            vec![
                q("Q1?", "H1", &["A1", "B1"], false),
                q("Q2?", "H2", &["A2", "B2"], false),
            ],
            Some(60),
        );
        // The user explicitly confirms Q1 (Enter), then the afk timeout fires
        // before Q2 is answered.
        view.handle_key(press(KeyCode::Enter));
        let outcome = view.auto_submit();
        assert!(matches!(outcome, ViewOutcome::Accepted(_)));
        let answers = rx.blocking_recv().expect("auto-submitted");
        // Only the confirmed Q1 is present; Q2 is NOT back-filled.
        assert_eq!(answers.get("Q1?").map(String::as_str), Some("A1"));
        assert_eq!(answers.get("Q2?"), None);
    }
}
