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
    let ev = rx.try_recv().expect("TurnStarted must be emitted on the tx");
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
