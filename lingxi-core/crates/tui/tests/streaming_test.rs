//! End-to-end streaming + cancel tests for the TUI app. (M6-03 Task 8)

use lingxi_tui::app::handle_ctrl_c;
use lingxi_tui::events::orchestrator_bridge::TurnEvent;
use lingxi_tui::state::{AppState, RenderedMessage, StatusSnapshot, StreamingState};
use lingxi_tui::streaming::apply_event;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn new_state() -> AppState {
    AppState::new(StatusSnapshot::default())
}

#[tokio::test]
async fn five_deltas_with_50ms_gap_concatenate_correctly() {
    let mut state = new_state();
    let notify = Notify::new();
    apply_event(&mut state, TurnEvent::TurnStarted, &notify);
    let chunks = ["h", "el", "lo ", "wor", "ld"];
    for c in chunks {
        apply_event(&mut state, TurnEvent::TextDelta(c.into()), &notify);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    apply_event(
        &mut state,
        TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::EndTurn),
        &notify,
    );

    assert_eq!(state.messages.len(), 1);
    match state.messages.last().unwrap() {
        RenderedMessage::AssistantText { body, .. } => {
            assert_eq!(body, "hello world");
        }
        other => panic!("unexpected last message: {other:?}"),
    }
    assert!(state.streaming.is_none());
}

#[tokio::test]
async fn spinner_mount_predicate_tracks_streaming_field() {
    let mut state = new_state();
    let notify = Notify::new();
    assert!(state.streaming.is_none());
    apply_event(&mut state, TurnEvent::TurnStarted, &notify);
    assert!(state.streaming.is_some());
    apply_event(
        &mut state,
        TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::EndTurn),
        &notify,
    );
    assert!(state.streaming.is_none());
}

#[tokio::test]
async fn ctrl_c_cancels_within_100ms_and_clears_streaming() {
    let mut state = new_state();
    let notify = Notify::new();
    let cancel = CancellationToken::new();
    apply_event(&mut state, TurnEvent::TurnStarted, &notify);
    state.cancel_token = Some(cancel.clone());

    let start = std::time::Instant::now();
    handle_ctrl_c(&mut state);
    // The orchestrator task will detect cancellation and eventually emit
    // TurnEnded. Simulate that path:
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    apply_event(
        &mut state,
        TurnEvent::TurnEnded(lingxi_traits::TurnOutcome::Cancelled),
        &notify,
    );

    assert!(cancel.is_cancelled());
    assert!(state.streaming.is_none());
    assert!(state.cancel_token.is_none());
    assert!(start.elapsed() < std::time::Duration::from_millis(100));
}

#[tokio::test]
async fn ctrl_c_with_no_streaming_is_noop() {
    let mut state = new_state();
    // No streaming, no cancel token — must not panic.
    handle_ctrl_c(&mut state);
    assert!(state.streaming.is_none());
    assert!(state.cancel_token.is_none());
}

#[tokio::test]
async fn streaming_state_started_at_is_recent() {
    let s = StreamingState::new();
    let elapsed = s.started_at.elapsed();
    assert!(elapsed < std::time::Duration::from_secs(1));
}

#[test]
fn repl_render_includes_spinner_when_streaming() {
    let mut state = new_state();
    assert!(!lingxi_tui::screens::repl::should_render_spinner(&state));
    state.streaming = Some(StreamingState::new());
    assert!(lingxi_tui::screens::repl::should_render_spinner(&state));
}
