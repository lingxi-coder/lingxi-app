//! Run-artifact I/O (design doc §执行状态机, Phase 3).
//!
//! Reads/writes the on-disk record for a single dual-LLM run under
//! `<repo>/.lingxi/multi-agent/runs/<run_id>/`. The path layout itself lives
//! in [`crate::state::RunArtifacts`]; this module performs the filesystem
//! operations and classifies failures.
//!
//! Failure policy (design doc §Artifact write failures):
//!
//! - task-brief / `state.json` / candidate-log write failure → **fatal**
//!   ([`MultiAgentError::ArtifactIo`]). Losing implementation evidence is not
//!   tolerated.
//! - review-document write failure → fatal for the review phase, but the run
//!   may skip review and continue to arbitration (caller's policy; this layer
//!   simply surfaces the error).
//!
//! Cleanup is **fail-closed** (design doc §安全约束 #6): if the worktree dirty
//! summary cannot be determined, or the worktree carries un-merged work, the
//! worktree is retained and its path reported rather than removed.

use crate::error::MultiAgentError;
use crate::state::RunArtifacts;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use traits::WorktreeHandle;
use traits::WorktreeManager;

/// Filesystem store for a single run's artifacts.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    paths: RunArtifacts,
}

impl ArtifactStore {
    /// Create a store rooted at `<repo_root>/.lingxi/multi-agent/runs/<run_id>`.
    #[must_use]
    pub fn new(repo_root: impl AsRef<Path>, run_id: &str) -> Self {
        Self {
            paths: RunArtifacts::new(repo_root, run_id),
        }
    }

    /// Access the underlying path helper.
    #[must_use]
    pub fn paths(&self) -> &RunArtifacts {
        &self.paths
    }

    /// Create the run root directory (and any candidate/review subdirectories
    /// lazily on first write). Fatal on failure.
    pub fn init(&self) -> Result<(), MultiAgentError> {
        create_dir_all(self.paths.root())
    }

    /// Write `task-brief.md`. **Fatal** on failure.
    pub fn write_task_brief(&self, contents: &str) -> Result<(), MultiAgentError> {
        write_fatal(&self.paths.task_brief(), contents)
    }

    /// Read `task-brief.md`.
    pub fn read_task_brief(&self) -> Result<String, MultiAgentError> {
        read_fatal(&self.paths.task_brief())
    }

    /// Write `state.json` from a serializable state value. **Fatal** on
    /// serialization or write failure.
    pub fn write_state<S: serde::Serialize>(&self, state: &S) -> Result<(), MultiAgentError> {
        let path = self.paths.state_json();
        let json = serde_json::to_string_pretty(state).map_err(|e| MultiAgentError::ArtifactIo {
            path: path.clone(),
            reason: format!("serialize state.json: {e}"),
        })?;
        write_fatal(&path, &json)
    }

    /// Read and deserialize `state.json`.
    pub fn read_state<D: serde::de::DeserializeOwned>(&self) -> Result<D, MultiAgentError> {
        let path = self.paths.state_json();
        let raw = read_fatal(&path)?;
        serde_json::from_str(&raw).map_err(|e| MultiAgentError::ArtifactIo {
            path,
            reason: format!("parse state.json: {e}"),
        })
    }

    /// Write a candidate's `self-report.md`. **Fatal** on failure (candidate
    /// log evidence must not be lost).
    pub fn write_candidate_self_report(
        &self,
        candidate_id: &str,
        contents: &str,
    ) -> Result<(), MultiAgentError> {
        self.ensure_candidate_dir(candidate_id)?;
        write_fatal(&self.paths.candidate_self_report(candidate_id), contents)
    }

    /// Write a candidate's `patch.diff`. **Fatal** on failure.
    pub fn write_candidate_patch(
        &self,
        candidate_id: &str,
        diff: &str,
    ) -> Result<(), MultiAgentError> {
        self.ensure_candidate_dir(candidate_id)?;
        write_fatal(&self.paths.candidate_patch(candidate_id), diff)
    }

    /// Write a candidate's `verification.log`. **Fatal** on failure (candidate
    /// log evidence must not be lost).
    pub fn write_candidate_verification_log(
        &self,
        candidate_id: &str,
        log: &str,
    ) -> Result<(), MultiAgentError> {
        self.ensure_candidate_dir(candidate_id)?;
        write_fatal(&self.paths.candidate_verification_log(candidate_id), log)
    }

    /// Write a cross-review document.
    pub fn write_review(
        &self,
        reviewer_id: &str,
        target_id: &str,
        round: u32,
        contents: &str,
    ) -> Result<(), MultiAgentError> {
        create_dir_all(&self.paths.reviews_dir())?;
        write_fatal(&self.paths.review_doc(reviewer_id, target_id, round), contents)
    }

    /// Write a failed-cross-review marker (`*.error.md`).
    pub fn write_review_error(
        &self,
        reviewer_id: &str,
        target_id: &str,
        round: u32,
        contents: &str,
    ) -> Result<(), MultiAgentError> {
        create_dir_all(&self.paths.reviews_dir())?;
        write_fatal(
            &self.paths.review_error_doc(reviewer_id, target_id, round),
            contents,
        )
    }

    /// Write `arbitration.md`.
    pub fn write_arbitration(&self, contents: &str) -> Result<(), MultiAgentError> {
        write_fatal(&self.paths.arbitration(), contents)
    }

    /// Write `final.patch`.
    pub fn write_final_patch(&self, diff: &str) -> Result<(), MultiAgentError> {
        write_fatal(&self.paths.final_patch(), diff)
    }

    /// Write `final-verification.log`.
    pub fn write_final_verification_log(&self, log: &str) -> Result<(), MultiAgentError> {
        write_fatal(&self.paths.final_verification_log(), log)
    }

    /// Write `finalizer-error.md`.
    pub fn write_finalizer_error(&self, contents: &str) -> Result<(), MultiAgentError> {
        write_fatal(&self.paths.finalizer_error(), contents)
    }

    fn ensure_candidate_dir(&self, candidate_id: &str) -> Result<(), MultiAgentError> {
        create_dir_all(&self.paths.candidate_dir(candidate_id))
    }
}

/// Outcome of a fail-closed cleanup attempt for a single worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// The worktree was confirmed clean and removed.
    Removed {
        /// The path that was removed.
        path: PathBuf,
    },
    /// The worktree was retained. Retention is the safe default whenever the
    /// dirty state is unknown, the worktree carries un-merged work, or removal
    /// itself failed.
    Retained {
        /// The retained worktree path (report this to the user).
        path: PathBuf,
        /// Why it was kept.
        reason: RetainReason,
    },
}

/// Why a worktree was retained during cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetainReason {
    /// Dirty state could not be determined → fail-closed, assume unsafe.
    DirtyStateUnknown,
    /// The worktree has uncommitted files and/or ahead-commits.
    HasUnmergedWork,
    /// The `remove_worktree` call itself failed.
    RemovalFailed(String),
}

/// Attempt to remove `handle`'s worktree **fail-closed** (design doc §安全约束
/// #6): only remove when the dirty summary is *known* and *clean*. Any
/// uncertainty (summary unavailable, dirty work present, or removal error)
/// retains the worktree and reports its path. This never errors — it returns a
/// [`CleanupOutcome`] the caller surfaces to the user.
pub async fn cleanup_worktree_fail_closed(
    manager: &dyn WorktreeManager,
    handle: &WorktreeHandle,
) -> CleanupOutcome {
    let summary = manager.worktree_change_summary(handle).await;
    match summary {
        Ok(Some(s)) if s.is_dirty() => CleanupOutcome::Retained {
            path: handle.path.clone(),
            reason: RetainReason::HasUnmergedWork,
        },
        Ok(Some(_clean)) => match manager.remove_worktree(handle).await {
            Ok(()) => CleanupOutcome::Removed {
                path: handle.path.clone(),
            },
            Err(e) => CleanupOutcome::Retained {
                path: handle.path.clone(),
                reason: RetainReason::RemovalFailed(e.to_string()),
            },
        },
        // `Ok(None)` (unknown) or any error querying state → fail-closed.
        _ => CleanupOutcome::Retained {
            path: handle.path.clone(),
            reason: RetainReason::DirtyStateUnknown,
        },
    }
}

fn create_dir_all(dir: &Path) -> Result<(), MultiAgentError> {
    fs::create_dir_all(dir).map_err(|e| MultiAgentError::ArtifactIo {
        path: dir.to_path_buf(),
        reason: format!("create dir: {e}"),
    })
}

fn write_fatal(path: &Path, contents: &str) -> Result<(), MultiAgentError> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    fs::write(path, contents).map_err(|e| MultiAgentError::ArtifactIo {
        path: path.to_path_buf(),
        reason: format!("write: {e}"),
    })
}

fn read_fatal(path: &Path) -> Result<String, MultiAgentError> {
    fs::read_to_string(path).map_err(|e| MultiAgentError::ArtifactIo {
        path: path.to_path_buf(),
        reason: format!("read: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::time::Duration;
    use traits::worktree::WorktreeChangeSummary;
    use traits::WorktreeError;
    use traits::WorktreeInfo;

    fn store(root: &Path) -> ArtifactStore {
        ArtifactStore::new(root, "01HX9ABCD12")
    }

    #[test]
    fn layout_matches_design_doc() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.init().unwrap();

        s.write_task_brief("brief").unwrap();
        s.write_state(&serde_json::json!({"phase": "implement_candidates"}))
            .unwrap();
        s.write_candidate_self_report("candidate-a", "report-a").unwrap();
        s.write_candidate_patch("candidate-a", "diff-a").unwrap();
        s.write_candidate_verification_log("candidate-a", "log-a").unwrap();
        s.write_candidate_self_report("candidate-b", "report-b").unwrap();
        s.write_review("candidate-a", "candidate-b", 1, "review-ab")
            .unwrap();
        s.write_arbitration("arb").unwrap();
        s.write_final_patch("final-diff").unwrap();
        s.write_final_verification_log("final-log").unwrap();

        let base = tmp.path().join(".lingxi/multi-agent/runs/01HX9ABCD12");
        assert!(base.join("task-brief.md").is_file());
        assert!(base.join("state.json").is_file());
        assert!(base.join("candidate-a/self-report.md").is_file());
        assert!(base.join("candidate-a/patch.diff").is_file());
        assert!(base.join("candidate-a/verification.log").is_file());
        assert!(base.join("candidate-b/self-report.md").is_file());
        assert!(base
            .join("reviews/candidate-a-on-candidate-b-round-1.md")
            .is_file());
        assert!(base.join("arbitration.md").is_file());
        assert!(base.join("final.patch").is_file());
        assert!(base.join("final-verification.log").is_file());
    }

    #[test]
    fn roundtrip_task_brief_and_state() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        s.write_task_brief("hello brief").unwrap();
        assert_eq!(s.read_task_brief().unwrap(), "hello brief");

        s.write_state(&serde_json::json!({"k": 7})).unwrap();
        let back: serde_json::Value = s.read_state().unwrap();
        assert_eq!(back, serde_json::json!({"k": 7}));
    }

    #[test]
    fn task_brief_write_failure_is_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        // Make the run root a *file* so creating subdirs/writing underneath
        // fails — exercising the fatal classification.
        let runs = tmp.path().join(".lingxi/multi-agent/runs");
        fs::create_dir_all(&runs).unwrap();
        fs::write(runs.join("01HX9ABCD12"), "i am a file, not a dir").unwrap();

        let s = store(tmp.path());
        let err = s.write_task_brief("brief").unwrap_err();
        match err {
            MultiAgentError::ArtifactIo { .. } => {}
            other => panic!("expected fatal ArtifactIo, got {other:?}"),
        }
    }

    #[test]
    fn candidate_log_write_failure_is_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let runs = tmp.path().join(".lingxi/multi-agent/runs");
        fs::create_dir_all(&runs).unwrap();
        // Block the candidate dir by planting a file where the dir must go.
        let run_root = runs.join("01HX9ABCD12");
        fs::create_dir_all(&run_root).unwrap();
        fs::write(run_root.join("candidate-a"), "blocker").unwrap();

        let s = store(tmp.path());
        let err = s
            .write_candidate_verification_log("candidate-a", "log")
            .unwrap_err();
        assert!(matches!(err, MultiAgentError::ArtifactIo { .. }));
    }

    // --- cleanup fail-closed ---

    struct ScriptedManager {
        summary: std::sync::Mutex<Option<Option<WorktreeChangeSummary>>>,
        summary_errors: bool,
        remove_fails: bool,
        removed: std::sync::Mutex<Vec<WorktreeHandle>>,
    }

    impl ScriptedManager {
        fn new() -> Self {
            Self {
                summary: std::sync::Mutex::new(None),
                summary_errors: false,
                remove_fails: false,
                removed: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn with_summary(self, s: Option<WorktreeChangeSummary>) -> Self {
            *self.summary.lock().unwrap() = Some(s);
            self
        }
        fn summary_errors(mut self) -> Self {
            self.summary_errors = true;
            self
        }
        fn remove_fails(mut self) -> Self {
            self.remove_fails = true;
            self
        }
    }

    #[async_trait]
    impl WorktreeManager for ScriptedManager {
        async fn create_worktree(
            &self,
            _slug: &str,
            _b: Option<&str>,
            _c: &[PathBuf],
        ) -> Result<WorktreeHandle, WorktreeError> {
            unreachable!("not used in cleanup tests")
        }
        async fn remove_worktree(&self, handle: &WorktreeHandle) -> Result<(), WorktreeError> {
            if self.remove_fails {
                return Err(WorktreeError::Git("remove failed".into()));
            }
            self.removed.lock().unwrap().push(handle.clone());
            Ok(())
        }
        async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError> {
            Ok(Vec::new())
        }
        async fn cleanup_stale(&self, _m: Duration) -> Result<Vec<PathBuf>, WorktreeError> {
            Ok(Vec::new())
        }
        fn is_supported(&self) -> bool {
            true
        }
        async fn worktree_change_summary(
            &self,
            _handle: &WorktreeHandle,
        ) -> Result<Option<WorktreeChangeSummary>, WorktreeError> {
            if self.summary_errors {
                return Err(WorktreeError::Git("cannot query".into()));
            }
            Ok(self.summary.lock().unwrap().clone().unwrap_or(None))
        }
    }

    fn handle() -> WorktreeHandle {
        WorktreeHandle {
            path: PathBuf::from("/tmp/wt/multi-agent+RUN+candidate-a"),
            branch_name: "worktree-multi-agent+RUN+candidate-a".into(),
        }
    }

    #[tokio::test]
    async fn cleanup_removes_when_known_clean() {
        let clean = WorktreeChangeSummary {
            changed_files: 0,
            commits: 0,
        };
        let mgr = ScriptedManager::new().with_summary(Some(clean));
        let out = cleanup_worktree_fail_closed(&mgr, &handle()).await;
        assert!(matches!(out, CleanupOutcome::Removed { .. }));
        assert_eq!(mgr.removed.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cleanup_retains_when_state_unknown() {
        let mgr = ScriptedManager::new().with_summary(None);
        let out = cleanup_worktree_fail_closed(&mgr, &handle()).await;
        match out {
            CleanupOutcome::Retained { reason, .. } => {
                assert_eq!(reason, RetainReason::DirtyStateUnknown);
            }
            other => panic!("expected retain, got {other:?}"),
        }
        assert!(mgr.removed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cleanup_retains_when_summary_query_errors() {
        let mgr = ScriptedManager::new().summary_errors();
        let out = cleanup_worktree_fail_closed(&mgr, &handle()).await;
        assert!(matches!(
            out,
            CleanupOutcome::Retained {
                reason: RetainReason::DirtyStateUnknown,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn cleanup_retains_when_dirty() {
        let dirty = WorktreeChangeSummary {
            changed_files: 3,
            commits: 1,
        };
        let mgr = ScriptedManager::new().with_summary(Some(dirty));
        let out = cleanup_worktree_fail_closed(&mgr, &handle()).await;
        assert!(matches!(
            out,
            CleanupOutcome::Retained {
                reason: RetainReason::HasUnmergedWork,
                ..
            }
        ));
        assert!(mgr.removed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cleanup_retains_when_removal_fails() {
        let clean = WorktreeChangeSummary {
            changed_files: 0,
            commits: 0,
        };
        let mgr = ScriptedManager::new().with_summary(Some(clean)).remove_fails();
        let out = cleanup_worktree_fail_closed(&mgr, &handle()).await;
        match out {
            CleanupOutcome::Retained {
                reason: RetainReason::RemovalFailed(_),
                ..
            } => {}
            other => panic!("expected RemovalFailed retain, got {other:?}"),
        }
    }
}
