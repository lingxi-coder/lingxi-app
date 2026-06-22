//! LingXi workflow runtime.
//!
//! Executes claude-code-style workflow scripts — JavaScript that orchestrates
//! subagents via injected globals (`agent()` / `parallel()` / `pipeline()` /
//! `phase()` / `log()` / `budget` / `workflow()`). claude-code runs these with
//! a JS `AsyncFunction`; LingXi embeds QuickJS (via `rquickjs`) so the same
//! model-authored scripts run with matching semantics.
//!
//! ## Stages
//! * **done** — embedded engine; async event-loop; the full global surface
//!   (`agent`, `parallel`, `pipeline`, `phase`, `log`, `budget`, `args`,
//!   `workflow`); CONCURRENT batch dispatch of `agent()` calls (agents pending
//!   together run as one batch). The pluggable `agent_runner` resolves each
//!   batch.
//! * **next** — bridge `agent_runner` to LingXi's subagent spawner (the
//!   `LocalWorkflowHandler` / `local_agent` seam), then journaling/resume, real
//!   budget tracking, and `Workflow` tool registration.

use std::cell::RefCell;
use std::rc::Rc;

/// JS prelude defining `agent()` (a deferred promise) + the `parallel()` /
/// `pipeline()` orchestration primitives, injected before the workflow body.
/// The argument-validation `throw` messages are byte-locked to claude-code's.
/// `parallel`/`pipeline` start every chain before awaiting, so agents pending at
/// the same time are dispatched as ONE concurrent batch by the Rust driver
/// (`run`'s pump loop). A throwing thunk/stage resolves to `null` (claude-code's
/// `.filter(Boolean)` contract).
const WORKFLOW_PRELUDE: &str = r#"
// agent() returns a DEFERRED promise and queues the request; it never blocks.
// The Rust driver pumps the queue between microtask drains, dispatching every
// promise that is pending at the same time as ONE concurrent batch (via the
// native __wf_dispatch_batch), then resolving them. So agents that are started
// before the first await (parallel's fan-out) run concurrently, while a
// sequential `await agent()` chain dispatches one at a time.
globalThis.__wf_queue = [];
globalThis.agent = (prompt) => new Promise((res) => { globalThis.__wf_queue.push({ prompt: String(prompt), res }); });
globalThis.__wf_pump = () => {
  const q = globalThis.__wf_queue;
  if (q.length === 0) return false;
  globalThis.__wf_queue = [];
  const results = globalThis.__wf_dispatch_batch(q.map((x) => x.prompt));
  for (let i = 0; i < q.length; i++) q[i].res(results[i]);
  return true;
};
// parallel(): start EVERY thunk first (so their agents queue together → one
// concurrent batch), then collect; a throwing thunk / rejected promise → null.
globalThis.parallel = async (thunks) => {
  if (!Array.isArray(thunks)) throw new Error("parallel() expects an array of thunks");
  const ps = thunks.map((t) => { try { return Promise.resolve(t()); } catch (e) { return Promise.resolve(null); } });
  const out = [];
  for (const p of ps) { try { out.push(await p); } catch (e) { out.push(null); } }
  return out;
};
// pipeline(): each item runs its stage chain independently with NO barrier
// between stages — expressed as parallel() over per-item chains, so item A can
// be in a later stage while item B is still early, and each stage's agents
// batch. A throwing stage drops that item to null.
globalThis.pipeline = async (items, ...stages) => {
  if (!Array.isArray(items)) throw new Error("pipeline() expects an array as the first argument");
  for (const s of stages) if (typeof s !== "function") throw new Error("pipeline() stages must be functions: pipeline(items, item => ..., result => ...)");
  const chain = async (item, idx) => {
    let v = item;
    for (const s of stages) v = await s(v, item, idx);
    return v;
  };
  return await parallel(items.map((it, i) => () => chain(it, i)));
};
// Remaining globals. `budget` defaults to no-target (total null, spent 0,
// remaining Infinity) — a real token-tracking budget is supplied once agent()
// is bridged to subagents. `args` is the caller-provided input (undefined by
// default). `workflow()` (nested run) is not supported yet and throws clearly.
if (!('budget' in globalThis)) globalThis.budget = { total: null, spent: () => 0, remaining: () => Infinity };
if (!('args' in globalThis)) globalThis.args = undefined;
if (!('workflow' in globalThis)) globalThis.workflow = async () => { throw new Error("workflow(): nested workflows are not supported in this runtime yet"); };
"#;

/// A progress event emitted by a running workflow script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// `phase(title)` — starts a new progress group.
    Phase(String),
    /// `log(message)` — a narrator line.
    Log(String),
}

/// Errors from parsing or executing a workflow script.
#[derive(Debug)]
pub enum WorkflowError {
    /// The embedded engine failed to start.
    Engine(String),
    /// The script threw or failed to evaluate.
    Script(String),
}

impl std::fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine(e) => write!(f, "workflow engine error: {e}"),
            Self::Script(e) => write!(f, "workflow script error: {e}"),
        }
    }
}

impl std::error::Error for WorkflowError {}

/// The outcome of running a workflow script body.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// Progress events (`phase()` / `log()`) in emission order.
    pub progress: Vec<Progress>,
}

/// Rewrite a leading `export const meta = …` / `export const meta=…` into a
/// plain `const meta = …` so the script body can be evaluated as a classic
/// script. claude-code reads `meta` from the module's export; here the meta
/// object is bound as a normal `const` (the executor reads it back via the
/// global scope). Only the `export ` keyword on the `meta` declaration is
/// stripped — every other statement is preserved verbatim.
#[must_use]
pub fn strip_meta_export(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    for (i, line) in script.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("export const meta") {
            let indent = &line[..line.len() - trimmed.len()];
            out.push_str(indent);
            out.push_str("const meta");
            out.push_str(rest);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Execute a workflow script's **synchronous** body, capturing `phase()` and
/// `log()` calls. The orchestration globals that require the async engine
/// (`agent`/`parallel`/`pipeline`) are not yet injected here — this validates
/// the engine + globals-injection + `meta` model the full executor extends.
///
/// # Errors
/// Returns [`WorkflowError`] if the engine fails to start or the script throws.
pub fn run_sync(script: &str) -> Result<RunOutcome, WorkflowError> {
    use rquickjs::{Context, Function, Runtime};

    let rt = Runtime::new().map_err(|e| WorkflowError::Engine(e.to_string()))?;
    let ctx = Context::full(&rt).map_err(|e| WorkflowError::Engine(e.to_string()))?;

    let progress: Rc<RefCell<Vec<Progress>>> = Rc::new(RefCell::new(Vec::new()));
    let prepared = strip_meta_export(script);

    ctx.with(|ctx| -> Result<(), WorkflowError> {
        let globals = ctx.globals();

        let p_log = progress.clone();
        let log = Function::new(ctx.clone(), move |msg: String| {
            p_log.borrow_mut().push(Progress::Log(msg));
        })
        .map_err(|e| WorkflowError::Engine(e.to_string()))?;
        globals
            .set("log", log)
            .map_err(|e| WorkflowError::Engine(e.to_string()))?;

        let p_phase = progress.clone();
        let phase = Function::new(ctx.clone(), move |title: String| {
            p_phase.borrow_mut().push(Progress::Phase(title));
        })
        .map_err(|e| WorkflowError::Engine(e.to_string()))?;
        globals
            .set("phase", phase)
            .map_err(|e| WorkflowError::Engine(e.to_string()))?;

        ctx.eval::<(), _>(prepared.as_bytes())
            .map_err(|e| WorkflowError::Script(e.to_string()))?;
        Ok(())
    })?;

    // The injected functions still hold `Rc` clones (the context owns them), so
    // clone the captured events out rather than unwrapping the `Rc`.
    let progress = progress.borrow().clone();
    Ok(RunOutcome { progress })
}

/// Execute a workflow script's **async** body, capturing `phase()`/`log()` and
/// routing `agent(prompt)` through `agent_runner`.
///
/// The body is wrapped in an `async` IIFE so top-level `await` works (claude-code
/// runs the script as an `AsyncFunction`); QuickJS schedules the continuations as
/// microtasks, which we drive to completion via the runtime job queue. `agent()`
/// resolves synchronously through `agent_runner` (the real runtime blocks on a
/// subagent and returns its result), so sequential `await agent(...)` chains run
/// in order. (`parallel()`/`pipeline()` concurrency is a later stage.)
///
/// # Errors
/// Returns [`WorkflowError`] if the engine fails to start, the script throws, or
/// a job raises.
pub fn run<R>(script: &str, agent_runner: R) -> Result<RunOutcome, WorkflowError>
where
    R: FnMut(&[String]) -> Vec<String> + 'static,
{
    use rquickjs::{Context, Function, Runtime};

    let rt = Runtime::new().map_err(|e| WorkflowError::Engine(e.to_string()))?;
    let ctx = Context::full(&rt).map_err(|e| WorkflowError::Engine(e.to_string()))?;

    let progress: Rc<RefCell<Vec<Progress>>> = Rc::new(RefCell::new(Vec::new()));
    let error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let runner = Rc::new(RefCell::new(agent_runner));
    let prepared = strip_meta_export(script);
    // Wrap in an async IIFE; route a throw into `__wf_error` so it survives the
    // microtask boundary (a rejected top-level promise would otherwise be lost).
    // claude-code (V8) captures `(e && e.stack) || e`, but QuickJS's `e.stack`
    // omits the message line, so capture `String(e)` (the message) plus the
    // stack — the V8-vs-QuickJS stack frames differ inherently regardless.
    let wrapped = format!(
        "(async () => {{ try {{\n{prepared}\n}} catch (e) {{ globalThis.__wf_error(String(e) + (e && e.stack ? \"\\n\" + e.stack : \"\")); }} }})();"
    );

    ctx.with(|ctx| -> Result<(), WorkflowError> {
        let globals = ctx.globals();
        let eng = |e: rquickjs::Error| WorkflowError::Engine(e.to_string());

        let p_log = progress.clone();
        globals
            .set(
                "log",
                Function::new(ctx.clone(), move |msg: String| {
                    p_log.borrow_mut().push(Progress::Log(msg));
                })
                .map_err(eng)?,
            )
            .map_err(eng)?;

        let p_phase = progress.clone();
        globals
            .set(
                "phase",
                Function::new(ctx.clone(), move |title: String| {
                    p_phase.borrow_mut().push(Progress::Phase(title));
                })
                .map_err(eng)?,
            )
            .map_err(eng)?;

        // Native batch dispatcher: the JS `__wf_pump` hands it every concurrently
        // pending agent prompt at once; the runner resolves them (the real
        // runtime spawns the subagents in parallel).
        let r = runner.clone();
        globals
            .set(
                "__wf_dispatch_batch",
                Function::new(ctx.clone(), move |prompts: Vec<String>| -> Vec<String> {
                    (r.borrow_mut())(&prompts)
                })
                .map_err(eng)?,
            )
            .map_err(eng)?;

        let err = error.clone();
        globals
            .set(
                "__wf_error",
                Function::new(ctx.clone(), move |msg: String| {
                    *err.borrow_mut() = Some(msg);
                })
                .map_err(eng)?,
            )
            .map_err(eng)?;

        // Orchestration prelude: agent() (deferred-promise) + parallel() /
        // pipeline() + budget/args/workflow defined in JS, with the byte-locked
        // argument validation. The native __wf_dispatch_batch (above) receives
        // each concurrent batch the pump produces.
        ctx.eval::<(), _>(WORKFLOW_PRELUDE.as_bytes())
            .map_err(|e| WorkflowError::Engine(e.to_string()))?;

        // Eval the IIFE (returns a pending promise we drive below).
        ctx.eval::<rquickjs::Value, _>(wrapped.as_bytes())
            .map_err(|e| WorkflowError::Script(e.to_string()))?;
        Ok(())
    })?;

    // Drive the runtime: drain microtasks, then pump the agent queue (dispatch
    // every concurrently-pending agent as ONE batch + resolve), and repeat until
    // the workflow settles (no jobs and no queued agents).
    loop {
        while rt
            .execute_pending_job()
            .map_err(|e| WorkflowError::Script(format!("{e:?}")))?
        {}
        let pumped = ctx.with(|ctx| -> Result<bool, WorkflowError> {
            let pump: Function = ctx
                .globals()
                .get("__wf_pump")
                .map_err(|e| WorkflowError::Engine(e.to_string()))?;
            pump.call::<_, bool>(())
                .map_err(|e| WorkflowError::Script(e.to_string()))
        })?;
        if !pumped {
            break;
        }
    }

    if let Some(e) = error.borrow().clone() {
        return Err(WorkflowError::Script(e));
    }
    let progress = progress.borrow().clone();
    Ok(RunOutcome { progress })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Batch runner for scripts that never call `agent()` (the batch is always
    /// empty, so this is never actually invoked).
    fn no_agents(prompts: &[String]) -> Vec<String> {
        prompts.iter().map(|_| String::new()).collect()
    }

    #[test]
    fn engine_evaluates_js() {
        // (kept as a fast smoke test of the embedded engine)
        let out = run_sync("log(String(1 + 2 * 3))").unwrap();
        assert_eq!(out.progress, vec![Progress::Log("7".into())]);
    }

    #[test]
    fn strip_meta_export_only_touches_the_meta_decl() {
        let src = "export const meta = { name: 'x' }\nlog('hi')";
        assert_eq!(
            strip_meta_export(src),
            "const meta = { name: 'x' }\nlog('hi')"
        );
        // Indentation preserved; unrelated `export` lines untouched.
        assert_eq!(strip_meta_export("  export const meta={}"), "  const meta={}");
        assert_eq!(strip_meta_export("export default 1"), "export default 1");
    }

    #[test]
    fn runs_a_workflow_style_script_capturing_phase_and_log() {
        let script = r#"
export const meta = {
  name: 'demo',
  description: 'foundation smoke test',
  phases: [{ title: 'Scan' }, { title: 'Report' }],
}
phase('Scan')
const items = ['a', 'b', 'c']
log(`${items.length} items to scan`)
phase('Report')
log(`done: ${items.join(',')}`)
"#;
        let out = run_sync(script).unwrap();
        assert_eq!(
            out.progress,
            vec![
                Progress::Phase("Scan".into()),
                Progress::Log("3 items to scan".into()),
                Progress::Phase("Report".into()),
                Progress::Log("done: a,b,c".into()),
            ]
        );
    }

    #[test]
    fn script_error_is_surfaced() {
        let err = run_sync("log(undefinedThing.x)").unwrap_err();
        assert!(matches!(err, WorkflowError::Script(_)), "got {err:?}");
    }

    #[test]
    fn runs_async_script_with_sequential_agents() {
        // `await agent(...)` chains run in order; the runner sees the prompts
        // sequentially and its results flow back into the script.
        let script = r#"
export const meta = { name: 'a', description: 'async smoke' }
phase('Work')
const a = await agent('first')
log('got: ' + a)
const b = await agent('second')
log('got: ' + b)
"#;
        let mut calls = 0;
        let out = run(script, move |prompts: &[String]| -> Vec<String> {
            prompts
                .iter()
                .map(|prompt| {
                    calls += 1;
                    format!("[r{calls}:{prompt}]")
                })
                .collect()
        })
        .unwrap();
        assert_eq!(
            out.progress,
            vec![
                Progress::Phase("Work".into()),
                Progress::Log("got: [r1:first]".into()),
                Progress::Log("got: [r2:second]".into()),
            ]
        );
    }

    #[test]
    fn async_script_uses_agent_result_in_logic() {
        // The result of an awaited agent drives subsequent control flow.
        let script = r#"
const n = Number(await agent('count'))
for (let i = 0; i < n; i++) log('item ' + i)
"#;
        let out = run(script, |prompts: &[String]| {
            prompts.iter().map(|_| "3".to_string()).collect()
        })
        .unwrap();
        assert_eq!(
            out.progress,
            vec![
                Progress::Log("item 0".into()),
                Progress::Log("item 1".into()),
                Progress::Log("item 2".into()),
            ]
        );
    }

    #[test]
    fn async_script_throw_surfaces_after_await() {
        let err = run(
            "await agent('x'); throw new Error('boom')",
            |prompts: &[String]| prompts.iter().map(|_| "ok".to_string()).collect(),
        )
        .unwrap_err();
        match err {
            WorkflowError::Script(s) => assert!(s.contains("boom"), "got: {s}"),
            other => panic!("expected Script error, got {other:?}"),
        }
    }

    #[test]
    fn parallel_collects_results_in_order() {
        let script = r#"
const rs = await parallel([
  () => agent('a'),
  () => agent('b'),
  () => agent('c'),
])
log(rs.join('|'))
"#;
        let mut n = 0;
        let out = run(script, move |prompts: &[String]| -> Vec<String> {
            prompts
                .iter()
                .map(|p| {
                    n += 1;
                    format!("{p}{n}")
                })
                .collect()
        })
        .unwrap();
        assert_eq!(out.progress, vec![Progress::Log("a1|b2|c3".into())]);
    }

    #[test]
    fn parallel_throwing_thunk_becomes_null() {
        let script = r#"
const rs = await parallel([
  () => agent('ok'),
  () => { throw new Error('boom') },
])
log(String(rs[0]) + ',' + String(rs[1]))
"#;
        let out = run(script, |prompts: &[String]| {
            prompts.iter().map(|_| "OK".to_string()).collect()
        })
        .unwrap();
        assert_eq!(out.progress, vec![Progress::Log("OK,null".into())]);
    }

    #[test]
    fn parallel_dispatches_concurrent_agents_as_one_batch() {
        // parallel() starts every thunk before awaiting → the agents queue
        // together and the driver dispatches them as a SINGLE concurrent batch.
        let script = r#"
const rs = await parallel([() => agent('a'), () => agent('b'), () => agent('c')])
log(rs.join('|'))
"#;
        let batches: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
        let b = batches.clone();
        let out = run(script, move |prompts: &[String]| -> Vec<String> {
            b.borrow_mut().push(prompts.len());
            prompts.iter().map(|p| format!("R:{p}")).collect()
        })
        .unwrap();
        assert_eq!(out.progress, vec![Progress::Log("R:a|R:b|R:c".into())]);
        assert_eq!(
            *batches.borrow(),
            vec![3],
            "all three agents dispatched in ONE concurrent batch"
        );
    }

    #[test]
    fn sequential_awaits_are_separate_batches() {
        // A sequential `await agent()` chain dispatches one prompt at a time.
        let script = r#"
const a = await agent('a')
const b = await agent('b')
log(a + b)
"#;
        let batches: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
        let b = batches.clone();
        let out = run(script, move |prompts: &[String]| -> Vec<String> {
            b.borrow_mut().push(prompts.len());
            prompts.iter().map(|p| p.to_uppercase()).collect()
        })
        .unwrap();
        assert_eq!(out.progress, vec![Progress::Log("AB".into())]);
        assert_eq!(
            *batches.borrow(),
            vec![1, 1],
            "two sequential awaits → two batches of one"
        );
    }

    #[test]
    fn pipeline_stages_batch_across_items() {
        // pipeline runs items independently, so each stage's agents across all
        // items dispatch together: stage 1 over [x,y] is one batch, stage 2 is
        // the next.
        let script = r#"
const rs = await pipeline(
  ['x', 'y'],
  (it) => agent('s1:' + it),
  (prev) => agent('s2:' + prev),
)
log(rs.join('|'))
"#;
        let batches: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
        let b = batches.clone();
        let out = run(script, move |prompts: &[String]| -> Vec<String> {
            b.borrow_mut().push(prompts.len());
            prompts.iter().map(|p| format!("[{p}]")).collect()
        })
        .unwrap();
        assert_eq!(out.progress, vec![Progress::Log("[s2:[s1:x]]|[s2:[s1:y]]".into())]);
        assert_eq!(
            *batches.borrow(),
            vec![2, 2],
            "stage 1 over both items is one batch, stage 2 the next"
        );
    }

    #[test]
    fn pipeline_runs_each_item_through_all_stages() {
        let script = r#"
const rs = await pipeline(
  [1, 2, 3],
  (x) => x + 1,
  (x) => x * 10,
)
log(rs.join(','))
"#;
        let out = run(script, no_agents).unwrap();
        assert_eq!(out.progress, vec![Progress::Log("20,30,40".into())]);
    }

    #[test]
    fn pipeline_rejects_non_array_first_arg() {
        let err = run("await pipeline('nope', x => x)", no_agents).unwrap_err();
        match err {
            WorkflowError::Script(s) => assert!(
                s.contains("pipeline() expects an array as the first argument"),
                "got: {s}"
            ),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn pipeline_rejects_non_function_stage() {
        let err = run("await pipeline([1], 'notafn')", no_agents).unwrap_err();
        match err {
            WorkflowError::Script(s) => assert!(
                s.contains("pipeline() stages must be functions"),
                "got: {s}"
            ),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn budget_args_workflow_globals_present() {
        let script = r#"
log('total=' + String(budget.total))
log('remaining=' + String(budget.remaining()))
log('spent=' + String(budget.spent()))
log('args=' + String(args))
log('wf=' + (typeof workflow))
"#;
        let out = run(script, no_agents).unwrap();
        assert_eq!(
            out.progress,
            vec![
                Progress::Log("total=null".into()),
                Progress::Log("remaining=Infinity".into()),
                Progress::Log("spent=0".into()),
                Progress::Log("args=undefined".into()),
                Progress::Log("wf=function".into()),
            ]
        );
    }

    #[test]
    fn nested_workflow_call_throws_clearly() {
        let err = run("await workflow('child')", no_agents).unwrap_err();
        match err {
            WorkflowError::Script(s) => {
                assert!(s.contains("nested workflows are not supported"), "got: {s}");
            }
            other => panic!("got {other:?}"),
        }
    }
}
