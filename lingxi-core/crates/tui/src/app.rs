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

/// Idle Ctrl-C re-arm window: the second Ctrl-C confirms exit only when
/// the first was within this many seconds. Matches the M5-13 stdio REPL.
pub const SIGINT_WINDOW_SECS: u64 = 2;

/// Crate version string surfaced to the placeholder line.
///
/// Sourced from the lingxi-tui crate's own `CARGO_PKG_VERSION` so version
/// bumps automatically propagate. M6-09 bumps the crate to 0.7.0, at which
/// point the rendered placeholder reads `"lingxi-tui v0.7.0"`.
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
            let line = std::mem::take(&mut st.prompt_text);
            st.prompt_cursor = 0;
            st.history.push(line.clone());
            st.history_cursor = None;
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
                st.push_message(RenderedMessage::SystemText {
                    body: "^C interrupted by user".into(),
                    timestamp: chrono::Utc::now().timestamp(),
                    is_error: false,
                });
            } else if !st.prompt_text.is_empty() {
                st.prompt_text.clear();
                st.prompt_cursor = 0;
            } else {
                match st.sigint_armed_at {
                    Some(t) if t.elapsed().as_secs() < SIGINT_WINDOW_SECS => {
                        st.should_exit = true;
                    }
                    _ => {
                        st.sigint_armed_at = Some(Instant::now());
                        st.push_message(RenderedMessage::SystemText {
                            body: "^C (press Ctrl-C again or type /exit to quit)".into(),
                            timestamp: chrono::Utc::now().timestamp(),
                            is_error: false,
                        });
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
            if let Some(id) = st.focused_tool_id {
                st.toggle_expanded(&id);
            }
            false
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
pub async fn handle_submit_line(
    st: &mut AppState,
    line: &str,
    dispatcher: &dyn lingxi_traits::SlashCommandDispatcher,
) -> bool {
    if let Some(cmd) = line.strip_prefix('/') {
        // Local intercepts (M5-09 stubs don't yet do these).
        let trimmed = cmd.split_whitespace().next().unwrap_or("");
        match trimmed {
            "clear" => {
                st.messages.clear();
                st.scroll_offset = 0;
                return false;
            }
            "exit" | "quit" => {
                st.should_exit = true;
                return false;
            }
            _ => {}
        }
        // Fall through to the registry for everything else.
        let outcome = dispatcher.dispatch(line).await;
        match outcome {
            lingxi_traits::SlashDispatchResult::Handled { display }
            | lingxi_traits::SlashDispatchResult::Unknown { display, .. } => {
                st.push_message(RenderedMessage::SystemText {
                    body: display,
                    timestamp: chrono::Utc::now().timestamp(),
                    is_error: false,
                });
            }
            lingxi_traits::SlashDispatchResult::NotASlashCommand => {
                // Shouldn't happen — we stripped the leading "/" already.
            }
        }
        return false;
    }
    true // plain text — caller runs `orchestrator.run_turn`
}

/// Build the full `ReplScreen` element from an `AppState` snapshot for
/// the given viewport height. Used by the per-frame render path; tests
/// also exercise it to verify the screen composes without panicking.
///
/// The full reactive event-loop wiring (`use_state` hooks, key event →
/// dispatch, re-render on terminal resize) lands in M6-03 along with
/// the streaming spinner. M6-02 ships the pure render function so
/// downstream tasks have a stable assembly point.
#[must_use]
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
        use lingxi_permission::gate::PermissionRequest;
        return match &pp.request {
            PermissionRequest::ToolUseConfirm {
                tool_name,
                tool_input,
                ..
            } => {
                let pretty = serde_json::to_string_pretty(tool_input).unwrap_or_default();
                let tool_name = tool_name.clone();
                let focus = state.tool_use_dialog_state.focus;
                element! {
                    ToolUseConfirm(
                        tool_name: tool_name,
                        tool_input_pretty: pretty,
                        focus: focus,
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
    let status = state.status.clone();
    let messages = state.messages.clone();
    let prompt_text = state.prompt_text.clone();
    let prompt_cursor = state.prompt_cursor;
    let scroll_offset = state.scroll_offset;
    let show_spinner = should_render_spinner(state);
    let expanded = state.expanded.clone();
    let focused_tool_id = state.focused_tool_id;
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
            scroll_offset: scroll_offset,
            viewport_height: viewport_height,
            show_spinner: show_spinner,
            expanded: expanded,
            focused_tool_id: focused_tool_id,
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
    dispatcher: &dyn lingxi_traits::SlashCommandDispatcher,
) {
    let should_run = handle_submit_line(st, submitted, dispatcher).await;
    if !should_run {
        return;
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let turn_id = next_turn_id();
    st.in_flight_turn = Some(crate::state::TurnInFlight {
        turn_id,
        cancel: cancel.clone(),
    });

    let outcome = orch.run_turn(submitted, cancel).await;
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
    handle: std::sync::Arc<dyn lingxi_traits::OrchestratorHandle>,
    prompt: String,
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
            .run_turn_streaming_with_cancel(&prompt, cancel_clone)
            .await;
        let ev = match outcome {
            Ok(o) => TurnEvent::TurnEnded(o),
            Err(e) => {
                tracing::error!(error = ?e, "streaming turn failed");
                // Surface error completion as a clean EndTurn for now;
                // M7 may introduce a dedicated TurnEnded(Error) variant.
                TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::EndTurn)
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
    let new = match dir {
        ScrollDir::LineUp => cur + 1,
        ScrollDir::LineDown => cur - 1,
        ScrollDir::PageUp => cur + viewport_height as i64,
        ScrollDir::PageDown => cur - viewport_height as i64,
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
    use lingxi_permission::PermissionMode;
    use std::path::PathBuf;

    fn s() -> AppState {
        AppState::new(StatusSnapshot {
            model: "claude-sonnet-4.5".into(),
            cwd: PathBuf::from("/a/b"),
            cost: "$0.0000".to_string(),
            context_pct: 0.0,
            permission_mode: PermissionMode::Default,
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

    #[test]
    fn pgup_increments_scroll_offset_by_viewport() {
        let mut st = s();
        for i in 0..30 {
            st.push_message(RenderedMessage::UserText {
                body: format!("m{i}"),
                timestamp: 0,
            });
        }
        scroll_with_viewport(&mut st, ScrollDir::PageUp, 10);
        assert_eq!(st.scroll_offset, 10);
        scroll_with_viewport(&mut st, ScrollDir::PageUp, 10);
        assert_eq!(st.scroll_offset, 20);
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

    /// Build a `RegistrySlashDispatcher` seeded with the M5-09 built-ins.
    fn dispatcher() -> lingxi_commands::dispatcher::RegistrySlashDispatcher {
        use lingxi_commands::dispatcher::RegistrySlashDispatcher;
        use lingxi_commands::registry::{register_all_builtin_commands, CommandRegistry};
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
        let should_run = handle_submit_line(&mut st, "/clear", &disp).await;
        assert!(!should_run);
        assert!(st.messages.is_empty());
    }

    #[tokio::test]
    async fn slash_exit_sets_should_exit() {
        let mut st = s();
        let disp = dispatcher();
        handle_submit_line(&mut st, "/exit", &disp).await;
        assert!(st.should_exit);
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
        assert!(rendered.contains("● hi"), "got: {rendered}");
        assert!(rendered.contains("claude-sonnet-4.5"));
    }

    #[tokio::test]
    async fn slash_help_pushes_system_message() {
        let mut st = s();
        let disp = dispatcher();
        // /help is a M5-09 stub returning a "not implemented" display string.
        // The TUI renders that display as a SystemText regardless.
        handle_submit_line(&mut st, "/help", &disp).await;
        assert!(matches!(
            st.messages.last(),
            Some(RenderedMessage::SystemText { .. })
        ));
    }
}
