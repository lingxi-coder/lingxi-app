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

use std::collections::HashSet;
use std::collections::HashMap;
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::StatefulImage;
use permission::gate::{PermissionRequest, PermissionResponse};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tui_core::message::RenderedMessage;
use tui_core::orchestrator_bridge::TurnEvent;
use tui_core::permission_bridge::PermissionExchange;
use tui_core::render::{NamedColor, SpanStyle, StyleColor, StyledSpan};
use tui_core::theme::Theme;

use crate::composer::Composer;
use crate::overlay::{Dialog, DialogOutcome};
use crate::palette::{command_items, CompletionPopup};
use crate::picker::{ModelPicker, PickerOutcome};
use crate::screens::{FullScreen, ScreenOutcome};
use crate::session::SessionInfo;
use crate::vim::{VimOutcome, VimState};
use crate::{render, restore_terminal, setup_terminal, RataTerminal};

/// Lines moved per PageUp / PageDown.
const PAGE: u16 = 10;

/// Maximum composer content height before it stops growing and scrolls.
const COMPOSER_MAX_LINES: usize = 6;

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
    /// Lines scrolled up from the bottom (`0` = stuck to the latest output).
    scroll_up: u16,
    /// Cancellation token for the in-flight turn, if any.
    current_turn: Option<CancellationToken>,
    /// Active permission prompt, if any (owns the keyboard while open).
    pending_permission: Option<PendingPermission>,
    /// Active full-page screen (e.g. `/help`), if any (owns the keyboard).
    active_screen: Option<FullScreen>,
    /// Active `/model` picker, if any (owns the keyboard).
    active_model_picker: Option<ModelPicker>,
    /// Command/file completion popup shown while a `/command` or `@file` token
    /// is being typed.
    completion: Option<CompletionPopup>,
    /// `true` → expand collapsible messages (thinking body, tool-use JSON,
    /// grouped children). Toggled by Ctrl-O.
    verbose: bool,
    /// Vim editing state when `/vim` is enabled (`None` → plain editor).
    vim: Option<VimState>,
    /// Selected scrollback message index while browsing (`None` → composer
    /// focused). Ctrl-Up enters; Ctrl-Down past the end exits.
    selected_msg: Option<usize>,
    /// Message indices individually expanded via the selection cursor (an
    /// override on top of the global `verbose` flag).
    expanded_msgs: HashSet<usize>,
    /// Terminal graphics picker (kitty/iTerm2/sixel), set after terminal setup;
    /// `None` in tests and non-tty sessions.
    picker: Option<Picker>,
    /// Lazily-built inline-image protocols, keyed by message index.
    image_previews: HashMap<usize, StatefulProtocol>,
    /// Startup snapshot the full-page screens render from.
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
            scroll_up: 0,
            current_turn: None,
            pending_permission: None,
            active_screen: None,
            active_model_picker: None,
            completion: None,
            verbose: false,
            vim: None,
            selected_msg: None,
            expanded_msgs: HashSet::new(),
            picker: None,
            image_previews: HashMap::new(),
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
        if self.active_screen.is_some() {
            return self.on_screen_key(key.code);
        }
        if self.active_model_picker.is_some() {
            return self.on_picker_key(key.code);
        }
        if self.completion.is_some() {
            if let Some(outcome) = self.on_completion_key(key.code) {
                return outcome;
            }
        }
        // Scrollback selection mode owns the keyboard once active.
        if self.selected_msg.is_some() {
            return self.on_selection_key(key.code);
        }
        // Ctrl-Up enters selection mode on the last message.
        if key.code == KeyCode::Up && key.modifiers.contains(KeyModifiers::CONTROL) {
            if !self.messages.is_empty() {
                self.selected_msg = Some(self.messages.len() - 1);
                self.scroll_up = 0;
            }
            return KeyOutcome::Continue;
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
        self.scroll_up = 0;
        self.messages.push(RenderedMessage::UserText {
            body: text.clone(),
            timestamp: 0,
        });
        let token = CancellationToken::new();
        self.current_turn = Some(token.clone());
        KeyOutcome::Submit(text, token)
    }

    fn on_composer_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => KeyOutcome::Quit,
            // Ctrl-C cancels an in-flight turn; with nothing running it exits.
            KeyCode::Char('c') if ctrl => {
                if let Some(token) = self.current_turn.take() {
                    token.cancel();
                    KeyOutcome::Continue
                } else {
                    KeyOutcome::Quit
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
            KeyCode::Enter if key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) => {
                self.composer.insert_newline();
                KeyOutcome::Continue
            }
            KeyCode::Enter => self.submit_composer(),
            KeyCode::PageUp => {
                self.scroll_up = self.scroll_up.saturating_add(PAGE);
                KeyOutcome::Continue
            }
            KeyCode::PageDown => {
                self.scroll_up = self.scroll_up.saturating_sub(PAGE);
                KeyOutcome::Continue
            }
            KeyCode::Home if ctrl => {
                self.scroll_up = u16::MAX;
                KeyOutcome::Continue
            }
            KeyCode::End if ctrl => {
                self.scroll_up = 0;
                KeyOutcome::Continue
            }
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
                self.messages.push(RenderedMessage::AssistantText {
                    body: String::new(),
                    timestamp: 0,
                });
            }
            TurnEvent::TextDelta(delta) => {
                if let Some(RenderedMessage::AssistantText { body, .. }) = self.messages.last_mut() {
                    body.push_str(&delta);
                } else {
                    self.messages.push(RenderedMessage::AssistantText {
                        body: delta,
                        timestamp: 0,
                    });
                }
            }
            TurnEvent::TurnEnded(_) => {
                self.current_turn = None;
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
            self.selected_msg = Some(self.messages.len() - 1);
            self.scroll_up = 0;
            return Some(KeyOutcome::Continue);
        }
        match trimmed {
            "/help" => {
                self.active_screen = Some(FullScreen::help());
                Some(KeyOutcome::Continue)
            }
            "/doctor" => {
                self.active_screen = Some(FullScreen::doctor(&self.session.doctor));
                Some(KeyOutcome::Continue)
            }
            "/mcp" => {
                self.active_screen = Some(FullScreen::from_rows(
                    "MCP servers",
                    "MCP servers",
                    &self.session.mcp,
                    "No MCP servers configured.",
                ));
                Some(KeyOutcome::Continue)
            }
            "/hooks" => {
                self.active_screen = Some(FullScreen::from_rows(
                    "Hooks",
                    "Hooks",
                    &self.session.hooks,
                    "No hooks configured.",
                ));
                Some(KeyOutcome::Continue)
            }
            "/agents" => {
                self.active_screen = Some(FullScreen::from_rows(
                    "Agents",
                    "Agents",
                    &self.session.agents,
                    "No agents configured.",
                ));
                Some(KeyOutcome::Continue)
            }
            "/clear" => {
                self.messages.clear();
                self.scroll_up = 0;
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
                    self.active_model_picker =
                        Some(ModelPicker::new(self.session.models.clone()));
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

    fn on_screen_key(&mut self, code: KeyCode) -> KeyOutcome {
        if let Some(screen) = self.active_screen.as_mut() {
            if screen.on_key(code) == ScreenOutcome::Close {
                self.active_screen = None;
            }
        }
        KeyOutcome::Continue
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

    /// Handle a key while a scrollback message is selected: Up/Down move the
    /// selection (down past the end exits), Enter/Space toggle its expansion,
    /// Esc exits back to the composer.
    fn on_selection_key(&mut self, code: KeyCode) -> KeyOutcome {
        let Some(sel) = self.selected_msg else {
            return KeyOutcome::Continue;
        };
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected_msg = Some(sel.saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if sel + 1 < self.messages.len() {
                    self.selected_msg = Some(sel + 1);
                } else {
                    self.selected_msg = None;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if !self.expanded_msgs.remove(&sel) {
                    self.expanded_msgs.insert(sel);
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.selected_msg = None;
            }
            _ => {}
        }
        KeyOutcome::Continue
    }

    fn scrollback_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        for (i, m) in self.messages.iter().enumerate() {
            let verbose = self.verbose || self.expanded_msgs.contains(&i);
            let mut lines = crate::message::render_message(m, width, &self.theme, verbose);
            if Some(i) == self.selected_msg {
                mark_selected(&mut lines);
            }
            out.extend(lines.iter().map(render::styled_line_to_ratatui));
        }
        out
    }

    /// Draw the selected image message's real pixels in a centered preview
    /// pane (graphics terminals only; no-op otherwise). Lazily decodes the file
    /// into a protocol on first display.
    fn render_image_preview(&mut self, frame: &mut ratatui::Frame) {
        let Some(sel) = self.selected_msg else {
            return;
        };
        // Extract the path, releasing the borrow on `messages`.
        let path = match self.messages.get(sel) {
            Some(RenderedMessage::UserImage {
                source_path: Some(p),
                ..
            }) => p.clone(),
            _ => return,
        };
        // Lazily decode the image into a protocol the first time it's shown.
        if !self.image_previews.contains_key(&sel) {
            let Some(picker) = self.picker.as_ref() else {
                return;
            };
            match crate::image_view::load_protocol(picker, std::path::Path::new(&path)) {
                Some(proto) => {
                    self.image_previews.insert(sel, proto);
                }
                None => return,
            }
        }
        let Some(proto) = self.image_previews.get_mut(&sel) else {
            return;
        };
        let a = frame.area();
        let area = crate::overlay::centered_rect(a.width * 3 / 4, a.height * 3 / 4, a);
        frame.render_widget(Clear, area);
        let block = Block::new()
            .borders(Borders::ALL)
            .title("Image preview · Esc: close");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_stateful_widget(StatefulImage::new(), inner, proto);
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        // The composer grows with its line count (capped), so compute its
        // height before splitting the layout.
        let composer_lines = self.composer.lines();
        let content_h = u16::try_from(composer_lines.len().clamp(1, COMPOSER_MAX_LINES))
            .unwrap_or(1);
        let zones = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(content_h + 2),
            ])
            .split(frame.area());

        let width = zones[0].width as usize;
        let lines = self.scrollback_lines(width.max(1));
        let view_h = zones[0].height as usize;
        let max_scroll = u16::try_from(lines.len().saturating_sub(view_h)).unwrap_or(u16::MAX);
        let scroll = max_scroll.saturating_sub(self.scroll_up);
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), zones[0]);

        let base = if self.current_turn.is_some() {
            "streaming…  ·  Ctrl-C: cancel  ·  PgUp/PgDn: scroll  ·  Esc: quit"
        } else if self.selected_msg.is_some() {
            "↑/↓: select  ·  Enter/Space: expand  ·  Esc: back to composer"
        } else if self.completion.is_some() {
            "↑/↓: pick  ·  Tab: complete  ·  Esc: dismiss  ·  Enter: run"
        } else if self.verbose {
            "Enter: send  ·  Ctrl-O: collapse  ·  ↑/↓: history  ·  Esc: quit"
        } else {
            "Enter: send  ·  Alt+Enter: newline  ·  Ctrl-O: expand  ·  Esc: quit"
        };
        let status = match &self.vim {
            Some(vim) => format!("[{}]  {base}", vim.label()),
            None => base.to_string(),
        };
        frame.render_widget(Paragraph::new(status), zones[1]);

        let block = Block::new().borders(Borders::ALL);
        let inner = block.inner(zones[2]);
        frame.render_widget(block, zones[2]);
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
        let cursor_x = inner.x + 2 + u16::try_from(ccol).unwrap_or(0);
        frame.set_cursor_position((
            cursor_x.min(inner.x + inner.width.saturating_sub(1)),
            cursor_y.min(inner.y + inner.height.saturating_sub(1)),
        ));

        // The completion popup sits just above the composer box.
        if let Some(popup) = &self.completion {
            popup.render(frame, zones[2]);
        }

        // A selected image message displays its real pixels in a preview pane
        // (graphics terminals only); everything else draws on top.
        self.render_image_preview(frame);

        // A full-page screen (e.g. /help) draws over the base layout; a
        // permission prompt draws over everything and owns the keyboard.
        if let Some(screen) = &self.active_screen {
            screen.render(frame);
        }
        if let Some(picker) = &self.active_model_picker {
            picker.render(frame);
        }
        if let Some(pending) = &self.pending_permission {
            pending.dialog.render(frame);
        }
    }
}

/// Prefix a selected message's lines with a bright gutter marker (`▸ ` on the
/// first line, `│ ` on wrapped lines) so the selection is visible in the flat
/// scrollback.
fn mark_selected(lines: &mut [tui_core::render::StyledLine]) {
    let style = SpanStyle {
        fg: StyleColor::Named(NamedColor::BrightCyan),
        bold: true,
        ..SpanStyle::default()
    };
    for (i, line) in lines.iter_mut().enumerate() {
        let marker = if i == 0 { "▸ " } else { "│ " };
        line.spans.insert(0, StyledSpan::styled(marker, style));
    }
}

/// Run the interactive chat app: set up the terminal, loop until the user
/// quits — draining `events_rx` each tick and invoking `on_submit(prompt,
/// token)` when the user sends a message — then restore the terminal.
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
    let mut terminal = setup_terminal()?;
    let mut app = RataApp::new(messages).with_session(session);
    // Query the terminal for graphics support now that raw mode is on, so
    // selected image messages can display real pixels.
    app.picker = Some(crate::image_view::make_picker());
    let result = app_loop(
        &mut terminal,
        &mut app,
        &mut events_rx,
        &mut permission_rx,
        &mut on_submit,
        &mut on_switch_model,
    );
    restore_terminal(&mut terminal)?;
    result
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
        terminal.draw(|frame| app.render(frame))?;
        while let Ok(event) = events_rx.try_recv() {
            app.apply_turn_event(event);
        }
        // Take a new permission request only when none is currently shown.
        if app.pending_permission.is_none() {
            if let Ok(exchange) = permission_rx.try_recv() {
                app.open_permission(exchange);
            }
        }
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match app.on_key(key) {
                        KeyOutcome::Quit => return Ok(()),
                        KeyOutcome::Submit(prompt, token) => on_submit(prompt, token),
                        KeyOutcome::SwitchModel(model, profile) => on_switch_model(model, profile),
                        KeyOutcome::Continue => {}
                    }
                }
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
        assert!(matches!(app.on_key(press(KeyCode::Enter)), KeyOutcome::Continue));
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
    fn ctrl_c_cancels_turn_then_quits() {
        let mut app = RataApp::new(Vec::new());
        app.on_key(press(KeyCode::Char('x')));
        app.on_key(press(KeyCode::Enter));
        let token = app.current_turn.clone().unwrap();
        assert!(!token.is_cancelled());
        // First Ctrl-C cancels the in-flight turn.
        assert!(matches!(app.on_key(ctrl(KeyCode::Char('c'))), KeyOutcome::Continue));
        assert!(token.is_cancelled());
        assert!(app.current_turn.is_none());
        // Second Ctrl-C (nothing running) quits.
        assert!(matches!(app.on_key(ctrl(KeyCode::Char('c'))), KeyOutcome::Quit));
    }

    #[test]
    fn page_keys_move_scroll_offset() {
        let mut app = RataApp::new(Vec::new());
        app.on_key(press(KeyCode::PageUp));
        assert_eq!(app.scroll_up, PAGE);
        // Scroll-to-bottom moved to Ctrl+End (bare End is composer line-end now).
        app.on_key(ctrl(KeyCode::End));
        assert_eq!(app.scroll_up, 0);
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

    fn app_with_messages(n: usize) -> RataApp {
        let msgs = (0..n)
            .map(|i| RenderedMessage::SystemText {
                body: format!("msg {i}"),
                timestamp: 0,
                is_error: false,
            })
            .collect();
        RataApp::new(msgs)
    }

    #[test]
    fn ctrl_up_enters_selection_on_last_message() {
        let mut app = app_with_messages(3);
        assert!(app.selected_msg.is_none());
        app.on_key(ctrl(KeyCode::Up));
        assert_eq!(app.selected_msg, Some(2));
    }

    #[test]
    fn selection_moves_and_exits_at_bottom() {
        let mut app = app_with_messages(3);
        app.on_key(ctrl(KeyCode::Up)); // select 2
        app.on_key(press(KeyCode::Up)); // → 1
        assert_eq!(app.selected_msg, Some(1));
        app.on_key(press(KeyCode::Down)); // → 2
        app.on_key(press(KeyCode::Down)); // past end → exit
        assert!(app.selected_msg.is_none());
    }

    #[test]
    fn enter_toggles_selected_message_expansion() {
        let mut app = app_with_messages(2);
        app.on_key(ctrl(KeyCode::Up)); // select 1
        assert!(!app.expanded_msgs.contains(&1));
        app.on_key(press(KeyCode::Enter));
        assert!(app.expanded_msgs.contains(&1));
        app.on_key(press(KeyCode::Enter));
        assert!(!app.expanded_msgs.contains(&1));
    }

    #[test]
    fn esc_exits_selection_without_quitting() {
        let mut app = app_with_messages(2);
        app.on_key(ctrl(KeyCode::Up));
        assert!(app.selected_msg.is_some());
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(app.selected_msg.is_none());
    }

    #[test]
    fn slash_image_pushes_image_message_and_selects_it() {
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
        // It is auto-selected so a graphics terminal previews it immediately.
        assert_eq!(app.selected_msg, Some(0));
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
        (PermissionExchange { request, resp_tx, worker: None }, resp_rx)
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
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::AllowOnce);
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
    fn slash_help_opens_screen_without_sending_a_prompt() {
        let mut app = RataApp::new(Vec::new());
        for c in "/help".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        let outcome = app.on_key(press(KeyCode::Enter));
        // Recognized screen command: no Submit, no user message pushed.
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(app.active_screen.is_some());
        assert_eq!(app.composer.text(), "");
        assert!(app.messages.is_empty());
        assert!(app.current_turn.is_none());
    }

    #[test]
    fn open_screen_owns_keyboard_and_esc_closes() {
        let mut app = RataApp::new(Vec::new());
        for c in "/help".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        app.on_key(press(KeyCode::Enter));
        assert!(app.active_screen.is_some());
        // Typing is swallowed by the screen, never reaching the composer.
        app.on_key(press(KeyCode::Char('x')));
        assert_eq!(app.composer.text(), "");
        // Esc closes the screen (does NOT quit the app).
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(app.active_screen.is_none());
        // With the screen closed, Esc now quits as normal.
        assert!(matches!(app.on_key(press(KeyCode::Esc)), KeyOutcome::Quit));
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
        assert!(app.active_screen.is_none());
        assert_eq!(app.messages.len(), 1);
    }

    fn submit_command(app: &mut RataApp, cmd: &str) -> KeyOutcome {
        for c in cmd.chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        app.on_key(press(KeyCode::Enter))
    }

    #[test]
    fn slash_clear_empties_scrollback_without_a_screen() {
        let mut app = RataApp::new(vec![RenderedMessage::SystemText {
            body: "old".to_string(),
            timestamp: 0,
            is_error: false,
        }]);
        let outcome = submit_command(&mut app, "/clear");
        assert!(matches!(outcome, KeyOutcome::Continue));
        assert!(app.messages.is_empty());
        assert!(app.active_screen.is_none());
    }

    #[test]
    fn slash_exit_quits() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(submit_command(&mut app, "/exit"), KeyOutcome::Quit));
    }

    #[test]
    fn slash_doctor_and_mcp_open_screens() {
        let mut app = RataApp::new(Vec::new());
        assert!(matches!(submit_command(&mut app, "/doctor"), KeyOutcome::Continue));
        assert!(app.active_screen.is_some());
        // Close, then open another data screen.
        app.on_key(press(KeyCode::Esc));
        assert!(app.active_screen.is_none());
        assert!(matches!(submit_command(&mut app, "/mcp"), KeyOutcome::Continue));
        assert!(app.active_screen.is_some());
        assert!(app.messages.is_empty());
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
        assert!(matches!(submit_command(&mut app, "/model"), KeyOutcome::Continue));
        assert!(app.active_model_picker.is_none());
        assert_eq!(app.messages.len(), 1);
    }

    #[test]
    fn slash_model_opens_picker_and_enter_switches() {
        let mut app = app_with_models();
        assert!(matches!(submit_command(&mut app, "/model"), KeyOutcome::Continue));
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
}
