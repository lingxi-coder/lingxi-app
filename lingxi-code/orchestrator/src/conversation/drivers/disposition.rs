//! Shared end-of-turn signal handling (§4 / §5.3).
//!
//! The two drivers reach their terminal state differently and the plan keeps
//! that difference on purpose, so what is shared here is the CONSUMPTION of each
//! signal — not the order the signals are consulted in. Order stays with each
//! driver, because it is observable: batched resolves the lone wakeup before the
//! EndConversation slot, streaming the other way round, and the two leave
//! different next-turn state as a result
//! (`tests/turn_end_conversation_boundary_test.rs`).

use super::*;

impl ConversationOrchestrator {
    /// Consume a pending `EndConversation` request, surfacing the user-facing
    /// end message when one fired.
    ///
    /// Like `take_lone_wakeup_turn_end`, this CONSUMES by being called: the swap
    /// runs whether or not a request was pending, so a driver that calls it
    /// speculatively has already cleared the slot. Unlike the wakeup flag,
    /// nothing guards this call today — both drivers reach it unconditionally on
    /// the tool-round path, so the slot never survives a turn that got that far.
    ///
    /// WHERE it is called is the driver's business and is not shared: batched
    /// calls it after resolving the lone wakeup, streaming before, and that
    /// asymmetry decides whether a wakeup arming survives the turn.
    pub(crate) async fn take_end_conversation_request(&self) -> bool {
        let requested = self
            .end_conversation_slot
            .as_ref()
            .is_some_and(|slot| slot.swap(false, std::sync::atomic::Ordering::SeqCst));
        if requested {
            self.output
                .emit_text(crate::prompt::end_conversation::END_CONVERSATION_ENDED_MESSAGE)
                .await;
        }
        requested
    }
}
