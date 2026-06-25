//! Execution state machine + run-artifact path helpers (design doc
//! §执行状态机). Artifacts live under
//! `<repo>/.lingxi/multi-agent/runs/<run_id>/` — a LingXi-only orchestration
//! record, NOT claude-code parity state.

use std::path::Path;
use std::path::PathBuf;

/// Phases of a dual-LLM competitive run, in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DualLlmPhase {
    /// Build the shared task brief.
    BuildTaskBrief,
    /// Create the two candidate worktrees (serial).
    CreateWorktrees,
    /// Run both candidate implementations (parallel).
    ImplementCandidates,
    /// Collect each candidate's self-report.
    CollectSelfReports,
    /// Candidates review each other's diff (document-only).
    CrossReview,
    /// Each author revises its own branch from the review.
    AuthorRevision,
    /// Arbiter selects a winner (or rejects both).
    Arbitration,
    /// Finalizer applies the winning patch to the main workspace.
    ApplyFinalPatch,
    /// Run verification against the final workspace.
    Verification,
    /// Best-effort worktree cleanup.
    Cleanup,
    /// Terminal success.
    Complete,
    /// Terminal failure.
    Failed,
}

impl DualLlmPhase {
    /// Short, stable, snake_case identifier for telemetry / `state.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            DualLlmPhase::BuildTaskBrief => "build_task_brief",
            DualLlmPhase::CreateWorktrees => "create_worktrees",
            DualLlmPhase::ImplementCandidates => "implement_candidates",
            DualLlmPhase::CollectSelfReports => "collect_self_reports",
            DualLlmPhase::CrossReview => "cross_review",
            DualLlmPhase::AuthorRevision => "author_revision",
            DualLlmPhase::Arbitration => "arbitration",
            DualLlmPhase::ApplyFinalPatch => "apply_final_patch",
            DualLlmPhase::Verification => "verification",
            DualLlmPhase::Cleanup => "cleanup",
            DualLlmPhase::Complete => "complete",
            DualLlmPhase::Failed => "failed",
        }
    }

    /// Whether this is a terminal phase.
    pub const fn is_terminal(self) -> bool {
        matches!(self, DualLlmPhase::Complete | DualLlmPhase::Failed)
    }
}

/// Path helpers for a single run's artifact tree.
///
/// Layout (design doc §执行状态机):
/// ```text
/// .lingxi/multi-agent/runs/<run_id>/
///   task-brief.md
///   state.json
///   <candidate_id>/
///     self-report.md
///     patch.diff
///     verification.log
///   reviews/
///     <reviewer>-on-<target>-round-<n>.md
///   arbitration.md
///   final.patch
///   final-verification.log
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArtifacts {
    root: PathBuf,
}

impl RunArtifacts {
    /// The repo-relative base directory for all multi-agent runs.
    pub const BASE: &'static str = ".lingxi/multi-agent/runs";

    /// Build the artifact helper for `run_id` under `repo_root`.
    pub fn new(repo_root: impl AsRef<Path>, run_id: &str) -> Self {
        let root = repo_root.as_ref().join(Self::BASE).join(run_id);
        Self { root }
    }

    /// The run's root artifact directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `task-brief.md`.
    pub fn task_brief(&self) -> PathBuf {
        self.root.join("task-brief.md")
    }

    /// `state.json`.
    pub fn state_json(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// A candidate's subdirectory (`<candidate_id>/`).
    pub fn candidate_dir(&self, candidate_id: &str) -> PathBuf {
        self.root.join(candidate_id)
    }

    /// A candidate's `self-report.md`.
    pub fn candidate_self_report(&self, candidate_id: &str) -> PathBuf {
        self.candidate_dir(candidate_id).join("self-report.md")
    }

    /// A candidate's `patch.diff`.
    pub fn candidate_patch(&self, candidate_id: &str) -> PathBuf {
        self.candidate_dir(candidate_id).join("patch.diff")
    }

    /// A candidate's `verification.log`.
    pub fn candidate_verification_log(&self, candidate_id: &str) -> PathBuf {
        self.candidate_dir(candidate_id).join("verification.log")
    }

    /// The `reviews/` directory.
    pub fn reviews_dir(&self) -> PathBuf {
        self.root.join("reviews")
    }

    /// A cross-review document: `reviews/<reviewer>-on-<target>-round-<round>.md`.
    pub fn review_doc(&self, reviewer_id: &str, target_id: &str, round: u32) -> PathBuf {
        self.reviews_dir()
            .join(format!("{reviewer_id}-on-{target_id}-round-{round}.md"))
    }

    /// A failed cross-review marker: `reviews/<reviewer>-on-<target>-round-<round>.error.md`.
    pub fn review_error_doc(&self, reviewer_id: &str, target_id: &str, round: u32) -> PathBuf {
        self.reviews_dir()
            .join(format!("{reviewer_id}-on-{target_id}-round-{round}.error.md"))
    }

    /// `arbitration.md`.
    pub fn arbitration(&self) -> PathBuf {
        self.root.join("arbitration.md")
    }

    /// `final.patch`.
    pub fn final_patch(&self) -> PathBuf {
        self.root.join("final.patch")
    }

    /// `final-verification.log`.
    pub fn final_verification_log(&self) -> PathBuf {
        self.root.join("final-verification.log")
    }

    /// `finalizer-error.md` (written when patch application fails).
    pub fn finalizer_error(&self) -> PathBuf {
        self.root.join("finalizer-error.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_ordering_and_strings() {
        assert!(DualLlmPhase::BuildTaskBrief < DualLlmPhase::CreateWorktrees);
        assert!(DualLlmPhase::Arbitration < DualLlmPhase::ApplyFinalPatch);
        assert_eq!(DualLlmPhase::CrossReview.as_str(), "cross_review");
        assert!(DualLlmPhase::Complete.is_terminal());
        assert!(DualLlmPhase::Failed.is_terminal());
        assert!(!DualLlmPhase::Verification.is_terminal());
    }

    #[test]
    fn artifact_paths_under_lingxi_runs() {
        let a = RunArtifacts::new("/repo", "01HX9ABCD12");
        assert_eq!(
            a.root(),
            Path::new("/repo/.lingxi/multi-agent/runs/01HX9ABCD12")
        );
        assert!(a.task_brief().ends_with("task-brief.md"));
        assert!(a.state_json().ends_with("state.json"));
        assert_eq!(
            a.candidate_self_report("candidate-a"),
            Path::new("/repo/.lingxi/multi-agent/runs/01HX9ABCD12/candidate-a/self-report.md")
        );
        assert_eq!(
            a.candidate_patch("candidate-b"),
            Path::new("/repo/.lingxi/multi-agent/runs/01HX9ABCD12/candidate-b/patch.diff")
        );
    }

    #[test]
    fn review_doc_naming() {
        let a = RunArtifacts::new("/repo", "RUN");
        assert_eq!(
            a.review_doc("candidate-a", "candidate-b", 1),
            Path::new("/repo/.lingxi/multi-agent/runs/RUN/reviews/candidate-a-on-candidate-b-round-1.md")
        );
        assert!(a
            .review_error_doc("candidate-b", "candidate-a", 2)
            .ends_with("candidate-b-on-candidate-a-round-2.error.md"));
    }

    #[test]
    fn final_artifact_paths() {
        let a = RunArtifacts::new("/repo", "RUN");
        assert!(a.arbitration().ends_with("arbitration.md"));
        assert!(a.final_patch().ends_with("final.patch"));
        assert!(a.final_verification_log().ends_with("final-verification.log"));
        assert!(a.finalizer_error().ends_with("finalizer-error.md"));
    }
}
