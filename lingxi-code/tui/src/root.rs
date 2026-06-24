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
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::app::{dispatch, scroll_with_viewport, spawn_streaming_turn};
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

/// (TUI-PERM) Take-once slot for the `TuiPermissionGate` receiver, mirroring
/// [`BridgeRxSlot`]. The permission pump `take()`s it once on first render.
pub type PermissionRxSlot = Arc<
    std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<crate::permission_bridge::PermissionExchange>>>,
>;

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
    /// (MULTIMODAL.1) Clone of the bridge SENDER. The live-key turn-spawn pump
    /// (`pump_turn`, on the ticker `use_future`) emits `TurnStarted`/`TurnEnded`
    /// on it for a streaming turn — the same channel the orchestrator's
    /// `BridgeOutputStream` streams text/tool events onto, so both land on the
    /// bridge pump's `bridge_rx`. `None` (resume picker / smoke gates / no
    /// bridge) makes the turn-spawn pump inert — the live loop echoes the user
    /// line but spawns no turn, correct for those mounts.
    pub turn_tx: Option<UnboundedSender<TurnEvent>>,
    /// (TUI-PERM) Receiver for `TuiPermissionGate` exchanges. `None` for
    /// bridge-less mounts (resume picker / smoke gates) — the pump stays inert.
    pub permission_rx: Option<PermissionRxSlot>,
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
        // (RRS-05) No empty-prompt j/k/g/G → scroll mappings: claude-code is not
        // vim-modal by default, so those keys type the character (otherwise a
        // message could never START with j/k/g/G). They fall through to the
        // printable-char catch-all below.
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

/// (GAP D) Lower an iocraft `KeyEvent` into a `command_core` [`InputKey`] — the
/// crossterm-agnostic shape the keybindings resolver consumes (the analogue of
/// claude-code's `getKeyName(input, key)` over Ink's `Key`). The base key name
/// is normalized into the same vocabulary as `ParsedKeystroke.key`
/// (`"escape"`, `"enter"`, `"up"`, `"k"`, `" "`, …). Returns `None` for codes
/// the keymap can never bind (so the caller skips the consult and falls straight
/// through to the legacy table).
///
/// Note iocraft/crossterm reports terminal Alt as the `ALT` modifier; the
/// resolver collapses alt/meta (terminals can't distinguish them), so ALT maps
/// to `InputKey.meta` — byte-faithful with `match.ts`'s `getInkModifiers`.
fn iocraft_to_input_key(k: &KeyEvent) -> Option<command_core::keybindings::InputKey> {
    let key: String = match &k.code {
        KeyCode::Esc => "escape".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        // Single printable char → lowercased name (getKeyName: input.toLowerCase()).
        KeyCode::Char(c) => c.to_lowercase().to_string(),
        // Anything else has no ParsedKeystroke vocabulary → skip the consult.
        _ => return None,
    };
    Some(command_core::keybindings::InputKey {
        key,
        ctrl: k.modifiers.contains(KeyModifiers::CONTROL),
        shift: k.modifiers.contains(KeyModifiers::SHIFT),
        // Terminal Alt arrives as ALT; collapsed into the resolver's meta.
        meta: k.modifiers.contains(KeyModifiers::ALT),
        super_: false,
        escape: matches!(k.code, KeyCode::Esc),
    })
}

/// (GAP D) Map a resolved claude-code action id (e.g. `"app:redraw"`) to the
/// TUI's `KeyAction` enum, for the Global/Chat chords the live primary dispatch
/// currently owns.
///
/// Only the actions the live `map_iocraft_key` table already produces are
/// covered — so the consult path can REPLACE that table for those chords
/// without changing behavior. Every other action returns `None`, and the caller
/// then falls through to the legacy table (which still owns scroll/edit/vim and
/// the per-screen overlays). This is what keeps the defaults byte-identical: a
/// default keymap resolves these exact chords to these exact `KeyAction`s.
///
/// `prompt_empty` / `focus_active` reproduce the existing context-sensitivity:
/// Enter toggles a focused tool block (focus mode) vs. submits; arrows walk tool
/// focus vs. step history.
// The explicit `"chat:cancel" => None` arm documents that Esc is owned by the
// overlay/teammate traps + editor (NOT this adapter), even though its body
// matches the wildcard — keep it for clarity over the lint's preference.
#[allow(clippy::match_same_arms)]
fn action_to_keyaction(
    action: &str,
    prompt_empty: bool,
    focus_active: bool,
    multiline: bool,
) -> Option<KeyAction> {
    use KeyAction::{Cancel, FocusToolStep, HistoryStep, Submit, ToggleExpanded};
    match action {
        // Global.
        "app:interrupt" => Some(Cancel), // ctrl+c (default chord)
        // app:redraw / app:toggleTodos / app:toggleTranscript have no existing
        // live KeyAction in the primary table — fall through (legacy table is a
        // no-op for them too, so behavior is unchanged).
        // Chat.
        "chat:submit" => {
            if focus_active && prompt_empty {
                Some(ToggleExpanded)
            } else {
                Some(Submit)
            }
        }
        "chat:cancel" => None, // Esc: owned by overlay/teammate traps + editor.
        // Up/Down → history, EXCEPT in a multi-line buffer where the legacy
        // table routes them to vertical cursor motion. Returning `None` for the
        // multiline case lets `map_iocraft_key` own it (byte-identical to before
        // the keymap consult existed). Focus mode walks tool blocks instead.
        "history:previous" if !multiline => {
            if focus_active {
                Some(FocusToolStep(-1))
            } else {
                Some(HistoryStep(-1))
            }
        }
        "history:next" if !multiline => {
            if focus_active {
                Some(FocusToolStep(1))
            } else {
                Some(HistoryStep(1))
            }
        }
        _ => None,
    }
}

/// (GAP D) The keybinding contexts the live PRIMARY dispatch is active in.
///
/// The primary `handle_live_key` path runs only AFTER the permission (1),
/// screen (2), and overlay (3) traps have returned — i.e. the prompt is the
/// focused element — so the active contexts are `Chat` (the focused input) plus
/// `Global` (everywhere). The per-screen/overlay contexts (`Help`, `Select`,
/// `ModelPicker`, …) are owned by their own reducers and are the documented
/// residual (still on the hardcoded `KeyCode` matches).
fn primary_active_contexts() -> Vec<String> {
    vec!["Chat".to_string(), "Global".to_string()]
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

/// (GAP D — per-screen) The keymap consult outcome for a full-page screen key,
/// mirroring claude-code's per-component `useKeybinding` model: a resolved
/// chord is lowered back into the synthetic [`KeyEvent`] the screen's legacy
/// reducer already understands (so the reducer body stays byte-identical), a
/// chord-prefix is consumed, and an unbound / unmatched key falls through to
/// the legacy hardcoded `match` unchanged.
enum ScreenKey {
    /// A binding resolved to a screen-nav action; feed this synthetic key to
    /// the legacy reducer (e.g. `select:next` → `Down`).
    Translate(KeyEvent),
    /// A multi-keystroke chord prefix began/extended — consume the key, the
    /// screen stays open and nothing is dispatched.
    Consume,
    /// No binding (or an action this screen doesn't own) — run the legacy
    /// reducer on the ORIGINAL key, preserving every default chord byte-for-byte
    /// and letting text-entry screens keep typing unbound printable chars.
    Fallthrough,
}

/// (GAP D — per-screen) Map a resolved keybinding action id to the synthetic
/// `KeyCode` the per-screen reducers already branch on. This is the inverse of
/// the default keymap: the default `up`→`select:previous` round-trips back to
/// `KeyCode::Up`, so with NO `keybindings.json` the consult is behavior-neutral
/// (a default chord resolves to its action which lowers to the very key the
/// reducer received). A user override (e.g. `ctrl+n`→`select:next`) lowers to
/// the same `Down` the reducer handles, so the override reaches the screen.
///
/// Only the nav/control actions the screens own are mapped; any other action
/// (e.g. `chat:*`, `app:*`, the effort-arrow `modelPicker:*` which the picker
/// types literally) returns `None` so the caller falls through to the legacy
/// reducer on the original key. Returning `None` for `modelPicker:*` is what
/// keeps the model picker's `j`/`k`/char typing intact.
fn action_to_screen_keycode(action: &str) -> Option<KeyCode> {
    Some(match action {
        // Vertical selection (Select / Settings / ModelPicker-nav contexts).
        "select:next" | "messageSelector:down" | "footer:down" | "diff:nextFile" => KeyCode::Down,
        "select:previous" | "messageSelector:up" | "footer:up" | "diff:previousFile" => KeyCode::Up,
        // Accept / commit a selection.
        "select:accept" | "settings:close" | "confirm:yes" | "footer:openSelected"
        | "messageSelector:select" | "diff:viewDetails" => KeyCode::Enter,
        // Cancel / dismiss / close. Every screen's legacy reducer treats Esc as
        // its close/back key, so a rebound cancel chord still closes.
        "select:cancel" | "confirm:no" | "help:dismiss" | "settings:search"
        | "transcript:exit" | "diff:dismiss" | "attachments:exit" | "footer:clearSelection" => {
            KeyCode::Esc
        }
        // Tab navigation (Settings tabs / Stats tabs).
        "tabs:next" | "confirm:nextField" => KeyCode::Tab,
        "tabs:previous" | "confirm:cycleMode" => KeyCode::BackTab,
        // Horizontal nav (Settings tab cycle / back-out of a detail view).
        "tabs:previous-left" | "diff:previousSource" => KeyCode::Left,
        "tabs:next-right" | "diff:nextSource" => KeyCode::Right,
        // Keyboard scroll (Scroll context). Wheel actions are intentionally
        // left inert (the TUI surfaces no wheel events to this seam).
        "scroll:pageUp" => KeyCode::PageUp,
        "scroll:pageDown" => KeyCode::PageDown,
        "scroll:top" => KeyCode::Home,
        "scroll:bottom" => KeyCode::End,
        _ => return None,
    })
}

/// (GAP D — per-screen) Resolve a live screen key against the keymap for the
/// given screen contexts, mirroring claude-code's `useKeybinding`:
/// `[...screen_contexts, "Global"]` deduped (first-occurrence-wins). Returns a
/// [`ScreenKey`] telling the caller whether to translate, consume, or fall
/// through. The pending-chord state is threaded through `st.pending_chord`
/// (shared with the primary dispatch — only one input path is live at a time).
fn resolve_screen_key(st: &mut AppState, k: &KeyEvent, screen_contexts: &[&str]) -> ScreenKey {
    use command_core::keybindings::keymap::Resolution;
    let Some(input) = iocraft_to_input_key(k) else {
        return ScreenKey::Fallthrough;
    };
    // [...screen_contexts, "Global"] deduped, first-occurrence-wins.
    let mut contexts: Vec<String> = Vec::with_capacity(screen_contexts.len() + 1);
    for c in screen_contexts.iter().chain(std::iter::once(&"Global")) {
        if !contexts.iter().any(|x| x == c) {
            contexts.push((*c).to_string());
        }
    }
    match st.keymap.resolve(&input, &contexts, &mut st.pending_chord) {
        Resolution::Action(act) => match action_to_screen_keycode(&act) {
            Some(code) => {
                // Lower to the synthetic key the legacy reducer handles. Esc is
                // emitted with NONE modifiers so the `q`-no-modifier close guards
                // and the bare-Esc arms match.
                ScreenKey::Translate(KeyEvent::new(KeyEventKind::Press, code))
            }
            // An action this screen doesn't own (or a literal-typed effort arrow)
            // → run the legacy reducer on the original key.
            None => ScreenKey::Fallthrough,
        },
        Resolution::ChordPending => ScreenKey::Consume,
        Resolution::Unbound | Resolution::None => ScreenKey::Fallthrough,
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
#[allow(
    clippy::match_same_arms,
    reason = "the empty-context arms (Memory-editing / Connect / Doctor) are kept distinct for their differing rationale comments"
)]
fn handle_screen_key(st: &mut AppState, k: &KeyEvent) {
    use crate::screens::Screen;

    // (GAP D — per-screen) Consult the runtime keymap BEFORE the legacy
    // hardcoded reducers, mirroring claude-code's per-component `useKeybinding`.
    // The active screen maps to a `useKeybinding`-style context list; a resolved
    // chord is lowered back into the synthetic key the reducer already branches
    // on (so reducers are untouched), a chord-prefix is consumed, and anything
    // unbound / unmapped falls through to the legacy `match` on the ORIGINAL key.
    // With NO `keybindings.json` this is behavior-neutral: a default chord
    // resolves to the action that lowers to the very key it came from.
    let screen_contexts: &[&str] = match &st.active_screen {
        Some(Screen::Help(_)) => &["Help", "Scroll"],
        Some(Screen::Model(_)) => &["ModelPicker"],
        // (GAP D fix) Settings is a TAB NAVIGATOR (Config/Settings/Status/Usage
        // tabs + an `e`/Enter $EDITOR handoff on the Config tab) — NOT a
        // settings-panel select-list. claude-code drives tab navigation through
        // the `Tabs` context (`useTabHeaderFocus` → `tabs:next`/`tabs:previous`,
        // see design-system/Tabs.tsx), so map it there. The OLD `Settings`
        // mapping was wrong: that context's `/`→settings:search (lowered to Esc)
        // CLOSED the screen and `space`→select:accept (lowered to Enter) fired
        // the Config-tab $EDITOR handoff — both inert before, an unexcused
        // default-config behavior regression. `Tabs` binds only tab/shift+tab/
        // right/left → the reducer's existing next/prev tab cycle (byte-neutral),
        // while Esc/`q` fall through to the reducer's native close.
        Some(Screen::Settings(_)) => &["Tabs"],
        // Memory has two modes in one reducer: a tier SELECTOR (a `<Select>` list
        // in claude-code's MemoryFileSelector — Up/Down/`j`/`k`/Enter/Esc) and an
        // inline CLAUDE.md EDITOR (free text). Consult `Select` (the faithful
        // claude-code context) only in the selector; while editing, `Select`'s
        // `j`/`k`/Enter/Esc would hijack typed chars, so editing falls through
        // (text-entry residual). The OLD `Settings` mapping was wrong here too —
        // its `/`→settings:search (Esc) CLOSED the screen and `space`→accept
        // (Enter) OPENED the editor, both inert before.
        Some(Screen::Memory(m)) if !m.editing => &["Select"],
        Some(Screen::Memory(_)) => &[],
        // ThemePicker live-previews on Up/Down; its reducer does NOT bind `j`/`k`,
        // so adding `Select` (whose `j`/`k`→nav) would change defaults. Use only
        // `ThemePicker` — its sole binding (`ctrl+t`) isn't a nav action, so the
        // consult is a pure fall-through on defaults while still honoring a user
        // `ThemePicker`/`Global` override.
        Some(Screen::Theme(_)) => &["ThemePicker"],
        // Connect is a free-text API-key field: `y`/`n`/letters are typed into
        // the key buffer, so it must NOT consult `Confirmation` (whose `y`/`n`
        // would hijack typing). Its only control key (Esc-cancel) is owned
        // unconditionally by the reducer, so no consult context is needed.
        Some(Screen::Connect(_)) => &[],
        // (GAP D fix) Stats is a TAB NAVIGATOR (Overview/Models via Tab) + a
        // keyboard SCROLL pager — NOT a select-list. The OLD `Select` mapping
        // lowered `j`/`k`→select:next/previous→Down/Up and SCROLLED the body
        // where `j`/`k` were inert (an unexcused default-keymap change on a
        // tab-navigator screen). `Tabs` drives the tab cycle (byte-neutral with
        // the reducer's Tab/BackTab toggle); `Scroll` keeps the native keyboard
        // scroll. Esc/`q` fall through to the reducer's native close.
        Some(Screen::Stats(_)) => &["Tabs", "Scroll"],
        // (GAP D fix) Skills is a read-only SCROLL pager (no tabs, no selection,
        // no accept) — so the OLD `Select` mapping (whose `j`/`k`→Down/Up would
        // scroll, and whose `enter`→accept is meaningless here) was wrong. Use
        // `Scroll` only: the native PageUp/PageDown/Home/End scroll keys, with
        // Esc/`q` falling through to the reducer's native close.
        Some(Screen::Skills(_)) => &["Scroll"],
        Some(
            Screen::Mcp(_)
            | Screen::Hooks(_)
            | Screen::Permissions(_)
            | Screen::Agents(_)
            | Screen::BackgroundTasks(_)
            | Screen::Resume(_),
        ) => &["Select"],
        // Doctor is read-only (Esc/q only) and needs no keymap consult; an
        // absent screen never reaches here.
        Some(Screen::Doctor(_)) | None => &[],
    };
    let eff_owned;
    let k: &KeyEvent = if screen_contexts.is_empty() {
        k
    } else {
        match resolve_screen_key(st, k, screen_contexts) {
            ScreenKey::Translate(ev) => {
                eff_owned = ev;
                &eff_owned
            }
            // Mid-chord prefix: consume the key, keep the screen open.
            ScreenKey::Consume => return,
            ScreenKey::Fallthrough => k,
        }
    };

    match &mut st.active_screen {
        Some(Screen::Doctor(_)) => {
            // (M7-11) Read-only screen. (doctor-1) Enter is the advertised
            // dismiss affordance (claude-code `PressEnterToContinue`); Esc / `q`
            // are kept as harmless extras.
            match k.code {
                // (M7-16) `close_screen` emits `tengu_tui_screen_closed` on the
                // real `Some → None` transition, so the close telemetry is wired
                // through the shared close path (not duplicated per screen).
                KeyCode::Enter | KeyCode::Esc => st.close_screen(),
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
        Some(Screen::Permissions(state)) => {
            // Read-only permissions viewer. Pure list↔detail reducer; mirrors
            // the Hooks/Mcp arm.
            use crate::screens::permissions::{handle_permissions_key, PermissionsOutcome};
            let ct_key = iocraft_to_crossterm028_key(k);
            match handle_permissions_key(state, ct_key.code) {
                PermissionsOutcome::Close => st.close_screen(),
                PermissionsOutcome::Stay => {}
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
                ModelOutcome::Commit { provider_id, request_model } => {
                    // Record the selection in the persisted recents (catalog
                    // Phase 3-B) so it surfaces in the picker's Recent group on
                    // the next open; best-effort, never load-bearing. Then raise
                    // the async switch (pump_switch_model performs the write +
                    // refreshes the status line) and close.
                    crate::recent_models::record_recent_model(&provider_id, &request_model);
                    // "builtin" / "alias" are picker-internal placeholders, not real
                    // provider profiles — pass None so resolution stays unscoped
                    // (a real profile_name scopes resolution; a sentinel would error).
                    let profile = switch_profile_for(provider_id);
                    st.pending_switch_model = Some((request_model, profile));
                    st.close_screen();
                }
                ModelOutcome::Connect { provider_id } => {
                    // (Plan 3c §6.4) Unconfigured provider: close the picker +
                    // raise the `/connect` flow. The SYNC key path can't `.await`
                    // the device-flow / keychain, so it raises `pending_connect`;
                    // `pump_open_connect` opens the `/connect` screen next tick.
                    st.pending_connect = Some(provider_id);
                    st.close_screen();
                }
                ModelOutcome::Cancel => st.close_screen(),
                ModelOutcome::Stay => {}
            }
        }
        Some(Screen::Connect(state)) => {
            // (Plan 3c §6.3) `/connect` credential screen. Esc → cancel + close;
            // Enter on a non-empty key field → raise the host-side keychain store
            // (`pending_store_key`, drained by `pump_store_provider_key`) + close;
            // Copilot-flow typing is inert (the host drives the poll).
            use crate::screens::connect::{handle_connect_key, ConnectAction};
            let ct_key = iocraft_to_crossterm028_key(k);
            match handle_connect_key(state, ct_key.code) {
                ConnectAction::SubmitKey { provider_id, key } => {
                    st.pending_store_key = Some((provider_id, key));
                    st.close_screen();
                }
                ConnectAction::Cancel => st.close_screen(),
                ConnectAction::None => {}
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

/// Map a picker `provider_id` to the routing profile for `switch_model`.
///
/// The `/model` picker synthesises two sentinel `provider_id` values that are
/// NOT real provider profile names:
///   - `"builtin"` — an unmapped bare model-id (e.g. `claude-opus-4-5`) whose
///     provider could not be determined from the catalog.
///   - `"alias"` — an `@`-prefixed alias row (e.g. `@claude`).
///
/// Passing either sentinel as a profile to `switch_model` would scope
/// resolution to a non-existent profile and produce a `ModelUnavailable`
/// error. Map them to `None` (unscoped resolution, the correct pre-change
/// behaviour). Real profile names (e.g. `"anthropic"`, `"openai"`,
/// `"openrouter"`) are returned as `Some(provider_id)` unchanged.
pub(crate) fn switch_profile_for(provider_id: String) -> Option<String> {
    match provider_id.as_str() {
        "builtin" | "alias" => None,
        _ => Some(provider_id),
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

    // === (RRS-02) Esc interrupts a streaming turn (claude-code
    // `escape: 'chat:cancel'` → `useCancelRequest` `onCancel` when
    // `canCancelRunningTask`). Sits AFTER the permission (1) + active_screen (2)
    // + overlay (3) + teammate Esc traps so those still own Esc; runs BEFORE vim
    // (3.5/4) and the default editor so Esc cancels the turn instead of leaking
    // to the prompt buffer. Mirrors the Ctrl+C `KeyAction::Cancel` branch
    // (cancel the token + push an interrupt marker). Only fires while a turn is
    // in flight; otherwise Esc falls through to its normal editor behavior. ===
    if k.code == KeyCode::Esc {
        if let Some(tif) = &st.in_flight_turn {
            tif.cancel.cancel();
            st.push_message(crate::state::RenderedMessage::SystemText {
                body: "Interrupted by user".into(),
                timestamp: chrono::Utc::now().timestamp(),
                is_error: false,
            });
            return;
        }
    }
    // === end Esc-interrupt ===

    // === (RRS-07) Ctrl+D double-press exit (claude-code `app:exit`), only when
    // the prompt is empty. First press arms the window + shows a hint; a second
    // press within the window quits. Shares the `sigint_armed_at` window with
    // Ctrl+C. A non-empty prompt leaves Ctrl+D unbound (falls through). ===
    if k.code == KeyCode::Char('d')
        && k.modifiers.contains(KeyModifiers::CONTROL)
        && st.prompt_text.is_empty()
    {
        match st.sigint_armed_at {
            Some(t) if t.elapsed().as_secs() < crate::app::SIGINT_WINDOW_SECS => {
                st.should_exit = true;
            }
            _ => {
                st.sigint_armed_at = Some(std::time::Instant::now());
                st.push_message(crate::state::RenderedMessage::SystemText {
                    body: "Press Ctrl-D again to exit".into(),
                    timestamp: chrono::Utc::now().timestamp(),
                    is_error: false,
                });
            }
        }
        return;
    }
    // === end Ctrl+D exit ===

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

    // === (GAP D) Consult the user keymap FIRST for the Global/Chat command
    // chords. With the default keymap (gate off / no `keybindings.json`) this
    // resolves the same chords to the same `KeyAction`s the legacy
    // `map_iocraft_key` table below produces — so default behavior is identical.
    // A user override in `~/.claude/keybindings.json` is honored here.
    //
    // Fall-through discipline (so nothing existing regresses):
    //   - `Action` with a mapped `KeyAction`  → dispatch it, done.
    //   - `ChordPending`                       → consume the key (await the rest
    //                                             of the chord), done.
    //   - `Action` with NO adapter mapping / `Unbound` / `None` → fall through to
    //                                             the legacy `map_iocraft_key`
    //                                             table (which still owns editing,
    //                                             scroll, cursor moves, and any
    //                                             action the adapter doesn't cover).
    //
    // The legacy table is intentionally NOT deleted: it remains the source of
    // truth for every action `action_to_keyaction` doesn't yet map, so existing
    // keys can't break. ===
    if let Some(input_key) = iocraft_to_input_key(k) {
        use command_core::keybindings::keymap::Resolution;
        let contexts = primary_active_contexts();
        match st
            .keymap
            .resolve(&input_key, &contexts, &mut st.pending_chord)
        {
            Resolution::Action(act) => {
                if let Some(mut action) =
                    action_to_keyaction(&act, prompt_empty, focus_active, multiline)
                {
                    // Preserve the backslash-return fallback for a resolved Submit.
                    if matches!(action, KeyAction::Submit)
                        && st.prompt_cursor > 0
                        && st.prompt_text[..st.prompt_cursor].ends_with('\\')
                    {
                        action = KeyAction::InsertNewline;
                    }
                    let _ = dispatch(action, st);
                    resync_overlays(st);
                    return;
                }
                // Resolved to an action with no live KeyAction mapping (e.g.
                // app:redraw, chat:killAgents): fall through to the legacy table.
            }
            Resolution::ChordPending => {
                // Mid-chord prefix (e.g. ctrl+x of `ctrl+x ctrl+k`). Consume the
                // key — never let it reach the editor. ctrl-modified keys never
                // InsertChar anyway, so this is behavior-neutral on defaults.
                return;
            }
            Resolution::Unbound | Resolution::None => { /* fall through */ }
        }
    }

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

/// (MULTIMODAL.1) Async turn-spawn pump — the FIRST production turn-spawn in
/// the TUI live key loop.
///
/// The sync submit path (`app::dispatch(KeyAction::Submit)`) echoes a real
/// (non-slash) prompt as `UserText`, clears the prompt, and RAISES
/// `AppState.pending_turn = Some(line)`. It cannot spawn the streaming turn
/// itself: that needs the [`OrchestratorHandle`] + the bridge sender, and the
/// pure sync dispatcher holds neither. This pump — driven by the same 100ms
/// ticker `use_future` that runs the screen-open pumps (where `state.lock()`,
/// the handle, and the sender are all reachable) — observes the flag, drains
/// any pasted/dragged image paths via [`crate::components::prompt_input::PasteState::take_image_paths`],
/// DROPS the lock, then calls [`spawn_streaming_turn`]. That helper emits
/// `TurnStarted` synchronously (spinner appears at once) and `tokio::spawn`s a
/// task awaiting [`OrchestratorHandle::run_turn_streaming_with_images`], to
/// which the drained `image_paths` are forwarded (MULTIMODAL.1: each
/// `[Image #N]` placeholder in the echoed line points back at one of these
/// paths; the override loads them into `ImageSource::Base64` content blocks).
///
/// The returned [`CancellationToken`] is stored back on
/// [`AppState::cancel_token`] so Ctrl-C ([`crate::app::handle_ctrl_c`]) can
/// interrupt the turn; the bridge's `TurnEnded` event clears both `streaming`
/// and `cancel_token`.
///
/// **Priority / single-turn guard:** never spawns while a permission dialog or
/// a full-page screen owns the surface, or while a turn is already streaming
/// (one turn at a time). In those cases the flag is left set and a later tick
/// retries once the surface frees / the turn ends. Returns `true` iff a turn
/// was spawned (the caller bumps the redraw tick).
///
/// [`OrchestratorHandle`]: traits::OrchestratorHandle
/// [`OrchestratorHandle::run_turn_streaming_with_images`]: traits::OrchestratorHandle::run_turn_streaming_with_images
/// [`CancellationToken`]: tokio_util::sync::CancellationToken
pub async fn pump_turn(
    state: &Arc<Mutex<AppState>>,
    handle: &Arc<dyn traits::OrchestratorHandle>,
    turn_tx: &UnboundedSender<TurnEvent>,
) -> bool {
    // 1) Take the request under the lock, respecting priority + single-turn.
    //    Drain the image paths here (under the same lock) so they ride into
    //    THIS turn and don't leak into the next prompt's registry.
    let (prompt, image_paths) = {
        let mut st = state.lock().await;
        if st.pending_turn.is_none() {
            return false;
        }
        if st.pending_permission.is_some()
            || st.active_screen.is_some()
            || st.streaming.is_some()
        {
            // Priority 1/2 own the surface, or a turn is already in flight:
            // leave the flag set and retry on a later tick.
            return false;
        }
        let prompt = st.pending_turn.take().expect("checked is_some");
        let image_paths = st.paste.take_image_paths();
        (prompt, image_paths)
    };

    // 2) Spawn the streaming turn OUTSIDE the lock. `spawn_streaming_turn`
    //    emits `TurnStarted` synchronously on `turn_tx` (→ bridge pump → spinner)
    //    and `tokio::spawn`s the awaited `run_turn_streaming_with_images`.
    let cancel = spawn_streaming_turn(handle.clone(), prompt, image_paths, turn_tx.clone());

    // 3) Store the cancel token so Ctrl-C can interrupt; `TurnEnded` clears it.
    let mut st = state.lock().await;
    st.cancel_token = Some(cancel);
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
            // (mcp-status-vocabulary) claude-code shows "failed" for both the
            // not-connected and errored states (MCPListPanel `statusText`).
            let status = match i.status {
                traits::orchestrator::McpStatus::Connected => "connected".to_string(),
                traits::orchestrator::McpStatus::Disconnected
                | traits::orchestrator::McpStatus::Error(_) => "failed".to_string(),
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

    // Fetch BOTH the routable model ids and the llm-client provider catalog
    // OUTSIDE the lock; `build_model_entries` merges + de-dups them, then joins
    // the App's engine-threaded availability + provider maps to group rows and
    // badge unconfigured providers.
    let models = handle.list_available_models().await;
    let catalog = handle.list_model_listings().await;
    // Recents (catalog Phase 3-B): load the persisted `recentModels` list OUTSIDE
    // the lock (best-effort file read), mapped to the picker's
    // `(provider_id, request_model)` keys. Empty on any error.
    let recent: Vec<(String, String)> = crate::recent_models::load_recent_models()
        .into_iter()
        .map(|r| (r.provider_id, r.request_model))
        .collect();

    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        st.pending_open_model = true;
        return false;
    }
    let current = st.status.model.clone();
    let rows = crate::screens::model::build_model_entries(
        models,
        catalog,
        &st.provider_availability,
        &st.model_providers,
    );
    st.open_model(rows, recent, current);
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
    let (model, profile) = {
        let mut st = state.lock().await;
        match st.pending_switch_model.take() {
            Some(pair) => pair,
            None => return false,
        }
    };

    let result = handle.switch_model(&model, profile.as_deref()).await;

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

/// (Plan 3c §6.3) Async `/connect` open pump. Consumes `AppState.pending_connect`
/// (set by the picker's `Connect` outcome or a `/connect <provider>` intercept)
/// under the priority guard and opens the credential screen: `github-copilot`
/// opens the device-flow phase, every other provider opens a masked API-key
/// field. Re-raises `pending_connect` and returns `false` if a higher-priority
/// surface (permission prompt / another screen) is up. Returns `true` iff a
/// screen was opened (redraw needed).
pub async fn pump_open_connect(state: &Arc<Mutex<AppState>>) -> bool {
    let provider = {
        let mut st = state.lock().await;
        if st.pending_permission.is_some() || st.active_screen.is_some() {
            return false;
        }
        match st.pending_connect.take() {
            Some(p) => p,
            None => return false,
        }
    };
    let screen = if provider == "github-copilot" {
        crate::screens::connect::ConnectScreenState::copilot_pending()
    } else {
        // The picker carried the human label, but the flag only holds the id; the
        // header reads "Connect <id>" (the engine `/connect` group resolves the
        // canonical label on the registry path).
        crate::screens::connect::ConnectScreenState::api_key(&provider, &provider)
    };
    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        st.pending_connect = Some(provider);
        return false;
    }
    st.open_connect(screen);
    true
}

/// (Plan 3c C1) Async provider-key persistence pump. Drains
/// `AppState.pending_store_key` (`(provider_id, key)`, set by the `/connect`
/// screen's `SubmitKey`) and **actually persists** it through the bound
/// `provider_key_store.set_provider_key` keychain write. When NO store is bound
/// (headless / tests) it logs a no-op and stores nothing (byte-identical to the
/// pre-seam behavior). Returns `true` iff a key was successfully stored.
pub async fn pump_store_provider_key(state: &Arc<Mutex<AppState>>) -> bool {
    let (pending, store) = {
        let mut st = state.lock().await;
        (st.pending_store_key.take(), st.provider_key_store.clone())
    };
    let Some((provider_id, key)) = pending else {
        return false;
    };
    let Some(store) = store else {
        // No credential store bound (headless / tests): preserve the historical
        // no-op log; the key is not persisted and the user re-runs `/connect`.
        tracing::info!(
            provider = %provider_id,
            "no credential store bound; /connect provider key not persisted (len {})",
            key.len()
        );
        return false;
    };
    match store.set_provider_key(&provider_id, &key).await {
        Ok(()) => {
            tracing::info!(provider = %provider_id, "stored /connect provider key (len {})", key.len());
            true
        }
        Err(e) => {
            tracing::warn!(provider = %provider_id, error = %e, "failed to store /connect provider key");
            false
        }
    }
}

/// (`/compact`) Async forced-compaction pump.
///
/// Mirrors `pump_switch_model` (handle-backed, pushes a result message): the
/// sync `/compact` submit path raises `AppState.pending_compact = true` (it
/// can't `.await OrchestratorHandle::force_compact`). This pump — driven by the
/// same 100ms ticker `use_future` — observes the flag, runs `force_compact`
/// OUTSIDE the lock, then folds the outcome into the scrollback:
///   - `Ok(summary)` → a `RenderedMessage::CompactBoundary` built EXACTLY the
///     way the bridge `CompactionCompleted` handler in `streaming.rs`
///     constructs it (`messages_before` / `messages_after` from the summary),
///     so the rendered surface (`✻ Conversation compacted …`) is identical.
///   - `Err(e)` → an `is_error` `SystemText` matching the established
///     `Could not compact: {msg}` wording (the `crates/commands` `CompactHandler`
///     failure display + the `--no-tui` text path).
///
/// `/compact <instructions>` (a non-empty arg form) is OUT OF SCOPE: the frozen
/// `OrchestratorHandle::force_compact()` takes no instructions arg, so the sync
/// intercept only fires for the bare `/compact`.
///
/// **Priority guard (parent spec §2.5):** the compaction NEVER runs while a
/// permission dialog (priority 1) or a full-page screen (priority 2) owns the
/// surface, or while a turn is streaming — in those cases the flag is left set
/// and a later tick retries. Returns `true` iff a compaction was attempted (the
/// caller bumps the redraw tick).
pub async fn pump_compact(
    state: &Arc<Mutex<AppState>>,
    handle: &Arc<dyn traits::OrchestratorHandle>,
) -> bool {
    // 1) Take the request under the lock, respecting priority + single-turn.
    {
        let mut st = state.lock().await;
        if !st.pending_compact {
            return false;
        }
        if st.pending_permission.is_some()
            || st.active_screen.is_some()
            || st.streaming.is_some()
        {
            // Priority 1/2 own the surface, or a turn is already in flight:
            // leave the flag set and retry on a later tick.
            return false;
        }
        st.pending_compact = false;
    }

    // 2) Run the compaction OUTSIDE the lock.
    let result = handle.force_compact().await;

    // 3) Re-acquire the lock and fold the outcome into the scrollback.
    let mut st = state.lock().await;
    match result {
        Ok(summary) => {
            // Mirror the bridge `CompactionCompleted` handler (streaming.rs):
            // a `CompactBoundary` carrying the before/after counts → the UI
            // renders `✻ Conversation compacted (ctrl+o for history)`.
            st.push_message(crate::state::RenderedMessage::CompactBoundary {
                messages_before: summary.messages_before,
                messages_after: summary.messages_after,
            });
        }
        Err(e) => {
            st.push_message(crate::state::RenderedMessage::SystemText {
                body: format!("Could not compact: {e}"),
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

/// Async `/permissions` viewer open pump. Mirrors [`pump_open_skills`] (no
/// `OrchestratorHandle` needed): reads the three persistable settings tiers OFF
/// the UI executor on the blocking pool and opens the read-only viewer. The
/// frozen `PermissionGate` exposes no live-policy accessor, so this reads the
/// PERSISTED rules from disk (the same files the enforcement loader + 3c use).
pub async fn pump_open_permissions(state: &Arc<Mutex<AppState>>) -> bool {
    // 1) Observe the flag + capture `cwd` under the lock, then DROP the lock
    //    before the fs reads (never held across blocking I/O).
    let cwd = {
        let mut st = state.lock().await;
        if !st.pending_open_permissions {
            return false;
        }
        if st.pending_permission.is_some() || st.active_screen.is_some() {
            // Priority 1/2 own the surface: leave the flag, retry next tick.
            return false;
        }
        st.pending_open_permissions = false;
        st.status.cwd.clone()
    };

    // 2) Read + parse the settings tiers on the blocking pool.
    let claude_home = claude_home_dir();
    let screen_state = tokio::task::spawn_blocking(move || {
        crate::screens::permissions::load_permission_sections(&cwd, &claude_home)
    })
    .await
    .unwrap_or_default();

    // 3) Re-acquire the lock and open — re-check the priority guard (a
    //    permission dialog / screen may have arrived during the read).
    let mut st = state.lock().await;
    if st.pending_permission.is_some() || st.active_screen.is_some() {
        st.pending_open_permissions = true;
        return false;
    }
    st.open_permissions(screen_state);
    true
}

/// (M9-10) Resolve the claude config home — the same resolution the rest of the
/// workspace uses (`$CLAUDE_CONFIG_DIR` → `~/.claude`). Mirrors
/// `screens::doctor::claude_home_dir`. Falls back to `.` when the home dir is
/// unknown so the walk simply finds nothing.
fn claude_home_dir() -> std::path::PathBuf {
    // claude-code `tr()` `??`: a SET `$CLAUDE_CONFIG_DIR` wins verbatim (incl.
    // empty → cwd-relative); only UNSET falls back to `<home>/.claude`.
    if let Ok(explicit) = std::env::var("CLAUDE_CONFIG_DIR") {
        return std::path::PathBuf::from(explicit);
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

/// (#6 main-loop parity) Drain `AppState.pending_terminal_sequence` and write the
/// validated OSC/BEL escape sequence to the TUI's stdout — the host that owns the
/// controlling terminal the orchestrator process lacks (claude-code `BEo` writes
/// the allowlisted sequence to `process.stdout`). Driven by the same 100 ms
/// ticker `use_future` as [`pump_copy_clipboard`].
///
/// The orchestrator already validated the sequence against the OSC
/// 0/1/2/9/99/777 + BEL allowlist (`hooks::terminal_seq::validate_terminal_sequence`),
/// so only screen-safe control sequences (terminal title, notifications, bell)
/// reach stdout — the terminal interprets them out-of-band, so they do not
/// corrupt iocraft's rendered frame. The write runs OUTSIDE the `AppState` lock
/// on a blocking thread and is best-effort (a write/flush error is ignored,
/// matching claude-code's fire-and-forget terminal write). Returns `true` iff a
/// sequence was pending.
pub async fn pump_terminal_sequence(state: &Arc<Mutex<AppState>>) -> bool {
    let seq = {
        let mut st = state.lock().await;
        let Some(seq) = st.pending_terminal_sequence.take() else {
            return false;
        };
        seq
    };
    tokio::task::spawn_blocking(move || {
        use std::io::Write as _;
        let mut out = std::io::stdout();
        let _ = out.write_all(seq.as_bytes());
        let _ = out.flush();
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

    // ---- Permission pump (TUI-PERM): drain PermissionExchange → FIFO --------
    // Mirrors the bridge/multiagent pumps. Each exchange is queued and the
    // front is promoted into the single active dialog when free, so concurrent
    // gate.check()s never overwrite an active dialog's resp_tx. Inert when
    // `permission_rx` is None (bridge-less mounts).
    {
        let state = state.clone();
        let rx_slot = props.permission_rx.clone();
        let mut tick_for_perm = tick;
        hooks.use_future(async move {
            let Some(slot) = rx_slot else {
                return;
            };
            let Some(mut rx) = slot.lock().expect("permission rx slot poisoned").take() else {
                return;
            };
            while let Some(exchange) = rx.recv().await {
                let mut st = state.lock().await;
                st.permission_queue.push_back(exchange);
                crate::state::promote_next_permission(&mut st);
                drop(st);
                tick_for_perm.set(tick_for_perm.get().wrapping_add(1));
            }
        });
    }

    // ---- Statusline pump (A6 batch-6 Task 2): debounced, single-flight -----
    // The TUI analog of claude-code's `StatusLine.tsx` execute-on-change effect
    // (debounce 300ms, abortable execute, set-only-on-change, silent errors).
    // A 300ms `Skip` interval is the debounce analog; the dirty flag is the
    // re-trigger (set ONLY by `apply_event` on `TurnEnded`). Single-flight: the
    // command runs to completion before the next tick can re-arm — no
    // generation counter needed (re-trigger rides `status_line_dirty`).
    //
    // Lock discipline: the payload build and the result apply are each scoped
    // blocks; NO lock is held across the `spawn_blocking().await` (the command
    // is sync `std::process` so it runs off the async runtime). On any failure
    // (spawn error / timeout / non-zero / empty output / JoinError) the result
    // is `None` and the previous `status_line_text` is kept.
    {
        let state = state.clone();
        let mut tick_for_status = tick;
        hooks.use_future(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(300));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                // Build the (command, stdin-json) payload under the lock, then
                // DROP the lock before the off-thread command run. Skip the tick
                // unless a turn re-armed the pump (clear-on-consume).
                let payload = {
                    let mut st = state.lock().await;
                    if !st.status_line_dirty {
                        continue;
                    }
                    st.status_line_dirty = false;
                    crate::state::build_pump_payload(&st)
                };
                let Some((command, stdin_json)) = payload else {
                    continue;
                };
                // Run the sync command off the async runtime. `JoinError`
                // (panic / cancel) maps to `None` via `unwrap_or_default`.
                let out = tokio::task::spawn_blocking(move || {
                    crate::components::status_line_command::run_status_line_command(
                        &command,
                        &stdin_json,
                        crate::components::status_line_command::STATUS_LINE_TIMEOUT,
                    )
                })
                .await
                .unwrap_or_default();
                // `run_status_line_command` ALREADY returns formatted text —
                // assign directly (do NOT format again). Set-only-on-change +
                // repaint (claude-code `prev.statusLineText === text` guard).
                if let Some(text) = out {
                    let mut st = state.lock().await;
                    if st.status_line_text.as_deref() != Some(text.as_str()) {
                        st.status_line_text = Some(text);
                        drop(st);
                        tick_for_status.set(tick_for_status.get().wrapping_add(1));
                    }
                }
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
        // (MULTIMODAL.1) Bridge sender clone for the live-key turn-spawn pump
        // (`pump_turn`). `None` (resume picker / smoke gates) makes the pump
        // inert — the live loop echoes the user line but spawns no turn.
        let turn_tx = props.turn_tx.clone();
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
                    // (`/compact`) Forced-compaction pump — handle-backed
                    // (`force_compact`), so it shares the wired-handle block. When
                    // a `/compact` submit raised `pending_compact` this runs the
                    // compaction and folds a `CompactBoundary` (or error
                    // `SystemText`) into the scrollback, under the same
                    // permission/screen/streaming priority guard.
                    if pump_compact(&state, handle).await {
                        needs_redraw = true;
                    }
                }
                // (MULTIMODAL.1) Turn-spawn pump — the live loop's only
                // production turn-spawn. When `dispatch(Submit)` echoed a real
                // (non-slash) prompt it RAISED `pending_turn`; this spawns the
                // streaming turn (forwarding any pasted image paths into
                // `run_turn_streaming_with_images`) and stores the cancel token.
                // Gated on BOTH a wired handle + bridge sender (the desktop mount
                // supplies both; resume/smoke mounts pass `None` → inert).
                if let (Some(handle), Some(tx)) = (orchestrator.as_ref(), turn_tx.as_ref()) {
                    if pump_turn(&state, handle, tx).await {
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
                // `/permissions` read-only viewer pump — off-disk like `/skills`
                // (reads the settings tiers; no handle needed).
                if pump_open_permissions(&state).await {
                    needs_redraw = true;
                }
                // (Plan 3c §6.3) `/connect` open pump — opens the credential
                // screen when the picker's `Connect` outcome or a `/connect
                // <provider>` intercept raised `pending_connect`. Handle-free
                // (the screen reducer + keychain store carry the work).
                if pump_open_connect(&state).await {
                    needs_redraw = true;
                }
                // (Plan 3c C1) `/connect` provider-key persistence pump — drains
                // `pending_store_key` and writes the key through the bound
                // `CredentialManager::set_provider_key`. Handle-free; no-op when
                // nothing is pending or no store is bound.
                if pump_store_provider_key(&state).await {
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
                // (#6) Terminal-sequence write pump. Runs UNCONDITIONALLY: a
                // hook-returned, allowlisted OSC/BEL sequence staged by
                // `apply_event` is written to stdout here (the TUI owns the
                // controlling terminal the orchestrator lacks). No-op when
                // nothing is pending; no redraw tick (out-of-band terminal
                // control, not screen content).
                let _wrote_term_seq = pump_terminal_sequence(&state).await;
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

    /// (GAP D) On the DEFAULT keymap (no user `keybindings.json`), the
    /// consult-first path must resolve the Global/Chat command chords to the
    /// SAME `KeyAction` the bare legacy `map_iocraft_key` table produces — proving
    /// zero behavior change when no override exists. Covers a representative set:
    /// `ctrl+c` (Cancel), `enter` (Submit), `up` (`HistoryStep`). A printable char and a
    /// scroll key (which the keymap does NOT bind) must still fall through to the
    /// legacy table unchanged.
    #[test]
    fn default_keymap_consult_matches_legacy_for_global_chat_chords() {
        let km = command_core::keybindings::Keymap::defaults();
        let contexts = primary_active_contexts();

        // Helper: resolve a key via the keymap → adapter, mirroring the live
        // consult (single-line, no focus).
        let consult = |k: &KeyEvent| -> Option<KeyAction> {
            use command_core::keybindings::keymap::Resolution;
            let mut pending = None;
            let input = iocraft_to_input_key(k)?;
            match km.resolve(&input, &contexts, &mut pending) {
                Resolution::Action(act) => action_to_keyaction(&act, false, false, false),
                _ => None,
            }
        };

        // ctrl+c → both produce Cancel.
        let mut ctrl_c = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('c'));
        ctrl_c.modifiers = KeyModifiers::CONTROL;
        assert_eq!(consult(&ctrl_c), Some(KeyAction::Cancel));
        assert_eq!(
            map_iocraft_key(&ctrl_c, false, false, false),
            Some(KeyAction::Cancel)
        );

        // enter → both produce Submit.
        let enter = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
        assert_eq!(consult(&enter), Some(KeyAction::Submit));
        assert_eq!(
            map_iocraft_key(&enter, false, false, false),
            Some(KeyAction::Submit)
        );

        // up → both produce HistoryStep(-1) (single-line).
        let up = KeyEvent::new(KeyEventKind::Press, KeyCode::Up);
        assert_eq!(consult(&up), Some(KeyAction::HistoryStep(-1)));
        assert_eq!(
            map_iocraft_key(&up, false, false, false),
            Some(KeyAction::HistoryStep(-1))
        );

        // A printable 'h' is NOT a keymap chord → consult yields None → the
        // legacy table owns it (InsertChar).
        let h = iocraft_char_key('h');
        assert_eq!(consult(&h), None);
        assert_eq!(
            map_iocraft_key(&h, false, false, false),
            Some(KeyAction::InsertChar('h'))
        );
    }

    /// (GAP D) A user override of a Global chord (ctrl+l → app:interrupt) is
    /// honored by the live consult, while an unspecified chord still resolves to
    /// its default. Loads the override through the real `load_keybindings` path
    /// (gate on, temp `keybindings.json`), then drives the keymap exactly as
    /// `handle_live_key` does — so the end-to-end loader→resolver→adapter seam is
    /// exercised without a direct `indexmap` dependency in the TUI crate.
    #[test]
    fn user_override_changes_live_dispatch_unspecified_falls_back() {
        use command_core::keybindings::{load_keybindings, Keymap};
        use std::io::Write;

        let json = r#"{ "bindings": [ { "context": "Global", "bindings": { "ctrl+l": "app:interrupt" } } ] }"#;
        let path = std::env::temp_dir().join(format!(
            "lingxi-tui-kb-override-{}.json",
            std::process::id()
        ));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(json.as_bytes())
            .unwrap();
        let km = Keymap::from_load_result(load_keybindings(true, &path, false));
        let _ = std::fs::remove_file(&path);

        let contexts = primary_active_contexts();
        let resolve = |km: &Keymap, k: &KeyEvent| -> Option<KeyAction> {
            use command_core::keybindings::keymap::Resolution;
            let mut pending = None;
            let input = iocraft_to_input_key(k)?;
            match km.resolve(&input, &contexts, &mut pending) {
                Resolution::Action(act) => action_to_keyaction(&act, false, false, false),
                _ => None,
            }
        };

        // ctrl+l (no live KeyAction by default → None) now → Cancel via override.
        let mut ctrl_l = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('l'));
        ctrl_l.modifiers = KeyModifiers::CONTROL;
        assert_eq!(resolve(&km, &ctrl_l), Some(KeyAction::Cancel));

        // ctrl+c (unspecified by the override) still resolves to its default
        // app:interrupt → Cancel (last-wins merge kept the default).
        let mut ctrl_c = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('c'));
        ctrl_c.modifiers = KeyModifiers::CONTROL;
        assert_eq!(resolve(&km, &ctrl_c), Some(KeyAction::Cancel));
    }

    /// (GAP D — per-screen, TDD) A `keybindings.json` override of a `Select`
    /// chord (`ctrl+n` → `select:next`) reaches the MCP viewer's live dispatch
    /// through `handle_screen_key`: ctrl+n now advances the selection exactly as
    /// Down does. An unspecified key (Enter → enter detail) still falls back to
    /// the legacy reducer. This proves the per-screen keymap consult is wired.
    #[test]
    fn per_screen_override_reaches_mcp_dispatch_unspecified_falls_back() {
        use crate::screens::mcp::{McpRow, McpScreenState};
        use crate::screens::Screen;
        use command_core::keybindings::{load_keybindings, Keymap};
        use std::io::Write;

        let json = r#"{ "bindings": [ { "context": "Select", "bindings": { "ctrl+n": "select:next" } } ] }"#;
        let path = std::env::temp_dir().join(format!(
            "lingxi-tui-screen-kb-{}.json",
            std::process::id()
        ));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(json.as_bytes())
            .unwrap();
        let km = Keymap::from_load_result(load_keybindings(true, &path, false));
        let _ = std::fs::remove_file(&path);

        let rows = vec![
            McpRow { name: "a".into(), status: "connected".into(), transport: "stdio".into() },
            McpRow { name: "b".into(), status: "connected".into(), transport: "stdio".into() },
        ];
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.set_keymap(km);
        st.active_screen = Some(Screen::Mcp(McpScreenState { rows, ..Default::default() }));

        // ctrl+n is NOT a default Select chord, but the override binds it to
        // select:next → lowered to Down → advances the selection.
        let mut ctrl_n = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('n'));
        ctrl_n.modifiers = KeyModifiers::CONTROL;
        handle_screen_key(&mut st, &ctrl_n);
        match &st.active_screen {
            Some(Screen::Mcp(s)) => assert_eq!(s.selected, 1, "ctrl+n advanced selection"),
            other => panic!("expected Mcp screen, got {other:?}"),
        }

        // Enter (unspecified by the override) still falls back to the legacy
        // reducer → enters detail mode.
        let enter = KeyEvent::new(KeyEventKind::Press, KeyCode::Enter);
        handle_screen_key(&mut st, &enter);
        match &st.active_screen {
            Some(Screen::Mcp(s)) => {
                assert_eq!(s.mode, crate::screens::mcp::McpDialogMode::Detail);
            }
            other => panic!("expected Mcp screen in detail, got {other:?}"),
        }
    }

    /// (GAP D — per-screen) The DEFAULT keymap round-trips behavior-neutrally
    /// for a `Select` LIST reducer: Down on the MCP viewer resolves `select:next`
    /// → lowered back to Down → the legacy reducer advances the selection,
    /// exactly as before the consult.
    ///
    /// NOTE: this only exercises a list-reducer that NATIVELY handles `j`/`k`, so
    /// it does NOT cover the TAB-NAVIGATOR screens (Settings/Stats) where mapping
    /// to a list context was a regression. Those are covered by
    /// [`settings_tab_navigator_default_keymap_is_behavior_neutral`] and
    /// [`stats_tab_navigator_default_keymap_is_behavior_neutral`].
    #[test]
    fn per_screen_default_keymap_is_behavior_neutral() {
        use crate::screens::mcp::{McpRow, McpScreenState};
        use crate::screens::Screen;
        let rows = vec![
            McpRow { name: "a".into(), status: "x".into(), transport: "stdio".into() },
            McpRow { name: "b".into(), status: "x".into(), transport: "stdio".into() },
        ];
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        // Default keymap (no override).
        st.active_screen = Some(Screen::Mcp(McpScreenState { rows, ..Default::default() }));
        let down = KeyEvent::new(KeyEventKind::Press, KeyCode::Down);
        handle_screen_key(&mut st, &down);
        match &st.active_screen {
            Some(Screen::Mcp(s)) => assert_eq!(s.selected, 1, "Down advanced via default keymap"),
            other => panic!("expected Mcp screen, got {other:?}"),
        }
    }

    /// (GAP D — per-screen) The model picker is a TEXT-ENTRY screen using the
    /// `ModelPicker` context (which binds only the effort arrows, not nav). A
    /// printable char must STILL type into the search query (fall through to the
    /// legacy reducer), and `j`/`k` are typed — NOT treated as nav — preserving
    /// byte-identical search behavior.
    #[test]
    fn per_screen_model_picker_typing_falls_through() {
        use crate::screens::model::{ModelRow, ModelScreenState};
        use crate::screens::Screen;
        let rows = vec![ModelRow {
            display_model: "Opus".into(),
            request_model: "claude-opus".into(),
            provider_id: "anthropic".into(),
            provider_label: "Anthropic".into(),
            available: true,
        }];
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.active_screen = Some(Screen::Model(ModelScreenState {
            rows,
            ..Default::default()
        }));
        // Typing 'j' must append to the query (NOT scroll), proving ModelPicker
        // does not bind nav for printable chars.
        let j = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('j'));
        handle_screen_key(&mut st, &j);
        match &st.active_screen {
            Some(Screen::Model(s)) => assert_eq!(s.query, "j", "j typed into query"),
            other => panic!("expected Model screen, got {other:?}"),
        }
    }

    /// Build a default-data Settings screen on `tab` (mirrors the screen's own
    /// `fixture_state`). Used by the tab-navigator behavior-neutrality tests.
    fn settings_screen_on(tab: crate::screens::settings::SettingsTab) -> crate::screens::Screen {
        use crate::screens::settings::{SettingsData, SettingsState};
        use engine::settings::tracer::ProvenanceTrace;
        use engine::settings::{EffectiveSettings, SettingsJson};
        use traits::{CostSnapshot, StatusSnapshot};
        crate::screens::Screen::Settings(SettingsState::new(
            tab,
            SettingsData {
                effective: EffectiveSettings {
                    settings: SettingsJson::default(),
                    trace: ProvenanceTrace::default(),
                },
                status: StatusSnapshot::default(),
                cost: CostSnapshot::default(),
            },
        ))
    }

    /// (GAP D fix — tab navigators, TDD) The Settings screen is a TAB NAVIGATOR
    /// (Config/Settings/Status/Usage tabs + an `e`/Enter $EDITOR handoff), NOT a
    /// select-list. Under the DEFAULT keymap (no keybindings.json) it must NOT
    /// inherit the `Settings`/`Select` LIST context, whose `/`→settings:search
    /// (lowered to Esc) would CLOSE the screen and whose `space`→select:accept
    /// (lowered to Enter) would trigger the Config-tab $EDITOR handoff. Both were
    /// inert before the per-screen consult; this asserts they STAY inert.
    #[test]
    fn settings_tab_navigator_default_keymap_is_behavior_neutral() {
        use crate::screens::settings::SettingsTab;
        use crate::screens::Screen;

        // `/` must NOT close the Settings screen under defaults (was inert).
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.active_screen = Some(settings_screen_on(SettingsTab::Config));
        let slash = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('/'));
        handle_screen_key(&mut st, &slash);
        assert!(
            matches!(st.active_screen, Some(Screen::Settings(_))),
            "`/` must stay inert on the Settings tab navigator, not close it"
        );

        // `space` must NOT trigger the $EDITOR handoff on the Config tab.
        st.pending_config_edit = false;
        let space = KeyEvent::new(KeyEventKind::Press, KeyCode::Char(' '));
        handle_screen_key(&mut st, &space);
        assert!(
            matches!(st.active_screen, Some(Screen::Settings(_))),
            "`space` must stay inert (no EditConfig) on the Settings tab navigator"
        );
        assert!(
            !st.pending_config_edit,
            "`space` must NOT raise the $EDITOR handoff under defaults"
        );

        // Tab navigation still works (Tab cycles tabs — the screen's real job).
        let tab = KeyEvent::new(KeyEventKind::Press, KeyCode::Tab);
        handle_screen_key(&mut st, &tab);
        match &st.active_screen {
            Some(Screen::Settings(s)) => {
                assert_eq!(s.tab, SettingsTab::Config.next(), "Tab cycles to next tab");
            }
            other => panic!("expected Settings screen, got {other:?}"),
        }
    }

    /// (GAP D fix) A `Tabs`-context override (`ctrl+l` → tabs:next) reaches the
    /// Settings tab navigator's live dispatch, while Esc still closes (fallback).
    #[test]
    fn settings_tab_navigator_override_reaches_dispatch() {
        use crate::screens::settings::SettingsTab;
        use crate::screens::Screen;
        use command_core::keybindings::{load_keybindings, Keymap};
        use std::io::Write;

        let json = r#"{ "bindings": [ { "context": "Tabs", "bindings": { "ctrl+l": "tabs:next" } } ] }"#;
        let path = std::env::temp_dir()
            .join(format!("lingxi-tui-settings-kb-{}.json", std::process::id()));
        std::fs::File::create(&path).unwrap().write_all(json.as_bytes()).unwrap();
        let km = Keymap::from_load_result(load_keybindings(true, &path, false));
        let _ = std::fs::remove_file(&path);

        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.set_keymap(km);
        st.active_screen = Some(settings_screen_on(SettingsTab::Config));

        // ctrl+l (override) → tabs:next → lowered to Tab → next tab.
        let mut ctrl_l = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('l'));
        ctrl_l.modifiers = KeyModifiers::CONTROL;
        handle_screen_key(&mut st, &ctrl_l);
        match &st.active_screen {
            Some(Screen::Settings(s)) => {
                assert_eq!(s.tab, SettingsTab::Config.next(), "ctrl+l advanced the tab");
            }
            other => panic!("expected Settings screen, got {other:?}"),
        }

        // Esc (unspecified by the override) falls back → closes.
        let esc = KeyEvent::new(KeyEventKind::Press, KeyCode::Esc);
        handle_screen_key(&mut st, &esc);
        assert!(st.active_screen.is_none(), "Esc closes the Settings screen (fallback)");
    }

    /// (RRS-02) Esc interrupts a streaming turn — claude-code
    /// `escape: 'chat:cancel'`. With a turn in flight (and no screen/overlay/
    /// teammate trap active) Esc cancels the token and pushes an interrupt
    /// marker, mirroring the Ctrl+C `KeyAction::Cancel` branch.
    #[test]
    fn esc_interrupts_in_flight_turn() {
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        let token = tokio_util::sync::CancellationToken::new();
        st.in_flight_turn = Some(crate::state::TurnInFlight { turn_id: 1, cancel: token.clone() });

        let esc = KeyEvent::new(KeyEventKind::Press, KeyCode::Esc);
        handle_live_key(&mut st, &esc, 24);

        assert!(token.is_cancelled(), "Esc must cancel the in-flight turn token");
        assert!(
            st.messages.iter().any(|m| matches!(
                m,
                crate::state::RenderedMessage::SystemText { body, .. } if body == "Interrupted by user"
            )),
            "Esc must push the 'Interrupted by user' interrupt marker"
        );
    }

    /// (RRS-02) Esc is a no-op interrupt when NO turn is in flight — it falls
    /// through to normal editor behavior (no spurious interrupt marker).
    #[test]
    fn esc_without_in_flight_turn_does_not_interrupt() {
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        assert!(st.in_flight_turn.is_none());
        let esc = KeyEvent::new(KeyEventKind::Press, KeyCode::Esc);
        handle_live_key(&mut st, &esc, 24);
        assert!(
            !st.messages.iter().any(|m| matches!(
                m,
                crate::state::RenderedMessage::SystemText { body, .. } if body == "Interrupted by user"
            )),
            "no interrupt marker when no turn is in flight"
        );
    }

    /// (RRS-07) Ctrl+D on an empty prompt arms, then exits on a second press.
    #[test]
    fn ctrl_d_double_press_exits_on_empty_prompt() {
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        let ctrl_d = {
            let mut e = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('d'));
            e.modifiers = KeyModifiers::CONTROL;
            e
        };
        handle_live_key(&mut st, &ctrl_d, 24);
        assert!(!st.should_exit, "first Ctrl+D only arms");
        assert!(st.messages.iter().any(|m| matches!(
            m,
            crate::state::RenderedMessage::SystemText { body, .. } if body == "Press Ctrl-D again to exit"
        )));
        handle_live_key(&mut st, &ctrl_d, 24);
        assert!(st.should_exit, "second Ctrl+D within the window exits");
    }

    /// (RRS-07) Ctrl+D with a non-empty prompt does NOT exit (falls through).
    #[test]
    fn ctrl_d_with_text_does_not_exit() {
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.prompt_text = "hi".into();
        st.prompt_cursor = 2;
        let mut ctrl_d = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('d'));
        ctrl_d.modifiers = KeyModifiers::CONTROL;
        handle_live_key(&mut st, &ctrl_d, 24);
        assert!(!st.should_exit, "Ctrl+D with text must not exit");
    }

    /// (GAP D fix — tab navigators, TDD) The Stats screen is a TAB NAVIGATOR
    /// (Overview/Models via Tab) + a keyboard SCROLL pager. It must NOT inherit
    /// the `Select` LIST context, whose `j`/`k`→select:next/previous (lowered to
    /// Down/Up) would SCROLL the body where `j`/`k` were inert before. This
    /// asserts `j`/`k` stay inert under the default keymap.
    #[test]
    fn stats_tab_navigator_default_keymap_is_behavior_neutral() {
        use crate::screens::stats::{ModelUsage, StatsData, StatsState, StatsTab};
        use crate::screens::Screen;
        use std::collections::BTreeMap;

        // Build a Models tab body LONGER than the 16-line VIEWPORT (each model
        // = 2 lines) so that a scroll key genuinely moves the offset; this makes
        // the "j stays inert / Down scrolls" distinction non-vacuous.
        let mut model_usage = BTreeMap::new();
        for i in 0..12 {
            model_usage.insert(
                format!("model-{i:02}"),
                ModelUsage { input_tokens: 1000 + i, output_tokens: 500 + i, cache_read_tokens: 0 },
            );
        }
        let data = StatsData { model_usage, total_sessions: 3, ..StatsData::default() };
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.active_screen = Some(Screen::Stats(StatsState::new(data)));
        // Toggle to Models via Tab so the embedded scroll window is sized to the
        // (long) Models body — the private `set_tab` re-anchors the scroll.
        let tab = KeyEvent::new(KeyEventKind::Press, KeyCode::Tab);
        handle_screen_key(&mut st, &tab);
        let at_models_top = match &st.active_screen {
            Some(Screen::Stats(s)) => {
                assert_eq!(s.tab, StatsTab::Overview.toggled(), "Tab toggles to Models");
                s.scroll.offset()
            }
            other => panic!("expected Stats screen, got {other:?}"),
        };

        // `j` must NOT scroll under defaults (was inert; the wrong Select mapping
        // would lower j→Down and scroll it).
        let j = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('j'));
        handle_screen_key(&mut st, &j);
        match &st.active_screen {
            Some(Screen::Stats(s)) => {
                assert_eq!(s.scroll.offset(), at_models_top, "`j` must stay inert (no scroll)");
            }
            other => panic!("expected Stats screen, got {other:?}"),
        }

        // `k` likewise inert.
        let k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('k'));
        handle_screen_key(&mut st, &k);
        match &st.active_screen {
            Some(Screen::Stats(s)) => {
                assert_eq!(s.scroll.offset(), at_models_top, "`k` must stay inert (no scroll)");
            }
            other => panic!("expected Stats screen, got {other:?}"),
        }

        // Down (native scroll key, Scroll context) STILL scrolls — proving the
        // body is scrollable and the Scroll context is intact.
        let down = KeyEvent::new(KeyEventKind::Press, KeyCode::Down);
        handle_screen_key(&mut st, &down);
        match &st.active_screen {
            Some(Screen::Stats(s)) => {
                assert!(s.scroll.offset() > at_models_top, "Down still scrolls the body");
            }
            other => panic!("expected Stats screen, got {other:?}"),
        }
    }

    /// (GAP D fix — Memory selector, TDD) The Memory tier SELECTOR is a
    /// `<Select>` list (claude-code `MemoryFileSelector`), so it must NOT inherit
    /// the `Settings` panel context whose `/`→settings:search (lowered to Esc)
    /// would CLOSE the screen — inert before. Under the default keymap `/` must
    /// fall through to the selector's inert `_` arm, leaving the screen open.
    #[test]
    fn memory_selector_slash_does_not_close_under_defaults() {
        use crate::screens::memory::MemoryScreenState;
        use crate::screens::Screen;
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.active_screen = Some(Screen::Memory(MemoryScreenState::default()));
        let slash = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('/'));
        handle_screen_key(&mut st, &slash);
        assert!(
            matches!(&st.active_screen, Some(Screen::Memory(m)) if !m.editing),
            "`/` must stay inert on the Memory selector, not close it"
        );
        // `space` likewise must not open the editor (was inert; Settings' accept
        // would have opened it).
        let space = KeyEvent::new(KeyEventKind::Press, KeyCode::Char(' '));
        handle_screen_key(&mut st, &space);
        assert!(
            matches!(&st.active_screen, Some(Screen::Memory(m)) if !m.editing),
            "`space` must stay inert on the Memory selector (no editor open)"
        );
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

    // ── Plan 3c `/connect` provider-key persistence pump (C1) ────────────────

    /// (Plan 3c C1) In-memory `SecureStorage` so the `/connect`-key store pump can
    /// be exercised against a real `CredentialManager` without touching a keychain.
    /// Mirrors the `MemStorage` double in `secret/src/credential.rs` tests.
    #[derive(Default)]
    struct MemStorage {
        map: std::sync::Mutex<
            std::collections::HashMap<(String, String), protocol::SecureStorageData>,
        >,
    }

    #[async_trait::async_trait]
    impl traits::SecureStorage for MemStorage {
        async fn store(
            &self,
            service: &str,
            account: &str,
            data: protocol::SecureStorageData,
        ) -> Result<(), traits::SecureStorageError> {
            self.map
                .lock()
                .unwrap()
                .insert((service.into(), account.into()), data);
            Ok(())
        }
        async fn retrieve(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<protocol::SecureStorageData>, traits::SecureStorageError> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .get(&(service.into(), account.into()))
                .cloned())
        }
        async fn delete(
            &self,
            service: &str,
            account: &str,
        ) -> Result<(), traits::SecureStorageError> {
            self.map
                .lock()
                .unwrap()
                .remove(&(service.into(), account.into()));
            Ok(())
        }
        async fn list(&self, service: &str) -> Result<Vec<String>, traits::SecureStorageError> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .keys()
                .filter(|(s, _)| s == service)
                .map(|(_, a)| a.clone())
                .collect())
        }
        fn is_encrypted(&self) -> bool {
            false
        }
        fn backend(&self) -> traits::SecureStorageBackend {
            traits::SecureStorageBackend::PlainText
        }
    }

    struct FixedClock;
    impl traits::Clock for FixedClock {
        fn now(&self) -> std::time::SystemTime {
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000)
        }
    }

    /// HTTP transport that panics — the key-store pump never makes HTTP calls.
    struct NoHttp;
    #[async_trait::async_trait]
    impl traits::HttpTransport for NoHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            panic!("key-store pump must not perform HTTP");
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            panic!("key-store pump must not perform HTTP");
        }
    }

    /// (Plan 3c C1, regression) The `/connect` screen collects a key and raises
    /// `pending_store_key`; the pump MUST persist it via the bound credential
    /// store (`set_provider_key`), not drop it. Wires an in-memory
    /// `CredentialManager`, sets a pending key, runs the pump, and asserts the key
    /// round-trips through `get_provider_key` — proving picker → Connect →
    /// type-key → Enter actually authenticates the provider.
    #[tokio::test]
    async fn pump_store_provider_key_persists_collected_key() {
        use secret::CredentialManager;

        let storage = Arc::new(MemStorage::default());
        let cm = Arc::new(CredentialManager::new(
            storage.clone() as Arc<dyn traits::SecureStorage>,
            Arc::new(FixedClock),
            Arc::new(NoHttp),
        ));

        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.set_provider_key_store(Some(cm.clone()));
        st.pending_store_key = Some(("openrouter".to_string(), "sk-x".to_string()));
        let state = Arc::new(Mutex::new(st));

        let stored = pump_store_provider_key(&state).await;
        assert!(stored, "pump must report a successful store when bound");

        // The pending slot is drained.
        assert!(state.lock().await.pending_store_key.is_none());

        // The key actually round-trips through the credential store.
        let got = cm
            .get_provider_key("openrouter")
            .await
            .expect("get_provider_key ok")
            .expect("key present");
        assert_eq!(got.expose_secret(), "sk-x");
    }

    /// (Plan 3c C1) On a headless / no-store build (the default), the pump still
    /// drains the pending key but persists nothing and reports `false` — preserving
    /// the historical no-op behavior for smoke gates / tests.
    #[tokio::test]
    async fn pump_store_provider_key_noop_when_no_store_bound() {
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.pending_store_key = Some(("deepseek".to_string(), "sk-y".to_string()));
        let state = Arc::new(Mutex::new(st));

        let stored = pump_store_provider_key(&state).await;
        assert!(!stored, "no store bound ⇒ pump stores nothing and returns false");
        assert!(state.lock().await.pending_store_key.is_none());
    }

    /// (Plan 3c §6.3) `pump_open_connect` opens the masked API-key screen for an
    /// api-key provider, and the Copilot device-flow screen for `github-copilot`.
    #[tokio::test]
    async fn pump_open_connect_opens_the_right_flow() {
        use crate::screens::connect::{ConnectFlow, ConnectScreenState};
        use crate::screens::Screen;

        // api-key provider → masked key field.
        let mut st = AppState::new(crate::state::StatusSnapshot::default());
        st.pending_connect = Some("openrouter".to_string());
        let state = Arc::new(Mutex::new(st));
        assert!(pump_open_connect(&state).await);
        {
            let st = state.lock().await;
            match &st.active_screen {
                Some(Screen::Connect(ConnectScreenState {
                    flow: ConnectFlow::ApiKey { provider_id, .. },
                    ..
                })) => assert_eq!(provider_id, "openrouter"),
                other => panic!("expected api-key connect screen, got {other:?}"),
            }
            assert!(st.pending_connect.is_none(), "pending_connect drained");
        }

        // github-copilot → device-flow.
        let mut st2 = AppState::new(crate::state::StatusSnapshot::default());
        st2.pending_connect = Some("github-copilot".to_string());
        let state2 = Arc::new(Mutex::new(st2));
        assert!(pump_open_connect(&state2).await);
        {
            let st2 = state2.lock().await;
            assert!(matches!(
                &st2.active_screen,
                Some(Screen::Connect(ConnectScreenState {
                    flow: ConnectFlow::Copilot,
                    ..
                }))
            ));
        }
    }

    /// `switch_profile_for` maps the picker-internal sentinel `provider_id`
    /// values to `None` (unscoped resolution) so they don't reach the
    /// orchestrator as fake profile names and trigger `ModelUnavailable`.
    /// Real provider names pass through as `Some(name)` unchanged.
    #[test]
    fn switch_profile_for_maps_sentinels_to_none() {
        // Picker sentinels → None (unscoped, pre-change behaviour preserved).
        assert_eq!(
            switch_profile_for("builtin".to_string()),
            None,
            "\"builtin\" sentinel must yield no profile"
        );
        assert_eq!(
            switch_profile_for("alias".to_string()),
            None,
            "\"alias\" sentinel must yield no profile"
        );
        // Real provider profile names → Some(name) (profile-scoped resolution).
        assert_eq!(
            switch_profile_for("anthropic".to_string()),
            Some("anthropic".to_string()),
            "real profile \"anthropic\" must pass through"
        );
        assert_eq!(
            switch_profile_for("openai".to_string()),
            Some("openai".to_string()),
            "real profile \"openai\" must pass through"
        );
        assert_eq!(
            switch_profile_for("openrouter".to_string()),
            Some("openrouter".to_string()),
            "real profile \"openrouter\" must pass through"
        );
    }
}
