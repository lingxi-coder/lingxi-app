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

use crate::daemon_lock::{self, DaemonLock, LockProbe, SystemLockProbe};
use crate::daemon_roster::{self, ProcProbe, SystemProbe};
use crate::exit_codes;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Heartbeat cadence for the supervise loop (grounded engineering choice, NOT
/// byte-parity).
const HEARTBEAT_MS: u64 = 2000;

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

    let stop_for_loop = stop.clone();
    let result = tokio::task::spawn_blocking(move || {
        run_supervisor(
            &runtime_dir,
            pid,
            version,
            &SystemLockProbe,
            &SystemProbe,
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

/// The testable supervisor core: `sleep` and `should_stop` are injected so the
/// loop runs with no real time and a deterministic shutdown. `lock_probe` drives
/// [`daemon_lock::acquire_or_yield`]; `proc_probe` drives
/// [`daemon_roster::retain_adoptable`].
#[allow(clippy::too_many_arguments)]
fn run_supervisor<LP: LockProbe, PP: ProcProbe>(
    runtime_dir: &Path,
    pid: i32,
    version: &str,
    lock_probe: &LP,
    proc_probe: &PP,
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

    // 3. INITIAL ADOPT — read the roster, reap non-adoptable workers, claim it.
    heartbeat(runtime_dir, pid, proc_probe);

    // 4. SUPERVISE LOOP — re-read + reap + re-persist updatedAt each heartbeat.
    //    The roster file is the CLI→supervisor dispatch channel; picking up new
    //    WorkerRecord/Dispatch rows = re-reading + retaining them here.
    loop {
        if should_stop() {
            break;
        }
        sleep(heartbeat_ms);
        if should_stop() {
            break;
        }
        heartbeat(runtime_dir, pid, proc_probe);
    }

    // 5. SHUTDOWN — release our lock (ENOENT-swallowing).
    let _ = daemon_lock::release(runtime_dir);
    exit_codes::SUCCESS
}

/// One roster sweep: re-read, drop dead/recycled workers, stamp our
/// `supervisorPid` + a fresh `updatedAt`, and re-persist.
fn heartbeat<PP: ProcProbe>(runtime_dir: &Path, pid: i32, proc_probe: &PP) {
    let mut roster = daemon_roster::read_roster(runtime_dir, pid, true).into_roster();
    let _dropped = daemon_roster::retain_adoptable(&mut roster, proc_probe);
    roster.supervisor_pid = pid;
    roster.updated_at = now_millis();
    let _ = daemon_roster::write_roster(runtime_dir, &roster);
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
}
