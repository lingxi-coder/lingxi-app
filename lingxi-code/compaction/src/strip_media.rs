//! Pre-summarization media stripping — port of `stripImagesFromMessages`
//! (`compact.ts:145-200`).
//!
//! Before the auto-compact summary request, image and document content blocks
//! in USER messages are replaced with a text placeholder (`[image]` /
//! `[document]`) so the text summarizer never receives raw media data.
//! claude-code also strips media nested inside `tool_result` content arrays, but
//! the Rust `protocol`'s `ToolResult.content` is a flat `String` — so that case
//! has no representation here (a documented divergence: nothing to strip). Only
//! `user` messages are touched, matching `compact.ts:147`.

use protocol::{ContentBlock, ConversationMessage};

/// Text placeholder substituted for a stripped image block — byte-locked to
/// claude-code (`compact.ts:160`).
pub const STRIPPED_IMAGE_PLACEHOLDER: &str = "[image]";

/// Text placeholder substituted for a stripped document block — byte-locked to
/// claude-code's `document` strip placeholder.
pub const STRIPPED_DOCUMENT_PLACEHOLDER: &str = "[document]";

/// Replace image blocks in user messages with a `[image]` text placeholder.
/// Non-user messages and non-image blocks pass through unchanged. 1:1 with
/// `stripImagesFromMessages` for the cases the Rust protocol can represent.
#[must_use]
pub fn strip_images_from_messages(messages: Vec<ConversationMessage>) -> Vec<ConversationMessage> {
    messages.into_iter().map(strip_one).collect()
}

fn strip_one(message: ConversationMessage) -> ConversationMessage {
    // TS strips only `user` messages (`compact.ts:147`); others pass through.
    let ConversationMessage::User {
        id,
        content,
        is_meta,
    } = message
    else {
        return message;
    };
    let new_content = content
        .into_iter()
        .map(|block| match block {
            ContentBlock::Image { .. } => ContentBlock::Text {
                text: STRIPPED_IMAGE_PLACEHOLDER.to_string(),
            },
            ContentBlock::Document { .. } => ContentBlock::Text {
                text: STRIPPED_DOCUMENT_PLACEHOLDER.to_string(),
            },
            other => other,
        })
        .collect();
    ConversationMessage::User {
        id,
        content: new_content,
        is_meta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ContentBlock, ConversationMessage, ImageSource, MessageId};

    fn image_block() -> ContentBlock {
        ContentBlock::Image {
            source: ImageSource::Url {
                url: "https://img.example/x.png".to_string(),
            },
        }
    }

    #[test]
    fn user_image_block_becomes_placeholder_text() {
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![
                ContentBlock::Text {
                    text: "before".to_string(),
                },
                image_block(),
                ContentBlock::Text {
                    text: "after".to_string(),
                },
            ],
            is_meta: false,
        };
        let out = strip_images_from_messages(vec![msg]);
        let ConversationMessage::User { content, .. } = &out[0] else {
            panic!("expected user message");
        };
        // The image became a `[image]` text block; surrounding text is intact
        // and block order/count is preserved.
        assert_eq!(content.len(), 3);
        assert!(matches!(&content[0], ContentBlock::Text { text } if text == "before"));
        assert!(matches!(&content[1], ContentBlock::Text { text } if text == "[image]"));
        assert!(matches!(&content[2], ContentBlock::Text { text } if text == "after"));
    }

    #[test]
    fn message_without_media_is_untouched() {
        let msg = ConversationMessage::user(MessageId::new(), "just text".to_string());
        let out = strip_images_from_messages(vec![msg.clone()]);
        assert_eq!(out[0], msg);
    }

    #[test]
    fn assistant_messages_are_not_stripped() {
        // TS only strips `user` messages; an assistant message is returned as-is
        // even if it (hypothetically) carried an image block.
        let assistant = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![image_block()],
            stop_reason: None,
        };
        let out = strip_images_from_messages(vec![assistant.clone()]);
        assert_eq!(out[0], assistant);
    }
}
