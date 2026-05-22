//! Conversation message DTOs.
//!
//! Mirrors `Anthropic`'s `Message` content-block model but is API-neutral —
//! provider adapters in `lingxi-api-client` map their native shapes to these.

use crate::ids::{MessageId, ToolUseId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// High-level role of a conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// Message from the human user.
    User,
    /// Message produced by the assistant model.
    Assistant,
    /// Out-of-band system instructions.
    System,
}

/// One block of structured content within a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain UTF-8 text.
    Text {
        /// The text body.
        text: String,
    },
    /// A tool invocation requested by the assistant.
    ToolUse {
        /// Identifier echoed back in the matching `ToolResult`.
        id: ToolUseId,
        /// Tool name (e.g. `"Read"`, `"Bash"`).
        name: String,
        /// Tool-specific structured input.
        input: Value,
    },
    /// The result of a previously-requested tool call.
    ToolResult {
        /// ID of the `ToolUse` this result belongs to.
        tool_use_id: ToolUseId,
        /// Stringified output payload.
        content: String,
        /// Whether the tool reported failure.
        is_error: bool,
    },
    /// Extended-thinking reasoning trace.
    Thinking {
        /// The reasoning text.
        thinking: String,
        /// Optional cryptographic signature attesting to the trace.
        signature: Option<String>,
    },
}

/// A single message in a conversation, role-tagged for serde.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ConversationMessage {
    /// User-authored message — always structured content blocks.
    User {
        /// Stable message identifier.
        id: MessageId,
        /// Ordered content blocks (typically text and/or tool results).
        content: Vec<ContentBlock>,
    },
    /// Assistant-authored message — may include tool-use blocks.
    Assistant {
        /// Stable message identifier.
        id: MessageId,
        /// Ordered content blocks.
        content: Vec<ContentBlock>,
        /// Provider-reported stop reason, if any.
        stop_reason: Option<String>,
    },
    /// System prompt — flat string, no tool blocks.
    System {
        /// Stable message identifier.
        id: MessageId,
        /// The system prompt body.
        content: String,
    },
}

impl ConversationMessage {
    /// Construct a simple user message containing a single text block.
    #[must_use]
    pub fn user(id: MessageId, text: String) -> Self {
        Self::User {
            id,
            content: vec![ContentBlock::Text { text }],
        }
    }

    /// Return the role of this message.
    #[must_use]
    pub fn role(&self) -> MessageRole {
        match self {
            Self::User { .. } => MessageRole::User,
            Self::Assistant { .. } => MessageRole::Assistant,
            Self::System { .. } => MessageRole::System,
        }
    }

    /// Return the stable identifier of this message.
    #[must_use]
    pub fn id(&self) -> MessageId {
        match self {
            Self::User { id, .. } | Self::Assistant { id, .. } | Self::System { id, .. } => *id,
        }
    }

    /// Concatenate all `Text` blocks. Returns "" if none.
    #[must_use]
    pub fn text_content(&self) -> String {
        match self {
            Self::User { content, .. } | Self::Assistant { content, .. } => content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            Self::System { content, .. } => content.clone(),
        }
    }

    /// Return `true` if this is an assistant message that requested at least one tool call.
    #[must_use]
    pub fn has_tool_use(&self) -> bool {
        match self {
            Self::Assistant { content, .. } => content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. })),
            _ => false,
        }
    }

    /// Returns references to `ToolUse` blocks; empty if not Assistant.
    #[must_use]
    pub fn tool_calls(&self) -> Vec<&ContentBlock> {
        match self {
            Self::Assistant { content, .. } => content
                .iter()
                .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_constructs() {
        let id = MessageId::new();
        let m = ConversationMessage::user(id, "hello".to_string());
        assert!(matches!(m.role(), MessageRole::User));
        assert_eq!(m.text_content(), "hello");
    }

    #[test]
    fn message_roundtrip_json() {
        let m = ConversationMessage::user(MessageId::new(), "hi".into());
        let s = serde_json::to_string(&m).unwrap();
        let m2: ConversationMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn assistant_message_with_tool_use_extracts_tool_calls() {
        let m = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![
                ContentBlock::Text {
                    text: "I'll read the file.".into(),
                },
                ContentBlock::ToolUse {
                    id: ToolUseId::new(),
                    name: "Read".into(),
                    input: serde_json::json!({"path": "/tmp/x"}),
                },
            ],
            stop_reason: None,
        };
        assert!(m.has_tool_use());
        assert_eq!(m.tool_calls().len(), 1);
    }
}
