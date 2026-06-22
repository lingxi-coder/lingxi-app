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
//! The required method is a single non-streaming round-trip. A streaming
//! variant ([`SubagentApiClient::messages_create_stream`]) layers on top with a
//! default that wraps the non-streaming call, so retry/cost wiring lives behind
//! the concrete impl exactly as the orchestrator's `execute_one_turn` consumes
//! it. The production orchestrator adapter overrides the streaming method to
//! delegate to its real SSE transport.

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use llm_client::{LlmError, LlmEvent, LlmResponse};

/// `messages.create` seam used by the subagent loop.
///
/// Object-safe: callers hold an `Arc<dyn SubagentApiClient>`. The concrete
/// production impl lives in the orchestrator (the Wire step); test fixtures
/// provide a scripted mock (see [`crate::runner`] tests).
#[async_trait]
pub trait SubagentApiClient: Send + Sync {
    /// Issue one non-streaming model round-trip.
    ///
    /// `system` is the assembled system prompt (stable across the run);
    /// `messages` is the full conversation history, oldest first; `tools` is
    /// the wire tool-definition array (`{name, description, input_schema}`)
    /// advertised to the model, from [`crate::context::SubagentContext::tool_schemas`]
    /// (empty = no tools).
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<protocol::ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError>;

    /// Issue one model round-trip over the streaming SSE transport, returning
    /// the wire-decoded [`LlmEvent`] stream (yielding until `message_stop` or
    /// `completed`). The [`crate::runner::run_subagent`] loop drains this
    /// through `crate::accumulator::accumulate_stream` into the same
    /// `LlmResponse` the non-streaming path returns, so the turn loop is
    /// transport-agnostic.
    ///
    /// The default wraps [`SubagentApiClient::messages_create`] in a synthetic,
    /// lossless event sequence — a client that only implements the
    /// non-streaming round-trip still satisfies this seam (the round-trip
    /// reproduces the response exactly). The production orchestrator adapter
    /// overrides this to delegate to its real `StreamingApiClient::stream`.
    async fn messages_create_stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<protocol::ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let resp = self.messages_create(model, system, messages, tools).await?;
        let events = crate::accumulator::response_to_stream_events(resp);
        Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed())
    }

    /// Like [`Self::messages_create_stream`], but FORCES the model to call the
    /// named tool (`tool_choice`) — used to make a subagent emit structured
    /// output by forcing a synthetic `StructuredOutput` tool. The default
    /// implementation ignores `forced_tool` (no forcing), so existing impls and
    /// the non-schema path are unchanged; the production provider adapter
    /// overrides it to thread `tool_choice` into the request.
    async fn messages_create_stream_forced(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<protocol::ConversationMessage>,
        tools: Vec<serde_json::Value>,
        forced_tool: Option<&str>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let _ = forced_tool;
        self.messages_create_stream(model, system, messages, tools)
            .await
    }
}
