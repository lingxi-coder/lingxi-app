//! Conversation state set. WILL BE FILLED IN TASK 10.
//!
//! Task 10 expands this enum with the real states (`Idle`, `Streaming`, ...).
//! This stub exists only so `pub use state_machine::ConversationState;` in
//! `lib.rs` resolves while the rest of the crate is being scaffolded.

/// Placeholder for the conversation state set. Real variants land in Task 10.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationState {
    /// Stub variant — replaced when Task 10 introduces real states.
    Placeholder,
}
