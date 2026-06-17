//! Tiny extension helper: estimate the "useful payload" byte size of a
//! [`ConversationMessage`]. Used by M6-08 to compute the `bytes_saved`
//! field of `CompactionSummary` (UX estimate only — the exact cost
//! accounting lives in `lingxi-cost`).

use crate::{ContentBlock, ConversationMessage};

/// Returns the sum, in bytes, of every text payload carried by `msg`.
///
/// Tool-use input JSON and tool-result content are sized as the
/// serialized JSON length (best-effort; falls back to 0 on serializer
/// error).
#[must_use]
pub fn text_byte_size(msg: &ConversationMessage) -> u64 {
    match msg {
        ConversationMessage::User { content, .. }
        | ConversationMessage::Assistant { content, .. } => {
            content.iter().map(content_block_size).sum()
        }
        ConversationMessage::System { content, .. } => content.len() as u64,
    }
}

fn content_block_size(b: &ContentBlock) -> u64 {
    match b {
        ContentBlock::Text { text, .. } => text.len() as u64,
        ContentBlock::ToolUse { input, .. } => serde_json::to_string(input)
            .map(|s| s.len() as u64)
            .unwrap_or(0),
        ContentBlock::ToolResult { content, .. } => serde_json::to_string(content)
            .map(|s| s.len() as u64)
            .unwrap_or(0),
        ContentBlock::Thinking { .. } => 0,
        ContentBlock::Image { source } => match source {
            crate::ImageSource::Base64 { data, .. } => data.len() as u64,
            crate::ImageSource::Url { url } => url.len() as u64,
        },
        ContentBlock::Document { source } => match source {
            crate::DocumentSource::Base64 { data, .. } => data.len() as u64,
        },
        // Low-frequency server-side blocks: opaque payloads sized best-effort.
        ContentBlock::RedactedThinking { data } => data.len() as u64,
        ContentBlock::ServerToolUse { input, .. } => serde_json::to_string(input)
            .map(|s| s.len() as u64)
            .unwrap_or(0),
        ContentBlock::ConnectorText { connector_text, .. } => connector_text.len() as u64,
        ContentBlock::AdvisorToolResult { content, .. } => serde_json::to_string(content)
            .map(|s| s.len() as u64)
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentBlock, ConversationMessage, MessageId, ToolUseId};

    #[test]
    fn user_text_message_returns_string_byte_length() {
        let m = ConversationMessage::user(MessageId::new(), "hello".into());
        assert_eq!(text_byte_size(&m), 5);
    }

    #[test]
    fn system_message_returns_content_length() {
        let m = ConversationMessage::System {
            id: MessageId::new(),
            content: "abc".into(),
        };
        assert_eq!(text_byte_size(&m), 3);
    }

    #[test]
    fn image_block_sized_by_base64_data_len() {
        let m = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: crate::ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "YWJj".to_string(),
                },
            }],
        };
        assert_eq!(text_byte_size(&m), 4); // "YWJj".len()
    }

    #[test]
    fn tool_use_block_sized_as_json() {
        let m = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id: ToolUseId::new(),
                name: "Read".into(),
                input: serde_json::json!({"path": "/a"}),
                provider_id: None,
            }],
            stop_reason: Some("tool_use".into()),
        };
        // {"path":"/a"} → 13 bytes
        assert_eq!(text_byte_size(&m), 13);
    }
}
