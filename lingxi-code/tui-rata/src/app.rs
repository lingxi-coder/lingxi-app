//! Interactive `tui-rata` chat app: the runtime event loop around
//! [`ChatWidget`].
//!
//! `RataApp` owns loop plumbing only: the terminal session/draw boundary,
//! channel draining (turn events + permission requests), viewport sizing,
//! and the submit/switch-model callbacks to the embedding CLI. All
//! conversation state — transcript, bottom pane, session snapshot, per-turn
//! state — lives in [`ChatWidget`] (plan Phase 6).
//!
//! The app is decoupled from orchestrator construction: `run_app` takes a
//! `TurnEvent` receiver (drained each tick) and an `on_submit` callback that
//! receives the prompt + a per-turn `CancellationToken` (the caller spawns the
//! real turn; the widget cancels it on Ctrl-C).

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

use crate::chat_widget::{ChatOutcome, ChatWidget};
use crate::session::SessionInfo;
use crate::terminal::TerminalSession;
use crate::RataTerminal;

/// Interactive chat runtime: the event-loop shell around [`ChatWidget`].
pub struct RataApp {
    /// The chat surface: transcript + bottom pane + session snapshot +
    /// per-turn state (plan Phase 6). The app keeps only loop plumbing
    /// around it (reduced fully in plan Phase 7).
    chat_widget: ChatWidget,
}

impl RataApp {
    /// Build an app seeded with an initial conversation (may be empty).
    #[must_use]
    pub fn new(messages: Vec<RenderedMessage>) -> Self {
        Self {
            chat_widget: ChatWidget::new(messages, SessionInfo::default()),
        }
    }

    /// Attach the startup [`SessionInfo`] snapshot the full-page screens render
    /// from (builder; the default is an empty session).
    #[must_use]
    pub fn with_session(mut self, session: SessionInfo) -> Self {
        self.chat_widget.set_session(session);
        self
    }

    /// Route one key press into the chat widget.
    fn on_key(&mut self, key: KeyEvent) -> ChatOutcome {
        self.chat_widget.handle_key(key)
    }

    /// Route a bracketed paste into the chat widget.
    fn on_paste(&mut self, text: &str) -> ChatOutcome {
        self.chat_widget.handle_paste(text)
    }

    /// Fold one streaming event from the orchestrator bridge into the chat
    /// widget's transcript/turn state (see [`ChatWidget::apply_turn_event`]).
    pub fn apply_turn_event(&mut self, event: TurnEvent) {
        self.chat_widget.apply_turn_event(event);
    }

    /// Open a permission prompt for `exchange` — or queue it when one is
    /// already open; prompts are serialized inside the widget (see
    /// [`ChatWidget::open_permission`]).
    pub fn open_permission(&mut self, exchange: PermissionExchange) {
        self.chat_widget.open_permission(exchange);
    }

    /// Whether a permission prompt is currently open (queued exchanges wait
    /// inside the widget until it resolves).
    #[must_use]
    pub fn has_open_permission(&self) -> bool {
        self.chat_widget.has_open_permission()
    }

    /// Desired inline-viewport height at `width` columns: the widget reports
    /// its own height ([`ChatWidget::desired_height`]); the 4/20 clamp is
    /// deliberately app-side viewport policy.
    fn viewport_height(&self, width: u16) -> u16 {
        self.chat_widget.desired_height(width).clamp(4, 20)
    }

    /// Commit finalized transcript cells into the terminal's native
    /// scrollback (see [`ChatWidget::flush_scrollback`]).
    fn flush_scrollback<B: Backend + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        self.chat_widget.flush_scrollback(terminal)
    }

    /// Draw the bottom viewport through the chat widget's render contract —
    /// the thin frame wrapper codex's `App::render_chat_widget_frame` keeps
    /// at the terminal draw boundary: the widget renders into the frame's
    /// buffer, then its cursor claim is copied onto the frame (no claim →
    /// cursor hidden).
    fn render_viewport(&mut self, frame: &mut crate::terminal::Frame) {
        let area = frame.area();
        self.chat_widget.render(area, frame.buffer_mut());
        if let Some(pos) = self.chat_widget.cursor_pos(area) {
            frame.set_cursor_position(pos);
            frame.set_cursor_style(self.chat_widget.cursor_style(area));
        }
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
        // Drain permission requests into the widget: it serializes prompts
        // (one owns the keyboard; later arrivals queue) — plan Phase 6 moved
        // the one-at-a-time gate out of this loop.
        while let Ok(exchange) = permission_rx.try_recv() {
            app.open_permission(exchange);
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
                _ => ChatOutcome::Continue,
            };
            match outcome {
                ChatOutcome::Quit => return Ok(()),
                ChatOutcome::Submit(prompt, token) => on_submit(prompt, token),
                ChatOutcome::SwitchModel(model, profile) => on_switch_model(model, profile),
                ChatOutcome::Continue => {}
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
    use crate::bottom_pane::screen_view::ScreenView;
    use crate::history_cell::MessageHistoryCell;

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
            .chat_widget
            .transcript()
            .committed_cells()
            .iter()
            .map(|cell| cell_message(cell.as_ref()))
            .collect();
        out.extend(app.chat_widget.transcript().active_cell().map(cell_message));
        out
    }

    #[test]
    fn alt_enter_inserts_newline_plain_enter_submits_whole_buffer() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "line one");
        // Alt+Enter adds a newline instead of submitting.
        let outcome = app.on_key(alt(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Continue));
        typ(&mut app, "line two");
        assert_eq!(
            app.chat_widget.bottom_pane().composer().text(),
            "line one\nline two"
        );
        // Plain Enter submits the full multi-line buffer.
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, _) if p == "line one\nline two"));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "");
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
        assert_eq!(
            app.chat_widget.bottom_pane().composer().text(),
            "second prompt"
        );
        app.on_key(press(KeyCode::Up));
        assert_eq!(
            app.chat_widget.bottom_pane().composer().text(),
            "first prompt"
        );
    }

    #[test]
    fn left_arrow_then_typing_inserts_at_cursor() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "ac");
        app.on_key(press(KeyCode::Left)); // between a|c
        app.on_key(press(KeyCode::Char('b')));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "abc");
    }

    #[test]
    fn typing_then_submit_echoes_user_and_returns_prompt() {
        let mut app = RataApp::new(Vec::new());
        for c in "hi".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "hi");
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, _) if p == "hi"));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "");
        assert!(app.chat_widget.turn_running());
        let msgs = messages(&app);
        assert_eq!(msgs.len(), 1);
        assert!(matches!(msgs[0], RenderedMessage::UserText { .. }));
    }

    #[test]
    fn empty_submit_is_ignored() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            app.on_key(press(KeyCode::Enter)),
            ChatOutcome::Continue
        ));
        assert!(messages(&app).is_empty());
    }

    #[test]
    fn streaming_deltas_grow_reply_and_turn_ended_clears_token() {
        let mut app = RataApp::new(Vec::new());
        app.on_key(press(KeyCode::Char('x')));
        app.on_key(press(KeyCode::Enter));
        assert!(app.chat_widget.turn_running());
        app.apply_turn_event(TurnEvent::TurnStarted);
        app.apply_turn_event(TurnEvent::TextDelta("Hel".to_string()));
        app.apply_turn_event(TurnEvent::TextDelta("lo".to_string()));
        match &messages(&app)[1] {
            RenderedMessage::AssistantText { body, .. } => assert_eq!(body, "Hello"),
            other => panic!("expected assistant text, got {other:?}"),
        }
        app.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        assert!(!app.chat_widget.turn_running());
    }

    #[test]
    fn ctrl_c_cancels_turn_then_needs_two_presses_to_quit() {
        let mut app = RataApp::new(Vec::new());
        app.on_key(press(KeyCode::Char('x')));
        let ChatOutcome::Submit(_, token) = app.on_key(press(KeyCode::Enter)) else {
            panic!("expected submit");
        };
        assert!(!token.is_cancelled());
        // Ctrl-C during a turn interrupts it (does NOT quit) and clears activity.
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('c'))),
            ChatOutcome::Continue
        ));
        assert!(token.is_cancelled());
        assert!(!app.chat_widget.turn_running());
        // First idle Ctrl-C only arms the exit; it does not quit.
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('c'))),
            ChatOutcome::Continue
        ));
        assert!(app.chat_widget.bottom_pane().ctrl_c_armed());
        // Second idle Ctrl-C within the window quits.
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('c'))),
            ChatOutcome::Quit
        ));
    }

    #[test]
    fn typing_disarms_ctrl_c_exit() {
        let mut app = RataApp::new(Vec::new());
        // Arm the exit with an idle Ctrl-C, then type: the arm must reset so a
        // later single Ctrl-C does not quit unexpectedly.
        app.on_key(ctrl(KeyCode::Char('c')));
        assert!(app.chat_widget.bottom_pane().ctrl_c_armed());
        app.on_key(press(KeyCode::Char('h')));
        assert!(!app.chat_widget.bottom_pane().ctrl_c_armed());
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('c'))),
            ChatOutcome::Continue
        ));
    }

    #[test]
    fn composer_line_and_word_editing_keys() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "foo bar");
        // Bare Home/End move the composer cursor (not scrollback).
        app.on_key(press(KeyCode::Home));
        assert_eq!(
            app.chat_widget.bottom_pane().composer().cursor_row_col(),
            (0, 0)
        );
        app.on_key(press(KeyCode::End));
        assert_eq!(
            app.chat_widget.bottom_pane().composer().cursor_row_col(),
            (0, 7)
        );
        // Ctrl+W deletes the previous word.
        app.on_key(ctrl(KeyCode::Char('w')));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "foo ");
        // Ctrl+U kills to line start.
        app.on_key(ctrl(KeyCode::Char('u')));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "");
    }

    #[test]
    fn ctrl_o_toggles_verbose() {
        let mut app = RataApp::new(Vec::new());
        assert!(!app.chat_widget.transcript().verbose());
        app.on_key(ctrl(KeyCode::Char('o')));
        assert!(app.chat_widget.transcript().verbose());
        app.on_key(ctrl(KeyCode::Char('o')));
        assert!(!app.chat_widget.transcript().verbose());
    }

    #[test]
    fn viewport_height_grows_for_overlays() {
        let mut app = RataApp::new(Vec::new());
        let base = app.viewport_height(80);
        // Opening the completion popup grows the viewport.
        typ(&mut app, "/");
        assert!(app.chat_widget.bottom_pane().completion().is_some());
        assert!(app.viewport_height(80) > base);
    }

    #[test]
    fn paste_non_image_inserts_into_composer() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "pre ");
        app.on_paste("hello world");
        assert_eq!(
            app.chat_widget.bottom_pane().composer().text(),
            "pre hello world"
        );
        // A non-existent image path is treated as text, not an image message.
        app.on_paste(" /no/such/file.png ");
        assert!(messages(&app).is_empty());
    }

    #[test]
    fn slash_image_pushes_image_message() {
        let mut app = RataApp::new(Vec::new());
        let outcome = submit_command(&mut app, "/image /tmp/pic.png");
        assert!(matches!(outcome, ChatOutcome::Continue));
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
        assert!(!app.chat_widget.bottom_pane().vim_enabled());
        submit_command(&mut app, "/vim");
        assert!(app.chat_widget.bottom_pane().vim_enabled());
        submit_command(&mut app, "/vim");
        assert!(!app.chat_widget.bottom_pane().vim_enabled());
    }

    #[test]
    fn vim_esc_enters_normal_and_motions_edit_instead_of_typing() {
        let mut app = RataApp::new(Vec::new());
        submit_command(&mut app, "/vim");
        typ(&mut app, "hello");
        // Esc → Normal mode (does NOT quit the app).
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, ChatOutcome::Continue));
        // In Normal mode, `0` moves to line start and `x` deletes — not typed.
        app.on_key(press(KeyCode::Char('0')));
        app.on_key(press(KeyCode::Char('x')));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "ello");
        // `i` returns to Insert; typing inserts again.
        app.on_key(press(KeyCode::Char('i')));
        typ(&mut app, "H");
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "Hello");
    }

    #[test]
    fn vim_normal_enter_submits() {
        let mut app = RataApp::new(Vec::new());
        submit_command(&mut app, "/vim");
        typ(&mut app, "hi");
        app.on_key(press(KeyCode::Esc)); // → Normal
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, _) if p == "hi"));
    }

    #[test]
    fn ctrl_left_right_move_by_word() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "alpha beta");
        app.on_key(ctrl(KeyCode::Left)); // to start of "beta"
        assert_eq!(
            app.chat_widget.bottom_pane().composer().cursor_row_col(),
            (0, 6)
        );
        app.on_key(ctrl(KeyCode::Left)); // to start of "alpha"
        assert_eq!(
            app.chat_widget.bottom_pane().composer().cursor_row_col(),
            (0, 0)
        );
    }

    #[test]
    fn typing_slash_opens_and_filters_command_palette() {
        let mut app = RataApp::new(Vec::new());
        app.on_key(press(KeyCode::Char('/')));
        assert!(app.chat_widget.bottom_pane().completion().is_some());
        typ(&mut app, "m"); // "/m" narrows to /model + /mcp
        let p = app.chat_widget.bottom_pane().completion().unwrap();
        assert_eq!(p.selected_insert(), "/model");
        // A space ends the command token and closes the popup.
        typ(&mut app, " x");
        assert!(app.chat_widget.bottom_pane().completion().is_none());
    }

    #[test]
    fn tab_completes_selected_command_into_composer() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "/mc");
        assert!(app.chat_widget.bottom_pane().completion().is_some());
        app.on_key(press(KeyCode::Tab));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "/mcp");
    }

    #[test]
    fn palette_arrows_navigate_and_esc_dismisses_without_quitting() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "/");
        app.on_key(press(KeyCode::Down)); // navigate the popup, not history
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(app.chat_widget.bottom_pane().completion().is_none());
        // Composer text is untouched by the dismiss.
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "/");
    }

    #[test]
    fn typing_at_opens_file_completion_and_tab_completes_in_place() {
        let mut app = RataApp::new(Vec::new());
        // `@Carg` should match Cargo.toml in the tui-rata crate cwd.
        typ(&mut app, "see @Carg");
        assert!(
            app.chat_widget.bottom_pane().completion().is_some(),
            "@ token opens file completion"
        );
        app.on_key(press(KeyCode::Tab));
        // The @token is replaced in place, leaving the prefix intact.
        assert!(
            app.chat_widget
                .bottom_pane()
                .composer()
                .text()
                .starts_with("see @Cargo.toml"),
            "got: {}",
            app.chat_widget.bottom_pane().composer().text()
        );
    }

    #[test]
    fn esc_quits() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(app.on_key(press(KeyCode::Esc)), ChatOutcome::Quit));
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
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "");

        // Enter selects the highlighted option (index 0 = AllowOnce).
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Continue));
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
        assert!(matches!(outcome, ChatOutcome::Continue));
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
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "");
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ScreenView>());
        assert!(messages(&app).is_empty(), "no scrollback dump");
        assert!(!app.chat_widget.turn_running());
    }

    #[test]
    fn screen_view_owns_keys_scrolls_and_esc_closes_without_quitting() {
        let mut app = RataApp::new(Vec::new());
        submit_command(&mut app, "/help");
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ScreenView>());
        // Keys go to the view, not the composer.
        app.on_key(press(KeyCode::Char('x')));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "");
        // Down scrolls the screen body.
        app.on_key(press(KeyCode::Down));
        let scroll = app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .active()
            .and_then(|v| v.as_any().downcast_ref::<ScreenView>())
            .expect("help screen active")
            .scroll();
        assert_eq!(scroll, 1);
        // Esc closes the view (does NOT quit the app) and returns the keys.
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(app.chat_widget.bottom_pane().view_stack().is_empty());
        app.on_key(press(KeyCode::Char('h')));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "h");
    }

    #[test]
    fn screen_view_q_closes() {
        let mut app = RataApp::new(Vec::new());
        submit_command(&mut app, "/help");
        let outcome = app.on_key(press(KeyCode::Char('q')));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(app.chat_widget.bottom_pane().view_stack().is_empty());
    }

    #[test]
    fn non_command_slash_input_is_sent_as_a_prompt() {
        let mut app = RataApp::new(Vec::new());
        for c in "/frobnicate".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        let outcome = app.on_key(press(KeyCode::Enter));
        // Unrecognized slash command falls through as a normal prompt.
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, _) if p == "/frobnicate"));
        assert_eq!(messages(&app).len(), 1);
    }

    fn submit_command(app: &mut RataApp, cmd: &str) -> ChatOutcome {
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
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(messages(&app).is_empty());
        assert_eq!(app.chat_widget.transcript().committed_to_terminal(), 0);
    }

    #[test]
    fn slash_exit_quits() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/exit"),
            ChatOutcome::Quit
        ));
    }

    #[test]
    fn slash_doctor_and_mcp_open_screen_views() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/doctor"),
            ChatOutcome::Continue
        ));
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ScreenView>());
        app.on_key(press(KeyCode::Esc)); // close /doctor
        assert!(app.chat_widget.bottom_pane().view_stack().is_empty());
        assert!(matches!(
            submit_command(&mut app, "/mcp"),
            ChatOutcome::Continue
        ));
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ScreenView>());
        // Focused views, not scrollback dumps; and no prompt turn started.
        assert!(messages(&app).is_empty());
        assert!(!app.chat_widget.turn_running());
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
            ChatOutcome::Continue
        ));
        assert!(!app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        assert_eq!(messages(&app).len(), 1);
    }

    #[test]
    fn slash_model_opens_picker_and_enter_switches() {
        let mut app = app_with_models();
        assert!(matches!(
            submit_command(&mut app, "/model"),
            ChatOutcome::Continue
        ));
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        // Picker owns the keyboard: move up to the first (Opus) row and confirm.
        app.on_key(press(KeyCode::Up));
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ChatOutcome::SwitchModel(ref m, ref p)
                if m == "claude-opus" && p.as_deref() == Some("anthropic")
        ));
        assert!(!app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
    }

    #[test]
    fn model_picker_esc_cancels_without_switching() {
        let mut app = app_with_models();
        submit_command(&mut app, "/model");
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(!app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
    }

    #[test]
    fn permission_stacks_over_picker_and_returns_keys_to_it() {
        let mut app = app_with_models();
        submit_command(&mut app, "/model");
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        // A permission request arriving while the picker is open stacks on
        // top and owns the keyboard.
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        assert_eq!(app.chat_widget.bottom_pane().view_stack().len(), 2);
        let outcome = app.on_key(press(KeyCode::Enter)); // resolves permission
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
        // The picker beneath survives and gets the keyboard back.
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        app.on_key(press(KeyCode::Up));
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::SwitchModel(ref m, _) if m == "claude-opus"));
    }

    // NOTE (plan Phase 6): `view_run_command_outcomes_dispatch_to_the_app`
    // moved to `chat_widget::tests::view_run_command_outcomes_dispatch_to_the_widget`
    // verbatim — RunCommand dispatch ownership moved into ChatWidget and the
    // stub view needs mutable pane access the app no longer exposes.

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
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, _) if p == "hi there"));
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
            ChatOutcome::Continue
        ));
        assert!(messages(&app).is_empty());
        assert!(!app.chat_widget.turn_running());
    }

    #[test]
    fn slash_hooks_agents_and_quit_route_as_commands() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/hooks"),
            ChatOutcome::Continue
        ));
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ScreenView>());
        app.on_key(press(KeyCode::Esc)); // close /hooks
        assert!(matches!(
            submit_command(&mut app, "/agents"),
            ChatOutcome::Continue
        ));
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ScreenView>());
        app.on_key(press(KeyCode::Esc)); // close /agents
        assert!(messages(&app).is_empty(), "views, not scrollback dumps");
        assert!(
            !app.chat_widget.turn_running(),
            "no prompt turn for commands"
        );
        assert!(matches!(
            submit_command(&mut app, "/quit"),
            ChatOutcome::Quit
        ));
    }

    #[test]
    fn permission_prompt_second_option_allows_always() {
        let mut app = RataApp::new(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        app.on_key(press(KeyCode::Down)); // highlight "Yes, allow always"
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Continue));
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
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "x");
        assert!(matches!(
            app.on_key(press(KeyCode::Enter)),
            ChatOutcome::Submit(ref p, _) if p == "x"
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
            app.chat_widget.bottom_pane().composer().text(),
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
        assert_eq!(app.chat_widget.transcript().committed_to_terminal(), 1);
        app.apply_turn_event(TurnEvent::TextDelta("lo".to_string()));
        app.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(
            app.chat_widget.transcript().committed_to_terminal(),
            1,
            "still streaming: tail stays held back"
        );
        app.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        app.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(
            app.chat_widget.transcript().committed_to_terminal(),
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
        assert_eq!(app.chat_widget.transcript().committed_to_terminal(), 2);
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
        // The live tail (the just-opened active cell's 1-row marker) renders
        // above the pane since plan Phase 6, so the status row moved to row 1.
        assert!(rows[1].contains("esc to interrupt"), "status: {}", rows[1]);
        assert!(rows[1].contains("Ctrl-C: cancel"), "status: {}", rows[1]);
    }

    #[test]
    fn layout_streaming_tail_is_visible_above_the_pane_before_turn_ends() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "go");
        app.on_key(press(KeyCode::Enter));
        app.apply_turn_event(TurnEvent::TurnStarted);
        app.apply_turn_event(TurnEvent::TextDelta("streamed reply words".to_string()));
        // The viewport grows for the tail: idle pane (4) + one tail row.
        assert_eq!(app.viewport_height(80), 5, "tail grows the viewport");
        let terminal = draw_viewport(&mut app);
        let rows = buffer_rows(&terminal);
        // The mid-turn delta is visible in the drawn frame BEFORE TurnEnded,
        // above the running-status row (acceptance criterion 12).
        let text_row = rows
            .iter()
            .position(|row| row.contains("streamed reply words"))
            .unwrap_or_else(|| panic!("mid-turn delta not visible:\n{}", rows.join("\n")));
        let status_row = rows
            .iter()
            .position(|row| row.contains("esc to interrupt"))
            .expect("running status row");
        assert!(
            text_row < status_row,
            "tail above the pane: text row {text_row}, status row {status_row}"
        );
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
        assert!(app.chat_widget.bottom_pane().completion().is_some());
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
