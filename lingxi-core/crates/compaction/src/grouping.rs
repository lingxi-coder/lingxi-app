//! Group messages into "API rounds" (one user → assistant turn boundary). Used
//! by PTL retry to truncate by-the-round instead of by-the-message.

use lingxi_protocol::ConversationMessage;

/// A contiguous range of messages forming one API round.
#[derive(Debug, Clone)]
pub struct ApiRoundGroup {
    /// Inclusive start index into the messages slice.
    pub start: usize,
    /// Exclusive end index into the messages slice.
    pub end: usize,
    /// Cheap token estimate for the round.
    pub estimated_tokens: u64,
}

/// Split a flat message list into [`ApiRoundGroup`]s on every User boundary.
#[must_use]
pub fn group_messages_by_api_round(messages: &[ConversationMessage]) -> Vec<ApiRoundGroup> {
    let mut groups = Vec::new();
    let mut start = 0usize;
    for (i, m) in messages.iter().enumerate() {
        if matches!(m, ConversationMessage::User { .. }) && i > start {
            groups.push(ApiRoundGroup {
                start,
                end: i,
                estimated_tokens: estimate_tokens_for_range(&messages[start..i]),
            });
            start = i;
        }
    }
    if start < messages.len() {
        groups.push(ApiRoundGroup {
            start,
            end: messages.len(),
            estimated_tokens: estimate_tokens_for_range(&messages[start..]),
        });
    }
    groups
}

/// Cheap token estimator: 4 chars per token across text content.
#[must_use]
pub fn estimate_tokens_for_range(msgs: &[ConversationMessage]) -> u64 {
    msgs.iter()
        .map(|m| u64::try_from(m.text_content().len()).unwrap_or(u64::MAX) / 4)
        .sum()
}
