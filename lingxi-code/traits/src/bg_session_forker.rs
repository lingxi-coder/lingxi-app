//! `BgSessionForker` — the injected seam behind the 2.1.212 `/fork` (`vAd`):
//! copy the CURRENT conversation into a NEW BACKGROUND session while the
//! interactive session stays live.
//!
//! The redefined `/fork` (`vAd = "Copy this conversation into a new background
//! session and keep working here"`) has NO `load` in the binary — upstream's
//! special dispatcher intercepts `name === "fork"` and routes it through the
//! background (`--bg`/daemon) session-copy path. This engine mirrors that split
//! with an injected seam: [`crate::OrchestratorHandle::fork_to_background_session`]
//! reads the live session's history + rendered system prompt and calls this
//! trait; the concrete impl lives in the CLI composition root (which owns the
//! `--bg`/daemon dispatch machinery and mints the new short id), so the leaf
//! `orchestrator` crate never depends on `apps/cli` (correct layering — mirrors
//! the existing `fork_spawner`/`fork_budget` optional-seam on the handle).
//!
//! The impl SNAPSHOTS the passed conversation into the new background session's
//! `<config-home>/projects/<sanitize(cwd)>/<new-uuid>.jsonl` transcript (the
//! same on-disk shape the resume loader reads), then dispatches a detached
//! daemon worker that RESUMES that copy. It returns the system line the LIVE
//! session displays — the composition root owns that text because it alone
//! knows the newly-minted background short id.

use async_trait::async_trait;
use std::sync::Arc;
use thiserror::Error;

/// Failure modes for [`BgSessionForker::fork_to_background`]. Projected to a
/// coarse `HandleError::ActionFailed` by the handle impl (the `/fork` handler
/// renders "Could not fork to background session: …").
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BgForkError {
    /// The conversation snapshot could not be written to the new session's
    /// transcript (I/O / serialization failure).
    #[error("could not snapshot conversation: {0}")]
    Snapshot(String),
    /// The background daemon dispatch (job/roster write or daemon ensure)
    /// failed.
    #[error("could not dispatch background session: {0}")]
    Dispatch(String),
}

/// Copies the current conversation into a new background session.
///
/// Injected into [`crate::OrchestratorHandle`] via
/// `ConversationOrchestrator::with_bg_session_forker`; `None` (tests /
/// non-desktop roots) ⇒ `fork_to_background_session` fails with a clear
/// `ActionFailed` rather than panicking.
#[async_trait]
pub trait BgSessionForker: Send + Sync {
    /// Snapshot `history` (+ the parent's rendered `system_prompt`, when the
    /// live session has completed a turn) into a new background session and
    /// dispatch a detached worker that resumes it. `prompt` is the OPTIONAL
    /// `[prompt]` argument (empty string = no seed turn).
    ///
    /// Returns the system line to display in the LIVE (interactive) session —
    /// the composition root owns the exact text since it mints the new short id.
    async fn fork_to_background(
        &self,
        history: &[protocol::ConversationMessage],
        system_prompt: Option<Arc<str>>,
        prompt: &str,
    ) -> Result<String, BgForkError>;
}
