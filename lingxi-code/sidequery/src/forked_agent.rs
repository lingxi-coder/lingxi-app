//! Forked-agent runner: full agent loop rooted at the parent's cache-safe
//! prompt prefix.
//!
//! Unlike [`crate::side_query::SideQueryClient`] (stateless one-shot), a
//! forked agent runs the complete subagent loop in a borrowed slot from
//! the host's state-machine pool. It serializes its prompt with the same
//! byte layout as the parent (via [`CacheSafeParams`]) so Anthropic's
//! prompt cache hits on the shared prefix.
//!
//! **Architectural note (M1):** `lingxi-sidequery` deliberately does not
//! depend on `lingxi-agent` — the agent crate already depends on
//! `lingxi-memory`, and `lingxi-memory` depends on this crate (for the
//! refactored selector in Task 5). To avoid the dependency cycle, the
//! runner accepts a `SubagentSlotProvider` trait object that the agent
//! crate implements on `StateMachinePool` in a later wiring plan. The
//! field is unused while only the single-turn path is wired.
//!
//! **Single-turn path (this revision):** when a [`SideQueryClient`] is wired
//! via [`ForkedAgentRunner::with_side_query_client`], [`ForkedAgentRunner::run`]
//! performs a real, *single-turn* forked call: it replays the parent's
//! cache-safe prefix (`cache_safe_params.fork_context_messages`) ahead of the
//! fork's own `prompt_messages` so Anthropic's prompt cache hits on the shared
//! prefix, then issues one stateless LLM call through the client and returns
//! the assistant text + usage. No tool loop runs on this path.
//!
//! **Still future work:** the FULL multi-turn, tool-using subagent loop
//! (the §10 `runner.rs` `run_subagent` driver that borrows a slot from
//! [`SubagentSlotProvider`] and iterates tool calls) is not implemented here.
//! When no client is wired, [`ForkedAgentRunner::run`] returns the legacy
//! `"[forked-agent-stub]"` sentinel so existing callers keep building against
//! the final shape.

use crate::cache_safe_params::CacheSafeParams;
use crate::purposes::QuerySource;
use crate::side_query::{SideQueryClient, SideQueryRequest};
use protocol::ConversationMessage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

/// Output cap applied to a single-turn forked call when the request does not
/// override it. Mirrors `MAX_OUTPUT_TOKENS_FOR_SUMMARY` from the compaction
/// path — single-turn forks today are summarization-shaped.
const DEFAULT_FORK_MAX_TOKENS: u32 = 20_000;

/// Pool-shaped abstraction that lets the runner allocate forked slots
/// without depending on `lingxi-agent`. Implemented by
/// `agent::StateMachinePool` in the wiring layer (later plan).
///
/// M1.14 only requires `Send + Sync` so the runner can hold an `Arc`.
pub trait SubagentSlotProvider: Send + Sync {}

/// Coarse purpose tag for a forked agent. Mirrors [`QuerySource`] for the
/// subset of purposes that legitimately fork (full loops), and is carried
/// inside [`ForkedAgentRequest`] for log/telemetry rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ForkPurpose {
    /// §13.6 autocompactor.
    Compaction,
    /// §6.5 post-session memory extraction.
    SessionMemoryExtraction,
    /// §10.7 supervisor.
    Supervisor,
    /// Prompt suggestion.
    PromptSuggestion,
    /// Post-turn summary.
    PostTurnSummary,
    /// Classifier with rationale.
    ClassifierExplainer,
    /// §9 skill execution.
    SkillExecution,
    /// Caller-labelled purpose.
    Custom(String),
}

/// Input to one forked-agent run.
#[derive(Clone)]
pub struct ForkedAgentRequest {
    /// Prompt messages appended after the cache-safe prefix.
    pub prompt_messages: Vec<ConversationMessage>,
    /// Snapshot of the parent's prompt prefix to replay verbatim.
    pub cache_safe_params: CacheSafeParams,
    /// Free-form label used in logs (`compaction`, `supervisor`, ...).
    pub fork_label: String,
    /// COGS tag.
    pub query_source: QuerySource,
    /// Override output cap; `None` lets the engine pick a default.
    pub max_output_tokens: Option<u32>,
}

/// Decoded outcome of one forked-agent run.
#[derive(Debug, Clone)]
pub struct ForkedAgentResult {
    /// Aggregated final assistant text.
    pub final_text: String,
    /// Token / cost usage for COGS attribution.
    pub usage: cost::Usage,
}

/// Forked-agent failure surface.
#[derive(Debug, Clone, Error)]
pub enum ForkError {
    /// Pool refused to allocate a slot (saturated or shutting down).
    #[error("pool error: {0}")]
    Pool(String),
    /// The shared [`crate::CacheSafeParamsSlot`] is empty — the parent has
    /// not completed a turn yet.
    #[error("no cache-safe params available")]
    NoCacheSafeParams,
    /// Internal logic error.
    #[error("internal: {0}")]
    Internal(String),
}

/// Runs forked agents inside slots borrowed from a shared pool.
///
/// The pool reference is intentionally retained even though the single-turn
/// path does not yet allocate slots: it locks in the public surface for when
/// the §10 multi-turn runner graduates, and lets callers plumb the runner from
/// `CompactionOrchestrator::new` today.
///
/// When a [`SideQueryClient`] is wired via [`Self::with_side_query_client`],
/// [`Self::run`] performs a real single-turn forked call (see the module
/// docs); otherwise it returns the legacy stub sentinel.
pub struct ForkedAgentRunner {
    #[allow(dead_code)] // Used once the §10 multi-turn runner is wired in.
    pool: Arc<dyn SubagentSlotProvider>,
    /// Optional single-turn backend: `(client, model)`. `None` until a caller
    /// opts in via [`Self::with_side_query_client`], preserving the stub path.
    side_query: Option<(Arc<dyn SideQueryClient>, String)>,
}

impl ForkedAgentRunner {
    /// Build a runner backed by the given pool. Multiple subsystems
    /// (compaction, supervisor, ...) share the same pool instance.
    ///
    /// The runner starts with no single-turn backend; call
    /// [`Self::with_side_query_client`] to opt into the real single-turn path.
    #[must_use]
    pub fn new(pool: Arc<dyn SubagentSlotProvider>) -> Self {
        Self {
            pool,
            side_query: None,
        }
    }

    /// Wire a real single-turn forked path backed by `client`, calling `model`.
    ///
    /// `model` is supplied at construction (rather than per-request or scraped
    /// from `CacheSafeParams.system_context`) because neither
    /// [`ForkedAgentRequest`] nor [`CacheSafeParams`] carries a first-class
    /// model string today, and the wiring site (engine / autocompactor) owns
    /// the active model — e.g. `AutocompactConfig::summary_model` or
    /// `OrchestratorConfig::model`.
    #[must_use]
    pub fn with_side_query_client(
        mut self,
        client: Arc<dyn SideQueryClient>,
        model: String,
    ) -> Self {
        self.side_query = Some((client, model));
        self
    }

    /// Run one forked agent and return its final assistant text + usage.
    ///
    /// When a single-turn backend is wired (via
    /// [`Self::with_side_query_client`]) this performs a real, single-turn
    /// forked call:
    ///
    /// 1. Replay the parent's cache-safe prefix
    ///    (`cache_safe_params.fork_context_messages`) *first*, then append the
    ///    fork's own `prompt_messages`, so the shared prefix is byte-identical
    ///    to the parent's and Anthropic's prompt cache hits.
    /// 2. Reuse the parent's already-rendered `system_prompt` verbatim and set
    ///    `skip_system_prompt_prefix` so the client does not prepend its own
    ///    engine prefix (which would break the cache hit).
    /// 3. Expose no tools — this is the single-turn path; lowering
    ///    `tool_use_options` into a tool loop is the multi-turn runner's job.
    /// 4. Issue exactly one [`SideQueryClient::query`] and map its response.
    ///
    /// When no backend is wired the runner returns the legacy
    /// `"[forked-agent-stub]"` sentinel, unchanged.
    ///
    /// The FULL multi-turn, tool-using subagent loop (the §10 `run_subagent`
    /// driver that borrows a slot from [`SubagentSlotProvider`]) remains future
    /// work.
    ///
    /// # Errors
    ///
    /// Returns [`ForkError::Internal`] when the wired
    /// [`SideQueryClient::query`] call fails.
    pub async fn run(&self, req: ForkedAgentRequest) -> Result<ForkedAgentResult, ForkError> {
        let Some((client, model)) = &self.side_query else {
            // No single-turn backend wired: preserve the legacy stub.
            return Ok(ForkedAgentResult {
                final_text: "[forked-agent-stub]".into(),
                usage: cost::Usage::default(),
            });
        };

        let cp = &req.cache_safe_params;

        // Replay the cache-safe PREFIX first, then the fork's own prompt, so
        // Anthropic's prompt cache hits on the shared, byte-identical prefix.
        let mut messages = cp.fork_context_messages.clone();
        messages.extend(req.prompt_messages.iter().cloned());

        let request = SideQueryRequest {
            model: model.clone(),
            // Replay the parent's already-rendered system prompt verbatim;
            // `user_context` / `system_context` were inputs the parent used to
            // render it and must NOT be re-applied here (double-rendering would
            // break the byte-identical layout the cache relies on).
            system_prompt: Some(cp.system_prompt.to_string()),
            messages,
            // Single-turn: no tool loop. Lowering `cp.tool_use_options` into a
            // `Vec<Value>` is the multi-turn runner's responsibility.
            tools: Vec::new(),
            tool_choice: None,
            output_format: None,
            max_tokens: req.max_output_tokens.unwrap_or(DEFAULT_FORK_MAX_TOKENS),
            // Host owns retry policy for forked calls.
            max_retries: 0,
            temperature: None,
            thinking_budget: None,
            stop_sequences: Vec::new(),
            query_source: req.query_source.clone(),
            // The prefix is already baked into `system_prompt`; any extra
            // engine-injected prefix would break the cache hit.
            skip_system_prompt_prefix: true,
        };

        let resp = client
            .query(request)
            .await
            .map_err(|e| ForkError::Internal(e.to_string()))?;

        Ok(ForkedAgentResult {
            final_text: resp.text.unwrap_or_default(),
            usage: resp.usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::side_query::{SideQueryError, SideQueryResponse};
    use async_trait::async_trait;
    use cost::Usage;
    use protocol::{ConversationMessage, MessageId};
    use std::sync::Mutex;
    use tool_api::context::ToolUseOptions;

    /// A `SubagentSlotProvider` that is never asked to allocate on the
    /// single-turn path (the runner does not borrow a slot here).
    struct NoopProvider;
    impl SubagentSlotProvider for NoopProvider {}

    /// Mock `SideQueryClient`: records the request it was handed and returns a
    /// canned response so tests can assert both the request mapping and the
    /// response mapping.
    struct MockClient {
        seen: Mutex<Option<SideQueryRequest>>,
        canned_text: String,
        canned_usage: Usage,
    }

    #[async_trait]
    impl SideQueryClient for MockClient {
        async fn query(
            &self,
            request: SideQueryRequest,
        ) -> Result<SideQueryResponse, SideQueryError> {
            *self.seen.lock().unwrap() = Some(request);
            Ok(SideQueryResponse {
                text: Some(self.canned_text.clone()),
                structured: None,
                tool_calls: Vec::new(),
                usage: self.canned_usage,
                stop_reason: Some("end_turn".into()),
            })
        }
    }

    /// Mock `SideQueryClient` that always fails, so the `run` error-mapping
    /// path (`SideQueryError` -> `ForkError::Internal`) can be exercised.
    struct FailingClient {
        message: String,
    }

    #[async_trait]
    impl SideQueryClient for FailingClient {
        async fn query(
            &self,
            _request: SideQueryRequest,
        ) -> Result<SideQueryResponse, SideQueryError> {
            Err(SideQueryError::InvalidResponse(self.message.clone()))
        }
    }

    fn tool_use_options() -> ToolUseOptions {
        ToolUseOptions {
            debug: false,
            verbose: false,
            main_loop_model: "test".into(),
            max_budget_nano_usd: None,
            mcp_clients: vec![],
            is_non_interactive_session: false,
            custom_system_prompt: None,
            append_system_prompt: None,
        }
    }

    fn user_msg(text: &str) -> ConversationMessage {
        ConversationMessage::user(MessageId::new(), text.into())
    }

    fn request_with(
        prefix: Vec<ConversationMessage>,
        prompt: Vec<ConversationMessage>,
        max_output_tokens: Option<u32>,
    ) -> ForkedAgentRequest {
        ForkedAgentRequest {
            prompt_messages: prompt,
            cache_safe_params: CacheSafeParams {
                system_prompt: Arc::from("PARENT SYSTEM PROMPT"),
                user_context: std::collections::HashMap::new(),
                system_context: std::collections::HashMap::new(),
                tool_use_options: tool_use_options(),
                fork_context_messages: prefix,
                generation: 7,
            },
            fork_label: "test-fork".into(),
            query_source: QuerySource::Compaction,
            max_output_tokens,
        }
    }

    #[tokio::test]
    async fn run_without_client_returns_stub() {
        let runner = ForkedAgentRunner::new(Arc::new(NoopProvider));
        let req = request_with(vec![user_msg("prefix")], vec![user_msg("prompt")], None);

        let result = runner.run(req).await.expect("stub run succeeds");

        assert_eq!(result.final_text, "[forked-agent-stub]");
        // `cost::Usage` does not implement `PartialEq`; assert on its fields.
        assert_eq!(result.usage.tokens, Usage::default().tokens);
    }

    #[tokio::test]
    async fn run_with_client_builds_prefix_first_request_and_maps_result() {
        let mut canned_usage = Usage::default();
        canned_usage.tokens.input = 11;
        canned_usage.tokens.output = 22;

        let client = Arc::new(MockClient {
            seen: Mutex::new(None),
            canned_text: "FORKED SUMMARY".into(),
            canned_usage,
        });

        let runner = ForkedAgentRunner::new(Arc::new(NoopProvider))
            .with_side_query_client(client.clone(), "claude-opus-4-6".into());

        let req = request_with(
            vec![user_msg("PREFIX-A"), user_msg("PREFIX-B")],
            vec![user_msg("PROMPT-A")],
            Some(512),
        );

        let result = runner.run(req).await.expect("wired run succeeds");

        // Response mapping: text -> final_text, usage carried through.
        assert_eq!(result.final_text, "FORKED SUMMARY");
        assert_eq!(result.usage.tokens.input, 11);
        assert_eq!(result.usage.tokens.output, 22);

        // Request mapping: assert the exact SideQueryRequest the runner built.
        let sent = client.seen.lock().unwrap().clone().expect("client called once");

        assert_eq!(sent.model, "claude-opus-4-6");
        assert_eq!(sent.system_prompt.as_deref(), Some("PARENT SYSTEM PROMPT"));
        assert_eq!(sent.max_tokens, 512); // honored override
        assert_eq!(sent.max_retries, 0);
        assert!(sent.tools.is_empty(), "single-turn: no tools");
        assert!(sent.tool_choice.is_none());
        assert!(sent.skip_system_prompt_prefix);
        assert_eq!(sent.query_source, QuerySource::Compaction);

        // Prefix-first ordering: cache-safe prefix, THEN the fork's prompt.
        let order: Vec<String> = sent
            .messages
            .iter()
            .map(ConversationMessage::text_content)
            .collect();
        assert_eq!(order, vec!["PREFIX-A", "PREFIX-B", "PROMPT-A"]);
    }

    #[tokio::test]
    async fn run_with_client_maps_query_failure_to_internal() {
        // The wired single-turn path promises `SideQueryError` ->
        // `ForkError::Internal` with the underlying message forwarded.
        let client = Arc::new(FailingClient {
            message: "boom".into(),
        });
        let runner = ForkedAgentRunner::new(Arc::new(NoopProvider))
            .with_side_query_client(client, "m".into());

        let req = request_with(vec![user_msg("prefix")], vec![user_msg("prompt")], None);
        let err = runner.run(req).await.expect_err("failing client surfaces error");

        match err {
            ForkError::Internal(msg) => {
                // `Display` of the source error is forwarded verbatim.
                assert!(msg.contains("boom"), "message not forwarded: {msg}");
            }
            other => panic!("expected ForkError::Internal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_with_client_defaults_max_tokens_when_unset() {
        let client = Arc::new(MockClient {
            seen: Mutex::new(None),
            canned_text: String::new(),
            canned_usage: Usage::default(),
        });
        let runner = ForkedAgentRunner::new(Arc::new(NoopProvider))
            .with_side_query_client(client.clone(), "m".into());

        let req = request_with(vec![], vec![user_msg("only-prompt")], None);
        let result = runner.run(req).await.expect("run succeeds");

        // Empty text maps to an empty String, not a panic.
        assert_eq!(result.final_text, "");

        let sent = client.seen.lock().unwrap().clone().unwrap();
        assert_eq!(sent.max_tokens, DEFAULT_FORK_MAX_TOKENS);
        // No prefix => messages are exactly the fork's prompt.
        let order: Vec<String> = sent
            .messages
            .iter()
            .map(ConversationMessage::text_content)
            .collect();
        assert_eq!(order, vec!["only-prompt"]);
    }
}
