//! `LocalWorkflowHandler` — runs a model-authored workflow script and bridges
//! its `agent()` calls to real subagent spawns.
//!
//! claude-code executes workflow scripts as a JS `AsyncFunction` whose injected
//! `agent()` global spawns a child agent and resolves with its result. LingXi
//! runs the same scripts on the embedded QuickJS runtime (the `workflow` crate);
//! this module is the bridge between that runtime's batch `agent_runner` seam
//! and the [`SubagentSpawner`] the Task subsystem uses to spawn children.
//!
//! ## The sync ↔ async bridge
//!
//! `workflow::run` is synchronous (it drives the QuickJS job queue on the
//! calling thread) and its `agent_runner` callback is invoked from sync JS, but
//! [`SubagentSpawner::spawn`] is `async` and engine code must not `block_on` the
//! runtime directly (it spawns through the `RuntimeSpawner` seam). We therefore
//! run `workflow::run` on a dedicated `std::thread` and connect it to the async
//! world with channels:
//!
//! * the script thread's batch runner sends the batch's prompts over an mpsc
//!   channel and blocks (`blocking_recv`) on a per-batch oneshot reply;
//! * an async worker loop (driven on the caller's runtime) receives each batch,
//!   spawns its subagents concurrently — bounded by [`concurrency_cap`], in the
//!   prompts' order — and sends the ordered results back over the reply channel.
//!
//! There is at most one batch in flight (the runner blocks until its reply), so
//! the channels cannot deadlock: the script only makes progress once the worker
//! has answered, and `workflow::run` only returns once every batch has been
//! answered — at which point the runner (holding the request sender) is dropped,
//! the worker loop sees the closed channel and ends, and the outcome that the
//! script thread sends last is delivered to the caller.

use std::sync::Arc;

use futures::stream::StreamExt;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use traits::{
    BudgetEnforcerHandle, SubagentInheritance, SubagentResult, SubagentSpawnError,
    SubagentSpawnRequest, SubagentSpawner, ToolInvoker,
};

/// Handler for the local-workflow task type.
///
/// The task lifecycle wiring (status sink, output spooling, kill) is added in a
/// later increment; [`run_workflow_script`] is the reusable core that drives a
/// script to completion against a [`SubagentSpawner`].
pub struct LocalWorkflowHandler;

/// The subagent type spawned for a bare `agent(prompt)` call — claude-code's
/// default workflow subagent.
pub const DEFAULT_WORKFLOW_SUBAGENT: &str = "general-purpose";

/// claude-code's concurrency cap for in-flight `agent()` calls:
/// `min(16, cpu_cores - 2)`, at least 1.
fn concurrency_cap() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    cores.saturating_sub(2).clamp(1, 16)
}

/// Build the `SubagentSpawnRequest` for one `agent(prompt)` call. Mirrors the
/// Task tool's local-agent template: a plain prompt-only spawn with every
/// optional field defaulted. (The `agent()` opts — schema/label/phase/model/
/// effort/agentType/isolation — are threaded through in a later increment.)
fn make_request(subagent_type: &str, prompt: &str) -> SubagentSpawnRequest {
    SubagentSpawnRequest {
        subagent_type: subagent_type.to_string(),
        prompt: prompt.to_string(),
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
    }
}

/// Render a subagent's JSON `content` payload as the string the workflow's
/// `agent()` resolves with. A bare JSON string resolves to its inner text
/// (claude-code: `agent()` without a schema returns the subagent's final text);
/// any other shape (an object/array from a structured `schema` run) is
/// serialised verbatim so the script can `JSON.parse` it.
fn value_to_text(content: Value) -> String {
    match content {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

/// Map a terminal subagent result to the string the runner returns for that
/// prompt. A failed/killed/errored agent maps to the empty string, which is
/// falsy in JS and so is dropped by the `.filter(Boolean)` the scripts use —
/// matching claude-code's `null` return for a skipped/dead agent.
fn result_to_string(result: Result<SubagentResult, SubagentSpawnError>) -> String {
    match result {
        Ok(SubagentResult::Completed { content, .. }) => value_to_text(content),
        Ok(SubagentResult::Failed { .. } | SubagentResult::Killed { .. }) | Err(_) => {
            String::new()
        }
    }
}

/// Run a workflow `script` to completion, spawning each `agent()` call as a real
/// subagent of type `subagent_type` via `spawner`. Returns the script's
/// [`workflow::RunOutcome`] (its `phase()`/`log()` progress) or a
/// [`workflow::WorkflowError`].
///
/// `tool_invoker` and `budget` are the parent's inheritance `Arc`s; the same
/// `Arc`s (cloned handle, identical inner) are handed to every child so the
/// recursion lock and budget aggregate across the whole agent tree.
pub async fn run_workflow_script(
    script: &str,
    subagent_type: &str,
    spawner: Arc<dyn SubagentSpawner>,
    tool_invoker: Arc<dyn ToolInvoker>,
    budget: Arc<dyn BudgetEnforcerHandle>,
) -> Result<workflow::RunOutcome, workflow::WorkflowError> {
    // Request channel: each in-flight batch is (prompts, reply-sender). A buffer
    // of one suffices — the runner blocks on its reply before sending the next.
    let (req_tx, mut req_rx) = mpsc::channel::<(Vec<String>, oneshot::Sender<Vec<String>>)>(1);
    let (outcome_tx, outcome_rx) =
        oneshot::channel::<Result<workflow::RunOutcome, workflow::WorkflowError>>();
    let script_owned = script.to_string();

    // The script runs synchronously on its own OS thread; its batch runner is
    // plain sync code, so `blocking_send`/`blocking_recv` are safe here (this is
    // not a runtime worker thread).
    std::thread::Builder::new()
        .name("workflow-script".into())
        .spawn(move || {
            let runner = move |prompts: &[String]| -> Vec<String> {
                let (reply_tx, reply_rx) = oneshot::channel();
                if req_tx.blocking_send((prompts.to_vec(), reply_tx)).is_err() {
                    return Vec::new();
                }
                reply_rx.blocking_recv().unwrap_or_default()
            };
            let outcome = workflow::run(&script_owned, runner);
            let _ = outcome_tx.send(outcome);
        })
        .expect("spawn workflow-script thread");

    let cap = concurrency_cap();
    // Async worker: answer each batch by spawning its subagents concurrently
    // (bounded, order-preserving). The loop ends when the runner's sender is
    // dropped — i.e. when `workflow::run` returns.
    while let Some((prompts, reply)) = req_rx.recv().await {
        let results: Vec<String> = futures::stream::iter(prompts.into_iter().map(|prompt| {
            let spawner = spawner.clone();
            let inherit = SubagentInheritance {
                tool_invoker: tool_invoker.clone(),
                budget: budget.clone(),
            };
            let subagent_type = subagent_type.to_string();
            async move {
                let request = make_request(&subagent_type, &prompt);
                result_to_string(spawner.spawn(request, inherit).await)
            }
        }))
        .buffered(cap)
        .collect()
        .await;
        // Receiver gone only if the script thread vanished; nothing to do.
        let _ = reply.send(results);
    }

    outcome_rx.await.map_err(|_| {
        workflow::WorkflowError::Engine(
            "workflow script thread terminated without an outcome".into(),
        )
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::any::Any;
    use std::sync::Mutex as StdMutex;
    use traits::tool_invoker::{SubagentInvocationContext, ToolInvokerError};
    use traits::{BudgetError, SubagentUsage};

    /// Spawner that echoes each prompt back as `echo:<prompt>` (or fails every
    /// spawn when `fail` is set), recording the prompts it saw.
    #[derive(Default)]
    struct EchoSpawner {
        seen: StdMutex<Vec<String>>,
        fail: bool,
    }

    #[async_trait]
    impl SubagentSpawner for EchoSpawner {
        async fn spawn(
            &self,
            request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            self.seen.lock().unwrap().push(request.prompt.clone());
            if self.fail {
                return Ok(SubagentResult::Failed {
                    agent_id: protocol::AgentId::new(),
                    reason: "boom".into(),
                });
            }
            Ok(SubagentResult::Completed {
                agent_id: protocol::AgentId::new(),
                content: Value::String(format!("echo:{}", request.prompt)),
                usage: SubagentUsage::default(),
                total_tool_use_count: 0,
                total_duration_ms: 0,
                total_tokens: 0,
                assistant_message_count: 0,
                response_char_count: 0,
                last_request_id: None,
            })
        }
    }

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

    fn logs(outcome: &workflow::RunOutcome) -> Vec<String> {
        outcome
            .progress
            .iter()
            .filter_map(|p| match p {
                workflow::Progress::Log(s) => Some(s.clone()),
                workflow::Progress::Phase(_) => None,
            })
            .collect()
    }

    async fn run(script: &str, spawner: Arc<EchoSpawner>) -> workflow::RunOutcome {
        run_workflow_script(
            script,
            DEFAULT_WORKFLOW_SUBAGENT,
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
        )
        .await
        .expect("workflow runs to completion")
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
        let outcome = run(script, spawner.clone()).await;
        // Result order follows prompt order even though the spawns run concurrently.
        assert_eq!(logs(&outcome), vec!["R:echo:a,echo:b,echo:c".to_string()]);
        // The spawner saw all three prompts (concurrent → order not asserted).
        let mut seen = spawner.seen.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[tokio::test]
    async fn sequential_awaits_preserve_order() {
        let spawner = Arc::new(EchoSpawner::default());
        let script = r#"
            const a = await agent('1');
            const b = await agent('2');
            log(a + '|' + b);
        "#;
        let outcome = run(script, spawner.clone()).await;
        assert_eq!(logs(&outcome), vec!["echo:1|echo:2".to_string()]);
        // Sequential awaits are separate batches, so the spawner order is fixed.
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
        let outcome = run(script, spawner).await;
        // Empty string is falsy → the `||` fallback fires, matching `null`.
        assert_eq!(logs(&outcome), vec!["got:NONE".to_string()]);
    }

    #[tokio::test]
    async fn script_with_no_agents_still_completes() {
        let spawner = Arc::new(EchoSpawner::default());
        let outcome = run("log('done');", spawner.clone()).await;
        assert_eq!(logs(&outcome), vec!["done".to_string()]);
        assert!(spawner.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pipeline_stages_run_each_item_through_the_spawner() {
        let spawner = Arc::new(EchoSpawner::default());
        // Two items, two stages: stage 1 spawns agent(item), stage 2 spawns
        // agent(prev + '!'). Final results are the stage-2 outputs.
        let script = r#"
            const rs = await pipeline(
              ['x', 'y'],
              (item) => agent(item),
              (prev) => agent(prev + '!'),
            );
            log('P:' + rs.join(','));
        "#;
        let outcome = run(script, spawner.clone()).await;
        assert_eq!(
            logs(&outcome),
            vec!["P:echo:echo:x!,echo:echo:y!".to_string()]
        );
    }
}
