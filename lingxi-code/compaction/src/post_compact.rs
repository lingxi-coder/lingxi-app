//! Builds the post-compact message set: the boundary summary plus restored
//! file/skill attachments. Plans 09 (skills) and 10 (session) inject real data;
//! M1.7 ships the shape only.

use protocol::ConversationMessage;

/// Output of [`PostCompactBuilder::build`].
#[derive(Debug, Clone)]
pub struct PostCompactMessages {
    /// Messages to insert at the compact boundary (typically one system msg).
    pub summary_messages: Vec<ConversationMessage>,
    /// Untyped attachment payloads (filled in by later plans).
    pub attachments: Vec<serde_json::Value>,
}

/// Stateless builder for the post-compact boundary.
pub struct PostCompactBuilder;

impl PostCompactBuilder {
    /// Restore recent files + active skills attachments. Plans 09 (skills) /
    /// 10 (session) inject real data; this M1.7 ships the shape.
    #[must_use]
    pub fn build(summary_text: &str) -> PostCompactMessages {
        PostCompactMessages {
            summary_messages: vec![ConversationMessage::System {
                id: protocol::MessageId::new(),
                content: format!("Compact boundary:\n{summary_text}"),
            }],
            attachments: Vec::new(),
        }
    }
}
