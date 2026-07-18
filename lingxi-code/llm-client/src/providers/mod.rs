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

/// HTTP-status fallback used when a provider error envelope is missing or
/// carries an unrecognized code.
pub(crate) fn map_error_status(
    status: u16,
    message: String,
    retry_after: Option<Duration>,
) -> LlmError {
    match status {
        401 => LlmError::Authentication,
        403 => LlmError::PermissionDenied,
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
        400 | 422 => LlmError::InvalidRequest { message },
        529 => LlmError::Overloaded { repeated: false },
        _ => LlmError::ProviderInternal,
    }
}

#[cfg(test)]
mod tests {
    use super::{map_error_status, LlmError};

    #[test]
    fn status_413_splits_on_context_window() {
        // 413 without "context window" → RequestTooLarge (images/attachments).
        assert!(matches!(
            map_error_status(413, "request entity too large".to_string(), None),
            LlmError::RequestTooLarge
        ));
        // 413 mentioning the context window → ContextOverflow (prompt-too-long).
        assert!(matches!(
            map_error_status(
                413,
                "input length exceeds the CONTEXT WINDOW".to_string(),
                None
            ),
            LlmError::ContextOverflow { token_gap: 0 }
        ));
    }
}
