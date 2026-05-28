//! M6-08 — TUI handles `CompactionCompleted` by appending a
//! `[Compacted N → M messages]` SystemText to scrollback.

use lingxi_tui::events::orchestrator_bridge::TurnEvent;
use lingxi_tui::state::{AppState, RenderedMessage, StatusSnapshot};
use lingxi_tui::streaming::apply_event;
use tokio::sync::Notify;

#[tokio::test]
async fn compaction_completed_event_appends_marker_to_scrollback() {
    let mut state = AppState::new(StatusSnapshot::default());
    let notify = Notify::new();

    apply_event(
        &mut state,
        TurnEvent::CompactionCompleted {
            messages_before: 50,
            messages_after: 2,
            bytes_saved: 12_345,
        },
        &notify,
    );

    let last = state.messages.last().expect("scrollback non-empty");
    match last {
        RenderedMessage::SystemText { body, is_error, .. } => {
            assert!(!*is_error, "compact marker should not be error-styled");
            assert!(body.contains("Compacted"), "got: {body}");
            assert!(body.contains("50"), "got: {body}");
            assert!(body.contains("2"), "got: {body}");
        }
        other => panic!("expected SystemText marker; got {other:?}"),
    }
}

#[tokio::test]
async fn bridge_translates_emit_compaction_completed_to_turn_event() {
    use lingxi_traits::OutputStream;
    use tokio::sync::mpsc;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let bridge = lingxi_tui::BridgeOutputStream::new(tx);
    bridge.emit_compaction_completed(40, 3, 9_999).await;

    let ev = rx.recv().await.expect("event received");
    assert!(
        matches!(
            ev,
            TurnEvent::CompactionCompleted {
                messages_before: 40,
                messages_after: 3,
                bytes_saved: 9_999
            }
        ),
        "got: {ev:?}"
    );
}
