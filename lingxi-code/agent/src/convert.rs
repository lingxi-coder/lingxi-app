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
/// `normalizeMessagesForAPI` consecutive-user merge + `mergeUserMessages`,
/// `utils/messages.ts:2411`).
///
/// `Assistant`/`System` messages pass through unchanged and act as separators.
/// The merged message keeps the FIRST message's id. The two operands' content
/// blocks are merged via the faithful claude-code merge pipeline
/// `hoistToolResults(joinTextAtSeam(a, b))`:
///
/// * [`join_text_at_seam`] — when `a`'s last block and `b`'s first block are
///   both `Text`, append `'\n'` to `a`'s last text before concatenating, so two
///   queued text prompts `"2 + 2"` + `"3 + 3"` don't reach the model glued as
///   `"2 + 23 + 3"` (the API concatenates adjacent text blocks with no
///   separator). The `\n` goes on `a`'s side so no block's `startsWith`
///   classification changes (`joinTextAtSeam`, `messages.ts:2505`).
/// * [`hoist_tool_results`] — stable-partition `ToolResult` blocks to the front
///   so they lead the merged user turn, avoiding "tool result must follow tool
///   use" API errors (`hoistToolResults`, `messages.ts:2470`).
///
/// Single or non-adjacent user messages are unaffected (identity).
///
/// claude-code rationale: "Bedrock doesn't support multiple user messages in a
/// row; 1P API merges them into a single user turn."
///
/// # Remainder of `normalizeMessagesForAPI` that is N/A to LingXi
///
/// claude-code's full `normalizeMessagesForAPI` (`messages.ts:1989-2370`) runs
/// several other transforms ahead of the merge. They have no substrate in
/// LingXi's message model and are therefore deliberately NOT ported (porting a
/// stub would be an unfaithful divergence):
///
/// * `reorderAttachmentsForAPI` / `isVirtual` filtering — N/A: there is no
///   attachment-typed or virtual `ConversationMessage`; the protocol has only
///   `User`/`Assistant`/`System` (`protocol/src/messages.rs:168`). Attachments
///   are already plain `User` content blocks inserted in position by the caller.
/// * `progress` / `system(non-local-command)` / synthetic-api-error filtering —
///   N/A: no `progress`, `synthetic_api_error`, or `local_command` message
///   types exist; `System` is *rejected* at [`convert_message`], never
///   filtered-to-user.
/// * `stripTargets` error-block stripping (PDF/image/request-too-large → strip
///   `document`/`image` from the preceding `isMeta` user) — N/A: requires an
///   `isMeta` flag and `isSyntheticApiErrorMessage` markers; the protocol has
///   no `isMeta` flag (see `conversation.rs:1389`, `turn_loop.rs:940`) and no
///   `RequestTooLarge`/`PdfTooLarge` markers.
/// * `stripToolReferenceBlocksFromUserMessage` / `TOOL_REFERENCE_TURN_BOUNDARY`
///   injection — N/A: no `tool_reference` content block exists; tool search
///   returns a plain name list (`tools/meta/src/tool_search.rs:20`).
/// * assistant tool-input normalization (`normalizeToolInputForAPI` stripping
///   `plan`/`caller`/synthetic-edit fields) — N/A: LingXi has no
///   `normalizeToolInput` *producer* to reverse; tool inputs are model-authored
///   and pass through unmodified (`tools/plan/src/plan_mode.rs:150,466`).
///   Stripping a model-authored field would corrupt faithful round-trips.
#[must_use]
pub fn normalize_messages_for_api(
    messages: Vec<ConversationMessage>,
) -> Vec<ConversationMessage> {
    let mut out: Vec<ConversationMessage> = Vec::with_capacity(messages.len());
    for msg in messages {
        match (out.last_mut(), msg) {
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
                join_text_at_seam(prev_content, new_content);
                hoist_tool_results(prev_content);
            }
            (_, msg) => out.push(msg),
        }
    }
    out
}

/// Append `b` onto `a`, first joining a text|text seam with a `'\n'`.
///
/// Faithful port of claude-code `joinTextAtSeam` (`utils/messages.ts:2505`):
/// when `a`'s last block and `b`'s first block are both `Text`, the `'\n'` is
/// appended to `a`'s last text so no block's leading bytes change.
fn join_text_at_seam(a: &mut Vec<ProtoBlock>, mut b: Vec<ProtoBlock>) {
    if let (Some(ProtoBlock::Text { text: last }), Some(ProtoBlock::Text { .. })) =
        (a.last_mut(), b.first())
    {
        last.push('\n');
    }
    a.append(&mut b);
}

/// Stable-partition `ToolResult` blocks to the front, preserving relative order
/// within each group.
///
/// Faithful port of claude-code `hoistToolResults` (`utils/messages.ts:2470`):
/// tool_result blocks must lead the user turn to avoid "tool result must follow
/// tool use" API errors.
fn hoist_tool_results(content: &mut [ProtoBlock]) {
    // Stable sort on a boolean key = stable partition: `false` (tool_result)
    // sorts before `true` (everything else), and `sort_by_key` preserves the
    // relative order of equal-keyed elements within each group.
    content.sort_by_key(|b| !matches!(b, ProtoBlock::ToolResult { .. }));
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
            // `id` IS the canonical provider-issued id (Anthropic `toolu_…`,
            // OpenAI `call_…`). `provider_id` is now vestigial/always None, so
            // this resolves to `id.to_string()` = the canonical id (byte parity).
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
            // Must echo the same canonical id the paired `tool_use` carried so the
            // provider pairs them; `provider_tool_use_id` is vestigial/always None.
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
        // Low-frequency server-side blocks: replayed verbatim into the next API
        // request so the provider round-trips them (protected-thinking/advisor/
        // connector betas). The Anthropic encoder round-trips RedactedThinking +
        // ServerToolUse and rejects ConnectorText/AdvisorToolResult on egress.
        ProtoBlock::RedactedThinking { data } => Ok(LlmBlock::RedactedThinking { data }),
        ProtoBlock::ServerToolUse { id, name, input } => {
            Ok(LlmBlock::ServerToolUse { id, name, input })
        }
        ProtoBlock::ConnectorText {
            connector_text,
            signature,
        } => Ok(LlmBlock::ConnectorText {
            connector_text,
            signature,
        }),
        ProtoBlock::AdvisorToolResult {
            tool_use_id,
            content,
            is_error,
        } => Ok(LlmBlock::AdvisorToolResult {
            tool_use_id,
            content,
            is_error,
        }),
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
        // P0: the canonical provider id (Anthropic `toolu_…`) carried in the
        // `ToolUseId` MUST be replayed verbatim as the egress `tool_call` id.
        let msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ProtoBlock::ToolUse {
                id: ToolUseId::from("toolu_01ABCDEF"),
                name: "Read".to_string(),
                input: serde_json::json!({"path": "/tmp/x"}),
                provider_id: None,
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
        // P0: the paired `tool_result` must echo the SAME verbatim canonical id.
        let msg = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ProtoBlock::ToolResult {
                tool_use_id: ToolUseId::from("toolu_01ABCDEF"),
                content: "file content".to_string(),
                is_error: false,
                provider_tool_use_id: None,
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
    fn redacted_thinking_replayed_verbatim() {
        let msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ProtoBlock::RedactedThinking {
                data: "enc==".to_string(),
            }],
            stop_reason: None,
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::RedactedThinking { data } if data == "enc=="
        ));
    }

    #[test]
    fn server_tool_use_replayed_verbatim() {
        let msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ProtoBlock::ServerToolUse {
                id: "srvtoolu_01".to_string(),
                name: "web_search".to_string(),
                input: serde_json::json!({"query": "rust"}),
            }],
            stop_reason: None,
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::ServerToolUse { id, name, input }
                if id == "srvtoolu_01" && name == "web_search" && input["query"] == "rust"
        ));
    }

    #[test]
    fn connector_text_and_advisor_result_replayed_verbatim() {
        let msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![
                ProtoBlock::ConnectorText {
                    connector_text: "hi".to_string(),
                    signature: Some("sig".to_string()),
                },
                ProtoBlock::AdvisorToolResult {
                    tool_use_id: "srvtoolu_01".to_string(),
                    content: serde_json::json!("ok"),
                    is_error: true,
                },
            ],
            stop_reason: None,
        };
        let result = to_llm_messages(vec![msg]).unwrap();
        assert!(matches!(
            &result[0].content[0],
            LlmBlock::ConnectorText { connector_text, signature }
                if connector_text == "hi" && signature.as_deref() == Some("sig")
        ));
        assert!(matches!(
            &result[0].content[1],
            LlmBlock::AdvisorToolResult { tool_use_id, content, is_error }
                if tool_use_id == "srvtoolu_01" && content == "ok" && *is_error
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
                // joinTextAtSeam inserts a `\n` on a's last text at a text|text seam.
                assert_eq!(text_of(content), vec!["a\n", "b"]);
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
                // Each text|text seam (a|b then b|c) gets its own `\n`.
                assert_eq!(text_of(content), vec!["a\n", "b\n", "c"]);
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
                assert_eq!(text_of(content), vec!["a\n", "b"]);
            }
            other => panic!("expected merged User, got {other:?}"),
        }
        assert!(matches!(out[2], ConversationMessage::Assistant { .. }));
    }

    // ── hoistToolResults + joinTextAtSeam (mergeUserMessages pipeline) ────────

    fn user_blocks(id: MessageId, content: Vec<ProtoBlock>) -> ConversationMessage {
        ConversationMessage::User { id, content }
    }

    fn tool_result(content: &str) -> ProtoBlock {
        ProtoBlock::ToolResult {
            tool_use_id: ToolUseId::new(),
            content: content.to_string(),
            is_error: false,
            provider_tool_use_id: None,
        }
    }

    fn image() -> ProtoBlock {
        ProtoBlock::Image {
            source: ImageSource::Url { url: "https://example.com/i.png".to_string() },
        }
    }

    /// Classify a block as one of a few coarse kinds for order assertions.
    fn kinds(blocks: &[ProtoBlock]) -> Vec<&'static str> {
        blocks
            .iter()
            .map(|b| match b {
                ProtoBlock::Text { .. } => "text",
                ProtoBlock::ToolResult { .. } => "tool_result",
                ProtoBlock::Image { .. } => "image",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn merge_two_text_users_inserts_newline_at_seam() {
        let id = MessageId::new();
        let out = normalize_messages_for_api(vec![user(id, "a"), user(MessageId::new(), "b")]);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ConversationMessage::User { content, .. } => {
                assert_eq!(text_of(content), vec!["a\n", "b"]);
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn merge_hoists_tool_result_before_text() {
        // [User[Text"hi"], User[ToolResult, Text"after"]] → tool_result leads.
        let id = MessageId::new();
        let out = normalize_messages_for_api(vec![
            user_blocks(id, vec![ProtoBlock::Text { text: "hi".to_string() }]),
            user_blocks(
                MessageId::new(),
                vec![tool_result("r"), ProtoBlock::Text { text: "after".to_string() }],
            ),
        ]);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ConversationMessage::User { content, .. } => {
                // hoist: tool_result first, then the two text blocks in order.
                // No seam `\n` is added because b leads with a non-text block,
                // so the text|text adjacency never occurs at the seam.
                assert_eq!(kinds(content), vec!["tool_result", "text", "text"]);
                assert_eq!(text_of(&content[1..]), vec!["hi", "after"]);
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn merge_toolresult_user_then_image_user_keeps_toolresult_leading() {
        // The real Read-image shape: [User[ToolResult], User[Image]] → [ToolResult, Image].
        let id = MessageId::new();
        let out = normalize_messages_for_api(vec![
            user_blocks(id, vec![tool_result("file bytes")]),
            user_blocks(MessageId::new(), vec![image()]),
        ]);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ConversationMessage::User { content, .. } => {
                assert_eq!(kinds(content), vec!["tool_result", "image"]);
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn hoist_preserves_relative_order_within_groups() {
        // [tr1, txtX, tr2, txtY] across two users → [tr1, tr2, txtX, txtY].
        let id = MessageId::new();
        let out = normalize_messages_for_api(vec![
            user_blocks(
                id,
                vec![tool_result("tr1"), ProtoBlock::Text { text: "X".to_string() }],
            ),
            user_blocks(
                MessageId::new(),
                vec![tool_result("tr2"), ProtoBlock::Text { text: "Y".to_string() }],
            ),
        ]);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ConversationMessage::User { content, .. } => {
                assert_eq!(
                    kinds(content),
                    vec!["tool_result", "tool_result", "text", "text"]
                );
                // intra-group order preserved: tr1 before tr2, X before Y.
                match (&content[0], &content[1]) {
                    (
                        ProtoBlock::ToolResult { content: c0, .. },
                        ProtoBlock::ToolResult { content: c1, .. },
                    ) => {
                        assert_eq!(c0, "tr1");
                        assert_eq!(c1, "tr2");
                    }
                    _ => panic!("expected two leading tool_results"),
                }
                assert_eq!(text_of(&content[2..]), vec!["X", "Y"]);
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn seam_no_newline_when_b_leads_with_non_text() {
        // [User[Text"a"], User[ToolResult]] → hoist runs, no seam `\n`.
        let id = MessageId::new();
        let out = normalize_messages_for_api(vec![
            user_blocks(id, vec![ProtoBlock::Text { text: "a".to_string() }]),
            user_blocks(MessageId::new(), vec![tool_result("r")]),
        ]);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ConversationMessage::User { content, .. } => {
                assert_eq!(kinds(content), vec!["tool_result", "text"]);
                // text block kept its exact bytes — no trailing `\n`.
                assert_eq!(text_of(&content[1..]), vec!["a"]);
            }
            other => panic!("expected User, got {other:?}"),
        }
    }
}
