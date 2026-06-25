//! Error taxonomy for the dual-LLM multi-agent run (design doc §Error
//! taxonomy). Errors are a first-class state of the workflow — failures are
//! never inferred from LLM free text.
//!
//! Phase 3 exercises only [`MultiAgentError::Worktree`] (worktree creation is
//! fatal, never degraded to the main workspace) and
//! [`MultiAgentError::ArtifactIo`] (task-brief / state / candidate-log write
//! failure is fatal). The remaining variants are declared now so later phases
//! (candidate runner, review/revision/arbiter, finalizer/verification) extend
//! a single source of truth rather than re-inventing it.

use crate::state::DualLlmPhase;
use std::path::PathBuf;

use crate::config::ConfigError;
use traits::WorktreeError;

/// Failure modes for a dual-LLM multi-agent run.
///
/// Variants beyond `Worktree` / `ArtifactIo` are seams for later phases; they
/// compile and are constructible today.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MultiAgentError {
    /// Configuration could not be parsed (fatal, fail fast).
    #[error("config error: {0}")]
    Config(#[from] ConfigError),

    /// A candidate's model/provider could not be resolved or reached.
    #[error("provider unavailable for candidate {candidate_id} (model {model}): {reason}")]
    ProviderUnavailable {
        /// Stable candidate id from config.
        candidate_id: String,
        /// The configured `profile/model` reference.
        model: String,
        /// Human-readable cause.
        reason: String,
    },

    /// Could not create an isolated worktree. **Fatal** — the run never
    /// degrades to running candidates in the main workspace (that would break
    /// the isolation guarantee). See [`crate::worktrees`].
    #[error("worktree error: {0}")]
    Worktree(#[from] WorktreeError),

    /// A candidate implementation phase failed but the run may continue if the
    /// other candidate is viable.
    #[error("candidate {candidate_id} failed in {phase:?}: {reason}")]
    CandidateFailed {
        /// Stable candidate id.
        candidate_id: String,
        /// Phase in which the failure occurred.
        phase: DualLlmPhase,
        /// Human-readable cause.
        reason: String,
    },

    /// A candidate exceeded its phase timeout.
    #[error("candidate {candidate_id} timed out in {phase:?}")]
    CandidateTimedOut {
        /// Stable candidate id.
        candidate_id: String,
        /// Phase that timed out.
        phase: DualLlmPhase,
    },

    /// A cross-review produced an error (recoverable: review is a quality
    /// enhancement, not a correctness gate).
    #[error("review by {reviewer_id} on {target_id} (round {round}) failed: {reason}")]
    ReviewFailed {
        /// Reviewer candidate id.
        reviewer_id: String,
        /// Reviewed candidate id.
        target_id: String,
        /// Review round (0-based loop counter + 1).
        round: u32,
        /// Human-readable cause.
        reason: String,
    },

    /// A cross-review timed out (recoverable).
    #[error("review by {reviewer_id} on {target_id} (round {round}) timed out")]
    ReviewTimedOut {
        /// Reviewer candidate id.
        reviewer_id: String,
        /// Reviewed candidate id.
        target_id: String,
        /// Review round.
        round: u32,
    },

    /// Arbitration could not produce a structured decision after retry +
    /// deterministic fallback.
    #[error("arbitration failed: {reason}")]
    ArbitrationFailed {
        /// Human-readable cause.
        reason: String,
    },

    /// The finalizer could not apply the winning patch to the main workspace
    /// (fatal — no partial apply).
    #[error("finalizer failed: {reason}")]
    FinalizerFailed {
        /// Human-readable cause.
        reason: String,
    },

    /// Verification ran and produced a definite non-zero exit (fatal evidence).
    #[error("verification failed: {command} (exit {exit_code:?}), log {log_path:?}")]
    VerificationFailed {
        /// The verification command that failed.
        command: String,
        /// Process exit code, if known.
        exit_code: Option<i32>,
        /// Path to the captured log.
        log_path: PathBuf,
    },

    /// Verification timed out — result inconclusive, must NOT claim complete.
    #[error("verification timed out: {command}, log {log_path:?}")]
    VerificationTimedOut {
        /// The verification command.
        command: String,
        /// Path to the captured log.
        log_path: PathBuf,
    },

    /// An artifact write failed. **Fatal** for task-brief / state /
    /// candidate-log writes — losing implementation evidence is not tolerated.
    #[error("artifact I/O error at {path:?}: {reason}")]
    ArtifactIo {
        /// The artifact path that could not be written/read.
        path: PathBuf,
        /// Human-readable cause.
        reason: String,
    },

    /// The user cancelled the run.
    #[error("run cancelled")]
    Cancelled,

    /// The whole-run wall-clock timeout fired.
    #[error("run {run_id} timed out in phase {phase:?}")]
    RunTimedOut {
        /// The run id.
        run_id: String,
        /// Phase in flight when the timeout fired.
        phase: DualLlmPhase,
    },
}

/// Convenience result alias for the crate.
pub type Result<T> = std::result::Result<T, MultiAgentError>;
