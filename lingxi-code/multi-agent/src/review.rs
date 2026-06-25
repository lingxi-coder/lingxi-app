//! Cross-review phase (design doc §Cross-review 规则, §Review/revision failure
//! handling, Phase 5).
//!
//! Each candidate reviews the *competing* implementation and produces a
//! **document only** — it must NOT edit any file. The review is a single-shot
//! LLM call routed through [`sidequery::SideQueryClient`] with an **empty tool
//! set**, which is how read-only is enforced at the host level (design doc
//! §安全约束 #2: "reviewer read-only: review 阶段使用只读 sandbox 或不给
//! ToolInvoker 写权限"). A reviewer never receives write-capable tools, so it
//! cannot mutate any worktree even if the prompt were ignored.
//!
//! Review is a **quality enhancement, not a correctness gate** (design doc
//! §Review/revision failure handling): a reviewer timeout or provider error is
//! recoverable — we write a `*.error.md` marker and continue to
//! revision/arbitration.
//!
//! The review-loop count has a SINGLE source of truth:
//! [`crate::config::ReviewConfig::max_review_rounds`]. `max_review_rounds == 0`
//! skips review entirely; reaching the cap forces arbitration. There is no
//! duplicate knob under `limits`.

use crate::artifacts::ArtifactStore;
use crate::config::ReviewConfig;
use crate::error::MultiAgentError;
use crate::state::DualLlmPhase;
use protocol::ConversationMessage;
use protocol::MessageId;
use sidequery::purposes::QuerySource;
use sidequery::side_query::SideQueryClient;
use sidequery::side_query::SideQueryRequest;
use std::sync::Arc;
use std::time::Duration;

/// Output-token cap for a single review document.
const REVIEW_MAX_TOKENS: u32 = 4096;

/// One candidate's implementation, as seen by the reviewer (read-only inputs).
#[derive(Debug, Clone)]
pub struct ReviewTarget {
    /// Stable id of the candidate being reviewed.
    pub candidate_id: String,
    /// The candidate's unified diff (`patch.diff`).
    pub patch_diff: String,
    /// The candidate's self-report (`self-report.md`).
    pub self_report: String,
}

/// A classified review finding (design doc reviewer prompt: blocking /
/// non-blocking / suggestion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Must be addressed; gates a clean review round.
    Blocking,
    /// Worth addressing but not gating.
    NonBlocking,
    /// Optional improvement.
    Suggestion,
}

/// The result of one reviewer pass over one target.
#[derive(Debug, Clone)]
pub struct ReviewDocument {
    /// Reviewer candidate id.
    pub reviewer_id: String,
    /// Reviewed candidate id.
    pub target_id: String,
    /// 1-based review round.
    pub round: u32,
    /// Full review markdown (persisted to `reviews/<reviewer>-on-<target>-round-N.md`).
    pub body: String,
    /// Whether the body contained at least one blocking finding.
    pub has_blocking: bool,
}

impl ReviewDocument {
    /// Number of blocking findings is collapsed to a boolean for the loop
    /// stop-condition; this convenience mirrors that.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        !self.has_blocking
    }
}

/// Per-round outcome for one (reviewer, target) pair: either a produced
/// document, or a recorded failure (timeout / provider error / artifact write)
/// that the run tolerated and continued past.
#[derive(Debug)]
pub enum ReviewOutcome {
    /// The reviewer produced a document.
    Produced(ReviewDocument),
    /// The review failed but the run continued; a `*.error.md` was written.
    Missing {
        /// Reviewer candidate id.
        reviewer_id: String,
        /// Reviewed candidate id.
        target_id: String,
        /// 1-based round.
        round: u32,
        /// The recoverable error (timeout or provider error).
        error: MultiAgentError,
    },
}

impl ReviewOutcome {
    /// The produced document, if any.
    #[must_use]
    pub fn document(&self) -> Option<&ReviewDocument> {
        match self {
            ReviewOutcome::Produced(d) => Some(d),
            ReviewOutcome::Missing { .. } => None,
        }
    }

    /// Whether this outcome carries a blocking finding. A *missing* review is
    /// NOT treated as blocking (review is not a correctness gate); it simply
    /// does not contribute a "clean" signal.
    #[must_use]
    pub fn has_blocking(&self) -> bool {
        matches!(self, ReviewOutcome::Produced(d) if d.has_blocking)
    }
}

/// Detect whether a review body reports a blocking finding.
///
/// The reviewer prompt asks the model to classify findings as
/// `blocking` / `non-blocking` / `suggestion`. We look for a `blocking`
/// classification that is NOT immediately the word `non-blocking` (so
/// "non-blocking" alone does not trip the flag), and treat an explicit
/// "no blocking" / "no blocking findings" phrase as clean.
#[must_use]
pub fn body_has_blocking(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    // Explicit "no blocking" statements => clean.
    if lower.contains("no blocking findings")
        || lower.contains("no blocking issues")
        || lower.contains("0 blocking")
        || lower.contains("none blocking")
    {
        return false;
    }
    // Find a "blocking" occurrence that is not part of "non-blocking".
    let bytes = lower.as_bytes();
    let needle = b"blocking";
    let mut i = 0;
    while let Some(pos) = lower[i..].find("blocking") {
        let abs = i + pos;
        // Preceded by "non-" or "non "? then it's "non-blocking".
        let is_non = abs >= 4 && (&bytes[abs - 4..abs] == b"non-" || &bytes[abs - 4..abs] == b"non ");
        if !is_non {
            return true;
        }
        i = abs + needle.len();
    }
    false
}

/// Receives per-review usage for cost/telemetry. Stub seam — desktop
/// composition root wires a real `cost`/`telemetry` sink later.
pub trait ReviewUsageSink: Send + Sync {
    /// Record one review's usage and wall-clock duration.
    fn record_review_usage(&self, reviewer_id: &str, target_id: &str, usage: &cost::Usage, duration: Duration);
}

/// No-op [`ReviewUsageSink`].
#[derive(Debug, Default, Clone, Copy)]
pub struct NullReviewUsageSink;

impl ReviewUsageSink for NullReviewUsageSink {
    fn record_review_usage(&self, _r: &str, _t: &str, _u: &cost::Usage, _d: Duration) {}
}

/// Drives cross-review rounds for a dual-LLM run.
///
/// Each reviewer is a single-shot [`SideQueryClient`] call with an empty tool
/// set (read-only by construction). The reviewer model is the *reviewing
/// candidate's own* model — i.e. candidate-a reviews candidate-b using
/// candidate-a's configured model.
pub struct CrossReviewer<'a> {
    client: Arc<dyn SideQueryClient>,
    store: &'a ArtifactStore,
    usage: Arc<dyn ReviewUsageSink>,
    /// Per-review (single-pass) timeout — the `CrossReview` phase timeout.
    review_timeout: Duration,
}

impl<'a> CrossReviewer<'a> {
    /// Construct a cross-reviewer.
    #[must_use]
    pub fn new(
        client: Arc<dyn SideQueryClient>,
        store: &'a ArtifactStore,
        usage: Arc<dyn ReviewUsageSink>,
        review_timeout: Duration,
    ) -> Self {
        Self { client, store, usage, review_timeout }
    }

    /// Whether review should run at all under `cfg`.
    ///
    /// `max_review_rounds == 0` (the single source of truth) OR
    /// `cross_review == false` skips the entire cross-review phase.
    #[must_use]
    pub fn enabled(cfg: &ReviewConfig) -> bool {
        cfg.cross_review && cfg.max_review_rounds > 0
    }

    /// Run one review: `reviewer` reviews `target` for `round` (1-based), using
    /// `reviewer`'s configured `reviewer_model`.
    ///
    /// Read-only is enforced by passing an empty `tools` list to the
    /// side-query client (the reviewer literally cannot call a write tool).
    ///
    /// On timeout or provider error the review is recorded as
    /// [`ReviewOutcome::Missing`] and a `*.error.md` marker is written; the run
    /// continues (design doc §Review/revision failure handling). An
    /// **artifact write failure** for the produced/error document is fatal for
    /// the review phase and propagates.
    pub async fn review_one(
        &self,
        reviewer_id: &str,
        reviewer_model: &str,
        target: &ReviewTarget,
        task_brief: &str,
        round: u32,
    ) -> Result<ReviewOutcome, MultiAgentError> {
        let start = std::time::Instant::now();
        let prompt = build_review_prompt(task_brief, target);
        let request = SideQueryRequest {
            model: reviewer_model.to_string(),
            system_prompt: Some(crate::prompts::REVIEWER.to_string()),
            messages: vec![ConversationMessage::user(MessageId::new(), prompt)],
            // EMPTY tools => read-only by construction (design doc §安全约束 #2).
            tools: Vec::new(),
            tool_choice: None,
            output_format: None,
            max_tokens: REVIEW_MAX_TOKENS,
            max_retries: 0,
            temperature: None,
            thinking_budget: None,
            stop_sequences: Vec::new(),
            query_source: QuerySource::Custom("multi-agent-review".to_string()),
            skip_system_prompt_prefix: true,
        };

        let call = self.client.query(request);
        let result = match tokio::time::timeout(self.review_timeout, call).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(api_err)) => Err(MultiAgentError::ReviewFailed {
                reviewer_id: reviewer_id.to_string(),
                target_id: target.candidate_id.clone(),
                round,
                reason: api_err.to_string(),
            }),
            Err(_elapsed) => Err(MultiAgentError::ReviewTimedOut {
                reviewer_id: reviewer_id.to_string(),
                target_id: target.candidate_id.clone(),
                round,
            }),
        };

        match result {
            Ok(resp) => {
                let body = resp
                    .text
                    .clone()
                    .or_else(|| resp.structured.as_ref().map(|v| v.to_string()))
                    .unwrap_or_default();
                self.usage
                    .record_review_usage(reviewer_id, &target.candidate_id, &resp.usage, start.elapsed());

                if body.trim().is_empty() {
                    // An empty review is not useful: record it as a missing
                    // review (recoverable) and continue.
                    let err = MultiAgentError::ReviewFailed {
                        reviewer_id: reviewer_id.to_string(),
                        target_id: target.candidate_id.clone(),
                        round,
                        reason: "reviewer returned empty body".to_string(),
                    };
                    self.store.write_review_error(
                        reviewer_id,
                        &target.candidate_id,
                        round,
                        &format!("# review failed\n\n{err}\n"),
                    )?;
                    return Ok(ReviewOutcome::Missing {
                        reviewer_id: reviewer_id.to_string(),
                        target_id: target.candidate_id.clone(),
                        round,
                        error: err,
                    });
                }

                let has_blocking = body_has_blocking(&body);
                // Persisting the review doc is fatal-for-review on failure.
                self.store
                    .write_review(reviewer_id, &target.candidate_id, round, &body)?;
                Ok(ReviewOutcome::Produced(ReviewDocument {
                    reviewer_id: reviewer_id.to_string(),
                    target_id: target.candidate_id.clone(),
                    round,
                    body,
                    has_blocking,
                }))
            }
            Err(err) => {
                // Recoverable: write a `*.error.md` marker and continue.
                self.store.write_review_error(
                    reviewer_id,
                    &target.candidate_id,
                    round,
                    &format!("# review failed in {}\n\n{err}\n", DualLlmPhase::CrossReview.as_str()),
                )?;
                Ok(ReviewOutcome::Missing {
                    reviewer_id: reviewer_id.to_string(),
                    target_id: target.candidate_id.clone(),
                    round,
                    error: err,
                })
            }
        }
    }
}

/// Build the user prompt fed to the reviewer: task brief + the competing
/// implementation's diff + self-report. The reviewer never sees its own work
/// (it is reviewing the *other* candidate).
#[must_use]
fn build_review_prompt(task_brief: &str, target: &ReviewTarget) -> String {
    format!(
        "## Task brief\n\n{task_brief}\n\n## Competing implementation: {id}\n\n\
         ### Self-report\n\n{report}\n\n### Unified diff\n\n```diff\n{diff}\n```\n",
        task_brief = task_brief,
        id = target.candidate_id,
        report = target.self_report,
        diff = target.patch_diff,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sidequery::side_query::SideQueryError;
    use sidequery::side_query::SideQueryResponse;
    use std::sync::Mutex;

    /// A scriptable side-query client. Records the requests it saw and returns
    /// a scripted response (or hangs / errors).
    enum Reply {
        Text(String),
        Err(String),
        /// Block forever (until the host timeout fires).
        Hang,
    }

    struct MockClient {
        replies: Mutex<Vec<Reply>>,
        seen: Mutex<Vec<SideQueryRequest>>,
    }

    impl MockClient {
        fn new(replies: Vec<Reply>) -> Self {
            Self { replies: Mutex::new(replies), seen: Mutex::new(Vec::new()) }
        }
    }

    #[async_trait]
    impl SideQueryClient for MockClient {
        async fn query(&self, request: SideQueryRequest) -> Result<SideQueryResponse, SideQueryError> {
            self.seen.lock().unwrap().push(request);
            let reply = {
                let mut g = self.replies.lock().unwrap();
                if g.is_empty() {
                    Reply::Text("no blocking findings".into())
                } else {
                    g.remove(0)
                }
            };
            match reply {
                Reply::Text(t) => Ok(SideQueryResponse {
                    text: Some(t),
                    structured: None,
                    tool_calls: Vec::new(),
                    usage: cost::Usage::default(),
                    stop_reason: Some("end_turn".into()),
                }),
                Reply::Err(e) => Err(SideQueryError::InvalidResponse(e)),
                Reply::Hang => {
                    futures_pending().await;
                    unreachable!()
                }
            }
        }
    }

    // A future that never resolves (so the host timeout must fire).
    async fn futures_pending() {
        std::future::pending::<()>().await
    }

    fn target() -> ReviewTarget {
        ReviewTarget {
            candidate_id: "candidate-b".into(),
            patch_diff: "diff --git a/x.rs b/x.rs\n+line\n".into(),
            self_report: "did the thing".into(),
        }
    }

    #[test]
    fn blocking_detection() {
        assert!(body_has_blocking("Finding 1: blocking — missing null check"));
        assert!(!body_has_blocking("All findings are non-blocking suggestions"));
        assert!(!body_has_blocking("No blocking findings. Two suggestions follow."));
        assert!(!body_has_blocking("nothing here"));
        assert!(body_has_blocking("non-blocking: x\nblocking: y"));
    }

    #[test]
    fn enabled_gating() {
        let mut cfg = ReviewConfig::default();
        assert!(CrossReviewer::enabled(&cfg));
        cfg.max_review_rounds = 0;
        assert!(!CrossReviewer::enabled(&cfg), "maxReviewRounds=0 skips review");
        cfg.max_review_rounds = 1;
        cfg.cross_review = false;
        assert!(!CrossReviewer::enabled(&cfg));
    }

    #[tokio::test]
    async fn reviewer_is_read_only_empty_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path(), "RUN");
        store.init().unwrap();
        let client = Arc::new(MockClient::new(vec![Reply::Text("blocking: bug".into())]));
        let reviewer = CrossReviewer::new(
            client.clone(),
            &store,
            Arc::new(NullReviewUsageSink),
            Duration::from_secs(5),
        );

        let out = reviewer
            .review_one("candidate-a", "profile-a/model-a", &target(), "brief", 1)
            .await
            .unwrap();
        let doc = out.document().expect("produced");
        assert!(doc.has_blocking);
        assert_eq!(doc.target_id, "candidate-b");

        // The side-query was issued with an EMPTY tool set — read-only by
        // construction (the reviewer literally cannot call a write tool).
        let seen = client.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].tools.is_empty(), "reviewer must get no tools");
        assert!(seen[0].tool_choice.is_none());

        // The review doc is written; no candidate worktree is touched.
        let p = store.paths().review_doc("candidate-a", "candidate-b", 1);
        assert!(p.is_file());
    }

    #[tokio::test]
    async fn review_timeout_writes_error_md_and_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path(), "RUN");
        store.init().unwrap();
        let client = Arc::new(MockClient::new(vec![Reply::Hang]));
        let reviewer = CrossReviewer::new(
            client,
            &store,
            Arc::new(NullReviewUsageSink),
            Duration::from_millis(50),
        );

        let out = reviewer
            .review_one("candidate-a", "profile-a/model-a", &target(), "brief", 1)
            .await
            .expect("timeout is recoverable, returns Ok(Missing)");
        match out {
            ReviewOutcome::Missing { error, .. } => {
                assert!(matches!(error, MultiAgentError::ReviewTimedOut { .. }));
            }
            other => panic!("expected Missing, got {other:?}"),
        }
        // `.error.md` marker written; no success doc.
        assert!(store
            .paths()
            .review_error_doc("candidate-a", "candidate-b", 1)
            .is_file());
        assert!(!store
            .paths()
            .review_doc("candidate-a", "candidate-b", 1)
            .is_file());
    }

    #[tokio::test]
    async fn provider_error_writes_error_md_and_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path(), "RUN");
        store.init().unwrap();
        let client = Arc::new(MockClient::new(vec![Reply::Err("502 bad gateway".into())]));
        let reviewer = CrossReviewer::new(
            client,
            &store,
            Arc::new(NullReviewUsageSink),
            Duration::from_secs(5),
        );

        let out = reviewer
            .review_one("candidate-a", "profile-a/model-a", &target(), "brief", 1)
            .await
            .unwrap();
        assert!(matches!(
            out,
            ReviewOutcome::Missing { error: MultiAgentError::ReviewFailed { .. }, .. }
        ));
        assert!(store
            .paths()
            .review_error_doc("candidate-a", "candidate-b", 1)
            .is_file());
    }
}
