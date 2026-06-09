//! Canonical request → `OpenAI` `/chat/completions` request body (pure).

use crate::request::CanonicalRequest;
use protocol::{ContentBlock, ConversationMessage, ImageSource};
use serde_json::{json, Map, Value};

/// Default `OpenAI` API base URL (no trailing slash).
pub const OPENAI_DEFAULT_BASE: &str = "https://api.openai.com/v1";

/// Build the `OpenAI` chat-completions request body for `req`.
///
/// Maps the canonical conversation to `OpenAI`'s `messages` array (system /
/// user / assistant / tool roles), translates canonical tool schemas to
/// `OpenAI` `function` tools, and sets `stream`. `Thinking` blocks are dropped
/// (no `OpenAI` equivalent).
///
/// When `reasoning_effort` is `Some`, emits `reasoning_effort` +
/// `max_completion_tokens` and omits `temperature`. Otherwise emits `max_tokens`
/// and `temperature` (if set) — identical to the pre-reasoning behaviour.
#[must_use]
pub fn encode_chat_body(
    req: &CanonicalRequest,
    reasoning_effort: Option<crate::request::ReasoningEffort>,
) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = &req.system {
        messages.push(json!({"role": "system", "content": system}));
    }
    for msg in &req.messages {
        encode_message(msg, &mut messages);
    }

    let mut body = Map::new();
    body.insert("model".to_string(), json!(req.model));
    body.insert("messages".to_string(), Value::Array(messages));
    if let Some(effort) = reasoning_effort {
        body.insert("max_completion_tokens".to_string(), json!(req.max_tokens));
        body.insert("reasoning_effort".to_string(), json!(effort.as_str()));
    } else {
        body.insert("max_tokens".to_string(), json!(req.max_tokens));
        if let Some(t) = req.temperature {
            body.insert("temperature".to_string(), json!(t));
        }
    }
    if !req.tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(req.tools.iter().map(encode_tool).collect()),
        );
    }
    if req.stream {
        body.insert("stream".to_string(), json!(true));
        // Ask for a final usage chunk on the stream.
        body.insert("stream_options".to_string(), json!({"include_usage": true}));
    }
    Value::Object(body)
}

/// Translate one canonical tool schema (`{name, description, input_schema}`)
/// into an `OpenAI` function tool (`{type:"function", function:{...}}`).
fn encode_tool(tool: &Value) -> Value {
    let name = tool.get("name").cloned().unwrap_or(Value::Null);
    let description = tool.get("description").cloned().unwrap_or(Value::Null);
    let parameters = tool
        .get("input_schema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}));
    json!({
        "type": "function",
        "function": {"name": name, "description": description, "parameters": parameters}
    })
}

/// Flatten one canonical message into 0..n `OpenAI` messages.
fn encode_message(msg: &ConversationMessage, out: &mut Vec<Value>) {
    match msg {
        ConversationMessage::System { content, .. } => {
            out.push(json!({"role": "system", "content": content}));
        }
        ConversationMessage::User { content, .. } => encode_user(content, out),
        ConversationMessage::Assistant { content, .. } => encode_assistant(content, out),
    }
}

/// User content: text + images → one `user` message (string `content` when
/// text-only, else an array of `text`/`image_url` parts); tool results → `tool`
/// messages (each answers one `tool_call_id`).
fn encode_user(content: &[ContentBlock], out: &mut Vec<Value>) {
    let mut text = String::new();
    let mut images: Vec<Value> = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text: t } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            ContentBlock::Image { source } => {
                images.push(json!({
                    "type": "image_url",
                    "image_url": {"url": openai_image_url(source)},
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content: result,
                is_error,
            } => {
                // OpenAI has no is_error flag; prefix on error so the model sees it.
                let body = if *is_error {
                    format!("[error] {result}")
                } else {
                    result.clone()
                };
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id.as_uuid().to_string(),
                    "content": body,
                }));
            }
            // OpenAI has no native PDF input — emit a text placeholder so the
            // model is at least told a document was present.
            ContentBlock::Document { .. } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str("[PDF document — not supported on this provider]");
            }
            // Thinking has no OpenAI equivalent; ToolUse in a user message is malformed — drop both.
            ContentBlock::Thinking { .. } | ContentBlock::ToolUse { .. } => {}
        }
    }
    if images.is_empty() {
        if !text.is_empty() {
            out.push(json!({"role": "user", "content": text}));
        }
    } else {
        let mut parts: Vec<Value> = Vec::new();
        if !text.is_empty() {
            parts.push(json!({"type": "text", "text": text}));
        }
        parts.extend(images);
        out.push(json!({"role": "user", "content": Value::Array(parts)}));
    }
}

/// Build an `OpenAI` `image_url.url` from a canonical image source: base64 → a
/// `data:` URL; a remote URL passes through unchanged.
fn openai_image_url(source: &ImageSource) -> String {
    match source {
        ImageSource::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
        ImageSource::Url { url } => url.clone(),
    }
}

/// Assistant content: text + tool-use blocks → one `assistant` message with
/// `content` and/or `tool_calls`.
fn encode_assistant(content: &[ContentBlock], out: &mut Vec<Value>) {
    let mut text = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text: t } => text.push_str(t),
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(json!({
                    "id": id.as_uuid().to_string(),
                    "type": "function",
                    "function": {"name": name, "arguments": input.to_string()},
                }));
            }
            ContentBlock::ToolResult { .. }
            | ContentBlock::Thinking { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::Document { .. } => {}
        }
    }
    let mut m = Map::new();
    m.insert("role".to_string(), json!("assistant"));
    // OpenAI requires `content` to be present (may be null) on an assistant msg.
    m.insert(
        "content".to_string(),
        if text.is_empty() {
            Value::Null
        } else {
            json!(text)
        },
    );
    if !tool_calls.is_empty() {
        m.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    out.push(Value::Object(m));
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ContentBlock, ConversationMessage, ImageSource, MessageId, ToolUseId};

    fn user_text(s: &str) -> ConversationMessage {
        ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Text {
                text: s.to_string(),
            }],
        }
    }

    #[test]
    fn system_prompt_becomes_first_message() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.system = Some("be helpful".to_string());
        req.messages = vec![user_text("hi")];
        let body = encode_chat_body(&req, None);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "be helpful");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "hi");
        assert_eq!(body["model"], "gpt-4o");
    }

    #[test]
    fn tools_translate_to_function_shape() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.tools = vec![serde_json::json!({
            "name": "Read", "description": "read a file",
            "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
        })];
        let body = encode_chat_body(&req, None);
        let tool = &body["tools"][0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["function"]["name"], "Read");
        assert_eq!(tool["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn assistant_tool_use_and_tool_result_share_id() {
        let id = ToolUseId::new();
        let id_str = id.as_uuid().to_string();
        let assistant = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id,
                name: "Read".to_string(),
                input: serde_json::json!({"path": "/x"}),
            }],
            stop_reason: Some("tool_use".to_string()),
        };
        let user = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id,
                content: "file body".to_string(),
                is_error: false,
            }],
        };
        let mut req = CanonicalRequest::new("gpt-4o");
        req.messages = vec![assistant, user];
        let body = encode_chat_body(&req, None);
        let msgs = body["messages"].as_array().unwrap();
        // assistant message with tool_calls
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["tool_calls"][0]["id"], id_str);
        assert_eq!(msgs[0]["tool_calls"][0]["function"]["name"], "Read");
        // tool message answering the same id
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], id_str);
        assert_eq!(msgs[1]["content"], "file body");
    }

    #[test]
    fn stream_sets_stream_and_usage_options() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.stream = true;
        let body = encode_chat_body(&req, None);
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn thinking_block_is_dropped() {
        let assistant = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![
                ContentBlock::Thinking {
                    thinking: "hmm".to_string(),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "answer".to_string(),
                },
            ],
            stop_reason: None,
        };
        let mut req = CanonicalRequest::new("gpt-4o");
        req.messages = vec![assistant];
        let body = encode_chat_body(&req, None);
        assert_eq!(body["messages"][0]["content"], "answer");
        assert!(body["messages"][0].get("thinking").is_none());
    }

    #[test]
    fn text_only_user_stays_string_content() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.messages = vec![user_text("hi")];
        let body = encode_chat_body(&req, None);
        assert_eq!(body["messages"][0]["content"], "hi"); // string, not array
    }

    #[test]
    fn user_image_emits_parts_array_with_data_url() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.messages = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![
                ContentBlock::Text { text: "look".to_string() },
                ContentBlock::Image {
                    source: ImageSource::Base64 {
                        media_type: "image/png".to_string(),
                        data: "YWJj".to_string(),
                    },
                },
            ],
        }];
        let body = encode_chat_body(&req, None);
        let parts = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "look");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,YWJj");
    }

    #[test]
    fn user_image_url_passes_through() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.messages = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Image {
                source: ImageSource::Url { url: "https://x/y.png".to_string() },
            }],
        }];
        let body = encode_chat_body(&req, None);
        assert_eq!(
            body["messages"][0]["content"][0]["image_url"]["url"],
            "https://x/y.png"
        );
    }

    #[test]
    fn reasoning_effort_uses_max_completion_tokens_and_no_temperature() {
        let mut req = CanonicalRequest::new("o3-mini");
        req.temperature = Some(0.7);
        let body = encode_chat_body(&req, Some(crate::request::ReasoningEffort::High));
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["max_completion_tokens"], req.max_tokens);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn no_reasoning_keeps_max_tokens_and_temperature() {
        let mut req = CanonicalRequest::new("gpt-4o");
        req.temperature = Some(0.5);
        let body = encode_chat_body(&req, None);
        assert_eq!(body["max_tokens"], req.max_tokens);
        assert_eq!(body["temperature"], 0.5);
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("max_completion_tokens").is_none());
    }
}
