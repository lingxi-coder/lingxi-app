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
    pub session_id: Option<lingxi_protocol::SessionId>,
    /// Wall-clock instant the session began (for `FIRST_RENDER` latency).
    pub started_at: Option<Instant>,
}

/// Map an iocraft `KeyEvent` into the workspace's `KeyAction` enum.
///
/// Mirrors `crate::events::keymap::map_key` (which consumes crossterm-0.28
/// events) but operates on iocraft's crossterm-0.29 re-exports. The mapping
/// table is intentionally kept in sync byte-for-byte with `keymap::map_key`;
/// see that file for the canonical bindings.
#[allow(clippy::too_many_lines)]
fn map_iocraft_key(evt: &KeyEvent, prompt_empty: bool, focus_active: bool) -> Option<KeyAction> {
    use KeyAction::{
        Backspace, Cancel, FocusToolStep, HistoryStep, InsertChar, MoveCursor, ScrollStep, Submit,
        ToggleExpanded,
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
        (KeyCode::Enter, _) => Some(Submit),
        (KeyCode::Backspace, _) => Some(Backspace),
        (KeyCode::Char('c'), m) if m == KeyModifiers::CONTROL => Some(Cancel),
        (KeyCode::Left, _) => Some(MoveCursor(CursorMove::Left)),
        (KeyCode::Right, _) => Some(MoveCursor(CursorMove::Right)),
        (KeyCode::Home, _) => Some(MoveCursor(CursorMove::Home)),
        (KeyCode::End, _) => Some(MoveCursor(CursorMove::End)),
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
        (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
            Some(InsertChar(*c))
        }
        _ => None,
    }
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
/// `viewport` is the scrollback viewport height (rows minus reserved chrome).
pub fn handle_live_key(st: &mut AppState, k: &KeyEvent, viewport: usize) {
    // === FOCUS TRAP: a permission dialog owns all keys while open. ===
    if st.pending_permission.is_some() {
        let ct_key = iocraft_to_crossterm028_key(k);
        let _ = crate::events::keymap::handle_key(st, ct_key);
        return;
    }
    // === end focus trap ===

    let prompt_empty = st.prompt_text.is_empty();
    // Focus mode activates when there's at least one tool block in scrollback
    // AND the prompt is empty.
    let focus_active = prompt_empty
        && st
            .messages
            .iter()
            .any(|m| matches!(m, crate::state::RenderedMessage::AssistantToolUse { .. }));
    if let Some(action) = map_iocraft_key(k, prompt_empty, focus_active) {
        if let KeyAction::ScrollStep(dir) = action {
            scroll_with_viewport(st, dir, viewport);
        } else {
            let _ = dispatch(action, st);
        }
    }
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

    // ---- Ticker: 100ms spinner refresh while streaming -----------------
    {
        let state = state.clone();
        let mut tick_for_ticker = tick;
        hooks.use_future(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let streaming = state.lock().await.streaming.is_some();
                if streaming {
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
        let viewport = viewport_height(rows);
        hooks.use_terminal_events(move |ev| match ev {
            TerminalEvent::Key(k) if k.kind != KeyEventKind::Release => {
                // Lock briefly to route the key. `try_lock` because we're in
                // iocraft's synchronous event callback and the mutex is only
                // held momentarily by the bridge pump.
                let Ok(mut st) = state.try_lock() else {
                    return;
                };
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
        let viewport = viewport_height(rows);
        let vp_width = viewport_width(cols);
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

/// Compute the viewport height for the scrollback given the live terminal
/// `rows`. The REPL screen reserves: 1 row for the status line + 1 for the
/// prompt + 1 for the optional spinner. We deliberately reserve the spinner
/// row even when it's hidden so the scrollback doesn't jitter on `TurnStart`.
fn viewport_height(rows: u16) -> usize {
    usize::from(rows.saturating_sub(3))
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
    fn viewport_height_subtracts_three_rows() {
        assert_eq!(viewport_height(24), 21);
        assert_eq!(viewport_height(3), 0);
        assert_eq!(viewport_height(0), 0);
    }

    #[test]
    fn map_iocraft_key_char_inserts() {
        let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('h'));
        assert!(matches!(
            map_iocraft_key(&k, false, false),
            Some(KeyAction::InsertChar('h'))
        ));
    }

    #[test]
    fn map_iocraft_key_enter_submits() {
        let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
        assert!(matches!(
            map_iocraft_key(&k, false, false),
            Some(KeyAction::Submit)
        ));
    }

    #[test]
    fn map_iocraft_key_ctrl_c_cancels() {
        let mut k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('c'));
        k.modifiers = KeyModifiers::CONTROL;
        assert!(matches!(
            map_iocraft_key(&k, false, false),
            Some(KeyAction::Cancel)
        ));
    }

    #[test]
    fn map_iocraft_key_focus_active_routes_e_to_toggle() {
        let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('e'));
        assert!(matches!(
            map_iocraft_key(&k, true, true),
            Some(KeyAction::ToggleExpanded)
        ));
    }
}
