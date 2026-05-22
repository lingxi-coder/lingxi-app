//! Session state model. Persistence lives in `lingxi-session` (Plan 10).

use crate::token::Usage;
use lingxi_protocol::{ConversationMessage, SessionId};
use serde::{Deserialize, Serialize};

/// Running total of `Usage` across all turns in a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CumulativeUsage(pub Usage);

impl CumulativeUsage {
    /// Accumulate a per-turn `Usage` into the cumulative total.
    pub fn add(&mut self, u: &Usage) {
        self.0.add(u);
    }
}

/// In-memory model of a conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Unique identifier for this session.
    pub session_id: SessionId,
    /// Ordered conversation history.
    pub history: Vec<ConversationMessage>,
    /// Cumulative token usage across all turns.
    pub usage: CumulativeUsage,
    /// Model identifier (e.g. `"claude-opus-4-7"`).
    pub model: String,
}

impl SessionState {
    /// Construct a fresh session with no history.
    #[must_use]
    pub fn empty(session_id: SessionId, model: String) -> Self {
        Self {
            session_id,
            history: Vec::new(),
            usage: CumulativeUsage::default(),
            model,
        }
    }
}
