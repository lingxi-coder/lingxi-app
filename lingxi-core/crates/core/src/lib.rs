//! Core conversation state machine.
//!
//! - `events::Event` — inputs to the reducer
//! - `state_machine::ConversationState` — the state set
//! - `reducer::reduce` — pure-function state transitions
//! - `prompt::assemble_request` — build the API request body
//! - `session::SessionState` — persisted session model
//!
//! No I/O. No `tokio::spawn`. All side effects are returned as
//! `lingxi_protocol::Effect` values.

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
pub use session::{CumulativeUsage, SessionState};
pub use state_machine::ConversationState;
pub use token::Usage;
