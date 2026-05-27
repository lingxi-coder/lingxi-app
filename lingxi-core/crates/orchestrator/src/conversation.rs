//! Conversation orchestrator.
//!
//! Drives the v0.6.0 batched turn loop:
//! `append_user → messages_create_non_stream → dispatch_tools → append_assistant → loop_or_end`.

use async_trait::async_trait;
use lingxi_api_client::{types::MessageResponse, ApiError};
use lingxi_protocol::ConversationMessage;

/// Minimal contract the orchestrator needs from the API client.
///
/// Production: `AnthropicProviderAdapter` wraps `AnthropicProvider` + a
/// `HttpTransport` into this shape (added in Task 10).
/// Tests: `crate::test_support::MockApiClient` implements this directly.
#[async_trait]
pub trait OrchestratorApiClient: Send + Sync {
    /// Non-streaming `messages.create`. Returns the full response after
    /// the model finishes generating.
    async fn messages_create(
        &self,
        model: &str,
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError>;
}
