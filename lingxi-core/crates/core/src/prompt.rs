//! Build the `Anthropic`-shape request body from session state + new user input.
//!
//! M1.1: minimal — no system prompt yet, no tools, no thinking config.
//! Later plans extend with `system_prompt`, tools list, thinking budget, etc.

use crate::session::SessionState;
use serde_json::{json, Value};

/// Assemble the JSON request body for one turn given the current session and a new user message.
#[must_use]
pub fn assemble_request(session: &SessionState, user_message: &str) -> Value {
    let mut messages: Vec<Value> = session.history.iter().map(message_to_api_shape).collect();
    messages.push(json!({"role": "user", "content": user_message}));

    json!({
        "model": session.model,
        "max_tokens": 8192,
        "messages": messages,
    })
}

fn message_to_api_shape(m: &lingxi_protocol::ConversationMessage) -> Value {
    use lingxi_protocol::ConversationMessage;
    match m {
        ConversationMessage::User { content, .. } => {
            json!({"role": "user", "content": content_blocks_to_api(content)})
        }
        ConversationMessage::Assistant { content, .. } => {
            json!({"role": "assistant", "content": content_blocks_to_api(content)})
        }
        ConversationMessage::System { content, .. } => {
            json!({"role": "system", "content": content})
        }
    }
}

fn content_blocks_to_api(blocks: &[lingxi_protocol::ContentBlock]) -> Value {
    use lingxi_protocol::ContentBlock;
    let arr: Vec<Value> = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => json!({"type": "text", "text": text}),
            ContentBlock::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                json!({"type": "tool_result", "tool_use_id": tool_use_id,
                       "content": content, "is_error": is_error})
            }
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                json!({"type": "thinking", "thinking": thinking, "signature": signature})
            }
        })
        .collect();
    Value::Array(arr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;
    use lingxi_protocol::SessionId;

    #[test]
    fn assemble_includes_history_and_new_user_message() {
        let session = SessionState::empty(SessionId::nil(), "claude-opus-4-6".into());
        let req = assemble_request(&session, "what's 2+2?");
        let body = req.as_object().unwrap();
        assert_eq!(body["model"], "claude-opus-4-6");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn assemble_appends_prior_history() {
        use lingxi_protocol::{ConversationMessage, MessageId};
        let mut session = SessionState::empty(SessionId::nil(), "claude-opus-4-6".into());
        session.history.push(ConversationMessage::user(
            MessageId::nil(),
            "earlier".into(),
        ));
        let req = assemble_request(&session, "now");
        let messages = req["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "user");
    }
}
