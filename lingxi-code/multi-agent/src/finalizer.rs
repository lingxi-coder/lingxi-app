//! Finalizer phase (design doc §Finalizer, §Finalizer failure handling,
//! Phase 6).
//!
//! The finalizer is the **only** writer to the main workspace (design doc
//! §安全约束 #3). Given the arbiter's [`Decision`] and the candidate patches it
//! performs the host-controlled apply sequence:
//!
//! 1. Check the main workspace for user changes that conflict with the target
//!    files. A dirty conflict means **do not overwrite** → fail with
//!    [`MultiAgentError::FinalizerFailed`].
//! 2. Generate `final.patch` from the winning candidate.
//! 3. **dry-run** apply (`git apply --check`-style). A dry-run failure means
//!    **no partial apply** — write `finalizer-error.md`, keep the winner
//!    worktree, report the conflicting files, and fail the run.
//! 4. Only after the dry-run succeeds, apply for real.
//!
//! Decision handling (design doc §Finalizer strategy):
//!
//! - `accept A` / `accept B` → apply that candidate's patch.
//! - `synthesize hybrid` → requires an **explicit patch strategy** in the
//!   arbitration (a non-empty hybrid plan). Hybrid without a strategy is
//!   rejected ([`FinalizeError`]→`FinalizerFailed`) rather than guessed at.
//! - `reject both` → keep both worktrees + review docs, **request the user**;
//!   never a silent single-agent fallback.

use crate::arbiter::Arbitration;
use crate::arbiter::Decision;
use crate::artifacts::ArtifactStore;
use crate::error::MultiAgentError;
use async_trait::async_trait;
use std::path::Path;
use std::path::PathBuf;

/// A patch chosen for a candidate, ready for the finalizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidatePatch {
    /// Stable candidate id.
    pub candidate_id: String,
    /// The unified diff to apply to the main workspace.
    pub patch_diff: String,
    /// Workspace-relative paths the patch touches (used for conflict checks and
    /// to narrow verification). Host-derived from the diff, never LLM prose.
    pub changed_paths: Vec<PathBuf>,
    /// The candidate's worktree path (retained on `reject_both` / conflict).
    pub worktree_path: PathBuf,
}

/// The hybrid patch plan extracted from arbitration. Hybrid finalization
/// **requires** an explicit, non-empty plan; an empty plan is rejected.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HybridPlan {
    /// The synthesized unified diff (the explicit patch strategy).
    pub patch_diff: String,
    /// Workspace-relative paths the plan touches.
    pub changed_paths: Vec<PathBuf>,
}

impl HybridPlan {
    /// Whether the plan carries an explicit, applicable strategy.
    #[must_use]
    pub fn is_explicit(&self) -> bool {
        !self.patch_diff.trim().is_empty()
    }
}

/// What the finalizer was asked to apply, resolved from the arbiter decision.
#[derive(Debug, Clone)]
pub enum FinalizePlan {
    /// Apply a single candidate's patch verbatim.
    AcceptCandidate(CandidatePatch),
    /// Apply a synthesized hybrid patch (requires an explicit plan).
    Hybrid(HybridPlan),
    /// Neither candidate is acceptable — request the user (no apply).
    RejectBoth {
        /// The retained candidate worktree paths to report.
        retained_worktrees: Vec<PathBuf>,
    },
}

impl FinalizePlan {
    /// Build the finalize plan from the arbiter decision and the candidate
    /// patches. `hybrid` is the explicit plan extracted from arbitration (only
    /// consulted for `SynthesizeHybrid`).
    ///
    /// Returns an error for a hybrid decision **without** an explicit plan
    /// (design doc §Finalizer failure handling: "hybrid synthesis incomplete →
    /// reject hybrid").
    pub fn from_decision(
        arbitration: &Arbitration,
        a: CandidatePatch,
        b: CandidatePatch,
        hybrid: Option<HybridPlan>,
    ) -> Result<Self, MultiAgentError> {
        match arbitration.decision {
            Decision::AcceptA => Ok(FinalizePlan::AcceptCandidate(a)),
            Decision::AcceptB => Ok(FinalizePlan::AcceptCandidate(b)),
            Decision::SynthesizeHybrid => match hybrid {
                Some(plan) if plan.is_explicit() => Ok(FinalizePlan::Hybrid(plan)),
                _ => Err(MultiAgentError::FinalizerFailed {
                    reason: "hybrid decision requires an explicit patch strategy, but none was \
                             provided (refusing to guess)"
                        .to_string(),
                }),
            },
            Decision::RejectBoth => Ok(FinalizePlan::RejectBoth {
                retained_worktrees: vec![a.worktree_path, b.worktree_path],
            }),
        }
    }
}

/// Applies (and dry-runs) patches against the main workspace. Abstracted so the
/// finalizer is testable without a real git repository. The real implementation
/// wraps `git apply` (`--check` for dry-run); see [`GitPatchApplier`].
#[async_trait]
pub trait PatchApplier: Send + Sync {
    /// Report workspace-relative paths in the main workspace that currently
    /// carry **uncommitted user changes** (used to detect conflicts before
    /// applying). Empty = clean.
    async fn dirty_paths(&self, workspace: &Path) -> Result<Vec<PathBuf>, ApplyError>;

    /// Dry-run apply `patch` to `workspace` (no mutation). `Ok(())` means it
    /// would apply cleanly; `Err(ApplyError::Conflict { files })` lists the
    /// rejecting files.
    async fn dry_run(&self, workspace: &Path, patch: &str) -> Result<(), ApplyError>;

    /// Apply `patch` to `workspace` for real. Only called after a successful
    /// [`PatchApplier::dry_run`].
    async fn apply(&self, workspace: &Path, patch: &str) -> Result<(), ApplyError>;
}

/// Why a patch operation failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ApplyError {
    /// The patch does not apply cleanly; `files` are the rejecting paths.
    #[error("patch conflict in {} file(s)", files.len())]
    Conflict {
        /// The conflicting files.
        files: Vec<PathBuf>,
    },
    /// The underlying VCS / IO operation failed.
    #[error("apply backend error: {0}")]
    Backend(String),
}

/// Result of a successful finalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizeOutcome {
    /// A patch was applied to the main workspace.
    Applied {
        /// The candidate id whose patch was applied (`<hybrid>` for hybrid).
        applied_id: String,
        /// Paths written into the main workspace.
        changed_paths: Vec<PathBuf>,
    },
    /// The decision was `reject both` — nothing was applied; the user must
    /// decide. Worktrees + review docs are retained.
    RejectedBoth {
        /// Retained worktree paths to surface to the user.
        retained_worktrees: Vec<PathBuf>,
    },
}

/// The host-controlled finalizer. Holds the patch applier and the artifact
/// store; `finalize` is the single entry point and the **only** code path that
/// mutates the main workspace.
pub struct Finalizer<'a> {
    applier: &'a dyn PatchApplier,
    store: &'a ArtifactStore,
    /// The main workspace root (the only directory `finalize` may mutate).
    workspace: PathBuf,
}

impl<'a> Finalizer<'a> {
    /// Construct a finalizer rooted at the main `workspace`.
    #[must_use]
    pub fn new(
        applier: &'a dyn PatchApplier,
        store: &'a ArtifactStore,
        workspace: impl AsRef<Path>,
    ) -> Self {
        Self { applier, store, workspace: workspace.as_ref().to_path_buf() }
    }

    /// Execute the finalize plan against the main workspace.
    ///
    /// Sequence (design doc §Finalizer failure handling):
    /// 1. `reject both` → retain worktrees + request user (no apply).
    /// 2. Conflict check: a target file with uncommitted user changes →
    ///    `FinalizerFailed` (do not overwrite).
    /// 3. Write `final.patch`.
    /// 4. dry-run apply. Failure → write `finalizer-error.md`, keep worktree,
    ///    report conflict files, fail — **no partial apply**.
    /// 5. Apply for real.
    pub async fn finalize(&self, plan: FinalizePlan) -> Result<FinalizeOutcome, MultiAgentError> {
        let (applied_id, patch_diff, changed_paths, retained_worktree) = match plan {
            FinalizePlan::RejectBoth { retained_worktrees } => {
                // No apply; request the user. Record the reason for audit.
                self.store.write_finalizer_error(
                    "# Finalizer: reject both\n\nThe arbiter rejected both candidates. No patch \
                     was applied to the main workspace. Candidate worktrees and review documents \
                     are retained; user decision required.\n",
                )?;
                return Ok(FinalizeOutcome::RejectedBoth { retained_worktrees });
            }
            FinalizePlan::AcceptCandidate(c) => {
                (c.candidate_id, c.patch_diff, c.changed_paths, c.worktree_path)
            }
            FinalizePlan::Hybrid(plan) => {
                // `from_decision` already guaranteed the plan is explicit, but
                // re-check defensively: a hybrid must never be guessed at.
                if !plan.is_explicit() {
                    return Err(MultiAgentError::FinalizerFailed {
                        reason: "hybrid plan is not explicit".to_string(),
                    });
                }
                ("<hybrid>".to_string(), plan.patch_diff, plan.changed_paths, PathBuf::new())
            }
        };

        // --- 2. Conflict check against uncommitted user changes. ---
        let dirty = self
            .applier
            .dirty_paths(&self.workspace)
            .await
            .map_err(|e| MultiAgentError::FinalizerFailed {
                reason: format!("could not determine dirty main-workspace files: {e}"),
            })?;
        let conflicts: Vec<PathBuf> = changed_paths
            .iter()
            .filter(|p| dirty.contains(p))
            .cloned()
            .collect();
        if !conflicts.is_empty() {
            let msg = format!(
                "# Finalizer error: dirty main-workspace conflict\n\n\
                 The main workspace has uncommitted user changes to files this patch would \
                 modify. Refusing to overwrite user work (no partial apply).\n\n\
                 Conflicting files:\n{}\n\nWinner worktree retained at: {}\n",
                render_paths(&conflicts),
                retained_worktree.display(),
            );
            self.store.write_finalizer_error(&msg)?;
            return Err(MultiAgentError::FinalizerFailed {
                reason: format!(
                    "dirty main-workspace conflict on {} file(s); user changes not overwritten",
                    conflicts.len()
                ),
            });
        }

        // --- 3. Persist final.patch (audit record). ---
        self.store.write_final_patch(&patch_diff)?;

        // --- 4. dry-run apply (no partial apply on failure). ---
        if let Err(e) = self.applier.dry_run(&self.workspace, &patch_diff).await {
            let conflict_files = match &e {
                ApplyError::Conflict { files } => files.clone(),
                ApplyError::Backend(_) => Vec::new(),
            };
            let msg = format!(
                "# Finalizer error: patch dry-run failed\n\n\
                 The winning patch did not apply cleanly to the main workspace. No changes were \
                 made (no partial apply).\n\nError: {e}\nConflicting files:\n{}\n\n\
                 Winner worktree retained at: {}\n",
                render_paths(&conflict_files),
                retained_worktree.display(),
            );
            self.store.write_finalizer_error(&msg)?;
            return Err(MultiAgentError::FinalizerFailed {
                reason: format!("patch dry-run failed: {e} (no partial apply)"),
            });
        }

        // --- 5. Real apply (only after a clean dry-run). ---
        self.applier
            .apply(&self.workspace, &patch_diff)
            .await
            .map_err(|e| MultiAgentError::FinalizerFailed {
                reason: format!("patch apply failed after a clean dry-run: {e}"),
            })?;

        Ok(FinalizeOutcome::Applied { applied_id, changed_paths })
    }
}

fn render_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "  (none reported)".to_string()
    } else {
        paths.iter().map(|p| format!("  - {}", p.display())).collect::<Vec<_>>().join("\n")
    }
}

/// Real [`PatchApplier`] backed by the `git` CLI run with the main-workspace as
/// cwd (design doc §Finalizer, §安全约束 #3):
///
/// - [`dirty_paths`](PatchApplier::dirty_paths) = `git status --porcelain`
///   (any tracked-but-modified or untracked path that a patch could clobber).
/// - [`dry_run`](PatchApplier::dry_run) = `git apply --check -` (patch on stdin,
///   no mutation).
/// - [`apply`](PatchApplier::apply) = `git apply -` (patch on stdin).
///
/// A failure to even run `git` (binary missing, not a repo, …) surfaces as
/// [`ApplyError::Backend`]; a non-zero `git apply` exit surfaces as
/// [`ApplyError::Conflict`] with the rejecting files parsed from git's stderr.
#[derive(Debug, Default, Clone, Copy)]
pub struct GitPatchApplier;

impl GitPatchApplier {
    /// Run `git <args>` in `workspace`, optionally feeding `stdin`. Returns the
    /// completed output or a [`ApplyError::Backend`] if git could not be run.
    async fn run_git(
        workspace: &Path,
        args: &[&str],
        stdin: Option<&str>,
    ) -> Result<std::process::Output, ApplyError> {
        use tokio::io::AsyncWriteExt;
        let mut cmd = tokio::process::Command::new("git");
        cmd.current_dir(workspace)
            .args(args)
            .stdin(if stdin.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| ApplyError::Backend(format!("could not spawn git: {e}")))?;
        if let Some(data) = stdin {
            let mut sink = child
                .stdin
                .take()
                .ok_or_else(|| ApplyError::Backend("git stdin unavailable".to_string()))?;
            sink.write_all(data.as_bytes())
                .await
                .map_err(|e| ApplyError::Backend(format!("writing patch to git stdin: {e}")))?;
            // Drop the handle to send EOF so `git apply` stops reading.
            drop(sink);
        }
        child
            .wait_with_output()
            .await
            .map_err(|e| ApplyError::Backend(format!("waiting for git: {e}")))
    }

    /// Map a non-zero `git apply` invocation to a [`ApplyError`]: a clean
    /// success is `Ok`, a non-zero exit is a `Conflict` carrying the rejecting
    /// files parsed from stderr.
    fn apply_result(out: std::process::Output) -> Result<(), ApplyError> {
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(ApplyError::Conflict { files: parse_rejected_files(&stderr) })
    }
}

#[async_trait]
impl PatchApplier for GitPatchApplier {
    async fn dirty_paths(&self, workspace: &Path) -> Result<Vec<PathBuf>, ApplyError> {
        let out = Self::run_git(workspace, &["status", "--porcelain"], None).await?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(ApplyError::Backend(format!(
                "git status failed: {}",
                stderr.trim()
            )));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        Ok(parse_porcelain_paths(&stdout))
    }

    async fn dry_run(&self, workspace: &Path, patch: &str) -> Result<(), ApplyError> {
        let out = Self::run_git(workspace, &["apply", "--check", "-"], Some(patch)).await?;
        Self::apply_result(out)
    }

    async fn apply(&self, workspace: &Path, patch: &str) -> Result<(), ApplyError> {
        let out = Self::run_git(workspace, &["apply", "-"], Some(patch)).await?;
        Self::apply_result(out)
    }
}

/// Parse `git status --porcelain` output into the set of paths that carry
/// uncommitted changes (tracked-modified, staged, or untracked). Rename entries
/// (`R  old -> new`) contribute the new path.
fn parse_porcelain_paths(stdout: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        // Porcelain v1: 2 status chars + space, then the path.
        let rest = &line[3..];
        let path = match rest.split_once(" -> ") {
            Some((_old, new)) => new, // rename: the new path is what would clobber
            None => rest,
        };
        let path = path.trim().trim_matches('"');
        if !path.is_empty() {
            paths.push(PathBuf::from(path));
        }
    }
    paths
}

/// Extract the rejecting file paths from `git apply` stderr. git emits lines
/// like `error: patch failed: src/a.rs:12` and
/// `error: src/a.rs: patch does not apply`.
fn parse_rejected_files(stderr: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for line in stderr.lines() {
        let line = line.trim();
        let path = if let Some(rest) = line.strip_prefix("error: patch failed: ") {
            // `<path>:<line>` — strip the trailing `:<line>`.
            rest.rsplit_once(':').map(|(p, _)| p).unwrap_or(rest)
        } else if let Some(rest) = line.strip_prefix("error: ") {
            // `<path>: patch does not apply`
            match rest.strip_suffix(": patch does not apply") {
                Some(p) => p,
                None => continue,
            }
        } else {
            continue;
        };
        let path = PathBuf::from(path.trim());
        if !files.contains(&path) {
            files.push(path);
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbiter::DecisionSource;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    fn arbitration(decision: Decision) -> Arbitration {
        Arbitration { decision, source: DecisionSource::Arbiter, rationale: "r".into() }
    }

    fn patch(id: &str, paths: &[&str]) -> CandidatePatch {
        CandidatePatch {
            candidate_id: id.into(),
            patch_diff: format!("diff --git a/{0} b/{0}\n+line\n", paths.first().unwrap_or(&"x")),
            changed_paths: paths.iter().map(PathBuf::from).collect(),
            worktree_path: PathBuf::from(format!("/wt/{id}")),
        }
    }

    /// A patch applier that records calls and is scriptable for conflicts.
    #[derive(Default)]
    struct MockApplier {
        dirty: Vec<PathBuf>,
        dry_run_conflict: Option<Vec<PathBuf>>,
        dry_run_backend_err: bool,
        applied: Mutex<Vec<String>>,
        dry_runs: AtomicUsize,
        real_applies: AtomicUsize,
    }

    #[async_trait]
    impl PatchApplier for MockApplier {
        async fn dirty_paths(&self, _workspace: &Path) -> Result<Vec<PathBuf>, ApplyError> {
            Ok(self.dirty.clone())
        }
        async fn dry_run(&self, _workspace: &Path, _patch: &str) -> Result<(), ApplyError> {
            self.dry_runs.fetch_add(1, Ordering::SeqCst);
            if self.dry_run_backend_err {
                return Err(ApplyError::Backend("backend boom".into()));
            }
            match &self.dry_run_conflict {
                Some(files) => Err(ApplyError::Conflict { files: files.clone() }),
                None => Ok(()),
            }
        }
        async fn apply(&self, _workspace: &Path, patch: &str) -> Result<(), ApplyError> {
            self.real_applies.fetch_add(1, Ordering::SeqCst);
            self.applied.lock().unwrap().push(patch.to_string());
            Ok(())
        }
    }

    fn store(tmp: &Path) -> ArtifactStore {
        let s = ArtifactStore::new(tmp, "RUN");
        s.init().unwrap();
        s
    }

    #[tokio::test]
    async fn accept_a_applies_a_patch() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        let applier = MockApplier::default();
        let fin = Finalizer::new(&applier, &s, tmp.path());

        let plan = FinalizePlan::from_decision(
            &arbitration(Decision::AcceptA),
            patch("candidate-a", &["src/a.rs"]),
            patch("candidate-b", &["src/b.rs"]),
            None,
        )
        .unwrap();
        let out = fin.finalize(plan).await.unwrap();
        match out {
            FinalizeOutcome::Applied { applied_id, .. } => assert_eq!(applied_id, "candidate-a"),
            other => panic!("expected applied, got {other:?}"),
        }
        // dry-run preceded the real apply exactly once each.
        assert_eq!(applier.dry_runs.load(Ordering::SeqCst), 1);
        assert_eq!(applier.real_applies.load(Ordering::SeqCst), 1);
        // final.patch persisted.
        assert!(s.paths().final_patch().is_file());
    }

    #[tokio::test]
    async fn accept_b_applies_b_patch() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        let applier = MockApplier::default();
        let fin = Finalizer::new(&applier, &s, tmp.path());
        let plan = FinalizePlan::from_decision(
            &arbitration(Decision::AcceptB),
            patch("candidate-a", &["src/a.rs"]),
            patch("candidate-b", &["src/b.rs"]),
            None,
        )
        .unwrap();
        let out = fin.finalize(plan).await.unwrap();
        assert!(matches!(out, FinalizeOutcome::Applied { applied_id, .. } if applied_id == "candidate-b"));
    }

    #[tokio::test]
    async fn hybrid_requires_explicit_patch_strategy() {
        // Hybrid with NO plan → rejected at plan construction.
        let err = FinalizePlan::from_decision(
            &arbitration(Decision::SynthesizeHybrid),
            patch("candidate-a", &["src/a.rs"]),
            patch("candidate-b", &["src/b.rs"]),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, MultiAgentError::FinalizerFailed { .. }));

        // Hybrid with an EMPTY plan → also rejected.
        let err = FinalizePlan::from_decision(
            &arbitration(Decision::SynthesizeHybrid),
            patch("candidate-a", &["src/a.rs"]),
            patch("candidate-b", &["src/b.rs"]),
            Some(HybridPlan::default()),
        )
        .unwrap_err();
        assert!(matches!(err, MultiAgentError::FinalizerFailed { .. }));

        // Hybrid WITH an explicit plan → applies.
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        let applier = MockApplier::default();
        let fin = Finalizer::new(&applier, &s, tmp.path());
        let plan = FinalizePlan::from_decision(
            &arbitration(Decision::SynthesizeHybrid),
            patch("candidate-a", &["src/a.rs"]),
            patch("candidate-b", &["src/b.rs"]),
            Some(HybridPlan {
                patch_diff: "diff --git a/src/h.rs b/src/h.rs\n+hybrid\n".into(),
                changed_paths: vec![PathBuf::from("src/h.rs")],
            }),
        )
        .unwrap();
        let out = fin.finalize(plan).await.unwrap();
        assert!(matches!(out, FinalizeOutcome::Applied { applied_id, .. } if applied_id == "<hybrid>"));
    }

    #[tokio::test]
    async fn reject_both_requests_user_and_applies_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        let applier = MockApplier::default();
        let fin = Finalizer::new(&applier, &s, tmp.path());
        let plan = FinalizePlan::from_decision(
            &arbitration(Decision::RejectBoth),
            patch("candidate-a", &["src/a.rs"]),
            patch("candidate-b", &["src/b.rs"]),
            None,
        )
        .unwrap();
        let out = fin.finalize(plan).await.unwrap();
        match out {
            FinalizeOutcome::RejectedBoth { retained_worktrees } => {
                assert_eq!(retained_worktrees.len(), 2);
            }
            other => panic!("expected rejected-both, got {other:?}"),
        }
        // Nothing applied, but the audit note exists.
        assert_eq!(applier.real_applies.load(Ordering::SeqCst), 0);
        assert_eq!(applier.dry_runs.load(Ordering::SeqCst), 0);
        assert!(s.paths().finalizer_error().is_file());
    }

    #[tokio::test]
    async fn dry_run_failure_does_not_partial_apply() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        let applier = MockApplier {
            dry_run_conflict: Some(vec![PathBuf::from("src/a.rs")]),
            ..Default::default()
        };
        let fin = Finalizer::new(&applier, &s, tmp.path());
        let plan = FinalizePlan::from_decision(
            &arbitration(Decision::AcceptA),
            patch("candidate-a", &["src/a.rs"]),
            patch("candidate-b", &["src/b.rs"]),
            None,
        )
        .unwrap();
        let err = fin.finalize(plan).await.unwrap_err();
        assert!(matches!(err, MultiAgentError::FinalizerFailed { .. }));
        // CRITICAL: the real apply was NEVER called → no partial apply.
        assert_eq!(applier.real_applies.load(Ordering::SeqCst), 0);
        assert_eq!(applier.dry_runs.load(Ordering::SeqCst), 1);
        // finalizer-error.md records the conflicting file.
        let err_doc = std::fs::read_to_string(s.paths().finalizer_error()).unwrap();
        assert!(err_doc.contains("src/a.rs"));
        assert!(err_doc.contains("/wt/candidate-a"), "winner worktree path reported");
    }

    #[tokio::test]
    async fn dirty_main_workspace_conflict_fails_without_apply() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        // The user has uncommitted changes to a file the patch would touch.
        let applier = MockApplier {
            dirty: vec![PathBuf::from("src/a.rs")],
            ..Default::default()
        };
        let fin = Finalizer::new(&applier, &s, tmp.path());
        let plan = FinalizePlan::from_decision(
            &arbitration(Decision::AcceptA),
            patch("candidate-a", &["src/a.rs"]),
            patch("candidate-b", &["src/b.rs"]),
            None,
        )
        .unwrap();
        let err = fin.finalize(plan).await.unwrap_err();
        assert!(matches!(err, MultiAgentError::FinalizerFailed { .. }));
        // Neither dry-run nor apply happened — we bailed at the conflict check.
        assert_eq!(applier.dry_runs.load(Ordering::SeqCst), 0);
        assert_eq!(applier.real_applies.load(Ordering::SeqCst), 0);
        assert!(s.paths().finalizer_error().is_file());
    }

    #[tokio::test]
    async fn finalizer_is_sole_main_workspace_writer() {
        // Only `apply` mutates the workspace, and only on the accept paths after
        // a clean dry-run. reject_both / conflict / dry-run-fail never call it.
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());

        // reject_both: zero applies.
        let applier = MockApplier::default();
        let fin = Finalizer::new(&applier, &s, tmp.path());
        let _ = fin
            .finalize(
                FinalizePlan::from_decision(
                    &arbitration(Decision::RejectBoth),
                    patch("candidate-a", &["a.rs"]),
                    patch("candidate-b", &["b.rs"]),
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(applier.real_applies.load(Ordering::SeqCst), 0);

        // accept: exactly one apply.
        let applier2 = MockApplier::default();
        let fin2 = Finalizer::new(&applier2, &s, tmp.path());
        let _ = fin2
            .finalize(
                FinalizePlan::from_decision(
                    &arbitration(Decision::AcceptA),
                    patch("candidate-a", &["a.rs"]),
                    patch("candidate-b", &["b.rs"]),
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(applier2.real_applies.load(Ordering::SeqCst), 1);
        assert_eq!(applier2.applied.lock().unwrap().len(), 1);
    }

}

/// Real-git backend tests. These shell out to `git` against throwaway repos to
/// prove the [`GitPatchApplier`] honours the [`PatchApplier`] contract.
#[cfg(test)]
mod git_applier_tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git").current_dir(dir).args(args).output().expect("spawn git")
    }

    fn git_ok(dir: &Path, args: &[&str]) {
        let out = git(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo with one committed file `a.txt` = "line1\nline2\nline3\n".
    fn init_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        git_ok(p, &["init", "-q"]);
        git_ok(p, &["config", "user.email", "t@example.com"]);
        git_ok(p, &["config", "user.name", "Tester"]);
        std::fs::write(p.join("a.txt"), "line1\nline2\nline3\n").unwrap();
        git_ok(p, &["add", "."]);
        git_ok(p, &["commit", "-q", "-m", "init"]);
        tmp
    }

    /// A patch that edits `a.txt` line 2 -> CHANGED, leaving the worktree clean.
    fn patch_editing_a(dir: &Path) -> String {
        std::fs::write(dir.join("a.txt"), "line1\nCHANGED\nline3\n").unwrap();
        let out = git(dir, &["diff"]);
        let patch = String::from_utf8(out.stdout).unwrap();
        git_ok(dir, &["checkout", "--", "a.txt"]);
        assert!(!patch.trim().is_empty(), "expected a non-empty diff");
        patch
    }

    #[tokio::test]
    async fn dirty_paths_empty_on_clean_repo() {
        let tmp = init_repo();
        let dirty = GitPatchApplier.dirty_paths(tmp.path()).await.unwrap();
        assert!(dirty.is_empty(), "clean repo reported dirty = {dirty:?}");
    }

    #[tokio::test]
    async fn dirty_paths_reports_uncommitted_edit() {
        let tmp = init_repo();
        std::fs::write(tmp.path().join("a.txt"), "edited\n").unwrap();
        let dirty = GitPatchApplier.dirty_paths(tmp.path()).await.unwrap();
        assert_eq!(dirty, vec![PathBuf::from("a.txt")]);
    }

    #[tokio::test]
    async fn dirty_paths_reports_untracked_file() {
        let tmp = init_repo();
        std::fs::write(tmp.path().join("new.txt"), "x\n").unwrap();
        let dirty = GitPatchApplier.dirty_paths(tmp.path()).await.unwrap();
        assert_eq!(dirty, vec![PathBuf::from("new.txt")]);
    }

    #[tokio::test]
    async fn dry_run_ok_does_not_mutate_workspace() {
        let tmp = init_repo();
        let patch = patch_editing_a(tmp.path());
        GitPatchApplier.dry_run(tmp.path(), &patch).await.unwrap();
        // dry-run must not touch the file.
        let content = std::fs::read_to_string(tmp.path().join("a.txt")).unwrap();
        assert_eq!(content, "line1\nline2\nline3\n");
    }

    #[tokio::test]
    async fn apply_mutates_workspace() {
        let tmp = init_repo();
        let patch = patch_editing_a(tmp.path());
        GitPatchApplier.apply(tmp.path(), &patch).await.unwrap();
        let content = std::fs::read_to_string(tmp.path().join("a.txt")).unwrap();
        assert_eq!(content, "line1\nCHANGED\nline3\n");
    }

    #[tokio::test]
    async fn dry_run_conflict_reports_rejecting_file() {
        let tmp = init_repo();
        let patch = patch_editing_a(tmp.path());
        // Move the base content out from under the patch so it cannot apply.
        std::fs::write(tmp.path().join("a.txt"), "totally\ndifferent\ncontent\n").unwrap();
        git_ok(tmp.path(), &["commit", "-aqm", "diverge base"]);

        let err = GitPatchApplier.dry_run(tmp.path(), &patch).await.unwrap_err();
        match err {
            ApplyError::Conflict { files } => {
                assert!(files.contains(&PathBuf::from("a.txt")), "files = {files:?}");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dirty_paths_backend_error_on_non_repo() {
        let tmp = tempfile::tempdir().unwrap(); // not a git repo
        let err = GitPatchApplier.dirty_paths(tmp.path()).await.unwrap_err();
        assert!(matches!(err, ApplyError::Backend(_)), "got {err:?}");
    }
}
