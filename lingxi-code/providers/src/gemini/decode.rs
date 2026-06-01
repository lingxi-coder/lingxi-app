//! Google `Gemini` `generateContent` response → canonical `MessageResponse`.

use api_client::types::{ContentBlockApi, MessageResponse, UsageApi};
use api_client::ApiError;
use protocol::ToolUseId;
use serde_json::Value;

/// Map a `Gemini` `finishReason` to the canonical stop-reason vocabulary.
/// (Tool-use is detected separately by the presence of a `functionCall`.)
#[must_use]
pub fn map_finish_reason(gemini: &str) -> String {
    match gemini {
        "STOP" => "end_turn".to_string(),
        "MAX_TOKENS" => "max_tokens".to_string(),
        other => other.to_string(),
    }
}

/// Decode a non-streaming `Gemini` response body.
///
/// # Errors
/// * Non-2xx → [`ApiError::Server`]. Unparseable / shape-invalid 2xx →
///   [`ApiError::MalformedStream`].
pub fn decode_generate_response(status: u16, body: &str) -> Result<MessageResponse, ApiError> {
    if !(200..300).contains(&status) {
        return Err(ApiError::Server {
            status,
            body: body.to_string(),
        });
    }
    let root: Value = serde_json::from_str(body)
        .map_err(|e| ApiError::MalformedStream(format!("gemini response decode: {e}")))?;

    let id = root
        .get("responseId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let model = root
        .get("modelVersion")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let candidate = root
        .get("candidates")
        .and_then(|c| c.get(0))
        .ok_or_else(|| ApiError::MalformedStream("gemini response: no candidates".to_string()))?;

    let mut content: Vec<ContentBlockApi> = Vec::new();
    let mut saw_tool = false;
    if let Some(parts) = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
    {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    content.push(ContentBlockApi::Text {
                        text: text.to_string(),
                    });
                }
            } else if let Some(fc) = part.get("functionCall") {
                saw_tool = true;
                let name = fc
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let input = fc
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                content.push(ContentBlockApi::ToolUse {
                    id: ToolUseId::new(),
                    name,
                    input,
                });
            }
        }
    }

    // Gemini's finishReason stays STOP even with a functionCall, so detect
    // tool-use by the presence of a functionCall part.
    let stop_reason = if saw_tool {
        Some("tool_use".to_string())
    } else {
        candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .map(map_finish_reason)
    };

    let usage = usage_from_value(root.get("usageMetadata"));

    Ok(MessageResponse {
        id,
        model,
        content,
        stop_reason,
        usage,
    })
}

/// Map `Gemini` `usageMetadata` to canonical `UsageApi`.
#[must_use]
pub fn usage_from_value(usage: Option<&Value>) -> UsageApi {
    let Some(u) = usage else {
        return UsageApi::default();
    };
    UsageApi {
        input_tokens: u
            .get("promptTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: u
            .get("candidatesTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: u
            .get("cachedContentTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"{"modelVersion":"gemini-2.0-flash","candidates":[{"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":2}}"#;
    const TOOL: &str = r#"{"modelVersion":"gemini-2.0-flash","candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"Bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":20,"candidatesTokenCount":5,"cachedContentTokenCount":4}}"#;

    #[test]
    fn non_2xx_is_server_error() {
        assert!(matches!(
            decode_generate_response(403, "denied").unwrap_err(),
            ApiError::Server { status: 403, .. }
        ));
    }

    #[test]
    fn text_response_decodes() {
        let r = decode_generate_response(200, TEXT).unwrap();
        assert_eq!(r.model, "gemini-2.0-flash");
        assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
        match &r.content[0] {
            ContentBlockApi::Text { text } => assert_eq!(text, "hello"),
            other => panic!("expected text, got {other:?}"),
        }
        assert_eq!(r.usage.input_tokens, 8);
    }

    #[test]
    fn function_call_decodes_as_tool_use_with_tool_use_stop_reason() {
        let r = decode_generate_response(200, TOOL).unwrap();
        // finishReason is STOP but a functionCall is present → tool_use.
        assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
        match &r.content[0] {
            ContentBlockApi::ToolUse { name, input, .. } => {
                assert_eq!(name, "Bash");
                assert_eq!(input["command"], "ls");
            }
            other => panic!("expected tool_use, got {other:?}"),
        }
        assert_eq!(r.usage.cache_read_input_tokens, 4);
    }

    #[test]
    fn finish_reason_map() {
        assert_eq!(map_finish_reason("STOP"), "end_turn");
        assert_eq!(map_finish_reason("MAX_TOKENS"), "max_tokens");
        assert_eq!(map_finish_reason("SAFETY"), "SAFETY");
    }
}
