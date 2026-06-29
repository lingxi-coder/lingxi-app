//! Root iocraft component + key-action dispatcher.
//!
//! M6-01 shipped only a static placeholder view + a binary `should_quit`
//! flag. M6-02 keeps that placeholder intact (still consumed by the
//! `render_placeholder` snapshot test + the iocraft-prototype gate)
//! and layers two new responsibilities on top:
//!
//! 1. `pub fn dispatch(KeyAction, &mut AppState) -> bool` — the pure
//!    state-transition for a single user keystroke. Heavy I/O (slash
//!    dispatch, `run_turn`) is handled by `app::handle_submit_line` and
//!    `app::run_one_submit` which call `dispatch` first and then act on
//!    the returned `should_run_turn` flag.
//! 2. `pub fn scroll_with_viewport(&mut AppState, ScrollDir, vh)` — the
//!    scroll-offset math, which needs the live viewport height (passed
//!    by the per-frame loop) and is therefore separated from `dispatch`.
//!
//! M6-02 does not yet mount `ReplScreen` in the event loop; that wiring
//! lands in Task 14. The placeholder root is still what `run_tui_session`
//! renders on startup.

use std::time::Instant;

use iocraft::prelude::*;

use crate::components::prompt_input::{
    apply_backspace, apply_insert, apply_move, apply_newline, CursorMove as PiCursor,
};
use crate::events::keymap::{CursorMove, KeyAction, ScrollDir};
use crate::state::{AppState, RenderedMessage};

/// (RRS-08) Double-press re-arm window (Ctrl-C exit confirm, and Ctrl-D —
/// RRS-07 — which shares this same window): the second press confirms only
/// when the first was within this many milliseconds. claude-code
/// `hooks/useDoublePress.ts`'s default.
pub const SIGINT_WINDOW_MS: u64 = 800;

/// Crate version string surfaced to the placeholder line.
///
/// Sourced from the lingxi-tui crate's own `CARGO_PKG_VERSION` so version
/// bumps automatically propagate. M7-16 bumps the crate to 0.8.0, at which
/// point the rendered placeholder reads `"lingxi-tui v0.8.0"`.
fn version_line() -> String {
    format!("lingxi-tui v{}", env!("CARGO_PKG_VERSION"))
}

/// Top-level TUI application state. M6-01 keeps this minimal: no fields
/// are needed to render the placeholder. M6-02 grows it into
/// `{ messages, prompt_text, streaming, ... }`.
#[derive(Debug, Default, Clone)]
pub struct TuiApp {
    /// Whether the user has requested quit. Wired by the event loop in
    /// `run_tui_session` once Ctrl-C / Ctrl-D classifies as `KeyClass::Quit`.
    pub should_quit: bool,
}

impl TuiApp {
    /// Construct a fresh, idle app state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the app as wanting to quit. Called by the event loop on
    /// `KeyClass::Quit` or when the cancel token trips.
    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Render the root frame. Returns an iocraft element tree owned for
    /// `'static`, which the session loop hands to iocraft's render driver.
    #[must_use]
    pub fn render(&self) -> AnyElement<'static> {
        let line = version_line();
        element! {
            View(padding: 1, flex_direction: FlexDirection::Column) {
                Text(content: line)
            }
        }
        .into_any()
    }
}

/// Process one `KeyAction` against the live `AppState`.
///
/// Returns `true` iff the action was a `Submit` that the caller should
/// follow up with a real orchestrator round-trip (via
/// [`run_one_submit`]). All other branches return `false`.
///
/// The function is pure with respect to I/O — it only mutates `st`.
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub fn dispatch(action: KeyAction, st: &mut AppState) -> bool {
    match action {
        KeyAction::InsertChar(c) => {
            let (t, cur) = apply_insert(&st.prompt_text, st.prompt_cursor, c);
            st.prompt_text = t;
            st.prompt_cursor = cur;
            false
        }
        KeyAction::Backspace => {
            let (t, cur) = apply_backspace(&st.prompt_text, st.prompt_cursor);
            st.prompt_text = t;
            st.prompt_cursor = cur;
            false
        }
        KeyAction::InsertNewline => {
            // Backslash-return fallback: if the char immediately before the
            // cursor is a lone '\', strip it before inserting the newline so
            // the literal '\' the user typed doesn't linger.
            if st.prompt_cursor > 0 && st.prompt_text[..st.prompt_cursor].ends_with('\\') {
                let (t, cur) = apply_backspace(&st.prompt_text, st.prompt_cursor);
                st.prompt_text = t;
                st.prompt_cursor = cur;
            }
            let (t, cur) = apply_newline(&st.prompt_text, st.prompt_cursor);
            st.prompt_text = t;
            st.prompt_cursor = cur;
            false
        }
        KeyAction::MoveCursor(m) => {
            let pi = match m {
                CursorMove::Left => PiCursor::Left,
                CursorMove::Right => PiCursor::Right,
                CursorMove::Home => PiCursor::Home,
                CursorMove::End => PiCursor::End,
            };
            st.prompt_cursor = apply_move(&st.prompt_text, st.prompt_cursor, pi);
            false
        }
        KeyAction::MoveCursorVertical(delta) => {
            st.prompt_cursor = crate::components::prompt_input::apply_move_vertical(
                &st.prompt_text,
                st.prompt_cursor,
                i32::from(delta),
            );
            false
        }
        KeyAction::Submit => {
            if st.prompt_text.is_empty() {
                return false;
            }
            // (M7-11) `/doctor` opens the Doctor screen instead of echoing /
            // running a turn. Intercept here (the live submit path) the same
            // way `handle_submit_line` intercepts `/clear` / `/exit`. The
            // stdio `--no-tui` `/doctor` text report is unchanged.
            if st.prompt_text.trim() == "/doctor" {
                let diag = crate::screens::doctor::DoctorDiagnostics::capture(
                    &st.status.cwd,
                    st.status.mcp_configured,
                    st.status.mcp_connected,
                    st.status.term_size,
                );
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.open_doctor(diag);
                return false;
            }
            // (M7-14) `/memory` opens the in-TUI Memory editor screen instead of
            // echoing / running a turn or shelling out to $EDITOR. Like
            // `/doctor` the open is fully synchronous (the tier list is resolved
            // each frame from `hierarchy::walk` — no handle, no `.await`), so we
            // open the screen directly here on the live submit path. The
            // `crates/commands` `MemoryHandler` stays the `--no-tui` $EDITOR path.
            if st.prompt_text.trim() == "/memory" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.open_memory();
                return false;
            }
            // (M7-15) `/theme` opens the in-TUI theme picker screen instead of
            // echoing / running a turn. Like `/doctor` / `/memory` the open is
            // fully synchronous: the theme registry is static (no handle, no
            // `.await`), so the picker opens directly here on the live submit
            // path, focused on the currently-active setting. The `crates/commands`
            // theme handler stays the `--no-tui` path. No echo, no turn.
            if st.prompt_text.trim() == "/theme" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.open_theme_picker();
                return false;
            }
            // (M7-14 review) `/export` opens the export flow DIRECTLY (the
            // claude-code ExportDialog: a filename prompt, not a search box).
            // `open_export` pre-fills the editable filename with
            // `default_export_filename()`; the live key path (priority-3
            // overlay branch in `root::handle_live_key`) drives the flow and
            // runs `export_transcript` on confirm (§4 R10 overwrite-confirm).
            // Ctrl-T still opens the search/jump overlay. This replaces the
            // M5-11 `/export` unimplemented stub on the TUI surface (no echo,
            // no turn). Registers NO telemetry event (M7-16 audit).
            if st.prompt_text.trim() == "/export" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.message_selector.open_export();
                return false;
            }
            // (M7-13 review) `/config` / `/status` open the Settings screen on
            // the matching tab. Unlike `/doctor` (whose diagnostics capture is
            // synchronous + handle-free), the Settings open needs an async
            // `SettingsData::snapshot(handle, eff)` read — which the sync
            // `dispatch` seam can't `.await`. So we mirror the keybinding: RAISE
            // `pending_open_settings`; the async open pump in `root.rs` builds
            // the snapshot + opens the screen. No echo, no turn. The M5-11
            // `/config` / `/status` handlers stay the `--no-tui` path, untouched.
            {
                use crate::screens::settings::SettingsTab;
                let open_tab = match st.prompt_text.trim() {
                    "/config" => Some(SettingsTab::Config),
                    "/status" => Some(SettingsTab::Status),
                    _ => None,
                };
                if let Some(tab) = open_tab {
                    st.prompt_text.clear();
                    st.prompt_cursor = 0;
                    st.pending_open_settings = Some(tab);
                    return false;
                }
            }
            // (M9-08) `/agents` opens the agent-discovery screen. Like
            // `/config`/`/status`, the open needs an async
            // `OrchestratorHandle::list_agents()` call — which the sync `dispatch`
            // seam can't `.await`. So we RAISE `pending_open_agents`; the async
            // open pump in `root.rs` (`pump_open_agents`) fetches the catalog and
            // opens the screen. No echo, no turn.
            if st.prompt_text.trim() == "/agents" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.pending_open_agents = true;
                return false;
            }
            // `/mcp` opens the read-only MCP-server viewer. Like `/agents`, the
            // open needs an async `OrchestratorHandle::list_mcp_servers()` call
            // the sync `dispatch` seam can't `.await`, so we RAISE
            // `pending_open_mcp`; `root::pump_open_mcp` fetches the servers and
            // opens the screen. The `crates/commands` mcp handler stays the
            // `--no-tui` text path.
            if st.prompt_text.trim() == "/mcp" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.pending_open_mcp = true;
                return false;
            }
            // `/hooks` opens the read-only hooks viewer. Same recipe as `/mcp`,
            // backed by `OrchestratorHandle::list_hooks()` via
            // `root::pump_open_hooks`.
            if st.prompt_text.trim() == "/hooks" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.pending_open_hooks = true;
                return false;
            }
            // `/permissions` opens the read-only permissions viewer. OFF-DISK
            // like `/skills` (no handle): `root::pump_open_permissions` reads the
            // settings tiers on the blocking pool and opens the screen.
            if st.prompt_text.trim() == "/permissions" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.pending_open_permissions = true;
                return false;
            }
            // `/model` (no arg) opens the model picker. Like `/agents`, the open
            // needs an async `OrchestratorHandle::list_available_models()` call,
            // so we RAISE `pending_open_model`; `root::pump_open_model` fetches
            // the models + current and opens the screen. (The `/model <arg>`
            // form falls through to the `crates/commands` text handler.)
            if st.prompt_text.trim() == "/model" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.pending_open_model = true;
                return false;
            }
            // (Plan 3c §6.4) `/connect <provider>` opens the interactive
            // credential screen. Like `/model`, opening needs an async step (the
            // device-flow / keychain), so we RAISE `pending_connect`;
            // `root::pump_open_connect` opens the screen on the next tick.
            {
                let provider = st
                    .prompt_text
                    .trim()
                    .strip_prefix("/connect ")
                    .map(|rest| rest.trim().to_string())
                    .filter(|p| !p.is_empty());
                if let Some(provider) = provider {
                    st.prompt_text.clear();
                    st.prompt_cursor = 0;
                    st.pending_connect = Some(provider);
                    return false;
                }
            }
            // A bare `/connect` (no provider arg) opens the grouped provider
            // PICKER (an opencode-style searchable list, modeled on `/model`).
            // The catalog is STATIC (no async fetch), so we open the screen
            // synchronously here; selecting a row raises `pending_connect` →
            // `root::pump_open_connect` opens the key-entry `/connect` screen.
            if st.prompt_text.trim() == "/connect" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                let picker = crate::screens::connect_picker::ConnectPickerState::from_connectable(
                    &st.provider_auth_methods,
                    &st.provider_availability,
                );
                st.open_connect_picker(picker);
                return false;
            }
            // `/vim` toggles the editor's vim keybindings (claude-code
            // `commands/vim/vim.ts`). An IMMEDIATE command (not a screen): flip
            // the existing `vim_enabled` — mirroring the Ctrl-Alt-V `ToggleVim`
            // keybinding (reset the `VimState` to Insert on enable) — then echo
            // claude-code's exact mode message. Session-only: the existing toggle
            // does not persist `editorMode` to settings.json either (that persist
            // is a deferred follow-up).
            if st.prompt_text.trim() == "/vim" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.vim_enabled = !st.vim_enabled;
                let body = if st.vim_enabled {
                    st.vim = crate::components::prompt_input::VimState::default();
                    "Editor mode set to vim. Use Escape key to toggle between INSERT and NORMAL modes."
                        .to_string()
                } else {
                    "Editor mode set to normal. Using standard (readline) keyboard bindings."
                        .to_string()
                };
                st.push_message(RenderedMessage::SystemText {
                    body,
                    timestamp: chrono::Utc::now().timestamp(),
                    is_error: false,
                });
                return false;
            }
            // (M9-09) `/skills` opens the read-only skill-registry viewer. Like
            // `/agents`/`/stats`, the open needs async work the sync `dispatch`
            // seam can't `.await`: an on-disk `.lingxi/skills/` dir walk (the
            // project ancestors up to the git root + the user home), reading +
            // parsing each `SKILL.md`. The frozen `OrchestratorHandle` exposes
            // no `list_skills`, so the TUI reads the dirs itself. We RAISE
            // `pending_open_skills`; the async open pump in `root.rs`
            // (`pump_open_skills`) walks the dirs OUTSIDE the `AppState` lock,
            // builds the grouped sections, and opens the screen. When no skill
            // exists on disk the sections are empty → the locked `No skills
            // found` empty state. No echo, no turn. The `crates/commands` skills
            // handler stays the `--no-tui` path.
            if st.prompt_text.trim() == "/skills" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.pending_open_skills = true;
                return false;
            }
            // (M9-10) `/stats` opens the usage-stats screen. Like
            // `/config`/`/agents`, the open needs async work the sync `dispatch`
            // seam can't `.await`: a multi-project `*.jsonl` fs walk +
            // aggregation (slow over many files). So we RAISE
            // `pending_open_stats`; the async open pump in `root.rs`
            // (`pump_open_stats`) walks `<lingxi_home>/projects/` OUTSIDE the
            // `AppState` lock, aggregates, and opens the screen. No echo, no
            // turn. The `crates/commands` stats handler stays the `--no-tui`
            // path, untouched.
            if st.prompt_text.trim() == "/stats" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.pending_open_stats = true;
                return false;
            }
            // `/tasks` opens the background-tasks dialog (claude-code
            // `commands/tasks/tasks.tsx` → `<BackgroundTasksDialog>`). Unlike
            // `/agents`/`/stats`, the open is SYNCHRONOUS — no fs walk or
            // `OrchestratorHandle` call is needed: the dialog browses the live
            // `AppState.multiagent.tasks` list. So we open the screen inline
            // here, mirroring the Shift+Down open binding in `root.rs` (seed a
            // fresh `BackgroundTasksState`, emit the same `screen_opened`
            // telemetry). No echo, no turn. The `crates/commands` tasks handler
            // stays the `--no-tui` text path, untouched.
            if st.prompt_text.trim() == "/tasks" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.active_screen = Some(crate::screens::Screen::BackgroundTasks(
                    crate::screens::background_tasks::BackgroundTasksState::default(),
                ));
                crate::telemetry::screen_opened("background_tasks");
                return false;
            }
            // `/help` opens the read-only keyboard-shortcuts + slash-command
            // viewer (claude-code `HelpV2`). Like `/tasks`, the open is fully
            // SYNCHRONOUS — the content is a static shortcuts/command table (no
            // fs walk, no `OrchestratorHandle` call), so we open the screen
            // inline here (seed a fresh `HelpState`, emit the same
            // `screen_opened` telemetry). No echo, no turn. The `crates/commands`
            // `HelpHandler` stays the `--no-tui` text path, untouched.
            if st.prompt_text.trim() == "/help" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.active_screen = Some(crate::screens::Screen::Help(
                    crate::screens::help::HelpState::new(),
                ));
                crate::telemetry::screen_opened("help");
                return false;
            }
            // (`/clear`/`/exit`/`/quit`/`/compact`) The four immediate local
            // commands claude-code `handlePromptSubmit` (~229) executes inline on
            // a leading-slash submit: each runs its action, clears the prompt
            // buffer, and returns WITHOUT queuing a turn — exactly the stdio
            // `handle_submit_line` "clear"/"exit"/"quit" branches, lifted onto the
            // live submit path. We `.trim()` the bare name in every match because
            // the palette Accept path rewrites the buffer to e.g. "/clear " (with
            // a trailing space) before the second Enter reaches `dispatch`.
            //
            // `/clear` wipes the scrollback + resets the scroll offset (the exact
            // fields the stdio "clear" branch around `handle_submit_line` uses).
            // `/exit` / `/quit` (the claude-code `commands/exit` alias) flip
            // `should_exit` — the same flag the Ctrl-C/Ctrl-D quit path + the
            // stdio "exit"/"quit" branch use. `/compact` is the lone ASYNC one:
            // `force_compact` needs the `OrchestratorHandle` the sync seam can't
            // `.await`, so we RAISE `pending_compact`; `root::pump_compact` runs
            // it OUTSIDE the lock. The `crates/commands` clear/exit/compact
            // handlers stay the `--no-tui` text path, untouched. No echo, no turn.
            if st.prompt_text.trim() == "/clear" {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.messages.clear();
                st.scroll_offset = 0;
                // The rate-limit dedupe slot must reset with the scrollback: a
                // suppressed identical notice would otherwise never reappear in
                // the now-empty transcript.
                // (`has_shown_overage_notification` deliberately NOT reset: the
                // TS flag is component state, surviving transcript clears.)
                st.last_rate_limit_text = None;
                return false;
            }
            if matches!(st.prompt_text.trim(), "/exit" | "/quit") {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.should_exit = true;
                return false;
            }
            if st.prompt_text.trim() == "/compact" {
                // The `/compact <instructions>` arg form is OUT OF SCOPE here: the
                // frozen `OrchestratorHandle::force_compact()` takes no
                // instructions arg, so only the bare `/compact` is intercepted; an
                // arg form falls through to the registry text handler.
                st.prompt_text.clear();
                st.prompt_cursor = 0;
                st.pending_compact = true;
                return false;
            }
            // (`/color`) Set the prompt-bar agent color for this session. An
            // IMMEDIATE arg command (claude-code `immediate: true`, NOT a
            // screen): parse the arg, push the `system` display, set the
            // session-color field for immediate effect, and RAISE
            // `pending_save_color` so the async pump in `root.rs`
            // (`pump_save_color`) persists the choice to the transcript
            // (claude-code `saveAgentColor`) OUTSIDE the lock. No echo, no turn.
            {
                let trimmed = st.prompt_text.trim();
                let is_color =
                    trimmed == "/color" || trimmed.split_whitespace().next() == Some("/color");
                if is_color {
                    // Own the args before the `&mut st` borrow in
                    // `apply_color_command` (which clears `prompt_text`).
                    let args = trimmed.strip_prefix("/color").unwrap_or("").to_string();
                    apply_color_command(st, &args);
                    st.prompt_text.clear();
                    st.prompt_cursor = 0;
                    return false;
                }
            }
            // (`/copy [N]`) Copy the most recent assistant text (or the Nth) to
            // the system clipboard (claude-code `commands/copy/copy.tsx`). A
            // `local-jsx` command — NOT a screen for the no-arg/`N` fast path
            // (the marked-based code-block selector dialog is deferred, bucket
            // (b)). Parse the trailing integer, run the byte-locked
            // `collect_recent_assistant_texts` selection over the live
            // transcript, push the confirmation/error `SystemText`
            // SYNCHRONOUSLY, and on success RAISE `pending_copy_clipboard` so
            // the async pump in `root.rs` (`pump_copy_clipboard`) writes to the
            // clipboard OUTSIDE the lock + the render frame (the iocraft
            // reconciler owns stdout). No echo, no turn. The `crates/commands`
            // copy name stays the `--no-tui` text path, untouched.
            {
                let trimmed = st.prompt_text.trim();
                let is_copy =
                    trimmed == "/copy" || trimmed.split_whitespace().next() == Some("/copy");
                if is_copy {
                    // Own the args before the `&mut st` borrow in
                    // `apply_copy_command` (which clears `prompt_text`).
                    let args = trimmed.strip_prefix("/copy").unwrap_or("").to_string();
                    apply_copy_command(st, &args);
                    st.prompt_text.clear();
                    st.prompt_cursor = 0;
                    return false;
                }
            }
            let line = std::mem::take(&mut st.prompt_text);
            st.prompt_cursor = 0;
            st.history.push(line.clone());
            st.history_cursor = None;
            // (MULTIMODAL.1) RAISE the turn-spawn request. The sync dispatcher
            // can't spawn the streaming turn itself (it has no `OrchestratorHandle`
            // / bridge sender), so it records the line; the ticker `use_future`'s
            // `root::pump_turn` observes the flag, drains any pasted image paths
            // (`st.paste` stays untouched here so they reach that turn), and calls
            // `spawn_streaming_turn`.
            //
            // The FIXED LIST of screen-launch / display builtins above
            // (`/doctor`, `/memory`, `/theme`, `/export`, `/config`, `/status`,
            // `/copy`, `/agents`, `/mcp`, `/hooks`, `/permissions`, `/model`,
            // `/vim`, `/skills`, `/stats`, `/tasks`, `/help`, `/clear`,
            // `/compact`, `/exit`) intercept before this point. ANY OTHER slash
            // input — prompt-commands like `/loop` and Markdown/Plugin commands,
            // or an unknown command — falls through here.
            //
            // The sync dispatcher holds no slash `dispatcher` (it can't `.await`
            // `dispatch()`), so a slash command is raised as `pending_slash` and
            // the async `root::pump_slash` consults the dispatcher: `RunAsTurn`
            // runs the expanded prompt as a turn, `Handled`/`Unknown` surface
            // their text. Plain (non-slash) text is raised as `pending_turn` and
            // run verbatim by `pump_turn`. Both echo the typed line as `UserText`.
            // (Matches how the CLI/bridge/mobile surfaces route a typed slash.)
            // (`!` bash mode) A `!`-prefixed line is NOT sent to the model: it
            // RUNS the command through the host's sandboxed Bash executor (the
            // SAME `BashTool` the model uses) and renders its output inline, with
            // no LLM turn — 1:1 with claude-code's bash mode. The sync dispatcher
            // can't `.await` that executor, so it echoes the command as a
            // `UserBashInput` row and RAISES `pending_bash`; the async
            // `root::pump_bash` observes the flag, runs it through the wired
            // `BashRunner`, and folds the captured stdout/stderr into a
            // `UserBashOutput` row. A BARE `!` (nothing after the prefix) is just
            // the composer's bash-mode marker, not a command — it falls through to
            // the normal prompt/slash routing below.
            if let Some(rest) = line.strip_prefix('!') {
                let command = rest.trim_start();
                if !command.is_empty() {
                    let command = command.to_string();
                    st.push_message(RenderedMessage::UserBashInput {
                        command: command.clone(),
                    });
                    st.pending_bash = Some(command);
                    return true;
                }
            }
            if line.starts_with('/') {
                st.pending_slash = Some(line.clone());
            } else {
                st.pending_turn = Some(line.clone());
            }
            st.push_message(RenderedMessage::UserText {
                body: line,
                timestamp: chrono::Utc::now().timestamp(),
            });
            true
        }
        KeyAction::Cancel => {
            if st.in_flight_turn.is_some() {
                if let Some(tif) = &st.in_flight_turn {
                    tif.cancel.cancel();
                }
                // (RRS-08) claude-code's INTERRUPT_MESSAGE — a UserText body,
                // not a SystemText line; UserTextMessage special-cases it to
                // render the InterruptedByUser line. Mirrors the Esc-interrupt
                // branch (root.rs).
                st.push_message(RenderedMessage::UserText {
                    body: crate::components::messages::user_tool_result::INTERRUPT_MESSAGE
                        .to_string(),
                    timestamp: chrono::Utc::now().timestamp(),
                });
            } else if !st.prompt_text.is_empty() {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
            } else {
                match st.sigint_armed_at {
                    Some(t) if t.elapsed().as_millis() < u128::from(SIGINT_WINDOW_MS) => {
                        st.should_exit = true;
                    }
                    _ => {
                        // (RRS-08) claude-code renders this as a transient
                        // footer hint ("Press {key} again to exit"), not a
                        // scrollback line — see PromptInputFooter's
                        // sigint_armed_at-driven exit_hint.
                        st.sigint_armed_at = Some(Instant::now());
                        st.sigint_armed_key = "Ctrl-C";
                    }
                }
            }
            false
        }
        KeyAction::HistoryStep(delta) => {
            if st.history.is_empty() {
                return false;
            }
            #[allow(clippy::match_same_arms)]
            let new_cursor: Option<usize> = match (st.history_cursor, delta) {
                (None, -1) => Some(st.history.len() - 1),
                (None, 1) => None,
                (Some(0), -1) => Some(0),
                (Some(i), -1) => Some(i - 1),
                (Some(i), 1) if i + 1 < st.history.len() => Some(i + 1),
                (Some(_), 1) => None,
                _ => st.history_cursor,
            };
            st.history_cursor = new_cursor;
            st.prompt_text = match new_cursor {
                Some(i) => st.history[i].clone(),
                None => String::new(),
            };
            st.prompt_cursor = st.prompt_text.len();
            false
        }
        KeyAction::ScrollStep(dir) => {
            // Default unit step uses height=1 for j/k. PageUp/Down delegate
            // to `scroll_with_viewport` from the per-frame loop where the
            // real viewport height is known.
            scroll_with_viewport(st, dir, 1);
            false
        }
        // M6-04 T10: focus walking + expand toggle.
        KeyAction::FocusToolStep(delta) => {
            if delta < 0 {
                st.focus_prev_tool();
            } else {
                st.focus_next_tool();
            }
            false
        }
        KeyAction::ToggleExpanded => {
            if let Some(id) = st.focused_tool_id.clone() {
                st.toggle_expanded(&id);
            }
            false
        }
        KeyAction::ToggleVim => {
            st.vim_enabled = !st.vim_enabled;
            if st.vim_enabled {
                // Entering vim starts in Insert (claude-code createInitialVimState).
                st.vim = crate::components::prompt_input::VimState::default();
            }
            false
        }
        // (M7-13 review) The sync key path can't `.await SettingsData::snapshot`,
        // so it only RAISES the open request. The async open pump in `root.rs`
        // (the ticker `use_future`) observes `pending_open_settings`, reads the
        // snapshot via the handle, and opens the screen. We do NOT open here.
        KeyAction::OpenSettings(tab) => {
            st.pending_open_settings = Some(tab);
            false
        }
    }
}

/// Apply a parsed `/color` command to `AppState`: push the `system` display
/// message and apply the session-color + persistence effects. PURE w.r.t. I/O —
/// the actual disk write is deferred to `root::pump_save_color` via the
/// `pending_save_color` flag this raises. 1:1 with claude-code `color.ts`'s
/// `onDone` + `setAppState` + `saveAgentColor` triple.
///
/// `args` is the text AFTER the `/color` command word (may be empty).
fn apply_color_command(st: &mut AppState, args: &str) {
    use crate::commands::color::{parse_color_command, ColorCommand, DEFAULT_SENTINEL};
    let (display, is_error) = match parse_color_command(args) {
        ColorCommand::List { display } => (display, false),
        ColorCommand::Reset { display } => {
            // Clear the immediate session color + persist the `"default"`
            // sentinel (NOT empty) so resume re-applies the reset.
            st.session_agent_color = None;
            st.pending_save_color = Some(DEFAULT_SENTINEL.to_string());
            (display, false)
        }
        ColorCommand::Set { name, display } => {
            // Set the immediate session color + persist the name.
            st.session_agent_color = Some(name.clone());
            st.pending_save_color = Some(name);
            (display, false)
        }
        ColorCommand::Invalid { display } => (display, true),
    };
    st.push_message(RenderedMessage::SystemText {
        body: display,
        timestamp: chrono::Utc::now().timestamp(),
        is_error,
    });
}

/// Apply a parsed `/copy [N]` command to `AppState`: select the assistant text
/// over the live transcript, push the confirmation/error `SystemText`, and on
/// success raise `pending_copy_clipboard` so the async pump in `root.rs`
/// (`pump_copy_clipboard`) writes it to the system clipboard OUTSIDE the lock
/// (the iocraft reconciler owns stdout). PURE w.r.t. I/O — no clipboard write
/// happens here. 1:1 with the no-arg/`N` fast path of claude-code
/// `commands/copy/copy.tsx`'s `call` (`onDone` + `setClipboard`).
///
/// `args` is the text AFTER the `/copy` command word (may be empty).
fn apply_copy_command(st: &mut AppState, args: &str) {
    use crate::commands::copy::{parse_copy_command, CopyCommand};
    match parse_copy_command(&st.messages, args) {
        CopyCommand::Copy { text, display } => {
            // Raise the clipboard write for the async pump; show the
            // confirmation immediately (clipboard writes are best-effort,
            // exactly as claude-code's OSC-52 path is fire-and-forget).
            st.pending_copy_clipboard = Some(text);
            st.push_message(RenderedMessage::SystemText {
                body: display,
                timestamp: chrono::Utc::now().timestamp(),
                is_error: false,
            });
        }
        CopyCommand::Error { display } => {
            st.push_message(RenderedMessage::SystemText {
                body: display,
                timestamp: chrono::Utc::now().timestamp(),
                is_error: true,
            });
        }
    }
}

/// Process a submitted line. If `/`-prefixed → slash dispatch (with TUI
/// intercept of `/clear` and `/exit`). Otherwise the caller is expected
/// to route the line into the orchestrator's `run_turn`. Returns `true`
/// iff a turn should run.
///
/// ### Deviation from plan
///
/// The plan assumed `RegistrySlashDispatcher::dispatch` would return a
/// rich `SlashOutcome::{Cleared, Exit, Message, ...}` variant. The
/// actual M5-09 dispatcher returns `SlashDispatchResult::{Handled |
/// Unknown | NotASlashCommand}` where `/clear` and `/exit` are stubs
/// that resolve to `Handled { display: "<name>: not implemented" }`.
/// We therefore intercept `/clear` and `/exit` *before* the dispatcher
/// for the M6-02 contract, and let everything else fall through.
/// Outcome of [`handle_submit_line`]: whether the caller should run a turn, and
/// with what prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitDisposition {
    /// Fully handled (slash intercept / display-only command) — no turn runs.
    Handled,
    /// Run a turn with the typed line as-is (plain text). The caller applies its
    /// normal paste-pill / image expansion to `submitted`.
    RunTyped,
    /// Run a turn with this ALREADY-EXPANDED prompt (a `type: "prompt"` command
    /// like `/loop` or a Markdown/Plugin command). The caller submits it
    /// verbatim — no paste expansion (the command builder already produced the
    /// final text).
    RunExpanded(String),
}

/// Dispatch a submitted line through the slash `dispatcher`, expanding a
/// prompt-command (`/loop`, Markdown/Plugin → [`SubmitDisposition::RunExpanded`])
/// or surfacing a display-only builtin ([`SubmitDisposition::Handled`]).
///
/// PARITY-TODO / WARNING: this fn (and [`run_one_submit`]) is NOT on the live
/// production submit path. The interactive TUI submits via the SYNC
/// `dispatch(KeyAction::Submit)` → `AppState::pending_turn` → `root::pump_turn`,
/// which holds only an `OrchestratorHandle` and never consults a slash
/// dispatcher — so a typed `/loop` is sent to the model as raw text there (see
/// the PARITY-TODO at the `pending_turn` assignment in `dispatch`). The only
/// callers of this fn are unit/integration tests. It models the CORRECT
/// expand-then-run behavior and is ready to wire once a `SlashCommandDispatcher`
/// is threaded into the TUI session/`pump_turn`; until then it must not be
/// mistaken for live coverage of typed-`/loop` expansion.
pub async fn handle_submit_line(
    st: &mut AppState,
    line: &str,
    dispatcher: &dyn traits::SlashCommandDispatcher,
) -> SubmitDisposition {
    if let Some(cmd) = line.strip_prefix('/') {
        // Local intercepts (M5-09 stubs don't yet do these).
        let trimmed = cmd.split_whitespace().next().unwrap_or("");
        match trimmed {
            "clear" => {
                st.messages.clear();
                st.scroll_offset = 0;
                // Same rationale as the sync `/clear` intercept above: the
                // rate-limit dedupe slot resets with the scrollback
                // (`has_shown_overage_notification` deliberately NOT reset —
                // TS component state survives transcript clears).
                st.last_rate_limit_text = None;
                return SubmitDisposition::Handled;
            }
            "exit" | "quit" => {
                st.should_exit = true;
                return SubmitDisposition::Handled;
            }
            _ => {}
        }
        // Fall through to the registry for everything else.
        let outcome = dispatcher.dispatch(line).await;
        match outcome {
            traits::SlashDispatchResult::Handled { display }
            | traits::SlashDispatchResult::Unknown { display, .. } => {
                st.push_message(RenderedMessage::SystemText {
                    body: display,
                    timestamp: chrono::Utc::now().timestamp(),
                    is_error: false,
                });
                return SubmitDisposition::Handled;
            }
            // A prompt-expanding command (`/loop`, Markdown/Plugin): the expanded
            // prompt becomes the user turn (claude-code `type: "prompt"`), so the
            // caller runs the model with it instead of printing it.
            traits::SlashDispatchResult::RunAsTurn { prompt } => {
                return SubmitDisposition::RunExpanded(prompt);
            }
            traits::SlashDispatchResult::NotASlashCommand => {
                // Shouldn't happen — we stripped the leading "/" already.
            }
        }
        return SubmitDisposition::Handled;
    }
    SubmitDisposition::RunTyped // plain text — caller runs `orchestrator.run_turn`
}

/// Build the full `ReplScreen` element from an `AppState` snapshot for
/// the given viewport height. Used by the per-frame render path; tests
/// also exercise it to verify the screen composes without panicking.
///
/// The full reactive event-loop wiring (`use_state` hooks, key event →
/// dispatch, re-render on terminal resize) lands in M6-03 along with
/// the streaming spinner. M6-02 ships the pure render function so
/// downstream tasks have a stable assembly point.

/// Build the complete [`crate::components::picker_popup::PopupLine`] list for
/// the `/connect` picker popup: the grouped provider list PLUS the highlighted
/// provider's detail section (connected state, models, login methods).
///
/// Extracted from the `Screen::ConnectPicker` render arm so the detail-append
/// logic has a dedicated unit-test anchor (`connect_picker_detail_tests`).
/// The arm calls this and forwards the result to `render_picker_popup`.
#[must_use]
pub fn connect_picker_popup_lines(
    c: &crate::screens::connect_picker::ConnectPickerState,
    model_providers: &std::collections::BTreeMap<String, (String, String)>,
    provider_availability: &std::collections::BTreeMap<String, bool>,
    provider_auth_methods: &std::collections::BTreeMap<String, String>,
) -> Vec<crate::components::picker_popup::PopupLine> {
    use crate::components::picker_popup::{PopupLine, PopupMarker};
    use crate::screens::connect_picker::{
        provider_detail_lines, provider_label, provider_methods, VisibleLine,
    };

    let mut lines: Vec<PopupLine> = Vec::new();
    let mut item_pos = 0usize;
    for vl in c.visible_lines() {
        match vl {
            VisibleLine::Header(h) => lines.push(PopupLine::Header(h)),
            VisibleLine::Item(idx) => {
                let row = &c.rows[idx];
                let selected = item_pos == c.selected;
                item_pos += 1;
                lines.push(PopupLine::Item {
                    marker: if row.connected { PopupMarker::Check } else { PopupMarker::None },
                    label: row.label.clone(),
                    detail: row.description.clone(),
                    badge: String::new(),
                    selected,
                });
            }
        }
    }
    // --- detail section for the highlighted provider ---
    if let Some(pid) = c.highlighted_provider_id() {
        let connected = provider_availability.get(pid).copied().unwrap_or(false);
        let auth_tag = provider_auth_methods.get(pid).map(String::as_str);
        let methods = provider_methods(pid, auth_tag);
        let label = provider_label(pid);
        let detail = provider_detail_lines(pid, &label, connected, &methods, model_providers);
        // blank separator: Header("") renders with padding_top:1 → visual gap
        lines.push(PopupLine::Header(String::new()));
        for d in detail {
            lines.push(PopupLine::Item {
                marker: PopupMarker::None,
                label: d,
                detail: String::new(),
                badge: String::new(),
                selected: false,
            });
        }
    }
    lines
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn render_screen(
    state: &AppState,
    viewport_height: usize,
    viewport_width: usize,
) -> AnyElement<'static> {
    use crate::screens::repl::{should_render_spinner, ReplScreen};
    // M6-05 focus-trap render path: when a permission dialog is open,
    // it owns the screen. Iocraft 0.8 doesn't expose a portable z-index
    // overlay primitive, so we render the dialog INSTEAD OF the
    // 3-zone layout (documented plan fallback). PromptInput isn't shown
    // at all while the dialog is up — coupled with the keymap focus
    // trap, this guarantees the prompt buffer is untouched.
    if let Some(pp) = &state.pending_permission {
        use crate::components::permissions::bypass_permissions::BypassPermissionsMode;
        use crate::components::permissions::exit_plan_mode::ExitPlanMode;
        use crate::components::permissions::tool_use_confirm::ToolUseConfirm;
        use permission::gate::PermissionRequest;
        return match &pp.request {
            PermissionRequest::ToolUseConfirm {
                tool_name,
                tool_input,
                ..
            } => {
                let tool_name = tool_name.clone();
                let tool_input = tool_input.clone();
                let focus = state.tool_use_dialog_state.focus;
                let worker_name = pp.worker.as_ref().map(|w| w.name.clone());
                element! {
                    ToolUseConfirm(
                        tool_name: tool_name,
                        tool_input: tool_input,
                        cwd: std::env::current_dir().unwrap_or_default(),
                        focus: focus,
                        worker_name: worker_name,
                        theme: state.theme,
                    )
                }
                .into_any()
            }
            PermissionRequest::ExitPlanMode { plan } => {
                let plan = plan.clone();
                let focus = state.exit_plan_dialog_state.focus;
                element! { ExitPlanMode(plan: plan, focus: focus) }.into_any()
            }
            PermissionRequest::BypassPermissionsMode => {
                let typed = state.bypass_dialog_state.typed.clone();
                element! { BypassPermissionsMode(typed: typed) }.into_any()
            }
        };
    }
    // (M7-11) Screen overlay: a full-page screen renders INSTEAD OF the REPL
    // (same render-instead-of discipline as the permission overlay above — no
    // z-index primitive in iocraft 0.8). A pending permission still wins (its
    // branch above returns first, consistent with permission winning keys).
    // Reused by M7-12/13/14 (they add a `Screen` match arm here).
    if let Some(screen) = &state.active_screen {
        use crate::screens::Screen;
        // (M7-11 review) Each variant carries its own state inline; destructure
        // to feed the per-screen component. M7-12/13/14 add a `match` arm here.
        return match screen {
            Screen::Doctor(diag) => {
                use crate::screens::doctor::DoctorScreen;
                element! { DoctorScreen(diag: Some(diag.clone())) }.into_any()
            }
            Screen::Resume(rs) => {
                use crate::screens::resume::ResumeScreen;
                element! { ResumeScreen(state: rs.clone(), theme: state.theme) }.into_any()
            }
            Screen::Settings(ss) => {
                use crate::screens::settings::SettingsScreen;
                element! { SettingsScreen(state: Some(ss.clone())) }.into_any()
            }
            Screen::Memory(ms) => {
                // (M7-14) Re-resolve the tier list synchronously each frame from
                // the M3 walker (cheap fs probe); the editor buffer/selection
                // come from the variant's carried state.
                use crate::screens::memory::{memory_tiers, MemoryScreen};
                let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
                let tiers = memory_tiers(&state.status.cwd, &home);
                element! {
                    MemoryScreen(
                        tiers: tiers,
                        selected: ms.selected,
                        editing: ms.editing,
                        buffer: ms.buffer.clone(),
                        dirty: ms.dirty,
                        status: ms.status.clone(),
                    )
                }
                .into_any()
            }
            Screen::Theme(ps) => {
                // (M7-15) The picker renders under the LIVE palette
                // (`state.theme` — which the Up/Down preview mutates), so the
                // header/preview recolor with the highlighted theme each frame.
                use crate::screens::theme::ThemePickerScreen;
                element! {
                    ThemePickerScreen(
                        state: ps.clone(),
                        theme: state.theme,
                        theme_name: state.theme_setting.resolve(),
                    )
                }
                .into_any()
            }
            Screen::BackgroundTasks(bts) => {
                // (M9-05) The background-tasks dialog renders the pure
                // `render_background_tasks_to_string` body (list↔detail, snapshot-
                // tested) line-by-line. The list it browses lives in
                // `AppState.multiagent.tasks`, kept fresh by the MultiAgent pump.
                use crate::screens::background_tasks::render_background_tasks_to_string;
                // (BASH-ROW-NO-TRUNCATION) claude-code's
                // `maxActivityWidth = Math.max(30, columns - 26)`.
                let max_activity_width = viewport_width.saturating_sub(26).max(30);
                let body = render_background_tasks_to_string(
                    bts,
                    &state.multiagent.tasks,
                    max_activity_width,
                );
                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                element! {
                    View(flex_direction: FlexDirection::Column, padding: 1) {
                        #(lines.into_iter().map(|line| element! {
                            Text(content: line)
                        }))
                    }
                }
                .into_any()
            }
            Screen::Agents(ags) => {
                // (M9-08) The agent-discovery screen renders the pure
                // `render_agents_to_string` body (list↔detail, snapshot-tested)
                // line-by-line in a column View. Mirrors the BackgroundTasks arm.
                use crate::screens::agents::render_agents_to_string;
                let body = render_agents_to_string(ags);
                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                element! {
                    View(flex_direction: FlexDirection::Column, padding: 1) {
                        #(lines.into_iter().map(|line| element! {
                            Text(content: line)
                        }))
                    }
                }
                .into_any()
            }
            Screen::Mcp(m) => {
                // The read-only MCP-server viewer renders the pure
                // `render_mcp_to_string` body (list↔detail, snapshot-tested)
                // line-by-line in a column View. Mirrors the Agents arm.
                use crate::screens::mcp::render_mcp_to_string;
                let body = render_mcp_to_string(m);
                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                element! {
                    View(flex_direction: FlexDirection::Column, padding: 1) {
                        #(lines.into_iter().map(|line| element! {
                            Text(content: line)
                        }))
                    }
                }
                .into_any()
            }
            Screen::Hooks(h) => {
                // The read-only hooks viewer renders the pure
                // `render_hooks_to_string` body (list↔detail, snapshot-tested)
                // line-by-line in a column View. Mirrors the Agents/Mcp arm.
                use crate::screens::hooks::render_hooks_to_string;
                let body = render_hooks_to_string(h);
                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                element! {
                    View(flex_direction: FlexDirection::Column, padding: 1) {
                        #(lines.into_iter().map(|line| element! {
                            Text(content: line)
                        }))
                    }
                }
                .into_any()
            }
            Screen::Model(m) => {
                // (opencode-style popup) The `/model` picker renders as a
                // centered, rounded-border popup window: title + `esc`, a Search
                // line, a `Recent` group (rows mixing providers → provider shown
                // dim), then one group per provider. The current model gets a `●`
                // marker; an unconfigured provider's rows badge `[Connect]`.
                use crate::components::picker_popup::{render_picker_popup, PopupLine, PopupMarker};
                use crate::screens::model::VisibleLine;
                let mut lines: Vec<PopupLine> = Vec::new();
                let mut item_pos = 0usize;
                let mut in_recent = false;
                for vl in m.visible_lines() {
                    match vl {
                        VisibleLine::Header(h) => {
                            in_recent = h == "Recent";
                            lines.push(PopupLine::Header(h));
                        }
                        VisibleLine::Item(idx) => {
                            let row = &m.rows[idx];
                            let selected = item_pos == m.selected;
                            item_pos += 1;
                            let marker = if row.request_model == m.current {
                                PopupMarker::Dot
                            } else {
                                PopupMarker::None
                            };
                            // Recent rows mix providers → show the provider dim;
                            // a provider group's header already names it.
                            let detail = if in_recent {
                                row.provider_label.clone()
                            } else {
                                String::new()
                            };
                            let badge = if row.available {
                                String::new()
                            } else {
                                "[Connect]".to_string()
                            };
                            lines.push(PopupLine::Item {
                                marker,
                                label: row.display_model.clone(),
                                detail,
                                badge,
                                selected,
                            });
                        }
                    }
                }
                render_picker_popup(
                    "Select model",
                    &m.query,
                    &lines,
                    Some("Connect provider  ctrl+a"),
                    viewport_width,
                    viewport_height,
                    &state.theme,
                )
            }
            Screen::ConnectPicker(c) => {
                // (opencode-style popup) The bare-`/connect` provider picker
                // renders as a centered, rounded-border popup window: a bold
                // title + `esc`, a Search line, and the Popular/Providers groups
                // with `✓` for connected providers + a peach highlight on the
                // selected row. After the list, the highlighted provider's detail
                // section (connected state, models, sign-in method) is appended
                // via `connect_picker_popup_lines`. All in the SAME box.
                use crate::components::picker_popup::render_picker_popup;
                let lines = connect_picker_popup_lines(
                    c,
                    &state.model_providers,
                    &state.provider_availability,
                    &state.provider_auth_methods,
                );
                render_picker_popup(
                    "Connect a provider",
                    &c.query,
                    &lines,
                    None,
                    viewport_width,
                    viewport_height,
                    &state.theme,
                )
            }
            Screen::GithubDeployment(g) => {
                // (GitHub Copilot Enterprise) The deployment-type sub-flow, in the
                // same popup: Choose phase = a 2-row menu (Public / Enterprise);
                // Host phase = the Enterprise host typed into the search slot.
                use crate::components::picker_popup::{render_picker_popup, PopupLine, PopupMarker};
                use crate::screens::github_deploy::DeployPhase;
                match &g.phase {
                    DeployPhase::Choose { selected } => {
                        let lines = vec![
                            PopupLine::Item {
                                marker: PopupMarker::None,
                                label: "GitHub.com Public".to_string(),
                                detail: String::new(),
                                badge: String::new(),
                                selected: *selected == 0,
                            },
                            PopupLine::Item {
                                marker: PopupMarker::None,
                                label: "GitHub Enterprise".to_string(),
                                detail: "Data residency or self-hosted".to_string(),
                                badge: String::new(),
                                selected: *selected == 1,
                            },
                        ];
                        render_picker_popup(
                            "Select GitHub deployment type",
                            "",
                            &lines,
                            None,
                            viewport_width,
                            viewport_height,
                            &state.theme,
                        )
                    }
                    DeployPhase::Host { buffer } => render_picker_popup(
                        "GitHub Enterprise host",
                        buffer,
                        &[],
                        Some("e.g. company.ghe.com  \u{00B7}  Enter to continue  \u{00B7}  Esc to go back"),
                        viewport_width,
                        viewport_height,
                        &state.theme,
                    ),
                }
            }
            Screen::ConnectMethod(m) => {
                // (T2b) The login-method choice (multi-method providers, e.g.
                // Anthropic): a menu of `choice_label`s in the shared popup.
                use crate::components::picker_popup::{render_picker_popup, PopupLine, PopupMarker};
                let lines: Vec<PopupLine> = m
                    .options
                    .iter()
                    .enumerate()
                    .map(|(i, opt)| PopupLine::Item {
                        marker: PopupMarker::None,
                        label: opt.choice_label().to_string(),
                        detail: String::new(),
                        badge: String::new(),
                        selected: i == m.selected,
                    })
                    .collect();
                render_picker_popup(
                    &format!("Connect {}", m.label),
                    "",
                    &lines,
                    Some("\u{2191}\u{2193} select  \u{00B7}  Enter  \u{00B7}  Esc to cancel"),
                    viewport_width,
                    viewport_height,
                    &state.theme,
                )
            }
            Screen::Permissions(p) => {
                // The read-only permissions viewer renders the pure
                // `render_permissions_to_string` body (list↔detail, snapshot-
                // tested) line-by-line in a column View. Mirrors the Hooks/Mcp arm.
                use crate::screens::permissions::render_permissions_to_string;
                let body = render_permissions_to_string(p);
                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                element! {
                    View(flex_direction: FlexDirection::Column, padding: 1) {
                        #(lines.into_iter().map(|line| element! {
                            Text(content: line)
                        }))
                    }
                }
                .into_any()
            }
            Screen::Connect(c) => {
                // (Plan 3c §6.3) The `/connect` credential screen renders the pure
                // `render_connect_to_string` body (masked key field or Copilot
                // device-flow, snapshot-tested) line-by-line in a column View.
                // Mirrors the Model/Permissions arm.
                use crate::screens::connect::render_connect_to_string;
                let body = render_connect_to_string(c);
                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                element! {
                    View(flex_direction: FlexDirection::Column, padding: 1) {
                        #(lines.into_iter().map(|line| element! {
                            Text(content: line)
                        }))
                    }
                }
                .into_any()
            }
            Screen::Skills(sks) => {
                // (M9-09) The skill-registry viewer renders the pure
                // `render_skills_to_string` body (grouped sections, snapshot-
                // tested) line-by-line in a column View. Mirrors the Agents arm.
                use crate::screens::skills::render_skills_to_string;
                let body = render_skills_to_string(sks);
                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                element! {
                    View(flex_direction: FlexDirection::Column, padding: 1) {
                        #(lines.into_iter().map(|line| element! {
                            Text(content: line)
                        }))
                    }
                }
                .into_any()
            }
            Screen::Stats(sts) => {
                // (M9-10) The usage-stats screen renders the pure
                // `render_stats_to_string` body (tab header + active-tab body +
                // sparkline/heatmap, snapshot-tested) line-by-line in a column
                // View. Mirrors the Skills/Agents arms.
                use crate::screens::stats::{render_stats_to_string, EMPTY_LINE, MODELS_EMPTY_LINE};
                use crate::theme::TuiTheme;
                let body = render_stats_to_string(sts);
                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                element! {
                    View(flex_direction: FlexDirection::Column, padding: 1) {
                        // (stats-empty-state-color, partial) claude-code colors
                        // the empty-state line `warning`; everything else
                        // (metric values, etc.) needs per-line-type
                        // classification this plain-line render can't do yet.
                        #(lines.into_iter().map(|line| {
                            let is_empty_state = line == EMPTY_LINE || line == MODELS_EMPTY_LINE;
                            let color = if is_empty_state { TuiTheme::WARNING } else { Color::Reset };
                            element! {
                                Text(content: line, color: color)
                            }
                        }))
                    }
                }
                .into_any()
            }
            Screen::Help(h) => {
                // The `/help` shortcuts + slash-command viewer renders the pure
                // `render_help_to_string` body (Shortcuts + Slash-commands
                // sections) line-by-line in a column View. Mirrors the
                // Skills/Stats arms.
                // (GAP D — help display) Render shortcut chords from the LIVE
                // keymap so a user's `keybindings.json` override is reflected.
                use crate::screens::help::render_help_to_string_with;
                let body = render_help_to_string_with(h, state.keymap.bindings());
                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                // (help-6) claude-code's HelpV2 dismiss hint (`{chord} to
                // cancel`) is italic. It's the final line of the body.
                let footer = crate::screens::help::FOOTER;
                element! {
                    View(flex_direction: FlexDirection::Column, padding: 1) {
                        #(lines.into_iter().map(|line| {
                            let italic = line == footer;
                            element! {
                                Text(content: line, italic: italic)
                            }
                        }))
                    }
                }
                .into_any()
            }
            Screen::Transcript(tr) => {
                // (RRS-06) The Ctrl+O transcript renders the pure
                // `render_transcript_to_string` body (the verbose scrollback dump
                // window + scroll indicator + footer) line-by-line in a column
                // View. Mirrors the Skills/Help arms.
                use crate::screens::transcript::render_transcript_to_string;
                let body = render_transcript_to_string(tr);
                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                element! {
                    View(flex_direction: FlexDirection::Column, padding: 1) {
                        #(lines.into_iter().map(|line| element! {
                            Text(content: line)
                        }))
                    }
                }
                .into_any()
            }
        };
    }
    // (M7-14) Message search overlay renders over the REPL at priority 3 (after
    // the permission + screen branches above, mirroring their render-instead-of
    // discipline — no z-index primitive in iocraft 0.8).
    if state.message_selector.open {
        use crate::components::message_selector::{preview_label, MessageSelector};
        let sel = &state.message_selector;
        let result_labels: Vec<String> = sel
            .filtered
            .iter()
            .filter_map(|&i| state.messages.get(i))
            .map(preview_label)
            .collect();
        return element! {
            MessageSelector(
                mode: sel.mode,
                query: sel.query.clone(),
                result_labels: result_labels,
                selected: sel.selected_filtered,
                export: sel.export.clone(),
            )
        }
        .into_any();
    }
    let status = state.status.clone();
    let messages = state.messages.clone();
    let prompt_text = state.prompt_text.clone();
    let prompt_cursor = state.prompt_cursor;
    let scroll_offset = state.scroll_offset;
    let show_spinner = should_render_spinner(state);
    let expanded = state.expanded.clone();
    let focused_tool_id = state.focused_tool_id.clone();
    // (M7-07) Thread the overlay state so the REPL screen can draw the active
    // palette/completion dropdown above the prompt.
    let palette = Some(state.palette.clone());
    let completion = Some(state.completion.clone());
    // (M7-08) Thread the vim flag + mode so the footer can draw `-- MODE --`.
    let vim_enabled = state.vim_enabled;
    let vim_mode = state.vim.mode;
    // (M7-09) Whether the active Visual selection is linewise (`V`) — picks
    // `-- VISUAL LINE --` over `-- VISUAL --`.
    let vim_visual_linewise = state.vim.visual.is_some_and(|v| v.linewise);
    // (M7-10) Thread the active Ctrl-R history-search overlay so the REPL
    // screen can draw its row above the prompt.
    let history_search = state.history_search.clone();
    // (M7-03 review) Thread the already-width-synced height cache instead of
    // rebuilding it inside `VirtualMessageList` every frame (that was
    // O(total messages) per frame). The live render path keeps
    // `state.height_cache` fresh via `root.rs`'s
    // `refresh_height_cache(viewport_width)` immediately before this call,
    // so the common case is a cheap O(1) clone. As a defensive guard
    // against any path that renders without first refreshing (e.g. unit
    // tests building an `AppState` directly), rebuild locally only when the
    // cache is stale vs. the width/length we're asked to render at — the
    // cache can therefore never be stale relative to `viewport_width`.
    let vp_width = viewport_width.max(1);
    let cache = if state.height_cache.width() == vp_width
        && state.height_cache.len() == state.messages.len()
    {
        state.height_cache.clone()
    } else {
        crate::components::virtual_message_list::HeightCache::build(&state.messages, vp_width)
    };
    element! {
        ReplScreen(
            status: status,
            messages: messages,
            cache: cache,
            prompt_text: prompt_text,
            prompt_cursor: prompt_cursor,
            prompt_width: vp_width,
            // (ARGS.3) Inline progressive argument-hint computed from the live
            // buffer + the command→argNames lookup. `None` for every built-in /
            // non-command buffer, so the prompt is byte-identical to today.
            prompt_argument_hint: state.prompt_argument_hint(),
            scroll_offset: scroll_offset,
            viewport_height: viewport_height,
            show_spinner: show_spinner,
            expanded: expanded,
            focused_tool_id: focused_tool_id,
            palette: palette,
            completion: completion,
            vim_enabled: vim_enabled,
            vim_mode: vim_mode,
            vim_visual_linewise: vim_visual_linewise,
            history_search: history_search,
            // (M7-15) active palette + theme name → whole REPL recolors live.
            theme: state.theme,
            theme_name: state.theme_setting.resolve(),
            // (M9-05) Background-task footer pill (hidden when no tasks / all
            // teammates). Driven by the live `multiagent.tasks` list.
            task_footer: crate::components::tasks::status_footer::render_task_footer(
                &state.multiagent.tasks,
            ),
            // (M9-06) Team-status footer pill (hidden when no teammates /
            // only `team-lead`). Driven by the live `multiagent.workers` list.
            team_footer: crate::components::coordinator::team_status::render_team_footer(
                &state.multiagent.workers,
                false,
            ),
            // (M9-06) Teammate-view mode: `Some(name)` renders the header
            // above the transcript; `None` leaves normal mode unchanged.
            viewing_teammate: state.viewing_teammate.clone(),
            // (`/color`) Session agent-color: `Some(name)` tints a banner rule
            // line above the prompt (claude-code `useSwarmBanner` standalone
            // branch); `None` hides it. Reset (app.rs `/color default`) clears
            // it back to `None`.
            session_agent_color: state.session_agent_color.clone(),
            // (A6) Custom status-line text + padding. `Some(text)` swaps the
            // built-in status row for the custom command's transformed stdout;
            // `None` (the default) keeps the built-in row byte-identical. The
            // padding comes from the parsed `statusLine.padding` setting.
            status_line_text: state.status_line_text.clone(),
            status_line_padding: state
                .status_line_config
                .as_ref()
                .map_or(0, |c| c.padding),
            // (TokenWarning) live context-pressure banner above the prompt.
            context_pressure: state.context_pressure.clone(),
            // (RRS-08) Footer-left override while the double-press exit
            // window is armed and not yet expired.
            exit_hint: state.sigint_armed_at.and_then(|t| {
                (t.elapsed().as_millis() < u128::from(SIGINT_WINDOW_MS))
                    .then_some(state.sigint_armed_key)
            }),
            // (SS-06) reduced-motion → static spinner.
            reduced_motion: state.reduced_motion,
            // (SS-08) Active todo → drives the spinner's leader verb.
            current_todo: state.current_todo.clone(),
        )
    }
    .into_any()
}

/// Outcome of a turn from the TUI's vantage point. M6-02 only needs the
/// accumulated assistant text; richer fields (tool calls, cost, stop
/// reason) arrive in M6-03+.
#[derive(Debug, Clone, Default)]
pub struct TurnTextOutcome {
    /// Accumulated assistant text body for this turn.
    pub text: String,
}

/// Local trait the TUI uses to invoke a turn. Mirrors the upstream
/// `ConversationOrchestrator::run_turn_with_cancel` contract but lets
/// tests pass a fake orchestrator without coupling to the real type.
#[async_trait::async_trait]
pub trait ConversationOrchestratorTrait: Send + Sync {
    /// Drive one user prompt through the orchestrator and return the
    /// accumulated assistant text once the turn ends.
    async fn run_turn(
        &self,
        prompt: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<TurnTextOutcome, String>;

    /// (MULTIMODAL.1) Image-capable variant of [`Self::run_turn`]: carries the
    /// pasted/dragged image paths the prompt's `[Image #N]` placeholders refer
    /// to, mirroring the orchestrator handle's
    /// `run_turn_streaming_with_images`. The live submit ([`run_one_submit`])
    /// calls THIS so captured image paths reach the model instead of being
    /// dropped.
    ///
    /// The default DROPS the images and delegates to the text-only
    /// [`Self::run_turn`], so existing impls keep compiling and a submit with
    /// no captured images is byte-identical to the prior text-only path. A
    /// production impl overrides this to forward the paths to
    /// `OrchestratorHandle::run_turn_streaming_with_images`.
    async fn run_turn_with_images(
        &self,
        prompt: &str,
        image_paths: &[std::path::PathBuf],
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<TurnTextOutcome, String> {
        let _ = image_paths;
        self.run_turn(prompt, cancel).await
    }
}

/// Process one submitted line end-to-end: slash dispatch OR `run_turn`.
///
/// The caller (per-frame loop) has already pushed the user message and
/// cleared the prompt via `dispatch(KeyAction::Submit, ...)`. This fn
/// then either routes the line to `handle_submit_line` (for slash
/// commands) or to `orch.run_turn`, pushing the resulting assistant
/// text (or error) into the scrollback.
pub async fn run_one_submit(
    st: &mut AppState,
    submitted: &str,
    orch: &dyn ConversationOrchestratorTrait,
    dispatcher: &dyn traits::SlashCommandDispatcher,
) {
    // A `type: "prompt"` command (`/loop`, Markdown/Plugin) yields an
    // already-expanded prompt to run verbatim; plain text runs the typed line
    // (after paste/image expansion); everything else is fully handled.
    let expanded_override = match handle_submit_line(st, submitted, dispatcher).await {
        SubmitDisposition::Handled => return,
        SubmitDisposition::RunTyped => None,
        SubmitDisposition::RunExpanded(prompt) => Some(prompt),
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let turn_id = next_turn_id();
    st.in_flight_turn = Some(crate::state::TurnInFlight {
        turn_id,
        cancel: cancel.clone(),
    });

    // (PIC-05) Expand any `[Pasted text #N]` pills back to their original
    // content BEFORE draining images — `take_image_paths` resets the WHOLE
    // paste registry (images AND pasted-text pairs), so pasted-text must be
    // drained first or it's lost.
    let pasted_texts = st.paste.take_pasted_texts();
    // (MULTIMODAL.1) Consume any pasted/dragged image paths captured in the
    // prompt's paste registry so they ride along to the model as real image
    // content blocks (the `[Image #N]` placeholders in `submitted` point back
    // to these). `take_image_paths` both drains the paths AND resets the
    // registry, mirroring how `dispatch(Submit)` consumed `prompt_text` — so
    // they reach THIS turn and don't leak into the next one. When nothing was
    // pasted the list is empty and the trait's default delegates to the
    // text-only `run_turn`, byte-identical to the prior behavior.
    let image_paths = st.paste.take_image_paths();
    // A `RunAsTurn` command supplies the final prompt directly (no paste-pill
    // expansion — the builder already produced the text); plain text gets the
    // normal paste-ref expansion of the typed line.
    let expanded = match expanded_override {
        Some(prompt) => prompt,
        None => crate::components::prompt_input::expand_pasted_text_refs(submitted, &pasted_texts),
    };
    let outcome = orch
        .run_turn_with_images(&expanded, &image_paths, cancel)
        .await;
    st.in_flight_turn = None;
    match outcome {
        Ok(TurnTextOutcome { text }) => {
            st.push_message(RenderedMessage::AssistantText {
                body: text,
                timestamp: chrono::Utc::now().timestamp(),
            });
        }
        Err(e) => {
            st.push_message(RenderedMessage::SystemText {
                body: format!("error: {e}"),
                timestamp: chrono::Utc::now().timestamp(),
                is_error: true,
            });
        }
    }
}

fn next_turn_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Ctrl-C handler during streaming. (M6-03 T8)
///
/// Idempotent if `streaming.is_none()` (caller delegates to M6-02's
/// prompt-clear / second-Ctrl-C logic in that case). When streaming, fires
/// the cancel token; the orchestrator task will return
/// `TurnOutcome::Cancelled` and the bridge will emit `TurnEnded`, which
/// `apply_event` translates into `streaming = None`.
///
/// We do NOT clear `state.streaming` here — we wait for the bridge's
/// `TurnEnded` event so the spinner stays up until the orchestrator
/// actually unwinds.
pub fn handle_ctrl_c(state: &mut AppState) {
    if let Some(token) = state.cancel_token.take() {
        token.cancel();
    }
}

/// Spawn a streaming turn. (M6-03 T8)
///
/// Synchronously emits `TurnEvent::TurnStarted` on the provided sender
/// (so the spinner appears immediately on Enter), then spawns a task that
/// awaits `handle.run_turn_streaming_with_cancel(prompt, cancel)` and
/// sends a final `TurnEvent::TurnEnded(outcome)` once it completes.
///
/// Returns the [`CancellationToken`] the caller should store in
/// `state.cancel_token`. Per-turn text/tool events flow through the
/// orchestrator's `OutputStream` (the `BridgeOutputStream` wired at
/// construction) — this helper does NOT see them.
///
/// [`CancellationToken`]: tokio_util::sync::CancellationToken
#[must_use]
#[allow(clippy::needless_pass_by_value)]
pub fn spawn_streaming_turn(
    handle: std::sync::Arc<dyn traits::OrchestratorHandle>,
    prompt: String,
    image_paths: Vec<std::path::PathBuf>,
    tx: tokio::sync::mpsc::UnboundedSender<crate::events::orchestrator_bridge::TurnEvent>,
) -> tokio_util::sync::CancellationToken {
    use crate::events::orchestrator_bridge::TurnEvent;

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();
    let tx_clone = tx.clone();

    // Emit TurnStarted SYNCHRONOUSLY before spawning so the UI shows
    // the spinner immediately on Enter, not after the first network
    // roundtrip.
    let _ = tx.send(TurnEvent::TurnStarted);

    tokio::spawn(async move {
        let outcome = handle
            .run_turn_streaming_with_images(&prompt, &image_paths, cancel_clone)
            .await;
        let ev = match outcome {
            Ok(o) => TurnEvent::TurnEnded(o),
            Err(e) => {
                tracing::error!(error = ?e, "streaming turn failed");
                // Surface error completion as a clean EndTurn for now;
                // M7 may introduce a dedicated TurnEnded(Error) variant.
                TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn)
            }
        };
        let _ = tx_clone.send(ev);
    });

    cancel
}

/// Apply a scroll direction with a known viewport height. Called per
/// frame for PageUp/PageDown, where the viewport is known; line-step
/// (`j`/`k`) reuses this with `height=1`.
///
/// Internally widens to `i64` so that "scroll past the top/bottom" is
/// expressible as a signed value before clamping back into `usize`.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn scroll_with_viewport(st: &mut AppState, dir: ScrollDir, viewport_height: usize) {
    // (M7-03) Scroll math is LINE-based: clamp against total rendered lines
    // from the height cache, not the message count. Refresh the cache at
    // the current viewport width first so `total_lines` is accurate.
    st.refresh_height_cache(st.viewport_width.max(1));
    let total = st.height_cache.total_lines();
    let max = total.saturating_sub(viewport_height) as i64;
    let cur = st.scroll_offset as i64;
    // (RRS-01) PageUp/PageDown move HALF a viewport (claude-code), not a full
    // one. Step = max(1, viewport/2) via viewport.max(2)/2.
    let page = (viewport_height.max(2) / 2) as i64;
    let new = match dir {
        ScrollDir::LineUp => cur + 1,
        ScrollDir::LineDown => cur - 1,
        ScrollDir::PageUp => cur + page,
        ScrollDir::PageDown => cur - page,
        ScrollDir::Top => max,
        ScrollDir::Bottom => 0,
    };
    let prev_offset = st.scroll_offset;
    st.scroll_offset = new.clamp(0, max) as usize;
    // (M6-09) Emit scroll-mode lifecycle on the offset transition: 0 →
    // non-zero opens scroll mode; non-zero → 0 closes it (back at bottom).
    if prev_offset == 0 && st.scroll_offset != 0 {
        crate::telemetry::scroll_started(st.scroll_offset);
    } else if prev_offset != 0 && st.scroll_offset == 0 {
        crate::telemetry::scroll_ended();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_idle_state() {
        let app = TuiApp::new();
        assert!(!app.should_quit);
    }

    #[test]
    fn request_quit_sets_flag() {
        let mut app = TuiApp::new();
        app.request_quit();
        assert!(app.should_quit);
    }

    #[test]
    fn version_line_includes_crate_version() {
        let v = version_line();
        assert!(v.starts_with("lingxi-tui v"));
        assert!(v.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn render_returns_an_element() {
        let app = TuiApp::new();
        let _el = app.render();
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use crate::events::keymap::{KeyAction, ScrollDir};
    use crate::state::{AppState, RenderedMessage, StatusSnapshot};
    use permission::PermissionMode;
    use std::path::PathBuf;

    fn s() -> AppState {
        AppState::new(StatusSnapshot {
            model: "claude-sonnet-4.5".into(),
            cwd: PathBuf::from("/a/b"),
            cost: "$0.0000".to_string(),
            context_pct: 0.0,
            permission_mode: PermissionMode::Default,
            ..StatusSnapshot::default()
        })
    }

    #[test]
    fn insert_chars_then_backspace() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('h'), &mut st);
        dispatch(KeyAction::InsertChar('i'), &mut st);
        assert_eq!(st.prompt_text, "hi");
        assert_eq!(st.prompt_cursor, 2);
        dispatch(KeyAction::Backspace, &mut st);
        assert_eq!(st.prompt_text, "h");
        assert_eq!(st.prompt_cursor, 1);
    }

    #[test]
    fn insert_newline_adds_line() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('a'), &mut st);
        dispatch(KeyAction::InsertNewline, &mut st);
        dispatch(KeyAction::InsertChar('b'), &mut st);
        assert_eq!(st.prompt_text, "a\nb");
        assert_eq!(st.prompt_cursor, 3);
    }

    #[test]
    fn backslash_return_strips_backslash_and_adds_newline() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('a'), &mut st);
        dispatch(KeyAction::InsertChar('\\'), &mut st);
        // Enter with a trailing backslash → keymap emits InsertNewline.
        dispatch(KeyAction::InsertNewline, &mut st);
        assert_eq!(st.prompt_text, "a\n"); // trailing '\' stripped, '\n' inserted
        assert_eq!(st.prompt_cursor, 2);
    }

    #[test]
    fn move_cursor_vertical_down() {
        let mut st = s();
        st.prompt_text = "abc\ndef".into();
        st.prompt_cursor = 2;
        dispatch(KeyAction::MoveCursorVertical(1), &mut st);
        assert_eq!(st.prompt_cursor, 6);
    }

    #[test]
    fn submit_clears_prompt_and_pushes_user_message() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('h'), &mut st);
        dispatch(KeyAction::InsertChar('i'), &mut st);
        dispatch(KeyAction::Submit, &mut st);
        assert_eq!(st.prompt_text, "");
        assert_eq!(st.messages.len(), 1);
        assert!(matches!(&st.messages[0], RenderedMessage::UserText { body, .. } if body == "hi"));
        assert_eq!(st.history.last().map(String::as_str), Some("hi"));
    }

    /// (`!` bash mode) Submitting `!echo hi` does NOT raise a turn/slash: it
    /// echoes a `UserBashInput` row and raises `pending_bash` with the command
    /// text (the `!` stripped, leading ws trimmed) for `root::pump_bash` to run.
    #[test]
    fn submit_bang_command_raises_pending_bash_not_a_turn() {
        let mut st = s();
        for c in "!  echo hi".chars() {
            dispatch(KeyAction::InsertChar(c), &mut st);
        }
        let acted = dispatch(KeyAction::Submit, &mut st);
        assert!(acted);
        assert_eq!(st.prompt_text, "");
        assert_eq!(st.pending_bash.as_deref(), Some("echo hi"));
        assert!(st.pending_turn.is_none(), "a `!` line must NOT raise a turn");
        assert!(st.pending_slash.is_none(), "a `!` line must NOT raise a slash");
        assert!(
            matches!(
                st.messages.last(),
                Some(RenderedMessage::UserBashInput { command }) if command == "echo hi"
            ),
            "the command must be echoed as a UserBashInput row"
        );
    }

    /// A BARE `!` (no command after the prefix) is just the composer's bash-mode
    /// marker — it falls through to the normal prompt path (raises `pending_turn`).
    #[test]
    fn submit_bare_bang_falls_through_to_normal_prompt() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('!'), &mut st);
        let acted = dispatch(KeyAction::Submit, &mut st);
        assert!(acted);
        assert!(st.pending_bash.is_none(), "bare `!` must NOT raise pending_bash");
        assert_eq!(st.pending_turn.as_deref(), Some("!"));
    }

    #[test]
    fn pgup_increments_scroll_offset_by_viewport() {
        let mut st = s();
        for i in 0..30 {
            st.push_message(RenderedMessage::UserText {
                body: format!("m{i}"),
                timestamp: 0,
            });
        }
        // (RRS-01) PageUp steps HALF a viewport: 10/2 = 5.
        scroll_with_viewport(&mut st, ScrollDir::PageUp, 10);
        assert_eq!(st.scroll_offset, 5);
        scroll_with_viewport(&mut st, ScrollDir::PageUp, 10);
        assert_eq!(st.scroll_offset, 10);
    }

    #[test]
    fn ctrl_c_clears_nonempty_prompt() {
        let mut st = s();
        dispatch(KeyAction::InsertChar('x'), &mut st);
        assert_eq!(st.prompt_text, "x");
        dispatch(KeyAction::Cancel, &mut st);
        assert_eq!(st.prompt_text, "");
        assert!(st.sigint_armed_at.is_none());
    }

    /// (RRS-08) Ctrl+C on an in-flight turn cancels the token and pushes
    /// claude-code's INTERRUPT_MESSAGE as a UserText body (not the old
    /// invented "^C interrupted by user" SystemText), mirroring the Esc
    /// branch (root.rs).
    #[test]
    fn ctrl_c_interrupts_in_flight_turn_with_claude_code_marker() {
        let mut st = s();
        let token = tokio_util::sync::CancellationToken::new();
        st.in_flight_turn = Some(crate::state::TurnInFlight { turn_id: 1, cancel: token.clone() });
        dispatch(KeyAction::Cancel, &mut st);
        assert!(token.is_cancelled(), "Ctrl+C must cancel the in-flight turn token");
        assert!(
            st.messages.iter().any(|m| matches!(
                m,
                RenderedMessage::UserText { body, .. }
                    if body == crate::components::messages::user_tool_result::INTERRUPT_MESSAGE
            )),
            "Ctrl+C must push the INTERRUPT_MESSAGE marker, got: {:?}",
            st.messages
        );
    }

    #[test]
    fn idle_ctrl_c_arms_without_pushing_a_scrollback_message() {
        // (RRS-08) The "Press Ctrl-C again to exit" confirmation is a
        // transient footer hint (PromptInputFooter.exit_hint), not a pushed
        // scrollback line.
        let mut st = s();
        let before = st.messages.len();
        dispatch(KeyAction::Cancel, &mut st);
        assert!(st.sigint_armed_at.is_some(), "first idle Ctrl+C arms");
        assert_eq!(st.sigint_armed_key, "Ctrl-C");
        assert_eq!(st.messages.len(), before, "no scrollback message pushed");
        dispatch(KeyAction::Cancel, &mut st);
        assert!(st.should_exit, "second idle Ctrl+C within the window exits");
    }

    #[test]
    fn vim_command_toggles_mode_and_echoes() {
        let mut st = s();
        // `/vim` → enable: `vim_enabled` flips true, prompt cleared, exact echo
        // (claude-code commands/vim/vim.ts).
        st.prompt_text = "/vim".to_string();
        dispatch(KeyAction::Submit, &mut st);
        assert!(st.vim_enabled);
        assert_eq!(st.prompt_text, "");
        match st.messages.last() {
            Some(RenderedMessage::SystemText {
                body,
                is_error: false,
                ..
            }) => assert_eq!(
                body,
                "Editor mode set to vim. Use Escape key to toggle between INSERT and NORMAL modes."
            ),
            other => panic!("expected vim-on SystemText, got {other:?}"),
        }
        // `/vim` again → disable.
        st.prompt_text = "/vim".to_string();
        dispatch(KeyAction::Submit, &mut st);
        assert!(!st.vim_enabled);
        match st.messages.last() {
            Some(RenderedMessage::SystemText {
                body,
                is_error: false,
                ..
            }) => assert_eq!(
                body,
                "Editor mode set to normal. Using standard (readline) keyboard bindings."
            ),
            other => panic!("expected vim-off SystemText, got {other:?}"),
        }
    }

    /// `/copy` with an empty transcript pushes the "nothing to copy" error
    /// `SystemText`, clears the prompt, does NOT raise a clipboard write, and
    /// does NOT run a turn (Submit returns false).
    #[test]
    fn copy_command_empty_transcript_graceful() {
        let mut st = s();
        st.prompt_text = "/copy".to_string();
        let runs_turn = dispatch(KeyAction::Submit, &mut st);
        assert!(!runs_turn, "/copy must not run a turn");
        assert_eq!(st.prompt_text, "");
        assert!(st.pending_copy_clipboard.is_none());
        match st.messages.last() {
            Some(RenderedMessage::SystemText {
                body,
                is_error: true,
                ..
            }) => assert_eq!(body, "No assistant message to copy"),
            other => panic!("expected nothing-to-copy error, got {other:?}"),
        }
    }

    /// `/copy` (no arg) copies the LATEST assistant text: raises
    /// `pending_copy_clipboard` with that body + pushes the confirmation.
    #[test]
    fn copy_command_copies_latest_assistant_message() {
        let mut st = s();
        st.push_message(RenderedMessage::AssistantText {
            body: "older".into(),
            timestamp: 0,
        });
        st.push_message(RenderedMessage::AssistantText {
            body: "newest".into(),
            timestamp: 0,
        });
        st.prompt_text = "/copy".to_string();
        let runs_turn = dispatch(KeyAction::Submit, &mut st);
        assert!(!runs_turn, "/copy must not run a turn");
        assert_eq!(st.prompt_text, "");
        assert_eq!(st.pending_copy_clipboard.as_deref(), Some("newest"));
        match st.messages.last() {
            Some(RenderedMessage::SystemText {
                body,
                is_error: false,
                ..
            }) => assert_eq!(body, "Copied to clipboard (6 characters, 1 lines)"),
            other => panic!("expected copy confirmation, got {other:?}"),
        }
    }

    /// `/copy N` selects the Nth-latest (2 = second-to-latest).
    #[test]
    fn copy_command_n_selects_nth_latest() {
        let mut st = s();
        for body in ["third", "second", "first"] {
            st.push_message(RenderedMessage::AssistantText {
                body: body.into(),
                timestamp: 0,
            });
        }
        st.prompt_text = "/copy 2".to_string();
        let runs_turn = dispatch(KeyAction::Submit, &mut st);
        assert!(!runs_turn, "/copy N must not run a turn");
        // /copy 2 → second-to-latest = "second".
        assert_eq!(st.pending_copy_clipboard.as_deref(), Some("second"));
    }

    /// A bad `/copy` arg pushes the usage error and raises no clipboard write.
    #[test]
    fn copy_command_bad_arg_returns_usage_error() {
        let mut st = s();
        st.push_message(RenderedMessage::AssistantText {
            body: "x".into(),
            timestamp: 0,
        });
        st.prompt_text = "/copy abc".to_string();
        let runs_turn = dispatch(KeyAction::Submit, &mut st);
        assert!(!runs_turn);
        assert!(st.pending_copy_clipboard.is_none());
        match st.messages.last() {
            Some(RenderedMessage::SystemText {
                body,
                is_error: true,
                ..
            }) => assert_eq!(
                body,
                "Usage: /copy [N] where N is 1 (latest), 2, 3, \u{2026} Got: abc"
            ),
            other => panic!("expected usage error, got {other:?}"),
        }
    }

    /// Build a `RegistrySlashDispatcher` seeded with the M5-09 built-ins.
    fn dispatcher() -> command_api::RegistrySlashDispatcher {
        use command_api::CommandRegistry;
        use command_api::RegistrySlashDispatcher;
        use command_core::register_all_builtin_commands;
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)))
    }

    #[tokio::test]
    async fn slash_clear_empties_messages() {
        let mut st = s();
        st.push_message(RenderedMessage::AssistantText {
            body: "old".into(),
            timestamp: 0,
        });
        assert_eq!(st.messages.len(), 1);

        let disp = dispatcher();
        let disp_result = handle_submit_line(&mut st, "/clear", &disp).await;
        assert_eq!(disp_result, SubmitDisposition::Handled);
        assert!(st.messages.is_empty());
    }

    #[tokio::test]
    async fn slash_exit_sets_should_exit() {
        let mut st = s();
        let disp = dispatcher();
        handle_submit_line(&mut st, "/exit", &disp).await;
        assert!(st.should_exit);
    }

    /// A `type: "prompt"` command (Markdown here; bundled `/loop` shares the
    /// path) yields its EXPANDED prompt as a `RunExpanded` disposition — the
    /// caller runs the model with it — while a builtin stays display-only
    /// (`Handled`). Proves Item #2's routing distinction at the TUI surface.
    #[tokio::test]
    async fn prompt_command_runs_as_turn_builtin_stays_display() {
        use command_api::model::{
            CommandFrontmatter, CommandSource, SlashCommand, SlashCommandKind,
        };
        use command_api::{CommandRegistry, RegistrySlashDispatcher};
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let mut reg = CommandRegistry::new();
        // A builtin stub (display-only) + a markdown prompt command.
        command_core::register_all_builtin_commands(&mut reg);
        reg.register_command(SlashCommand {
            name: "deploy".to_string(),
            description: "Deploy".to_string(),
            source: CommandSource::Project,
            kind: SlashCommandKind::Markdown {
                file_path: std::path::PathBuf::from("/tmp/deploy.md"),
                frontmatter: CommandFrontmatter::default(),
                prompt_template: "Ship $ARGUMENTS".to_string(),
            },
            ..SlashCommand::default()
        });
        let disp = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));

        // Markdown prompt command → RunExpanded with the expanded body.
        let mut st = s();
        let r = handle_submit_line(&mut st, "/deploy prod", &disp).await;
        assert_eq!(r, SubmitDisposition::RunExpanded("Ship prod".to_string()));

        // A builtin (`/help`) stays display-only — never runs a turn.
        let mut st2 = s();
        let r2 = handle_submit_line(&mut st2, "/help", &disp).await;
        assert_eq!(r2, SubmitDisposition::Handled);
    }

    #[test]
    fn render_screen_smoke() {
        let mut st = s();
        st.push_message(RenderedMessage::AssistantText {
            body: "hi".into(),
            timestamp: 0,
        });
        let mut element = render_screen(&st, 5, 80);
        let rendered = element.to_string();
        // (ma-03) marker glyph is platform-conditional (⏺ macOS / ● else).
        let marker = crate::components::messages::assistant_text::MARKER;
        assert!(rendered.contains(&format!("{marker}hi")), "got: {rendered}");
        // (SS-01) No built-in status row renders without a custom statusLine
        // command, so the model id no longer appears in the chrome.
        assert!(
            !rendered.contains("claude-sonnet-4.5"),
            "built-in status row must not render: {rendered}"
        );
    }

    #[test]
    fn doctor_slash_opens_screen_via_dispatch() {
        use crate::screens::Screen;
        let mut st = s();
        // Submit a "/doctor" line through the same path the live Enter uses.
        st.prompt_text = "/doctor".to_string();
        st.prompt_cursor = "/doctor".len();
        let should_run = dispatch(KeyAction::Submit, &mut st);
        assert!(!should_run, "/doctor opens a screen, never runs a turn");
        // (M7-11 review) The Doctor variant carries its captured diagnostics.
        assert!(
            matches!(&st.active_screen, Some(Screen::Doctor(_))),
            "diagnostics captured inside the Doctor variant at open"
        );
        assert!(st.prompt_text.is_empty(), "prompt cleared on submit");
        // No UserText pushed for the intercepted slash command.
        assert!(
            !matches!(st.messages.last(), Some(RenderedMessage::UserText { .. })),
            "/doctor must not echo as a user message"
        );
    }

    #[test]
    fn tasks_slash_opens_background_tasks_screen_via_dispatch() {
        use crate::screens::Screen;
        let mut st = s();
        // Submit "/tasks" through the same path the live Enter uses. The open
        // is synchronous (mirrors the Shift+Down binding in root.rs): seeds a
        // fresh BackgroundTasksState, no pending flag, no UserText echo.
        st.prompt_text = "/tasks".to_string();
        st.prompt_cursor = "/tasks".len();
        let should_run = dispatch(KeyAction::Submit, &mut st);
        assert!(!should_run, "/tasks opens a screen, never runs a turn");
        assert!(
            matches!(&st.active_screen, Some(Screen::BackgroundTasks(_))),
            "/tasks opens the BackgroundTasks screen inline"
        );
        assert!(st.prompt_text.is_empty(), "prompt cleared on submit");
        // No system message and no UserText echo for the intercepted command.
        assert!(
            st.messages.is_empty(),
            "/tasks must not push a system or user message"
        );
    }

    #[test]
    fn non_tasks_line_does_not_open_background_tasks_screen() {
        let mut st = s();
        // A normal prompt line must NOT be mistaken for the /tasks intercept:
        // it runs a turn and leaves no screen open.
        st.prompt_text = "list the tasks".to_string();
        st.prompt_cursor = "list the tasks".len();
        let should_run = dispatch(KeyAction::Submit, &mut st);
        assert!(should_run, "a normal line runs a turn");
        assert!(
            st.active_screen.is_none(),
            "a normal line never opens the BackgroundTasks screen"
        );
        // The line is echoed as a user message (normal submit path).
        assert!(matches!(
            st.messages.last(),
            Some(RenderedMessage::UserText { .. })
        ));
    }

    #[test]
    fn color_set_via_dispatch_sets_field_raises_save_and_pushes_system() {
        let mut st = s();
        st.prompt_text = "/color cyan".to_string();
        st.prompt_cursor = "/color cyan".len();
        let should_run = dispatch(KeyAction::Submit, &mut st);
        assert!(!should_run, "/color is immediate, never runs a turn");
        assert!(st.prompt_text.is_empty(), "prompt cleared on submit");
        // Immediate session-color field set.
        assert_eq!(st.session_agent_color.as_deref(), Some("cyan"));
        // Persistence raised with the color name (not the sentinel).
        assert_eq!(st.pending_save_color.as_deref(), Some("cyan"));
        // A non-error system display, NOT an echoed user message.
        match st.messages.last() {
            Some(RenderedMessage::SystemText { body, is_error, .. }) => {
                assert_eq!(body, "Session color set to: cyan");
                assert!(!is_error);
            }
            other => panic!("expected SystemText, got {other:?}"),
        }
    }

    #[test]
    fn color_reset_via_dispatch_clears_field_and_persists_default_sentinel() {
        let mut st = s();
        st.session_agent_color = Some("orange".to_string());
        st.prompt_text = "/color default".to_string();
        st.prompt_cursor = st.prompt_text.len();
        dispatch(KeyAction::Submit, &mut st);
        assert_eq!(st.session_agent_color, None, "reset clears the field");
        // Reset persists the "default" sentinel (NOT empty / NOT cleared flag).
        assert_eq!(st.pending_save_color.as_deref(), Some("default"));
        assert!(matches!(
            st.messages.last(),
            Some(RenderedMessage::SystemText {
                is_error: false,
                ..
            })
        ));
    }

    #[test]
    fn color_invalid_via_dispatch_errors_without_touching_state() {
        let mut st = s();
        st.prompt_text = "/color chartreuse".to_string();
        st.prompt_cursor = st.prompt_text.len();
        dispatch(KeyAction::Submit, &mut st);
        // No session-color change, no persistence raised.
        assert_eq!(st.session_agent_color, None);
        assert_eq!(st.pending_save_color, None);
        match st.messages.last() {
            Some(RenderedMessage::SystemText { is_error, .. }) => assert!(is_error),
            other => panic!("expected error SystemText, got {other:?}"),
        }
    }

    #[test]
    fn color_bare_via_dispatch_lists_colors_without_persisting() {
        let mut st = s();
        st.prompt_text = "/color".to_string();
        st.prompt_cursor = st.prompt_text.len();
        dispatch(KeyAction::Submit, &mut st);
        assert_eq!(st.session_agent_color, None);
        assert_eq!(st.pending_save_color, None, "listing does not persist");
        match st.messages.last() {
            Some(RenderedMessage::SystemText { body, is_error, .. }) => {
                assert!(body.contains("Available colors:"));
                assert!(!is_error);
            }
            other => panic!("expected SystemText, got {other:?}"),
        }
    }

    #[test]
    fn render_screen_renders_doctor_when_active() {
        use crate::screens::doctor::DoctorDiagnostics;
        let mut st = s();
        st.open_doctor(DoctorDiagnostics::capture(
            std::path::Path::new("/work"),
            1,
            0,
            (80, 24),
        ));
        let mut element = render_screen(&st, 20, 80);
        let rendered = element.to_string();
        assert!(rendered.contains("Diagnostics"), "got: {rendered}");
        assert!(
            !rendered.contains("claude-sonnet-4.5"),
            "REPL status hidden while screen up"
        );
    }

    /// (M7-11 review FIX #3) `/doctor` reads the LIVE terminal size off the
    /// status snapshot (the live `use_terminal_events` closure writes
    /// `st.status.term_size = (cols, rows)` before routing each key). When that
    /// is a real value, the captured diagnostics + rendered screen show it —
    /// NOT the `(0,0)` default that produced "0x0" in the live Doctor.
    #[test]
    fn doctor_uses_live_term_size_not_zero() {
        use crate::screens::Screen;
        let mut st = s();
        // The live closure publishes the real size onto the status before the
        // Enter that opens Doctor. Simulate that here.
        st.status.term_size = (137, 51);
        st.prompt_text = "/doctor".to_string();
        st.prompt_cursor = "/doctor".len();
        let _ = dispatch(KeyAction::Submit, &mut st);
        // The captured diagnostics inside the variant carry the live size.
        match &st.active_screen {
            Some(Screen::Doctor(diag)) => assert_eq!(diag.term_size, (137, 51)),
            other => panic!("expected open Doctor screen, got {other:?}"),
        }
        // ...and it renders, not "0x0".
        let mut element = render_screen(&st, 20, 80);
        let rendered = element.to_string();
        assert!(
            rendered.contains("137x51"),
            "live Doctor must show real terminal size, got: {rendered}"
        );
        assert!(
            !rendered.contains("0x0"),
            "must not show the (0,0) default, got: {rendered}"
        );
    }

    #[test]
    fn help_slash_opens_help_screen_via_dispatch() {
        use crate::screens::Screen;
        let mut st = s();
        // Submit "/help" through the same path the live Enter uses. The open is
        // synchronous (mirrors the /tasks inline open): seeds a fresh HelpState,
        // no pending flag, no UserText echo.
        st.prompt_text = "/help".to_string();
        st.prompt_cursor = "/help".len();
        let should_run = dispatch(KeyAction::Submit, &mut st);
        assert!(!should_run, "/help opens a screen, never runs a turn");
        assert!(
            matches!(&st.active_screen, Some(Screen::Help(_))),
            "/help opens the Help screen inline"
        );
        assert!(st.prompt_text.is_empty(), "prompt cleared on submit");
        // No system message and no UserText echo for the intercepted command.
        assert!(
            st.messages.is_empty(),
            "/help must not push a system or user message"
        );
    }
}

/// (MULTIMODAL.1) The live submit must forward captured paste/drag image paths
/// through the image-capable orchestrator entry (so they reach the model) and
/// consume the paste registry on submit, while staying byte-identical to the
/// text-only path when nothing was pasted.
#[cfg(test)]
mod image_submit_tests {
    use super::*;
    use crate::components::prompt_input::{process_paste, PasteState};
    use crate::state::{AppState, RenderedMessage, StatusSnapshot};
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn s() -> AppState {
        AppState::new(StatusSnapshot::default())
    }

    fn dispatcher() -> command_api::RegistrySlashDispatcher {
        use command_api::{CommandRegistry, RegistrySlashDispatcher};
        use command_core::register_all_builtin_commands;
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let mut reg = CommandRegistry::new();
        register_all_builtin_commands(&mut reg);
        RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)))
    }

    /// Fake orchestrator that records the prompt + image paths it was asked to
    /// run via the image-capable entry, so the test can assert the live submit
    /// forwards the captured paste image paths.
    #[derive(Default)]
    struct RecordingOrch {
        prompt: Mutex<Option<String>>,
        images: Mutex<Option<Vec<PathBuf>>>,
    }

    #[async_trait::async_trait]
    impl ConversationOrchestratorTrait for RecordingOrch {
        async fn run_turn(
            &self,
            prompt: &str,
            _cancel: tokio_util::sync::CancellationToken,
        ) -> Result<TurnTextOutcome, String> {
            // Should not be hit in these tests — the live submit always routes
            // through `run_turn_with_images`. Record empty so a regression is
            // visible.
            *self.prompt.lock().unwrap() = Some(prompt.to_string());
            *self.images.lock().unwrap() = Some(Vec::new());
            Ok(TurnTextOutcome { text: "ok".into() })
        }

        async fn run_turn_with_images(
            &self,
            prompt: &str,
            image_paths: &[PathBuf],
            _cancel: tokio_util::sync::CancellationToken,
        ) -> Result<TurnTextOutcome, String> {
            *self.prompt.lock().unwrap() = Some(prompt.to_string());
            *self.images.lock().unwrap() = Some(image_paths.to_vec());
            Ok(TurnTextOutcome { text: "ok".into() })
        }
    }

    /// Text-only fake: implements ONLY `run_turn` and relies on the trait's
    /// default `run_turn_with_images`. Proves the no-image submit still drives
    /// the text-only path unchanged (default delegation = byte-identical).
    struct TextOnlyOrch(&'static str);

    #[async_trait::async_trait]
    impl ConversationOrchestratorTrait for TextOnlyOrch {
        async fn run_turn(
            &self,
            _prompt: &str,
            _cancel: tokio_util::sync::CancellationToken,
        ) -> Result<TurnTextOutcome, String> {
            Ok(TurnTextOutcome {
                text: self.0.to_string(),
            })
        }
    }

    #[tokio::test]
    async fn submit_forwards_captured_image_paths_and_clears_registry() {
        let mut st = s();
        // Simulate a Finder drag-paste of two image paths: records two
        // attachments in the paste registry (the `[Image #N]` refs land in the
        // prompt, the source paths in `st.paste`).
        st.paste = process_paste("/tmp/a.png /tmp/b.jpg", st.paste.clone()).state;
        assert_eq!(st.paste.attachments.len(), 2);

        let orch = RecordingOrch::default();
        let disp = dispatcher();
        run_one_submit(&mut st, "look [Image #1] [Image #2]", &orch, &disp).await;

        // The image-capable entry received the two captured paths.
        let images = orch.images.lock().unwrap().clone().expect("a turn ran");
        assert_eq!(
            images,
            vec![PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.jpg")],
            "captured paste image paths must be forwarded to the model"
        );
        // Registry consumed on submit → the next turn starts fresh.
        assert_eq!(
            st.paste,
            PasteState::default(),
            "paste registry must be cleared on submit so images don't leak"
        );
        assert!(st.in_flight_turn.is_none(), "in-flight turn cleared");
        // The assistant reply was pushed (turn ran end-to-end).
        assert!(matches!(
            st.messages.last(),
            Some(RenderedMessage::AssistantText { body, .. }) if body == "ok"
        ));
    }

    #[tokio::test]
    async fn submit_without_images_forwards_empty_list() {
        let mut st = s();
        assert!(st.paste.attachments.is_empty());

        let orch = RecordingOrch::default();
        let disp = dispatcher();
        run_one_submit(&mut st, "just text", &orch, &disp).await;

        let images = orch.images.lock().unwrap().clone().expect("a turn ran");
        assert!(images.is_empty(), "no captured images → empty path list");
        // Registry was already default; remains default (no observable change).
        assert_eq!(st.paste, PasteState::default());
    }

    #[tokio::test]
    async fn submit_expands_pasted_text_pill_back_to_full_content() {
        // (PIC-05) The model must see the ORIGINAL pasted text, not the
        // `[Pasted text #N]` placeholder — unlike images, pasted text has no
        // separate attachment channel.
        use crate::components::prompt_input::PASTE_THRESHOLD;
        let mut st = s();
        let original = "y".repeat(PASTE_THRESHOLD + 1);
        st.paste = process_paste(&original, st.paste.clone()).state;
        assert_eq!(st.paste.pasted_texts, vec![(1, original.clone())]);

        let orch = RecordingOrch::default();
        let disp = dispatcher();
        run_one_submit(&mut st, "before [Pasted text #1] after", &orch, &disp).await;

        let sent = orch.prompt.lock().unwrap().clone().expect("a turn ran");
        assert_eq!(sent, format!("before {original} after"));
        assert!(
            st.paste.pasted_texts.is_empty(),
            "pasted-text registry drained on submit"
        );
    }

    #[tokio::test]
    async fn submit_without_images_is_byte_identical_text_only_path() {
        // A fake that ONLY implements `run_turn` exercises the default
        // `run_turn_with_images`, proving the no-image submit is unchanged from
        // the prior text-only behavior (mirrors `behavior_run_turn.rs`).
        let mut st = s();
        let orch = TextOnlyOrch("Hello!");
        let disp = dispatcher();
        run_one_submit(&mut st, "hi", &orch, &disp).await;

        assert_eq!(st.messages.len(), 1);
        assert!(matches!(
            &st.messages[0],
            RenderedMessage::AssistantText { body, .. } if body == "Hello!"
        ));
        assert!(st.in_flight_turn.is_none());
    }
}

/// Tests for the `connect_picker_popup_lines` helper (T2b): verifies that the
/// detail section is appended after the provider list rows, contains the
/// highlighted provider's models, connected state, and sign-in method(s).
#[cfg(test)]
mod connect_picker_detail_tests {
    use super::*;
    use crate::components::picker_popup::PopupLine;
    use crate::screens::connect_picker::ConnectPickerState;
    use std::collections::BTreeMap;

    fn make_auth() -> BTreeMap<String, String> {
        [("anthropic", "api_key"), ("openai", "api_key")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn make_model_providers() -> BTreeMap<String, (String, String)> {
        [
            ("claude-opus-4-8", ("anthropic", "Anthropic")),
            ("claude-sonnet-4-6", ("anthropic", "Anthropic")),
            ("gpt-5", ("openai", "OpenAI")),
        ]
        .into_iter()
        .map(|(m, (p, l))| (m.to_string(), (p.to_string(), l.to_string())))
        .collect()
    }

    /// Flatten popup lines into a single string for easy assertion.
    fn text_of(lines: &[PopupLine]) -> String {
        lines
            .iter()
            .map(|l| match l {
                PopupLine::Header(h) => h.clone(),
                PopupLine::Item { label, .. } => label.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn detail_section_present_for_highlighted_provider() {
        let c = ConnectPickerState::from_connectable(&make_auth(), &BTreeMap::new());
        let mp = make_model_providers();
        let avail = BTreeMap::new();
        let auth_methods = make_auth();

        let lines = connect_picker_popup_lines(&c, &mp, &avail, &auth_methods);
        let text = text_of(&lines);

        // Detail section must appear
        assert!(text.contains("Models:"), "missing 'Models:' in:\n{text}");
        assert!(
            text.contains("connected") || text.contains("not connected"),
            "missing connection state in:\n{text}"
        );
        // Anthropic models only (not another provider's model)
        assert!(
            text.contains("claude-opus-4-8") || text.contains("claude-sonnet-4-6"),
            "missing anthropic model in:\n{text}"
        );
        assert!(!text.contains("gpt-5"), "openai model should NOT appear:\n{text}");
    }

    #[test]
    fn detail_absent_when_picker_is_empty() {
        let c = ConnectPickerState::default(); // no rows → highlighted_provider_id() = None
        let lines =
            connect_picker_popup_lines(&c, &BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new());
        assert!(lines.is_empty(), "empty picker should yield empty lines, got: {lines:?}");
    }

    #[test]
    fn list_rows_precede_detail_section() {
        let c = ConnectPickerState::from_connectable(&make_auth(), &BTreeMap::new());
        let lines =
            connect_picker_popup_lines(&c, &BTreeMap::new(), &BTreeMap::new(), &make_auth());

        // At least one Header (group) + at least 2 Item rows (the two providers)
        // before the separator Header + detail Items.
        let headers: Vec<_> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l, PopupLine::Header(_)))
            .collect();
        assert!(headers.len() >= 2, "expected ≥2 headers (group + separator): {headers:?}");

        // The separator is a Header("") — it must appear after at least one Item.
        let sep_idx = lines
            .iter()
            .position(|l| matches!(l, PopupLine::Header(h) if h.is_empty()))
            .expect("blank separator Header not found");
        let items_before_sep = lines[..sep_idx]
            .iter()
            .filter(|l| matches!(l, PopupLine::Item { .. }))
            .count();
        assert!(
            items_before_sep >= 1,
            "expected list items before the detail separator, got {items_before_sep}"
        );
    }

    #[test]
    fn anthropic_shows_both_oauth_and_api_key_sign_in() {
        // Anthropic is a dual-method provider; the detail "Sign in:" line must
        // mention both methods regardless of what auth_methods map says.
        let mut auth = BTreeMap::new();
        auth.insert("anthropic".to_string(), "api_key".to_string());
        let c = ConnectPickerState::from_connectable(&auth, &BTreeMap::new());
        let lines = connect_picker_popup_lines(&c, &BTreeMap::new(), &BTreeMap::new(), &auth);
        let text = text_of(&lines);
        assert!(
            text.contains("Pro/Max") && text.contains("API key"),
            "anthropic detail must list both sign-in methods:\n{text}"
        );
    }
}
