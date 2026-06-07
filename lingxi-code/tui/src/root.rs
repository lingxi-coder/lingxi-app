//! Iocraft root component — owns the reactive runtime.
//!
//! Before this module existed, `session.rs` ran a hand-rolled `tokio::select!`
//! loop, called `app.render()` to construct an element tree, and dropped it.
//! Nothing painted to the terminal — M6-01..M6-03 components rendered only
//! inside unit tests.
//!
//! This module mounts iocraft's actual reconciler. Architecture choice **B**
//! from the M6-04 brief: iocraft's `Element::fullscreen().await` owns the
//! main loop (raw mode + alt screen + event pump), and external mpsc events
//! (the orchestrator bridge channel) are fed into the component tree via
//! `use_future` + a shared `Arc<Mutex<AppState>>` + a `use_state` redraw tick.
//!
//! Iocraft 0.8.3 ships its own crossterm-0.29 re-exports (`iocraft::KeyEvent`,
//! `iocraft::KeyCode`, `iocraft::KeyModifiers`). The rest of the workspace
//! is pinned to crossterm 0.28 — so the keymap module's `map_key` cannot
//! consume iocraft events directly. We handle keys inline here against the
//! iocraft re-exports and call into `app::dispatch` with the workspace's
//! `KeyAction` enum (which is crossterm-version-agnostic).

use std::sync::Arc;
use std::time::{Duration, Instant};

use iocraft::prelude::*;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::app::{dispatch, scroll_with_viewport};
use crate::components::prompt_input::completion::CompletionKeyOutcome;
use crate::components::prompt_input::palette::PaletteKeyOutcome;
use crate::events::keymap::{CursorMove, KeyAction, ScrollDir};
use crate::events::orchestrator_bridge::TurnEvent;
use crate::state::AppState;
use crate::streaming::apply_event;
use crate::telemetry::{
    FIRST_RENDER, RESIZE, SESSION_ENDED, STREAMING_RENDER_ENDED, STREAMING_RENDER_STARTED,
};

/// Slot holding the bridge receiver. Wrapped in `Mutex<Option<...>>` so the
/// root component can take it once (first render) and the `use_future`
/// owns it thereafter. `Arc` so the same slot is shared between the caller
/// constructing the props and the closure spawned inside `use_future`.
pub type BridgeRxSlot = Arc<std::sync::Mutex<Option<UnboundedReceiver<TurnEvent>>>>;

/// (M9-05) Slot carrying the `MultiAgentEvent` receiver, mirroring
/// [`BridgeRxSlot`]. The second render-loop pump `take()`s it once (first
/// render) and owns it thereafter, draining it into `apply_multiagent_event`.
pub type MultiAgentRxSlot =
    Arc<std::sync::Mutex<Option<UnboundedReceiver<crate::multiagent::MultiAgentEvent>>>>;

/// Props for the iocraft root component.
#[derive(Default, Props)]
pub struct TuiRootProps {
    /// Shared state, mutated by the bridge pump task + key handlers, read
    /// by the render method.
    pub state: Option<Arc<Mutex<AppState>>>,
    /// Bridge receiver slot (taken once in the first `use_future`).
    pub bridge_rx: Option<BridgeRxSlot>,
    /// External cancellation token. When fired, the root flips
    /// `state.should_exit` and exits.
    pub cancel: Option<CancellationToken>,
    /// Process-local session id (forwarded to telemetry events).
    pub session_id: Option<protocol::SessionId>,
    /// Wall-clock instant the session began (for `FIRST_RENDER` latency).
    pub started_at: Option<Instant>,
    /// (M7-13 review) Orchestrator handle used by the async Settings open pump
    /// to read `SettingsData::snapshot(handle, eff)` (status + cost). `None`
    /// (e.g. the resume picker, smoke gates) disables the open pump — Settings
    /// is unreachable without a handle, which is correct for those bridge-less
    /// mounts.
    pub orchestrator: Option<Arc<dyn traits::OrchestratorHandle>>,
    /// (M9-05) `MultiAgentEvent` receiver slot, drained by the second pump (a
    /// `use_future` mirroring the bridge pump). `None` (resume picker, smoke
    /// gates, bridge-less mounts) makes the pump inert — it returns immediately.
    pub multiagent_rx: Option<MultiAgentRxSlot>,
    /// (M9-05) Live multi-agent feed (the desktop `PollerFeed` over the real
    /// `TaskRegistryHandle`). The ticker calls `pump_once(feed, tx)` each tick;
    /// the produced events flow back through `multiagent_rx` into `AppState`.
    /// `None` skips the ticker poll (no live task surface for that mount).
    pub multiagent_feed: Option<Arc<dyn crate::multiagent::MultiAgentFeed>>,
    /// (M9-05) Sender paired with `multiagent_rx`. The ticker pushes the feed's
    /// events onto it via `pump_once`. `None` skips the ticker poll.
    pub multiagent_tx:
        Option<tokio::sync::mpsc::UnboundedSender<crate::multiagent::MultiAgentEvent>>,
}

/// Map an iocraft `KeyEvent` into the workspace's `KeyAction` enum.
///
/// Mirrors `crate::events::keymap::map_key` (which consumes crossterm-0.28
/// events) but operates on iocraft's crossterm-0.29 re-exports. The mapping
/// table is intentionally kept in sync byte-for-byte with `keymap::map_key`;
/// see that file for the canonical bindings.
#[allow(clippy::too_many_lines)]
fn map_iocraft_key(
    evt: &KeyEvent,
    prompt_empty: bool,
    focus_active: bool,
    multiline: bool,
) -> Option<KeyAction> {
    use KeyAction::{
        Backspace, Cancel, FocusToolStep, HistoryStep, InsertChar, InsertNewline, MoveCursor,
        MoveCursorVertical, ScrollStep, Submit, ToggleExpanded,
    };
    if focus_active {
        match (&evt.code, evt.modifiers) {
            (KeyCode::Up, _) => return Some(FocusToolStep(-1)),
            (KeyCode::Down, _) => return Some(FocusToolStep(1)),
            (KeyCode::Char('e'), m) if m == KeyModifiers::NONE && prompt_empty => {
                return Some(ToggleExpanded);
            }
            (KeyCode::Enter, _) if prompt_empty => return Some(ToggleExpanded),
            _ => {}
        }
    }
    match (&evt.code, evt.modifiers) {
        (KeyCode::Enter, m) if m.contains(KeyModifiers::SHIFT) => Some(InsertNewline),
        (KeyCode::Enter, _) => Some(Submit),
        (KeyCode::Backspace, _) => Some(Backspace),
        (KeyCode::Char('c'), m) if m == KeyModifiers::CONTROL => Some(Cancel),
        (KeyCode::Left, _) => Some(MoveCursor(CursorMove::Left)),
        (KeyCode::Right, _) => Some(MoveCursor(CursorMove::Right)),
        (KeyCode::Home, _) => Some(MoveCursor(CursorMove::Home)),
        (KeyCode::End, _) => Some(MoveCursor(CursorMove::End)),
        // Multi-line buffers move the cursor vertically; single-line buffers
        // keep the M6 history-step behaviour.
        (KeyCode::Up, _) if multiline => Some(MoveCursorVertical(-1)),
        (KeyCode::Down, _) if multiline => Some(MoveCursorVertical(1)),
        (KeyCode::Up, _) => Some(HistoryStep(-1)),
        (KeyCode::Down, _) => Some(HistoryStep(1)),
        (KeyCode::PageUp, _) => Some(ScrollStep(ScrollDir::PageUp)),
        (KeyCode::PageDown, _) => Some(ScrollStep(ScrollDir::PageDown)),
        // Vim-style nav only when prompt is empty.
        (KeyCode::Char('j'), m) if m == KeyModifiers::NONE && prompt_empty => {
            Some(ScrollStep(ScrollDir::LineDown))
        }
        (KeyCode::Char('k'), m) if m == KeyModifiers::NONE && prompt_empty => {
            Some(ScrollStep(ScrollDir::LineUp))
        }
        (KeyCode::Char('g'), m) if m == KeyModifiers::NONE && prompt_empty => {
            Some(ScrollStep(ScrollDir::Top))
        }
        (KeyCode::Char('G'), m) if m == KeyModifiers::SHIFT && prompt_empty => {
            Some(ScrollStep(ScrollDir::Bottom))
        }
        // (M7-13 review) Ctrl-G opens the Settings screen (Config tab). Mirrors
        // `keymap::map_key_ml`. Placed before the printable-char catch-all; the
        // CONTROL modifier means it never collides with the vim-nav `g`
        // (`KeyModifiers::NONE`). The action only RAISES `pending_open_settings`
        // in `dispatch` — the async open pump does the snapshot + open.
        (KeyCode::Char('g'), m) if m.contains(KeyModifiers::CONTROL) => Some(
            KeyAction::OpenSettings(crate::screens::settings::SettingsTab::Config),
        ),
        (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
            Some(InsertChar(*c))
        }
        _ => None,
    }
}

/// (M7-08 review) Is this key the `KeyAction::ToggleVim` binding (Ctrl-Alt-V)?
///
/// The toggle must be modal-independent: it flips `vim_enabled` regardless of
/// vim mode (Normal/Insert) or whether vim is even enabled. `handle_live_key`
/// checks this AFTER the permission focus-trap (priority 1) and the overlay
/// focus-trap (priority 3) but BEFORE the priority-4 vim branch, so a pending
/// permission or an open overlay still wins — yet Ctrl-Alt-V toggles vim off
/// from ANY vim mode. The live `map_iocraft_key` deliberately does NOT map
/// `ToggleVim` (an open overlay passes a printable through to the editor, and
/// routing the toggle there would let it preempt the overlay).
fn is_toggle_vim_key(k: &KeyEvent) -> bool {
    matches!(k.code, KeyCode::Char('v'))
        && k.modifiers.contains(KeyModifiers::CONTROL)
        && k.modifiers.contains(KeyModifiers::ALT)
}

/// Convert an iocraft (crossterm-0.29) `KeyEvent` into a workspace
/// (crossterm-0.28) `KeyEvent`, as consumed by `keymap::handle_key` and the
/// per-dialog `handle_key` helpers.
///
/// Only the key codes and modifiers the dialog state machines and `map_key`
/// actually inspect are mapped (`Char`, `Enter`, `Backspace`, `Esc`, arrows,
/// `Home`/`End`, `PageUp`/`PageDown`, `Tab`/`BackTab`, plus `CONTROL`/`SHIFT`
/// modifiers). Anything unmapped becomes `KeyCode::Null`, which every dialog
/// handler treats as an inert no-op — so an exotic key can never accidentally
/// resolve a permission dialog.
fn iocraft_to_crossterm028_key(k: &KeyEvent) -> crossterm::event::KeyEvent {
    use crossterm::event::{KeyCode as Ct, KeyEvent as CtEvent, KeyModifiers as CtMods};

    let code = match k.code {
        KeyCode::Char(c) => Ct::Char(c),
        KeyCode::Enter => Ct::Enter,
        KeyCode::Backspace => Ct::Backspace,
        KeyCode::Esc => Ct::Esc,
        KeyCode::Up => Ct::Up,
        KeyCode::Down => Ct::Down,
        KeyCode::Left => Ct::Left,
        KeyCode::Right => Ct::Right,
        KeyCode::Home => Ct::Home,
        KeyCode::End => Ct::End,
        KeyCode::PageUp => Ct::PageUp,
        KeyCode::PageDown => Ct::PageDown,
        KeyCode::Tab => Ct::Tab,
        KeyCode::BackTab => Ct::BackTab,
        KeyCode::Delete => Ct::Delete,
        KeyCode::Insert => Ct::Insert,
        // Codes the keymap / dialogs never act on collapse to Null (no-op).
        _ => Ct::Null,
    };

    // Preserve the modifiers the keymap inspects. Iocraft's KeyModifiers
    // share the same CONTROL/SHIFT/ALT bit semantics as crossterm-0.28.
    let mut mods = CtMods::NONE;
    if k.modifiers.contains(KeyModifiers::CONTROL) {
        mods |= CtMods::CONTROL;
    }
    if k.modifiers.contains(KeyModifiers::SHIFT) {
        mods |= CtMods::SHIFT;
    }
    if k.modifiers.contains(KeyModifiers::ALT) {
        mods |= CtMods::ALT;
    }

    CtEvent::new(code, mods)
}

/// Route a single LIVE key event into the `AppState`.
///
/// This is THE function the live `use_terminal_events` closure invokes, and
/// it is the seam between M6-04's iocraft mount and M6-05's permission
/// dialogs. The live mount receives an `iocraft::KeyEvent` (crossterm-0.29
/// re-export), whereas `keymap::handle_key` and the per-dialog handlers
/// consume crossterm-0.28 events. We bridge the skew here.
///
/// **Focus trap (M6-05 final-review fix):** when a permission dialog is open
/// (`state.pending_permission.is_some()`), the key is converted and routed
/// through `keymap::handle_key`, which owns the dialog state machines and
/// fires the `resp_tx` oneshot back to the orchestrator's
/// `TuiPermissionGate`. This guarantees the prompt buffer stays untouched and
/// the dialog actually resolves in the real binary. Before this fix the live
/// path went straight to `map_iocraft_key` + `dispatch`, so dialog keystrokes
/// silently mutated the hidden prompt and the gate await hung forever.
///
/// When no dialog is open, this falls through to the M6-02/M6-04 pipeline
/// (`map_iocraft_key` → `dispatch` / `scroll_with_viewport`).
///
/// Apply a `VimEffect` to the prompt buffer + cursor in `AppState` (M7-08).
fn apply_vim_effect(st: &mut AppState, effect: crate::components::prompt_input::vim::VimEffect) {
    use crate::components::prompt_input::vim::VimEffect;
    match effect {
        VimEffect::Move(off) => {
            st.prompt_cursor = off.min(st.prompt_text.len());
        }
        VimEffect::Edit { text, cursor } => {
            st.prompt_text = text;
            st.prompt_cursor = cursor.min(st.prompt_text.len());
        }
        VimEffect::None => {}
    }
}

/// Route a key to the active full-page screen. Dispatches PER-VARIANT on the
/// active `Screen` (M7-12): each screen owns its own key semantics while the
/// shared contract — Esc/`q` close, no key leaks to `PromptInput` — holds for
/// every variant.
///
/// - `Screen::Doctor` (M7-11) is read-only: Esc / `q` (no modifiers) close it;
///   every other key is swallowed (the M7-11 no-leak guarantee). Byte-identical
///   to the original M7-11 behavior.
/// - `Screen::Resume` (M7-12) is the FIRST interactive screen: Up/Down select,
///   Enter resumes the selected uuid, Esc/`q` cancel. We bridge the iocraft
///   (crossterm-0.29) `KeyEvent` to crossterm-0.28 and run the pure
///   `resume::handle_resume_key`, then act on its `ResumeOutcome`:
///     - `Stay`   → keep the screen open (selection moved or inert key).
///     - `Resume` → record `resume_request` + flip `should_exit` so the mount
///       unwinds back to the CLI, which loads the chosen session.
///     - `Cancel` → close the screen (back to REPL).
/// - `Screen::Settings` (M7-13) is the tab overlay: Left/Right/`h`/`l`/Tab
///   cycle the four tabs (wrap-around), Esc/`q` close, and `e`/Enter on the
///   Config tab raises `pending_config_edit` for the bridge's `$EDITOR`
///   handoff (§4 R7 — the ONLY settings write). All via the pure
///   `settings::apply_settings_key` reducer.
///
/// M7-14 adds a further `match` arm here for its screen.
#[allow(clippy::too_many_lines, reason = "flat per-screen match dispatcher; one arm per Screen variant")]
fn handle_screen_key(st: &mut AppState, k: &KeyEvent) {
    use crate::screens::Screen;
    match &mut st.active_screen {
        Some(Screen::Doctor(_)) => {
            // (M7-11) Read-only screen: Esc / `q` close; everything else inert.
            match k.code {
                // (M7-16) `close_screen` emits `tengu_tui_screen_closed` on the
                // real `Some → None` transition, so the close telemetry is wired
                // through the shared close path (not duplicated per screen).
                KeyCode::Esc => st.close_screen(),
                KeyCode::Char('q') if k.modifiers == KeyModifiers::NONE => st.close_screen(),
                _ => {}
            }
        }
        Some(Screen::Resume(state)) => {
            use crate::screens::resume::{handle_resume_key, ResumeOutcome};
            let ct = iocraft_to_crossterm028_key(k);
            match handle_resume_key(state, ct) {
                ResumeOutcome::Stay => { /* keep the screen open */ }
                ResumeOutcome::Resume(uuid) => {
                    st.resume_request = Some(uuid);
                    st.close_screen();
                    st.should_exit = true; // hand control back to the CLI
                }
                ResumeOutcome::Cancel => st.close_screen(),
            }
        }
        Some(Screen::Settings(state)) => {
            // (M7-13) Tab nav (Left/Right/h/l/Tab/BackTab), Esc/`q` close, and
            // the Config tab's `e`/Enter $EDITOR handoff — all via the pure
            // `apply_settings_key` reducer, mirroring the Resume arm.
            use crate::screens::settings::{apply_settings_key, SettingsOutcome};
            let ct = iocraft_to_crossterm028_key(k);
            match apply_settings_key(state, ct) {
                SettingsOutcome::Stay => { /* tab moved or inert; keep open */ }
                SettingsOutcome::Close => st.close_screen(),
                SettingsOutcome::EditConfig => {
                    // §4 R7: the ONLY settings write is the $EDITOR handoff. We
                    // CANNOT `.await edit_config_file()` here (sync key path),
                    // so raise the request; the bridge pump awaits it + re-snaps
                    // (wired by M7-16). Screen stays open meanwhile.
                    st.pending_config_edit = true;
                }
            }
        }
        Some(Screen::Memory(state)) => {
            // (M7-14) Pick a CLAUDE.md tier, edit it inline, save through the
            // M3 store. Tiers are re-resolved synchronously each key from
            // `hierarchy::walk` (no async open pump). The pure
            // `handle_memory_key` reducer drives selection / editing; we act on
            // its `MemoryAction`:
            //   - CloseScreen  → back to REPL.
            //   - Save{path,body} → atomic write to the SAME HierarchyEntry path
            //     (§4 R7 — the ONLY write); Esc never produces Save, so a cancel
            //     never writes.
            //   - BackToSelector / None → keep the screen open.
            use crate::screens::memory::{
                handle_memory_key, memory_tiers, save_tier_body, MemoryAction,
            };
            let ct = iocraft_to_crossterm028_key(k);
            let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
            let tiers = memory_tiers(&st.status.cwd, &home);
            match handle_memory_key(state, &tiers, ct) {
                MemoryAction::CloseScreen => st.close_screen(),
                MemoryAction::Save { path, body } => match save_tier_body(&path, &body) {
                    Ok(()) => {
                        state.dirty = false;
                        state.status = Some(format!("Saved {}", path.display()));
                    }
                    Err(e) => {
                        state.status = Some(format!("Could not save memory: {e}"));
                    }
                },
                MemoryAction::BackToSelector | MemoryAction::None => {}
            }
        }
        Some(Screen::Theme(_)) => {
            // (M7-15) Theme picker. Up/Down LIVE-PREVIEW the highlighted theme
            // (writing `st.theme` so the whole UI re-renders), Enter commits +
            // best-effort persists, Esc/`q` cancels and restores the prior
            // theme. `theme_picker_handle_key` needs BOTH `&mut ThemePickerState`
            // (inside the `Screen` variant) AND `&mut AppState` — a
            // double-mut-borrow if taken in place. We TAKE the screen out of
            // `active_screen` first (mirroring the M6 `pending_permission`
            // focus-trap discipline of owning the state for the duration), run
            // the pure reducer, then put it back only when the screen stays
            // open. No parallel key path: this is the sole priority-2 entry.
            use crate::screens::theme::{theme_picker_handle_key, ThemePickerOutcome};
            let Some(Screen::Theme(mut picker)) = st.active_screen.take() else {
                return;
            };
            let ct = iocraft_to_crossterm028_key(k);
            match theme_picker_handle_key(&mut picker, st, ct) {
                ThemePickerOutcome::Stay => {
                    // Preview applied to `st.theme`; keep the screen open.
                    st.active_screen = Some(Screen::Theme(picker));
                }
                ThemePickerOutcome::Commit => {
                    // `set_theme` already applied; best-effort persist, then
                    // close (screen already taken out by `.take()` above).
                    crate::theme_persist::save_theme_setting(st.theme_setting);
                }
                ThemePickerOutcome::Cancel => {
                    // Prior theme/setting restored by the reducer; close (screen
                    // already taken out).
                }
            }
        }
        Some(Screen::BackgroundTasks(state)) => {
            // (M9-05) Background-tasks dialog. Pure list↔detail reducer over the
            // LIVE task ids (read from `AppState.multiagent.tasks`, kept fresh by
            // the MultiAgent pump). Mirrors the Memory/Resume arms: bridge the
            // iocraft key to crossterm-0.28, run the reducer, act on its outcome.
            //   - Close       → back to REPL (the shared `close_screen` path).
            //   - Stay         → selection/mode changed or inert; keep open.
            //   - OpenedDetail → entered a task's detail; the ticker pump (Task 7)
            //     drives the output tail, so there is nothing to do synchronously.
            use crate::screens::background_tasks::{
                handle_background_tasks_key, TaskDialogOutcome,
            };
            let ids: Vec<String> = st
                .multiagent
                .tasks
                .iter()
                .map(|t| t.task_id.clone())
                .collect();
            let ct = iocraft_to_crossterm028_key(k);
            match handle_background_tasks_key(state, &ids, ct.code) {
                TaskDialogOutcome::Close => st.close_screen(),
                TaskDialogOutcome::Stay => { /* keep the screen open */ }
                TaskDialogOutcome::OpenedDetail(_id) => {
                    // Tailing is driven by the ticker pump (Task 7); nothing here.
                }
            }
        }
        Some(Screen::Agents(state)) => {
            // (M9-08) Agent-discovery screen. Pure list↔detail reducer over the
            // agent catalog rows (carried in the `Screen::Agents` variant). Mirrors
            // the BackgroundTasks arm: bridge the iocraft key to crossterm-0.28,
            // run the reducer, act on its outcome.
            //   - Close → back to REPL (the shared `close_screen` path).
            //   - Stay  → selection/mode changed or inert; keep open.
            use crate::screens::agents::{handle_agents_key, AgentsOutcome};
            let ct_key = iocraft_to_crossterm028_key(k);
            match handle_agents_key(state, ct_key.code) {
                AgentsOutcome::Close => st.close_screen(),
                AgentsOutcome::Stay => {}
            }
        }
        Some(Screen::Mcp(state)) => {
            // Read-only MCP-server viewer. Pure list↔detail reducer; mirrors the
            // Agents arm. Close → REPL (shared `close_screen`); Stay → keep open.
            use crate::screens::mcp::{handle_mcp_key, McpOutcome};
            let ct_key = iocraft_to_crossterm028_key(k);
            match handle_mcp_key(state, ct_key.code) {
                McpOutcome::Close => st.close_screen(),
                McpOutcome::Stay => {}
            }
        }
        Some(Screen::Hooks(state)) => {
            // Read-only hooks viewer. Pure list↔detail reducer; mirrors the
            // Agents/Mcp arm.
            use crate::screens::hooks::{handle_hooks_key, HooksOutcome};
            let ct_key = iocraft_to_crossterm028_key(k);
            match handle_hooks_key(state, ct_key.code) {
                HooksOutcome::Close => st.close_screen(),
                HooksOutcome::Stay => {}
            }
        }
        Some(Screen::Model(state)) => {
            // Model picker. The SYNC key path can't `.await switch_model`, so on
            // Commit it raises `pending_switch_model` (the async
            // `pump_switch_model` performs the write + refreshes the status line)
            // and closes; Cancel just closes. Stay keeps the highlight.
            use crate::screens::model::{handle_model_key, ModelOutcome};
            let ct_key = iocraft_to_crossterm028_key(k);
            match handle_model_key(state, ct_key.code) {
                ModelOutcome::Commit(model) => {
                    st.pending_switch_model = Some(model);
                    st.close_screen();
                }
                ModelOutcome::Cancel => st.close_screen(),
                ModelOutcome::Stay => {}
            }
        }
        Some(Screen::Skills(state)) => {
            // (M9-09) Skill-registry viewer (read-only). The pure
            // `handle_skills_key` reducer first delegates scroll keys to the
            // embedded `ScrollState`, then closes on Esc / bare `q`. It needs
            // the FULL `KeyEvent` (for the `q`-no-modifier guard + the scroll
            // bindings), so we pass the bridged crossterm-0.28 event, mirroring
            // the Resume/Theme arms.
            //   - Close → back to REPL (the shared `close_screen` path).
            //   - Stay  → scrolled or inert; keep open.
            use crate::screens::skills::{handle_skills_key, SkillsOutcome};
            let ct = iocraft_to_crossterm028_key(k);
            match handle_skills_key(state, ct) {
                SkillsOutcome::Close => st.close_screen(),
                SkillsOutcome::Stay => {}
            }
        }
        Some(Screen::Stats(state)) => {
            // (M9-10) Usage-stats screen (read-only). The pure `handle_stats_key`
            // reducer toggles the active tab on Tab/Shift-Tab (re-anchoring the
            // embedded `ScrollState`), delegates scroll keys to it, and closes on
            // Esc / bare `q`. It needs the FULL `KeyEvent` (the `q`-no-modifier
            // guard + the scroll bindings), so we pass the bridged crossterm-0.28
            // event, mirroring the Skills arm.
            //   - Close → back to REPL (the shared `close_screen` path).
            //   - Stay  → tab switched / scrolled / inert; keep open.
            use crate::screens::stats::{handle_stats_key, StatsOutcome};
            let ct = iocraft_to_crossterm028_key(k);
            match handle_stats_key(state, ct) {
                StatsOutcome::Close => st.close_screen(),
                StatsOutcome::Stay => {}
            }
        }
        Some(Screen::Help(state)) => {
            // The `/help` shortcuts + slash-command viewer (read-only). The pure
            // `handle_help_key` reducer delegates scroll keys to the embedded
            // `ScrollState`, then closes on Esc / bare `q`. It needs the FULL
            // `KeyEvent` (for the `q`-no-modifier guard + the scroll bindings),
            // so we pass the bridged crossterm-0.28 event, mirroring the
            // Skills/Stats arms.
            //   - Close → back to REPL (the shared `close_screen` path).
            //   - Stay  → scrolled or inert; keep open.
            use crate::screens::help::{handle_help_key, HelpOutcome};
            let ct = iocraft_to_crossterm028_key(k);
            match handle_help_key(state, ct) {
                HelpOutcome::Close => st.close_screen(),
                HelpOutcome::Stay => {}
            }
        }
        None => {}
    }
}

/// `viewport` is the scrollback viewport height (rows minus reserved chrome).
#[allow(clippy::too_many_lines)]
pub fn handle_live_key(st: &mut AppState, k: &KeyEvent, viewport: usize) {
    // === Priority 1: permission focus-trap (M6-05). A permission dialog owns
    // all keys while open — it MUST win even over an open palette/completion
    // overlay (priority 3 below), so it returns first. ===
    if st.pending_permission.is_some() {
        let ct_key = iocraft_to_crossterm028_key(k);
        let _ = crate::events::keymap::handle_key(st, ct_key);
        return;
    }
    // === end priority 1 ===

    // === PRIORITY 2: a full-page screen owns all keys while open (M7-11). ===
    // Priority order (parent spec §2.5): permission (1, above) → screen (2,
    // here) → input/scroll (below). The permission check above STILL fires
    // first and returns, so a screen can never steal a permission key. When a
    // screen is active it is MODAL: `handle_screen_key` consumes the key
    // (Esc/`q` close; everything else is swallowed) and we return before the
    // history-search / palette / completion overlays (priority 3) or vim
    // (priority 4) ever run — no key leaks to `PromptInput`. Reused by
    // M7-12/13/14 (they add `match`-on-`Screen` arms in `handle_screen_key`).
    if st.active_screen.is_some() {
        handle_screen_key(st, k);
        return;
    }
    // === end priority 2 ===

    // === Priority 3 (input overlay A): Ctrl-R history search (M7-10). When the
    // overlay is open it owns EVERY key until Enter/Esc — exactly the focus-trap
    // discipline the permission dialog (priority 1) established. It sits in the
    // SAME priority-3 region as the M7-07 palette/completion overlays and is
    // MUTUALLY EXCLUSIVE with them: opening Ctrl-R (the fall-through binding
    // below) only fires when no other overlay is active, and while
    // `history_search.is_some()` we return here before the palette/completion
    // branch ever runs. No parallel key path — this is the single dispatcher. ===
    if st.history_search.is_some() {
        use crate::components::prompt_input::{handle_history_search_key, HsKeyOutcome};
        let ct_key = iocraft_to_crossterm028_key(k);
        let hs = st.history_search.take().expect("checked is_some");
        match handle_history_search_key(hs, &ct_key, &st.history) {
            HsKeyOutcome::Continue(next) => st.history_search = Some(next),
            HsKeyOutcome::Accept(text) => {
                st.prompt_cursor = text.len();
                st.prompt_text = text;
                st.history_cursor = None;
            }
            HsKeyOutcome::Cancel(prompt, cursor) => {
                st.prompt_text = prompt;
                st.prompt_cursor = cursor;
            }
        }
        return;
    }
    // === end priority 3 (input overlay A) ===

    // === Priority 3 (input overlay C): message search/jump selector (M7-14).
    // Same focus-trap discipline as the history-search (3A) and palette/
    // completion (3B) overlays, in the SAME priority-3 region and MUTUALLY
    // EXCLUSIVE with them: the Ctrl-T open binding below only fires when no
    // other overlay is active, and while `message_selector.open` we return here
    // before the palette/completion branch runs. Permission (1) and screen (2)
    // still win above. No parallel key path — the single `handle_live_key`. ===
    if st.message_selector.open {
        use crate::components::message_selector::{
            export_transcript, handle_message_selector_key, message_line_offset, SelectorAction,
        };
        let ct_key = iocraft_to_crossterm028_key(k);
        let messages = st.messages.clone();
        match handle_message_selector_key(&mut st.message_selector, &messages, ct_key) {
            SelectorAction::Jump { message_index } => {
                // Set the line-based scroll offset (M7-03 model) so the chosen
                // message sits at the top of the viewport. Refresh the height
                // cache against the live width first so the offset is accurate.
                st.refresh_height_cache(st.viewport_width.max(1));
                let cache = st.height_cache.clone();
                st.scroll_offset = message_line_offset(&messages, &cache, message_index, viewport);
            }
            // (M7-14 review) The export flow confirmed: run the actual write
            // here (the live caller owns the messages + resolves the export
            // dir, keeping the key handler pure), then fold the outcome back
            // into the sub-state. §4 R10: `overwrite=false` on the first
            // attempt → `Exists` arms the overwrite-confirm prompt (no silent
            // clobber); `overwrite=true` only after the user pressed `y`.
            SelectorAction::Export { overwrite } => {
                let dir = st.message_selector.resolved_export_dir();
                let filename = st.message_selector.export.filename.clone();
                let outcome = export_transcript(&messages, &dir, &filename, overwrite);
                st.message_selector.report_export(&outcome);
            }
            SelectorAction::Close | SelectorAction::None => {}
        }
        return;
    }
    // === end priority 3 (input overlay C) ===

    // === Priority 3 (input overlay B): palette / completion overlay focus-trap
    // (M7-07). While an
    // overlay is open it owns EVERY key until Esc/accept; consumed/navigation
    // keys `return` so they never reach the default editor path. PassThrough
    // falls through (e.g. a printable char re-runs the editor, then the tail
    // re-sync re-opens/refilters the overlay). Only one overlay is open at a
    // time (palette wins on `/`). ===
    if st.palette.open {
        match st.palette.handle_key(k.code) {
            PaletteKeyOutcome::Consumed | PaletteKeyOutcome::Dismiss => return,
            PaletteKeyOutcome::Accept(text) => {
                st.prompt_cursor = text.len();
                st.prompt_text = text;
                st.palette.sync_from_prompt(&st.prompt_text);
                return;
            }
            PaletteKeyOutcome::PassThrough => { /* fall through to default edit */ }
        }
    } else if st.completion.open {
        match st
            .completion
            .handle_key_with_prompt(k.code, &st.prompt_text, st.prompt_cursor)
        {
            CompletionKeyOutcome::Consumed | CompletionKeyOutcome::Dismiss => return,
            CompletionKeyOutcome::Accept {
                new_prompt,
                new_cursor,
            } => {
                st.prompt_text = new_prompt;
                st.prompt_cursor = new_cursor;
                let candidates = st.completion.candidates.clone();
                st.completion
                    .sync(&st.prompt_text, st.prompt_cursor, &candidates);
                return;
            }
            CompletionKeyOutcome::PassThrough => { /* fall through */ }
        }
    }
    // === end priority 3 ===

    // === Priority 3 open binding: Ctrl-R opens the history-search overlay
    // (M7-10). We reach here only when NO overlay/dialog is already active — the
    // permission trap (1) and the history-search trap (3A) returned above. We
    // additionally gate on palette/completion being closed so the three
    // priority-3 overlays stay MUTUALLY EXCLUSIVE (an open palette owns `r` as a
    // filter char; Ctrl-R does not preempt it). Opening snapshots the current
    // prompt so Esc can restore it. ===
    if !st.palette.open
        && !st.completion.open
        && matches!(k.code, KeyCode::Char('r'))
        && k.modifiers.contains(KeyModifiers::CONTROL)
    {
        st.history_search = Some(crate::components::prompt_input::hs_open(
            &st.prompt_text,
            st.prompt_cursor,
        ));
        return;
    }
    // === end Ctrl-R open binding ===

    // === Priority 3 open binding: Ctrl-T opens the message search/jump
    // selector (M7-14). Same mutual-exclusion gating as Ctrl-R: only fires when
    // no other priority-3 overlay (palette/completion/history-search) is open,
    // so the four overlays stay mutually exclusive. We refilter immediately so
    // the overlay shows the full scrollback on open. ===
    if !st.palette.open
        && !st.completion.open
        && st.history_search.is_none()
        && matches!(k.code, KeyCode::Char('t'))
        && k.modifiers.contains(KeyModifiers::CONTROL)
    {
        st.message_selector.open();
        let messages = st.messages.clone();
        st.message_selector.refilter_all(&messages);
        return;
    }
    // === end Ctrl-T open binding ===

    // === Priority 3 open binding: Shift+Down opens the background-tasks dialog
    // (M9-05, claude-code `BackgroundTaskStatus.tsx` ` · ↓ to view`). Mirrors
    // the Ctrl-R / Ctrl-T open bindings: it fires only from the normal editing
    // state — the permission (1) and screen (2) gates returned above, and we
    // gate on no priority-3 overlay (palette/completion/history-search/message-
    // selector) being open so the opener never preempts an overlay that owns the
    // key. (`active_screen` is already `None` here — the priority-2 gate above
    // returns whenever a screen is open — so opening can never clobber one.)
    // Seeds a fresh `BackgroundTasksState`; the dialog then browses the live
    // `AppState.multiagent.tasks` list. ===
    if !st.palette.open
        && !st.completion.open
        && st.history_search.is_none()
        && !st.message_selector.open
        && matches!(k.code, KeyCode::Down)
        && k.modifiers.contains(KeyModifiers::SHIFT)
    {
        st.active_screen = Some(crate::screens::Screen::BackgroundTasks(
            crate::screens::background_tasks::BackgroundTasksState::default(),
        ));
        crate::telemetry::screen_opened("background_tasks");
        return;
    }
    // === end Shift+Down open binding ===

    // === (M9-06) Teammate-view mode: Esc returns to the normal transcript.
    // Sits after the permission (1) + active_screen (2) + overlay (3) gates so
    // those still win. Runs before vim (3.5/4) and default editor input so the
    // Esc key is fully consumed and never leaks to the prompt buffer. ===
    if st.viewing_teammate.is_some() && k.code == KeyCode::Esc {
        st.leave_teammate_view();
        return;
    }
    // === end teammate-view Esc ===

    // === PRIORITY 3.5: vim toggle (M7-08 review). The Ctrl-Alt-V binding must
    // be modal-independent — it flips `vim_enabled` from ANY vim mode (Normal or
    // Insert) or when vim is off. It sits AFTER the permission (1) and overlay
    // (3) focus-traps so those still win: when an overlay is open it owns the
    // key (a printable `v` passes through to the editor, NOT the toggle), so we
    // gate on no overlay being open. Placed BEFORE the priority-4 vim branch so
    // Normal mode can no longer swallow the toggle. ===
    if !st.palette.open && !st.completion.open && is_toggle_vim_key(k) {
        let _ = dispatch(KeyAction::ToggleVim, st);
        return;
    }
    // === end priority 3.5 ===

    // === PRIORITY 4: vim input (M7-08), only when enabled. ===
    // Sits AFTER the M7-07 overlay focus-trap (priority 3) so palette/completion
    // still win, and gates the entire branch on `st.vim_enabled` so M6 default
    // editing is byte-identical when vim is off. `PassThrough` (Insert-mode
    // typing, Enter, Ctrl-C, etc.) falls through to the existing
    // `map_iocraft_key` + `dispatch` pipeline below — NOT a parallel key path.
    if st.vim_enabled {
        let ct_key = iocraft_to_crossterm028_key(k);
        let outcome = crate::components::prompt_input::vim::handle_vim_key(
            &mut st.vim,
            &st.prompt_text,
            st.prompt_cursor,
            ct_key,
        );
        match outcome {
            crate::components::prompt_input::vim::VimOutcome::Effect(effect) => {
                apply_vim_effect(st, effect);
                return;
            }
            crate::components::prompt_input::vim::VimOutcome::Pending => {
                return; // consumed; awaiting more keys
            }
            crate::components::prompt_input::vim::VimOutcome::PassThrough => {
                // fall through to default editing (Insert-mode typing, Enter, etc.)
            }
        }
    }
    // === end vim ===

    let prompt_empty = st.prompt_text.is_empty();
    // Multi-line buffers route Up/Down to vertical cursor motion (Task 9).
    let multiline = st.prompt_text.contains('\n');
    // Focus mode activates when there's at least one tool block in scrollback
    // AND the prompt is empty.
    let focus_active = prompt_empty
        && st
            .messages
            .iter()
            .any(|m| matches!(m, crate::state::RenderedMessage::AssistantToolUse { .. }));
    if let Some(mut action) = map_iocraft_key(k, prompt_empty, focus_active, multiline) {
        // Backslash-return fallback: a plain-Enter Submit becomes InsertNewline
        // when the char before the cursor is a lone '\' (terminals that can't
        // distinguish Shift+Enter from Enter). Runs only AFTER the permission
        // focus-trap branch returns, so §2.5 priority order is preserved.
        if matches!(action, KeyAction::Submit)
            && st.prompt_cursor > 0
            && st.prompt_text[..st.prompt_cursor].ends_with('\\')
        {
            action = KeyAction::InsertNewline;
        }
        if let KeyAction::ScrollStep(dir) = action {
            scroll_with_viewport(st, dir, viewport);
        } else {
            let _ = dispatch(action, st);
        }
    }

    // === Re-sync overlays after a default-path edit (M7-07). ===
    resync_overlays(st);
}

/// Re-sync the `/` palette and `@` completion overlays against the current
/// prompt text/cursor after an edit (typed char or pasted block). Mirrors the
/// edit-tail logic so paste and typing drive the overlays identically.
///
/// Palette wins when the buffer is a `/command` token; otherwise check the
/// `@` token against a fresh cwd listing. Only one overlay is open at a time.
fn resync_overlays(st: &mut AppState) {
    st.palette.sync_from_prompt(&st.prompt_text);
    if st.palette.open {
        st.completion.open = false;
    } else if crate::components::prompt_input::completion::active_at_token(
        &st.prompt_text,
        st.prompt_cursor,
    )
    .is_some()
    {
        // Only an active `@` token needs the cwd listing. Reading it on every
        // non-`@` keystroke (plain typing, arrows, Backspace) was a per-keypress
        // `read_dir` syscall whose result `sync`'s no-`@` branch discarded — the
        // plan computes candidates once per directory, not per keystroke.
        let cwd_entries =
            crate::components::prompt_input::completion::read_cwd_entries(&st.status.cwd);
        st.completion
            .sync(&st.prompt_text, st.prompt_cursor, &cwd_entries);
    } else {
        // No `@` token: close/clear the overlay without touching the filesystem.
        // `sync` with an empty candidate slice takes its no-token branch, which
        // resets open/filter/selected/candidates — identical to the prior path.
        st.completion.sync(&st.prompt_text, st.prompt_cursor, &[]);
    }
}

/// (M7-10) Apply a coalesced paste block to the prompt buffer in-place,
/// reusing the pure `apply_paste_block` (image lines → `[Image #N]` + recorded
/// attachment, text verbatim, the whole block one insertion — no per-line
/// submit). After inserting we re-sync the M7-07 overlays so a pasted
/// `/command` / `@token` still drives the palette/completion the same way
/// typing would. We do NOT clear `history_search` here: the coalescer is gated
/// off while the overlay owns keys, so a block never lands mid-search.
fn apply_block(st: &mut AppState, block: &str) {
    use crate::components::prompt_input::apply_paste_block;
    let r = apply_paste_block(&st.prompt_text, st.prompt_cursor, block, st.paste.clone());
    st.prompt_text = r.prompt;
    st.prompt_cursor = r.cursor;
    st.paste = r.state;
    // Re-sync overlays against the pasted buffer (mirrors the default-edit tail
    // in `handle_live_key`). Palette wins when the buffer is a `/command`.
    resync_overlays(st);
}

/// (M7-13 review) Async Settings open pump.
///
/// This is THE seam that makes the Settings screen live-reachable. The
/// synchronous key/submit path (Ctrl-G, `/config`, `/status`) only RAISES
/// `AppState.pending_open_settings = Some(tab)` because the open needs an async
/// `SettingsData::snapshot(handle, eff)` read it can't `.await`. This pump —
/// driven by the ticker `use_future` (the same place the M7-10 paste coalescer
/// flushes, where `state.lock().await` + the `OrchestratorHandle` are both
/// available) — observes the flag and performs the async open.
///
/// **Priority guard (parent spec §2.5):** the open NEVER fires while a
/// permission dialog (priority 1) or another full-page screen (priority 2) owns
/// the surface. We re-check the guard AFTER the snapshot `.await` (state may
/// have changed across the await point) before committing the open, and we take
/// the tab under the FIRST lock so the request fires exactly once.
///
/// Returns `true` iff the screen was opened (the caller bumps the redraw tick).
///
/// The snapshot read (`SettingsData::snapshot` + `Settings::load`) happens
/// OUTSIDE the lock so we never hold the `AppState` mutex across the handle's
/// async calls.
pub async fn pump_open_settings(
    state: &Arc<Mutex<AppState>>,
    handle: &Arc<dyn traits::OrchestratorHandle>,
) -> bool {
    // 1) Take the request under the lock, respecting priority. If a permission
    //    or another screen owns the surface, leave the flag set and bail — the
    //    next tick retries once the surface frees up.
    let (tab, project_dir) = {
        let mut st = state.lock().await;
        if st.pending_open_settings.is_none() {
            return false;
        }
        if st.pending_permission.is_some() || st.active_screen.is_some() {
            // Priority 1/2 own the surface: do NOT consume the request yet.
            return false;
        }
        let tab = st.pending_open_settings.take().expect("checked is_some");
        (tab, st.status.cwd.clone())
    };

    // 2) Build the effective settings + read the snapshot OUTSIDE the lock.
    let eff = load_effective_settings(&project_dir);
    let data = crate::screens::settings::SettingsData::snapshot(handle, eff).await;

    // 3) Re-acquire the lock and open — re-checking the priority guard, since
    //    a permission / screen may have arrived across the snapshot `.await`.
    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        // Lost the race: re-raise the request so a later tick reopens once the
        // higher-priority surface clears.
        st.pending_open_settings = Some(tab);
        return false;
    }
    st.open_settings(crate::screens::settings::SettingsState::new(tab, data));
    true
}

/// (M9-08) Async agent-discovery open pump.
///
/// Mirrors `pump_open_settings`. The sync `/agents` submit path raises
/// `AppState.pending_open_agents = true` (it cannot `.await
/// OrchestratorHandle::list_agents`). This pump — driven by the same 100ms
/// ticker `use_future` — observes the flag, fetches the catalog outside the
/// lock, and calls `AppState::open_agents`. Same priority guard as Settings:
/// never fires while a permission dialog (priority 1) or another screen
/// (priority 2) owns the surface. Returns `true` iff the screen was opened.
pub async fn pump_open_agents(
    state: &Arc<Mutex<AppState>>,
    handle: &Arc<dyn traits::OrchestratorHandle>,
) -> bool {
    // 1) Take the request under the lock, respecting priority.
    {
        let mut st = state.lock().await;
        if !st.pending_open_agents {
            return false;
        }
        if st.pending_permission.is_some() || st.active_screen.is_some() {
            // Priority 1/2 own the surface: leave the flag and retry next tick.
            return false;
        }
        st.pending_open_agents = false;
    }

    // 2) Fetch the agent catalog OUTSIDE the lock.
    let infos = handle.list_agents().await;
    let rows: Vec<crate::screens::agents::AgentRow> = infos
        .into_iter()
        .map(|i| crate::screens::agents::AgentRow {
            name: i.name,
            description: i.description,
            tools: i.tools_allowed,
            ..Default::default()
        })
        .collect();

    // 3) Re-acquire the lock and open — re-check priority guard.
    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        // Lost the race: re-raise so the next tick retries.
        st.pending_open_agents = true;
        return false;
    }
    st.open_agents(rows);
    true
}

/// Async `/mcp` server-viewer open pump. Mirrors [`pump_open_agents`]: reads the
/// real `OrchestratorHandle::list_mcp_servers` OUTSIDE the lock under the
/// permission/screen priority guard, maps each `McpServerInfo` (rendering
/// `McpStatus` to a display string), then opens the screen. Returns `true` iff
/// the screen was opened.
pub async fn pump_open_mcp(
    state: &Arc<Mutex<AppState>>,
    handle: &Arc<dyn traits::OrchestratorHandle>,
) -> bool {
    {
        let mut st = state.lock().await;
        if !st.pending_open_mcp {
            return false;
        }
        if st.pending_permission.is_some() || st.active_screen.is_some() {
            return false;
        }
        st.pending_open_mcp = false;
    }

    let infos = handle.list_mcp_servers().await;
    let rows: Vec<crate::screens::mcp::McpRow> = infos
        .into_iter()
        .map(|i| {
            let status = match i.status {
                traits::orchestrator::McpStatus::Connected => "connected".to_string(),
                traits::orchestrator::McpStatus::Disconnected => "disconnected".to_string(),
                traits::orchestrator::McpStatus::Error(e) => format!("error: {e}"),
            };
            crate::screens::mcp::McpRow {
                name: i.name,
                status,
                transport: i.transport,
            }
        })
        .collect();

    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        st.pending_open_mcp = true;
        return false;
    }
    st.open_mcp(rows);
    true
}

/// Async `/hooks` viewer open pump. Mirrors [`pump_open_mcp`], backed by the
/// real `OrchestratorHandle::list_hooks`.
pub async fn pump_open_hooks(
    state: &Arc<Mutex<AppState>>,
    handle: &Arc<dyn traits::OrchestratorHandle>,
) -> bool {
    {
        let mut st = state.lock().await;
        if !st.pending_open_hooks {
            return false;
        }
        if st.pending_permission.is_some() || st.active_screen.is_some() {
            return false;
        }
        st.pending_open_hooks = false;
    }

    let infos = handle.list_hooks().await;
    let rows: Vec<crate::screens::hooks::HookRow> = infos
        .into_iter()
        .map(|i| crate::screens::hooks::HookRow {
            name: i.name,
            event: i.event,
            matcher: i.matcher,
            timeout_ms: i.timeout_ms,
        })
        .collect();

    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        st.pending_open_hooks = true;
        return false;
    }
    st.open_hooks(rows);
    true
}

/// Async `/model` picker open pump. Mirrors [`pump_open_agents`]: fetches
/// `OrchestratorHandle::list_available_models` OUTSIDE the lock under the
/// priority guard, reads the active model from the status snapshot, then opens
/// the picker pre-highlighted on the current model.
pub async fn pump_open_model(
    state: &Arc<Mutex<AppState>>,
    handle: &Arc<dyn traits::OrchestratorHandle>,
) -> bool {
    {
        let mut st = state.lock().await;
        if !st.pending_open_model {
            return false;
        }
        if st.pending_permission.is_some() || st.active_screen.is_some() {
            return false;
        }
        st.pending_open_model = false;
    }

    let models = handle.list_available_models().await;

    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        st.pending_open_model = true;
        return false;
    }
    let current = st.status.model.clone();
    st.open_model(models, current);
    true
}

/// Async model-switch commit pump. Consumes `AppState.pending_switch_model` (set
/// by the picker's Enter), performs the async `OrchestratorHandle::switch_model`
/// write OUTSIDE the lock, then on success updates the status-line model (so the
/// header reflects the change immediately) or on failure pushes an error
/// `SystemText`. Returns `true` iff a switch was attempted (redraw needed). No
/// priority guard: the picker already closed itself on commit.
pub async fn pump_switch_model(
    state: &Arc<Mutex<AppState>>,
    handle: &Arc<dyn traits::OrchestratorHandle>,
) -> bool {
    let model = {
        let mut st = state.lock().await;
        match st.pending_switch_model.take() {
            Some(m) => m,
            None => return false,
        }
    };

    let result = handle.switch_model(&model).await;

    let mut st = state.lock().await;
    match result {
        Ok(()) => {
            // The status line reads `status.model`; update it so the header
            // reflects the switch immediately (the next status refresh agrees).
            st.status.model.clone_from(&model);
            st.push_message(crate::state::RenderedMessage::SystemText {
                body: format!("Set model to {model}"),
                timestamp: chrono::Utc::now().timestamp(),
                is_error: false,
            });
        }
        Err(e) => {
            st.push_message(crate::state::RenderedMessage::SystemText {
                body: format!("Failed to switch model: {e}"),
                timestamp: chrono::Utc::now().timestamp(),
                is_error: true,
            });
        }
    }
    true
}

/// (M9-10) Async usage-stats open pump.
///
/// Mirrors `pump_open_agents`, but needs NO `OrchestratorHandle`: the data is a
/// multi-project `*.jsonl` fs walk over `<claude_home>/projects/`, aggregated
/// by the pure `stats::{parse_session, aggregate}`. The sync `/stats` submit
/// path raises `AppState.pending_open_stats = true` (it can't `.await` the
/// walk); this pump — driven by the same 100ms ticker `use_future` — observes
/// the flag, walks + aggregates OUTSIDE the lock, then opens the screen under
/// the SAME priority guard (never over a permission dialog or another screen),
/// re-checking after the walk. Returns `true` iff the screen was opened. Called
/// unconditionally (not gated on a wired handle).
pub async fn pump_open_stats(state: &Arc<Mutex<AppState>>) -> bool {
    // PHASE 1 — a fresh `/stats` request opens the LOADING screen immediately so
    // the user gets instant feedback. The heavy walk runs in PHASE 2 on the NEXT
    // tick (so the loading screen renders before the multi-GB aggregation starts).
    {
        let mut st = state.lock().await;
        if st.pending_open_stats {
            if st.pending_permission.is_some() || st.active_screen.is_some() {
                // Priority 1/2 own the surface: leave the flag, retry next tick.
                return false;
            }
            st.pending_open_stats = false;
            st.open_stats_loading();
            return true;
        }
    }

    // PHASE 2 — if a LOADING `/stats` screen is up, aggregate + fill it. The walk
    // runs via `spawn_blocking` (see `aggregate_stats_from_disk`) so the CPU-bound
    // JSON parse of a possibly-multi-GB history never starves the UI executor —
    // otherwise the cursor freezes. The lock is NOT held across the walk.
    let needs_compute = {
        let st = state.lock().await;
        matches!(
            &st.active_screen,
            Some(crate::screens::Screen::Stats(s)) if s.loading
        )
    };
    if !needs_compute {
        return false;
    }
    let data = aggregate_stats_from_disk().await;
    let mut st = state.lock().await;
    if let Some(crate::screens::Screen::Stats(s)) = &mut st.active_screen {
        if s.loading {
            s.set_data(data);
            return true;
        }
    }
    false
}

/// (M9-09 real data) Async `/skills` open pump.
///
/// Mirrors [`pump_open_stats`] (no `OrchestratorHandle` needed): the frozen
/// `OrchestratorHandle` exposes no `list_skills`, so the data is an on-disk
/// `.claude/skills/` dir walk (project ancestors up to the git root + the user
/// home), read + parsed by the pure `skills::load_skill_sections`. The sync
/// `/skills` submit path raises `AppState.pending_open_skills = true` (it can't
/// `.await` the walk); this pump — driven by the same 100ms ticker `use_future`
/// — observes the flag, walks OUTSIDE the lock, then opens the screen under the
/// SAME priority guard (never over a permission dialog or another screen),
/// re-checking after the walk. Returns `true` iff the screen was opened. Called
/// unconditionally (not gated on a wired handle).
///
/// Single-phase (no loading screen): the walk is a handful of small `SKILL.md`
/// reads — far lighter than the `/stats` history aggregation — so it opens in
/// one tick. The reads still run on the blocking pool (`spawn_blocking`) so the
/// fs I/O never touches the UI executor.
pub async fn pump_open_skills(state: &Arc<Mutex<AppState>>) -> bool {
    // 1) Observe the flag + capture `cwd` under the lock, then DROP the lock
    //    before the fs walk (never held across `.await`/blocking I/O).
    let cwd = {
        let mut st = state.lock().await;
        if !st.pending_open_skills {
            return false;
        }
        if st.pending_permission.is_some() || st.active_screen.is_some() {
            // Priority 1/2 own the surface: leave the flag, retry next tick.
            return false;
        }
        st.pending_open_skills = false;
        st.status.cwd.clone()
    };

    // 2) Walk + parse on the blocking pool (fs reads off the UI executor).
    let claude_home = claude_home_dir();
    let sections = tokio::task::spawn_blocking(move || {
        crate::screens::skills::load_skill_sections(&cwd, &claude_home)
    })
    .await
    .unwrap_or_default();

    // 3) Re-acquire the lock and open — re-check the priority guard (a
    //    permission dialog / screen may have arrived during the walk).
    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        // Lost the race: re-raise so the next tick retries.
        st.pending_open_skills = true;
        return false;
    }
    st.open_skills(sections);
    true
}

/// (M9-10) Resolve the claude config home — the same resolution the rest of the
/// workspace uses (`$CLAUDE_CONFIG_DIR` → `~/.claude`). Mirrors
/// `screens::doctor::claude_home_dir`. Falls back to `.` when the home dir is
/// unknown so the walk simply finds nothing.
fn claude_home_dir() -> std::path::PathBuf {
    if let Ok(explicit) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !explicit.is_empty() {
            return std::path::PathBuf::from(explicit);
        }
    }
    dirs::home_dir().map_or_else(|| std::path::PathBuf::from("."), |h| h.join(".claude"))
}

/// (M9-10) Walk every `*.jsonl` transcript under `<claude_home>/projects/`
/// (claude-code `getAllSessionFiles`: main session files directly in each
/// project dir + `subagents/agent-*.jsonl`), parse each into a
/// `stats::SessionContribution`, and aggregate. Returns the empty
/// `StatsData::default()` on any I/O failure (the screen shows the locked empty
/// state). Runs OUTSIDE the `AppState` lock (it is `.await`ed only by
/// `pump_open_stats`, which holds no lock across the call).
async fn aggregate_stats_from_disk() -> crate::screens::stats::StatsData {
    // Run the whole walk on the blocking pool: it reads + JSON-parses the entire
    // `<claude_home>/projects/` history (can be many GB across thousands of
    // files), which is CPU-bound and would starve the async UI executor (frozen
    // cursor) if run inline. `spawn_blocking` keeps the executor free to render.
    tokio::task::spawn_blocking(aggregate_stats_blocking)
        .await
        .unwrap_or_default()
}

/// (`/stats` result cache) The on-disk cache file path
/// (`<claude_home>/stats-cache.json`). The filename intentionally matches
/// claude-code's `STATS_CACHE_FILENAME` (`getStatsCachePath`).
fn stats_cache_path() -> std::path::PathBuf {
    claude_home_dir().join("stats-cache.json")
}

/// (`/stats` result cache) In-process result cache: the last computed
/// `(fingerprint, data)` pair for THIS process. Checked before the disk read so
/// a second `/stats` open within one session is instant (skips even the disk
/// read); the disk cache (see [`stats_cache_path`]) covers the cross-process
/// case. This is the optional/secondary layer — the disk-cache behaviour does
/// not depend on it (claude-code's process-lifetime cache analogue).
static STATS_MEM_CACHE: std::sync::OnceLock<
    std::sync::Mutex<Option<(crate::screens::stats::HistoryFingerprint, crate::screens::stats::StatsData)>>,
> = std::sync::OnceLock::new();

/// Collect every `*.jsonl` transcript path under `projects_dir` as
/// `(path, is_subagent)` pairs, mirroring claude-code `getAllSessionFiles`: main
/// session files directly in each project dir, plus `subagents/agent-*.jsonl`.
/// Pure readdir (no file reads) so it is cheap enough to run on every `/stats`
/// open even when the cache HITS. Returns `[]` on any I/O failure (the caller
/// then aggregates an empty set → the locked empty state).
fn collect_jsonl_paths(projects_dir: &std::path::Path) -> Vec<(std::path::PathBuf, bool)> {
    use std::fs;

    let mut paths: Vec<(std::path::PathBuf, bool)> = Vec::new();

    let Ok(project_entries) = fs::read_dir(projects_dir) else {
        return paths;
    };
    for project in project_entries.flatten() {
        let project_path = project.path();
        if !project_path.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&project_path) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.is_dir() {
                // A session subdir may hold `subagents/agent-*.jsonl`.
                let subagents = path.join("subagents");
                if let Ok(sub) = fs::read_dir(&subagents) {
                    for s in sub.flatten() {
                        let sp = s.path();
                        let name = sp.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if name.starts_with("agent-") && sp.extension().is_some_and(|e| e == "jsonl")
                        {
                            paths.push((sp, true));
                        }
                    }
                }
                continue;
            }
            if path.extension().is_some_and(|e| e == "jsonl") {
                paths.push((path, false));
            }
        }
    }

    paths
}

/// Build the [`HistoryFingerprint`](crate::screens::stats::HistoryFingerprint)
/// of the given transcript paths from each file's `(mtime, size)` metadata
/// (claude-code's cheap mtime change signal). A file whose metadata cannot be
/// read contributes a zeroed `(mtime_ns=0, size=0)` entry — still keyed by path,
/// so its later appearance/disappearance still moves the fingerprint. The impure
/// `fs::metadata` reads live here (not in the pure `stats` module).
fn fingerprint_paths(paths: &[(std::path::PathBuf, bool)]) -> crate::screens::stats::HistoryFingerprint {
    use crate::screens::stats::{FileFingerprint, HistoryFingerprint};
    use std::time::UNIX_EPOCH;

    let entries: Vec<FileFingerprint> = paths
        .iter()
        .map(|(path, _)| {
            let meta = std::fs::metadata(path).ok();
            let mtime_ns = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
            FileFingerprint {
                path: path.to_string_lossy().into_owned(),
                mtime_ns,
                size,
            }
        })
        .collect();
    HistoryFingerprint::from_entries(entries)
}

/// Synchronous body of [`aggregate_stats_from_disk`], run via `spawn_blocking`.
/// Uses blocking `std::fs` (it is already off the async executor). Returns the
/// empty `StatsData::default()` on any I/O failure (the screen then shows the
/// locked empty state).
///
/// (`/stats` result cache, claude-code `aggregateClaudeCodeStats`) Resolves the
/// `<claude_home>/projects/` walk root + the [`stats_cache_path`], checks the
/// in-process [`STATS_MEM_CACHE`] first (a 2nd open within one process returns
/// instantly), then delegates to the path-parameterized [`aggregate_stats_at`]
/// for the disk-cache + walk, finally updating the mem cache on the way out.
fn aggregate_stats_blocking() -> crate::screens::stats::StatsData {
    let projects_dir = claude_home_dir().join("projects");
    let cache_path = stats_cache_path();

    // (i) Collect the transcript paths + (ii) fingerprint them. Both are cheap
    // (readdir + metadata) relative to the read+parse the cache lets us skip.
    let paths = collect_jsonl_paths(&projects_dir);
    let current_fp = fingerprint_paths(&paths);

    // In-process layer (optional/secondary): a 2nd `/stats` open within one
    // process returns instantly when the fingerprint is unchanged — skipping
    // even the disk read. Kept here (not in `aggregate_stats_at`) so the
    // disk-cache tests don't depend on this process-global static.
    let mem = STATS_MEM_CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(guard) = mem.lock() {
        if let Some((fp, data)) = guard.as_ref() {
            if *fp == current_fp {
                return data.clone();
            }
        }
    }

    let data = aggregate_stats_at(&paths, &current_fp, &cache_path);
    update_stats_mem_cache(mem, &current_fp, &data);
    data
}

/// The disk-cache + walk core of [`aggregate_stats_blocking`], parameterized on
/// the already-collected `paths`, their `current_fp` fingerprint, and the
/// `cache_path`. Does NOT touch the process-global [`STATS_MEM_CACHE`], so it is
/// deterministically testable: a HIT (the on-disk cache decodes against
/// `current_fp`) returns the cached [`StatsData`] verbatim with no read+parse; a
/// MISS reads+parses every file, aggregates, and best-effort writes the cache.
fn aggregate_stats_at(
    paths: &[(std::path::PathBuf, bool)],
    current_fp: &crate::screens::stats::HistoryFingerprint,
    cache_path: &std::path::Path,
) -> crate::screens::stats::StatsData {
    use crate::screens::stats::{
        aggregate, decode_stats_cache, encode_stats_cache, parse_session, SessionContribution,
    };
    use std::fs;

    // Disk layer (cross-process): read the cache file (ignore errors) and decode
    // it against the current fingerprint. A HIT returns without read+parse.
    if let Ok(json) = fs::read_to_string(cache_path) {
        if let Some(data) = decode_stats_cache(&json, current_fp) {
            return data;
        }
    }

    // MISS: parse every file and aggregate (behaviour identical to the
    // un-cached walk).
    let mut contribs: Vec<SessionContribution> = Vec::with_capacity(paths.len());
    for (path, is_subagent) in paths {
        if let Ok(content) = fs::read_to_string(path) {
            contribs.push(parse_session(&content, *is_subagent));
        }
    }
    let data = aggregate(&contribs);

    // Persist the cache, best-effort: a read-only `CLAUDE_CONFIG_DIR` must never
    // break `/stats`, so write failures are swallowed. The tmp-sibling + rename
    // keeps a concurrent reader from seeing a half-written file (mirrors
    // `memory::save_tier_body`). The tmp name carries the PID so two concurrent
    // LingXi *processes* both writing `/stats` cannot rename each other's
    // partially-written file (a corrupt result would only force a harmless
    // re-walk, but the PID suffix avoids it entirely).
    let tmp = cache_path.with_extension(format!("json.{}.lingxi-tmp", std::process::id()));
    if fs::write(&tmp, encode_stats_cache(current_fp, &data)).is_ok() {
        let _ = fs::rename(&tmp, cache_path);
    }

    data
}

/// Replace the in-process [`STATS_MEM_CACHE`] entry with `(fingerprint, data)`.
/// A poisoned lock is silently ignored (the cache is a best-effort optimization,
/// never a correctness dependency).
fn update_stats_mem_cache(
    mem: &std::sync::Mutex<Option<(crate::screens::stats::HistoryFingerprint, crate::screens::stats::StatsData)>>,
    fingerprint: &crate::screens::stats::HistoryFingerprint,
    data: &crate::screens::stats::StatsData,
) {
    if let Ok(mut guard) = mem.lock() {
        *guard = Some((fingerprint.clone(), data.clone()));
    }
}

/// (`/color`) Async agent-color persistence pump.
///
/// Mirrors `pump_open_stats` (no `OrchestratorHandle` needed), but it persists
/// rather than opens a screen, so it carries NO priority guard: a metadata
/// write is harmless under a permission dialog or another screen, and
/// claude-code's `saveAgentColor` fires unconditionally. The sync `/color`
/// submit path raises `AppState.pending_save_color = Some(color)` (it can't
/// `.await` the disk append); this pump — driven by the same 100ms ticker
/// `use_future` — takes the string, resolves the session transcript path the
/// same way the resume loader + stats walk do
/// (`<claude_home>/projects/<sanitize(cwd)>/<session>.jsonl`), and appends the
/// byte-locked `agent-color` entry OUTSIDE the `AppState` lock. No-op (returns
/// `false`) when no `/color` write is pending or no session id is wired (the
/// resume picker / smoke gates pass `None` — there is no transcript to write).
/// Returns `true` iff a line was written.
pub async fn pump_save_color(
    state: &Arc<Mutex<AppState>>,
    session_id: Option<protocol::SessionId>,
) -> bool {
    use tokio::io::AsyncWriteExt;
    // 1) Take the pending write + the cwd under the lock (no priority guard).
    let (color, cwd) = {
        let mut st = state.lock().await;
        let Some(color) = st.pending_save_color.take() else {
            return false;
        };
        (color, st.status.cwd.clone())
    };

    // 2) Resolve the transcript path. Without a session id there is no file to
    //    append to — drop the (already-applied) immediate effect silently.
    let Some(sid) = session_id else {
        return false;
    };

    // 3) Append the `agent-color` entry OUTSIDE the lock, reusing the session
    //    crate's byte-locked line builder + the same raw `tokio::fs` append
    //    style as `aggregate_stats_from_disk`'s reads (no `FileSystem` dep).
    let cwd_str = cwd.to_string_lossy();
    let uuid = sid.as_uuid().to_string();
    let path = session::session_path(&claude_home_dir(), &cwd_str, &uuid);
    let entry = session::agent_color_entry(&uuid, &color);
    let Ok(line) = serde_json::to_string(&entry) else {
        return false;
    };
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && tokio::fs::create_dir_all(parent).await.is_err() {
            return false;
        }
    }
    let payload = format!("{line}\n");
    let opened = tokio::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .await;
    let Ok(mut file) = opened else {
        return false;
    };
    file.write_all(payload.as_bytes()).await.is_ok()
}

/// (`/copy`) Write a pending `/copy` selection to the system clipboard.
///
/// Mirrors [`pump_save_color`] (no `OrchestratorHandle`, no priority guard): the
/// sync `/copy` submit intercept raises `AppState.pending_copy_clipboard =
/// Some(text)` (it can't `.await`, and the iocraft reconciler owns stdout, so a
/// native clipboard shell-out must run OUTSIDE the render frame). This pump —
/// driven by the same 100 ms ticker `use_future` — takes the text and shells
/// out to the platform clipboard utility OUTSIDE the `AppState` lock.
///
/// claude-code's `setClipboard` fires its native safety net FIRST on darwin
/// (`copyNative` → `pbcopy`) and writes OSC-52 to stdout as a portable fallback.
/// Because this TUI's stdout is owned by iocraft's fullscreen reconciler (a raw
/// OSC-52 write mid-frame would corrupt the rendered output), we use ONLY the
/// native utility path here: `pbcopy` (macOS), `wl-copy`/`xclip`/`xsel` (Linux),
/// `clip` (Windows) — the same utilities claude-code's `copyNative` probes. The
/// spawn is best-effort (fire-and-forget): a missing utility is silently
/// ignored, matching claude-code's `execFileNoThrow`. Returns `true` iff a
/// clipboard write was attempted (a `/copy` was pending).
pub async fn pump_copy_clipboard(state: &Arc<Mutex<AppState>>) -> bool {
    // 1) Take the pending text under the lock.
    let text = {
        let mut st = state.lock().await;
        let Some(text) = st.pending_copy_clipboard.take() else {
            return false;
        };
        text
    };

    // 2) Shell out OUTSIDE the lock on a blocking thread (the clipboard utility
    //    is a subprocess that reads stdin; `spawn_blocking` keeps the async
    //    ticker responsive). Fire-and-forget — failures are silent, exactly as
    //    claude-code's `execFileNoThrow` swallows them.
    tokio::task::spawn_blocking(move || {
        copy_to_clipboard_native(&text);
    });
    true
}

/// Shell out to a native clipboard utility, writing `text` to its stdin. Best
/// effort: a missing binary or non-zero exit is ignored (claude-code
/// `copyNative` / `execFileNoThrow`). Probes the same per-platform utilities
/// claude-code uses; on Linux it tries the Wayland tool first, then X11.
fn copy_to_clipboard_native(text: &str) {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    // (cmd, args) candidates in probe order for the current platform.
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        &[("clip", &[])]
    } else {
        // Linux/other: Wayland (wl-copy) → X11 (xclip → xsel).
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    };

    for (cmd, args) in candidates {
        let spawned = Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = spawned else {
            // Binary not found — try the next candidate.
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
            // Drop stdin to signal EOF before waiting.
        }
        // Wait so the pipe is fully consumed; ignore the exit status. Stop after
        // the first utility that successfully spawned (matches claude-code's
        // cached-winner behavior — we don't fan out to every tool).
        let _ = child.wait();
        return;
    }
}

/// (M7-13 review) Load the 4-layer effective settings the Settings screen
/// displays, mirroring the M3 `Settings::load` read API (the ONLY settings read
/// path; §4 R7). A load error degrades gracefully to defaults so the screen can
/// always open — the Config tab simply shows `(default)` rows.
fn load_effective_settings(project_dir: &std::path::Path) -> engine::settings::EffectiveSettings {
    use engine::settings::{EffectiveSettings, LoadInputs, Settings, SettingsJson};
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    Settings::load(LoadInputs {
        env: &env,
        project_dir,
        defaults: SettingsJson::default(),
    })
    .unwrap_or_else(|_| EffectiveSettings {
        settings: SettingsJson::default(),
        trace: engine::settings::tracer::ProvenanceTrace::default(),
    })
}

/// Top-level iocraft component. Drives the REPL screen and signals exit on
/// `state.should_exit`.
#[component]
#[allow(clippy::too_many_lines)]
pub fn TuiRoot(mut hooks: Hooks, props: &TuiRootProps) -> impl Into<AnyElement<'static>> {
    // `clone()`-friendly handles into the props slots.
    let state = props
        .state
        .clone()
        .expect("TuiRoot requires `state` prop (Arc<Mutex<AppState>>)");
    let cancel = props.cancel.clone().unwrap_or_default();
    let session_id = props.session_id;
    let started_at = props.started_at.unwrap_or_else(Instant::now);

    // `use_state` values let us cheaply force a re-render: increment `tick`
    // or flip `quit`. Also: `prev_streaming` lets us emit the streaming
    // render telemetry transitions exactly once.
    let tick = hooks.use_state(|| 0u64);
    let quit = hooks.use_state(|| false);
    let mut first_render = hooks.use_state(|| false);
    let mut prev_streaming = hooks.use_state(|| false);
    let mut last_size = hooks.use_state(|| (0u16, 0u16));

    // ---- Paste coalescer (M7-10): iocraft 0.8.3 surfaces NO paste event
    // (TerminalEvent = Key|FullscreenMouse|Resize), so a paste arrives as a
    // rapid burst of single-char Key events (one Enter per newline). We buffer
    // printable chars + pasted newlines arriving inside `BURST_WINDOW` and
    // flush them as ONE block (multi-line paste inserts atomically, embedded
    // Enter never submits). It lives on the mount (not serializable session
    // state) via `use_ref` — `PasteCoalescer` is Send+Sync so the Ref is
    // capturable by both the key closure and the idle-flush ticker. ----
    let paste_coalescer = hooks.use_ref(crate::components::prompt_input::PasteCoalescer::new);

    // System context handle — used to break iocraft's render loop on quit.
    let mut system = hooks.use_context_mut::<SystemContext>();

    // Live terminal size, refreshed each render.
    let (cols, rows) = hooks.use_terminal_size();

    // ---- Bridge pump: drain the rx into AppState ----------------------
    {
        let state = state.clone();
        let rx_slot = props.bridge_rx.clone();
        let mut tick_for_bridge = tick;
        hooks.use_future(async move {
            let Some(slot) = rx_slot else {
                return;
            };
            let Some(mut rx) = slot.lock().expect("rx slot poisoned").take() else {
                // Already taken (component re-mounted) — no-op.
                return;
            };
            let notify = std::sync::Arc::new(tokio::sync::Notify::new());
            while let Some(ev) = rx.recv().await {
                let mut st = state.lock().await;
                apply_event(&mut st, ev, &notify);
                drop(st);
                tick_for_bridge.set(tick_for_bridge.get().wrapping_add(1));
            }
        });
    }

    // ---- MultiAgent pump (M9-05): drain MultiAgentEvent → AppState ------
    // A second channel + drain loop EXACTLY mirroring the bridge pump above,
    // but carrying `MultiAgentEvent` and mutating ONLY via the single
    // `apply_multiagent_event` seam (pump discipline: one drain loop per
    // channel). Fed by `pump_once(PollerFeed)` on the ticker (Task 7); inert
    // (returns immediately) when `multiagent_rx` is `None`.
    {
        let state = state.clone();
        let rx_slot = props.multiagent_rx.clone();
        let mut tick_for_ma = tick;
        hooks.use_future(async move {
            let Some(slot) = rx_slot else {
                return;
            };
            let Some(mut rx) = slot.lock().expect("multiagent rx slot poisoned").take() else {
                // Already taken (component re-mounted) — no-op.
                return;
            };
            let notify = std::sync::Arc::new(tokio::sync::Notify::new());
            while let Some(ev) = rx.recv().await {
                let mut st = state.lock().await;
                crate::multiagent::apply_multiagent_event(&mut st, ev, &notify);
                drop(st);
                tick_for_ma.set(tick_for_ma.get().wrapping_add(1));
            }
        });
    }

    // ---- Ticker: 100ms spinner refresh + paste idle-flush + Settings open pump
    {
        let state = state.clone();
        let mut tick_for_ticker = tick;
        let mut coalescer = paste_coalescer;
        // (M7-13 review) The orchestrator handle drives the async Settings open
        // pump below. `None` (resume picker / smoke gates) leaves Settings
        // unreachable, which is correct for those bridge-less mounts.
        let orchestrator = props.orchestrator.clone();
        // (M9-05) The live multi-agent feed + its sender. Each tick we
        // `pump_once(feed, tx)` so registry state flows into the channel; the
        // MultiAgent pump (above) drains it and bumps the redraw tick. `None`
        // (no `TaskRegistry` wired) skips the poll entirely.
        let multiagent_feed = props.multiagent_feed.clone();
        let multiagent_tx = props.multiagent_tx.clone();
        // (`/color`) Session id for the agent-color persistence pump. `Copy`, so
        // capturing it here does not disturb the key handler's own use.
        let ticker_session_id = session_id;
        hooks.use_future(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                // (M7-10) Flush a paste burst that ended WITHOUT a trailing
                // keystroke (e.g. a paste whose final char was a newline). The
                //100ms cadence is > the 50ms BURST_WINDOW, so a finished burst
                // lands on the next tick. `flush_if_idle` is a no-op (returns
                // None) when the buffer is empty or still within the window, so
                // this never disturbs live typing. Done under the same lock the
                // key path uses, then bump `tick` to repaint.
                // Take the flushed block in a TIGHT scope so the non-Send
                // `RefMutRef` guard is dropped BEFORE the `state.lock().await`
                // below (an `await` may not hold a non-Send guard).
                let flushed: Option<String> =
                    coalescer.write().flush_if_idle(std::time::Instant::now());
                let mut needs_redraw = false;
                if let Some(block) = flushed {
                    let mut st = state.lock().await;
                    apply_block(&mut st, &block);
                    drop(st);
                    needs_redraw = true;
                }
                // (M7-13 review) Settings open pump. When a Ctrl-G / `/config` /
                // `/status` request raised `pending_open_settings`, this reads the
                // snapshot via the handle and opens the screen (respecting the
                // permission/screen priority guard inside `pump_open_settings`).
                // Runs on the same 100ms cadence so the screen opens promptly
                // after the key/submit. No-op (returns false) when no request is
                // pending or no handle is wired.
                if let Some(handle) = orchestrator.as_ref() {
                    if pump_open_settings(&state, handle).await {
                        needs_redraw = true;
                    }
                    // (M9-08) Agent-discovery open pump — mirrors Settings.
                    if pump_open_agents(&state, handle).await {
                        needs_redraw = true;
                    }
                    // `/mcp` + `/hooks` read-only viewer pumps — handle-backed,
                    // so they share the wired-handle block with Agents/Settings.
                    if pump_open_mcp(&state, handle).await {
                        needs_redraw = true;
                    }
                    if pump_open_hooks(&state, handle).await {
                        needs_redraw = true;
                    }
                    // `/model` picker open + the model-switch commit — both
                    // handle-backed (list_available_models / switch_model).
                    if pump_open_model(&state, handle).await {
                        needs_redraw = true;
                    }
                    if pump_switch_model(&state, handle).await {
                        needs_redraw = true;
                    }
                }
                // (M9-10) Usage-stats open pump. Runs UNCONDITIONALLY (not gated
                // on a wired `OrchestratorHandle`): the stats data is a
                // multi-project `*.jsonl` fs walk that needs no handle. No-op
                // (returns false) when no `/stats` request is pending.
                if pump_open_stats(&state).await {
                    needs_redraw = true;
                }
                // (M9-09 real data) `/skills` open pump. Runs UNCONDITIONALLY
                // (not gated on a wired `OrchestratorHandle`): the skill data is
                // an on-disk `.claude/skills/` dir walk that needs no handle.
                // No-op (returns false) when no `/skills` request is pending.
                if pump_open_skills(&state).await {
                    needs_redraw = true;
                }
                // (`/color`) Agent-color persistence pump. Runs UNCONDITIONALLY
                // (no handle, no priority guard): a pending `/color` choice is
                // appended to the session transcript. No-op when nothing is
                // pending or no session id is wired. We do NOT bump the redraw
                // tick — the `system` display + the immediate `session_agent_color`
                // were already applied synchronously by the submit intercept;
                // only the disk write is deferred here.
                let _wrote_color = pump_save_color(&state, ticker_session_id).await;
                // (`/copy`) Clipboard write pump. Runs UNCONDITIONALLY (no
                // handle, no priority guard): a pending `/copy` selection is
                // written to the system clipboard via the platform utility
                // (`pbcopy` on macOS — claude-code's `copyNative` darwin path).
                // No-op when nothing is pending. We do NOT bump the redraw tick
                // — the confirmation `system` display was already pushed
                // synchronously by the submit intercept; only the clipboard
                // write is deferred here (best-effort, fire-and-forget).
                let _copied = pump_copy_clipboard(&state).await;
                // (M9-05) Poll the live multi-agent feed once on the SAME
                // cadence and forward its events into the channel. We do NOT bump
                // `tick` here — the MultiAgent pump (which drains the channel)
                // bumps its own redraw tick after `apply_multiagent_event`, the
                // same decoupling the bridge pump uses. No-op when no feed/sender
                // is wired (the desktop mount supplies both; others pass `None`).
                if let (Some(feed), Some(tx)) = (multiagent_feed.as_ref(), multiagent_tx.as_ref()) {
                    let _sent = crate::multiagent::pump_once(feed.as_ref(), tx).await;
                }
                let streaming = state.lock().await.streaming.is_some();
                if streaming || needs_redraw {
                    tick_for_ticker.set(tick_for_ticker.get().wrapping_add(1));
                }
            }
        });
    }

    // ---- Cancel watch: trip should_exit on external cancel -------------
    {
        let state = state.clone();
        let cancel = cancel.clone();
        let mut quit_for_cancel = quit;
        hooks.use_future(async move {
            cancel.cancelled().await;
            let mut st = state.lock().await;
            st.should_exit = true;
            drop(st);
            quit_for_cancel.set(true);
        });
    }

    // ---- Terminal events: keystrokes route through `handle_live_key` ---
    {
        let state = state.clone();
        let mut tick_for_keys = tick;
        let key_rows = rows;
        let key_cols = cols;
        let mut coalescer = paste_coalescer;
        hooks.use_terminal_events(move |ev| match ev {
            TerminalEvent::Key(k) if k.kind != KeyEventKind::Release => {
                // Lock briefly to route the key. `try_lock` because we're in
                // iocraft's synchronous event callback and the mutex is only
                // held momentarily by the bridge pump.
                let Ok(mut st) = state.try_lock() else {
                    return;
                };
                // (M7-11 review) Publish the LIVE terminal size onto the status
                // snapshot BEFORE routing the key, so when `/doctor`'s Submit
                // dispatch builds `DoctorDiagnostics::capture` it reads the real
                // (cols, rows) instead of the `(0,0)` default — the live Doctor
                // must show the actual terminal size. Cheap (a tuple write) and
                // correct on every key, including the Enter that opens Doctor.
                st.status.term_size = (key_cols, key_rows);
                // (M7-06) Compute the scrollback viewport from the LIVE prompt
                // height so the scroll math shrinks as the prompt grows. The
                // prompt is content-driven (1 → N rows) + a 2-row footer, so
                // the viewport is `rows - (FIXED_CHROME_ROWS + prompt rows)`.
                let prompt_rows = crate::components::prompt_input::visual_row_count(
                    &st.prompt_text,
                    viewport_width(key_cols),
                );
                let viewport = viewport_height(key_rows, prompt_rows);

                // (M7-10) Paste-coalescer routing. While an overlay/dialog owns
                // keys we do NOT coalesce — those consume keys directly and a
                // buffered burst must land first. Otherwise printable chars (and
                // pasted newlines that continue a burst) buffer; the block
                // flushes on the idle tick, a non-printable key, or a deliberate
                // Enter. This is the SAME `handle_live_key` dispatcher — the
                // coalescer only batches printables before they reach it.
                let now = std::time::Instant::now();
                let overlay_active = st.pending_permission.is_some()
                    || st.history_search.is_some()
                    || st.palette.open
                    || st.completion.open;
                let printable = matches!(k.code, KeyCode::Char(_))
                    && !k.modifiers.contains(KeyModifiers::CONTROL)
                    && !k.modifiers.contains(KeyModifiers::ALT);
                let plain_enter = matches!(k.code, KeyCode::Enter)
                    && !k.modifiers.contains(KeyModifiers::CONTROL)
                    && !k.modifiers.contains(KeyModifiers::ALT)
                    && !k.modifiers.contains(KeyModifiers::SHIFT);

                if !overlay_active && printable {
                    if let KeyCode::Char(c) = k.code {
                        // NOTE: timing-tradeoff (M7-10). A printable that
                        // CONTINUES a burst (arrives within the 50ms
                        // BURST_WINDOW of the previous char) is buffered —
                        // invisibly — until flush (the quiet idle tick ~100ms,
                        // or the next non-printable key). So genuinely fast
                        // typing renders in chunks rather than per-char; normal-
                        // cadence typing (>50ms inter-key) flushes the prior
                        // buffer and echoes immediately. Inherent to timing-
                        // based paste detection under iocraft 0.8.3's
                        // no-paste-event constraint; M8 may use bracketed-paste
                        // markers to echo every keystroke instantly.
                        if let Some(block) = coalescer.write().push_char(c, now) {
                            apply_block(&mut st, &block);
                        }
                        drop(st);
                        tick_for_keys.set(tick_for_keys.get().wrapping_add(1));
                        return;
                    }
                }

                // A plain Enter that CONTINUES an active burst is a *pasted*
                // newline → buffer it (no submit). A plain Enter with no pending
                // burst (or after the window) is a *deliberate* submit → fall
                // through to flush + dispatch. This single rule preserves the
                // M5/M6 single-Enter submit while preventing per-line submit on
                // a multi-line paste.
                //
                // NOTE: timing-tradeoff (M7-10). A deliberate Enter arriving
                // within the 50ms BURST_WINDOW of a preceding char — genuinely
                // sub-50ms fast typing, or held-Enter autorepeat right after a
                // char — is buffered as a literal newline rather than submitting.
                // This is an inherent limitation of timing-based paste detection
                // under iocraft 0.8.3's no-paste-event constraint: chars are
                // never lost, and normal (>50ms inter-key) typing submits
                // normally. M8 may enable bracketed-paste markers for exact
                // detection, removing the timing heuristic entirely.
                if !overlay_active && plain_enter && coalescer.read().would_continue_burst(now) {
                    let _ = coalescer.write().push_char('\n', now);
                    drop(st);
                    tick_for_keys.set(tick_for_keys.get().wrapping_add(1));
                    return;
                }

                // Non-printable key (Enter-submit, arrows, Ctrl-*, …) or an
                // overlay is active: flush any buffered burst FIRST so it lands
                // before the key acts, then route the key normally.
                if let Some(block) = coalescer.write().flush_now() {
                    apply_block(&mut st, &block);
                }
                handle_live_key(&mut st, &k, viewport);
                drop(st);
                tick_for_keys.set(tick_for_keys.get().wrapping_add(1));
            }
            _ => {}
        });
    }

    // ---- Telemetry on every render -------------------------------------
    if !first_render.get() {
        if let Some(sid) = session_id {
            tracing::info!(
                event = FIRST_RENDER,
                session_id = %sid,
                latency_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            );
        }
        first_render.set(true);
    }

    let (lc, lr) = last_size.get();
    if (lc, lr) != (cols, rows) {
        if lc != 0 && lr != 0 {
            if let Some(sid) = session_id {
                tracing::info!(
                    event = RESIZE,
                    session_id = %sid,
                    cols = cols,
                    rows = rows,
                );
            }
        }
        last_size.set((cols, rows));
    }

    // ---- Render the live state ----------------------------------------
    // `try_lock` here: we're in iocraft's synchronous render path, and the
    // mutex is only held briefly by the bridge / key handlers. If somehow
    // contended, fall back to an empty frame for this tick.
    let snapshot = state.try_lock().map(|mut st| {
        let cur_streaming = st.streaming.is_some();
        let prev = prev_streaming.get();
        if cur_streaming != prev {
            if let Some(sid) = session_id {
                if cur_streaming {
                    tracing::info!(
                        event = STREAMING_RENDER_STARTED,
                        session_id = %sid,
                    );
                } else {
                    tracing::info!(
                        event = STREAMING_RENDER_ENDED,
                        session_id = %sid,
                    );
                }
            }
            prev_streaming.set(cur_streaming);
        }
        let should_quit = st.should_exit;
        let vp_width = viewport_width(cols);
        // (M7-06) The scrollback viewport shrinks as the prompt grows: the
        // prompt zone is content-driven (1 → N rows) and a 2-row footer sits
        // below it, so reserve `FIXED_CHROME_ROWS + prompt rows`. Computing
        // `viewport` from the SAME `visual_row_count` the `PromptInput`
        // component uses keeps M7-03's `render_window` clamp in lock-step with
        // the real layout (no scrollback/prompt overlap or gap).
        let prompt_rows =
            crate::components::prompt_input::visual_row_count(&st.prompt_text, vp_width);
        let viewport = viewport_height(rows, prompt_rows);
        // (M7-03) Refresh the line-height cache to the live width before
        // rendering so windowing + scroll clamp math agree on `total_lines`.
        st.refresh_height_cache(vp_width);
        let element = crate::app::render_screen(&st, viewport, vp_width);
        (element, should_quit)
    });

    let (element, should_quit) = match snapshot {
        Ok((el, q)) => (el, q),
        Err(_) => (element! { View() }.into_any(), false),
    };

    // Register a render-dep on `tick` so we re-render whenever the bridge
    // pump or key handler bumps it.
    let _ = tick.get();

    if should_quit || quit.get() {
        if let Some(sid) = session_id {
            tracing::info!(
                event = SESSION_ENDED,
                session_id = %sid,
                duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
                ended_via = "quit",
            );
        }
        system.exit();
    }

    element
}

/// Fixed (non-prompt) chrome rows the REPL screen reserves around the
/// scrollback: 1 status line + 1 spinner row (always reserved so the
/// scrollback doesn't jitter on `TurnStart`) + 2 footer rows (the
/// mode-indicator/placeholder row + the help/newline hint row from
/// [`crate::components::prompt_input::PromptInputFooter`]).
const FIXED_CHROME_ROWS: usize = 4;

/// Compute the scrollback viewport height given the live terminal `rows` and
/// the **current prompt height** (`prompt_visual_rows`, from
/// [`crate::components::prompt_input::visual_row_count`]).
///
/// (M7-06) The prompt zone is now content-driven (1 → N rows) and a 2-row
/// footer sits below it, so the scrollback's available height is
/// `rows - (FIXED_CHROME_ROWS + prompt_visual_rows)`, NOT the old fixed
/// `rows - 3`. Feeding the stale `rows - 3` to M7-03's `render_window` /
/// `scroll_with_viewport` while the prompt is N rows tall would overlap or
/// gap the scrollback against the prompt; this keeps the windowing math in
/// lock-step with the real layout.
fn viewport_height(rows: u16, prompt_visual_rows: usize) -> usize {
    usize::from(rows).saturating_sub(FIXED_CHROME_ROWS + prompt_visual_rows)
}

/// Columns available to the scrollback. The REPL reserves no horizontal
/// chrome today, so this is the full terminal width (min 1).
fn viewport_width(cols: u16) -> usize {
    (cols as usize).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_height_reserves_fixed_chrome_plus_single_prompt_row() {
        // Single-line prompt (1 visual row) → reserve FIXED_CHROME_ROWS(4) + 1
        // = 5 rows. 24 rows → 19 visible; saturates to 0 below the floor.
        assert_eq!(viewport_height(24, 1), 19);
        assert_eq!(viewport_height(5, 1), 0);
        assert_eq!(viewport_height(0, 1), 0);
    }

    /// (M7-06 viewport seam) When the prompt grows to N visual rows, the
    /// scrollback viewport height drops by exactly (N-1) versus the single-row
    /// case — the prompt zone eats into the scrollback, and the footer's 2
    /// fixed rows are already counted by `FIXED_CHROME_ROWS`. This pins the
    /// scroll/window math (`render_window` / `scroll_with_viewport`) against
    /// the now-variable prompt+footer height so they never overlap or gap.
    #[test]
    fn viewport_height_shrinks_as_prompt_grows() {
        let rows = 24u16;
        let single = viewport_height(rows, 1);
        // A 3-line prompt steals 2 extra rows from the scrollback.
        let three = viewport_height(rows, 3);
        assert_eq!(single - three, 2, "3-row prompt drops viewport by (3-1)=2");
        // Generalised: N rows drops the viewport by (N-1) vs. the 1-row case.
        for n in 1..=10usize {
            assert_eq!(
                viewport_height(rows, n),
                single.saturating_sub(n - 1),
                "prompt of {n} rows must drop viewport by {} vs single-line",
                n - 1
            );
        }
        // The 2-row footer is baked into FIXED_CHROME_ROWS: single-row prompt
        // reserves status(1)+spinner(1)+footer(2)+prompt(1) = 5.
        assert_eq!(single, usize::from(rows) - 5);
    }

    /// (M7-08) Build an iocraft `KeyEvent` for a printable char (Press).
    fn iocraft_char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyEventKind::Press, KeyCode::Char(c))
    }

    #[test]
    fn vim_normal_motion_moves_prompt_cursor() {
        use crate::components::prompt_input::VimMode;
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.vim_enabled = true;
        st.vim.mode = VimMode::Normal;
        st.prompt_text = "hello".into();
        st.prompt_cursor = 0;
        // 'l' moves right
        let k = iocraft_char_key('l');
        handle_live_key(&mut st, &k, 24);
        assert_eq!(st.prompt_cursor, 1);
        assert_eq!(st.prompt_text, "hello"); // unchanged
    }

    #[test]
    fn vim_disabled_typing_is_default_editing() {
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.vim_enabled = false;
        st.prompt_text = "h".into();
        st.prompt_cursor = 1;
        handle_live_key(&mut st, &iocraft_char_key('i'), 24);
        assert_eq!(st.prompt_text, "hi"); // default insert, NOT vim 'i'
        assert_eq!(st.prompt_cursor, 2);
    }

    #[test]
    fn map_iocraft_key_char_inserts() {
        let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('h'));
        assert!(matches!(
            map_iocraft_key(&k, false, false, false),
            Some(KeyAction::InsertChar('h'))
        ));
    }

    #[test]
    fn map_iocraft_key_enter_submits() {
        let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
        assert!(matches!(
            map_iocraft_key(&k, false, false, false),
            Some(KeyAction::Submit)
        ));
    }

    #[test]
    fn map_iocraft_key_shift_enter_inserts_newline() {
        let mut k = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
        k.modifiers = KeyModifiers::SHIFT;
        assert!(matches!(
            map_iocraft_key(&k, false, false, false),
            Some(KeyAction::InsertNewline)
        ));
    }

    #[test]
    fn map_iocraft_key_plain_enter_submits() {
        let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
        assert!(matches!(
            map_iocraft_key(&k, false, false, false),
            Some(KeyAction::Submit)
        ));
    }

    #[test]
    fn map_iocraft_key_up_is_vertical_when_multiline() {
        let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Up);
        assert!(matches!(
            map_iocraft_key(&k, false, false, true),
            Some(KeyAction::MoveCursorVertical(-1))
        ));
    }

    #[test]
    fn map_iocraft_key_ctrl_c_cancels() {
        let mut k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('c'));
        k.modifiers = KeyModifiers::CONTROL;
        assert!(matches!(
            map_iocraft_key(&k, false, false, false),
            Some(KeyAction::Cancel)
        ));
    }

    #[test]
    fn map_iocraft_key_focus_active_routes_e_to_toggle() {
        let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('e'));
        assert!(matches!(
            map_iocraft_key(&k, true, true, false),
            Some(KeyAction::ToggleExpanded)
        ));
    }

    /// (M7-08 review) Ctrl-Alt-V is recognised as the vim-toggle binding.
    #[test]
    fn is_toggle_vim_key_matches_ctrl_alt_v() {
        let mut k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('v'));
        k.modifiers = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert!(is_toggle_vim_key(&k));
    }

    /// Plain 'v', Ctrl-only 'v', and Alt-only 'v' are NOT the toggle.
    #[test]
    fn is_toggle_vim_key_rejects_partial_modifiers() {
        let plain = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('v'));
        assert!(!is_toggle_vim_key(&plain));
        let mut ctrl = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('v'));
        ctrl.modifiers = KeyModifiers::CONTROL;
        assert!(!is_toggle_vim_key(&ctrl));
        let mut alt = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('v'));
        alt.modifiers = KeyModifiers::ALT;
        assert!(!is_toggle_vim_key(&alt));
        // Different char with Ctrl-Alt is not the toggle.
        let mut other = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('x'));
        other.modifiers = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert!(!is_toggle_vim_key(&other));
    }

    /// (`/stats` result cache) End-to-end disk-cache behaviour, driven through
    /// the path-parameterized [`aggregate_stats_at`] (so it touches NO global
    /// env var or process-static and is race-free under the default parallel
    /// test runner): a 2nd aggregate HITS the cache (identical `StatsData` +
    /// `stats-cache.json` exists), and mutating a transcript bumps the
    /// fingerprint → the next aggregate re-walks and reflects the change.
    #[test]
    fn stats_disk_cache_hits_then_invalidates_on_file_change() {
        use crate::screens::stats::StatsData;
        use std::fs;
        use tempfile::TempDir;

        // Aggregate helper: re-collect paths + re-fingerprint each call (mirrors
        // the real `aggregate_stats_blocking` minus the mem-cache).
        let aggregate_once = |projects: &std::path::Path, cache: &std::path::Path| -> StatsData {
            let paths = collect_jsonl_paths(projects);
            let fp = fingerprint_paths(&paths);
            aggregate_stats_at(&paths, &fp, cache)
        };

        let home = TempDir::new().expect("temp home");
        let projects = home.path().join("projects");
        let repo_a = projects.join("repo");
        fs::create_dir_all(&repo_a).expect("mkdir project");
        let cache = home.path().join("stats-cache.json");

        let sess = repo_a.join("session-a.jsonl");
        let line = |date: &str, input: u64, output: u64| -> String {
            format!(
                r#"{{"type":"assistant","isSidechain":false,"timestamp":"{date}T10:00:00.000Z","message":{{"model":"claude-opus","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":0}}}}}}"#
            )
        };
        fs::write(&sess, format!("{}\n", line("2026-05-01", 100, 50))).expect("write session a");
        // A second project file so the walk crosses ≥2 files.
        let repo_b = projects.join("repo2");
        fs::create_dir_all(&repo_b).expect("mkdir project 2");
        fs::write(repo_b.join("session-b.jsonl"), format!("{}\n", line("2026-05-02", 10, 5)))
            .expect("write session b");

        // First aggregate: MISS → full walk → writes the cache.
        let first = aggregate_once(&projects, &cache);
        assert_eq!(first.total_sessions, 2);
        assert_eq!(first.total_tokens(), 165);
        assert!(cache.exists(), "stats-cache.json must exist after the first aggregate");
        // No leftover tmp sibling (atomic write completed). The tmp name is
        // PID-suffixed so it cannot collide with a concurrent process's tmp.
        assert!(!cache.with_extension(format!("json.{}.lingxi-tmp", std::process::id())).exists());

        // Second aggregate with NO file change: HIT → identical data.
        let second = aggregate_once(&projects, &cache);
        assert_eq!(second, first, "unchanged history must return the cached StatsData");

        // Mutate one transcript (append a line → size grows → fingerprint
        // changes), then re-aggregate: the cache is invalidated and the new
        // tokens are reflected.
        fs::write(
            &sess,
            format!("{}\n{}\n", line("2026-05-01", 100, 50), line("2026-05-01", 7, 3)),
        )
        .expect("rewrite session a");
        let third = aggregate_once(&projects, &cache);
        assert_ne!(third, first, "a changed transcript must re-walk, not serve stale data");
        assert_eq!(third.total_tokens(), 165 + 10, "new tokens reflected after invalidation");
    }
}
