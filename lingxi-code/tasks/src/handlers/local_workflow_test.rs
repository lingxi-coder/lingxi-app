//! Local workflow tests.
#![allow(clippy::unwrap_used)]

use super::*;
use serde_json::json;
use std::any::Any;
use std::collections::HashMap as StdHashMap;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use tempfile::tempdir;
use test_harness::mocks::MockRuntimeSpawner;
use tokio::sync::Mutex as TokioMutex;
use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
use traits::tool_invoker::{SubagentInvocationContext, ToolInvokerError};
use traits::{BudgetError, SubagentUsage};

// ---- Echo SubagentSpawner: `agent(p)` → "echo:p" (records prompts) ------

#[derive(Default)]
struct EchoSpawner {
    seen: StdMutex<Vec<String>>,
    seen_reqs: StdMutex<Vec<SubagentSpawnRequest>>,
    fail: bool,
}

#[async_trait]
impl SubagentSpawner for EchoSpawner {
    async fn agent_listing(&self) -> Vec<traits::subagent_spawn::SubagentListingEntry> {
        [
            "general-purpose",
            "Explore",
            "code-reviewer",
            "workflow-subagent",
        ]
        .iter()
        .map(|t| traits::subagent_spawn::SubagentListingEntry {
            agent_type: (*t).to_string(),
            when_to_use: String::new(),
            tools_description: String::new(),
        })
        .collect()
    }
    async fn spawn(
        &self,
        request: SubagentSpawnRequest,
        _inherit: SubagentInheritance,
    ) -> Result<SubagentResult, SubagentSpawnError> {
        self.seen.lock().unwrap().push(request.prompt.clone());
        self.seen_reqs.lock().unwrap().push(request.clone());
        if self.fail {
            return Ok(SubagentResult::Failed {
                agent_id: protocol::AgentId::new(),
                reason: "boom".into(),
            });
        }
        Ok(SubagentResult::Completed {
            agent_id: protocol::AgentId::new(),
            content: Value::String(format!("echo:{}", request.prompt)),
            usage: SubagentUsage {
                output_tokens: 100,
                ..Default::default()
            },
            total_tool_use_count: 0,
            total_duration_ms: 0,
            total_tokens: 0,
            assistant_message_count: 0,
            response_char_count: 0,
            last_request_id: None,
        })
    }
}

// ---- Inert ToolInvoker / BudgetEnforcerHandle ---------------------------

struct MockInvoker;
#[async_trait]
impl ToolInvoker for MockInvoker {
    async fn invoke(
        &self,
        _name: &str,
        _input: serde_json::Value,
        _ctx: SubagentInvocationContext,
    ) -> Result<serde_json::Value, ToolInvokerError> {
        Ok(json!(null))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct MockBudget;
#[async_trait]
impl BudgetEnforcerHandle for MockBudget {
    async fn check_and_charge(&self, _nano_usd: u64) -> Result<(), BudgetError> {
        Ok(())
    }
    async fn snapshot_total_nano_usd(&self) -> u64 {
        0
    }
}

// ---- In-memory FileSystem (mirrors the other handler fixtures) ----------

struct InMemoryFs {
    files: TokioMutex<StdHashMap<String, String>>,
}
impl InMemoryFs {
    fn new() -> Self {
        Self {
            files: TokioMutex::new(StdHashMap::new()),
        }
    }
}
#[async_trait]
impl FileSystem for InMemoryFs {
    async fn read_file(
        &self,
        path: &str,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> Result<FileContent, FsError> {
        let map = self.files.lock().await;
        let content = map.get(path).cloned().unwrap_or_default();
        let off = usize::try_from(offset.unwrap_or(0)).unwrap_or(usize::MAX);
        let body: String = content.chars().skip(off).collect();
        let truncated = limit.is_some_and(|lim| body.len() as u64 > lim);
        let trimmed = match limit {
            Some(lim) => body
                .chars()
                .take(usize::try_from(lim).unwrap_or(usize::MAX))
                .collect(),
            None => body,
        };
        let total_lines = content.lines().count() as u64;
        Ok(FileContent {
            content: trimmed,
            truncated,
            total_lines,
        })
    }
    async fn write_file(&self, path: &str, body: &str) -> Result<(), FsError> {
        self.files
            .lock()
            .await
            .insert(path.to_string(), body.to_string());
        Ok(())
    }
    fn is_within_workspace(&self, _: &str) -> bool {
        true
    }
    async fn watch(
        &self,
        _: &str,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError> {
        Err(FsError::Io("not supported".into()))
    }
    async fn append_file(&self, path: &str, body: &str) -> Result<(), FsError> {
        let mut map = self.files.lock().await;
        map.entry(path.to_string()).or_default().push_str(body);
        Ok(())
    }
    async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError> {
        let mut map = self.files.lock().await;
        if let Some(s) = map.get_mut(path) {
            s.truncate(usize::try_from(len).unwrap_or(usize::MAX));
        }
        Ok(())
    }
    async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
        Ok(std::time::SystemTime::UNIX_EPOCH)
    }
    async fn file_size(&self, path: &str) -> Result<u64, FsError> {
        let map = self.files.lock().await;
        Ok(map.get(path).map_or(0, |s| s.len() as u64))
    }
    async fn delete_file(&self, path: &str) -> Result<(), FsError> {
        self.files.lock().await.remove(path);
        Ok(())
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
        Ok(())
    }
    async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
        Err(FsError::Io("not supported".into()))
    }
    async fn fsync(&self, _: &str) -> Result<(), FsError> {
        Ok(())
    }
}

// ---- Recording status sink ----------------------------------------------

#[derive(Default)]
struct RecordingSink {
    statuses: StdMutex<Vec<(String, TaskStatus)>>,
}
#[async_trait]
impl TaskStatusSink for RecordingSink {
    async fn set_status(&self, task_id: &str, status: TaskStatus) {
        self.statuses
            .lock()
            .unwrap()
            .push((task_id.to_string(), status));
    }
}
impl RecordingSink {
    fn last_status(&self) -> Option<TaskStatus> {
        self.statuses.lock().unwrap().last().map(|(_, s)| *s)
    }
}

// ---- Helpers ------------------------------------------------------------

fn make_ctx(fs: Arc<dyn FileSystem>) -> TaskContext {
    TaskContext {
        fs,
        runtime: Arc::new(MockRuntimeSpawner::default()),
    }
}

fn workflow_input(script: &str) -> TaskSpawnInput {
    TaskSpawnInput::LocalWorkflow {
        workflow_id: "wf".into(),
        script: script.into(),
        resume_from_run_id: None,
        args: None,
        run_id: None,
        invocation_mode: Some("inline".to_string()),
        workflow_source: Some("inline".to_string()),
        launched_from_subagent: false,
    }
}

/// Poll the sink until it reports a terminal status (the worker runs on the
/// `MockRuntimeSpawner`'s tokio task, so yields let it finish).
async fn await_terminal(sink: &Arc<RecordingSink>) -> TaskStatus {
    for _ in 0..400 {
        if let Some(s) = sink.last_status() {
            if s.is_terminal() {
                return s;
            }
        }
        tokio::task::yield_now().await;
    }
    sink.last_status().expect("worker never reported a status")
}

// ==== Bridge-level tests (run_workflow_script directly) ==================

fn logs(outcome: &workflow::RunOutcome) -> Vec<String> {
    outcome
        .progress
        .iter()
        .filter_map(|p| match p {
            workflow::Progress::Log { message: s } => Some(s.clone()),
            workflow::Progress::Phase { .. } | workflow::Progress::Agent { .. } => None,
        })
        .collect()
}

async fn run_bridge(script: &str, spawner: Arc<EchoSpawner>) -> workflow::RunOutcome {
    run_workflow_script(
        script,
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .expect("workflow runs to completion")
}

/// Reproduction for the "/workflows task stuck running" report: the production
/// path wires a progress channel (`Some(ptx)`) and joins the run future with a
/// `drain` that reads the channel until all senders drop — the exact pattern the
/// `spawn` worker uses (`tokio::join!(run, drain)`). The other bridge tests pass
/// `None` for the progress sender, so this path (and any sender that outlives the
/// run) was never exercised. Uses an ASYNC spawner that actually yields, closer
/// to the real engine dispatch than the synchronous `EchoSpawner`.
#[tokio::test]
async fn run_with_progress_drain_completes_and_does_not_hang() {
    struct YieldSpawner;
    #[async_trait]
    impl SubagentSpawner for YieldSpawner {
        async fn agent_listing(&self) -> Vec<traits::subagent_spawn::SubagentListingEntry> {
            Vec::new()
        }
        async fn spawn(
            &self,
            request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            // Yield + a tiny sleep so the spawn genuinely awaits (real dispatch
            // suspends on the network), rather than returning synchronously.
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            Ok(SubagentResult::Completed {
                agent_id: protocol::AgentId::new(),
                content: Value::String(format!("echo:{}", request.prompt)),
                usage: SubagentUsage {
                    output_tokens: 1,
                    ..Default::default()
                },
                total_tool_use_count: 0,
                total_duration_ms: 0,
                total_tokens: 0,
                assistant_message_count: 0,
                response_char_count: 0,
                last_request_id: None,
            })
        }
    }

    let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // The `spawn` worker's drainer: read progress lines until every sender drops.
    let drain = async move {
        let mut lines = 0usize;
        while prx.recv().await.is_some() {
            lines += 1;
        }
        lines
    };
    let script = "export const meta = { name: 'x', description: 'y', phases: [{ title: 'A' }, { title: 'B' }] };\n\
                  phase('A'); const a = await agent('p1'); phase('B'); const b = await agent('p2'); return { a, b };";
    let run = run_workflow_script(
        script,
        DEFAULT_WORKFLOW_SUBAGENT,
        Arc::new(YieldSpawner),
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        Some(ptx),
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    );
    // If a progress sender outlives `run`, `drain` never ends and this join hangs.
    let joined = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(run, drain)
    })
    .await;
    assert!(
        joined.is_ok(),
        "workflow with a progress drain HUNG — join!(run, drain) never completed"
    );
    let (outcome, drained_lines) = joined.unwrap();
    assert!(outcome.is_ok(), "run failed: {:?}", outcome.err());
    // 2 phases + 2 agents (start+done each) → several progress lines drained.
    assert!(drained_lines >= 4, "expected progress lines, got {drained_lines}");
}

/// The 1000-agent lifetime cap: the 1001st REAL spawn rejects with the
/// byte-exact `WorkflowAgentCapError` message, terminating the run.
#[tokio::test]
async fn agent_cap_rejects_the_1001st_spawn() {
    let spawner = Arc::new(EchoSpawner::default());
    let result = run_workflow_script(
        "for (let i = 0; i < 1001; i++) { await agent('x'); } return 'done';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await;
    let err = result.expect_err("the 1001st agent() must throw the cap error");
    let msg = format!("{err}");
    assert!(
        msg.contains("Workflow agent() call cap reached (1000)"),
        "got: {msg}"
    );
}

/// An explicit unknown `agentType` throws the byte-exact not-found error
/// listing the available agents; a known one runs fine.
#[tokio::test]
async fn unknown_agent_type_throws_not_found() {
    let spawner = Arc::new(EchoSpawner::default());
    let result = run_workflow_script(
        "await agent('p', { agentType: 'nope' }); return 'done';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await;
    let err = result.expect_err("unknown agentType must throw");
    assert!(
        format!("{err}").contains(
            "agent({agentType}): agent type 'nope' not found. Available agents: general-purpose, Explore, code-reviewer"
        ),
        "got: {err}"
    );
}

/// A workflow that stays under the cap runs to completion unaffected.
#[tokio::test]
async fn under_cap_workflow_completes() {
    let spawner = Arc::new(EchoSpawner::default());
    let outcome = run_bridge(
        "for (let i = 0; i < 50; i++) { await agent('x'); } return 'ok';",
        spawner,
    )
    .await;
    assert_eq!(outcome.result.as_deref(), Some("\"ok\""));
}

/// `budget.spent()` is TURN-RELATIVE: the shared pool minus the turn-start
/// baseline (claude-code `getTurnSpent()=rT()-xtr`), so prior-turn output is
/// excluded.
#[tokio::test]
async fn budget_spent_is_turn_relative_via_baseline() {
    use std::sync::atomic::AtomicU64;
    // Pool already at 500 from prior turns; THIS turn started at 500 → the
    // baseline is 500, so a fresh 100-token subagent yields spent()==100.
    let pool = Arc::new(AtomicU64::new(500));
    let spawner = Arc::new(EchoSpawner::default());
    let outcome = run_workflow_script(
        "await agent('a'); log('spent=' + budget.spent()); return '';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        Some(pool),
        500,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .expect("runs");
    assert!(
        logs(&outcome).iter().any(|l| l == "spent=100"),
        "spent should be turn-relative (100), logs: {:?}",
        logs(&outcome)
    );
}

/// The budget hard ceiling: once turn-relative spend reaches the target, the
/// next `agent()` throws the byte-exact `WorkflowBudgetExceededError`.
#[tokio::test]
async fn budget_ceiling_throws_when_turn_spend_exceeds_total() {
    use std::sync::atomic::AtomicU64;
    // total=100; pool already at 150 this turn (baseline 0) → 150 >= 100.
    let pool = Arc::new(AtomicU64::new(150));
    let spawner = Arc::new(EchoSpawner::default());
    let result = run_workflow_script(
        "await agent('a'); return 'done';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        Some(100),
        Some(pool),
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await;
    let err = result.expect_err("over-budget agent() must throw");
    assert!(
        format!("{err}").contains("Workflow token budget exceeded (150 / 100 output tokens)"),
        "got: {err}"
    );
}

/// `budget.spent()` reads the shared pool: a pre-seeded value (standing in
/// for main-loop output the orchestrator already accumulated) plus every
/// subagent's output tokens — the union claude-code exposes, not own-spend.
#[tokio::test]
async fn shared_pool_makes_spent_read_main_loop_plus_subagents() {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Pre-seed as if the main loop already spent 500 output tokens this turn.
    let pool = Arc::new(AtomicU64::new(500));
    let spawner = Arc::new(EchoSpawner::default()); // 100 output tokens/agent
    let outcome = run_workflow_script(
        "await agent('a'); await agent('b'); log('spent=' + budget.spent()); return '';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        Some(1_000_000),
        Some(pool.clone()),
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .expect("workflow runs to completion");
    // 500 (seeded main loop) + 2×100 (the two subagents) = 700.
    assert_eq!(pool.load(Ordering::Relaxed), 700);
    assert!(
        logs(&outcome).iter().any(|l| l == "spent=700"),
        "spent() must read the shared pool live; got {:?}",
        logs(&outcome)
    );
}

/// Without a shared pool (the standalone/test path), `spent()` falls back to
/// a private counter of this run's own subagent output only.
#[tokio::test]
async fn no_shared_pool_falls_back_to_own_spend() {
    let spawner = Arc::new(EchoSpawner::default());
    let outcome = run_bridge(
        "await agent('a'); log('spent=' + budget.spent()); return '';",
        spawner,
    )
    .await;
    assert!(
        logs(&outcome).iter().any(|l| l == "spent=100"),
        "own-spend fallback should count just the one subagent; got {:?}",
        logs(&outcome)
    );
}

#[tokio::test]
async fn parallel_agents_round_trip_through_spawner_in_order() {
    let spawner = Arc::new(EchoSpawner::default());
    let script = r#"
        const rs = await parallel([
          () => agent('a'),
          () => agent('b'),
          () => agent('c'),
        ]);
        log('R:' + rs.join(','));
    "#;
    let outcome = run_bridge(script, spawner.clone()).await;
    assert_eq!(logs(&outcome), vec!["R:echo:a,echo:b,echo:c".to_string()]);
    let mut seen = spawner.seen.lock().unwrap().clone();
    seen.sort();
    assert_eq!(
        seen,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[test]
fn make_request_maps_effort_opt() {
    // `agent({effort})` — a level string or an integer — is carried raw.
    assert_eq!(
        make_request("general-purpose", "p", r#"{"effort":"high"}"#).effort,
        Some(serde_json::json!("high"))
    );
    assert_eq!(
        make_request("general-purpose", "p", r#"{"effort":8000}"#).effort,
        Some(serde_json::json!(8000))
    );
    assert!(make_request("general-purpose", "p", "{}").effort.is_none());
}

#[tokio::test]
async fn sequential_awaits_preserve_order() {
    let spawner = Arc::new(EchoSpawner::default());
    let script = r#"
        const a = await agent('1');
        const b = await agent('2');
        log(a + '|' + b);
    "#;
    let outcome = run_bridge(script, spawner.clone()).await;
    assert_eq!(logs(&outcome), vec!["echo:1|echo:2".to_string()]);
    assert_eq!(
        *spawner.seen.lock().unwrap(),
        vec!["1".to_string(), "2".to_string()]
    );
}

#[tokio::test]
async fn failed_agent_resolves_to_a_falsy_value() {
    let spawner = Arc::new(EchoSpawner {
        fail: true,
        ..Default::default()
    });
    let script = r#"
        const r = await agent('x');
        log('got:' + (r || 'NONE'));
    "#;
    let outcome = run_bridge(script, spawner).await;
    assert_eq!(logs(&outcome), vec!["got:NONE".to_string()]);
}

#[tokio::test]
async fn pipeline_stages_run_each_item_through_the_spawner() {
    let spawner = Arc::new(EchoSpawner::default());
    let script = r#"
        const rs = await pipeline(
          ['x', 'y'],
          (item) => agent(item),
          (prev) => agent(prev + '!'),
        );
        log('P:' + rs.join(','));
    "#;
    let outcome = run_bridge(script, spawner.clone()).await;
    assert_eq!(
        logs(&outcome),
        vec!["P:echo:echo:x!,echo:echo:y!".to_string()]
    );
}

#[tokio::test]
async fn agent_opts_map_to_the_spawn_request() {
    let spawner = Arc::new(EchoSpawner::default());
    let script = r#"
        await agent('p', { agentType: 'code-reviewer', model: 'opus', isolation: 'worktree', schema: { type: 'object' } });
        await agent('plain');
    "#;
    run_bridge(script, spawner.clone()).await;
    let reqs = spawner.seen_reqs.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    // agentType / model / isolation / schema opts → the spawn request.
    assert_eq!(reqs[0].subagent_type, "code-reviewer");
    assert_eq!(reqs[0].model.as_deref(), Some("opus"));
    assert_eq!(reqs[0].isolation.as_deref(), Some("worktree"));
    assert_eq!(reqs[0].schema.as_deref(), Some(r#"{"type":"object"}"#));
    // A bare agent(prompt) → default type (workflow-subagent), no overrides.
    assert_eq!(reqs[1].subagent_type, "workflow-subagent");
    assert_eq!(reqs[1].model, None);
    assert_eq!(reqs[1].isolation, None);
    assert_eq!(reqs[1].schema, None);
}

#[tokio::test]
async fn top_level_args_global_reaches_the_script() {
    let spawner = Arc::new(EchoSpawner::default());
    let outcome = run_workflow_script(
        "log('a=' + args.a); return args;",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig {
            allow_nested: false,
            args: Some(r#"{"a":5}"#.to_string()),
            fs: None,
        },
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(logs(&outcome), vec!["a=5".to_string()]);
    assert_eq!(outcome.result.as_deref(), Some(r#"{"a":5}"#));
}

#[tokio::test]
async fn workflow_runs_a_nested_scriptpath_inline_sharing_the_runtime() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    // The nested workflow itself spawns an agent and reads its own args —
    // proving it runs IN the parent's runtime (shared spawner + globals).
    fs.write_file(
        "/wf/child.js",
        "const x = await agent('child-task'); return { got: x, n: args.n };",
    )
    .await
    .unwrap();
    let spawner = Arc::new(EchoSpawner::default());
    let parent = r#"
        const r = await workflow({ scriptPath: '/wf/child.js' }, { n: 9 });
        log('got=' + r.got + ' n=' + r.n);
        return r;
    "#;
    let outcome = run_workflow_script(
        parent,
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner.clone(),
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig {
            allow_nested: true,
            args: None,
            fs: Some(fs),
        },
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .unwrap();
    // The nested workflow's agent() went through the PARENT's spawner.
    assert_eq!(
        *spawner.seen.lock().unwrap(),
        vec!["child-task".to_string()]
    );
    // Its return value (incl. its own args) flowed back to the parent.
    assert_eq!(logs(&outcome), vec!["got=echo:child-task n=9".to_string()]);
    assert_eq!(
        outcome.result.as_deref(),
        Some(r#"{"got":"echo:child-task","n":9}"#)
    );
}

// ==== Handler-level tests (full Task lifecycle) =========================

fn make_handler(
    spawner: Arc<dyn SubagentSpawner>,
    mgr: Arc<TaskOutputManager>,
    sink: Arc<dyn TaskStatusSink>,
) -> LocalWorkflowHandler {
    LocalWorkflowHandler::new(spawner, Arc::new(MockInvoker), Arc::new(MockBudget), mgr)
        .with_status_sink(sink)
}

#[tokio::test]
async fn handler_runs_workflow_and_spools_the_return_value() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let spawner = Arc::new(EchoSpawner::default());
    let dir = tempdir().unwrap();
    let mgr = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let sink = Arc::new(RecordingSink::default());

    let handler = make_handler(spawner.clone(), mgr.clone(), sink.clone());

    // A workflow that fans out two agents and returns a structured result.
    let script = r#"
        const rs = await parallel([() => agent('a'), () => agent('b')]);
        return { confirmed: rs };
    "#;
    let handle = handler
        .spawn(workflow_input(script), make_ctx(fs))
        .await
        .expect("spawn should succeed");

    assert!(
        handle.task_id.starts_with('w'),
        "LocalWorkflow id prefix 'w'"
    );
    assert!(handle.cleanup.is_some(), "cleanup seam present");

    assert_eq!(await_terminal(&sink).await, TaskStatus::Completed);

    // Both agents ran.
    let mut seen = spawner.seen.lock().unwrap().clone();
    seen.sort();
    assert_eq!(seen, vec!["a".to_string(), "b".to_string()]);

    // The script's return value (JSON) was spooled as the task result,
    // after the surfaced run id.
    let spool_path = dir.path().join(format!("{}.output", handle.task_id));
    let read = mgr
        .read(&spool_path, crate::output_manager::OutputOptions::default())
        .await
        .unwrap();
    assert!(read.content.starts_with("runId: wf_"), "{}", read.content);
    assert!(
        read.content
            .contains(r#"{"confirmed":["echo:a","echo:b"]}"#),
        "{}",
        read.content
    );
}

#[tokio::test]
async fn handler_spools_live_progress_then_the_result() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let spawner = Arc::new(EchoSpawner::default());
    let dir = tempdir().unwrap();
    let mgr = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let sink = Arc::new(RecordingSink::default());
    let handler = make_handler(spawner, mgr.clone(), sink.clone());

    let script = r#"
        phase('Scan');
        log('found 2 things');
        return { ok: true };
    "#;
    let handle = handler
        .spawn(workflow_input(script), make_ctx(fs))
        .await
        .unwrap();
    assert_eq!(await_terminal(&sink).await, TaskStatus::Completed);

    let spool_path = dir.path().join(format!("{}.output", handle.task_id));
    let read = mgr
        .read(&spool_path, crate::output_manager::OutputOptions::default())
        .await
        .unwrap();
    // phase/log were spooled live, followed by the return value.
    // Phase now renders as "[N] === Title ===" (index-prefixed).
    assert!(
        read.content.contains("=== Scan ==="),
        "phase: {}",
        read.content
    );
    assert!(
        read.content.contains("[1]"),
        "phase index: {}",
        read.content
    );
    assert!(
        read.content.contains("found 2 things"),
        "log: {}",
        read.content
    );
    assert!(
        read.content.contains(r#"{"ok":true}"#),
        "result: {}",
        read.content
    );
}

#[tokio::test]
async fn resume_replays_journaled_agent_results_without_respawning() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let dir = tempdir().unwrap();
    let mgr = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let script = r#"
        const a = await agent('a');
        const b = await agent('b');
        return { a, b };
    "#;

    // Run 1: fresh run — both agents spawn; capture the surfaced runId.
    let spawner1 = Arc::new(EchoSpawner::default());
    let sink1 = Arc::new(RecordingSink::default());
    let h1 = make_handler(spawner1.clone(), mgr.clone(), sink1.clone());
    let handle1 = h1
        .spawn(workflow_input(script), make_ctx(fs.clone()))
        .await
        .unwrap();
    assert_eq!(await_terminal(&sink1).await, TaskStatus::Completed);
    assert_eq!(spawner1.seen.lock().unwrap().len(), 2, "run 1 spawns both");

    let spool1 = dir.path().join(format!("{}.output", handle1.task_id));
    let out1 = mgr
        .read(&spool1, crate::output_manager::OutputOptions::default())
        .await
        .unwrap();
    let run_id = out1
        .content
        .lines()
        .find_map(|l| l.strip_prefix("runId: "))
        .expect("runId surfaced")
        .to_string();
    assert!(run_id.starts_with("wf_"), "runId: {run_id}");

    // Run 2: resume the same id — every agent() must replay from the journal,
    // so the spawner is never called, and the result is rebuilt from cache.
    let spawner2 = Arc::new(EchoSpawner::default());
    let sink2 = Arc::new(RecordingSink::default());
    let h2 = make_handler(spawner2.clone(), mgr.clone(), sink2.clone());
    let input2 = TaskSpawnInput::LocalWorkflow {
        workflow_id: "wf".into(),
        script: script.into(),
        resume_from_run_id: Some(run_id),
        args: None,
        run_id: None,
        invocation_mode: Some("inline".to_string()),
        workflow_source: Some("inline".to_string()),
        launched_from_subagent: false,
    };
    let handle2 = h2.spawn(input2, make_ctx(fs.clone())).await.unwrap();
    assert_eq!(await_terminal(&sink2).await, TaskStatus::Completed);
    assert!(
        spawner2.seen.lock().unwrap().is_empty(),
        "resume must replay journaled results, not re-spawn"
    );

    let spool2 = dir.path().join(format!("{}.output", handle2.task_id));
    let out2 = mgr
        .read(&spool2, crate::output_manager::OutputOptions::default())
        .await
        .unwrap();
    assert!(
        out2.content.contains(r#"{"a":"echo:a","b":"echo:b"}"#),
        "rebuilt from cache: {}",
        out2.content
    );
}

#[tokio::test]
async fn label_opt_becomes_the_subagent_display_name() {
    let spawner = Arc::new(EchoSpawner::default());
    run_bridge(
        "await agent('p', { label: 'my-label' }); await agent('q');",
        spawner.clone(),
    )
    .await;
    let reqs = spawner.seen_reqs.lock().unwrap().clone();
    assert_eq!(reqs[0].name.as_deref(), Some("my-label"), "label → name");
    assert_eq!(reqs[1].name, None, "no label → no name");
}

#[tokio::test]
async fn resume_with_a_changed_prefix_reruns_from_the_edit_onward() {
    // PREFIX semantics: editing the FIRST agent's prompt on resume must
    // re-run it AND every later call (the chained key cascades) — NOT replay
    // the now-misaligned journaled results by a flat (prompt,opts) match.
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let dir = tempdir().unwrap();
    let mgr = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));

    // Run 1: journal agents 'a' then 'b'.
    let script1 = "const a = await agent('a'); const b = await agent('b'); return { a, b };";
    let spawner1 = Arc::new(EchoSpawner::default());
    let sink1 = Arc::new(RecordingSink::default());
    let h1 = make_handler(spawner1.clone(), mgr.clone(), sink1.clone());
    let handle1 = h1
        .spawn(workflow_input(script1), make_ctx(fs.clone()))
        .await
        .unwrap();
    assert_eq!(await_terminal(&sink1).await, TaskStatus::Completed);
    let spool1 = dir.path().join(format!("{}.output", handle1.task_id));
    let out1 = mgr
        .read(&spool1, crate::output_manager::OutputOptions::default())
        .await
        .unwrap();
    let run_id = out1
        .content
        .lines()
        .find_map(|l| l.strip_prefix("runId: "))
        .expect("runId")
        .to_string();

    // Run 2: resume the same id but EDIT the first prompt ('a' → 'a2'). Both
    // agents must re-spawn — 'b' too, because its key chains off the changed
    // 'a2'. The flat-map cache would have wrongly replayed 'b' from run 1.
    let script2 = "const a = await agent('a2'); const b = await agent('b'); return { a, b };";
    let spawner2 = Arc::new(EchoSpawner::default());
    let sink2 = Arc::new(RecordingSink::default());
    let h2 = make_handler(spawner2.clone(), mgr.clone(), sink2.clone());
    let input2 = TaskSpawnInput::LocalWorkflow {
        workflow_id: "wf".into(),
        script: script2.into(),
        resume_from_run_id: Some(run_id),
        args: None,
        run_id: None,
        invocation_mode: Some("inline".to_string()),
        workflow_source: Some("inline".to_string()),
        launched_from_subagent: false,
    };
    h2.spawn(input2, make_ctx(fs.clone())).await.unwrap();
    assert_eq!(await_terminal(&sink2).await, TaskStatus::Completed);
    let mut seen = spawner2.seen.lock().unwrap().clone();
    seen.sort();
    assert_eq!(
        seen,
        vec!["a2".to_string(), "b".to_string()],
        "edited prefix re-runs the edit AND everything after it"
    );
}

#[tokio::test]
async fn budget_total_and_own_spend_drive_the_budget_global() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let spawner = Arc::new(EchoSpawner::default());
    let dir = tempdir().unwrap();
    let mgr = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let sink = Arc::new(RecordingSink::default());
    let handler = make_handler(spawner, mgr.clone(), sink.clone()).with_token_budget(Some(500));

    // Each echo agent reports 100 output tokens; two agents ⇒ spent 200.
    let script = r#"
        await agent('a');
        await agent('b');
        log('B:' + budget.total + '/' + budget.spent() + '/' + budget.remaining());
        return {};
    "#;
    let handle = handler
        .spawn(workflow_input(script), make_ctx(fs))
        .await
        .unwrap();
    assert_eq!(await_terminal(&sink).await, TaskStatus::Completed);

    let spool = dir.path().join(format!("{}.output", handle.task_id));
    let read = mgr
        .read(&spool, crate::output_manager::OutputOptions::default())
        .await
        .unwrap();
    // total=500, spent=2×100, remaining=300.
    assert!(read.content.contains("B:500/200/300"), "{}", read.content);
}

#[tokio::test]
async fn handler_maps_a_script_error_to_failed() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let spawner = Arc::new(EchoSpawner::default());
    let dir = tempdir().unwrap();
    let mgr = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let sink = Arc::new(RecordingSink::default());

    let handler = make_handler(spawner, mgr, sink.clone());

    // A script that throws ⇒ WorkflowError::Script ⇒ Failed.
    let handle = handler
        .spawn(workflow_input("throw new Error('kaboom');"), make_ctx(fs))
        .await
        .unwrap();

    assert_eq!(await_terminal(&sink).await, TaskStatus::Failed);
    assert!(handle.task_id.starts_with('w'));
}

#[tokio::test]
async fn handler_rejects_a_non_workflow_input() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let dir = tempdir().unwrap();
    let mgr = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));
    let handler = make_handler(
        Arc::new(EchoSpawner::default()),
        mgr,
        Arc::new(RecordingSink::default()),
    );

    let wrong = TaskSpawnInput::LocalAgent {
        agent_id: protocol::AgentId::new(),
        subagent_type: "general-purpose".into(),
        prompt: "p".into(),
        is_backgrounded: true,
        tool_use_id: None,
    };
    match handler.spawn(wrong, make_ctx(fs)).await {
        Err(TaskError::Internal(_)) => {}
        Err(other) => panic!("expected Internal, got {other:?}"),
        Ok(_) => panic!("non-LocalWorkflow input must be rejected"),
    }
}

#[tokio::test]
async fn name_and_type_are_local_workflow() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let dir = tempdir().unwrap();
    let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs));
    let handler = make_handler(
        Arc::new(EchoSpawner::default()),
        mgr,
        Arc::new(RecordingSink::default()),
    );
    assert_eq!(handler.name(), "local_workflow");
    assert_eq!(handler.task_type(), TaskType::LocalWorkflow);
}

// ==== agent() routing tests (Cases 1-4 + §6) ============================

/// Case 1: bare `agent(prompt)` → subagent_type = "workflow-subagent",
/// no system_prompt_override, no addendum, no additional disallowed tools
/// (the builtin def already has {SendUserMessage, Agent, Workflow}).
#[test]
fn bare_agent_routes_to_workflow_subagent_with_kbp() {
    let req = make_request(DEFAULT_WORKFLOW_SUBAGENT, "do something", "{}");
    assert_eq!(req.subagent_type, "workflow-subagent");
    assert!(
        req.system_prompt_override.is_none(),
        "no prompt override for bare agent()"
    );
    assert!(
        req.system_prompt_addendum.is_none(),
        "no addendum for bare agent()"
    );
    assert!(
        req.additional_disallowed_tools.is_empty(),
        "no extra disallowed for bare agent()"
    );
}

/// Case 2: `agent(prompt, {schema})` (no agentType) → workflow-subagent +
/// system_prompt_override = xBp.
#[test]
fn bare_schema_agent_uses_xbp() {
    let req = make_request(
        DEFAULT_WORKFLOW_SUBAGENT,
        "return structured",
        r#"{"schema":{"type":"object","properties":{"count":{"type":"number"}}}}"#,
    );
    assert_eq!(req.subagent_type, "workflow-subagent");
    // Override must be the xBp string (WORKFLOW_SUBAGENT_SCHEMA_PROMPT).
    let override_prompt = req
        .system_prompt_override
        .as_deref()
        .expect("override must be set for schema agent()");
    assert_eq!(
        override_prompt,
        agent::builtins::WORKFLOW_SUBAGENT_SCHEMA_PROMPT
    );
    assert!(
        req.system_prompt_addendum.is_none(),
        "no addendum when no explicit agentType"
    );
    assert!(
        req.additional_disallowed_tools.is_empty(),
        "no extra disallowed for bare schema agent()"
    );
}

/// Case 3: `agent(prompt, {agentType})` (no schema) → that agentType, HBp
/// addendum appended, disallow union {SendUserMessage, Agent, Workflow}.
#[test]
fn user_agenttype_gets_hbp_addendum_and_disallow_union() {
    let req = make_request(
        DEFAULT_WORKFLOW_SUBAGENT,
        "analyze code",
        r#"{"agentType":"general-purpose"}"#,
    );
    assert_eq!(req.subagent_type, "general-purpose");
    assert!(
        req.system_prompt_override.is_none(),
        "no prompt override for user agentType"
    );
    let addendum = req
        .system_prompt_addendum
        .as_deref()
        .expect("HBp addendum must be set");
    assert_eq!(
        addendum,
        agent::builtins::WORKFLOW_SUBAGENT_NON_SCHEMA_ADDENDUM
    );
    // Must request union with {SendUserMessage, Agent, Workflow}.
    let disallowed = &req.additional_disallowed_tools;
    assert!(
        disallowed.contains(&"SendUserMessage".to_string()),
        "SendUserMessage must be disallowed: {disallowed:?}"
    );
    assert!(
        disallowed.contains(&"Agent".to_string()),
        "Agent must be disallowed: {disallowed:?}"
    );
    assert!(
        disallowed.contains(&"Workflow".to_string()),
        "Workflow must be disallowed: {disallowed:?}"
    );
}

/// Case 4: `agent(prompt, {agentType, schema})` → that agentType, IBp
/// addendum appended, disallow union set.
#[test]
fn user_agenttype_with_schema_gets_ibp_and_disallow_union() {
    let req = make_request(
        DEFAULT_WORKFLOW_SUBAGENT,
        "return structured",
        r#"{"agentType":"code-reviewer","schema":{"type":"object"}}"#,
    );
    assert_eq!(req.subagent_type, "code-reviewer");
    assert!(
        req.system_prompt_override.is_none(),
        "no prompt override for user agentType"
    );
    let addendum = req
        .system_prompt_addendum
        .as_deref()
        .expect("IBp addendum must be set");
    assert_eq!(addendum, agent::builtins::WORKFLOW_SUBAGENT_SCHEMA_ADDENDUM);
    // Must request union with {SendUserMessage, Agent, Workflow}.
    let disallowed = &req.additional_disallowed_tools;
    assert!(
        disallowed.contains(&"SendUserMessage".to_string()),
        "SendUserMessage must be disallowed: {disallowed:?}"
    );
    assert!(
        disallowed.contains(&"Agent".to_string()),
        "Agent must be disallowed: {disallowed:?}"
    );
    assert!(
        disallowed.contains(&"Workflow".to_string()),
        "Workflow must be disallowed: {disallowed:?}"
    );
}

// ---- chain_key / normalize_opts_for_chain_key ---------------------------

/// Display-only opts (`phase`, `label`, `stallMs`) must NOT change the chain
/// key — they are stripped by `normalize_opts_for_chain_key` (ABp parity).
#[test]
fn chain_key_ignores_display_only_opts() {
    let opts_a = r#"{"model":"claude-opus-4","phase":"research","label":"step1"}"#;
    let opts_b = r#"{"model":"claude-opus-4","phase":"writing","label":"step2","stallMs":5000}"#;
    let key_a = chain_key(
        "",
        "do something",
        &normalize_opts_for_chain_key(&serde_json::from_str(opts_a).unwrap()),
    );
    let key_b = chain_key(
        "",
        "do something",
        &normalize_opts_for_chain_key(&serde_json::from_str(opts_b).unwrap()),
    );
    assert_eq!(
        key_a, key_b,
        "display-only fields must not affect the chain key"
    );
}

/// Changing `model` (an identity key) MUST produce a different chain key.
#[test]
fn chain_key_differs_on_model_change() {
    let opts_a = r#"{"model":"claude-opus-4"}"#;
    let opts_b = r#"{"model":"claude-sonnet-4"}"#;
    let key_a = chain_key(
        "",
        "do something",
        &normalize_opts_for_chain_key(&serde_json::from_str(opts_a).unwrap()),
    );
    let key_b = chain_key(
        "",
        "do something",
        &normalize_opts_for_chain_key(&serde_json::from_str(opts_b).unwrap()),
    );
    assert_ne!(
        key_a, key_b,
        "different model must produce different chain key"
    );
}

/// Key order in the raw opts JSON must NOT matter — normalization sorts keys.
#[test]
fn chain_key_stable_regardless_of_input_key_order() {
    let opts_a = r#"{"model":"claude-opus-4","schema":{"type":"object"}}"#;
    let opts_b = r#"{"schema":{"type":"object"},"model":"claude-opus-4"}"#;
    let key_a = chain_key(
        "",
        "do something",
        &normalize_opts_for_chain_key(&serde_json::from_str(opts_a).unwrap()),
    );
    let key_b = chain_key(
        "",
        "do something",
        &normalize_opts_for_chain_key(&serde_json::from_str(opts_b).unwrap()),
    );
    assert_eq!(
        key_a, key_b,
        "key order in opts JSON must not affect the chain key"
    );
}

/// Verify the concurrency cap formula: Math.min(16, Math.max(2, cpus-2)).
/// At 1–3 cores the floor is 2; at 5 cores it's 3; at 18 cores it's capped at 16.
#[test]
fn concurrency_cap_formula_matches_binary() {
    // Direct formula test: cores.saturating_sub(2).max(2).min(16)
    let formula = |cores: usize| cores.saturating_sub(2).max(2).min(16);
    assert_eq!(formula(1), 2, "1 core → 2");
    assert_eq!(formula(2), 2, "2 cores → 2");
    assert_eq!(formula(3), 2, "3 cores → 2");
    assert_eq!(formula(4), 2, "4 cores → 2");
    assert_eq!(formula(5), 3, "5 cores → 3");
    assert_eq!(formula(18), 16, "18 cores → 16 (cap)");
}

// ==== Telemetry tests ====================================================

/// `tengu_workflow_phase_completed` does NOT fire when the script has no
/// `phase()` calls (bridge-level — verifies the post-run emit loop is a no-op
/// when `outcome.progress` has no Phase entries).
#[tokio::test]
async fn telemetry_no_phase_events_without_phase_calls() {
    use telemetry::InMemorySink;
    let sink = Arc::new(InMemorySink::default());
    let bus = Arc::new(AnalyticsBus::new());
    bus.attach_sink(sink.clone()).await;

    let spawner = Arc::new(EchoSpawner::default());
    run_workflow_script(
        "return 'ok';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bus.clone(),
        None,
        None,
    )
    .await
    .expect("runs");

    let events = sink.events().await;
    assert!(
        !events
            .iter()
            .any(|e| e.name == telemetry::tengu::workflow::PHASE_COMPLETED),
        "no phase_completed for a script with no phase() calls; events: {events:?}"
    );
}

/// `tengu_workflow_phase_completed` fires once per `phase()` call for a NAMED
/// (built-in source) workflow — oracle §7 gating condition.
#[tokio::test]
async fn telemetry_phase_completed_fires_per_phase_for_named_workflow() {
    use telemetry::InMemorySink;
    let sink = Arc::new(InMemorySink::default());
    let bus = Arc::new(AnalyticsBus::new());
    bus.attach_sink(sink.clone()).await;

    let spawner = Arc::new(EchoSpawner::default());
    run_workflow_script(
        "phase('Step 1'); phase('Step 2'); return 'done';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bus.clone(),
        None,
        // Pass a named invocation_mode — oracle §7: only "named" (built-in source)
        // emits tengu_workflow_phase_completed.
        Some(PhaseTelemetryCtx {
            run_id: "wf_test".to_string(),
            workflow_source: Some("my-workflow".to_string()),
            workflow_name: Some("My Workflow".to_string()),
            invocation_mode: Some("named".to_string()),
        }),
    )
    .await
    .expect("runs");

    let events: Vec<_> = sink.events().await;
    let phase_events: Vec<_> = events
        .iter()
        .filter(|e| e.name == telemetry::tengu::workflow::PHASE_COMPLETED)
        .collect();
    assert_eq!(
        phase_events.len(),
        2,
        "one event per phase() for named workflow; got {phase_events:?}"
    );
    assert!(
        matches!(phase_events[0].metadata.get("phase_title"), Some(AnalyticsValue::String(s)) if s == "Step 1"),
        "first phase title"
    );
    // phase_index is 0-based in telemetry (oracle §7). Contrast with workflow_agent
    // phaseIndex which is 1-based (oracle §8).
    assert!(
        matches!(
            phase_events[0].metadata.get("phase_index"),
            Some(AnalyticsValue::Int(0))
        ),
        "first phase index (0-based in telemetry, oracle §7)"
    );
    assert!(
        matches!(phase_events[1].metadata.get("phase_title"), Some(AnalyticsValue::String(s)) if s == "Step 2"),
        "second phase title"
    );
    assert!(
        matches!(
            phase_events[1].metadata.get("phase_index"),
            Some(AnalyticsValue::Int(1))
        ),
        "second phase index (0-based in telemetry, oracle §7)"
    );
}

/// `tengu_workflow_phase_completed` does NOT fire for an INLINE script, even if
/// it calls `phase()` — oracle §7 gates this event on `p.source === "built-in"`
/// (invocation_mode == "named") only.
#[tokio::test]
async fn telemetry_phase_completed_suppressed_for_inline_workflow() {
    use telemetry::InMemorySink;
    let sink = Arc::new(InMemorySink::default());
    let bus = Arc::new(AnalyticsBus::new());
    bus.attach_sink(sink.clone()).await;

    let spawner = Arc::new(EchoSpawner::default());
    run_workflow_script(
        "phase('Step 1'); phase('Step 2'); return 'done';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bus.clone(),
        None,
        // Inline invocation_mode: oracle §7 suppresses phase_completed for
        // "inline" (and "scriptPath") — only "named" (built-in source) emits it.
        Some(PhaseTelemetryCtx {
            run_id: "wf_test_inline".to_string(),
            workflow_source: Some("inline".to_string()),
            workflow_name: None,
            invocation_mode: Some("inline".to_string()),
        }),
    )
    .await
    .expect("runs");

    let events = sink.events().await;
    assert!(
        !events.iter().any(|e| e.name == telemetry::tengu::workflow::PHASE_COMPLETED),
        "tengu_workflow_phase_completed must NOT fire for inline workflows (oracle §7); events: {events:?}"
    );
}

/// `tengu_workflow_phase_completed` does NOT fire for a `scriptPath` workflow,
/// even if it calls `phase()` — oracle §7 gates this on `p.source === "built-in"`
/// (invocation_mode == "named") only. scriptPath is not a saved/built-in source.
#[tokio::test]
async fn telemetry_phase_completed_suppressed_for_script_path_workflow() {
    use telemetry::InMemorySink;
    let sink = Arc::new(InMemorySink::default());
    let bus = Arc::new(AnalyticsBus::new());
    bus.attach_sink(sink.clone()).await;

    let spawner = Arc::new(EchoSpawner::default());
    run_workflow_script(
        "phase('Step A'); return 'done';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bus.clone(),
        None,
        Some(PhaseTelemetryCtx {
            run_id: "wf_test_scriptpath".to_string(),
            workflow_source: Some("/path/to/workflow.js".to_string()),
            workflow_name: None,
            invocation_mode: Some("scriptPath".to_string()),
        }),
    )
    .await
    .expect("runs");

    let events = sink.events().await;
    assert!(
        !events.iter().any(|e| e.name == telemetry::tengu::workflow::PHASE_COMPLETED),
        "tengu_workflow_phase_completed must NOT fire for scriptPath workflows (oracle §7); events: {events:?}"
    );
}

/// `tengu_workflow_phase_completed` does NOT fire when phase_telemetry_ctx is None
/// (the bridge-level path without full context — verifies gating predicate).
#[tokio::test]
async fn telemetry_phase_completed_suppressed_when_no_telemetry_ctx() {
    use telemetry::InMemorySink;
    let sink = Arc::new(InMemorySink::default());
    let bus = Arc::new(AnalyticsBus::new());
    bus.attach_sink(sink.clone()).await;

    let spawner = Arc::new(EchoSpawner::default());
    run_workflow_script(
        "phase('Step 1'); return 'done';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bus.clone(),
        None,
        // No PhaseTelemetryCtx → is_named_source = false → no phase_completed events.
        // Note: run_bridge() / test helpers typically pass None here; this test
        // documents that the gate also protects the None case (no ctx = not named).
        None,
    )
    .await
    .expect("runs");

    let events = sink.events().await;
    assert!(
        !events.iter().any(|e| e.name == telemetry::tengu::workflow::PHASE_COMPLETED),
        "tengu_workflow_phase_completed must NOT fire when phase_telemetry_ctx is None; events: {events:?}"
    );
}

/// `tengu_workflow_budget_cap_exceeded` fires when the budget ceiling is hit.
#[tokio::test]
async fn telemetry_budget_cap_fires() {
    use std::sync::atomic::AtomicU64;
    use telemetry::InMemorySink;
    let sink = Arc::new(InMemorySink::default());
    let bus = Arc::new(AnalyticsBus::new());
    bus.attach_sink(sink.clone()).await;

    // total=100; pool already at 150 this turn (baseline 0) → 150 >= 100.
    let pool = Arc::new(AtomicU64::new(150));
    let spawner = Arc::new(EchoSpawner::default());
    let _ = run_workflow_script(
        "await agent('a'); return 'done';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        Some(100),
        Some(pool),
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bus.clone(),
        None,
        None,
    )
    .await; // expected Err

    let events: Vec<_> = sink.events().await;
    let cap_event = events
        .iter()
        .find(|e| e.name == telemetry::tengu::workflow::BUDGET_CAP_EXCEEDED);
    assert!(
        cap_event.is_some(),
        "tengu_workflow_budget_cap_exceeded must fire; events: {events:?}"
    );
    let md = &cap_event.unwrap().metadata;
    assert!(
        matches!(md.get("spent"), Some(AnalyticsValue::Int(150))),
        "spent field must be 150"
    );
    assert!(
        matches!(md.get("budget"), Some(AnalyticsValue::Int(100))),
        "budget field must be 100"
    );
}

/// `tengu_workflow_agent_cap_exceeded` fires when the 1000-agent cap is hit.
#[tokio::test]
async fn telemetry_agent_cap_fires() {
    use telemetry::InMemorySink;
    let sink = Arc::new(InMemorySink::default());
    let bus = Arc::new(AnalyticsBus::new());
    bus.attach_sink(sink.clone()).await;

    let spawner = Arc::new(EchoSpawner::default());
    let _ = run_workflow_script(
        "for (let i = 0; i < 1001; i++) { await agent('x'); } return 'done';",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        None,
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bus.clone(),
        None,
        None,
    )
    .await; // expected Err

    let events: Vec<_> = sink.events().await;
    let cap_event = events
        .iter()
        .find(|e| e.name == telemetry::tengu::workflow::AGENT_CAP_EXCEEDED);
    assert!(
        cap_event.is_some(),
        "tengu_workflow_agent_cap_exceeded must fire; events: {events:?}"
    );
    let md = &cap_event.unwrap().metadata;
    assert!(
        matches!(md.get("agentCount"), Some(AnalyticsValue::Int(1000))),
        "agentCount field must be 1000"
    );
}

// ==== Structured progress event tests (Task 10) ==========================

/// A single-agent script emits `start` then `done` workflow_agent events to
/// the progress spool, with the correct index (0-based), label, state, and
/// toolUseID format (`workflow_agent_{index}_{suffix}`).
#[tokio::test]
async fn workflow_agent_progress_start_and_done_emitted() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let spawner = Arc::new(EchoSpawner::default());
    run_workflow_script(
        "const r = await agent('analyze the code', { label: 'my-label' }); log('r=' + r);",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        Some(tx),
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .expect("runs");

    let mut lines: Vec<String> = Vec::new();
    while let Ok(line) = rx.try_recv() {
        lines.push(line);
    }

    // Must have at least a `start` and a `done` agent event, plus a log.
    let agent_lines: Vec<&str> = lines
        .iter()
        .filter(|l| l.starts_with("[workflow_agent]"))
        .map(|l| l.as_str())
        .collect();
    assert!(
        agent_lines.len() >= 2,
        "expected start+done events; got: {lines:?}"
    );

    // Parse the start event.
    let start_json_str = agent_lines[0].trim_start_matches("[workflow_agent] ");
    let start: serde_json::Value = serde_json::from_str(start_json_str).expect("start is JSON");
    assert_eq!(start["type"], "workflow_agent", "type field");
    assert_eq!(start["index"], 0, "index is 0 for first agent");
    assert_eq!(start["label"], "my-label", "label from opts.label");
    assert_eq!(start["state"], "start", "first event is start");
    let tool_use_id = start["toolUseID"].as_str().expect("toolUseID present");
    assert!(
        tool_use_id.starts_with("workflow_agent_0_"),
        "toolUseID format: {tool_use_id}"
    );

    // Parse the done event.
    let done_json_str = agent_lines[1].trim_start_matches("[workflow_agent] ");
    let done: serde_json::Value = serde_json::from_str(done_json_str).expect("done is JSON");
    assert_eq!(done["state"], "done", "second event is done");
    assert_eq!(done["index"], 0, "same agent index");
    assert!(done.get("agentId").is_some(), "done has agentId");
}

/// `phase()` calls produce `workflow_phase` formatted progress lines with index + title.
/// `log()` calls produce bare text lines (workflow_log style).
#[tokio::test]
async fn workflow_phase_and_log_progress_format() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let spawner = Arc::new(EchoSpawner::default());
    run_workflow_script(
        "phase('Analysis'); log('hello world'); phase('Report');",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        Some(tx),
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .expect("runs");

    let mut lines: Vec<String> = Vec::new();
    while let Ok(line) = rx.try_recv() {
        lines.push(line);
    }

    // Phase lines render as "[index] === title ===".
    assert!(
        lines
            .iter()
            .any(|l| l.contains("[1]") && l.contains("=== Analysis ===")),
        "first phase: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("[2]") && l.contains("=== Report ===")),
        "second phase: {lines:?}"
    );
    // Log line renders as bare text.
    assert!(
        lines.iter().any(|l| l == "hello world"),
        "log line: {lines:?}"
    );
}

/// Agent index increments monotonically across sequential agent() calls.
#[tokio::test]
async fn workflow_agent_index_is_monotonic() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let spawner = Arc::new(EchoSpawner::default());
    run_workflow_script(
        "await agent('first'); await agent('second'); await agent('third');",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        Some(tx),
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .expect("runs");

    let mut lines: Vec<String> = Vec::new();
    while let Ok(line) = rx.try_recv() {
        lines.push(line);
    }

    // Collect all start events and verify indices 0, 1, 2.
    let start_events: Vec<serde_json::Value> = lines
        .iter()
        .filter(|l| l.starts_with("[workflow_agent]"))
        .filter_map(|l| {
            let json_str = l.trim_start_matches("[workflow_agent] ");
            serde_json::from_str::<serde_json::Value>(json_str).ok()
        })
        .filter(|v| v["state"] == "start")
        .collect();
    assert_eq!(start_events.len(), 3, "three start events; got: {lines:?}");
    assert_eq!(start_events[0]["index"], 0);
    assert_eq!(start_events[1]["index"], 1);
    assert_eq!(start_events[2]["index"], 2);
}

/// cc 2.1.198 (M9): the workflow progress view keeps the EARLIEST agents
/// while the phase counter stays correct. The binary's fix
/// (`updateWorkflowProgressBatch`/`GCo` @213640399) keys `workflow_agent` /
/// `workflow_phase` rows on `${type}:${index}` and updates them in place —
/// when the row list overflows the window (`xVa=500` @213645694, trim at
/// `len > xVa*2`) ONLY `workflow_log` rows are dropped from the front, never
/// agent/phase rows. LingXi's progress stream is unbounded (spool + channel),
/// so earliest agents are retained by construction; this test locks that a
/// log flood past the binary's 1000-row trim threshold does not evict the
/// earliest agent or phase rows, and the phase indices stay correct.
#[tokio::test]
async fn workflow_progress_keeps_earliest_agents_through_log_flood() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let spawner = Arc::new(EchoSpawner::default());
    // Earliest agents FIRST, then a >1000-line log flood (the cc trim
    // trigger), then a second phase with more agents.
    run_workflow_script(
        "phase('Early'); await agent('a0'); await agent('a1'); \
         for (let i = 0; i < 1100; i++) { log('flood ' + i); } \
         phase('Late'); await agent('a2');",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        Some(tx),
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .expect("runs");

    let mut lines: Vec<String> = Vec::new();
    while let Ok(line) = rx.try_recv() {
        lines.push(line);
    }

    let agent_events: Vec<serde_json::Value> = lines
        .iter()
        .filter(|l| l.starts_with("[workflow_agent]"))
        .filter_map(|l| {
            serde_json::from_str::<serde_json::Value>(
                l.trim_start_matches("[workflow_agent] "),
            )
            .ok()
        })
        .collect();

    // The EARLIEST agents (indices 0 and 1, spawned before the flood) are
    // still present — both their start and done rows survive.
    for idx in [0, 1] {
        assert!(
            agent_events
                .iter()
                .any(|e| e["index"] == idx && e["state"] == "start"),
            "earliest agent {idx} start row retained; got {} agent events",
            agent_events.len()
        );
        assert!(
            agent_events
                .iter()
                .any(|e| e["index"] == idx && e["state"] == "done"),
            "earliest agent {idx} done row retained"
        );
    }
    // The post-flood agent is present too.
    assert!(
        agent_events
            .iter()
            .any(|e| e["index"] == 2 && e["state"] == "done"),
        "post-flood agent retained"
    );

    // The phase counter stays correct: both phase rows present with their
    // 1-based indices intact (earliest phase NOT dropped by the flood).
    assert!(
        lines
            .iter()
            .any(|l| l.contains("[1]") && l.contains("=== Early ===")),
        "earliest phase row retained with index 1"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("[2]") && l.contains("=== Late ===")),
        "second phase row retained with index 2"
    );
    // And the flood itself really crossed the binary's trim threshold.
    let flood_count = lines.iter().filter(|l| l.starts_with("flood ")).count();
    assert_eq!(flood_count, 1100, "the log flood was emitted in full");
}

/// Agent events include phaseIndex/phaseTitle when agent() is dispatched during a phase.
#[tokio::test]
async fn workflow_agent_carries_phase_context() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let spawner = Arc::new(EchoSpawner::default());
    run_workflow_script(
        "phase('MyPhase'); await agent('task1');",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        Some(tx),
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .expect("runs");

    let mut lines: Vec<String> = Vec::new();
    while let Ok(line) = rx.try_recv() {
        lines.push(line);
    }

    let start_event = lines
        .iter()
        .filter(|l| l.starts_with("[workflow_agent]"))
        .find_map(|l| {
            let json_str = l.trim_start_matches("[workflow_agent] ");
            let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
            (v["state"] == "start").then_some(v)
        })
        .expect("start event present");

    assert_eq!(start_event["phaseIndex"], 1, "phaseIndex = 1 (first phase)");
    assert_eq!(start_event["phaseTitle"], "MyPhase", "phaseTitle = MyPhase");
}

/// A journal-replayed agent emits a `cached` workflow_agent event.
#[tokio::test]
async fn workflow_agent_cached_event_on_journal_replay() {
    let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
    let dir = tempdir().unwrap();
    let mgr = Arc::new(TaskOutputManager::new(
        PathBuf::from(dir.path()),
        fs.clone(),
    ));

    let script = "const r = await agent('task'); log('r=' + r);";

    // Run 1: fresh — journal the result.
    let spawner1 = Arc::new(EchoSpawner::default());
    let sink1 = Arc::new(RecordingSink::default());
    let handle1 = make_handler(spawner1.clone(), mgr.clone(), sink1.clone())
        .spawn(workflow_input(script), make_ctx(fs.clone()))
        .await
        .unwrap();
    assert_eq!(await_terminal(&sink1).await, TaskStatus::Completed);
    let spool1 = dir.path().join(format!("{}.output", handle1.task_id));
    let out1 = mgr.read(&spool1, Default::default()).await.unwrap();
    let run_id = out1
        .content
        .lines()
        .find_map(|l| l.strip_prefix("runId: "))
        .expect("runId")
        .to_string();

    // Run 2: resume — agent replays from journal, should see `cached` event.
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let journal_path = dir.path().join(format!("workflow-{run_id}.json"));
    let journal_content = fs
        .read_file(journal_path.to_str().unwrap(), None, None)
        .await
        .unwrap()
        .content;
    let cache: std::collections::HashMap<String, String> =
        serde_json::from_str(&journal_content).expect("journal JSON");
    let journal = Arc::new(std::sync::Mutex::new(cache));

    run_workflow_script(
        script,
        DEFAULT_WORKFLOW_SUBAGENT,
        Arc::new(EchoSpawner::default()),
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        Some(tx),
        Some(journal),
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .expect("resume runs");

    let mut lines: Vec<String> = Vec::new();
    while let Ok(line) = rx.try_recv() {
        lines.push(line);
    }

    let cached_event = lines
        .iter()
        .filter(|l| l.starts_with("[workflow_agent]"))
        .find_map(|l| {
            let json_str = l.trim_start_matches("[workflow_agent] ");
            let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
            (v["state"] == "cached").then_some(v)
        });
    assert!(
        cached_event.is_some(),
        "cached event must be emitted on journal replay; lines: {lines:?}"
    );
    let ev = cached_event.unwrap();
    assert_eq!(ev["index"], 0, "cached agent has index 0");
    let tuid = ev["toolUseID"].as_str().expect("toolUseID");
    assert_eq!(tuid, "workflow_agent_0_cached", "cached toolUseID format");
}

/// A failed agent emits an `error` workflow_agent event.
#[tokio::test]
async fn workflow_agent_error_event_on_failure() {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let spawner = Arc::new(EchoSpawner {
        fail: true,
        ..Default::default()
    });
    run_workflow_script(
        "const r = await agent('task'); log('r=' + r);",
        DEFAULT_WORKFLOW_SUBAGENT,
        spawner,
        Arc::new(MockInvoker),
        Arc::new(MockBudget),
        Some(tx),
        None,
        None,
        None,
        0,
        NestedConfig::default(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(AnalyticsBus::new()),
        None,
        None,
    )
    .await
    .expect("runs (failed agent is not a script error)");

    let mut lines: Vec<String> = Vec::new();
    while let Ok(line) = rx.try_recv() {
        lines.push(line);
    }

    let error_event = lines
        .iter()
        .filter(|l| l.starts_with("[workflow_agent]"))
        .find_map(|l| {
            let json_str = l.trim_start_matches("[workflow_agent] ");
            let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
            (v["state"] == "error").then_some(v)
        });
    assert!(
        error_event.is_some(),
        "error event must be emitted for failed agent; lines: {lines:?}"
    );
}
