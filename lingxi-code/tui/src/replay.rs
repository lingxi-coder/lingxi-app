#![forbid(unsafe_code)]
//! Resume transcript replay: persisted conversation → rendered scrollback.
//!
//! When a session is RESUMED (`--resume` / `--continue` / the picker), the
//! prior conversation must be visible in the TUI before the user continues —
//! claude-code seeds the REPL's message list from the loaded transcript
//! (`main.tsx` → `loadConversationForResume` → `initialMessages` →
//! `useState(initialMessages ?? [])`), so the user sees the existing history
//! on the very first frame. A FRESH session passes no initial messages and
//! starts empty.
//!
//! This module is the Rust analog of that seed step. It maps the persisted
//! [`protocol::ConversationMessage`] history (loaded by
//! `session::SessionStorage::load` / `session::jsonl::load_session`) into the
//! TUI's [`RenderedMessage`] scrollback, reusing the SAME per-kind mapping the
//! live event path uses ([`crate::streaming::apply_event`]):
//!
//! - `User` text block      → [`RenderedMessage::UserText`]
//! - `Assistant` text block → [`RenderedMessage::AssistantText`]
//! - `Assistant` tool-use   → [`RenderedMessage::AssistantToolUse`] (input stashed
//!   in a side-table keyed by `ToolUseId`)
//! - `User` tool-result     → [`RenderedMessage::UserToolResult`], grouped under
//!   its originating tool-use via that side-table (the claude-code
//!   `sourceToolUseID` correlation) and decorated with the diff inputs derived
//!   by [`crate::streaming::diff_inputs_for`] — exactly as the live
//!   `ToolUseResult` event does.
//! - `Assistant`/`User` thinking → [`RenderedMessage::AssistantThinking`]
//!   (collapsed) / [`RenderedMessage::AssistantRedactedThinking`] (no body).
//!
//! `System` messages and `Image` blocks are NOT replayed: `System` lines are
//! reconstructed at runtime from settings + memory on resume (matching
//! `orchestrator::resume::build_state_from_jsonl`, which skips `type:"system"`),
//! and the persisted image block carries no scrollback id/metadata to render a
//! faithful `[Image #N]` placeholder. Both are intentional, documented gaps —
//! the common turn kinds (user/assistant text, tool-use, tool-result) replay in
//! order.

use crate::state::RenderedMessage;
use protocol::{ContentBlock, ConversationMessage, ToolUseId};
use std::collections::HashMap;

/// Build the seeded scrollback for a resumed session from its persisted
/// conversation history (in file/arrival order).
///
/// Pure + terminal-free: no I/O, no clock reads on the hot blocks (text rows
/// carry a `0` resume timestamp — see below). Tool results are correlated to
/// their tool-use by id through a local side-table, mirroring the live
/// `tool_call_inputs` stash in [`crate::state::AppState`].
///
/// Timestamps: replayed text rows use `0` (epoch) rather than `now()` so the
/// seed is deterministic and a replay is reproducible; the prior turn's real
/// wall-clock isn't carried on `ConversationMessage` (only the transcript
/// envelope holds it, and the renderer shows timestamps only on hover/expand).
#[must_use]
pub fn rebuild_messages(history: &[ConversationMessage]) -> Vec<RenderedMessage> {
    let mut acc = ReplayAcc::default();
    for msg in history {
        match msg {
            ConversationMessage::User { content, .. } => acc.push_user_blocks(content),
            ConversationMessage::Assistant { content, .. } => acc.push_assistant_blocks(content),
            // System lines are reconstructed at runtime (settings + memory),
            // not replayed — see module docs + build_state_from_jsonl.
            ConversationMessage::System { .. } => {}
        }
    }
    acc.out
}

/// Build the seeded scrollback directly from the LOADER's raw JSONL messages
/// (`session::jsonl::load_session` → `Vec<JsonlMessage>`), the shape the CLI's
/// `--resume <uuid>` path already has in hand.
///
/// Each `JsonlMessage` carries `message_type` (`"user"`/`"assistant"`/…) and a
/// raw `message` JSON whose `content` is either a string or an array of content
/// blocks. We decode `content` into `Vec<ContentBlock>` (accepting both shapes,
/// exactly as `orchestrator::resume::extract_content_blocks` does) and route by
/// type through the SAME per-block mapping as [`rebuild_messages`]. `system` and
/// any non-user/assistant entry types are skipped (reconstructed at runtime).
#[must_use]
pub fn rebuild_from_jsonl(messages: &[session::jsonl::JsonlMessage]) -> Vec<RenderedMessage> {
    let mut acc = ReplayAcc::default();
    for m in messages {
        let blocks = decode_content_blocks(&m.message);
        match m.message_type.as_str() {
            "user" => acc.push_user_blocks(&blocks),
            "assistant" => acc.push_assistant_blocks(&blocks),
            // system / attachment / summary / sidechain — not replayed.
            _ => {}
        }
    }
    acc.out
}

/// Best-effort `message.content` → `Vec<ContentBlock>` decode. Mirrors
/// `orchestrator::resume::extract_content_blocks`: a string becomes a single
/// `Text` block; an array is deserialized as content blocks; anything else (or
/// a decode failure) yields an empty `Vec` (the turn still anchors order but
/// contributes no rows).
fn decode_content_blocks(message: &serde_json::Value) -> Vec<ContentBlock> {
    let Some(content) = message.get("content") else {
        return Vec::new();
    };
    if let Some(s) = content.as_str() {
        return vec![ContentBlock::Text {
            text: s.to_string(),
        }];
    }
    if content.is_array() {
        if let Ok(blocks) = serde_json::from_value::<Vec<ContentBlock>>(content.clone()) {
            return blocks;
        }
    }
    Vec::new()
}

/// Replay accumulator: the rendered rows plus the tool-call side-table that
/// correlates a later `ToolResult` to its originating `ToolUse` (the resume-time
/// analog of `AppState.tool_call_inputs` / claude-code's `sourceToolUseID`).
#[derive(Default)]
struct ReplayAcc {
    out: Vec<RenderedMessage>,
    tool_inputs: HashMap<ToolUseId, serde_json::Value>,
    tool_names: HashMap<ToolUseId, String>,
}

impl ReplayAcc {
    fn push_user_blocks(&mut self, content: &[ContentBlock]) {
        for block in content {
            push_user_block(block, &self.tool_inputs, &self.tool_names, &mut self.out);
        }
    }

    fn push_assistant_blocks(&mut self, content: &[ContentBlock]) {
        for block in content {
            push_assistant_block(
                block,
                &mut self.tool_inputs,
                &mut self.tool_names,
                &mut self.out,
            );
        }
    }
}

/// Map one block of a persisted USER message into scrollback.
fn push_user_block(
    block: &ContentBlock,
    tool_inputs: &HashMap<ToolUseId, serde_json::Value>,
    tool_names: &HashMap<ToolUseId, String>,
    out: &mut Vec<RenderedMessage>,
) {
    match block {
        ContentBlock::Text { text } => {
            out.push(RenderedMessage::UserText {
                body: text.clone(),
                timestamp: 0,
            });
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => {
            // Recover the tool name + the originating call input (for the diff
            // fields) from the side-table populated by the earlier ToolUse. If
            // the pairing call is missing (torn transcript), fall back to an
            // empty tool name and no diff inputs — the result still renders.
            let tool = tool_names.get(tool_use_id).cloned().unwrap_or_default();
            let (old_string, new_string, file_path) = tool_inputs
                .get(tool_use_id)
                .map_or((None, None, None), |input| {
                    crate::streaming::diff_inputs_for(&tool, input)
                });
            // The live path stores the raw JSON result payload; the persisted
            // transcript stores the stringified `content`. Re-wrap it as a JSON
            // string so the UserToolResult renderer (which expects a
            // `serde_json::Value`) treats it identically to a live result.
            let result = serde_json::Value::String(content.clone());
            let _ = is_error; // is_error is not carried on RenderedMessage::UserToolResult
            out.push(RenderedMessage::UserToolResult {
                id: tool_use_id.clone(),
                tool,
                result,
                old_string,
                new_string,
                file_path,
            });
        }
        // A user message may also carry an Image or Document block (paste/PDF
        // path); the persisted block has no scrollback id/metadata, so it is not
        // replayed.
        ContentBlock::Image { .. }
        | ContentBlock::Document { .. }
        | ContentBlock::ToolUse { .. }
        | ContentBlock::Thinking { .. }
        // Low-frequency server-side blocks are preserved in the JSONL for
        // resume/replay byte parity but have no scrollback renderer.
        | ContentBlock::RedactedThinking { .. }
        | ContentBlock::ServerToolUse { .. }
        | ContentBlock::ConnectorText { .. }
        | ContentBlock::AdvisorToolResult { .. } => {}
    }
}

/// Map one block of a persisted ASSISTANT message into scrollback, updating the
/// tool side-table for the corresponding result.
fn push_assistant_block(
    block: &ContentBlock,
    tool_inputs: &mut HashMap<ToolUseId, serde_json::Value>,
    tool_names: &mut HashMap<ToolUseId, String>,
    out: &mut Vec<RenderedMessage>,
) {
    match block {
        ContentBlock::Text { text } => {
            out.push(RenderedMessage::AssistantText {
                body: text.clone(),
                timestamp: 0,
            });
        }
        ContentBlock::ToolUse { id, name, input, .. } => {
            tool_inputs.insert(id.clone(), input.clone());
            tool_names.insert(id.clone(), name.clone());
            out.push(RenderedMessage::AssistantToolUse {
                id: id.clone(),
                tool: name.clone(),
                input: input.clone(),
            });
        }
        ContentBlock::Thinking { thinking, .. } => {
            // Collapsed by default — same default the live AssistantThinking
            // path uses (expanded state lives in AppState.expanded).
            out.push(RenderedMessage::AssistantThinking {
                thinking: thinking.clone(),
                expanded: false,
            });
        }
        // A tool result block should never appear on an assistant message;
        // images and documents aren't replayed (see module docs).
        ContentBlock::ToolResult { .. }
        | ContentBlock::Image { .. }
        | ContentBlock::Document { .. }
        // Low-frequency server-side blocks are preserved in the JSONL for
        // resume/replay byte parity but have no scrollback renderer.
        | ContentBlock::RedactedThinking { .. }
        | ContentBlock::ServerToolUse { .. }
        | ContentBlock::ConnectorText { .. }
        | ContentBlock::AdvisorToolResult { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ImageSource, MessageId};

    fn user_text(text: &str) -> ConversationMessage {
        ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Text { text: text.into() }],
            is_meta: false,
        }
    }

    fn assistant_text(text: &str) -> ConversationMessage {
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::Text { text: text.into() }],
            stop_reason: None,
        }
    }

    #[test]
    fn empty_history_seeds_no_messages() {
        assert!(rebuild_messages(&[]).is_empty());
    }

    #[test]
    fn user_then_assistant_text_in_order() {
        let h = vec![user_text("hello"), assistant_text("hi there")];
        let out = rebuild_messages(&h);
        assert_eq!(out.len(), 2);
        assert!(matches!(
            &out[0],
            RenderedMessage::UserText { body, timestamp } if body == "hello" && *timestamp == 0
        ));
        assert!(matches!(
            &out[1],
            RenderedMessage::AssistantText { body, .. } if body == "hi there"
        ));
    }

    #[test]
    fn tool_use_then_result_groups_by_id_and_carries_diff_inputs() {
        let id = ToolUseId::new();
        let h = vec![
            ConversationMessage::Assistant {
                id: MessageId::new(),
                content: vec![ContentBlock::ToolUse {
                    id: id.clone(),
                    name: "Edit".into(),
                    input: serde_json::json!({
                        "file_path": "/tmp/x.rs",
                        "old_string": "a",
                        "new_string": "b",
                    }),
                    provider_id: None,
                }],
                stop_reason: None,
            },
            ConversationMessage::User {
                id: MessageId::new(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: "edited".into(),
                    is_error: false,
                    provider_tool_use_id: None,
                    content_blocks: None,
                }],
                is_meta: false,
            },
        ];
        let out = rebuild_messages(&h);
        assert_eq!(out.len(), 2);
        match &out[0] {
            RenderedMessage::AssistantToolUse { id: gid, tool, .. } => {
                assert_eq!(*gid, id);
                assert_eq!(tool, "Edit");
            }
            other => panic!("expected AssistantToolUse, got {other:?}"),
        }
        match &out[1] {
            RenderedMessage::UserToolResult {
                id: gid,
                tool,
                result,
                old_string,
                new_string,
                file_path,
            } => {
                // grouped under its originating tool-use (sourceToolUseID).
                assert_eq!(*gid, id);
                assert_eq!(tool, "Edit");
                assert_eq!(result, &serde_json::Value::String("edited".into()));
                // diff inputs derived from the call via diff_inputs_for.
                assert_eq!(old_string.as_deref(), Some("a"));
                assert_eq!(new_string.as_deref(), Some("b"));
                assert_eq!(file_path.as_deref(), Some("/tmp/x.rs"));
            }
            other => panic!("expected UserToolResult, got {other:?}"),
        }
    }

    #[test]
    fn orphan_tool_result_renders_without_pairing_call() {
        // A torn transcript: a result with no preceding tool-use. It must still
        // render (empty tool name, no diff inputs) rather than be dropped.
        let id = ToolUseId::new();
        let h = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id,
                content: "stdout".into(),
                is_error: false,
                provider_tool_use_id: None,
                content_blocks: None,
            }],
            is_meta: false,
        }];
        let out = rebuild_messages(&h);
        assert_eq!(out.len(), 1);
        match &out[0] {
            RenderedMessage::UserToolResult {
                tool,
                old_string,
                new_string,
                file_path,
                ..
            } => {
                assert_eq!(tool, "");
                assert!(old_string.is_none() && new_string.is_none() && file_path.is_none());
            }
            other => panic!("expected UserToolResult, got {other:?}"),
        }
    }

    #[test]
    fn thinking_block_replays_collapsed() {
        let h = vec![ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::Thinking {
                thinking: "reasoning".into(),
                signature: None,
            }],
            stop_reason: None,
        }];
        let out = rebuild_messages(&h);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            &out[0],
            RenderedMessage::AssistantThinking { thinking, expanded }
                if thinking == "reasoning" && !*expanded
        ));
    }

    #[test]
    fn system_and_image_blocks_are_not_replayed() {
        let h = vec![
            ConversationMessage::System {
                id: MessageId::new(),
                content: "you are a helpful assistant".into(),
            },
            ConversationMessage::User {
                id: MessageId::new(),
                content: vec![ContentBlock::Image {
                    source: ImageSource::Url {
                        url: "https://example/x.png".into(),
                    },
                }],
                is_meta: false,
            },
        ];
        assert!(rebuild_messages(&h).is_empty());
    }

    /// A `JsonlMessage` built from a wire-shape JSON line — the loader output
    /// the CLI `--resume <uuid>` path holds. Exercises [`rebuild_from_jsonl`].
    fn jsonl(message_type: &str, content: &serde_json::Value) -> session::jsonl::JsonlMessage {
        serde_json::from_value(serde_json::json!({
            "type": message_type,
            "uuid": uuid::Uuid::new_v4().to_string(),
            "parentUuid": null,
            "sessionId": uuid::Uuid::new_v4().to_string(),
            "timestamp": "2026-05-25T14:30:00.000Z",
            "cwd": "/tmp",
            "version": "0.0.0",
            "message": { "content": content },
        }))
        .expect("valid JsonlMessage")
    }

    #[test]
    fn jsonl_string_content_becomes_one_text_row() {
        // claude-code persists simple text turns as a bare string `content`.
        let msgs = vec![jsonl("user", &serde_json::json!("hi from jsonl"))];
        let out = rebuild_from_jsonl(&msgs);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            &out[0],
            RenderedMessage::UserText { body, .. } if body == "hi from jsonl"
        ));
    }

    #[test]
    fn jsonl_tool_use_then_result_groups_by_id() {
        let id = ToolUseId::new();
        // The persisted transcript serializes a `ToolUseId` via its derived
        // `Serialize` (a bare UUID), NOT its `tu:`-prefixed `Display`. Build the
        // fixture id the same way the engine writes it so the decode round-trips.
        let id_wire = serde_json::to_value(id.clone()).unwrap();
        let msgs = vec![
            jsonl(
                "assistant",
                &serde_json::json!([
                    { "type": "text", "text": "reading" },
                    { "type": "tool_use", "id": id_wire.clone(), "name": "Read",
                      "input": { "file_path": "/a" } },
                ]),
            ),
            jsonl(
                "user",
                &serde_json::json!([
                    { "type": "tool_result", "tool_use_id": id_wire,
                      "content": "file body", "is_error": false },
                ]),
            ),
        ];
        let out = rebuild_from_jsonl(&msgs);
        assert_eq!(out.len(), 3);
        assert!(
            matches!(&out[0], RenderedMessage::AssistantText { body, .. } if body == "reading")
        );
        assert!(
            matches!(&out[1], RenderedMessage::AssistantToolUse { id: gid, tool, .. } if *gid == id && tool == "Read")
        );
        match &out[2] {
            RenderedMessage::UserToolResult {
                id: gid,
                tool,
                result,
                ..
            } => {
                assert_eq!(*gid, id);
                assert_eq!(tool, "Read");
                assert_eq!(result, &serde_json::Value::String("file body".into()));
            }
            other => panic!("expected UserToolResult, got {other:?}"),
        }
    }

    #[test]
    fn jsonl_system_entries_are_skipped() {
        let msgs = vec![
            jsonl("system", &serde_json::json!("system prompt")),
            jsonl("user", &serde_json::json!("real turn")),
        ];
        let out = rebuild_from_jsonl(&msgs);
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], RenderedMessage::UserText { body, .. } if body == "real turn"));
    }

    #[test]
    fn multi_block_assistant_message_preserves_block_order() {
        // assistant text + tool-use in ONE message → two rows, in order.
        let id = ToolUseId::new();
        let h = vec![ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![
                ContentBlock::Text {
                    text: "let me read it".into(),
                },
                ContentBlock::ToolUse {
                    id,
                    name: "Read".into(),
                    input: serde_json::json!({"file_path": "/a"}),
                    provider_id: None,
                },
            ],
            stop_reason: None,
        }];
        let out = rebuild_messages(&h);
        assert_eq!(out.len(), 2);
        assert!(
            matches!(&out[0], RenderedMessage::AssistantText { body, .. } if body == "let me read it")
        );
        assert!(
            matches!(&out[1], RenderedMessage::AssistantToolUse { tool, .. } if tool == "Read")
        );
    }
}
