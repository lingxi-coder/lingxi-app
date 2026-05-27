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
    /// Non-streaming `messages.create` with optional system prompt.
    ///
    /// `system` is the assembled system prompt (M5-03). `None` is a
    /// no-op (the API call omits the `"system"` key). Callers that
    /// want the assembled LingXi prompt populate it via
    /// `ConversationOrchestrator::build_system_prompt` (private).
    /// Callers with an override populate it from
    /// `OrchestratorConfig::system_prompt_override`.
    async fn messages_create(
        &self,
        model: &str,
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError>;
}

/// Streaming-API surface used by the orchestrator's streaming turn loop.
///
/// Mirrors [`OrchestratorApiClient`] but returns a typed
/// `BoxStream<'static, Result<StreamEvent, ApiError>>` instead of a
/// single `MessageResponse`. The orchestrator owns the stream and drives
/// it to completion (or `message_stop`).
///
/// Production: `AnthropicProviderStreamingAdapter` (added in Task 11)
/// wraps `AnthropicProvider::messages_create_stream` + a transport.
/// Tests: `MockStreamingApiClient` in `test_support_stream.rs`.
#[async_trait]
pub trait StreamingApiClient: Send + Sync {
    /// Open a streaming `messages.create` request. The returned stream
    /// yields wire-decoded `StreamEvent` values until the server emits
    /// `message_stop`. The implementation is responsible for HTTP, SSE
    /// chunk buffering, and JSON-decoding the `data:` lines into typed
    /// `StreamEvent` values.
    async fn stream(
        &self,
        model: &str,
        system: Option<&str>,
        messages: Vec<ConversationMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<
        futures::stream::BoxStream<
            'static,
            Result<lingxi_api_client::types::StreamEvent, ApiError>,
        >,
        ApiError,
    >;
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
    /// CLAUDE.md hierarchy provider (M5-03). The orchestrator calls
    /// `memory.load(&cwd).await` once per `run_turn` to gather the
    /// memory files spliced into the system prompt.
    pub(crate) memory: Arc<dyn crate::prompt::MemoryHierarchyProvider>,
    /// Working directory used as the root for the env + file-tree +
    /// git-status + memory probes inside `build_system_prompt`. M5-12
    /// CLI will plumb `--cwd`; until then, callers pass the platform
    /// caller's cwd here.
    pub(crate) cwd: std::path::PathBuf,
}

impl ConversationOrchestrator {
    /// Construct a new orchestrator with a fresh in-memory session.
    ///
    /// M5-03 added two trailing parameters (`memory`, `cwd`) — callers
    /// constructed before this plan must update their `::new(...)`
    /// invocation. For empty/no-op memory, see `StaticMemoryProvider::empty()`
    /// in `crate::test_support`.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        config: OrchestratorConfig,
        api: Arc<dyn OrchestratorApiClient>,
        tools: Arc<ToolRegistry>,
        hooks: Arc<dyn HookExecutor>,
        perms: Arc<dyn PermissionGate>,
        output: Arc<dyn OutputStream>,
        memory: Arc<dyn crate::prompt::MemoryHierarchyProvider>,
        cwd: std::path::PathBuf,
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
            memory,
            cwd,
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
        // 0. Build the system prompt for THIS turn. Override always wins.
        let system_prompt: Option<String> = match &self.config.system_prompt_override {
            Some(custom) => Some(custom.clone()),
            None => Some(self.build_system_prompt().await),
        };

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

            let step = execute_one_turn(self, system_prompt.as_deref()).await?;
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

    /// Build the per-turn system prompt by gathering cwd / git / file
    /// tree / memory / tool-name context and calling
    /// [`crate::prompt::assemble_system_prompt`]. Bypassed when
    /// `OrchestratorConfig::system_prompt_override` is `Some(_)`.
    async fn build_system_prompt(&self) -> String {
        use crate::prompt::{assemble_system_prompt, file_tree, git_status, SystemPromptContext};

        let cwd = self.cwd.clone();
        let memory_files = self.memory.load(&cwd).await;

        let git = git_status::probe(&cwd);
        let tree = file_tree::probe(&cwd, file_tree::DEFAULT_DEPTH_LIMIT);

        // Tool name extraction: ToolRegistry's `all_names()` is the
        // unfiltered set (builtin + plugin + MCP). M5-03 uses the
        // unfiltered list because the registry's enable-filter requires
        // a `ToolStaticContext` that's only meaningful at dispatch time.
        // tools_block::format sorts alphabetically inside.
        let tool_names: Vec<String> = self.tools.all_names();

        let shell = std::env::var("SHELL")
            .ok()
            .and_then(|s| {
                std::path::Path::new(&s)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "sh".into());

        let ctx = SystemPromptContext {
            cwd,
            platform: std::env::consts::OS.to_string(),
            model: self.config.model.clone(),
            model_marketing_name: None, // M5-12 CLI fills this when known.
            knowledge_cutoff: None,     // M5-12 CLI fills this when known.
            shell,
            os_version: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            git_status: git,
            file_tree: tree,
            memory_files,
            tool_names,
        };
        assemble_system_prompt(&ctx)
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
        system: Option<&str>,
        msgs: Vec<ConversationMessage>,
    ) -> Result<MessageResponse, ApiError> {
        self.provider
            .messages_create_non_stream(model, system, msgs, self.transport.as_ref())
            .await
    }
}
