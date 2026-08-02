#[allow(missing_docs)]
mod anthropic;
#[allow(missing_docs)]
mod azure_openai;
#[allow(missing_docs)]
pub mod bedrock_claude;
#[allow(missing_docs)]
pub mod foundry_claude;
#[allow(missing_docs)]
mod gemini;
pub mod gemini_files;
#[allow(missing_docs)]
mod openai;
#[allow(missing_docs)]
mod openai_responses;
#[allow(missing_docs)]
pub mod vertex_claude;
#[allow(missing_docs)]
pub mod vertex_gemini;

use std::time::Duration;

use crate::{LlmError, StreamDecoder, WireCodec};

pub use anthropic::AnthropicMessagesCodec;
pub use azure_openai::AzureOpenAiCodec;
pub use bedrock_claude::BedrockClaudeCodec;
pub use foundry_claude::FoundryClaudeCodec;
pub use gemini::GeminiCodec;
pub use gemini_files::GeminiFile;
pub use openai::OpenAiChatCodec;
pub use openai_responses::OpenAiResponsesCodec;
pub use vertex_claude::VertexClaudeCodec;
pub use vertex_gemini::VertexGeminiCodec;

/// Create an inner Anthropic stream decoder for delegation.
///
/// Used by [`BedrockClaudeCodec`]'s stream decoder to unwrap base64-encoded
/// Bedrock event payloads and forward them to the canonical Anthropic decoder.
pub(crate) fn bedrock_claude_inner_decoder() -> Box<dyn StreamDecoder> {
    AnthropicMessagesCodec::new("", "bedrock-2023-05-31").stream_decoder()
}

/// Is `v` truthy by JavaScript's rules?
///
/// `makeMessage` guards on `t?.message ? … : (t ? … : r)`, so an EMPTY string,
/// `0`, `false` and `null` all fall through. Modelling this as "is the key
/// present" instead would take the wrong branch for `{"message":""}`.
fn js_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        serde_json::Value::String(s) => !s.is_empty(),
        // Objects and arrays are always truthy in JS, `{}` and `[]` included.
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => true,
    }
}

/// Build the error text the way the Anthropic SDK does — `APIError.makeMessage`
/// (2.1.220 @225796853):
///
/// ```js
/// let n = t?.message ? (typeof t.message==="string" ? t.message : JSON.stringify(t.message))
///       : (t ? JSON.stringify(t) : r);
/// if (e && n) return `${e} ${n}`;
/// if (e)      return `${e} status code (no body)`;
/// if (n)      return n;
/// return "(no status code or body)";
/// ```
///
/// The STATUS PREFIX is load-bearing, not decoration. claude-code carries HTTP
/// status provenance in the message rather than a side field, and recovers it
/// downstream by stripping the prefix (`replace(/^429\s+/,"")` @230600821,
/// `replace(/^400\s+/,"")` @230602614). The 429 handler then JSON-parses the
/// remainder and reads `error.message ?? message` — which only works because the
/// WHOLE body is stringified in. Storing the bare `error.message` (what this port
/// did before) loses both the bytes and the status.
///
/// Key order matches `JSON.stringify` because the workspace enables serde_json's
/// `preserve_order`; without it the default `BTreeMap` would sort keys and the
/// bytes would diverge.
///
/// `body` is the WHOLE parsed body (`{"type":"error","error":{…}}` for
/// Anthropic), not the inner error object — the SDK reads `t?.message` at the
/// top level, which Anthropic bodies do not have, so they take the
/// stringify-everything branch.
pub(crate) fn api_error_message(status: u16, body: &serde_json::Value, fallback: &str) -> String {
    let n: String = match body.get("message") {
        Some(m) if js_truthy(m) => match m {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        _ => {
            if js_truthy(body) {
                body.to_string()
            } else {
                fallback.to_string()
            }
        }
    };
    match (status, n.is_empty()) {
        (0, true) => "(no status code or body)".to_string(),
        (0, false) => n,
        (_, true) => format!("{status} status code (no body)"),
        (_, false) => format!("{status} {n}"),
    }
}

/// HTTP-status fallback used when a provider error envelope is missing or
/// carries an unrecognized code.
///
/// `message` is the provider's RAW `error.message` and is used ONLY to
/// classify; `display` is [`api_error_message`]'s output and is what gets
/// STORED. Keeping them separate is what lets the stored text gain the status
/// prefix without shifting any of the ~24 `message.contains(...)` classifiers
/// downstream.
pub(crate) fn map_error_status(
    status: u16,
    message: &str,
    display: String,
    retry_after: Option<Duration>,
) -> LlmError {
    match status {
        // The provider's text is STORED, not just classified on: the auth copy
        // downstream gates on this wording. Dropping it here is what made the
        // whole family unreachable.
        401 => LlmError::Authentication { message: display },
        403 => LlmError::PermissionDenied { message: display },
        404 => LlmError::ModelUnavailable,
        // 413 split (parity 2.1.212): "context window" in the message means a
        // token overflow (prompt-too-long / compaction path); anything else is
        // an oversized request body (accumulated images/attachments).
        413 if message.to_ascii_lowercase().contains("context window") => {
            LlmError::ContextOverflow { token_gap: 0 }
        }
        413 => LlmError::RequestTooLarge,
        429 => LlmError::RateLimited {
            retry_after,
            scope: None,
        },
        400 | 422 => LlmError::InvalidRequest { message: display },
        529 => LlmError::Overloaded { repeated: false },
        _ => LlmError::ProviderInternal,
    }
}

#[cfg(test)]
mod tests {
    use super::{api_error_message, map_error_status, LlmError};
    use serde_json::json;

    /// `APIError.makeMessage` (2.1.220 @225796853), all four branches.
    ///
    /// The status prefix is not cosmetic: two oracle sites recover the status by
    /// stripping it (`/^429\s+/`, `/^400\s+/`), and the 429 path then JSON-parses
    /// the remainder — which only works because the WHOLE body is stringified in.
    #[test]
    fn api_error_message_matches_the_sdk() {
        // Anthropic's shape has no top-level `message`, so the whole body is
        // stringified. This is the common case and the one the 429 handler parses.
        let body = json!({"type":"error","error":{"type":"invalid_request_error","message":"bad"}});
        assert_eq!(
            api_error_message(400, &body, ""),
            r#"400 {"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#
        );

        // A top-level string `message` is used verbatim.
        assert_eq!(
            api_error_message(429, &json!({"message":"slow down"}), ""),
            "429 slow down"
        );

        // A non-string `message` is stringified rather than used raw.
        assert_eq!(
            api_error_message(400, &json!({"message":{"code":7}}), ""),
            r#"400 {"code":7}"#
        );

        // No body at all → the sentinel, NOT an empty tail.
        assert_eq!(
            api_error_message(500, &serde_json::Value::Null, ""),
            "500 status code (no body)"
        );

        // No status and no body → the second sentinel.
        assert_eq!(
            api_error_message(0, &serde_json::Value::Null, ""),
            "(no status code or body)"
        );

        // No status but a body → bare message, no prefix.
        assert_eq!(api_error_message(0, &json!({"message":"boom"}), ""), "boom");
    }

    /// JS falsiness, not Rust `Option`-ness: an EMPTY `message` is falsy, so the
    /// oracle falls through to stringifying the whole body.
    #[test]
    fn an_empty_message_field_falls_through_to_the_body() {
        assert_eq!(
            api_error_message(400, &json!({"message":""}), ""),
            r#"400 {"message":""}"#
        );
    }

    /// The fallback argument is used only when there is no body at all.
    #[test]
    fn the_fallback_is_used_only_without_a_body() {
        assert_eq!(
            api_error_message(0, &serde_json::Value::Null, "connection reset"),
            "connection reset"
        );
    }

    #[test]
    fn status_413_splits_on_context_window() {
        // 413 without "context window" → RequestTooLarge (images/attachments).
        assert!(matches!(
            map_error_status(413, "request entity too large", String::new(), None),
            LlmError::RequestTooLarge
        ));
        // 413 mentioning the context window → ContextOverflow (prompt-too-long).
        assert!(matches!(
            map_error_status(
                413,
                "input length exceeds the CONTEXT WINDOW",
                String::new(),
                None
            ),
            LlmError::ContextOverflow { token_gap: 0 }
        ));
    }

    /// The split that makes the whole change safe: classification reads the RAW
    /// message, the variant stores the PREFIXED one. If these were ever fed the
    /// same string, adding the status prefix would silently shift every
    /// downstream `message.contains(...)` classifier.
    #[test]
    fn classification_reads_raw_while_the_variant_stores_the_prefixed_text() {
        // Raw says "context window" → ContextOverflow, even though the display
        // text (which is what a naive implementation would classify on) is a
        // JSON blob that merely happens to contain it too.
        assert!(matches!(
            map_error_status(
                413,
                "exceeds the context window",
                "irrelevant".to_string(),
                None
            ),
            LlmError::ContextOverflow { token_gap: 0 }
        ));
        // Raw does NOT say it → RequestTooLarge, even if the display text does.
        assert!(matches!(
            map_error_status(413, "too big", "413 context window".to_string(), None),
            LlmError::RequestTooLarge
        ));
        // And the stored text is the display one, prefix and all.
        match map_error_status(400, "bad", "400 {\"x\":1}".to_string(), None) {
            LlmError::InvalidRequest { message } => assert_eq!(message, "400 {\"x\":1}"),
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }
}
