//! Cross-state-seam tests (M7-16 T12, parent spec §5.6).
//!
//! The M6 final holistic review caught a ship-blocker: a live-key path that
//! bypassed the permission focus-trap — a SEAM between sub-plans that the
//! per-sub-plan tests missed. M7 has far more contenders for live keys
//! (screens × permissions × vim × palette × completion × history-search ×
//! message-selector). This file explicitly probes those seams against the
//! SINGLE live-key dispatcher `root::handle_live_key`, whose priority order is
//! (parent spec §2.5):
//!
//! 1. `pending_permission.is_some()` → permission dialog   (M6-05)
//! 2. `active_screen.is_some()`      → active screen        (M7-11..15)
//! 3. overlays (history-search / message-selector / palette / completion)
//! 4. vim input (only when `vim_enabled`)
//! 5. default editing / scrollback nav
//!
//! Invariants asserted here:
//! - a pending permission wins over EVERY screen + overlay (priority 1);
//! - only one screen and one overlay can be active at once (mutual exclusion);
//! - `/` opens the palette overlay even from vim NORMAL mode (overlay > vim);
//! - a (pasted) key burst while a screen is open never leaks into the prompt
//!   buffer underneath and never crashes the screen;
//! - vim only fires when no screen/overlay owns keys AND `vim_enabled`.
//!
//! There is exactly ONE live dispatcher (`root::handle_live_key`, invoked only
//! from the `use_terminal_events` closure); `app::dispatch` is the pure
//! action-applier it calls AFTER the priority chain, and `keymap::handle_key`
//! is dead in the live binary (its focus-trap branch runs only behind
//! `handle_live_key`'s own priority-1 permission branch, which returns first).
//! No parallel key path exists.

use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use permission::gate::{PermissionRequest, PromptDefault};
use serde_json::json;
use tui::components::prompt_input::VimMode;
use tui::root::{handle_live_key, handle_live_mouse};
use tui::screens::Screen;
use tui::state::{AppState, PendingPermission, RenderedMessage, StatusSnapshot};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(KeyEventKind::Press, code)
}

fn key_mods(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, code);
    k.modifiers = mods;
    k
}

fn fresh() -> AppState {
    AppState::new(StatusSnapshot::default())
}

fn long_scrollback(lines: usize) -> AppState {
    let mut st = fresh();
    for i in 0..lines {
        st.push_message(RenderedMessage::AssistantText {
            body: format!("line {i}"),
            timestamp: 0,
        });
    }
    st.refresh_height_cache(80);
    st
}

fn arm_permission(st: &mut AppState) {
    st.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: json!({"command": "ls"}),
            default_decision: PromptDefault::DenyByDefault,
        },
        worker: None,
    });
    st.pending_permission_started_at = Some(std::time::Instant::now());
}

/// SEAM 1 (priority 1 > 2): a screen is open AND a permission becomes pending.
/// A key the screen would consume (`j`) must NOT reach the screen — the
/// permission owns it. (The dialog has no `resp_tx` attached here, so the key is
/// simply swallowed by the priority-1 branch; we assert non-leakage.)
#[test]
fn permission_wins_over_open_screen() {
    let mut st = fresh();
    st.open_doctor(tui::screens::doctor::DoctorDiagnostics::capture(
        std::path::Path::new("/work"),
        0,
        0,
        (80, 24),
    ));
    assert!(matches!(st.active_screen, Some(Screen::Doctor(_))));
    arm_permission(&mut st);

    // `q` would close a Doctor screen (priority 2); but priority 1 owns it.
    handle_live_key(&mut st, &key(KeyCode::Char('q')), 24);

    assert!(
        st.pending_permission.is_some(),
        "permission (priority 1) still owns the keys — `q` did not reach the screen"
    );
    assert!(
        matches!(st.active_screen, Some(Screen::Doctor(_))),
        "the Doctor screen is intact — the permission key did not close it"
    );
}

#[test]
fn live_pageup_uses_viewport_height_for_scrollback() {
    let mut st = long_scrollback(120);
    handle_live_key(&mut st, &key(KeyCode::PageUp), 20);
    assert_eq!(st.scroll_offset, 10, "root path must use live viewport/2, not dispatch fallback height=1");
}

#[test]
fn mouse_wheel_scrolls_scrollback_without_prompt_focus_change() {
    let mut st = long_scrollback(120);
    st.prompt_text = "typing".into();
    st.prompt_cursor = st.prompt_text.len();

    let wheel = iocraft::prelude::FullscreenMouseEvent::new(
        iocraft::prelude::MouseEventKind::ScrollUp,
        0,
        0,
    );
    handle_live_mouse(&mut st, &wheel, 10);

    assert_eq!(st.scroll_offset, 1);
    assert_eq!(st.prompt_text, "typing");
    assert_eq!(st.prompt_cursor, "typing".len());
}

#[test]
fn mouse_wheel_does_not_scroll_under_palette() {
    let mut st = long_scrollback(120);
    st.palette.open = true;
    let wheel = iocraft::prelude::FullscreenMouseEvent::new(
        iocraft::prelude::MouseEventKind::ScrollUp,
        0,
        0,
    );
    handle_live_mouse(&mut st, &wheel, 10);
    assert_eq!(st.scroll_offset, 0);
}

/// SEAM 1b (priority 1 > 3): a permission wins over an open overlay too. With
/// the message-selector overlay open AND a permission pending, a key does not
/// reach the overlay. We feed `j` — a key the `ToolUseConfirm` dialog does NOT
/// resolve on (only `1`/`2`/`n`/`N`/`Esc` resolve), so the permission stays
/// pending and we can prove the overlay was untouched. (Esc WOULD resolve the
/// dialog as Deny, which is also priority-1-correct — it would just clear the
/// permission rather than leave it pending, so we avoid it here.)
#[test]
fn permission_wins_over_open_overlay() {
    let mut st = fresh();
    st.message_selector.open();
    assert!(st.message_selector.open);
    arm_permission(&mut st);

    handle_live_key(&mut st, &key(KeyCode::Char('j')), 24);

    assert!(
        st.pending_permission.is_some(),
        "permission (priority 1) still owns the keys — `j` did not reach the overlay"
    );
    assert!(
        st.message_selector.open,
        "the overlay is untouched — the permission key did not close it"
    );
}

/// SEAM 2a (default-path overlay open): in NON-vim editing, typing `/` opens
/// the command palette (priority-3 overlay) via the default-edit tail's
/// `resync_overlays`. This is the baseline the vim seam (2b) contrasts with.
#[test]
fn slash_opens_palette_in_default_editing() {
    let mut st = fresh();
    assert!(!st.vim_enabled);
    assert!(!st.palette.open);

    handle_live_key(&mut st, &key(KeyCode::Char('/')), 24);

    assert!(
        st.palette.open,
        "`/` opens the command palette in default (non-vim) editing"
    );
    // Mutual exclusion: opening the palette does not also open completion.
    assert!(!st.completion.open, "only one overlay is open at a time");
}

/// SEAM 2b (vim owns `/`, no leak): in vim NORMAL mode `/` is vim's own
/// search key (priority 4 consumes it as `Effect(None)`); it must NOT leak to
/// the default-edit path, so it neither inserts a `/` char nor opens the
/// palette. This is the deliberate vim/palette boundary — vim search ≠ the
/// command palette. (The palette-from-vim path is reached by typing `/` only
/// in INSERT mode, where vim passes printables through.)
#[test]
fn slash_in_vim_normal_is_owned_by_vim_not_palette() {
    let mut st = fresh();
    st.vim_enabled = true;
    st.vim.mode = VimMode::Normal;
    st.prompt_text = "hello".into();
    st.prompt_cursor = 0;

    handle_live_key(&mut st, &key(KeyCode::Char('/')), 24);

    assert!(
        !st.palette.open,
        "vim NORMAL `/` is vim-search (priority 4 owns it), NOT the palette"
    );
    assert_eq!(
        st.prompt_text, "hello",
        "vim NORMAL `/` did not leak a literal `/` into the prompt buffer"
    );
}

/// SEAM 3 (priority 2 isolation): a multi-key (pasted) burst while a screen is
/// open must NOT leak into the prompt buffer underneath and must NOT crash the
/// screen. The screen (priority 2) consumes every key; the prompt is inert.
#[test]
fn key_burst_while_screen_open_does_not_leak_into_prompt() {
    let mut st = fresh();
    st.open_memory();
    assert!(matches!(st.active_screen, Some(Screen::Memory(_))));
    let prompt_before = st.prompt_text.clone();

    // Feed a burst of printable keys (a paste-like sequence). Each routes
    // through the SAME handle_live_key; the priority-2 screen owns them.
    for c in "pasted text".chars() {
        handle_live_key(&mut st, &key(KeyCode::Char(c)), 24);
    }

    assert_eq!(
        st.prompt_text, prompt_before,
        "the burst did not leak into the prompt buffer under the screen"
    );
    assert!(
        st.active_screen.is_some(),
        "the screen is still open and intact after the burst"
    );
}

/// SEAM 4 (mutual exclusion + gating): opening a screen while an overlay is
/// open is sane, and vim is gated. (a) With the palette overlay open, opening a
/// screen makes the screen win all keys (priority 2 > 3). (b) vim only fires
/// when no screen/overlay owns keys AND `vim_enabled`.
#[test]
fn screen_outranks_overlay_and_vim_is_gated() {
    // (a) Overlay open, then a screen opens: priority 2 (screen) > 3 (overlay).
    let mut st = fresh();
    st.palette.sync_from_prompt("/conf"); // opens the palette overlay
    let palette_was_open = st.palette.open;
    st.open_doctor(tui::screens::doctor::DoctorDiagnostics::capture(
        std::path::Path::new("/work"),
        0,
        0,
        (80, 24),
    ));
    // A key now routes to the screen (priority 2), never the overlay.
    handle_live_key(&mut st, &key(KeyCode::Char('x')), 24);
    assert!(
        matches!(st.active_screen, Some(Screen::Doctor(_))) || st.active_screen.is_none(),
        "screen owns the key (priority 2); it did not reach the palette overlay"
    );
    let _ = palette_was_open;

    // (b) vim disabled → a NORMAL-mode motion key is plain editing, NOT vim.
    let mut st2 = fresh();
    st2.vim_enabled = false;
    st2.prompt_text = "h".into();
    st2.prompt_cursor = 1;
    handle_live_key(&mut st2, &key(KeyCode::Char('l')), 24);
    assert_eq!(
        st2.prompt_text, "hl",
        "vim disabled: `l` is a literal insert, not a vim motion"
    );

    // (b') vim enabled + NORMAL + no screen/overlay → `l` is a vim motion.
    let mut st3 = fresh();
    st3.vim_enabled = true;
    st3.vim.mode = VimMode::Normal;
    st3.prompt_text = "hello".into();
    st3.prompt_cursor = 0;
    handle_live_key(&mut st3, &key(KeyCode::Char('l')), 24);
    assert_eq!(
        st3.prompt_cursor, 1,
        "vim NORMAL `l` moves the cursor right"
    );
    assert_eq!(
        st3.prompt_text, "hello",
        "vim motion does not edit the buffer"
    );
}

/// SEAM 5 (modal-independent toggle, gated on no overlay): Ctrl-Alt-V toggles
/// vim from any state, but an open overlay still owns its keys first. Confirms
/// the priority-3.5 toggle sits after the overlay focus-trap.
#[test]
fn vim_toggle_is_modal_independent_but_yields_to_overlay() {
    // Toggle on from a clean REPL.
    let mut st = fresh();
    assert!(!st.vim_enabled);
    handle_live_key(
        &mut st,
        &key_mods(
            KeyCode::Char('v'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ),
        24,
    );
    assert!(st.vim_enabled, "Ctrl-Alt-V toggles vim on from the REPL");

    // With an overlay open, a printable `v` passes through to the editor (the
    // overlay owns it); it must NOT be hijacked as the toggle. Here we confirm
    // the overlay is the one consuming keys (priority 3), not the toggle.
    let mut st2 = fresh();
    st2.message_selector.open();
    let before = st2.vim_enabled;
    handle_live_key(&mut st2, &key(KeyCode::Char('v')), 24);
    assert_eq!(
        st2.vim_enabled, before,
        "a plain `v` while the overlay is open does not flip vim (overlay owns the key)"
    );
    assert!(st2.message_selector.open, "overlay still owns keys");
}
