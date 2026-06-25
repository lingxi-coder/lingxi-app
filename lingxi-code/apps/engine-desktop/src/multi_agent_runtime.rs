//! Composition-root adapter that makes the `multi-agent` crate's injected
//! seams REAL by driving the cross-crate `traits::SubagentSpawner`.
//!
//! The `multi-agent` crate is deliberately thin + trait-driven: it owns the
//! orchestration state machine and the [`CandidateRunner`] / [`Reviser`] /
//! [`VerificationFixer`] trait seams, but depends only on
//! `traits/llm-client/sidequery/protocol/cost` — NOT on the `agent` engine
//! crate. This module is the single place that bridges those seams to the
//! real subagent multi-turn loop:
//!
//! - `agent::PoolSubagentSpawner` (assembled in `lib.rs build()` with the real
//!   api_client / tool registry / permission policy / hooks / env renderer)
//!   already encapsulates all `SubagentContext` construction and pumps
//!   `SubagentEvent` to a terminal [`SubagentResult`] internally, so the
//!   adapter needs no event loop — it calls
//!   [`traits::subagent_spawn::SubagentSpawner::spawn`] and matches the result.
//! - The cwd-pin is achieved by setting `SubagentSpawnRequest.cwd` to the
//!   candidate / author worktree; the runner threads that onto every dispatched
//!   tool's `ToolUseContext.cwd`, confining file edits to that worktree.
//! - The diff is NOT in the spawn result (the agent returns text); the host
//!   derives it from `git diff` of the worktree (design doc: "host 从 git diff
//!   生成"), reusing the same git-CLI spawn pattern as
//!   `multi_agent::finalizer::GitPatchApplier`.
//!
//! ## What is verified vs runtime-only
//!
//! These adapters are unit-tested with a deterministic mock
//! `Arc<dyn SubagentSpawner>` that SIMULATES file writes into `ctx.cwd` before
//! returning [`SubagentResult::Completed`] (the recording-mock pattern). That
//! covers: the spawn request shape (cwd / subagent_type / prompt / inherited
//! Arcs via `Arc::ptr_eq`), result→outcome mapping, usage mapping, real
//! `git diff` extraction against a real temp repo, the empty-diff path, and
//! cancellation-before-spawn.
//!
//! Runtime-only (NOT faked green here): that a REAL model actually edits files
//! producing a meaningful diff; real token-usage numbers + streaming; the full
//! dual-LLM pipeline against TWO live providers. Per the spike, the spawner
//! seam routes both candidates through the ONE wired subagent api_client
//! (`model` is only a family alias `sonnet|opus|haiku`), so this is
//! dual-candidate, not dual-provider — matching design doc §最小可行版本.
//! Full per-provider routing needs a spawn-time api_client override and is
//! deferred.

use async_trait::async_trait;
use llm_client::TokenUsage;
use multi_agent::orchestrator::CandidateOutcome;
use multi_agent::orchestrator::CandidateRunContext;
use multi_agent::orchestrator::CandidateRunner;
use multi_agent::revision::RevisedOutcome;
use multi_agent::revision::Reviser;
use multi_agent::revision::RevisionContext;
use multi_agent::state::DualLlmPhase;
use multi_agent::verification::FixContext;
use multi_agent::verification::VerificationFixer;
use multi_agent::MultiAgentError;
use std::path::Path;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use traits::subagent_spawn::SubagentInheritance;
use traits::subagent_spawn::SubagentResult;
use traits::subagent_spawn::SubagentSpawnError;
use traits::subagent_spawn::SubagentSpawnRequest;
use traits::subagent_spawn::SubagentSpawner;
use traits::subagent_spawn::SubagentUsage;

/// The subagent type the candidate / reviser / fixer runs as. `general-purpose`
/// resolves to the All-tools agent (Edit / Write / Bash) so the agent can
/// actually edit files in its worktree.
const FILE_EDITING_AGENT: &str = "general-purpose";

/// Map a [`SubagentUsage`] (claude-shaped token buckets) onto the
/// `llm-client` [`TokenUsage`] the cost/telemetry sink consumes. Field-by-field;
/// the subagent path has no separately-billed reasoning bucket, so
/// `reasoning_output` is `0` (not faked).
fn map_usage(u: &SubagentUsage) -> TokenUsage {
    TokenUsage {
        input: u.input_tokens,
        output: u.output_tokens,
        cache_write: u.cache_creation_input_tokens,
        cache_read: u.cache_read_input_tokens,
        reasoning_output: 0,
    }
}

/// Extract the unified diff of everything changed under `cwd` since the
/// worktree's committed HEAD baseline. Runs `git add -A` (so new/untracked
/// files appear) then `git diff --cached`. Reuses the `GitPatchApplier`
/// spawn/stdin/exit pattern; a backend failure (git missing / not a repo)
/// surfaces as a [`MultiAgentError::CandidateFailed`]-shaped error via the
/// caller's `phase`.
async fn worktree_diff(cwd: &Path) -> Result<String, String> {
    // Stage everything so untracked files are included in the cached diff.
    let add = run_git(cwd, &["add", "-A"]).await?;
    if !add.status.success() {
        return Err(format!(
            "git add -A failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        ));
    }
    let diff = run_git(cwd, &["diff", "--cached"]).await?;
    if !diff.status.success() {
        return Err(format!(
            "git diff --cached failed: {}",
            String::from_utf8_lossy(&diff.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&diff.stdout).into_owned())
}

/// Run `git <args>` in `cwd`. Errors (as a `String`) only when git could not be
/// spawned / waited on; a non-zero exit is surfaced to the caller via the
/// returned `Output.status`.
async fn run_git(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    tokio::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not spawn git: {e}"))?
        .wait_with_output()
        .await
        .map_err(|e| format!("waiting for git: {e}"))
}

/// Race a spawn future against a cancellation token. `SubagentSpawner::spawn`
/// takes no token, so dropping the future (when cancelled) drops the pool slot;
/// the runner future is cancel-safe.
async fn spawn_or_cancel(
    spawner: &dyn SubagentSpawner,
    request: SubagentSpawnRequest,
    inherit: SubagentInheritance,
    cancel: &CancellationToken,
) -> Result<SubagentResult, SpawnFlowError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(SpawnFlowError::Cancelled),
        r = spawner.spawn(request, inherit) => r.map_err(SpawnFlowError::Spawn),
    }
}

/// Internal flow error from a spawn race.
enum SpawnFlowError {
    /// The run was cancelled before / during the spawn.
    Cancelled,
    /// The spawner itself errored (pool full / runtime).
    Spawn(SubagentSpawnError),
}

/// Drive ONE cwd-pinned file-editing agent to completion and return the
/// worktree diff + the agent's text self-report + mapped usage. Shared by the
/// candidate runner and the reviser (and adapted by the fixer). `prompt` is the
/// fully-composed first user message; `phase` tags any failure.
async fn run_editing_agent(
    spawner: &dyn SubagentSpawner,
    inherit: &SubagentInheritance,
    cwd: &Path,
    prompt: String,
    cancel: &CancellationToken,
    candidate_id: &str,
    phase: DualLlmPhase,
) -> Result<EditingOutcome, MultiAgentError> {
    let request = SubagentSpawnRequest {
        subagent_type: FILE_EDITING_AGENT.to_string(),
        prompt,
        cwd: Some(cwd.to_string_lossy().into_owned()),
        ..default_request()
    };

    let result = spawn_or_cancel(spawner, request, inherit.clone(), cancel).await;

    let (content, usage) = match result {
        Ok(SubagentResult::Completed { content, usage, .. }) => (content, usage),
        Ok(SubagentResult::Failed { reason, .. }) => {
            return Err(MultiAgentError::CandidateFailed {
                candidate_id: candidate_id.to_string(),
                phase,
                reason,
            });
        }
        Ok(SubagentResult::Killed { .. }) => return Err(MultiAgentError::Cancelled),
        Err(SpawnFlowError::Cancelled) => return Err(MultiAgentError::Cancelled),
        Err(SpawnFlowError::Spawn(e)) => {
            return Err(MultiAgentError::CandidateFailed {
                candidate_id: candidate_id.to_string(),
                phase,
                reason: format!("subagent spawn failed: {e}"),
            });
        }
    };

    // The diff is host-derived from git, never from LLM prose.
    let patch_diff = worktree_diff(cwd).await.map_err(|reason| {
        MultiAgentError::CandidateFailed { candidate_id: candidate_id.to_string(), phase, reason }
    })?;

    Ok(EditingOutcome { patch_diff, self_report: self_report_text(&content), usage: map_usage(&usage) })
}

/// A fully-defaulted spawn request (every optional field absent). Keeps the
/// per-call request construction to the two fields the adapter actually sets
/// (`subagent_type` + `cwd`) plus `prompt`.
fn default_request() -> SubagentSpawnRequest {
    SubagentSpawnRequest {
        subagent_type: String::new(),
        prompt: String::new(),
        context_paths: Vec::new(),
        description: None,
        model: None,
        run_in_background: false,
        name: None,
        team_name: None,
        mode: None,
        isolation: None,
        cwd: None,
        fork_context_messages: None,
        fork_parent_system_prompt: None,
        schema: None,
        effort: None,
        tool_use_id: None,
        system_prompt_override: None,
        system_prompt_addendum: None,
        additional_disallowed_tools: Vec::new(),
    }
}

/// Render the subagent's free-form JSON `content` payload to a text self-report.
/// A JSON string is unwrapped; anything else is pretty-printed.
fn self_report_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// Internal product of [`run_editing_agent`].
struct EditingOutcome {
    patch_diff: String,
    self_report: String,
    usage: TokenUsage,
}

/// Real [`CandidateRunner`] — drives one candidate implementation in its
/// worktree via the injected spawner. Seeds the implementer prompt + task brief
/// as the first user message.
pub struct SpawnerCandidateRunner {
    /// The session's real subagent spawner (`PoolSubagentSpawner`).
    pub spawner: std::sync::Arc<dyn SubagentSpawner>,
    /// The session's tool-invoker + budget Arcs, handed to the child verbatim.
    pub inherit: SubagentInheritance,
}

#[async_trait]
impl CandidateRunner for SpawnerCandidateRunner {
    async fn run(&self, ctx: &CandidateRunContext) -> Result<CandidateOutcome, MultiAgentError> {
        let prompt = format!(
            "{}\n\n# Task\n\n{}",
            multi_agent::prompts::IMPLEMENTER,
            ctx.task_brief
        );
        let out = run_editing_agent(
            self.spawner.as_ref(),
            &self.inherit,
            &ctx.cwd,
            prompt,
            &ctx.cancel,
            &ctx.candidate_id,
            DualLlmPhase::ImplementCandidates,
        )
        .await?;
        Ok(CandidateOutcome {
            patch_diff: out.patch_diff,
            self_report: out.self_report,
            usage: out.usage,
        })
    }
}

/// Real [`Reviser`] — drives one author's revision in its OWN worktree, seeding
/// the reviser prompt + task brief + the review feedback + the pre-revision
/// patch. Errors are recoverable (the orchestrator keeps the pre-revision
/// patch).
pub struct SpawnerReviser {
    /// The session's real subagent spawner.
    pub spawner: std::sync::Arc<dyn SubagentSpawner>,
    /// The session's tool-invoker + budget Arcs.
    pub inherit: SubagentInheritance,
}

#[async_trait]
impl Reviser for SpawnerReviser {
    async fn revise(&self, ctx: &RevisionContext) -> Result<RevisedOutcome, MultiAgentError> {
        let prompt = format!(
            "{prompt}\n\n# Task\n\n{brief}\n\n# Review of your implementation\n\n{review}\n\n\
             # Your current patch\n\n```diff\n{patch}\n```",
            prompt = multi_agent::prompts::REVISER,
            brief = ctx.task_brief,
            review = ctx.review_feedback,
            patch = ctx.pre_revision_patch,
        );
        let out = run_editing_agent(
            self.spawner.as_ref(),
            &self.inherit,
            &ctx.cwd,
            prompt,
            &ctx.cancel,
            &ctx.candidate_id,
            DualLlmPhase::AuthorRevision,
        )
        .await?;
        Ok(RevisedOutcome { patch_diff: out.patch_diff, note: out.self_report })
    }
}

/// Real [`VerificationFixer`] — drives a single repair pass in the final
/// workspace / winner worktree, seeding the failure log. Operates on
/// `ctx.workspace` (NOT a candidate worktree). The fix loop calls this between
/// verification attempts; an `Err` stops the loop (marking failed).
pub struct SpawnerVerificationFixer {
    /// The session's real subagent spawner.
    pub spawner: std::sync::Arc<dyn SubagentSpawner>,
    /// The session's tool-invoker + budget Arcs.
    pub inherit: SubagentInheritance,
}

#[async_trait]
impl VerificationFixer for SpawnerVerificationFixer {
    async fn fix(&self, ctx: &FixContext) -> Result<(), MultiAgentError> {
        let prompt = format!(
            "You are fixing a failing verification run in the final workspace. \
             Apply the minimal change to make verification pass; do not introduce \
             unrelated changes.\n\n# Verification failure (attempt {attempt})\n\n\
             ```\n{log}\n```",
            attempt = ctx.attempt,
            log = ctx.failure_log,
        );
        // The fixer has no candidate id; tag with a stable sentinel and the
        // verification phase. A spawn/agent failure becomes the `Err` that stops
        // the loop. We do not require a diff (the loop re-runs verification to
        // decide success), so an empty edit is not itself a failure here.
        let request = SubagentSpawnRequest {
            subagent_type: FILE_EDITING_AGENT.to_string(),
            prompt,
            cwd: Some(ctx.workspace.to_string_lossy().into_owned()),
            ..default_request()
        };
        // The fixer is not handed a cancellation token at this seam; use a fresh
        // (never-cancelled) one so the spawn races nothing. The outer phase
        // timeout in the orchestrator bounds the overall attempt.
        let cancel = CancellationToken::new();
        match spawn_or_cancel(self.spawner.as_ref(), request, self.inherit.clone(), &cancel).await {
            Ok(SubagentResult::Completed { .. }) => Ok(()),
            Ok(SubagentResult::Failed { reason, .. }) => {
                Err(MultiAgentError::FinalizerFailed { reason: format!("verification fixer failed: {reason}") })
            }
            Ok(SubagentResult::Killed { .. }) | Err(SpawnFlowError::Cancelled) => {
                Err(MultiAgentError::Cancelled)
            }
            Err(SpawnFlowError::Spawn(e)) => {
                Err(MultiAgentError::FinalizerFailed { reason: format!("verification fixer spawn failed: {e}") })
            }
        }
    }
}

/// Select the [`VerificationFixer`] the fix loop should use, replacing the
/// crate's `NoopFixer` "no verification fixer wired" default with the REAL
/// spawner-backed fixer whenever there is a fix budget (P11).
///
/// `max_iterations` is [`multi_agent::verification::verify_with_fix_loop`]'s
/// budget ([`multi_agent::config::LimitConfig::max_iterations`]):
/// - `0` ⇒ verification runs exactly once with no fix attempt, so the loop
///   never invokes a fixer. We hand back the genuine
///   [`multi_agent::verification::NoopFixer`] (never spawns a model) — this is
///   the documented `max_iterations == 0` case the seam keeps available.
/// - `> 0` ⇒ the loop may hand a failure to the fixer; we return the real
///   [`SpawnerVerificationFixer`] driving the session's subagent spawner against
///   the final workspace / winner worktree.
#[must_use]
pub fn fixer_for(
    max_iterations: u32,
    spawner: std::sync::Arc<dyn SubagentSpawner>,
    inherit: SubagentInheritance,
) -> std::sync::Arc<dyn VerificationFixer> {
    if max_iterations == 0 {
        std::sync::Arc::new(multi_agent::verification::NoopFixer)
    } else {
        std::sync::Arc::new(SpawnerVerificationFixer { spawner, inherit })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path as StdPath;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::Mutex;
    use traits::budget::BudgetEnforcerHandle;
    use traits::budget::BudgetError;
    use traits::tool_invoker::SubagentInvocationContext;
    use traits::tool_invoker::ToolInvoker;
    use traits::tool_invoker::ToolInvokerError;

    // ---- Mock inheritance Arcs (recursion-lock + budget identity asserts). ----

    struct NoopInvoker;
    #[async_trait]
    impl ToolInvoker for NoopInvoker {
        async fn invoke(
            &self,
            _name: &str,
            _input: serde_json::Value,
            _ctx: SubagentInvocationContext,
        ) -> Result<serde_json::Value, ToolInvokerError> {
            Ok(serde_json::Value::Null)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct NoopBudget;
    #[async_trait]
    impl BudgetEnforcerHandle for NoopBudget {
        async fn check_and_charge(&self, _nano_usd: u64) -> Result<(), BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }

    fn inheritance() -> (
        SubagentInheritance,
        Arc<dyn ToolInvoker>,
        Arc<dyn BudgetEnforcerHandle>,
    ) {
        let tool_invoker: Arc<dyn ToolInvoker> = Arc::new(NoopInvoker);
        let budget: Arc<dyn BudgetEnforcerHandle> = Arc::new(NoopBudget);
        (
            SubagentInheritance { tool_invoker: tool_invoker.clone(), budget: budget.clone() },
            tool_invoker,
            budget,
        )
    }

    /// A recording spawner: captures the request + the inherited Arcs, optionally
    /// SIMULATES file writes into `request.cwd` before returning a scripted
    /// terminal result.
    #[derive(Clone)]
    enum SpawnScript {
        /// Write `(filename, contents)` into the spawn cwd, then Completed.
        WriteThenComplete {
            file: String,
            contents: String,
            content: serde_json::Value,
            usage: SubagentUsage,
        },
        /// Completed without writing anything (→ empty diff).
        CompleteNoWrite,
        /// The agent failed.
        Failed(String),
        /// The agent was killed.
        Killed,
        /// Block until the spawn future is dropped (cancellation race).
        Hang,
    }

    struct RecordingSpawner {
        script: SpawnScript,
        last_request: Mutex<Option<SubagentSpawnRequest>>,
        last_inherit: Mutex<Option<SubagentInheritance>>,
        spawn_calls: std::sync::atomic::AtomicUsize,
    }

    impl RecordingSpawner {
        fn new(script: SpawnScript) -> Self {
            Self {
                script,
                last_request: Mutex::new(None),
                last_inherit: Mutex::new(None),
                spawn_calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl SubagentSpawner for RecordingSpawner {
        async fn spawn(
            &self,
            request: SubagentSpawnRequest,
            inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            self.spawn_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.last_inherit.lock().unwrap() = Some(inherit);
            let cwd = request.cwd.clone();
            *self.last_request.lock().unwrap() = Some(request);

            match self.script.clone() {
                SpawnScript::WriteThenComplete { file, contents, content, usage } => {
                    let dir = PathBuf::from(cwd.expect("cwd must be set on the spawn request"));
                    std::fs::write(dir.join(file), contents).unwrap();
                    Ok(SubagentResult::Completed {
                        agent_id: protocol::AgentId::new(),
                        content,
                        usage,
                        total_tool_use_count: 1,
                        total_duration_ms: 1,
                        total_tokens: 0,
                        assistant_message_count: 1,
                        response_char_count: 1,
                        last_request_id: None,
                    })
                }
                SpawnScript::CompleteNoWrite => Ok(SubagentResult::Completed {
                    agent_id: protocol::AgentId::new(),
                    content: serde_json::Value::String("no changes".into()),
                    usage: SubagentUsage::default(),
                    total_tool_use_count: 0,
                    total_duration_ms: 1,
                    total_tokens: 0,
                    assistant_message_count: 1,
                    response_char_count: 1,
                    last_request_id: None,
                }),
                SpawnScript::Failed(reason) => {
                    Ok(SubagentResult::Failed { agent_id: protocol::AgentId::new(), reason })
                }
                SpawnScript::Killed => {
                    Ok(SubagentResult::Killed { agent_id: protocol::AgentId::new() })
                }
                SpawnScript::Hang => {
                    // Never resolves — the spawn_or_cancel select must drop us.
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            }
        }
    }

    // ---- A real git worktree with a committed HEAD baseline to diff against. ----

    fn git_ok(dir: &StdPath, args: &[&str]) {
        let out = Command::new("git").current_dir(dir).args(args).output().expect("spawn git");
        assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn init_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        git_ok(p, &["init", "-q"]);
        git_ok(p, &["config", "user.email", "t@example.com"]);
        git_ok(p, &["config", "user.name", "Tester"]);
        std::fs::write(p.join("README.md"), "base\n").unwrap();
        git_ok(p, &["add", "."]);
        git_ok(p, &["commit", "-q", "-m", "baseline"]);
        tmp
    }

    fn candidate_ctx(cwd: &StdPath, cancel: CancellationToken) -> CandidateRunContext {
        // Build a real ResolvedCandidate via the providers resolver is heavy;
        // the adapter never reads `resolved`, so construct a minimal one through
        // the public test seam used by the orchestrator tests is unnecessary —
        // we only need the fields the adapter consumes. The struct requires a
        // `ResolvedCandidate`, so build it via the resolver helper.
        CandidateRunContext {
            candidate_id: "candidate-a".into(),
            task_brief: "Add a greeting function".into(),
            cwd: cwd.to_path_buf(),
            resolved: resolved_candidate(),
            cancel,
        }
    }

    /// Build a `ResolvedCandidate` through the real `ModelResolver` (the adapter
    /// does not read it, but the context struct requires it).
    fn resolved_candidate() -> multi_agent::ResolvedCandidate {
        use llm_client::AuthStrategy;
        use llm_client::Capabilities;
        use llm_client::ClientConfig;
        use llm_client::CredentialConfig;
        use llm_client::ModelProfile;
        use llm_client::PricingConfig;
        use llm_client::ProtocolFamily;
        use llm_client::ProviderId;
        use llm_client::ProviderProfile;
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
        let resolver = multi_agent::ModelResolver::from_client_config(cfg).unwrap();
        resolver
            .resolve_endpoint(&multi_agent::config::AgentEndpoint {
                id: "candidate-a".into(),
                label: None,
                model: "p/m".into(),
                role: None,
            })
            .unwrap()
    }

    #[test]
    fn map_usage_is_field_by_field() {
        let u = SubagentUsage {
            total_tokens: 999,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 4,
        };
        let t = map_usage(&u);
        assert_eq!(t.input, 10);
        assert_eq!(t.output, 20);
        assert_eq!(t.cache_write, 3);
        assert_eq!(t.cache_read, 4);
        assert_eq!(t.reasoning_output, 0);
    }

    #[tokio::test]
    async fn candidate_runner_pins_cwd_extracts_diff_and_maps_usage() {
        let repo = init_repo();
        let (inherit, ti, bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::WriteThenComplete {
            file: "greeting.rs".into(),
            contents: "pub fn hi() {}\n".into(),
            content: serde_json::Value::String("implemented greeting".into()),
            usage: SubagentUsage {
                total_tokens: 30,
                input_tokens: 11,
                output_tokens: 22,
                cache_creation_input_tokens: 1,
                cache_read_input_tokens: 2,
            },
        }));
        let runner = SpawnerCandidateRunner { spawner: spawner.clone(), inherit };
        let ctx = candidate_ctx(repo.path(), CancellationToken::new());

        let out = runner.run(&ctx).await.expect("candidate run");

        // Diff is host-derived from git and contains the new file the mock wrote
        // into the pinned cwd.
        assert!(out.patch_diff.contains("greeting.rs"), "diff = {}", out.patch_diff);
        assert!(out.patch_diff.contains("pub fn hi()"));
        // Self-report = the agent's text content.
        assert_eq!(out.self_report, "implemented greeting");
        // Usage mapped field-by-field.
        assert_eq!(out.usage.input, 11);
        assert_eq!(out.usage.output, 22);
        assert_eq!(out.usage.cache_write, 1);
        assert_eq!(out.usage.cache_read, 2);

        // cwd pin + subagent type on the request.
        let req = spawner.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(req.cwd.as_deref(), Some(repo.path().to_string_lossy().as_ref()));
        assert_eq!(req.subagent_type, FILE_EDITING_AGENT);
        assert!(req.prompt.contains("Add a greeting function"));
        assert!(req.prompt.contains("independent implementation agent"));

        // The SAME inherited Arcs were handed to the child (recursion lock +
        // budget identity) — not fresh ones.
        let got = spawner.last_inherit.lock().unwrap().clone().unwrap();
        assert!(Arc::ptr_eq(&got.tool_invoker, &ti));
        assert!(Arc::ptr_eq(&got.budget, &bud));
    }

    #[tokio::test]
    async fn candidate_runner_empty_diff_is_non_error_empty_patch() {
        // The adapter returns an empty patch; the orchestrator (not the adapter)
        // classifies empty-diff as a candidate failure. Here we assert the
        // adapter surfaces the empty patch cleanly.
        let repo = init_repo();
        let (inherit, _ti, _bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::CompleteNoWrite));
        let runner = SpawnerCandidateRunner { spawner, inherit };
        let ctx = candidate_ctx(repo.path(), CancellationToken::new());
        let out = runner.run(&ctx).await.expect("candidate run");
        assert!(out.patch_diff.trim().is_empty(), "no edits ⇒ empty diff");
    }

    #[tokio::test]
    async fn candidate_runner_failed_spawn_is_recoverable_candidate_failed() {
        let repo = init_repo();
        let (inherit, _ti, _bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::Failed("model exploded".into())));
        let runner = SpawnerCandidateRunner { spawner, inherit };
        let ctx = candidate_ctx(repo.path(), CancellationToken::new());
        let err = runner.run(&ctx).await.unwrap_err();
        match err {
            MultiAgentError::CandidateFailed { candidate_id, phase, reason } => {
                assert_eq!(candidate_id, "candidate-a");
                assert_eq!(phase, DualLlmPhase::ImplementCandidates);
                assert!(reason.contains("model exploded"));
            }
            other => panic!("expected CandidateFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn candidate_runner_killed_maps_to_cancelled() {
        let repo = init_repo();
        let (inherit, _ti, _bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::Killed));
        let runner = SpawnerCandidateRunner { spawner, inherit };
        let ctx = candidate_ctx(repo.path(), CancellationToken::new());
        let err = runner.run(&ctx).await.unwrap_err();
        assert!(matches!(err, MultiAgentError::Cancelled));
    }

    #[tokio::test]
    async fn candidate_runner_cancel_before_spawn_completes_is_cancelled() {
        let repo = init_repo();
        let (inherit, _ti, _bud) = inheritance();
        // The spawn hangs; cancelling the token wins the select.
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::Hang));
        let runner = SpawnerCandidateRunner { spawner, inherit };
        let cancel = CancellationToken::new();
        cancel.cancel(); // already cancelled ⇒ the biased select takes the cancel arm
        let ctx = candidate_ctx(repo.path(), cancel);
        let err = runner.run(&ctx).await.unwrap_err();
        assert!(matches!(err, MultiAgentError::Cancelled));
    }

    #[tokio::test]
    async fn reviser_seeds_review_feedback_and_revises_own_worktree() {
        let repo = init_repo();
        let (inherit, _ti, _bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::WriteThenComplete {
            file: "fix.rs".into(),
            contents: "// addressed review\n".into(),
            content: serde_json::Value::String("fixed: the blocking issue".into()),
            usage: SubagentUsage::default(),
        }));
        let reviser = SpawnerReviser { spawner: spawner.clone(), inherit };
        let ctx = RevisionContext {
            candidate_id: "candidate-a".into(),
            task_brief: "Add greeting".into(),
            review_feedback: "BLOCKING: handle empty input".into(),
            cwd: repo.path().to_path_buf(),
            pre_revision_patch: "diff --git a/x b/x\n+old\n".into(),
            cancel: CancellationToken::new(),
        };
        let out = reviser.revise(&ctx).await.expect("revise");
        assert!(out.patch_diff.contains("fix.rs"));
        assert_eq!(out.note, "fixed: the blocking issue");

        let req = spawner.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(req.cwd.as_deref(), Some(repo.path().to_string_lossy().as_ref()));
        assert!(req.prompt.contains("BLOCKING: handle empty input"));
        assert!(req.prompt.contains("Only modify your assigned worktree."));
        assert!(req.prompt.contains("+old"), "pre-revision patch is seeded");
    }

    #[tokio::test]
    async fn reviser_only_writes_its_own_worktree_diff_is_own_worktree_derived() {
        // Two independent real git worktrees. The reviser is pinned to repo_a's
        // cwd; the mock writes (as the agent would) ONLY into `request.cwd`. The
        // host derives the post-revision diff from `git diff --cached` of THAT
        // cwd, so the produced patch must describe repo_a's change and repo_b
        // (the sibling author's worktree) must be left pristine — proving the
        // cwd-pin confines both the edit and the diff to the author's own
        // worktree (design doc §安全约束 #1 / §Worktree 隔离).
        let repo_a = init_repo();
        let repo_b = init_repo();
        let (inherit, _ti, _bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::WriteThenComplete {
            file: "owned.rs".into(),
            contents: "// only in author a's worktree\n".into(),
            content: serde_json::Value::String("revised a".into()),
            usage: SubagentUsage::default(),
        }));
        let reviser = SpawnerReviser { spawner: spawner.clone(), inherit };
        let ctx = RevisionContext {
            candidate_id: "candidate-a".into(),
            task_brief: "t".into(),
            review_feedback: "fix".into(),
            cwd: repo_a.path().to_path_buf(),
            pre_revision_patch: "diff --git a/x b/x\n+old\n".into(),
            cancel: CancellationToken::new(),
        };
        let out = reviser.revise(&ctx).await.expect("revise");

        // The diff is derived from repo_a's worktree and names the file written
        // there.
        assert!(out.patch_diff.contains("owned.rs"), "diff = {}", out.patch_diff);
        assert!(out.patch_diff.contains("only in author a's worktree"));

        // The spawn was pinned to repo_a, NOT repo_b.
        let req = spawner.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(req.cwd.as_deref(), Some(repo_a.path().to_string_lossy().as_ref()));

        // The author's own worktree carries the new file; the sibling worktree
        // was never touched.
        assert!(repo_a.path().join("owned.rs").is_file());
        assert!(!repo_b.path().join("owned.rs").exists());
        // repo_b is still pristine (only the baseline README.md, no staged diff).
        let b_diff = worktree_diff(repo_b.path()).await.expect("b diff");
        assert!(b_diff.trim().is_empty(), "sibling worktree b must be untouched: {b_diff}");
    }

    #[tokio::test]
    async fn reviser_failure_is_recoverable_error() {
        let repo = init_repo();
        let (inherit, _ti, _bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::Failed("revise boom".into())));
        let reviser = SpawnerReviser { spawner, inherit };
        let ctx = RevisionContext {
            candidate_id: "candidate-a".into(),
            task_brief: "t".into(),
            review_feedback: "fix".into(),
            cwd: repo.path().to_path_buf(),
            pre_revision_patch: "diff\n".into(),
            cancel: CancellationToken::new(),
        };
        let err = reviser.revise(&ctx).await.unwrap_err();
        assert!(matches!(
            err,
            MultiAgentError::CandidateFailed { phase, .. } if phase == DualLlmPhase::AuthorRevision
        ));
    }

    #[tokio::test]
    async fn fixer_runs_in_workspace_and_succeeds() {
        let repo = init_repo();
        let (inherit, _ti, _bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::WriteThenComplete {
            file: "patch.rs".into(),
            contents: "// fixed\n".into(),
            content: serde_json::Value::Null,
            usage: SubagentUsage::default(),
        }));
        let fixer = SpawnerVerificationFixer { spawner: spawner.clone(), inherit };
        let ctx = FixContext {
            attempt: 1,
            workspace: repo.path().to_path_buf(),
            failure_log: "error[E0382]: borrow of moved value".into(),
        };
        fixer.fix(&ctx).await.expect("fix ok");
        let req = spawner.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(req.cwd.as_deref(), Some(repo.path().to_string_lossy().as_ref()));
        assert!(req.prompt.contains("error[E0382]"));
        assert_eq!(req.subagent_type, FILE_EDITING_AGENT);
    }

    #[tokio::test]
    async fn fixer_failure_stops_the_loop() {
        let repo = init_repo();
        let (inherit, _ti, _bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::Failed("cannot fix".into())));
        let fixer = SpawnerVerificationFixer { spawner, inherit };
        let ctx = FixContext {
            attempt: 1,
            workspace: repo.path().to_path_buf(),
            failure_log: "boom".into(),
        };
        let err = fixer.fix(&ctx).await.unwrap_err();
        assert!(matches!(err, MultiAgentError::FinalizerFailed { .. }));
    }

    #[tokio::test]
    async fn fixer_for_zero_iterations_is_noop_and_never_spawns() {
        // P11: with a 0-iteration fix budget the verification loop runs once and
        // never invokes a fixer; the selector must hand back the genuine no-op
        // (NOT the spawner-backed fixer) so even a stray call cannot trigger a
        // model spawn. We prove that by giving it a spawner that would PANIC if
        // ever asked to spawn (it returns Failed, which the real fixer turns into
        // an Err), yet `fix()` returns Ok and the spawn count stays 0.
        let (inherit, _ti, _bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::Failed("must not spawn".into())));
        let fixer = fixer_for(0, spawner.clone(), inherit);
        let ctx = FixContext {
            attempt: 1,
            workspace: PathBuf::from("/does/not/matter"),
            failure_log: "irrelevant".into(),
        };
        // No-op fixer always succeeds and performs no spawn.
        fixer.fix(&ctx).await.expect("noop fix is Ok");
        assert_eq!(
            spawner.spawn_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the 0-iteration fixer must never spawn a model"
        );
    }

    #[tokio::test]
    async fn fixer_for_positive_iterations_drives_the_real_spawner() {
        // P11: with iterations remaining the selector must hand back the REAL
        // spawner-backed fixer — it drives one spawn in the workspace and an
        // agent failure surfaces as the Err that stops the loop.
        let repo = init_repo();
        let (inherit, _ti, _bud) = inheritance();
        let spawner = Arc::new(RecordingSpawner::new(SpawnScript::WriteThenComplete {
            file: "fixed.rs".into(),
            contents: "// repaired\n".into(),
            content: serde_json::Value::Null,
            usage: SubagentUsage::default(),
        }));
        let fixer = fixer_for(2, spawner.clone(), inherit);
        let ctx = FixContext {
            attempt: 1,
            workspace: repo.path().to_path_buf(),
            failure_log: "error[E0277]".into(),
        };
        fixer.fix(&ctx).await.expect("real fix ok");
        assert_eq!(
            spawner.spawn_calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the real fixer drives exactly one spawn"
        );
        let req = spawner.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(req.cwd.as_deref(), Some(repo.path().to_string_lossy().as_ref()));
        assert!(req.prompt.contains("error[E0277]"));
    }
}
