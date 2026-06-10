use std::time::Duration;

use llm_client::providers::GeminiCodec;
use llm_client::{AnthropicMessagesCodec, LlmError, OpenAiChatCodec, ProviderResponse, WireCodec};

fn anthropic_codec() -> AnthropicMessagesCodec {
    AnthropicMessagesCodec::new("https://api.anthropic.com", "2023-06-01")
}

fn openai_codec() -> OpenAiChatCodec {
    OpenAiChatCodec::new("https://api.openai.com/v1")
}

fn gemini_codec() -> GeminiCodec {
    GeminiCodec::new("https://generativelanguage.googleapis.com/v1beta")
}

fn anthropic_error(status: u16, error_type: &str, message: &str) -> ProviderResponse {
    ProviderResponse::json(status, serde_json::json!({
        "type": "error",
        "error": {"type": error_type, "message": message}
    }))
}

fn openai_error(status: u16, code: &str, message: &str) -> ProviderResponse {
    ProviderResponse::json(status, serde_json::json!({
        "error": {"message": message, "type": "invalid_request_error", "code": code}
    }))
}

fn gemini_error(status: u16, google_status: &str, message: &str) -> ProviderResponse {
    ProviderResponse::json(status, serde_json::json!({
        "error": {"code": status, "message": message, "status": google_status}
    }))
}

#[test]
fn anthropic_error_envelope_maps_to_taxonomy() {
    let codec = anthropic_codec();

    assert!(matches!(
        codec.decode_response(anthropic_error(401, "authentication_error", "invalid x-api-key")).unwrap_err(),
        LlmError::Authentication
    ));
    assert!(matches!(
        codec.decode_response(anthropic_error(400, "invalid_request_error", "prompt is too long: 250000 tokens")).unwrap_err(),
        LlmError::ContextOverflow
    ));
    assert!(matches!(
        codec.decode_response(anthropic_error(400, "invalid_request_error", "messages: roles must alternate")).unwrap_err(),
        LlmError::InvalidRequest { message } if message.contains("roles must alternate")
    ));
    assert!(matches!(
        codec.decode_response(anthropic_error(529, "overloaded_error", "Overloaded")).unwrap_err(),
        LlmError::ProviderInternal
    ));
}

#[test]
fn anthropic_rate_limit_carries_retry_after_with_ms_precedence() {
    let codec = anthropic_codec();

    let mut seconds_only = anthropic_error(429, "rate_limit_error", "slow down");
    seconds_only.headers.insert("Retry-After".to_string(), "7".to_string());
    assert!(matches!(
        codec.decode_response(seconds_only).unwrap_err(),
        LlmError::RateLimited { retry_after: Some(after), .. } if after == Duration::from_secs(7)
    ));

    let mut with_ms = anthropic_error(429, "rate_limit_error", "slow down");
    with_ms.headers.insert("retry-after".to_string(), "7".to_string());
    with_ms.headers.insert("retry-after-ms".to_string(), "250".to_string());
    assert!(matches!(
        codec.decode_response(with_ms).unwrap_err(),
        LlmError::RateLimited { retry_after: Some(after), .. } if after == Duration::from_millis(250)
    ));
}

#[test]
fn openai_error_envelope_maps_to_taxonomy() {
    let codec = openai_codec();

    assert!(matches!(
        codec.decode_response(openai_error(401, "invalid_api_key", "Incorrect API key provided")).unwrap_err(),
        LlmError::Authentication
    ));
    assert!(matches!(
        codec.decode_response(openai_error(429, "insufficient_quota", "You exceeded your current quota")).unwrap_err(),
        LlmError::QuotaExceeded
    ));
    assert!(matches!(
        codec.decode_response(openai_error(429, "rate_limit_exceeded", "Rate limit reached")).unwrap_err(),
        LlmError::RateLimited { .. }
    ));
    assert!(matches!(
        codec.decode_response(openai_error(400, "context_length_exceeded", "This model's maximum context length is exceeded")).unwrap_err(),
        LlmError::ContextOverflow
    ));
    assert!(matches!(
        codec.decode_response(openai_error(400, "invalid_value", "Invalid value for tool_choice")).unwrap_err(),
        LlmError::InvalidRequest { message } if message.contains("tool_choice")
    ));
    assert!(matches!(
        codec.decode_response(openai_error(500, "server_error", "The server had an error")).unwrap_err(),
        LlmError::ProviderInternal
    ));
}

#[test]
fn gemini_error_envelope_maps_to_taxonomy() {
    let codec = gemini_codec();

    assert!(matches!(
        codec.decode_response(gemini_error(400, "INVALID_ARGUMENT", "Invalid JSON payload")).unwrap_err(),
        LlmError::InvalidRequest { message } if message.contains("Invalid JSON payload")
    ));
    assert!(matches!(
        codec.decode_response(gemini_error(401, "UNAUTHENTICATED", "API key not valid")).unwrap_err(),
        LlmError::Authentication
    ));
    assert!(matches!(
        codec.decode_response(gemini_error(403, "PERMISSION_DENIED", "no access")).unwrap_err(),
        LlmError::PermissionDenied
    ));
    assert!(matches!(
        codec.decode_response(gemini_error(404, "NOT_FOUND", "model not found")).unwrap_err(),
        LlmError::ModelUnavailable
    ));
    assert!(matches!(
        codec.decode_response(gemini_error(429, "RESOURCE_EXHAUSTED", "quota exceeded")).unwrap_err(),
        LlmError::RateLimited { .. }
    ));
    assert!(matches!(
        codec.decode_response(gemini_error(503, "UNAVAILABLE", "service overloaded")).unwrap_err(),
        LlmError::ProviderInternal
    ));
}

#[test]
fn error_status_without_envelope_falls_back_to_status_mapping() {
    assert!(matches!(
        anthropic_codec().decode_response(ProviderResponse::json(401, serde_json::Value::Null)).unwrap_err(),
        LlmError::Authentication
    ));
    assert!(matches!(
        openai_codec().decode_response(ProviderResponse::json(502, serde_json::json!("bad gateway"))).unwrap_err(),
        LlmError::ProviderInternal
    ));
    assert!(matches!(
        gemini_codec().decode_response(ProviderResponse::json(429, serde_json::Value::Null)).unwrap_err(),
        LlmError::RateLimited { .. }
    ));
}
