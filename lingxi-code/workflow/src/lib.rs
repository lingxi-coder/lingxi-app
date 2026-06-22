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
}
