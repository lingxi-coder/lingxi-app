//! `lingxi-cli __bg-run <short>` — the hidden background-agent **worker**
//! (increment 3 of the coherent-minimum daemon).
//!
//! This is the detached process the daemon supervisor spawns (never
//! user-facing) to actually EXECUTE a `--bg` job. It closes the gap the
//! foundation left open: `--bg` persisted a `state:"working"` phantom job but
//! nothing ran it. The worker:
//!
//! 1. Reads its `jobs/<short>/state.json` (the durable dispatch artifact the
//!    `--bg` CLI wrote) to recover the `initialPrompt`, `cwd`, and `sessionId`.
//!    A missing or already-terminal job is a no-op (exit 0) — the supervisor's
//!    at-least-once spawn is idempotent this way.
//! 2. Registers a LIVE `kind:"bg"` session ([`SessionRegistration::register_bg`])
//!    keyed by `jobId=<short>` so `agents --json` can match the live worker to
//!    its job row, and marks it `busy`. The registration is dropped (unlinked)
//!    on exit (RAII).
//! 3. Builds a headless runtime for the job cwd (synthesizing a print-shaped,
//!    non-`--background` [`Argv`] so it reuses the same
//!    [`build_runtime`](crate::init::build_runtime) → `run_turn` path as
//!    `--print`, and does NOT recursively re-dispatch) and runs the turn.
//! 4. Rewrites `state.json` to the reader-recognized terminal state — `"done"`
//!    on success, `"failed"` on error — via
//!    [`agents_registry::update_job_state`], which preserves the pinned key
//!    order and clears `workerPid`.
//!
//! The ACTUAL `run_turn` is injected behind the [`run_worker_core`] `execute`
//! seam (exactly like `daemon.rs` injects `sleep`/`WorkerSpawner`), so a unit
//! test drives the `working → done`/`failed` transition with a stubbed executor
//! and NO live LLM.
//!
//! The supervisor spawns this worker with the `LINGXI_*` background-session
//! environment ([`crate::commands::daemon`]'s `bg_worker_env`), so the turn
//! receives the `# Background Session` prompt section and `/stop` resolves the
//! job; a vanished worker is respawned by the supervisor with a bounded budget.
//!
//! The worker also owns an authenticated live attach socket when the daemon
//! provides `LINGXI_BG_ATTACH_*` env. `agents attach` connects to that socket
//! while the worker is still running, avoiding a second `--resume` JSONL writer.
//! Attached terminals can feed follow-up input and Ctrl-C control back into the
//! same live turn loop.

use crate::agents_registry::{self, SessionRegistration};
use crate::argv::Argv;
use crate::exit_codes;
use crate::output::{OutputSink, PlainSink};
use crate::output_adapter::SinkAdapter;
use orchestrator::TurnOutcome;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use traits::{SlashCommandDispatcher, SlashDispatchResult};

/// `__bg-run` subcommand args: the 8-hex job short id to execute.
#[derive(Debug, Clone, clap::Args)]
pub struct Cli {
    /// The `jobs/<short>/` id whose persisted job this worker executes.
    pub short: String,
}

/// The job fields a worker needs to execute a turn, lifted out of
/// `state.json`. Passed to the injected executor.
#[derive(Debug, Clone)]
pub struct JobSpec {
    /// The job short id (`jobs/<short>/`).
    pub short: String,
    /// The task to run — the persisted `initialPrompt`.
    pub prompt: String,
    /// The directory the turn should run in (the persisted `cwd`).
    pub cwd: String,
    /// The worker's session UUID (persisted `sessionId`).
    pub session_id: String,
    /// Display label for the live session record (name → intent fallback).
    pub name: Option<String>,
}

/// Production entrypoint: resolve the shared config home and drive
/// [`run_worker_core`] with the REAL `run_turn`-backed executor.
pub async fn run(cli: &Cli) -> i32 {
    let config_home = crate::run::daemon_runtime_dir();
    run_worker_core(&config_home, &cli.short, execute_job).await
}

/// Testable worker core: the `execute` seam stands in for the real
/// `build_runtime` → `run_turn` execution so a unit test can assert the
/// `working → done`/`failed` state transition deterministically.
///
/// Returns the process exit code (always [`exit_codes::SUCCESS`]: a failed
/// TURN is reported by writing `state:"failed"` to the job, not by a non-zero
/// worker exit — the worker's job is to record the outcome, and a non-zero
/// exit would only confuse the supervisor's liveness reaping).
pub async fn run_worker_core<F, Fut>(config_home: &Path, short: &str, execute: F) -> i32
where
    F: FnOnce(JobSpec) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    // 1. Recover the durable job. Missing/terminal ⇒ nothing to do.
    let Some(job) = agents_registry::read_job(config_home, short) else {
        return exit_codes::SUCCESS;
    };
    if agents_registry::job_is_terminal(&job) {
        return exit_codes::SUCCESS;
    }
    let prompt = job.initial_prompt.clone().unwrap_or_default();
    if prompt.trim().is_empty() {
        // A prompt-less job can never run a turn; mark it done so it doesn't
        // linger as a perpetual "working" phantom.
        let _ = agents_registry::update_job_state(config_home, short, "done", None);
        return exit_codes::SUCCESS;
    }

    let spec = JobSpec {
        short: short.to_string(),
        prompt,
        cwd: job.cwd.clone().unwrap_or_default(),
        session_id: job.session_id.clone().unwrap_or_default(),
        name: job.name.clone().or_else(|| job.intent.clone()),
    };

    // 2. Register a LIVE bg session (unlinked on drop / explicit deregister).
    let session_id = spec.session_id.clone();
    let reg = SessionRegistration::register_bg(
        config_home,
        (!session_id.is_empty()).then_some(session_id.as_str()),
        spec.name.as_deref(),
        short,
    );
    // Mark the worker actively running. `"busy"` is the live-status token the
    // registry taxonomy uses (idle/busy/waiting) and the one `merged_state`
    // treats as "working" for the matched job row.
    reg.update_status("busy", None);

    // 3. Execute the task (injected). 4. Record the terminal outcome.
    let outcome = execute(spec).await;
    let new_state = if outcome.is_ok() { "done" } else { "failed" };
    if let Err(e) = agents_registry::update_job_state(config_home, short, new_state, None) {
        tracing::warn!("lingxi-cli __bg-run: could not persist terminal job state: {e}");
    }

    // Unlink the live session now (explicit, though Drop would also do it).
    reg.deregister();
    exit_codes::SUCCESS
}

/// Synthesize the print-shaped [`Argv`] the worker runs the turn with.
/// `--print`-shaped so the headless deny-on-ask permission default applies (no
/// interactive prompt is reachable from a detached worker); `background:false`
/// so it does NOT re-enter the `--bg` dispatch path; and the job's RECORDED
/// `session_id` is threaded as the runtime's `session_id_override` (init.rs:570)
/// so the transcript lands in `<sessionId>.jsonl` — the SAME id the job row and
/// `register_bg` advertise. Omitting it (the earlier bug) minted a fresh id, so
/// resuming the job's session found no transcript.
fn worker_argv(spec: &JobSpec) -> Argv {
    Argv {
        prompt: Some(spec.prompt.clone()),
        print: true,
        background: false,
        session_id: (!spec.session_id.is_empty()).then(|| spec.session_id.clone()),
        ..Argv::default()
    }
}

/// The production executor: chdir into the job cwd, synthesize a print-shaped
/// [`Argv`], build the runtime, run the initial turn, then keep driving the
/// same runtime from live attach input while an attach client remains connected.
async fn execute_job(spec: JobSpec) -> Result<(), String> {
    // Run the turn in the job's directory (tool + config resolution keys off
    // `std::env::current_dir()`, which `resolve_desktop_config` reads).
    if !spec.cwd.is_empty() {
        if let Err(e) = std::env::set_current_dir(&spec.cwd) {
            return Err(format!("could not enter job cwd {}: {e}", spec.cwd));
        }
    }

    let argv = worker_argv(&spec);

    let attach_hub = match crate::bg_attach::AttachHub::start_from_env() {
        Ok(hub) => hub,
        Err(e) => {
            tracing::warn!("lingxi-cli __bg-run: could not start live attach socket: {e}");
            None
        }
    };
    let mut attach_rx = attach_hub
        .as_ref()
        .and_then(crate::bg_attach::AttachHub::take_input_rx);
    let sink: Arc<dyn OutputSink> = match attach_hub.as_ref() {
        Some(hub) => Arc::new(crate::bg_attach::AttachSink::new(hub.clone())),
        None => Arc::new(PlainSink::new()),
    };
    let adapter: Arc<dyn traits::OutputStream> = Arc::new(SinkAdapter::new(sink.clone()));
    let permission_mode = permission::PermissionMode::Default;

    let runtime = crate::init::build_runtime(&argv, adapter, permission_mode)
        .await
        .map_err(|e| e.to_string())?;

    let mut queued = VecDeque::new();
    run_attached_turn(&runtime, &spec.prompt, attach_rx.as_mut(), &mut queued).await?;
    if let (Some(hub), Some(rx)) = (attach_hub.as_ref(), attach_rx.as_mut()) {
        run_attach_input_loop(&runtime, sink.as_ref(), hub, rx, queued).await?;
    }
    Ok(())
}

async fn run_attached_turn(
    runtime: &crate::init::Runtime,
    prompt: &str,
    mut attach_rx: Option<&mut mpsc::UnboundedReceiver<crate::bg_attach::AttachInput>>,
    queued: &mut VecDeque<String>,
) -> Result<(), String> {
    if let Some(rx) = attach_rx.as_mut() {
        let cancel = CancellationToken::new();
        let turn = runtime
            .orchestrator
            .run_turn_streaming_with_cancel(prompt, cancel.clone());
        tokio::pin!(turn);
        let mut rx_closed = false;
        loop {
            tokio::select! {
                result = &mut turn => return handle_turn_result(result),
                input = rx.recv(), if !rx_closed => {
                    match input {
                        Some(crate::bg_attach::AttachInput::Line(line)) => queued.push_back(line),
                        Some(crate::bg_attach::AttachInput::Interrupt) => cancel.cancel(),
                        Some(crate::bg_attach::AttachInput::ClientDetached) => {}
                        None => rx_closed = true,
                    }
                }
            }
        }
    }

    runtime
        .orchestrator
        .run_turn(prompt)
        .await
        .map(|_outcome| ())
        .map_err(|e| e.to_string())
}

fn handle_turn_result(
    result: Result<TurnOutcome, orchestrator::OrchestratorError>,
) -> Result<(), String> {
    match result {
        Ok(TurnOutcome::EndTurn | TurnOutcome::Cancelled) => Ok(()),
        Ok(TurnOutcome::MaxTurns) => Err("reached MAX_TURNS_PER_CONVERSATION".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

async fn run_attach_input_loop(
    runtime: &crate::init::Runtime,
    sink: &dyn OutputSink,
    hub: &crate::bg_attach::AttachHub,
    rx: &mut mpsc::UnboundedReceiver<crate::bg_attach::AttachInput>,
    mut queued: VecDeque<String>,
) -> Result<(), String> {
    while let Some(input) = next_attach_line(hub, rx, &mut queued).await {
        if input.trim().is_empty() {
            continue;
        }
        if input.starts_with('/') {
            match runtime.dispatcher.dispatch(&input).await {
                SlashDispatchResult::Handled { display }
                | SlashDispatchResult::Unknown { display, .. } => {
                    sink.command_output(&input, &display).await;
                }
                SlashDispatchResult::RunAsTurn { prompt } => {
                    sink.turn_start().await;
                    run_attached_turn(runtime, &prompt, Some(rx), &mut queued).await?;
                }
                SlashDispatchResult::NotASlashCommand => {}
            }
            if runtime.orchestrator.current_should_exit() {
                break;
            }
        } else {
            sink.turn_start().await;
            run_attached_turn(runtime, &input, Some(rx), &mut queued).await?;
        }
    }
    Ok(())
}

async fn next_attach_line(
    hub: &crate::bg_attach::AttachHub,
    rx: &mut mpsc::UnboundedReceiver<crate::bg_attach::AttachInput>,
    queued: &mut VecDeque<String>,
) -> Option<String> {
    loop {
        if let Some(line) = queued.pop_front() {
            return Some(line);
        }
        while let Ok(input) = rx.try_recv() {
            match input {
                crate::bg_attach::AttachInput::Line(line) => return Some(line),
                crate::bg_attach::AttachInput::Interrupt => continue,
                crate::bg_attach::AttachInput::ClientDetached => continue,
            }
        }
        if !hub.has_clients() {
            return None;
        }
        match rx.recv().await {
            Some(crate::bg_attach::AttachInput::Line(line)) => return Some(line),
            Some(crate::bg_attach::AttachInput::Interrupt) => continue,
            Some(crate::bg_attach::AttachInput::ClientDetached) => {
                if !hub.has_clients() {
                    return None;
                }
            }
            None => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents_registry::{read_job, write_job_state, JobStateWrite};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmpdir() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("lingxi-bgworker-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn seed_job(home: &Path, short: &str, prompt: &str) {
        let respawn: Vec<String> = Vec::new();
        let cwd = home.display().to_string();
        let job = JobStateWrite {
            state: "working",
            tempo: Some("active"),
            name: None,
            session_id: Some("11111111-1111-1111-1111-111111111111"),
            cwd: Some(&cwd),
            origin_cwd: Some(&cwd),
            created_at: Some("2026-07-04T00:00:00.000Z"),
            intent: Some("do the thing"),
            display_intent: None,
            template: Some("bg"),
            respawn_flags: &respawn,
            in_flight: None,
            backend: Some("daemon"),
            initial_prompt: Some(prompt),
            worker_pid: None,
        };
        write_job_state(home, short, &job).unwrap();
    }

    #[test]
    fn worker_argv_threads_the_jobs_session_id() {
        // Regression: the worker must run the turn under the job's RECORDED
        // session id (→ `session_id_override` → `<sessionId>.jsonl`), not a
        // freshly-minted one, so the job row / live registration / transcript
        // all agree and `--resume <sessionId>` finds the turn.
        let spec = JobSpec {
            short: "74d8a00f".to_string(),
            prompt: "say hi".to_string(),
            cwd: "/tmp/x".to_string(),
            session_id: "cb1f9d13-20a6-4e53-ad3e-5720d438a5f2".to_string(),
            name: None,
        };
        let argv = worker_argv(&spec);
        assert_eq!(
            argv.session_id.as_deref(),
            Some("cb1f9d13-20a6-4e53-ad3e-5720d438a5f2")
        );
        assert!(argv.print && !argv.background);
        // An empty recorded id falls back to a minted one (None override).
        let spec_empty = JobSpec {
            session_id: String::new(),
            ..spec
        };
        assert_eq!(worker_argv(&spec_empty).session_id, None);
    }

    #[tokio::test]
    async fn ok_execution_marks_job_done() {
        let home = tmpdir();
        seed_job(&home, "bc7c6b33", "port the daemon");
        let mut seen: Option<String> = None;
        let code = run_worker_core(&home, "bc7c6b33", |spec| {
            seen = Some(spec.prompt.clone());
            async move { Ok(()) }
        })
        .await;
        assert_eq!(code, exit_codes::SUCCESS);
        // The executor received the persisted prompt.
        assert_eq!(seen.as_deref(), Some("port the daemon"));
        // The job transitioned working → done (terminal).
        let job = read_job(&home, "bc7c6b33").unwrap();
        assert_eq!(job.state, "done");
        assert!(agents_registry::job_is_terminal(&job));
        assert_eq!(job.worker_pid, None);
    }

    #[tokio::test]
    async fn err_execution_marks_job_failed() {
        let home = tmpdir();
        seed_job(&home, "aaaa1111", "explode please");
        let code = run_worker_core(
            &home,
            "aaaa1111",
            |_spec| async move { Err("boom".to_string()) },
        )
        .await;
        assert_eq!(code, exit_codes::SUCCESS);
        let job = read_job(&home, "aaaa1111").unwrap();
        assert_eq!(job.state, "failed");
        assert!(agents_registry::job_is_terminal(&job));
    }

    #[tokio::test]
    async fn missing_job_is_a_noop() {
        let home = tmpdir();
        let mut called = false;
        let code = run_worker_core(&home, "nope0000", |_spec| {
            called = true;
            async move { Ok(()) }
        })
        .await;
        assert_eq!(code, exit_codes::SUCCESS);
        assert!(!called, "executor never runs for a missing job");
    }

    #[tokio::test]
    async fn already_terminal_job_is_a_noop() {
        let home = tmpdir();
        seed_job(&home, "dddd4444", "done already");
        agents_registry::update_job_state(&home, "dddd4444", "done", None).unwrap();
        let mut called = false;
        let code = run_worker_core(&home, "dddd4444", |_spec| {
            called = true;
            async move { Ok(()) }
        })
        .await;
        assert_eq!(code, exit_codes::SUCCESS);
        assert!(!called, "executor never runs for an already-terminal job");
    }

    #[tokio::test]
    async fn empty_prompt_job_is_marked_done_without_executing() {
        let home = tmpdir();
        seed_job(&home, "eeee5555", "   ");
        let mut called = false;
        let code = run_worker_core(&home, "eeee5555", |_spec| {
            called = true;
            async move { Ok(()) }
        })
        .await;
        assert_eq!(code, exit_codes::SUCCESS);
        assert!(!called, "no turn for an empty prompt");
        assert_eq!(read_job(&home, "eeee5555").unwrap().state, "done");
    }

    #[tokio::test]
    async fn live_bg_session_registered_during_execution_then_unlinked() {
        let home = tmpdir();
        seed_job(&home, "ffff6666", "observe me");
        // While the executor runs, a live `kind:"bg"` session for THIS process
        // exists under sessions/<pid>.json (own pid is alive → reader keeps it).
        let observed = std::cell::Cell::new(0usize);
        let code = run_worker_core(&home, "ffff6666", |_spec| {
            let sessions =
                agents_registry::read_live_sessions(&agents_registry::sessions_dir(&home));
            observed.set(sessions.iter().filter(|s| s.kind == "bg").count());
            async move { Ok(()) }
        })
        .await;
        assert_eq!(code, exit_codes::SUCCESS);
        assert_eq!(observed.get(), 1, "a live bg session is registered mid-run");
        // …and it is unlinked once the worker returns.
        let after = agents_registry::read_live_sessions(&agents_registry::sessions_dir(&home));
        assert!(
            after.iter().all(|s| s.kind != "bg"),
            "live bg session unlinked on exit"
        );
    }

    #[tokio::test]
    async fn attach_input_loop_drains_queued_line_without_clients() {
        let home = tmpdir();
        let sock = home.join("attach.sock");
        let hub = crate::bg_attach::AttachHub::start(sock, "token-1".to_string()).unwrap();
        let mut rx = hub.take_input_rx().unwrap();
        let mut queued = VecDeque::from(["follow up".to_string()]);

        assert_eq!(
            next_attach_line(&hub, &mut rx, &mut queued).await,
            Some("follow up".to_string())
        );
        assert_eq!(next_attach_line(&hub, &mut rx, &mut queued).await, None);
    }
}
