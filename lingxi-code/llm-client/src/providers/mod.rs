#[allow(missing_docs)]
mod anthropic;
#[allow(missing_docs)]
mod azure_openai;
#[allow(missing_docs)]
pub mod bedrock_claude;
#[allow(missing_docs)]
mod gemini;
#[allow(missing_docs)]
mod openai;

use std::time::Duration;

use crate::{LlmError, StreamDecoder, WireCodec};

pub use anthropic::AnthropicMessagesCodec;
pub use azure_openai::AzureOpenAiCodec;
pub use bedrock_claude::BedrockClaudeCodec;
pub use gemini::GeminiCodec;
pub use openai::OpenAiChatCodec;

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
        413 => LlmError::ContextOverflow { token_gap: 0 },
        429 => LlmError::RateLimited {
            retry_after,
            scope: None,
        },
        400 | 422 => LlmError::InvalidRequest { message },
        529 => LlmError::Overloaded { repeated: false },
        _ => LlmError::ProviderInternal,
    }
}
