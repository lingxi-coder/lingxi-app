//! Interactive `tui` chat app: the runtime event loop around
//! [`ChatWidget`].
//!
//! `RataApp` is runtime orchestration ONLY (plan Phase 7): it owns the chat
//! widget, the event receivers it drains each tick (turn events + permission
//! requests), the redraw cadence, and the [`AppCallbacks`] to the embedding
//! CLI. All conversation state — transcript, bottom pane, session snapshot,
//! per-turn state, spinner text — lives in [`ChatWidget`]; all terminal state
//! (viewport rect, buffers, cursor) lives in [`crate::terminal::Terminal`].
//!
//! The app is decoupled from orchestrator construction: `run_app` takes a
//! `TurnEvent` receiver (drained each tick) and an `on_submit` callback that
//! receives the prompt + a per-turn `CancellationToken` (the caller spawns the
//! real turn; the widget cancels it on Ctrl-C/Esc).

use std::io;
use std::io::Write;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::backend::{Backend, CrosstermBackend};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};
use tokio_util::sync::CancellationToken;
use tui_core::ask_user_question_bridge::AskUserQuestionExchange;
use tui_core::computer_access_bridge::ComputerAccessExchange;
use tui_core::message::RenderedMessage;
use tui_core::orchestrator_bridge::TurnEvent;
use tui_core::permission_bridge::PermissionExchange;

use crate::bottom_pane::permissions_editor_view::PermissionsSnapshot;
use crate::bottom_pane::{ConnectAction, PermissionAction, PluginAction, TaskAction, WebAction};
use crate::chat_widget::{ChatOutcome, ChatWidget};
use crate::session::SessionInfo;
use crate::terminal::TerminalSession;
use crate::RataTerminal;

/// How the [`run_app`] event loop exited.
///
/// The historical loop only ever ended one way (the user quit), so `run`
/// returned `io::Result<()>`. `/resume` adds a second, in-band exit: the picker
/// asks the loop to UNWIND carrying the chosen session uuid so the embedder can
/// re-mount that session in-process (the JSONL writer is retargeted at build) —
/// NOT an in-place `resume_session` swap, which would fork the conversation
/// across files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppExit {
    /// The user quit (`/exit`, `/stop`, Ctrl-C twice): the embedder tears down.
    Quit,
    /// The `/resume` picker resolved to this session uuid: the embedder must
    /// re-mount that session in-process (writer retargeted via the startup
    /// resume seam).
    SwitchSession(uuid::Uuid),
    /// `/branch`: fork the current conversation into a new session and switch
    /// into it. The embedder creates the branch transcript off-loop then
    /// re-mounts the new session in-process (same unwind path as
    /// [`Self::SwitchSession`]). Carries the optional `/branch [name]` title.
    BranchSession { title: Option<String> },
    /// `/rewind`: restore the working tree and/or conversation to `message`.
    /// The embedder runs the code file-rewind + optional transcript truncation
    /// off-loop, then re-mounts in-process (same unwind path as
    /// [`Self::SwitchSession`] / [`Self::BranchSession`]).
    Rewind {
        /// The target user-message uuid.
        message: uuid::Uuid,
        /// Which parts to restore.
        scope: crate::bottom_pane::view::RewindScope,
    },
}

/// The embedding CLI/orchestrator callbacks the event loop executes when the
/// chat widget returns an app-level [`ChatOutcome`].
pub struct AppCallbacks<'cb> {
    /// Executed on [`ChatOutcome::Submit`]: the caller drives a turn for the
    /// prompt, honoring the paired [`CancellationToken`] (the widget cancels
    /// it on Ctrl-C/Esc).
    pub on_submit: Box<dyn FnMut(String, Vec<std::path::PathBuf>, CancellationToken) + 'cb>,
    /// Executed on [`ChatOutcome::SwitchModel`] with the picked
    /// `(request_model, profile)` pair.
    pub on_switch_model: Box<dyn FnMut(String, Option<String>) + 'cb>,
    /// Executed on [`ChatOutcome::WebAction`]: the caller runs the `/web`
    /// effect (persist a key/settings, or a test search) asynchronously and
    /// reports the result back via a [`TurnEvent::SystemNotice`].
    pub on_web_action: Box<dyn FnMut(WebAction) + 'cb>,
    /// Executed on [`ChatOutcome::ConnectAction`]: the caller runs the
    /// `/connect` effect (store an API key, or kick off a Copilot/OAuth
    /// sign-in) asynchronously and reports the result back via a
    /// [`TurnEvent::SystemNotice`].
    pub on_connect_action: Box<dyn FnMut(ConnectAction) + 'cb>,
    /// Executed on [`ChatOutcome::PermissionAction`]: the caller persists the
    /// added/removed permission rule to its settings file (and pushes an added
    /// allow rule into the live `session_allow_rules`) asynchronously and
    /// reports the result back via a [`TurnEvent::SystemNotice`].
    pub on_permission_action: Box<dyn FnMut(PermissionAction) + 'cb>,
    /// Executed on [`ChatOutcome::PluginAction`]: the caller toggles the on-disk
    /// `enabledPlugins` allowlist (CLI `plugin_settings::run_enable`/
    /// `run_disable`) asynchronously, refreshes the shared `/plugin` snapshot,
    /// and reports the result via a [`TurnEvent::SystemNotice`].
    pub on_plugin_action: Box<dyn FnMut(PluginAction) + 'cb>,
    /// Executed on [`ChatOutcome::ReloadPlugins`] (`/reload-plugins`): the caller
    /// re-reads the on-disk enabled set and applies pending plugin enable/disable
    /// changes to the LIVE session (commands/hooks/agents/MCP/LSP swap in place),
    /// reporting the component tallies via a [`TurnEvent::SystemNotice`]. No-op
    /// (informational notice) when plugins are disabled for the session.
    pub on_reload_plugins: Box<dyn FnMut() + 'cb>,
    /// Executed on [`ChatOutcome::RunBash`]: the caller runs the `!`-prefixed
    /// command through the sandboxed bash runner (no LLM turn) and folds its
    /// output back via [`TurnEvent::BashOutput`].
    pub on_bash: Box<dyn FnMut(String) + 'cb>,
    /// Executed on [`ChatOutcome::Compact`]: the caller drives
    /// `OrchestratorHandle::force_compact` asynchronously on the LIVE engine
    /// runtime (never on the render thread) and reports the summary back via a
    /// [`TurnEvent::SystemNotice`]. The `String` is the argument tail.
    pub on_compact: Box<dyn FnMut(String, CancellationToken) + 'cb>,
    /// Executed on [`ChatOutcome::RenameSession`]: the caller appends the
    /// `custom-title` line via `OrchestratorHandle::rename_session` off the
    /// render thread and reports the result back via a
    /// [`TurnEvent::SystemNotice`]. The `String` is the new title.
    pub on_rename: Box<dyn FnMut(String) + 'cb>,
    /// Executed on [`ChatOutcome::FastMode`]: the caller flips the session's
    /// fast-mode flag off-loop via `OrchestratorHandle::set_fast_mode` (reading
    /// the current value first when the arg is `None`, a bare `/fast` toggle)
    /// and reports the applied state through a [`TurnEvent::SystemNotice`]. The
    /// `Option<bool>` is `Some(target)` for `on`/`off`, `None` for toggle.
    pub on_fast_mode: Box<dyn FnMut(Option<bool>) + 'cb>,
    /// Executed on [`ChatOutcome::PlanMode`]: the caller reads the session's
    /// plan-mode flag off-loop and, when not already in plan mode, flips it on
    /// via `OrchestratorHandle::set_plan_mode`; the applied state (or the
    /// "already in plan mode" view message) returns via a
    /// [`TurnEvent::SystemNotice`], same shape as `on_fast_mode`. The `String`
    /// is the trimmed argument tail.
    pub on_plan_mode: Box<dyn FnMut(String) + 'cb>,
    /// Executed on [`ChatOutcome::SetPermissionMode`]: Shift+Tab cycled the
    /// session permission mode; the caller pushes the wire mode id to the live
    /// engine off-loop via `OrchestratorHandle::set_permission_mode` so
    /// enforcement follows the indicator the pane already updated.
    pub on_set_permission_mode: Box<dyn FnMut(String) + 'cb>,
    /// Executed on [`ChatOutcome::SandboxAction`]: the caller persists the
    /// toggled `sandbox.enabled` to user settings, or appends an `exclude`
    /// pattern to local settings, off-loop; the result returns via a
    /// [`TurnEvent::SystemNotice`] (same shape as `on_permission_action`). The
    /// live session flip already happened in the widget via the shared cell.
    pub on_sandbox_action: Box<dyn FnMut(crate::chat_widget::SandboxAction) + 'cb>,
    /// Executed on [`ChatOutcome::TaskAction`]: the caller stops the background
    /// task off-loop via `TaskRegistryHandle::kill` on the live runtime; the
    /// result returns via `TurnEvent::SystemNotice` (same shape as
    /// `on_permission_action`).
    pub on_task_action: Box<dyn FnMut(TaskAction) + 'cb>,
    /// Executed on [`ChatOutcome::DispatchSlash`]: the caller dispatches the
    /// carried raw input through the live `RegistrySlashDispatcher` off-loop
    /// (expanding a `type:"prompt"` command like `/loop` or a user command/skill
    /// and running it as a turn, or surfacing a local command's output / the
    /// unknown-command literal via a [`TurnEvent::SystemNotice`]). The widget has
    /// already echoed the invocation and registered the paired
    /// [`CancellationToken`] as its active turn, so the caller passes that token
    /// to `run_turn` (Ctrl-C then cancels the dispatched turn) and emits a
    /// `TurnEnded` for a non-turn result to clear the widget's running state.
    /// `None` (tests / no registry) is a no-op.
    pub on_dispatch_slash: Box<dyn FnMut(String, CancellationToken) + 'cb>,
}

/// Interactive chat runtime: the event-loop shell around [`ChatWidget`].
/// Runtime plumbing only — the widget owns every piece of conversation state.
pub struct RataApp<'cb> {
    /// The chat surface: transcript + bottom pane + session snapshot +
    /// per-turn state (plan Phase 6).
    chat_widget: ChatWidget,
    /// Streaming turn events from the orchestrator bridge, drained into the
    /// widget at the top of every tick.
    events_rx: UnboundedReceiver<TurnEvent>,
    /// Permission requests from the permission bridge, drained into the
    /// widget right after the turn events (the widget serializes prompts).
    permission_rx: Receiver<PermissionExchange>,
    /// Interactive AskUserQuestion exchanges, drained into the dedicated
    /// questionnaire view.
    ask_user_question_rx: Receiver<AskUserQuestionExchange>,
    /// Interactive `computer` tool `request_access` exchanges, drained into
    /// the dedicated approval view.
    computer_access_rx: Receiver<ComputerAccessExchange>,
    /// Off-thread clipboard-image paste results (`ChatOutcome::PasteImage`):
    /// the loop spawns the (slow) clipboard read + PNG encode on a worker
    /// thread and drains its result here each tick.
    paste_tx: std::sync::mpsc::Sender<Result<String, String>>,
    paste_rx: std::sync::mpsc::Receiver<Result<String, String>>,
    /// Embedder callbacks executed for app-level widget outcomes.
    callbacks: AppCallbacks<'cb>,
    /// Redraw cadence: the input-poll timeout, i.e. how long a tick waits for
    /// input before redrawing anyway (spinner animation, streamed deltas).
    redraw_interval: Duration,
}

impl<'cb> RataApp<'cb> {
    /// Build an app seeded with an initial conversation (may be empty), the
    /// startup [`SessionInfo`] snapshot, the event receivers the loop drains,
    /// and the embedder callbacks.
    #[must_use]
    pub fn new(
        messages: Vec<RenderedMessage>,
        session: SessionInfo,
        events_rx: UnboundedReceiver<TurnEvent>,
        permission_rx: Receiver<PermissionExchange>,
        ask_user_question_rx: Receiver<AskUserQuestionExchange>,
        computer_access_rx: Receiver<ComputerAccessExchange>,
        callbacks: AppCallbacks<'cb>,
    ) -> Self {
        let (paste_tx, paste_rx) = std::sync::mpsc::channel();
        Self {
            chat_widget: ChatWidget::new(messages, session),
            events_rx,
            permission_rx,
            ask_user_question_rx,
            computer_access_rx,
            paste_tx,
            paste_rx,
            callbacks,
            redraw_interval: Duration::from_millis(50),
        }
    }

    /// Run the event loop until the user quits. Per-tick order (locked by the
    /// Phase 1 handoff): drain turn events into the widget, drain permission
    /// requests into the widget, size the bottom viewport BEFORE flushing
    /// (history insertion wraps at the viewport width), flush finalized
    /// history to native scrollback, draw the widget, then route input and
    /// execute the returned callbacks.
    ///
    /// # Errors
    /// Propagates the first terminal IO error.
    pub fn run(&mut self, terminal: &mut RataTerminal) -> io::Result<AppExit> {
        loop {
            while let Ok(event) = self.events_rx.try_recv() {
                self.apply_turn_event(event);
            }
            // The widget serializes permission prompts (one owns the
            // keyboard; later arrivals queue), so the drain is unconditional.
            while let Ok(exchange) = self.permission_rx.try_recv() {
                self.open_permission(exchange);
            }
            while let Ok(exchange) = self.ask_user_question_rx.try_recv() {
                self.open_ask_user_question(exchange);
            }
            while let Ok(exchange) = self.computer_access_rx.try_recv() {
                self.open_computer_access(exchange);
            }
            // Off-thread clipboard-image paste results (Ctrl+V): attach the
            // temp PNG (or surface the error) as soon as the worker delivers.
            while let Ok(result) = self.paste_rx.try_recv() {
                self.chat_widget.clipboard_image_result(result);
            }
            // Flush a due non-bracketed paste burst (held first char renders
            // as typing; a completed burst lands as one paste). The pump
            // never submits, so the outcome needs no callback dispatch.
            let _ = self.chat_widget.pump_paste_burst();
            // Countdown-backed modal views must advance while the terminal is
            // idle. This runs before drawing so an expired questionnaire is
            // popped and its queued successor can render on the same tick.
            self.chat_widget.pump_view_timeout();
            // Hook-returned terminal escapes (`TurnEvent::TerminalSequence`,
            // already validated + BEL-normalized) write through to the tty
            // BEFORE the draw so the diff pass never interleaves with them.
            self.write_terminal_sequences(terminal)?;
            self.render_tick(terminal)?;
            // While burst state is pending (a held first char, an unflushed
            // buffer) the 8ms flush deadline must not wait out the full
            // redraw interval — a keystroke would echo up to ~50ms late.
            let poll_timeout = if self.chat_widget.paste_burst_pending() {
                Duration::from_millis(10)
            } else {
                self.redraw_interval
            };
            if event::poll(poll_timeout)? {
                let outcome = match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key),
                    Event::Paste(text) => self.on_paste(&text),
                    _ => ChatOutcome::Continue,
                };
                match outcome {
                    ChatOutcome::Quit => return Ok(AppExit::Quit),
                    ChatOutcome::ForceRedraw => {
                        self.chat_widget.reset_terminal_commit();
                        terminal.reset_for_replay()?;
                    }
                    // `/resume`: the picker resolved a session uuid. UNWIND the
                    // loop carrying it — the embedder re-mounts that session
                    // in-process (writer retargeted) rather than swapping the
                    // live engine in place (which would fork the JSONL file).
                    ChatOutcome::SwitchSession(uuid) => {
                        // Switch-safety: cancel any in-flight streaming turn on
                        // the OUTGOING runtime before unwinding, so a
                        // half-streamed turn does not keep writing into the
                        // session file the user just left. No-op when idle.
                        self.chat_widget.cancel_active_turn();
                        return Ok(AppExit::SwitchSession(uuid));
                    }
                    // `/branch`: same switch-safety as SwitchSession — stop any
                    // in-flight turn on the OUTGOING runtime before unwinding so
                    // the embedder can create and mount the branch.
                    ChatOutcome::BranchSession { title } => {
                        self.chat_widget.cancel_active_turn();
                        return Ok(AppExit::BranchSession { title });
                    }
                    // `/rewind`: same switch-safety as SwitchSession/Branch —
                    // stop the in-flight turn before unwinding so the embedder
                    // can rewind files / truncate the transcript and re-mount.
                    ChatOutcome::Rewind { message, scope } => {
                        self.chat_widget.cancel_active_turn();
                        return Ok(AppExit::Rewind { message, scope });
                    }
                    ChatOutcome::Submit(prompt, images, token) => {
                        // The queued images ride inside the Submit payload
                        // (they become `ContentBlock::Image` on the user
                        // message).
                        (self.callbacks.on_submit)(prompt, images, token);
                    }
                    ChatOutcome::PasteImage => {
                        // Clipboard image read + PNG encode can take hundreds
                        // of ms on a large screenshot — run it OFF the render
                        // thread; the result lands in `paste_rx` (drained at
                        // the top of every tick).
                        let tx = self.paste_tx.clone();
                        std::thread::spawn(move || {
                            let result = crate::clipboard_paste::paste_image_to_temp_png()
                                .map(|(path, _info)| path.display().to_string())
                                .map_err(|e| e.to_string());
                            let _ = tx.send(result);
                        });
                    }
                    ChatOutcome::SwitchModel(model, profile) => {
                        (self.callbacks.on_switch_model)(model, profile);
                    }
                    // A `/web` config effect: the caller persists/tests
                    // off-loop and pushes the result back as a
                    // `TurnEvent::SystemNotice`.
                    ChatOutcome::WebAction(action) => {
                        (self.callbacks.on_web_action)(action);
                    }
                    // A `/connect` effect: store a key or kick off a
                    // Copilot/OAuth sign-in off-loop, same shape as
                    // `WebAction` above.
                    ChatOutcome::ConnectAction(action) => {
                        (self.callbacks.on_connect_action)(action);
                    }
                    // A `/permissions` effect: persist the added/removed rule
                    // off-loop (and push a live allow rule); the result returns
                    // via `TurnEvent::SystemNotice`, same shape as `WebAction`.
                    ChatOutcome::PermissionAction(action) => {
                        (self.callbacks.on_permission_action)(action);
                    }
                    // A `/plugin` toggle: flip the on-disk `enabledPlugins`
                    // allowlist off-loop + refresh the snapshot; result via
                    // `TurnEvent::SystemNotice`, same shape as `PermissionAction`.
                    ChatOutcome::PluginAction(action) => {
                        (self.callbacks.on_plugin_action)(action);
                    }
                    // `/reload-plugins`: re-read the enabled set + apply pending
                    // plugin changes to the live session off-loop; the component
                    // tallies return via `TurnEvent::SystemNotice`.
                    ChatOutcome::ReloadPlugins => {
                        (self.callbacks.on_reload_plugins)();
                    }
                    // A `!`-prefixed bash-mode command: run it off the model
                    // path; the output returns via `TurnEvent::BashOutput`.
                    ChatOutcome::RunBash(command) => {
                        (self.callbacks.on_bash)(command);
                    }
                    // `/compact`: drive `force_compact` off-loop on the live
                    // engine runtime (never block the render thread); the
                    // summary returns via `TurnEvent::SystemNotice`.
                    ChatOutcome::Compact(args, cancel) => {
                        (self.callbacks.on_compact)(args, cancel);
                    }
                    // `/rename`: append the custom-title JSONL line off-loop on
                    // the live engine runtime; the confirmation returns via
                    // `TurnEvent::SystemNotice`, same shape as `Compact` above.
                    ChatOutcome::RenameSession(name) => {
                        (self.callbacks.on_rename)(name);
                    }
                    // `/fast`: flip the session fast-mode flag off-loop on the
                    // live engine runtime; the applied state returns via
                    // `TurnEvent::SystemNotice`, same shape as `Compact`.
                    ChatOutcome::FastMode(target) => {
                        (self.callbacks.on_fast_mode)(target);
                    }
                    // `/plan`: read + flip the session plan-mode flag off-loop on
                    // the live engine runtime; the applied state (or view
                    // message) returns via `TurnEvent::SystemNotice`, same shape
                    // as `FastMode`.
                    ChatOutcome::PlanMode(args) => {
                        (self.callbacks.on_plan_mode)(args);
                    }
                    // Shift+Tab cycled the permission mode: push the wire mode id
                    // to the live engine off-loop (`set_permission_mode`); the
                    // pane already updated its indicator.
                    ChatOutcome::SetPermissionMode(mode) => {
                        (self.callbacks.on_set_permission_mode)(mode);
                    }
                    // `/sandbox`: the live toggle already flipped in the widget;
                    // persist the choice / append an exclude off-loop, result via
                    // `TurnEvent::SystemNotice`.
                    ChatOutcome::SandboxAction(action) => {
                        (self.callbacks.on_sandbox_action)(action);
                    }
                    // `/tasks`: stop a running background task off-loop on the
                    // live runtime; the result returns via
                    // `TurnEvent::SystemNotice`, same shape as `SandboxAction`.
                    ChatOutcome::TaskAction(action) => {
                        (self.callbacks.on_task_action)(action);
                    }
                    // A registry-backed slash command (`/loop`, a user command,
                    // a skill, a plugin/bundled command): dispatch the raw input
                    // through the live `RegistrySlashDispatcher` off-loop; the
                    // expanded prompt runs as a turn (or the local output / the
                    // unknown-command literal returns via `TurnEvent::SystemNotice`).
                    ChatOutcome::DispatchSlash(input, token) => {
                        (self.callbacks.on_dispatch_slash)(input, token);
                    }
                    // The widget already applied the theme live; persist the
                    // preference best-effort (no-op on any IO failure).
                    ChatOutcome::SetTheme(setting) => {
                        tui_core::theme_persist::save_theme_setting(setting);
                    }
                    // The confirmation is already in the transcript; write the
                    // clipboard off-loop (fire-and-forget, like the iocraft
                    // backend's `pump_copy_clipboard`).
                    ChatOutcome::CopyToClipboard(text) => {
                        std::thread::spawn(move || {
                            crate::copy::copy_to_clipboard_native(&text);
                        });
                    }
                    ChatOutcome::Continue => {}
                }
            }
        }
    }

    /// Fold one streaming event from the orchestrator bridge into the chat
    /// widget's transcript/turn state (the `events_rx` drain path; see
    /// [`ChatWidget::apply_turn_event`]).
    fn apply_turn_event(&mut self, event: TurnEvent) {
        self.chat_widget.apply_turn_event(event);
        if let Some((args, token)) = self.chat_widget.take_ready_compact() {
            (self.callbacks.on_compact)(args, token);
        }
    }

    /// Open a permission prompt for `exchange` — or queue it when one is
    /// already open; prompts are serialized inside the widget (the
    /// `permission_rx` drain path; see [`ChatWidget::open_permission`]).
    fn open_permission(&mut self, exchange: PermissionExchange) {
        self.chat_widget.open_permission(exchange);
    }

    fn open_ask_user_question(&mut self, exchange: AskUserQuestionExchange) {
        self.chat_widget.open_ask_user_question(exchange);
    }

    fn open_computer_access(&mut self, exchange: ComputerAccessExchange) {
        self.chat_widget.open_computer_access(exchange);
    }

    /// Route one key press into the chat widget.
    fn on_key(&mut self, key: KeyEvent) -> ChatOutcome {
        self.chat_widget.handle_key(key)
    }

    /// Route a bracketed paste into the chat widget.
    fn on_paste(&mut self, text: &str) -> ChatOutcome {
        self.chat_widget.handle_paste(text)
    }

    /// Desired inline-viewport height at `width` columns: the widget reports
    /// its own height ([`ChatWidget::desired_height`]); the 4/20 clamp is
    /// deliberately app-side viewport policy.
    fn viewport_height(&self, width: u16) -> u16 {
        self.chat_widget.desired_height(width).clamp(4, 20)
    }

    /// Write the staged hook-returned terminal escape sequences
    /// (`TurnEvent::TerminalSequence`) straight to the terminal's writer —
    /// the host that owns the controlling tty (claude-code `BEo`; the old
    /// backend's async `pump_terminal_sequence` writing to stdout). The
    /// sequences are allowlisted OSC/BEL escapes that never move the cursor,
    /// so writing them between frames cannot corrupt the viewport diff.
    fn write_terminal_sequences<B: Backend + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        let sequences = self.chat_widget.take_terminal_sequences();
        if sequences.is_empty() {
            return Ok(());
        }
        let backend = terminal.backend_mut();
        for seq in sequences {
            backend.write_all(seq.as_bytes())?;
        }
        // Disambiguated: the raw writer flush (`io::Write`), not
        // `ratatui::backend::Backend::flush`.
        Write::flush(backend)
    }

    /// Commit finalized transcript cells into the terminal's native
    /// scrollback (see [`ChatWidget::flush_scrollback`]).
    fn flush_scrollback<B: Backend + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        self.chat_widget.flush_scrollback(terminal)
    }

    /// Draw one frame through the widget's render contract
    /// ([`ChatWidget::render_frame`] is the frame adapter).
    fn draw<B: Backend + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        let chat_widget = &mut self.chat_widget;
        terminal.draw(|frame| chat_widget.render_frame(frame))
    }

    /// One frame: viewport sizing, history flush, and the widget draw, all
    /// inside a synchronized-update bracket so the terminal applies the frame
    /// atomically (codex `Tui::draw`). The bracket must close even when a
    /// step fails — a dangling `?2026h` freezes the terminal.
    fn render_tick<B: Backend + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
    ) -> io::Result<()> {
        terminal.begin_sync_update()?;
        let result = (|| {
            let size = terminal.size()?;
            self.chat_widget.set_terminal_rows(size.height);
            terminal.set_bottom_viewport_height(self.viewport_height(size.width))?;
            self.flush_scrollback(terminal)?;
            self.draw(terminal)
        })();
        let end = terminal.end_sync_update();
        result.and(end)
    }
}

/// Run the interactive chat app on the bottom-anchored custom terminal:
/// history is committed to the terminal's native scrollback via
/// [`ChatWidget::flush_scrollback`]; the bottom viewport (status + composer +
/// overlays) is diff-redrawn each tick and resized in place via
/// [`crate::terminal::Terminal::set_bottom_viewport_height`] when its desired
/// height changes (composer growth / overlay open). Terminal modes are
/// restored on exit — including panics — by the [`TerminalSession`] guard and
/// the terminal's own drop (cursor style/visibility).
///
/// `subscription` is the composition root's shared subscription slot (seeded/
/// filled by its background fetch); the rate-limit composer reads the live
/// snapshot at compose time. Pass `None` when the embedder has no slot — the
/// copy degrades to the unknown-subscription default (TS-conservative).
///
/// `web_snapshot` is the composition root's shared `/web` config snapshot
/// slot (preloaded from real config + credential-store presence at startup);
/// `/web` reads a clone to seed the picker and the `on_web_action` effect
/// closure updates it after a save/test. Pass `None` when the embedder has no
/// slot — `/web` opens with the default (unconfigured) snapshot.
///
/// `connect_auth_methods`/`connect_availability` are the composition root's
/// real per-provider login-method + availability maps (derived from the live
/// multi-provider catalog at startup); `/connect` reads clones to build its
/// picker. Empty maps (the default) render an empty picker.
///
/// # Errors
/// Propagates the first terminal IO error (after restoring the terminal).
#[allow(clippy::too_many_arguments)]
pub fn run_app(
    messages: Vec<RenderedMessage>,
    initial_prompt: Option<String>,
    session: SessionInfo,
    events_rx: UnboundedReceiver<TurnEvent>,
    permission_rx: Receiver<PermissionExchange>,
    ask_user_question_rx: Receiver<AskUserQuestionExchange>,
    computer_access_rx: Receiver<ComputerAccessExchange>,
    subscription: Option<traits::subscription::SharedSubscription>,
    status_line: Option<crate::status_line::SharedStatusLine>,
    web_snapshot: Option<std::sync::Arc<std::sync::Mutex<crate::web::picker::WebConfigSnapshot>>>,
    permission_snapshot: Option<std::sync::Arc<std::sync::Mutex<PermissionsSnapshot>>>,
    plugin_snapshot: Option<
        std::sync::Arc<std::sync::Mutex<crate::bottom_pane::plugins_view::PluginsSnapshot>>,
    >,
    resume_rows: Vec<crate::resume::ResumeRow>,
    connect_auth_methods: std::collections::BTreeMap<String, String>,
    connect_availability: std::collections::BTreeMap<String, bool>,
    shell_expansion: Option<std::sync::Arc<dyn command_api::ShellExpansionProvider>>,
    orchestrator: Option<std::sync::Arc<dyn traits::OrchestratorHandle>>,
    sandbox_toggle: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    command_registry: Option<std::sync::Arc<tokio::sync::RwLock<command_api::CommandRegistry>>>,
    task_registry: Option<std::sync::Arc<dyn traits::task_registry::TaskRegistryHandle>>,
    // Persistent prompt-history store (`~/.lingxi/history.jsonl`, cc 2.1.218):
    // seeds the composer recall + persists submissions. `None` = session-local
    // recall only (tests, `CLAUDE_CODE_SKIP_PROMPT_HISTORY`).
    prompt_history: Option<std::sync::Arc<session::prompt_history::PromptHistoryStore>>,
    // The resolved boot permission mode + whether bypass is an available
    // Shift+Tab cycle target — seeds the below-composer mode indicator.
    initial_permission_mode: permission::PermissionMode,
    bypass_available: bool,
    emoji_completion_enabled: bool,
    on_submit: impl FnMut(String, Vec<std::path::PathBuf>, CancellationToken),
    on_switch_model: impl FnMut(String, Option<String>),
    on_web_action: impl FnMut(WebAction),
    on_connect_action: impl FnMut(ConnectAction),
    on_permission_action: impl FnMut(PermissionAction),
    on_plugin_action: impl FnMut(PluginAction),
    on_reload_plugins: impl FnMut(),
    on_bash: impl FnMut(String),
    on_compact: impl FnMut(String, CancellationToken),
    on_rename: impl FnMut(String),
    on_fast_mode: impl FnMut(Option<bool>),
    on_plan_mode: impl FnMut(String),
    on_set_permission_mode: impl FnMut(String),
    on_sandbox_action: impl FnMut(crate::chat_widget::SandboxAction),
    on_task_action: impl FnMut(TaskAction),
    on_dispatch_slash: impl FnMut(String, CancellationToken),
) -> io::Result<AppExit> {
    // Startup theme (production path only, keeping widget construction
    // hermetic for tests): OSC-11 background detection first — it manages
    // raw mode itself, so it runs BEFORE the session guard — then the
    // persisted preference (default `Auto`, resolved against the detection).
    let detached_pty = std::env::var_os("LINGXI_BG_PTY_CHILD").is_some();
    if !detached_pty {
        tui_core::theme_detect::detect_terminal_theme();
    }
    let startup_theme = tui_core::theme_persist::load_theme_setting()
        .unwrap_or(tui_core::theme::ThemeSetting::Auto);
    // Guard first, terminal second: locals drop in reverse order, so the
    // terminal resets the cursor while raw mode is still active, then the
    // guard restores cooked mode + bracketed paste.
    let _session_guard = TerminalSession::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = if detached_pty {
        crate::terminal::Terminal::with_options_at_origin(backend)?
    } else {
        crate::terminal::Terminal::with_options(backend)?
    };
    let mut app = RataApp::new(
        messages,
        session,
        events_rx,
        permission_rx,
        ask_user_question_rx,
        computer_access_rx,
        AppCallbacks {
            on_submit: Box::new(on_submit),
            on_switch_model: Box::new(on_switch_model),
            on_web_action: Box::new(on_web_action),
            on_connect_action: Box::new(on_connect_action),
            on_permission_action: Box::new(on_permission_action),
            on_plugin_action: Box::new(on_plugin_action),
            on_reload_plugins: Box::new(on_reload_plugins),
            on_bash: Box::new(on_bash),
            on_compact: Box::new(on_compact),
            on_rename: Box::new(on_rename),
            on_fast_mode: Box::new(on_fast_mode),
            on_plan_mode: Box::new(on_plan_mode),
            on_set_permission_mode: Box::new(on_set_permission_mode),
            on_sandbox_action: Box::new(on_sandbox_action),
            on_task_action: Box::new(on_task_action),
            on_dispatch_slash: Box::new(on_dispatch_slash),
        },
    );
    app.chat_widget.set_theme(startup_theme);
    // Persisted UI prefs (`/config verbose=…` / `vim=…`) read back from
    // settings.json, mirroring the `theme` load above.
    app.chat_widget.apply_startup_prefs(
        tui_core::theme_persist::load_verbose(),
        tui_core::theme_persist::load_editor_mode_is_vim(),
        tui_core::theme_persist::load_vim_insert_mode_remaps(),
    );
    app.chat_widget
        .set_emoji_completion_enabled(emoji_completion_enabled);
    if let Some(slot) = subscription {
        app.chat_widget.set_subscription(slot);
    }
    if let Some(slot) = status_line {
        app.chat_widget.set_status_line(slot);
    }
    if let Some(slot) = web_snapshot {
        app.chat_widget.set_web_snapshot(slot);
    }
    if let Some(slot) = permission_snapshot {
        app.chat_widget.set_permission_snapshot(slot);
    }
    if let Some(store) = prompt_history {
        app.chat_widget.set_prompt_history_store(store);
    }
    if let Some(slot) = plugin_snapshot {
        app.chat_widget.set_plugin_snapshot(slot);
    }
    app.chat_widget.set_resume_rows(resume_rows);
    app.chat_widget
        .set_connect_data(connect_auth_methods, connect_availability);
    // (#3) Wire the prompt shell-expansion provider so `/commit` … expand their
    // embedded `!`git …`` bodies in `run_core_command` before submit. `None`
    // (no embedder slot) keeps the historical verbatim path.
    if let Some(provider) = shell_expansion {
        app.chat_widget.set_shell_expansion(provider);
    }
    // Wire the live engine handle (drives `/context`, `/files`, `/usage`,
    // `/effort`, `/goal`, `/compact`) and the shared command registry (drives
    // `/reload-skills`). `None` (tests) keeps those commands graceful no-ops.
    if let Some(handle) = orchestrator {
        app.chat_widget.set_orchestrator(handle);
    }
    // Seed the below-composer permission-mode indicator from the resolved boot
    // mode (so `--dangerously-skip-permissions` shows `⏵⏵ bypass permissions on`).
    app.chat_widget
        .set_permission_mode(initial_permission_mode, bypass_available);
    // `/sandbox`: wire the shared bash-sandbox toggle cell (the same one the
    // bash tool reads). `None` (tests / unsupported host) keeps `/sandbox` a
    // graceful "unavailable" no-op.
    if let Some(toggle) = sandbox_toggle {
        app.chat_widget.set_sandbox_toggle(toggle);
    }
    if let Some(registry) = command_registry {
        app.chat_widget.set_command_registry(registry);
    }
    // `/tasks`: wire the live background-task registry so the picker can read a
    // snapshot. `None` (tests / no engine) keeps `/tasks` a graceful no-op.
    if let Some(handle) = task_registry {
        app.chat_widget.set_task_registry(handle);
    }
    // Agents-view settings (claude 2.1.220): `leftArrowOpensAgents`
    // (`kCt = Rt().leftArrowOpensAgents !== false`, default ON) gates the
    // ←-on-empty gesture; it is ANDed with the agent-view enablement gate,
    // because a disabled agent view fails `kGt`'s `Zan(C2t({fleetEnabled:
    // $H(), …}))` check and installs no handler at all.
    let agent_view_enabled = traits::agent_view::is_enabled();
    app.chat_widget.set_left_arrow_opens_agents(
        tui_core::theme_persist::load_left_arrow_opens_agents().unwrap_or(true)
            && agent_view_enabled,
    );
    // `defaultToAgentsView` ("Open agents view by default" / "Start in agent
    // view", default OFF): open the agents view over the fresh conversation at
    // startup — the oracle mounts the fleet view as the whole UI here
    // (`v = y().defaultToAgentsView === !0` … `tengu_fleetview`), empty or not.
    // Runs after the task registry is wired so the snapshot can see it.
    if tui_core::theme_persist::load_default_to_agents_view().unwrap_or(false) && agent_view_enabled
    {
        app.chat_widget.open_agents_view();
    }
    // A background PTY session can carry an initial prompt even though its
    // hidden child is deliberately launched without a positional prompt (a
    // positional prompt selects print mode at the CLI router). Submit it only
    // after the terminal and the complete interactive widget are mounted, and
    // route it through the same ChatWidget submission path as pressing Enter:
    // the user row, cancellation token and running-state bookkeeping must all
    // remain identical to an ordinary interactive turn.
    if let Some(prompt) = initial_prompt.filter(|value| !value.trim().is_empty()) {
        if let ChatOutcome::Submit(prompt, images, token) = app.chat_widget.submit_prompt(prompt) {
            (app.callbacks.on_submit)(prompt, images, token);
        }
    }
    app.run(&mut terminal)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};
    use permission::gate::{PermissionRequest, PermissionResponse};
    use tokio::sync::oneshot;

    use super::*;
    use crate::bottom_pane::model_picker_view::ModelPickerView;
    use crate::bottom_pane::screen_view::ScreenView;
    use crate::history_cell::attachments::UserImageCell;
    use crate::history_cell::message::{AssistantTextCell, UserTextCell};

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    /// An app over dummy (immediately closed) channels and no-op callbacks:
    /// behavior tests drive keys/events directly through the private seams
    /// the loop itself uses.
    fn test_app(messages: Vec<RenderedMessage>) -> RataApp<'static> {
        let (_events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_permission_tx, permission_rx) = tokio::sync::mpsc::channel(1);
        let (_ask_user_question_tx, ask_user_question_rx) = tokio::sync::mpsc::channel(1);
        let (_computer_access_tx, computer_access_rx) = tokio::sync::mpsc::channel(1);
        let app = RataApp::new(
            messages,
            SessionInfo::default(),
            events_rx,
            permission_rx,
            ask_user_question_rx,
            computer_access_rx,
            AppCallbacks {
                on_submit: Box::new(|_, _, _| {}),
                on_switch_model: Box::new(|_, _| {}),
                on_web_action: Box::new(|_| {}),
                on_connect_action: Box::new(|_| {}),
                on_permission_action: Box::new(|_| {}),
                on_plugin_action: Box::new(|_| {}),
                on_reload_plugins: Box::new(|| {}),
                on_bash: Box::new(|_| {}),
                on_compact: Box::new(|_, _| {}),
                on_rename: Box::new(|_| {}),
                on_fast_mode: Box::new(|_| {}),
                on_plan_mode: Box::new(|_| {}),
                on_set_permission_mode: Box::new(|_| {}),
                on_sandbox_action: Box::new(|_| {}),
                on_task_action: Box::new(|_| {}),
                on_dispatch_slash: Box::new(|_, _| {}),
            },
        );
        app
    }

    fn typ(app: &mut RataApp, s: &str) {
        for c in s.chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
    }

    /// The transcript's cells in order — committed then the active
    /// (streaming) cell — keeping the pre-Transcript `app.messages` indices
    /// for the behavior-lock assertions, now at cell level (the message-cells
    /// split replaced the old `messages()` reconstruction).
    fn cells<'a>(app: &'a RataApp<'_>) -> Vec<&'a dyn crate::history_cell::HistoryCell> {
        let mut out: Vec<&dyn crate::history_cell::HistoryCell> = app
            .chat_widget
            .transcript()
            .committed_cells()
            .iter()
            .map(AsRef::as_ref)
            .collect();
        out.extend(app.chat_widget.transcript().active_cell());
        out
    }

    /// Downcast transcript cell `idx` (committed order, active last) to its
    /// concrete cell type.
    fn cell<'a, T: 'static>(app: &'a RataApp<'_>, idx: usize) -> &'a T {
        cells(app)[idx]
            .as_any()
            .downcast_ref::<T>()
            .expect("concrete cell type")
    }

    #[test]
    fn alt_enter_inserts_newline_plain_enter_submits_whole_buffer() {
        let mut app = test_app(Vec::new());
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
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, ..) if p == "line one\nline two"));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "");
    }

    #[test]
    fn up_arrow_recalls_submitted_history() {
        let mut app = test_app(Vec::new());
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
        let mut app = test_app(Vec::new());
        typ(&mut app, "ac");
        app.on_key(press(KeyCode::Left)); // between a|c
        app.on_key(press(KeyCode::Char('b')));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "abc");
    }

    #[test]
    fn typing_then_submit_echoes_user_and_returns_prompt() {
        let mut app = test_app(Vec::new());
        for c in "hi".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "hi");
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, ..) if p == "hi"));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "");
        assert!(app.chat_widget.turn_running());
        assert_eq!(cells(&app).len(), 1);
        assert_eq!(cell::<UserTextCell>(&app, 0).body(), "hi");
    }

    #[test]
    fn empty_submit_is_ignored() {
        let mut app = test_app(Vec::new());
        assert!(matches!(
            app.on_key(press(KeyCode::Enter)),
            ChatOutcome::Continue
        ));
        assert!(cells(&app).is_empty());
    }

    #[test]
    fn streaming_deltas_grow_reply_and_turn_ended_clears_token() {
        let mut app = test_app(Vec::new());
        app.on_key(press(KeyCode::Char('x')));
        app.on_key(press(KeyCode::Enter));
        assert!(app.chat_widget.turn_running());
        app.apply_turn_event(TurnEvent::TurnStarted);
        app.apply_turn_event(TurnEvent::TextDelta("Hel".to_string()));
        app.apply_turn_event(TurnEvent::TextDelta("lo".to_string()));
        assert_eq!(cell::<AssistantTextCell>(&app, 1).body(), "Hello");
        app.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        assert!(!app.chat_widget.turn_running());
    }

    #[test]
    fn ctrl_c_cancels_turn_then_needs_two_presses_to_quit() {
        let mut app = test_app(Vec::new());
        app.on_key(press(KeyCode::Char('x')));
        let ChatOutcome::Submit(_, _, token) = app.on_key(press(KeyCode::Enter)) else {
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
        let mut app = test_app(Vec::new());
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
        let mut app = test_app(Vec::new());
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
        let mut app = test_app(Vec::new());
        assert!(!app.chat_widget.transcript().verbose());
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('o'))),
            ChatOutcome::ForceRedraw
        ));
        assert!(app.chat_widget.transcript().verbose());
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('o'))),
            ChatOutcome::ForceRedraw
        ));
        assert!(!app.chat_widget.transcript().verbose());
    }

    #[test]
    fn viewport_height_grows_for_overlays() {
        let mut app = test_app(Vec::new());
        let base = app.viewport_height(80);
        // Opening the completion popup grows the viewport.
        typ(&mut app, "/");
        assert!(app.chat_widget.bottom_pane().completion().is_some());
        assert!(app.viewport_height(80) > base);
    }

    #[test]
    fn paste_non_image_inserts_into_composer() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "pre ");
        app.on_paste("hello world");
        assert_eq!(
            app.chat_widget.bottom_pane().composer().text(),
            "pre hello world"
        );
        // A non-existent image path is treated as text, not an image message.
        app.on_paste(" /no/such/file.png ");
        assert!(cells(&app).is_empty());
    }

    #[test]
    fn slash_image_pushes_image_message() {
        let path =
            std::env::temp_dir().join(format!("tui-app-slash-image-{}.png", std::process::id()));
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").expect("write fixture image");
        let mut app = test_app(Vec::new());
        let outcome = submit_command(&mut app, &format!("/image {}", path.display()));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert_eq!(cells(&app).len(), 1);
        let image = cell::<UserImageCell>(&app, 0);
        assert_eq!(
            image.source_path(),
            Some(path.display().to_string().as_str())
        );
        assert_eq!(image.metadata(), path.file_name().and_then(|n| n.to_str()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn slash_vim_toggles_vim_mode() {
        let mut app = test_app(Vec::new());
        assert!(!app.chat_widget.bottom_pane().vim_enabled());
        submit_command(&mut app, "/vim");
        assert!(app.chat_widget.bottom_pane().vim_enabled());
        submit_command(&mut app, "/vim");
        assert!(!app.chat_widget.bottom_pane().vim_enabled());
    }

    #[test]
    fn vim_esc_enters_normal_and_motions_edit_instead_of_typing() {
        let mut app = test_app(Vec::new());
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
        let mut app = test_app(Vec::new());
        submit_command(&mut app, "/vim");
        typ(&mut app, "hi");
        app.on_key(press(KeyCode::Esc)); // → Normal
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, ..) if p == "hi"));
    }

    #[test]
    fn ctrl_left_right_move_by_word() {
        let mut app = test_app(Vec::new());
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
        let mut app = test_app(Vec::new());
        app.on_key(press(KeyCode::Char('/')));
        assert!(app.chat_widget.bottom_pane().completion().is_some());
        typ(&mut app, "m"); // "/m" narrows to the m-matching commands
        let p = app.chat_widget.bottom_pane().completion().unwrap();
        // Prefix matches rank shorter-name first (claude-code comparator).
        assert_eq!(p.selected_insert(), "/mcp");
        // A space ends the command token and closes the popup.
        typ(&mut app, " x");
        assert!(app.chat_widget.bottom_pane().completion().is_none());
    }

    #[test]
    fn tab_completes_selected_command_into_composer() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "/mc");
        assert!(app.chat_widget.bottom_pane().completion().is_some());
        app.on_key(press(KeyCode::Tab));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "/mcp");
    }

    #[test]
    fn palette_arrows_navigate_and_esc_dismisses_without_quitting() {
        let mut app = test_app(Vec::new());
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
        let mut app = test_app(Vec::new());
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
        let mut app = test_app(Vec::new());
        assert!(matches!(app.on_key(press(KeyCode::Esc)), ChatOutcome::Quit));
    }

    // ===== Layered Ctrl-C/Esc routing (acceptance criterion 14, plan Phase 7):
    // active view first, composer/completion second, chat-widget
    // interrupt/quit policy last. =====

    #[test]
    fn esc_interrupts_running_turn_then_quits_when_idle() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "go");
        let ChatOutcome::Submit(_, _, token) = app.on_key(press(KeyCode::Enter)) else {
            panic!("expected submit");
        };
        app.apply_turn_event(TurnEvent::TurnStarted);
        // No view, no completion: Esc reaches the interrupt/quit policy layer
        // — running, so it interrupts (the spinner's "esc to interrupt").
        assert!(matches!(
            app.on_key(press(KeyCode::Esc)),
            ChatOutcome::Continue
        ));
        assert!(token.is_cancelled());
        assert!(!app.chat_widget.turn_running());
        // Idle now: the same key falls through to the quit policy.
        assert!(matches!(app.on_key(press(KeyCode::Esc)), ChatOutcome::Quit));
    }

    #[test]
    fn esc_routes_to_active_view_before_the_interrupt_policy() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "go");
        let ChatOutcome::Submit(_, _, token) = app.on_key(press(KeyCode::Enter)) else {
            panic!("expected submit");
        };
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        // Layer 1 — the active view owns Esc: the permission resolves (deny);
        // the running turn is untouched.
        assert!(matches!(
            app.on_key(press(KeyCode::Esc)),
            ChatOutcome::Continue
        ));
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
        assert!(!token.is_cancelled(), "view-owned Esc must not interrupt");
        assert!(app.chat_widget.turn_running());
        // Layer 3 — with no view left, Esc interrupts the turn.
        assert!(matches!(
            app.on_key(press(KeyCode::Esc)),
            ChatOutcome::Continue
        ));
        assert!(token.is_cancelled());
        // And once idle, Esc quits.
        assert!(matches!(app.on_key(press(KeyCode::Esc)), ChatOutcome::Quit));
    }

    #[test]
    fn esc_dismisses_completion_before_the_interrupt_policy() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "go");
        let ChatOutcome::Submit(_, _, token) = app.on_key(press(KeyCode::Enter)) else {
            panic!("expected submit");
        };
        // Layer 2 — the completion popup owns Esc while open.
        typ(&mut app, "/");
        assert!(app.chat_widget.bottom_pane().completion().is_some());
        assert!(matches!(
            app.on_key(press(KeyCode::Esc)),
            ChatOutcome::Continue
        ));
        assert!(app.chat_widget.bottom_pane().completion().is_none());
        assert!(!token.is_cancelled(), "popup-owned Esc must not interrupt");
        assert!(app.chat_widget.turn_running());
        // Layer 3 — the next Esc reaches the policy layer and interrupts.
        assert!(matches!(
            app.on_key(press(KeyCode::Esc)),
            ChatOutcome::Continue
        ));
        assert!(token.is_cancelled());
    }

    #[test]
    fn ctrl_c_routes_to_active_view_before_the_interrupt_policy() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "go");
        let ChatOutcome::Submit(_, _, token) = app.on_key(press(KeyCode::Enter)) else {
            panic!("expected submit");
        };
        let (exchange, _resp_rx) = tool_exchange();
        app.open_permission(exchange);
        // Layer 1 — the active view swallows Ctrl-C (it owns the keyboard
        // until resolved): no interrupt, no quit, prompt still open.
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('c'))),
            ChatOutcome::Continue
        ));
        assert!(app.chat_widget.has_open_permission());
        assert!(
            !token.is_cancelled(),
            "view-owned Ctrl-C must not interrupt"
        );
        // Resolve the prompt ('1' = allow once); layer 3 then interrupts.
        app.on_key(press(KeyCode::Char('1')));
        assert!(!app.chat_widget.has_open_permission());
        assert!(matches!(
            app.on_key(ctrl(KeyCode::Char('c'))),
            ChatOutcome::Continue
        ));
        assert!(token.is_cancelled());
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
        let mut app = test_app(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        assert!(app.chat_widget.has_open_permission());

        // While a prompt is open, normal keys are swallowed by the dialog and
        // never reach the composer.
        app.on_key(press(KeyCode::Char('x')));
        assert_eq!(app.chat_widget.bottom_pane().composer().text(), "");

        // Enter selects the highlighted option (index 0 = AllowOnce).
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(!app.chat_widget.has_open_permission());
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowOnce
        );
    }

    #[test]
    fn permission_prompt_esc_denies() {
        let mut app = test_app(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        let outcome = app.on_key(press(KeyCode::Esc));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(!app.chat_widget.has_open_permission());
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn permission_prompt_number_three_denies() {
        let mut app = test_app(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        // '3' shortcut = third option = Deny.
        app.on_key(press(KeyCode::Char('3')));
        assert!(!app.chat_widget.has_open_permission());
        assert_eq!(resp_rx.blocking_recv().unwrap(), PermissionResponse::Deny);
    }

    #[test]
    fn slash_help_opens_screen_view_without_sending_a_prompt() {
        let mut app = test_app(Vec::new());
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
        assert!(cells(&app).is_empty(), "no scrollback dump");
        assert!(!app.chat_widget.turn_running());
    }

    #[test]
    fn screen_view_owns_keys_scrolls_and_esc_closes_without_quitting() {
        let mut app = test_app(Vec::new());
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
        let mut app = test_app(Vec::new());
        submit_command(&mut app, "/help");
        let outcome = app.on_key(press(KeyCode::Char('q')));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(app.chat_widget.bottom_pane().view_stack().is_empty());
    }

    #[test]
    fn non_command_slash_input_is_sent_as_a_prompt() {
        let mut app = test_app(Vec::new());
        for c in "/frobnicate".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        let outcome = app.on_key(press(KeyCode::Enter));
        // Unrecognized slash command falls through as a normal prompt.
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, ..) if p == "/frobnicate"));
        assert_eq!(cells(&app).len(), 1);
    }

    fn submit_command(app: &mut RataApp, cmd: &str) -> ChatOutcome {
        for c in cmd.chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        app.on_key(press(KeyCode::Enter))
    }

    #[test]
    fn slash_clear_empties_messages() {
        let mut app = test_app(vec![RenderedMessage::SystemText {
            body: "old".to_string(),
            timestamp: 0,
            is_error: false,
        }]);
        let outcome = submit_command(&mut app, "/clear");
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(cells(&app).is_empty());
        assert_eq!(app.chat_widget.transcript().committed_to_terminal(), 0);
    }

    #[test]
    fn slash_exit_quits() {
        let mut app = test_app(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/exit"),
            ChatOutcome::Quit
        ));
    }

    #[test]
    fn slash_doctor_and_mcp_open_screen_views() {
        let mut app = test_app(Vec::new());
        // `/doctor` was removed in claude-code 2.1.205 (it lives on as a
        // bundled skill): it no longer resolves as a slash command.
        assert!(crate::command::resolve("/doctor").is_none());
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
        assert!(cells(&app).is_empty());
        assert!(!app.chat_widget.turn_running());
    }

    fn app_with_models() -> RataApp<'static> {
        let mut app = test_app(Vec::new());
        app.chat_widget.set_session(crate::session::SessionInfo {
            models: vec![
                crate::session::ModelRow {
                    display: "Opus".into(),
                    request_model: "claude-opus-4-8".into(),
                    profile: Some("anthropic".into()),
                    provider_label: "Anthropic".into(),
                    is_current: true,
                    supports_reasoning: true,
                },
                crate::session::ModelRow {
                    display: "Sonnet".into(),
                    request_model: "claude-sonnet-5".into(),
                    profile: Some("anthropic".into()),
                    provider_label: "Anthropic".into(),
                    is_current: false,
                    supports_reasoning: true,
                },
            ],
            ..Default::default()
        });
        // The /model picker gates by live provider availability: anthropic must
        // be connected for its (curated) models to show.
        app.chat_widget.set_connect_data(
            std::collections::BTreeMap::new(),
            [("anthropic".to_string(), true)].into_iter().collect(),
        );
        app
    }

    #[test]
    fn slash_model_with_no_models_opens_picker_with_in_view_empty_message() {
        // Plan Phase 11 step 5 (deliberate behavior change from the Phase 0
        // lock): the empty state moved INTO the picker view — `/model` with
        // no models opens the picker showing its own message instead of
        // dumping a transcript line.
        let mut app = test_app(Vec::new());
        assert!(matches!(
            submit_command(&mut app, "/model"),
            ChatOutcome::Continue
        ));
        assert!(app
            .chat_widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        assert!(cells(&app).is_empty(), "no scrollback dump");
        let terminal = draw_viewport(&mut app);
        let all = buffer_rows(&terminal).join("\n");
        assert!(
            all.contains("No models available. Configure a provider to enable /model."),
            "{all}"
        );
        // Esc closes back to the composer without switching anything.
        assert!(matches!(
            app.on_key(press(KeyCode::Esc)),
            ChatOutcome::Continue
        ));
        assert!(app.chat_widget.bottom_pane().view_stack().is_empty());
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
                if m == "claude-opus-4-8" && p.as_deref() == Some("anthropic")
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
        assert!(matches!(outcome, ChatOutcome::SwitchModel(ref m, _) if m == "claude-opus-4-8"));
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

    /// Draw the bottom viewport at its self-reported height (80 columns)
    /// through the app's own draw path ([`RataApp::draw`] →
    /// [`ChatWidget::render_frame`]); returns the terminal for buffer/cursor
    /// inspection.
    fn draw_viewport(app: &mut RataApp) -> Terminal<TestWriteBackend> {
        let mut terminal = inline_test_terminal(app.viewport_height(80));
        app.draw(&mut terminal).expect("draw");
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
        let mut app = test_app(Vec::new());
        typ(&mut app, "  hi there  ");
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Submit(ref p, ..) if p == "hi there"));
        assert_eq!(cell::<UserTextCell>(&app, 0).body(), "hi there");
    }

    #[test]
    fn whitespace_only_submit_is_ignored() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "   ");
        assert!(matches!(
            app.on_key(press(KeyCode::Enter)),
            ChatOutcome::Continue
        ));
        assert!(cells(&app).is_empty());
        assert!(!app.chat_widget.turn_running());
    }

    #[test]
    fn slash_hooks_agents_and_quit_route_as_commands() {
        let mut app = test_app(Vec::new());
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
                                         // `/agents` prints the 2.1.205 removed-notice (no picker view).
        assert!(matches!(
            submit_command(&mut app, "/agents"),
            ChatOutcome::Continue
        ));
        assert!(app.chat_widget.bottom_pane().view_stack().is_empty());
        let agents_cells = cells(&app);
        assert_eq!(agents_cells.len(), 1, "one removed-notice cell");
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
        let mut app = test_app(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        app.on_key(press(KeyCode::Down)); // highlight "Yes, allow always"
        let outcome = app.on_key(press(KeyCode::Enter));
        assert!(matches!(outcome, ChatOutcome::Continue));
        assert!(!app.chat_widget.has_open_permission());
        assert_eq!(
            resp_rx.blocking_recv().unwrap(),
            PermissionResponse::AllowAlways
        );
    }

    #[test]
    fn permission_resolution_is_single_shot_and_releases_keyboard() {
        let mut app = test_app(Vec::new());
        let (exchange, resp_rx) = tool_exchange();
        app.open_permission(exchange);
        // '1' shortcut resolves with the first option (AllowOnce)…
        app.on_key(press(KeyCode::Char('1')));
        assert!(!app.chat_widget.has_open_permission());
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
            ChatOutcome::Submit(ref p, ..) if p == "x"
        ));
    }

    #[test]
    fn paste_existing_image_path_becomes_image_message() {
        let path =
            std::env::temp_dir().join(format!("tui-rata-p0-paste-{}.png", std::process::id()));
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").expect("write fixture image");
        let mut app = test_app(Vec::new());
        // Surrounding whitespace is trimmed for detection AND stored path.
        app.on_paste(&format!(" {} ", path.display()));
        std::fs::remove_file(&path).ok();
        assert_eq!(
            app.chat_widget.bottom_pane().composer().text(),
            "",
            "image paste must not touch composer"
        );
        assert_eq!(cells(&app).len(), 1);
        let file_name = path.file_name().unwrap().to_str().unwrap();
        let image = cell::<UserImageCell>(&app, 0);
        assert_eq!(
            image.source_path(),
            Some(path.display().to_string().as_str())
        );
        assert_eq!(
            image.metadata(),
            Some(file_name),
            "metadata is the file name"
        );
    }

    #[test]
    fn flush_scrollback_holds_streaming_tail_until_turn_ends() {
        let mut app = test_app(Vec::new());
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
        let mut app = test_app(vec![
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
    fn resume_seeded_transcript_flushes_every_message_to_scrollback() {
        // Bug 1 (live QA "resume 没有回复所有的messages"): a resumed session
        // seeds N prior messages as committed cells. The first real frame
        // (`render_tick`: viewport sizing + flush + draw) must flush ALL of
        // them into native scrollback AND their text must reach the tty byte
        // stream — not merely advance the commit cursor.
        let mut msgs = Vec::new();
        for i in 0..15 {
            msgs.push(RenderedMessage::UserText {
                body: format!("USERMSG{i:02}"),
                timestamp: 0,
            });
            msgs.push(RenderedMessage::AssistantText {
                body: format!("ASSTMSG{i:02}"),
                timestamp: 0,
            });
        }
        let total = msgs.len();
        let mut app = test_app(msgs);
        let backend = crate::terminal::test_support::TestWriteBackend::new(80, 24);
        let raw = backend.raw_handle();
        let mut terminal = crate::terminal::Terminal::with_options(backend).unwrap();
        app.render_tick(&mut terminal).unwrap();
        assert_eq!(
            app.chat_widget.transcript().committed_to_terminal(),
            total,
            "every resumed cell must flush on the first frame"
        );
        let out = String::from_utf8_lossy(&raw.borrow()).into_owned();
        for i in 0..15 {
            assert!(
                out.contains(&format!("USERMSG{i:02}")),
                "resumed user msg {i} never reached the scrollback byte stream"
            );
            assert!(
                out.contains(&format!("ASSTMSG{i:02}")),
                "resumed assistant msg {i} never reached the scrollback byte stream"
            );
        }
    }

    #[test]
    fn running_render_emits_no_degenerate_scroll_region() {
        // Regression: inserting the first history line above a top-anchored
        // viewport (fresh session) produced a DEGENERATE `ESC[1;1r` scroll region
        // (a 1-row DECSTBM, top == bottom), which iTerm2 mishandled by leaking a
        // stray `[` glyph onto the status + composer rows. Every emitted scroll
        // region must have top < bottom.
        let backend = crate::terminal::test_support::TestWriteBackend::new(80, 24);
        let raw = backend.raw_handle();
        let mut terminal = crate::terminal::Terminal::with_options(backend).unwrap();
        let mut app = test_app(Vec::new());
        typ(&mut app, "hello");
        app.on_key(press(KeyCode::Enter));
        app.apply_turn_event(TurnEvent::TurnStarted);
        app.render_tick(&mut terminal).unwrap();
        app.render_tick(&mut terminal).unwrap();

        let out = String::from_utf8_lossy(&raw.borrow()).into_owned();
        let mut rest = out.as_str();
        while let Some(i) = rest.find("\x1b[") {
            rest = &rest[i + 2..];
            let seq: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == ';')
                .collect();
            let terminated_by_r = rest[seq.len()..].starts_with('r');
            if terminated_by_r {
                if let Some((top, bottom)) = seq.split_once(';') {
                    let (top, bottom): (u16, u16) = (top.parse().unwrap(), bottom.parse().unwrap());
                    assert!(
                        top < bottom,
                        "degenerate scroll region ESC[{seq}r (top must be < bottom)"
                    );
                }
            }
        }
    }

    #[test]
    fn full_turn_then_idle_emits_no_degenerate_scroll_region() {
        // Regression (reported via live iTerm2, screenshot): a stray `[` prefix
        // persisted on the composer AFTER a completed exchange on a fresh
        // session. The earlier running-render fix guarded ONE scroll-region site
        // (`insert_history_lines`), but the reply / turn-end / idle phase still
        // emitted a degenerate `ESC[N;Nr` from another site. Drive the WHOLE
        // scenario (fresh session → user msg → streamed reply → turn end → idle
        // flush) and assert EVERY scroll region has top < bottom.
        for height in [24u16, 12, 10, 8, 6, 5, 4, 3] {
            let backend = crate::terminal::test_support::TestWriteBackend::new(80, height);
            let raw = backend.raw_handle();
            let mut terminal = crate::terminal::Terminal::with_options(backend).unwrap();
            let mut app = test_app(Vec::new());
            // A LONG prompt wraps to several composer rows → grows the bottom
            // viewport → `set_bottom_viewport_height` → `scroll_region_up`.
            typ(
                &mut app,
                "hello there this is a fairly long line of input that will wrap across several rows",
            );
            app.on_key(press(KeyCode::Enter));
            app.apply_turn_event(TurnEvent::TurnStarted);
            app.apply_turn_event(TurnEvent::TextDelta(
                "Hello! How can I help you today?".to_string(),
            ));
            app.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
            app.render_tick(&mut terminal).unwrap();
            app.flush_scrollback(&mut terminal).unwrap();
            app.render_tick(&mut terminal).unwrap();

            let out = String::from_utf8_lossy(&raw.borrow()).into_owned();
            let mut rest = out.as_str();
            while let Some(i) = rest.find("\x1b[") {
                rest = &rest[i + 2..];
                let seq: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == ';')
                    .collect();
                if rest[seq.len()..].starts_with('r') && seq.contains(';') {
                    let (top, bottom) = seq.split_once(';').unwrap();
                    let (top, bottom): (u16, u16) = (top.parse().unwrap(), bottom.parse().unwrap());
                    assert!(
                        top < bottom,
                        "height {height}: degenerate scroll region ESC[{seq}r (top must be < bottom) — leaks a stray `[` on iTerm2"
                    );
                }
            }
        }
    }

    #[test]
    fn tick_frame_is_bracketed_in_a_synchronized_update() {
        let backend = crate::terminal::test_support::TestWriteBackend::new(80, 24);
        let raw = backend.raw_handle();
        let mut terminal = crate::terminal::Terminal::with_options(backend).unwrap();
        let mut app = test_app(Vec::new());
        app.render_tick(&mut terminal).unwrap();
        let out = String::from_utf8_lossy(&raw.borrow()).into_owned();
        let begin = out.find("\x1b[?2026h").expect("begin synchronized update");
        let end = out.rfind("\x1b[?2026l").expect("end synchronized update");
        assert!(begin < end, "bracket must open before it closes");
        // The viewport sizing + draw escapes all land INSIDE the bracket.
        assert!(
            out[..begin].find("\x1b[").is_none(),
            "no escapes before the bracket: {out:?}"
        );
    }

    #[test]
    fn viewport_grows_with_multiline_composer_up_to_cap() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "one");
        for _ in 0..9 {
            app.on_key(alt(KeyCode::Enter));
        }
        // 10 content lines clamp at composer::MAX_VISIBLE_LINES (6): 1 + 6 + 2 = 9.
        assert_eq!(app.viewport_height(80), 9);
    }

    #[test]
    fn layout_80x24_idle_status_line_plus_borderless_composer() {
        let mut app = test_app(Vec::new());
        assert_eq!(app.viewport_height(80), 4, "idle bottom viewport is 4 rows");
        let terminal = draw_viewport(&mut app);
        let rows = buffer_rows(&terminal);
        // Row 0 is the composer's top padding (idle has no leading status
        // row — the key hints moved to the LAST row as the footer).
        assert!(
            !rows[0].contains('┌'),
            "composer top padding has no border: {}",
            rows[0]
        );
        assert!(rows[1].starts_with('›'), "prompt row: {}", rows[1]);
        assert!(
            !rows[2].contains('└'),
            "composer bottom padding has no border: {}",
            rows[2]
        );
        assert!(rows[3].contains("Enter: send"), "footer row: {}", rows[3]);
        assert!(rows[3].contains("Esc: quit"), "footer row: {}", rows[3]);
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
        let mut app = test_app(Vec::new());
        typ(&mut app, "你好");
        let mut terminal = draw_viewport(&mut app);
        let pos = terminal.get_cursor_position().unwrap();
        // x = gutter inner.x(2) + two wide chars × 2 columns = 6; y = row 1
        // (idle has no leading status row — the composer's top padding is
        // row 0, the prompt row is row 1).
        assert_eq!((pos.x, pos.y), (6, 1));
    }

    #[test]
    fn layout_running_turn_shows_spinner_status_with_interrupt_hint() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "go");
        app.on_key(press(KeyCode::Enter));
        app.apply_turn_event(TurnEvent::TurnStarted);
        let terminal = draw_viewport(&mut app);
        let rows = buffer_rows(&terminal);
        // The just-opened active cell is empty → renders NO tail row (no stray
        // `●` before content), so the status row is the top row (row 0).
        assert!(rows[0].contains("esc to interrupt"), "status: {}", rows[0]);
        assert!(rows[0].contains("Ctrl-C: cancel"), "status: {}", rows[0]);
    }

    #[test]
    fn layout_streaming_tail_is_visible_above_the_pane_before_turn_ends() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "go");
        app.on_key(press(KeyCode::Enter));
        app.apply_turn_event(TurnEvent::TurnStarted);
        app.apply_turn_event(TurnEvent::TextDelta("streamed reply words".to_string()));
        // The viewport grows for the tail: running pane (5: status + composer
        // 3 + footer) + one tail row.
        assert_eq!(app.viewport_height(80), 6, "tail grows the viewport");
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
        let mut app = test_app(Vec::new());
        app.on_key(ctrl(KeyCode::Char('c')));
        let terminal = draw_viewport(&mut app);
        let rows = buffer_rows(&terminal);
        // Idle: the reminder lives in the footer, the LAST row.
        assert!(
            rows.last().unwrap().contains("Press Ctrl-C again to exit"),
            "footer: {rows:?}"
        );
    }

    #[test]
    fn layout_completion_popup_grows_viewport_and_draws_over_it() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "/");
        assert!(app.chat_widget.bottom_pane().completion().is_some());
        // composer 3 + full-registry popup 8 (the popup REPLACES the footer
        // below the composer; no leading status row while idle).
        assert_eq!(app.viewport_height(80), 11, "completion viewport height");
        let terminal = draw_viewport(&mut app);
        let all = buffer_rows(&terminal).join("\n");
        assert!(all.contains("Complete"), "popup title visible:\n{all}");
    }

    #[test]
    fn layout_permission_dialog_overlays_viewport() {
        let mut app = test_app(Vec::new());
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
        // 2 Anthropic models + 1 provider header + 1 Search row + 4 modal chrome = 8.
        assert_eq!(app.viewport_height(80), 8, "picker viewport height");
        let terminal = draw_viewport(&mut app);
        let all = buffer_rows(&terminal).join("\n");
        assert!(all.contains("Select model"), "{all}");
        assert!(all.contains("Search:"), "{all}");
        assert!(all.contains("Opus"), "{all}");
        assert!(all.contains("Sonnet"), "{all}");
    }

    #[test]
    fn layout_help_screen_fills_viewport_without_status_or_composer() {
        let mut app = test_app(Vec::new());
        submit_command(&mut app, "/help");
        // The help body wants more rows than the viewport allows: clamps at
        // the 20-row viewport cap (new Phase 4 lock).
        assert_eq!(app.viewport_height(80), 20, "help screen viewport height");
        let terminal = draw_viewport(&mut app);
        let rows = buffer_rows(&terminal);
        assert_eq!(rows.len(), 20);
        let all = rows.join("\n");
        assert!(all.contains("Shortcuts"), "{all}");
        assert!(all.contains("for commands"), "{all}");
        // Full-frame view: no status hints, no composer prompt beneath — the
        // borderless composer's `›` gutter prompt renders at column 0 of its
        // pane, so no help row may start with it.
        assert!(!all.contains("Enter: send"), "status suppressed:\n{all}");
        assert!(
            !rows.iter().any(|r| r.starts_with('›')),
            "composer suppressed:\n{all}"
        );
        // The view claims no cursor, so the draw hides it.
        assert!(terminal.cursor_hidden(), "screen view hides the cursor");
    }

    #[test]
    fn terminal_restores_cursor_style_even_when_a_draw_panics() {
        // Panic-safety smoke for the run-loop's restore guarantees: `run_app`
        // declares the `TerminalSession` guard before the terminal, so an
        // unwinding panic drops the terminal (cursor style/visibility reset)
        // and then the guard (raw mode + bracketed paste — untestable here:
        // it needs a real tty). This exercises the terminal half through the
        // app's own tick path.
        let backend = TestWriteBackend::new(80, 24);
        let raw = backend.raw_handle();
        let observed = std::rc::Rc::clone(&raw);
        let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut app = test_app(Vec::new());
            let mut terminal = Terminal::with_options(backend).expect("terminal");
            // One healthy tick: viewport sizing, flush, draw.
            terminal
                .set_bottom_viewport_height(app.viewport_height(80))
                .expect("viewport");
            app.flush_scrollback(&mut terminal).expect("flush");
            app.draw(&mut terminal).expect("draw");
            // Only the panicking draw + unwind cleanup from here on.
            raw.borrow_mut().clear();
            let _ = terminal.draw(|_frame| panic!("render panic"));
        }));
        assert!(panic_result.is_err(), "the draw panic must propagate");
        let bytes = observed.borrow().clone();
        let escapes = String::from_utf8_lossy(&bytes);
        assert!(
            escapes.contains("\x1b[0 q"),
            "terminal drop must reset the cursor style during unwind; got: {escapes:?}"
        );
    }

    #[test]
    fn staged_terminal_sequences_write_through_to_the_terminal_raw_stream() {
        // Fix round 1: `TurnEvent::TerminalSequence` (hook-returned, already
        // validated) must reach the tty byte stream — the tick drains the
        // widget's stage and writes it to the terminal's writer verbatim.
        let mut app = test_app(Vec::new());
        let backend = TestWriteBackend::new(80, 24);
        let raw = backend.raw_handle();
        let mut terminal = Terminal::with_options(backend).expect("terminal");
        app.apply_turn_event(TurnEvent::TerminalSequence {
            seq: "\u{1b}]0;lingxi title\u{7}".to_string(),
        });
        app.apply_turn_event(TurnEvent::TerminalSequence {
            seq: "\u{1b}]9;done\u{7}".to_string(),
        });
        raw.borrow_mut().clear();
        app.write_terminal_sequences(&mut terminal).expect("write");
        let bytes = raw.borrow().clone();
        let out = String::from_utf8_lossy(&bytes);
        let title = out
            .find("\u{1b}]0;lingxi title\u{7}")
            .expect("title escape");
        let notify = out.find("\u{1b}]9;done\u{7}").expect("notify escape");
        assert!(
            title < notify,
            "write-through preserves FIFO order: {out:?}"
        );
        // The stage drained: a second tick writes nothing.
        raw.borrow_mut().clear();
        app.write_terminal_sequences(&mut terminal).expect("write");
        assert!(raw.borrow().is_empty(), "stage consumed on first drain");
    }

    // ===== Plan Phase 13: layout and resize behavior =====
    // Buffer tests over the app's own draw path at multiple terminal sizes
    // (80x24 locks live above; these add 120x40, 40x12 narrow, and
    // minimum-height clipping), plus native-scrollback/viewport interaction.

    /// A test terminal of an arbitrary size with its bottom viewport sized to
    /// the app's self-reported height at that width (one run-loop tick's
    /// sizing; `set_bottom_viewport_height` clamps to the screen height).
    fn sized_terminal(app: &RataApp, width: u16, height: u16) -> Terminal<TestWriteBackend> {
        let mut terminal =
            Terminal::with_options(TestWriteBackend::new(width, height)).expect("test terminal");
        terminal
            .set_bottom_viewport_height(app.viewport_height(width))
            .expect("viewport height");
        terminal
    }

    /// Draw the viewport at an arbitrary terminal size through the app's own
    /// draw path; returns the terminal for buffer/cursor inspection.
    fn draw_viewport_at(app: &mut RataApp, width: u16, height: u16) -> Terminal<TestWriteBackend> {
        let mut terminal = sized_terminal(app, width, height);
        app.draw(&mut terminal).expect("draw");
        terminal
    }

    /// The 1-based bottom rows of every history-write scroll region
    /// (`ESC[1;{n}r`) in a raw escape stream: `insert_history_lines` confines
    /// history writes to rows `1..=n`, so `n` must never exceed the viewport
    /// top (0-based) or history would overwrite the bottom pane.
    fn history_scroll_region_bottoms(out: &str) -> Vec<u16> {
        let mut bottoms = Vec::new();
        let mut rest = out;
        while let Some(idx) = rest.find("\x1b[1;") {
            rest = &rest[idx + 4..];
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            if digits > 0 && rest[digits..].starts_with('r') {
                bottoms.push(rest[..digits].parse::<u16>().expect("region bottom"));
            }
        }
        bottoms
    }

    #[test]
    fn layout_120x40_idle_wide_composer_spans_full_width() {
        let mut app = test_app(Vec::new());
        assert_eq!(app.viewport_height(120), 4, "idle viewport is 4 rows");
        let terminal = draw_viewport_at(&mut app, 120, 40);
        let rows = buffer_rows(&terminal);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].chars().count(), 120, "rows span the full width");
        // Idle has no leading status row: rows 0 and 2 are composer padding,
        // background-styled across the full width, with no border glyphs.
        assert!(!rows[0].contains('┌') && !rows[0].contains('┐'));
        assert!(rows[1].starts_with('›'));
        assert!(!rows[2].contains('└') && !rows[2].contains('┘'));
        assert!(rows[3].contains("Enter: send"), "footer row: {}", rows[3]);
    }

    #[test]
    fn layout_120x40_long_markdown_streaming_tail_above_running_pane() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "go");
        app.on_key(press(KeyCode::Enter));
        app.apply_turn_event(TurnEvent::TurnStarted);
        // The just-opened EMPTY assistant cell renders NO tail row (no stray `●`
        // before content) — the viewport is just the running pane (5: status +
        // composer 3 + footer). The tail appears once content streams.
        assert_eq!(
            app.viewport_height(120),
            5,
            "no tail row until content streams"
        );
        // A long markdown paragraph wraps at 120 columns into several rows.
        let paragraph = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(10);
        app.apply_turn_event(TurnEvent::TextDelta(paragraph));
        let viewport = app.viewport_height(120);
        assert!(viewport > 5, "wrapped tail grows the viewport: {viewport}");
        let terminal = draw_viewport_at(&mut app, 120, 40);
        let rows = buffer_rows(&terminal);
        assert_eq!(rows.len(), usize::from(viewport));
        let text_row = rows
            .iter()
            .position(|row| row.contains("lorem ipsum"))
            .unwrap_or_else(|| panic!("streamed markdown not visible:\n{}", rows.join("\n")));
        let status_row = rows
            .iter()
            .position(|row| row.contains("esc to interrupt"))
            .expect("running status row");
        assert!(text_row < status_row, "tail above the running status");
        // The pane stays pinned beneath the tail: its last FOUR rows are the
        // composer (top padding, `›` prompt row, bottom padding) then the
        // footer — with no tail text bleeding into any of them.
        assert!(!rows[rows.len() - 4].contains('┌'));
        assert!(rows[rows.len() - 3].starts_with('›'));
        assert!(!rows[rows.len() - 2].contains('└'));
        assert!(rows[rows.len() - 1].contains("Enter: send"), "footer last");
        assert!(!rows[rows.len() - 3].contains("lorem"), "no overlap");
    }

    #[test]
    fn layout_40x12_narrow_completion_popup_and_composer_share_the_screen() {
        let mut app = test_app(Vec::new());
        typ(&mut app, "/");
        // composer 3 + full-registry popup 8: the popup REPLACES the footer
        // hints below the composer (no leftover hint row at 40 columns).
        assert_eq!(
            app.viewport_height(40),
            11,
            "completion viewport fills the composer + popup rows"
        );
        let terminal = draw_viewport_at(&mut app, 40, 12);
        let rows = buffer_rows(&terminal);
        assert_eq!(rows.len(), 11);
        // The composer comes FIRST now (no leading status row while idle).
        // Row 0 is top padding (no border glyph).
        assert!(!rows[0].contains('┌'), "composer top padding: {}", rows[0]);
        assert!(rows[1].starts_with("› /"), "prompt row: {}", rows[1]);
        assert!(
            !rows[2].contains('└'),
            "composer bottom padding: {}",
            rows[2]
        );
        // The popup box (6-item window) sits directly beneath the composer.
        assert!(rows[3].contains("Complete"), "popup title: {}", rows[3]);
        // Alphabetical popup order (claude-code): /add-dir sorts first.
        assert!(rows[4].contains("› /add-dir"), "first item: {}", rows[4]);
        assert!(rows[10].starts_with('└'), "popup bottom: {}", rows[10]);
    }

    #[test]
    fn layout_40x12_narrow_permission_modal_clips_gracefully() {
        let mut app = test_app(Vec::new());
        let (exchange, _resp_rx) = tool_exchange();
        app.open_permission(exchange);
        assert_eq!(app.viewport_height(40), 9, "permission viewport height");
        let terminal = draw_viewport_at(&mut app, 40, 12);
        let all = buffer_rows(&terminal).join("\n");
        assert!(all.contains("Permission required"), "{all}");
        assert!(all.contains("Yes, allow once"), "{all}");
        assert!(all.contains("No, deny"), "{all}");
    }

    /// One full tick (flush + draw) on a `height`-row terminal: must clip
    /// gracefully — never panic, never escape the screen, never park a
    /// visible cursor outside the viewport.
    fn assert_clips_gracefully(name: &str, mut app: RataApp<'static>, height: u16) {
        let mut terminal = sized_terminal(&app, 80, height);
        app.flush_scrollback(&mut terminal)
            .unwrap_or_else(|e| panic!("{name}@{height}: flush: {e}"));
        app.draw(&mut terminal)
            .unwrap_or_else(|e| panic!("{name}@{height}: draw: {e}"));
        let area = terminal.viewport_area;
        assert!(
            area.bottom() <= height,
            "{name}@{height}: viewport {area:?} escapes the screen"
        );
        assert_eq!(
            terminal.last_frame_buffer().area,
            area,
            "{name}@{height}: frame covers exactly the viewport"
        );
        // Any claimed cursor stays inside the viewport rect.
        if !terminal.cursor_hidden() {
            let pos = terminal.get_cursor_position().unwrap();
            assert!(
                area.contains(pos),
                "{name}@{height}: cursor {pos:?} outside viewport {area:?}"
            );
        }
    }

    #[test]
    fn layout_minimum_height_terminals_clip_gracefully_without_panic() {
        for height in 1..=3u16 {
            assert_clips_gracefully("idle", test_app(Vec::new()), height);

            let mut app = test_app(Vec::new());
            typ(&mut app, "go");
            app.on_key(press(KeyCode::Enter));
            app.apply_turn_event(TurnEvent::TurnStarted);
            app.apply_turn_event(TurnEvent::TextDelta("tiny stream".to_string()));
            assert_clips_gracefully("streaming", app, height);

            let mut app = test_app(Vec::new());
            typ(&mut app, "/");
            assert_clips_gracefully("completion", app, height);

            let mut app = test_app(Vec::new());
            let (exchange, _resp_rx) = tool_exchange();
            app.open_permission(exchange);
            assert_clips_gracefully("permission", app, height);

            let mut app = test_app(Vec::new());
            submit_command(&mut app, "/help");
            assert_clips_gracefully("help", app, height);
        }
    }

    #[test]
    fn layout_completion_popup_items_visible_between_status_and_composer() {
        // End-to-end lock for the Phase 13 popup-zone fix through the app's
        // own draw path (the pre-fix render squeezed the popup into a single
        // border row: items were never visible). Plan Task 5 reorder: the
        // composer now comes FIRST, with the popup directly beneath it.
        let mut app = test_app(Vec::new());
        typ(&mut app, "/");
        assert_eq!(app.viewport_height(80), 11);
        let terminal = draw_viewport(&mut app);
        let rows = buffer_rows(&terminal);
        assert!(rows[1].starts_with("› /"), "composer above: {}", rows[1]);
        // Alphabetical popup order (claude-code): /add-dir sorts first.
        assert!(rows[4].contains("› /add-dir"), "items visible: {}", rows[4]);
        // No row mixes popup chrome with the composer row.
        assert!(
            !rows[1].contains("Complete"),
            "popup and composer overlap:\n{}",
            rows.join("\n")
        );
    }

    #[test]
    fn layout_cursor_stays_inside_composer_rect_with_wide_chars_at_narrow_width() {
        let mut app = test_app(Vec::new());
        // 25 CJK chars = 50 display columns, wider than the 37-column inner
        // rect of a 40-column composer: the line soft-wraps (18 chars = 36
        // cols, then 7 chars = 14 cols) and the cursor follows onto the
        // second visual row instead of escaping through the right margin.
        typ(&mut app, &"你".repeat(25));
        assert_eq!(app.viewport_height(40), 5, "2 wrapped rows + padding");
        let mut terminal = draw_viewport_at(&mut app, 40, 12);
        let pos = terminal.get_cursor_position().unwrap();
        // y = 2: idle has no leading status row (top padding is row 0, the
        // prompt row is row 1, the wrapped continuation row is row 2).
        assert_eq!(
            (pos.x, pos.y),
            (16, 2),
            "cursor after 7 wide chars on the wrapped row"
        );
        // The 1-column right margin is untouched by text — wrapped rows stay
        // inside the inset textarea, not bleeding into the margin.
        let buf = terminal.last_frame_buffer();
        assert_eq!(
            buf.cell(Position::new(39, 1)).map(Cell::symbol),
            Some(" "),
            "right margin intact on the full first row"
        );
    }

    #[test]
    fn viewport_height_shrinks_when_composer_shrinks_and_overlays_close() {
        let mut app = test_app(Vec::new());
        // Composer growth (locked above) … and the reverse: deleting lines
        // shrinks the viewport back down step by step.
        typ(&mut app, "one");
        for _ in 0..9 {
            app.on_key(alt(KeyCode::Enter));
        }
        assert_eq!(app.viewport_height(80), 9, "grown to the composer cap");
        for _ in 0..5 {
            app.on_key(press(KeyCode::Backspace));
        }
        assert_eq!(app.viewport_height(80), 8, "5 lines left: 1 + 5 + 2");
        for _ in 0..4 {
            app.on_key(press(KeyCode::Backspace));
        }
        assert_eq!(app.viewport_height(80), 4, "back to the idle height");
        // Overlay open/close moves it the same way: completion popup…
        app.on_key(ctrl(KeyCode::Char('u'))); // clear the leftover "one"
        typ(&mut app, "/");
        assert_eq!(app.viewport_height(80), 11);
        app.on_key(press(KeyCode::Esc));
        assert_eq!(app.viewport_height(80), 4, "popup dismissed: idle again");
    }

    #[test]
    fn long_finalized_transcript_flush_pins_viewport_to_bottom_of_screen() {
        // 30 finalized one-line messages: more than the 20 rows available
        // above the initial top-anchored viewport, so the flush must push
        // the viewport all the way to the bottom edge and keep every
        // insertion above it.
        let messages: Vec<RenderedMessage> = (0..30)
            .map(|i| RenderedMessage::SystemText {
                body: format!("scrollback line {i:02}"),
                timestamp: 0,
                is_error: false,
            })
            .collect();
        let mut app = test_app(messages);
        let mut terminal = sized_terminal(&app, 80, 24);
        assert_eq!(
            terminal.viewport_area,
            ratatui::layout::Rect::new(0, 0, 80, 4)
        );
        app.flush_scrollback(&mut terminal).unwrap();
        assert_eq!(app.chat_widget.transcript().committed_to_terminal(), 30);
        // Bottom-pinned: 24-row screen minus the 4-row pane.
        assert_eq!(
            terminal.viewport_area,
            ratatui::layout::Rect::new(0, 20, 80, 4),
            "viewport pinned to the bottom edge"
        );
        // All 30 lines went out, oldest first.
        let out = String::from_utf8_lossy(&terminal.backend().raw_handle().borrow()).into_owned();
        let first = out.find("scrollback line 00").expect("first line written");
        let last = out.find("scrollback line 29").expect("last line written");
        assert!(first < last, "commit order preserved");
        // The pane still draws cleanly into the moved viewport.
        app.draw(&mut terminal).unwrap();
        let rows = buffer_rows(&terminal);
        assert!(rows[1].starts_with('›'), "prompt row: {}", rows[1]);
        assert!(rows[3].contains("Enter: send"), "footer row: {}", rows[3]);
    }

    #[test]
    fn native_scrollback_insertions_stay_above_the_pane_across_viewport_height_changes() {
        // Criterion 22: grow the viewport (completion), shrink it back
        // (dismiss), then flush new history — every insertion's scroll region
        // must stay strictly above the (possibly re-anchored) viewport.
        let messages: Vec<RenderedMessage> = (0..30)
            .map(|i| RenderedMessage::SystemText {
                body: format!("warmup line {i:02}"),
                timestamp: 0,
                is_error: false,
            })
            .collect();
        let mut app = test_app(messages);
        let mut terminal = sized_terminal(&app, 80, 24);
        app.flush_scrollback(&mut terminal).unwrap();
        app.draw(&mut terminal).unwrap();
        assert_eq!(terminal.viewport_area.top(), 20, "warmup pinned to bottom");

        // Grow: the completion popup expands the viewport upward (composer 3
        // + full-registry popup 8 = 11 rows; 24 - 11 = 13 top).
        typ(&mut app, "/");
        terminal
            .set_bottom_viewport_height(app.viewport_height(80))
            .unwrap();
        app.draw(&mut terminal).unwrap();
        assert_eq!(
            terminal.viewport_area,
            ratatui::layout::Rect::new(0, 13, 80, 11)
        );

        // Shrink: dismissing the popup keeps the viewport top anchored (codex
        // parity) — the pane is no longer at the bottom edge.
        app.on_key(press(KeyCode::Esc));
        terminal
            .set_bottom_viewport_height(app.viewport_height(80))
            .unwrap();
        app.draw(&mut terminal).unwrap();
        assert_eq!(
            terminal.viewport_area,
            ratatui::layout::Rect::new(0, 13, 80, 4)
        );

        // New finalized content after the height changes: the insertion may
        // scroll the viewport back down, but must never write into it.
        let raw = terminal.backend().raw_handle();
        raw.borrow_mut().clear();
        app.apply_turn_event(TurnEvent::TurnStarted);
        app.apply_turn_event(TurnEvent::TextDelta("post-shrink reply".to_string()));
        app.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        app.flush_scrollback(&mut terminal).unwrap();
        let out = String::from_utf8_lossy(&raw.borrow()).into_owned();
        assert!(out.contains("post-shrink reply"), "reply flushed");
        let area = terminal.viewport_area;
        assert!(area.top() >= 13, "insertions only push the viewport down");
        assert!(area.bottom() <= 24, "viewport stays on screen");
        let bottoms = history_scroll_region_bottoms(&out);
        assert!(!bottoms.is_empty(), "history writes use a scroll region");
        assert!(
            bottoms.iter().all(|&n| n <= area.top()),
            "history region rows {bottoms:?} must stay above viewport top {}",
            area.top()
        );
        // And the pane still draws cleanly afterwards.
        app.draw(&mut terminal).unwrap();
        let rows = buffer_rows(&terminal);
        assert!(rows[1].starts_with('›'), "prompt row: {}", rows[1]);
        assert!(rows[3].contains("Enter: send"), "footer row: {}", rows[3]);
    }
}
