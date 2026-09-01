//! Conversation state machine (cargo package `core`, rust ident `lingxi_core`).
//!
//! - `events::Event` — inputs to the reducer
//! - `state_machine::ConversationState` — the state set
//! - `reducer::reduce` — pure-function state transitions
//! - `prompt::assemble_request` — build the API request body
//! - `session::SessionState` — persisted session model
//!
//! No I/O. No `tokio::spawn`. All side effects are returned as
//! `protocol::Effect` values.

#![forbid(unsafe_code)]

pub mod events;
pub mod model;
pub mod prompt;
pub mod reducer;
pub mod session;
pub mod settings;
pub mod state_machine;
pub mod token;

pub use events::Event;
pub use reducer::reduce;
pub use session::{CumulativeUsage, SessionState, TodoItem, TodoState};
pub use state_machine::ConversationState;
pub use token::Usage;
