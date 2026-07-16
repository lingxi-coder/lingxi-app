//! `--background`/`--bg` dispatch (increment 2 of the coherent-minimum daemon):
//! the JOB WRITER + roster handoff.
//!
//! When `--bg` is set the CLI does NOT build the in-process engine/turn. It
//! instead, purely by writing files + ensuring the daemon:
//!
//! 1. Mints a short id + session UUID.
//! 2. Writes the durable `jobs/<short>/state.json` (the primary visibility
//!    artifact — a workerless `state:"working"` row `agents --json` renders
//!    without `--all`).
//! 3. Appends a [`WorkerRecord`]/[`Dispatch`] keyed by `<short>` into
//!    `roster.json` — this file IS the CLI→supervisor dispatch channel (no
//!    `control.sock`).
//! 4. ENSURES the supervisor: if no live holder owns `daemon.lock`, spawns a
//!    detached `lingxi-cli daemon` child; the daemon consumes the handoff by
//!    re-reading `roster.json`.
//! 5. Prints `<short>` and returns.
//!
//! This is NOT the in-process `registerAsyncAgent` path
//! (`engine-desktop::background_agent`) — that is unrelated to the OS daemon.
//!
//! Where exact parity is unrecoverable (there is no source/binary/strings
//! reference for the orchestration layer) the choices are grounded, not
//! byte-verified — see the daemon design `parity_choices`. In particular:
//! `rendezvousSock` is an empty placeholder (the real value is the out-of-scope
//! `control.sock` rendezvous path), the [`WorkerRecord`] carries the transient
//! CLI pid (reaped by `retain_adoptable` once the CLI exits — the DURABLE record
//! is `jobs/<short>/state.json`), and no live `kind:"bg"` session is registered
//! (deferred to the future pty-backed worker via
//! [`SessionRegistration::register_bg`](crate::agents_registry::SessionRegistration::register_bg)).

use crate::agents_registry::{self, JobStateWrite};
use crate::argv::Argv;
use crate::daemon_lock::{self, LockProbe, SystemLockProbe};
use crate::daemon_roster::{self, Dispatch, DispatchSource, Isolation, Launch, Seed, WorkerRecord};
use crate::exit_codes;
use std::collections::BTreeMap;
use std::path::Path;

/// Spawns the detached daemon child. Abstracted so the writer + handoff are
/// testable without launching a real process.
pub trait DaemonSpawner {
    /// Spawn `argv` (with `argv[0]` the executable) fully detached — the CLI
    /// must return immediately.
    fn spawn(&mut self, argv: &[String]) -> std::io::Result<()>;
}

/// Production spawner: a detached `Command` with null stdio, placed in its own
/// process group (so a terminal SIGINT/SIGHUP to the CLI's foreground group is
/// not delivered to the daemon). No `unsafe` — `process_group` is a safe
/// `CommandExt` method (the crate is `#![forbid(unsafe_code)]`, so `setsid` via
/// `pre_exec` is unavailable; a fresh process group is the detachment we can get
/// safely and is sufficient for the coherent minimum).
struct RealSpawner;

impl DaemonSpawner for RealSpawner {
    fn spawn(&mut self, argv: &[String]) -> std::io::Result<()> {
        use std::process::{Command, Stdio};
        let Some((program, args)) = argv.split_first() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty daemon argv",
            ));
        };
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        // Spawn and drop the handle — we do NOT wait.
        cmd.spawn().map(|_child| ())
    }
}

/// Epoch-millis (roster/dispatch `createdAt`/`startedAt` are epoch-millis `i64`).
fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The first line of the prompt, trimmed and length-capped, used as the job
/// intent/label. `None` for an empty/whitespace-only prompt.
fn intent_from_prompt(prompt: Option<&str>) -> Option<String> {
    let first = prompt?.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        return None;
    }
    // Cap the seeded intent so a giant prompt doesn't bloat the roster/job row.
    const MAX: usize = 200;
    let capped: String = first.chars().take(MAX).collect();
    Some(capped)
}

/// Reconstruct the launch args the worker would run with (`Launch::Prompt`):
/// the `--background` flag plus the prompt token, if any.
fn reconstruct_launch_args(argv: &Argv) -> Vec<String> {
    let mut args = vec!["--background".to_string()];
    if let Some(p) = argv.prompt.as_deref() {
        args.push(p.to_string());
    }
    args
}

/// `--bg` entry point (production): resolves the shared config home + daemon
/// runtime dir and dispatches with the real spawner.
pub async fn dispatch_background(argv: &Argv) -> i32 {
    let config_home = crate::run::lingxi_home_dir();
    let runtime_dir = crate::run::daemon_runtime_dir();
    dispatch_background_inner(
        argv,
        &config_home,
        &runtime_dir,
        &SystemLockProbe,
        &mut RealSpawner,
    )
}

/// Testable core: `config_home`/`runtime_dir`, the lock-liveness probe, and the
/// daemon spawner are all injected.
fn dispatch_background_inner<LP: LockProbe, S: DaemonSpawner>(
    argv: &Argv,
    config_home: &Path,
    runtime_dir: &Path,
    lock_probe: &LP,
    spawner: &mut S,
) -> i32 {
    let cli_pid = i32::try_from(std::process::id()).unwrap_or(i32::MAX);
    let version = env!("CARGO_PKG_VERSION");
    let now = now_millis();

    // 1. Mint ids.
    let short = agents_registry::mint_short_id(config_home);
    let session_id = uuid::Uuid::new_v4().to_string();

    // 2. Fields.
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let intent = intent_from_prompt(argv.prompt.as_deref());

    // 3. WRITE THE JOB (the primary visibility artifact).
    let respawn_flags: Vec<String> = Vec::new();
    let job = JobStateWrite {
        state: "working",
        tempo: Some("active"),
        name: None,
        session_id: Some(&session_id),
        cwd: Some(&cwd),
        origin_cwd: Some(&cwd),
        created_at: Some(&created_at),
        intent: intent.as_deref(),
        display_intent: None,
        template: Some("bg"),
        respawn_flags: &respawn_flags,
        in_flight: None,
        backend: Some("daemon"),
        initial_prompt: argv.prompt.as_deref(),
        detail: None,
        // No live worker yet — the supervisor records the worker pid on spawn.
        worker_pid: None,
    };
    if let Err(e) = agents_registry::write_job_state(config_home, &short, &job) {
        eprintln!("lingxi-cli: could not write background job: {e}");
        return exit_codes::RUNTIME_ERROR;
    }

    // 4. ROSTER HANDOFF — append a WorkerRecord/Dispatch keyed by <short>.
    let mut roster = daemon_roster::read_roster(runtime_dir, 0, false).into_roster();
    let record = WorkerRecord {
        pid: cli_pid,
        proc_start: daemon_roster::read_proc_start(cli_pid),
        session_id: session_id.clone(),
        // Placeholder: the real value is the out-of-scope control.sock rendezvous
        // path. Honors the (non-Option) schema, NOT byte-parity.
        rendezvous_sock: String::new(),
        pty_sock: None,
        messaging_sock: None,
        cli_version: Some(version.to_string()),
        started_at: now,
        attempt: 0,
        cwd: cwd.clone(),
        worktree_path: None,
        dispatch: Dispatch {
            proto: daemon_roster::PROTO,
            short: short.clone(),
            nonce: None,
            session_id: session_id.clone(),
            created_at: now,
            source: DispatchSource::Shell,
            cwd: cwd.clone(),
            launch: Launch::Prompt {
                args: reconstruct_launch_args(argv),
            },
            env: dispatch_env(),
            reattach_env: None,
            worktree: None,
            isolation: Isolation::None,
            respawn_flags: Vec::new(),
            attach_stall_respawns: None,
            agent: None,
            routine: None,
            seed: intent.clone().map(|intent| Seed { intent, name: None }),
            cols: None,
            rows: None,
        },
        pending_respawn: None,
        dec_modes: None,
        rv_auth: None,
        pty_auth: None,
        extra: serde_json::Map::new(),
    };
    roster.workers.insert(short.clone(), record);
    let _ = daemon_roster::write_roster(runtime_dir, &roster);

    // 5. ENSURE THE DAEMON — if no live holder owns the lock, spawn one detached.
    ensure_daemon(runtime_dir, lock_probe, spawner);

    // 6. Print the job id + return.
    println!("{short}");
    exit_codes::SUCCESS
}

/// Spawn a detached `lingxi-cli daemon` unless a live supervisor already holds
/// `daemon.lock`. A benign double-spawn is fine — `acquire_or_yield` resolves
/// the loser via `Yield`/`AlreadyOurs` (exit 0). The spawned argv carries the
/// literal `daemon` token in `argv[1..4]` so `classify_cmdline` recognises the
/// child (a HARD constraint — else a peer would steal its lock).
fn ensure_daemon<LP: LockProbe, S: DaemonSpawner>(
    runtime_dir: &Path,
    lock_probe: &LP,
    spawner: &mut S,
) {
    if let Some(holder) = daemon_lock::read_lock(runtime_dir) {
        if lock_probe.is_alive(holder.pid) && lock_probe.is_daemon_process(holder.pid) {
            // A live supervisor already owns the fleet — nothing to spawn.
            return;
        }
    }
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "lingxi-cli".to_string());
    let argv =
        crate::process_wrapper::wrap_argv(vec![exe, daemon_lock::DAEMON_SUBCOMMAND.to_string()]);
    if let Err(e) = spawner.spawn(&argv) {
        // Best-effort: a failed spawn leaves the job/roster durable on disk; a
        // later `--bg` or a manually-launched daemon still adopts them.
        tracing::warn!("lingxi-cli: could not spawn background daemon: {e}");
    }
}

fn dispatch_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in [
        "LINGXI_CODE_PROCESS_WRAPPER",
        "CLAUDE_CODE_PROCESS_WRAPPER",
        "LINGXI_HOME",
        "CLAUDE_CONFIG_DIR",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.to_string(), value);
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_roster::{read_roster, roster_path, Launch};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    fn tmpdir() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("lingxi-bgdispatch-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Captures the spawned argv instead of launching a process.
    #[derive(Default)]
    struct CaptureSpawner {
        spawns: Vec<Vec<String>>,
    }
    impl DaemonSpawner for CaptureSpawner {
        fn spawn(&mut self, argv: &[String]) -> std::io::Result<()> {
            self.spawns.push(argv.to_vec());
            Ok(())
        }
    }

    /// Lock probe reporting a fixed set of live daemon pids.
    struct FakeLockProbe {
        live: HashMap<i32, bool>,
    }
    impl LockProbe for FakeLockProbe {
        fn is_alive(&self, pid: i32) -> bool {
            *self.live.get(&pid).unwrap_or(&false)
        }
        fn is_daemon_process(&self, pid: i32) -> bool {
            *self.live.get(&pid).unwrap_or(&false)
        }
        fn proc_start(&self, _pid: i32, _skip: bool) -> Option<String> {
            None
        }
    }

    fn bg_argv(prompt: &str) -> Argv {
        Argv {
            prompt: Some(prompt.to_string()),
            background: true,
            ..Argv::default()
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn writes_job_and_roster_and_spawns_daemon() {
        let home = tmpdir();
        let mut spawner = CaptureSpawner::default();
        let lockp = FakeLockProbe {
            live: HashMap::new(),
        };
        let argv = bg_argv("port the daemon supervisor\nsecond line");
        let code = dispatch_background_inner(&argv, &home, &home, &lockp, &mut spawner);
        assert_eq!(code, exit_codes::SUCCESS);

        // (a) A job row + a roster WorkerRecord exist for the same short id.
        let jobs = agents_registry::read_jobs(&agents_registry::jobs_dir(&home));
        assert_eq!(jobs.len(), 1);
        let short = jobs[0].0.clone();
        assert_eq!(jobs[0].1.state, "working");
        assert_eq!(jobs[0].1.template.as_deref(), Some("bg"));
        assert_eq!(jobs[0].1.backend.as_deref(), Some("daemon"));
        // Intent = first prompt line only.
        assert_eq!(
            jobs[0].1.intent.as_deref(),
            Some("port the daemon supervisor")
        );

        assert!(roster_path(&home).exists());
        let roster = read_roster(&home, 0, false).into_roster();
        assert!(roster.workers.contains_key(&short));
        let rec = &roster.workers[&short];
        match &rec.dispatch.launch {
            Launch::Prompt { args } => {
                assert_eq!(args[0], "--background");
                assert!(args.iter().any(|a| a.contains("port the daemon")));
            }
            other => panic!("expected Prompt launch, got {other:?}"),
        }
        assert_eq!(
            rec.dispatch.seed.as_ref().unwrap().intent,
            "port the daemon supervisor"
        );

        // (b) The captured spawn argv carries the literal `daemon` token in
        //     argv[1..4] (classify_cmdline's recognition window).
        assert_eq!(spawner.spawns.len(), 1);
        let spawned = &spawner.spawns[0];
        assert!(
            spawned.iter().skip(1).take(3).any(|t| t == "daemon"),
            "daemon token in argv[1..4]: {spawned:?}"
        );
    }

    #[test]
    fn skips_spawn_when_a_live_daemon_holds_the_lock() {
        let home = tmpdir();
        // A live supervisor already owns the lock.
        let mut held = daemon_lock::DaemonLock::new(5555, "0.0.0");
        held.proc_start = Some("S".to_string());
        daemon_lock::acquire(&home, &held).unwrap();

        let mut live = HashMap::new();
        live.insert(5555, true);
        let lockp = FakeLockProbe { live };
        let mut spawner = CaptureSpawner::default();

        let code = dispatch_background_inner(&bg_argv("hello"), &home, &home, &lockp, &mut spawner);
        assert_eq!(code, exit_codes::SUCCESS);
        // Job + roster still written, but no daemon spawned (live holder).
        assert_eq!(
            agents_registry::read_jobs(&agents_registry::jobs_dir(&home)).len(),
            1
        );
        assert!(
            spawner.spawns.is_empty(),
            "no spawn when a live daemon holds the lock"
        );
    }

    #[test]
    fn dispatch_records_env_and_wraps_daemon_spawn() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("LINGXI_CODE_PROCESS_WRAPPER", "/tmp/wrap --trace");

        let home = tmpdir();
        let mut spawner = CaptureSpawner::default();
        let lockp = FakeLockProbe {
            live: HashMap::new(),
        };
        let code = dispatch_background_inner(&bg_argv("hello"), &home, &home, &lockp, &mut spawner);

        std::env::remove_var("LINGXI_CODE_PROCESS_WRAPPER");

        assert_eq!(code, exit_codes::SUCCESS);
        let roster = read_roster(&home, 0, false).into_roster();
        let rec = roster.workers.values().next().expect("worker record");
        assert_eq!(
            rec.dispatch
                .env
                .get("LINGXI_CODE_PROCESS_WRAPPER")
                .map(String::as_str),
            Some("/tmp/wrap --trace")
        );

        let spawned = spawner.spawns.first().expect("daemon spawn");
        assert_eq!(spawned.first().map(String::as_str), Some("/tmp/wrap"));
        assert!(
            spawned.iter().skip(1).take(4).any(|t| t == "daemon"),
            "daemon token preserved after wrapper: {spawned:?}"
        );
    }

    #[test]
    fn empty_prompt_yields_no_intent() {
        assert_eq!(intent_from_prompt(Some("   ")), None);
        assert_eq!(intent_from_prompt(None), None);
        assert_eq!(
            intent_from_prompt(Some("first\nsecond")).as_deref(),
            Some("first")
        );
    }
}
