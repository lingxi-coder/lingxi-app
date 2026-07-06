//! `lingxi-cli daemon` — the background-agent **supervisor** (increment 1 of the
//! coherent-minimum daemon).
//!
//! This is the hidden, internal subcommand the `--bg` dispatcher spawns (never
//! user-facing). It composes the already-ported primitives verbatim:
//!
//! 1. [`daemon_lock::acquire_or_yield`] — take the single-supervisor
//!    `daemon.lock`, take over a stale one, or **yield** (exit 0) to a live peer.
//! 2. INITIAL ADOPT — [`daemon_roster::read_roster`] + [`retain_adoptable`] to
//!    reap dead/recycled workers, then re-persist with our `supervisorPid` +
//!    bumped `updatedAt`.
//! 3. SUPERVISE LOOP — a minimal heartbeat that re-reads `roster.json` (the
//!    file the `--bg` CLI appends dispatch rows to — the roster IS the dispatch
//!    channel), reaps, and re-persists `updatedAt` every [`HEARTBEAT_MS`].
//!    "Adopt new pending dispatches" in the coherent minimum = persisting the
//!    rows re-read from disk; NO worker spawn (spawn needs pty/control.sock,
//!    which is out of scope).
//! 4. SHUTDOWN — on SIGTERM/SIGINT, break the loop and [`daemon_lock::release`]
//!    our lock.
//!
//! DELIBERATELY OUT OF SCOPE (unrecoverable wire protocol / higher risk, all
//! follow-ons): the auth'd `control.sock` + `rvAuth`/`ptyAuth` IPC, actual
//! pty-backed worker spawn, respawn/stall watchdog, low-memory handling, orphan
//! reap beyond `retain_adoptable`, and upgrade takeover.
//!
//! The literal `daemon` token in `argv[1..4]` is what lets
//! [`daemon_lock::classify_cmdline`] recognise this child as one of *our* daemon
//! processes — without it a launching peer's [`daemon_lock::evaluate_holder`]
//! would classify us as `StaleReason::NotDaemon` and steal our live lock. A
//! `clap` subcommand token gives us that argv slot for free.

use crate::agents_registry;
use crate::daemon_lock::{self, DaemonLock, LockProbe, SystemLockProbe};
use crate::daemon_roster::{
    self, Dispatch, DispatchSource, Isolation, Launch, ProcProbe, Roster, Seed, SystemProbe,
    WorkerRecord, PROTO,
};
use crate::exit_codes;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Heartbeat cadence for the supervise loop (grounded engineering choice, NOT
/// byte-parity).
const HEARTBEAT_MS: u64 = 2000;

/// Spawns a detached `__bg-run <short>` worker process. Abstracted (like
/// [`crate::background_dispatch::DaemonSpawner`]) so the supervise loop's
/// spawn decisions are testable without launching a real process.
pub trait WorkerSpawner {
    /// Spawn the detached headless worker for job `short`, returning its pid.
    fn spawn_worker(&mut self, short: &str) -> std::io::Result<i32>;
}

/// Production worker spawner: a detached `<current_exe> __bg-run <short>` with
/// null stdio in its own process group (so a terminal signal to the daemon's
/// group is not delivered to the worker). No `unsafe` — `process_group` is a
/// safe `CommandExt` method (the crate is `#![forbid(unsafe_code)]`).
struct RealWorkerSpawner;

impl WorkerSpawner for RealWorkerSpawner {
    fn spawn_worker(&mut self, short: &str) -> std::io::Result<i32> {
        use std::process::{Command, Stdio};
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.arg("__bg-run")
            .arg(short)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let child = cmd.spawn()?;
        Ok(i32::try_from(child.id()).unwrap_or(i32::MAX))
    }
}

/// `daemon` subcommand args — no options (internal, spawned by `--bg`).
#[derive(Debug, Clone, clap::Args)]
pub struct Cli {}

/// Current epoch-millis (roster `updatedAt` is epoch-millis `i64`).
fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Run the supervisor. Acquires the lock (or yields), adopts the roster, then
/// heartbeats until a termination signal trips the shutdown flag.
pub async fn run(_cli: &Cli) -> i32 {
    let runtime_dir = crate::run::daemon_runtime_dir();
    let pid = i32::try_from(std::process::id()).unwrap_or(i32::MAX);
    let version = env!("CARGO_PKG_VERSION");

    // Shutdown flag, tripped by SIGTERM/SIGINT. The blocking supervise loop polls
    // it each heartbeat, so shutdown lands within one HEARTBEAT_MS of the signal.
    let stop = Arc::new(AtomicBool::new(false));
    spawn_signal_watch(stop.clone());
    // Reap exited `__bg-run` worker children so finished workers don't pile up
    // as `<defunct>` zombies in the daemon's process table (see
    // [`spawn_child_reaper`]).
    #[cfg(unix)]
    spawn_child_reaper(stop.clone());

    let stop_for_loop = stop.clone();
    let result = tokio::task::spawn_blocking(move || {
        run_supervisor(
            &runtime_dir,
            pid,
            version,
            &SystemLockProbe,
            &SystemProbe,
            &mut RealWorkerSpawner,
            HEARTBEAT_MS,
            &mut |ms| std::thread::sleep(std::time::Duration::from_millis(ms)),
            &mut || stop_for_loop.load(Ordering::Relaxed),
        )
    })
    .await;

    result.unwrap_or(exit_codes::RUNTIME_ERROR)
}

/// Trip `stop` on the next SIGTERM or SIGINT (Ctrl-C). Best-effort: if the
/// signal streams can't be installed the daemon simply runs until killed.
fn spawn_signal_watch(stop: Arc<AtomicBool>) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate()).ok();
            let mut intr = signal(SignalKind::interrupt()).ok();
            match (term.as_mut(), intr.as_mut()) {
                (Some(t), Some(i)) => {
                    tokio::select! {
                        _ = t.recv() => {},
                        _ = i.recv() => {},
                    }
                }
                (Some(t), None) => {
                    t.recv().await;
                }
                (None, Some(i)) => {
                    i.recv().await;
                }
                (None, None) => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        stop.store(true, Ordering::Relaxed);
    });
}

/// Reap exited worker children so they don't accumulate as zombies.
///
/// The daemon spawns detached `__bg-run` workers (see [`RealWorkerSpawner`])
/// but never `wait(2)`s on them, so each exited worker would linger as a
/// `<defunct>` zombie in the daemon's process table until the daemon itself
/// exits. On Linux+macOS setting the `SIGCHLD` disposition to `SIG_IGN` makes
/// the kernel auto-reap children — but every disposition-setting API
/// (`nix::sys::signal::{signal, sigaction}`, `libc::signal`) is `unsafe`, and
/// this crate is `#![forbid(unsafe_code)]`. So we get the same no-zombie
/// guarantee the safe way: a task that wakes on each `SIGCHLD` and drains every
/// reapable child with a non-blocking `waitpid(-1, WNOHANG)` loop (the daemon's
/// only OS children are its workers, so a blanket reap is correct).
#[cfg(unix)]
fn spawn_child_reaper(stop: Arc<AtomicBool>) {
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        // If the SIGCHLD stream can't be installed, fall back to relying on the
        // OS to reap at daemon exit (best-effort, mirrors spawn_signal_watch).
        let Ok(mut sigchld) = signal(SignalKind::child()) else {
            return;
        };
        loop {
            // SIGCHLD can coalesce when several workers exit at once, so drain
            // all currently-reapable children on every wake.
            reap_exited_children();
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if sigchld.recv().await.is_none() {
                return;
            }
        }
    });
}

/// Non-blocking drain of all exited children: `waitpid(-1, WNOHANG)` until it
/// reports `StillAlive` (nothing more to reap right now) or errors (`ECHILD`
/// when the daemon has no children). Fully safe — `nix::sys::wait::waitpid` is a
/// safe wrapper (the `signal` feature already pulls in nix's `process` module).
#[cfg(unix)]
fn reap_exited_children() {
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    loop {
        match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
            // Nothing more to reap right now (`StillAlive`) or no children at
            // all (`Err` = ECHILD, or an interrupted call) → stop draining.
            Ok(WaitStatus::StillAlive) | Err(_) => break,
            // Reaped an exited child; keep draining (SIGCHLD can coalesce).
            Ok(_) => continue,
        }
    }
}

/// The testable supervisor core: `sleep` and `should_stop` are injected so the
/// loop runs with no real time and a deterministic shutdown. `lock_probe` drives
/// [`daemon_lock::acquire_or_yield`]; `proc_probe` drives
/// [`daemon_roster::retain_adoptable`].
#[allow(clippy::too_many_arguments)]
fn run_supervisor<LP: LockProbe, PP: ProcProbe, WS: WorkerSpawner>(
    runtime_dir: &Path,
    pid: i32,
    version: &str,
    lock_probe: &LP,
    proc_probe: &PP,
    spawner: &mut WS,
    heartbeat_ms: u64,
    sleep: &mut dyn FnMut(u64),
    should_stop: &mut dyn FnMut() -> bool,
) -> i32 {
    // 1. Build our lock, filling the recycled-PID guard (`DaemonLock::new` leaves
    //    proc_start None).
    let mut lock = DaemonLock::new(pid, version.to_string());
    lock.proc_start = daemon_roster::read_proc_start(pid);
    lock.origin = Some("service".to_string());

    // 2. Acquire, take over a stale lock, or yield to a live peer.
    match daemon_lock::acquire_or_yield(runtime_dir, &lock, lock_probe, sleep) {
        Ok(daemon_lock::LockOutcome::Yield { holder_pid, .. }) => {
            tracing::info!("{}", daemon_lock::yield_skip_log(Some(holder_pid)));
            daemon_lock::emit_yield();
            return exit_codes::SUCCESS;
        }
        Ok(daemon_lock::LockOutcome::Acquired { .. } | daemon_lock::LockOutcome::AlreadyOurs) => {
            // We own the lock — proceed to adopt + supervise.
        }
        Err(e) => {
            eprintln!("lingxi-cli daemon: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    }

    // Jobs already claimed (spawned this supervisor lifetime), so a benign
    // re-scan does not double-spawn the same job before its worker records a
    // pid. The durable cross-restart guard is the recorded `workerPid` in
    // `state.json` (checked in [`spawn_pending_workers`]).
    let mut claimed: HashSet<String> = HashSet::new();

    // 3. INITIAL ADOPT — read the roster, reap non-adoptable workers, spawn
    //    pending jobs, claim it.
    heartbeat(runtime_dir, pid, version, proc_probe, spawner, &mut claimed);

    // 4. SUPERVISE LOOP — re-read + reap + spawn-pending + re-persist each
    //    heartbeat. The JOB FILES (durable, never reaped) are the source of
    //    truth for pending work; the roster carries live-worker state the
    //    supervisor rewrites on spawn.
    loop {
        if should_stop() {
            break;
        }
        sleep(heartbeat_ms);
        if should_stop() {
            break;
        }
        heartbeat(runtime_dir, pid, version, proc_probe, spawner, &mut claimed);
    }

    // 5. SHUTDOWN — release our lock (ENOENT-swallowing).
    let _ = daemon_lock::release(runtime_dir);
    exit_codes::SUCCESS
}

/// One roster sweep: re-read, drop dead/recycled workers, spawn a detached
/// worker for each pending `--bg` job, stamp our `supervisorPid` + a fresh
/// `updatedAt`, and re-persist.
fn heartbeat<PP: ProcProbe, WS: WorkerSpawner>(
    runtime_dir: &Path,
    pid: i32,
    version: &str,
    proc_probe: &PP,
    spawner: &mut WS,
    claimed: &mut HashSet<String>,
) {
    let mut roster = daemon_roster::read_roster(runtime_dir, pid, true).into_roster();
    let _dropped = daemon_roster::retain_adoptable(&mut roster, proc_probe);
    // Spawn detached workers for pending jobs (mutates the roster with each new
    // live worker record so the NEXT `retain_adoptable` keeps it while alive).
    spawn_pending_workers(runtime_dir, &mut roster, version, proc_probe, spawner, claimed);
    roster.supervisor_pid = pid;
    roster.updated_at = now_millis();
    let _ = daemon_roster::write_roster(runtime_dir, &roster);
}

/// Scan the durable job store and spawn a detached `__bg-run` worker for each
/// pending job that has none. A job is PENDING when it is non-terminal, in
/// `state:"working"`, has not already been claimed this supervisor lifetime,
/// and has no live recorded `workerPid` (the cross-restart double-spawn guard).
///
/// On spawn the child pid is recorded into BOTH `state.json` (via
/// [`agents_registry::update_job_state`], so a restarted supervisor sees a live
/// worker) and a fresh roster [`WorkerRecord`] (so `retain_adoptable` keeps the
/// worker listed while alive and reaps it once it exits).
///
/// CRASH HANDLING: a `working` job whose recorded `workerPid` is NO LONGER alive
/// (the worker died before writing its own terminal state) is marked terminally
/// `failed` here — never respawned — so it can't wedge in "working" forever.
///
/// `runtime_dir` == the config home (`daemon_runtime_dir()`), so the jobs live
/// at `jobs_dir(runtime_dir)`.
fn spawn_pending_workers<PP: ProcProbe, WS: WorkerSpawner>(
    runtime_dir: &Path,
    roster: &mut Roster,
    version: &str,
    proc_probe: &PP,
    spawner: &mut WS,
    claimed: &mut HashSet<String>,
) {
    let jobs = agents_registry::read_jobs(&agents_registry::jobs_dir(runtime_dir));
    for (short, job) in jobs {
        if agents_registry::job_is_terminal(&job) {
            continue;
        }
        if job.state != "working" {
            continue;
        }
        // OWNED job: it has a recorded worker pid, or we spawned it this
        // supervisor lifetime (`claimed`). Its next step depends on whether
        // that worker is still alive.
        if let Some(worker_pid) = job.worker_pid {
            // Cross-restart guard: a recorded, still-live worker pid means a
            // worker is already running this job — adopt (claim) it, don't
            // re-spawn.
            if proc_probe.is_alive(worker_pid) {
                claimed.insert(short);
                continue;
            }
            // The recorded worker DIED without writing a terminal state (the
            // worker itself writes "done"/"failed"). It crashed — mark the job
            // terminally `failed` so it doesn't wedge in "working" forever. We
            // deliberately do NOT auto-respawn: a genuinely-crashing task would
            // respawn endlessly; a human/retry re-dispatches instead.
            if let Err(e) = agents_registry::update_job_state(runtime_dir, &short, "failed", None) {
                tracing::warn!(
                    "lingxi-cli daemon: could not mark crashed job {short} failed: {e}"
                );
            }
            claimed.remove(&short);
            continue;
        }
        // No recorded worker pid. If we already claimed it this lifetime the
        // pid simply hasn't been persisted yet (or its write failed) — protect
        // it from a same-heartbeat double-spawn; we can't probe liveness with
        // no pid, so leave it working for a later heartbeat to resolve.
        if claimed.contains(&short) {
            continue;
        }

        match spawner.spawn_worker(&short) {
            Ok(child_pid) => {
                // Durable pid record (state stays "working").
                if let Err(e) =
                    agents_registry::update_job_state(runtime_dir, &short, "working", Some(child_pid))
                {
                    tracing::warn!(
                        "lingxi-cli daemon: could not record workerPid for {short}: {e}"
                    );
                }
                // Live-worker roster record so retain_adoptable keeps it.
                let proc_start = proc_probe.start_time(child_pid);
                roster.workers.insert(
                    short.clone(),
                    worker_record_for_job(&short, &job, child_pid, proc_start, version),
                );
                claimed.insert(short);
            }
            Err(e) => {
                // Best-effort: the durable job stays "working"; the next
                // heartbeat retries the spawn.
                tracing::warn!("lingxi-cli daemon: could not spawn worker for {short}: {e}");
            }
        }
    }
}

/// Build a live-worker [`WorkerRecord`] for a spawned worker from its durable
/// job. Only the fields the roster reaper/reader consume are meaningful here
/// (`pid`/`procStart` drive `retain_adoptable`); the PTY/rendezvous sockets and
/// auth tokens stay empty/absent (out of scope — this is a plain headless
/// worker, not a PTY worker).
fn worker_record_for_job(
    short: &str,
    job: &agents_registry::JobState,
    pid: i32,
    proc_start: Option<String>,
    version: &str,
) -> WorkerRecord {
    let session_id = job.session_id.clone().unwrap_or_default();
    let cwd = job.cwd.clone().unwrap_or_default();
    let now = now_millis();
    let prompt = job.initial_prompt.clone().unwrap_or_default();
    WorkerRecord {
        pid,
        proc_start,
        session_id: session_id.clone(),
        rendezvous_sock: String::new(),
        pty_sock: None,
        messaging_sock: None,
        cli_version: Some(version.to_string()),
        started_at: now,
        attempt: 0,
        cwd: cwd.clone(),
        worktree_path: None,
        dispatch: Dispatch {
            proto: PROTO,
            short: short.to_string(),
            nonce: None,
            session_id,
            created_at: now,
            source: DispatchSource::Shell,
            cwd,
            launch: Launch::Prompt {
                args: vec!["--background".to_string(), prompt],
            },
            env: BTreeMap::new(),
            reattach_env: None,
            worktree: None,
            isolation: Isolation::None,
            respawn_flags: Vec::new(),
            attach_stall_respawns: None,
            agent: None,
            routine: None,
            seed: job
                .intent
                .clone()
                .map(|intent| Seed { intent, name: None }),
            cols: None,
            rows: None,
        },
        pending_respawn: None,
        dec_modes: None,
        rv_auth: None,
        pty_auth: None,
        extra: serde_json::Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_lock::{acquire, StaleReason};
    use crate::daemon_roster::{
        empty_roster, read_roster, roster_path, Dispatch, DispatchSource, Isolation, Launch,
        WorkerRecord, PROTO,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmpdir() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("lingxi-daemon-cmd-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Fake lock probe: a pid is a live daemon iff seeded.
    struct FakeLockProbe {
        alive_daemon: HashMap<i32, String>,
    }
    impl LockProbe for FakeLockProbe {
        fn is_alive(&self, pid: i32) -> bool {
            self.alive_daemon.contains_key(&pid)
        }
        fn is_daemon_process(&self, pid: i32) -> bool {
            self.alive_daemon.contains_key(&pid)
        }
        fn proc_start(&self, pid: i32, _skip: bool) -> Option<String> {
            self.alive_daemon.get(&pid).cloned()
        }
    }

    /// Fake proc probe for the roster reaper.
    struct FakeProc {
        alive: HashMap<i32, bool>,
        start: HashMap<i32, String>,
    }
    impl ProcProbe for FakeProc {
        fn is_alive(&self, pid: i32) -> bool {
            *self.alive.get(&pid).unwrap_or(&false)
        }
        fn start_time(&self, pid: i32) -> Option<String> {
            self.start.get(&pid).cloned()
        }
    }

    fn no_sleep() -> impl FnMut(u64) {
        |_| {}
    }

    /// Records the shorts it was asked to spawn and hands back monotonically
    /// increasing fake pids (starting at 90_000). No real process is launched.
    #[derive(Default)]
    struct FakeWorkerSpawner {
        spawned: Vec<String>,
        next_pid: i32,
    }
    impl WorkerSpawner for FakeWorkerSpawner {
        fn spawn_worker(&mut self, short: &str) -> std::io::Result<i32> {
            self.spawned.push(short.to_string());
            let pid = 90_000 + self.next_pid;
            self.next_pid += 1;
            Ok(pid)
        }
    }

    /// A spawner that must never be called (asserts no spawn happens).
    struct NeverSpawner;
    impl WorkerSpawner for NeverSpawner {
        fn spawn_worker(&mut self, short: &str) -> std::io::Result<i32> {
            panic!("unexpected worker spawn for {short}");
        }
    }

    /// Write a minimal pending `--bg` job (`state:"working"`) under `home`.
    fn seed_working_job(home: &Path, short: &str) {
        let respawn: Vec<String> = Vec::new();
        let job = agents_registry::JobStateWrite {
            state: "working",
            tempo: Some("active"),
            name: None,
            session_id: Some("11111111-1111-1111-1111-111111111111"),
            cwd: Some("/work"),
            origin_cwd: Some("/work"),
            created_at: Some("2026-07-04T00:00:00.000Z"),
            intent: Some("do the thing"),
            display_intent: None,
            template: Some("bg"),
            respawn_flags: &respawn,
            in_flight: None,
            backend: Some("daemon"),
            initial_prompt: Some("do the thing"),
            worker_pid: None,
        };
        agents_registry::write_job_state(home, short, &job).unwrap();
    }

    fn worker(pid: i32) -> WorkerRecord {
        WorkerRecord {
            pid,
            proc_start: None,
            session_id: "11111111-1111-1111-1111-111111111111".to_string(),
            rendezvous_sock: String::new(),
            pty_sock: None,
            messaging_sock: None,
            cli_version: Some("0.0.0".to_string()),
            started_at: 1_700_000_000_000,
            attempt: 0,
            cwd: "/work".to_string(),
            worktree_path: None,
            dispatch: Dispatch {
                proto: PROTO,
                short: "abcd1234".to_string(),
                nonce: None,
                session_id: "11111111-1111-1111-1111-111111111111".to_string(),
                created_at: 1_700_000_000_000,
                source: DispatchSource::Shell,
                cwd: "/work".to_string(),
                launch: Launch::Prompt {
                    args: vec!["hi".to_string()],
                },
                env: std::collections::BTreeMap::new(),
                reattach_env: None,
                worktree: None,
                isolation: Isolation::None,
                respawn_flags: Vec::new(),
                attach_stall_respawns: None,
                agent: None,
                routine: None,
                seed: None,
                cols: None,
                rows: None,
            },
            pending_respawn: None,
            dec_modes: None,
            rv_auth: None,
            pty_auth: None,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn fresh_acquire_adopts_seeds_roster_and_bumps_updated_at() {
        let dir = tmpdir();
        // Seed a roster with one LIVE worker (kept) + one DEAD worker (dropped).
        let mut roster = empty_roster(999);
        roster.updated_at = 1;
        roster.workers.insert("live0000".to_string(), worker(100));
        roster.workers.insert("dead0000".to_string(), worker(300));
        daemon_roster::write_roster(&dir, &roster).unwrap();

        let mut alive = HashMap::new();
        alive.insert(100, true); // live, adoptable
        let proc = FakeProc {
            alive,
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        // should_stop true immediately → 0 heartbeat-loop iterations; only the
        // INITIAL adopt runs (deterministic, no wall-clock loop).
        let code = run_supervisor(
            &dir,
            4242,
            "0.0.0",
            &lockp,
            &proc,
            &mut FakeWorkerSpawner::default(),
            HEARTBEAT_MS,
            &mut no_sleep(),
            &mut || true,
        );
        assert_eq!(code, exit_codes::SUCCESS);

        let got = read_roster(&dir, 0, false).into_roster();
        assert_eq!(got.supervisor_pid, 4242, "claimed by us");
        assert!(got.updated_at > 1, "updatedAt bumped");
        assert!(got.workers.contains_key("live0000"), "live worker retained");
        assert!(!got.workers.contains_key("dead0000"), "dead worker reaped");
        // Lock released on shutdown.
        assert!(!daemon_lock::lock_path(&dir).exists());
    }

    #[test]
    fn heartbeat_loop_runs_once_then_stops() {
        let dir = tmpdir();
        let proc = FakeProc {
            alive: HashMap::new(),
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        // false, false → one heartbeat; then true → break.
        let calls = AtomicUsize::new(0);
        let code = run_supervisor(
            &dir,
            7,
            "0.0.0",
            &lockp,
            &proc,
            &mut FakeWorkerSpawner::default(),
            HEARTBEAT_MS,
            &mut no_sleep(),
            &mut || calls.fetch_add(1, Ordering::Relaxed) >= 2,
        );
        assert_eq!(code, exit_codes::SUCCESS);
        // roster.json exists (both initial adopt + the one heartbeat wrote it).
        assert!(roster_path(&dir).exists());
        let got = read_roster(&dir, 0, false).into_roster();
        assert_eq!(got.supervisor_pid, 7);
    }

    #[test]
    fn yields_to_a_live_peer_without_writing_roster() {
        let dir = tmpdir();
        // A live peer already holds the lock.
        let mut peer = DaemonLock::new(5555, "0.0.0");
        peer.proc_start = Some("PEER-START".to_string());
        acquire(&dir, &peer).unwrap();

        let proc = FakeProc {
            alive: HashMap::new(),
            start: HashMap::new(),
        };
        let mut alive_daemon = HashMap::new();
        alive_daemon.insert(5555, "PEER-START".to_string());
        let lockp = FakeLockProbe { alive_daemon };

        let code = run_supervisor(
            &dir,
            4242,
            "0.0.0",
            &lockp,
            &proc,
            &mut FakeWorkerSpawner::default(),
            HEARTBEAT_MS,
            &mut no_sleep(),
            &mut || true,
        );
        assert_eq!(code, exit_codes::SUCCESS);
        // We did NOT steal the peer's lock, and never wrote a roster.
        assert_eq!(daemon_lock::read_lock(&dir).unwrap().pid, 5555);
        assert!(!roster_path(&dir).exists(), "yield writes no roster");
    }

    #[test]
    fn takes_over_a_stale_dead_lock() {
        let dir = tmpdir();
        // A DEAD holder's lock.
        let mut stale = DaemonLock::new(5555, "0.0.0");
        stale.proc_start = Some("OLD".to_string());
        acquire(&dir, &stale).unwrap();

        let proc = FakeProc {
            alive: HashMap::new(),
            start: HashMap::new(),
        };
        // 5555 not in alive_daemon → dead → stale → we take over.
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        let code = run_supervisor(
            &dir,
            4242,
            "0.0.0",
            &lockp,
            &proc,
            &mut FakeWorkerSpawner::default(),
            HEARTBEAT_MS,
            &mut no_sleep(),
            &mut || true,
        );
        assert_eq!(code, exit_codes::SUCCESS);
        // Took over the stale lock (then released it on shutdown).
        assert!(!daemon_lock::lock_path(&dir).exists());
        // Sanity: the takeover reason path is exercised (Dead).
        let _ = StaleReason::Dead;
    }

    // ---- worker spawn (Piece B) ---------------------------------------------

    #[test]
    fn one_pending_job_spawns_one_worker_and_records_pid() {
        let dir = tmpdir();
        seed_working_job(&dir, "bc7c6b33");

        let proc = FakeProc {
            alive: HashMap::new(),
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        let mut spawner = FakeWorkerSpawner::default();
        // should_stop true immediately → only the INITIAL adopt heartbeat runs.
        let code = run_supervisor(
            &dir,
            4242,
            "0.0.0",
            &lockp,
            &proc,
            &mut spawner,
            HEARTBEAT_MS,
            &mut no_sleep(),
            &mut || true,
        );
        assert_eq!(code, exit_codes::SUCCESS);

        // Exactly one spawn, for our job.
        assert_eq!(spawner.spawned, vec!["bc7c6b33".to_string()]);
        // The child pid was recorded into the durable job (state still working).
        let job = agents_registry::read_job(&dir, "bc7c6b33").unwrap();
        assert_eq!(job.state, "working");
        assert_eq!(job.worker_pid, Some(90_000));
        assert!(!agents_registry::job_is_terminal(&job));
        // …and a live-worker roster record exists carrying that pid.
        let roster = read_roster(&dir, 0, false).into_roster();
        let rec = roster.workers.get("bc7c6b33").expect("worker record");
        assert_eq!(rec.pid, 90_000);
    }

    #[test]
    fn terminal_job_is_not_spawned() {
        let dir = tmpdir();
        seed_working_job(&dir, "dddd4444");
        // Mark it done (terminal) before the supervisor runs.
        agents_registry::update_job_state(&dir, "dddd4444", "done", None).unwrap();

        let proc = FakeProc {
            alive: HashMap::new(),
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        // NeverSpawner panics if a spawn is attempted.
        let code = run_supervisor(
            &dir,
            4242,
            "0.0.0",
            &lockp,
            &proc,
            &mut NeverSpawner,
            HEARTBEAT_MS,
            &mut no_sleep(),
            &mut || true,
        );
        assert_eq!(code, exit_codes::SUCCESS);
    }

    #[test]
    fn job_with_live_worker_pid_is_not_respawned() {
        let dir = tmpdir();
        seed_working_job(&dir, "eeee5555");
        // Record a still-live worker pid on the job.
        agents_registry::update_job_state(&dir, "eeee5555", "working", Some(4321)).unwrap();

        let mut alive = HashMap::new();
        alive.insert(4321, true); // the recorded worker is alive
        let proc = FakeProc {
            alive,
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        // NeverSpawner: the live-worker guard must prevent a re-spawn.
        let code = run_supervisor(
            &dir,
            4242,
            "0.0.0",
            &lockp,
            &proc,
            &mut NeverSpawner,
            HEARTBEAT_MS,
            &mut no_sleep(),
            &mut || true,
        );
        assert_eq!(code, exit_codes::SUCCESS);
    }

    #[test]
    fn claimed_job_is_not_double_spawned_across_heartbeats() {
        let dir = tmpdir();
        seed_working_job(&dir, "ffff6666");

        // The fake worker pid (90_000) is NOT reported alive by the proc probe,
        // so ONLY the in-memory `claimed` guard can prevent the second spawn.
        let proc = FakeProc {
            alive: HashMap::new(),
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        let mut spawner = FakeWorkerSpawner::default();
        // false, false → one loop heartbeat; then true → break. Plus the
        // initial adopt heartbeat = TWO heartbeats total over the same job.
        let calls = AtomicUsize::new(0);
        let code = run_supervisor(
            &dir,
            4242,
            "0.0.0",
            &lockp,
            &proc,
            &mut spawner,
            HEARTBEAT_MS,
            &mut no_sleep(),
            &mut || calls.fetch_add(1, Ordering::Relaxed) >= 2,
        );
        assert_eq!(code, exit_codes::SUCCESS);
        // Despite two heartbeats, the job was spawned exactly once.
        assert_eq!(spawner.spawned, vec!["ffff6666".to_string()]);
    }

    // ---- crashed-worker recovery (FIX 2) ------------------------------------

    #[test]
    fn job_whose_worker_died_is_marked_failed_not_respawned() {
        let dir = tmpdir();
        seed_working_job(&dir, "cafe0001");
        // Record a worker pid on the job, then let the worker "die": the proc
        // probe reports it NOT alive and it never wrote a terminal state.
        agents_registry::update_job_state(&dir, "cafe0001", "working", Some(4321)).unwrap();

        let proc = FakeProc {
            alive: HashMap::new(), // 4321 is NOT alive → crashed
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        // NeverSpawner: a crashed job must be failed, NOT respawned.
        let code = run_supervisor(
            &dir,
            4242,
            "0.0.0",
            &lockp,
            &proc,
            &mut NeverSpawner,
            HEARTBEAT_MS,
            &mut no_sleep(),
            &mut || true,
        );
        assert_eq!(code, exit_codes::SUCCESS);

        // The stuck job was moved to a terminal `failed` state (worker pid cleared).
        let job = agents_registry::read_job(&dir, "cafe0001").unwrap();
        assert_eq!(job.state, "failed", "crashed worker → job marked failed");
        assert!(
            agents_registry::job_is_terminal(&job),
            "failed job is terminal (won't re-render as working)"
        );
        assert_eq!(job.worker_pid, None, "stale worker pid cleared");
    }

    #[test]
    fn job_with_alive_worker_is_left_untouched_not_failed() {
        let dir = tmpdir();
        seed_working_job(&dir, "cafe0002");
        agents_registry::update_job_state(&dir, "cafe0002", "working", Some(7777)).unwrap();

        let mut alive = HashMap::new();
        alive.insert(7777, true); // the worker is still running
        let proc = FakeProc {
            alive,
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        // NeverSpawner: a live worker must be neither respawned nor failed.
        let code = run_supervisor(
            &dir,
            4242,
            "0.0.0",
            &lockp,
            &proc,
            &mut NeverSpawner,
            HEARTBEAT_MS,
            &mut no_sleep(),
            &mut || true,
        );
        assert_eq!(code, exit_codes::SUCCESS);

        // Left exactly as-is: still working, pid intact, not terminal.
        let job = agents_registry::read_job(&dir, "cafe0002").unwrap();
        assert_eq!(job.state, "working", "live worker's job untouched");
        assert_eq!(job.worker_pid, Some(7777), "live worker pid preserved");
        assert!(!agents_registry::job_is_terminal(&job));
    }
}
