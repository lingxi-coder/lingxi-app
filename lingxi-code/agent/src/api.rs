//! API seam for the subagent runner.
//!
//! [`SubagentApiClient`] is the narrow, object-safe trait the multi-turn
//! [`crate::runner::run_subagent`] loop uses to call the model. It mirrors the
//! orchestrator's `messages_create` shape but lives in the agent crate so the
//! agent crate never takes a dep on the orchestrator (which would form a
//! cycle). Production wiring (M5-Wire) implements this trait on the
//! orchestrator's API client adapter and hands an `Arc<dyn SubagentApiClient>`
//! to [`crate::handle::PoolSubagentSpawner`].
//!
//! The trait is kept deliberately minimal — a single non-streaming
//! round-trip. SSE streaming, retry, and cost wiring all live behind the
//! concrete impl, exactly as the orchestrator's `execute_one_turn` consumes
//! them.

use async_trait::async_trait;

/// Non-streaming `messages.create` seam used by the subagent loop.
///
/// Object-safe: callers hold an `Arc<dyn SubagentApiClient>`. The concrete
/// production impl lives in the orchestrator (the Wire step); test fixtures
/// provide a scripted mock (see [`crate::runner`] tests).
#[async_trait]
pub trait SubagentApiClient: Send + Sync {
    /// Issue one non-streaming model round-trip.
    ///
    /// `system` is the assembled system prompt (stable across the run);
    /// `messages` is the full conversation history, oldest first.
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<protocol::ConversationMessage>,
    ) -> Result<api_client::MessageResponse, api_client::ApiError>;
}
