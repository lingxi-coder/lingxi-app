//! `OpenAI` chat-completions response → canonical `MessageResponse` (pure).

use api_client::types::{ContentBlockApi, MessageResponse, UsageApi};
use api_client::ApiError;
use protocol::ToolUseId;
use serde_json::Value;

/// Map an `OpenAI` `finish_reason` to the canonical stop-reason vocabulary.
#[must_use]
pub fn map_finish_reason(openai: &str) -> String {
    match openai {
        "tool_calls" => "tool_use".to_string(),
        "stop" => "end_turn".to_string(),
        "length" => "max_tokens".to_string(),
        other => other.to_string(),
    }
}

/// Decode a non-streaming `OpenAI` response body.
///
/// # Errors
/// * Non-2xx status → [`ApiError::Server`] (carrying the body for diagnostics).
/// * Unparseable / shape-invalid 2xx → [`ApiError::MalformedStream`].
pub fn decode_chat_response(status: u16, body: &str) -> Result<MessageResponse, ApiError> {
    if !(200..300).contains(&status) {
        return Err(ApiError::Server {
            status,
            body: body.to_string(),
        });
    }
    let root: Value = serde_json::from_str(body)
        .map_err(|e| ApiError::MalformedStream(format!("openai response decode: {e}")))?;

    let id = root
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let model = root
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let choice = root
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| ApiError::MalformedStream("openai response: no choices".to_string()))?;
    let message = choice
        .get("message")
        .ok_or_else(|| ApiError::MalformedStream("openai response: no message".to_string()))?;

    let mut content: Vec<ContentBlockApi> = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            content.push(ContentBlockApi::Text { text: text.to_string() });
        }
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tc in tool_calls {
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let args_str = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}");
            // OpenAI `arguments` is a JSON *string*; parse to a Value (fall back
            // to an empty object on malformed partials).
            let input = serde_json::from_str::<Value>(args_str)
                .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
            content.push(ContentBlockApi::ToolUse {
                id: ToolUseId::new(),
                name,
                input,
            });
        }
    }

    let stop_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(map_finish_reason);

    let usage = decode_usage(root.get("usage"));

    Ok(MessageResponse {
        id,
        model,
        content,
        stop_reason,
        usage,
    })
}

/// Map `OpenAI` `usage` to canonical `UsageApi`. Cached input tokens (when
/// present under `prompt_tokens_details.cached_tokens`) map to `cache_read`.
fn decode_usage(usage: Option<&Value>) -> UsageApi {
    let Some(u) = usage else {
        return UsageApi::default();
    };
    let input = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output = u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
    let cache_read = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    UsageApi {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT_RESP: &str = r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#;
    const TOOL_RESP: &str = r#"{"id":"chatcmpl-2","model":"gpt-4o","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_x","type":"function","function":{"name":"Read","arguments":"{\"path\":\"/x\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":20,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":8}}}"#;

    #[test]
    fn non_2xx_is_server_error() {
        let err = decode_chat_response(429, "rate limited").unwrap_err();
        assert!(matches!(err, ApiError::Server { status: 429, .. }));
    }

    #[test]
    fn text_response_decodes() {
        let r = decode_chat_response(200, TEXT_RESP).unwrap();
        assert_eq!(r.id, "chatcmpl-1");
        assert_eq!(r.model, "gpt-4o");
        assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
        match &r.content[0] {
            ContentBlockApi::Text { text } => assert_eq!(text, "hello"),
            other => panic!("expected text, got {other:?}"),
        }
        assert_eq!(r.usage.input_tokens, 10);
        assert_eq!(r.usage.output_tokens, 2);
    }

    #[test]
    fn tool_call_response_decodes_with_parsed_input_and_cache() {
        let r = decode_chat_response(200, TOOL_RESP).unwrap();
        assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
        match &r.content[0] {
            ContentBlockApi::ToolUse { name, input, .. } => {
                assert_eq!(name, "Read");
                assert_eq!(input["path"], "/x");
            }
            other => panic!("expected tool_use, got {other:?}"),
        }
        assert_eq!(r.usage.cache_read_input_tokens, 8);
    }

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(map_finish_reason("tool_calls"), "tool_use");
        assert_eq!(map_finish_reason("stop"), "end_turn");
        assert_eq!(map_finish_reason("length"), "max_tokens");
        assert_eq!(map_finish_reason("content_filter"), "content_filter");
    }
}
