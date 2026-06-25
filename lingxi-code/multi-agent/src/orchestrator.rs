//! Dual-LLM candidate-implementation run loop (design doc §执行状态机,
//! §Agent 运行方式, §超时与错误处理, Phase 4).
//!
//! This module drives the early phases of a [`DualLlmPhase`] run: build the
//! task brief, create the two isolated candidate worktrees (serial), then run
//! both candidate implementations **concurrently**, each with its working
//! directory pinned to its own worktree. Cross-review / revision / arbitration
//! / finalizer / verification are later phases and are left as explicit
//! `// TODO(multi-agent):` seams here.
//!
//! ## Trait seams
//!
//! - [`CandidateRunner`] abstracts "run one candidate implementation in a
//!   worktree". [`MockCandidateRunner`] is the test double. The real
//!   `llm-client` / `agent` subagent loop is a clearly-marked seam
//!   ([`TodoLlmCandidateRunner`]) so this crate compiles before that
//!   integration lands.
//! - [`UsageSink`] receives per-candidate cost/telemetry usage. A no-op stub
//!   ([`NullUsageSink`]) is provided; the desktop composition root will wire a
//!   real `cost` / `telemetry` sink later.
//!
//! ## Isolation + safety (enforced here, not by prompt)
//!
//! - Each candidate's [`CandidateRunContext::cwd`] is its own worktree path; a
//!   candidate must only write under that path (design doc §安全约束 #1).
//! - Worktrees are created **sequentially** (shared `.git` lock) then candidates
//!   run in parallel (independent indices).
//! - A candidate timeout/failure does NOT fail the whole run if the other
//!   candidate is viable; both failing → run failed with artifacts retained
//!   (design doc §Candidate failure handling).
//! - Cancellation propagates a [`CancellationToken`] to in-flight candidates and
//!   retains artifacts (design doc §安全约束 #7).

use crate::artifacts::ArtifactStore;
use crate::config::AgentEndpoint;
use crate::config::MultiAgentConfig;
use crate::error::MultiAgentError;
use crate::providers::ModelResolver;
use crate::providers::ResolvedCandidate;
use crate::state::DualLlmPhase;
use crate::worktrees::new_run_id;
use crate::worktrees::WorktreeProvisioner;
use async_trait::async_trait;
use llm_client::TokenUsage;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use traits::WorktreeHandle;
use traits::WorktreeManager;

/// What a candidate produced in its worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateOutcome {
    /// The unified diff the candidate produced (`patch.diff` artifact).
    pub patch_diff: String,
    /// The candidate's self-report (`self-report.md` artifact).
    pub self_report: String,
    /// Token usage reported for the candidate's run (for cost/telemetry).
    pub usage: TokenUsage,
}

/// Inputs handed to a [`CandidateRunner`] for one candidate.
#[derive(Debug, Clone)]
pub struct CandidateRunContext {
    /// Stable candidate id (e.g. `candidate-a`).
    pub candidate_id: String,
    /// The shared task brief.
    pub task_brief: String,
    /// The candidate's **isolated** working directory (its worktree). The
    /// candidate must only write under this path.
    pub cwd: PathBuf,
    /// The resolved model route (provider id + wire model).
    pub resolved: ResolvedCandidate,
    /// Cooperative cancellation: runners should observe this and stop promptly.
    pub cancel: CancellationToken,
}

/// Runs a single candidate implementation inside its assigned worktree.
///
/// Implementations MUST confine all writes to [`CandidateRunContext::cwd`].
#[async_trait]
pub trait CandidateRunner: Send + Sync {
    /// Implement the task in `ctx.cwd`, returning the patch + self-report.
    async fn run(&self, ctx: &CandidateRunContext) -> Result<CandidateOutcome, MultiAgentError>;
}

/// Receives per-candidate usage for cost + telemetry. Stub seam for Phase 4 —
/// the desktop composition root wires a real `cost` / `telemetry` sink later.
pub trait UsageSink: Send + Sync {
    /// Record one candidate's usage and wall-clock duration.
    fn record_candidate_usage(&self, candidate_id: &str, usage: &TokenUsage, duration: Duration);
}

/// No-op [`UsageSink`].
#[derive(Debug, Default, Clone, Copy)]
pub struct NullUsageSink;

impl UsageSink for NullUsageSink {
    fn record_candidate_usage(
        &self,
        _candidate_id: &str,
        _usage: &TokenUsage,
        _duration: Duration,
    ) {
    }
}

// TODO(multi-agent): real candidate runner over `llm-client` + the `agent`
// subagent multi-turn loop. It must:
//   1. construct an `agent` runner whose cwd is `ctx.cwd` (its worktree),
//   2. drive a tool loop with the same permission/sandbox policy as the main
//      session (or stricter — design doc §安全约束 #4),
//   3. honor `ctx.cancel` to abort subprocesses on cancellation,
//   4. produce `patch.diff` (git diff of the worktree) + a structured
//      self-report, and surface real `TokenUsage`.
// Full real-loop integration may exceed a single pass; this is a compiling
// seam so the crate stays green until that lands.
/// Placeholder real runner. Always returns [`MultiAgentError::CandidateFailed`]
/// with a `not yet wired` reason; replaced by the real `llm-client`/`agent`
/// loop in a later pass.
#[derive(Debug, Default, Clone, Copy)]
pub struct TodoLlmCandidateRunner;

#[async_trait]
impl CandidateRunner for TodoLlmCandidateRunner {
    async fn run(&self, ctx: &CandidateRunContext) -> Result<CandidateOutcome, MultiAgentError> {
        Err(MultiAgentError::CandidateFailed {
            candidate_id: ctx.candidate_id.clone(),
            phase: DualLlmPhase::ImplementCandidates,
            reason: "real llm-client/agent candidate runner not yet wired (TODO(multi-agent))"
                .to_string(),
        })
    }
}

/// Per-candidate result of the implementation phase.
#[derive(Debug)]
pub struct CandidateResult {
    /// Stable candidate id.
    pub candidate_id: String,
    /// The candidate's worktree handle (retained on failure/cancel).
    pub worktree: WorktreeHandle,
    /// `Ok` with the produced outcome, or `Err` describing the failure
    /// (timeout, runner error, empty diff, …). Recoverable per design doc.
    pub outcome: Result<CandidateOutcome, MultiAgentError>,
}

impl CandidateResult {
    /// Whether this candidate produced a viable (non-error) outcome.
    #[must_use]
    pub fn is_viable(&self) -> bool {
        self.outcome.is_ok()
    }
}

/// Result of the candidate-implementation run (Phase 4 scope).
#[derive(Debug)]
pub struct ImplementationRun {
    /// The run id (also the worktree slug + artifact dir component).
    pub run_id: String,
    /// Both candidate results, in creation order.
    pub candidates: Vec<CandidateResult>,
}

impl ImplementationRun {
    /// True when at least one candidate produced a viable outcome (the run may
    /// proceed to arbitration). False → both failed (run failed; artifacts kept).
    #[must_use]
    pub fn any_viable(&self) -> bool {
        self.candidates.iter().any(CandidateResult::is_viable)
    }
}

/// Drives the candidate-implementation phases of a dual-LLM run.
///
/// Holds borrowed collaborators: the worktree manager, the model resolver, the
/// candidate runner, and a usage sink. Constructed per run.
pub struct DualLlm<'a> {
    manager: &'a dyn WorktreeManager,
    resolver: &'a ModelResolver,
    runner: Arc<dyn CandidateRunner>,
    usage: Arc<dyn UsageSink>,
    config: &'a MultiAgentConfig,
    repo_root: PathBuf,
}

impl<'a> DualLlm<'a> {
    /// Construct a run driver.
    #[must_use]
    pub fn new(
        manager: &'a dyn WorktreeManager,
        resolver: &'a ModelResolver,
        runner: Arc<dyn CandidateRunner>,
        usage: Arc<dyn UsageSink>,
        config: &'a MultiAgentConfig,
        repo_root: impl AsRef<Path>,
    ) -> Self {
        Self {
            manager,
            resolver,
            runner,
            usage,
            config,
            repo_root: repo_root.as_ref().to_path_buf(),
        }
    }

    /// Run BuildTaskBrief → CreateWorktrees → ImplementCandidates.
    ///
    /// `cancel` is the run-level cancellation token; it is propagated to both
    /// candidates. The whole-run wall-clock timeout
    /// ([`MultiAgentConfig::limits`].`timeout`) wraps the implementation phase;
    /// firing it cancels both candidates and returns
    /// [`MultiAgentError::RunTimedOut`] (artifacts are flushed first, worktrees
    /// retained).
    ///
    /// Fatal failures (config: not exactly 2 candidates; provider unresolvable;
    /// worktree creation) fail fast. Candidate-level failures/timeouts are
    /// recorded per-candidate; the run only "fails" (returns an
    /// [`ImplementationRun`] with no viable candidate) when **both** fail.
    pub async fn run_implementation(
        &self,
        task_brief: &str,
        cancel: CancellationToken,
    ) -> Result<ImplementationRun, MultiAgentError> {
        // --- Fatal config gate: exactly 2 candidates (design doc §Error taxonomy). ---
        let [cand_a, cand_b] = match self.config.candidates.as_slice() {
            [a, b] => [a, b],
            other => {
                return Err(MultiAgentError::Config(
                    crate::config::ConfigError::CandidateCount { found: other.len() },
                ))
            }
        };

        // --- Fatal provider gate: resolve both model refs before any work. ---
        let resolved_a = self.resolver.resolve_endpoint(cand_a)?;
        let resolved_b = self.resolver.resolve_endpoint(cand_b)?;

        let run_id = new_run_id();

        // --- BuildTaskBrief: persist the brief. Artifact write is fatal. ---
        let store = ArtifactStore::new(&self.repo_root, &run_id);
        store.init()?;
        store.write_task_brief(task_brief)?;

        // --- CreateWorktrees: serial creation, no degrade (fatal on failure). ---
        let provisioner = WorktreeProvisioner::new(self.manager);
        let worktrees = provisioner
            .create_candidates(
                &run_id,
                [cand_a.id.as_str(), cand_b.id.as_str()],
                None,
                &[],
            )
            .await?;

        // --- ImplementCandidates: run both concurrently, each in its worktree. ---
        let run_timeout = self.config.limits.timeout;
        let phase_timeout = self.config.limits.phase_timeout.implementation;

        let ctx_a = self.context_for(cand_a, resolved_a, task_brief, &worktrees.first.1, &cancel);
        let ctx_b = self.context_for(cand_b, resolved_b, task_brief, &worktrees.second.1, &cancel);

        let fut = async {
            // Concurrent execution; per-candidate phase timeout applied inside.
            let (out_a, out_b) = tokio::join!(
                self.run_one(ctx_a, phase_timeout),
                self.run_one(ctx_b, phase_timeout),
            );
            (out_a, out_b)
        };

        let (res_a, res_b) = match tokio::time::timeout(run_timeout, fut).await {
            Ok(pair) => pair,
            Err(_elapsed) => {
                // Run-level wall-clock timeout: cancel children, flush artifacts,
                // retain worktrees (do NOT clean), surface RunTimedOut.
                cancel.cancel();
                let _ = store.write_state(&serde_json::json!({
                    "run_id": run_id,
                    "phase": DualLlmPhase::ImplementCandidates.as_str(),
                    "result": "run_timed_out",
                }));
                return Err(MultiAgentError::RunTimedOut {
                    run_id,
                    phase: DualLlmPhase::ImplementCandidates,
                });
            }
        };

        // Persist each candidate's evidence. Artifact writes are fatal.
        self.persist_candidate(&store, &cand_a.id, &res_a)?;
        self.persist_candidate(&store, &cand_b.id, &res_b)?;

        let candidates = vec![
            CandidateResult {
                candidate_id: cand_a.id.clone(),
                worktree: worktrees.first.1.clone(),
                outcome: res_a,
            },
            CandidateResult {
                candidate_id: cand_b.id.clone(),
                worktree: worktrees.second.1.clone(),
                outcome: res_b,
            },
        ];

        let run = ImplementationRun { run_id: run_id.clone(), candidates };

        // Record terminal state. Both failed → run failed (artifacts retained;
        // worktrees retained for inspection — cleanup is a later phase). At
        // least one viable → continue (later: arbitration).
        let result_tag = if run.any_viable() {
            "candidates_implemented"
        } else {
            "failed_all_candidates"
        };
        store.write_state(&serde_json::json!({
            "run_id": run_id,
            "phase": DualLlmPhase::ImplementCandidates.as_str(),
            "result": result_tag,
            "viable": run.any_viable(),
        }))?;

        Ok(run)
    }

    fn context_for(
        &self,
        endpoint: &AgentEndpoint,
        resolved: ResolvedCandidate,
        task_brief: &str,
        worktree: &WorktreeHandle,
        cancel: &CancellationToken,
    ) -> CandidateRunContext {
        CandidateRunContext {
            candidate_id: endpoint.id.clone(),
            task_brief: task_brief.to_string(),
            cwd: worktree.path.clone(),
            resolved,
            // Child token so the run-level token cancels children, but a child
            // failing never cancels its sibling.
            cancel: cancel.child_token(),
        }
    }

    /// Run a single candidate with a per-candidate phase timeout, mapping
    /// timeout → [`MultiAgentError::CandidateTimedOut`] and an empty diff →
    /// [`MultiAgentError::CandidateFailed`] (design doc §Candidate failure
    /// handling). Usage is recorded on success.
    async fn run_one(
        &self,
        ctx: CandidateRunContext,
        phase_timeout: Duration,
    ) -> Result<CandidateOutcome, MultiAgentError> {
        let candidate_id = ctx.candidate_id.clone();
        let start = Instant::now();

        let outcome = match tokio::time::timeout(phase_timeout, self.runner.run(&ctx)).await {
            Ok(inner) => inner,
            Err(_elapsed) => {
                // Cancel just this candidate's subtasks; the sibling keeps going.
                ctx.cancel.cancel();
                return Err(MultiAgentError::CandidateTimedOut {
                    candidate_id,
                    phase: DualLlmPhase::ImplementCandidates,
                });
            }
        };

        let outcome = outcome?;

        // Empty diff is a candidate failure (design doc §Candidate failure handling).
        if outcome.patch_diff.trim().is_empty() {
            return Err(MultiAgentError::CandidateFailed {
                candidate_id,
                phase: DualLlmPhase::ImplementCandidates,
                reason: "empty diff".to_string(),
            });
        }

        // Enforce the changed-files cap if a count can be derived from the diff.
        let changed = count_changed_files(&outcome.patch_diff);
        if changed > self.config.limits.max_changed_files as usize {
            return Err(MultiAgentError::CandidateFailed {
                candidate_id,
                phase: DualLlmPhase::ImplementCandidates,
                reason: format!(
                    "diff too large: {changed} changed files exceeds limit {}",
                    self.config.limits.max_changed_files
                ),
            });
        }

        self.usage
            .record_candidate_usage(&candidate_id, &outcome.usage, start.elapsed());
        Ok(outcome)
    }

    /// Persist a candidate's evidence. On success: self-report + patch. On
    /// failure: a self-report describing the failure (so artifacts always carry
    /// the reason). Artifact write failures are fatal.
    fn persist_candidate(
        &self,
        store: &ArtifactStore,
        candidate_id: &str,
        result: &Result<CandidateOutcome, MultiAgentError>,
    ) -> Result<(), MultiAgentError> {
        match result {
            Ok(outcome) => {
                store.write_candidate_self_report(candidate_id, &outcome.self_report)?;
                store.write_candidate_patch(candidate_id, &outcome.patch_diff)?;
            }
            Err(e) => {
                store.write_candidate_self_report(
                    candidate_id,
                    &format!("# candidate {candidate_id} failed\n\n{e}\n"),
                )?;
            }
        }
        Ok(())
    }
}

/// Count distinct files touched by a unified diff by tallying `diff --git`
/// headers (falling back to `+++ ` target lines when no git headers exist).
/// Used only for the changed-files cap heuristic.
#[must_use]
fn count_changed_files(diff: &str) -> usize {
    let git_headers = diff.lines().filter(|l| l.starts_with("diff --git ")).count();
    if git_headers > 0 {
        return git_headers;
    }
    diff.lines()
        .filter(|l| l.starts_with("+++ ") && !l.starts_with("+++ /dev/null"))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentEndpoint;
    use crate::config::ArbiterConfig;
    use crate::config::LimitConfig;
    use crate::config::MultiAgentMode;
    use crate::config::MultiAgentStrategyKind;
    use crate::config::PhaseTimeouts;
    use crate::config::ReviewConfig;
    use crate::config::TriggerConfig;
    use llm_client::AuthStrategy;
    use llm_client::Capabilities;
    use llm_client::ClientConfig;
    use llm_client::CredentialConfig;
    use llm_client::ModelProfile;
    use llm_client::PricingConfig;
    use llm_client::ProtocolFamily;
    use llm_client::ProviderId;
    use llm_client::ProviderProfile;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use traits::worktree::WorktreeChangeSummary;
    use traits::WorktreeError;
    use traits::WorktreeInfo;

    // --- A real-filesystem worktree manager: each "worktree" is a temp dir. ---
    struct TempWorktreeManager {
        base: PathBuf,
        created: Mutex<Vec<WorktreeHandle>>,
    }

    impl TempWorktreeManager {
        fn new(base: PathBuf) -> Self {
            Self { base, created: Mutex::new(Vec::new()) }
        }
    }

    #[async_trait]
    impl WorktreeManager for TempWorktreeManager {
        async fn create_worktree(
            &self,
            slug: &str,
            _b: Option<&str>,
            _c: &[PathBuf],
        ) -> Result<WorktreeHandle, WorktreeError> {
            let flat = slug.replace('/', "+");
            let path = self.base.join("worktrees").join(&flat);
            std::fs::create_dir_all(&path)
                .map_err(|e| WorktreeError::Git(format!("mkdir {path:?}: {e}")))?;
            let h = WorktreeHandle { path, branch_name: format!("wt-{flat}") };
            self.created.lock().unwrap().push(h.clone());
            Ok(h)
        }
        async fn remove_worktree(&self, _h: &WorktreeHandle) -> Result<(), WorktreeError> {
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
            _h: &WorktreeHandle,
        ) -> Result<Option<WorktreeChangeSummary>, WorktreeError> {
            Ok(None)
        }
    }

    fn model(name: &str) -> ModelProfile {
        ModelProfile {
            display_model: name.to_string(),
            request_model: name.to_string(),
            billing_model: name.to_string(),
            aliases: Vec::new(),
            capabilities: Capabilities::default(),
        }
    }

    fn provider(profile: &str, model_name: &str) -> ProviderProfile {
        ProviderProfile {
            provider_id: ProviderId::OpenAICompatible { name: profile.to_string() },
            profile_name: profile.to_string(),
            base_url: "https://example.test/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::ApiKey,
            credential: CredentialConfig::Static { id: profile.to_string() },
            models: vec![model(model_name)],
            pricing: PricingConfig::default(),
            signing: None,
            azure: None,
            supports_websockets: false,
            supports_websocket_compression: false,
            websocket_connect_timeout_ms: None,
        }
    }

    fn resolver() -> ModelResolver {
        let cfg = ClientConfig {
            providers: vec![
                provider("profile-a", "model-a"),
                provider("profile-b", "model-b"),
                provider("profile-arb", "model-arb"),
            ],
        };
        ModelResolver::from_client_config(cfg).unwrap()
    }

    fn config_with_limits(limits: LimitConfig) -> MultiAgentConfig {
        MultiAgentConfig {
            enabled: true,
            mode: MultiAgentMode::Force,
            strategy: MultiAgentStrategyKind::DualLlmCompetitive,
            candidates: vec![
                AgentEndpoint {
                    id: "candidate-a".into(),
                    label: None,
                    model: "profile-a/model-a".into(),
                    role: None,
                },
                AgentEndpoint {
                    id: "candidate-b".into(),
                    label: None,
                    model: "profile-b/model-b".into(),
                    role: None,
                },
            ],
            reviewers: ReviewConfig::default(),
            arbiter: ArbiterConfig { model: "profile-arb/model-arb".into(), allow_hybrid: false },
            triggers: TriggerConfig::default(),
            limits,
        }
    }

    fn fast_limits() -> LimitConfig {
        LimitConfig {
            max_iterations: 1,
            timeout: Duration::from_secs(30),
            phase_timeout: PhaseTimeouts {
                implementation: Duration::from_secs(5),
                review: Duration::from_secs(5),
                revision: Duration::from_secs(5),
                arbitration: Duration::from_secs(5),
                verification: Duration::from_secs(5),
            },
            max_changed_files: 50,
            cleanup_worktrees: crate::config::CleanupPolicy::OnSuccess,
        }
    }

    /// A scriptable mock runner. Each candidate id maps to a behavior. On a
    /// "succeed" behavior the runner writes a marker file in its OWN cwd
    /// (proving cwd isolation) and returns a synthetic diff/report.
    #[derive(Clone)]
    enum Behavior {
        Succeed { changed_files: usize },
        Fail,
        EmptyDiff,
        /// Block until cancelled or the phase timeout fires.
        Hang,
    }

    struct MockCandidateRunner {
        behaviors: BTreeMap<String, Behavior>,
        /// Records the cwd each candidate was actually invoked with.
        invoked_cwds: Mutex<BTreeMap<String, PathBuf>>,
        /// How many candidates ran concurrently (peak).
        in_flight: AtomicUsize,
        peak_in_flight: AtomicUsize,
    }

    impl MockCandidateRunner {
        fn new(behaviors: BTreeMap<String, Behavior>) -> Self {
            Self {
                behaviors,
                invoked_cwds: Mutex::new(BTreeMap::new()),
                in_flight: AtomicUsize::new(0),
                peak_in_flight: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl CandidateRunner for MockCandidateRunner {
        async fn run(
            &self,
            ctx: &CandidateRunContext,
        ) -> Result<CandidateOutcome, MultiAgentError> {
            // Track concurrency.
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_in_flight.fetch_max(now, Ordering::SeqCst);
            // Yield so a sibling can overlap (proving parallelism).
            tokio::task::yield_now().await;

            self.invoked_cwds
                .lock()
                .unwrap()
                .insert(ctx.candidate_id.clone(), ctx.cwd.clone());

            let behavior = self.behaviors.get(&ctx.candidate_id).cloned();
            let result = match behavior {
                Some(Behavior::Succeed { changed_files }) => {
                    // Write ONLY inside our own cwd — proves isolation.
                    let marker = ctx.cwd.join(format!("{}.touched", ctx.candidate_id));
                    std::fs::write(&marker, b"x").unwrap();
                    let mut diff = String::new();
                    for i in 0..changed_files {
                        diff.push_str(&format!("diff --git a/f{i}.rs b/f{i}.rs\n+line\n"));
                    }
                    Ok(CandidateOutcome {
                        patch_diff: diff,
                        self_report: format!("report for {}", ctx.candidate_id),
                        usage: TokenUsage { input: 10, output: 20, ..TokenUsage::default() },
                    })
                }
                Some(Behavior::EmptyDiff) => Ok(CandidateOutcome {
                    patch_diff: String::new(),
                    self_report: "nothing changed".into(),
                    usage: TokenUsage::default(),
                }),
                Some(Behavior::Fail) => Err(MultiAgentError::CandidateFailed {
                    candidate_id: ctx.candidate_id.clone(),
                    phase: DualLlmPhase::ImplementCandidates,
                    reason: "scripted failure".into(),
                }),
                Some(Behavior::Hang) => {
                    // Wait until cancelled (or get killed by the phase timeout).
                    ctx.cancel.cancelled().await;
                    Err(MultiAgentError::Cancelled)
                }
                None => Err(MultiAgentError::CandidateFailed {
                    candidate_id: ctx.candidate_id.clone(),
                    phase: DualLlmPhase::ImplementCandidates,
                    reason: "no behavior scripted".into(),
                }),
            };

            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            result
        }
    }

    /// Usage sink that records calls.
    #[derive(Default)]
    struct RecordingUsage {
        calls: Mutex<Vec<(String, TokenUsage)>>,
    }
    impl UsageSink for RecordingUsage {
        fn record_candidate_usage(&self, id: &str, usage: &TokenUsage, _d: Duration) {
            self.calls.lock().unwrap().push((id.to_string(), *usage));
        }
    }

    fn behaviors(items: &[(&str, Behavior)]) -> BTreeMap<String, Behavior> {
        items.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect()
    }

    #[tokio::test]
    async fn both_candidates_run_in_parallel_each_in_own_cwd() {
        let tmp = TempDir::new().unwrap();
        let mgr = TempWorktreeManager::new(tmp.path().to_path_buf());
        let res = resolver();
        let cfg = config_with_limits(fast_limits());
        let runner = Arc::new(MockCandidateRunner::new(behaviors(&[
            ("candidate-a", Behavior::Succeed { changed_files: 1 }),
            ("candidate-b", Behavior::Succeed { changed_files: 2 }),
        ])));
        let usage = Arc::new(RecordingUsage::default());

        let dual = DualLlm::new(&mgr, &res, runner.clone(), usage.clone(), &cfg, tmp.path());
        let run = dual
            .run_implementation("do the thing", CancellationToken::new())
            .await
            .expect("implementation run");

        assert!(run.any_viable());
        assert_eq!(run.candidates.len(), 2);
        assert!(run.candidates.iter().all(CandidateResult::is_viable));

        // Parallelism actually happened.
        assert_eq!(runner.peak_in_flight.load(Ordering::SeqCst), 2);

        // Each candidate wrote ONLY inside its own worktree cwd.
        let cwds = runner.invoked_cwds.lock().unwrap().clone();
        let cwd_a = &cwds["candidate-a"];
        let cwd_b = &cwds["candidate-b"];
        assert_ne!(cwd_a, cwd_b, "candidates must have distinct worktrees");
        assert!(cwd_a.join("candidate-a.touched").is_file());
        assert!(cwd_b.join("candidate-b.touched").is_file());
        // Neither wrote into the other's cwd.
        assert!(!cwd_a.join("candidate-b.touched").exists());
        assert!(!cwd_b.join("candidate-a.touched").exists());

        // Usage recorded for both.
        assert_eq!(usage.calls.lock().unwrap().len(), 2);

        // Artifacts persisted under .lingxi/.../<run_id>/<candidate>/.
        let base = tmp.path().join(".lingxi/multi-agent/runs").join(&run.run_id);
        assert!(base.join("task-brief.md").is_file());
        assert!(base.join("candidate-a/patch.diff").is_file());
        assert!(base.join("candidate-b/patch.diff").is_file());
        assert!(base.join("state.json").is_file());
    }

    #[tokio::test]
    async fn one_candidate_times_out_other_continues() {
        let tmp = TempDir::new().unwrap();
        let mgr = TempWorktreeManager::new(tmp.path().to_path_buf());
        let res = resolver();
        // Tight implementation timeout so the hanging candidate is reaped.
        let mut limits = fast_limits();
        limits.phase_timeout.implementation = Duration::from_millis(150);
        let cfg = config_with_limits(limits);
        let runner = Arc::new(MockCandidateRunner::new(behaviors(&[
            ("candidate-a", Behavior::Succeed { changed_files: 1 }),
            ("candidate-b", Behavior::Hang),
        ])));
        let usage = Arc::new(RecordingUsage::default());

        let dual = DualLlm::new(&mgr, &res, runner, usage, &cfg, tmp.path());
        let run = dual
            .run_implementation("brief", CancellationToken::new())
            .await
            .expect("run completes despite one timeout");

        assert!(run.any_viable(), "the succeeding candidate keeps the run viable");
        let a = run.candidates.iter().find(|c| c.candidate_id == "candidate-a").unwrap();
        let b = run.candidates.iter().find(|c| c.candidate_id == "candidate-b").unwrap();
        assert!(a.is_viable());
        assert!(matches!(
            b.outcome,
            Err(MultiAgentError::CandidateTimedOut { .. })
        ));
        // Both worktrees retained on the (timed-out) candidate.
        assert!(b.worktree.path.exists());
    }

    #[tokio::test]
    async fn both_candidates_fail_run_failed_artifacts_kept() {
        let tmp = TempDir::new().unwrap();
        let mgr = TempWorktreeManager::new(tmp.path().to_path_buf());
        let res = resolver();
        let cfg = config_with_limits(fast_limits());
        let runner = Arc::new(MockCandidateRunner::new(behaviors(&[
            ("candidate-a", Behavior::Fail),
            ("candidate-b", Behavior::EmptyDiff),
        ])));
        let usage = Arc::new(RecordingUsage::default());

        let dual = DualLlm::new(&mgr, &res, runner, usage, &cfg, tmp.path());
        let run = dual
            .run_implementation("brief", CancellationToken::new())
            .await
            .expect("run returns even when both fail");

        assert!(!run.any_viable(), "both candidates failed → run not viable");
        // Artifacts (incl. failure self-reports + state) are retained.
        let base = tmp.path().join(".lingxi/multi-agent/runs").join(&run.run_id);
        assert!(base.join("candidate-a/self-report.md").is_file());
        assert!(base.join("candidate-b/self-report.md").is_file());
        assert!(base.join("state.json").is_file());
        // Empty-diff candidate was classified as a failure.
        let b = run.candidates.iter().find(|c| c.candidate_id == "candidate-b").unwrap();
        assert!(matches!(
            b.outcome,
            Err(MultiAgentError::CandidateFailed { .. })
        ));
    }

    #[tokio::test]
    async fn user_cancel_stops_candidates_and_keeps_worktrees() {
        let tmp = TempDir::new().unwrap();
        let mgr = TempWorktreeManager::new(tmp.path().to_path_buf());
        let res = resolver();
        let cfg = config_with_limits(fast_limits());
        let runner = Arc::new(MockCandidateRunner::new(behaviors(&[
            ("candidate-a", Behavior::Hang),
            ("candidate-b", Behavior::Hang),
        ])));
        let usage = Arc::new(RecordingUsage::default());

        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        // Cancel shortly after the run starts.
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel_for_task.cancel();
        });

        let dual = DualLlm::new(&mgr, &res, runner, usage, &cfg, tmp.path());
        let run = dual
            .run_implementation("brief", cancel)
            .await
            .expect("run returns after cancellation");

        // Both candidates observed cancellation; neither is viable.
        assert!(!run.any_viable());
        for c in &run.candidates {
            assert!(matches!(c.outcome, Err(MultiAgentError::Cancelled)));
            // Worktrees retained on cancellation (design doc §安全约束 #7).
            assert!(c.worktree.path.exists());
        }
    }

    #[tokio::test]
    async fn diff_exceeding_max_changed_files_fails_candidate() {
        let tmp = TempDir::new().unwrap();
        let mgr = TempWorktreeManager::new(tmp.path().to_path_buf());
        let res = resolver();
        let mut limits = fast_limits();
        limits.max_changed_files = 1;
        let cfg = config_with_limits(limits);
        let runner = Arc::new(MockCandidateRunner::new(behaviors(&[
            ("candidate-a", Behavior::Succeed { changed_files: 5 }),
            ("candidate-b", Behavior::Succeed { changed_files: 1 }),
        ])));
        let usage = Arc::new(RecordingUsage::default());

        let dual = DualLlm::new(&mgr, &res, runner, usage, &cfg, tmp.path());
        let run = dual
            .run_implementation("brief", CancellationToken::new())
            .await
            .unwrap();

        let a = run.candidates.iter().find(|c| c.candidate_id == "candidate-a").unwrap();
        let b = run.candidates.iter().find(|c| c.candidate_id == "candidate-b").unwrap();
        assert!(matches!(
            a.outcome,
            Err(MultiAgentError::CandidateFailed { .. })
        ));
        assert!(b.is_viable(), "the small diff stays viable");
        assert!(run.any_viable());
    }

    #[tokio::test]
    async fn unresolvable_candidate_model_is_fatal() {
        let tmp = TempDir::new().unwrap();
        let mgr = TempWorktreeManager::new(tmp.path().to_path_buf());
        let res = resolver();
        let mut cfg = config_with_limits(fast_limits());
        cfg.candidates[0].model = "profile-a/ghost-model".into();
        let runner = Arc::new(MockCandidateRunner::new(behaviors(&[])));
        let usage = Arc::new(RecordingUsage::default());

        let dual = DualLlm::new(&mgr, &res, runner, usage, &cfg, tmp.path());
        let err = dual
            .run_implementation("brief", CancellationToken::new())
            .await
            .expect_err("unresolvable model fails fast");
        assert!(matches!(err, MultiAgentError::ProviderUnavailable { .. }));
    }

    #[tokio::test]
    async fn todo_real_runner_is_a_compiling_seam() {
        // The real runner is a marked seam; it currently fails cleanly rather
        // than panicking, so the orchestrator stays green until it's wired.
        let ctx = CandidateRunContext {
            candidate_id: "candidate-a".into(),
            task_brief: "x".into(),
            cwd: PathBuf::from("/tmp/does-not-matter"),
            resolved: resolver()
                .resolve_endpoint(&AgentEndpoint {
                    id: "candidate-a".into(),
                    label: None,
                    model: "profile-a/model-a".into(),
                    role: None,
                })
                .unwrap(),
            cancel: CancellationToken::new(),
        };
        let err = TodoLlmCandidateRunner.run(&ctx).await.unwrap_err();
        assert!(matches!(err, MultiAgentError::CandidateFailed { .. }));
    }

    #[test]
    fn count_changed_files_counts_git_headers() {
        let diff = "diff --git a/x.rs b/x.rs\n+a\ndiff --git a/y.rs b/y.rs\n+b\n";
        assert_eq!(count_changed_files(diff), 2);
        // Fallback to +++ lines when no git headers.
        let plain = "--- a/x\n+++ b/x\n+a\n--- a/y\n+++ b/y\n+b\n";
        assert_eq!(count_changed_files(plain), 2);
        assert_eq!(count_changed_files(""), 0);
    }
}
