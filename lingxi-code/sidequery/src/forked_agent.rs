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
//! field is unused in the M1.14 stub.
//!
//! M1.14 ships a wired stub: the runner accepts the request and returns a
//! sentinel response so callers (autocompactor, supervisor, etc.) can build
//! against the final shape. The full subagent run loop lands when §10
//! `runner.rs` graduates from its M1 stub.

use crate::cache_safe_params::CacheSafeParams;
use crate::purposes::QuerySource;
use protocol::ConversationMessage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

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
/// The pool reference is intentionally retained in M1.14 even though the
/// stub does not yet allocate slots: it locks in the public surface for
/// when the §10 runner graduates from its stub, and lets callers plumb the
/// runner from `CompactionOrchestrator::new` today.
pub struct ForkedAgentRunner {
    #[allow(dead_code)] // M1.14 stub — used once §10 runner is wired in.
    pool: Arc<dyn SubagentSlotProvider>,
}

impl ForkedAgentRunner {
    /// Build a runner backed by the given pool. Multiple subsystems
    /// (compaction, supervisor, ...) share the same pool instance.
    #[must_use]
    pub fn new(pool: Arc<dyn SubagentSlotProvider>) -> Self {
        Self { pool }
    }

    /// Run one forked agent and aggregate its final assistant text.
    ///
    /// M1.14 ships a stub that proves the wiring; production logic lands
    /// when the full subagent run loop in §10 runner.rs is implemented.
    ///
    /// # Errors
    ///
    /// Returns [`ForkError::Pool`] when slot allocation fails, and
    /// [`ForkError::Internal`] for unexpected stub-time failures.
    #[allow(clippy::unused_async)] // M1.14 stub — production impl awaits on the slot.
    pub async fn run(
        &self,
        #[allow(unused_variables)] req: ForkedAgentRequest,
    ) -> Result<ForkedAgentResult, ForkError> {
        let _ = req;
        Ok(ForkedAgentResult {
            final_text: "[forked-agent-stub]".into(),
            usage: cost::Usage::default(),
        })
    }
}
