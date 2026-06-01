//! Canonical request → Google `Gemini` `generateContent` request body (pure).

use crate::request::CanonicalRequest;
use protocol::{ContentBlock, ConversationMessage};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Default `Gemini` API base URL (no trailing slash).
pub const GEMINI_DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Build the `Gemini` `generateContent` request body for `req`.
///
/// Maps the canonical conversation to `Gemini`'s `contents` array (`user` /
/// `model` roles + `functionCall` / `functionResponse` parts), hoists the
/// system prompt to `systemInstruction`, and translates canonical tool schemas
/// to `functionDeclarations`. `Thinking` blocks are dropped.
#[must_use]
pub fn encode_generate_body(req: &CanonicalRequest) -> Value {
    // Gemini pairs functionResponse → functionCall by NAME, but a canonical
    // ToolResult carries only the tool_use_id. Build id → name from every
    // ToolUse block first, so a ToolResult can recover the function name.
    let id_to_name = build_id_name_map(&req.messages);

    let mut contents: Vec<Value> = Vec::new();
    let mut system_text = req.system.clone().unwrap_or_default();

    for msg in &req.messages {
        match msg {
            ConversationMessage::System { content, .. } => {
                if !system_text.is_empty() {
                    system_text.push('\n');
                }
                system_text.push_str(content);
            }
            ConversationMessage::User { content, .. } => {
                if let Some(c) = encode_user(content, &id_to_name) {
                    contents.push(c);
                }
            }
            ConversationMessage::Assistant { content, .. } => {
                if let Some(c) = encode_assistant(content) {
                    contents.push(c);
                }
            }
        }
    }

    let mut body = Map::new();
    body.insert("contents".to_string(), Value::Array(contents));
    if !system_text.is_empty() {
        body.insert(
            "systemInstruction".to_string(),
            json!({"parts": [{"text": system_text}]}),
        );
    }
    if !req.tools.is_empty() {
        let decls: Vec<Value> = req.tools.iter().map(encode_tool).collect();
        body.insert(
            "tools".to_string(),
            json!([{"functionDeclarations": decls}]),
        );
    }
    let mut gen_config = Map::new();
    gen_config.insert("maxOutputTokens".to_string(), json!(req.max_tokens));
    if let Some(t) = req.temperature {
        gen_config.insert("temperature".to_string(), json!(t));
    }
    body.insert("generationConfig".to_string(), Value::Object(gen_config));
    Value::Object(body)
}

/// Map every `ToolUse` block's `id.as_uuid()` string → its function name.
fn build_id_name_map(messages: &[ConversationMessage]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for msg in messages {
        let blocks = match msg {
            ConversationMessage::User { content, .. }
            | ConversationMessage::Assistant { content, .. } => content,
            ConversationMessage::System { .. } => continue,
        };
        for b in blocks {
            if let ContentBlock::ToolUse { id, name, .. } = b {
                m.insert(id.as_uuid().to_string(), name.clone());
            }
        }
    }
    m
}

/// Canonical tool (`{name, description, input_schema}`) → `Gemini`
/// `functionDeclaration` (`{name, description, parameters}`).
fn encode_tool(tool: &Value) -> Value {
    let name = tool.get("name").cloned().unwrap_or(Value::Null);
    let description = tool.get("description").cloned().unwrap_or(Value::Null);
    let parameters = tool
        .get("input_schema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}));
    json!({"name": name, "description": description, "parameters": parameters})
}

/// User content → one `{role:"user", parts:[…]}` (text + `functionResponse`).
fn encode_user(content: &[ContentBlock], id_to_name: &BTreeMap<String, String>) -> Option<Value> {
    let mut parts: Vec<Value> = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text } => parts.push(json!({"text": text})),
            ContentBlock::ToolResult {
                tool_use_id,
                content: result,
                is_error,
            } => {
                let name = id_to_name
                    .get(&tool_use_id.as_uuid().to_string())
                    .cloned()
                    .unwrap_or_default();
                let response = if *is_error {
                    json!({"error": result})
                } else {
                    json!({"result": result})
                };
                parts.push(json!({"functionResponse": {"name": name, "response": response}}));
            }
            ContentBlock::Thinking { .. } | ContentBlock::ToolUse { .. } => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(json!({"role": "user", "parts": parts}))
    }
}

/// Assistant content → one `{role:"model", parts:[…]}` (text + `functionCall`).
fn encode_assistant(content: &[ContentBlock]) -> Option<Value> {
    let mut parts: Vec<Value> = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text } => parts.push(json!({"text": text})),
            ContentBlock::ToolUse { name, input, .. } => {
                parts.push(json!({"functionCall": {"name": name, "args": input}}));
            }
            ContentBlock::ToolResult { .. } | ContentBlock::Thinking { .. } => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(json!({"role": "model", "parts": parts}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};

    #[test]
    fn system_prompt_hoisted_to_system_instruction() {
        let mut req = CanonicalRequest::new("gemini-2.0-flash");
        req.system = Some("be helpful".to_string());
        req.messages = vec![ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        }];
        let body = encode_generate_body(&req);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be helpful");
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
    }

    #[test]
    fn tools_become_function_declarations() {
        let mut req = CanonicalRequest::new("gemini-2.0-flash");
        req.tools = vec![json!({"name":"Read","description":"d","input_schema":{"type":"object"}})];
        let body = encode_generate_body(&req);
        let decl = &body["tools"][0]["functionDeclarations"][0];
        assert_eq!(decl["name"], "Read");
        assert_eq!(decl["parameters"]["type"], "object");
    }

    #[test]
    fn assistant_tool_use_then_tool_result_pairs_by_name() {
        let id = ToolUseId::new();
        let assistant = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolUse {
                id,
                name: "Bash".to_string(),
                input: json!({"command": "ls"}),
            }],
            stop_reason: Some("tool_use".to_string()),
        };
        let user = ConversationMessage::User {
            id: MessageId::new(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id,
                content: "file1 file2".to_string(),
                is_error: false,
            }],
        };
        let mut req = CanonicalRequest::new("gemini-2.0-flash");
        req.messages = vec![assistant, user];
        let body = encode_generate_body(&req);
        // model turn carries the functionCall
        assert_eq!(body["contents"][0]["role"], "model");
        assert_eq!(
            body["contents"][0]["parts"][0]["functionCall"]["name"],
            "Bash"
        );
        assert_eq!(
            body["contents"][0]["parts"][0]["functionCall"]["args"]["command"],
            "ls"
        );
        // user turn answers with functionResponse matched on the SAME name
        assert_eq!(body["contents"][1]["role"], "user");
        assert_eq!(
            body["contents"][1]["parts"][0]["functionResponse"]["name"],
            "Bash"
        );
        assert_eq!(
            body["contents"][1]["parts"][0]["functionResponse"]["response"]["result"],
            "file1 file2"
        );
    }

    #[test]
    fn error_tool_result_uses_error_key() {
        let id = ToolUseId::new();
        let req = CanonicalRequest {
            messages: vec![
                ConversationMessage::Assistant {
                    id: MessageId::new(),
                    content: vec![ContentBlock::ToolUse {
                        id,
                        name: "X".to_string(),
                        input: json!({}),
                    }],
                    stop_reason: None,
                },
                ConversationMessage::User {
                    id: MessageId::new(),
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: "boom".to_string(),
                        is_error: true,
                    }],
                },
            ],
            ..CanonicalRequest::new("gemini-2.0-flash")
        };
        let body = encode_generate_body(&req);
        assert_eq!(
            body["contents"][1]["parts"][0]["functionResponse"]["response"]["error"],
            "boom"
        );
    }
}
