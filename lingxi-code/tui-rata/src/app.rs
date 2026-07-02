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

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use permission::gate::{PermissionRequest, PermissionResponse};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tui_core::message::RenderedMessage;
use tui_core::orchestrator_bridge::TurnEvent;
use tui_core::permission_bridge::PermissionExchange;
use tui_core::theme::Theme;

use crate::composer::Composer;
use crate::overlay::{Dialog, DialogOutcome};
use crate::palette::{command_items, CompletionPopup};
use crate::picker::{ModelPicker, PickerOutcome};
use crate::screens::FullScreen;
use crate::session::SessionInfo;
use crate::terminal::TerminalSession;
use crate::vim::{VimOutcome, VimState};
use crate::{render, RataTerminal};

/// Maximum composer content height before it stops growing and scrolls.
const COMPOSER_MAX_LINES: usize = 6;

/// How long an idle Ctrl-C stays "armed" before a second press quits.
const CTRL_C_EXIT_WINDOW: Duration = Duration::from_secs(2);

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

/// A permission request awaiting the user's decision.
struct PendingPermission {
    dialog: Dialog,
    resp_tx: oneshot::Sender<PermissionResponse>,
}

/// Interactive chat state.
pub struct RataApp {
    messages: Vec<RenderedMessage>,
    composer: Composer,
    theme: Theme,
    /// Count of `messages` already committed (inserted) into the terminal's
    /// native scrollback; the tail beyond this is pending or actively streaming.
    committed: usize,
    /// Cancellation token for the in-flight turn, if any.
    current_turn: Option<CancellationToken>,
    /// Active permission prompt, if any (owns the keyboard while open).
    pending_permission: Option<PendingPermission>,
    /// Active `/model` picker, if any (owns the keyboard).
    active_model_picker: Option<ModelPicker>,
    /// Command/file completion popup shown while a `/command` or `@file` token
    /// is being typed.
    completion: Option<CompletionPopup>,
    /// `true` → expand collapsible messages (thinking body, tool-use JSON,
    /// grouped children) when committing them to scrollback. Toggled by Ctrl-O.
    verbose: bool,
    /// Vim editing state when `/vim` is enabled (`None` → plain editor).
    vim: Option<VimState>,
    /// Wall-clock start, used to advance the streaming spinner animation.
    start: std::time::Instant,
    /// When the in-flight turn began, for the spinner's elapsed-seconds counter.
    turn_started_at: Option<std::time::Instant>,
    /// Human label for what the turn is currently doing (e.g. `Running Bash`),
    /// set from `ToolUseStart` and shown by the spinner instead of a bare verb.
    activity: Option<String>,
    /// First unconfirmed idle Ctrl-C, for claude-code's press-twice-to-exit. A
    /// second Ctrl-C within [`CTRL_C_EXIT_WINDOW`] quits; otherwise it re-arms.
    ctrl_c_at: Option<std::time::Instant>,
    /// Startup snapshot the read-only screens render from.
    session: SessionInfo,
}

impl RataApp {
    /// Build an app seeded with an initial conversation (may be empty).
    #[must_use]
    pub fn new(messages: Vec<RenderedMessage>) -> Self {
        Self {
            messages,
            composer: Composer::default(),
            theme: Theme::dark(),
            committed: 0,
            current_turn: None,
            pending_permission: None,
            active_model_picker: None,
            completion: None,
            verbose: false,
            vim: None,
            start: std::time::Instant::now(),
            turn_started_at: None,
            activity: None,
            ctrl_c_at: None,
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

    fn on_key(&mut self, key: KeyEvent) -> KeyOutcome {
        if self.pending_permission.is_some() {
            return self.on_permission_key(key.code);
        }
        if self.active_model_picker.is_some() {
            return self.on_picker_key(key.code);
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
                    return KeyOutcome::Continue;
                }
                VimOutcome::Submit => {
                    let outcome = self.submit_composer();
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

    /// Take the composer buffer and either run a slash command or emit a
    /// `Submit` for the caller to drive a turn. Shared by the Enter key and
    /// vim's Normal-mode `Enter`.
    fn submit_composer(&mut self) -> KeyOutcome {
        if self.composer.is_blank() {
            return KeyOutcome::Continue;
        }
        let text = self.composer.take();
        let text = text.trim().to_string();
        if let Some(outcome) = self.handle_slash(&text) {
            return outcome;
        }
        self.messages.push(RenderedMessage::UserText {
            body: text.clone(),
            timestamp: 0,
        });
        let token = CancellationToken::new();
        self.current_turn = Some(token.clone());
        KeyOutcome::Submit(text, token)
    }

    /// Handle a bracketed paste. An image file path (existing `.png`/`.jpg`/…)
    /// becomes an image message (auto-selected for preview); anything else is
    /// inserted into the composer at the cursor.
    fn on_paste(&mut self, text: &str) {
        let trimmed = text.trim();
        if is_image_path(trimmed) {
            let name = std::path::Path::new(trimmed)
                .file_name()
                .and_then(|n| n.to_str())
                .map(String::from);
            self.messages.push(RenderedMessage::UserImage {
                image_id: None,
                metadata: name,
                source_path: Some(trimmed.to_string()),
            });
        } else {
            self.composer.insert_str(text);
            self.sync_completion();
        }
    }

    fn on_composer_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let is_ctrl_c = ctrl && matches!(key.code, KeyCode::Char('c'));
        // Any key other than a repeat Ctrl-C disarms the press-twice-to-exit.
        if !is_ctrl_c {
            self.ctrl_c_at = None;
        }
        match key.code {
            KeyCode::Esc => KeyOutcome::Quit,
            // Ctrl-C interrupts an in-flight turn; when idle it arms, and a second
            // press within the window quits (claude-code parity).
            KeyCode::Char('c') if ctrl => {
                if let Some(token) = self.current_turn.take() {
                    token.cancel();
                    self.turn_started_at = None;
                    self.activity = None;
                    KeyOutcome::Continue
                } else if self
                    .ctrl_c_at
                    .is_some_and(|t| t.elapsed() <= CTRL_C_EXIT_WINDOW)
                {
                    KeyOutcome::Quit
                } else {
                    self.ctrl_c_at = Some(std::time::Instant::now());
                    KeyOutcome::Continue
                }
            }
            // Emacs-style composer edits.
            KeyCode::Char('a') if ctrl => {
                self.composer.home();
                KeyOutcome::Continue
            }
            KeyCode::Char('e') if ctrl => {
                self.composer.end();
                KeyOutcome::Continue
            }
            KeyCode::Char('w') if ctrl => {
                self.composer.delete_word();
                KeyOutcome::Continue
            }
            KeyCode::Char('u') if ctrl => {
                self.composer.kill_to_line_start();
                KeyOutcome::Continue
            }
            // Ctrl-O toggles verbose (expand thinking/tool-use/grouped blocks).
            KeyCode::Char('o') if ctrl => {
                self.verbose = !self.verbose;
                KeyOutcome::Continue
            }
            // Modified Enter (Alt/Shift) inserts a newline; plain Enter submits.
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.composer.insert_newline();
                KeyOutcome::Continue
            }
            KeyCode::Enter => self.submit_composer(),
            KeyCode::Home => {
                self.composer.home();
                KeyOutcome::Continue
            }
            KeyCode::End => {
                self.composer.end();
                KeyOutcome::Continue
            }
            KeyCode::Left if ctrl => {
                self.composer.move_word_left();
                KeyOutcome::Continue
            }
            KeyCode::Right if ctrl => {
                self.composer.move_word_right();
                KeyOutcome::Continue
            }
            KeyCode::Left => {
                self.composer.move_left();
                KeyOutcome::Continue
            }
            KeyCode::Right => {
                self.composer.move_right();
                KeyOutcome::Continue
            }
            KeyCode::Up => {
                self.composer.up();
                KeyOutcome::Continue
            }
            KeyCode::Down => {
                self.composer.down();
                KeyOutcome::Continue
            }
            KeyCode::Backspace => {
                self.composer.backspace();
                KeyOutcome::Continue
            }
            KeyCode::Delete => {
                self.composer.delete();
                KeyOutcome::Continue
            }
            KeyCode::Char(c) if !ctrl => {
                self.composer.insert(c);
                KeyOutcome::Continue
            }
            _ => KeyOutcome::Continue,
        }
    }

    /// Fold one streaming event from the orchestrator bridge into the message
    /// list: `TurnStarted` opens an empty assistant reply, `TextDelta` appends
    /// to it, `TurnEnded` clears the in-flight cancel token, and other variants
    /// are ignored for now.
    pub fn apply_turn_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::TurnStarted => {
                self.turn_started_at = Some(std::time::Instant::now());
                self.activity = None;
                self.messages.push(RenderedMessage::AssistantText {
                    body: String::new(),
                    timestamp: 0,
                });
            }
            TurnEvent::TextDelta(delta) => {
                if let Some(RenderedMessage::AssistantText { body, .. }) = self.messages.last_mut()
                {
                    body.push_str(&delta);
                } else {
                    self.messages.push(RenderedMessage::AssistantText {
                        body: delta,
                        timestamp: 0,
                    });
                }
            }
            TurnEvent::ToolUseStart { tool, .. } => {
                self.activity = Some(activity_label(&tool));
            }
            TurnEvent::ToolUseResult { .. } => {
                self.activity = None;
            }
            TurnEvent::TurnEnded(_) => {
                self.current_turn = None;
                self.turn_started_at = None;
                self.activity = None;
            }
            _ => {}
        }
    }

    /// Open a permission prompt for `exchange`; it owns the keyboard until the
    /// user resolves it (Enter/1-3 approve or deny, Esc denies).
    pub fn open_permission(&mut self, exchange: PermissionExchange) {
        let who = exchange
            .worker
            .as_ref()
            .map_or_else(|| "The assistant".to_string(), |w| format!("@{}", w.name));
        let (tool, mut input) = match &exchange.request {
            PermissionRequest::ToolUseConfirm {
                tool_name,
                tool_input,
                ..
            } => (tool_name.clone(), tool_input.to_string()),
            PermissionRequest::ExitPlanMode { plan } => ("ExitPlanMode".to_string(), plan.clone()),
            PermissionRequest::BypassPermissionsMode => {
                ("BypassPermissionsMode".to_string(), String::new())
            }
        };
        if input.chars().count() > 68 {
            input = format!("{}…", input.chars().take(67).collect::<String>());
        }
        let dialog = Dialog::new(
            "Permission required",
            vec![format!("{who} wants to use {tool}:"), input],
            vec![
                "Yes, allow once".to_string(),
                "Yes, allow always".to_string(),
                "No, deny".to_string(),
            ],
        );
        self.pending_permission = Some(PendingPermission {
            dialog,
            resp_tx: exchange.resp_tx,
        });
    }

    fn on_permission_key(&mut self, code: KeyCode) -> KeyOutcome {
        let Some(pending) = self.pending_permission.as_mut() else {
            return KeyOutcome::Continue;
        };
        match pending.dialog.on_key(code) {
            DialogOutcome::Pending => {}
            DialogOutcome::Selected(idx) => {
                let response = match idx {
                    0 => PermissionResponse::AllowOnce,
                    1 => PermissionResponse::AllowAlways,
                    _ => PermissionResponse::Deny,
                };
                self.resolve_permission(response);
            }
            DialogOutcome::Cancelled => self.resolve_permission(PermissionResponse::Deny),
        }
        KeyOutcome::Continue
    }

    fn resolve_permission(&mut self, response: PermissionResponse) {
        if let Some(pending) = self.pending_permission.take() {
            let _ = pending.resp_tx.send(response);
        }
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
            self.messages.push(RenderedMessage::UserImage {
                image_id: None,
                metadata: name,
                source_path: Some(path),
            });
            return Some(KeyOutcome::Continue);
        }
        match trimmed {
            "/help" => {
                self.push_screen_text(&FullScreen::help());
                Some(KeyOutcome::Continue)
            }
            "/doctor" => {
                self.push_screen_text(&FullScreen::doctor(&self.session.doctor));
                Some(KeyOutcome::Continue)
            }
            "/mcp" => {
                self.push_screen_text(&FullScreen::from_rows(
                    "MCP servers",
                    "MCP servers",
                    &self.session.mcp,
                    "No MCP servers configured.",
                ));
                Some(KeyOutcome::Continue)
            }
            "/hooks" => {
                self.push_screen_text(&FullScreen::from_rows(
                    "Hooks",
                    "Hooks",
                    &self.session.hooks,
                    "No hooks configured.",
                ));
                Some(KeyOutcome::Continue)
            }
            "/agents" => {
                self.push_screen_text(&FullScreen::from_rows(
                    "Agents",
                    "Agents",
                    &self.session.agents,
                    "No agents configured.",
                ));
                Some(KeyOutcome::Continue)
            }
            "/clear" => {
                self.messages.clear();
                self.committed = 0;
                Some(KeyOutcome::Continue)
            }
            "/model" => {
                if self.session.models.is_empty() {
                    self.messages.push(RenderedMessage::SystemText {
                        body: "No models available.".to_string(),
                        timestamp: 0,
                        is_error: false,
                    });
                } else {
                    self.active_model_picker = Some(ModelPicker::new(self.session.models.clone()));
                }
                Some(KeyOutcome::Continue)
            }
            "/exit" | "/quit" => Some(KeyOutcome::Quit),
            "/vim" => {
                let now_on = self.vim.is_none();
                self.vim = if now_on { Some(VimState::new()) } else { None };
                self.messages.push(RenderedMessage::SystemText {
                    body: format!("Vim mode {}.", if now_on { "enabled" } else { "disabled" }),
                    timestamp: 0,
                    is_error: false,
                });
                Some(KeyOutcome::Continue)
            }
            _ => None,
        }
    }

    /// Push a read-only screen's content into the conversation scrollback as a
    /// plain-text system message (inline-viewport model: no full-page overlay).
    fn push_screen_text(&mut self, screen: &FullScreen) {
        self.messages.push(RenderedMessage::SystemText {
            body: screen.plain_text(),
            timestamp: 0,
            is_error: false,
        });
    }

    fn on_picker_key(&mut self, code: KeyCode) -> KeyOutcome {
        let Some(picker) = self.active_model_picker.as_mut() else {
            return KeyOutcome::Continue;
        };
        match picker.on_key(code) {
            PickerOutcome::Pending => KeyOutcome::Continue,
            PickerOutcome::Cancelled => {
                self.active_model_picker = None;
                KeyOutcome::Continue
            }
            PickerOutcome::Selected(model, profile) => {
                self.active_model_picker = None;
                self.messages.push(RenderedMessage::SystemText {
                    body: format!("Switching model to {model}…"),
                    timestamp: 0,
                    is_error: false,
                });
                KeyOutcome::SwitchModel(model, profile)
            }
        }
    }

    /// Handle a key while the completion popup is open. Returns `Some(outcome)`
    /// when the popup consumes the key (nav / complete / dismiss), or `None` to
    /// let it fall through to the composer (so typing keeps filtering).
    fn on_completion_key(&mut self, code: KeyCode) -> Option<KeyOutcome> {
        match code {
            KeyCode::Up => {
                self.completion.as_mut()?.prev();
                Some(KeyOutcome::Continue)
            }
            KeyCode::Down => {
                self.completion.as_mut()?.next();
                Some(KeyOutcome::Continue)
            }
            KeyCode::Tab => {
                let insert = self.completion.as_ref()?.selected_insert().to_string();
                // An `@file` token completes in place; a `/command` replaces the
                // whole buffer.
                if let Some((at, _)) = self.composer.at_fragment() {
                    self.composer.complete_at(at, &insert);
                } else {
                    self.composer.replace_all(&insert);
                }
                self.sync_completion();
                Some(KeyOutcome::Continue)
            }
            KeyCode::Esc => {
                self.completion = None;
                Some(KeyOutcome::Continue)
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
            self.completion = CompletionPopup::new(command_items(&text));
            return;
        }
        if let Some((_, fragment)) = self.composer.at_fragment() {
            self.completion = CompletionPopup::new(crate::files::file_completions(&fragment));
            return;
        }
        self.completion = None;
    }

    /// Desired inline-viewport height: status + composer, grown to fit an
    /// active overlay (permission dialog / model picker / completion popup).
    fn viewport_height(&self) -> u16 {
        let composer =
            u16::try_from(self.composer.lines().len().clamp(1, COMPOSER_MAX_LINES)).unwrap_or(1);
        let base = 1 + composer + 2; // status + composer content + border
        let overlay = if self.pending_permission.is_some() {
            9
        } else if self.active_model_picker.is_some() {
            u16::try_from(self.session.models.len())
                .unwrap_or(0)
                .min(12)
                + 4
        } else if self.completion.is_some() {
            base + 8
        } else {
            0
        };
        base.max(overlay).clamp(4, 20)
    }

    /// Commit finalized `messages` into the terminal's native scrollback via
    /// [`crate::terminal::Terminal::insert_history_lines`] (written ABOVE the
    /// bottom viewport). The actively-streaming last message is held back until
    /// the turn ends (it still grows), so it commits once as a whole.
    ///
    /// Generic over the backend so tests can drive it with a test backend; the
    /// runtime passes [`RataTerminal`].
    fn flush_scrollback<B: Backend + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        let width = usize::from(terminal.size()?.width.max(1));
        while self.committed < self.messages.len() {
            let is_last = self.committed + 1 == self.messages.len();
            if is_last && self.current_turn.is_some() {
                break;
            }
            let lines: Vec<Line<'static>> = crate::message::render_message(
                &self.messages[self.committed],
                width,
                &self.theme,
                self.verbose,
            )
            .iter()
            .map(render::styled_line_to_ratatui)
            .collect();
            self.committed += 1;
            if lines.is_empty() {
                continue;
            }
            terminal.insert_history_lines(&lines)?;
        }
        Ok(())
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
    /// scrollback via [`Self::flush_scrollback`]): a status line + the composer
    /// box, with any active overlay (completion / model picker / permission)
    /// drawn on top. Draws into the custom terminal's absolute-rect
    /// [`crate::terminal::Frame`].
    fn render_viewport(&mut self, frame: &mut crate::terminal::Frame) {
        let zones = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(3)])
            .split(frame.area());

        let dim = crate::style_adapter::to_ratatui(self.theme.dim);
        let status: Line = if self.current_turn.is_some() {
            let claude = crate::style_adapter::to_ratatui(self.theme.claude);
            Line::from(vec![
                Span::styled(self.spinner_text(), Style::default().fg(claude)),
                Span::styled(
                    "   ·  Ctrl-C: cancel  ·  Esc: quit",
                    Style::default().fg(dim),
                ),
            ])
        } else if self
            .ctrl_c_at
            .is_some_and(|t| t.elapsed() <= CTRL_C_EXIT_WINDOW)
        {
            let claude = crate::style_adapter::to_ratatui(self.theme.claude);
            Line::from(Span::styled(
                "Press Ctrl-C again to exit",
                Style::default().fg(claude),
            ))
        } else {
            let base = if self.completion.is_some() {
                "↑/↓: pick  ·  Tab: complete  ·  Esc: dismiss  ·  Enter: run"
            } else if self.verbose {
                "Enter: send  ·  Ctrl-O: collapse  ·  ↑/↓: history  ·  Esc: quit"
            } else {
                "Enter: send  ·  Alt+Enter: newline  ·  Ctrl-O: verbose  ·  Esc: quit"
            };
            let text = match &self.vim {
                Some(vim) => format!("[{}]  {base}", vim.label()),
                None => base.to_string(),
            };
            Line::from(Span::styled(text, Style::default().fg(dim)))
        };
        frame.render_widget(Paragraph::new(status), zones[0]);

        let block = Block::new().borders(Borders::ALL);
        let inner = block.inner(zones[1]);
        frame.render_widget(block, zones[1]);
        let composer_lines = self.composer.lines();
        // The first line carries the "> " prompt; wrapped lines align under it.
        let body: Vec<Line> = composer_lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let prefix = if i == 0 { "> " } else { "  " };
                Line::from(format!("{prefix}{l}"))
            })
            .collect();
        let (crow, ccol) = self.composer.cursor_row_col();
        let visible_rows = inner.height as usize;
        let first_row = crow.saturating_sub(visible_rows.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(body).scroll((u16::try_from(first_row).unwrap_or(0), 0)),
            inner,
        );
        let cursor_y = inner.y + u16::try_from(crow - first_row).unwrap_or(0);
        // Cursor X in DISPLAY columns (CJK/wide chars are 2 cols), not char count.
        let before_cursor: String = composer_lines
            .get(crow)
            .map(|l| l.chars().take(ccol).collect())
            .unwrap_or_default();
        let disp_w = unicode_width::UnicodeWidthStr::width(before_cursor.as_str());
        let cursor_x = inner.x + 2 + u16::try_from(disp_w).unwrap_or(0);
        frame.set_cursor_position((
            cursor_x.min(inner.x + inner.width.saturating_sub(1)),
            cursor_y.min(inner.y + inner.height.saturating_sub(1)),
        ));

        // Overlays draw over the viewport.
        if let Some(popup) = &self.completion {
            popup.render(frame, zones[1]);
        }
        if let Some(picker) = &self.active_model_picker {
            picker.render(frame);
        }
        if let Some(pending) = &self.pending_permission {
            pending.dialog.render(frame);
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

/// Whether `s` is a single existing image file path (used to route pastes to an
/// image message vs composer text).
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
        if app.pending_permission.is_none() {
            if let Ok(exchange) = permission_rx.try_recv() {
                app.open_permission(exchange);
            }
        }
        // Size the absolute bottom viewport for this tick, THEN commit
        // finalized history above it (insertion wraps at the viewport width).
        terminal.set_bottom_viewport_height(app.viewport_height())?;
        app.flush_scrollback(terminal)?;
        terminal.draw(|frame| app.render_viewport(frame))?;
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match app.on_key(key) {
                    KeyOutcome::Quit => return Ok(()),
                    KeyOutcome::Submit(prompt, token) => on_submit(prompt, token),
                    KeyOutcome::SwitchModel(model, profile) => on_switch_model(model, profile),
                    KeyOutcome::Continue => {}
                },
                Event::Paste(text) => app.on_paste(&text),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn alt_enter_inserts_newline_plain_enter_submits_whole_buffer() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "line one");
        // Alt+Enter adds a newline instead of submitting.
        let outcome = app.on_key(alt(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Continue));
        typ(&mut app, "line two");
        assert_eq!(app.composer.text(), "line one\nline two");
        // Plain Enter submits the full multi-line buffer.
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Submit(ref p, _) if p == "line one\nline two"));
        assert_eq!(app.composer.text(), "");
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
        assert_eq!(app.composer.text(), "second prompt");
        app.on_key(press(KeyCode::Up));
        assert_eq!(app.composer.text(), "first prompt");
    }

    #[test]
    fn left_arrow_then_typing_inserts_at_cursor() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "ac");
        app.on_key(press(KeyCode::Left)); // between a|c
        app.on_key(press(KeyCode::Char('b')));
        assert_eq!(app.composer.text(), "abc");
    }

    #[test]
    fn typing_then_submit_echoes_user_and_returns_prompt() {
        let mut app = RataApp::new(Vec::new());
        for c in "hi".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        assert_eq!(app.composer.text(), "hi");
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Submit(ref p, _) if p == "hi"));
        assert_eq!(app.composer.text(), "");
        assert!(app.current_turn.is_some());
        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0], RenderedMessage::UserText { .. }));
    }

    #[test]
    fn empty_submit_is_ignored() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            app.on_key(press(KeyCode::Enter)),
            KeyOutcome::Continue
        ));
        assert!(app.messages.is_empty());
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
        match &app.messages[1] {
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
        assert!(app.ctrl_c_at.is_some());
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
        assert!(app.ctrl_c_at.is_some());
        app.on_key(press(KeyCode::Char('h')));
        assert!(app.ctrl_c_at.is_none());
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
        assert_eq!(app.composer.cursor_row_col(), (0, 0));
        app.on_key(press(KeyCode::End));
        assert_eq!(app.composer.cursor_row_col(), (0, 7));
        // Ctrl+W deletes the previous word.
        app.on_key(ctrl(KeyCode::Char('w')));
        assert_eq!(app.composer.text(), "foo ");
        // Ctrl+U kills to line start.
        app.on_key(ctrl(KeyCode::Char('u')));
        assert_eq!(app.composer.text(), "");
    }

    #[test]
    fn ctrl_o_toggles_verbose() {
        let mut app = RataApp::new(Vec::new());
        assert!(!app.verbose);
        app.on_key(ctrl(KeyCode::Char('o')));
        assert!(app.verbose);
        app.on_key(ctrl(KeyCode::Char('o')));
        assert!(!app.verbose);
    }

    #[test]
    fn viewport_height_grows_for_overlays() {
        let mut app = RataApp::new(Vec::new());
        let base = app.viewport_height();
        // Opening the completion popup grows the viewport.
        typ(&mut app, "/");
        assert!(app.completion.is_some());
        assert!(app.viewport_height() > base);
    }

    #[test]
    fn paste_non_image_inserts_into_composer() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "pre ");
        app.on_paste("hello world");
        assert_eq!(app.composer.text(), "pre hello world");
        // A non-existent image path is treated as text, not an image message.
        app.on_paste(" /no/such/file.png ");
        assert!(app.messages.is_empty());
    }

    #[test]
    fn slash_image_pushes_image_message() {
        let mut app = RataApp::new(Vec::new());
        let outcome = submit_command(&mut app, "/image /tmp/pic.png");
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert_eq!(app.messages.len(), 1);
        match &app.messages[0] {
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
        assert!(app.vim.is_none());
        submit_command(&mut app, "/vim");
        assert!(app.vim.is_some());
        submit_command(&mut app, "/vim");
        assert!(app.vim.is_none());
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
        assert_eq!(app.composer.text(), "ello");
        // `i` returns to Insert; typing inserts again.
        app.on_key(press(KeyCode::Char('i')));
        typ(&mut app, "H");
        assert_eq!(app.composer.text(), "Hello");
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
        assert_eq!(app.composer.cursor_row_col(), (0, 6));
        app.on_key(ctrl(KeyCode::Left)); // to start of "alpha"
        assert_eq!(app.composer.cursor_row_col(), (0, 0));
    }

    #[test]
    fn typing_slash_opens_and_filters_command_palette() {
        let mut app = RataApp::new(Vec::new());
        app.on_key(press(KeyCode::Char('/')));
        assert!(app.completion.is_some());
        typ(&mut app, "m"); // "/m" narrows to /model + /mcp
        let p = app.completion.as_ref().unwrap();
        assert_eq!(p.selected_insert(), "/model");
        // A space ends the command token and closes the popup.
        typ(&mut app, " x");
        assert!(app.completion.is_none());
    }

    #[test]
    fn tab_completes_selected_command_into_composer() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "/mc");
        assert!(app.completion.is_some());
        app.on_key(press(KeyCode::Tab));
        assert_eq!(app.composer.text(), "/mcp");
    }

    #[test]
    fn palette_arrows_navigate_and_esc_dismisses_without_quitting() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "/");
        app.on_key(press(KeyCode::Down)); // navigate the popup, not history
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(app.completion.is_none());
        // Composer text is untouched by the dismiss.
        assert_eq!(app.composer.text(), "/");
    }

    #[test]
    fn typing_at_opens_file_completion_and_tab_completes_in_place() {
        let mut app = RataApp::new(Vec::new());
        // `@Carg` should match Cargo.toml in the tui-rata crate cwd.
        typ(&mut app, "see @Carg");
        assert!(app.completion.is_some(), "@ token opens file completion");
        app.on_key(press(KeyCode::Tab));
        // The @token is replaced in place, leaving the prefix intact.
        assert!(
            app.composer.text().starts_with("see @Cargo.toml"),
            "got: {}",
            app.composer.text()
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
        assert!(app.pending_permission.is_some());

        // While a prompt is open, normal keys are swallowed by the dialog and
        // never reach the composer.
        app.on_key(press(KeyCode::Char('x')));
        assert_eq!(app.composer.text(), "");

        // Enter selects the highlighted option (index 0 = AllowOnce).
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(app.pending_permission.is_none());
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
        assert!(app.pending_permission.is_none());
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn permission_prompt_number_three_denies() {
        let mut app = RataApp::new(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        // '3' shortcut = third option = Deny.
        app.on_key(press(KeyCode::Char('3')));
        assert!(app.pending_permission.is_none());
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn slash_help_prints_into_scrollback_without_sending_a_prompt() {
        let mut app = RataApp::new(Vec::new());
        for c in "/help".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        let outcome = app.on_key(press(KeyCode::Enter));
        // Recognized command: no Submit; help text pushed into scrollback.
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert_eq!(app.composer.text(), "");
        assert_eq!(app.messages.len(), 1);
        assert!(matches!(
            app.messages[0],
            RenderedMessage::SystemText { .. }
        ));
        assert!(app.current_turn.is_none());
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
        assert_eq!(app.messages.len(), 1);
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
        assert!(app.messages.is_empty());
        assert_eq!(app.committed, 0);
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
    fn slash_doctor_and_mcp_print_into_scrollback() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/doctor"),
            KeyOutcome::Continue
        ));
        assert!(matches!(
            submit_command(&mut app, "/mcp"),
            KeyOutcome::Continue
        ));
        // Both commands print their content into scrollback as system messages.
        assert_eq!(app.messages.len(), 2);
        assert!(app
            .messages
            .iter()
            .all(|m| matches!(m, RenderedMessage::SystemText { .. })));
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
        assert!(app.active_model_picker.is_none());
        assert_eq!(app.messages.len(), 1);
    }

    #[test]
    fn slash_model_opens_picker_and_enter_switches() {
        let mut app = app_with_models();
        assert!(matches!(
            submit_command(&mut app, "/model"),
            KeyOutcome::Continue
        ));
        assert!(app.active_model_picker.is_some());
        // Picker owns the keyboard: move up to the first (Opus) row and confirm.
        app.on_key(press(KeyCode::Up));
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            KeyOutcome::SwitchModel(ref m, ref p)
                if m == "claude-opus" && p.as_deref() == Some("anthropic")
        ));
        assert!(app.active_model_picker.is_none());
    }

    #[test]
    fn model_picker_esc_cancels_without_switching() {
        let mut app = app_with_models();
        submit_command(&mut app, "/model");
        assert!(app.active_model_picker.is_some());
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(app.active_model_picker.is_none());
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

    /// Draw the bottom viewport at its self-reported height; returns the
    /// terminal for buffer/cursor inspection.
    fn draw_viewport(app: &mut RataApp) -> Terminal<TestWriteBackend> {
        let mut terminal = inline_test_terminal(app.viewport_height());
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
        match &app.messages[0] {
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
        assert!(app.messages.is_empty());
        assert!(app.current_turn.is_none());
    }

    #[test]
    fn slash_hooks_agents_and_quit_route_as_commands() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/hooks"),
            KeyOutcome::Continue
        ));
        assert!(matches!(
            submit_command(&mut app, "/agents"),
            KeyOutcome::Continue
        ));
        assert_eq!(app.messages.len(), 2);
        assert!(app
            .messages
            .iter()
            .all(|m| matches!(m, RenderedMessage::SystemText { .. })));
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
        assert!(app.pending_permission.is_none());
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
        assert!(app.pending_permission.is_none());
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
        // …after which the keyboard belongs to the composer again; further keys
        // cannot re-resolve the consumed exchange (its sender is gone).
        app.on_key(press(KeyCode::Char('x')));
        assert_eq!(app.composer.text(), "x");
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
            app.composer.text(),
            "",
            "image paste must not touch composer"
        );
        assert_eq!(app.messages.len(), 1);
        let file_name = path.file_name().unwrap().to_str().unwrap();
        match &app.messages[0] {
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
        assert_eq!(app.committed, 1);
        app.apply_turn_event(TurnEvent::TextDelta("lo".to_string()));
        app.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(app.committed, 1, "still streaming: tail stays held back");
        app.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        app.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(app.committed, 2, "turn ended: reply commits as a whole");
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
        assert_eq!(app.committed, 2);
    }

    #[test]
    fn viewport_grows_with_multiline_composer_up_to_cap() {
        let mut app = RataApp::new(Vec::new());
        typ(&mut app, "one");
        for _ in 0..9 {
            app.on_key(alt(KeyCode::Enter));
        }
        // 10 content lines clamp at COMPOSER_MAX_LINES (6): 1 + 6 + 2 = 9.
        assert_eq!(app.viewport_height(), 9);
    }

    #[test]
    fn layout_80x24_idle_status_line_plus_bordered_composer() {
        let mut app = RataApp::new(Vec::new());
        assert_eq!(app.viewport_height(), 4, "idle bottom viewport is 4 rows");
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
        assert!(app.completion.is_some());
        assert_eq!(app.viewport_height(), 12, "completion viewport height");
        let terminal = draw_viewport(&mut app);
        let all = buffer_rows(&terminal).join("\n");
        assert!(all.contains("Complete"), "popup title visible:\n{all}");
    }

    #[test]
    fn layout_permission_dialog_overlays_viewport() {
        let mut app = RataApp::new(Vec::new());
        let (exchange, _resp_rx) = tool_exchange();
        app.open_permission(exchange);
        assert_eq!(app.viewport_height(), 9, "permission viewport height");
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
        assert_eq!(app.viewport_height(), 6, "picker viewport height");
        let terminal = draw_viewport(&mut app);
        let all = buffer_rows(&terminal).join("\n");
        assert!(all.contains("Select model"), "{all}");
        assert!(all.contains("Opus"), "{all}");
        assert!(all.contains("Sonnet"), "{all}");
    }
}
