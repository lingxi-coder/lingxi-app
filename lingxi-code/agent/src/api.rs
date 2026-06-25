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
    /// `effort` is the per-request thinking-effort hint (claude-code
    /// `output_config.effort`): a level string or integer budget, or `None`.
    /// The default (synthetic, non-streaming) path ignores it — only the
    /// production provider adapter threads it onto the request + emits the
    /// `effort-2025-11-24` beta.
    async fn messages_create_stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<protocol::ConversationMessage>,
        tools: Vec<serde_json::Value>,
        effort: Option<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let _ = effort;
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
        effort: Option<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let _ = forced_tool;
        self.messages_create_stream(model, system, messages, tools, effort)
            .await
    }

    /// Like [`Self::messages_create`], but threads a provider `profile` so the
    /// per-spawn route (e.g. a dual-LLM candidate's resolved provider) reaches
    /// the underlying multi-provider client. The DEFAULT body ignores `profile`
    /// and delegates to [`Self::messages_create`] — so the ~5 existing impls and
    /// test mocks that only implement the profile-less method keep their legacy
    /// (default-provider) behavior unchanged (frozen-trait rule). The production
    /// orchestrator adapter OVERRIDES this to forward `profile` to
    /// `DefaultLlmClient::messages_create(model, profile, …)`.
    async fn messages_create_in(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        messages: Vec<protocol::ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, LlmError> {
        let _ = profile;
        self.messages_create(model, system, messages, tools).await
    }

    /// Streaming analog of [`Self::messages_create_in`] — threads the provider
    /// `profile` onto the SSE round-trip. The DEFAULT body ignores `profile` and
    /// delegates to [`Self::messages_create_stream`] (which itself defaults to a
    /// synthetic stream over [`Self::messages_create`]), so non-streaming /
    /// profile-less impls and mocks are unaffected. The production orchestrator
    /// adapter OVERRIDES this to forward `profile` to its real SSE transport.
    async fn messages_create_stream_in(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        messages: Vec<protocol::ConversationMessage>,
        tools: Vec<serde_json::Value>,
        effort: Option<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let _ = profile;
        self.messages_create_stream(model, system, messages, tools, effort)
            .await
    }

    /// Like [`Self::messages_create_stream_in`], but FORCES the named tool
    /// (`tool_choice`). The DEFAULT delegates to
    /// [`Self::messages_create_stream_forced`] (ignoring `profile`); the
    /// production adapter overrides it to thread BOTH `profile` and the forced
    /// tool. Keeps the structured-output (schema) subagent path provider-routed.
    async fn messages_create_stream_forced_in(
        &self,
        model: &str,
        profile: Option<&str>,
        system: Option<&str>,
        messages: Vec<protocol::ConversationMessage>,
        tools: Vec<serde_json::Value>,
        forced_tool: Option<&str>,
        effort: Option<serde_json::Value>,
    ) -> Result<BoxStream<'static, Result<LlmEvent, LlmError>>, LlmError> {
        let _ = profile;
        self.messages_create_stream_forced(model, system, messages, tools, forced_tool, effort)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A mock that implements ONLY the required `messages_create`. It records
    /// the call so we can prove the DEFAULTED `messages_create_in` routes back
    /// through it (ignoring the profile) without the impl knowing about the new
    /// method — the frozen-trait back-compat guarantee.
    struct LegacyOnlyClient {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl SubagentApiClient for LegacyOnlyClient {
        async fn messages_create(
            &self,
            _model: &str,
            _system: Option<&str>,
            _messages: Vec<protocol::ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<LlmResponse, LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(LlmResponse {
                id: "mock".into(),
                model: "mock".into(),
                content: Vec::new(),
                stop_reason: Some("end_turn".into()),
                stop_details: None,
                usage: llm_client::Usage::default(),
                cost: None,
                provider_metadata: serde_json::Value::Null,
            })
        }
    }

    #[tokio::test]
    async fn messages_create_in_default_routes_through_legacy_method() {
        let client = Arc::new(LegacyOnlyClient {
            calls: AtomicUsize::new(0),
        });
        // Calling the NEW profile-aware method on a mock that only implements
        // the legacy one must transparently fall through to `messages_create`.
        let resp = client
            .messages_create_in("some-model", Some("a-profile"), None, Vec::new(), Vec::new())
            .await;
        assert!(resp.is_ok(), "default messages_create_in delegates cleanly");
        assert_eq!(
            client.calls.load(Ordering::SeqCst),
            1,
            "the defaulted messages_create_in must route through the legacy messages_create"
        );
    }
}
