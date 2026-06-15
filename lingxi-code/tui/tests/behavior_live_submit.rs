//! (MULTIMODAL.1) Behavior tests for the production TUI live-key turn-spawn.
//!
//! These exercise the two halves of the only missing wire that lets the live
//! key loop actually run a streaming turn on Enter:
//!
//! 1. `app::dispatch(KeyAction::Submit)` for a REAL (non-slash) prompt echoes
//!    the `UserText`, clears the prompt buffer, and RAISES `pending_turn`. The
//!    pure sync dispatcher cannot spawn the turn itself (no handle / sender).
//! 2. `root::pump_turn` (driven by the ticker `use_future`) observes the flag,
//!    drains any pasted/dragged image paths via `PasteState::take_image_paths`,
//!    calls `app::spawn_streaming_turn` (emitting `TurnStarted` on the bridge
//!    sender so the spinner appears at once), clears `pending_turn`, and stores
//!    the returned `CancellationToken` so Ctrl-C can interrupt the turn.
//!
//! Downstream of `spawn_streaming_turn`
//! (`OrchestratorHandle::run_turn_streaming_with_images` → `load_images` →
//! `ImageSource::Base64`) is already covered by the orchestrator tests, so here
//! we assert only the trigger + tx plumbing that was previously missing.

use std::sync::Arc;

use orchestrator::test_support::MockOrchestratorHandle;
use tokio::sync::Mutex;
use traits::OrchestratorHandle;

use tui::app::dispatch;
use tui::components::prompt_input::{process_paste, PasteState};
use tui::events::keymap::KeyAction;
use tui::events::orchestrator_bridge::TurnEvent;
use tui::root::pump_turn;
use tui::state::{AppState, RenderedMessage, StatusSnapshot};

fn fresh_state() -> AppState {
    AppState::new(StatusSnapshot::default())
}

fn handle() -> Arc<dyn OrchestratorHandle> {
    Arc::new(MockOrchestratorHandle::new())
}

// ---- Half 1: the sync submit raises `pending_turn` (no handle involved) ----

#[test]
fn submit_real_prompt_raises_pending_turn_and_echoes_user_text() {
    let mut st = fresh_state();
    st.prompt_text = "summarize this repo".to_string();
    st.prompt_cursor = st.prompt_text.len();

    let runs_turn = dispatch(KeyAction::Submit, &mut st);

    // The dispatcher signals "run a turn" AND records the line for the pump.
    assert!(runs_turn, "real prompt Submit must return true");
    assert_eq!(st.pending_turn.as_deref(), Some("summarize this repo"));
    // Prompt buffer cleared; the line was echoed as UserText.
    assert!(st.prompt_text.is_empty());
    assert_eq!(st.prompt_cursor, 0);
    assert!(matches!(
        st.messages.last(),
        Some(RenderedMessage::UserText { body, .. }) if body == "summarize this repo"
    ));
}

#[test]
fn submit_empty_prompt_does_not_raise_pending_turn() {
    let mut st = fresh_state();
    // Empty buffer: the Submit arm bails before raising anything.
    let runs_turn = dispatch(KeyAction::Submit, &mut st);
    assert!(!runs_turn);
    assert!(st.pending_turn.is_none());
}

// ---- Half 2: the pump spawns the streaming turn + drains image paths ----

#[tokio::test]
async fn pump_turn_spawns_turn_clears_flag_and_emits_turn_started() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    let st = Arc::new(Mutex::new(fresh_state()));
    {
        let mut g = st.lock().await;
        g.pending_turn = Some("hello".to_string());
    }

    let spawned = pump_turn(&st, &handle(), &tx).await;

    assert!(spawned, "pump_turn must spawn when a turn is pending");
    let g = st.lock().await;
    // Flag consumed so the next tick does not double-spawn.
    assert!(g.pending_turn.is_none());
    // Cancel token stored so Ctrl-C (`handle_ctrl_c`) can interrupt the turn.
    assert!(
        g.cancel_token.is_some(),
        "pump_turn must store the cancel token"
    );
    drop(g);

    // `spawn_streaming_turn` emits TurnStarted SYNCHRONOUSLY before spawning,
    // so the spinner shows immediately — it is already on the channel.
    let ev = rx
        .try_recv()
        .expect("TurnStarted must be emitted on the tx");
    assert!(matches!(ev, TurnEvent::TurnStarted));
}

#[tokio::test]
async fn pump_turn_drains_pasted_image_paths() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    let st = Arc::new(Mutex::new(fresh_state()));
    {
        let mut g = st.lock().await;
        // Record a pasted image attachment (drag/paste of an image file path),
        // exactly as the live paste path does via `apply_block` → `process_paste`.
        g.paste = process_paste("/tmp/screenshot.png", std::mem::take(&mut g.paste)).state;
        // Sanity: the registry now holds a drainable image path.
        let mut probe = g.paste.clone();
        assert_eq!(
            probe.take_image_paths(),
            vec![std::path::PathBuf::from("/tmp/screenshot.png")],
            "setup: the paste registry should hold the image path"
        );
        g.pending_turn = Some("look at [Image #1]".to_string());
    }

    let spawned = pump_turn(&st, &handle(), &tx).await;
    assert!(spawned);

    // The pump drained the registry (the paths rode into the streaming turn via
    // `take_image_paths` → `spawn_streaming_turn` → `run_turn_streaming_with_images`),
    // so the next prompt starts with a fresh, empty attachment registry.
    let mut g = st.lock().await;
    assert_eq!(g.paste, PasteState::default());
    assert!(g.paste.take_image_paths().is_empty());
}

// ---- Priority / single-turn guard: the pump never spawns over a screen ----

#[tokio::test]
async fn pump_turn_is_a_noop_while_a_screen_owns_the_surface() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    let st = Arc::new(Mutex::new(fresh_state()));
    {
        let mut g = st.lock().await;
        g.pending_turn = Some("hello".to_string());
        // A full-page screen owns the surface (priority 2).
        g.active_screen = Some(tui::screens::Screen::Help(
            tui::screens::help::HelpState::new(),
        ));
    }

    let spawned = pump_turn(&st, &handle(), &tx).await;

    assert!(!spawned, "pump_turn must not spawn over an open screen");
    let g = st.lock().await;
    // Flag left set so a later tick retries once the screen closes.
    assert_eq!(g.pending_turn.as_deref(), Some("hello"));
    assert!(g.cancel_token.is_none());
    drop(g);
    // No TurnStarted emitted.
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn pump_turn_is_a_noop_when_nothing_is_pending() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TurnEvent>();
    let st = Arc::new(Mutex::new(fresh_state()));
    let spawned = pump_turn(&st, &handle(), &tx).await;
    assert!(!spawned);
    assert!(st.lock().await.cancel_token.is_none());
}

// ============================================================================
// TUI live-loop slash-command routing: /clear, /exit, /quit, /compact.
//
// The four immediate local commands claude-code `handlePromptSubmit` (~229)
// executes inline on a leading-slash submit. `/clear`/`/exit`/`/quit` are fully
// SYNCHRONOUS in `dispatch(Submit)`; `/compact` is ASYNC via `pump_compact`.
// ============================================================================

mod slash_routing {
    use super::*;

    use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use orchestrator::test_support::MockOrchestratorHandle;
    use traits::CompactionSummary;

    use tui::root::{handle_live_key, pump_compact};

    fn key(code: KeyCode) -> KeyEvent {
        let mut k = KeyEvent::new(KeyEventKind::Press, code);
        k.modifiers = KeyModifiers::NONE;
        k
    }

    // ---- dispatch-level: each command's sync effect + turn suppression ----

    #[test]
    fn slash_clear_empties_messages_and_suppresses_turn() {
        let mut st = fresh_state();
        st.push_message(RenderedMessage::AssistantText {
            body: "stale".into(),
            timestamp: 0,
        });
        st.scroll_offset = 7;
        st.prompt_text = "/clear".to_string();
        st.prompt_cursor = st.prompt_text.len();

        let runs_turn = dispatch(KeyAction::Submit, &mut st);

        assert!(!runs_turn, "/clear must suppress the turn");
        assert!(st.messages.is_empty(), "/clear wipes the scrollback");
        assert_eq!(st.scroll_offset, 0, "/clear resets the scroll offset");
        assert!(st.prompt_text.is_empty(), "prompt buffer cleared");
        assert_eq!(st.prompt_cursor, 0);
        assert!(st.pending_turn.is_none(), "no turn queued");
    }

    #[test]
    fn slash_clear_with_trailing_space_from_palette_accept_still_fires() {
        // The palette Accept path rewrites the buffer to "/clear " (trailing
        // space). The `.trim()` match must still intercept it.
        let mut st = fresh_state();
        st.push_message(RenderedMessage::AssistantText {
            body: "stale".into(),
            timestamp: 0,
        });
        st.prompt_text = "/clear ".to_string();
        st.prompt_cursor = st.prompt_text.len();

        let runs_turn = dispatch(KeyAction::Submit, &mut st);

        assert!(!runs_turn);
        assert!(st.messages.is_empty());
        assert!(st.prompt_text.is_empty());
    }

    #[test]
    fn slash_exit_sets_should_exit_and_suppresses_turn() {
        let mut st = fresh_state();
        st.prompt_text = "/exit".to_string();
        st.prompt_cursor = st.prompt_text.len();

        let runs_turn = dispatch(KeyAction::Submit, &mut st);

        assert!(!runs_turn, "/exit must suppress the turn");
        assert!(st.should_exit, "/exit flips should_exit");
        assert!(st.prompt_text.is_empty());
        assert!(st.pending_turn.is_none());
    }

    #[test]
    fn slash_quit_alias_sets_should_exit_and_suppresses_turn() {
        let mut st = fresh_state();
        st.prompt_text = "/quit".to_string();
        st.prompt_cursor = st.prompt_text.len();

        let runs_turn = dispatch(KeyAction::Submit, &mut st);

        assert!(!runs_turn, "/quit (the /exit alias) must suppress the turn");
        assert!(st.should_exit, "/quit flips should_exit");
        assert!(st.prompt_text.is_empty());
        assert!(st.pending_turn.is_none());
    }

    #[test]
    fn slash_compact_raises_pending_compact_and_suppresses_turn() {
        let mut st = fresh_state();
        st.prompt_text = "/compact".to_string();
        st.prompt_cursor = st.prompt_text.len();

        let runs_turn = dispatch(KeyAction::Submit, &mut st);

        assert!(!runs_turn, "/compact must suppress the turn");
        assert!(st.pending_compact, "/compact raises pending_compact");
        assert!(st.prompt_text.is_empty());
        assert!(st.pending_turn.is_none(), "no streaming turn queued");
    }

    #[test]
    fn plain_prompt_mentioning_clear_still_runs_a_turn() {
        // SAFETY INVARIANT: an ordinary prompt that merely MENTIONS a command
        // word is NOT intercepted — it runs a normal turn.
        let mut st = fresh_state();
        st.prompt_text = "please clear the build cache".to_string();
        st.prompt_cursor = st.prompt_text.len();

        let runs_turn = dispatch(KeyAction::Submit, &mut st);

        assert!(runs_turn, "a plain prompt must run a turn");
        assert_eq!(
            st.pending_turn.as_deref(),
            Some("please clear the build cache")
        );
        assert!(!st.should_exit, "a plain prompt never flips should_exit");
        assert!(
            !st.pending_compact,
            "a plain prompt never raises pending_compact"
        );
        assert!(matches!(
            st.messages.last(),
            Some(RenderedMessage::UserText { body, .. })
                if body == "please clear the build cache"
        ));
    }

    // ---- live-key level: drive the real `handle_live_key` (palette + Enter) --

    #[test]
    fn live_clear_reaches_dispatch_via_palette_accept_two_enters() {
        // `/clear` HAS a palette row → first Enter is owned by the palette
        // (Accept rewrites the buffer to "/clear " + closes it); the second
        // Enter reaches dispatch where the `.trim()` match fires.
        let mut st = fresh_state();
        st.push_message(RenderedMessage::AssistantText {
            body: "stale".into(),
            timestamp: 0,
        });
        for ch in "/clear".chars() {
            handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
        }
        assert!(st.palette.open, "/clear opens the palette");
        assert!(
            st.palette.rows().iter().any(|r| r.name == "clear"),
            "the palette has a clear row"
        );

        // First Enter: palette Accept rewrites to "/clear " and closes.
        handle_live_key(&mut st, &key(KeyCode::Enter), 24);
        assert_eq!(st.prompt_text, "/clear ");
        assert!(!st.palette.open, "palette closed after Accept");
        assert!(
            !st.messages.is_empty(),
            "first Enter does NOT clear yet (it only completed the palette)"
        );

        // Second Enter: reaches dispatch → `/clear` fires.
        handle_live_key(&mut st, &key(KeyCode::Enter), 24);
        assert!(st.messages.is_empty(), "second Enter ran /clear");
        assert!(st.prompt_text.is_empty());
    }

    #[test]
    fn live_exit_reaches_dispatch_via_palette_accept_two_enters() {
        let mut st = fresh_state();
        for ch in "/exit".chars() {
            handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
        }
        assert!(st.palette.open);
        assert!(st.palette.rows().iter().any(|r| r.name == "exit"));

        handle_live_key(&mut st, &key(KeyCode::Enter), 24);
        assert_eq!(st.prompt_text, "/exit ");
        assert!(!st.palette.open);
        assert!(!st.should_exit, "first Enter only completed the palette");

        handle_live_key(&mut st, &key(KeyCode::Enter), 24);
        assert!(st.should_exit, "second Enter ran /exit");
        assert!(st.prompt_text.is_empty());
    }

    #[test]
    fn live_quit_submits_on_a_single_enter_no_palette_match() {
        // No builtin command name contains 'q', so the fuzzy filter yields zero
        // rows → palette Enter is PassThrough → dispatch fires on the FIRST Enter.
        let mut st = fresh_state();
        for ch in "/quit".chars() {
            handle_live_key(&mut st, &key(KeyCode::Char(ch)), 24);
        }
        assert!(st.palette.open, "the leading / opens the palette");
        assert!(
            st.palette.rows().is_empty(),
            "no name matches 'quit' → zero rows"
        );

        handle_live_key(&mut st, &key(KeyCode::Enter), 24);
        assert!(st.should_exit, "single Enter ran /quit");
        assert!(st.prompt_text.is_empty());
    }

    // ---- pump level: pump_compact runs force_compact + pushes a boundary -----

    #[tokio::test]
    async fn pump_compact_runs_force_compact_and_pushes_compact_boundary() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_compact_summary(CompactionSummary {
            messages_before: 42,
            messages_after: 6,
            bytes_saved: 1234,
        });
        let handle: Arc<dyn OrchestratorHandle> = mock;

        let st = Arc::new(Mutex::new(fresh_state()));
        st.lock().await.pending_compact = true;

        let ran = pump_compact(&st, &handle).await;

        assert!(ran, "pump_compact runs when pending_compact is set");
        let g = st.lock().await;
        assert!(!g.pending_compact, "flag consumed so it does not re-run");
        // Mirrors the bridge `CompactionCompleted` handler: a CompactBoundary
        // carrying the summary counts.
        assert!(matches!(
            g.messages.last(),
            Some(RenderedMessage::CompactBoundary {
                messages_before: 42,
                messages_after: 6,
            })
        ));
    }

    #[tokio::test]
    async fn pump_compact_pushes_error_system_text_on_failure() {
        let mock = Arc::new(MockOrchestratorHandle::new());
        mock.set_compact_error("model 429".to_string());
        let handle: Arc<dyn OrchestratorHandle> = mock;

        let st = Arc::new(Mutex::new(fresh_state()));
        st.lock().await.pending_compact = true;

        let ran = pump_compact(&st, &handle).await;

        assert!(ran);
        let g = st.lock().await;
        assert!(matches!(
            g.messages.last(),
            Some(RenderedMessage::SystemText { is_error: true, body, .. })
                if body.starts_with("Could not compact:")
        ));
    }

    #[tokio::test]
    async fn pump_compact_is_a_noop_while_a_screen_owns_the_surface() {
        let st = Arc::new(Mutex::new(fresh_state()));
        {
            let mut g = st.lock().await;
            g.pending_compact = true;
            // A full-page screen owns the surface (priority 2).
            g.active_screen = Some(tui::screens::Screen::Help(
                tui::screens::help::HelpState::new(),
            ));
        }

        let ran = pump_compact(&st, &handle()).await;

        assert!(!ran, "pump_compact must not run over an open screen");
        let g = st.lock().await;
        assert!(
            g.pending_compact,
            "flag left set so a later tick retries once the screen closes"
        );
        // No boundary / error pushed.
        assert!(g.messages.is_empty());
    }

    #[tokio::test]
    async fn pump_compact_is_a_noop_when_nothing_is_pending() {
        let st = Arc::new(Mutex::new(fresh_state()));
        let ran = pump_compact(&st, &handle()).await;
        assert!(!ran);
        assert!(st.lock().await.messages.is_empty());
    }
}
