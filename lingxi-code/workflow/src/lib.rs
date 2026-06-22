//! LingXi workflow runtime.
//!
//! Executes claude-code-style workflow scripts — JavaScript that orchestrates
//! subagents via injected globals (`agent()` / `parallel()` / `pipeline()` /
//! `phase()` / `log()` / `budget` / `workflow()`). claude-code runs these with
//! a JS `AsyncFunction`; LingXi embeds QuickJS (via `rquickjs`) so the same
//! model-authored scripts run with matching semantics.
//!
//! The full global surface is implemented (`agent`, `parallel`, `pipeline`,
//! `phase`, `log`, `budget`, `args`, `workflow`) with CONCURRENT batch dispatch
//! of `agent()` calls (agents pending together run as one batch) via the
//! pluggable `agent_runner`. The host (`tasks::handlers::local_workflow`) bridges
//! `agent_runner` to LingXi's subagent spawner and adds live progress, a real
//! token budget ([`WorkflowBudgetSource`]), structured-output `agent({schema})`,
//! journaling/resume, and `workflow()` nesting; the `Workflow` tool
//! (`tool-workflow`) is registered + wired at the desktop composition root.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

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
globalThis.agent = (prompt, opts) => new Promise((res) => { globalThis.__wf_queue.push({ prompt: String(prompt), opts: opts || {}, res }); });
globalThis.__wf_pump = () => {
  const q = globalThis.__wf_queue;
  if (q.length === 0) return false;
  globalThis.__wf_queue = [];
  // Dispatch the batch as two parallel arrays: the prompts and the JSON-encoded
  // opts ({agentType, model, isolation, schema, label, phase, effort}). The host
  // runner maps the spawn-affecting opts onto each subagent request.
  const results = globalThis.__wf_dispatch_batch(q.map((x) => x.prompt), q.map((x) => JSON.stringify(x.opts || {})));
  for (let i = 0; i < q.length; i++) q[i].res(results[i]);
  return true;
};
// parallel(): start EVERY thunk first (so their agents queue together → one
// concurrent batch), then collect; a throwing thunk / rejected promise → null.
globalThis.parallel = async (thunks) => {
  if (!Array.isArray(thunks)) throw new Error("parallel() expects an array of thunks");
  if (thunks.length > 4096) throw new Error("array length " + thunks.length + " exceeds the maximum of 4096 supported across the workflow VM boundary");
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
  if (items.length > 4096) throw new Error("array length " + items.length + " exceeds the maximum of 4096 supported across the workflow VM boundary");
  for (const s of stages) if (typeof s !== "function") throw new Error("pipeline() stages must be functions: pipeline(items, item => ..., result => ...)");
  const chain = async (item, idx) => {
    let v = item;
    for (const s of stages) v = await s(v, item, idx);
    return v;
  };
  return await parallel(items.map((it, i) => () => chain(it, i)));
};
// Default globals. The host OVERRIDES each (before this prelude runs) when it
// has a real value: `budget` (a WorkflowBudgetSource), `args` (the tool input),
// and `workflow()` (a real nested-run impl when nesting is allowed). These
// defaults apply otherwise: no-target budget; `undefined` args; and a
// `workflow()` that throws — the state inside a nested run, where claude-code's
// one-level nesting limit is reached.
if (!('budget' in globalThis)) globalThis.budget = { total: null, spent: () => 0, remaining: () => Infinity };
if (!('args' in globalThis)) globalThis.args = undefined;
if (!('workflow' in globalThis)) globalThis.workflow = async () => { throw new Error("workflow(): nested workflows are not supported (workflow() inside a child)"); };
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
    /// The script's return value, JSON-serialised. claude-code returns the
    /// script's resolved value (e.g. `return { confirmed }`) as the Workflow
    /// tool result; this is `JSON.stringify(value)`. `None` when the script
    /// returns `undefined` (no `return`).
    pub result: Option<String>,
}

/// Supplies the live token budget the script's `budget` global reflects: the
/// turn's target (`budget.total`) and the shared output-token spend
/// (`budget.spent()`), read on demand while the script runs. The host (the
/// composition root) backs this with the orchestrator's token target plus the
/// session's cumulative usage — the same shared pool the main loop and every
/// subagent add to. `None` leaves the no-target default (total `null`, spent
/// `0`, remaining `Infinity`).
pub trait WorkflowBudgetSource: Send + Sync {
    /// The turn's token target, or `None` if no `+Nk` target was set.
    fn total(&self) -> Option<u64>;
    /// Output tokens spent this turn across the shared pool (main loop + all
    /// workflows/subagents).
    fn spent(&self) -> u64;
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
    // A classic (non-async) script cannot use top-level `return`, so there is
    // never a captured result on this path.
    Ok(RunOutcome {
        progress,
        result: None,
    })
}

/// Execute a workflow script's **async** body, capturing `phase()`/`log()` and
/// routing `agent(prompt)` through `agent_runner`.
///
/// The body is wrapped in an `async` IIFE so top-level `await` works (claude-code
/// runs the script as an `AsyncFunction`); QuickJS schedules the continuations as
/// microtasks, which we drive to completion via the runtime job queue. `agent()`
/// resolves synchronously through `agent_runner` (the real runtime blocks on a
/// subagent and returns its result), so sequential `await agent(...)` chains run
/// in order.
///
/// # Errors
/// Returns [`WorkflowError`] if the engine fails to start, the script throws, or
/// a job raises.
pub fn run<R>(script: &str, agent_runner: R) -> Result<RunOutcome, WorkflowError>
where
    R: FnMut(&[String], &[String]) -> Vec<String> + 'static,
{
    run_with_progress(script, agent_runner, |_: &Progress| {}, None, false, None)
}

/// Like [`run`], but also fires `on_progress` for each `phase()`/`log()` event
/// **as it happens** (live), in addition to collecting them into the returned
/// [`RunOutcome`]. The host forwards these so a running workflow's progress is
/// visible (e.g. spooled to the task output) before the script completes.
///
/// # Errors
/// Returns [`WorkflowError`] if the engine fails to start, the script throws, or
/// a job raises.
pub fn run_with_progress<R, P>(
    script: &str,
    agent_runner: R,
    on_progress: P,
    budget: Option<Arc<dyn WorkflowBudgetSource>>,
    // When `true`, `workflow()` runs a nested workflow inline (the host runner
    // resolves+runs it via an `__wf_nested` agent() call); `false` (nested runs)
    // leaves the throwing default — claude-code's one-level nesting limit.
    allow_nested: bool,
    // The `args` global value as a JSON string (the Workflow tool's `args` input
    // / a `workflow()` call's args). `None` ⇒ `undefined`. JSON is a subset of JS
    // expressions, so it is a valid initializer.
    args: Option<String>,
) -> Result<RunOutcome, WorkflowError>
where
    // `(prompts, opts_json) -> results`: the two parallel arrays the pump
    // dispatches — each `opts_json[i]` is `JSON.stringify(agent()'s opts)` for
    // `prompts[i]`. The host runner maps the spawn-affecting opts (agentType /
    // model / isolation) onto each subagent request.
    R: FnMut(&[String], &[String]) -> Vec<String> + 'static,
    // Fires for every `phase()`/`log()` as it is emitted (live).
    P: FnMut(&Progress) + 'static,
{
    use rquickjs::{Context, Function, Runtime};

    let rt = Runtime::new().map_err(|e| WorkflowError::Engine(e.to_string()))?;
    let ctx = Context::full(&rt).map_err(|e| WorkflowError::Engine(e.to_string()))?;

    let progress: Rc<RefCell<Vec<Progress>>> = Rc::new(RefCell::new(Vec::new()));
    let error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let result_slot: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let runner = Rc::new(RefCell::new(agent_runner));
    let on_progress = Rc::new(RefCell::new(on_progress));
    let prepared = strip_meta_export(script);
    // Wrap in an async IIFE; route a throw into `__wf_error` so it survives the
    // microtask boundary (a rejected top-level promise would otherwise be lost).
    // claude-code (V8) captures `(e && e.stack) || e`, but QuickJS's `e.stack`
    // omits the message line, so capture `String(e)` (the message) plus the
    // stack — the V8-vs-QuickJS stack frames differ inherently regardless.
    // The IIFE's resolved value is the script's `return` value; capture it via
    // `.then` into `__wf_result` (JSON-serialised) so the Workflow tool can
    // return it. `undefined` (no `return`) leaves the result unset. A throw is
    // already routed to `__wf_error` inside the `catch`, so the success handler
    // sees `undefined` and the rejection handler is a no-op.
    let wrapped = format!(
        "(async () => {{ try {{\n{prepared}\n}} catch (e) {{ globalThis.__wf_error(String(e) + (e && e.stack ? \"\\n\" + e.stack : \"\")); }} }})().then((v) => {{ try {{ if (v !== undefined) globalThis.__wf_result(JSON.stringify(v)); }} catch (e) {{ globalThis.__wf_error(String(e)); }} }}, () => {{}});"
    );

    ctx.with(|ctx| -> Result<(), WorkflowError> {
        let globals = ctx.globals();
        let eng = |e: rquickjs::Error| WorkflowError::Engine(e.to_string());

        let p_log = progress.clone();
        let op_log = on_progress.clone();
        globals
            .set(
                "log",
                Function::new(ctx.clone(), move |msg: String| {
                    let prog = Progress::Log(msg);
                    (op_log.borrow_mut())(&prog);
                    p_log.borrow_mut().push(prog);
                })
                .map_err(eng)?,
            )
            .map_err(eng)?;

        let p_phase = progress.clone();
        let op_phase = on_progress.clone();
        globals
            .set(
                "phase",
                Function::new(ctx.clone(), move |title: String| {
                    let prog = Progress::Phase(title);
                    (op_phase.borrow_mut())(&prog);
                    p_phase.borrow_mut().push(prog);
                })
                .map_err(eng)?,
            )
            .map_err(eng)?;

        // Native batch dispatcher: the JS `__wf_pump` hands it every concurrently
        // pending agent prompt + its JSON-encoded opts at once; the runner
        // resolves them (the real runtime spawns the subagents in parallel).
        let r = runner.clone();
        globals
            .set(
                "__wf_dispatch_batch",
                Function::new(
                    ctx.clone(),
                    move |prompts: Vec<String>, opts_json: Vec<String>| -> Vec<String> {
                        (r.borrow_mut())(&prompts, &opts_json)
                    },
                )
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

        // Native sink for the script's JSON-serialised return value.
        let res = result_slot.clone();
        globals
            .set(
                "__wf_result",
                Function::new(ctx.clone(), move |v: String| {
                    *res.borrow_mut() = Some(v);
                })
                .map_err(eng)?,
            )
            .map_err(eng)?;

        // Real token budget (when the host supplies a source): set
        // `globalThis.budget` BEFORE the prelude so its `if (!('budget' in
        // globalThis))` default is skipped. `budget.total` snapshots the turn's
        // target; `spent()`/`remaining()` read the shared pool live.
        if let Some(src) = budget.as_ref() {
            let s_total = src.clone();
            globals
                .set(
                    "__wf_budget_total",
                    Function::new(ctx.clone(), move || -> Option<f64> {
                        s_total.total().map(|t| t as f64)
                    })
                    .map_err(eng)?,
                )
                .map_err(eng)?;
            let s_spent = src.clone();
            globals
                .set(
                    "__wf_budget_spent",
                    Function::new(ctx.clone(), move || -> f64 { s_spent.spent() as f64 })
                        .map_err(eng)?,
                )
                .map_err(eng)?;
            ctx.eval::<(), _>(
                b"globalThis.budget = { total: __wf_budget_total(), spent: () => __wf_budget_spent(), remaining: () => { const t = globalThis.budget.total; return (t === null || t === undefined) ? Infinity : Math.max(0, t - __wf_budget_spent()); } };" as &[u8],
            )
            .map_err(|e| WorkflowError::Engine(e.to_string()))?;
        }

        // `args` global (the Workflow tool's `args` input / a `workflow()` call's
        // args). Set BEFORE the prelude so its `if (!('args' in globalThis))`
        // default is skipped. The value is already a JSON string (a valid JS
        // initializer).
        if let Some(args_json) = &args {
            ctx.eval::<(), _>(format!("globalThis.args = {args_json};").as_bytes())
                .map_err(|e| WorkflowError::Engine(e.to_string()))?;
        }

        // Top-level `workflow()`: run a nested workflow INLINE, in this same
        // runtime (claude-code shares the parent's runtime state — concurrency
        // cap, agent counter, abort, budget). It (1) resolves the script via the
        // host (an `agent('', { __wf_resolve })` call the runner reads as a file
        // request, returning the source) and (2) evaluates that source as an
        // async function in THIS context, so its `agent()`/`parallel()`/`budget`
        // all share the parent's globals/queue. A depth counter throws on a
        // nested `workflow()` (claude-code's one-level limit); `args` is swapped
        // to the call's value for the nested body. (`agent` is resolved lazily at
        // call time, so defining this before the prelude is fine.)
        if allow_nested {
            ctx.eval::<(), _>(
                b"globalThis.__wf_depth = 0; globalThis.workflow = async (nameOrRef, a) => { if (globalThis.__wf_depth >= 1) throw new Error(\"workflow(): nested workflows are not supported (workflow() inside a child)\"); const spec = (typeof nameOrRef === 'string') ? { name: nameOrRef } : nameOrRef; const src = await agent('', { __wf_resolve: JSON.stringify(spec) }); if (src === '') throw new Error(\"workflow(): could not resolve the nested workflow\"); globalThis.__wf_depth += 1; const savedArgs = globalThis.args; globalThis.args = a; try { return await (new Function('return (async () => {\\n' + src + '\\n})();'))(); } finally { globalThis.__wf_depth -= 1; globalThis.args = savedArgs; } };" as &[u8],
            )
            .map_err(|e| WorkflowError::Engine(e.to_string()))?;
        }

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
    let result = result_slot.borrow().clone();
    Ok(RunOutcome { progress, result })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Batch runner for scripts that never call `agent()` (the batch is always
    /// empty, so this is never actually invoked).
    fn no_agents(prompts: &[String], _opts: &[String]) -> Vec<String> {
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
        let out = run(script, move |prompts: &[String], _opts: &[String]| -> Vec<String> {
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
        let out = run(script, |prompts: &[String], _opts: &[String]| {
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
            |prompts: &[String], _opts: &[String]| prompts.iter().map(|_| "ok".to_string()).collect(),
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
        let out = run(script, move |prompts: &[String], _opts: &[String]| -> Vec<String> {
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
        let out = run(script, |prompts: &[String], _opts: &[String]| {
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
        let out = run(script, move |prompts: &[String], _opts: &[String]| -> Vec<String> {
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
        let out = run(script, move |prompts: &[String], _opts: &[String]| -> Vec<String> {
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
        let out = run(script, move |prompts: &[String], _opts: &[String]| -> Vec<String> {
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

    #[test]
    fn captures_the_scripts_return_value_as_json() {
        // The script's `return` value is the Workflow tool's result.
        let out = run("return { ok: 1, items: [2, 3] };", no_agents).unwrap();
        assert_eq!(out.result.as_deref(), Some(r#"{"ok":1,"items":[2,3]}"#));
    }

    #[test]
    fn no_return_leaves_the_result_unset() {
        let out = run("log('side effect only');", no_agents).unwrap();
        assert_eq!(out.result, None);
        assert_eq!(out.progress, vec![Progress::Log("side effect only".into())]);
    }

    #[test]
    fn captures_a_return_value_built_from_agent_results() {
        let script = r#"
            const rs = await parallel([() => agent('a'), () => agent('b')]);
            return { confirmed: rs };
        "#;
        let out = run(script, |prompts: &[String], _opts: &[String]| {
            prompts.iter().map(|p| format!("{p}!")).collect()
        })
        .unwrap();
        assert_eq!(out.result.as_deref(), Some(r#"{"confirmed":["a!","b!"]}"#));
    }

    #[test]
    fn agent_opts_reach_the_runner_as_json() {
        // `agent(prompt, opts)` — the opts object is JSON-encoded and handed to
        // the runner alongside each prompt (parallel arrays). A bare `agent(p)`
        // yields `{}`.
        let captured: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let cap = captured.clone();
        let script = r#"
            await agent('p1', { agentType: 'reviewer', model: 'opus', isolation: 'worktree' });
            await agent('p2');
        "#;
        run(script, move |prompts: &[String], opts: &[String]| {
            cap.borrow_mut().extend(opts.iter().cloned());
            prompts.iter().map(|_| "ok".to_string()).collect()
        })
        .unwrap();
        let got = captured.borrow().clone();
        assert_eq!(got.len(), 2);
        assert!(got[0].contains(r#""agentType":"reviewer""#), "got: {}", got[0]);
        assert!(got[0].contains(r#""model":"opus""#));
        assert!(got[0].contains(r#""isolation":"worktree""#));
        assert_eq!(got[1], "{}", "a bare agent(p) carries empty opts");
    }

    #[test]
    fn run_with_progress_fires_live_callbacks() {
        let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        let script = r#"
            phase('A');
            log('one');
            phase('B');
            log('two');
        "#;
        let out = run_with_progress(
            script,
            no_agents,
            move |p: &Progress| {
                s.borrow_mut().push(match p {
                    Progress::Phase(t) => format!("phase:{t}"),
                    Progress::Log(m) => format!("log:{m}"),
                });
            },
            None,
            false,
            None,
        )
        .unwrap();
        // Callbacks fired live, in emission order.
        assert_eq!(
            *seen.borrow(),
            vec!["phase:A", "log:one", "phase:B", "log:two"]
        );
        // The same events are still collected in the outcome.
        assert_eq!(out.progress.len(), 4);
    }

    #[test]
    fn real_budget_source_drives_the_budget_globals() {
        struct Src;
        impl WorkflowBudgetSource for Src {
            fn total(&self) -> Option<u64> {
                Some(500_000)
            }
            fn spent(&self) -> u64 {
                120_000
            }
        }
        let script = r#"
            log('total=' + String(budget.total));
            log('spent=' + String(budget.spent()));
            log('remaining=' + String(budget.remaining()));
        "#;
        let out = run_with_progress(
            script,
            no_agents,
            |_: &Progress| {},
            Some(Arc::new(Src) as Arc<dyn WorkflowBudgetSource>),
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            out.progress,
            vec![
                Progress::Log("total=500000".into()),
                Progress::Log("spent=120000".into()),
                Progress::Log("remaining=380000".into()),
            ]
        );
    }

    #[test]
    fn args_global_reflects_the_passed_value() {
        let out = run_with_progress(
            "log('x=' + args.x); log('len=' + args.items.length);",
            no_agents,
            |_: &Progress| {},
            None,
            false,
            Some(r#"{ "x": 7, "items": [1, 2, 3] }"#.to_string()),
        )
        .unwrap();
        assert_eq!(
            out.progress,
            vec![Progress::Log("x=7".into()), Progress::Log("len=3".into())]
        );
    }

    #[test]
    fn workflow_runs_nested_inline_when_allowed() {
        // allow_nested=true → workflow() resolves the nested SOURCE via an
        // `__wf_resolve` agent() call, then evaluates it IN THIS runtime. The
        // nested body's `return` flows back; its `args` is the call's value.
        let out = run_with_progress(
            r#"
                const r = await workflow('child', { n: 1 });
                log('ok=' + r.ok + ' n=' + r.n);
                return r;
            "#,
            |_prompts: &[String], opts_json: &[String]| {
                opts_json
                    .iter()
                    .map(|o| {
                        if o.contains("__wf_resolve") {
                            // The nested workflow's SOURCE (sees the swapped args).
                            "return { ok: true, n: args.n };".to_string()
                        } else {
                            String::new()
                        }
                    })
                    .collect()
            },
            |_: &Progress| {},
            None,
            true,
            None,
        )
        .unwrap();
        assert_eq!(out.progress, vec![Progress::Log("ok=true n=1".into())]);
        assert_eq!(out.result.as_deref(), Some(r#"{"ok":true,"n":1}"#));
    }
}
