//! LingXi workflow runtime.
//!
//! Executes claude-code-style workflow scripts — JavaScript that orchestrates
//! subagents via injected globals (`agent()` / `parallel()` / `pipeline()` /
//! `phase()` / `log()` / `budget` / `workflow()`). claude-code runs these with
//! a JS `AsyncFunction`; LingXi embeds QuickJS (via `rquickjs`) so the same
//! model-authored scripts run with matching semantics.
//!
//! ## Stages
//! This is the runtime foundation: the embedded engine plus the
//! globals-injection + `meta`-extraction model the orchestration primitives
//! build on. Stage status:
//! * **done** — embedded engine, `meta` extraction, synchronous script body
//!   execution with `phase()` / `log()` captured.
//! * **next** — `agent()` bridged to the subagent spawner (needs the async
//!   engine; gated on the `futures`-feature MSRV pin), then `parallel()` /
//!   `pipeline()`, journaling/resume, budget, and tool/handler wiring.

use std::cell::RefCell;
use std::rc::Rc;

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
    R: FnMut(&str) -> String + 'static,
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

        let r = runner.clone();
        globals
            .set(
                "agent",
                Function::new(ctx.clone(), move |prompt: String| -> String {
                    (r.borrow_mut())(&prompt)
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

        // Eval the IIFE (returns a pending promise we drive below).
        ctx.eval::<rquickjs::Value, _>(wrapped.as_bytes())
            .map_err(|e| WorkflowError::Script(e.to_string()))?;
        Ok(())
    })?;

    // Drive the microtask/job queue until the IIFE settles.
    while rt
        .execute_pending_job()
        .map_err(|e| WorkflowError::Script(format!("{e:?}")))?
    {}

    if let Some(e) = error.borrow().clone() {
        return Err(WorkflowError::Script(e));
    }
    let progress = progress.borrow().clone();
    Ok(RunOutcome { progress })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let out = run(script, move |prompt: &str| {
            calls += 1;
            format!("[r{calls}:{prompt}]")
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
        let out = run(script, |_| "3".to_string()).unwrap();
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
        let err = run("await agent('x'); throw new Error('boom')", |_| {
            "ok".to_string()
        })
        .unwrap_err();
        match err {
            WorkflowError::Script(s) => assert!(s.contains("boom"), "got: {s}"),
            other => panic!("expected Script error, got {other:?}"),
        }
    }
}
