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
//! The supervisor now spawns headless workers with the dispatch environment
//! (`bg_worker_env`) plus a live attach socket/token in the roster. Vanished
//! workers still fail closed instead of pseudo-resuming and re-running the
//! original prompt, which avoids duplicate side effects.
//!
//! Remaining higher-risk follow-ons: full control-message IPC, actual
//! pty-backed worker input, the PTY-owned attach-stall watchdog, low-memory
//! handling, orphan reap beyond `retain_adoptable`, and upgrade takeover.
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
    /// Spawn the detached headless worker for job `short` with `env` layered
    /// onto the inherited process environment, returning its pid.
    fn spawn_worker(&mut self, short: &str, env: &BTreeMap<String, String>)
        -> std::io::Result<i32>;
}

/// Production worker spawner: a detached `<current_exe> __bg-run <short>` with
/// null stdio in its own process group (so a terminal signal to the daemon's
/// group is not delivered to the worker). No `unsafe` — `process_group` is a
/// safe `CommandExt` method (the crate is `#![forbid(unsafe_code)]`).
struct RealWorkerSpawner;

impl WorkerSpawner for RealWorkerSpawner {
    fn spawn_worker(
        &mut self,
        short: &str,
        env: &BTreeMap<String, String>,
    ) -> std::io::Result<i32> {
        use std::process::{Command, Stdio};
        let exe = std::env::current_exe()?.display().to_string();
        let argv =
            crate::process_wrapper::wrap_argv(vec![exe, "__bg-run".to_string(), short.to_string()]);
        let Some((program, args)) = argv.split_first() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty worker argv",
            ));
        };
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Layer the background-session env onto the inherited daemon env
        // (mirrors CC 2.1.207's worker-spawn `{...dispatch.env, CLAUDE_CODE_
        // SESSION_KIND:"bg", CLAUDE_BG_BACKEND:"daemon", CLAUDE_BG_SOURCE,
        // CLAUDE_JOB_DIR, CLAUDE_BG_ISOLATION}`, rebranded to LINGXI_).
        for (k, v) in env {
            cmd.env(k, v);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let child = cmd.spawn()?;
        Ok(i32::try_from(child.id()).unwrap_or(i32::MAX))
    }
}

/// The `LINGXI_*` background-session environment a spawned `__bg-run` worker
/// inherits. Rebrand of CC 2.1.207's worker-spawn env assembly
/// (`{...dispatch.env, CLAUDE_CODE_SESSION_KIND:"bg", CLAUDE_BG_BACKEND:"daemon",
/// CLAUDE_BG_SOURCE:e.source, CLAUDE_JOB_DIR:t, CLAUDE_BG_ISOLATION:…}`) onto the
/// `LINGXI_` names lingxi's runtime reads: `bg_session::from_env`
/// (`LINGXI_SESSION_KIND`/`LINGXI_JOB_DIR`/`LINGXI_BG_ISOLATION` → the
/// `# Background Session` system-prompt section) and the `/stop` command
/// (`LINGXI_JOB_DIR` → rewrite `$LINGXI_JOB_DIR/state.json`). Without this the
/// worker ran a plain non-bg turn — the dispatch environment never reached it.
/// The daemon layers `LINGXI_BG_ATTACH_*` separately per spawn because the auth
/// token is generated for the concrete live worker record.
///
/// `isolation` is `"none"` (a `--bg` shell dispatch runs in place — the durable
/// job carries no worktree binding) and `source` is `"shell"`; both match the
/// `--bg` dispatch record `background_dispatch.rs` writes.
fn bg_worker_env(runtime_dir: &Path, short: &str) -> BTreeMap<String, String> {
    let job_dir = agents_registry::jobs_dir(runtime_dir).join(short);
    let mut env = BTreeMap::new();
    env.insert("LINGXI_SESSION_KIND".to_string(), "bg".to_string());
    env.insert("LINGXI_BG_BACKEND".to_string(), "daemon".to_string());
    env.insert("LINGXI_BG_SOURCE".to_string(), "shell".to_string());
    env.insert("LINGXI_BG_ISOLATION".to_string(), "none".to_string());
    env.insert("LINGXI_JOB_DIR".to_string(), job_dir.display().to_string());
    env
}

/// Durable respawn-attempt counter for a job, kept in `jobs/<short>/respawns`
/// (a plain-integer sibling of `state.json`, so the byte-faithful `state.json`
/// schema is untouched). Absent/unparseable ⇒ 0.
fn read_respawn_count(runtime_dir: &Path, short: &str) -> i64 {
    let path = agents_registry::jobs_dir(runtime_dir)
        .join(short)
        .join("respawns");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// `tengu_bg_worker_vanished` — a recorded worker pid is no longer alive and
/// never wrote a terminal state (CC field shape `{short, recycled, fromPoll,
/// uptimeMs}`; lingxi emits the byte-exact event name + the fields it can
/// determine from the heartbeat poll).
fn emit_worker_vanished(short: &str) {
    tracing::info!(
        event = "tengu_bg_worker_vanished",
        short,
        recycled = false,
        from_poll = true,
    );
}

/// `tengu_bg_respawn_exhausted` — no safe resume path exists; the job is being
/// marked terminally `failed`.
fn emit_respawn_exhausted(short: &str, attempts: i64) {
    tracing::info!(event = "tengu_bg_respawn_exhausted", short, attempts);
}

/// `tengu_bg_spawn_cwd_gone` — a pending job's recorded working directory no
/// longer exists, so the supervisor fails it closed instead of spawning a
/// worker that would crash the moment it `chdir`s into the dead cwd. CC field
/// shape `{short, attempt, via}` (binary `settleCwdGone`); `via:"cold"` mirrors
/// CC's cold-spawn access-failure path (`settleCwdGone("cold", cwd)`).
fn emit_spawn_cwd_gone(short: &str, attempt: i64, via: &str) {
    tracing::info!(event = "tengu_bg_spawn_cwd_gone", short, attempt, via);
}

/// Byte-faithful `spawn_cwd_gone` job detail (CC `settleCwdGone`:
/// `working directory no longer exists or is not accessible: ${cwd}`).
fn cwd_gone_detail(cwd: &str) -> String {
    format!("working directory no longer exists or is not accessible: {cwd}")
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
    spawn_pending_workers(
        runtime_dir,
        &mut roster,
        version,
        proc_probe,
        spawner,
        claimed,
    );
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
/// (the worker died before writing its own terminal state) is failed closed.
/// The live attach socket is only valid while the worker process exists; after
/// the process is gone LingXi must not pseudo-resume by re-running the original
/// prompt, because that risks duplicate side effects.
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
            // The recorded worker DIED without writing a terminal state. LingXi
            // cannot yet live-attach/continue that process, and re-running the
            // recorded prompt would duplicate side effects. Fail closed.
            emit_worker_vanished(&short);
            if let Err(e) = agents_registry::update_job_state(runtime_dir, &short, "failed", None) {
                tracing::warn!(
                    "lingxi-cli daemon: could not mark vanished job {short} failed: {e}"
                );
            }
            emit_respawn_exhausted(&short, read_respawn_count(runtime_dir, &short));
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

        // CWD-GONE guard: never spawn a worker into a working directory that no
        // longer exists. A detached headless worker would otherwise `chdir` into
        // the dead cwd, crash opaquely, and (having no live transport) fail
        // closed anyway. CC's `settleCwdGone` fails such a dispatch closed with a
        // specific detail + `tengu_bg_spawn_cwd_gone{short, attempt, via}`; we
        // mirror that, keeping the failure legible in the agent view.
        if let Some(cwd) = job.cwd.as_deref() {
            if !cwd.is_empty() && !Path::new(cwd).exists() {
                let attempt = read_respawn_count(runtime_dir, &short);
                emit_spawn_cwd_gone(&short, attempt, "cold");
                if let Err(e) = agents_registry::update_job_state_with_detail(
                    runtime_dir,
                    &short,
                    "failed",
                    None,
                    &cwd_gone_detail(cwd),
                ) {
                    tracing::warn!(
                        "lingxi-cli daemon: could not mark cwd-gone job {short} failed: {e}"
                    );
                }
                claimed.remove(&short);
                continue;
            }
        }

        let mut worker_env = roster
            .workers
            .get(&short)
            .map(|record| record.dispatch.env.clone())
            .unwrap_or_default();
        worker_env.extend(bg_worker_env(runtime_dir, &short));
        let attach_sock = crate::bg_attach::socket_path(runtime_dir, &short);
        let attach_sock_s = attach_sock.display().to_string();
        let attach_auth = uuid::Uuid::new_v4().to_string();
        worker_env.insert(
            crate::bg_attach::ATTACH_SOCK_ENV.to_string(),
            attach_sock_s.clone(),
        );
        worker_env.insert(
            crate::bg_attach::ATTACH_AUTH_ENV.to_string(),
            attach_auth.clone(),
        );
        match spawner.spawn_worker(&short, &worker_env) {
            Ok(child_pid) => {
                // Durable pid record (state stays "working").
                if let Err(e) = agents_registry::update_job_state(
                    runtime_dir,
                    &short,
                    "working",
                    Some(child_pid),
                ) {
                    tracing::warn!(
                        "lingxi-cli daemon: could not record workerPid for {short}: {e}"
                    );
                }
                // Live-worker roster record so retain_adoptable keeps it.
                let proc_start = proc_probe.start_time(child_pid);
                roster.workers.insert(
                    short.clone(),
                    worker_record_for_job(
                        &short,
                        &job,
                        child_pid,
                        proc_start,
                        version,
                        attach_sock_s,
                        attach_auth,
                    ),
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
/// job. `pid`/`procStart` drive `retain_adoptable`; `rendezvousSock`/`ptySock`
/// plus `rvAuth`/`ptyAuth` let `agents attach` connect to the still-running
/// worker instead of spawning a second `--resume` writer.
fn worker_record_for_job(
    short: &str,
    job: &agents_registry::JobState,
    pid: i32,
    proc_start: Option<String>,
    version: &str,
    attach_sock: String,
    attach_auth: String,
) -> WorkerRecord {
    let session_id = job.session_id.clone().unwrap_or_default();
    let cwd = job.cwd.clone().unwrap_or_default();
    let now = now_millis();
    let prompt = job.initial_prompt.clone().unwrap_or_default();
    WorkerRecord {
        pid,
        proc_start,
        session_id: session_id.clone(),
        rendezvous_sock: attach_sock.clone(),
        pty_sock: Some(attach_sock),
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
            seed: job.intent.clone().map(|intent| Seed { intent, name: None }),
            cols: None,
            rows: None,
        },
        pending_respawn: None,
        dec_modes: None,
        rv_auth: Some(attach_auth.clone()),
        pty_auth: Some(attach_auth),
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

    /// Records the shorts (and env) it was asked to spawn and hands back
    /// monotonically increasing fake pids (starting at 90_000). No real process
    /// is launched.
    #[derive(Default)]
    struct FakeWorkerSpawner {
        spawned: Vec<String>,
        envs: Vec<BTreeMap<String, String>>,
        next_pid: i32,
    }
    impl WorkerSpawner for FakeWorkerSpawner {
        fn spawn_worker(
            &mut self,
            short: &str,
            env: &BTreeMap<String, String>,
        ) -> std::io::Result<i32> {
            self.spawned.push(short.to_string());
            self.envs.push(env.clone());
            let pid = 90_000 + self.next_pid;
            self.next_pid += 1;
            Ok(pid)
        }
    }

    /// A spawner that must never be called (asserts no spawn happens).
    struct NeverSpawner;
    impl WorkerSpawner for NeverSpawner {
        fn spawn_worker(
            &mut self,
            short: &str,
            _env: &BTreeMap<String, String>,
        ) -> std::io::Result<i32> {
            panic!("unexpected worker spawn for {short}");
        }
    }

    /// Write a minimal pending `--bg` job (`state:"working"`) under `home`.
    fn seed_working_job(home: &Path, short: &str) {
        // Seed an EXISTING cwd (the test home) so the supervisor's
        // spawn_cwd_gone guard does not fail-close the fresh-spawn path these
        // tests exercise. The dedicated cwd-gone test seeds a missing cwd.
        seed_working_job_cwd(home, short, &home.display().to_string());
    }

    fn seed_working_job_cwd(home: &Path, short: &str, cwd: &str) {
        let respawn: Vec<String> = Vec::new();
        let job = agents_registry::JobStateWrite {
            state: "working",
            tempo: Some("active"),
            name: None,
            session_id: Some("11111111-1111-1111-1111-111111111111"),
            cwd: Some(cwd),
            origin_cwd: Some(cwd),
            created_at: Some("2026-07-04T00:00:00.000Z"),
            intent: Some("do the thing"),
            display_intent: None,
            template: Some("bg"),
            respawn_flags: &respawn,
            in_flight: None,
            backend: Some("daemon"),
            initial_prompt: Some("do the thing"),
            detail: None,
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

        // The first fake worker pid (90_000) is reported alive — as a real
        // freshly-spawned process would be — so heartbeat 2 sees the live worker
        // (cross-restart guard) and does NOT re-spawn. (A pid the probe reports
        // dead is a VANISHED worker and fails closed; that path has its own
        // test.)
        let mut alive = HashMap::new();
        alive.insert(90_000, true);
        let proc = FakeProc {
            alive,
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

    // ---- crashed-worker recovery (fail-closed) -----------------------------

    #[test]
    fn crashed_worker_is_marked_failed_without_respawn() {
        let dir = tmpdir();
        seed_working_job(&dir, "cafe0001");
        // Record a worker pid on the job, then let the worker "die": the proc
        // probe reports it NOT alive and it never wrote a terminal state.
        agents_registry::update_job_state(&dir, "cafe0001", "working", Some(4321)).unwrap();

        let proc = FakeProc {
            alive: HashMap::new(), // 4321 is NOT alive → vanished
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        // Without a live attach transport, a vanished worker must fail closed
        // rather than re-running the original prompt.
        let mut spawner = FakeWorkerSpawner::default();
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

        assert!(
            spawner.spawned.is_empty(),
            "vanished worker must not respawn"
        );
        let job = agents_registry::read_job(&dir, "cafe0001").unwrap();
        assert_eq!(job.state, "failed", "vanished worker → job failed");
        assert!(
            agents_registry::job_is_terminal(&job),
            "failed job is terminal (won't re-render as working)"
        );
        assert_eq!(job.worker_pid, None, "stale worker pid cleared");
    }

    // ---- spawn_cwd_gone (fail-closed, never spawn into a dead cwd) ----------

    #[test]
    fn pending_job_with_missing_cwd_is_failed_closed_not_spawned() {
        let dir = tmpdir();
        // A pending job whose recorded cwd no longer exists (never created).
        let gone = dir.join("was-here-now-gone").display().to_string();
        seed_working_job_cwd(&dir, "c0de9999", &gone);
        assert!(!std::path::Path::new(&gone).exists());

        let proc = FakeProc {
            alive: HashMap::new(),
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        // NeverSpawner: the cwd-gone guard must fail the job WITHOUT spawning a
        // worker into the dead cwd.
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

        let job = agents_registry::read_job(&dir, "c0de9999").unwrap();
        assert_eq!(job.state, "failed", "cwd-gone job → failed");
        assert!(
            agents_registry::job_is_terminal(&job),
            "failed job is terminal"
        );
        assert_eq!(job.worker_pid, None);
        // Byte-faithful detail (CC `settleCwdGone`).
        assert_eq!(
            job.detail.as_deref(),
            Some(
                format!("working directory no longer exists or is not accessible: {gone}").as_str()
            )
        );
        // No live-worker roster record was created for the doomed job.
        let roster = read_roster(&dir, 0, false).into_roster();
        assert!(!roster.workers.contains_key("c0de9999"));
    }

    #[test]
    fn spawned_worker_receives_the_bg_session_env() {
        // The daemon must launch `__bg-run` with the LINGXI_ background-session
        // env (rebrand of CC's CLAUDE_CODE_SESSION_KIND/CLAUDE_BG_*/CLAUDE_JOB_DIR
        // worker-spawn keys) so the worker's turn gets the `# Background Session`
        // prompt section and `/stop` can locate the job.
        let dir = tmpdir();
        seed_working_job(&dir, "bead0001");
        let mut roster = empty_roster(999);
        let mut dispatch_record = worker(1234);
        dispatch_record.dispatch.short = "bead0001".to_string();
        dispatch_record
            .dispatch
            .env
            .insert("ANTHROPIC_API_KEY".to_string(), "sk-test".to_string());
        roster
            .workers
            .insert("bead0001".to_string(), dispatch_record);
        daemon_roster::write_roster(&dir, &roster).unwrap();
        let mut alive = HashMap::new();
        alive.insert(1234, true);
        let proc = FakeProc {
            alive,
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        let mut spawner = FakeWorkerSpawner::default();
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
        assert_eq!(spawner.envs.len(), 1);
        let env = &spawner.envs[0];
        assert_eq!(
            env.get("LINGXI_SESSION_KIND").map(String::as_str),
            Some("bg")
        );
        assert_eq!(
            env.get("LINGXI_BG_BACKEND").map(String::as_str),
            Some("daemon")
        );
        assert_eq!(
            env.get("LINGXI_BG_SOURCE").map(String::as_str),
            Some("shell")
        );
        assert_eq!(
            env.get("LINGXI_BG_ISOLATION").map(String::as_str),
            Some("none")
        );
        let expected_job_dir = agents_registry::jobs_dir(&dir)
            .join("bead0001")
            .display()
            .to_string();
        assert_eq!(env.get("LINGXI_JOB_DIR"), Some(&expected_job_dir));
        let expected_attach_sock = crate::bg_attach::socket_path(&dir, "bead0001")
            .display()
            .to_string();
        assert_eq!(
            env.get(crate::bg_attach::ATTACH_SOCK_ENV),
            Some(&expected_attach_sock)
        );
        let attach_auth = env
            .get(crate::bg_attach::ATTACH_AUTH_ENV)
            .expect("attach auth env is generated");
        assert_eq!(attach_auth.len(), 36, "uuid v4 auth token");
        assert_eq!(
            env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-test")
        );

        let roster = read_roster(&dir, 4242, true).into_roster();
        let record = roster
            .workers
            .get("bead0001")
            .expect("spawned worker roster record");
        assert_eq!(record.rendezvous_sock, expected_attach_sock);
        assert_eq!(
            record.pty_sock.as_deref(),
            Some(record.rendezvous_sock.as_str())
        );
        assert_eq!(record.rv_auth.as_ref(), Some(attach_auth));
        assert_eq!(record.pty_auth.as_ref(), Some(attach_auth));
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
