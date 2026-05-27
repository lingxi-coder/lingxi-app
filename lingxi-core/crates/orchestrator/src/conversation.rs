//! Conversation orchestrator.
//!
//! Drives the v0.6.0 batched turn loop. See module-level docs in `lib.rs`.

use crate::config::OrchestratorConfig;
use crate::error::OrchestratorError;
use crate::test_support::{HookExecutor, PermissionGate};
use crate::turn_loop::{execute_one_turn, TurnStepOutcome};
use async_trait::async_trait;
use lingxi_api_client::{types::MessageResponse, AnthropicProvider, ApiError};
use lingxi_core::SessionState;
use lingxi_protocol::{ConversationMessage, MessageId, SessionId};
use lingxi_telemetry::tengu::orchestrator as orch_events;
use lingxi_tools::registry::ToolRegistry;
use lingxi_traits::{HttpTransport, OutputStream};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Minimal contract the orchestrator needs from the API client.
///
/// Production: `AnthropicProviderAdapter` wraps `AnthropicProvider` +
/// `HttpTransport` into this shape. Tests: `MockApiClient`.
#[async_trait]
pub trait OrchestratorApiClient: Send + Sync {
    /// Non-streaming `messages.create`.
    async fn messages_create(
        &self,
        model: &str,
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError>;
}

/// Result of `ConversationOrchestrator::run_turn` on success.
///
/// Only one variant in M5-02; M5-04 may add `Cancelled { ... }` later.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ConversationOutcome {
    /// Model emitted `stop_reason == "end_turn"` after `turn_count` API
    /// calls. `final_message_id` is the id of the final assistant message
    /// appended to the session.
    EndTurn {
        /// Number of API round-trips it took to reach `end_turn`.
        turn_count: u32,
        /// Stable identifier of the final assistant message.
        final_message_id: MessageId,
    },
}

/// The orchestrator. Owns the session, dispatches tools, drives the loop.
///
/// Construction is via `new(...)`. Driven via `run_turn(prompt)`.
pub struct ConversationOrchestrator {
    pub(crate) config: OrchestratorConfig,
    pub(crate) api: Arc<dyn OrchestratorApiClient>,
    pub(crate) tools: Arc<ToolRegistry>,
    pub(crate) hooks: Arc<dyn HookExecutor>,
    pub(crate) perms: Arc<dyn PermissionGate>,
    pub(crate) output: Arc<dyn OutputStream>,
    pub(crate) session: Arc<Mutex<SessionState>>,
}

impl ConversationOrchestrator {
    /// Construct a new orchestrator with a fresh in-memory session.
    #[must_use]
    pub fn new(
        config: OrchestratorConfig,
        api: Arc<dyn OrchestratorApiClient>,
        tools: Arc<ToolRegistry>,
        hooks: Arc<dyn HookExecutor>,
        perms: Arc<dyn PermissionGate>,
        output: Arc<dyn OutputStream>,
    ) -> Self {
        let session = SessionState::empty(SessionId::new(), config.model.clone());
        Self {
            config,
            api,
            tools,
            hooks,
            perms,
            output,
            session: Arc::new(Mutex::new(session)),
        }
    }

    /// Drive one user prompt through the turn loop until `end_turn` or
    /// `max_turns` is exhausted.
    ///
    /// Emits 3 telemetry events:
    /// - [`orch_events::CONVERSATION_STARTED`] at entry
    /// - [`orch_events::CONVERSATION_COMPLETED`] on success
    /// - [`orch_events::CONVERSATION_FAILED`] on error
    pub async fn run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError> {
        tracing::info!(
            event = orch_events::CONVERSATION_STARTED,
            prompt_len = prompt.len()
        );
        let result = self.try_run_turn(prompt).await;
        // ConversationOutcome is #[non_exhaustive] so future variants will
        // also log as Completed when the only existing variant is EndTurn.
        match &result {
            Ok(ConversationOutcome::EndTurn { turn_count, .. }) => {
                tracing::info!(
                    event = orch_events::CONVERSATION_COMPLETED,
                    turn_count = *turn_count
                );
            }
            Err(err) => {
                tracing::error!(
                    event = orch_events::CONVERSATION_FAILED,
                    reason = %err
                );
            }
        }
        result
    }

    /// Internal turn driver (no telemetry — wrapped by `run_turn`).
    async fn try_run_turn(&self, prompt: &str) -> Result<ConversationOutcome, OrchestratorError> {
        // 1. Append the user prompt to session history.
        {
            let mut s = self.session.lock().await;
            let msg = ConversationMessage::user(MessageId::new(), prompt.to_string());
            s.history.push(msg);
        }

        // 2. Turn-by-turn driver.
        let mut turn_count: u32 = 0;
        let final_message_id;
        loop {
            if turn_count >= self.config.max_turns {
                return Err(OrchestratorError::MaxTurnsReached {
                    max_turns: self.config.max_turns,
                });
            }
            turn_count = turn_count.saturating_add(1);

            let step = execute_one_turn(self).await?;
            match step {
                TurnStepOutcome::Continue => continue,
                TurnStepOutcome::Ended {
                    final_message_id: id,
                    stop_reason,
                } => {
                    let cost = {
                        let s = self.session.lock().await;
                        crate::turn_loop::cost_snapshot_from_session(&s)
                    };
                    self.output.emit_end_turn(&stop_reason, &cost).await;
                    final_message_id = id;
                    break;
                }
            }
        }

        Ok(ConversationOutcome::EndTurn {
            turn_count,
            final_message_id,
        })
    }

    /// Borrow the in-memory session (read-write lock surrogate). Useful for tests.
    #[must_use]
    pub fn session(&self) -> Arc<Mutex<SessionState>> {
        self.session.clone()
    }
}

/// Production adapter: wraps `AnthropicProvider` + an `HttpTransport` into
/// the `OrchestratorApiClient` shape.
///
/// Concrete type so callers can construct without knowing the transport
/// type parameter (the constructor takes `Arc<dyn OrchestratorApiClient>`).
pub struct AnthropicProviderAdapter<T: HttpTransport + Send + Sync + 'static> {
    provider: AnthropicProvider,
    transport: Arc<T>,
}

impl<T: HttpTransport + Send + Sync + 'static> AnthropicProviderAdapter<T> {
    /// Construct from an existing provider + transport.
    #[must_use]
    pub fn new(provider: AnthropicProvider, transport: Arc<T>) -> Self {
        Self {
            provider,
            transport,
        }
    }
}

#[async_trait]
impl<T: HttpTransport + Send + Sync + 'static> OrchestratorApiClient
    for AnthropicProviderAdapter<T>
{
    async fn messages_create(
        &self,
        model: &str,
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError> {
        self.provider
            .messages_create_non_stream(model, msgs, self.transport.as_ref())
            .await
    }
}
