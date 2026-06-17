//! Group messages into "API rounds" (one API round-trip per group). Used by
//! PTL retry to truncate by-the-round instead of by-the-message.
//!
//! Port of TS `groupMessagesByApiRound` (`grouping.ts:22-63`): a boundary fires
//! when a **new assistant response** begins — i.e. an `Assistant` message whose
//! `id` differs from the most-recently-seen assistant `id`, with the current
//! group non-empty. Streaming chunks from the same API response share an id, so
//! interleaved `[tool_use(id=X), tool_result, tool_use(id=X)]` stays one group;
//! two distinct assistant ids produce two groups; a preamble (no assistant, or a
//! single assistant) collapses to one group.
//!
//! This replaces the prior "split on every user message" boundary, matching TS
//! so PTL head-truncation drops the correct round boundaries.

use protocol::{ConversationMessage, MessageId};

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

/// Split a flat message list into [`ApiRoundGroup`]s on each **new assistant
/// message id** (TS `groupMessagesByApiRound`).
///
/// A boundary is placed before an `Assistant` message whose `id` differs from
/// the previously-seen assistant `id`, provided the current group is non-empty.
#[must_use]
pub fn group_messages_by_api_round(messages: &[ConversationMessage]) -> Vec<ApiRoundGroup> {
    let mut groups = Vec::new();
    let mut start = 0usize;
    // `id` of the most-recently-seen assistant message. Sole boundary gate:
    // streaming chunks from one API response share an id, so a boundary only
    // fires at the start of a genuinely new round.
    let mut last_assistant_id: Option<MessageId> = None;

    for (i, m) in messages.iter().enumerate() {
        if let ConversationMessage::Assistant { id, .. } = m {
            // Boundary: a new assistant id begins and the current group is
            // non-empty (`i > start`).
            if last_assistant_id != Some(*id) && i > start {
                groups.push(ApiRoundGroup {
                    start,
                    end: i,
                    estimated_tokens: estimate_tokens_for_range(&messages[start..i]),
                });
                start = i;
            }
            last_assistant_id = Some(*id);
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

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ContentBlock, MessageId, ToolUseId};
    use serde_json::json;

    fn assistant_with_id(id: MessageId, tool: &str) -> ConversationMessage {
        ConversationMessage::Assistant {
            id,
            content: vec![ContentBlock::ToolUse {
                id: ToolUseId::new(),
                name: tool.into(),
                input: json!({}),
                provider_id: None,
            }],
            stop_reason: None,
        }
    }

    fn user_result(tool_use_id: ToolUseId) -> ConversationMessage {
        ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id,
                content: "ok".into(),
                is_error: false,
                provider_tool_use_id: None,
            }],
            is_meta: false,
        }
    }

    /// Interleaved `[tu_A(id=X), result_A, tu_B(id=X)]` stays one group — the
    /// two assistant blocks share an id, so no boundary fires.
    #[test]
    fn interleaved_same_id_one_group() {
        let id = MessageId::new();
        let tua = ToolUseId::new();
        let msgs = vec![
            assistant_with_id(id, "Read"),
            user_result(tua),
            assistant_with_id(id, "Bash"),
        ];
        let groups = group_messages_by_api_round(&msgs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].start, 0);
        assert_eq!(groups[0].end, 3);
    }

    /// Two distinct assistant ids → two groups.
    #[test]
    fn two_distinct_ids_two_groups() {
        let id_a = MessageId::new();
        let id_b = MessageId::new();
        let msgs = vec![
            assistant_with_id(id_a, "Read"),
            user_result(ToolUseId::new()),
            assistant_with_id(id_b, "Bash"),
            user_result(ToolUseId::new()),
        ];
        let groups = group_messages_by_api_round(&msgs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].start, 0);
        assert_eq!(groups[0].end, 2);
        assert_eq!(groups[1].start, 2);
        assert_eq!(groups[1].end, 4);
    }

    /// Leading user messages (a preamble) then a *first* assistant: TS splits
    /// the preamble off because the first assistant's id differs from the
    /// (undefined) prior id and `current` is already non-empty. → 2 groups.
    /// This matches TS `groupMessagesByApiRound` byte-for-byte.
    #[test]
    fn preamble_then_first_assistant_splits() {
        let msgs = vec![
            ConversationMessage::user(MessageId::new(), "hello".into()),
            ConversationMessage::user(MessageId::new(), "world".into()),
            assistant_with_id(MessageId::new(), "Read"),
        ];
        let groups = group_messages_by_api_round(&msgs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].start, 0);
        assert_eq!(groups[0].end, 2);
        assert_eq!(groups[1].start, 2);
        assert_eq!(groups[1].end, 3);
    }

    /// Preamble-only (no assistant message at all) → one group.
    #[test]
    fn no_assistant_one_group() {
        let msgs = vec![
            ConversationMessage::user(MessageId::new(), "a".into()),
            ConversationMessage::user(MessageId::new(), "b".into()),
        ];
        let groups = group_messages_by_api_round(&msgs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].end, 2);
    }

    /// Empty input → no groups.
    #[test]
    fn empty_no_groups() {
        let groups = group_messages_by_api_round(&[]);
        assert!(groups.is_empty());
    }
}
