//! Behavior test (M6-02 T11): feed h/i/Enter → `run_one_submit` → assert
//! scrollback grew by 2 entries (UserText("hi") + AssistantText("Hello!"))
//! and the prompt was cleared.

use async_trait::async_trait;
use lingxi_tui::app::{
    dispatch, run_one_submit, ConversationOrchestratorTrait, TurnTextOutcome,
};
use lingxi_tui::events::keymap::KeyAction;
use lingxi_tui::state::{AppState, RenderedMessage};
use tokio_util::sync::CancellationToken;

mod support;
use support::{fake_dispatcher, fake_status};

struct FakeOrch(&'static str);

#[async_trait]
impl ConversationOrchestratorTrait for FakeOrch {
    async fn run_turn(
        &self,
        _prompt: &str,
        _cancel: CancellationToken,
    ) -> Result<TurnTextOutcome, String> {
        Ok(TurnTextOutcome {
            text: self.0.to_string(),
        })
    }
}

#[tokio::test]
async fn feed_h_i_enter_runs_one_turn() {
    let mut st = AppState::new(fake_status());
    dispatch(KeyAction::InsertChar('h'), &mut st);
    dispatch(KeyAction::InsertChar('i'), &mut st);
    assert_eq!(st.prompt_text, "hi");

    // Submit pushes UserText + clears prompt. Capture the submitted line
    // (mirroring the per-frame loop's responsibility).
    let line = st.prompt_text.clone();
    let did_submit = dispatch(KeyAction::Submit, &mut st);
    assert!(did_submit);
    assert_eq!(st.prompt_text, "");

    let orch = FakeOrch("Hello!");
    let disp = fake_dispatcher();
    run_one_submit(&mut st, &line, &orch, &disp).await;

    assert_eq!(st.messages.len(), 2);
    assert!(matches!(
        &st.messages[0],
        RenderedMessage::UserText { body, .. } if body == "hi"
    ));
    assert!(matches!(
        &st.messages[1],
        RenderedMessage::AssistantText { body, .. } if body == "Hello!"
    ));
    assert!(st.in_flight_turn.is_none(), "in-flight turn must be cleared");
}
