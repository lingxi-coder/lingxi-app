//! Candidate worktree provisioning (design doc §Worktree 隔离, Phase 3).
//!
//! Wraps [`traits::worktree::WorktreeManager`] to create the two isolated
//! candidate worktrees for a dual-LLM run.
//!
//! Hard rules enforced here:
//!
//! - **Direct `create_worktree`, no degrade.** We call
//!   [`WorktreeManager::create_worktree`] directly and treat any failure as
//!   **fatal** ([`MultiAgentError::Worktree`]). We deliberately do NOT use
//!   `agent::worktree_policy::create_worktree_or_degrade`, which falls back to
//!   the current cwd when a worktree cannot be created — for dual-LLM that
//!   would mean both candidates writing the main workspace, defeating the
//!   isolation guarantee.
//! - **Sequential creation.** Concurrent `git worktree add` under one repo
//!   races the main `.git` lock, so candidate-a and candidate-b worktrees are
//!   created one after another. (Candidate *execution* can then run in
//!   parallel — each worktree has its own index.)
//! - **Slug shape.** `multi-agent/<run_id>/<candidate_id>`, with `run_id` the
//!   first [`RUN_ID_LEN`] chars of a ULID. The platform worktree layer flattens
//!   `/`→`+` and enforces a 64-char cap ([`MAX_WORKTREE_SLUG_LENGTH`]).

use crate::error::MultiAgentError;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use traits::WorktreeHandle;
use traits::WorktreeManager;

/// Maximum total slug length the worktree layer accepts. Mirrors
/// `MAX_WORKTREE_SLUG_LENGTH` in `tools/worktree` and the platform worktree
/// crates (kept local so `multi-agent` need not depend on a platform crate).
pub const MAX_WORKTREE_SLUG_LENGTH: usize = 64;

/// Number of leading ULID characters used as a run id. Keeps the full slug
/// `multi-agent/<run_id>/<candidate_id>` comfortably under
/// [`MAX_WORKTREE_SLUG_LENGTH`].
pub const RUN_ID_LEN: usize = 12;

/// Fixed slug prefix for every dual-LLM worktree.
pub const SLUG_PREFIX: &str = "multi-agent";

/// Build the worktree slug for `candidate_id` within `run_id`.
///
/// Shape: `multi-agent/<run_id>/<candidate_id>`. The platform layer flattens
/// `/`→`+` (so the on-disk dir is `multi-agent+<run_id>+<candidate_id>`) and
/// rejects slugs over [`MAX_WORKTREE_SLUG_LENGTH`].
#[must_use]
pub fn candidate_slug(run_id: &str, candidate_id: &str) -> String {
    format!("{SLUG_PREFIX}/{run_id}/{candidate_id}")
}

/// Generate a fresh run id: the first [`RUN_ID_LEN`] characters of a ULID.
///
/// ULID = 48-bit millisecond timestamp + 80 bits of entropy, Crockford
/// base32, uppercase — every character is filename-safe and slug-legal. We
/// take only the leading [`RUN_ID_LEN`] chars (timestamp + a little entropy)
/// to keep the worktree slug short, per the design doc.
#[must_use]
pub fn new_run_id() -> String {
    let ulid = generate_ulid();
    ulid.chars().take(RUN_ID_LEN).collect()
}

/// Crockford base32 alphabet (no I, L, O, U), per the ULID spec.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Generate a 26-character ULID string.
///
/// Encodes a 128-bit value: the high 48 bits are the current Unix time in
/// milliseconds, the low 80 bits are entropy. Without a `rand` dependency in
/// this workspace, the entropy is sourced from sub-millisecond clock bits, a
/// process-monotonic atomic counter, and address-space jitter — sufficient to
/// keep concurrent run ids distinct, which is all a slug component needs.
fn generate_ulid() -> String {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let ms = u128::from(now.as_millis()) & ((1u128 << 48) - 1);

    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    // Address-space jitter as a cheap entropy source (no rand dep).
    let jitter = {
        let local = 0u8;
        std::ptr::addr_of!(local) as u64
    };
    // Place the monotonic counter in the *top* bits of the 80-bit entropy
    // field so it lands inside the leading `RUN_ID_LEN` characters: this
    // guarantees consecutive `new_run_id()` results differ even within the
    // same millisecond. Bit layout of the 128-bit value: ms is bits 80..127,
    // entropy is bits 0..79. The leading 12 base32 chars cover value bits
    // 70..127, so only entropy bits 70..79 (the top 10 entropy bits) reach the
    // prefix — the counter must sit there (`<< 70`) to perturb the prefix on
    // every increment. Lower bits mix sub-millisecond nanos and address jitter.
    let entropy = (u128::from(counter) << 70)
        ^ u128::from(now.subsec_nanos())
        ^ (u128::from(jitter) << 8);
    let entropy = entropy & ((1u128 << 80) - 1);

    let value = (ms << 80) | entropy;

    // 128 bits → 26 base32 chars (130 bits, top 2 padding bits are 0).
    let mut out = [0u8; 26];
    let mut v = value;
    for slot in out.iter_mut().rev() {
        *slot = CROCKFORD[(v & 0x1f) as usize];
        v >>= 5;
    }
    String::from_utf8(out.to_vec()).expect("crockford alphabet is ASCII")
}

/// The two candidate worktree handles for a run, in creation order.
#[derive(Debug, Clone)]
pub struct CandidateWorktrees {
    /// The run id these worktrees belong to.
    pub run_id: String,
    /// `(candidate_id, handle)` for the first candidate.
    pub first: (String, WorktreeHandle),
    /// `(candidate_id, handle)` for the second candidate.
    pub second: (String, WorktreeHandle),
}

impl CandidateWorktrees {
    /// Iterate both `(candidate_id, handle)` pairs in creation order.
    pub fn iter(&self) -> impl Iterator<Item = &(String, WorktreeHandle)> {
        std::iter::once(&self.first).chain(std::iter::once(&self.second))
    }
}

/// Provisions and tears down candidate worktrees over a [`WorktreeManager`].
pub struct WorktreeProvisioner<'m> {
    manager: &'m dyn WorktreeManager,
}

impl<'m> WorktreeProvisioner<'m> {
    /// Wrap a [`WorktreeManager`].
    #[must_use]
    pub fn new(manager: &'m dyn WorktreeManager) -> Self {
        Self { manager }
    }

    /// Create the two candidate worktrees for `run_id`, **sequentially**.
    ///
    /// `candidate_ids` must contain exactly two stable, filename-safe ids
    /// (the MVP arity; enforced by config). Any creation failure is fatal and
    /// surfaces as [`MultiAgentError::Worktree`] — there is no degrade to the
    /// main workspace. If the second creation fails, the already-created first
    /// worktree is best-effort removed before returning, so a failed run does
    /// not leak a half-provisioned pair.
    ///
    /// `base_branch` and `copy_includes` are forwarded to the manager as-is.
    pub async fn create_candidates(
        &self,
        run_id: &str,
        candidate_ids: [&str; 2],
        base_branch: Option<&str>,
        copy_includes: &[std::path::PathBuf],
    ) -> Result<CandidateWorktrees, MultiAgentError> {
        let [id_a, id_b] = candidate_ids;

        // Candidate A first.
        let slug_a = candidate_slug(run_id, id_a);
        let handle_a = self
            .manager
            .create_worktree(&slug_a, base_branch, copy_includes)
            .await
            .map_err(MultiAgentError::Worktree)?;

        // Candidate B next — strictly after A, never concurrently (shared
        // `.git` lock). On failure, unwind A so we don't leak a worktree.
        let slug_b = candidate_slug(run_id, id_b);
        let handle_b = match self
            .manager
            .create_worktree(&slug_b, base_branch, copy_includes)
            .await
        {
            Ok(h) => h,
            Err(e) => {
                // Best-effort cleanup of the already-created A worktree.
                let _ = self.manager.remove_worktree(&handle_a).await;
                return Err(MultiAgentError::Worktree(e));
            }
        };

        Ok(CandidateWorktrees {
            run_id: run_id.to_string(),
            first: (id_a.to_string(), handle_a),
            second: (id_b.to_string(), handle_b),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;
    use traits::WorktreeError;
    use traits::WorktreeInfo;

    /// Records creation order and lets a test script a failure for the Nth
    /// `create_worktree` call (1-based). Also records `remove_worktree` calls
    /// so the unwind path can be asserted.
    #[derive(Default)]
    struct RecordingManager {
        inner: Mutex<Inner>,
    }

    #[derive(Default)]
    struct Inner {
        created_slugs: Vec<String>,
        removed: Vec<WorktreeHandle>,
        /// 1-based call index that should fail, if any.
        fail_on_call: Option<usize>,
        call_count: usize,
        /// When true, a second create that starts before the first finished
        /// would be observed (we assert it never happens).
        in_create: bool,
        concurrency_violation: bool,
    }

    impl RecordingManager {
        fn fail_on(call: usize) -> Self {
            let m = Self::default();
            m.inner.lock().unwrap().fail_on_call = Some(call);
            m
        }
        fn created_slugs(&self) -> Vec<String> {
            self.inner.lock().unwrap().created_slugs.clone()
        }
        fn removed(&self) -> Vec<WorktreeHandle> {
            self.inner.lock().unwrap().removed.clone()
        }
        fn concurrency_violation(&self) -> bool {
            self.inner.lock().unwrap().concurrency_violation
        }
    }

    #[async_trait]
    impl WorktreeManager for RecordingManager {
        async fn create_worktree(
            &self,
            slug: &str,
            _base_branch: Option<&str>,
            _copy_includes: &[PathBuf],
        ) -> Result<WorktreeHandle, WorktreeError> {
            // Detect overlap: if another create is in flight, flag it.
            {
                let mut g = self.inner.lock().unwrap();
                if g.in_create {
                    g.concurrency_violation = true;
                }
                g.in_create = true;
                g.call_count += 1;
            }
            // Yield to give any racing task a chance to overlap (it must not).
            tokio::task::yield_now().await;

            let (this_call, fail) = {
                let g = self.inner.lock().unwrap();
                (g.call_count, g.fail_on_call)
            };

            let result = if fail == Some(this_call) {
                Err(WorktreeError::Git(format!("scripted failure on call {this_call}")))
            } else {
                // Mirror production slug validation/flatten so length/char
                // rules are exercised end-to-end.
                validate_slug(slug)?;
                let flat = slug.replace('/', "+");
                Ok(WorktreeHandle {
                    path: PathBuf::from("/tmp/mock-repo/.claude/worktrees").join(&flat),
                    branch_name: format!("worktree-{flat}"),
                })
            };

            {
                let mut g = self.inner.lock().unwrap();
                g.in_create = false;
                if let Ok(ref h) = result {
                    let _ = h;
                    g.created_slugs.push(slug.to_string());
                }
            }
            result
        }

        async fn remove_worktree(&self, handle: &WorktreeHandle) -> Result<(), WorktreeError> {
            self.inner.lock().unwrap().removed.push(handle.clone());
            Ok(())
        }

        async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError> {
            Ok(Vec::new())
        }

        async fn cleanup_stale(
            &self,
            _max_age: Duration,
        ) -> Result<Vec<PathBuf>, WorktreeError> {
            Ok(Vec::new())
        }

        fn is_supported(&self) -> bool {
            true
        }
    }

    // Local mirror of the platform slug validator (length + flatten rules).
    fn validate_slug(slug: &str) -> Result<(), WorktreeError> {
        if slug.is_empty() {
            return Err(WorktreeError::InvalidSlug("slug is empty".into()));
        }
        if slug.len() > MAX_WORKTREE_SLUG_LENGTH {
            return Err(WorktreeError::InvalidSlug(format!(
                "slug exceeds {MAX_WORKTREE_SLUG_LENGTH} chars (got {})",
                slug.len()
            )));
        }
        for segment in slug.split('/') {
            if segment.is_empty() {
                return Err(WorktreeError::InvalidSlug("empty segment".into()));
            }
            for ch in segment.chars() {
                let ok = ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-';
                if !ok {
                    return Err(WorktreeError::InvalidSlug(format!("bad char {ch:?}")));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn run_id_is_short_and_slug_safe() {
        let run_id = new_run_id();
        assert_eq!(run_id.len(), RUN_ID_LEN);
        assert!(run_id.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn run_ids_are_distinct() {
        let a = new_run_id();
        let b = new_run_id();
        assert_ne!(a, b, "consecutive run ids must differ");
    }

    #[test]
    fn slug_shape_and_flatten() {
        let slug = candidate_slug("01HX9ABCD12X", "candidate-a");
        assert_eq!(slug, "multi-agent/01HX9ABCD12X/candidate-a");
        assert_eq!(slug.replace('/', "+"), "multi-agent+01HX9ABCD12X+candidate-a");
    }

    #[test]
    fn full_slug_fits_under_max_length() {
        // Worst-case realistic ids: 12-char run id + a long candidate id.
        let run_id = "01HX9ABCD12X";
        let slug = candidate_slug(run_id, "candidate-a");
        assert!(
            slug.len() <= MAX_WORKTREE_SLUG_LENGTH,
            "slug {:?} len {} exceeds {}",
            slug,
            slug.len(),
            MAX_WORKTREE_SLUG_LENGTH
        );
    }

    #[tokio::test]
    async fn creates_both_sequentially_in_order() {
        let mgr = RecordingManager::default();
        let prov = WorktreeProvisioner::new(&mgr);
        let wts = prov
            .create_candidates("RUN12345678X", ["candidate-a", "candidate-b"], None, &[])
            .await
            .expect("both worktrees created");

        assert_eq!(wts.first.0, "candidate-a");
        assert_eq!(wts.second.0, "candidate-b");
        assert_eq!(
            mgr.created_slugs(),
            vec![
                "multi-agent/RUN12345678X/candidate-a".to_string(),
                "multi-agent/RUN12345678X/candidate-b".to_string(),
            ],
            "candidates created in order, sequentially"
        );
        assert!(
            !mgr.concurrency_violation(),
            "the two create_worktree calls must not overlap"
        );
        assert_eq!(wts.iter().count(), 2);
    }

    #[tokio::test]
    async fn first_worktree_failure_is_fatal_no_degrade() {
        let mgr = RecordingManager::fail_on(1);
        let prov = WorktreeProvisioner::new(&mgr);
        let err = prov
            .create_candidates("RUN", ["a", "b"], None, &[])
            .await
            .expect_err("first creation failure must be fatal");
        assert!(matches!(err, MultiAgentError::Worktree(_)));
        // No degrade, and nothing was created.
        assert!(mgr.created_slugs().is_empty());
    }

    #[tokio::test]
    async fn second_worktree_failure_is_fatal_and_unwinds_first() {
        let mgr = RecordingManager::fail_on(2);
        let prov = WorktreeProvisioner::new(&mgr);
        let err = prov
            .create_candidates("RUN", ["a", "b"], None, &[])
            .await
            .expect_err("second creation failure must be fatal");
        assert!(matches!(err, MultiAgentError::Worktree(_)));
        // First was created then unwound; never degrades to cwd.
        assert_eq!(mgr.created_slugs(), vec!["multi-agent/RUN/a".to_string()]);
        assert_eq!(mgr.removed().len(), 1, "the first worktree must be cleaned up");
    }

    #[tokio::test]
    async fn oversized_slug_is_rejected_as_worktree_error() {
        let mgr = RecordingManager::default();
        let prov = WorktreeProvisioner::new(&mgr);
        // A pathologically long candidate id pushes the slug past the cap.
        let long_id = "x".repeat(80);
        let err = prov
            .create_candidates("RUN", [long_id.as_str(), "b"], None, &[])
            .await
            .expect_err("over-length slug must fail");
        match err {
            MultiAgentError::Worktree(WorktreeError::InvalidSlug(_)) => {}
            other => panic!("expected InvalidSlug worktree error, got {other:?}"),
        }
    }
}
