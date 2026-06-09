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
    /// An image input (vision). Serializes to the Anthropic image-block wire
    /// shape; the OpenAI/Gemini codecs translate it to their native forms.
    Image {
        /// Where the image bytes come from.
        source: ImageSource,
    },
    /// A document input (e.g. a PDF). Serializes to the Anthropic document-block
    /// wire shape; the OpenAI/Gemini codecs translate or drop it.
    Document {
        /// Where the document bytes come from.
        source: DocumentSource,
    },
}

/// Source of a [`ContentBlock::Image`]. Serializes to Anthropic's
/// `source` wire shape (`{"type":"base64",…}` / `{"type":"url",…}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Inline base64-encoded image bytes.
    Base64 {
        /// MIME type, e.g. `image/png`.
        media_type: String,
        /// Base64-encoded image bytes (no `data:` prefix).
        data: String,
    },
    /// A remote image URL the provider fetches.
    Url {
        /// The image URL.
        url: String,
    },
}

/// Source of a [`ContentBlock::Document`]. Serializes to Anthropic's `source`
/// wire shape (`{"type":"base64","media_type":"application/pdf","data":…}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocumentSource {
    /// Inline base64-encoded document bytes.
    Base64 {
        /// MIME type, e.g. `application/pdf`.
        media_type: String,
        /// Base64-encoded document bytes (no `data:` prefix).
        data: String,
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

    /// Construct a user message with a leading text block (when non-empty)
    /// followed by one [`ContentBlock::Image`] per source. Used by the TUI
    /// paste→image path; with no images this is equivalent to [`Self::user`]
    /// (modulo an empty-text message carrying no blocks).
    #[must_use]
    pub fn user_with_images(id: MessageId, text: String, images: Vec<ImageSource>) -> Self {
        let mut content = Vec::with_capacity(1 + images.len());
        if !text.is_empty() {
            content.push(ContentBlock::Text { text });
        }
        for source in images {
            content.push(ContentBlock::Image { source });
        }
        Self::User { id, content }
    }

    /// Like [`Self::user_with_images`] but for document sources (P4a).
    #[must_use]
    pub fn user_with_documents(id: MessageId, text: String, documents: Vec<DocumentSource>) -> Self {
        let mut content = Vec::new();
        if !text.is_empty() {
            content.push(ContentBlock::Text { text });
        }
        for source in documents {
            content.push(ContentBlock::Document { source });
        }
        Self::User { id, content }
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

/// One in-memory unit surfaced from the loader to the agent loop.
///
/// Boundary type shared by `lingxi-memory` (producer) and `lingxi-agent`
/// (consumer). Tier ordering and scoring rules live in `lingxi-memory`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MemoryEntry {
    /// Absolute path the entry was loaded from.
    pub path: std::path::PathBuf,
    /// Tier the entry belongs to (`Session`, `Project`, `Team`, `User`).
    pub tier: MemoryEntryTier,
    /// Raw markdown body (frontmatter stripped, secrets redacted).
    pub body: String,
    /// Age in whole days from the load `now`. `0` for a just-written file.
    pub age_days: u64,
    /// File size in bytes (post-redaction body length).
    pub size_bytes: u64,
}

/// Cross-crate stand-in for `memory::MemoryTier`.
///
/// Defined here to keep `lingxi-protocol` free of platform deps; the richer
/// variants (`Project { repo_root }`, etc.) live in `lingxi-memory::tier`.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub enum MemoryEntryTier {
    /// Session-scoped entry (highest tier weight).
    Session,
    /// Project-scoped entry (next tier).
    Project,
    /// Team-scoped entry (subject to team-boost gate).
    Team,
    /// User-scoped entry (lowest tier).
    User,
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
    fn memory_entry_roundtrip_json() {
        let e = MemoryEntry {
            path: std::path::PathBuf::from("/tmp/CLAUDE.md"),
            tier: MemoryEntryTier::Project,
            body: "hello".into(),
            age_days: 3,
            size_bytes: 5,
        };
        let s = serde_json::to_string(&e).unwrap();
        let e2: MemoryEntry = serde_json::from_str(&s).unwrap();
        assert_eq!(e, e2);
    }

    #[test]
    fn user_with_images_appends_image_blocks_after_text() {
        let img = ImageSource::Base64 {
            media_type: "image/png".to_string(),
            data: "AAAA".to_string(),
        };
        let m = ConversationMessage::user_with_images(
            MessageId::new(),
            "look:".to_string(),
            vec![img.clone()],
        );
        match &m {
            ConversationMessage::User { content, .. } => {
                assert_eq!(content.len(), 2);
                assert!(matches!(&content[0], ContentBlock::Text { text } if text == "look:"));
                assert!(matches!(&content[1], ContentBlock::Image { source } if *source == img));
            }
            _ => panic!("expected user message"),
        }
        // Empty text → only the image block (no empty text block).
        let m2 = ConversationMessage::user_with_images(MessageId::new(), String::new(), vec![img]);
        match m2 {
            ConversationMessage::User { content, .. } => {
                assert_eq!(content.len(), 1);
                assert!(matches!(content[0], ContentBlock::Image { .. }));
            }
            _ => panic!("expected user message"),
        }
    }

    #[test]
    fn image_base64_block_matches_anthropic_wire() {
        let block = ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: "aGVsbG8=".to_string(),
            },
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": "image/png", "data": "aGVsbG8="}
            })
        );
        let back: ContentBlock = serde_json::from_value(v).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn document_base64_block_matches_anthropic_wire() {
        let block = ContentBlock::Document {
            source: DocumentSource::Base64 {
                media_type: "application/pdf".to_string(),
                data: "JVBERi0=".to_string(),
            },
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(v, serde_json::json!({
            "type": "document",
            "source": { "type": "base64", "media_type": "application/pdf", "data": "JVBERi0=" }
        }));
        let back: ContentBlock = serde_json::from_value(v).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn image_url_block_matches_anthropic_wire() {
        let block = ContentBlock::Image {
            source: ImageSource::Url { url: "https://x/y.png".to_string() },
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type":"image","source":{"type":"url","url":"https://x/y.png"}})
        );
    }

    #[test]
    fn conversation_message_with_image_roundtrips_jsonl() {
        let m = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Base64 {
                    media_type: "image/jpeg".to_string(),
                    data: "Zm9v".to_string(),
                },
            }],
        };
        let line = serde_json::to_string(&m).unwrap();
        let back: ConversationMessage = serde_json::from_str(&line).unwrap();
        assert_eq!(back, m);
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
