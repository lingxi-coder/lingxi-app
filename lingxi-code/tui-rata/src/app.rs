//! Interactive `tui-rata` chat app: state + event loop.
//!
//! Holds the scrollback message list + composer buffer + scroll position,
//! renders the 3-zone layout (scrollback / status / composer), processes key
//! events (type, backspace, submit, scroll, cancel, quit), and drains
//! streaming `TurnEvent`s from the orchestrator bridge to grow the in-flight
//! assistant reply live.
//!
//! The app is decoupled from orchestrator construction: `run_app` takes a
//! `TurnEvent` receiver (drained each tick) and an `on_submit` callback that
//! receives the prompt + a per-turn `CancellationToken` (the caller spawns the
//! real turn; the app cancels it on Ctrl-C).

use std::io;
use std::io::Write;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::backend::{Backend, CrosstermBackend};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};
use tokio_util::sync::CancellationToken;
use tui_core::message::RenderedMessage;
use tui_core::orchestrator_bridge::TurnEvent;
use tui_core::permission_bridge::PermissionExchange;
use tui_core::theme::Theme;

use crate::bottom_pane::permission_view::PermissionView;
use crate::bottom_pane::screen_view::ScreenView;
use crate::bottom_pane::{BottomPane, BottomPaneOutcome, BottomPaneStatus, CommandAction};
use crate::history_cell::MessageHistoryCell;
use crate::renderable::Renderable;
use crate::session::SessionInfo;
use crate::terminal::TerminalSession;
use crate::transcript::Transcript;
use crate::RataTerminal;

/// What a key press means to the event loop.
enum KeyOutcome {
    /// Keep looping.
    Continue,
    /// Exit the app.
    Quit,
    /// The user submitted `prompt`; the caller should drive a turn for it,
    /// honoring the paired [`CancellationToken`] (the app cancels it on Ctrl-C).
    Submit(String, CancellationToken),
    /// The user picked a model in `/model`; the caller should switch to
    /// `(request_model, profile)` via `OrchestratorHandle::switch_model`.
    SwitchModel(String, Option<String>),
}

/// Interactive chat state.
pub struct RataApp {
    /// Conversation history: committed cells + the active streaming cell +
    /// the native-scrollback commit cursor + verbose/render mode.
    transcript: Transcript,
    /// The interactive footer: composer + completion + vim + status hints +
    /// the transient view stack (plan Phase 5). The pane routes local input;
    /// process-level intents come back as [`BottomPaneOutcome`]s.
    bottom_pane: BottomPane,
    theme: Theme,
    /// Cancellation token for the in-flight turn, if any.
    current_turn: Option<CancellationToken>,
    /// Wall-clock start, used to advance the streaming spinner animation.
    start: std::time::Instant,
    /// When the in-flight turn began, for the spinner's elapsed-seconds counter.
    turn_started_at: Option<std::time::Instant>,
    /// Human label for what the turn is currently doing (e.g. `Running Bash`),
    /// set from `ToolUseStart` and shown by the spinner instead of a bare verb.
    activity: Option<String>,
    /// Startup snapshot the read-only screens render from.
    session: SessionInfo,
}

impl RataApp {
    /// Build an app seeded with an initial conversation (may be empty).
    #[must_use]
    pub fn new(messages: Vec<RenderedMessage>) -> Self {
        let theme = Theme::dark();
        Self {
            transcript: Transcript::from_messages(messages),
            bottom_pane: BottomPane::new(theme),
            theme,
            current_turn: None,
            start: std::time::Instant::now(),
            turn_started_at: None,
            activity: None,
            session: SessionInfo::default(),
        }
    }

    /// Attach the startup [`SessionInfo`] snapshot the full-page screens render
    /// from (builder; the default is an empty session).
    #[must_use]
    pub fn with_session(mut self, session: SessionInfo) -> Self {
        self.session = session;
        self
    }

    /// The pane's task-status input, recomputed from the app's turn state
    /// (the pane holds no turn state of its own — plan Phase 5 boundary).
    fn pane_status(&self) -> BottomPaneStatus {
        BottomPaneStatus {
            running: self.current_turn.is_some(),
            text: self.spinner_text(),
        }
    }

    fn on_key(&mut self, key: KeyEvent) -> KeyOutcome {
        // Feed the pane the current turn status (Ctrl-C routing depends on
        // it), then route the key through the pane's layered input handling:
        // active view first, then completion, then vim, then the composer.
        self.bottom_pane.set_task_running(self.pane_status());
        let outcome = self.bottom_pane.handle_key(key);
        self.on_pane_outcome(outcome)
    }

    /// Execute the app-level intent the pane returned from a key or paste.
    fn on_pane_outcome(&mut self, outcome: BottomPaneOutcome) -> KeyOutcome {
        match outcome {
            BottomPaneOutcome::Consumed => KeyOutcome::Continue,
            BottomPaneOutcome::Quit => KeyOutcome::Quit,
            BottomPaneOutcome::Interrupt => {
                if let Some(token) = self.current_turn.take() {
                    token.cancel();
                }
                self.turn_started_at = None;
                self.activity = None;
                KeyOutcome::Continue
            }
            BottomPaneOutcome::ToggleVerbose => {
                self.transcript.toggle_verbose();
                self.bottom_pane.set_verbose(self.transcript.verbose());
                KeyOutcome::Continue
            }
            BottomPaneOutcome::Submitted(text) => {
                if let Some(outcome) = self.handle_slash(&text) {
                    return outcome;
                }
                self.submit_prompt(text)
            }
            BottomPaneOutcome::SubmitPrompt(prompt) => self.submit_prompt(prompt),
            BottomPaneOutcome::SwitchModel {
                request_model,
                profile,
            } => {
                self.transcript.push_message(RenderedMessage::SystemText {
                    body: format!("Switching model to {request_model}…"),
                    timestamp: 0,
                    is_error: false,
                });
                KeyOutcome::SwitchModel(request_model, profile)
            }
            BottomPaneOutcome::RunCommand(action) => self.run_command(action),
            BottomPaneOutcome::PastedImage(path) => {
                let name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(String::from);
                self.transcript.push_message(RenderedMessage::UserImage {
                    image_id: None,
                    metadata: name,
                    source_path: Some(path),
                });
                KeyOutcome::Continue
            }
        }
    }

    /// Record `text` as the user's prompt and hand it to the caller with a
    /// fresh per-turn cancellation token. Shared by the composer submit path
    /// and [`BottomPaneOutcome::SubmitPrompt`].
    fn submit_prompt(&mut self, text: String) -> KeyOutcome {
        self.transcript.push_message(RenderedMessage::UserText {
            body: text.clone(),
            timestamp: 0,
        });
        let token = CancellationToken::new();
        self.current_turn = Some(token.clone());
        KeyOutcome::Submit(text, token)
    }

    /// Execute a command effect a view requested via
    /// [`BottomPaneOutcome::RunCommand`].
    fn run_command(&mut self, action: CommandAction) -> KeyOutcome {
        match action {
            CommandAction::ClearTranscript => {
                self.transcript.clear();
                KeyOutcome::Continue
            }
            CommandAction::Quit => KeyOutcome::Quit,
        }
    }

    /// Handle a bracketed paste by routing it through the pane (active view
    /// first, then image-path detection, then composer insertion).
    fn on_paste(&mut self, text: &str) -> KeyOutcome {
        let outcome = self.bottom_pane.handle_paste(text);
        self.on_pane_outcome(outcome)
    }

    /// Fold one streaming event from the orchestrator bridge into the
    /// transcript: `TurnStarted` opens an empty active assistant cell,
    /// `TextDelta` mutates it in place, `TurnEnded` finalizes it (moves it to
    /// the committed history) and clears the in-flight cancel token; other
    /// variants are ignored for now.
    pub fn apply_turn_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::TurnStarted => {
                self.turn_started_at = Some(std::time::Instant::now());
                self.activity = None;
                // A straggler active cell (missed TurnEnded) is finalized, not
                // dropped, before the new streaming reply opens.
                self.transcript.flush_active();
                self.transcript.set_active(Box::new(MessageHistoryCell::new(
                    RenderedMessage::AssistantText {
                        body: String::new(),
                        timestamp: 0,
                    },
                )));
            }
            TurnEvent::TextDelta(delta) => {
                let appended = self
                    .transcript
                    .mutate_active(|cell| {
                        if let Some(RenderedMessage::AssistantText { body, .. }) = cell
                            .as_any_mut()
                            .downcast_mut::<MessageHistoryCell>()
                            .map(MessageHistoryCell::message_mut)
                        {
                            body.push_str(&delta);
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !appended {
                    self.transcript.flush_active();
                    self.transcript.set_active(Box::new(MessageHistoryCell::new(
                        RenderedMessage::AssistantText {
                            body: delta,
                            timestamp: 0,
                        },
                    )));
                }
            }
            TurnEvent::ToolUseStart { tool, .. } => {
                self.activity = Some(activity_label(&tool));
            }
            TurnEvent::ToolUseResult { .. } => {
                self.activity = None;
            }
            TurnEvent::TurnEnded(_) => {
                self.transcript.flush_active();
                self.current_turn = None;
                self.turn_started_at = None;
                self.activity = None;
            }
            _ => {}
        }
    }

    /// Open a permission prompt for `exchange` by pushing a
    /// [`PermissionView`]: it owns the keyboard until the user resolves it
    /// (Enter/1-3 approve or deny, Esc denies) and delivers the response
    /// through the exchange's one-shot channel exactly once.
    pub fn open_permission(&mut self, exchange: PermissionExchange) {
        self.bottom_pane.show_permission(exchange);
    }

    /// Whether a permission prompt is anywhere on the view stack (the event
    /// loop defers further permission requests until it resolves).
    fn has_open_permission(&self) -> bool {
        self.bottom_pane.view_stack().contains::<PermissionView>()
    }

    /// Route a recognized slash command. Returns `Some(outcome)` when the input
    /// is a handled command (screen open, clear, exit), or `None` to fall
    /// through and send the input as a normal prompt.
    fn handle_slash(&mut self, input: &str) -> Option<KeyOutcome> {
        let trimmed = input.trim();
        // `/image <path>` pushes an image message + selects it so a graphics
        // terminal shows the real pixels in the preview pane.
        if let Some(path) = trimmed.strip_prefix("/image ") {
            let path = path.trim().to_string();
            let name = std::path::Path::new(&path)
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from);
            self.transcript.push_message(RenderedMessage::UserImage {
                image_id: None,
                metadata: name,
                source_path: Some(path),
            });
            return Some(KeyOutcome::Continue);
        }
        match trimmed {
            "/help" => {
                self.bottom_pane.show_view(Box::new(ScreenView::help()));
                Some(KeyOutcome::Continue)
            }
            "/doctor" => {
                self.bottom_pane
                    .show_view(Box::new(ScreenView::doctor(&self.session.doctor)));
                Some(KeyOutcome::Continue)
            }
            "/mcp" => {
                self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
                    "MCP servers",
                    "MCP servers",
                    &self.session.mcp,
                    "No MCP servers configured.",
                )));
                Some(KeyOutcome::Continue)
            }
            "/hooks" => {
                self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
                    "Hooks",
                    "Hooks",
                    &self.session.hooks,
                    "No hooks configured.",
                )));
                Some(KeyOutcome::Continue)
            }
            "/agents" => {
                self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
                    "Agents",
                    "Agents",
                    &self.session.agents,
                    "No agents configured.",
                )));
                Some(KeyOutcome::Continue)
            }
            "/clear" => {
                self.transcript.clear();
                Some(KeyOutcome::Continue)
            }
            "/model" => {
                if self.session.models.is_empty() {
                    self.transcript.push_message(RenderedMessage::SystemText {
                        body: "No models available.".to_string(),
                        timestamp: 0,
                        is_error: false,
                    });
                } else {
                    self.bottom_pane
                        .show_model_picker(self.session.models.clone());
                }
                Some(KeyOutcome::Continue)
            }
            "/exit" | "/quit" => Some(KeyOutcome::Quit),
            "/vim" => {
                let now_on = self.bottom_pane.toggle_vim();
                self.transcript.push_message(RenderedMessage::SystemText {
                    body: format!("Vim mode {}.", if now_on { "enabled" } else { "disabled" }),
                    timestamp: 0,
                    is_error: false,
                });
                Some(KeyOutcome::Continue)
            }
            _ => None,
        }
    }

    /// Desired inline-viewport height at `width` columns: the pane reports
    /// its own height (status + composer, grown to fit the active stacked
    /// view or the completion popup); the app applies the viewport clamp.
    fn viewport_height(&self, width: u16) -> u16 {
        self.bottom_pane.desired_height(width).clamp(4, 20)
    }

    /// Commit finalized transcript cells into the terminal's native scrollback
    /// via [`crate::terminal::Terminal::insert_history_lines`] (written ABOVE
    /// the bottom viewport), delegating to
    /// [`Transcript::flush_to_native_scrollback`]. The actively-streaming cell
    /// is never committed here (it still grows in place); it commits once as a
    /// whole when `TurnEnded` finalizes it.
    ///
    /// Generic over the backend so tests can drive it with a test backend; the
    /// runtime passes [`RataTerminal`].
    fn flush_scrollback<B: Backend + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        let width = terminal.size()?.width.max(1);
        self.transcript
            .flush_to_native_scrollback(terminal, width, &self.theme)
    }

    /// The current streaming-spinner text: an animated Claude-accent glyph, the
    /// live activity (`Running Bash` from `ToolUseStart`, else `Working`), and an
    /// elapsed-seconds counter with an interrupt hint — claude-code status parity.
    fn spinner_text(&self) -> String {
        const FRAMES: &[&str] = &["·", "✢", "✳", "✶", "✻", "✽", "✽", "✻", "✶", "✳", "✢", "·"];
        let idx =
            usize::try_from(self.start.elapsed().as_millis() / 120).unwrap_or(0) % FRAMES.len();
        let verb = self.activity.as_deref().unwrap_or("Working");
        let secs = self.turn_started_at.map_or(0, |t| t.elapsed().as_secs());
        format!("{} {verb}… ({secs}s · esc to interrupt)", FRAMES[idx])
    }

    /// Draw the bottom viewport (history lives in the terminal's native
    /// scrollback via [`Self::flush_scrollback`]): the [`BottomPane`] renders
    /// the status line + composer box + overlays through the `(Rect, &mut
    /// Buffer)` [`Renderable`] contract.
    ///
    /// The [`crate::terminal::Frame`] is only the terminal draw BOUNDARY: the
    /// pane draws into the frame's buffer, and the pane's cursor claim
    /// (composer cursor, or a full-frame view's) is copied back onto the
    /// frame at the end (no claim → cursor hidden).
    fn render_viewport(&mut self, frame: &mut crate::terminal::Frame) {
        // Refresh the pane's status input so the spinner text/animation
        // reflect this tick's turn state.
        self.bottom_pane.set_task_running(self.pane_status());
        let area = frame.area();
        self.bottom_pane.render(area, frame.buffer_mut());
        if let Some(pos) = self.bottom_pane.cursor_pos(area) {
            frame.set_cursor_position(pos);
            frame.set_cursor_style(self.bottom_pane.cursor_style(area));
        }
    }
}

/// Human label shown in the spinner for an in-flight tool call, mapping the
/// tool name to a claude-code-style gerund (`Bash` → `Running Bash`).
fn activity_label(tool: &str) -> String {
    match tool {
        "Bash" | "BashOutput" => "Running Bash".to_string(),
        "Read" => "Reading".to_string(),
        "Write" => "Writing".to_string(),
        "Edit" | "MultiEdit" => "Editing".to_string(),
        "Grep" | "Glob" => "Searching".to_string(),
        "WebFetch" | "WebSearch" => "Browsing".to_string(),
        "Task" => "Delegating".to_string(),
        other => format!("Running {other}"),
    }
}

/// Run the interactive chat app on the bottom-anchored custom terminal:
/// history is committed to the terminal's native scrollback via
/// [`RataApp::flush_scrollback`]; the bottom viewport (status + composer +
/// overlays) is diff-redrawn each tick and resized in place via
/// [`crate::terminal::Terminal::set_bottom_viewport_height`] when its desired
/// height changes (composer growth / overlay open). Terminal modes are
/// restored on exit — including panics — by the [`TerminalSession`] guard and
/// the terminal's own drop (cursor style/visibility).
///
/// # Errors
/// Propagates the first terminal IO error (after restoring the terminal).
pub fn run_app(
    messages: Vec<RenderedMessage>,
    session: SessionInfo,
    mut events_rx: UnboundedReceiver<TurnEvent>,
    mut permission_rx: Receiver<PermissionExchange>,
    mut on_submit: impl FnMut(String, CancellationToken),
    mut on_switch_model: impl FnMut(String, Option<String>),
) -> io::Result<()> {
    // Guard first, terminal second: locals drop in reverse order, so the
    // terminal resets the cursor while raw mode is still active, then the
    // guard restores cooked mode + bracketed paste.
    let _session_guard = TerminalSession::new()?;
    let mut terminal =
        crate::terminal::Terminal::with_options(CrosstermBackend::new(io::stdout()))?;
    let mut app = RataApp::new(messages).with_session(session);
    app_loop(
        &mut terminal,
        &mut app,
        &mut events_rx,
        &mut permission_rx,
        &mut on_submit,
        &mut on_switch_model,
    )
}

fn app_loop(
    terminal: &mut RataTerminal,
    app: &mut RataApp,
    events_rx: &mut UnboundedReceiver<TurnEvent>,
    permission_rx: &mut Receiver<PermissionExchange>,
    on_submit: &mut impl FnMut(String, CancellationToken),
    on_switch_model: &mut impl FnMut(String, Option<String>),
) -> io::Result<()> {
    loop {
        while let Ok(event) = events_rx.try_recv() {
            app.apply_turn_event(event);
        }
        // Take a new permission request only when none is currently shown.
        if !app.has_open_permission() {
            if let Ok(exchange) = permission_rx.try_recv() {
                app.open_permission(exchange);
            }
        }
        // Size the absolute bottom viewport for this tick, THEN commit
        // finalized history above it (insertion wraps at the viewport width).
        let width = terminal.size()?.width;
        terminal.set_bottom_viewport_height(app.viewport_height(width))?;
        app.flush_scrollback(terminal)?;
        terminal.draw(|frame| app.render_viewport(frame))?;
        if event::poll(Duration::from_millis(50))? {
            let outcome = match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Paste(text) => app.on_paste(&text),
                _ => KeyOutcome::Continue,
            };
            match outcome {
                KeyOutcome::Quit => return Ok(()),
                KeyOutcome::Submit(prompt, token) => on_submit(prompt, token),
                KeyOutcome::SwitchModel(model, profile) => on_switch_model(model, profile),
                KeyOutcome::Continue => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};
    use permission::gate::{PermissionRequest, PermissionResponse};
    use tokio::sync::oneshot;

    use super::*;
    use crate::bottom_pane::model_picker_view::ModelPickerView;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    fn typ(app: &mut RataApp, s: &str) {
        for c in s.chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
    }

    /// The transcript's messages in order — committed cells then the active
    /// (streaming) cell — reconstructing the pre-Transcript `app.messages`
    /// view so the behavior-lock assertions keep their exact indices.
    fn messages(app: &RataApp) -> Vec<RenderedMessage> {
        let cell_message = |cell: &dyn crate::history_cell::HistoryCell| {
            cell.as_any()
                .downcast_ref::<MessageHistoryCell>()
                .expect("phase 3 transcript holds adapter cells only")
                .message()
                .clone()
        };
        let mut out: Vec<RenderedMessage> = app
            .transcript
            .committed_cells()
            .iter()
            .map(|cell| cell_message(cell.as_ref()))
            .collect();
        out.extend(app.transcript.active_cell().map(cell_message));
        out
    }

    #[test]
    fn alt_enter_inserts_newline_plain_enter_submits_whole_buffer() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "line one");
        // Alt+Enter adds a newline instead of submitting.
        let outcome = app.on_key(alt(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Continue));
        typ(&mut app, "line two");
        assert_eq!(app.bottom_pane.composer().text(), "line one\nline two");
        // Plain Enter submits the full multi-line buffer.
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Submit(ref p, _) if p == "line one\nline two"));
        assert_eq!(app.bottom_pane.composer().text(), "");
    }

    #[test]
    fn up_arrow_recalls_submitted_history() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "first prompt");
        app.on_key(press(KeyCode::Enter));
        typ(&mut app, "second prompt");
        app.on_key(press(KeyCode::Enter));
        // Composer is empty; Up walks newest → oldest.
        app.on_key(press(KeyCode::Up));
        assert_eq!(app.bottom_pane.composer().text(), "second prompt");
        app.on_key(press(KeyCode::Up));
        assert_eq!(app.bottom_pane.composer().text(), "first prompt");
    }

    #[test]
    fn left_arrow_then_typing_inserts_at_cursor() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "ac");
        app.on_key(press(KeyCode::Left)); // between a|c
        app.on_key(press(KeyCode::Char('b')));
        assert_eq!(app.bottom_pane.composer().text(), "abc");
    }

    #[test]
    fn typing_then_submit_echoes_user_and_returns_prompt() {
        let mut app = RataApp::new(Vec::new());
        for c in "hi".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        assert_eq!(app.bottom_pane.composer().text(), "hi");
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Submit(ref p, _) if p == "hi"));
        assert_eq!(app.bottom_pane.composer().text(), "");
        assert!(app.current_turn.is_some());
        let msgs = messages(&app);
        assert_eq!(msgs.len(), 1);
        assert!(matches!(msgs[0], RenderedMessage::UserText { .. }));
    }

    #[test]
    fn empty_submit_is_ignored() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            app.on_key(press(KeyCode::Enter)),
            KeyOutcome::Continue
        ));
        assert!(messages(&app).is_empty());
    }

    #[test]
    fn streaming_deltas_grow_reply_and_turn_ended_clears_token() {
        let mut app = RataApp::new(Vec::new());
        app.on_key(press(KeyCode::Char('x')));
        app.on_key(press(KeyCode::Enter));
        assert!(app.current_turn.is_some());
        app.apply_turn_event(TurnEvent::TurnStarted);
        app.apply_turn_event(TurnEvent::TextDelta("Hel".to_string()));
        app.apply_turn_event(TurnEvent::TextDelta("lo".to_string()));
        match &messages(&app)[1] {
            RenderedMessage::AssistantText { body, .. } => assert_eq!(body, "Hello"),
            other => panic!("expected assistant text, got {other:?}"),
        }
        app.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        assert!(app.current_turn.is_none());
    }

    #[test]
    fn ctrl_c_cancels_turn_then_needs_two_presses_to_quit() {
        let mut app = RataApp::new(Vec::new());
        app.on_key(press(KeyCode::Char('x')));
        app.on_key(press(KeyCode::Enter));
        let token = app.current_turn.clone().unwrap();
        assert!(!token.is_cancelled());
        // Ctrl-C during a turn interrupts it (does NOT quit) and clears activity.
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('c'))),
            KeyOutcome::Continue
        ));
        assert!(token.is_cancelled());
        assert!(app.current_turn.is_none());
        // First idle Ctrl-C only arms the exit; it does not quit.
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('c'))),
            KeyOutcome::Continue
        ));
        assert!(app.bottom_pane.ctrl_c_armed());
        // Second idle Ctrl-C within the window quits.
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('c'))),
            KeyOutcome::Quit
        ));
    }

    #[test]
    fn typing_disarms_ctrl_c_exit() {
        let mut app = RataApp::new(Vec::new());
        // Arm the exit with an idle Ctrl-C, then type: the arm must reset so a
        // later single Ctrl-C does not quit unexpectedly.
        app.on_key(ctrl(KeyCode::Char('c')));
        assert!(app.bottom_pane.ctrl_c_armed());
        app.on_key(press(KeyCode::Char('h')));
        assert!(!app.bottom_pane.ctrl_c_armed());
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('c'))),
            KeyOutcome::Continue
        ));
    }

    #[test]
    fn composer_line_and_word_editing_keys() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "foo bar");
        // Bare Home/End move the composer cursor (not scrollback).
        app.on_key(press(KeyCode::Home));
        assert_eq!(app.bottom_pane.composer().cursor_row_col(), (0, 0));
        app.on_key(press(KeyCode::End));
        assert_eq!(app.bottom_pane.composer().cursor_row_col(), (0, 7));
        // Ctrl+W deletes the previous word.
        app.on_key(ctrl(KeyCode::Char('w')));
        assert_eq!(app.bottom_pane.composer().text(), "foo ");
        // Ctrl+U kills to line start.
        app.on_key(ctrl(KeyCode::Char('u')));
        assert_eq!(app.bottom_pane.composer().text(), "");
    }

    #[test]
    fn ctrl_o_toggles_verbose() {
        let mut app = RataApp::new(Vec::new());
        assert!(!app.transcript.verbose());
        app.on_key(ctrl(KeyCode::Char('o')));
        assert!(app.transcript.verbose());
        app.on_key(ctrl(KeyCode::Char('o')));
        assert!(!app.transcript.verbose());
    }

    #[test]
    fn viewport_height_grows_for_overlays() {
        let mut app = RataApp::new(Vec::new());
        let base = app.viewport_height(80);
        // Opening the completion popup grows the viewport.
        typ(&mut app, "/");
        assert!(app.bottom_pane.completion().is_some());
        assert!(app.viewport_height(80) > base);
    }

    #[test]
    fn paste_non_image_inserts_into_composer() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "pre ");
        app.on_paste("hello world");
        assert_eq!(app.bottom_pane.composer().text(), "pre hello world");
        // A non-existent image path is treated as text, not an image message.
        app.on_paste(" /no/such/file.png ");
        assert!(messages(&app).is_empty());
    }

    #[test]
    fn slash_image_pushes_image_message() {
        let mut app = RataApp::new(Vec::new());
        let outcome = submit_command(&mut app, "/image /tmp/pic.png");
        assert!(matches!(outcome, KeyOutcome::Continue));
        let msgs = messages(&app);
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            RenderedMessage::UserImage {
                source_path: Some(p),
                metadata,
                ..
            } => {
                assert_eq!(p, "/tmp/pic.png");
                assert_eq!(metadata.as_deref(), Some("pic.png"));
            }
            other => panic!("expected UserImage, got {other:?}"),
        }
    }

    #[test]
    fn slash_vim_toggles_vim_mode() {
        let mut app = RataApp::new(Vec::new());
        assert!(!app.bottom_pane.vim_enabled());
        submit_command(&mut app, "/vim");
        assert!(app.bottom_pane.vim_enabled());
        submit_command(&mut app, "/vim");
        assert!(!app.bottom_pane.vim_enabled());
    }

    #[test]
    fn vim_esc_enters_normal_and_motions_edit_instead_of_typing() {
        let mut app = RataApp::new(Vec::new());
        submit_command(&mut app, "/vim");
        typ(&mut app, "hello");
        // Esc → Normal mode (does NOT quit the app).
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, KeyOutcome::Continue));
        // In Normal mode, `0` moves to line start and `x` deletes — not typed.
        app.on_key(press(KeyCode::Char('0')));
        app.on_key(press(KeyCode::Char('x')));
        assert_eq!(app.bottom_pane.composer().text(), "ello");
        // `i` returns to Insert; typing inserts again.
        app.on_key(press(KeyCode::Char('i')));
        typ(&mut app, "H");
        assert_eq!(app.bottom_pane.composer().text(), "Hello");
    }

    #[test]
    fn vim_normal_enter_submits() {
        let mut app = RataApp::new(Vec::new());
        submit_command(&mut app, "/vim");
        typ(&mut app, "hi");
        app.on_key(press(KeyCode::Esc)); // → Normal
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Submit(ref p, _) if p == "hi"));
    }

    #[test]
    fn ctrl_left_right_move_by_word() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "alpha beta");
        app.on_key(ctrl(KeyCode::Left)); // to start of "beta"
        assert_eq!(app.bottom_pane.composer().cursor_row_col(), (0, 6));
        app.on_key(ctrl(KeyCode::Left)); // to start of "alpha"
        assert_eq!(app.bottom_pane.composer().cursor_row_col(), (0, 0));
    }

    #[test]
    fn typing_slash_opens_and_filters_command_palette() {
        let mut app = RataApp::new(Vec::new());
        app.on_key(press(KeyCode::Char('/')));
        assert!(app.bottom_pane.completion().is_some());
        typ(&mut app, "m"); // "/m" narrows to /model + /mcp
        let p = app.bottom_pane.completion().unwrap();
        assert_eq!(p.selected_insert(), "/model");
        // A space ends the command token and closes the popup.
        typ(&mut app, " x");
        assert!(app.bottom_pane.completion().is_none());
    }

    #[test]
    fn tab_completes_selected_command_into_composer() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "/mc");
        assert!(app.bottom_pane.completion().is_some());
        app.on_key(press(KeyCode::Tab));
        assert_eq!(app.bottom_pane.composer().text(), "/mcp");
    }

    #[test]
    fn palette_arrows_navigate_and_esc_dismisses_without_quitting() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "/");
        app.on_key(press(KeyCode::Down)); // navigate the popup, not history
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(app.bottom_pane.completion().is_none());
        // Composer text is untouched by the dismiss.
        assert_eq!(app.bottom_pane.composer().text(), "/");
    }

    #[test]
    fn typing_at_opens_file_completion_and_tab_completes_in_place() {
        let mut app = RataApp::new(Vec::new());
        // `@Carg` should match Cargo.toml in the tui-rata crate cwd.
        typ(&mut app, "see @Carg");
        assert!(
            app.bottom_pane.completion().is_some(),
            "@ token opens file completion"
        );
        app.on_key(press(KeyCode::Tab));
        // The @token is replaced in place, leaving the prefix intact.
        assert!(
            app.bottom_pane
                .composer()
                .text()
                .starts_with("see @Cargo.toml"),
            "got: {}",
            app.bottom_pane.composer().text()
        );
    }

    #[test]
    fn esc_quits() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(app.on_key(press(KeyCode::Esc)), KeyOutcome::Quit));
    }

    fn tool_exchange() -> (PermissionExchange, oneshot::Receiver<PermissionResponse>) {
        let (resp_tx, resp_rx) = oneshot::channel();
        let request = PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({ "command": "ls -la" }),
            default_decision: permission::gate::PromptDefault::DenyByDefault,
        };
        (
            PermissionExchange {
                request,
                resp_tx,
                worker: None,
            },
            resp_rx,
        )
    }

    #[test]
    fn permission_prompt_owns_keyboard_and_enter_allows_once() {
        let mut app = RataApp::new(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        assert!(app.has_open_permission());

        // While a prompt is open, normal keys are swallowed by the dialog and
        // never reach the composer.
        app.on_key(press(KeyCode::Char('x')));
        assert_eq!(app.bottom_pane.composer().text(), "");

        // Enter selects the highlighted option (index 0 = AllowOnce).
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(!app.has_open_permission());
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
    }

    #[test]
    fn permission_prompt_esc_denies() {
        let mut app = RataApp::new(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(!app.has_open_permission());
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn permission_prompt_number_three_denies() {
        let mut app = RataApp::new(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        // '3' shortcut = third option = Deny.
        app.on_key(press(KeyCode::Char('3')));
        assert!(!app.has_open_permission());
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn slash_help_opens_screen_view_without_sending_a_prompt() {
        let mut app = RataApp::new(Vec::new());
        for c in "/help".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        let outcome = app.on_key(press(KeyCode::Enter));
        // Recognized command: no Submit; a focused ScreenView opens instead
        // of dumping text into scrollback (plan Phase 4 view-stack routing).
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert_eq!(app.bottom_pane.composer().text(), "");
        assert!(app.bottom_pane.view_stack().contains::<ScreenView>());
        assert!(messages(&app).is_empty(), "no scrollback dump");
        assert!(app.current_turn.is_none());
    }

    #[test]
    fn screen_view_owns_keys_scrolls_and_esc_closes_without_quitting() {
        let mut app = RataApp::new(Vec::new());
        submit_command(&mut app, "/help");
        assert!(app.bottom_pane.view_stack().contains::<ScreenView>());
        // Keys go to the view, not the composer.
        app.on_key(press(KeyCode::Char('x')));
        assert_eq!(app.bottom_pane.composer().text(), "");
        // Down scrolls the screen body.
        app.on_key(press(KeyCode::Down));
        let scroll = app
            .bottom_pane
            .view_stack()
            .active()
            .and_then(|v| v.as_any().downcast_ref::<ScreenView>())
            .expect("help screen active")
            .scroll();
        assert_eq!(scroll, 1);
        // Esc closes the view (does NOT quit the app) and returns the keys.
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(app.bottom_pane.view_stack().is_empty());
        app.on_key(press(KeyCode::Char('h')));
        assert_eq!(app.bottom_pane.composer().text(), "h");
    }

    #[test]
    fn screen_view_q_closes() {
        let mut app = RataApp::new(Vec::new());
        submit_command(&mut app, "/help");
        let outcome = app.on_key(press(KeyCode::Char('q')));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(app.bottom_pane.view_stack().is_empty());
    }

    #[test]
    fn non_command_slash_input_is_sent_as_a_prompt() {
        let mut app = RataApp::new(Vec::new());
        for c in "/frobnicate".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        let outcome = app.on_key(press(KeyCode::Enter));
        // Unrecognized slash command falls through as a normal prompt.
        assert!(matches!(outcome, KeyOutcome::Submit(ref p, _) if p == "/frobnicate"));
        assert_eq!(messages(&app).len(), 1);
    }

    fn submit_command(app: &mut RataApp, cmd: &str) -> KeyOutcome {
        for c in cmd.chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        app.on_key(press(KeyCode::Enter))
    }

    #[test]
    fn slash_clear_empties_messages() {
        let mut app = RataApp::new(vec![RenderedMessage::SystemText {
            body: "old".to_string(),
            timestamp: 0,
            is_error: false,
        }]);
        let outcome = submit_command(&mut app, "/clear");
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(messages(&app).is_empty());
        assert_eq!(app.transcript.committed_to_terminal(), 0);
    }

    #[test]
    fn slash_exit_quits() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/exit"),
            KeyOutcome::Quit
        ));
    }

    #[test]
    fn slash_doctor_and_mcp_open_screen_views() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/doctor"),
            KeyOutcome::Continue
        ));
        assert!(app.bottom_pane.view_stack().contains::<ScreenView>());
        app.on_key(press(KeyCode::Esc)); // close /doctor
        assert!(app.bottom_pane.view_stack().is_empty());
        assert!(matches!(
            submit_command(&mut app, "/mcp"),
            KeyOutcome::Continue
        ));
        assert!(app.bottom_pane.view_stack().contains::<ScreenView>());
        // Focused views, not scrollback dumps; and no prompt turn started.
        assert!(messages(&app).is_empty());
        assert!(app.current_turn.is_none());
    }

    fn app_with_models() -> RataApp {
        RataApp::new(Vec::new()).with_session(crate::session::SessionInfo {
            models: vec![
                crate::session::ModelRow {
                    display: "Opus".into(),
                    request_model: "claude-opus".into(),
                    profile: Some("anthropic".into()),
                    provider_label: "Anthropic".into(),
                    is_current: true,
                },
                crate::session::ModelRow {
                    display: "Sonnet".into(),
                    request_model: "claude-sonnet".into(),
                    profile: Some("anthropic".into()),
                    provider_label: "Anthropic".into(),
                    is_current: false,
                },
            ],
            ..Default::default()
        })
    }

    #[test]
    fn slash_model_with_no_models_reports_instead_of_opening() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/model"),
            KeyOutcome::Continue
        ));
        assert!(!app.bottom_pane.view_stack().contains::<ModelPickerView>());
        assert_eq!(messages(&app).len(), 1);
    }

    #[test]
    fn slash_model_opens_picker_and_enter_switches() {
        let mut app = app_with_models();
        assert!(matches!(
            submit_command(&mut app, "/model"),
            KeyOutcome::Continue
        ));
        assert!(app.bottom_pane.view_stack().contains::<ModelPickerView>());
        // Picker owns the keyboard: move up to the first (Opus) row and confirm.
        app.on_key(press(KeyCode::Up));
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            KeyOutcome::SwitchModel(ref m, ref p)
                if m == "claude-opus" && p.as_deref() == Some("anthropic")
        ));
        assert!(!app.bottom_pane.view_stack().contains::<ModelPickerView>());
    }

    #[test]
    fn model_picker_esc_cancels_without_switching() {
        let mut app = app_with_models();
        submit_command(&mut app, "/model");
        assert!(app.bottom_pane.view_stack().contains::<ModelPickerView>());
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(!app.bottom_pane.view_stack().contains::<ModelPickerView>());
    }

    #[test]
    fn permission_stacks_over_picker_and_returns_keys_to_it() {
        let mut app = app_with_models();
        submit_command(&mut app, "/model");
        assert!(app.bottom_pane.view_stack().contains::<ModelPickerView>());
        // A permission request arriving while the picker is open stacks on
        // top and owns the keyboard.
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        assert_eq!(app.bottom_pane.view_stack().len(), 2);
        let outcome = app.on_key(press(KeyCode::Enter)); // resolves permission
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
        // The picker beneath survives and gets the keyboard back.
        assert!(app.bottom_pane.view_stack().contains::<ModelPickerView>());
        app.on_key(press(KeyCode::Up));
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::SwitchModel(ref m, _) if m == "claude-opus"));
    }

    #[test]
    fn view_run_command_outcomes_dispatch_to_the_app() {
        use std::any::Any;

        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        /// A stub view that returns a scripted command on Enter.
        struct CommandStub(Option<CommandAction>);

        impl Renderable for CommandStub {
            fn render(&self, _area: Rect, _buf: &mut Buffer) {}
            fn desired_height(&self, _width: u16) -> u16 {
                1
            }
        }

        impl crate::bottom_pane::BottomPaneView for CommandStub {
            fn handle_key(&mut self, _key: KeyEvent) -> crate::bottom_pane::ViewOutcome {
                self.0.take().map_or(
                    crate::bottom_pane::ViewOutcome::Pending,
                    crate::bottom_pane::ViewOutcome::RunCommand,
                )
            }

            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        // ClearTranscript empties the transcript.
        let mut app = RataApp::new(vec![RenderedMessage::SystemText {
            body: "old".to_string(),
            timestamp: 0,
            is_error: false,
        }]);
        app.bottom_pane
            .show_view(Box::new(CommandStub(Some(CommandAction::ClearTranscript))));
        assert!(matches!(
            app.on_key(press(KeyCode::Enter)),
            KeyOutcome::Continue
        ));
        assert!(messages(&app).is_empty());
        assert!(
            app.bottom_pane.view_stack().is_empty(),
            "completed view popped"
        );

        // Quit surfaces as KeyOutcome::Quit.
        let mut app = RataApp::new(Vec::new());
        app.bottom_pane
            .show_view(Box::new(CommandStub(Some(CommandAction::Quit))));
        assert!(matches!(
            app.on_key(press(KeyCode::Enter)),
            KeyOutcome::Quit
        ));
    }

    // ===== Phase 0 behavior locks (codex-ui-structure plan) =====
    // These tests freeze RataApp's CURRENT behavior before the ChatWidget /
    // Transcript / BottomPane extraction. Adapt locations when structure moves,
    // but preserve every assertion.

    use ratatui::buffer::Cell;
    use ratatui::layout::Position;

    use crate::terminal::test_support::TestWriteBackend;
    use crate::terminal::Terminal;

    /// A bottom-anchored custom terminal over an 80x24 test backend with its
    /// viewport sized to `viewport` rows, mirroring the production runtime
    /// (viewport anchored at rows `0..h` because the test cursor starts at the
    /// origin).
    fn inline_test_terminal(viewport: u16) -> Terminal<TestWriteBackend> {
        let mut terminal =
            Terminal::with_options(TestWriteBackend::new(80, 24)).expect("test terminal");
        terminal
            .set_bottom_viewport_height(viewport)
            .expect("viewport height");
        terminal
    }

    /// Draw the bottom viewport at its self-reported height (80 columns);
    /// returns the terminal for buffer/cursor inspection.
    fn draw_viewport(app: &mut RataApp) -> Terminal<TestWriteBackend> {
        let mut terminal = inline_test_terminal(app.viewport_height(80));
        terminal
            .draw(|frame| app.render_viewport(frame))
            .expect("draw");
        terminal
    }

    /// The last drawn frame (== the viewport rect) as one string per row.
    fn buffer_rows(terminal: &Terminal<TestWriteBackend>) -> Vec<String> {
        let buf = terminal.last_frame_buffer();
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| buf.cell(Position::new(x, y)).map_or(" ", Cell::symbol))
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn submitted_prompt_is_trimmed_before_echo_and_submit() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "  hi there  ");
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Submit(ref p, _) if p == "hi there"));
        match &messages(&app)[0] {
            RenderedMessage::UserText { body, .. } => assert_eq!(body, "hi there"),
            other => panic!("expected user text, got {other:?}"),
        }
    }

    #[test]
    fn whitespace_only_submit_is_ignored() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "   ");
        assert!(matches!(
            app.on_key(press(KeyCode::Enter)),
            KeyOutcome::Continue
        ));
        assert!(messages(&app).is_empty());
        assert!(app.current_turn.is_none());
    }

    #[test]
    fn slash_hooks_agents_and_quit_route_as_commands() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/hooks"),
            KeyOutcome::Continue
        ));
        assert!(app.bottom_pane.view_stack().contains::<ScreenView>());
        app.on_key(press(KeyCode::Esc)); // close /hooks
        assert!(matches!(
            submit_command(&mut app, "/agents"),
            KeyOutcome::Continue
        ));
        assert!(app.bottom_pane.view_stack().contains::<ScreenView>());
        app.on_key(press(KeyCode::Esc)); // close /agents
        assert!(messages(&app).is_empty(), "views, not scrollback dumps");
        assert!(app.current_turn.is_none(), "no prompt turn for commands");
        assert!(matches!(
            submit_command(&mut app, "/quit"),
            KeyOutcome::Quit
        ));
    }

    #[test]
    fn permission_prompt_second_option_allows_always() {
        let mut app = RataApp::new(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        app.on_key(press(KeyCode::Down)); // highlight "Yes, allow always"
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(!app.has_open_permission());
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowAlways
        );
    }

    #[test]
    fn permission_resolution_is_single_shot_and_releases_keyboard() {
        let mut app = RataApp::new(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        // '1' shortcut resolves with the first option (AllowOnce)…
        app.on_key(press(KeyCode::Char('1')));
        assert!(!app.has_open_permission());
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
        // …after which the keyboard belongs to the composer again; further keys
        // cannot re-resolve the consumed exchange (its sender is gone).
        app.on_key(press(KeyCode::Char('x')));
        assert_eq!(app.bottom_pane.composer().text(), "x");
        assert!(matches!(
            app.on_key(press(KeyCode::Enter)),
            KeyOutcome::Submit(ref p, _) if p == "x"
        ));
    }

    #[test]
    fn paste_existing_image_path_becomes_image_message() {
        let path =
            std::env::temp_dir().join(format!("tui-rata-p0-paste-{}.png", std::process::id()));
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").expect("write fixture image");
        let mut app = RataApp::new(Vec::new());
        // Surrounding whitespace is trimmed for detection AND stored path.
        app.on_paste(&format!(" {} ", path.display()));
        std::fs::remove_file(&path).ok();
        assert_eq!(
            app.bottom_pane.composer().text(),
            "",
            "image paste must not touch composer"
        );
        let msgs = messages(&app);
        assert_eq!(msgs.len(), 1);
        let file_name = path.file_name().unwrap().to_str().unwrap();
        match &msgs[0] {
            RenderedMessage::UserImage {
                source_path: Some(p),
                metadata: Some(name),
                ..
            } => {
                assert_eq!(p, &path.display().to_string());
                assert_eq!(name, file_name, "metadata is the file name");
            }
            other => panic!("expected UserImage, got {other:?}"),
        }
    }

    #[test]
    fn flush_scrollback_holds_streaming_tail_until_turn_ends() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "hi");
        app.on_key(press(KeyCode::Enter)); // user message + current_turn
        app.apply_turn_event(TurnEvent::TurnStarted);
        app.apply_turn_event(TurnEvent::TextDelta("Hel".to_string()));
        let mut terminal = inline_test_terminal(4);
        app.flush_scrollback(&mut terminal).unwrap();
        // The finalized user message commits; the streaming reply is held back.
        assert_eq!(app.transcript.committed_to_terminal(), 1);
        app.apply_turn_event(TurnEvent::TextDelta("lo".to_string()));
        app.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(
            app.transcript.committed_to_terminal(),
            1,
            "still streaming: tail stays held back"
        );
        app.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        app.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(
            app.transcript.committed_to_terminal(),
            2,
            "turn ended: reply commits as a whole"
        );
    }

    #[test]
    fn flush_scrollback_commits_everything_when_idle_including_zero_height() {
        let mut app = RataApp::new(vec![
            // Renders to zero lines: consumed by the commit cursor, no insert.
            RenderedMessage::UserText {
                body: String::new(),
                timestamp: 0,
            },
            RenderedMessage::SystemText {
                body: "ready".to_string(),
                timestamp: 0,
                is_error: false,
            },
        ]);
        let mut terminal = inline_test_terminal(4);
        app.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(app.transcript.committed_to_terminal(), 2);
    }

    #[test]
    fn viewport_grows_with_multiline_composer_up_to_cap() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "one");
        for _ in 0..9 {
            app.on_key(alt(KeyCode::Enter));
        }
        // 10 content lines clamp at composer::MAX_VISIBLE_LINES (6): 1 + 6 + 2 = 9.
        assert_eq!(app.viewport_height(80), 9);
    }

    #[test]
    fn layout_80x24_idle_status_line_plus_bordered_composer() {
        let mut app = RataApp::new(Vec::new());
        assert_eq!(app.viewport_height(80), 4, "idle bottom viewport is 4 rows");
        let terminal = draw_viewport(&mut app);
        let rows = buffer_rows(&terminal);
        assert!(rows[0].contains("Enter: send"), "status row: {}", rows[0]);
        assert!(rows[0].contains("Esc: quit"), "status row: {}", rows[0]);
        assert!(rows[1].starts_with('┌'), "composer top border: {}", rows[1]);
        assert!(rows[2].starts_with("│> "), "prompt row: {}", rows[2]);
        assert!(
            rows[3].starts_with('└'),
            "composer bottom border: {}",
            rows[3]
        );
        // The draw buffer covers EXACTLY the 4 viewport rows — rows below the
        // viewport belong to the terminal's native scrollback and cannot be
        // painted by the viewport draw (absolute-rect invariant).
        assert_eq!(rows.len(), 4);
        assert_eq!(
            terminal.viewport_area,
            ratatui::layout::Rect::new(0, 0, 80, 4)
        );
    }

    #[test]
    fn layout_cursor_uses_display_columns_for_cjk() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "你好");
        let mut terminal = draw_viewport(&mut app);
        let pos = terminal.get_cursor_position().unwrap();
        // x = border(1) + "> "(2) + two wide chars × 2 columns = 7; y = row 2.
        assert_eq!((pos.x, pos.y), (7, 2));
    }

    #[test]
    fn layout_running_turn_shows_spinner_status_with_interrupt_hint() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "go");
        app.on_key(press(KeyCode::Enter));
        app.apply_turn_event(TurnEvent::TurnStarted);
        let terminal = draw_viewport(&mut app);
        let rows = buffer_rows(&terminal);
        assert!(rows[0].contains("esc to interrupt"), "status: {}", rows[0]);
        assert!(rows[0].contains("Ctrl-C: cancel"), "status: {}", rows[0]);
    }

    #[test]
    fn layout_armed_ctrl_c_shows_press_again_hint() {
        let mut app = RataApp::new(Vec::new());
        app.on_key(ctrl(KeyCode::Char('c')));
        let terminal = draw_viewport(&mut app);
        let rows = buffer_rows(&terminal);
        assert!(
            rows[0].contains("Press Ctrl-C again to exit"),
            "status: {}",
            rows[0]
        );
    }

    #[test]
    fn layout_completion_popup_grows_viewport_and_draws_over_it() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "/");
        assert!(app.bottom_pane.completion().is_some());
        assert_eq!(app.viewport_height(80), 12, "completion viewport height");
        let terminal = draw_viewport(&mut app);
        let all = buffer_rows(&terminal).join("\n");
        assert!(all.contains("Complete"), "popup title visible:\n{all}");
    }

    #[test]
    fn layout_permission_dialog_overlays_viewport() {
        let mut app = RataApp::new(Vec::new());
        let (exchange, _resp_rx) = tool_exchange();
        app.open_permission(exchange);
        assert_eq!(app.viewport_height(80), 9, "permission viewport height");
        let terminal = draw_viewport(&mut app);
        let all = buffer_rows(&terminal).join("\n");
        assert!(all.contains("Permission required"), "{all}");
        assert!(all.contains("Yes, allow once"), "{all}");
        assert!(all.contains("No, deny"), "{all}");
    }

    #[test]
    fn layout_model_picker_overlays_viewport() {
        let mut app = app_with_models();
        submit_command(&mut app, "/model");
        assert_eq!(app.viewport_height(80), 6, "picker viewport height");
        let terminal = draw_viewport(&mut app);
        let all = buffer_rows(&terminal).join("\n");
        assert!(all.contains("Select model"), "{all}");
        assert!(all.contains("Opus"), "{all}");
        assert!(all.contains("Sonnet"), "{all}");
    }

    #[test]
    fn layout_help_screen_fills_viewport_without_status_or_composer() {
        let mut app = RataApp::new(Vec::new());
        submit_command(&mut app, "/help");
        // The help body wants more rows than the viewport allows: clamps at
        // the 20-row viewport cap (new Phase 4 lock).
        assert_eq!(app.viewport_height(80), 20, "help screen viewport height");
        let terminal = draw_viewport(&mut app);
        let rows = buffer_rows(&terminal);
        assert_eq!(rows.len(), 20);
        let all = rows.join("\n");
        assert!(all.contains("Shortcuts"), "{all}");
        assert!(all.contains("for bash mode"), "{all}");
        // Full-frame view: no status hints, no composer prompt beneath.
        assert!(!all.contains("Enter: send"), "status suppressed:\n{all}");
        assert!(!all.contains("│> "), "composer suppressed:\n{all}");
        // The view claims no cursor, so the draw hides it.
        assert!(terminal.cursor_hidden(), "screen view hides the cursor");
    }
}
