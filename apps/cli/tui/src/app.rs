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

use crate::bottom_pane::permissions_editor_view::PermissionsSnapshot;
use crate::bottom_pane::{
    ConnectAction, FusionSetupAction, PermissionAction, PluginAction, TaskAction,
};
use crate::chat_widget::{ChatOutcome, ChatWidget};
use crate::session::SessionInfo;
use crate::terminal::TerminalSession;
use crate::RataTerminal;
#[cfg(test)]
use crossterm::event::KeyEvent;
use crossterm::event::{self, Event, KeyEventKind};
use permission::computer_access::ComputerAccessExchange;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::io::Write;
use std::time::Duration;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};
use tokio_util::sync::CancellationToken;
use tool_api::ask_user_question::AskUserQuestionExchange;
use tui_core::message::RenderedMessage;
use tui_core::orchestrator_bridge::TurnEvent;
use tui_core::permission_bridge::PermissionExchange;
mod events;
mod input;
mod render;

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
    /// Exit after a successful durable background handoff.
    Backgrounded(String),
    /// The `/resume` picker resolved to this session uuid: the embedder must
    /// re-mount that session in-process (writer retargeted via the startup
    /// resume seam).
    SwitchSession(uuid::Uuid),
    /// The agents view selected a session; the embedder must resolve its live
    /// owner and attach/restart safely before deciding whether remounting is
    /// permitted.
    OpenAgentSession(crate::bottom_pane::view::AgentSessionTarget),
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
    /// it on Ctrl-C/Esc). The second `String` is the visible row's opaque
    /// correlation token.
    pub on_submit: Box<dyn FnMut(String, String, Vec<std::path::PathBuf>, CancellationToken) + 'cb>,
    /// Executed for a prompt entered while a turn is already active. The host
    /// queues it at the canonical `Next` priority; the second `String` is the
    /// visible row's opaque correlation token.
    pub on_queue_prompt:
        Box<dyn FnMut(String, String, Vec<std::path::PathBuf>, CancellationToken) + 'cb>,
    /// Executed on [`ChatOutcome::SwitchModel`] with the picked
    /// `(request_model, profile)` pair.
    pub on_switch_model: Box<dyn FnMut(String, Option<String>) + 'cb>,
    /// Executed on [`ChatOutcome::FusionSetupAction`]: the caller merges the
    /// chosen Fusion model roles into `~/.lingxi/settings.json` asynchronously
    /// and reports the result back via a [`TurnEvent::SystemNotice`].
    pub on_fusion_setup_action: Box<dyn FnMut(FusionSetupAction) + 'cb>,
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
    /// Recompute the visible command catalog after `/reload-skills`.
    pub on_refresh_command_catalog: Box<dyn FnMut() + 'cb>,
    /// Executed on [`ChatOutcome::RunBash`]: the caller runs the `!`-prefixed
    /// command through the sandboxed bash runner (no LLM turn) and folds its
    /// output back via [`TurnEvent::BashOutput`].
    pub on_bash: Box<dyn FnMut(String) + 'cb>,
    /// Executed on [`ChatOutcome::Compact`]: the caller drives
    /// `OrchestratorHandle::force_compact` asynchronously on the LIVE engine
    /// runtime (never on the render thread) and reports the summary back via a
    /// [`TurnEvent::SystemNotice`]. The `String` is the argument tail.
    pub on_compact: Box<dyn FnMut(String, CancellationToken) + 'cb>,
    /// Executed on [`ChatOutcome::Summarize`]: the caller drives
    /// `OrchestratorHandle::summarize_at` off-loop and reports the result via
    /// [`TurnEvent::SystemNotice`], exactly like `on_compact`.
    ///
    /// ⛔ This must NOT unwind the app — see [`ChatOutcome::Summarize`].
    pub on_summarize: Box<
        dyn FnMut(
                uuid::Uuid,
                lingxi_core::host::SummarizeDirection,
                Option<String>,
                CancellationToken,
            ) + 'cb,
    >,
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
    /// Clear the backend first; acknowledge the reset before erasing scrollback.
    pub on_clear_session: Box<dyn FnMut(Option<String>) + 'cb>,
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
    pub on_dispatch_slash: Box<dyn FnMut(String, crate::chat_widget::PendingSlashDispatch) + 'cb>,
    /// Executed on [`ChatOutcome::RewakePeer`]: the caller drives
    /// `OrchestratorHandle::run_async_hook_rewake` off the render thread so
    /// a just-delivered held peer message is injected without a synthetic
    /// user prompt.
    pub on_rewake_peer: Box<dyn FnMut() + 'cb>,
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
    /// Off-thread full-screen copy-on-select result.  Only failures are
    /// surfaced and each session shows at most one failure notice.
    copy_tx: std::sync::mpsc::Sender<Result<crate::copy::ClipboardTransport, String>>,
    copy_rx: std::sync::mpsc::Receiver<Result<crate::copy::ClipboardTransport, String>>,
    copy_error_shown: bool,
    /// Off-thread `ui.render` results for the live terminal AbovePrompt site.
    mod_ui_render_tx: std::sync::mpsc::Sender<(
        u64,
        u64,
        serde_json::Value,
        Result<serde_json::Value, String>,
    )>,
    mod_ui_render_rx: std::sync::mpsc::Receiver<(
        u64,
        u64,
        serde_json::Value,
        Result<serde_json::Value, String>,
    )>,
    /// Lightweight session-local invalidate generation polled off-thread
    /// while the input loop waits for its next redraw.
    mod_ui_generation_tx: std::sync::mpsc::Sender<u64>,
    mod_ui_generation_rx: std::sync::mpsc::Receiver<u64>,
    mod_ui_generation_poll_in_flight: bool,
    mod_ui_generation_tracker: crate::mod_ui_render::RenderGenerationTracker,
    mod_ui_orchestrator: Option<std::sync::Arc<dyn lingxi_core::host::OrchestratorHandle>>,
    mod_ui_render_input: Option<serde_json::Value>,
    mod_ui_render_pending: Option<serde_json::Value>,
    mod_ui_render_in_flight: bool,
    /// Button action identities extracted from the latest accepted render.
    mod_ui_button_actions: Vec<crate::mod_ui_render::AbovePromptButtonAction>,
    /// Reject results from requests started before a cache reset, including
    /// `/clear` where viewport props and the ModHost generation stay equal.
    mod_ui_render_revision: u64,
    /// Alternate-screen renderer state (`/tui`).
    fullscreen: bool,
    copy_on_select: bool,
    selection: crate::selection::SelectionState,
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
        let (copy_tx, copy_rx) = std::sync::mpsc::channel();
        let (mod_ui_render_tx, mod_ui_render_rx) = std::sync::mpsc::channel();
        let (mod_ui_generation_tx, mod_ui_generation_rx) = std::sync::mpsc::channel();
        Self {
            chat_widget: ChatWidget::new(messages, session),
            events_rx,
            permission_rx,
            ask_user_question_rx,
            computer_access_rx,
            paste_tx,
            paste_rx,
            copy_tx,
            copy_rx,
            copy_error_shown: false,
            mod_ui_render_tx,
            mod_ui_render_rx,
            mod_ui_generation_tx,
            mod_ui_generation_rx,
            mod_ui_generation_poll_in_flight: false,
            mod_ui_generation_tracker: crate::mod_ui_render::RenderGenerationTracker::default(),
            mod_ui_orchestrator: None,
            mod_ui_render_input: None,
            mod_ui_render_pending: None,
            mod_ui_render_in_flight: false,
            mod_ui_button_actions: Vec::new(),
            mod_ui_render_revision: 0,
            fullscreen: false,
            copy_on_select: true,
            selection: crate::selection::SelectionState::default(),
            callbacks,
            redraw_interval: Duration::from_millis(50),
        }
    }

    fn set_orchestrator(
        &mut self,
        handle: std::sync::Arc<dyn lingxi_core::host::OrchestratorHandle>,
    ) {
        self.chat_widget.set_orchestrator(handle.clone());
        self.mod_ui_orchestrator = Some(handle);
    }

    pub(super) fn request_mod_ui_render(&mut self, columns: u16, rows: u16) {
        let max_rows = self
            .chat_widget
            .mod_ui_row_budget(columns, rows, self.fullscreen);
        let input = crate::mod_ui_render::above_prompt_event(
            columns,
            rows,
            self.fullscreen,
            self.chat_widget.mod_ui_is_working(),
            max_rows,
        );
        if self.mod_ui_render_input.as_ref() == Some(&input) {
            return;
        }
        self.mod_ui_render_input = Some(input.clone());
        if self.mod_ui_render_in_flight {
            self.mod_ui_render_pending = Some(input);
            return;
        }
        self.start_mod_ui_render(input);
    }

    fn start_mod_ui_render(&mut self, input: serde_json::Value) {
        let Some(orchestrator) = self.mod_ui_orchestrator.clone() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.mod_ui_render_revision = self.mod_ui_render_revision.saturating_add(1);
        self.mod_ui_button_actions.clear();
        self.chat_widget.clear_mod_ui_render_tree();
        self.mod_ui_render_in_flight = true;
        let tx = self.mod_ui_render_tx.clone();
        let request = input.clone();
        let generation = self.mod_ui_generation_tracker.generation().unwrap_or(0);
        let revision = self.mod_ui_render_revision;
        runtime.spawn(async move {
            // Revoke the currently displayed actions before dispatching this
            // render. Keeping both calls in one task preserves their order, so
            // an old press cannot win a race while a redraw is pending.
            let result = crate::mod_ui_render::clear_then_render(
                || orchestrator.mod_ui_clear_press_actions(revision),
                || orchestrator.mod_ui_render(input, revision),
            )
            .await
            .map_err(|error| error.to_string());
            let _ = tx.send((generation, revision, request, result));
        });
    }

    fn poll_mod_ui_render_generation(&mut self) {
        if self.mod_ui_generation_poll_in_flight {
            return;
        }
        let Some(orchestrator) = self.mod_ui_orchestrator.clone() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.mod_ui_generation_poll_in_flight = true;
        let tx = self.mod_ui_generation_tx.clone();
        runtime.spawn(async move {
            let generation = orchestrator.mod_ui_render_generation().await;
            let _ = tx.send(generation);
        });
    }

    fn drain_mod_ui_render_generation(&mut self) -> bool {
        let mut changed = false;
        while let Ok(generation) = self.mod_ui_generation_rx.try_recv() {
            self.mod_ui_generation_poll_in_flight = false;
            if self.mod_ui_generation_tracker.observe(generation) {
                self.reset_mod_ui_render_cache();
                changed = true;
            }
        }
        changed
    }

    fn reset_mod_ui_render_cache(&mut self) {
        self.mod_ui_render_revision = self.mod_ui_render_revision.saturating_add(1);
        self.mod_ui_render_input = None;
        self.mod_ui_render_pending = None;
        self.mod_ui_button_actions.clear();
        self.chat_widget.clear_mod_ui_render_tree();
        let Some(orchestrator) = self.mod_ui_orchestrator.clone() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let revision = self.mod_ui_render_revision;
        runtime.spawn(async move {
            let _ = orchestrator.mod_ui_clear_press_actions(revision).await;
        });
    }

    fn drain_mod_ui_render_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok((generation, revision, request, result)) = self.mod_ui_render_rx.try_recv() {
            self.mod_ui_render_in_flight = false;
            if self.mod_ui_generation_tracker.generation().unwrap_or(0) == generation
                && self.mod_ui_render_revision == revision
                && self.mod_ui_render_input.as_ref() == Some(&request)
            {
                if let Ok(tree) = result {
                    let max_rows = request["props"]["maxRows"].as_u64().unwrap_or(0) as u16;
                    self.chat_widget.set_mod_ui_render_tree(&tree, max_rows);
                    self.mod_ui_button_actions =
                        crate::mod_ui_render::tree_buttons(&tree, revision).unwrap_or_default();
                    changed = true;
                }
            }
            if let Some(pending) = self.mod_ui_render_pending.take() {
                if (pending != request
                    || revision != self.mod_ui_render_revision
                    || generation != self.mod_ui_generation_tracker.generation().unwrap_or(0))
                    && self.mod_ui_render_input.as_ref() == Some(&pending)
                {
                    self.start_mod_ui_render(pending);
                }
            }
        }
        changed
    }

    fn press_mod_ui_button(&self, action: crate::mod_ui_render::AbovePromptButtonAction) {
        let Some(orchestrator) = self.mod_ui_orchestrator.clone() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            let _ = orchestrator
                .mod_ui_press(action.press_event(), action.site_revision)
                .await;
        });
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
        self.run_inner(terminal, None)
    }

    /// Production loop with access to the terminal RAII guard, allowing
    /// `/tui` to enter/leave alternate-screen + mouse capture without leaking
    /// modes on errors or panic.
    pub fn run_with_session(
        &mut self,
        terminal: &mut RataTerminal,
        session: &mut TerminalSession,
    ) -> io::Result<AppExit> {
        self.run_inner(terminal, Some(session))
    }

    fn run_inner(
        &mut self,
        terminal: &mut RataTerminal,
        mut session: Option<&mut TerminalSession>,
    ) -> io::Result<AppExit> {
        let mut need_draw = true;
        loop {
            let mut dirty = false;
            dirty |= self.drain_mod_ui_render_results();
            dirty |= self.drain_mod_ui_render_generation();
            self.poll_mod_ui_render_generation();
            while let Ok(event) = self.events_rx.try_recv() {
                self.apply_turn_event(event);
                dirty = true;
            }
            // The widget serializes permission prompts (one owns the
            // keyboard; later arrivals queue), so the drain is unconditional.
            while let Ok(exchange) = self.permission_rx.try_recv() {
                self.open_permission(exchange);
                dirty = true;
            }
            while let Ok(exchange) = self.ask_user_question_rx.try_recv() {
                self.open_ask_user_question(exchange);
                dirty = true;
            }
            while let Ok(exchange) = self.computer_access_rx.try_recv() {
                self.open_computer_access(exchange);
                dirty = true;
            }
            // Off-thread clipboard-image paste results (Ctrl+V): attach the
            // temp PNG (or surface the error) as soon as the worker delivers.
            while let Ok(result) = self.paste_rx.try_recv() {
                self.chat_widget.clipboard_image_result(result);
                dirty = true;
            }
            while let Ok(result) = self.copy_rx.try_recv() {
                if let Err(error) = result {
                    if !self.copy_error_shown {
                        self.copy_error_shown = true;
                        self.apply_turn_event(TurnEvent::SystemNotice {
                            body: format!("Clipboard copy failed: {error}"),
                            is_error: true,
                        });
                    }
                }
                dirty = true;
            }
            // Flush a due non-bracketed paste burst (held first char renders
            // as typing; a completed burst lands as one paste). The pump
            // never submits, so the outcome needs no callback dispatch.
            let paste_was_pending = self.chat_widget.paste_burst_pending();
            let paste_outcome = self.chat_widget.pump_paste_burst();
            if !matches!(paste_outcome, ChatOutcome::Continue)
                || (paste_was_pending && !self.chat_widget.paste_burst_pending())
            {
                dirty = true;
            }
            // Countdown-backed modal views must advance while the terminal is
            // idle. This runs before drawing so an expired questionnaire is
            // popped and its queued successor can render on the same tick.
            if self.chat_widget.pump_view_timeout() {
                dirty = true;
            }
            // A ← handoff may be waiting for the active tool boundary or its
            // 10-second defer cap. Advance it on the same redraw clock.
            self.chat_widget.pump_backgrounding();
            // Hook-returned terminal escapes (`TurnEvent::TerminalSequence`,
            // already validated + BEL-normalized) write through to the tty
            // BEFORE the draw so the diff pass never interleaves with them.
            self.write_terminal_sequences(terminal)?;
            if need_draw || dirty || self.chat_widget.needs_animated_redraw() {
                self.render_tick(terminal)?;
                need_draw = false;
            }
            // While burst state is pending (a held first char, an unflushed
            // buffer) the 8ms flush deadline must not wait out the full
            // redraw interval — a keystroke would echo up to ~50ms late.
            let poll_timeout = if self.chat_widget.paste_burst_pending() {
                Duration::from_millis(10)
            } else {
                self.redraw_interval
            };
            if event::poll(poll_timeout)? {
                need_draw = true;
                let outcome = match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key),
                    Event::Paste(text) => self.on_paste(&text),
                    Event::Mouse(mouse) if self.fullscreen => {
                        self.on_mouse(mouse, terminal);
                        ChatOutcome::Continue
                    }
                    _ => ChatOutcome::Continue,
                };
                match outcome {
                    ChatOutcome::Quit => return Ok(AppExit::Quit),
                    ChatOutcome::BackgroundedExit(receipt) => {
                        return Ok(AppExit::Backgrounded(receipt))
                    }
                    ChatOutcome::Detach => {
                        if let Ok(token) =
                            std::env::var(tui_core::background_detach::DETACH_TOKEN_ENV)
                        {
                            let sequence =
                                tui_core::background_detach::detach_request_sequence(&token);
                            terminal.backend_mut().write_all(&sequence)?;
                            Write::flush(terminal.backend_mut())?;
                        }
                    }
                    ChatOutcome::ForceRedraw => {
                        self.chat_widget.reset_terminal_commit();
                        terminal.reset_for_replay()?;
                    }
                    ChatOutcome::ToggleFullscreen => {
                        let Some(session) = session.as_deref_mut() else {
                            continue;
                        };
                        let enabled = !self.fullscreen;
                        session.set_fullscreen(enabled)?;
                        self.fullscreen = enabled;
                        self.chat_widget.set_collapse_fullscreen(enabled);
                        self.selection.clear();
                        self.chat_widget.set_mod_ui_selection(None);
                        // Entering clears the alternate screen; leaving must
                        // rebuild native scrollback from structured cells.
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
                    ChatOutcome::OpenAgentSession(target) => {
                        self.chat_widget.cancel_active_turn();
                        return Ok(AppExit::OpenAgentSession(target));
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
                    // `/rewind` → Summarize: runs IN PLACE on the live runtime.
                    // Deliberately no `AppExit`: unwinding here would discard
                    // the very conversation the summarizer is about to read.
                    ChatOutcome::Summarize {
                        message,
                        direction,
                        context,
                    } => {
                        (self.callbacks.on_summarize)(
                            message,
                            direction,
                            context,
                            CancellationToken::new(),
                        );
                    }
                    ChatOutcome::Submit(prompt, row_token, images, token) => {
                        // The queued images ride inside the Submit payload
                        // (they become `ContentBlock::Image` on the user
                        // message).
                        (self.callbacks.on_submit)(prompt, row_token, images, token);
                    }
                    ChatOutcome::QueuePrompt(prompt, row_token, images, owner) => {
                        (self.callbacks.on_queue_prompt)(prompt, row_token, images, owner);
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
                    // A finished `/fusion setup`: persist the model roles
                    // off-loop.
                    ChatOutcome::FusionSetupAction(action) => {
                        (self.callbacks.on_fusion_setup_action)(action);
                    }
                    // A `/connect` effect: store a key or kick off a
                    // Copilot/OAuth sign-in off-loop.
                    ChatOutcome::ConnectAction(action) => {
                        (self.callbacks.on_connect_action)(action);
                    }
                    // A `/permissions` effect: persist the added/removed rule
                    // off-loop (and push a live allow rule); the result returns
                    // via `TurnEvent::SystemNotice`.
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
                    ChatOutcome::RefreshCommandCatalog => {
                        (self.callbacks.on_refresh_command_catalog)();
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
                    ChatOutcome::ClearSession(title) => {
                        self.selection.clear();
                        self.chat_widget.set_mod_ui_selection(None);
                        (self.callbacks.on_clear_session)(title);
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
                    ChatOutcome::RewakePeer => {
                        (self.callbacks.on_rewake_peer)();
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
    background_handoff: Option<lingxi_core::host::BackgroundingSnapshot>,
    session: SessionInfo,
    events_rx: UnboundedReceiver<TurnEvent>,
    permission_rx: Receiver<PermissionExchange>,
    ask_user_question_rx: Receiver<AskUserQuestionExchange>,
    computer_access_rx: Receiver<ComputerAccessExchange>,
    subscription: Option<lingxi_core::host::subscription::SharedSubscription>,
    status_line: Option<crate::status_line::SharedStatusLine>,
    fusion_settings: Option<
        std::sync::Arc<std::sync::Mutex<crate::fusion::setup::FusionSettingsSnapshot>>,
    >,
    permission_snapshot: Option<std::sync::Arc<std::sync::Mutex<PermissionsSnapshot>>>,
    plugin_snapshot: Option<
        std::sync::Arc<std::sync::Mutex<crate::bottom_pane::plugins_view::PluginsSnapshot>>,
    >,
    resume_rows: Vec<crate::resume::ResumeRow>,
    connect_auth_methods: std::collections::BTreeMap<String, String>,
    connect_availability: std::collections::BTreeMap<String, bool>,
    shell_expansion: Option<std::sync::Arc<dyn command_api::ShellExpansionProvider>>,
    orchestrator: Option<std::sync::Arc<dyn lingxi_core::host::OrchestratorHandle>>,
    // Live session-cwd reader used by native scrollback attachment links.
    // `None` keeps hermetic/unit callers on the startup `SessionInfo` cwd.
    hyperlink_cwd_provider: Option<std::sync::Arc<dyn Fn() -> std::path::PathBuf + Send + Sync>>,
    sandbox_toggle: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    command_registry: Option<std::sync::Arc<tokio::sync::RwLock<command_api::CommandRegistry>>>,
    task_registry: Option<std::sync::Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>>,
    // Persistent prompt-history store (`~/.lingxi/history.jsonl`, cc 2.1.218):
    // seeds the composer recall + persists submissions. `None` = session-local
    // recall only (tests, `CLAUDE_CODE_SKIP_PROMPT_HISTORY`).
    prompt_history: Option<std::sync::Arc<session::prompt_history::PromptHistoryStore>>,
    // The resolved boot permission mode + whether bypass is an available
    // Shift+Tab cycle target — seeds the below-composer mode indicator.
    initial_permission_mode: permission::PermissionMode,
    bypass_available: bool,
    auto_available: bool,
    emoji_completion_enabled: bool,
    startup_view_mode: Option<String>,
    agents_snapshot_provider: Option<
        std::sync::Arc<dyn Fn() -> crate::bottom_pane::view::AgentsSnapshot + Send + Sync>,
    >,
    loop_interrupt: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    on_submit: impl FnMut(String, String, Vec<std::path::PathBuf>, CancellationToken),
    on_queue_prompt: impl FnMut(String, String, Vec<std::path::PathBuf>, CancellationToken),
    on_switch_model: impl FnMut(String, Option<String>),
    on_fusion_setup_action: impl FnMut(FusionSetupAction),
    on_connect_action: impl FnMut(ConnectAction),
    on_permission_action: impl FnMut(PermissionAction),
    on_plugin_action: impl FnMut(PluginAction),
    on_reload_plugins: impl FnMut(),
    on_refresh_command_catalog: impl FnMut(),
    on_bash: impl FnMut(String),
    on_compact: impl FnMut(String, CancellationToken),
    on_summarize: impl FnMut(
        uuid::Uuid,
        lingxi_core::host::SummarizeDirection,
        Option<String>,
        CancellationToken,
    ),
    on_rename: impl FnMut(String),
    on_fast_mode: impl FnMut(Option<bool>),
    on_plan_mode: impl FnMut(String),
    on_set_permission_mode: impl FnMut(String),
    on_clear_session: impl FnMut(Option<String>),
    on_sandbox_action: impl FnMut(crate::chat_widget::SandboxAction),
    on_task_action: impl FnMut(TaskAction),
    on_dispatch_slash: impl FnMut(String, crate::chat_widget::PendingSlashDispatch),
    on_rewake_peer: impl FnMut(),
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
    let fullscreen = std::env::var("LINGXI_TUI_FULLSCREEN")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "on"));
    // Guard first, terminal second: locals drop in reverse order, so the
    // terminal resets the cursor while raw mode is still active, then the
    // guard restores cooked mode + bracketed paste.
    let mut session_guard = if fullscreen {
        TerminalSession::new_fullscreen()?
    } else {
        TerminalSession::new()?
    };
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = if detached_pty || fullscreen {
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
            on_queue_prompt: Box::new(on_queue_prompt),
            on_switch_model: Box::new(on_switch_model),
            on_fusion_setup_action: Box::new(on_fusion_setup_action),
            on_connect_action: Box::new(on_connect_action),
            on_permission_action: Box::new(on_permission_action),
            on_plugin_action: Box::new(on_plugin_action),
            on_reload_plugins: Box::new(on_reload_plugins),
            on_refresh_command_catalog: Box::new(on_refresh_command_catalog),
            on_bash: Box::new(on_bash),
            on_compact: Box::new(on_compact),
            on_summarize: Box::new(on_summarize),
            on_rename: Box::new(on_rename),
            on_fast_mode: Box::new(on_fast_mode),
            on_plan_mode: Box::new(on_plan_mode),
            on_set_permission_mode: Box::new(on_set_permission_mode),
            on_clear_session: Box::new(on_clear_session),
            on_sandbox_action: Box::new(on_sandbox_action),
            on_task_action: Box::new(on_task_action),
            on_dispatch_slash: Box::new(on_dispatch_slash),
            on_rewake_peer: Box::new(on_rewake_peer),
        },
    );
    if let Some(callback) = loop_interrupt {
        app.chat_widget.set_loop_interrupt(callback);
    }
    if let Some(provider) = hyperlink_cwd_provider {
        app.chat_widget.set_hyperlink_cwd_provider(provider);
    }
    app.configure_fullscreen(
        fullscreen,
        tui_core::theme_persist::load_copy_on_select().unwrap_or(true),
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
    app.chat_widget
        .set_startup_view_mode(startup_view_mode.as_deref());
    if let Some(slot) = subscription {
        app.chat_widget.set_subscription(slot);
    }
    if let Some(slot) = status_line {
        app.chat_widget.set_status_line(slot);
    }
    if let Some(slot) = fusion_settings {
        app.chat_widget.set_fusion_settings(slot);
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
        app.set_orchestrator(handle);
    }
    // Seed the below-composer permission-mode indicator from the resolved boot
    // mode (so `--dangerously-skip-permissions` shows `⏵⏵ bypass permissions on`).
    app.chat_widget
        .set_permission_mode(initial_permission_mode, bypass_available, auto_available);
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
    app.chat_widget.set_agents_snapshot_provider(
        agents_snapshot_provider
            .unwrap_or_else(|| std::sync::Arc::new(ChatWidget::live_agents_snapshot)),
    );
    // Agents-view settings (claude 2.1.220): `leftArrowOpensAgents`
    // (`kCt = Rt().leftArrowOpensAgents !== false`, default ON) gates the
    // ←-on-empty gesture; it is ANDed with the agent-view enablement gate,
    // because a disabled agent view fails `kGt`'s `Zan(C2t({fleetEnabled:
    // $H(), …}))` check and installs no handler at all.
    let agent_view_enabled = lingxi_core::host::agent_view::is_enabled();
    app.chat_widget.set_left_arrow_opens_agents(
        tui_core::theme_persist::load_left_arrow_opens_agents().unwrap_or(true)
            && agent_view_enabled,
    );
    app.chat_widget
        .set_attached_background_session(detached_pty);
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
    if let Some(snapshot) = background_handoff.as_ref() {
        app.chat_widget.restore_background_handoff(snapshot);
    }
    if let Some(prompt) = initial_prompt.filter(|value| !value.trim().is_empty()) {
        if let ChatOutcome::Submit(prompt, row_token, images, token) =
            app.chat_widget.submit_prompt(prompt)
        {
            (app.callbacks.on_submit)(prompt, row_token, images, token);
        }
    }
    let result = app.run_with_session(&mut terminal, &mut session_guard);
    app.chat_widget.set_mod_ui_selection(None);
    app.chat_widget.cancel_active_turn();
    result
}

#[cfg(test)]
#[path = "app/tests/tests.rs"]
mod tests;
