//! Behavior test (M6-02 T13): type "/clear" + Enter → AppState.messages
//! is empty.
//!
//! Per the T0 decision, `Submit` pushes a UserText("/clear") into the
//! scrollback unconditionally; then `handle_submit_line` intercepts the
//! command and wipes everything (including the just-pushed entry).
//! Net effect: empty scrollback.

use lingxi_tui::app::{dispatch, handle_submit_line};
use lingxi_tui::events::keymap::KeyAction;
use lingxi_tui::state::{AppState, RenderedMessage};

mod support;
use support::{fake_dispatcher, fake_status};

#[tokio::test]
async fn slash_clear_empties_scrollback() {
    let mut st = AppState::new(fake_status());
    st.push_message(RenderedMessage::AssistantText {
        body: "stale".into(),
        timestamp: 0,
    });
    st.push_message(RenderedMessage::UserText {
        body: "older".into(),
        timestamp: 0,
    });
    assert_eq!(st.messages.len(), 2);

    // Type "/clear".
    for c in "/clear".chars() {
        dispatch(KeyAction::InsertChar(c), &mut st);
    }
    assert_eq!(st.prompt_text, "/clear");

    // Submit pushes UserText("/clear") and clears prompt.
    let line = st.prompt_text.clone();
    dispatch(KeyAction::Submit, &mut st);
    // The dispatcher then sees the "/clear" intercept and wipes messages.
    let disp = fake_dispatcher();
    handle_submit_line(&mut st, &line, &disp).await;

    assert!(
        st.messages.is_empty(),
        "messages must be empty after /clear, got {:?}",
        st.messages
    );
    assert_eq!(st.scroll_offset, 0);
}
