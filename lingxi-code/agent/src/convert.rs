//! Conversions between [`protocol`] types and [`llm_client`] types.
//!
//! The two crates use parallel but structurally equivalent type hierarchies.
//! This module is the single place that translates between them so the rest of
//! the agent crate can stay unaware of `llm_client` internals.
//!
//! # Mapping table
//!
//! | Protocol | `llm_client` |
//! |---|---|
//! | `ConversationMessage::User` | `Message { role: "user", .. }` |
//! | `ConversationMessage::Assistant` | `Message { role: "assistant", .. }` |
//! | `ConversationMessage::System` | rejected (`LlmError::InvalidRequest`) |
//! | `ContentBlock::Text` | `ContentBlock::Text { cache_control: None }` |
//! | `ContentBlock::ToolUse { id, name, input }` | `ContentBlock::ToolCall { id: id.to_string(), name, input }` |
//! | `ContentBlock::ToolResult { tool_use_id, content, is_error }` | `ContentBlock::ToolResult { tool_call_id: …, output: Value::String(content), is_error, cache_control: None }` |
//! | `ContentBlock::Thinking { thinking, signature }` | `ContentBlock::Reasoning { text: thinking, signature }` |
//! | `ContentBlock::Image { source: ImageSource::Base64 { media_type, data } }` | `ContentBlock::Image { media_type, bytes: base64_decode(data) }` |
//! | `ContentBlock::Image { source: ImageSource::Url { url } }` | `ContentBlock::ImageUrl { url }` |
//! | `ContentBlock::Document { source: DocumentSource::Base64 { media_type, data } }` | `ContentBlock::Document { media_type, bytes: base64_decode(data) }` |
//!
//! Output-only `llm_client` variants not reachable from protocol inputs:
//! `RedactedThinking`, `ServerToolUse`, `ConnectorText`, `AdvisorToolResult`, `ImageUrl`-decode.

use base64::Engine as _;
use llm_client::{ContentBlock as LlmBlock, LlmError, Message, ToolDeclaration};
use protocol::{ContentBlock as ProtoBlock, ConversationMessage, ImageSource, DocumentSource};
use serde_json::Value;

/// Convert a `Vec<ConversationMessage>` into `Vec<llm_client::Message>`.
///
/// Returns `Err(LlmError::InvalidRequest)` if any message is a `System`
/// variant — system prompts travel separately and must not appear in the
/// message vec.
///
/// Returns `Err(LlmError::InvalidRequest)` if any content block cannot be
/// converted (e.g. bad base64).
pub fn to_llm_messages(
    messages: Vec<ConversationMessage>,
) -> Result<Vec<Message>, LlmError> {
    messages.into_iter().map(convert_message).collect()
}

/// Merge consecutive `User` messages into a single user turn (claude-code
/// `normalizeMessagesForAPI` consecutive-user merge + `mergeUserMessages`).
///
/// `Assistant`/`System` messages pass through unchanged and act as separators.
/// The merged message keeps the FIRST message's id; content blocks are
/// concatenated in order (a.content ++ b.content). Single or non-adjacent user
/// messages are unaffected (identity), so this is a no-op for today's history
/// shape and load-bearing only after per-result `tool_result` splitting (11b).
///
/// claude-code rationale: "Bedrock doesn't support multiple user messages in a
/// row; 1P API merges them into a single user turn."
#[must_use]
pub fn normalize_messages_for_api(
    messages: Vec<ConversationMessage>,
) -> Vec<ConversationMessage> {
    let mut out: Vec<ConversationMessage> = Vec::with_capacity(messages.len());
    for msg in messages {
        match (out.last_mut(), &msg) {
            (
                Some(ConversationMessage::User {
                    content: prev_content,
                    ..
                }),
                ConversationMessage::User {
                    content: new_content,
                    ..
                },
            ) => {
                prev_content.extend(new_content.iter().cloned());
            }
            _ => out.push(msg),
        }
    }
    out
}

/// Convert a `Vec<serde_json::Value>` (tool declarations in wire JSON shape)
/// into `Vec<llm_client::ToolDeclaration>`.
///
/// Each value must have string fields `name` and `description` and a
/// non-null `input_schema`; missing or wrong-typed fields produce
/// `Err(LlmError::InvalidRequest)` naming the offending field.
pub fn to_tool_declarations(
    tools: Vec<Value>,
) -> Result<Vec<ToolDeclaration>, LlmError> {
    tools.into_iter().map(convert_tool_declaration).collect()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn convert_message(msg: ConversationMessage) -> Result<Message, LlmError> {
    match msg {
        ConversationMessage::User { content, .. } => Ok(Message {
            role: "user".to_string(),
            content: content
                .into_iter()
                .map(convert_block)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        ConversationMessage::Assistant { content, .. } => Ok(Message {
            role: "assistant".to_string(),
            content: content
                .into_iter()
                .map(convert_block)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        ConversationMessage::System { .. } => Err(LlmError::InvalidRequest {
            message: "System messages must not appear in the messages vec; pass them via the system parameter".to_string(),
        }),
    }
}

fn convert_block(block: ProtoBlock) -> Result<LlmBlock, LlmError> {
    match block {
        ProtoBlock::Text { text } => Ok(LlmBlock::Text {
            text,
            cache_control: None,
        }),
        ProtoBlock::ToolUse {
            id,
            name,
            input,
            provider_id,
        } => Ok(LlmBlock::ToolCall {
            // Replay the verbatim provider-issued id when preserved (Anthropic
            // `toolu_…`, OpenAI `call_…`); fall back to the `tu:<uuid>` Display
            // form only for internally-minted blocks with no provider round-trip.
            id: provider_id.unwrap_or_else(|| id.to_string()),
            name,
            input,
        }),
        ProtoBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            provider_tool_use_id,
        } => Ok(LlmBlock::ToolResult {
            // Must echo the same id the paired `tool_use` carried so Anthropic
            // pairs them; prefer the preserved provider id over the synthetic one.
            tool_call_id: provider_tool_use_id.unwrap_or_else(|| tool_use_id.to_string()),
            output: Value::String(content),
            is_error,
            cache_control: None,
        }),
        ProtoBlock::Thinking { thinking, signature } => Ok(LlmBlock::Reasoning {
            text: thinking,
            signature,
        }),
        ProtoBlock::Image { source } => convert_image_source(source),
        ProtoBlock::Document { source } => convert_document_source(source),
    }
}

fn convert_image_source(source: ImageSource) -> Result<LlmBlock, LlmError> {
    match source {
        ImageSource::Base64 { media_type, data } => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(|e| LlmError::InvalidRequest {
                    message: format!("Image base64 decode failed: {e}"),
                })?;
            Ok(LlmBlock::Image { media_type, bytes })
        }
        ImageSource::Url { url } => Ok(LlmBlock::ImageUrl { url }),
    }
}

fn convert_document_source(source: DocumentSource) -> Result<LlmBlock, LlmError> {
    match source {
        DocumentSource::Base64 { media_type, data } => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(|e| LlmError::InvalidRequest {
                    message: format!("Document base64 decode failed: {e}"),
                })?;
            Ok(LlmBlock::Document { media_type, bytes })
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
fn convert_tool_declaration(value: Value) -> Result<ToolDeclaration, LlmError> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| LlmError::InvalidRequest {
            message: "Tool declaration missing required string field: name".to_string(),
        })?
        .to_string();

    let description = value
        .get("description")
        .and_then(Value::as_str)
        .ok_or_else(|| LlmError::InvalidRequest {
            message: "Tool declaration missing required string field: description".to_string(),
        })?
        .to_string();

    let input_schema = value
        .get("input_schema")
        .cloned()
        .filter(|v| !v.is_null())
        .ok_or_else(|| LlmError::InvalidRequest {
            message: "Tool declaration missing required field: input_schema".to_string(),
        })?;

    Ok(ToolDeclaration {
        name,
        description,
        input_schema,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{MessageId, ToolUseId};

    // ── to_llm_messages ───────────────────────────────────────────────────────

    #[test]
    fn text_block_maps_to_llm_text_with_no_cache_control() {
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ProtoBlock::Text { text: "hello".to_string() }],
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::Text { text, cache_control: None } if text == "hello"
        ));
    }

    #[test]
    fn tool_use_block_maps_to_tool_call() {
        let id = ToolUseId::new();
        let id_str = id.to_string();
        let msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ProtoBlock::ToolUse {
                id,
                name: "Read".to_string(),
                input: serde_json::json!({"path": "/tmp/x"}),
                provider_id: None,
            }],
            stop_reason: None,
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert_eq!(result[0].role, "assistant");
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::ToolCall { id, name, input }
                if id == &id_str && name == "Read" && input["path"] == "/tmp/x"
        ));
    }

    #[test]
    fn tool_use_provider_id_replayed_verbatim_on_egress() {
        // P0: a preserved provider id (Anthropic `toolu_…`) MUST be replayed
        // verbatim as the egress `tool_call` id — NOT the synthetic `tu:<uuid>`.
        let msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ProtoBlock::ToolUse {
                id: ToolUseId::new(),
                name: "Read".to_string(),
                input: serde_json::json!({"path": "/tmp/x"}),
                provider_id: Some("toolu_01ABCDEF".to_string()),
            }],
            stop_reason: None,
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::ToolCall { id, .. } if id == "toolu_01ABCDEF"
        ));
    }

    #[test]
    fn tool_result_provider_id_replayed_verbatim_on_egress() {
        // P0: the paired `tool_result` must echo the SAME verbatim provider id.
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ProtoBlock::ToolResult {
                tool_use_id: ToolUseId::new(),
                content: "file content".to_string(),
                is_error: false,
                provider_tool_use_id: Some("toolu_01ABCDEF".to_string()),
            }],
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::ToolResult { tool_call_id, .. } if tool_call_id == "toolu_01ABCDEF"
        ));
    }

    #[test]
    fn tool_result_block_wraps_content_as_string_value() {
        let tool_use_id = ToolUseId::new();
        let tool_call_id_str = tool_use_id.to_string();
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ProtoBlock::ToolResult {
                tool_use_id,
                content: "file content".to_string(),
                is_error: false,
                provider_tool_use_id: None,
            }],
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::ToolResult { tool_call_id, output, is_error: false, cache_control: None }
                if tool_call_id == &tool_call_id_str && output == &Value::String("file content".to_string())
        ));
    }

    #[test]
    fn tool_result_error_flag_preserved() {
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ProtoBlock::ToolResult {
                tool_use_id: ToolUseId::new(),
                content: "boom".to_string(),
                is_error: true,
                provider_tool_use_id: None,
            }],
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::ToolResult { is_error: true, .. }
        ));
    }

    #[test]
    fn thinking_block_maps_to_reasoning() {
        let msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ProtoBlock::Thinking {
                thinking: "let me think".to_string(),
                signature: Some("sig_abc".to_string()),
            }],
            stop_reason: None,
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::Reasoning { text, signature }
                if text == "let me think" && signature.as_deref() == Some("sig_abc")
        ));
    }

    #[test]
    fn image_base64_source_decoded_to_bytes() {
        // "hello" base64 encodes to "aGVsbG8="
        let raw = b"hello";
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ProtoBlock::Image {
                source: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: encoded,
                },
            }],
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::Image { media_type, bytes }
                if media_type == "image/png" && bytes.as_slice() == raw
        ));
    }

    #[test]
    fn image_url_source_maps_to_image_url() {
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ProtoBlock::Image {
                source: ImageSource::Url { url: "https://example.com/img.png".to_string() },
            }],
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::ImageUrl { url } if url == "https://example.com/img.png"
        ));
    }

    #[test]
    fn multi_block_assistant_message_all_convert() {
        let id = ToolUseId::new();
        let id_str = id.to_string();
        let msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![
                ProtoBlock::Text { text: "sure".to_string() },
                ProtoBlock::ToolUse {
                    id,
                    name: "Read".to_string(),
                    input: serde_json::json!({"path": "/x"}),
                    provider_id: None,
                },
            ],
            stop_reason: Some("tool_use".to_string()),
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "assistant");
        assert_eq!(result[0].content.len(), 2);
        assert!(matches!(&result[0].content[0], LlmBlock::Text { text, .. } if text == "sure"));
        assert!(matches!(
            &result[0].content[1],
            LlmBlock::ToolCall { id, name, .. } if id == &id_str && name == "Read"
        ));
    }

    #[test]
    fn empty_messages_vec_returns_empty() {
        let result = to_llm_messages(vec![]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn document_base64_source_decoded_to_bytes() {
        let raw = b"%PDF-1.4";
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ProtoBlock::Document {
                source: DocumentSource::Base64 {
                    media_type: "application/pdf".to_string(),
                    data: encoded,
                },
            }],
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::Document { media_type, bytes }
                if media_type == "application/pdf" && bytes.as_slice() == raw
        ));
    }

    #[test]
    fn system_message_rejected_with_invalid_request() {
        let msg = ConversationMessage::System {
            id: MessageId::new(),
            content: "you are a helpful assistant".to_string(),
        };
        let err = to_llm_messages(vec![msg]).unwrap_err();
        assert!(matches!(err, LlmError::InvalidRequest { .. }));
    }

    #[test]
    fn user_and_assistant_roles_mapped_correctly() {
        let user = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ProtoBlock::Text { text: "hi".to_string() }],
        };
        let assistant = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ProtoBlock::Text { text: "hello".to_string() }],
            stop_reason: Some("end_turn".to_string()),
        };
        let result = to_llm_messages(vec![user, assistant]).unwrap();
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
    }

    // ── to_tool_declarations ─────────────────────────────────────────────────

    #[test]
    fn valid_tool_declaration_converts_successfully() {
        let tools = vec![serde_json::json!({
            "name": "Read",
            "description": "Read a file",
            "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
        })];
        let result = to_tool_declarations(tools).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Read");
        assert_eq!(result[0].description, "Read a file");
        assert_eq!(result[0].input_schema["type"], "object");
    }

    #[test]
    fn tool_declaration_missing_name_returns_error() {
        let tools = vec![serde_json::json!({
            "description": "Read a file",
            "input_schema": {"type": "object"}
        })];
        let err = to_tool_declarations(tools).unwrap_err();
        assert!(matches!(err, LlmError::InvalidRequest { message } if message.contains("name")));
    }

    #[test]
    fn tool_declaration_missing_description_returns_error() {
        let tools = vec![serde_json::json!({
            "name": "Read",
            "input_schema": {"type": "object"}
        })];
        let err = to_tool_declarations(tools).unwrap_err();
        assert!(matches!(err, LlmError::InvalidRequest { message } if message.contains("description")));
    }

    #[test]
    fn tool_declaration_missing_input_schema_returns_error() {
        let tools = vec![serde_json::json!({
            "name": "Read",
            "description": "Read a file"
        })];
        let err = to_tool_declarations(tools).unwrap_err();
        assert!(matches!(err, LlmError::InvalidRequest { message } if message.contains("input_schema")));
    }

    #[test]
    fn tool_declaration_null_input_schema_returns_error() {
        let tools = vec![serde_json::json!({
            "name": "Read",
            "description": "Read a file",
            "input_schema": null
        })];
        let err = to_tool_declarations(tools).unwrap_err();
        assert!(matches!(err, LlmError::InvalidRequest { message } if message.contains("input_schema")));
    }

    #[test]
    fn image_bad_base64_returns_invalid_request() {
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ProtoBlock::Image {
                source: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "not-valid-base64!!!".to_string(),
                },
            }],
        };
        let err = to_llm_messages(vec![msg]).unwrap_err();
        assert!(matches!(err, LlmError::InvalidRequest { message } if message.contains("base64")));
    }

    // ── normalize_messages_for_api ───────────────────────────────────────────

    fn user(id: MessageId, text: &str) -> ConversationMessage {
        ConversationMessage::User {
            id,
            content: vec![ProtoBlock::Text { text: text.to_string() }],
        }
    }

    fn assistant(text: &str) -> ConversationMessage {
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ProtoBlock::Text { text: text.to_string() }],
            stop_reason: None,
        }
    }

    fn text_of(blocks: &[ProtoBlock]) -> Vec<&str> {
        blocks
            .iter()
            .map(|b| match b {
                ProtoBlock::Text { text } => text.as_str(),
                _ => panic!("expected text block"),
            })
            .collect()
    }

    #[test]
    fn two_consecutive_users_merge_into_one_keeping_first_id_and_order() {
        let first_id = MessageId::new();
        let out = normalize_messages_for_api(vec![
            user(first_id, "a"),
            user(MessageId::new(), "b"),
        ]);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ConversationMessage::User { id, content } => {
                assert_eq!(id, &first_id, "merged message keeps the first message's id");
                assert_eq!(text_of(content), vec!["a", "b"]);
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn assistant_separates_users_no_merge() {
        let out = normalize_messages_for_api(vec![
            user(MessageId::new(), "a"),
            assistant("mid"),
            user(MessageId::new(), "b"),
        ]);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], ConversationMessage::User { .. }));
        assert!(matches!(out[1], ConversationMessage::Assistant { .. }));
        assert!(matches!(out[2], ConversationMessage::User { .. }));
    }

    #[test]
    fn single_user_is_unchanged() {
        let id = MessageId::new();
        let out = normalize_messages_for_api(vec![user(id, "solo")]);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ConversationMessage::User { id: got, content } => {
                assert_eq!(got, &id);
                assert_eq!(text_of(content), vec!["solo"]);
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn three_consecutive_users_merge_into_one_in_order() {
        let first_id = MessageId::new();
        let out = normalize_messages_for_api(vec![
            user(first_id, "a"),
            user(MessageId::new(), "b"),
            user(MessageId::new(), "c"),
        ]);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ConversationMessage::User { id, content } => {
                assert_eq!(id, &first_id);
                assert_eq!(text_of(content), vec!["a", "b", "c"]);
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn assistant_user_user_assistant_merges_only_the_middle_pair() {
        let mid_id = MessageId::new();
        let out = normalize_messages_for_api(vec![
            assistant("start"),
            user(mid_id, "a"),
            user(MessageId::new(), "b"),
            assistant("end"),
        ]);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], ConversationMessage::Assistant { .. }));
        match &out[1] {
            ConversationMessage::User { id, content } => {
                assert_eq!(id, &mid_id);
                assert_eq!(text_of(content), vec!["a", "b"]);
            }
            other => panic!("expected merged User, got {other:?}"),
        }
        assert!(matches!(out[2], ConversationMessage::Assistant { .. }));
    }
}
