//! Author-revision phase (design doc §Cross-review 规则 steps 5-6,
//! §Review/revision failure handling, Phase 5).
//!
//! After cross-review, each **author** revises its OWN implementation in its
//! OWN worktree, based on the review of *its* branch. The rules enforced here
//! (design doc §安全约束, §Worktree 隔离):
//!
//! - A reviser may write only its own worktree (`cwd`). Candidate-a never
//!   touches candidate-b's worktree and vice versa — the host hands each
//!   reviser only its own `cwd`, exactly as the implementation phase does.
//! - Revision is a tool-loop (it edits files), so — like the candidate runner —
//!   it is a [`Reviser`] trait seam. The real `llm-client`/`agent` loop is a
//!   marked `// TODO(multi-agent):` seam ([`TodoLlmReviser`]).
//! - Revision is **recoverable** (design doc §Review/revision failure
//!   handling): on timeout or provider error we KEEP the candidate's
//!   pre-revision patch and record the failure — we never discard the working
//!   implementation because a revision attempt failed.
//!
//! Revision only runs when `authors_fix_own_branch` is set and there is review
//! feedback to act on; the caller (orchestrator) gates that.

use crate::error::MultiAgentError;
use crate::providers::ResolvedCandidate;
use crate::state::DualLlmPhase;
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// Inputs handed to a [`Reviser`] for one author's revision pass.
#[derive(Debug, Clone)]
pub struct RevisionContext {
    /// Author candidate id (the one revising its own branch).
    pub candidate_id: String,
    /// The shared task brief.
    pub task_brief: String,
    /// The review of THIS candidate's branch (what the competitor wrote about
    /// this implementation). Empty when no review was produced.
    pub review_feedback: String,
    /// The author's **own** isolated worktree. The reviser must only write
    /// under this path.
    pub cwd: PathBuf,
    /// The author's pre-revision patch — retained verbatim if the revision
    /// fails so the run never loses the working implementation.
    pub pre_revision_patch: String,
    /// The author's resolved provider route (dual-LLM dual-PROVIDER): the
    /// reviser runs the revision pass against the SAME provider/model the
    /// candidate implemented with, so a candidate authored on provider A is
    /// revised on provider A. Mirrors [`crate::orchestrator::CandidateRunContext::resolved`].
    pub resolved: ResolvedCandidate,
    /// Cooperative cancellation.
    pub cancel: CancellationToken,
}

/// What a successful revision produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisedOutcome {
    /// The post-revision unified diff for the author's worktree.
    pub patch_diff: String,
    /// A short revision note (fixed / rejected / not-applicable per item).
    pub note: String,
}

/// Revises one author's implementation **in its own worktree**.
///
/// Implementations MUST confine all writes to [`RevisionContext::cwd`].
#[async_trait]
pub trait Reviser: Send + Sync {
    /// Revise the author's implementation in `ctx.cwd`, returning the updated
    /// patch + a note. Errors are recoverable (the caller keeps the
    /// pre-revision patch).
    async fn revise(&self, ctx: &RevisionContext) -> Result<RevisedOutcome, MultiAgentError>;
}

// The REAL reviser is the composition-root adapter
// `engine_desktop::multi_agent_runtime::SpawnerReviser`: it drives the session's
// `traits::SubagentSpawner` cwd-pinned to `ctx.cwd` (the author's OWN worktree),
// seeds [`crate::prompts::REVISER`] + the task brief + the review feedback + the
// pre-revision patch, and host-derives the post-revision `git diff`. It lives in
// `engine-desktop` (not here) so this crate stays thin + trait-driven (no `agent`
// dep). A reviser error is recoverable — [`revise_author`] keeps the
// pre-revision patch. Tests in this crate use [`MockReviser`].

/// The patch that should be carried into arbitration for a candidate after the
/// (possibly failed / skipped) revision phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionResult {
    /// Author candidate id.
    pub candidate_id: String,
    /// The patch to use downstream — the revised patch on success, or the
    /// pre-revision patch when revision failed / timed out / was skipped.
    pub effective_patch: String,
    /// Whether the patch was actually revised (`true`) or is the pre-revision
    /// patch kept after a failed/skipped revision (`false`).
    pub revised: bool,
    /// The revision note on success, or a short failure reason otherwise.
    pub note: String,
}

/// Run one author's revision in its own worktree, bounded by `revision_timeout`
/// (the `AuthorRevision` phase timeout).
///
/// On timeout the candidate's `cancel` is fired (so its subprocesses stop) and
/// the **pre-revision patch is kept** (design doc §Review/revision failure
/// handling). On a provider/runner error the pre-revision patch is likewise
/// kept. Only a clean success replaces the effective patch.
pub async fn revise_author(
    reviser: &dyn Reviser,
    ctx: RevisionContext,
    revision_timeout: Duration,
) -> RevisionResult {
    let candidate_id = ctx.candidate_id.clone();
    let pre = ctx.pre_revision_patch.clone();
    let _start = Instant::now();

    let outcome = match tokio::time::timeout(revision_timeout, reviser.revise(&ctx)).await {
        Ok(inner) => inner,
        Err(_elapsed) => {
            ctx.cancel.cancel();
            return RevisionResult {
                candidate_id,
                effective_patch: pre,
                revised: false,
                note: format!(
                    "revision timed out in {}; kept pre-revision patch",
                    DualLlmPhase::AuthorRevision.as_str()
                ),
            };
        }
    };

    match outcome {
        Ok(rev) if !rev.patch_diff.trim().is_empty() => RevisionResult {
            candidate_id,
            effective_patch: rev.patch_diff,
            revised: true,
            note: rev.note,
        },
        // A successful call that produced an empty diff means "no change" — keep
        // the pre-revision patch rather than wiping the implementation.
        Ok(_empty) => RevisionResult {
            candidate_id,
            effective_patch: pre,
            revised: false,
            note: "revision produced no changes; kept pre-revision patch".to_string(),
        },
        Err(e) => RevisionResult {
            candidate_id,
            effective_patch: pre,
            revised: false,
            note: format!("revision failed: {e}; kept pre-revision patch"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Clone)]
    enum Behavior {
        /// Succeed, writing a marker file into the author's OWN cwd.
        Succeed { patch: String, note: String },
        /// Succeed with an empty diff (no change).
        Empty,
        /// Return a recoverable error.
        Fail,
        /// Hang until cancelled / timed out.
        Hang,
    }

    struct MockReviser {
        behaviors: BTreeMap<String, Behavior>,
        invoked_cwds: Mutex<BTreeMap<String, PathBuf>>,
    }

    impl MockReviser {
        fn new(behaviors: BTreeMap<String, Behavior>) -> Self {
            Self { behaviors, invoked_cwds: Mutex::new(BTreeMap::new()) }
        }
    }

    #[async_trait]
    impl Reviser for MockReviser {
        async fn revise(&self, ctx: &RevisionContext) -> Result<RevisedOutcome, MultiAgentError> {
            self.invoked_cwds
                .lock()
                .unwrap()
                .insert(ctx.candidate_id.clone(), ctx.cwd.clone());
            match self.behaviors.get(&ctx.candidate_id).cloned() {
                Some(Behavior::Succeed { patch, note }) => {
                    // Write ONLY into our own cwd — proves worktree isolation.
                    let marker = ctx.cwd.join(format!("{}.revised", ctx.candidate_id));
                    std::fs::write(&marker, b"x").unwrap();
                    Ok(RevisedOutcome { patch_diff: patch, note })
                }
                Some(Behavior::Empty) => Ok(RevisedOutcome {
                    patch_diff: String::new(),
                    note: "no change".into(),
                }),
                Some(Behavior::Fail) => Err(MultiAgentError::CandidateFailed {
                    candidate_id: ctx.candidate_id.clone(),
                    phase: DualLlmPhase::AuthorRevision,
                    reason: "scripted revision failure".into(),
                }),
                Some(Behavior::Hang) => {
                    ctx.cancel.cancelled().await;
                    Err(MultiAgentError::Cancelled)
                }
                None => Err(MultiAgentError::CandidateFailed {
                    candidate_id: ctx.candidate_id.clone(),
                    phase: DualLlmPhase::AuthorRevision,
                    reason: "no behavior".into(),
                }),
            }
        }
    }

    /// Build a `ResolvedCandidate` (a single-provider route) for a test ctx.
    fn resolved_for(id: &str) -> ResolvedCandidate {
        use llm_client::{
            AuthStrategy, Capabilities, ClientConfig, CredentialConfig, ModelProfile,
            PricingConfig, ProtocolFamily, ProviderId, ProviderProfile,
        };
        let cfg = ClientConfig {
            providers: vec![ProviderProfile {
                provider_id: ProviderId::OpenAICompatible { name: "p".into() },
                profile_name: "p".into(),
                base_url: "https://example.test/v1".into(),
                protocol: ProtocolFamily::OpenAiChat,
                auth: AuthStrategy::ApiKey,
                credential: CredentialConfig::Static { id: "p".into() },
                models: vec![ModelProfile {
                    display_model: "m".into(),
                    request_model: "m".into(),
                    billing_model: "m".into(),
                    aliases: Vec::new(),
                    description: None,
                    capabilities: Capabilities::default(),
                }],
                pricing: PricingConfig::default(),
                signing: None,
                azure: None,
                supports_websockets: false,
                supports_websocket_compression: false,
                websocket_connect_timeout_ms: None,
            }],
        };
        crate::ModelResolver::from_client_config(cfg)
            .unwrap()
            .resolve_endpoint(&crate::config::AgentEndpoint {
                id: id.into(),
                label: None,
                model: "p/m".into(),
                role: None,
            })
            .unwrap()
    }

    fn ctx_in(dir: &std::path::Path, id: &str) -> RevisionContext {
        RevisionContext {
            candidate_id: id.to_string(),
            task_brief: "brief".into(),
            review_feedback: "blocking: fix it".into(),
            cwd: dir.to_path_buf(),
            pre_revision_patch: "diff --git a/old.rs b/old.rs\n+old\n".into(),
            resolved: resolved_for(id),
            cancel: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn revision_only_writes_its_own_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd_a = tmp.path().join("wt-a");
        let cwd_b = tmp.path().join("wt-b");
        std::fs::create_dir_all(&cwd_a).unwrap();
        std::fs::create_dir_all(&cwd_b).unwrap();

        let reviser = MockReviser::new(
            [
                (
                    "candidate-a".to_string(),
                    Behavior::Succeed {
                        patch: "diff --git a/new.rs b/new.rs\n+new\n".into(),
                        note: "fixed".into(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );

        let res = revise_author(
            &reviser,
            ctx_in(&cwd_a, "candidate-a"),
            Duration::from_secs(5),
        )
        .await;
        assert!(res.revised);
        assert_eq!(res.effective_patch, "diff --git a/new.rs b/new.rs\n+new\n");

        // Wrote ONLY into its own cwd.
        assert!(cwd_a.join("candidate-a.revised").is_file());
        assert!(!cwd_b.join("candidate-a.revised").exists());
        let cwds = reviser.invoked_cwds.lock().unwrap();
        assert_eq!(cwds["candidate-a"], cwd_a);
    }

    #[tokio::test]
    async fn revision_timeout_keeps_pre_revision_patch() {
        let tmp = tempfile::tempdir().unwrap();
        let reviser = MockReviser::new(
            [("candidate-a".to_string(), Behavior::Hang)].into_iter().collect(),
        );
        let ctx = ctx_in(tmp.path(), "candidate-a");
        let pre = ctx.pre_revision_patch.clone();
        let res = revise_author(&reviser, ctx, Duration::from_millis(50)).await;
        assert!(!res.revised, "timed-out revision is not counted as revised");
        assert_eq!(res.effective_patch, pre, "pre-revision patch is kept");
        assert!(res.note.contains("timed out"));
    }

    #[tokio::test]
    async fn revision_error_keeps_pre_revision_patch() {
        let tmp = tempfile::tempdir().unwrap();
        let reviser = MockReviser::new(
            [("candidate-a".to_string(), Behavior::Fail)].into_iter().collect(),
        );
        let ctx = ctx_in(tmp.path(), "candidate-a");
        let pre = ctx.pre_revision_patch.clone();
        let res = revise_author(&reviser, ctx, Duration::from_secs(5)).await;
        assert!(!res.revised);
        assert_eq!(res.effective_patch, pre);
        assert!(res.note.contains("revision failed"));
    }

    #[tokio::test]
    async fn empty_revision_keeps_pre_revision_patch() {
        let tmp = tempfile::tempdir().unwrap();
        let reviser = MockReviser::new(
            [("candidate-a".to_string(), Behavior::Empty)].into_iter().collect(),
        );
        let ctx = ctx_in(tmp.path(), "candidate-a");
        let pre = ctx.pre_revision_patch.clone();
        let res = revise_author(&reviser, ctx, Duration::from_secs(5)).await;
        assert!(!res.revised);
        assert_eq!(res.effective_patch, pre);
    }

}
