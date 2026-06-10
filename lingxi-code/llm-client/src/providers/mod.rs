#[allow(missing_docs)]
mod anthropic;
#[allow(missing_docs)]
mod gemini;
#[allow(missing_docs)]
mod openai;

use std::time::Duration;

use crate::LlmError;

pub use anthropic::AnthropicMessagesCodec;
pub use gemini::GeminiCodec;
pub use openai::OpenAiChatCodec;

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
        413 => LlmError::ContextOverflow,
        429 => LlmError::RateLimited {
            retry_after,
            scope: None,
        },
        400 | 422 => LlmError::InvalidRequest { message },
        _ => LlmError::ProviderInternal,
    }
}
