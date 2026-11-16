//! The `computer` tool's `request_access` approval widget: a bottom-pane view
//! showing either a TCC missing-permissions panel (Accessibility / Screen
//! Recording not yet granted) or an app-allowlist panel (checkboxes per
//! requested app + the three capability flags actually requested), resolving
//! the user's decision back through the exchange's one-shot channel.
//!
//! This is the live TUI counterpart of
//! `tool_computer_use::access_resolver::TuiBridgeResolver` — the same
//! round-trip [`crate::bottom_pane::ask_user_question_view::AskUserQuestionView`]
//! performs for `AskUserQuestion`. Parity target: claude-code's
//! `ComputerUseApproval.tsx` (`ComputerUseTccPanel` / `ComputerUseAppListPanel`).
//!
//! Interaction:
//! - TCC panel: `↑`/`↓` move the highlight across the 1-3 rows ("Open System
//!   Settings → Accessibility" / "→ Screen Recording", shown only for
//!   whichever permission is actually missing, then always "Try again");
//!   `Enter` on an "Open System Settings" row shells out to `open` and stays
//!   open; `Enter` on "Try again" resolves DENIED (there's no way to
//!   re-check TCC state from here — the model just calls `request_access`
//!   again once the user has actually granted the permission in System
//!   Settings, which re-checks fresh).
//! - App-list panel: `↑`/`↓` move the highlight across app rows, the
//!   requested flag rows, then the submit row; `Space` toggles the
//!   highlighted app/flag checkbox (apps start pre-checked, matching
//!   `ComputerUseAppListPanel`'s `Set(request.apps...)` full-select
//!   default); `Enter` submits the currently-checked set from ANY row.
//!   Background/full-screen preference rows use `Space` to choose a mode;
//!   selecting a preference does not authorize a screen takeover.
//! - Takeover panel: `↑`/`↓` selects Decline or Allow full-screen control,
//!   with Decline initially selected. Only `Enter` resolves the choice.
//! - Teaching step: Continue / End tour resolves a step-only response;
//!   `Esc` ends the tour. Teaching access uses the app panel with an explicit
//!   teaching approval action and the selected interaction preference.
//! - `Esc` cancels from either panel: `resp_tx` is dropped unsent, which
//!   [`tool_computer_use::access_resolver::TuiBridgeResolver`] maps to the
//!   fully-denied default (the same mapping a dropped `AskUserQuestion`
//!   channel gets).

use std::any::Any;

use crossterm::cursor::SetCursorStyle;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use permission::computer_access::{
    ComputerAccessExchange, ComputerAccessRequest, ComputerAccessResponse, ComputerInteraction,
    TccState,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tokio::sync::oneshot;

use crate::bottom_pane::dialog_view::centered_rect;
use crate::bottom_pane::view::{BottomPaneView, ViewAction, ViewOutcome};
use crate::renderable::Renderable;

/// Which approval panel is showing. Fixed for the view's lifetime — the
/// exchange already carries `tcc_state` at construction, and there is no
/// live re-check (see the module doc's "Try again" note).
enum Panel {
    Interaction {
        teach_step: bool,
        highlighted: usize,
    },
    Tcc {
        state: TccState,
        /// Highlighted row: rows are (missing-accessibility?)
        /// (missing-screen-recording?) then always "Try again", in that
        /// order, skipping any already-granted permission.
        highlighted: usize,
    },
    AppList {
        /// One checkbox per `request.apps`, parallel, all pre-checked.
        checked: Vec<bool>,
        clipboard_read_checked: bool,
        clipboard_write_checked: bool,
        system_key_combos_checked: bool,
        background_mode: bool,
        highlighted: usize,
    },
}

/// The live `request_access` approval widget.
pub struct ComputerAccessView {
    request: ComputerAccessRequest,
    panel: Panel,
    resp_tx: Option<oneshot::Sender<ComputerAccessResponse>>,
}

impl ComputerAccessView {
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

    /// Build the widget for `exchange` (the request + the answer channel).
    #[must_use]
    pub fn new(exchange: ComputerAccessExchange) -> Self {
        let ComputerAccessExchange { request, resp_tx } = exchange;
        let panel = match request.tcc_state {
            Some(state) => Panel::Tcc {
                state,
                highlighted: 0,
            },
            None if matches!(
                request.interaction,
                Some(ComputerInteraction::Takeover { .. })
            ) =>
            {
                Panel::Interaction {
                    teach_step: false,
                    highlighted: 0,
                }
            }
            None if matches!(
                request.interaction,
                Some(ComputerInteraction::TeachStep { .. })
            ) =>
            {
                Panel::Interaction {
                    teach_step: true,
                    highlighted: 0,
                }
            }
            None => Panel::AppList {
                checked: vec![true; request.apps.len()],
                clipboard_read_checked: request.clipboard_read,
                clipboard_write_checked: request.clipboard_write,
                system_key_combos_checked: request.system_key_combos,
                background_mode: true,
                highlighted: 0,
            },
        };
        Self {
            request,
            panel,
            resp_tx: Some(resp_tx),
        }
    }

    /// The TCC panel's rows, in display order: (label, is_open_settings_url).
    fn tcc_rows(state: &TccState) -> Vec<(&'static str, Option<&'static str>)> {
        let mut rows = Vec::with_capacity(3);
        if !state.accessibility {
            rows.push((
                "Open System Settings \u{2192} Accessibility",
                Some(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                ),
            ));
        }
        if !state.screen_recording {
            rows.push((
                "Open System Settings \u{2192} Screen Recording",
                Some(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
                ),
            ));
        }
        rows.push(("Try again", None));
        rows
    }

    /// Resolve DENIED (Esc, or TCC "Try again") — `resp_tx` dropped/sent with
    /// the all-empty default exactly once.
    fn deny(&mut self) -> ViewOutcome {
        if let Some(tx) = self.resp_tx.take() {
            let response = if matches!(
                self.panel,
                Panel::Interaction {
                    teach_step: true,
                    ..
                }
            ) {
                ComputerAccessResponse {
                    interaction_action: Some("exit".into()),
                    ..Default::default()
                }
            } else {
                ComputerAccessResponse::default()
            };
            let _ = tx.send(response);
        }
        ViewOutcome::Cancelled
    }

    /// Submit the app-list panel's current selection.
    fn submit(&mut self) -> ViewOutcome {
        let Panel::AppList {
            checked,
            clipboard_read_checked,
            clipboard_write_checked,
            system_key_combos_checked,
            background_mode,
            ..
        } = &self.panel
        else {
            return ViewOutcome::Pending;
        };
        let granted_apps: Vec<String> = self
            .request
            .apps
            .iter()
            .zip(checked)
            .filter_map(|(app, on)| on.then(|| app.label.clone()))
            .collect();
        let response = ComputerAccessResponse {
            granted_apps,
            clipboard_read: *clipboard_read_checked,
            clipboard_write: *clipboard_write_checked,
            system_key_combos: *system_key_combos_checked,
            interaction_action: matches!(
                self.request.interaction,
                Some(ComputerInteraction::TeachAccess { .. })
            )
            .then(|| "allow".into()),
            preferred_mode: Some(
                if *background_mode {
                    "background"
                } else {
                    "full_control"
                }
                .into(),
            ),
        };
        if let Some(tx) = self.resp_tx.take() {
            let _ = tx.send(response);
        }
        ViewOutcome::Accepted(ViewAction::Selected(0))
    }

    fn submit_interaction(&mut self, action: &'static str) -> ViewOutcome {
        let response = ComputerAccessResponse {
            interaction_action: Some(action.into()),
            ..Default::default()
        };
        if !self.request.allows_response(&response) {
            return self.deny();
        }
        if let Some(tx) = self.resp_tx.take() {
            let _ = tx.send(response);
        }
        if action == "exit" {
            ViewOutcome::Cancelled
        } else {
            ViewOutcome::Accepted(ViewAction::Selected(0))
        }
    }

    fn interaction_copy(&self) -> Vec<Line<'static>> {
        let Some(interaction) = &self.request.interaction else {
            return Vec::new();
        };
        let (reason, explanation, next_preview) = match interaction {
            ComputerInteraction::Takeover {
                reason,
                explanation,
                next_preview,
                ..
            }
            | ComputerInteraction::TeachAccess {
                reason,
                explanation,
                next_preview,
                ..
            }
            | ComputerInteraction::TeachStep {
                reason,
                explanation,
                next_preview,
                ..
            } => (reason, explanation, next_preview),
        };
        let mut lines = Vec::new();
        for text in [
            reason.as_deref().unwrap_or(&self.request.reason),
            explanation.as_deref().unwrap_or(""),
        ] {
            lines.extend(text.lines().map(|line| Line::from(line.to_owned())));
        }
        if let Some(preview) = next_preview {
            lines.push(Line::from(format!("Next step: {preview}")));
        }
        lines
    }

    /// The app-list panel's flag rows actually requested, in fixed order
    /// (clipboardRead, clipboardWrite, systemKeyCombos) — matching
    /// `ComputerUseAppListPanel`'s `ALL_FLAG_KEYS.filter(...)`.
    fn requested_flag_labels(&self) -> Vec<&'static str> {
        let mut labels = Vec::with_capacity(3);
        if self.request.clipboard_read {
            labels.push("Also allow reading the clipboard");
        }
        if self.request.clipboard_write {
            labels.push("Also allow writing the clipboard");
        }
        if self.request.system_key_combos {
            labels.push("Also allow system-level key combos (quit, switch app, lock screen)");
        }
        labels
    }

    /// One row per app/requested flag, two interaction preferences, and submit.
    fn app_list_row_count(&self) -> usize {
        self.request.apps.len() + self.requested_flag_labels().len() + 3
    }
}

impl Renderable for ComputerAccessView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines: Vec<Line<'static>> = Vec::new();
        match &self.panel {
            Panel::Interaction {
                teach_step,
                highlighted,
            } => {
                lines.push(Line::from(Span::styled(
                    if *teach_step {
                        "Guided tour step"
                    } else {
                        "Allow full-screen control?"
                    },
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                lines.extend(self.interaction_copy());
                if !*teach_step {
                    lines.push(Line::from(
                        "This temporarily controls the whole desktop, beyond selected apps.",
                    ));
                }
                lines.push(Line::from(""));
                let labels = if *teach_step {
                    ["Continue", "End tour"]
                } else {
                    ["Decline", "Allow full-screen control"]
                };
                for (row, label) in labels.iter().enumerate() {
                    let focused = row == *highlighted;
                    let style = if focused {
                        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(
                        format!("{} {label}", if focused { "\u{203a}" } else { " " }),
                        style,
                    )));
                }
                lines.push(Line::from(if *teach_step {
                    "Esc ends the tour"
                } else {
                    "Esc declines"
                }));
            }
            Panel::Tcc { state, highlighted } => {
                lines.push(Line::from(Span::styled(
                    "Computer Use needs macOS permissions",
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "Accessibility: {}",
                    if state.accessibility {
                        "granted"
                    } else {
                        "not granted"
                    }
                )));
                lines.push(Line::from(format!(
                    "Screen Recording: {}",
                    if state.screen_recording {
                        "granted"
                    } else {
                        "not granted"
                    }
                )));
                lines.push(Line::from(""));
                for (i, (label, _)) in Self::tcc_rows(state).iter().enumerate() {
                    let focused = i == *highlighted;
                    let marker = if focused { "\u{203a}" } else { " " };
                    let style = if focused {
                        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(format!("{marker} {label}"), style)));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Grant the missing permissions in System Settings, then select \"Try again\".",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            Panel::AppList {
                checked,
                clipboard_read_checked,
                clipboard_write_checked,
                system_key_combos_checked,
                background_mode,
                highlighted,
            } => {
                lines.push(Line::from(Span::styled(
                    if matches!(
                        self.request.interaction,
                        Some(ComputerInteraction::TeachAccess { .. })
                    ) {
                        "Guided tour application access"
                    } else {
                        "Computer Use"
                    },
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                if matches!(
                    self.request.interaction,
                    Some(ComputerInteraction::TeachAccess { .. })
                ) {
                    lines.extend(self.interaction_copy());
                } else {
                    lines.push(Line::from(self.request.reason.clone()));
                }
                lines.push(Line::from(format!(
                    "Requested tier: {} ({})",
                    self.request.tier.as_str(),
                    self.request.tier.description()
                )));
                lines.push(Line::from(""));
                let mut row = 0usize;
                for (app, on) in self.request.apps.iter().zip(checked) {
                    let focused = row == *highlighted;
                    let marker = if focused { "\u{203a}" } else { " " };
                    let checkbox = if *on { "[x] " } else { "[ ] " };
                    let style = if focused {
                        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(
                        format!("{marker} {checkbox}{}", app.label),
                        style,
                    )));
                    row += 1;
                }
                let flag_states = [
                    (self.request.clipboard_read, *clipboard_read_checked),
                    (self.request.clipboard_write, *clipboard_write_checked),
                    (self.request.system_key_combos, *system_key_combos_checked),
                ]
                .into_iter()
                .filter_map(|(requested, checked)| requested.then_some(checked));
                for (label, on) in self.requested_flag_labels().into_iter().zip(flag_states) {
                    let focused = row == *highlighted;
                    let marker = if focused { "\u{203a}" } else { " " };
                    let checkbox = if on { "[x] " } else { "[ ] " };
                    let style = if focused {
                        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(
                        format!("{marker} {checkbox}{label}"),
                        style,
                    )));
                    row += 1;
                }
                for (label, selected) in [
                    ("Background", *background_mode),
                    ("Full-screen", !*background_mode),
                ] {
                    let focused = row == *highlighted;
                    let style = if focused {
                        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(
                        format!(
                            "{} ({}) {label}",
                            if focused { "\u{203a}" } else { " " },
                            if selected { "x" } else { " " }
                        ),
                        style,
                    )));
                    row += 1;
                }
                lines.push(Line::from(
                    "Mode is a preference; takeover still needs separate confirmation.",
                ));
                let allowed_count = checked.iter().filter(|c| **c).count();
                let submit_label = format!(
                    "{} ({allowed_count} app{})",
                    if matches!(
                        self.request.interaction,
                        Some(ComputerInteraction::TeachAccess { .. })
                    ) {
                        "Allow and start tour"
                    } else {
                        "Allow for this session"
                    },
                    if allowed_count == 1 { "" } else { "s" }
                );
                let focused = row == *highlighted;
                let marker = if focused { "\u{203a}" } else { " " };
                let style = if focused {
                    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                };
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("{marker} {submit_label}"),
                    style,
                )));
                lines.push(Line::from(Span::styled(
                    "Space chooses apps/options; Enter allows; Esc denies all",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
        }

        let content_w = lines
            .iter()
            .map(ratatui::text::Line::width)
            .max()
            .unwrap_or(20);
        let width = u16::try_from(content_w + 4)
            .unwrap_or(u16::MAX)
            .min(area.width.saturating_sub(4))
            .max(20);
        let height = u16::try_from(lines.len() + 2)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let rect = centered_rect(width, height, area);

        Clear.render(rect, buf);
        let block = Block::new().borders(Borders::ALL).title("Computer Use");
        let inner = block.inner(rect);
        block.render(rect, buf);
        Paragraph::new(lines).render(inner, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        let rows = match &self.panel {
            Panel::Interaction { teach_step, .. } => {
                self.interaction_copy().len() + if *teach_step { 5 } else { 6 }
            }
            Panel::Tcc { state, .. } => 5 + Self::tcc_rows(state).len() + 2,
            Panel::AppList { .. } => {
                4 + self.app_list_row_count() + 3 + self.interaction_copy().len().saturating_sub(1)
            }
        };
        u16::try_from(rows + 2).unwrap_or(u16::MAX)
    }

    fn cursor_pos(&self, _area: Rect) -> Option<(u16, u16)> {
        None
    }

    fn cursor_style(&self, _area: Rect) -> SetCursorStyle {
        SetCursorStyle::DefaultUserShape
    }
}

impl BottomPaneView for ComputerAccessView {
    fn handle_key(&mut self, key: KeyEvent) -> ViewOutcome {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            // Ctrl chords stay swallowed by the modal (permission-view parity).
            return ViewOutcome::Pending;
        }
        // Pre-compute everything derived from `self.request` (immutable)
        // BEFORE matching `&mut self.panel` below — the borrow checker can't
        // see that these helper calls don't touch `self.panel`.
        let app_count = self.request.apps.len();
        // Which of the 3 flag slots (clipboardRead, clipboardWrite,
        // systemKeyCombos) were actually requested, in fixed order —
        // mirrors `requested_flag_labels`'s filter, but as slot indices so
        // the Space-toggle below can map a flag ROW back to which field to
        // flip.
        let requested_slots: Vec<usize> = [
            self.request.clipboard_read,
            self.request.clipboard_write,
            self.request.system_key_combos,
        ]
        .iter()
        .enumerate()
        .filter_map(|(slot, present)| present.then_some(slot))
        .collect();

        match key.code {
            KeyCode::Esc => return self.deny(),
            KeyCode::Up | KeyCode::BackTab => {
                match &mut self.panel {
                    Panel::Interaction { highlighted, .. } => {
                        *highlighted = highlighted.saturating_sub(1);
                    }
                    Panel::Tcc { highlighted, .. } => {
                        *highlighted = highlighted.saturating_sub(1);
                    }
                    Panel::AppList { highlighted, .. } => {
                        *highlighted = highlighted.saturating_sub(1);
                    }
                }
                return ViewOutcome::Pending;
            }
            KeyCode::Down | KeyCode::Tab => {
                match &mut self.panel {
                    Panel::Interaction { highlighted, .. } => {
                        *highlighted = (*highlighted + 1).min(1);
                    }
                    Panel::Tcc { state, highlighted } => {
                        let last = Self::tcc_rows(state).len() - 1;
                        if *highlighted < last {
                            *highlighted += 1;
                        }
                    }
                    Panel::AppList { highlighted, .. } => {
                        let last = app_count + requested_slots.len() + 2; // mode rows, then submit
                        if *highlighted < last {
                            *highlighted += 1;
                        }
                    }
                }
                return ViewOutcome::Pending;
            }
            _ => {}
        }

        // Two-phase: decide what to do WHILE borrowing `self.panel`, then act
        // on it after the borrow ends — `self.deny()`/`self.submit()` need
        // `&mut self` as a whole, which the borrow checker can't reconcile
        // with an in-progress `match &mut self.panel`.
        enum Action {
            None,
            Deny,
            Submit,
            Interaction(&'static str),
            OpenUrl(&'static str),
        }
        let action = match &mut self.panel {
            Panel::Interaction {
                teach_step,
                highlighted,
            } => {
                if key.code != KeyCode::Enter {
                    Action::None
                } else if *teach_step {
                    Action::Interaction(if *highlighted == 0 {
                        "continue"
                    } else {
                        "exit"
                    })
                } else if *highlighted == 0 {
                    Action::Deny
                } else {
                    Action::Interaction("allow")
                }
            }
            Panel::Tcc { state, highlighted } => {
                let rows = Self::tcc_rows(state);
                if key.code == KeyCode::Enter {
                    match rows[*highlighted].1 {
                        Some(url) => Action::OpenUrl(url),
                        // "Try again" — no way to re-check TCC state from
                        // here; the model re-calls request_access once the
                        // user has actually granted the permission.
                        None => Action::Deny,
                    }
                } else {
                    Action::None
                }
            }
            Panel::AppList {
                checked,
                clipboard_read_checked,
                clipboard_write_checked,
                system_key_combos_checked,
                background_mode,
                highlighted,
            } => match key.code {
                KeyCode::Char(' ') => {
                    if *highlighted < app_count {
                        checked[*highlighted] = !checked[*highlighted];
                    } else if let Some(&slot) = requested_slots.get(*highlighted - app_count) {
                        match slot {
                            0 => *clipboard_read_checked = !*clipboard_read_checked,
                            1 => *clipboard_write_checked = !*clipboard_write_checked,
                            _ => *system_key_combos_checked = !*system_key_combos_checked,
                        }
                    } else if *highlighted == app_count + requested_slots.len() {
                        *background_mode = true;
                    } else if *highlighted == app_count + requested_slots.len() + 1 {
                        *background_mode = false;
                    }
                    Action::None
                }
                KeyCode::Enter => Action::Submit,
                _ => Action::None,
            },
        };
        match action {
            Action::None => ViewOutcome::Pending,
            Action::Deny => self.deny(),
            Action::Submit => self.submit(),
            Action::Interaction(action) => self.submit_interaction(action),
            Action::OpenUrl(url) => {
                let _ = std::process::Command::new("open").arg(url).status();
                ViewOutcome::Pending
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use permission::computer_access::{AccessTier, RequestedApp};
    use ratatui::layout::Position;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app_list_request(apps: &[&str], flags: (bool, bool, bool)) -> ComputerAccessRequest {
        ComputerAccessRequest {
            reason: "automate chat".into(),
            apps: apps
                .iter()
                .map(|l| RequestedApp {
                    label: (*l).to_string(),
                })
                .collect(),
            tier: AccessTier::Full,
            clipboard_read: flags.0,
            clipboard_write: flags.1,
            system_key_combos: flags.2,
            tcc_state: None,
            interaction: None,
        }
    }

    fn exchange(
        request: ComputerAccessRequest,
    ) -> (
        ComputerAccessView,
        oneshot::Receiver<ComputerAccessResponse>,
    ) {
        let (resp_tx, resp_rx) = oneshot::channel();
        (
            ComputerAccessView::new(ComputerAccessExchange { request, resp_tx }),
            resp_rx,
        )
    }

    fn buffer_text(view: &ComputerAccessView, area: Rect) -> String {
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

    #[test]
    fn app_list_renders_apps_pre_checked() {
        let (view, _rx) = exchange(app_list_request(
            &["Slack", "Chrome"],
            (false, false, false),
        ));
        let text = buffer_text(&view, Rect::new(0, 0, 60, view.desired_height(60)));
        assert!(text.contains("[x] Slack"), "{text}");
        assert!(text.contains("[x] Chrome"), "{text}");
        assert!(text.contains("Allow for this session (2 apps)"), "{text}");
    }

    #[test]
    fn unchecking_an_app_updates_the_submit_count_and_result() {
        let (mut view, rx) = exchange(app_list_request(
            &["Slack", "Chrome"],
            (false, false, false),
        ));
        view.handle_key(press(KeyCode::Char(' '))); // uncheck Slack (row 0)
        let text = buffer_text(&view, Rect::new(0, 0, 60, view.desired_height(60)));
        assert!(text.contains("Allow for this session (1 app)"), "{text}");
        view.handle_key(press(KeyCode::Down)); // Chrome
        view.handle_key(press(KeyCode::Down)); // submit row
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Accepted(_)
        ));
        let response = rx.blocking_recv().expect("submitted");
        assert_eq!(response.granted_apps, vec!["Chrome".to_string()]);
    }

    #[test]
    fn only_requested_flags_render_and_toggle() {
        let (mut view, rx) = exchange(app_list_request(&["Slack"], (true, false, true)));
        let text = buffer_text(&view, Rect::new(0, 0, 80, view.desired_height(80)));
        assert!(text.contains("reading the clipboard"), "{text}");
        assert!(!text.contains("writing the clipboard"), "{text}");
        assert!(text.contains("system-level key combos"), "{text}");
        // Row 0 = Slack, row 1 = clipboardRead flag (pre-checked), row 2 =
        // systemKeyCombos flag (pre-checked), row 3 = submit.
        view.handle_key(press(KeyCode::Down)); // clipboardRead flag
        view.handle_key(press(KeyCode::Char(' '))); // uncheck it
        view.handle_key(press(KeyCode::Down)); // systemKeyCombos flag
        view.handle_key(press(KeyCode::Down)); // submit
        view.handle_key(press(KeyCode::Enter));
        let response = rx.blocking_recv().expect("submitted");
        assert!(!response.clipboard_read, "unchecked before submit");
        assert!(response.system_key_combos, "left checked");
    }

    #[test]
    fn esc_denies_and_drops_the_channel() {
        let (mut view, rx) = exchange(app_list_request(&["Slack"], (false, false, false)));
        assert!(matches!(
            view.handle_key(press(KeyCode::Esc)),
            ViewOutcome::Cancelled
        ));
        let response = rx.blocking_recv().expect("resolved on Esc");
        assert!(response.granted_apps.is_empty());
    }

    #[test]
    fn tcc_panel_shows_only_missing_permissions() {
        let request = ComputerAccessRequest {
            reason: "test".into(),
            apps: vec![],
            tier: AccessTier::Full,
            clipboard_read: false,
            clipboard_write: false,
            system_key_combos: false,
            tcc_state: Some(TccState {
                accessibility: true,
                screen_recording: false,
            }),
            interaction: None,
        };
        let (view, _rx) = exchange(request);
        let text = buffer_text(&view, Rect::new(0, 0, 80, view.desired_height(80)));
        assert!(!text.contains("Accessibility \u{2192}"), "{text}");
        assert!(text.contains("Screen Recording"), "{text}");
        assert!(text.contains("Try again"), "{text}");
    }

    #[test]
    fn tcc_try_again_denies() {
        let request = ComputerAccessRequest {
            reason: "test".into(),
            apps: vec![],
            tier: AccessTier::Full,
            clipboard_read: false,
            clipboard_write: false,
            system_key_combos: false,
            tcc_state: Some(TccState {
                accessibility: false,
                screen_recording: false,
            }),
            interaction: None,
        };
        let (mut view, rx) = exchange(request);
        // Rows: [Accessibility, ScreenRecording, Try again] — move to row 2.
        view.handle_key(press(KeyCode::Down));
        view.handle_key(press(KeyCode::Down));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Cancelled
        ));
        let response = rx.blocking_recv().expect("resolved");
        assert!(response.granted_apps.is_empty());
    }

    #[test]
    fn ctrl_chords_are_swallowed() {
        let (mut view, _rx) = exchange(app_list_request(&["Slack"], (false, false, false)));
        assert!(matches!(
            view.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            ViewOutcome::Pending
        ));
    }

    fn control_interaction(step: bool) -> ComputerAccessRequest {
        let mut request = app_list_request(&["Slack"], (true, true, true));
        let reason = Some("Review the guided task".into());
        let explanation = Some("Confirm the current step".into());
        let next_preview = Some("Open the next panel".into());
        request.interaction = Some(if step {
            ComputerInteraction::TeachStep {
                reason,
                explanation,
                next_preview,
                anchor: None,
            }
        } else {
            ComputerInteraction::Takeover {
                reason,
                explanation,
                next_preview,
                anchor: None,
            }
        });
        request
    }

    #[test]
    fn takeover_defaults_to_decline_and_explicit_allow_carries_no_grants() {
        let (mut view, rx) = exchange(control_interaction(false));
        let text = buffer_text(&view, Rect::new(0, 0, 100, view.desired_height(100)));
        assert!(text.contains("Allow full-screen control?"), "{text}");
        assert!(text.contains("Confirm the current step"), "{text}");
        assert!(text.contains("Next step: Open the next panel"), "{text}");
        assert!(text.contains("\u{203a} Decline"), "{text}");
        assert!(
            !text.contains("[x] Slack") && !text.contains("clipboard"),
            "{text}"
        );
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Cancelled
        ));
        assert!(rx.blocking_recv().unwrap().interaction_action.is_none());
        let (mut view, rx) = exchange(control_interaction(false));
        view.handle_key(press(KeyCode::Down));
        assert!(matches!(
            view.handle_key(press(KeyCode::Enter)),
            ViewOutcome::Accepted(_)
        ));
        let response = rx.blocking_recv().unwrap();
        assert_eq!(response.interaction_action.as_deref(), Some("allow"));
        assert!(
            response.granted_apps.is_empty()
                && !response.clipboard_read
                && !response.clipboard_write
                && !response.system_key_combos
        );
        assert!(response.preferred_mode.is_none());
    }

    #[test]
    fn teaching_step_supports_continue_end_tour_and_escape_without_grants() {
        for choice in ["continue", "exit", "escape"] {
            let (mut view, rx) = exchange(control_interaction(true));
            let text = buffer_text(&view, Rect::new(0, 0, 100, view.desired_height(100)));
            assert!(
                text.contains("Guided tour step")
                    && text.contains("Continue")
                    && text.contains("End tour"),
                "{text}"
            );
            if choice == "exit" {
                view.handle_key(press(KeyCode::Down));
            }
            view.handle_key(press(if choice == "escape" {
                KeyCode::Esc
            } else {
                KeyCode::Enter
            }));
            let response = rx.blocking_recv().unwrap();
            assert_eq!(
                response.interaction_action.as_deref(),
                Some(if choice == "continue" {
                    "continue"
                } else {
                    "exit"
                })
            );
            assert!(
                response.granted_apps.is_empty()
                    && !response.clipboard_read
                    && !response.clipboard_write
                    && !response.system_key_combos
            );
            assert!(response.preferred_mode.is_none());
        }
    }

    #[test]
    fn teaching_access_explicitly_allows_selected_apps_and_preferred_mode() {
        let mut request = app_list_request(&["Slack", "Notes"], (true, false, false));
        request.interaction = Some(ComputerInteraction::TeachAccess {
            reason: None,
            explanation: Some("Review your application choices".into()),
            next_preview: Some("Start the tour".into()),
            anchor: None,
        });
        let (mut view, rx) = exchange(request.clone());
        let text = buffer_text(&view, Rect::new(0, 0, 100, view.desired_height(100)));
        assert!(
            text.contains("Guided tour application access")
                && text.contains("Allow and start tour"),
            "{text}"
        );
        assert!(
            text.contains("Review your application choices")
                && text.contains("Next step: Start the tour"),
            "{text}"
        );
        for _ in 0..4 {
            view.handle_key(press(KeyCode::Down));
        }
        view.handle_key(press(KeyCode::Char(' '))); // Full-screen mode radio.
        view.handle_key(press(KeyCode::Enter));
        let response = rx.blocking_recv().unwrap();
        assert_eq!(response.interaction_action.as_deref(), Some("allow"));
        assert_eq!(response.preferred_mode.as_deref(), Some("full_control"));
        assert_eq!(response.granted_apps, vec!["Slack", "Notes"]);
        assert!(request.allows_response(&response));
    }

    #[test]
    fn ordinary_app_approval_defaults_to_background_preference() {
        let (mut view, rx) = exchange(app_list_request(&["Notes"], (false, false, false)));
        view.handle_key(press(KeyCode::Enter));
        let response = rx.blocking_recv().unwrap();
        assert_eq!(response.preferred_mode.as_deref(), Some("background"));
        assert!(response.interaction_action.is_none());
    }
}
