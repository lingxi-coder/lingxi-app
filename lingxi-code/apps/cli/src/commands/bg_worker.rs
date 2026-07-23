//! `lingxi-cli __bg-run <short>` — the hidden background-agent **worker**
//! and PTY supervisor.
//!
//! This is the detached process the daemon supervisor spawns (never
//! user-facing) to host a `--bg` job:
//!
//! 1. Reads `state.json` plus the owner-only, versioned `launch.json`.
//!    A missing or already-terminal job is a no-op (exit 0) — the supervisor's
//!    at-least-once spawn is idempotent this way.
//! 2. Starts the authenticated protocol-v2 attach endpoint and spawns the
//!    hidden `__bg-pty-session` child in a real PTY/ConPTY.
//! 3. Supervises byte input, resize, detach/re-attach, process-tree shutdown,
//!    output drain, and the PID-reuse-safe `pty.json` record. The child owns the
//!    live [`SessionRegistration`] and mounts the ordinary interactive TUI;
//!    becoming idle does not end the worker.
//! 4. Rewrites `state.json` to the terminal state only when that TUI exits — `"done"`
//!    on success, `"failed"` on error — via
//!    [`agents_registry::update_job_state`], which preserves the pinned key
//!    order and clears `workerPid`.
//!
//! The actual supervisor is injected behind the [`run_worker_core`] `execute`
//! seam (exactly like `daemon.rs` injects `sleep`/`WorkerSpawner`), so a unit
//! test drives the `working → done`/`failed` transition with a stubbed executor
//! and NO live LLM.
//!
//! The supervisor spawns this worker with the `LINGXI_*` background-session
//! environment ([`crate::commands::daemon`]'s `bg_worker_env`), so the child
//! receives the `# Background Session` prompt section and `/stop` resolves the
//! job. A vanished worker is failed closed to avoid duplicate side effects.
//!
//! The worker also owns an authenticated live attach socket when the daemon
//! provides `LINGXI_BG_ATTACH_*` env. `agents attach` connects to that endpoint
//! while the worker is still running, avoiding a second `--resume` JSONL writer.
//! Every byte except Ctrl-Z reaches the child unchanged; Ctrl-Z only detaches.

use crate::agents_registry::{self, SessionRegistration};
use crate::background_launch::{BackgroundLaunchKind, BackgroundLaunchSpec};
use crate::exit_codes;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `__bg-run` subcommand args: the 8-hex job short id to execute.
#[derive(Debug, Clone, clap::Args)]
pub struct Cli {
    /// The `jobs/<short>/` id whose persisted job this worker executes.
    pub short: String,
}

/// Hidden interactive child launched inside the worker-owned PTY.
#[derive(Debug, Clone, clap::Args)]
pub struct PtySessionCli {
    /// The `jobs/<short>/launch.json` context to mount.
    pub short: String,
}

/// Durable job identity passed to the injected PTY supervisor.
#[derive(Debug, Clone)]
pub struct JobSpec {
    /// The job short id (`jobs/<short>/`).
    pub short: String,
    /// Complete owner-only launch context. This is the sole source for prompt,
    /// cwd, resume/fork transcript, runtime flags, and child environment.
    pub launch: BackgroundLaunchSpec,
}

/// Production entrypoint: resolve the shared config home and drive
/// [`run_worker_core`] with the real PTY supervisor.
pub async fn run(cli: &Cli) -> i32 {
    let config_home = crate::run::daemon_runtime_dir();
    let exec_home = config_home.clone();
    run_worker_core(&config_home, &cli.short, move |spec| {
        execute_job(exec_home, spec)
    })
    .await
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
    let launch = match crate::background_launch::load_or_migrate_launch_spec(
        config_home,
        config_home,
        short,
    ) {
        Ok(Some(launch)) => launch,
        Ok(None) => {
            let _ = agents_registry::update_job_state_with_detail(
                config_home,
                short,
                "failed",
                None,
                "background launch context is missing or incompatible",
            );
            return exit_codes::SUCCESS;
        }
        Err(error) => {
            let _ = agents_registry::update_job_state_with_detail(
                config_home,
                short,
                "failed",
                None,
                &format!("could not load background launch context: {error}"),
            );
            return exit_codes::SUCCESS;
        }
    };
    let spec = JobSpec {
        short: short.to_string(),
        launch,
    };

    // The PTY child owns the live SessionRegistration. Registering this parent
    // too creates duplicate fleet rows and masks the TUI's idle/waiting state
    // with the supervisor's unconditional `busy` state.
    let outcome = execute(spec).await;
    let new_state = if outcome.is_ok() { "done" } else { "failed" };
    if let Err(e) = agents_registry::update_job_state(config_home, short, new_state, None) {
        tracing::warn!("lingxi-cli __bg-run: could not persist terminal job state: {e}");
    }

    exit_codes::SUCCESS
}

/// Spawn the normal CLI TUI in a real PTY and supervise its raw I/O for the
/// lifetime of the background job. The child remains mounted while idle; an
/// attach-client disconnect only removes a controller and never ends it.
async fn execute_job(config_home: PathBuf, job: JobSpec) -> Result<(), String> {
    let launch = job.launch;
    if !launch.preflight_approved {
        return Err("background launch preflight was not approved".to_string());
    }

    let attach_hub = crate::bg_attach::AttachHub::start_from_env()
        .map_err(|e| format!("could not start live attach endpoint: {e}"))?
        .ok_or_else(|| {
            "background worker is missing its protocol-v2 attach endpoint".to_string()
        })?;
    let cwd = nonempty_path(&launch.cwd)?;
    let executable = std::env::current_exe()
        .map_err(|e| format!("could not resolve current executable: {e}"))?;
    let program = executable.to_string_lossy().into_owned();
    let args = vec!["__bg-pty-session".to_string(), job.short.clone()];
    // The launch spec is the complete, allowlisted child environment. Never
    // inherit the daemon environment here: it can contain credentials or
    // per-process state intentionally excluded during dispatch.
    let mut env: HashMap<String, String> = launch
        .env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    env.insert("LINGXI_BG_PTY_CHILD".to_string(), "1".to_string());
    let mut pty_size = platform_pty::TerminalSize {
        rows: launch.terminal.rows.max(1),
        cols: launch.terminal.cols.max(1),
    };
    let spawned =
        platform_pty::spawn_pty_process(&program, &args, &cwd, &env, &None, pty_size, &[])
            .await
            .map_err(|e| format!("could not spawn background PTY child: {e}"))?;

    let child_pid = spawned
        .session
        .process_id()
        .ok_or_else(|| "background PTY backend did not expose a child pid".to_string())?;
    let child_proc_start = i32::try_from(child_pid)
        .ok()
        .and_then(crate::daemon_roster::read_proc_start)
        .ok_or_else(|| {
            spawned.session.terminate();
            "could not establish a PID-reuse-safe background PTY identity".to_string()
        })?;
    let runtime_record = crate::background_launch::BackgroundPtyRuntime {
        schema_version: 1,
        short: job.short.clone(),
        worker_pid: i32::try_from(std::process::id()).unwrap_or(i32::MAX),
        child_pid,
        child_proc_start: Some(child_proc_start),
        process_group_id: spawned.session.process_group_id(),
    };
    if let Err(error) =
        crate::background_launch::write_pty_runtime(&config_home, &job.short, &runtime_record)
    {
        spawned.session.terminate();
        return Err(format!(
            "could not persist background PTY runtime identity: {error}"
        ));
    }
    let _runtime_record_guard = PtyRuntimeRecordGuard {
        config_home: config_home.clone(),
        short: job.short.clone(),
    };

    let platform_pty::SpawnedProcess {
        session,
        mut stdout_rx,
        stderr_rx: _stderr_rx,
        mut exit_rx,
    } = spawned;
    attach_hub.ready();
    let output_hub = attach_hub.clone();
    let mut output_task = tokio::spawn(async move {
        while let Some(bytes) = stdout_rx.recv().await {
            output_hub.broadcast(&bytes);
        }
    });
    let mut input_rx = attach_hub.take_input_rx();
    let mut shutdown = Box::pin(shutdown_signal());
    let exit_code = loop {
        tokio::select! {
            code = &mut exit_rx => break code.unwrap_or(-1),
            () = &mut shutdown => {
                let _ = session.signal(platform_pty::ProcessSignal::Terminate);
                let code = match tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    &mut exit_rx,
                ).await {
                    Ok(Ok(code)) => code,
                    _ => {
                        let _ = session.signal(platform_pty::ProcessSignal::Kill);
                        session.wait().await
                    }
                };
                break code;
            }
            input = recv_attach_input(&mut input_rx), if input_rx.is_some() => {
                match input {
                    Some(crate::bg_attach::AttachInput::Bytes(bytes)) => {
                        session.write(bytes).await.map_err(|e| e.to_string())?;
                    }
                    Some(crate::bg_attach::AttachInput::Resize { cols, rows })
                        if cols > 0 && rows > 0 => {
                            let requested = platform_pty::TerminalSize { rows, cols };
                            // portable-pty/ConPTY may coalesce an unchanged size.
                            // Force a neighboring size first so reconnect always
                            // delivers a real resize and the TUI fully repaints.
                            if requested == pty_size {
                                let neighbor = neighboring_size(requested);
                                session
                                    .resize(neighbor)
                                    .map_err(|e| format!("could not nudge background PTY: {e}"))?;
                            }
                            session
                                .resize(requested)
                                .map_err(|e| format!("could not resize background PTY: {e}"))?;
                            pty_size = requested;
                    }
                    Some(crate::bg_attach::AttachInput::Detach)
                    | Some(crate::bg_attach::AttachInput::ClientDetached) => {}
                    Some(crate::bg_attach::AttachInput::Resize { .. }) => {}
                    None => input_rx = None,
                }
            }
        }
    };

    // Child wait can win before the blocking PTY reader has delivered its tail.
    if tokio::time::timeout(std::time::Duration::from_secs(2), &mut output_task)
        .await
        .is_err()
    {
        output_task.abort();
    }
    attach_hub.exit(exit_code);
    if exit_code == 0 {
        Ok(())
    } else {
        Err(format!(
            "background PTY child exited with status {exit_code}"
        ))
    }
}

struct PtyRuntimeRecordGuard {
    config_home: PathBuf,
    short: String,
}

impl Drop for PtyRuntimeRecordGuard {
    fn drop(&mut self) {
        crate::background_launch::remove_pty_runtime(&self.config_home, &self.short);
    }
}

fn neighboring_size(size: platform_pty::TerminalSize) -> platform_pty::TerminalSize {
    let cols = if size.cols > 1 {
        size.cols - 1
    } else {
        size.cols.saturating_add(1).max(1)
    };
    platform_pty::TerminalSize { cols, ..size }
}

async fn recv_attach_input(
    receiver: &mut Option<tokio::sync::mpsc::Receiver<crate::bg_attach::AttachInput>>,
) -> Option<crate::bg_attach::AttachInput> {
    match receiver.as_mut() {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

fn nonempty_path(value: &str) -> Result<PathBuf, String> {
    if value.trim().is_empty() {
        std::env::current_dir().map_err(|e| format!("could not resolve job cwd: {e}"))
    } else {
        Ok(PathBuf::from(value))
    }
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut interrupt = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    let mut hangup = signal(SignalKind::hangup()).expect("install SIGHUP handler");
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
        _ = hangup.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Run the hidden child within the worker-owned PTY. This is the sole owner of
/// the live background registration and the normal interactive TUI runtime.
pub async fn run_pty_session(cli: &PtySessionCli) -> i32 {
    let config_home = crate::run::daemon_runtime_dir();
    let launch = match crate::background_launch::read_launch_spec(&config_home, &cli.short) {
        Ok(spec) => spec,
        Err(e) => {
            eprintln!("lingxi-cli: could not load background launch spec: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    };
    if !launch.preflight_approved {
        eprintln!("lingxi-cli: background launch preflight was not approved");
        return exit_codes::RUNTIME_ERROR;
    }
    if let Err(e) = std::env::set_current_dir(&launch.cwd) {
        eprintln!(
            "lingxi-cli: could not enter background cwd {}: {e}",
            launch.cwd
        );
        return exit_codes::RUNTIME_ERROR;
    }
    let session_id = match uuid::Uuid::parse_str(&launch.session_id) {
        Ok(session_id) => session_id,
        Err(e) => {
            eprintln!("lingxi-cli: invalid background session id: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    };
    let mut argv = launch.tui_argv();
    // Resolve session-scoped downloads in the process that owns the TUI
    // lifetime. The guard keeps plugin archives alive until this child exits.
    let _startup_resources = match crate::startup_resources::prepare(&mut argv).await {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("lingxi-cli: background startup resource setup failed: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    };
    let name = launch.options.name.clone().or_else(|| {
        agents_registry::read_job(&config_home, &cli.short).and_then(|job| job.name.or(job.intent))
    });
    let registration = Arc::new(SessionRegistration::register_bg(
        &config_home,
        Some(&launch.session_id),
        name.as_deref(),
        &cli.short,
    ));
    registration.update_status("idle", None);

    let initial_prompt = launch
        .initial_prompt
        .clone()
        .filter(|prompt| !prompt.trim().is_empty());
    let outcome = match launch.launch {
        BackgroundLaunchKind::Fresh => {
            let tui_build =
                match crate::init::build_runtime_for_tui_inner(&argv, Some(session_id)).await {
                    Ok(tui_build) => tui_build,
                    Err(e) => {
                        eprintln!("lingxi-cli: background TUI init failed: {e}");
                        registration.deregister();
                        return exit_codes::RUNTIME_ERROR;
                    }
                };
            crate::mode::run_ratatui_with_initial_prompt(
                tui_build,
                Some(registration.clone()),
                Vec::new(),
                initial_prompt,
            )
            .await
        }
        BackgroundLaunchKind::Resume | BackgroundLaunchKind::Fork => {
            let messages = match load_exact_transcript(&config_home, &launch).await {
                Ok(messages) => messages,
                Err(e) => {
                    eprintln!("lingxi-cli: could not resume background session: {e}");
                    registration.deregister();
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            crate::run::mount_background_resumed_tui(
                &argv,
                session_id,
                messages,
                registration.clone(),
                initial_prompt,
            )
            .await
        }
    };
    let code = crate::run::drive_background_tui_switch_loop(
        &argv,
        outcome,
        Some(session_id),
        registration.clone(),
    )
    .await;
    registration.deregister();
    code
}

async fn load_exact_transcript(
    config_home: &Path,
    launch: &BackgroundLaunchSpec,
) -> Result<Vec<session::jsonl::JsonlMessage>, String> {
    let canonical_path = validate_exact_transcript_path(config_home, launch)?;
    let cwd = nonempty_path(&launch.cwd)?;
    let fs: Arc<dyn traits::FileSystem> = Arc::new(platform_posix::PosixFileSystem::new(cwd));
    let reader = session::jsonl::JsonlReader::new(canonical_path, fs);
    let loaded = reader.read_routed().await.map_err(|e| e.to_string())?;
    let (chain, _) = session::jsonl::build_conversation_chain(&loaded, &launch.session_id);
    if chain.is_empty() {
        Err("transcript contains no resumable conversation".to_string())
    } else {
        Ok(chain)
    }
}

fn validate_exact_transcript_path(
    config_home: &Path,
    launch: &BackgroundLaunchSpec,
) -> Result<PathBuf, String> {
    let path = PathBuf::from(&launch.transcript_path);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|e| format!("could not inspect transcript {}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("transcript is not a regular file".to_string());
    }
    let canonical_path = std::fs::canonicalize(&path)
        .map_err(|e| format!("could not resolve transcript {}: {e}", path.display()))?;
    let projects_root = config_home.join("projects");
    let canonical_projects = std::fs::canonicalize(&projects_root).map_err(|e| {
        format!(
            "could not resolve transcript root {}: {e}",
            projects_root.display()
        )
    })?;
    let expected_name = format!("{}.jsonl", launch.session_id);
    if !canonical_path.starts_with(&canonical_projects)
        || canonical_path.file_name().and_then(|name| name.to_str()) != Some(&expected_name)
    {
        return Err("transcript path does not match the recorded session".to_string());
    }
    Ok(canonical_path)
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
            // Public fleet state deliberately excludes the raw prompt.
            initial_prompt: None,
            detail: None,
            worker_pid: None,
        };
        write_job_state(home, short, &job).unwrap();
        crate::background_launch::write_launch_spec(
            home,
            short,
            &BackgroundLaunchSpec {
                schema_version: crate::background_launch::LAUNCH_SPEC_VERSION,
                short: short.to_string(),
                created_at: 1,
                preflight_approved: true,
                launch: BackgroundLaunchKind::Fresh,
                session_id: "11111111-1111-1111-1111-111111111111".to_string(),
                transcript_path: home.join("session.jsonl").display().to_string(),
                cwd: cwd.clone(),
                origin_cwd: cwd,
                worktree_path: None,
                worktree_ownership_token: None,
                initial_prompt: Some(prompt.to_string()),
                options: crate::background_launch::BackgroundLaunchOptions::default(),
                env: std::collections::BTreeMap::new(),
                terminal: crate::background_launch::TerminalSize::default(),
            },
        )
        .unwrap();
    }

    #[test]
    fn unchanged_resize_has_a_safe_repaint_neighbor() {
        let size = platform_pty::TerminalSize { rows: 24, cols: 80 };
        assert_eq!(
            neighboring_size(size),
            platform_pty::TerminalSize { rows: 24, cols: 79 }
        );
        let narrow = platform_pty::TerminalSize { rows: 1, cols: 1 };
        assert_eq!(neighboring_size(narrow).cols, 2);
    }

    #[tokio::test]
    async fn ok_execution_marks_job_done() {
        let home = tmpdir();
        seed_job(&home, "bc7c6b33", "port the daemon");
        let mut seen: Option<String> = None;
        let code = run_worker_core(&home, "bc7c6b33", |spec| {
            seen = spec.launch.initial_prompt.clone();
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
    async fn promptless_launch_still_mounts_an_idle_background_tui() {
        let home = tmpdir();
        seed_job(&home, "eeee5555", "   ");
        let mut called = false;
        let code = run_worker_core(&home, "eeee5555", |_spec| {
            called = true;
            async move { Ok(()) }
        })
        .await;
        assert_eq!(code, exit_codes::SUCCESS);
        assert!(called, "a promptless launch remains attachable while idle");
        assert_eq!(read_job(&home, "eeee5555").unwrap().state, "done");
    }

    #[tokio::test]
    async fn missing_private_launch_context_fails_closed() {
        let home = tmpdir();
        seed_job(&home, "face0001", "private prompt");
        std::fs::remove_file(crate::background_launch::launch_spec_path(
            &home, "face0001",
        ))
        .unwrap();
        let mut called = false;
        let code = run_worker_core(&home, "face0001", |_spec| {
            called = true;
            async move { Ok(()) }
        })
        .await;
        assert_eq!(code, exit_codes::SUCCESS);
        assert!(!called);
        let job = read_job(&home, "face0001").unwrap();
        assert_eq!(job.state, "failed");
        assert!(job
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("launch context")));
    }

    #[tokio::test]
    async fn supervisor_does_not_register_a_duplicate_bg_session() {
        let home = tmpdir();
        seed_job(&home, "ffff6666", "observe me");
        let observed = std::cell::Cell::new(0usize);
        let code = run_worker_core(&home, "ffff6666", |_spec| {
            let sessions =
                agents_registry::read_live_sessions(&agents_registry::sessions_dir(&home));
            observed.set(sessions.iter().filter(|s| s.kind == "bg").count());
            async move { Ok(()) }
        })
        .await;
        assert_eq!(code, exit_codes::SUCCESS);
        assert_eq!(observed.get(), 0, "only the PTY child may register the job");
    }

    #[cfg(unix)]
    #[test]
    fn exact_transcript_validation_rejects_symlink_and_outside_paths() {
        use std::os::unix::fs::symlink;

        let home = tmpdir();
        seed_job(&home, "aaaa7777", "resume");
        let mut launch = crate::background_launch::read_launch_spec(&home, "aaaa7777").unwrap();
        let projects = home.join("projects").join("-repo");
        std::fs::create_dir_all(&projects).unwrap();
        let exact = projects.join(format!("{}.jsonl", launch.session_id));
        std::fs::write(&exact, b"{}\n").unwrap();
        launch.transcript_path = exact.display().to_string();
        assert_eq!(
            validate_exact_transcript_path(&home, &launch).unwrap(),
            std::fs::canonicalize(&exact).unwrap()
        );

        let outside = home.join(format!("{}.jsonl", launch.session_id));
        std::fs::write(&outside, b"{}\n").unwrap();
        launch.transcript_path = outside.display().to_string();
        assert!(validate_exact_transcript_path(&home, &launch).is_err());

        let link = projects.join(format!("{}.jsonl", launch.session_id));
        std::fs::remove_file(&link).unwrap();
        symlink(&outside, &link).unwrap();
        launch.transcript_path = link.display().to_string();
        assert!(validate_exact_transcript_path(&home, &launch).is_err());
    }
}
