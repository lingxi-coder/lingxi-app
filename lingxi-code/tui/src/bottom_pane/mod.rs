//! The bottom pane of the chat UI: the composer plus every transient surface
//! layered over it (plan Phase 5).
//!
//! Modeled on codex-rs `tui/src/bottom_pane/mod.rs`: [`BottomPane`] owns the
//! composer (editable prompt input), the command/file completion popup, the
//! vim editing layer, the status hint row (including the armed-Ctrl-C visual
//! hint), the queued-input preview, and a stack of keyboard-owning
//! [`BottomPaneView`]s (permission prompt, model picker, read-only screens)
//! that temporarily take input away from the composer.
//!
//! Input routing is layered — active view first, then the completion popup,
//! then vim, then the composer — while higher-level intent (interrupt/quit
//! policy, turn submission, slash dispatch, model switching) is decided by
//! the owner acting on the returned [`BottomPaneOutcome`]. The pane never
//! cancels a turn or exits the process by itself.

pub mod completion_view;
pub mod dialog_view;
pub mod footer;
pub mod model_picker_view;
pub mod pending_input_preview;
pub mod permission_view;
pub mod screen_view;
pub mod theme_picker_view;
pub mod view;
pub mod web_config_view;
pub mod web_picker_view;

use std::time::{Duration, Instant};

use crossterm::cursor::SetCursorStyle;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use tui_core::permission_bridge::PermissionExchange;
use tui_core::theme::Theme;

use crate::bottom_pane::completion_view::{command_items, CompletionView};
use crate::bottom_pane::model_picker_view::ModelPickerView;
use crate::bottom_pane::pending_input_preview::PendingInputPreview;
use crate::bottom_pane::permission_view::PermissionView;
use crate::composer::{Composer, ComposerView, MAX_VISIBLE_LINES};
use crate::renderable::Renderable;
use crate::session::ModelRow;
use crate::vim::{VimOutcome, VimState};
pub use view::{BottomPaneView, CommandAction, ViewAction, ViewOutcome, WebAction};

/// How long an idle Ctrl-C stays "armed" before a second press quits.
const CTRL_C_EXIT_WINDOW: Duration = Duration::from_secs(2);

/// The owner-computed task status the pane renders while a turn is running.
/// The pane holds NO turn state itself (`current_turn`/`turn_started_at`/
/// `activity` stay with the owner); this input struct carries the display
/// result plus the running flag the pane needs for Ctrl-C routing.
#[derive(Debug, Clone, Default)]
pub struct BottomPaneStatus {
    /// Whether a turn is in flight (drives the spinner row and routes Ctrl-C
    /// to [`BottomPaneOutcome::Interrupt`] instead of the arm-to-quit chord).
    pub running: bool,
    /// The spinner/status text shown while running (owner-computed from its
    /// turn state: activity label, elapsed seconds, animation frame).
    pub text: String,
    /// Session-cumulative cost (`$0.0000` — `TurnEvent::CostUpdated`),
    /// appended dim at the end of the status row when known.
    pub cost: Option<String>,
}

/// What the owner must do after the pane routed one key or paste. Local
/// editing/navigation is fully consumed inside the pane; these variants carry
/// only the app-level intents the pane is not allowed to decide itself.
#[derive(Debug)]
pub enum BottomPaneOutcome {
    /// The pane consumed the input; nothing for the owner to do.
    Consumed,
    /// The composer submitted this (trimmed) buffer text. Slash-command
    /// routing versus prompt submission is the owner's decision.
    Submitted(String),
    /// A stacked view asks the owner to submit `String` as a user prompt.
    SubmitPrompt(String),
    /// The user picked a model — the exact `(request_model, profile)` args
    /// `OrchestratorHandle::switch_model` accepts.
    SwitchModel {
        /// The wire model id to switch to.
        request_model: String,
        /// The provider profile the model routes through, when qualified.
        profile: Option<String>,
    },
    /// A view asks the owner to run a command effect on its behalf.
    RunCommand(CommandAction),
    /// A view asks the owner to run a `/web` effect on its behalf (the view
    /// stays open; see [`ViewOutcome::RunWebAction`]).
    RunWebAction(WebAction),
    /// Ctrl-O: the owner should toggle transcript verbose mode (and reflect
    /// the new state back via [`BottomPane::set_verbose`]).
    ToggleVerbose,
    /// Ctrl-C or Esc while a task is running: the owner should cancel the
    /// turn (the interrupt-before-quit half of the layered routing policy —
    /// acceptance criterion 14).
    Interrupt,
    /// The user asked to quit (idle Esc with no local surface to consume it,
    /// or a second idle Ctrl-C inside the arm window). The owner's quit
    /// policy applies.
    Quit,
    /// A pasted path to an existing image file: the owner should record it as
    /// an image message (the composer text is untouched).
    PastedImage(String),
}

/// The interactive footer of the chat UI: composer + completion + vim +
/// status hints + queued-input preview + the transient view stack.
pub struct BottomPane {
    /// The multi-line input buffer. Retained even while a view is displayed
    /// so input state survives the view closing.
    composer: Composer,
    /// Command/file completion popup shown while a `/command` or `@file`
    /// token is being typed. NOT a stacked view — it coexists with the
    /// composer (typing keeps filtering).
    completion: Option<CompletionView>,
    /// Vim editing state when `/vim` is enabled (`None` → plain editor).
    vim: Option<VimState>,
    /// First unconfirmed idle Ctrl-C, for claude-code's press-twice-to-exit.
    /// Pane-local: it drives the "Press Ctrl-C again to exit" status hint; a
    /// second press within [`CTRL_C_EXIT_WINDOW`] surfaces
    /// [`BottomPaneOutcome::Quit`].
    ctrl_c_at: Option<Instant>,
    /// Transient keyboard-owning views stacked over the composer.
    view_stack: ViewStack,
    /// Owner-fed task status (spinner text + running flag).
    status: BottomPaneStatus,
    /// Mirror of the transcript's verbose mode, for the status hint text.
    verbose: bool,
    /// Preview of queued inputs (empty seam until a queue source exists).
    pending_input_preview: PendingInputPreview,
    /// Owner-fed context-pressure banner (`TurnEvent::ContextPressure` — the
    /// claude-code `<TokenWarning>` line). Rendered as its own row between the
    /// status row and the composer; `None` renders nothing.
    context_pressure: Option<traits::ContextPressureBanner>,
    /// Theme for status-row styling.
    theme: Theme,
    /// Session accent color (`/color`): tints the composer's `›` gutter
    /// prompt when set. `None` → the theme default (no tint).
    accent: Option<tui_core::render::StyleColor>,
}

impl BottomPane {
    /// An idle pane with an empty composer.
    #[must_use]
    pub fn new(theme: Theme) -> Self {
        Self {
            composer: Composer::default(),
            completion: None,
            vim: None,
            ctrl_c_at: None,
            view_stack: ViewStack::new(),
            status: BottomPaneStatus::default(),
            verbose: false,
            pending_input_preview: PendingInputPreview::new(),
            context_pressure: None,
            theme,
            accent: None,
        }
    }

    /// Route one key press. Layered: the active view owns the keyboard until
    /// it resolves, then the completion popup, then vim, then the composer.
    pub fn handle_key(&mut self, key: KeyEvent) -> BottomPaneOutcome {
        if let Some(outcome) = self.view_stack.route_key(key) {
            return Self::map_view_outcome(outcome);
        }
        if self.completion.is_some() {
            if let Some(outcome) = self.on_completion_key(key.code) {
                return outcome;
            }
        }
        // When vim is enabled, the vim layer sees the key first. It fully
        // handles Normal-mode motions/edits; Insert-mode typing + all Ctrl
        // chords fall through to the normal composer handling below.
        if self.vim.is_some() {
            let vim_outcome = {
                let vim = self.vim.as_mut().expect("vim is Some");
                crate::vim::handle_key(vim, &mut self.composer, key)
            };
            match vim_outcome {
                VimOutcome::Consumed => {
                    self.sync_completion();
                    return BottomPaneOutcome::Consumed;
                }
                VimOutcome::Submit => {
                    let outcome = self
                        .take_submission_state()
                        .map_or(BottomPaneOutcome::Consumed, BottomPaneOutcome::Submitted);
                    self.sync_completion();
                    return outcome;
                }
                VimOutcome::Passthrough => {}
            }
        }
        let outcome = self.on_composer_key(key);
        self.sync_completion();
        outcome
    }

    /// Route a bracketed paste. An active view owns the paste stream (modal
    /// views swallow it by default). Otherwise an image file path (existing
    /// `.png`/`.jpg`/…) surfaces as [`BottomPaneOutcome::PastedImage`];
    /// anything else is inserted into the composer at the cursor.
    pub fn handle_paste(&mut self, text: &str) -> BottomPaneOutcome {
        if let Some(outcome) = self.view_stack.route_paste(text) {
            return Self::map_view_outcome(outcome);
        }
        let trimmed = text.trim();
        if is_image_path(trimmed) {
            return BottomPaneOutcome::PastedImage(trimmed.to_string());
        }
        self.composer.insert_str(text);
        self.sync_completion();
        BottomPaneOutcome::Consumed
    }

    /// Push a transient view; it becomes the active (keyboard-owning) view.
    pub fn show_view(&mut self, view: Box<dyn BottomPaneView>) {
        self.view_stack.push(view);
    }

    /// Open a permission prompt for `exchange`: it owns the keyboard until
    /// the user resolves it and delivers the response through the exchange's
    /// one-shot channel exactly once.
    pub fn show_permission(&mut self, exchange: PermissionExchange) {
        self.view_stack
            .push(Box::new(PermissionView::new(exchange)));
    }

    /// Open the model picker over `rows` (an empty list renders the picker's
    /// own empty-state message — plan Phase 11 step 5).
    pub fn show_model_picker(&mut self, rows: Vec<ModelRow>) {
        self.view_stack.push(Box::new(ModelPickerView::new(rows)));
    }

    /// Open the `/web` provider picker over `snapshot` (the current
    /// configured/active web-search state).
    pub fn show_web_picker(&mut self, snapshot: crate::web::picker::WebConfigSnapshot) {
        self.view_stack
            .push(Box::new(web_picker_view::WebPickerView::new(snapshot)));
    }

    /// Feed the owner-computed task status (spinner text + running flag).
    /// Called before routing/rendering so Ctrl-C routing and the status row
    /// reflect the owner's current turn state.
    pub fn set_task_running(&mut self, status: BottomPaneStatus) {
        self.status = status;
    }

    /// Mirror the transcript's verbose mode for the status hint text.
    pub fn set_verbose(&mut self, enabled: bool) {
        self.verbose = enabled;
    }

    /// Set or clear the owner-fed context-pressure banner
    /// (`TurnEvent::ContextPressure`); `None` removes the banner row.
    pub fn set_context_pressure(&mut self, banner: Option<traits::ContextPressureBanner>) {
        self.context_pressure = banner;
    }

    /// The context-pressure banner currently shown, if any (tests/owner).
    #[must_use]
    pub fn context_pressure(&self) -> Option<&traits::ContextPressureBanner> {
        self.context_pressure.as_ref()
    }

    /// Swap the render theme (`/theme` picker commit).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Set or clear the session accent color (`/color`), tinting the
    /// composer's `›` gutter prompt.
    pub fn set_accent(&mut self, accent: Option<tui_core::render::StyleColor>) {
        self.accent = accent;
    }

    /// The session accent color, when one is set (`/color`).
    #[must_use]
    pub fn accent(&self) -> Option<tui_core::render::StyleColor> {
        self.accent
    }

    /// Whether the composer is empty (ignoring surrounding whitespace).
    #[must_use]
    pub fn composer_is_empty(&self) -> bool {
        self.composer.is_blank()
    }

    /// Take the composer's submission: pushes non-blank text to history,
    /// clears the buffer, and returns the trimmed text. `None` (buffer
    /// untouched) when the composer is blank.
    pub fn take_submission_state(&mut self) -> Option<String> {
        if self.composer.is_blank() {
            return None;
        }
        let text = self.composer.take();
        Some(text.trim().to_string())
    }

    /// Toggle vim editing mode; returns whether vim is now enabled.
    pub fn toggle_vim(&mut self) -> bool {
        let now_on = self.vim.is_none();
        self.vim = if now_on { Some(VimState::new()) } else { None };
        now_on
    }

    /// Whether vim editing mode is enabled.
    #[must_use]
    pub fn vim_enabled(&self) -> bool {
        self.vim.is_some()
    }

    /// Read-only access to the composer (rendering/tests; owners must not
    /// mutate composer internals directly).
    #[must_use]
    pub fn composer(&self) -> &Composer {
        &self.composer
    }

    /// The completion popup, when open.
    #[must_use]
    pub fn completion(&self) -> Option<&CompletionView> {
        self.completion.as_ref()
    }

    /// The transient view stack (queries like "is a permission prompt open?").
    #[must_use]
    pub fn view_stack(&self) -> &ViewStack {
        &self.view_stack
    }

    /// Whether an idle Ctrl-C is currently armed (a second press quits).
    #[must_use]
    pub fn ctrl_c_armed(&self) -> bool {
        self.ctrl_c_at
            .is_some_and(|t| t.elapsed() <= CTRL_C_EXIT_WINDOW)
    }

    /// Replace the queued-input preview contents (the future `ChatWidget`
    /// queue seam; empty in the current data path).
    pub fn set_queued_messages(&mut self, messages: Vec<String>) {
        self.pending_input_preview.set_queued_messages(messages);
    }

    /// Map a completed view's outcome to the pane boundary: view-local
    /// resolutions are consumed here; app-level requests pass through.
    fn map_view_outcome(outcome: ViewOutcome) -> BottomPaneOutcome {
        match outcome {
            // `OpenView` is consumed inside the stack and never surfaces here;
            // the other variants carry no app-level effect (a resolved
            // permission has already answered through its one-shot channel).
            ViewOutcome::Pending
            | ViewOutcome::Cancelled
            | ViewOutcome::Accepted(_)
            | ViewOutcome::PermissionResponse(_)
            | ViewOutcome::OpenView(_) => BottomPaneOutcome::Consumed,
            ViewOutcome::SubmitPrompt(prompt) => BottomPaneOutcome::SubmitPrompt(prompt),
            ViewOutcome::SwitchModel {
                request_model,
                profile,
            } => BottomPaneOutcome::SwitchModel {
                request_model,
                profile,
            },
            ViewOutcome::RunCommand(action) => BottomPaneOutcome::RunCommand(action),
            ViewOutcome::RunWebAction(action) => BottomPaneOutcome::RunWebAction(action),
        }
    }

    /// Handle a key while the completion popup is open. Returns
    /// `Some(outcome)` when the popup consumes the key (nav / complete /
    /// dismiss), or `None` to let it fall through to the composer (so typing
    /// keeps filtering).
    fn on_completion_key(&mut self, code: KeyCode) -> Option<BottomPaneOutcome> {
        match code {
            KeyCode::Up => {
                self.completion.as_mut()?.prev();
                Some(BottomPaneOutcome::Consumed)
            }
            KeyCode::Down => {
                self.completion.as_mut()?.next();
                Some(BottomPaneOutcome::Consumed)
            }
            KeyCode::Tab => {
                let insert = self.completion.as_ref()?.selected_insert().to_string();
                // An `@file` token completes in place; a `/command` replaces
                // the whole buffer.
                if let Some((at, _)) = self.composer.at_fragment() {
                    self.composer.complete_at(at, &insert);
                } else {
                    self.composer.replace_all(&insert);
                }
                self.sync_completion();
                Some(BottomPaneOutcome::Consumed)
            }
            // Enter on the popup COMMITS the highlighted item and CLOSES the
            // overlay (baseline iocraft: "Enter always commits the highlighted
            // row outright, overlay closes") — but ONLY when the buffer is not
            // already that exact command. Without this, picking `/model` from
            // the list while the buffer still held a partial "/" fell through
            // and submitted the bare "/". When the buffer ALREADY equals the
            // highlighted `/command` (the user typed it in full), Enter falls
            // through to the composer so the command actually RUNS — that's the
            // second-Enter-after-accept path too (the overlay is closed by then).
            KeyCode::Enter => {
                let insert = self.completion.as_ref()?.selected_insert().to_string();
                if let Some((at, _)) = self.composer.at_fragment() {
                    // `@file`: commit the highlighted path in place + close.
                    self.composer.complete_at(at, &insert);
                    self.completion = None;
                    Some(BottomPaneOutcome::Consumed)
                } else if self.composer.text() == insert {
                    // Buffer already IS the highlighted command → let Enter run it.
                    None
                } else {
                    // Partial/different `/command` → commit it + close the popup.
                    self.composer.replace_all(&insert);
                    self.completion = None;
                    Some(BottomPaneOutcome::Consumed)
                }
            }
            KeyCode::Esc => {
                self.completion = None;
                Some(BottomPaneOutcome::Consumed)
            }
            _ => None,
        }
    }

    /// Recompute the completion popup from the current composer text: a
    /// `/command` fragment (whole buffer) shows command matches; an `@file`
    /// token at the cursor shows file matches; anything else closes it.
    fn sync_completion(&mut self) {
        let text = self.composer.text();
        let is_command =
            text.starts_with('/') && !text.contains('\n') && !text.contains(char::is_whitespace);
        if is_command {
            self.completion = CompletionView::new(command_items(&text));
            return;
        }
        if let Some((_, fragment)) = self.composer.at_fragment() {
            self.completion = CompletionView::new(crate::files::file_completions(&fragment));
            return;
        }
        self.completion = None;
    }

    /// Composer-level key handling: editing keys are consumed locally; Esc,
    /// Ctrl-C, Ctrl-O, and Enter surface owner-level outcomes.
    fn on_composer_key(&mut self, key: KeyEvent) -> BottomPaneOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let is_ctrl_c = ctrl && matches!(key.code, KeyCode::Char('c'));
        // Any key other than a repeat Ctrl-C disarms the press-twice-to-exit.
        if !is_ctrl_c {
            self.ctrl_c_at = None;
        }
        match key.code {
            // Esc interrupts an in-flight turn (the spinner's "esc to
            // interrupt" hint); idle it quits. Local surfaces (active view,
            // completion, vim Normal-mode switch) consumed Esc earlier, so
            // this is the widget's interrupt/quit policy layer — last, per
            // acceptance criterion 14 — fed through `status.running`.
            KeyCode::Esc => {
                if self.status.running {
                    BottomPaneOutcome::Interrupt
                } else {
                    BottomPaneOutcome::Quit
                }
            }
            // Ctrl-C interrupts an in-flight turn; when idle it arms, and a
            // second press within the window quits (claude-code parity).
            KeyCode::Char('c') if ctrl => {
                if self.status.running {
                    BottomPaneOutcome::Interrupt
                } else if self.ctrl_c_armed() {
                    BottomPaneOutcome::Quit
                } else {
                    self.ctrl_c_at = Some(Instant::now());
                    BottomPaneOutcome::Consumed
                }
            }
            // Emacs-style composer edits.
            KeyCode::Char('a') if ctrl => {
                self.composer.home();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Char('e') if ctrl => {
                self.composer.end();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Char('w') if ctrl => {
                self.composer.delete_word();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Char('u') if ctrl => {
                self.composer.kill_to_line_start();
                BottomPaneOutcome::Consumed
            }
            // Ctrl-O toggles verbose (expand thinking/tool-use/grouped
            // blocks) — transcript state, so the owner executes it.
            KeyCode::Char('o') if ctrl => BottomPaneOutcome::ToggleVerbose,
            // Modified Enter (Alt/Shift) inserts a newline; plain Enter submits.
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.composer.insert_newline();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Enter => self
                .take_submission_state()
                .map_or(BottomPaneOutcome::Consumed, BottomPaneOutcome::Submitted),
            KeyCode::Home => {
                self.composer.home();
                BottomPaneOutcome::Consumed
            }
            KeyCode::End => {
                self.composer.end();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Left if ctrl => {
                self.composer.move_word_left();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Right if ctrl => {
                self.composer.move_word_right();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Left => {
                self.composer.move_left();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Right => {
                self.composer.move_right();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Up => {
                self.composer.up();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Down => {
                self.composer.down();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Backspace => {
                self.composer.backspace();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Delete => {
                self.composer.delete();
                BottomPaneOutcome::Consumed
            }
            KeyCode::Char(c) if !ctrl => {
                self.composer.insert(c);
                BottomPaneOutcome::Consumed
            }
            _ => BottomPaneOutcome::Consumed,
        }
    }

    /// The running status indicator shown ABOVE the composer (codex
    /// `StatusIndicatorWidget` position): owner-fed spinner text + Ctrl-C
    /// hint + session cost, 2-space indented (codex `LIVE_PREFIX_COLS`). Idle
    /// key hints live in the footer BELOW the composer now
    /// (`footer::footer_line`) — only called while `self.status.running`.
    fn status_indicator_line(&self) -> Line<'static> {
        let dim = crate::style_adapter::to_ratatui(self.theme.dim);
        let claude = crate::style_adapter::to_ratatui(self.theme.claude);
        // The spinner text already carries "esc to interrupt"; Esc while
        // running interrupts (it does NOT quit), so no "Esc: quit" here.
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(self.status.text.clone(), Style::default().fg(claude)),
            Span::styled("   ·  Ctrl-C: cancel", Style::default().fg(dim)),
        ];
        if let Some(cost) = &self.status.cost {
            spans.push(Span::styled(format!("  ·  {cost}"), Style::default().fg(dim)));
        }
        Line::from(spans)
    }

    /// Footer props for the current pane state (mode selection that codex
    /// keeps in `ChatComposer::footer_props`).
    fn footer_props(&self) -> footer::FooterProps {
        let mode = if self.ctrl_c_armed() {
            footer::FooterMode::CtrlCReminder
        } else if self.completion.is_some() {
            footer::FooterMode::CompletionActive
        } else if self.verbose {
            footer::FooterMode::IdleVerbose
        } else {
            footer::FooterMode::Idle
        };
        footer::FooterProps {
            mode,
            vim_label: self.vim.as_ref().map(|v| v.label().to_string()),
            cost: self.status.cost.clone(),
        }
    }

    /// The context-pressure banner row (claude-code `<TokenWarning>`): the
    /// byte-exact orchestrator-computed text, colored by severity (`Dim` →
    /// theme dim, `Warning`/`Error` → the matching theme colors). Only called
    /// when a banner is set (its zone is zero-height otherwise).
    fn context_pressure_line(&self, banner: &traits::ContextPressureBanner) -> Line<'static> {
        let color = match banner.level {
            traits::ContextPressureLevel::Dim => self.theme.dim,
            traits::ContextPressureLevel::Warning => self.theme.warning,
            traits::ContextPressureLevel::Error => self.theme.error,
        };
        Line::from(Span::styled(
            banner.text.clone(),
            Style::default().fg(crate::style_adapter::to_ratatui(color)),
        ))
    }

    /// The pane's vertical zones within `area`, in codex's order (codex
    /// `BottomPane::as_renderable`): the running status indicator ABOVE,
    /// then the context-pressure banner, the queued-input preview, the
    /// composer, and finally the below-composer slot — the completion popup
    /// when open (codex `ActivePopup` replacing the footer), the key-hint
    /// footer otherwise.
    fn zones(&self, area: Rect) -> std::rc::Rc<[Rect]> {
        let below = if let Some(c) = self.completion.as_ref() {
            CompletionView::desired_height(c)
        } else {
            footer::footer_height(&self.footer_props())
        };
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(u16::from(self.status.running)),
                Constraint::Length(u16::from(self.context_pressure.is_some())),
                Constraint::Length(self.pending_input_preview.desired_height(area.width)),
                Constraint::Min(3),
                Constraint::Length(below),
            ])
            .split(area)
    }

    /// A full-frame active view (e.g. `/help`), which owns the whole pane
    /// area: no status row, no composer, cursor is the view's to claim.
    fn full_frame_view(&self) -> Option<&dyn BottomPaneView> {
        self.view_stack
            .active()
            .filter(|view| !view.wants_status_line())
    }

    /// [`Renderable::desired_height`], but with `running` supplied by the
    /// caller instead of read from `self.status.running`. The pane's own
    /// running flag is refreshed lazily (only [`Self::set_task_running`]
    /// updates it, called from `handle_key`/`render`), so an owner sizing its
    /// viewport right after a turn starts/ends — before either of those runs
    /// again — would otherwise measure against a stale flag. Height is now
    /// running-dependent (the status indicator adds a row), so
    /// [`crate::chat_widget::ChatWidget::desired_height`] calls this with the
    /// freshly computed value instead.
    #[must_use]
    pub fn desired_height_for(&self, width: u16, running: bool) -> u16 {
        let composer =
            u16::try_from(self.composer.lines().len().clamp(1, MAX_VISIBLE_LINES)).unwrap_or(1) + 2;
        let preview = self.pending_input_preview.desired_height(width);
        let banner = u16::from(self.context_pressure.is_some());
        let running = u16::from(running);
        let below = if let Some(popup) = &self.completion {
            popup.desired_height()
        } else {
            footer::footer_height(&self.footer_props())
        };
        let base = running + banner + preview + composer + below;
        let overlay = if let Some(view) = self.view_stack.active() {
            view.desired_height(width)
        } else {
            0
        };
        base.max(overlay)
    }
}

impl Renderable for BottomPane {
    /// Draw the pane: the running status indicator (only while a turn is in
    /// flight) + context-pressure banner + queued-input preview + the
    /// borderless composer (background block with a `›` gutter prompt), then
    /// the below-composer slot — the completion popup when open, the
    /// key-hint footer otherwise — with stacked views painted bottom-to-top
    /// over the full area, unless the active view owns the whole frame.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if let Some(view) = self.full_frame_view() {
            view.render(area, buf);
            return;
        }
        let zones = self.zones(area);
        if self.status.running {
            Paragraph::new(self.status_indicator_line()).render(zones[0], buf);
        }
        if let Some(banner) = &self.context_pressure {
            Paragraph::new(self.context_pressure_line(banner)).render(zones[1], buf);
        }
        self.pending_input_preview.render(zones[2], buf);
        ComposerView::new(&self.composer)
            .with_accent(self.accent.map(crate::style_adapter::to_ratatui))
            .render(zones[3], buf);
        let below = zones[4];
        if let Some(popup) = &self.completion {
            // `CompletionView::render` anchors UPWARD from the rect it is
            // given (it grew up from the composer's top edge in the old
            // above-composer layout). Feeding it a zero-height rect pinned
            // to the below-zone's bottom edge makes the same upward-anchor
            // math land exactly on `below` — i.e. the popup now fills the
            // below-composer slot instead of sitting above the composer.
            let anchor = Rect {
                x: below.x,
                y: below.y.saturating_add(below.height),
                width: below.width,
                height: 0,
            };
            popup.render(anchor, buf);
        } else {
            footer::render_footer(below, buf, &self.footer_props(), &self.theme);
        }
        for view in self.view_stack.views() {
            view.render(area, buf);
        }
    }

    /// Desired pane height at `width` columns: running indicator + banner +
    /// preview + composer + the below-composer slot (completion popup or
    /// footer), grown to fit the active stacked view (which reports its own
    /// height). The owner applies the viewport min/max clamp.
    fn desired_height(&self, width: u16) -> u16 {
        self.desired_height_for(width, self.status.running)
    }

    /// The composer's cursor (claimed even while centered modals are open —
    /// pre-pane behavior preserved), or the full-frame view's cursor (hidden
    /// by default) when one owns the whole area.
    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if let Some(view) = self.full_frame_view() {
            return view.cursor_pos(area);
        }
        ComposerView::new(&self.composer).cursor_pos(self.zones(area)[3])
    }

    fn cursor_style(&self, area: Rect) -> SetCursorStyle {
        if let Some(view) = self.full_frame_view() {
            return view.cursor_style(area);
        }
        ComposerView::new(&self.composer).cursor_style(self.zones(area)[3])
    }
}

/// Whether `s` is a single existing image file path (used to route pastes to
/// an image message vs composer text).
fn is_image_path(s: &str) -> bool {
    if s.is_empty() || s.contains('\n') {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    let has_img_ext = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"]
        .iter()
        .any(|e| lower.ends_with(e));
    has_img_ext && std::path::Path::new(s).is_file()
}

/// The transient view stack: the TOP view owns the keyboard; views pop
/// themselves off through the [`ViewOutcome`] they return.
#[derive(Default)]
pub struct ViewStack {
    views: Vec<Box<dyn BottomPaneView>>,
}

impl ViewStack {
    /// An empty stack.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push `view`; it becomes the active (keyboard-owning) view.
    pub fn push(&mut self, view: Box<dyn BottomPaneView>) {
        self.views.push(view);
    }

    /// Whether no view is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    /// How many views are stacked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.views.len()
    }

    /// The active (top) view, if any.
    #[must_use]
    pub fn active(&self) -> Option<&dyn BottomPaneView> {
        self.views.last().map(std::convert::AsRef::as_ref)
    }

    /// Whether any stacked view (not just the top) is a `V` — e.g. "is a
    /// permission prompt open somewhere?".
    #[must_use]
    pub fn contains<V: 'static>(&self) -> bool {
        self.views.iter().any(|view| view.as_any().is::<V>())
    }

    /// All stacked views bottom-to-top, for painting them in order.
    #[must_use]
    pub fn views(&self) -> &[Box<dyn BottomPaneView>] {
        &self.views
    }

    /// Route a key to the active view. Returns `None` when no view is open
    /// (the caller should route the key to the composer instead); otherwise
    /// the stack has already performed the pop/push bookkeeping the outcome
    /// demands and the caller only acts on app-level effects.
    pub fn route_key(&mut self, key: KeyEvent) -> Option<ViewOutcome> {
        let view = self.views.last_mut()?;
        let outcome = view.handle_key(key);
        Some(self.apply(outcome))
    }

    /// Route a bracketed paste to the active view (same contract as
    /// [`Self::route_key`]).
    pub fn route_paste(&mut self, text: &str) -> Option<ViewOutcome> {
        let view = self.views.last_mut()?;
        let outcome = view.handle_paste(text);
        Some(self.apply(outcome))
    }

    /// Perform the stack bookkeeping `outcome` demands — pop the completed
    /// view, cascade parent dismissal on accept, push opened child views —
    /// and return the outcome the owner still has to act on.
    fn apply(&mut self, outcome: ViewOutcome) -> ViewOutcome {
        match outcome {
            ViewOutcome::Pending => ViewOutcome::Pending,
            // A `/web` effect keeps its view open — the owner runs it
            // asynchronously and the view's own state (e.g. the config
            // screen's input buffer, test status) stays live while it waits.
            ViewOutcome::RunWebAction(action) => ViewOutcome::RunWebAction(action),
            ViewOutcome::Cancelled => {
                // A cancelled child pops alone: parents stay open (codex
                // parity — cancel returns to the parent flow).
                self.views.pop();
                ViewOutcome::Cancelled
            }
            ViewOutcome::OpenView(child) => {
                // Consumed locally: the requesting view stays open beneath.
                self.views.push(child);
                ViewOutcome::Pending
            }
            accepted => {
                // Accepted / SubmitPrompt / SwitchModel / PermissionResponse /
                // RunCommand all complete the active view acceptingly: pop it,
                // then every parent that asked to dismiss with its child.
                self.views.pop();
                while self
                    .views
                    .last()
                    .is_some_and(|view| view.dismiss_after_child_accept())
                {
                    self.views.pop();
                }
                accepted
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::*;
    use crate::renderable::Renderable;

    /// A scripted view: returns the next queued outcome per key/paste.
    struct StubView {
        outcomes: Vec<ViewOutcome>,
        dismiss_after_child_accept: bool,
        keys_seen: usize,
        pastes_seen: usize,
    }

    impl StubView {
        fn returning(outcomes: Vec<ViewOutcome>) -> Self {
            Self {
                outcomes,
                dismiss_after_child_accept: false,
                keys_seen: 0,
                pastes_seen: 0,
            }
        }

        fn dismissing_parent() -> Self {
            Self {
                outcomes: Vec::new(),
                dismiss_after_child_accept: true,
                keys_seen: 0,
                pastes_seen: 0,
            }
        }

        fn next_outcome(&mut self) -> ViewOutcome {
            if self.outcomes.is_empty() {
                ViewOutcome::Pending
            } else {
                self.outcomes.remove(0)
            }
        }
    }

    impl Renderable for StubView {
        fn render(&self, _area: Rect, _buf: &mut Buffer) {}
        fn desired_height(&self, _width: u16) -> u16 {
            1
        }
    }

    impl BottomPaneView for StubView {
        fn handle_key(&mut self, _key: KeyEvent) -> ViewOutcome {
            self.keys_seen += 1;
            self.next_outcome()
        }

        fn handle_paste(&mut self, _text: &str) -> ViewOutcome {
            self.pastes_seen += 1;
            self.next_outcome()
        }

        fn dismiss_after_child_accept(&self) -> bool {
            self.dismiss_after_child_accept
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// A second concrete type so `contains::<V>()` has something to miss.
    struct OtherView;

    impl Renderable for OtherView {
        fn render(&self, _area: Rect, _buf: &mut Buffer) {}
        fn desired_height(&self, _width: u16) -> u16 {
            1
        }
    }

    impl BottomPaneView for OtherView {
        fn handle_key(&mut self, _key: KeyEvent) -> ViewOutcome {
            ViewOutcome::Pending
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn route_key_is_none_when_no_view_is_open() {
        let mut stack = ViewStack::new();
        assert!(stack.is_empty());
        assert!(stack.route_key(key(KeyCode::Enter)).is_none());
        assert!(stack.route_paste("x").is_none());
    }

    #[test]
    fn push_makes_the_view_active_and_pending_keeps_it_open() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(vec![ViewOutcome::Pending])));
        assert_eq!(stack.len(), 1);
        assert!(stack.active().is_some());
        let outcome = stack.route_key(key(KeyCode::Down)).expect("view active");
        assert!(matches!(outcome, ViewOutcome::Pending));
        assert_eq!(stack.len(), 1, "pending view stays open");
    }

    #[test]
    fn only_the_top_view_receives_keys() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(Vec::new())));
        stack.push(Box::new(OtherView));
        stack.route_key(key(KeyCode::Char('x')));
        let bottom = stack.views()[0]
            .as_any()
            .downcast_ref::<StubView>()
            .expect("bottom stub");
        assert_eq!(bottom.keys_seen, 0, "keys never reach covered views");
    }

    #[test]
    fn cancelled_pops_only_the_active_view_even_over_a_dismissing_parent() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::dismissing_parent()));
        stack.push(Box::new(StubView::returning(vec![ViewOutcome::Cancelled])));
        let outcome = stack.route_key(key(KeyCode::Esc)).expect("view active");
        assert!(matches!(outcome, ViewOutcome::Cancelled));
        assert_eq!(stack.len(), 1, "cancel returns to the parent flow");
    }

    #[test]
    fn accepted_pops_the_view_and_keeps_a_non_dismissing_parent() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(Vec::new())));
        stack.push(Box::new(StubView::returning(vec![ViewOutcome::Accepted(
            ViewAction::Selected(2),
        )])));
        let outcome = stack.route_key(key(KeyCode::Enter)).expect("view active");
        assert!(matches!(
            outcome,
            ViewOutcome::Accepted(ViewAction::Selected(2))
        ));
        assert_eq!(stack.len(), 1, "parent without the dismiss flag stays");
    }

    #[test]
    fn child_accept_dismisses_every_flagged_parent_in_a_row() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(Vec::new()))); // unflagged root
        stack.push(Box::new(StubView::dismissing_parent()));
        stack.push(Box::new(StubView::dismissing_parent()));
        stack.push(Box::new(StubView::returning(vec![ViewOutcome::Accepted(
            ViewAction::Selected(0),
        )])));
        stack.route_key(key(KeyCode::Enter));
        assert_eq!(stack.len(), 1, "both flagged parents dismissed with child");
    }

    #[test]
    fn open_view_pushes_a_child_on_top_of_the_requester() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(vec![ViewOutcome::OpenView(
            Box::new(OtherView),
        )])));
        let outcome = stack.route_key(key(KeyCode::Enter)).expect("view active");
        assert!(
            matches!(outcome, ViewOutcome::Pending),
            "OpenView is consumed by the stack"
        );
        assert_eq!(stack.len(), 2);
        assert!(stack.active().expect("child").as_any().is::<OtherView>());
    }

    #[test]
    fn app_level_outcomes_are_forwarded_and_pop_the_view() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(vec![
            ViewOutcome::RunCommand(CommandAction::Quit),
        ])));
        let outcome = stack.route_key(key(KeyCode::Enter)).expect("view active");
        assert!(matches!(
            outcome,
            ViewOutcome::RunCommand(CommandAction::Quit)
        ));
        assert!(stack.is_empty());

        stack.push(Box::new(StubView::returning(vec![
            ViewOutcome::SubmitPrompt("hi".to_string()),
        ])));
        let outcome = stack.route_key(key(KeyCode::Enter)).expect("view active");
        assert!(matches!(outcome, ViewOutcome::SubmitPrompt(ref p) if p == "hi"));
        assert!(stack.is_empty());
    }

    #[test]
    fn paste_routes_to_the_active_view_and_defaults_to_swallowed() {
        let mut stack = ViewStack::new();
        stack.push(Box::new(StubView::returning(Vec::new())));
        stack.push(Box::new(OtherView)); // default handle_paste → Pending
        let outcome = stack.route_paste("pasted").expect("view active");
        assert!(matches!(outcome, ViewOutcome::Pending));
        assert_eq!(stack.len(), 2, "swallowed paste closes nothing");
        let bottom = stack.views()[0]
            .as_any()
            .downcast_ref::<StubView>()
            .expect("bottom stub");
        assert_eq!(bottom.pastes_seen, 0, "paste never reaches covered views");
    }

    #[test]
    fn contains_finds_views_anywhere_in_the_stack() {
        let mut stack = ViewStack::new();
        assert!(!stack.contains::<StubView>());
        stack.push(Box::new(StubView::returning(Vec::new())));
        stack.push(Box::new(OtherView));
        assert!(stack.contains::<StubView>(), "buried view is still found");
        assert!(stack.contains::<OtherView>());
    }

    // ===== BottomPane (plan Phase 5) =====

    use ratatui::layout::Position;

    fn pane() -> BottomPane {
        BottomPane::new(Theme::dark())
    }

    fn typ(pane: &mut BottomPane, s: &str) {
        for c in s.chars() {
            let _ = pane.handle_key(key(KeyCode::Char(c)));
        }
    }

    fn buffer_row(buf: &Buffer, y: u16) -> String {
        (buf.area.left()..buf.area.right())
            .map(|x| {
                buf.cell(Position::new(x, y))
                    .map_or(" ", ratatui::buffer::Cell::symbol)
            })
            .collect()
    }

    /// Every row of `buf`, top to bottom (plan Task 5 layout tests).
    fn buffer_rows(buf: &Buffer) -> Vec<String> {
        (buf.area.top()..buf.area.bottom())
            .map(|y| buffer_row(buf, y))
            .collect()
    }

    // ===== Plan Task 5: codex bottom-pane order (status above, composer,
    // footer below) =====

    #[test]
    fn idle_pane_order_is_composer_then_footer() {
        let pane = pane();
        let area = Rect::new(0, 0, 80, pane.desired_height(80));
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        let rows: Vec<String> = buffer_rows(&buf);
        // LAST row carries the key hints…
        assert!(
            rows.last().unwrap().trim_start().starts_with("Enter: send"),
            "footer at bottom: {rows:?}"
        );
        // …and the first row is the composer's top padding, NOT a hint row.
        assert!(!rows[0].contains("Enter: send"), "no hints above the composer: {rows:?}");
        // Composer prompt sits directly above the footer block.
        assert!(
            rows.iter().any(|r| r.starts_with("› ") || r.starts_with('›')),
            "gutter prompt present: {rows:?}"
        );
    }

    #[test]
    fn running_pane_shows_spinner_above_and_no_hints_below() {
        let mut pane = pane();
        pane.set_task_running(BottomPaneStatus {
            running: true,
            text: "Simmering… (esc to interrupt)".to_string(),
            cost: None,
        });
        let area = Rect::new(0, 0, 80, pane.desired_height(80));
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        let rows: Vec<String> = buffer_rows(&buf);
        assert!(rows[0].contains("Simmering"), "spinner row above the composer: {rows:?}");
        assert!(
            rows[0].starts_with("  "),
            "codex status rows are LIVE_PREFIX_COLS-indented"
        );
    }

    #[test]
    fn key_routing_order_active_view_before_completion_before_composer() {
        let mut pane = pane();
        // Completion open: Down navigates the popup, not the composer/history.
        typ(&mut pane, "/");
        assert!(pane.completion().is_some());
        let _ = pane.handle_key(key(KeyCode::Down));
        assert_eq!(pane.completion().unwrap().selected(), 1);
        assert_eq!(pane.composer().text(), "/", "composer untouched");
        // A stacked view covers the completion popup AND the composer.
        pane.show_view(Box::new(StubView::returning(Vec::new())));
        let _ = pane.handle_key(key(KeyCode::Down));
        assert_eq!(
            pane.completion().unwrap().selected(),
            1,
            "view swallowed the key before completion"
        );
        let _ = pane.handle_key(key(KeyCode::Char('x')));
        assert_eq!(pane.composer().text(), "/", "view swallowed typing too");
    }

    #[test]
    fn completion_falls_through_to_composer_so_typing_keeps_filtering() {
        let mut pane = pane();
        typ(&mut pane, "/m");
        let popup = pane.completion().expect("popup open");
        assert_eq!(popup.selected_insert(), "/model");
        // Tab replaces the whole buffer for a /command.
        let _ = pane.handle_key(key(KeyCode::Tab));
        assert_eq!(pane.composer().text(), "/model");
    }

    #[test]
    fn enter_accepts_the_highlighted_completion_and_closes_the_popup() {
        // Regression: selecting a slash command from the popup with Enter must
        // COMMIT the highlighted command into the buffer and CLOSE the popup —
        // not fall through and submit the bare "/" typed so far (baseline
        // iocraft: "Enter always commits the highlighted row outright, overlay
        // closes"). Previously Enter was unhandled by the completion, so it
        // submitted the partial buffer.
        let mut pane = pane();
        typ(&mut pane, "/m");
        assert_eq!(
            pane.completion().expect("popup open").selected_insert(),
            "/model"
        );
        let outcome = pane.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(outcome, BottomPaneOutcome::Consumed),
            "Enter must accept the completion (Consumed), not submit: {outcome:?}"
        );
        assert_eq!(
            pane.composer().text(),
            "/model",
            "the highlighted command is committed into the buffer"
        );
        assert!(
            pane.completion().is_none(),
            "the popup closes after an Enter accept, so a second Enter can submit"
        );
    }

    #[test]
    fn bare_slash_opens_completion_listing_the_whole_registry() {
        // Plan Phase 12 step 1: command completion is sourced from the ONE
        // command registry — a bare "/" lists every advertised command in
        // registry order, and navigation clamps at the list edges (the
        // deliberate LingXi behavior, plan Phase 12 step 4).
        let mut pane = pane();
        typ(&mut pane, "/");
        let first = crate::command::advertised().next().unwrap().name;
        assert_eq!(
            pane.completion().expect("popup open").selected_insert(),
            first
        );
        // Walk to the last advertised command; further Downs clamp there.
        let total = crate::command::advertised().count();
        for _ in 0..total + 3 {
            let _ = pane.handle_key(key(KeyCode::Down));
        }
        let popup = pane.completion().expect("popup still open");
        assert_eq!(popup.selected(), total - 1, "clamped at the last item");
        let last = crate::command::advertised().last().unwrap().name;
        assert_eq!(popup.selected_insert(), last);
        // Tab completes the highlighted registry command into the buffer.
        let _ = pane.handle_key(key(KeyCode::Tab));
        assert_eq!(pane.composer().text(), last);
    }

    #[test]
    fn unknown_command_and_empty_file_results_close_the_popup() {
        // Empty results close the popup (BottomPane owns open/close): an
        // unknown /command fragment…
        let mut pane = pane();
        typ(&mut pane, "/m");
        assert!(pane.completion().is_some());
        typ(&mut pane, "z"); // "/mz" matches nothing in the registry
        assert!(pane.completion().is_none(), "no matches → popup closed");
        // …typing on keeps it closed, and Enter falls through as a normal
        // submission (the unknown command is the owner's routing decision).
        assert!(matches!(
            pane.handle_key(key(KeyCode::Enter)),
            BottomPaneOutcome::Submitted(ref p) if p == "/mz"
        ));
        // …and an @file fragment with no matching entries.
        let mut pane = super::BottomPane::new(Theme::dark());
        typ(&mut pane, "see @Carg");
        assert!(pane.completion().is_some());
        typ(&mut pane, "zzz"); // "@Cargzzz" matches no file
        assert!(pane.completion().is_none(), "no files → popup closed");
    }

    #[test]
    fn at_file_completion_replaces_token_in_place() {
        let mut pane = pane();
        typ(&mut pane, "see @Carg");
        assert!(pane.completion().is_some(), "@ token opens file completion");
        let _ = pane.handle_key(key(KeyCode::Tab));
        assert!(
            pane.composer().text().starts_with("see @Cargo.toml"),
            "got: {}",
            pane.composer().text()
        );
    }

    #[test]
    fn history_recall_via_up_and_down() {
        let mut pane = pane();
        typ(&mut pane, "first");
        assert!(matches!(
            pane.handle_key(key(KeyCode::Enter)),
            BottomPaneOutcome::Submitted(ref p) if p == "first"
        ));
        typ(&mut pane, "second");
        let _ = pane.handle_key(key(KeyCode::Enter));
        let _ = pane.handle_key(key(KeyCode::Up));
        assert_eq!(pane.composer().text(), "second");
        let _ = pane.handle_key(key(KeyCode::Up));
        assert_eq!(pane.composer().text(), "first");
        let _ = pane.handle_key(key(KeyCode::Down));
        assert_eq!(pane.composer().text(), "second");
    }

    #[test]
    fn take_submission_state_trims_and_ignores_blank() {
        let mut pane = pane();
        assert!(pane.composer_is_empty());
        assert_eq!(pane.take_submission_state(), None);
        typ(&mut pane, "   ");
        assert_eq!(pane.take_submission_state(), None, "blank stays put");
        assert_eq!(pane.composer().text(), "   ", "blank buffer untouched");
        let mut pane = super::BottomPane::new(Theme::dark());
        typ(&mut pane, "  hi there  ");
        assert_eq!(pane.take_submission_state().as_deref(), Some("hi there"));
        assert!(pane.composer_is_empty());
        // The submission was pushed to history.
        let _ = pane.handle_key(key(KeyCode::Up));
        assert_eq!(pane.composer().text(), "  hi there  ");
    }

    #[test]
    fn vim_routing_normal_mode_edits_and_enter_submits() {
        let mut pane = pane();
        assert!(!pane.vim_enabled());
        assert!(pane.toggle_vim());
        typ(&mut pane, "hello");
        // Esc → Normal mode (consumed by vim; NOT a Quit outcome).
        assert!(matches!(
            pane.handle_key(key(KeyCode::Esc)),
            BottomPaneOutcome::Consumed
        ));
        // `0` + `x` edit instead of typing.
        let _ = pane.handle_key(key(KeyCode::Char('0')));
        let _ = pane.handle_key(key(KeyCode::Char('x')));
        assert_eq!(pane.composer().text(), "ello");
        // Normal-mode Enter submits through the same take path.
        assert!(matches!(
            pane.handle_key(key(KeyCode::Enter)),
            BottomPaneOutcome::Submitted(ref p) if p == "ello"
        ));
        assert!(!pane.toggle_vim(), "second toggle disables vim");
    }

    #[test]
    fn esc_quits_and_ctrl_o_toggles_verbose_via_owner() {
        let mut pane = pane();
        assert!(matches!(
            pane.handle_key(key(KeyCode::Esc)),
            BottomPaneOutcome::Quit
        ));
        assert!(matches!(
            pane.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
            BottomPaneOutcome::ToggleVerbose
        ));
    }

    #[test]
    fn ctrl_c_interrupts_when_running_and_arms_then_quits_when_idle() {
        let mut pane = pane();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        pane.set_task_running(BottomPaneStatus {
            running: true,
            text: "Working…".to_string(),
            cost: None,
        });
        assert!(matches!(
            pane.handle_key(ctrl_c),
            BottomPaneOutcome::Interrupt
        ));
        assert!(
            !pane.ctrl_c_armed(),
            "interrupt does not arm the quit chord"
        );
        pane.set_task_running(BottomPaneStatus::default());
        assert!(matches!(
            pane.handle_key(ctrl_c),
            BottomPaneOutcome::Consumed
        ));
        assert!(pane.ctrl_c_armed());
        assert!(matches!(pane.handle_key(ctrl_c), BottomPaneOutcome::Quit));
    }

    #[test]
    fn esc_interrupts_when_running_and_quits_when_idle() {
        let mut pane = pane();
        pane.set_task_running(BottomPaneStatus {
            running: true,
            text: "✻ Working… (1s · esc to interrupt)".to_string(),
            cost: None,
        });
        // Running + no local surface: Esc surfaces the interrupt intent (the
        // spinner's "esc to interrupt" hint), never Quit.
        assert!(matches!(
            pane.handle_key(key(KeyCode::Esc)),
            BottomPaneOutcome::Interrupt
        ));
        assert!(
            !pane.ctrl_c_armed(),
            "interrupt does not arm the quit chord"
        );
        // Idle again: Esc falls back to the quit policy.
        pane.set_task_running(BottomPaneStatus::default());
        assert!(matches!(
            pane.handle_key(key(KeyCode::Esc)),
            BottomPaneOutcome::Quit
        ));
    }

    #[test]
    fn any_other_key_disarms_the_ctrl_c_quit_chord() {
        let mut pane = pane();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let _ = pane.handle_key(ctrl_c);
        assert!(pane.ctrl_c_armed());
        let _ = pane.handle_key(key(KeyCode::Char('h')));
        assert!(!pane.ctrl_c_armed());
        assert!(matches!(
            pane.handle_key(ctrl_c),
            BottomPaneOutcome::Consumed
        ));
    }

    #[test]
    fn paste_inserts_text_and_routes_image_paths_to_the_owner() {
        let mut pane = pane();
        typ(&mut pane, "pre ");
        assert!(matches!(
            pane.handle_paste("hello world"),
            BottomPaneOutcome::Consumed
        ));
        assert_eq!(pane.composer().text(), "pre hello world");
        // A non-existent image path is plain text.
        assert!(matches!(
            pane.handle_paste("/no/such/file.png"),
            BottomPaneOutcome::Consumed
        ));
        // An existing image file surfaces as PastedImage (trimmed).
        let path =
            std::env::temp_dir().join(format!("tui-rata-pane-paste-{}.png", std::process::id()));
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").expect("write fixture image");
        let before = pane.composer().text();
        let outcome = pane.handle_paste(&format!(" {} ", path.display()));
        std::fs::remove_file(&path).ok();
        assert!(matches!(
            outcome,
            BottomPaneOutcome::PastedImage(ref p) if *p == path.display().to_string()
        ));
        assert_eq!(pane.composer().text(), before, "image paste skips composer");
    }

    #[test]
    fn render_cjk_cursor_uses_display_columns_below_the_status_row() {
        let mut pane = pane();
        typ(&mut pane, "你好");
        let area = Rect::new(0, 0, 80, pane.desired_height(80).max(4));
        // x = gutter inner.x(2) + two wide chars × 2 columns = 6; y = the
        // composer's top padding (1) — idle has no leading status row now
        // (it moved below the composer as the footer).
        assert_eq!(pane.cursor_pos(area), Some((6, 1)));
    }

    #[test]
    fn render_multiline_composer_scrolls_to_keep_cursor_visible() {
        let mut pane = pane();
        typ(&mut pane, "l0");
        for i in 1..8 {
            let _ = pane.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
            typ(&mut pane, &format!("l{i}"));
        }
        // 8 content lines clamp at MAX_VISIBLE_LINES: 6 + 2 + 1(footer) = 9 rows.
        assert_eq!(pane.desired_height(80), 9);
        let area = Rect::new(0, 0, 80, 9);
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        // The composer scrolled so l7 (cursor row) is the bottom visible
        // content row: l2..l7 fill the 6 content rows. The `›` gutter prompt
        // is pinned to the top visible row (l2 here); the footer hints are
        // pinned to the LAST row, below the composer.
        assert!(buffer_row(&buf, 8).contains("Enter: send"));
        assert!(
            buffer_row(&buf, 1).starts_with("› l2"),
            "{}",
            buffer_row(&buf, 1)
        );
        assert!(
            buffer_row(&buf, 6).starts_with("  l7"),
            "{}",
            buffer_row(&buf, 6)
        );
        let (x, y) = pane.cursor_pos(area).expect("composer cursor");
        assert_eq!((x, y), (4, 6), "cursor on the bottom visible content row");
    }

    #[test]
    fn queued_input_preview_seam_grows_the_pane_and_renders_between_status_and_composer() {
        let mut pane = pane();
        assert_eq!(pane.desired_height(80), 4, "empty preview adds no rows");
        pane.set_queued_messages(vec!["queued draft".to_string()]);
        assert_eq!(pane.desired_height(80), 6, "header + 1 queued row");
        let area = Rect::new(0, 0, 80, 6);
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        // Idle has no leading status row: the preview is first now.
        assert!(buffer_row(&buf, 0).starts_with("Queued messages:"), "preview first");
        assert!(buffer_row(&buf, 1).starts_with("  ↳ queued draft"));
        // Row 2 is the composer's top padding (no border glyph); the `›`
        // gutter prompt renders on row 3, the first content row.
        assert!(
            buffer_row(&buf, 3).starts_with('›'),
            "composer prompt below preview: {}",
            buffer_row(&buf, 3)
        );
        // The footer hints are pinned to the last row, below the composer.
        assert!(buffer_row(&buf, 5).contains("Enter: send"), "footer last");
        // Cursor moves down with the composer: top padding row is now y=2.
        assert_eq!(pane.cursor_pos(area), Some((2, 3)));
    }

    #[test]
    fn status_line_variants_running_armed_and_vim_label() {
        let mut pane = pane();
        // Running: the spinner + Ctrl-C hint sits on the FIRST row (the
        // status indicator above the composer) — unaffected by the reorder.
        pane.set_task_running(BottomPaneStatus {
            running: true,
            text: "✻ Working… (3s · esc to interrupt)".to_string(),
            cost: None,
        });
        let area = Rect::new(0, 0, 80, pane.desired_height(80));
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        assert!(buffer_row(&buf, 0).contains("esc to interrupt"));
        assert!(buffer_row(&buf, 0).contains("Ctrl-C: cancel"));

        // Idle variants all live in the footer now — the LAST row.
        pane.set_task_running(BottomPaneStatus::default());
        let _ = pane.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let area = Rect::new(0, 0, 80, pane.desired_height(80));
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        assert!(buffer_row(&buf, area.height - 1).contains("Press Ctrl-C again to exit"));

        // Vim label prefixes the idle hints once enabled ('c' above disarmed…
        // actually ctrl-c armed; type to disarm, then enable vim).
        let _ = pane.handle_key(key(KeyCode::Backspace));
        assert!(pane.toggle_vim());
        let area = Rect::new(0, 0, 80, pane.desired_height(80));
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        assert!(
            buffer_row(&buf, area.height - 1).contains("[INSERT]"),
            "{}",
            buffer_row(&buf, area.height - 1)
        );

        pane.set_verbose(true);
        let area = Rect::new(0, 0, 80, pane.desired_height(80));
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        assert!(buffer_row(&buf, area.height - 1).contains("Ctrl-O: collapse"));
    }

    // ===== Fix round 1: cost suffix + context-pressure banner row =====

    #[test]
    fn status_row_appends_the_owner_fed_cost_dim_suffix() {
        let mut pane = pane();
        // Idle hints now live in the footer (the LAST row); it carries the
        // cost once known. Width 90 (not 80): the footer's 2-column indent
        // (`FOOTER_INDENT_COLS`) pushes this exact hint+cost combination past
        // 80 columns, clipping the cost suffix — an unrelated width edge
        // case, not what this test checks.
        pane.set_task_running(BottomPaneStatus {
            running: false,
            text: String::new(),
            cost: Some("$0.0123".to_string()),
        });
        let area = Rect::new(0, 0, 90, pane.desired_height(90));
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        let row = buffer_row(&buf, area.height - 1);
        assert!(
            row.contains("Enter: send") && row.contains("·  $0.0123"),
            "{row}"
        );
        // Running spinner row (still the FIRST row) keeps it too, after the
        // cancel hint.
        pane.set_task_running(BottomPaneStatus {
            running: true,
            text: "✻ Working… (3s · esc to interrupt)".to_string(),
            cost: Some("$0.0123".to_string()),
        });
        let area = Rect::new(0, 0, 90, pane.desired_height(90));
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        let row = buffer_row(&buf, 0);
        assert!(
            row.contains("Ctrl-C: cancel") && row.contains("·  $0.0123"),
            "{row}"
        );
    }

    #[test]
    fn context_pressure_banner_takes_one_row_between_status_and_composer() {
        let mut pane = pane();
        let without = pane.desired_height(80);
        pane.set_context_pressure(Some(traits::ContextPressureBanner {
            text: "Context left until auto-compact: 8%".to_string(),
            level: traits::ContextPressureLevel::Dim,
        }));
        assert_eq!(pane.desired_height(80), without + 1, "banner adds one row");
        let area = Rect::new(0, 0, 80, pane.desired_height(80));
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        // Idle has no leading status row: the banner is first now.
        assert!(
            buffer_row(&buf, 0).contains("Context left until auto-compact: 8%"),
            "banner row first: {}",
            buffer_row(&buf, 0)
        );
        // Row 1 is the composer's top padding (no border glyph); the `›`
        // gutter prompt renders on row 2, the first content row below it.
        assert!(
            buffer_row(&buf, 2).starts_with('›'),
            "composer prompt below the banner: {}",
            buffer_row(&buf, 2)
        );
        // The footer hints are pinned to the last row, below the composer.
        assert!(buffer_row(&buf, 4).contains("Enter: send"), "footer last");
        // The composer cursor tracks the shifted composer zone.
        assert_eq!(pane.cursor_pos(area).map(|(_, y)| y), Some(2));
        // Clearing removes the row and restores the height.
        pane.set_context_pressure(None);
        assert_eq!(pane.desired_height(80), without);
    }

    // ===== Plan Phase 13: completion popup layout (steps 1 + 3) =====

    #[test]
    fn completion_desired_height_tracks_the_popup_row_count() {
        let mut pane = pane();
        // Bare "/" lists the whole registry: 6 visible rows + 2 borders = 8
        // popup rows, REPLACING the footer below the composer (3 rows).
        typ(&mut pane, "/");
        assert!(pane.completion().is_some());
        assert_eq!(pane.desired_height(80), 11, "composer 3 + full popup 8");
        // "/m" narrows to 3 items (/model, /mcp, /memory): the pane shrinks
        // with the popup (3 + 2 borders = 5 popup rows) instead of keeping a
        // fixed +8.
        typ(&mut pane, "m");
        assert_eq!(pane.completion().unwrap().desired_height(), 5);
        assert_eq!(pane.desired_height(80), 8, "composer 3 + filtered popup 5");
    }

    #[test]
    fn completion_popup_items_and_composer_occupy_disjoint_rows() {
        // Codex order (plan Task 5): the composer comes FIRST, then the
        // completion popup occupies the below-composer slot exactly like
        // codex's `ActivePopup` replaces the footer — no hint row survives
        // while the popup is open.
        let mut pane = pane();
        typ(&mut pane, "/");
        let height = pane.desired_height(80);
        assert_eq!(height, 11);
        let area = Rect::new(0, 0, 80, height);
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        // Rows 0..=2: the composer (top padding, prompt, bottom padding) —
        // disjoint from the popup below it, no overlap.
        assert!(
            !buffer_row(&buf, 0).contains('┌'),
            "composer top padding has no border: {}",
            buffer_row(&buf, 0)
        );
        assert!(
            buffer_row(&buf, 1).starts_with("› /"),
            "prompt row: {}",
            buffer_row(&buf, 1)
        );
        assert!(
            !buffer_row(&buf, 2).contains('└'),
            "composer bottom padding has no border: {}",
            buffer_row(&buf, 2)
        );
        // Rows 3..=10: the popup box with its 6-item window fully visible,
        // directly beneath the composer.
        assert!(buffer_row(&buf, 3).contains("Complete"), "popup title row");
        assert!(
            buffer_row(&buf, 4).contains("› /help"),
            "first item highlighted: {}",
            buffer_row(&buf, 4)
        );
        assert!(
            buffer_row(&buf, 9).contains("/hooks"),
            "sixth item visible: {}",
            buffer_row(&buf, 9)
        );
        assert!(
            buffer_row(&buf, 10).starts_with('└'),
            "popup bottom border: {}",
            buffer_row(&buf, 10)
        );
        // The cursor sits on the composer's prompt row, after the "/".
        assert_eq!(pane.cursor_pos(area), Some((3, 1)));
    }

    // ===== Plan Phase 13 step 4: cursor containment =====

    #[test]
    fn cursor_claim_is_contained_or_hidden_at_tiny_pane_heights() {
        let mut pane = pane();
        typ(&mut pane, "hello");
        for height in 1..=4u16 {
            let area = Rect::new(0, 0, 80, height);
            match pane.cursor_pos(area) {
                None => {} // hidden is always safe
                Some((x, y)) => {
                    assert!(
                        x < area.right() && y < area.bottom(),
                        "cursor ({x},{y}) escapes the {height}-row pane"
                    );
                }
            }
        }
        // At the degenerate 2-row height the composer has no content row at
        // all: the cursor must be hidden, never parked outside the box.
        assert_eq!(pane.cursor_pos(Rect::new(0, 0, 80, 2)), None);
        // At the full idle height it is claimed inside the composer (idle has
        // no leading status row now, so the composer's top padding is row 0).
        assert_eq!(pane.cursor_pos(Rect::new(0, 0, 80, 4)), Some((7, 1)));
    }
}
