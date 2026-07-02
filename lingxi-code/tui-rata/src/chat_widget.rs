//! The main chat surface: transcript + bottom pane + per-turn state (plan
//! Phase 6).
//!
//! Modeled on codex-rs `tui/src/chatwidget.rs` (UI architecture pattern only —
//! no codex product types; `LingXi` keeps its own `RenderedMessage` /
//! [`SessionInfo`] / [`TurnEvent`] / [`PermissionExchange`] data model).
//! [`ChatWidget`] consumes orchestrator [`TurnEvent`]s into the
//! [`Transcript`]'s committed/active history cells, routes keys and pastes
//! through the [`BottomPane`] and maps the pane's local outcomes to app-level
//! [`ChatOutcome`]s, dispatches slash commands through the [`crate::command`]
//! registry, and serializes permission prompts through a pending queue (one
//! prompt owns the keyboard; later arrivals wait their turn).
//!
//! Its render surface is the exact four-method contract codex's `App` draws
//! its chat widget through (`codex-rs/tui/src/app.rs::render_chat_widget_frame`):
//! [`ChatWidget::desired_height`], [`ChatWidget::render`],
//! [`ChatWidget::cursor_pos`], and [`ChatWidget::cursor_style`].

use std::collections::VecDeque;
use std::io;
use std::io::Write;

use crossterm::cursor::SetCursorStyle;
use crossterm::event::KeyEvent;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tokio_util::sync::CancellationToken;
use tui_core::message::RenderedMessage;
use tui_core::orchestrator_bridge::TurnEvent;
use tui_core::permission_bridge::PermissionExchange;
use tui_core::theme::{theme_for, Theme, ThemeName, ThemeSetting};

use crate::bottom_pane::permission_view::PermissionView;
use crate::bottom_pane::screen_view::ScreenView;
use crate::bottom_pane::theme_picker_view::ThemePickerView;
use crate::bottom_pane::{BottomPane, BottomPaneOutcome, BottomPaneStatus, CommandAction};
use crate::history_cell::message::AssistantTextCell;
use crate::renderable::Renderable;
use crate::session::SessionInfo;
use crate::transcript::Transcript;

/// What one routed key press or paste means to the owning event loop.
pub enum ChatOutcome {
    /// Keep looping.
    Continue,
    /// Exit the app.
    Quit,
    /// The user submitted `prompt`; the caller should drive a turn for it,
    /// honoring the paired [`CancellationToken`] (the widget cancels it on
    /// Ctrl-C).
    Submit(String, CancellationToken),
    /// The user picked a model in `/model`; the caller should switch to
    /// `(request_model, profile)` via `OrchestratorHandle::switch_model`.
    SwitchModel(String, Option<String>),
    /// The user committed a theme in the `/theme` picker. The widget has
    /// already applied it live; the caller should persist the setting
    /// (best-effort, `tui_core::theme_persist`).
    SetTheme(ThemeSetting),
    /// `/copy` resolved this text; the caller should write it to the system
    /// clipboard (best-effort, [`crate::copy::copy_to_clipboard_native`]).
    /// The confirmation message is already in the transcript.
    CopyToClipboard(String),
}

/// The chat surface: owns the conversation state and the interactive footer,
/// leaving only loop plumbing (terminal, channels, callbacks) to the app.
pub struct ChatWidget {
    /// Conversation history: committed cells + the active streaming cell +
    /// the native-scrollback commit cursor + verbose/render mode.
    transcript: Transcript,
    /// The interactive footer: composer + completion + vim + status hints +
    /// the transient view stack. The pane routes local input; app-level
    /// intents come back as [`BottomPaneOutcome`]s the widget maps to
    /// [`ChatOutcome`]s.
    bottom_pane: BottomPane,
    /// Startup snapshot the read-only screens and the model picker render from.
    session: SessionInfo,
    /// Theme for transcript rendering (native-scrollback flush).
    theme: Theme,
    /// The active theme *preference* (drives the `/theme` picker's current
    /// marker; `Auto` re-resolves on apply).
    theme_setting: ThemeSetting,
    /// Resolved name of the active theme (the `/status`/`/config` rows).
    theme_name: ThemeName,
    /// Cancellation token for the in-flight turn, if any.
    current_turn: Option<CancellationToken>,
    /// When the in-flight turn began, for the spinner's elapsed-seconds counter.
    turn_started_at: Option<std::time::Instant>,
    /// Human label for what the turn is currently doing (e.g. `Running Bash`),
    /// set from `ToolUseStart` and shown by the spinner instead of a bare verb.
    activity: Option<String>,
    /// Permission requests waiting for the currently open prompt to resolve
    /// (prompts are serialized: one owns the keyboard at a time).
    pending_permissions: VecDeque<PermissionExchange>,
    /// Widget-lifetime clock driving the spinner's animation frame (and the
    /// `/stats` session-duration row).
    start: std::time::Instant,
    /// Where `/export` writes transcripts (default `~/.lingxi/exports`;
    /// overridable so tests and embedders stay hermetic).
    export_dir: std::path::PathBuf,
}

impl ChatWidget {
    /// Build a widget seeded with an initial conversation (may be empty) and
    /// the startup [`SessionInfo`] snapshot.
    #[must_use]
    pub fn new(messages: Vec<RenderedMessage>, session: SessionInfo) -> Self {
        let theme = Theme::dark();
        Self {
            transcript: Transcript::from_messages(messages),
            bottom_pane: BottomPane::new(theme),
            session,
            theme,
            theme_setting: ThemeSetting::Named(ThemeName::Dark),
            theme_name: ThemeName::Dark,
            current_turn: None,
            turn_started_at: None,
            activity: None,
            pending_permissions: VecDeque::new(),
            start: std::time::Instant::now(),
            export_dir: crate::export::default_export_dir(),
        }
    }

    /// Override where `/export` writes transcripts (tests/embedders; the
    /// default is [`crate::export::default_export_dir`]).
    pub fn set_export_dir(&mut self, dir: std::path::PathBuf) {
        self.export_dir = dir;
    }

    /// Replace the startup [`SessionInfo`] snapshot the screens and the
    /// model picker render from.
    pub fn set_session(&mut self, session: SessionInfo) {
        self.session = session;
    }

    /// Apply a theme preference live: resolve it (`Auto` consults the OSC-11
    /// cache / `$COLORFGBG` / color depth) and swap the render palette on the
    /// transcript flush path and the bottom pane. Used by the startup theme
    /// load in [`crate::app::run_app`] and the `/theme` picker commit.
    pub fn set_theme(&mut self, setting: ThemeSetting) {
        self.theme_setting = setting;
        self.theme_name = setting.resolve();
        self.theme = theme_for(self.theme_name);
        self.bottom_pane.set_theme(self.theme);
    }

    /// The active theme preference (`/theme` picker current marker; tests).
    #[must_use]
    pub fn theme_setting(&self) -> ThemeSetting {
        self.theme_setting
    }

    /// The resolved name of the active theme (`/status` row; tests).
    #[must_use]
    pub fn theme_name(&self) -> ThemeName {
        self.theme_name
    }

    /// Route one key press: feed the pane the current turn status (Ctrl-C
    /// routing depends on it), route through the pane's layered input
    /// handling (active view, completion, vim, composer), execute the
    /// returned intent, then surface the next queued permission if the key
    /// resolved the open prompt.
    pub fn handle_key(&mut self, key: KeyEvent) -> ChatOutcome {
        self.bottom_pane.set_task_running(self.pane_status());
        let outcome = self.bottom_pane.handle_key(key);
        let outcome = self.on_pane_outcome(outcome);
        self.open_next_queued_permission();
        outcome
    }

    /// Route a bracketed paste through the pane (active view first, then
    /// image-path detection, then composer insertion).
    pub fn handle_paste(&mut self, text: &str) -> ChatOutcome {
        let outcome = self.bottom_pane.handle_paste(text);
        let outcome = self.on_pane_outcome(outcome);
        self.open_next_queued_permission();
        outcome
    }

    /// Fold one streaming event from the orchestrator bridge into the
    /// transcript: `TurnStarted` opens an empty active assistant cell,
    /// `TextDelta` mutates it in place, `ToolUseStart`/`ToolUseResult` set and
    /// clear the spinner activity, `TurnEnded` finalizes the active cell
    /// (moves it to the committed history) and clears the in-flight cancel
    /// token; other variants are ignored for now.
    pub fn apply_turn_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::TurnStarted => {
                self.turn_started_at = Some(std::time::Instant::now());
                self.activity = None;
                // A straggler active cell (missed TurnEnded) is finalized, not
                // dropped, before the new streaming reply opens.
                self.transcript.flush_active();
                self.transcript
                    .set_active(Box::new(AssistantTextCell::new(String::new())));
            }
            TurnEvent::TextDelta(delta) => {
                let appended = self
                    .transcript
                    .mutate_active(|cell| {
                        if let Some(assistant) =
                            cell.as_any_mut().downcast_mut::<AssistantTextCell>()
                        {
                            assistant.append(&delta);
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !appended {
                    self.transcript.flush_active();
                    self.transcript
                        .set_active(Box::new(AssistantTextCell::new(delta)));
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
    /// through the exchange's one-shot channel exactly once. When a prompt is
    /// already open the exchange queues instead — prompts are serialized, and
    /// the queued one surfaces as soon as the open prompt resolves.
    pub fn open_permission(&mut self, exchange: PermissionExchange) {
        if self.has_open_permission() {
            self.pending_permissions.push_back(exchange);
        } else {
            self.bottom_pane.show_permission(exchange);
        }
    }

    /// Whether a permission prompt is anywhere on the view stack (queued
    /// exchanges wait in [`Self::open_permission`]'s queue until it resolves).
    #[must_use]
    pub fn has_open_permission(&self) -> bool {
        self.bottom_pane.view_stack().contains::<PermissionView>()
    }

    /// Route a recognized slash command through the [`crate::command`]
    /// registry. Returns `Some(outcome)` when the input resolves to a
    /// registered command (screen open, clear, exit, …), or `None` to fall
    /// through and send the input as a normal prompt.
    pub fn handle_slash(&mut self, input: &str) -> Option<ChatOutcome> {
        let (command, args) = crate::command::resolve(input)?;
        Some((command.run)(self, args))
    }

    /// Desired widget height at `width` columns: the streaming live tail (the
    /// active cell's rendered lines — zero when idle) stacked above the pane's
    /// own height (status + composer, grown to fit the active stacked view or
    /// the completion popup). The owner applies its viewport clamp policy.
    #[must_use]
    pub fn desired_height(&self, width: u16) -> u16 {
        self.live_tail_height(width)
            .saturating_add(self.bottom_pane.desired_height(width))
    }

    /// Draw the widget into `area` of `buf`: refreshes the pane's task status
    /// first so the spinner text/animation reflect this tick's turn state,
    /// then draws the streaming live tail ABOVE the pane (this is the only
    /// render path for in-flight text — it is never committed to native
    /// scrollback until finalized), then the pane (status line + composer box
    /// + overlays) pinned at the bottom.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.bottom_pane.set_task_running(self.pane_status());
        let tail = self.live_tail(area.width);
        let (tail_area, pane_area) = self.split_area(area, line_count(&tail));
        // A clipped tail shows its LAST rows (it is a tail: the newest
        // streamed lines stay visible next to the pane).
        let skip = tail.len().saturating_sub(usize::from(tail_area.height));
        for (row, line) in tail.iter().skip(skip).enumerate() {
            let row_area = Rect::new(
                tail_area.x,
                tail_area.y + u16::try_from(row).unwrap_or(u16::MAX),
                tail_area.width,
                1,
            );
            Renderable::render(line, row_area, buf);
        }
        self.bottom_pane.render(pane_area, buf);
    }

    /// The widget's cursor claim within `area` (the composer's cursor, or a
    /// full-frame view's; `None` → cursor hidden), positioned within the
    /// pane's sub-area below the live tail.
    #[must_use]
    pub fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let (_, pane_area) = self.split_area(area, self.live_tail_height(area.width));
        self.bottom_pane.cursor_pos(pane_area)
    }

    /// The cursor style applied when [`Self::cursor_pos`] claims a cursor.
    #[must_use]
    pub fn cursor_style(&self, area: Rect) -> SetCursorStyle {
        let (_, pane_area) = self.split_area(area, self.live_tail_height(area.width));
        self.bottom_pane.cursor_style(pane_area)
    }

    /// Draw the widget into one terminal [`crate::terminal::Frame`] through
    /// its own render contract — the thin adapter codex keeps at the terminal
    /// draw boundary (`App::render_chat_widget_frame`): render into the
    /// frame's buffer, then copy the widget's cursor claim onto the frame (no
    /// claim → the terminal hides the cursor).
    pub fn render_frame(&mut self, frame: &mut crate::terminal::Frame<'_>) {
        let area = frame.area();
        self.render(area, frame.buffer_mut());
        if let Some(pos) = self.cursor_pos(area) {
            frame.set_cursor_position(pos);
            frame.set_cursor_style(self.cursor_style(area));
        }
    }

    /// Commit finalized transcript cells into the terminal's native scrollback
    /// via [`crate::terminal::Terminal::insert_history_lines`] (written ABOVE
    /// the bottom viewport), delegating to
    /// [`Transcript::flush_to_native_scrollback`]. The actively-streaming cell
    /// is never committed here (it still grows in place); it commits once as a
    /// whole when `TurnEnded` finalizes it.
    ///
    /// Generic over the backend so tests can drive it with a test backend; the
    /// runtime passes [`crate::RataTerminal`].
    ///
    /// # Errors
    /// Propagates the first terminal IO error from the history insertion.
    pub fn flush_scrollback<B: Backend + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        let width = terminal.size()?.width.max(1);
        self.transcript
            .flush_to_native_scrollback(terminal, width, &self.theme)
    }

    /// Read-only access to the transcript (rendering/tests).
    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// Read-only access to the bottom pane (rendering/tests).
    #[must_use]
    pub fn bottom_pane(&self) -> &BottomPane {
        &self.bottom_pane
    }

    /// Whether a turn is currently in flight.
    #[must_use]
    pub fn turn_running(&self) -> bool {
        self.current_turn.is_some()
    }

    // ===== Registry-dispatched command handlers (`crate::command::BUILTIN`) =====

    /// `/help`: open the shortcuts + slash-commands screen.
    pub(crate) fn cmd_help(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::help()));
        ChatOutcome::Continue
    }

    /// `/model`: open the model picker (or report when no models exist).
    pub(crate) fn cmd_model(&mut self, _args: &str) -> ChatOutcome {
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
        ChatOutcome::Continue
    }

    /// `/doctor`: open the diagnostics screen.
    pub(crate) fn cmd_doctor(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane
            .show_view(Box::new(ScreenView::doctor(&self.session.doctor)));
        ChatOutcome::Continue
    }

    /// `/mcp`: open the MCP servers listing.
    pub(crate) fn cmd_mcp(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
            "MCP servers",
            "MCP servers",
            &self.session.mcp,
            "No MCP servers configured.",
        )));
        ChatOutcome::Continue
    }

    /// `/hooks`: open the hooks listing.
    pub(crate) fn cmd_hooks(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
            "Hooks",
            "Hooks",
            &self.session.hooks,
            "No hooks configured.",
        )));
        ChatOutcome::Continue
    }

    /// `/agents`: open the agents listing.
    pub(crate) fn cmd_agents(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
            "Agents",
            "Agents",
            &self.session.agents,
            "No agents configured.",
        )));
        ChatOutcome::Continue
    }

    /// `/skills`: open the skills listing (captured at launch from the
    /// on-disk `.lingxi/skills/` directories).
    pub(crate) fn cmd_skills(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
            "Skills",
            "Skills",
            &self.session.skills,
            "No skills found. Create skills in .lingxi/skills/ or ~/.lingxi/skills/",
        )));
        ChatOutcome::Continue
    }

    /// `/memory`: open the LINGXI.md memory-file listing (the tiers captured
    /// at launch; read-only — this backend has no in-TUI editor).
    pub(crate) fn cmd_memory(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane.show_view(Box::new(ScreenView::from_rows(
            "Memory files",
            "LINGXI.md memory files",
            &self.session.memory,
            "No memory files found.",
        )));
        ChatOutcome::Continue
    }

    /// `/status`: open the session status screen (snapshot facts + live
    /// editor toggles).
    pub(crate) fn cmd_status(&mut self, _args: &str) -> ChatOutcome {
        let view = ScreenView::status(
            &self.session.doctor,
            self.session.models.iter().find(|m| m.is_current),
            self.bottom_pane.vim_enabled(),
            self.transcript.verbose(),
            self.theme_name,
        );
        self.bottom_pane.show_view(Box::new(view));
        ChatOutcome::Continue
    }

    /// `/config`: open the read-only settings screen (session-scoped
    /// settings + on-disk settings files).
    pub(crate) fn cmd_config(&mut self, _args: &str) -> ChatOutcome {
        let view = ScreenView::settings(
            self.theme_name,
            self.bottom_pane.vim_enabled(),
            self.transcript.verbose(),
            &self.session.doctor.lingxi_home,
            &self.session.doctor.cwd,
        );
        self.bottom_pane.show_view(Box::new(view));
        ChatOutcome::Continue
    }

    /// `/stats`: open the session statistics screen (live widget state:
    /// duration, prompt/reply counts, transcript size, current model).
    pub(crate) fn cmd_stats(&mut self, _args: &str) -> ChatOutcome {
        use crate::history_cell::message::{AssistantTextCell, UserTextCell};
        let cells = self.transcript.committed_cells();
        let prompts = cells
            .iter()
            .filter(|c| c.as_any().downcast_ref::<UserTextCell>().is_some())
            .count();
        let replies = cells
            .iter()
            .filter(|c| c.as_any().downcast_ref::<AssistantTextCell>().is_some())
            .count();
        let view = ScreenView::stats(
            self.start.elapsed().as_secs(),
            prompts,
            replies,
            cells.len(),
            self.session.models.iter().find(|m| m.is_current),
        );
        self.bottom_pane.show_view(Box::new(view));
        ChatOutcome::Continue
    }

    /// `/export [filename]`: write the transcript (every committed cell's
    /// copy-friendly raw lines — the raw-scrollback text) to a `.txt` file in
    /// the export dir, echoing the outcome as a `system` message. No arg →
    /// the timestamped default name; an existing target is never clobbered.
    pub(crate) fn cmd_export(&mut self, args: &str) -> ChatOutcome {
        use std::fmt::Write as _;
        let mut body = String::new();
        for cell in self.transcript.committed_cells() {
            for line in cell.raw_lines() {
                let _ = writeln!(body, "{line}");
            }
        }
        let (display, is_error) = match crate::export::write_export(&self.export_dir, args, &body) {
            Ok(path) => (
                format!("Conversation exported to: {}", path.display()),
                false,
            ),
            Err(crate::export::ExportError::Exists(path)) => (
                format!(
                    "Failed to export conversation: {} already exists (pass a different filename)",
                    path.display()
                ),
                true,
            ),
            Err(crate::export::ExportError::Io(err)) => {
                (format!("Failed to export conversation: {err}"), true)
            }
        };
        self.transcript.push_message(RenderedMessage::SystemText {
            body: display,
            timestamp: 0,
            is_error,
        });
        ChatOutcome::Continue
    }

    /// `/copy [N]`: resolve the Nth-latest assistant text (1 = latest,
    /// default), echo the byte-locked confirmation/error, and hand the text
    /// to the caller for the actual clipboard write.
    pub(crate) fn cmd_copy(&mut self, args: &str) -> ChatOutcome {
        use crate::history_cell::message::AssistantTextCell;
        // Newest-first non-empty assistant bodies, capped at MAX_LOOKBACK
        // (claude-code `collectRecentAssistantTexts`).
        let texts: Vec<String> = self
            .transcript
            .committed_cells()
            .iter()
            .rev()
            .filter_map(|c| c.as_any().downcast_ref::<AssistantTextCell>())
            .map(|c| c.body().to_string())
            .filter(|body| !body.is_empty())
            .take(crate::copy::MAX_LOOKBACK)
            .collect();
        let (display, copy_text) = match crate::copy::parse_copy_command(&texts, args) {
            crate::copy::CopyCommand::Copy { text, display } => (display, Some(text)),
            crate::copy::CopyCommand::Error { display } => (display, None),
        };
        let is_error = copy_text.is_none();
        self.transcript.push_message(RenderedMessage::SystemText {
            body: display,
            timestamp: 0,
            is_error,
        });
        copy_text.map_or(ChatOutcome::Continue, ChatOutcome::CopyToClipboard)
    }

    /// `/theme`: open the theme picker on the active setting. The commit
    /// comes back as [`CommandAction::SetTheme`] via the view stack.
    pub(crate) fn cmd_theme(&mut self, _args: &str) -> ChatOutcome {
        self.bottom_pane
            .show_view(Box::new(ThemePickerView::new(self.theme_setting)));
        ChatOutcome::Continue
    }

    /// `/color [name]`: set/clear/list the session accent color (tints the
    /// composer box). Pure parse ([`crate::color::parse_color_command`]) +
    /// a byte-locked `system` echo; the accent itself is session-only.
    pub(crate) fn cmd_color(&mut self, args: &str) -> ChatOutcome {
        let (display, is_error) = match crate::color::parse_color_command(args) {
            crate::color::ColorCommand::List { display } => (display, false),
            crate::color::ColorCommand::Reset { display } => {
                self.bottom_pane.set_accent(None);
                (display, false)
            }
            crate::color::ColorCommand::Set { name, display } => {
                self.bottom_pane
                    .set_accent(Some(crate::color::accent_color(&name)));
                (display, false)
            }
            crate::color::ColorCommand::Invalid { display } => (display, true),
        };
        self.transcript.push_message(RenderedMessage::SystemText {
            body: display,
            timestamp: 0,
            is_error,
        });
        ChatOutcome::Continue
    }

    /// `/vim`: toggle vim editing mode and echo the new state.
    pub(crate) fn cmd_vim(&mut self, _args: &str) -> ChatOutcome {
        let now_on = self.bottom_pane.toggle_vim();
        self.transcript.push_message(RenderedMessage::SystemText {
            body: format!("Vim mode {}.", if now_on { "enabled" } else { "disabled" }),
            timestamp: 0,
            is_error: false,
        });
        ChatOutcome::Continue
    }

    /// `/clear`: drop the transcript (committed + active + commit cursor).
    pub(crate) fn cmd_clear(&mut self, _args: &str) -> ChatOutcome {
        self.transcript.clear();
        ChatOutcome::Continue
    }

    /// `/image <path>`: record an image message for `path` so a graphics
    /// terminal shows the real pixels.
    pub(crate) fn cmd_image(&mut self, args: &str) -> ChatOutcome {
        self.push_image(args)
    }

    /// The active streaming cell's rendered lines at `width` (empty when
    /// idle) — [`Transcript::visible_live_tail`] under the widget's theme.
    fn live_tail(&self, width: u16) -> Vec<ratatui::text::Line<'static>> {
        self.transcript.visible_live_tail(width, &self.theme)
    }

    /// How many rows the live tail wants at `width` (zero when idle).
    fn live_tail_height(&self, width: u16) -> u16 {
        line_count(&self.live_tail(width))
    }

    /// Split `area` into `(tail_area, pane_area)`: the pane keeps (at least)
    /// its desired height pinned at the bottom; the live tail takes the rows
    /// above, clipped to its own `tail_lines` count. Idle (empty tail) the
    /// pane keeps the WHOLE area, preserving the Phase 5 layout zones
    /// unchanged.
    fn split_area(&self, area: Rect, tail_lines: u16) -> (Rect, Rect) {
        let pane_desired = self.bottom_pane.desired_height(area.width);
        let tail_height = tail_lines.min(area.height.saturating_sub(pane_desired.min(area.height)));
        let tail_area = Rect::new(area.x, area.y, area.width, tail_height);
        let pane_area = Rect::new(
            area.x,
            area.y + tail_height,
            area.width,
            area.height - tail_height,
        );
        (tail_area, pane_area)
    }

    /// The pane's task-status input, recomputed from the widget's turn state
    /// (the pane holds no turn state of its own — plan Phase 5 boundary).
    fn pane_status(&self) -> BottomPaneStatus {
        BottomPaneStatus {
            running: self.current_turn.is_some(),
            text: self.spinner_text(),
        }
    }

    /// Execute the app-level intent the pane returned from a key or paste.
    fn on_pane_outcome(&mut self, outcome: BottomPaneOutcome) -> ChatOutcome {
        match outcome {
            BottomPaneOutcome::Consumed => ChatOutcome::Continue,
            BottomPaneOutcome::Quit => ChatOutcome::Quit,
            BottomPaneOutcome::Interrupt => {
                if let Some(token) = self.current_turn.take() {
                    token.cancel();
                }
                self.turn_started_at = None;
                self.activity = None;
                ChatOutcome::Continue
            }
            BottomPaneOutcome::ToggleVerbose => {
                self.transcript.toggle_verbose();
                self.bottom_pane.set_verbose(self.transcript.verbose());
                ChatOutcome::Continue
            }
            BottomPaneOutcome::Submitted(text) => self.dispatch_submission(text),
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
                ChatOutcome::SwitchModel(request_model, profile)
            }
            BottomPaneOutcome::RunCommand(action) => self.run_command(action),
            BottomPaneOutcome::PastedImage(path) => self.push_image(&path),
        }
    }

    /// Route a submitted composer buffer: a registered slash command
    /// dispatches through the registry; anything else is sent as a prompt.
    fn dispatch_submission(&mut self, text: String) -> ChatOutcome {
        if let Some(outcome) = self.handle_slash(&text) {
            return outcome;
        }
        self.submit_prompt(text)
    }

    /// Record `text` as the user's prompt and hand it to the caller with a
    /// fresh per-turn cancellation token. Shared by the composer submit path
    /// and [`BottomPaneOutcome::SubmitPrompt`].
    fn submit_prompt(&mut self, text: String) -> ChatOutcome {
        self.transcript.push_message(RenderedMessage::UserText {
            body: text.clone(),
            timestamp: 0,
        });
        let token = CancellationToken::new();
        self.current_turn = Some(token.clone());
        ChatOutcome::Submit(text, token)
    }

    /// Execute a command effect a view requested via
    /// [`BottomPaneOutcome::RunCommand`].
    fn run_command(&mut self, action: CommandAction) -> ChatOutcome {
        match action {
            CommandAction::ClearTranscript => {
                self.transcript.clear();
                ChatOutcome::Continue
            }
            CommandAction::Quit => ChatOutcome::Quit,
            CommandAction::SetTheme(setting) => {
                // Applied live here; the caller persists it (best-effort).
                self.set_theme(setting);
                ChatOutcome::SetTheme(setting)
            }
        }
    }

    /// Record an image message for `path` (metadata = the file name). Shared
    /// by `/image <path>` and pasted-image-path routing.
    fn push_image(&mut self, path: &str) -> ChatOutcome {
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
        ChatOutcome::Continue
    }

    /// Surface the oldest queued permission once no prompt is open (called
    /// after every key/paste, i.e. after a resolution could have happened).
    fn open_next_queued_permission(&mut self) {
        if !self.has_open_permission() {
            if let Some(exchange) = self.pending_permissions.pop_front() {
                self.bottom_pane.show_permission(exchange);
            }
        }
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
}

/// `lines.len()` as a saturating `u16` (row heights are `u16` everywhere).
fn line_count(lines: &[ratatui::text::Line<'static>]) -> u16 {
    u16::try_from(lines.len()).unwrap_or(u16::MAX)
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

#[cfg(test)]
mod tests {
    use std::any::Any;

    use crossterm::event::{KeyCode, KeyModifiers};
    use permission::gate::{PermissionRequest, PermissionResponse};
    use ratatui::layout::Position;
    use tokio::sync::oneshot;

    use super::*;
    use crate::bottom_pane::model_picker_view::ModelPickerView;
    use crate::bottom_pane::{BottomPaneView, ViewOutcome};
    use crate::history_cell::attachments::UserImageCell;
    use crate::session::ModelRow;
    use crate::terminal::test_support::TestWriteBackend;
    use crate::terminal::Terminal;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn widget() -> ChatWidget {
        ChatWidget::new(Vec::new(), SessionInfo::default())
    }

    fn typ(widget: &mut ChatWidget, s: &str) {
        for c in s.chars() {
            widget.handle_key(press(KeyCode::Char(c)));
        }
    }

    fn submit_command(widget: &mut ChatWidget, cmd: &str) -> ChatOutcome {
        typ(widget, cmd);
        widget.handle_key(press(KeyCode::Enter))
    }

    /// The transcript's cells in order — committed then the active
    /// (streaming) cell — for per-variant cell-level assertions (the
    /// message-cells split replaced the old `messages()` reconstruction).
    fn cells(widget: &ChatWidget) -> Vec<&dyn crate::history_cell::HistoryCell> {
        let mut out: Vec<&dyn crate::history_cell::HistoryCell> = widget
            .transcript
            .committed_cells()
            .iter()
            .map(AsRef::as_ref)
            .collect();
        out.extend(widget.transcript.active_cell());
        out
    }

    /// Downcast transcript cell `idx` (committed order, active last) to its
    /// concrete cell type.
    fn cell<T: 'static>(widget: &ChatWidget, idx: usize) -> &T {
        cells(widget)[idx]
            .as_any()
            .downcast_ref::<T>()
            .expect("concrete cell type")
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

    fn widget_with_models() -> ChatWidget {
        ChatWidget::new(
            Vec::new(),
            SessionInfo {
                models: vec![
                    ModelRow {
                        display: "Opus".into(),
                        request_model: "claude-opus".into(),
                        profile: Some("anthropic".into()),
                        provider_label: "Anthropic".into(),
                        is_current: true,
                    },
                    ModelRow {
                        display: "Sonnet".into(),
                        request_model: "claude-sonnet".into(),
                        profile: Some("anthropic".into()),
                        provider_label: "Anthropic".into(),
                        is_current: false,
                    },
                ],
                ..Default::default()
            },
        )
    }

    /// An 80x24 bottom-anchored test terminal with a 4-row viewport.
    fn test_terminal() -> Terminal<TestWriteBackend> {
        let mut terminal =
            Terminal::with_options(TestWriteBackend::new(80, 24)).expect("test terminal");
        terminal.set_bottom_viewport_height(4).expect("viewport");
        terminal
    }

    #[test]
    fn new_seeds_transcript_and_starts_idle() {
        let seed = RenderedMessage::SystemText {
            body: "seeded".to_string(),
            timestamp: 0,
            is_error: false,
        };
        let widget = ChatWidget::new(vec![seed], SessionInfo::default());
        assert_eq!(widget.transcript().committed_cells().len(), 1);
        assert!(widget.transcript().active_cell().is_none());
        assert!(!widget.turn_running());
        assert!(!widget.has_open_permission());
        assert!(widget.pending_permissions.is_empty());
    }

    #[test]
    fn event_lifecycle_started_deltas_tools_ended() {
        let mut widget = widget();
        let outcome = submit_command(&mut widget, "go");
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, _) if p == "go"));
        assert!(widget.turn_running());

        // TurnStarted opens an empty active assistant cell and starts the clock.
        widget.apply_turn_event(TurnEvent::TurnStarted);
        assert!(widget.turn_started_at.is_some());
        assert!(widget.transcript.active_cell().is_some());

        // Deltas grow the active cell in place.
        widget.apply_turn_event(TurnEvent::TextDelta("Hel".to_string()));
        widget.apply_turn_event(TurnEvent::TextDelta("lo".to_string()));
        assert_eq!(cell::<AssistantTextCell>(&widget, 1).body(), "Hello");

        // ToolUseStart sets the spinner activity; ToolUseResult clears it.
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Bash".to_string(),
            input: serde_json::json!({}),
        });
        assert_eq!(widget.activity.as_deref(), Some("Running Bash"));
        assert!(widget.spinner_text().contains("Running Bash"));
        widget.apply_turn_event(TurnEvent::ToolUseResult {
            id: protocol::ToolUseId::from("t1"),
            tool: "Bash".to_string(),
            result: serde_json::json!({}),
        });
        assert!(widget.activity.is_none());

        // TurnEnded finalizes the reply and clears every per-turn field.
        widget.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        assert!(!widget.turn_running());
        assert!(widget.turn_started_at.is_none());
        assert!(widget.transcript.active_cell().is_none(), "cell flushed");
        assert_eq!(widget.transcript.committed_cells().len(), 2);
    }

    #[test]
    fn active_streaming_tail_stays_live_until_turn_ends() {
        let mut widget = widget();
        submit_command(&mut widget, "hi");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        widget.apply_turn_event(TurnEvent::TextDelta("stream-tail-text".to_string()));
        let theme = Theme::dark();
        let tail: String = widget
            .transcript()
            .visible_live_tail(80, &theme)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(tail.contains("stream-tail-text"), "live tail: {tail}");

        // Flushing commits the finalized user message but holds the tail back.
        let mut terminal = test_terminal();
        widget.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(widget.transcript().committed_to_terminal(), 1);

        // TurnEnded finalizes: the reply commits as a whole, the tail empties.
        widget.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        widget.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(widget.transcript().committed_to_terminal(), 2);
        assert!(widget.transcript().visible_live_tail(80, &theme).is_empty());
    }

    #[test]
    fn ctrl_c_cancels_the_turn_and_clears_activity() {
        let mut widget = widget();
        typ(&mut widget, "x");
        let ChatOutcome::Submit(_, token) = widget.handle_key(press(KeyCode::Enter)) else {
            panic!("expected submit");
        };
        widget.apply_turn_event(TurnEvent::TurnStarted);
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Read".to_string(),
            input: serde_json::json!({}),
        });
        assert!(!token.is_cancelled());
        let outcome = widget.handle_key(ctrl(KeyCode::Char('c')));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(token.is_cancelled());
        assert!(!widget.turn_running());
        assert!(widget.activity.is_none());
        assert!(widget.turn_started_at.is_none());
    }

    #[test]
    fn slash_dispatch_routes_through_the_registry() {
        let mut widget = widget();
        // Registered command: focused view opens, no prompt turn starts.
        let outcome = submit_command(&mut widget, "/help");
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(widget.bottom_pane().view_stack().contains::<ScreenView>());
        assert!(widget.transcript().is_empty(), "no scrollback dump");
        assert!(!widget.turn_running());
        widget.handle_key(press(KeyCode::Esc)); // close /help

        // Aliases dispatch to the same registry entry.
        assert!(matches!(
            submit_command(&mut widget, "/quit"),
            ChatOutcome::Quit
        ));

        // Unregistered input falls through as a normal prompt.
        let outcome = submit_command(&mut widget, "/frobnicate");
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, _) if p == "/frobnicate"));

        // Argument gating mirrors the registry: no-arg commands reject
        // trailing args, arg-taking commands require them.
        assert!(widget.handle_slash("/help extra").is_none());
        assert!(widget.handle_slash("/image").is_none());
    }

    #[test]
    fn slash_image_dispatches_with_arguments() {
        let mut widget = widget();
        let outcome = submit_command(&mut widget, "/image /tmp/pic.png");
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert_eq!(cells(&widget).len(), 1);
        let image = cell::<UserImageCell>(&widget, 0);
        assert_eq!(image.source_path(), Some("/tmp/pic.png"));
        assert_eq!(image.metadata(), Some("pic.png"));
    }

    #[test]
    fn second_permission_request_queues_until_the_first_resolves() {
        let mut widget = widget();
        let (first, first_rx) = tool_exchange();
        let (second, second_rx) = tool_exchange();
        widget.open_permission(first);
        widget.open_permission(second);
        assert!(widget.has_open_permission());
        assert_eq!(
            widget.bottom_pane().view_stack().len(),
            1,
            "second prompt queues instead of stacking"
        );
        assert_eq!(widget.pending_permissions.len(), 1);

        // Resolving the first ('1' = AllowOnce) surfaces the queued prompt.
        widget.handle_key(press(KeyCode::Char('1')));
        assert_eq!(
            first_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
        assert!(widget.has_open_permission(), "queued prompt opened");
        assert!(widget.pending_permissions.is_empty());

        // The surfaced prompt owns the keyboard until resolved (Esc denies).
        widget.handle_key(press(KeyCode::Esc));
        assert_eq!(second_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
        assert!(!widget.has_open_permission());

        // Responses were one-shot; the keyboard belongs to the composer again.
        widget.handle_key(press(KeyCode::Char('z')));
        assert_eq!(widget.bottom_pane().composer().text(), "z");
    }

    #[test]
    fn queued_permissions_serialize_across_variants_and_drop_closes_channels() {
        let mut widget = widget();
        let (first, first_rx) = tool_exchange();
        let (plan_tx, plan_rx) = oneshot::channel();
        let plan = PermissionExchange {
            request: PermissionRequest::ExitPlanMode {
                plan: "1. Foo".to_string(),
            },
            resp_tx: plan_tx,
            worker: None,
        };
        widget.open_permission(first);
        widget.open_permission(plan);
        assert_eq!(widget.pending_permissions.len(), 1);
        // Resolve the first; the queued plan-approval prompt surfaces with its
        // own variant-specific keyboard ('2' = auto-accept edits).
        widget.handle_key(press(KeyCode::Esc));
        assert_eq!(first_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
        assert!(widget.has_open_permission(), "queued plan prompt surfaced");
        widget.handle_key(press(KeyCode::Char('2')));
        assert_eq!(
            plan_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowAlways
        );
        assert!(!widget.has_open_permission());

        // Dropping the widget with an open AND a queued exchange closes both
        // channels unsent (the gate maps a dropped resp_tx to a deny) instead
        // of leaking unanswerable prompts.
        let mut widget = ChatWidget::new(Vec::new(), SessionInfo::default());
        let (open, open_rx) = tool_exchange();
        let (queued, queued_rx) = tool_exchange();
        widget.open_permission(open);
        widget.open_permission(queued);
        drop(widget);
        assert!(open_rx.blocking_recv().is_err());
        assert!(queued_rx.blocking_recv().is_err());
    }

    #[test]
    fn model_switch_reports_request_model_and_profile() {
        let mut widget = widget_with_models();
        assert!(matches!(
            submit_command(&mut widget, "/model"),
            ChatOutcome::Continue
        ));
        assert!(widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        // Picker owns the keyboard: move up to the first (Opus) row and confirm.
        widget.handle_key(press(KeyCode::Up));
        let outcome = widget.handle_key(press(KeyCode::Enter));
        assert!(matches!(
            outcome,
            ChatOutcome::SwitchModel(ref m, ref p)
                if m == "claude-opus" && p.as_deref() == Some("anthropic")
        ));
        assert!(!widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        // The switch is echoed as a system message.
        let body = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0).body();
        assert!(body.contains("Switching model to claude-opus"), "{body}");
    }

    #[test]
    fn skills_memory_status_config_open_focused_views_not_scrollback_dumps() {
        // Criterion 18: each command opens a focused ScreenView and leaves
        // the transcript untouched (no scrollback dump), then Esc returns to
        // the composer.
        let session = SessionInfo {
            skills: vec![crate::session::InfoRow::new(
                "brainstorming",
                Some("Project skills · explore ideas".to_string()),
            )],
            memory: vec![crate::session::InfoRow::new(
                "Project memory",
                Some("Checked in at ./LINGXI.md".to_string()),
            )],
            ..Default::default()
        };
        for cmd in ["/skills", "/memory", "/status", "/config"] {
            let mut widget = ChatWidget::new(Vec::new(), session.clone());
            let outcome = submit_command(&mut widget, cmd);
            assert!(matches!(outcome, ChatOutcome::Continue), "{cmd}");
            assert!(
                widget.bottom_pane().view_stack().contains::<ScreenView>(),
                "{cmd} opens a focused view"
            );
            assert!(
                widget.transcript().is_empty(),
                "{cmd} must not dump into scrollback"
            );
            assert!(!widget.turn_running(), "{cmd} starts no turn");
            widget.handle_key(press(KeyCode::Esc));
            assert!(
                widget.bottom_pane().view_stack().is_empty(),
                "{cmd} closes back to the composer"
            );
        }
    }

    #[test]
    fn slash_theme_opens_picker_and_enter_applies_and_reports_for_persistence() {
        let mut widget = widget();
        assert_eq!(
            widget.theme_setting(),
            ThemeSetting::Named(ThemeName::Dark),
            "hermetic default"
        );
        assert!(matches!(
            submit_command(&mut widget, "/theme"),
            ChatOutcome::Continue
        ));
        assert!(widget
            .bottom_pane()
            .view_stack()
            .contains::<ThemePickerView>());
        // Move from "Dark mode" (index 1) to "Light mode" (index 2), commit.
        widget.handle_key(press(KeyCode::Down));
        let outcome = widget.handle_key(press(KeyCode::Enter));
        // The theme is applied live AND surfaced so the app can persist it.
        assert!(matches!(
            outcome,
            ChatOutcome::SetTheme(ThemeSetting::Named(ThemeName::Light))
        ));
        assert_eq!(
            widget.theme_setting(),
            ThemeSetting::Named(ThemeName::Light)
        );
        assert_eq!(widget.theme_name(), ThemeName::Light);
        assert_eq!(widget.theme, tui_core::theme::theme_for(ThemeName::Light));
        assert!(
            widget.bottom_pane().view_stack().is_empty(),
            "picker closes on commit"
        );
        assert!(widget.transcript().is_empty(), "no scrollback dump");

        // Esc cancels without touching the applied theme.
        submit_command(&mut widget, "/theme");
        widget.handle_key(press(KeyCode::Esc));
        assert!(widget.bottom_pane().view_stack().is_empty());
        assert_eq!(widget.theme_name(), ThemeName::Light);
    }

    #[test]
    fn slash_color_sets_resets_lists_and_rejects_with_system_echoes() {
        use crate::history_cell::system::SystemTextCell;
        let mut widget = widget();

        // Set: accent applied to the pane + byte-locked echo.
        submit_command(&mut widget, "/color cyan");
        assert_eq!(
            widget.bottom_pane().accent(),
            Some(crate::color::accent_color("cyan"))
        );
        assert_eq!(
            cell::<SystemTextCell>(&widget, 0).body(),
            "Session color set to: cyan"
        );

        // Invalid: error echo, accent untouched.
        submit_command(&mut widget, "/color chartreuse");
        assert!(widget.bottom_pane().accent().is_some());
        let invalid = cell::<SystemTextCell>(&widget, 1);
        assert!(invalid.body().starts_with("Invalid color \"chartreuse\"."));
        assert!(invalid.is_error(), "invalid color echoes as an error");

        // Reset alias: accent cleared.
        submit_command(&mut widget, "/color default");
        assert_eq!(widget.bottom_pane().accent(), None);
        assert_eq!(
            cell::<SystemTextCell>(&widget, 2).body(),
            "Session color reset to default"
        );

        // Bare /color: lists the available colors (ArgSpec::Optional).
        submit_command(&mut widget, "/color");
        let list = cell::<SystemTextCell>(&widget, 3);
        assert!(
            list.body().starts_with("Please provide a color."),
            "{}",
            list.body()
        );
        assert!(!widget.turn_running(), "no prompt turn for /color");
    }

    #[test]
    fn slash_stats_opens_session_stats_view_with_real_counts() {
        let mut widget = ChatWidget::new(
            vec![
                RenderedMessage::UserText {
                    body: "hi".to_string(),
                    timestamp: 0,
                },
                RenderedMessage::AssistantText {
                    body: "hello".to_string(),
                    timestamp: 0,
                },
                RenderedMessage::SystemText {
                    body: "note".to_string(),
                    timestamp: 0,
                    is_error: false,
                },
            ],
            SessionInfo::default(),
        );
        assert!(matches!(
            submit_command(&mut widget, "/stats"),
            ChatOutcome::Continue
        ));
        let stats = widget
            .bottom_pane()
            .view_stack()
            .active()
            .and_then(|v| v.as_any().downcast_ref::<ScreenView>())
            .expect("stats screen open");
        let text = stats.body_text();
        assert!(text.contains("This session"), "{text}");
        // 1 user prompt, 1 assistant reply, 3 committed cells.
        let count_of = |label: &str| {
            text.lines()
                .find(|l| l.contains(label))
                .unwrap_or_else(|| panic!("no {label} row:\n{text}"))
                .rsplit(' ')
                .next()
                .unwrap()
                .to_string()
        };
        assert_eq!(count_of("Prompts sent"), "1", "{text}");
        assert_eq!(count_of("Replies received"), "1", "{text}");
        assert_eq!(count_of("Transcript cells"), "3", "{text}");
        assert!(text.contains("Counts cover this session only."), "{text}");
        assert_eq!(widget.transcript().committed_cells().len(), 3, "no dump");
        widget.handle_key(press(KeyCode::Esc));
        assert!(widget.bottom_pane().view_stack().is_empty());
    }

    #[test]
    fn slash_export_writes_raw_transcript_and_never_clobbers() {
        let dir = std::env::temp_dir().join(format!("tui-rata-cmd-export-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let mut widget = ChatWidget::new(
            vec![
                RenderedMessage::UserText {
                    body: "hi".to_string(),
                    timestamp: 0,
                },
                RenderedMessage::AssistantText {
                    body: "hello there".to_string(),
                    timestamp: 0,
                },
            ],
            SessionInfo::default(),
        );
        widget.set_export_dir(dir.clone());

        // Named export writes the committed cells' raw (copy-friendly) text.
        assert!(matches!(
            submit_command(&mut widget, "/export conv"),
            ChatOutcome::Continue
        ));
        let body = std::fs::read_to_string(dir.join("conv.txt")).expect("exported file");
        assert!(body.contains("hi"), "{body}");
        assert!(body.contains("hello there"), "{body}");
        let echo = cell::<crate::history_cell::system::SystemTextCell>(&widget, 2);
        assert!(
            echo.body().starts_with("Conversation exported to: "),
            "{}",
            echo.body()
        );
        assert!(!echo.is_error());

        // Re-exporting to the same name refuses to clobber, as an error echo.
        submit_command(&mut widget, "/export conv");
        let echo = cell::<crate::history_cell::system::SystemTextCell>(&widget, 3);
        assert!(
            echo.body().starts_with("Failed to export conversation:"),
            "{}",
            echo.body()
        );
        assert!(echo.is_error());

        // Bare /export falls back to the timestamped default filename.
        submit_command(&mut widget, "/export");
        let echo = cell::<crate::history_cell::system::SystemTextCell>(&widget, 4);
        assert!(
            echo.body().contains("lingxi-transcript-"),
            "{}",
            echo.body()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn slash_copy_echoes_confirmation_and_hands_text_to_the_caller() {
        use crate::history_cell::system::SystemTextCell;
        let mut widget = widget();

        // Empty transcript: byte-locked error, no clipboard handoff.
        assert!(matches!(
            submit_command(&mut widget, "/copy"),
            ChatOutcome::Continue
        ));
        let echo = cell::<SystemTextCell>(&widget, 0);
        assert_eq!(echo.body(), "No assistant message to copy");
        assert!(echo.is_error());

        // Two replies: /copy takes the latest, /copy 2 reaches back.
        let mut widget = ChatWidget::new(
            vec![
                RenderedMessage::AssistantText {
                    body: "older reply".to_string(),
                    timestamp: 0,
                },
                RenderedMessage::AssistantText {
                    body: "newest".to_string(),
                    timestamp: 0,
                },
            ],
            SessionInfo::default(),
        );
        let outcome = submit_command(&mut widget, "/copy");
        assert!(matches!(outcome, ChatOutcome::CopyToClipboard(ref t) if t == "newest"));
        assert_eq!(
            cell::<SystemTextCell>(&widget, 2).body(),
            "Copied to clipboard (6 characters, 1 lines)"
        );
        let outcome = submit_command(&mut widget, "/copy 2");
        assert!(matches!(outcome, ChatOutcome::CopyToClipboard(ref t) if t == "older reply"));
        // Out-of-range and junk args echo errors without a handoff.
        assert!(matches!(
            submit_command(&mut widget, "/copy 9"),
            ChatOutcome::Continue
        ));
        assert_eq!(
            cell::<SystemTextCell>(&widget, 4).body(),
            "Only 2 assistant messages available to copy"
        );
        assert!(!widget.turn_running(), "/copy starts no turn");
    }

    #[test]
    fn slash_clear_resets_transcript_and_commit_cursor() {
        let mut widget = ChatWidget::new(
            vec![RenderedMessage::SystemText {
                body: "old".to_string(),
                timestamp: 0,
                is_error: false,
            }],
            SessionInfo::default(),
        );
        let mut terminal = test_terminal();
        widget.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(widget.transcript().committed_to_terminal(), 1);
        assert!(matches!(
            submit_command(&mut widget, "/clear"),
            ChatOutcome::Continue
        ));
        assert!(widget.transcript().is_empty());
        assert_eq!(widget.transcript().committed_to_terminal(), 0);

        // Criterion 17 (coherence): transcript state AND the native-scrollback
        // commit counter reset TOGETHER — the next flush is a no-op on the
        // emptied transcript, and new content commits cleanly from zero.
        widget.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(widget.transcript().committed_to_terminal(), 0);
        widget.transcript.push_message(RenderedMessage::SystemText {
            body: "fresh".to_string(),
            timestamp: 0,
            is_error: false,
        });
        widget.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(
            widget.transcript().committed_to_terminal(),
            1,
            "post-clear content commits from a zeroed cursor"
        );
    }

    /// One string per buffer row.
    fn buffer_rows(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| {
                (area.left()..area.right())
                    .map(|x| {
                        buf.cell(Position::new(x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn render_contract_refreshes_status_and_delegates_cursor_to_the_pane() {
        let mut widget = widget();
        typ(&mut widget, "go");
        widget.handle_key(press(KeyCode::Enter));
        widget.apply_turn_event(TurnEvent::TurnStarted);
        let width = 80;
        // Desired height = live tail (the just-opened empty assistant cell
        // renders its 1-row marker) stacked above the pane's height.
        let tail_height = widget.live_tail_height(width);
        assert_eq!(tail_height, 1, "empty active assistant cell = marker row");
        assert_eq!(
            widget.desired_height(width),
            widget.bottom_pane().desired_height(width) + tail_height
        );
        let area = Rect::new(0, 0, width, widget.desired_height(width).max(4));
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        // The render refreshed the pane's task status before drawing: the
        // spinner row (just below the tail) reflects the in-flight turn.
        let status_row = &buffer_rows(&buf)[usize::from(tail_height)];
        assert!(
            status_row.contains("esc to interrupt"),
            "status: {status_row}"
        );
        assert!(
            status_row.contains("Ctrl-C: cancel"),
            "status: {status_row}"
        );
        // Cursor position/style delegate to the pane within its sub-area
        // below the live tail.
        let (_, pane_area) = widget.split_area(area, tail_height);
        assert_eq!(pane_area.y, tail_height);
        assert_eq!(
            widget.cursor_pos(area),
            widget.bottom_pane().cursor_pos(pane_area)
        );
        assert!(widget.cursor_pos(area).is_some());
    }

    #[test]
    fn mid_turn_text_delta_is_visible_in_the_rendered_frame_before_turn_ended() {
        let mut widget = widget();
        submit_command(&mut widget, "go");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        let width = 80u16;
        let pane_only = widget.bottom_pane().desired_height(width);
        widget.apply_turn_event(TurnEvent::TextDelta("streamed reply words".to_string()));
        // desired_height accounts for the live tail…
        let height = widget.desired_height(width);
        assert!(
            height > pane_only,
            "tail must grow the widget: {height} <= {pane_only}"
        );
        // …and the mid-turn delta is VISIBLE in the rendered frame — before
        // any TurnEnded — drawn ABOVE the bottom pane (acceptance criterion
        // 12: streaming text stays visible while the turn is active).
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let rows = buffer_rows(&buf);
        let text_row = rows
            .iter()
            .position(|row| row.contains("streamed reply words"))
            .unwrap_or_else(|| panic!("streamed text not rendered:\n{}", rows.join("\n")));
        let status_row = rows
            .iter()
            .position(|row| row.contains("esc to interrupt"))
            .expect("running status row rendered");
        assert!(
            text_row < status_row,
            "tail above the pane: text row {text_row}, status row {status_row}"
        );
        // The composer prompt still renders beneath the tail.
        assert!(
            rows.iter().any(|row| row.starts_with("│> ")),
            "composer visible:\n{}",
            rows.join("\n")
        );
    }

    #[test]
    fn spinner_text_has_frame_activity_elapsed_seconds_and_interrupt_hint() {
        let mut widget = widget();
        submit_command(&mut widget, "go");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        // Default activity: an animation frame glyph, "Working", elapsed
        // seconds, and the esc-to-interrupt hint (claude-code status parity).
        let text = widget.spinner_text();
        let frame = text.chars().next().expect("spinner frame glyph");
        assert!("·✢✳✶✻✽".contains(frame), "unknown frame: {text}");
        assert!(text.contains("Working… ("), "verb + elapsed open: {text}");
        assert!(text.ends_with("s · esc to interrupt)"), "hint: {text}");
        // ToolUseStart swaps the verb for the activity label.
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Edit".to_string(),
            input: serde_json::json!({}),
        });
        assert!(widget.spinner_text().contains("Editing… ("));
    }

    #[test]
    fn activity_label_maps_known_tools_to_gerunds() {
        assert_eq!(activity_label("Bash"), "Running Bash");
        assert_eq!(activity_label("BashOutput"), "Running Bash");
        assert_eq!(activity_label("Read"), "Reading");
        assert_eq!(activity_label("Write"), "Writing");
        assert_eq!(activity_label("Edit"), "Editing");
        assert_eq!(activity_label("MultiEdit"), "Editing");
        assert_eq!(activity_label("Grep"), "Searching");
        assert_eq!(activity_label("Glob"), "Searching");
        assert_eq!(activity_label("WebFetch"), "Browsing");
        assert_eq!(activity_label("WebSearch"), "Browsing");
        assert_eq!(activity_label("Task"), "Delegating");
        assert_eq!(activity_label("SomeMcpTool"), "Running SomeMcpTool");
    }

    #[test]
    fn view_run_command_outcomes_dispatch_to_the_widget() {
        /// A stub view that returns a scripted command on any key.
        struct CommandStub(Option<CommandAction>);

        impl Renderable for CommandStub {
            fn render(&self, _area: Rect, _buf: &mut Buffer) {}
            fn desired_height(&self, _width: u16) -> u16 {
                1
            }
        }

        impl BottomPaneView for CommandStub {
            fn handle_key(&mut self, _key: KeyEvent) -> ViewOutcome {
                self.0
                    .take()
                    .map_or(ViewOutcome::Pending, ViewOutcome::RunCommand)
            }

            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        // ClearTranscript empties the transcript.
        let mut widget = ChatWidget::new(
            vec![RenderedMessage::SystemText {
                body: "old".to_string(),
                timestamp: 0,
                is_error: false,
            }],
            SessionInfo::default(),
        );
        widget
            .bottom_pane
            .show_view(Box::new(CommandStub(Some(CommandAction::ClearTranscript))));
        assert!(matches!(
            widget.handle_key(press(KeyCode::Enter)),
            ChatOutcome::Continue
        ));
        assert!(widget.transcript().is_empty());
        assert!(
            widget.bottom_pane().view_stack().is_empty(),
            "completed view popped"
        );

        // Quit surfaces as ChatOutcome::Quit.
        widget
            .bottom_pane
            .show_view(Box::new(CommandStub(Some(CommandAction::Quit))));
        assert!(matches!(
            widget.handle_key(press(KeyCode::Enter)),
            ChatOutcome::Quit
        ));
    }
}
