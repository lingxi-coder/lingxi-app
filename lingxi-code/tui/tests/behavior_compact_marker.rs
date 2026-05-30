//! M7-04 T4 — TUI handles `CompactionCompleted` by appending a real
//! `CompactBoundary` variant (rendered `✻ Conversation compacted
//! (ctrl+o for history)`), replacing M6-08's `[Compacted N → M messages]`
//! `SystemText` placeholder.

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
            messages_after: 5,
            bytes_saved: 4096,
        },
        &notify,
    );

    // M7-04 T4: CompactionCompleted now pushes a CompactBoundary variant,
    // not a SystemText `[Compacted …]` placeholder.
    let last = state.messages.last().expect("a message was pushed");
    assert!(
        matches!(
            last,
            RenderedMessage::CompactBoundary {
                messages_before: 50,
                messages_after: 5
            }
        ),
        "expected CompactBoundary, got: {last:?}",
    );
    // The rendered line is the locked boundary string (no counts).
    let rendered = lingxi_tui::components::messages::render_entry_to_string(last, false, false);
    assert_eq!(rendered, "✻ Conversation compacted (ctrl+o for history)");
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
