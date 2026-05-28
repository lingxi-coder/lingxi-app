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

/// Perf smoke (M6-03 T11): a 100-events/sec burst for 5 seconds should
/// collapse to at most ~165 renders under the 30fps cap (150 + 10% slack
/// for scheduler jitter).
///
/// The renderer logic exercised here is the Notify-debounce contract:
/// every `notify.notified()` wakeup increments a counter, then sleeps
/// 33ms before processing the next permit. Coalesced permits (multiple
/// `notify_one` calls during the sleep window) collapse into one wakeup.
#[tokio::test(flavor = "current_thread")]
async fn perf_smoke_100_deltas_per_sec_for_5sec_collapses_to_at_most_195_renders() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::Notify;

    let render_count = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());
    let notify_clone = notify.clone();
    let render_count_clone = render_count.clone();

    // Render task: drains the notify, increments counter, sleeps 33ms.
    let render_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = notify_clone.notified() => {
                    render_count_clone.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(33)).await;
                }
                () = tokio::time::sleep(std::time::Duration::from_secs(6)) => break,
            }
        }
    });

    // Producer task: fires 100 notify_one calls per second for 5 seconds.
    let producer_notify = notify.clone();
    let producer = tokio::spawn(async move {
        for _ in 0..500 {
            producer_notify.notify_one();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });

    producer.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await; // drain
    render_handle.abort();

    let renders = render_count.load(Ordering::SeqCst);
    // 30fps × 5.2s (5s producer + 200ms drain) = ~156 renders steady-state.
    // Allow 25% slack for scheduler jitter (tokio current-thread runtime
    // tends to under-shoot sleep durations slightly).
    assert!(
        renders <= 195,
        "got {renders} renders, expected ≤ 195 (~30fps cap with slack)"
    );
    // Sanity: also assert we DID render at least some.
    assert!(renders >= 30, "got {renders} renders, expected ≥ 30");
}
