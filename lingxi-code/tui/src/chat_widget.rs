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
use tui_core::message::CurrentTodo;
use tui_core::message::RenderedMessage;
use tui_core::orchestrator_bridge::TurnEvent;
use tui_core::permission_bridge::PermissionExchange;
use tui_core::theme::{theme_for, Theme, ThemeName, ThemeSetting};

use crate::bottom_pane::permission_view::PermissionView;
use crate::bottom_pane::screen_view::ScreenView;
use crate::bottom_pane::theme_picker_view::ThemePickerView;
use crate::bottom_pane::permissions_editor_view::PermissionsSnapshot;
use crate::bottom_pane::{
    BottomPane, BottomPaneOutcome, BottomPaneStatus, CommandAction, ConnectAction, PermissionAction,
    WebAction,
};
use crate::history_cell::message::AssistantTextCell;
use crate::history_cell::message::ThinkingCell;
use crate::renderable::Renderable;
use crate::session::SessionInfo;
use crate::spinner;
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
    /// A `/web` view asked the caller to run a secret/settings save or a test
    /// search. The caller runs it asynchronously and reports the result back
    /// through `TurnEvent::SystemNotice`.
    WebAction(WebAction),
    /// A `/connect` view asked the caller to store an API key or kick off a
    /// Copilot/OAuth sign-in. The caller runs it asynchronously and reports
    /// the result back through `TurnEvent::SystemNotice`.
    ConnectAction(ConnectAction),
    /// A `/permissions` view asked the caller to persist an added/removed
    /// allow/ask/deny rule. The caller runs the settings-file write (and, for
    /// an added allow rule, the in-memory `session_allow_rules` push for
    /// this-session effect) asynchronously and reports the result back through
    /// `TurnEvent::SystemNotice`.
    PermissionAction(PermissionAction),
    /// The user submitted a `!`-prefixed bash-mode command. The caller runs it
    /// through the sandboxed [`tui_core::bash_runner::BashRunner`] (no LLM
    /// turn) and folds the captured output back through
    /// `TurnEvent::BashOutput`.
    RunBash(String),
    /// `/compact` asked for a forced compaction pass. The caller drives
    /// [`traits::OrchestratorHandle::force_compact`] — a real multi-second LLM
    /// summarization round-trip — asynchronously on the LIVE engine runtime
    /// (correct reactor; never on the render thread's throwaway `block_on`,
    /// which would freeze input, drive the reqwest/websocket sockets on the
    /// wrong reactor, and race the turn loop's history swap) and reports the
    /// summary back through `TurnEvent::SystemNotice`. The `String` is the
    /// (currently unused) argument tail.
    Compact(String),
    /// `/fast [on|off]` asked the caller to set (`Some(true|false)`) or toggle
    /// (`None`, a bare `/fast`) the session's fast-mode flag. The caller flips
    /// it off-loop via `OrchestratorHandle::set_fast_mode` and reports the
    /// applied state ("⚡ Fast mode ON" / "Fast mode OFF") through
    /// `TurnEvent::SystemNotice`. When on and the active model supports fast
    /// mode (opus-4-7/opus-4-8), subsequent turns send `speed:"fast"`.
    FastMode(Option<bool>),
    /// The `/resume` picker resolved to this session uuid. The caller must
    /// UNWIND the app loop (via `AppExit::SwitchSession`) and re-mount that
    /// session in-process so the JSONL writer is retargeted to `<uuid>.jsonl` —
    /// NOT an in-place `resume_session` swap (which would fork the conversation
    /// across files).
    SwitchSession(uuid::Uuid),
    /// `/rename <name>` resolved to this new title. The caller appends the
    /// `custom-title` JSONL line OFF the render thread (state mutation goes
    /// off-loop, per doctrine) via `OrchestratorHandle::rename_session`, then
    /// reports `Session renamed to: <name>` (or a failure) back through
    /// `TurnEvent::SystemNotice`.
    RenameSession(String),
}

/// Live API retry-backoff status, mirroring Claude Code's `SystemAPIErrorMessage`.
#[derive(Debug, Clone)]
struct ApiRetryState {
    /// User-facing error text (e.g. `"provider internal error"`).
    message: String,
    /// 1-based attempt number about to be retried.
    attempt: u32,
    /// Configured retry cap.
    max_retries: u32,
    /// When the backoff ends — drives the live "Retrying in Ns…" countdown.
    deadline: std::time::Instant,
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
    /// Active API retry-backoff status (Claude Code's `SystemAPIErrorMessage`).
    /// `Some` while an API request is backing off before its next attempt; the
    /// spinner shows `"<message> · Retrying in Ns… (attempt X/Y)"` with a live
    /// countdown. Cleared when the turn produces content, starts, or ends.
    api_retry: Option<ApiRetryState>,
    /// Human label for what the turn is currently doing (e.g. `Running Bash`),
    /// set from `ToolUseStart` and shown by the spinner instead of a bare verb.
    activity: Option<String>,
    /// Running character length of the streamed response this turn (text +
    /// thinking deltas), reset on `TurnStarted`. Drives the spinner's live token
    /// estimate (`round(chars / 4)`, claude-code `Spinner.tsx:210`).
    response_chars: u64,
    /// The per-turn sampled spinner verb (claude-code `useState(() =>
    /// sample(getSpinnerVerbs()))`): drawn once from [`crate::spinner::SPINNER_VERBS`]
    /// on `TurnEvent::TurnStarted` and held for the turn's lifetime — the
    /// spinner's lowest-precedence default when there is no tool activity or
    /// active-todo verb.
    spinner_verb: &'static str,
    /// The session's currently in-progress todo (from the latest `TodoWrite`
    /// tool call), threaded to the spinner so it shows the task's `activeForm`
    /// instead of a generic tool-activity label (claude-code `Spinner.tsx:162`).
    /// `None` outside a turn or once no todo is in progress.
    current_todo: Option<CurrentTodo>,
    /// Permission requests waiting for the currently open prompt to resolve
    /// (prompts are serialized: one owns the keyboard at a time).
    pending_permissions: VecDeque<PermissionExchange>,
    /// Widget-lifetime clock driving the spinner's animation frame (and the
    /// `/stats` session-duration row).
    start: std::time::Instant,
    /// Where `/export` writes transcripts (default `~/.lingxi/exports`;
    /// overridable so tests and embedders stay hermetic).
    export_dir: std::path::PathBuf,
    /// Session-cumulative cost (`$0.0000`, 4-decimal claude-code parity) from
    /// the latest [`TurnEvent::CostUpdated`]; shown in the status row.
    cost: Option<String>,
    /// The last rate-limit notice text pushed to the transcript, so identical
    /// consecutive [`TurnEvent::RateLimit`] snapshots never stack (the old
    /// backend's `state.last_rate_limit_text` dedupe slot; reset by `/clear`
    /// with the scrollback).
    last_rate_limit_text: Option<String>,
    /// One-shot guard for the overage-transition notice
    /// (`useRateLimitWarningNotification.tsx` `hasShownOverageNotification`):
    /// set when the notice fires, reset when a `RateLimit` event shows the
    /// session has LEFT overage. Deliberately survives `/clear` — TS
    /// component state outlives transcript clears.
    has_shown_overage_notification: bool,
    /// Composition-root-shared subscription slot (`None` until the embedder
    /// wires one via [`Self::set_subscription`]; the root's background fetch
    /// fills it). Read at rate-limit compose time for subscription-granular
    /// copy; absent/unfilled degrades to the unknown-subscription default
    /// (TS-conservative), exactly like the old backend's
    /// `AppState::subscription_snapshot`.
    subscription: Option<traits::subscription::SharedSubscription>,
    /// Composition-root-shared custom-statusline slot (`None` unless the
    /// embedder wires one via [`Self::set_status_line`]). The widget writes the
    /// live pump inputs (cost / rate-limit utilization) + marks it dirty on
    /// `TurnEnded`; the async pump (CLI `run_ratatui`) runs the configured
    /// command and writes back `text`, which the bottom pane renders.
    status_line: Option<crate::status_line::SharedStatusLine>,
    /// Validated terminal escape sequences from
    /// [`TurnEvent::TerminalSequence`] (hook-returned, allowlisted +
    /// BEL-normalized by the orchestrator), staged until the app loop drains
    /// them via [`Self::take_terminal_sequences`] and writes the bytes to the
    /// terminal that owns the controlling tty (claude-code `BEo`).
    pending_terminal_sequences: Vec<String>,
    /// Live tool-call inputs keyed by `tool_use_id`, populated on
    /// [`TurnEvent::ToolUseStart`] and consumed on the paired
    /// [`TurnEvent::ToolUseResult`] to recover the Edit/Write diff fields
    /// (`old_string`/`new_string`/`file_path`) for the result cell — the same
    /// correlation the resume path does with its `tool_inputs` side-table.
    tool_inputs: std::collections::HashMap<protocol::ToolUseId, serde_json::Value>,
    /// Composition-root-shared `/web` config snapshot slot (`None` until the
    /// embedder wires one via [`Self::set_web_snapshot`]). [`Self::cmd_web`]
    /// reads a clone to seed the picker; the async `on_web_action` effect
    /// closure (CLI `run_ratatui`) updates it in place after a save/test so
    /// the NEXT `/web` open reflects the latest persisted state.
    web_snapshot: Option<std::sync::Arc<std::sync::Mutex<crate::web::picker::WebConfigSnapshot>>>,
    /// Composition-root-shared `/permissions` rule snapshot slot (`None` until
    /// the embedder wires one via [`Self::set_permission_snapshot`]). Preloaded
    /// at startup from the user/project/local settings files and kept current
    /// by the async `on_permission_action` effect closure after each edit.
    /// [`Self::cmd_permissions`] reads a clone to seed the editor.
    permission_snapshot: Option<std::sync::Arc<std::sync::Mutex<PermissionsSnapshot>>>,
    /// Preloaded `/resume` session rows (newest-first), wired at startup via
    /// [`Self::set_resume_rows`]. Loaded async from disk in the CLI before the
    /// blocking loop starts (the sync TUI loop can't `.await` a disk scan); kept
    /// DISTINCT from the startup `--resume` picker path. [`Self::cmd_resume`]
    /// clones these into the picker. Empty (the default) opens an empty-state
    /// picker.
    resume_rows: Vec<crate::resume::ResumeRow>,
    /// Per-provider login-method tag (from the catalog auth strategy), keyed
    /// by profile_name — the real data `/connect`'s picker groups/labels
    /// from. Empty (default) until [`Self::set_connect_data`] wires it.
    connect_auth_methods: std::collections::BTreeMap<String, String>,
    /// Per-provider availability flag (already has a usable credential),
    /// keyed by profile_name — joined into the `/connect` picker's `✓`
    /// marker. Empty (default) until [`Self::set_connect_data`] wires it.
    connect_availability: std::collections::BTreeMap<String, bool>,
    /// (#3) The shared prompt shell-expansion provider (`None` until the
    /// embedder wires one via [`Self::set_shell_expansion`]). When present,
    /// [`Self::run_core_command`]'s `InjectMessage` arm expands the builtin
    /// prompt bodies (`/commit`, `/commit-push-pr`, `/security-review`) — running
    /// their embedded `!`git …`` commands through the real host runner +
    /// policy-backed gate (that command's `allowed_tools` injected) — BEFORE the
    /// content is submitted as a turn. `None` (the default for every test
    /// widget) keeps the historical verbatim path: the RAW template is submitted
    /// and the model re-runs the git commands itself.
    shell_expansion: Option<std::sync::Arc<dyn command_api::ShellExpansionProvider>>,
    /// Live engine handle (`None` until the embedder wires one via
    /// [`Self::set_orchestrator`]). Drives the OrchestratorHandle-backed
    /// read/inject/effect commands (`/context`, `/files`, `/usage`, `/effort`,
    /// `/goal`, `/compact`). The read/inject ones route through the existing
    /// `run_core_command` `block_on` bridge (proven-safe: every method they
    /// call is a pure in-memory `session.lock()` + clone); `/compact`'s real
    /// network effect goes off-loop via [`ChatOutcome::Compact`] instead.
    /// `None` (every test widget) makes each of those a graceful
    /// "unavailable" system line rather than a panic.
    orchestrator: Option<std::sync::Arc<dyn traits::OrchestratorHandle>>,
    /// The shared slash-command registry (`None` until the embedder wires one
    /// via [`Self::set_command_registry`]). Drives `/reload-skills`, which
    /// reloads the SAME `Arc<RwLock<CommandRegistry>>` the headless dispatcher
    /// mutates (an in-memory + skill-dir fs scan; block_on-safe). Kept separate
    /// from `orchestrator` because [`command_core::reload_skills::ReloadSkillsHandler`]
    /// takes the registry, NOT the handle.
    command_registry:
        Option<std::sync::Arc<tokio::sync::RwLock<command_api::CommandRegistry>>>,
    /// The ONE persistent `/goal` handler, built off the live handle in
    /// [`Self::set_orchestrator`] (`None` until then). [`command_core::goal::GoalHandler`]
    /// keeps the active goal in a handler-local `Arc<Mutex>`, so a fresh
    /// per-call handler would forget a goal set by an earlier `/goal
    /// <condition>` — status/clear would always report "No goal set". Storing
    /// one instance and passing a cheap `.clone()` (shared-`Arc` state) into
    /// `run_core_command` keeps the goal alive across invocations.
    goal_handler: Option<command_core::goal::GoalHandler>,
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
            api_retry: None,
            activity: None,
            response_chars: 0,
            spinner_verb: spinner::sample_verb(),
            current_todo: None,
            pending_permissions: VecDeque::new(),
            start: std::time::Instant::now(),
            export_dir: crate::export::default_export_dir(),
            cost: None,
            last_rate_limit_text: None,
            has_shown_overage_notification: false,
            subscription: None,
            status_line: None,
            pending_terminal_sequences: Vec::new(),
            tool_inputs: std::collections::HashMap::new(),
            web_snapshot: None,
            permission_snapshot: None,
            resume_rows: Vec::new(),
            connect_auth_methods: std::collections::BTreeMap::new(),
            connect_availability: std::collections::BTreeMap::new(),
            shell_expansion: None,
            orchestrator: None,
            command_registry: None,
            goal_handler: None,
        }
    }

    /// Wire the shared prompt shell-expansion provider (#3). After this, a
    /// prompt-type builtin whose `InjectMessage` body carries embedded
    /// `` !`git …` `` / ` ```! ` patterns (`/commit`, `/commit-push-pr`,
    /// `/security-review`) has those commands expanded through the real host
    /// runner + policy-backed gate before the content is submitted as a turn —
    /// 1:1 with claude-code expanding the body inside `getPromptForCommand`. A
    /// permission-deny / run failure aborts the whole prompt (nothing is
    /// submitted). Wired from the CLI `run_app` off the engine runtime.
    pub fn set_shell_expansion(
        &mut self,
        provider: std::sync::Arc<dyn command_api::ShellExpansionProvider>,
    ) {
        self.shell_expansion = Some(provider);
    }

    /// Wire the live engine handle the OrchestratorHandle-backed commands
    /// (`/context`, `/files`, `/usage`, `/effort`, `/goal`, `/compact`) run
    /// against, and build the ONE persistent `/goal` handler off it (its goal
    /// state lives in a handler-local `Arc<Mutex>`, so a per-call handler would
    /// lose it — see the `goal_handler` field). Wired from the CLI `run_app`
    /// off the engine runtime; `None` (every test widget) keeps those commands
    /// as graceful no-ops.
    pub fn set_orchestrator(
        &mut self,
        handle: std::sync::Arc<dyn traits::OrchestratorHandle>,
    ) {
        self.goal_handler = Some(command_core::goal::GoalHandler::new(handle.clone()));
        self.orchestrator = Some(handle);
    }

    /// Wire the shared slash-command registry `/reload-skills` reloads (the
    /// SAME `Arc<RwLock<CommandRegistry>>` the dispatcher mutates). `None`
    /// (every test widget) keeps `/reload-skills` a graceful no-op.
    pub fn set_command_registry(
        &mut self,
        registry: std::sync::Arc<tokio::sync::RwLock<command_api::CommandRegistry>>,
    ) {
        self.command_registry = Some(registry);
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

    /// Commit the active streaming cell — but DISCARD it if it's the empty
    /// `AssistantTextCell` placeholder [`TurnEvent::TurnStarted`] opens. Used
    /// at every flush point that can fire before any assistant text streams (a
    /// leading tool call, a thinking-first turn, or a tool-only turn), so no
    /// stray bare `●` marker is committed with no body.
    fn flush_or_discard_active(&mut self) {
        let empty = self
            .transcript
            .mutate_active(|cell| {
                cell.as_any()
                    .downcast_ref::<AssistantTextCell>()
                    .is_some_and(|a| a.body().is_empty())
            })
            .unwrap_or(false);
        if empty {
            self.transcript.discard_active();
        } else {
            self.transcript.flush_active();
        }
    }

    /// Fold one streaming event from the orchestrator bridge into the
    /// transcript: `TurnStarted` opens an empty active assistant cell,
    /// `TextDelta` mutates it in place, `ToolUseStart`/`ToolUseResult` render
    /// the tool-call + result cells (and set/clear the spinner activity),
    /// `TurnEnded` finalizes the active cell (moves it to the committed
    /// history) and clears the in-flight cancel token. `CostUpdated` refreshes
    /// the status-row cost, `ContextPressure` sets/clears the pane banner,
    /// `CompactionCompleted` folds a compact-boundary marker, `RateLimit`
    /// composes a transcript notice, `TerminalSequence` stages a write-through
    /// escape, `SystemNotice`/`BashOutput` push system/bash-output rows — every
    /// bridge-emitted variant is handled (no wildcard drop).
    pub fn apply_turn_event(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::TurnStarted => {
                self.turn_started_at = Some(std::time::Instant::now());
                self.activity = None;
                self.api_retry = None;
                self.response_chars = 0;
                // Draw a fresh random verb for this turn (claude-code
                // `useState(() => sample(getSpinnerVerbs()))` — one verb per
                // turn, no rotation within it).
                self.spinner_verb = spinner::sample_verb();
                // A straggler active cell (missed TurnEnded) is finalized (or
                // discarded if it's the empty placeholder) before the new
                // streaming reply opens.
                self.flush_or_discard_active();
                self.transcript
                    .set_active(Box::new(AssistantTextCell::new(String::new())));
            }
            TurnEvent::TextDelta(delta) => {
                // Content arrived → the retried request succeeded; drop any
                // "Retrying…" status so the normal spinner/stream resumes.
                self.api_retry = None;
                self.response_chars = self
                    .response_chars
                    .saturating_add(delta.chars().count() as u64);
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
            TurnEvent::ThinkingDelta(delta) => {
                self.response_chars = self
                    .response_chars
                    .saturating_add(delta.chars().count() as u64);
                // M5 cc2.1.198 thinking streaming: append to the active
                // ThinkingCell (collapsed by default; Ctrl-O reveals the body),
                // mirroring the TextDelta path. A non-thinking active cell is
                // flushed first so a fresh collapsed thinking block opens; a
                // following TextDelta likewise flushes this thinking cell and
                // opens the assistant reply.
                let appended = self
                    .transcript
                    .mutate_active(|cell| {
                        if let Some(thinking) = cell.as_any_mut().downcast_mut::<ThinkingCell>() {
                            thinking.append(&delta);
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !appended {
                    self.flush_or_discard_active();
                    self.transcript.set_active(Box::new(ThinkingCell::new(delta)));
                }
            }
            TurnEvent::ToolUseStart { id, tool, input } => {
                self.activity = Some(activity_label(&tool));
                // (Gap B) A `TodoWrite` replaces the whole session todo list
                // each call, so its input is the authoritative source for the
                // spinner's "current todo" (claude-code derives `currentTodo`
                // from the live `tasksV2` list — `Spinner.tsx:162`). Refresh
                // on the START event so the verb tracks the new in-progress
                // task as soon as it's written, not only after the (later)
                // tool result returns. Assigned unconditionally (including
                // `None`) so a `TodoWrite` with no active task clears a
                // stale one.
                if tool == "TodoWrite" {
                    self.current_todo = current_todo_from_todowrite_input(&input);
                }
                // Commit any streamed assistant text ABOVE the tool call, then
                // render the tool-use header (`● {tool}` + input) into the
                // transcript — claude-code parity: each tool invocation shows
                // as its own scrollback cell between the assistant's text
                // segments. Without this flush the whole turn's text merged
                // into one active cell and every tool call was invisible.
                // Remember the input so the paired result can render the
                // Edit/Write diff (see `ToolUseResult`).
                self.flush_or_discard_active();
                self.tool_inputs.insert(id.clone(), input.clone());
                self.transcript
                    .push_message(RenderedMessage::AssistantToolUse { id, tool, input });
            }
            TurnEvent::ToolUseResult { id, tool, result } => {
                self.activity = None;
                // Recover the originating call's input (for diff tools) from the
                // side-table, mirroring the resume path's correlation, then
                // render the `⎿ {summary}` result cell (or an Edit/Write diff).
                let (old_string, new_string, file_path) = self
                    .tool_inputs
                    .remove(&id)
                    .map_or((None, None, None), |input| {
                        tui_core::active_turn::diff_inputs_for(&tool, &input)
                    });
                self.transcript.push_message(RenderedMessage::UserToolResult {
                    id,
                    tool,
                    result,
                    old_string,
                    new_string,
                    file_path,
                });
            }
            TurnEvent::TurnEnded(_) => {
                self.flush_or_discard_active();
                self.current_turn = None;
                self.turn_started_at = None;
                self.api_retry = None;
                self.activity = None;
                // (Gap B) `current_todo` is per-turn — clear it so the next
                // turn's spinner doesn't keep showing the previous turn's
                // `activeForm` until a fresh `TodoWrite` arrives.
                self.current_todo = None;
                // (review M1) Drop any un-paired tool inputs — a turn
                // interrupted between a `ToolUseStart` and its result would
                // otherwise leak the (possibly large) input for the rest of
                // the session. Mirrors `active_turn.rs` clearing on both
                // turn boundaries.
                self.tool_inputs.clear();
                // Re-arm the statusline pump (claude-code executes the command
                // on turn boundaries; the pump is debounced single-flight).
                self.with_status_line(|s| s.dirty = true);
            }
            TurnEvent::CostUpdated(cost_str) => {
                // Update the status-row cost so the next render pass shows the
                // post-turn dollar amount (old backend: `state.status.cost`).
                self.with_status_line(|s| s.data.cost = cost_str.clone());
                self.cost = Some(cost_str);
            }
            TurnEvent::ApiRetry {
                message,
                attempt,
                max_retries,
                delay_ms,
            } => {
                // An API request is backing off before its next attempt — show
                // Claude Code's `SystemAPIErrorMessage` line with a live
                // countdown. Cleared when content arrives / the turn ends.
                self.api_retry = Some(ApiRetryState {
                    message,
                    attempt,
                    max_retries,
                    deadline: std::time::Instant::now()
                        + std::time::Duration::from_millis(delay_ms),
                });
            }
            TurnEvent::ContextPressure {
                banner,
                used_fraction,
            } => {
                // (TokenWarning) The orchestrator-computed context-pressure
                // banner renders as its own pane row next pass; `None` clears a
                // previously-shown banner once the context drops below the
                // warning threshold (claude-code's `<TokenWarning>` returning
                // null).
                self.bottom_pane.set_context_pressure(banner);
                // Feed the live context usage into the statusline payload's
                // `context_window.used_percentage` (0-1 fraction).
                self.with_status_line(|s| s.data.context_pct = used_fraction);
            }
            TurnEvent::TerminalSequence { seq } => {
                // #6: stage the validated terminal escape sequence; the app
                // loop drains [`Self::take_terminal_sequences`] and writes the
                // bytes to the terminal that owns the controlling tty
                // (claude-code `BEo`; the old backend's async
                // `pump_terminal_sequence`).
                self.pending_terminal_sequences.push(seq);
            }
            TurnEvent::CompactionCompleted {
                messages_before,
                messages_after,
                ..
            } => {
                // M7-04 parity: fold a `CompactBoundary` marker (renders
                // `✻ Conversation compacted (ctrl+o for history)`; counts are
                // retained on the variant for debug/telemetry parity but not
                // rendered). Consecutive boundaries de-dupe to one marker —
                // the old backend's `push_compact_boundary` contract.
                self.push_compact_boundary(messages_before, messages_after);
            }
            TurnEvent::RateLimit {
                status,
                rate_limit_type,
                utilization,
                resets_at,
                claim_resets_at,
                overage_status,
                overage_resets_at,
                overage_disabled_reason,
                fallback_available,
            } => {
                self.apply_rate_limit(&crate::rate_limit_messages::RateLimitInfo {
                    status,
                    rate_limit_type,
                    utilization,
                    resets_at,
                    claim_resets_at,
                    overage_status,
                    overage_resets_at,
                    overage_disabled_reason,
                    fallback_available,
                });
            }
            // `RawUtilization`'s ONLY consumer is the configured statusline
            // command's `rate_limits` input: fold it into the shared pump slot
            // (no visible row of its own).
            TurnEvent::RawUtilization {
                five_hour_utilization,
                five_hour_resets_at,
                seven_day_utilization,
                seven_day_resets_at,
            } => {
                self.with_status_line(|s| {
                    s.data.raw_utilization =
                        Some(tui_core::status_line_command::RawUtilizationSnapshot {
                            five_hour_utilization,
                            five_hour_resets_at,
                            seven_day_utilization,
                            seven_day_resets_at,
                        });
                });
            }
            // The reserved `PermissionRequest` variant is never emitted by the
            // bridge — live prompts arrive through the `permission_bridge`
            // channel into [`Self::open_permission`] instead.
            TurnEvent::PermissionRequest { .. } => {}
            // A `/web` async effect (secret/settings save, test search)
            // finished off-loop; surface its result as a transcript line —
            // red for a failure, dim grey otherwise (`RenderedMessage::
            // SystemText`'s existing severity mapping).
            TurnEvent::SystemNotice { body, is_error } => {
                self.transcript.push_message(RenderedMessage::SystemText {
                    body,
                    timestamp: 0,
                    is_error,
                });
            }
            TurnEvent::BashOutput { stdout, stderr } => {
                // `!`-command output: render inline as a bash-output cell
                // (ANSI stdout + error-tinted stderr). No LLM turn involved.
                self.transcript
                    .push_message(RenderedMessage::UserBashOutput { stdout, stderr });
            }
            TurnEvent::ProviderConnected { provider_id } => {
                // A mid-session /connect succeeded: flip the live availability
                // map so the /model picker (gated by it) shows the newly
                // connected provider's models — and the /connect picker badges
                // it ✓ — without a restart. Keyed by profile_name, matching the
                // launch map. No transcript cell (the connect flow already
                // emitted its own ✓ SystemNotice).
                self.connect_availability.insert(provider_id, true);
            }
            TurnEvent::SubagentActivity { text } => {
                // A running subagent (Task/Agent) made a tool call — render it as
                // an indented `⎿` line so its inner work is visible under the
                // Task cell (otherwise a subagent's execution is invisible).
                self.transcript
                    .push_message(RenderedMessage::SubagentActivity { text });
            }
        }
    }

    /// Fold a compaction boundary into the transcript, de-duping consecutive
    /// boundaries (exactly one marker renders even if two sources report the
    /// same compaction — the old backend's `push_compact_boundary`).
    fn push_compact_boundary(&mut self, messages_before: u32, messages_after: u32) {
        if self
            .transcript
            .committed_cells()
            .last()
            .is_some_and(|cell| {
                cell.as_any()
                    .downcast_ref::<crate::history_cell::system::CompactBoundaryCell>()
                    .is_some()
            })
        {
            return;
        }
        self.transcript
            .push_message(RenderedMessage::CompactBoundary {
                messages_before,
                messages_after,
            });
    }

    /// Fold one `TurnEvent::RateLimit` header snapshot: compose the
    /// claude-code notice (`getRateLimitMessage`) and push it unless it
    /// duplicates the last rendered text, then fire the one-shot
    /// overage-transition notice (`useRateLimitWarningNotification.tsx`) on
    /// entering overage — reset the flag on leaving it. Ported from the old
    /// backend's `streaming::apply_event` `RateLimit` arm.
    fn apply_rate_limit(&mut self, info: &crate::rate_limit_messages::RateLimitInfo) {
        // Subscription-granular copy: the snapshot the composition root
        // resolved (`None`/unfilled until the background fetch lands →
        // default = unknown subscription, TS-conservative).
        let sub = self.subscription_snapshot().unwrap_or_default();
        if let Some(composed) = crate::rate_limit_messages::compose_rate_limit(info, &sub) {
            if self.last_rate_limit_text.as_deref() != Some(composed.text.as_str()) {
                self.last_rate_limit_text = Some(composed.text.clone());
                self.transcript.push_message(RenderedMessage::RateLimit {
                    text: composed.text,
                    upsell: composed.upsell,
                });
            }
        }
        // Overage-transition notice: fire ONCE on entering overage when
        // `!isTeamOrEnterprise || hasBillingAccess` (tsx :62); reset the
        // one-shot flag on leaving overage (tsx :70-72). Guarded by its own
        // flag (not the text dedupe slot) and survives `/clear`, like the TS
        // component state.
        if crate::rate_limit_messages::is_using_overage(info) {
            if !self.has_shown_overage_notification
                && (!sub.is_team_or_enterprise() || sub.has_claude_ai_billing_access())
            {
                self.has_shown_overage_notification = true;
                self.transcript.push_message(RenderedMessage::RateLimit {
                    text: crate::rate_limit_messages::using_overage_text(info, &sub),
                    upsell: None,
                });
            }
        } else {
            self.has_shown_overage_notification = false;
        }
    }

    /// Current resolved subscription snapshot, if the embedder wired a slot
    /// and the seed/background fetch has filled it. A poisoned lock degrades
    /// to `None` (conservative copy, never a panic in the event-fold path) —
    /// the documented `SharedSubscription` reader stance.
    fn subscription_snapshot(&self) -> Option<traits::subscription::SubscriptionSnapshot> {
        self.subscription
            .as_ref()
            .and_then(|s| s.read().ok())
            .and_then(|guard| guard.clone())
    }

    /// Wire the composition root's shared subscription slot so the rate-limit
    /// composer reads the live snapshot at compose time (the old backend's
    /// `Runtime::with_subscription`).
    pub fn set_subscription(&mut self, slot: traits::subscription::SharedSubscription) {
        self.subscription = Some(slot);
    }

    /// Wire the composition root's shared custom-statusline slot and seed its
    /// static pump inputs (model + cwd) from the current session. The async
    /// pump reads this slot; `TurnEvent`s keep the live cost / rate-limit
    /// inputs fresh (see [`Self::apply_turn_event`]).
    pub fn set_status_line(&mut self, slot: crate::status_line::SharedStatusLine) {
        if let Ok(mut s) = slot.lock() {
            let (id, display) = self.current_model_id_display();
            s.data.model_id = id;
            s.data.model = display;
            s.data.cwd = std::path::PathBuf::from(&self.session.doctor.cwd);
            // Run once on startup (claude-code executes the statusline on mount,
            // not only on turn boundaries).
            s.dirty = true;
        }
        self.status_line = Some(slot);
    }

    /// Wire the composition root's shared `/web` config snapshot slot, preloaded
    /// from real config + credential-store presence at startup. [`Self::cmd_web`]
    /// reads a clone of it to seed the picker; the async `on_web_action` effect
    /// closure keeps it current across saves/tests.
    pub fn set_web_snapshot(
        &mut self,
        slot: std::sync::Arc<std::sync::Mutex<crate::web::picker::WebConfigSnapshot>>,
    ) {
        self.web_snapshot = Some(slot);
    }

    /// Wire the composition root's shared `/permissions` rule snapshot slot,
    /// preloaded from the user/project/local settings files at startup.
    /// [`Self::cmd_permissions`] reads a clone of it to seed the editor; the
    /// async `on_permission_action` effect closure keeps it current across
    /// adds/removes.
    pub fn set_permission_snapshot(
        &mut self,
        slot: std::sync::Arc<std::sync::Mutex<PermissionsSnapshot>>,
    ) {
        self.permission_snapshot = Some(slot);
    }

    /// Wire the preloaded `/resume` session rows (loaded async from disk at
    /// startup, before the blocking loop). [`Self::cmd_resume`] clones these
    /// into the picker.
    pub fn set_resume_rows(&mut self, rows: Vec<crate::resume::ResumeRow>) {
        self.resume_rows = rows;
    }

    /// Cancel any in-flight streaming turn (its `CancellationToken`), used when
    /// the app loop is about to UNWIND for a `/resume` switch so the outgoing
    /// turn stops streaming into the session file the user just left. Mirrors
    /// the `Interrupt` handler's token cancel; a no-op when idle. Does NOT push
    /// the `[Request interrupted by user]` row — the outgoing session is being
    /// torn down, not returned to.
    pub fn cancel_active_turn(&mut self) {
        if let Some(token) = self.current_turn.take() {
            token.cancel();
        }
    }

    /// Wire the composition root's real per-provider login-method +
    /// availability maps (derived from the live multi-provider catalog at
    /// startup). [`Self::cmd_connect`] reads clones of both to build the
    /// `/connect` picker; empty maps (the default) render an empty picker.
    pub fn set_connect_data(
        &mut self,
        auth_methods: std::collections::BTreeMap<String, String>,
        availability: std::collections::BTreeMap<String, bool>,
    ) {
        self.connect_auth_methods = auth_methods;
        self.connect_availability = availability;
    }

    /// The current model's `(wire_id, display)` pair (falls back to
    /// `("(default)", "(default)")`), for the statusline payload's
    /// `model.id`/`model.display_name`.
    fn current_model_id_display(&self) -> (String, String) {
        self.session.models.iter().find(|m| m.is_current).map_or_else(
            || ("(default)".to_string(), "(default)".to_string()),
            |m| (m.request_model.clone(), m.display.clone()),
        )
    }

    /// The current model's display string (falls back to `(default)`), reused
    /// for the welcome banner + statusline seeding.
    fn current_model_display(&self) -> String {
        self.current_model_id_display().1
    }

    /// Re-point the snapshot's `is_current` marker at `request_model` after a
    /// `/model` switch. The `/model` picker's `●` marker, the statusline, and
    /// [`Self::current_model_id_display`] all derive the current model from
    /// `session.models`' `is_current` flag — which is otherwise frozen at
    /// launch, so without this a switch left the picker/statusline showing the
    /// OLD model. No-op if `request_model` isn't in the snapshot (then nothing
    /// is marked current, matching an unknown model).
    ///
    /// Matches by (model AND `profile`) so a wire id shared across providers
    /// (e.g. `gpt-5.5` on both OpenAI and GitHub Copilot) only marks the row of
    /// the ACTUAL switched-to provider — mirrors the launch-time marker in
    /// `mode.rs`. When `profile` is `None` (resolve-by-id, no provider pinned),
    /// falls back to matching by model id alone.
    fn set_current_model(&mut self, request_model: &str, profile: Option<&str>) {
        for m in &mut self.session.models {
            m.is_current = m.request_model == request_model
                && profile.is_none_or(|p| m.profile.as_deref() == Some(p));
        }
    }

    /// Update one field of the shared statusline slot (no-op when unwired).
    fn with_status_line<F: FnOnce(&mut crate::status_line::StatusLineShared)>(&self, f: F) {
        if let Some(slot) = &self.status_line {
            if let Ok(mut s) = slot.lock() {
                f(&mut s);
            }
        }
    }

    /// Drain the staged `TurnEvent::TerminalSequence` escapes (FIFO order).
    /// The app loop writes them through to the terminal's writer.
    #[must_use]
    pub fn take_terminal_sequences(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_terminal_sequences)
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
    ///
    /// Uses [`BottomPane::desired_height_for`] with the freshly computed
    /// [`Self::pane_status`] running flag rather than
    /// `self.bottom_pane.desired_height` directly: the pane's own running
    /// flag only updates inside `handle_key`/`render`, so right after
    /// `apply_turn_event` starts or ends a turn — before either of those runs
    /// again — the pane's stored flag is stale. Height is now
    /// running-dependent (the status indicator adds a row), so measuring
    /// against the stale flag would undersize the viewport for one tick and
    /// clip the live tail.
    #[must_use]
    pub fn desired_height(&self, width: u16) -> u16 {
        let running = self.pane_status().running;
        self.live_tail_height(width)
            .saturating_add(self.status_line_height())
            .saturating_add(self.bottom_pane.desired_height_for(width, running))
    }

    /// Draw the widget into `area` of `buf`: refreshes the pane's task status
    /// first so the spinner text/animation reflect this tick's turn state,
    /// then draws the streaming live tail ABOVE the pane (this is the only
    /// render path for in-flight text — it is never committed to native
    /// scrollback until finalized), then the pane (status line + composer
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
        // The custom statusline row(s) sit directly above the pane (below the
        // live tail). Carve them off the top of the pane area, drawn dim.
        let sl = self.status_line_lines();
        let sl_h = u16::try_from(sl.len()).unwrap_or(0).min(pane_area.height);
        for (row, line) in sl.iter().enumerate() {
            let row_area = Rect::new(
                pane_area.x,
                pane_area.y + u16::try_from(row).unwrap_or(u16::MAX),
                pane_area.width,
                1,
            );
            ratatui::widgets::Widget::render(
                ratatui::widgets::Paragraph::new(line.clone()),
                row_area,
                buf,
            );
        }
        let pane_area = Rect::new(
            pane_area.x,
            pane_area.y + sl_h,
            pane_area.width,
            pane_area.height.saturating_sub(sl_h),
        );
        self.bottom_pane.render(pane_area, buf);
    }

    /// The widget's cursor claim within `area` (the composer's cursor, or a
    /// full-frame view's; `None` → cursor hidden), positioned within the
    /// pane's sub-area below the live tail.
    #[must_use]
    pub fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        let (_, pane_area) = self.split_area(area, self.live_tail_height(area.width));
        // The statusline row(s) sit above the pane (see `render`); shift the
        // pane down by their height so the composer cursor lands correctly.
        let sl_h = self.status_line_height().min(pane_area.height);
        let pane_area = Rect::new(
            pane_area.x,
            pane_area.y + sl_h,
            pane_area.width,
            pane_area.height.saturating_sub(sl_h),
        );
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

    /// `/model`: open the model picker over the CONNECTED subset of
    /// [`SessionInfo::models`] — gated live by [`Self::connect_availability`]
    /// (seeded from the launch provider-availability map, updated in place by
    /// [`TurnEvent::ProviderConnected`] after a mid-session `/connect`). So a
    /// provider connected during the session (e.g. OpenRouter) shows its models
    /// immediately, and unconnected providers are hidden — no restart needed.
    /// An empty result still opens the picker (it renders its own empty
    /// message).
    pub(crate) fn cmd_model(&mut self, _args: &str) -> ChatOutcome {
        let rows =
            crate::session::connected_model_rows(&self.session.models, &self.connect_availability);
        self.bottom_pane.show_model_picker(rows);
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

    /// `/diff`: render uncommitted working-tree changes (`git diff HEAD`) into
    /// the transcript as read-only system output. Faithful v1 of claude-code's
    /// interactive `DiffDialog`; a scrollable overlay + per-turn-diff pages are
    /// deferred. The git call is a local, read-only host subprocess (no engine
    /// handle, no network, no state mutation), so it runs synchronously on the
    /// slash path — same class as the filesystem I/O `/export` performs — with
    /// no off-loop `ChatOutcome` effect.
    pub(crate) fn cmd_diff(&mut self, _args: &str) -> ChatOutcome {
        let cwd = std::path::PathBuf::from(&self.session.doctor.cwd);
        let out = crate::diff::collect_diff(&cwd);
        self.show_system_text(&out.body, out.is_error)
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

    /// `/web`: open the WebSearch provider picker, seeded from the shared
    /// snapshot (real config + credential-store presence, preloaded at
    /// startup and kept current by the async `on_web_action` effect closure).
    /// Falls back to the default (unconfigured) snapshot when no slot is
    /// wired (headless / tests).
    pub(crate) fn cmd_web(&mut self, _args: &str) -> ChatOutcome {
        let snapshot = self
            .web_snapshot
            .as_ref()
            .map(|m| m.lock().unwrap().clone())
            .unwrap_or_default();
        self.bottom_pane.show_web_picker(snapshot);
        ChatOutcome::Continue
    }

    /// `/permissions` (alias `/allowed-tools`): open the interactive
    /// allow/ask/deny rule editor, seeded from the shared snapshot (the
    /// user/project/local settings files, preloaded at startup and kept current
    /// by the async `on_permission_action` effect closure). Falls back to an
    /// empty snapshot when no slot is wired (headless / tests).
    pub(crate) fn cmd_permissions(&mut self, _args: &str) -> ChatOutcome {
        let snapshot = self
            .permission_snapshot
            .as_ref()
            .map(|m| m.lock().unwrap().clone())
            .unwrap_or_default();
        self.bottom_pane.show_permissions_editor(snapshot);
        ChatOutcome::Continue
    }

    /// `/add-dir <path>`: add a working directory to the session's
    /// `permissions.additionalDirectories`. The path is `~`-expanded, resolved
    /// against the cwd, normalized, and validated (must exist + be a directory,
    /// [`crate::add_dir`]); on success the durable settings write is driven
    /// off-loop via [`ChatOutcome::PermissionAction`] → `run_permission_action`
    /// (`permission::persist_workspace_directory`), reusing the `/permissions`
    /// effect channel. Validation errors and a bare-invocation usage line
    /// render synchronously as system text (1:1 with claude-code's
    /// `addDirHelpMessage`).
    pub(crate) fn cmd_add_dir(&mut self, args: &str) -> ChatOutcome {
        let input = args.trim();
        if input.is_empty() {
            return self.show_system_text("Usage: /add-dir <path>", false);
        }
        match crate::add_dir::resolve_and_validate(input) {
            crate::add_dir::AddDirValidation::Success { absolute } => {
                ChatOutcome::PermissionAction(PermissionAction::AddDirectory {
                    path: absolute,
                    dest: permission::PermissionUpdateDestination::LocalSettings,
                })
            }
            other => self.show_system_text(&crate::add_dir::help_message(&other), true),
        }
    }

    /// `/resume [term]` (alias `/continue`): open the interactive session
    /// picker, seeded from the rows preloaded at startup ([`Self::set_resume_rows`]).
    /// With a `term` argument the picker opens pre-filtered by title. On `Enter`
    /// the picker yields [`ChatOutcome::SwitchSession`], which unwinds the app
    /// loop so the runtime is re-mounted against the chosen session in-process
    /// (the JSONL writer is retargeted) — never an in-place engine swap.
    pub(crate) fn cmd_resume(&mut self, args: &str) -> ChatOutcome {
        self.bottom_pane
            .show_resume_picker(self.resume_rows.clone(), args.trim());
        ChatOutcome::Continue
    }

    /// `/connect [provider]`: with no argument, open the grouped provider
    /// picker seeded from the real catalog maps ([`Self::set_connect_data`]).
    /// With a `provider` argument, skip the picker and route directly into
    /// that provider's method-choice screen (multi-method) or its single
    /// flow — the key-entry view for `api_key`, or a
    /// [`ChatOutcome::ConnectAction`] effect for Copilot/OAuth providers.
    pub(crate) fn cmd_connect(&mut self, args: &str) -> ChatOutcome {
        let provider = args.trim();
        if provider.is_empty() {
            self.bottom_pane.show_connect_picker(
                self.connect_auth_methods.clone(),
                self.connect_availability.clone(),
            );
        } else {
            // /connect <provider>: route directly (skip picker) if known,
            // else open the picker.
            let tag = self.connect_auth_methods.get(provider).cloned();
            let methods = crate::connect::picker::provider_methods(provider, tag.as_deref());
            let label = crate::connect::picker::provider_label(provider);
            if methods.len() > 1 {
                self.bottom_pane.show_view(Box::new(
                    crate::bottom_pane::connect_method_view::ConnectMethodView::new(
                        provider.to_string(),
                        label,
                        methods,
                    ),
                ));
            } else {
                match methods.first().copied() {
                    Some(crate::connect::picker::ConnectMethod::ApiKey) | None => {
                        self.bottom_pane.show_view(Box::new(
                            crate::bottom_pane::connect_key_view::ConnectKeyView::new(provider),
                        ));
                    }
                    Some(crate::connect::picker::ConnectMethod::CopilotDevice) => {
                        return ChatOutcome::ConnectAction(ConnectAction::Copilot {
                            provider_id: provider.to_string(),
                        });
                    }
                    Some(_) => {
                        return ChatOutcome::ConnectAction(ConnectAction::OAuth {
                            provider_id: provider.to_string(),
                        });
                    }
                }
            }
        }
        ChatOutcome::Continue
    }

    /// `/color [name]`: set/clear/list the session accent color (tints the
    /// composer's `›` prompt). Pure parse ([`crate::color::parse_color_command`]) +
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
    /// The rate-limit dedupe slot resets with the scrollback: a suppressed
    /// identical notice would otherwise never reappear in the now-empty
    /// transcript. (`has_shown_overage_notification` deliberately NOT reset:
    /// the TS flag is component state, surviving transcript clears.)
    pub(crate) fn cmd_clear(&mut self, _args: &str) -> ChatOutcome {
        self.clear_transcript();
        ChatOutcome::Continue
    }

    /// Shared `/clear` effect (slash path and [`CommandAction::ClearTranscript`]
    /// view path): drop the transcript and the rate-limit dedupe slot together.
    fn clear_transcript(&mut self) {
        self.transcript.clear();
        self.last_rate_limit_text = None;
        // (review M1) Reset per-turn side-tables so `/clear` starts clean.
        self.tool_inputs.clear();
        self.current_todo = None;
    }

    /// `/image <path>`: record an image message for `path` so a graphics
    /// terminal shows the real pixels.
    pub(crate) fn cmd_image(&mut self, args: &str) -> ChatOutcome {
        self.push_image(args)
    }

    // ===== `command_core` bridge (batch: static-template + read-only handlers)
    //
    // A family of registry commands whose logic already lives in
    // `command_core` as `BuiltinCommandHandler`s. Rather than re-implement each
    // one against the TUI's launch-time snapshot, the TUI constructs the core
    // handler and runs it through the shared bridge below. Only handlers whose
    // `CommandResult` is `InjectMessage` (queue a prompt turn) or `Done`
    // (render read-only text) — and which construct from bare state — are wired
    // here; anything needing a live `OrchestratorHandle`/`AuthHandle` stays out.

    /// Bridge a `command_core` slash handler into the TUI: invoke the async
    /// handler and map its `CommandResult` to a `ChatOutcome`.
    ///
    /// Prompt-type handlers that embed `` !`git …` `` placeholders (`/commit`,
    /// `/commit-push-pr`, `/security-review`) have those commands expanded here
    /// (#3) through [`Self::shell_expansion`] — the real host runner +
    /// policy-backed gate, 1:1 with the dispatcher and claude-code's
    /// `getPromptForCommand`. The expansion runs on the same throwaway
    /// `block_on` used for the handler, so the sync render loop is not blocked
    /// beyond that already-blocking call. When no provider is wired (tests) the
    /// RAW template is submitted verbatim and the model re-runs the git commands.
    ///
    /// Known parity gap (self-healing, tracked as a follow-up):
    /// - `/skill-doctor` reports every skill as never-used because
    ///   `command_core::skill_doctor::record_skill_usage` is not yet wired into
    ///   the dispatcher (a shared-crate gap that predates this bridge and is
    ///   identical on the desktop/mobile registries).
    fn run_core_command(
        &mut self,
        name: &str,
        args: &str,
        handler: &dyn command_api::model::BuiltinCommandHandler,
    ) -> ChatOutcome {
        use command_api::model::CommandResult;
        let parsed = command_api::parser::ParsedSlashCommand {
            name: name.to_string(),
            raw_args: args.to_string(),
            positional_args: args.split_whitespace().map(str::to_string).collect(),
        };
        // The TUI event loop is synchronous (off the async runtime, on a
        // `spawn_blocking` thread), so a throwaway current-thread runtime is
        // safe. Build it fallibly: a failed runtime must surface a recoverable
        // error, not unwind the render task and take down the whole session
        // (mirrors the graceful `.build().map_err(..)` in
        // `apps/cli/src/commands/plugin_marketplace.rs`).
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => return self.show_system_text(&format!("/{name} failed: {err}"), true),
        };
        match runtime.block_on(handler.handle(&parsed)) {
            CommandResult::InjectMessage { content } => {
                // (#3) Expand embedded `!`git …`` / ` ```! ` bodies through the
                // real host runner + policy-backed gate BEFORE submitting, 1:1
                // with claude-code expanding the body inside `getPromptForCommand`
                // (each command injects its own `allowed_tools`; shell = None →
                // Bash for builtins). A permission-deny / run failure aborts the
                // WHOLE prompt: surface the error and do NOT submit (patterns are
                // never left in place). `None` provider (tests / no embedder) or a
                // body with no embedded pattern keeps the verbatim submit path.
                let has_embedded = content.contains("```!") || content.contains("!`");
                match self.shell_expansion.clone() {
                    Some(provider) if has_embedded => {
                        let allowed: Vec<String> = handler
                            .allowed_tools()
                            .iter()
                            .map(|s| (*s).to_string())
                            .collect();
                        let shell_ctx = provider.build(&allowed, None);
                        match runtime.block_on(command_api::execute_shell_commands_in_prompt(
                            &content,
                            &shell_ctx,
                            &format!("/{name}"),
                            None,
                        )) {
                            Ok(expanded) => self.submit_core_prompt(name, args, expanded),
                            Err(e) => self.show_system_text(&format!("/{name} failed: {e}"), true),
                        }
                    }
                    _ => self.submit_core_prompt(name, args, content),
                }
            }
            CommandResult::Done { display: Some(text) } => self.show_system_text(&text, false),
            CommandResult::Done { display: None } => ChatOutcome::Continue,
            // Only InjectMessage/Done commands are wired; guard the rest.
            CommandResult::EmitEffects { display, .. } => match display {
                Some(text) => self.show_system_text(&text, false),
                None => ChatOutcome::Continue,
            },
            CommandResult::RequestConfirmation { prompt, .. } => {
                self.show_system_text(&prompt, false)
            }
        }
    }

    /// Queue a prompt-type slash command's turn. The transcript shows only the
    /// compact `/name args` invocation the user typed, while the model receives
    /// the handler's expanded `content` — mirroring claude-code's
    /// displayed-metadata / hidden-`isMeta` split
    /// (`processSlashCommand.getMessagesForPromptSlashCommand`). Reusing
    /// [`Self::submit_prompt`] here would echo the entire template into the
    /// transcript as if the user had typed it.
    fn submit_core_prompt(&mut self, name: &str, args: &str, content: String) -> ChatOutcome {
        let invocation = if args.is_empty() {
            format!("/{name}")
        } else {
            format!("/{name} {args}")
        };
        self.transcript.push_message(RenderedMessage::UserText {
            body: invocation,
            timestamp: 0,
        });
        let token = CancellationToken::new();
        self.current_turn = Some(token.clone());
        ChatOutcome::Submit(content, token)
    }

    /// Render read-only command output into the transcript as a `system`
    /// message (the same path `/export` uses). `is_error` renders it in the
    /// error color.
    fn show_system_text(&mut self, text: &str, is_error: bool) -> ChatOutcome {
        self.transcript.push_message(RenderedMessage::SystemText {
            body: text.to_string(),
            timestamp: 0,
            is_error,
        });
        ChatOutcome::Continue
    }

    /// `/init`: inject the LINGXI.md initialization prompt as the next turn.
    pub(crate) fn cmd_init(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command("init", args, &command_core::init::InitHandler::new())
    }

    /// `/init-verifiers`: inject the verifier-skill scaffolding prompt.
    pub(crate) fn cmd_init_verifiers(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command(
            "init-verifiers",
            args,
            &command_core::init_verifiers::InitVerifiersHandler::new(),
        )
    }

    /// `/commit`: inject the git-commit prompt.
    pub(crate) fn cmd_commit(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command("commit", args, &command_core::commit::CommitHandler::new())
    }

    /// `/commit-push-pr`: inject the commit-push-PR prompt.
    pub(crate) fn cmd_commit_push_pr(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command(
            "commit-push-pr",
            args,
            &command_core::commit_push_pr::CommitPushPrHandler::new(),
        )
    }

    /// `/review`: inject the pull-request review prompt.
    pub(crate) fn cmd_review(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command("review", args, &command_core::review::ReviewHandler::new())
    }

    /// `/security-review`: inject the security-review prompt.
    pub(crate) fn cmd_security_review(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command(
            "security-review",
            args,
            &command_core::security_review::SecurityReviewHandler::new(),
        )
    }

    /// `/statusline`: inject the statusline-setup agent prompt.
    pub(crate) fn cmd_statusline(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command(
            "statusline",
            args,
            &command_core::statusline::StatuslineHandler::new(),
        )
    }

    /// `/insights`: inject the session-analysis report prompt.
    pub(crate) fn cmd_insights(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command(
            "insights",
            args,
            &command_core::insights::InsightsHandler::new(),
        )
    }

    /// `/version`: show the running version string.
    pub(crate) fn cmd_version(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command(
            "version",
            args,
            &command_core::version::VersionHandler::new(),
        )
    }

    /// `/release-notes`: show the changelog pointer.
    pub(crate) fn cmd_release_notes(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command(
            "release-notes",
            args,
            &command_core::release_notes::ReleaseNotesHandler::new(),
        )
    }

    /// `/stickers`: show the sticker-order message.
    pub(crate) fn cmd_stickers(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command(
            "stickers",
            args,
            &command_core::stickers::StickersHandler::new(),
        )
    }

    /// `/autocompact`: show the (read-only) auto-compact window status.
    pub(crate) fn cmd_autocompact(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command(
            "autocompact",
            args,
            &command_core::autocompact::AutocompactHandler::new(),
        )
    }

    /// `/terminal-setup`: detect the active terminal and install the Shift+Enter
    /// (Apple Terminal: Option+Enter) newline keybinding by writing the
    /// terminal's own config, then echo the result. Native-CSI-u terminals
    /// (Ghostty/Kitty/iTerm2/WezTerm/Warp) get an informational "already
    /// supported" line; unsupported terminals get setup guidance. Synchronous
    /// host I/O (no async engine / socket), so — unlike `/compact` — it is safe
    /// to run inline on the render thread's blocking loop.
    pub(crate) fn cmd_terminal_setup(&mut self, _args: &str) -> ChatOutcome {
        let (message, is_error) = tui_core::terminal_setup::run();
        self.show_system_text(&message, is_error)
    }

    /// `/keybindings`: open or preview the keybindings configuration.
    pub(crate) fn cmd_keybindings(&mut self, args: &str) -> ChatOutcome {
        self.run_core_command(
            "keybindings",
            args,
            &command_core::keybindings::KeybindingsHandler::new(),
        )
    }

    /// `/skill-doctor`: report which loaded skills are unused and costing
    /// context (computed from the launch-time filesystem roots).
    pub(crate) fn cmd_skill_doctor(&mut self, args: &str) -> ChatOutcome {
        let cwd = std::path::PathBuf::from(&self.session.doctor.cwd);
        let lingxi_home = std::path::PathBuf::from(&self.session.doctor.lingxi_home);
        self.run_core_command(
            "skill-doctor",
            args,
            &command_core::skill_doctor::SkillDoctorHandler::new(
                cwd,
                lingxi_home,
                None,
                Vec::new(),
            ),
        )
    }

    // ===== OrchestratorHandle-backed commands. The read/inject ones route
    // through `run_core_command`'s throwaway-`block_on` bridge — PROVEN safe
    // because every handle method they call is a pure in-memory `session.lock()`
    // + clone (no network / reactor / timer). Each is a graceful "unavailable"
    // system line when no handle is wired (`None`, every test widget). =====

    /// `/context`: show the current context-window usage (read-only).
    pub(crate) fn cmd_context(&mut self, args: &str) -> ChatOutcome {
        let Some(handle) = self.orchestrator.clone() else {
            return self
                .show_system_text("/context is unavailable (no engine handle wired)", true);
        };
        self.run_core_command(
            "context",
            args,
            &command_core::context::ContextHandler::new(handle),
        )
    }

    /// `/fork`: fork the conversation into a detached background agent (the live
    /// `fork_conversation` override spawns it via the `SubagentSpawner`). Bare
    /// `/fork` reaches the handler's "Usage: /fork <directive>" text.
    pub(crate) fn cmd_fork(&mut self, args: &str) -> ChatOutcome {
        let Some(handle) = self.orchestrator.clone() else {
            return self.show_system_text("/fork is unavailable (no engine handle wired)", true);
        };
        self.run_core_command("fork", args, &command_core::fork::ForkHandler::new(handle))
    }

    /// `/recap`: one-line session recap via the live `generate_recap` isolated,
    /// tool-denied side query (never mutates the conversation history).
    pub(crate) fn cmd_recap(&mut self, args: &str) -> ChatOutcome {
        let Some(handle) = self.orchestrator.clone() else {
            return self.show_system_text("/recap is unavailable (no engine handle wired)", true);
        };
        self.run_core_command("recap", args, &command_core::recap::RecapHandler::new(handle))
    }

    /// `/btw <question>`: ask a quick side question answered by an isolated,
    /// tool-denied, single-turn side query that shares the conversation context
    /// but NEVER enters the LLM history. Delivered through the SAME
    /// `run_core_command` bridge `/recap` uses (a throwaway `block_on` on the
    /// render-loop's blocking thread) against the history-inert
    /// `answer_side_question` seam; the trimmed answer renders as a system line.
    /// Bare `/btw` shows the usage line; graceful "unavailable" when no engine
    /// handle is wired.
    pub(crate) fn cmd_btw(&mut self, args: &str) -> ChatOutcome {
        if args.trim().is_empty() {
            return self.show_system_text("Usage: /btw <your question>", false);
        }
        let Some(handle) = self.orchestrator.clone() else {
            return self.show_system_text("/btw is unavailable (no engine handle wired)", true);
        };
        self.run_core_command(
            "btw",
            args,
            &command_core::side_question::SideQuestionHandler::new(handle),
        )
    }

    /// `/rename [name]`: persist a user-set title for the current session.
    /// With a name, returns [`ChatOutcome::RenameSession`] so the CLI appends
    /// the `custom-title` JSONL line off the render thread (state mutation goes
    /// off-loop) and echoes the confirmation via `TurnEvent::SystemNotice`,
    /// mirroring claude-code's `saveCustomTitle` + `onDone("Session renamed
    /// to: ...")`. Bare `/rename` renders a usage line — auto-name generation
    /// (claude-code's Haiku side-query) is deferred. Graceful "unavailable"
    /// no-op when no engine handle is wired.
    pub(crate) fn cmd_rename(&mut self, args: &str) -> ChatOutcome {
        if self.orchestrator.is_none() {
            return self.show_system_text("/rename is unavailable (no engine handle wired)", true);
        }
        let name = args.trim();
        if name.is_empty() {
            return self.show_system_text("Usage: /rename <name>", true);
        }
        ChatOutcome::RenameSession(name.to_string())
    }

    /// `/files`: list the files currently in context (read-only).
    pub(crate) fn cmd_files(&mut self, args: &str) -> ChatOutcome {
        let Some(handle) = self.orchestrator.clone() else {
            return self.show_system_text("/files is unavailable (no engine handle wired)", true);
        };
        self.run_core_command("files", args, &command_core::files::FilesHandler::new(handle))
    }

    /// `/usage`: show the session cost/token usage summary (read-only).
    pub(crate) fn cmd_usage(&mut self, args: &str) -> ChatOutcome {
        let Some(handle) = self.orchestrator.clone() else {
            return self.show_system_text("/usage is unavailable (no engine handle wired)", true);
        };
        self.run_core_command("usage", args, &command_core::usage::UsageHandler::new(handle))
    }

    /// `/effort`: show or set the model effort level. The set/clear write is a
    /// direct-fs `settings.json` merge inside the handler (not via the handle);
    /// it persists a default for NEW sessions, so the live turn's effort is
    /// unchanged — the same parity limitation as the headless dispatcher.
    /// `/fast [on|off]`: toggle fast mode (the priority `speed:"fast"` tier).
    /// `on`/`off` set the state; a bare `/fast` toggles it. The change is an
    /// off-loop effect ([`ChatOutcome::FastMode`] →
    /// `OrchestratorHandle::set_fast_mode`) so the multi-field flag flip never
    /// blocks the render thread; the applied state is reported via
    /// `TurnEvent::SystemNotice`. Env-gated: when `LINGXI_DISABLE_FAST_MODE`
    /// (or `CLAUDE_CODE_DISABLE_FAST_MODE`) is truthy, fast mode is unavailable
    /// (claude-code `isFastModeEnabled()`) and the command is a no-op line.
    /// Graceful "unavailable" when no engine handle is wired.
    pub(crate) fn cmd_fast(&mut self, args: &str) -> ChatOutcome {
        if self.orchestrator.is_none() {
            return self.show_system_text("/fast is unavailable (no engine handle wired)", true);
        }
        let disabled = std::env::var("LINGXI_DISABLE_FAST_MODE")
            .or_else(|_| std::env::var("CLAUDE_CODE_DISABLE_FAST_MODE"))
            .map(|v| !v.is_empty() && v != "0" && v != "false")
            .unwrap_or(false);
        if disabled {
            return self.show_system_text("Fast mode is not available", true);
        }
        match args.trim().to_ascii_lowercase().as_str() {
            "on" => ChatOutcome::FastMode(Some(true)),
            "off" => ChatOutcome::FastMode(Some(false)),
            "" => ChatOutcome::FastMode(None),
            other => self.show_system_text(
                &format!("Unknown /fast argument '{other}'. Usage: /fast [on|off]"),
                true,
            ),
        }
    }

    pub(crate) fn cmd_effort(&mut self, args: &str) -> ChatOutcome {
        let Some(handle) = self.orchestrator.clone() else {
            return self.show_system_text("/effort is unavailable (no engine handle wired)", true);
        };
        self.run_core_command(
            "effort",
            args,
            &command_core::effort::EffortHandler::new(handle),
        )
    }

    /// `/goal`: set, show, or clear a session-scoped stop-gating goal. Uses the
    /// ONE persistent [`command_core::goal::GoalHandler`] (built in
    /// [`Self::set_orchestrator`]) so status/clear see a goal set by a prior
    /// invocation — a fresh per-call handler would forget it (state lives in a
    /// handler-local `Arc<Mutex>`). The valid-condition arm returns
    /// `InjectMessage`, which the bridge submits as the next turn; status/clear
    /// return `Done` text.
    pub(crate) fn cmd_goal(&mut self, args: &str) -> ChatOutcome {
        let Some(handler) = self.goal_handler.clone() else {
            return self.show_system_text("/goal is unavailable (no engine handle wired)", true);
        };
        self.run_core_command("goal", args, &handler)
    }

    /// `/reload-skills`: pick up skills added or changed on disk this session,
    /// reloading them into the shared registry. Guards on the
    /// `command_registry` field (NOT the orchestrator). Cosmetic parity gap:
    /// the TUI slash palette is a static [`crate::command::BUILTIN`] table, so
    /// newly-added skills won't surface as new slash entries even though the
    /// reported count is truthful.
    pub(crate) fn cmd_reload_skills(&mut self, args: &str) -> ChatOutcome {
        let Some(registry) = self.command_registry.clone() else {
            return self.show_system_text(
                "/reload-skills is unavailable (no command registry wired)",
                true,
            );
        };
        self.run_core_command(
            "reload-skills",
            args,
            &command_core::reload_skills::ReloadSkillsHandler::new(registry),
        )
    }

    /// `/stop`: stop the session. Shows "Session stopped." then returns
    /// [`ChatOutcome::Quit`] — the app loop exits ONLY on `Quit` and never
    /// polls `current_should_exit`, so the plain `Done -> Continue` bridge
    /// would print the text but keep running. The handler's `request_exit()`
    /// does websocket-close I/O, so it is NOT run here (that would `block_on` a
    /// network future on the render thread's throwaway runtime — a
    /// cross-reactor hazard); the CLI fires `request_exit()` on the LIVE handle
    /// in its teardown after `run_app` returns. Behaves identically whether or
    /// not a handle is wired (the quit is handle-independent).
    pub(crate) fn cmd_stop(&mut self, _args: &str) -> ChatOutcome {
        self.show_system_text("Session stopped.", false);
        ChatOutcome::Quit
    }

    /// `/compact`: run a forced compaction pass. Returns
    /// [`ChatOutcome::Compact`] so the CLI drives
    /// [`traits::OrchestratorHandle::force_compact`] — a real multi-second LLM
    /// round-trip — on the LIVE runtime handle, reporting the summary via
    /// `TurnEvent::SystemNotice`. It must NOT go through `run_core_command`'s
    /// throwaway `block_on`: that would freeze the render/input thread for the
    /// whole call, drive the sockets on the wrong reactor, and race the turn
    /// loop's whole-history swap. Graceful "unavailable" no-op when unwired.
    pub(crate) fn cmd_compact(&mut self, args: &str) -> ChatOutcome {
        if self.orchestrator.is_none() {
            return self.show_system_text("/compact is unavailable (no engine handle wired)", true);
        }
        ChatOutcome::Compact(args.to_string())
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

    /// The custom-statusline command output as dim rows, read from the shared
    /// pump slot (empty when no statusline is configured or the command has not
    /// produced output yet). Rendered just above the bottom pane.
    fn status_line_lines(&self) -> Vec<ratatui::text::Line<'static>> {
        /// Cap on statusline rows so a pathological multi-line command cannot
        /// steal the whole pane and hide the composer (claude-code statuslines
        /// are single-line by convention; this is a defensive bound).
        const MAX_STATUS_LINE_ROWS: usize = 3;
        let Some(slot) = &self.status_line else {
            return Vec::new();
        };
        let Ok(shared) = slot.lock() else {
            return Vec::new();
        };
        let Some(text) = shared.text.as_deref() else {
            return Vec::new();
        };
        // `statusLine.padding` → left pad (claude-code `<Box paddingX>`); 0 default.
        let pad = " ".repeat(shared.config.as_ref().map_or(0, |c| c.padding));
        let dim = crate::style_adapter::to_ratatui(self.theme.dim);
        text.lines()
            .take(MAX_STATUS_LINE_ROWS)
            .map(|l| {
                ratatui::text::Line::from(ratatui::text::Span::styled(
                    format!("{pad}{l}"),
                    ratatui::style::Style::default().fg(dim),
                ))
            })
            .collect()
    }

    /// Row count the custom statusline wants (0 when none).
    fn status_line_height(&self) -> u16 {
        u16::try_from(self.status_line_lines().len()).unwrap_or(u16::MAX)
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
            cost: self.cost.clone(),
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
                self.current_todo = None;
                // (review) An interrupt IS a turn boundary — clear the tool
                // correlation map here too (a cancelled turn future may be
                // dropped before the bridge emits `TurnEnded`), matching the
                // other boundaries the M1 fix targeted.
                self.tool_inputs.clear();
                // Commit any streamed partial reply, then push the interrupt
                // row so scrollback shows `[Request interrupted by user]` (the
                // dim `Interrupted · …` line) — claude-code parity; the old
                // iocraft backend pushed the same UserText on its Cancel branch.
                self.flush_or_discard_active();
                self.transcript.push_message(RenderedMessage::UserText {
                    body: crate::history_cell::message::INTERRUPT_MESSAGE.to_string(),
                    timestamp: 0,
                });
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
                // Re-point the snapshot's current-model marker so the /model
                // picker `●`, statusline, and welcome line all follow the switch
                // (they read `session.models`' `is_current`, frozen at launch
                // otherwise). Provider-scoped so a wire id shared across
                // providers only dots the switched-to provider's row.
                self.set_current_model(&request_model, profile.as_deref());
                // Refresh the statusline model + re-arm the pump so the command
                // reports the new model (claude-code re-runs on model change).
                // Read the row `set_current_model` just marked `is_current` (already
                // provider-scoped) — NOT `find(request_model)` alone, which for a
                // wire id shared across providers could pick the wrong provider's
                // display and diverge from the ● marker.
                let display = self
                    .session
                    .models
                    .iter()
                    .find(|m| m.is_current)
                    .map_or_else(|| request_model.clone(), |m| m.display.clone());
                self.with_status_line(|s| {
                    s.data.model_id = request_model.clone();
                    s.data.model = display;
                    s.dirty = true;
                });
                ChatOutcome::SwitchModel(request_model, profile)
            }
            BottomPaneOutcome::RunCommand(action) => self.run_command(action),
            BottomPaneOutcome::PastedImage(path) => self.push_image(&path),
            BottomPaneOutcome::RunWebAction(action) => ChatOutcome::WebAction(action),
            BottomPaneOutcome::RunConnectAction(action) => ChatOutcome::ConnectAction(action),
            BottomPaneOutcome::RunPermissionAction(action) => {
                ChatOutcome::PermissionAction(action)
            }
            BottomPaneOutcome::SwitchSession(uuid) => ChatOutcome::SwitchSession(uuid),
        }
    }

    /// Route a submitted composer buffer: a registered slash command
    /// dispatches through the registry; anything else is sent as a prompt.
    fn dispatch_submission(&mut self, text: String) -> ChatOutcome {
        // (`!` bash mode) A `!`-prefixed line runs sandboxed and renders its
        // output inline — it is NOT sent to the model (no LLM turn). Echo the
        // `! {command}` row now; the CLI `on_bash` closure runs it and folds
        // stdout/stderr back as `TurnEvent::BashOutput`. 1:1 with claude-code
        // bash mode / the old iocraft `pending_bash` path.
        if let Some(rest) = text.strip_prefix('!') {
            let command = rest.trim().to_string();
            if !command.is_empty() {
                self.transcript.push_message(RenderedMessage::UserBashInput {
                    command: command.clone(),
                });
                return ChatOutcome::RunBash(command);
            }
        }
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
                self.clear_transcript();
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

    /// The current streaming-spinner text: an animated Claude-accent glyph, a
    /// verb, and an elapsed-seconds counter with an interrupt hint —
    /// claude-code status parity.
    ///
    /// Verb precedence (mirrors the deleted iocraft `SpinnerWithVerb` /
    /// claude-code `Spinner.tsx:169`'s `overrideMessage ?? currentTodo?.
    /// activeForm ?? currentTodo?.subject ?? randomVerb`, adapted to this
    /// backend's generic tool-activity label): the active `TodoWrite` todo's
    /// `activeForm`/`subject` (Gap B, [`spinner::todo_leader_verb`]) wins,
    /// then the in-flight tool's activity label (`Running Bash`, set from
    /// `ToolUseStart`), then the verb sampled once for this turn from
    /// [`spinner::SPINNER_VERBS`] (Gap A).
    fn spinner_text(&self) -> String {
        const FRAMES: &[&str] = &["·", "✢", "✳", "✶", "✻", "✽", "✽", "✻", "✶", "✳", "✢", "·"];
        let idx =
            usize::try_from(self.start.elapsed().as_millis() / 120).unwrap_or(0) % FRAMES.len();
        // While an API request is backing off, replace the verb with Claude
        // Code's `SystemAPIErrorMessage` line + a live countdown (attempt X/Y),
        // so the retry/error/backoff is visible during the otherwise-silent wait.
        if let Some(r) = &self.api_retry {
            let remaining = r
                .deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_secs();
            let unit = if remaining == 1 { "second" } else { "seconds" };
            return format!(
                "{} {} · Retrying in {remaining} {unit}… (attempt {}/{})",
                FRAMES[idx], r.message, r.attempt, r.max_retries
            );
        }
        let verb = spinner::todo_leader_verb(self.current_todo.as_ref())
            .or_else(|| self.activity.clone())
            .unwrap_or_else(|| self.spinner_verb.to_string());
        // Append the live `(<dur> · <↑|↓> <N> tokens)` counter from the start of
        // the turn (claude-code `SpinnerAnimationRow.tsx`; verified vs the real
        // binary). `receiving` (↓) once any delta/tool has arrived, else
        // requesting (↑). The interrupt hint is NOT here — it lives in the status
        // row (`status_indicator_line`).
        let elapsed_ms = self.turn_started_at.map_or(0, |t| t.elapsed().as_millis());
        let receiving = self.response_chars > 0 || self.activity.is_some();
        let paren = crate::spinner_status::status_paren(elapsed_ms, self.response_chars, receiving);
        format!("{} {verb}… {paren}", FRAMES[idx])
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

/// (Gap B) Resolve the spinner's "current todo" from a `TodoWrite` tool
/// input. A `TodoWrite` call replaces the entire session todo list; the
/// spinner shows the first todo that is neither `pending` nor `completed`
/// (claude-code `Spinner.tsx:162` — `tasksV2?.find(t => t.status !==
/// 'pending' && t.status !== 'completed')`). Returns `None` when the input is
/// malformed, the `todos` array is missing/empty, or every todo is
/// pending/completed.
///
/// The TodoWrite wire shape is `{ todos: [{ content, status, activeForm }] }`;
/// `content` is the spinner's `subject` fallback and `activeForm` (camelCase)
/// its preferred verb.
fn current_todo_from_todowrite_input(input: &serde_json::Value) -> Option<CurrentTodo> {
    let todos = input.get("todos")?.as_array()?;
    let item = todos.iter().find(|t| {
        let status = t.get("status").and_then(serde_json::Value::as_str);
        // Anything not pending/completed is "active" (in_progress, or any
        // other forward state). A missing status is treated as active too,
        // matching the `!==` semantics of the TS predicate.
        !matches!(status, Some("pending" | "completed"))
    })?;
    let subject = item
        .get("content")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let active_form = item
        .get("activeForm")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(CurrentTodo {
        subject,
        active_form,
    })
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

    /// The TUI event loop (`run_app`) runs inside `tokio::task::spawn_blocking`
    /// (apps/cli/src/mode.rs), so every `cmd_*` dispatch — including the
    /// `run_core_command` bridge's `Runtime::block_on` over the async core
    /// handler — executes on a blocking-pool thread that already has an entered
    /// runtime context. Reproduce that exact nesting to prove the bridge does
    /// not hit tokio's "Cannot start a runtime from within a runtime" panic.
    /// The plain `cmd_*` unit tests run on a bare test thread and would NOT
    /// catch a nested-runtime regression.
    #[test]
    fn core_bridge_block_on_survives_spawn_blocking_context() {
        let outer = tokio::runtime::Runtime::new().expect("outer runtime");
        let outcome = outer.block_on(async {
            tokio::task::spawn_blocking(|| {
                let mut widget = widget();
                widget.cmd_version("")
            })
            .await
            .expect("spawn_blocking join")
        });
        assert!(matches!(outcome, ChatOutcome::Continue));
    }

    /// A prompt-type command (InjectMessage): the model receives the handler's
    /// expanded template, but the transcript shows only the compact `/commit`
    /// invocation the user typed — not the whole template echoed as a user
    /// message (claude-code displayed-metadata / hidden-isMeta parity).
    #[test]
    fn core_bridge_inject_message_submits_template_but_displays_invocation() {
        let mut widget = widget();
        let outcome = widget.cmd_commit("");
        let ChatOutcome::Submit(payload, _token) = outcome else {
            panic!("prompt-type command must queue a turn");
        };
        // The model payload is the full expanded handler template …
        assert!(
            payload.contains("git status"),
            "model receives the expanded /commit template: {payload}"
        );
        // … while the transcript shows only the compact invocation.
        let shown = cell::<crate::history_cell::message::UserTextCell>(&widget, 0).body();
        assert_eq!(shown, "/commit", "transcript shows the invocation, not the template");
    }

    /// A read-only command (Done{display}): output is rendered into the
    /// transcript as a non-error system message and the loop keeps running.
    #[test]
    fn core_bridge_done_renders_system_text() {
        let mut widget = widget();
        let outcome = widget.cmd_version("");
        assert!(matches!(outcome, ChatOutcome::Continue));
        let systext = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0);
        assert!(!systext.body().is_empty(), "version output rendered as system text");
        assert!(!systext.is_error(), "version output is not an error");
    }

    /// A handler that echoes the `ParsedSlashCommand` the bridge built, so the
    /// test below can assert argument forwarding end-to-end.
    struct ArgEchoHandler;

    #[async_trait::async_trait]
    impl command_api::model::BuiltinCommandHandler for ArgEchoHandler {
        async fn handle(
            &self,
            args: &command_api::parser::ParsedSlashCommand,
        ) -> command_api::model::CommandResult {
            command_api::model::CommandResult::Done {
                display: Some(format!(
                    "raw=[{}] positional={:?}",
                    args.raw_args, args.positional_args
                )),
            }
        }
        fn name(&self) -> &str {
            "arg-echo"
        }
        fn description(&self) -> &str {
            "test"
        }
    }

    /// The bridge forwards the argument tail into the handler's
    /// `ParsedSlashCommand`: `raw_args` verbatim and `positional_args`
    /// whitespace-split. Locks the plumbing for the arg-taking commands
    /// (`/commit fix bug`, `/review 123`, `/autocompact 20`).
    #[test]
    fn core_bridge_forwards_args_to_handler() {
        let mut widget = widget();
        let outcome = widget.run_core_command("arg-echo", "fix the bug", &ArgEchoHandler);
        assert!(matches!(outcome, ChatOutcome::Continue));
        let body = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0).body();
        assert!(body.contains("raw=[fix the bug]"), "raw_args forwarded verbatim: {body}");
        assert!(
            body.contains(r#"positional=["fix", "the", "bug"]"#),
            "positional_args whitespace-split: {body}"
        );
    }

    // ===== OrchestratorHandle-backed commands. Driven by the reusable
    // `orchestrator::test_support::MockOrchestratorHandle` (a dev-dependency):
    // its read methods return defaults, which is all `/context` etc. need. =====

    /// A widget with a wired `MockOrchestratorHandle`. The `Arc` is returned too
    /// so a test can assert against the mock's recorded calls.
    fn widget_with_orchestrator() -> (
        ChatWidget,
        std::sync::Arc<orchestrator::test_support::MockOrchestratorHandle>,
    ) {
        let mock = std::sync::Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
        let mut widget = widget();
        widget.set_orchestrator(mock.clone());
        (widget, mock)
    }

    /// (a) With a live handle wired, a read-only OrchestratorHandle command
    /// renders its `Done` text into the transcript as a non-error system line.
    #[test]
    fn orchestrator_read_command_renders_done_text_when_wired() {
        let (mut widget, _mock) = widget_with_orchestrator();
        let outcome = widget.cmd_context("");
        assert!(matches!(outcome, ChatOutcome::Continue));
        let systext = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0);
        assert!(
            systext.body().contains("Context Usage"),
            "/context rendered its report: {}",
            systext.body()
        );
        assert!(!systext.is_error());

        // /usage on a fresh widget renders its cost/token summary too.
        let (mut widget, _mock) = widget_with_orchestrator();
        assert!(matches!(widget.cmd_usage(""), ChatOutcome::Continue));
        let systext = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0);
        assert!(!systext.body().is_empty(), "/usage rendered a summary");
        assert!(!systext.is_error());
    }

    /// `/fork` dispatches to `ForkHandler` with the live handle: bare `/fork`
    /// reaches the handler's usage text (proving it is NOT `ArgSpec::Required`),
    /// and a directive reaches the transcript gate (mock has no prior turn).
    #[test]
    fn fork_command_wired_to_orchestrator_handle() {
        let (mut widget, _mock) = widget_with_orchestrator();
        assert!(matches!(widget.cmd_fork(""), ChatOutcome::Continue));
        assert_eq!(
            cell::<crate::history_cell::system::SystemTextCell>(&widget, 0).body(),
            "Usage: /fork <directive>",
        );

        let (mut widget, _mock) = widget_with_orchestrator();
        assert!(matches!(widget.cmd_fork("investigate the bug"), ChatOutcome::Continue));
        assert_eq!(
            cell::<crate::history_cell::system::SystemTextCell>(&widget, 0).body(),
            "Cannot fork before the first conversation turn",
        );
    }

    /// `/recap` dispatches to `RecapHandler` with the live handle (renders text
    /// via the isolated side query, never panics).
    #[test]
    fn recap_command_wired_to_orchestrator_handle() {
        let (mut widget, _mock) = widget_with_orchestrator();
        assert!(matches!(widget.cmd_recap(""), ChatOutcome::Continue));
        let sys = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0);
        assert!(!sys.body().is_empty(), "/recap rendered text via the handler");
    }

    /// Unwired (no engine handle) → `/fork` and `/recap` are graceful error
    /// lines, never a panic.
    #[test]
    fn fork_recap_unavailable_when_unwired() {
        let mut w = widget();
        assert!(matches!(w.cmd_fork("x"), ChatOutcome::Continue));
        assert!(cell::<crate::history_cell::system::SystemTextCell>(&w, 0).is_error());

        let mut w2 = widget();
        assert!(matches!(w2.cmd_recap(""), ChatOutcome::Continue));
        assert!(cell::<crate::history_cell::system::SystemTextCell>(&w2, 0).is_error());
    }

    /// (b) `/stop` shows "Session stopped." then returns `ChatOutcome::Quit`
    /// (the plain `Done -> Continue` bridge would leave the TUI running).
    #[test]
    fn stop_command_shows_message_and_returns_quit() {
        let mut widget = widget();
        let outcome = widget.cmd_stop("");
        assert!(matches!(outcome, ChatOutcome::Quit), "/stop must quit the app");
        let systext = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0);
        assert_eq!(systext.body(), "Session stopped.");
        assert!(!systext.is_error());
    }

    /// (d) `/compact` returns `ChatOutcome::Compact(args)` — the CLI drives the
    /// real `force_compact()` off-loop. Returning the variant (rather than
    /// running it inline) is exactly what keeps the network call off the render
    /// thread's `block_on`.
    #[test]
    fn compact_command_returns_compact_outcome_off_loop() {
        let (mut widget, _mock) = widget_with_orchestrator();
        let outcome = widget.cmd_compact("");
        let ChatOutcome::Compact(args) = outcome else {
            panic!("/compact must return ChatOutcome::Compact");
        };
        assert_eq!(args, "");
    }

    /// (c) Unwired (`None`, every existing test widget), each new command is a
    /// graceful no-op: it renders an "unavailable" error line and keeps running
    /// — never panics. `/stop` is handle-independent, so it still quits.
    #[test]
    fn orchestrator_commands_are_graceful_noops_when_unwired() {
        let commands: &[(&str, fn(&mut ChatWidget, &str) -> ChatOutcome)] = &[
            ("/context", ChatWidget::cmd_context),
            ("/files", ChatWidget::cmd_files),
            ("/usage", ChatWidget::cmd_usage),
            ("/effort", ChatWidget::cmd_effort),
            ("/goal", ChatWidget::cmd_goal),
            ("/reload-skills", ChatWidget::cmd_reload_skills),
            ("/compact", ChatWidget::cmd_compact),
        ];
        for (name, run) in commands {
            let mut widget = widget();
            let outcome = run(&mut widget, "");
            assert!(
                matches!(outcome, ChatOutcome::Continue),
                "{name} unwired must Continue (no-op)"
            );
            let systext = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0);
            assert!(systext.is_error(), "{name} unwired renders an error line");
            assert!(
                systext.body().contains("unavailable"),
                "{name} unwired body: {}",
                systext.body()
            );
        }
        // /stop needs no handle: it quits regardless.
        let mut widget = widget();
        assert!(matches!(widget.cmd_stop(""), ChatOutcome::Quit));
    }

    /// `/goal` keeps its goal across invocations: setting a condition arms a
    /// turn, and a later status call on the SAME widget reports the active goal.
    /// A fresh per-call handler would report "No goal set" — this proves the ONE
    /// persistent `GoalHandler` instance retains its handler-local state.
    #[test]
    fn goal_handler_persists_state_across_invocations() {
        let (mut widget, _mock) = widget_with_orchestrator();
        let outcome = widget.cmd_goal("finish the task");
        assert!(
            matches!(outcome, ChatOutcome::Submit(_, _)),
            "/goal <condition> arms a turn (InjectMessage)"
        );
        let outcome = widget.cmd_goal("");
        assert!(matches!(outcome, ChatOutcome::Continue));
        // cells: [UserText "/goal finish the task", SystemText status].
        let status = cell::<crate::history_cell::system::SystemTextCell>(&widget, 1);
        assert!(
            status.body().contains("Goal active: finish the task"),
            "status echoes the persisted goal: {}",
            status.body()
        );
    }

    /// `/reload-skills` reloads the wired shared registry and reports the count.
    #[test]
    fn reload_skills_reports_count_when_registry_wired() {
        let mut widget = widget();
        let registry = std::sync::Arc::new(tokio::sync::RwLock::new(
            command_api::CommandRegistry::new(),
        ));
        widget.set_command_registry(registry);
        let outcome = widget.cmd_reload_skills("");
        assert!(matches!(outcome, ChatOutcome::Continue));
        let systext = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0);
        assert!(
            systext.body().contains("Reloaded skills:"),
            "reload message: {}",
            systext.body()
        );
        assert!(!systext.is_error());
    }

    /// A fake shell-expansion provider for the TUI expansion smoke tests: the
    /// runner echoes a fixed marker for any command, and the gate allows or
    /// denies. Proves `run_core_command` actually invokes expansion on a
    /// prompt-type command's `` !`git …` `` body before submitting — without
    /// touching the host shell.
    struct FakeExpansionProvider {
        deny: bool,
    }

    struct MarkerRunner;

    #[async_trait::async_trait]
    impl command_api::ShellRunner for MarkerRunner {
        async fn run(
            &self,
            _command: &str,
            _shell: Option<command_api::FrontmatterShell>,
        ) -> Result<command_api::ShellOut, command_api::ShellRunError> {
            Ok(command_api::ShellOut {
                stdout: "EXPANDED_MARKER".to_string(),
                stderr: String::new(),
                interrupted: false,
            })
        }
    }

    struct FixedGate {
        allow: bool,
    }

    impl command_api::ShellPermissionGate for FixedGate {
        fn check(
            &self,
            _command: &str,
            _shell: Option<command_api::FrontmatterShell>,
        ) -> command_api::ShellPermissionDecision {
            if self.allow {
                command_api::ShellPermissionDecision::Allow
            } else {
                command_api::ShellPermissionDecision::Deny {
                    message: Some("denied by test".to_string()),
                }
            }
        }
    }

    impl command_api::ShellExpansionProvider for FakeExpansionProvider {
        fn build(
            &self,
            _allowed_tools: &[String],
            _shell: Option<command_api::FrontmatterShell>,
        ) -> command_api::ShellExpansionCtx {
            command_api::ShellExpansionCtx {
                runner: std::sync::Arc::new(MarkerRunner),
                permission_gate: std::sync::Arc::new(FixedGate { allow: !self.deny }),
            }
        }
    }

    /// (#3) With a wired provider, a prompt-type command's embedded `` !`git …` ``
    /// patterns are expanded to the runner's output BEFORE submission — the model
    /// receives real command output, not the literal placeholder — while the
    /// transcript still shows the compact `/commit` invocation.
    #[test]
    fn tui_prompt_command_expands_embedded_shell_before_submit() {
        let mut widget = widget();
        widget.set_shell_expansion(std::sync::Arc::new(FakeExpansionProvider { deny: false }));
        let ChatOutcome::Submit(payload, _token) = widget.cmd_commit("") else {
            panic!("/commit must submit a turn");
        };
        assert!(
            payload.contains("EXPANDED_MARKER"),
            "embedded !`git …` replaced by runner output: {payload}"
        );
        assert!(
            !payload.contains("!`git status`"),
            "literal placeholder is gone after expansion: {payload}"
        );
        assert_eq!(
            cell::<crate::history_cell::message::UserTextCell>(&widget, 0).body(),
            "/commit",
            "transcript still shows the compact invocation"
        );
    }

    /// (#3) A permission-denied embedded command aborts the WHOLE prompt: the
    /// expansion errors, so nothing is submitted and the failure surfaces as an
    /// error message (patterns are never left in place / delivered).
    #[test]
    fn tui_prompt_command_denied_expansion_does_not_submit() {
        let mut widget = widget();
        widget.set_shell_expansion(std::sync::Arc::new(FakeExpansionProvider { deny: true }));
        let outcome = widget.cmd_commit("");
        assert!(
            matches!(outcome, ChatOutcome::Continue),
            "a denied expansion must NOT submit a turn"
        );
        let sys = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0);
        assert!(sys.is_error(), "expansion failure surfaced as an error cell");
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
        let mut widget = ChatWidget::new(
            Vec::new(),
            SessionInfo {
                models: vec![
                    ModelRow {
                        display: "Opus".into(),
                        request_model: "claude-opus-4-8".into(),
                        profile: Some("anthropic".into()),
                        provider_label: "Anthropic".into(),
                        is_current: true,
                        supports_reasoning: true,
                    },
                    ModelRow {
                        display: "Sonnet".into(),
                        request_model: "claude-sonnet-5".into(),
                        profile: Some("anthropic".into()),
                        provider_label: "Anthropic".into(),
                        is_current: false,
                        supports_reasoning: true,
                    },
                ],
                ..Default::default()
            },
        );
        // The /model picker gates by live provider availability: anthropic must
        // be connected for its (curated) models to show.
        widget.set_connect_data(
            std::collections::BTreeMap::new(),
            [("anthropic".to_string(), true)].into_iter().collect(),
        );
        widget
    }

    /// An 80x24 bottom-anchored test terminal with a 4-row viewport.
    fn test_terminal() -> Terminal<TestWriteBackend> {
        let mut terminal =
            Terminal::with_options(TestWriteBackend::new(80, 24)).expect("test terminal");
        terminal.set_bottom_viewport_height(4).expect("viewport");
        terminal
    }

    /// A tool call with NO preamble text (the common agentic case) renders the
    /// tool-use + result cells and does NOT leave a stray empty assistant
    /// placeholder — the `TurnStarted` empty cell is discarded, not committed.
    #[test]
    fn tool_first_turn_renders_tool_cells_without_empty_placeholder() {
        use crate::history_cell::tool::{ToolResultCell, ToolUseCell};
        let mut widget = widget();
        submit_command(&mut widget, "run it");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        // Straight to a tool call — no TextDelta first.
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Read".to_string(),
            input: serde_json::json!({ "file_path": "/tmp/x" }),
        });
        widget.apply_turn_event(TurnEvent::ToolUseResult {
            id: protocol::ToolUseId::from("t1"),
            tool: "Read".to_string(),
            result: serde_json::json!("file body"),
        });
        widget.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        // Exactly: user "run it" · ● Read · ⎿ result — NO empty assistant cell.
        assert_eq!(widget.transcript.committed_cells().len(), 3);
        assert_eq!(cell::<ToolUseCell>(&widget, 1).tool(), "Read");
        assert!(cells(&widget)[2].as_any().downcast_ref::<ToolResultCell>().is_some());
        // The (empty) placeholder must not survive as an assistant cell.
        assert!(cells(&widget)
            .iter()
            .all(|c| c.as_any().downcast_ref::<AssistantTextCell>().is_none()));
    }

    /// `!`-prefixed bash mode runs the command inline (no LLM turn): it echoes
    /// the `! {command}` row and returns `ChatOutcome::RunBash`, and the
    /// captured output folds back via `TurnEvent::BashOutput`.
    #[test]
    fn bang_command_runs_bash_inline_not_a_model_turn() {
        let mut widget = widget();
        typ(&mut widget, "!echo hi");
        let outcome = widget.handle_key(press(KeyCode::Enter));
        let ChatOutcome::RunBash(cmd) = outcome else {
            panic!("expected RunBash (bash mode), got a model turn");
        };
        assert_eq!(cmd, "echo hi", "leading ! stripped, trimmed");
        assert!(!widget.turn_running(), "bash mode raises no LLM turn");
        // The `! echo hi` input row echoed into scrollback.
        let all = cells(&widget);
        let input: String = all[all.len() - 1]
            .display_lines(80, &Theme::dark(), crate::history_cell::RenderMode::default())
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(input.contains("echo hi"), "bash input echoed: {input}");
        // Captured output folds back via BashOutput → a bash-output cell.
        widget.apply_turn_event(TurnEvent::BashOutput {
            stdout: "hi\n".to_string(),
            stderr: String::new(),
        });
        let all = cells(&widget);
        let out: String = all[all.len() - 1]
            .display_lines(80, &Theme::dark(), crate::history_cell::RenderMode::default())
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(out.contains("hi"), "bash output rendered: {out}");
    }

    /// (review M1) A turn ending between a tool's start and its result must
    /// not leak the un-paired input for the rest of the session.
    #[test]
    fn tool_inputs_cleared_when_turn_ends_without_a_result() {
        let mut widget = widget();
        submit_command(&mut widget, "go");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Write".to_string(),
            input: serde_json::json!({ "content": "big payload" }),
        });
        assert_eq!(widget.tool_inputs.len(), 1);
        widget.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        assert!(
            widget.tool_inputs.is_empty(),
            "un-paired tool input cleared on TurnEnded"
        );
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

        // ToolUseStart sets the spinner activity AND commits the streamed
        // text above the tool call, then renders the tool-use cell; the paired
        // ToolUseResult renders the result cell and clears the activity.
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Bash".to_string(),
            input: serde_json::json!({}),
        });
        assert_eq!(widget.activity.as_deref(), Some("Running Bash"));
        assert!(widget.spinner_text().contains("Running Bash"));
        // The "Hello" reply flushed to a committed cell; the tool-use cell
        // renders after it (cells: user, Hello, ● Bash).
        assert_eq!(
            cell::<crate::history_cell::tool::ToolUseCell>(&widget, 2).tool(),
            "Bash"
        );
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
        // user "hi" · assistant "Hello" · ● Bash tool-use · ⎿ Bash result = 4.
        assert_eq!(widget.transcript.committed_cells().len(), 4);
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
        // Interrupt pushes a `[Request interrupted by user]` row rendering the
        // dim `Interrupted · …` line into scrollback (claude-code parity).
        let all = cells(&widget);
        let last = all[all.len() - 1];
        let rendered: String = last
            .display_lines(80, &Theme::dark(), crate::history_cell::RenderMode::default())
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(rendered.contains("Interrupted"), "interrupt row: {rendered}");
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

    fn open_picker_request_models(widget: &ChatWidget) -> Vec<String> {
        widget
            .bottom_pane()
            .view_stack()
            .active()
            .and_then(|v| {
                v.as_any()
                    .downcast_ref::<crate::bottom_pane::model_picker_view::ModelPickerView>()
            })
            .expect("model picker open")
            .rows()
            .iter()
            .map(|r| r.request_model.clone())
            .collect()
    }

    #[test]
    fn connecting_a_provider_mid_session_surfaces_its_models_in_the_model_picker() {
        // Regression (reported): OpenRouter's models were absent from /model,
        // and /model must show only connected providers. Captured catalog holds
        // anthropic (current, connected) + OpenRouter (NOT yet connected).
        let mut widget = ChatWidget::new(
            Vec::new(),
            SessionInfo {
                models: vec![
                    ModelRow {
                        display: "Opus".into(),
                        request_model: "claude-opus-4-8".into(),
                        profile: Some("anthropic".into()),
                        provider_label: "Anthropic".into(),
                        is_current: true,
                        supports_reasoning: true,
                    },
                    ModelRow {
                        display: "OR Auto".into(),
                        request_model: "openrouter/auto".into(),
                        profile: Some("openrouter".into()),
                        provider_label: "OpenRouter".into(),
                        is_current: false,
                        supports_reasoning: true,
                    },
                    ModelRow {
                        display: "OR GPT".into(),
                        request_model: "openai/gpt-4o".into(),
                        profile: Some("openrouter".into()),
                        provider_label: "OpenRouter".into(),
                        is_current: false,
                        supports_reasoning: true,
                    },
                ],
                ..Default::default()
            },
        );
        widget.set_connect_data(
            std::collections::BTreeMap::new(),
            [("anthropic".to_string(), true)].into_iter().collect(),
        );

        // Before connecting OpenRouter: only anthropic's current model shows.
        submit_command(&mut widget, "/model");
        let before = open_picker_request_models(&widget);
        assert!(before.contains(&"claude-opus-4-8".to_string()));
        assert!(
            !before.iter().any(|m| m.contains('/')),
            "OpenRouter models hidden before connecting: {before:?}"
        );
        widget.handle_key(press(KeyCode::Esc));

        // Connect OpenRouter mid-session — the CLI's write-back event.
        widget.apply_turn_event(TurnEvent::ProviderConnected {
            provider_id: "openrouter".to_string(),
        });

        // Now /model surfaces OpenRouter's CURATED shortlist (the `openrouter/auto`
        // alias) alongside the still-connected anthropic current model. The paid
        // non-alias passthrough (`openai/gpt-4o`) is trimmed by the opencode-style
        // OpenRouter curation (free + latest-aliases only).
        submit_command(&mut widget, "/model");
        let after = open_picker_request_models(&widget);
        assert!(
            after.contains(&"openrouter/auto".to_string()),
            "OpenRouter alias appears after connecting: {after:?}"
        );
        assert!(
            !after.contains(&"openai/gpt-4o".to_string()),
            "paid non-alias OpenRouter model trimmed by curation: {after:?}"
        );
        assert!(
            after.contains(&"claude-opus-4-8".to_string()),
            "anthropic current model still shown: {after:?}"
        );
    }

    #[test]
    fn model_picker_current_marker_follows_a_switch() {
        // Regression (reported after resume): the /model picker `●` current
        // marker was frozen at launch — switching updated the statusline but not
        // `session.models`' is_current, so reopening /model still marked the OLD
        // model. Switch Opus→Sonnet, then the reopened picker marks Sonnet.
        let mut widget = widget_with_models(); // Opus (current) + Sonnet, anthropic connected
        assert_eq!(
            widget
                .session
                .models
                .iter()
                .find(|m| m.is_current)
                .map(|m| m.request_model.as_str()),
            Some("claude-opus-4-8")
        );
        submit_command(&mut widget, "/model");
        // Picker starts on the current row (Opus); move to Sonnet and confirm.
        widget.handle_key(press(KeyCode::Down));
        let outcome = widget.handle_key(press(KeyCode::Enter));
        assert!(
            matches!(outcome, ChatOutcome::SwitchModel(ref m, _) if m == "claude-sonnet-5"),
            "expected a switch to claude-sonnet-5"
        );
        // Reopen: the current marker now points at the switched-to model.
        submit_command(&mut widget, "/model");
        let current: Vec<String> = widget
            .bottom_pane()
            .view_stack()
            .active()
            .and_then(|v| {
                v.as_any()
                    .downcast_ref::<crate::bottom_pane::model_picker_view::ModelPickerView>()
            })
            .expect("picker open")
            .rows()
            .iter()
            .filter(|r| r.is_current)
            .map(|r| r.request_model.clone())
            .collect();
        assert_eq!(
            current,
            vec!["claude-sonnet-5".to_string()],
            "the `●` current marker follows the switch"
        );
    }

    #[test]
    fn set_current_model_is_provider_scoped_for_shared_wire_ids() {
        // Live-QA regression: `gpt-5.5` exists under BOTH OpenAI and GitHub
        // Copilot. Switching to Copilot's gpt-5.5 must dot ONLY the Copilot row
        // — the after-switch re-point previously matched request_model alone, so
        // both provider rows lit the `●`.
        let mut widget = ChatWidget::new(
            Vec::new(),
            SessionInfo {
                models: vec![
                    ModelRow {
                        display: "GPT-5.5".into(),
                        request_model: "gpt-5.5".into(),
                        profile: Some("openai".into()),
                        provider_label: "OpenAI".into(),
                        is_current: false,
                        supports_reasoning: true,
                    },
                    ModelRow {
                        display: "GPT-5.5".into(),
                        request_model: "gpt-5.5".into(),
                        profile: Some("github-copilot".into()),
                        provider_label: "GitHub Copilot".into(),
                        is_current: false,
                        supports_reasoning: true,
                    },
                ],
                ..Default::default()
            },
        );

        widget.set_current_model("gpt-5.5", Some("github-copilot"));
        let current: Vec<&str> = widget
            .session
            .models
            .iter()
            .filter(|m| m.is_current)
            .map(|m| m.profile.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(
            current,
            vec!["github-copilot"],
            "only the switched-to provider's gpt-5.5 row is current"
        );

        // Fallback: an unqualified switch (profile None — resolve-by-id) marks
        // by model id alone, mirroring the launch-time marker in mode.rs.
        widget.set_current_model("gpt-5.5", None);
        assert_eq!(
            widget.session.models.iter().filter(|m| m.is_current).count(),
            2,
            "profile-less switch falls back to model-only matching"
        );
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
                if m == "claude-opus-4-8" && p.as_deref() == Some("anthropic")
        ));
        assert!(!widget
            .bottom_pane()
            .view_stack()
            .contains::<ModelPickerView>());
        // The switch is echoed as a system message.
        let body = cell::<crate::history_cell::system::SystemTextCell>(&widget, 0).body();
        assert!(body.contains("Switching model to claude-opus-4-8"), "{body}");
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

    /// `/permissions` opens the interactive rule editor, and an add keystroke
    /// round-trips through the view stack to a `ChatOutcome::PermissionAction`
    /// while KEEPING the editor open (edit-in-place, like a `/web` test).
    #[test]
    fn permissions_command_opens_editor_and_add_round_trips_to_chat_outcome() {
        use crate::bottom_pane::permissions_editor_view::PermissionsEditorView;
        use crate::bottom_pane::PermissionAction;

        let mut widget = widget();
        assert!(matches!(
            submit_command(&mut widget, "/permissions"),
            ChatOutcome::Continue
        ));
        assert!(
            widget
                .bottom_pane()
                .view_stack()
                .contains::<PermissionsEditorView>(),
            "editor view opened"
        );
        // Type a rule and press Enter → PermissionAction(Add), editor stays open.
        typ(&mut widget, "Bash(npm:*)");
        let outcome = widget.handle_key(press(KeyCode::Enter));
        match outcome {
            ChatOutcome::PermissionAction(PermissionAction::Add {
                rule,
                behavior,
                dest,
            }) => {
                assert_eq!(rule, "Bash(npm:*)");
                assert_eq!(behavior, permission::PermissionBehavior::Allow);
                assert_eq!(dest, permission::PermissionUpdateDestination::LocalSettings);
            }
            _ => panic!("expected ChatOutcome::PermissionAction(Add)"),
        }
        assert!(
            widget
                .bottom_pane()
                .view_stack()
                .contains::<PermissionsEditorView>(),
            "editor stays open after an add"
        );
        // Esc closes it.
        widget.handle_key(press(KeyCode::Esc));
        assert!(widget.bottom_pane().view_stack().is_empty());
    }

    /// `/resume` opens the session picker seeded from the preloaded rows, and
    /// pressing Enter on a row round-trips to `ChatOutcome::SwitchSession(uuid)`
    /// (the loop-unwinding signal that re-mounts the chosen session).
    #[test]
    fn resume_command_opens_picker_and_enter_yields_switch_session() {
        use crate::bottom_pane::resume_picker_view::ResumePickerView;
        use crate::resume::ResumeRow;

        let want = uuid::Uuid::new_v4();
        let rows = vec![ResumeRow {
            uuid: want,
            title: "fix the parser".to_string(),
            metadata_label: "2 minutes ago \u{00b7} 4 messages".to_string(),
        }];

        let mut widget = widget();
        widget.set_resume_rows(rows);
        assert!(matches!(
            submit_command(&mut widget, "/resume"),
            ChatOutcome::Continue
        ));
        assert!(
            widget
                .bottom_pane()
                .view_stack()
                .contains::<ResumePickerView>(),
            "resume picker opened"
        );
        // Enter on the only row → SwitchSession(uuid), picker closes.
        match widget.handle_key(press(KeyCode::Enter)) {
            ChatOutcome::SwitchSession(uuid) => assert_eq!(uuid, want),
            _ => panic!("expected ChatOutcome::SwitchSession"),
        }
        assert!(
            widget.bottom_pane().view_stack().is_empty(),
            "picker closes after a pick"
        );
    }

    /// `/resume <term>` opens the picker pre-filtered by the argument.
    #[test]
    fn resume_command_with_arg_prefilters_the_picker() {
        use crate::bottom_pane::resume_picker_view::ResumePickerView;
        use crate::resume::ResumeRow;

        let mut widget = widget();
        widget.set_resume_rows(vec![
            ResumeRow {
                uuid: uuid::Uuid::new_v4(),
                title: "fix bug".to_string(),
                metadata_label: String::new(),
            },
            ResumeRow {
                uuid: uuid::Uuid::new_v4(),
                title: "add feature".to_string(),
                metadata_label: String::new(),
            },
        ]);
        submit_command(&mut widget, "/resume feature");
        let picker = widget
            .bottom_pane()
            .view_stack()
            .active()
            .and_then(|v| v.as_any().downcast_ref::<ResumePickerView>())
            .expect("resume picker active");
        assert_eq!(picker.state().filtered().len(), 1);
        assert_eq!(picker.state().filtered()[0].title, "add feature");
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
        // The just-opened empty active assistant cell renders NO tail row (no
        // stray `●` before content), so the tail height is 0 and the running
        // pane sits at the top.
        let tail_height = widget.live_tail_height(width);
        assert_eq!(tail_height, 0, "empty active cell renders no tail row");
        // `desired_height` uses the FRESHLY computed running flag (Task 5:
        // height is now running-dependent), not `bottom_pane().desired_height`
        // directly — the pane's own flag only updates inside
        // `handle_key`/`render`, and neither has run yet at this point.
        assert_eq!(
            widget.desired_height(width),
            widget.bottom_pane().desired_height_for(width, true) + tail_height
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
            rows.iter().any(|row| row.starts_with('›')),
            "composer visible:\n{}",
            rows.join("\n")
        );
    }

    #[test]
    fn spinner_shows_api_retry_status_then_clears_on_content() {
        let mut widget = widget();
        submit_command(&mut widget, "go");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        widget.apply_turn_event(TurnEvent::ApiRetry {
            message: "provider internal error".to_string(),
            attempt: 3,
            max_retries: 10,
            delay_ms: 5_000,
        });
        // The running line surfaces the error + attempt + a live countdown
        // (Claude Code's `SystemAPIErrorMessage`), not the plain verb.
        let text = widget.spinner_text();
        assert!(
            text.contains("provider internal error"),
            "shows the error: {text}"
        );
        assert!(text.contains("Retrying in"), "shows the countdown: {text}");
        assert!(text.contains("(attempt 3/10)"), "shows attempt/max: {text}");
        // Content arriving means the retried request succeeded → clear it.
        widget.apply_turn_event(TurnEvent::TextDelta("hi".to_string()));
        assert!(
            !widget.spinner_text().contains("Retrying in"),
            "retry status cleared once content streams"
        );
    }

    #[test]
    fn spinner_text_has_frame_activity_elapsed_seconds_and_interrupt_hint() {
        let mut widget = widget();
        submit_command(&mut widget, "go");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        // Default verb (Gap A): a per-turn random sample from the 187-entry
        // claude-code pool, NOT the literal "Working" — plus the animation
        // frame glyph, elapsed seconds, and the esc-to-interrupt hint
        // (claude-code status parity).
        let text = widget.spinner_text();
        let frame = text.chars().next().expect("spinner frame glyph");
        assert!("·✢✳✶✻✽".contains(frame), "unknown frame: {text}");
        // Format is "<frame> <verb>… (<dur> …)" — the timer paren shows from the
        // start (no 30s gate); the interrupt hint lives in the status row.
        let verb = text
            .split("… ")
            .next()
            .and_then(|prefix| prefix.split_once(' '))
            .map(|(_, verb)| verb)
            .unwrap_or_default();
        assert!(
            spinner::SPINNER_VERBS.contains(&verb),
            "expected a sampled pool verb, got {verb:?} in {text:?}"
        );
        assert!(text.contains("… (0s"), "spinner should carry the live timer: {text}");
        // ToolUseStart swaps the verb for the activity label.
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "Edit".to_string(),
            input: serde_json::json!({}),
        });
        assert!(widget.spinner_text().contains("Editing…"));
    }

    #[test]
    fn todowrite_in_progress_task_drives_spinner_verb_via_active_form() {
        // Gap B: a `TodoWrite` marking a task `in_progress` should show that
        // task's `activeForm` instead of the generic "Running TodoWrite"
        // tool-activity label or the per-turn random pool verb.
        let mut widget = widget();
        submit_command(&mut widget, "go");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t1"),
            tool: "TodoWrite".to_string(),
            input: serde_json::json!({
                "todos": [
                    {
                        "content": "Build the project",
                        "status": "in_progress",
                        "activeForm": "Compiling the project",
                    },
                    {
                        "content": "Write docs",
                        "status": "pending",
                        "activeForm": "Writing docs",
                    },
                ],
            }),
        });
        assert!(
            widget.spinner_text().contains("Compiling the project…"),
            "expected the in-progress todo's activeForm: {}",
            widget.spinner_text()
        );
        // A later, unrelated tool call does not override the active todo's
        // verb (claude-code's `currentTodo` outranks generic tool activity).
        widget.apply_turn_event(TurnEvent::ToolUseStart {
            id: protocol::ToolUseId::from("t2"),
            tool: "Bash".to_string(),
            input: serde_json::json!({}),
        });
        assert!(widget.spinner_text().contains("Compiling the project…"));
        // `TurnEnded` clears the per-turn todo so a later turn with no
        // `TodoWrite` doesn't keep showing the stale `activeForm`.
        widget.apply_turn_event(TurnEvent::ToolUseResult {
            id: protocol::ToolUseId::from("t2"),
            tool: "Bash".to_string(),
            result: serde_json::json!({}),
        });
        widget.apply_turn_event(TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
        widget.apply_turn_event(TurnEvent::TurnStarted);
        assert!(!widget.spinner_text().contains("Compiling the project"));
    }

    #[test]
    fn current_todo_from_todowrite_input_finds_first_active_task() {
        // No `todos` array → None.
        assert!(current_todo_from_todowrite_input(&serde_json::json!({})).is_none());
        // Empty `todos` → None.
        assert!(current_todo_from_todowrite_input(&serde_json::json!({ "todos": [] })).is_none());
        // All pending/completed → None (no active task).
        let all_done = serde_json::json!({
            "todos": [
                { "content": "a", "status": "completed" },
                { "content": "b", "status": "pending" },
            ]
        });
        assert!(current_todo_from_todowrite_input(&all_done).is_none());
        // The first non-pending/non-completed task wins, using its
        // `activeForm`/`content` verbatim.
        let active = serde_json::json!({
            "todos": [
                { "content": "a", "status": "completed" },
                { "content": "Run tests", "status": "in_progress", "activeForm": "Running tests" },
                { "content": "c", "status": "pending" },
            ]
        });
        let todo = current_todo_from_todowrite_input(&active).expect("an active todo");
        assert_eq!(todo.subject, "Run tests");
        assert_eq!(todo.active_form.as_deref(), Some("Running tests"));
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

    // ===== Fix round 1: the remaining bridge-emitted TurnEvent variants =====

    /// The rendered widget rows at `width`, sized to the desired height.
    fn rendered_rows(widget: &mut ChatWidget, width: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, widget.desired_height(width).max(4));
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        buffer_rows(&buf)
    }

    #[test]
    fn cost_updated_shows_in_the_status_row_idle_and_running() {
        let mut widget = widget();
        widget.apply_turn_event(TurnEvent::CostUpdated("$0.0123".to_string()));
        // Idle hints now live in the footer (the LAST row) and carry the dim
        // cost suffix. Width 90 (not 80): the footer's 2-column indent
        // pushes this exact hint+cost combination past 80 columns, clipping
        // the cost suffix — an unrelated width edge case, not what this test
        // checks.
        let rows = rendered_rows(&mut widget, 90);
        let footer = rows.last().expect("footer row");
        assert!(
            footer.contains("Enter: send") && footer.contains("$0.0123"),
            "idle footer row: {footer}"
        );
        // Running spinner row carries it too, and a later event replaces it.
        submit_command(&mut widget, "go");
        widget.apply_turn_event(TurnEvent::TurnStarted);
        widget.apply_turn_event(TurnEvent::CostUpdated("$0.0456".to_string()));
        let rows = rendered_rows(&mut widget, 90);
        let status = rows
            .iter()
            .find(|r| r.contains("esc to interrupt"))
            .expect("running status row");
        assert!(status.contains("$0.0456"), "running status row: {status}");
        assert!(!status.contains("$0.0123"), "stale cost replaced: {status}");
    }

    #[test]
    fn context_pressure_banner_renders_its_own_row_and_clears() {
        let mut widget = widget();
        let width = 80;
        let idle_height = widget.desired_height(width);
        widget.apply_turn_event(TurnEvent::ContextPressure {
            banner: Some(traits::ContextPressureBanner {
                text: "Context low (12% remaining)".to_string(),
                level: traits::ContextPressureLevel::Warning,
            }),
            used_fraction: 0.88,
        });
        assert_eq!(
            widget.bottom_pane().context_pressure().map(|b| b.level),
            Some(traits::ContextPressureLevel::Warning)
        );
        // The banner adds exactly one pane row. Idle has no leading status
        // row now, so the banner renders FIRST, above the composer.
        assert_eq!(widget.desired_height(width), idle_height + 1);
        let rows = rendered_rows(&mut widget, width);
        assert!(
            rows[0].contains("Context low (12% remaining)"),
            "banner row: {}",
            rows[0]
        );
        assert!(
            rows.iter().any(|r| r.starts_with('›')),
            "composer still visible:\n{}",
            rows.join("\n")
        );
        // `None` clears the banner and its row.
        widget.apply_turn_event(TurnEvent::ContextPressure {
            banner: None,
            used_fraction: 0.0,
        });
        assert!(widget.bottom_pane().context_pressure().is_none());
        assert_eq!(widget.desired_height(width), idle_height);
    }

    #[test]
    fn context_pressure_feeds_statusline_used_percentage() {
        // The numeric `used_fraction` on a ContextPressure event reaches the
        // statusline payload's `context_window.used_percentage` (× 100).
        let mut widget = widget();
        let slot = crate::status_line::new_slot(
            tui_core::status_line_command::StatusLineConfig::from_settings_value(
                &serde_json::json!({"type": "command", "command": "sl.sh"}),
            ),
        );
        widget.set_status_line(slot.clone());
        widget.apply_turn_event(TurnEvent::ContextPressure {
            banner: None,
            used_fraction: 0.375,
        });
        let (_, json) = crate::status_line::build_payload(&slot.lock().unwrap()).expect("payload");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // 0.375 fraction → 37.5% used.
        assert!(
            (v["context_window"]["used_percentage"].as_f64().unwrap() - 37.5).abs() < 1e-3,
            "used_percentage: {}",
            v["context_window"]["used_percentage"]
        );
    }

    #[test]
    fn compaction_completed_folds_one_deduped_compact_boundary() {
        use crate::history_cell::system::CompactBoundaryCell;
        let mut widget = widget();
        widget.apply_turn_event(TurnEvent::CompactionCompleted {
            messages_before: 40,
            messages_after: 8,
            bytes_saved: 1024,
        });
        assert_eq!(cells(&widget).len(), 1);
        let _ = cell::<CompactBoundaryCell>(&widget, 0);
        // A duplicate report of the same compaction de-dupes to ONE marker
        // (the old backend's `push_compact_boundary` contract).
        widget.apply_turn_event(TurnEvent::CompactionCompleted {
            messages_before: 40,
            messages_after: 8,
            bytes_saved: 1024,
        });
        assert_eq!(cells(&widget).len(), 1, "consecutive boundaries de-dupe");
        // A NON-boundary message in between makes the next boundary render.
        widget.transcript.push_message(RenderedMessage::SystemText {
            body: "between".to_string(),
            timestamp: 0,
            is_error: false,
        });
        widget.apply_turn_event(TurnEvent::CompactionCompleted {
            messages_before: 12,
            messages_after: 4,
            bytes_saved: 2048,
        });
        assert_eq!(cells(&widget).len(), 3);
        let _ = cell::<CompactBoundaryCell>(&widget, 2);
    }

    /// A rejected five-hour rate-limit header snapshot.
    fn rate_limit_rejected() -> TurnEvent {
        TurnEvent::RateLimit {
            status: Some("rejected".to_string()),
            rate_limit_type: Some("five_hour".to_string()),
            utilization: None,
            resets_at: None,
            claim_resets_at: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            fallback_available: None,
        }
    }

    #[test]
    fn rate_limit_composes_a_notice_and_dedupes_identical_text() {
        use crate::history_cell::system::RateLimitCell;
        let mut widget = widget();
        widget.apply_turn_event(rate_limit_rejected());
        assert_eq!(cells(&widget).len(), 1);
        let notice = cell::<RateLimitCell>(&widget, 0);
        assert_eq!(notice.text(), "You've hit your session limit");
        assert_eq!(notice.upsell(), None, "unknown subscription → no upsell");
        // An identical snapshot composes the same text → suppressed.
        widget.apply_turn_event(rate_limit_rejected());
        assert_eq!(cells(&widget).len(), 1, "identical notices never stack");
        // `/clear` resets the dedupe slot with the scrollback: the SAME
        // notice can reappear in the now-empty transcript.
        submit_command(&mut widget, "/clear");
        widget.apply_turn_event(rate_limit_rejected());
        assert_eq!(cells(&widget).len(), 1);
        assert_eq!(
            cell::<RateLimitCell>(&widget, 0).text(),
            "You've hit your session limit"
        );
    }

    #[test]
    fn rate_limit_overage_transition_notice_fires_once_and_rearms_on_leaving() {
        use crate::history_cell::system::RateLimitCell;
        let overage = || TurnEvent::RateLimit {
            status: Some("rejected".to_string()),
            rate_limit_type: Some("five_hour".to_string()),
            utilization: None,
            resets_at: None,
            claim_resets_at: None,
            overage_status: Some("allowed".to_string()),
            overage_resets_at: None,
            overage_disabled_reason: None,
            fallback_available: None,
        };
        let mut widget = widget();
        // Entering overage: no composed notice (isUsingOverage + allowed →
        // null), but the ONE-SHOT transition notice fires.
        widget.apply_turn_event(overage());
        assert_eq!(cells(&widget).len(), 1);
        assert_eq!(
            cell::<RateLimitCell>(&widget, 0).text(),
            "You're now using extra usage"
        );
        assert!(widget.has_shown_overage_notification);
        // Staying in overage: the flag suppresses a repeat.
        widget.apply_turn_event(overage());
        assert_eq!(cells(&widget).len(), 1, "one-shot while in overage");
        // The flag SURVIVES /clear (TS component state) — no repeat after it.
        submit_command(&mut widget, "/clear");
        widget.apply_turn_event(overage());
        assert!(
            widget.has_shown_overage_notification,
            "flag survives /clear"
        );
        assert!(cells(&widget).is_empty(), "no repeat notice after /clear");
        // Leaving overage resets the flag; re-entering fires again.
        widget.apply_turn_event(TurnEvent::RateLimit {
            status: Some("allowed".to_string()),
            rate_limit_type: None,
            utilization: None,
            resets_at: None,
            claim_resets_at: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            fallback_available: None,
        });
        assert!(!widget.has_shown_overage_notification);
        widget.apply_turn_event(overage());
        assert_eq!(cells(&widget).len(), 1);
        assert_eq!(
            cell::<RateLimitCell>(&widget, 0).text(),
            "You're now using extra usage"
        );
    }

    #[test]
    fn rate_limit_reads_the_wired_subscription_slot_at_compose_time() {
        use crate::history_cell::system::RateLimitCell;
        let mut widget = widget();
        // A live composition-root slot, filled AFTER wiring (the background
        // fetch landing) — the composer reads it at compose time.
        let slot: traits::subscription::SharedSubscription =
            std::sync::Arc::new(std::sync::RwLock::new(None));
        widget.set_subscription(std::sync::Arc::clone(&slot));
        *slot.write().unwrap() = Some(traits::subscription::SubscriptionSnapshot {
            is_subscriber: true,
            subscription_type: Some("pro".to_string()),
            billing_type: Some("stripe_subscription".to_string()),
            ..Default::default()
        });
        widget.apply_turn_event(rate_limit_rejected());
        let notice = cell::<RateLimitCell>(&widget, 0);
        assert_eq!(notice.text(), "You've hit your session limit");
        assert!(
            notice.upsell().is_some(),
            "subscriber snapshot → subscription-granular error upsell"
        );
    }

    #[test]
    fn terminal_sequences_stage_in_order_and_drain_once() {
        let mut widget = widget();
        assert!(widget.take_terminal_sequences().is_empty());
        widget.apply_turn_event(TurnEvent::TerminalSequence {
            seq: "\u{1b}]0;title\u{7}".to_string(),
        });
        widget.apply_turn_event(TurnEvent::TerminalSequence {
            seq: "\u{1b}]9;notify\u{7}".to_string(),
        });
        assert_eq!(
            widget.take_terminal_sequences(),
            vec![
                "\u{1b}]0;title\u{7}".to_string(),
                "\u{1b}]9;notify\u{7}".to_string()
            ],
            "FIFO order"
        );
        assert!(
            widget.take_terminal_sequences().is_empty(),
            "drain consumes the stage"
        );
        // Staging never touches the transcript.
        assert!(widget.transcript().is_empty());
    }

    #[test]
    fn raw_utilization_and_reserved_permission_variant_are_documented_noops() {
        // RawUtilization's only consumer is the configured statusline
        // *command* input — a surface this backend does not have; the
        // reserved PermissionRequest variant is never emitted by the bridge
        // (live prompts arrive via the permission channel). Both fold without
        // any visible or turn-state change.
        let mut widget = widget();
        widget.apply_turn_event(TurnEvent::RawUtilization {
            five_hour_utilization: Some(0.5),
            five_hour_resets_at: Some(1),
            seven_day_utilization: Some(0.2),
            seven_day_resets_at: Some(2),
        });
        widget.apply_turn_event(TurnEvent::PermissionRequest {
            tool: "Bash".to_string(),
            input: serde_json::json!({}),
        });
        assert!(widget.transcript().is_empty());
        assert!(!widget.turn_running());
        assert!(!widget.has_open_permission());
        assert!(widget.take_terminal_sequences().is_empty());
    }
}
