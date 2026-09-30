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
//! 3. SUPERVISE LOOP — re-read `roster.json` plus durable job state, spawn one
//!    detached PTY supervisor for each unclaimed background dispatch, reap
//!    dead/recycled identities, and re-persist `updatedAt` every
//!    [`HEARTBEAT_MS`].
//! 4. SHUTDOWN — on SIGTERM/SIGINT, break the loop and [`daemon_lock::release`]
//!    our lock.
//!
//! Each worker owns a real PTY/ConPTY TUI child and protocol-v2 live attach
//! endpoint. Vanished workers fail closed instead of pseudo-resuming and
//! re-running the original prompt, which avoids duplicate side effects.
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
const STALL_TERMINATION_POLL_MS: u64 = 50;
const STALL_TERMINATION_POLL_ATTEMPTS: usize = 20;
const CLAIM_RECOVERY_EXHAUSTED_REASON: &str = "background recovery budget exhausted";

/// Spawns a detached `__bg-run <short>` worker process. Abstracted (like
/// [`crate::background_dispatch::DaemonSpawner`]) so the supervise loop's
/// spawn decisions are testable without launching a real process.
pub trait WorkerSpawner {
    /// Spawn the detached PTY supervisor for job `short` with `env` layered
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
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
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
fn bg_worker_env(
    runtime_dir: &Path,
    short: &str,
    generation: &str,
    claim_token: &str,
) -> BTreeMap<String, String> {
    let job_dir = agents_registry::jobs_dir(runtime_dir).join(short);
    let mut env = BTreeMap::new();
    env.insert("LINGXI_SESSION_KIND".to_string(), "bg".to_string());
    env.insert("LINGXI_BG_BACKEND".to_string(), "daemon".to_string());
    env.insert("LINGXI_BG_SOURCE".to_string(), "shell".to_string());
    env.insert("LINGXI_BG_ISOLATION".to_string(), "none".to_string());
    env.insert("LINGXI_JOB_DIR".to_string(), job_dir.display().to_string());
    env.insert(
        crate::commands::respawn::BG_WORKER_GENERATION_ENV.to_string(),
        generation.to_string(),
    );
    env.insert(
        crate::commands::respawn::BG_WORKER_CLAIM_TOKEN_ENV.to_string(),
        claim_token.to_string(),
    );
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

/// `tengu_bg_reply_undelivered` — a follow-up reply the attach fallback
/// persisted to the durable offline queue can never be delivered as a turn: its
/// worker vanished and is failed closed (never respawned), so the queue's only
/// in-worker drain site (`bg_worker::execute_job` at spawn) will never run for
/// this job again. Emitted once per stranded reply so the user's input is
/// recorded, not silently lost.
fn emit_reply_undelivered(short: &str, text: &str) {
    tracing::warn!(
        event = "tengu_bg_reply_undelivered",
        short,
        "undelivered queued reply: {text}"
    );
}

/// Drain the durable offline reply queue for a job whose worker vanished and is
/// about to be failed closed, surfacing any stranded follow-up replies.
///
/// The daemon fails a vanished worker CLOSED and never respawns it, so the
/// queue's only in-worker drain site (`bg_worker::execute_job` at (re)spawn)
/// will never run for this job again — a reply the attach fallback persisted
/// mid-flight would otherwise sit on disk forever, undelivered (the exact
/// silent-loss harm the queue exists to prevent). Each reply is logged
/// (`tengu_bg_reply_undelivered`) and folded into a bounded, user-visible detail
/// line stamped on the failed job; draining CLAIMS the files (at-most-once) so a
/// reply is reported exactly once. Returns the detail line, or `None` when the
/// queue was empty (the caller then fails the job with no detail).
fn drain_undelivered_replies(runtime_dir: &Path, short: &str) -> Option<String> {
    let replies = crate::bg_reply_queue::drain_replies(runtime_dir, short);
    if replies.is_empty() {
        return None;
    }
    for reply in &replies {
        emit_reply_undelivered(short, &reply.text);
    }
    let mut joined = replies
        .iter()
        .map(|r| r.text.trim())
        .collect::<Vec<_>>()
        .join(" | ");
    // Bound the detail so a very long queued line cannot bloat state.json.
    const MAX: usize = 500;
    if joined.chars().count() > MAX {
        joined = joined.chars().take(MAX).collect::<String>() + "…";
    }
    Some(format!(
        "worker exited with {} undelivered queued repl{}: {joined}",
        replies.len(),
        if replies.len() == 1 { "y" } else { "ies" },
    ))
}

/// Fail the exact vanished owner before consuming its offline reply queue.
///
/// `queue_resume_if_matches` may already have failed the claimed row when its
/// launch context is unusable. In either case, establish a terminal row first
/// so a concurrent replacement cannot take ownership between draining the
/// queue and surfacing its text. Exact CAS prevents a newer generation from
/// being failed or having its replies stolen.
fn fail_vanished_job_and_surface_replies(
    runtime_dir: &Path,
    short: &str,
    observed: &agents_registry::JobState,
) {
    let Some(mut current) = agents_registry::read_job(runtime_dir, short) else {
        return;
    };
    if current.phase.as_deref() == Some(crate::commands::respawn::PHASE_DELETING) {
        return;
    }
    if current.state != "failed" {
        let failed = agents_registry::patch_job_state_if_matches(
            runtime_dir,
            short,
            agents_registry::JobStateMatch {
                state: &observed.state,
                phase: observed.phase.as_deref(),
                worker_pid: observed.worker_pid,
                worker_proc_start: observed.worker_proc_start.as_deref(),
                worker_generation: observed.worker_generation.as_deref(),
                claim_token: observed.claim_token.as_deref(),
                claim_owner: observed.claim_owner.as_deref(),
                claim_created_at: observed.claim_created_at,
                claim_lease_ms: observed.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                state: Some("failed"),
                worker_pid: Some(None),
                worker_proc_start: Some(None),
                phase: Some(None),
                worker_generation: Some(None),
                claim_token: Some(None),
                claim_owner: Some(None),
                claim_created_at: Some(None),
                claim_lease_ms: Some(None),
                ..Default::default()
            },
        );
        match failed {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                tracing::warn!("lingxi-cli daemon: could not fail vanished job {short}: {error}");
                return;
            }
        }
        let Some(updated) = agents_registry::read_job(runtime_dir, short) else {
            return;
        };
        current = updated;
    }
    if current.state != "failed"
        || current.phase.as_deref() == Some(crate::commands::respawn::PHASE_DELETING)
    {
        return;
    }
    let Some(undelivered) = drain_undelivered_replies(runtime_dir, short) else {
        return;
    };

    for _ in 0..3 {
        let Some(failed) = agents_registry::read_job(runtime_dir, short) else {
            return;
        };
        if failed.state != "failed"
            || failed.phase.as_deref() == Some(crate::commands::respawn::PHASE_DELETING)
        {
            return;
        }
        let detail = match failed.detail.as_deref() {
            Some(existing) if existing.contains(&undelivered) => existing.to_string(),
            Some(existing) if !existing.is_empty() => format!("{existing}; {undelivered}"),
            _ => undelivered.clone(),
        };
        match agents_registry::patch_job_state_if_matches(
            runtime_dir,
            short,
            agents_registry::JobStateMatch {
                state: &failed.state,
                phase: failed.phase.as_deref(),
                worker_pid: failed.worker_pid,
                worker_proc_start: failed.worker_proc_start.as_deref(),
                worker_generation: failed.worker_generation.as_deref(),
                claim_token: failed.claim_token.as_deref(),
                claim_owner: failed.claim_owner.as_deref(),
                claim_created_at: failed.claim_created_at,
                claim_lease_ms: failed.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                detail: Some(Some(&detail)),
                ..Default::default()
            },
        ) {
            Ok(true) => return,
            Ok(false) => continue,
            Err(error) => {
                tracing::warn!(
                    "lingxi-cli daemon: could not surface undelivered replies for {short}: {error}"
                );
                return;
            }
        }
    }
    tracing::warn!(
        "lingxi-cli daemon: job {short} kept changing while undelivered replies were surfaced"
    );
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
    let loop_runtime_dir = runtime_dir.clone();
    let result = tokio::task::spawn_blocking(move || {
        run_supervisor(
            &loop_runtime_dir,
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
    let code = result.unwrap_or(exit_codes::RUNTIME_ERROR);
    if stop.load(Ordering::Relaxed) {
        stop_all_workers(&runtime_dir);
    }
    code
}

/// Graceful daemon shutdown owns the workers it supervised. Ask each worker to
/// unwind first (so its PTY handle closes normally), then use the persisted PTY
/// identity as a process-tree fallback.
fn stop_all_workers(runtime_dir: &Path) {
    for (short, job) in agents_registry::read_jobs(&agents_registry::jobs_dir(runtime_dir)) {
        if agents_registry::job_is_terminal(&job) {
            continue;
        }
        let _ = stop_background_job(runtime_dir, &short, &job);
    }
}

fn verified_live_worker_identity<PP: ProcProbe>(
    record: &WorkerRecord,
    probe: &PP,
) -> Option<ObservedProcessIdentity> {
    let expected = record.proc_start.as_deref()?;
    if !probe.is_alive(record.pid) {
        return None;
    }
    (probe.start_time(record.pid).as_deref() == Some(expected)).then(|| ObservedProcessIdentity {
        pid: record.pid,
        proc_start: Some(expected.to_string()),
    })
}

fn job_live_identity(job: &agents_registry::JobState) -> Option<ObservedProcessIdentity> {
    job.worker_pid.map(|pid| ObservedProcessIdentity {
        pid,
        proc_start: job.worker_proc_start.clone(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrongWorkerIdentity {
    LiveVerified,
    LiveUnverified,
    GoneOrRecycled,
}

fn strong_worker_identity<PP: ProcProbe>(
    record: &WorkerRecord,
    probe: &PP,
) -> StrongWorkerIdentity {
    strong_observed_identity(&ObservedProcessIdentity::from_worker(record), probe)
}

fn strong_observed_identity<PP: ProcProbe>(
    identity: &ObservedProcessIdentity,
    probe: &PP,
) -> StrongWorkerIdentity {
    if identity.pid <= 1 || !probe.is_alive(identity.pid) {
        return StrongWorkerIdentity::GoneOrRecycled;
    }
    match (
        identity.proc_start.as_deref(),
        probe.start_time(identity.pid),
    ) {
        (Some(expected), Some(actual)) if actual == expected => StrongWorkerIdentity::LiveVerified,
        (Some(expected), Some(actual)) if actual != expected => {
            StrongWorkerIdentity::GoneOrRecycled
        }
        _ => StrongWorkerIdentity::LiveUnverified,
    }
}

fn worker_record_matches_job(record: &WorkerRecord, job: &agents_registry::JobState) -> bool {
    if job
        .session_id
        .as_deref()
        .is_some_and(|session_id| session_id != record.session_id)
    {
        return false;
    }
    let record_generation = record
        .dispatch
        .env
        .get(crate::commands::respawn::BG_WORKER_GENERATION_ENV)
        .map(String::as_str);
    if job.worker_generation.is_some() || record_generation.is_some() {
        return job.worker_generation.as_deref() == record_generation;
    }
    if job.session_id.as_deref() == Some(record.session_id.as_str()) {
        return true;
    }
    job.worker_pid == Some(record.pid)
        && job.worker_proc_start.is_some()
        && job.worker_proc_start == record.proc_start
}

fn roster_worker_publication_matches_disk(
    runtime_dir: &Path,
    short: &str,
    expected: &WorkerRecord,
) -> bool {
    daemon_roster::read_roster(runtime_dir, 0, false)
        .into_roster()
        .workers
        .get(short)
        == Some(expected)
}

fn spawned_worker_still_owned(
    runtime_dir: &Path,
    short: &str,
    session_id: Option<&str>,
    generation: &str,
    claim_token: &str,
    child_pid: i32,
) -> bool {
    let Some(current) = agents_registry::read_job(runtime_dir, short) else {
        return false;
    };
    if current.session_id.as_deref() != session_id
        || current.worker_generation.as_deref() != Some(generation)
    {
        return false;
    }
    let launching = current.state == "working"
        && current.phase.as_deref() == Some(crate::commands::respawn::PHASE_LAUNCHING)
        && current.worker_pid.is_none()
        && current.claim_token.as_deref() == Some(claim_token)
        && current.claim_owner.as_deref() == Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH);
    let worker_bound_first = current.state == "working"
        && current.phase.as_deref() == Some(crate::commands::respawn::PHASE_RUNNING)
        && current.worker_pid == Some(child_pid)
        && current.claim_token.is_none();
    launching || worker_bound_first
}

/// Stop one user-selected background job using the same worker and PTY
/// primitives as daemon shutdown.  Keeping this seam here prevents the public
/// `stop`/`kill` commands from inventing a second process-tree protocol.
pub(crate) fn stop_background_job(
    runtime_dir: &Path,
    short: &str,
    job: &agents_registry::JobState,
) -> bool {
    // Deletion owns quiescence and keeps an exact token until unlink. A
    // concurrent user stop or daemon shutdown must not clear that absorbing
    // claim after the deleting caller has already proved the worker gone.
    if job.phase.as_deref() == Some(crate::commands::respawn::PHASE_DELETING) {
        return false;
    }
    // A terminal job is already stopped from the user's perspective.  Keep
    // its recorded outcome and never act on a stale terminal workerPid.
    if agents_registry::job_is_terminal(job) {
        return true;
    }

    let probe = SystemProbe;
    daemon_roster::with_roster_lock(runtime_dir, || {
        let roster = daemon_roster::read_roster(runtime_dir, 0, false).into_roster();
        let record = roster.workers.get(short).cloned();

        let stopped_record = match record {
            Some(record) => {
                if !worker_record_matches_job(&record, job)
                    || job.worker_pid.is_some_and(|pid| pid != record.pid)
                    || job.worker_proc_start.as_deref() != record.proc_start.as_deref()
                    || record.proc_start.as_deref().is_none_or(str::is_empty)
                {
                    return Ok(false);
                }
                let mut terminator = SystemStallTerminator;
                if !terminate_stalled_worker(
                    runtime_dir,
                    short,
                    &record,
                    &probe,
                    &mut terminator,
                    StallTerminationMode::GracefulThenHard,
                ) {
                    return Ok(false);
                }
                Some(record)
            }
            None => {
                if job.worker_pid.is_some_and(|pid| probe.is_alive(pid)) {
                    return Ok(false);
                }
                cleanup_orphaned_pty(runtime_dir, short, job.worker_pid, &probe);
                if read_stall_pty_runtime(runtime_dir, short)
                    .as_ref()
                    .is_some_and(|runtime| !pty_runtime_is_gone(runtime, &probe))
                {
                    return Ok(false);
                }
                None
            }
        };

        match agents_registry::patch_job_state_if_matches(
            runtime_dir,
            short,
            agents_registry::JobStateMatch {
                state: &job.state,
                phase: job.phase.as_deref(),
                worker_pid: job.worker_pid,
                worker_proc_start: job.worker_proc_start.as_deref(),
                worker_generation: job.worker_generation.as_deref(),
                claim_token: job.claim_token.as_deref(),
                claim_owner: job.claim_owner.as_deref(),
                claim_created_at: job.claim_created_at,
                claim_lease_ms: job.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                state: Some("stopped"),
                tempo: None,
                cwd: None,
                detail: Some(None),
                worker_pid: Some(None),
                worker_proc_start: Some(None),
                phase: Some(None),
                worker_generation: Some(None),
                claim_token: Some(None),
                claim_owner: Some(None),
                claim_created_at: Some(None),
                claim_lease_ms: Some(None),
            },
        ) {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(error) => {
                tracing::warn!(
                    "lingxi-cli daemon: could not persist stop state for {short}: {error}"
                );
                return Ok(false);
            }
        }

        if let Some(stopped_record) = stopped_record {
            let mut latest = daemon_roster::read_roster(runtime_dir, 0, false).into_roster();
            let same_generation = latest.workers.get(short).is_some_and(|current| {
                current.pid == stopped_record.pid && current.proc_start == stopped_record.proc_start
            });
            if same_generation {
                latest.workers.remove(short);
                if let Err(error) =
                    daemon_roster::write_roster_with_lock_held(runtime_dir, &latest)
                {
                    tracing::warn!(
                        "lingxi-cli daemon: could not retire stopped worker record for {short}: {error}"
                    );
                }
            }
        }
        Ok(true)
    })
    .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub(crate) struct DeleteClaim {
    pub token: String,
    pub job: agents_registry::JobState,
}

/// Claim and quiesce one job for deletion while holding the roster lock.
///
/// The daemon owns launch publication under this same lock, so a delete cannot
/// slip between spawn and roster publication. The durable `deleting` claim is
/// intentionally retained after the worker is gone; only the deleting caller
/// holding the exact token may remove the job directory.
pub(crate) fn claim_and_quiesce_background_job_for_delete(
    runtime_dir: &Path,
    short: &str,
    expected: &agents_registry::JobState,
) -> Result<DeleteClaim, String> {
    let probe = SystemProbe;
    let mut terminator = SystemStallTerminator;
    claim_and_quiesce_background_job_for_delete_with(
        runtime_dir,
        short,
        expected,
        &probe,
        &mut terminator,
        now_millis(),
    )
}

fn claim_and_quiesce_background_job_for_delete_with<PP: ProcProbe, ST: StallTerminator>(
    runtime_dir: &Path,
    short: &str,
    expected: &agents_registry::JobState,
    probe: &PP,
    terminator: &mut ST,
    claimed_at: i64,
) -> Result<DeleteClaim, String> {
    let _roster_lock =
        daemon_roster::lock_roster(runtime_dir).map_err(|error| error.to_string())?;
    let token = uuid::Uuid::new_v4().to_string();
    let claimed = agents_registry::patch_job_state_if_matches(
        runtime_dir,
        short,
        agents_registry::JobStateMatch {
            state: &expected.state,
            phase: expected.phase.as_deref(),
            worker_pid: expected.worker_pid,
            worker_proc_start: expected.worker_proc_start.as_deref(),
            worker_generation: expected.worker_generation.as_deref(),
            claim_token: expected.claim_token.as_deref(),
            claim_owner: expected.claim_owner.as_deref(),
            claim_created_at: expected.claim_created_at,
            claim_lease_ms: expected.claim_lease_ms,
        },
        agents_registry::JobStatePatch {
            state: Some(&expected.state),
            tempo: None,
            cwd: None,
            detail: Some(None),
            worker_pid: None,
            worker_proc_start: None,
            phase: Some(Some(crate::commands::respawn::PHASE_DELETING)),
            worker_generation: None,
            claim_token: Some(Some(&token)),
            claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_DELETE)),
            claim_created_at: Some(Some(claimed_at)),
            claim_lease_ms: Some(Some(crate::commands::respawn::CLAIM_LEASE_MS)),
        },
    )
    .map_err(|error| error.to_string())?;
    if !claimed {
        return Err("session state changed before deletion".to_string());
    }
    let claimed_job = agents_registry::read_job(runtime_dir, short)
        .ok_or_else(|| "background job disappeared before deletion".to_string())?;

    let mut roster = daemon_roster::read_roster(runtime_dir, 0, false).into_roster();
    let mut quiesced_record_identity = None;
    if let Some(record) = roster.workers.get(short).cloned() {
        let identity_state = strong_worker_identity(&record, probe);
        if identity_state == StrongWorkerIdentity::LiveUnverified {
            return Err("background worker generation could not be verified".to_string());
        }
        if identity_state == StrongWorkerIdentity::LiveVerified
            && !worker_record_matches_job(&record, &claimed_job)
        {
            return Err("background worker generation changed before deletion".to_string());
        }
        if !terminate_stalled_worker(
            runtime_dir,
            short,
            &record,
            probe,
            terminator,
            StallTerminationMode::GracefulThenHard,
        ) {
            return Err("background worker did not stop".to_string());
        }
        roster.workers.remove(short);
        quiesced_record_identity = Some(ObservedProcessIdentity::from_worker(&record));
    }

    if let Some(worker) = job_live_identity(&claimed_job) {
        let already_quiesced = quiesced_record_identity.as_ref().is_some_and(|record| {
            record.pid == worker.pid && record.proc_start == worker.proc_start
        });
        if !already_quiesced
            && strong_observed_identity(&worker, probe) == StrongWorkerIdentity::LiveUnverified
        {
            return Err("background worker generation could not be verified".to_string());
        }
        if !already_quiesced
            && !terminate_observed_worker(
                runtime_dir,
                short,
                &worker,
                probe,
                terminator,
                StallTerminationMode::GracefulThenHard,
            )
        {
            return Err("background worker did not stop".to_string());
        }
    } else if quiesced_record_identity.is_none() {
        cleanup_orphaned_pty(runtime_dir, short, None, probe);
        if read_stall_pty_runtime(runtime_dir, short)
            .as_ref()
            .is_some_and(|runtime| !pty_runtime_is_gone(runtime, probe))
        {
            return Err("background PTY generation could not be verified".to_string());
        }
    }

    let cleared = agents_registry::patch_job_state_if_matches(
        runtime_dir,
        short,
        agents_registry::JobStateMatch {
            state: &claimed_job.state,
            phase: Some(crate::commands::respawn::PHASE_DELETING),
            worker_pid: claimed_job.worker_pid,
            worker_proc_start: claimed_job.worker_proc_start.as_deref(),
            worker_generation: claimed_job.worker_generation.as_deref(),
            claim_token: Some(&token),
            claim_owner: Some(crate::commands::respawn::CLAIM_OWNER_DELETE),
            claim_created_at: Some(claimed_at),
            claim_lease_ms: Some(crate::commands::respawn::CLAIM_LEASE_MS),
        },
        agents_registry::JobStatePatch {
            state: Some(&claimed_job.state),
            tempo: None,
            cwd: None,
            detail: Some(None),
            worker_pid: Some(None),
            worker_proc_start: Some(None),
            phase: Some(Some(crate::commands::respawn::PHASE_DELETING)),
            worker_generation: Some(claimed_job.worker_generation.as_deref()),
            claim_token: None,
            claim_owner: None,
            claim_created_at: None,
            claim_lease_ms: None,
        },
    )
    .map_err(|error| error.to_string())?;
    if !cleared {
        return Err("session state changed while deletion was stopping it".to_string());
    }
    daemon_roster::write_roster_with_lock_held(runtime_dir, &roster)
        .map_err(|error| error.to_string())?;
    let job = agents_registry::read_job(runtime_dir, short)
        .ok_or_else(|| "background job disappeared before deletion".to_string())?;
    Ok(DeleteClaim { token, job })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StallTerminationMode {
    GracefulThenHard,
    HardOnly,
}

#[derive(Debug, Clone)]
struct ObservedProcessIdentity {
    pid: i32,
    proc_start: Option<String>,
}

impl ObservedProcessIdentity {
    fn from_worker(record: &WorkerRecord) -> Self {
        Self {
            pid: record.pid,
            proc_start: record.proc_start.clone(),
        }
    }

    fn is_gone<PP: ProcProbe>(&self, probe: &PP) -> bool {
        if self.pid <= 1 || !probe.is_alive(self.pid) {
            return true;
        }
        match self.proc_start.as_deref() {
            // An unreadable live start time is UNKNOWN, not proof that the
            // recorded process exited. Treating `None` as "gone" lets stop/rm
            // retire the durable owner while an unverified worker keeps
            // running, after which the daemon may launch a duplicate.
            Some(expected) => probe
                .start_time(self.pid)
                .is_some_and(|actual| actual != expected),
            None => false,
        }
    }

    fn matches_live_process<PP: ProcProbe>(&self, probe: &PP) -> bool {
        if self.pid <= 1 || !probe.is_alive(self.pid) {
            return false;
        }
        match self.proc_start.as_deref() {
            Some(expected) => probe.start_time(self.pid).as_deref() == Some(expected),
            // Legacy roster entries have no PID-reuse identity. Treat a live
            // numeric PID as unverified: the restart barrier must fail closed
            // rather than signal a potentially unrelated process.
            None => false,
        }
    }
}

trait StallTerminator {
    fn signal_worker(&mut self, pid: i32, hard: bool);
    fn signal_pty_tree<PP: ProcProbe>(
        &mut self,
        runtime: &crate::background_launch::BackgroundPtyRuntime,
        probe: &PP,
        hard: bool,
    );
    fn sleep(&mut self, ms: u64);
}

struct SystemStallTerminator;

impl StallTerminator for SystemStallTerminator {
    fn signal_worker(&mut self, pid: i32, hard: bool) {
        kill_worker(pid, hard);
    }

    fn signal_pty_tree<PP: ProcProbe>(
        &mut self,
        runtime: &crate::background_launch::BackgroundPtyRuntime,
        probe: &PP,
        hard: bool,
    ) {
        signal_pty_tree(runtime, probe, hard);
    }

    fn sleep(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

enum ObservedPtyRuntime {
    Verified(crate::background_launch::BackgroundPtyRuntime),
    UnverifiedPresent,
}

fn read_stall_pty_runtime(runtime_dir: &Path, short: &str) -> Option<ObservedPtyRuntime> {
    if let Ok(runtime) = crate::background_launch::read_pty_runtime(runtime_dir, short) {
        return Some(ObservedPtyRuntime::Verified(runtime));
    }
    std::fs::symlink_metadata(crate::background_launch::pty_runtime_path(
        runtime_dir,
        short,
    ))
    .ok()
    .map(|_| ObservedPtyRuntime::UnverifiedPresent)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PtyRuntimeState {
    Gone,
    LiveVerified,
    Unknown,
}

fn verified_pty_runtime_state<PP: ProcProbe>(
    runtime: &crate::background_launch::BackgroundPtyRuntime,
    probe: &PP,
) -> PtyRuntimeState {
    let Ok(child_pid) = i32::try_from(runtime.child_pid) else {
        return PtyRuntimeState::Gone;
    };
    if !probe.is_alive(child_pid) {
        return PtyRuntimeState::Gone;
    }
    match (
        runtime.child_proc_start.as_deref(),
        probe.start_time(child_pid),
    ) {
        (Some(expected), Some(actual)) if actual == expected => PtyRuntimeState::LiveVerified,
        (Some(expected), Some(actual)) if actual != expected => PtyRuntimeState::Gone,
        _ => PtyRuntimeState::Unknown,
    }
}

fn pty_runtime_state<PP: ProcProbe>(runtime: &ObservedPtyRuntime, probe: &PP) -> PtyRuntimeState {
    match runtime {
        ObservedPtyRuntime::UnverifiedPresent => PtyRuntimeState::Unknown,
        ObservedPtyRuntime::Verified(runtime) => verified_pty_runtime_state(runtime, probe),
    }
}

fn pty_runtime_is_gone<PP: ProcProbe>(runtime: &ObservedPtyRuntime, probe: &PP) -> bool {
    pty_runtime_state(runtime, probe) == PtyRuntimeState::Gone
}

fn identities_are_gone<PP: ProcProbe>(
    probe: &PP,
    worker: &ObservedProcessIdentity,
    pty_runtime: Option<&ObservedPtyRuntime>,
) -> bool {
    worker.is_gone(probe) && pty_runtime.is_none_or(|runtime| pty_runtime_is_gone(runtime, probe))
}

fn wait_for_stall_termination<PP: ProcProbe, ST: StallTerminator>(
    probe: &PP,
    terminator: &mut ST,
    worker: &ObservedProcessIdentity,
    pty_runtime: Option<&ObservedPtyRuntime>,
) -> bool {
    if identities_are_gone(probe, worker, pty_runtime) {
        return true;
    }
    for _ in 0..STALL_TERMINATION_POLL_ATTEMPTS {
        terminator.sleep(STALL_TERMINATION_POLL_MS);
        if identities_are_gone(probe, worker, pty_runtime) {
            return true;
        }
    }
    false
}

fn terminate_stalled_worker<PP: ProcProbe, ST: StallTerminator>(
    runtime_dir: &Path,
    short: &str,
    record: &WorkerRecord,
    probe: &PP,
    terminator: &mut ST,
    mode: StallTerminationMode,
) -> bool {
    let worker = ObservedProcessIdentity::from_worker(record);
    terminate_observed_worker(runtime_dir, short, &worker, probe, terminator, mode)
}

fn terminate_observed_worker<PP: ProcProbe, ST: StallTerminator>(
    runtime_dir: &Path,
    short: &str,
    worker: &ObservedProcessIdentity,
    probe: &PP,
    terminator: &mut ST,
    mode: StallTerminationMode,
) -> bool {
    let pty_runtime = read_stall_pty_runtime(runtime_dir, short);
    let pty_runtime_ref = pty_runtime.as_ref();

    match mode {
        StallTerminationMode::GracefulThenHard => {
            if worker.matches_live_process(probe) {
                terminator.signal_worker(worker.pid, false);
            }
            if let Some(ObservedPtyRuntime::Verified(runtime)) = pty_runtime_ref.filter(|runtime| {
                pty_runtime_state(runtime, probe) == PtyRuntimeState::LiveVerified
            }) {
                terminator.signal_pty_tree(runtime, probe, false);
            }
            if wait_for_stall_termination(probe, terminator, worker, pty_runtime_ref) {
                crate::background_launch::remove_pty_runtime(runtime_dir, short);
                return true;
            }
            if worker.matches_live_process(probe) {
                terminator.signal_worker(worker.pid, true);
            }
            if let Some(ObservedPtyRuntime::Verified(runtime)) = pty_runtime_ref.filter(|runtime| {
                pty_runtime_state(runtime, probe) == PtyRuntimeState::LiveVerified
            }) {
                terminator.signal_pty_tree(runtime, probe, true);
            }
        }
        StallTerminationMode::HardOnly => {
            if worker.matches_live_process(probe) {
                terminator.signal_worker(worker.pid, true);
            }
            if let Some(ObservedPtyRuntime::Verified(runtime)) = pty_runtime_ref.filter(|runtime| {
                pty_runtime_state(runtime, probe) == PtyRuntimeState::LiveVerified
            }) {
                terminator.signal_pty_tree(runtime, probe, true);
            }
        }
    }

    let stopped = wait_for_stall_termination(probe, terminator, worker, pty_runtime_ref);
    if stopped {
        crate::background_launch::remove_pty_runtime(runtime_dir, short);
    }
    stopped
}

/// Kill a PTY tree left behind by a vanished worker. Numeric identities are
/// used only while the recorded creation time still matches, preventing stale
/// `pty.json` files from targeting a recycled PID.
fn cleanup_orphaned_pty<PP: ProcProbe>(
    runtime_dir: &Path,
    short: &str,
    expected_worker_pid: Option<i32>,
    probe: &PP,
) {
    let Some(runtime) = read_stall_pty_runtime(runtime_dir, short) else {
        return;
    };
    match pty_runtime_state(&runtime, probe) {
        PtyRuntimeState::Gone => {
            crate::background_launch::remove_pty_runtime(runtime_dir, short);
            return;
        }
        PtyRuntimeState::Unknown => {
            return;
        }
        PtyRuntimeState::LiveVerified => {}
    }
    let ObservedPtyRuntime::Verified(runtime) = runtime else {
        return;
    };
    if expected_worker_pid.is_some_and(|pid| pid != runtime.worker_pid) {
        return;
    }
    signal_pty_tree(&runtime, probe, false);
    std::thread::sleep(std::time::Duration::from_millis(100));
    if verified_pty_runtime_state(&runtime, probe) == PtyRuntimeState::Gone {
        crate::background_launch::remove_pty_runtime(runtime_dir, short);
        return;
    }
    signal_pty_tree(&runtime, probe, true);
    std::thread::sleep(std::time::Duration::from_millis(100));
    if verified_pty_runtime_state(&runtime, probe) == PtyRuntimeState::Gone {
        crate::background_launch::remove_pty_runtime(runtime_dir, short);
    }
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

/// Durable respawn-attempt counter writer (sibling of [`read_respawn_count`]).
fn write_respawn_count(runtime_dir: &Path, short: &str, count: i64) {
    let dir = agents_registry::jobs_dir(runtime_dir).join(short);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("respawns"), format!("{count}\n"));
}

/// P1-12 — service stall-respawn requests left by attach clients.
///
/// The attach client sees the stall (it owns the stream) but has no authority
/// to restart a supervised worker; the daemon has authority but never sees
/// frames. The client leaves a request file, and this consumes it.
///
/// The restart is a RESUME, never a re-prompt. That distinction is the whole
/// reason the daemon otherwise fails a vanished worker closed: re-running the
/// original prompt would duplicate whatever side effects the first run already
/// committed. Flipping the launch spec to `Resume` keeps the session's own
/// transcript as the continuation point.
fn service_stall_requests<PP: ProcProbe>(
    runtime_dir: &Path,
    roster: &mut Roster,
    proc_probe: &PP,
    claimed: &mut HashSet<String>,
) {
    let mut terminator = SystemStallTerminator;
    service_stall_requests_with_terminator(
        runtime_dir,
        roster,
        proc_probe,
        claimed,
        &mut terminator,
    );
}

fn service_stall_requests_with_terminator<PP: ProcProbe, ST: StallTerminator>(
    runtime_dir: &Path,
    roster: &mut Roster,
    proc_probe: &PP,
    claimed: &mut HashSet<String>,
    terminator: &mut ST,
) {
    let jobs = agents_registry::jobs_dir(runtime_dir);
    let shorts: Vec<String> = roster.workers.keys().cloned().collect();
    for short in shorts {
        if !crate::bg_attach_stall::take_stall_request(&jobs, &short) {
            continue;
        }
        let attempt = read_respawn_count(runtime_dir, &short);
        let Some(record) = roster.workers.get(&short).cloned() else {
            continue;
        };
        let Some(job) = agents_registry::read_job(runtime_dir, &short) else {
            continue;
        };
        if !worker_record_matches_job(&record, &job)
            || strong_worker_identity(&record, proc_probe) != StrongWorkerIdentity::LiveVerified
            || !stall_pty_identity_is_safe_for_restart(runtime_dir, &short, &record, proc_probe)
        {
            // A stall request authorizes restarting this job, not signaling a
            // same-short stale process or a PID whose generation cannot be
            // proved. Keep both durable and in-memory ownership intact; a
            // later verified heartbeat/request may retry safely.
            continue;
        }

        if attempt >= crate::bg_attach_stall::STALL_RESPAWN_BUDGET {
            // Budget spent. SIGKILL and fail closed with the oracle's reason —
            // a session that will not paint after two restarts is not going to.
            crate::bg_attach_stall::emit_stall_gave_up("starting", "daemon", attempt);
            let stopped = terminate_stalled_worker(
                runtime_dir,
                &short,
                &record,
                proc_probe,
                terminator,
                StallTerminationMode::HardOnly,
            );
            if stopped {
                let _ = agents_registry::update_job_state_if_matches(
                    runtime_dir,
                    &short,
                    &job.state,
                    job.worker_pid,
                    job.worker_proc_start.as_deref(),
                    "failed",
                    None,
                    None,
                    Some(crate::bg_attach_stall::KEEPS_STALLING_KILL_REASON),
                );
                roster.workers.remove(&short);
                claimed.remove(&short);
            }
            continue;
        }

        let claimed_resume = crate::commands::respawn::claim_resume_if_matches(
            runtime_dir,
            &short,
            &job.state,
            job.phase.as_deref(),
            job.worker_pid,
            job.worker_proc_start.as_deref(),
            job.worker_generation.as_deref(),
        );
        let Ok(Some(claim)) = claimed_resume else {
            continue;
        };
        let original_launch_spec =
            crate::background_launch::read_launch_spec(runtime_dir, &short).ok();
        // Prepare the durable resume BEFORE killing, so a crash between the
        // two never leaves a spec that would replay the original prompt.
        if let Err(_error) = crate::commands::respawn::prepare_resume(runtime_dir, &short) {
            let _ = crate::commands::respawn::restore_claimed_resume(
                runtime_dir,
                &short,
                &job.state,
                job.phase.as_deref(),
                job.worker_pid,
                job.worker_proc_start.as_deref(),
                job.worker_generation.as_deref(),
                &claim,
            );
            continue;
        }
        write_respawn_count(runtime_dir, &short, attempt + 1);
        let stopped = terminate_stalled_worker(
            runtime_dir,
            &short,
            &record,
            proc_probe,
            terminator,
            StallTerminationMode::GracefulThenHard,
        );
        if !stopped {
            if let Some(spec) = original_launch_spec.as_ref() {
                if let Err(error) =
                    crate::background_launch::write_launch_spec(runtime_dir, &short, spec)
                {
                    tracing::warn!(
                        "lingxi-cli daemon: could not restore launch context for {short}: {error}"
                    );
                }
            }
            write_respawn_count(runtime_dir, &short, attempt);
            let _ = crate::commands::respawn::restore_claimed_resume(
                runtime_dir,
                &short,
                &job.state,
                job.phase.as_deref(),
                job.worker_pid,
                job.worker_proc_start.as_deref(),
                job.worker_generation.as_deref(),
                &claim,
            );
            continue;
        }

        match crate::commands::respawn::queue_prepared_resume(runtime_dir, &short, &claim) {
            Ok(()) => {
                crate::bg_attach_stall::emit_stall_respawn("starting", "daemon", attempt);
                // Drop the record and the claim only AFTER the old writer is confirmed
                // gone, so the same heartbeat cannot double-spawn concurrent writers.
                roster.workers.remove(&short);
                claimed.remove(&short);
            }
            Err(error) => {
                roster.workers.remove(&short);
                claimed.remove(&short);
                let _ = crate::commands::respawn::fail_claimed_resume(
                    runtime_dir,
                    &short,
                    &claim,
                    Some(error.as_str()),
                );
            }
        }
    }
}

/// A stall request may only act on a PTY identity that is either already gone
/// or can be tied to the same verified worker record. A legacy/unreadable PTY
/// record is not permission to signal the worker: doing so could strand an
/// unverified writer while making its durable job runnable again.
fn stall_pty_identity_is_safe_for_restart<PP: ProcProbe>(
    runtime_dir: &Path,
    short: &str,
    record: &WorkerRecord,
    proc_probe: &PP,
) -> bool {
    let Some(runtime) = read_stall_pty_runtime(runtime_dir, short) else {
        return true;
    };
    match pty_runtime_state(&runtime, proc_probe) {
        PtyRuntimeState::Gone => true,
        PtyRuntimeState::Unknown => false,
        PtyRuntimeState::LiveVerified => matches!(
            runtime,
            ObservedPtyRuntime::Verified(ref runtime) if runtime.worker_pid == record.pid
        ),
    }
}

/// Signal a worker. `hard` selects SIGKILL over SIGTERM.
///
/// Uses the same `nix` path as [`stop_all_workers`] — the crate forbids
/// `unsafe`, so a raw `libc::kill` is not available here.
fn kill_worker(pid: i32, hard: bool) {
    #[cfg(unix)]
    {
        let sig = if hard {
            nix::sys::signal::Signal::SIGKILL
        } else {
            nix::sys::signal::Signal::SIGTERM
        };
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), Some(sig));
    }
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("taskkill.exe");
        command.args(["/PID", &pid.to_string(), "/T"]);
        if hard {
            command.arg("/F");
        }
        let _ = command.status();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, hard);
    }
}

fn signal_pty_tree<PP: ProcProbe>(
    runtime: &crate::background_launch::BackgroundPtyRuntime,
    probe: &PP,
    hard: bool,
) {
    if verified_pty_runtime_state(runtime, probe) != PtyRuntimeState::LiveVerified {
        return;
    }
    #[cfg(unix)]
    {
        let group = runtime.process_group_id.unwrap_or(runtime.child_pid);
        if let Ok(group) = i32::try_from(group) {
            let target = nix::unistd::Pid::from_raw(-group);
            let signal = if hard {
                nix::sys::signal::Signal::SIGKILL
            } else {
                nix::sys::signal::Signal::SIGTERM
            };
            let _ = nix::sys::signal::kill(target, Some(signal));
        }
    }
    #[cfg(windows)]
    {
        let Ok(child_pid) = i32::try_from(runtime.child_pid) else {
            return;
        };
        let mut command = std::process::Command::new("taskkill.exe");
        command.args(["/PID", &child_pid.to_string(), "/T"]);
        if hard {
            command.arg("/F");
        }
        let _ = command.status();
    }
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
    let _ = daemon_roster::with_roster_lock(runtime_dir, || {
        let mut roster = daemon_roster::read_roster(runtime_dir, pid, true).into_roster();
        let _dropped = daemon_roster::retain_adoptable(&mut roster, proc_probe);
        recover_stale_job_claims(runtime_dir, &mut roster, proc_probe, claimed, now_millis());
        service_stall_requests(runtime_dir, &mut roster, proc_probe, claimed);
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
        daemon_roster::write_roster_with_lock_held(runtime_dir, &roster)
    });
}

fn claim_is_stale(job: &agents_registry::JobState, now_ms: i64) -> bool {
    let Some(created_at) = job.claim_created_at else {
        // Transitional rows written by older builds predate lease metadata.
        // Treat them as recoverable legacy claims; each phase-specific recovery
        // path still proves roster/PTY state before making work runnable.
        return matches!(
            job.phase.as_deref(),
            Some(
                crate::commands::respawn::PHASE_CREATING
                    | crate::commands::respawn::PHASE_LAUNCHING
                    | crate::commands::respawn::PHASE_RESTARTING
            )
        );
    };
    let lease = job
        .claim_lease_ms
        .unwrap_or(crate::commands::respawn::CLAIM_LEASE_MS)
        .max(0);
    now_ms.saturating_sub(created_at) >= lease
}

fn clear_claim_to_queued(
    runtime_dir: &Path,
    short: &str,
    job: &agents_registry::JobState,
) -> std::io::Result<bool> {
    agents_registry::patch_job_state_if_matches(
        runtime_dir,
        short,
        agents_registry::JobStateMatch {
            state: &job.state,
            phase: job.phase.as_deref(),
            worker_pid: job.worker_pid,
            worker_proc_start: job.worker_proc_start.as_deref(),
            worker_generation: job.worker_generation.as_deref(),
            claim_token: job.claim_token.as_deref(),
            claim_owner: job.claim_owner.as_deref(),
            claim_created_at: job.claim_created_at,
            claim_lease_ms: job.claim_lease_ms,
        },
        agents_registry::JobStatePatch {
            state: Some("working"),
            tempo: None,
            cwd: None,
            detail: Some(None),
            worker_pid: Some(None),
            worker_proc_start: Some(None),
            phase: Some(Some(crate::commands::respawn::PHASE_QUEUED)),
            worker_generation: None,
            claim_token: Some(None),
            claim_owner: Some(None),
            claim_created_at: Some(None),
            claim_lease_ms: Some(None),
        },
    )
}

fn fail_stale_claim(
    runtime_dir: &Path,
    short: &str,
    job: &agents_registry::JobState,
    detail: &str,
) {
    let _ = agents_registry::patch_job_state_if_matches(
        runtime_dir,
        short,
        agents_registry::JobStateMatch {
            state: &job.state,
            phase: job.phase.as_deref(),
            worker_pid: job.worker_pid,
            worker_proc_start: job.worker_proc_start.as_deref(),
            worker_generation: job.worker_generation.as_deref(),
            claim_token: job.claim_token.as_deref(),
            claim_owner: job.claim_owner.as_deref(),
            claim_created_at: job.claim_created_at,
            claim_lease_ms: job.claim_lease_ms,
        },
        agents_registry::JobStatePatch {
            state: Some("failed"),
            tempo: None,
            cwd: None,
            detail: Some(Some(detail)),
            worker_pid: Some(None),
            worker_proc_start: Some(None),
            phase: Some(None),
            worker_generation: Some(None),
            claim_token: Some(None),
            claim_owner: Some(None),
            claim_created_at: Some(None),
            claim_lease_ms: Some(None),
        },
    );
}

fn recover_stale_creating_job<PP: ProcProbe>(
    runtime_dir: &Path,
    short: &str,
    job: &agents_registry::JobState,
    roster: &mut Roster,
    proc_probe: &PP,
    claimed: &mut HashSet<String>,
) {
    if let Some(record) = roster.workers.get(short) {
        match strong_worker_identity(record, proc_probe) {
            StrongWorkerIdentity::LiveVerified | StrongWorkerIdentity::LiveUnverified => {
                return;
            }
            StrongWorkerIdentity::GoneOrRecycled => {}
        }
    }
    if durable_owner_may_still_be_live(runtime_dir, short, job, proc_probe) {
        return;
    }
    roster.workers.remove(short);
    if crate::background_launch::read_launch_spec(runtime_dir, short).is_ok() {
        if clear_claim_to_queued(runtime_dir, short, job).unwrap_or(false) {
            claimed.remove(short);
        }
    } else {
        fail_stale_claim(
            runtime_dir,
            short,
            job,
            "background launch context is missing or incompatible",
        );
        claimed.remove(short);
    }
}

fn durable_owner_may_still_be_live<PP: ProcProbe>(
    runtime_dir: &Path,
    short: &str,
    job: &agents_registry::JobState,
    proc_probe: &PP,
) -> bool {
    if job_live_identity(job).as_ref().is_some_and(|identity| {
        strong_observed_identity(identity, proc_probe) != StrongWorkerIdentity::GoneOrRecycled
    }) {
        return true;
    }
    let Some(runtime) = read_stall_pty_runtime(runtime_dir, short) else {
        return false;
    };
    match pty_runtime_state(&runtime, proc_probe) {
        PtyRuntimeState::LiveVerified | PtyRuntimeState::Unknown => true,
        PtyRuntimeState::Gone => {
            crate::background_launch::remove_pty_runtime(runtime_dir, short);
            false
        }
    }
}

fn recover_stale_restarting_job<PP: ProcProbe>(
    runtime_dir: &Path,
    short: &str,
    job: &agents_registry::JobState,
    roster: &mut Roster,
    proc_probe: &PP,
    claimed: &mut HashSet<String>,
) {
    if let Some(record) = roster.workers.get(short) {
        match strong_worker_identity(record, proc_probe) {
            StrongWorkerIdentity::LiveVerified | StrongWorkerIdentity::LiveUnverified => {
                // A live-but-unverifiable PID is never evidence that a new
                // generation may start. A verified record from another
                // generation is also left blocked rather than killed/adopted.
                return;
            }
            StrongWorkerIdentity::GoneOrRecycled => {}
        }
    }
    if durable_owner_may_still_be_live(runtime_dir, short, job, proc_probe) {
        return;
    }
    let attempt = read_respawn_count(runtime_dir, short);
    if attempt >= crate::bg_attach_stall::STALL_RESPAWN_BUDGET {
        fail_stale_claim(runtime_dir, short, job, CLAIM_RECOVERY_EXHAUSTED_REASON);
        claimed.remove(short);
        return;
    }
    match crate::commands::respawn::prepare_resume(runtime_dir, short) {
        Ok(()) => {
            if clear_claim_to_queued(runtime_dir, short, job).unwrap_or(false) {
                write_respawn_count(runtime_dir, short, attempt + 1);
                claimed.remove(short);
            }
        }
        Err(error) => {
            fail_stale_claim(runtime_dir, short, job, &error);
            claimed.remove(short);
        }
    }
}

fn recover_stale_launching_job<PP: ProcProbe>(
    runtime_dir: &Path,
    short: &str,
    job: &agents_registry::JobState,
    roster: &mut Roster,
    proc_probe: &PP,
    claimed: &mut HashSet<String>,
) {
    if let Some(record) = roster.workers.get(short) {
        match strong_worker_identity(record, proc_probe) {
            StrongWorkerIdentity::LiveVerified if worker_record_matches_job(record, job) => {
                return;
            }
            StrongWorkerIdentity::LiveVerified | StrongWorkerIdentity::LiveUnverified => {
                // The PID exists but does not carry enough exact generation
                // proof. Keep the claim blocked; removing the roster here lets
                // this same heartbeat spawn a second writer.
                return;
            }
            StrongWorkerIdentity::GoneOrRecycled => {}
        }
    }
    if durable_owner_may_still_be_live(runtime_dir, short, job, proc_probe) {
        return;
    }
    roster.workers.remove(short);
    let attempt = read_respawn_count(runtime_dir, short);
    if attempt >= crate::bg_attach_stall::STALL_RESPAWN_BUDGET {
        fail_stale_claim(runtime_dir, short, job, CLAIM_RECOVERY_EXHAUSTED_REASON);
        claimed.remove(short);
        return;
    }
    if clear_claim_to_queued(runtime_dir, short, job).unwrap_or(false) {
        write_respawn_count(runtime_dir, short, attempt + 1);
        claimed.remove(short);
    }
}

fn recover_stale_job_claims<PP: ProcProbe>(
    runtime_dir: &Path,
    roster: &mut Roster,
    proc_probe: &PP,
    claimed: &mut HashSet<String>,
    now_ms: i64,
) {
    let jobs = agents_registry::read_jobs(&agents_registry::jobs_dir(runtime_dir));
    for (short, job) in jobs {
        if agents_registry::job_is_terminal(&job) || !claim_is_stale(&job, now_ms) {
            continue;
        }
        match job.phase.as_deref() {
            Some(crate::commands::respawn::PHASE_CREATING) => {
                recover_stale_creating_job(runtime_dir, &short, &job, roster, proc_probe, claimed);
            }
            Some(crate::commands::respawn::PHASE_RESTARTING) => {
                recover_stale_restarting_job(
                    runtime_dir,
                    &short,
                    &job,
                    roster,
                    proc_probe,
                    claimed,
                );
            }
            Some(crate::commands::respawn::PHASE_LAUNCHING) => {
                recover_stale_launching_job(runtime_dir, &short, &job, roster, proc_probe, claimed);
            }
            _ => {}
        }
    }
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
/// (the worker died before writing its own terminal state) is re-queued only
/// when its recorded transcript can be safely reopened under the same session
/// id. Otherwise it fails closed. The live attach socket is only valid while
/// the worker process exists; after the process is gone LingXi must never
/// replay the original launch prompt, because that risks duplicate side
/// effects.
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
    for (short, mut job) in jobs {
        if agents_registry::job_is_terminal(&job) {
            continue;
        }
        if job.state != "working" {
            continue;
        }
        let phase = job
            .phase
            .clone()
            .unwrap_or_else(|| crate::commands::respawn::PHASE_QUEUED.to_string());
        if matches!(
            phase.as_str(),
            crate::commands::respawn::PHASE_CREATING
                | crate::commands::respawn::PHASE_RESTARTING
                | crate::commands::respawn::PHASE_DELETING
        ) {
            continue;
        }
        if let Some(identity) = roster
            .workers
            .get(&short)
            .filter(|record| worker_record_matches_job(record, &job))
            .and_then(|record| verified_live_worker_identity(record, proc_probe))
        {
            if job.worker_pid != Some(identity.pid)
                || job.worker_proc_start.as_deref() != identity.proc_start.as_deref()
                || job.phase.as_deref() != Some(crate::commands::respawn::PHASE_RUNNING)
            {
                match agents_registry::patch_job_state_if_matches(
                    runtime_dir,
                    &short,
                    agents_registry::JobStateMatch {
                        state: &job.state,
                        phase: job.phase.as_deref(),
                        worker_pid: job.worker_pid,
                        worker_proc_start: job.worker_proc_start.as_deref(),
                        worker_generation: job.worker_generation.as_deref(),
                        claim_token: job.claim_token.as_deref(),
                        claim_owner: job.claim_owner.as_deref(),
                        claim_created_at: job.claim_created_at,
                        claim_lease_ms: job.claim_lease_ms,
                    },
                    agents_registry::JobStatePatch {
                        state: Some("working"),
                        tempo: None,
                        cwd: None,
                        detail: Some(None),
                        worker_pid: Some(Some(identity.pid)),
                        worker_proc_start: Some(identity.proc_start.as_deref()),
                        phase: Some(Some(crate::commands::respawn::PHASE_RUNNING)),
                        worker_generation: None,
                        claim_token: Some(None),
                        claim_owner: Some(None),
                        claim_created_at: Some(None),
                        claim_lease_ms: Some(None),
                    },
                ) {
                    Ok(true) => {
                        job.worker_pid = Some(identity.pid);
                        job.worker_proc_start = identity.proc_start;
                        job.phase = Some(crate::commands::respawn::PHASE_RUNNING.to_string());
                        job.claim_token = None;
                    }
                    Ok(false) => {}
                    Err(error) => tracing::warn!(
                        "lingxi-cli daemon: could not repair live worker identity for {short}: {error}"
                    ),
                }
            }
            claimed.insert(short.clone());
            continue;
        }
        if roster
            .workers
            .get(&short)
            .is_some_and(|record| proc_probe.is_alive(record.pid))
        {
            continue;
        }
        // OWNED job: it has a recorded worker pid, or we spawned it this
        // supervisor lifetime (`claimed`). Its next step depends on whether
        // that worker is still alive.
        if let Some(identity) = job_live_identity(&job) {
            // Cross-restart guard: a recorded, still-live worker pid means a
            // worker is already running this job — adopt (claim) it, don't
            // re-spawn.
            if identity.matches_live_process(proc_probe) {
                claimed.insert(short.clone());
                continue;
            }
            if proc_probe.is_alive(identity.pid) && identity.proc_start.is_none() {
                continue;
            }
            // The recorded worker DIED without writing a terminal state. LingXi
            // cannot live-attach that process anymore. If a recorded transcript
            // still exists, reopen it under the same session id; otherwise fail
            // closed rather than replaying the original launch prompt.
            emit_worker_vanished(&short);
            cleanup_orphaned_pty(runtime_dir, &short, Some(identity.pid), proc_probe);
            let attempt = read_respawn_count(runtime_dir, &short);
            let budget_spent = attempt >= crate::bg_attach_stall::STALL_RESPAWN_BUDGET;
            let queued = if !budget_spent {
                crate::commands::respawn::queue_resume_if_matches(
                    runtime_dir,
                    &short,
                    &job.state,
                    job.phase.as_deref(),
                    job.worker_pid,
                    job.worker_proc_start.as_deref(),
                    job.worker_generation.as_deref(),
                )
            } else {
                Ok(false)
            };
            if let Ok(true) = queued {
                write_respawn_count(runtime_dir, &short, attempt + 1);
                if let Some(updated) = agents_registry::read_job(runtime_dir, &short) {
                    job = updated;
                } else {
                    job.worker_pid = None;
                    job.worker_proc_start = None;
                    job.phase = Some(crate::commands::respawn::PHASE_QUEUED.to_string());
                }
            } else if matches!(queued, Ok(false)) && !budget_spent {
                continue;
            } else {
                // The vanished worker is failed closed and never respawns, so
                // the offline reply queue's only in-worker drain site never
                // runs for this job again. Claim the exact terminal ownership
                // before draining, then surface any follow-up replies the
                // attach fallback persisted mid-flight. This prevents a newer
                // generation from losing replies to an old vanish decision.
                fail_vanished_job_and_surface_replies(runtime_dir, &short, &job);
                // NB: do NOT emit `tengu_bg_respawn_exhausted` here. CC 2.1.208 emits
                // that event only from scheduleRespawn once the respawn budget
                // (Jpp=20) is reached; the fail-closed daemon never respawns, so there
                // is no budget to exhaust and firing it on the first vanish (with a
                // constant attempts:0) is a spurious signal. The vanish is already
                // recorded via `tengu_bg_worker_vanished` above.
                claimed.remove(&short);
                continue;
            }
        }
        // No recorded worker pid. If we already claimed it this lifetime the
        // pid simply hasn't been persisted yet (or its write failed) — protect
        // it from a same-heartbeat double-spawn; we can't probe liveness with
        // no pid, so leave it working for a later heartbeat to resolve.
        if claimed.contains(&short) {
            if roster.workers.contains_key(&short) {
                continue;
            }
            // A control command can retire a verified worker between
            // heartbeats. Once both durable PID and roster generation are
            // absent, release the in-memory claim so a queued resume can run.
            claimed.remove(&short);
        }
        let spawn_phase = job
            .phase
            .as_deref()
            .unwrap_or(crate::commands::respawn::PHASE_QUEUED);
        if spawn_phase != crate::commands::respawn::PHASE_QUEUED {
            continue;
        }

        // CWD-GONE guard: never spawn a worker into a working directory that no
        // longer exists. The detached PTY child would otherwise fail before it
        // can mount a usable attach transport. CC's `settleCwdGone` fails such a
        // dispatch closed with a specific detail +
        // `tengu_bg_spawn_cwd_gone{short, attempt, via}`; mirror that so the
        // failure remains legible in the agent view.
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

        // The owner-only launch spec is the canonical handoff for provider
        // credentials and other child-only environment. A dead foreground
        // roster row is deliberately reaped before this point, so relying on
        // that row silently strips the environment from the detached worker.
        // Retain the roster fallback only for legacy jobs without a launch
        // spec; the spawned roster record itself is sanitized below.
        let mut worker_env = crate::background_launch::read_launch_spec(runtime_dir, &short)
            .map(|spec| spec.env)
            .unwrap_or_else(|_| {
                roster
                    .workers
                    .get(&short)
                    .map(|record| record.dispatch.env.clone())
                    .unwrap_or_default()
            });
        let worker_generation = job
            .worker_generation
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let claim_token = uuid::Uuid::new_v4().to_string();
        let claimed_launch = match agents_registry::patch_job_state_if_matches(
            runtime_dir,
            &short,
            agents_registry::JobStateMatch {
                state: &job.state,
                phase: job.phase.as_deref(),
                worker_pid: job.worker_pid,
                worker_proc_start: job.worker_proc_start.as_deref(),
                worker_generation: job.worker_generation.as_deref(),
                claim_token: job.claim_token.as_deref(),
                claim_owner: job.claim_owner.as_deref(),
                claim_created_at: job.claim_created_at,
                claim_lease_ms: job.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                tempo: None,
                cwd: None,
                detail: Some(None),
                worker_pid: Some(None),
                worker_proc_start: Some(None),
                phase: Some(Some(crate::commands::respawn::PHASE_LAUNCHING)),
                worker_generation: Some(Some(&worker_generation)),
                claim_token: Some(Some(&claim_token)),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH)),
                claim_created_at: Some(Some(now_millis())),
                claim_lease_ms: Some(Some(crate::commands::respawn::CLAIM_LEASE_MS)),
            },
        ) {
            Ok(updated) => updated,
            Err(error) => {
                tracing::warn!("lingxi-cli daemon: could not claim queued job {short}: {error}");
                continue;
            }
        };
        if !claimed_launch {
            continue;
        }
        job.phase = Some(crate::commands::respawn::PHASE_LAUNCHING.to_string());
        job.worker_generation = Some(worker_generation.clone());
        job.claim_token = Some(claim_token.clone());
        job.claim_owner = Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH.to_string());
        job.claim_created_at = Some(now_millis());
        job.claim_lease_ms = Some(crate::commands::respawn::CLAIM_LEASE_MS);
        worker_env.extend(bg_worker_env(
            runtime_dir,
            &short,
            &worker_generation,
            &claim_token,
        ));
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
        let previous_dispatch = roster
            .workers
            .get(&short)
            .map(|record| record.dispatch.clone());
        match spawner.spawn_worker(&short, &worker_env) {
            Ok(child_pid) => {
                // Persist the authenticated endpoint before the separate job
                // workerPid update. If that second write fails, a restarted
                // daemon can adopt this record and repair state.json instead of
                // executing the initial prompt again.
                let proc_start = proc_probe.start_time(child_pid);
                let record = worker_record_for_job(
                    &short,
                    &job,
                    child_pid,
                    proc_start.clone(),
                    version,
                    runtime_dir,
                    previous_dispatch,
                    attach_sock_s,
                    attach_auth,
                );
                // The child may bind this claim before `spawn_worker` returns.
                // Accept that exact running generation, but revoke any spawn
                // whose durable state moved elsewhere (notably `deleting`)
                // before exposing a roster endpoint.
                if !spawned_worker_still_owned(
                    runtime_dir,
                    &short,
                    job.session_id.as_deref(),
                    &worker_generation,
                    &claim_token,
                    child_pid,
                ) {
                    kill_worker(child_pid, true);
                    cleanup_orphaned_pty(runtime_dir, &short, Some(child_pid), proc_probe);
                    claimed.remove(&short);
                    continue;
                }
                if let Some(worktree) = record.dispatch.worktree.as_ref() {
                    if let Err(e) = crate::daemon_roster::write_worktree_ownership_marker(
                        Path::new(&worktree.path),
                        &short,
                        &record.session_id,
                        &worktree.ownership_token,
                    ) {
                        tracing::warn!(
                            "lingxi-cli daemon: could not write worktree ownership marker for {short}: {e}"
                        );
                    }
                }
                roster.workers.insert(short.clone(), record);
                let publication = daemon_roster::write_roster_with_lock_held(runtime_dir, roster)
                    .and_then(|()| {
                        let expected = roster.workers.get(&short).ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "spawned worker vanished from the in-memory roster",
                            )
                        })?;
                        if roster_worker_publication_matches_disk(runtime_dir, &short, expected) {
                            Ok(())
                        } else {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::WriteZero,
                                "spawned worker roster publication was not durable",
                            ))
                        }
                    });
                if let Err(e) = publication {
                    tracing::warn!(
                        "lingxi-cli daemon: could not persist live endpoint for {short}: {e}"
                    );
                    roster.workers.remove(&short);
                    let spawned_identity = ObservedProcessIdentity {
                        pid: child_pid,
                        proc_start: proc_start.clone(),
                    };
                    // This PID came directly from our spawn call, so it is
                    // safe to signal even if the host cannot read its start
                    // identity. Confirmation remains identity/PTY-aware.
                    kill_worker(child_pid, true);
                    let mut terminator = SystemStallTerminator;
                    let stopped = terminate_observed_worker(
                        runtime_dir,
                        &short,
                        &spawned_identity,
                        proc_probe,
                        &mut terminator,
                        StallTerminationMode::HardOnly,
                    );
                    let expected = agents_registry::JobStateMatch {
                        state: "working",
                        phase: Some(crate::commands::respawn::PHASE_LAUNCHING),
                        worker_pid: None,
                        worker_proc_start: None,
                        worker_generation: Some(&worker_generation),
                        claim_token: Some(&claim_token),
                        claim_owner: Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH),
                        claim_created_at: None,
                        claim_lease_ms: Some(crate::commands::respawn::CLAIM_LEASE_MS),
                    };
                    if stopped {
                        let _ = agents_registry::patch_job_state_if_matches(
                            runtime_dir,
                            &short,
                            expected,
                            agents_registry::JobStatePatch {
                                state: Some("working"),
                                tempo: None,
                                cwd: None,
                                detail: None,
                                worker_pid: Some(None),
                                worker_proc_start: Some(None),
                                phase: Some(Some(crate::commands::respawn::PHASE_QUEUED)),
                                worker_generation: Some(Some(&worker_generation)),
                                claim_token: Some(None),
                                claim_owner: Some(None),
                                claim_created_at: Some(None),
                                claim_lease_ms: Some(None),
                            },
                        );
                        claimed.remove(&short);
                    } else {
                        let _ = agents_registry::patch_job_state_if_matches(
                            runtime_dir,
                            &short,
                            expected,
                            agents_registry::JobStatePatch {
                                worker_pid: Some(Some(child_pid)),
                                worker_proc_start: Some(proc_start.as_deref()),
                                ..Default::default()
                            },
                        );
                        claimed.insert(short.clone());
                    }
                    continue;
                }
                claimed.insert(short);
            }
            Err(e) => {
                // Best-effort: the durable job stays "working"; the next
                // heartbeat retries the spawn.
                tracing::warn!("lingxi-cli daemon: could not spawn worker for {short}: {e}");
                let _ = agents_registry::patch_job_state_if_matches(
                    runtime_dir,
                    &short,
                    agents_registry::JobStateMatch {
                        state: "working",
                        phase: Some(crate::commands::respawn::PHASE_LAUNCHING),
                        worker_pid: None,
                        worker_proc_start: None,
                        worker_generation: Some(&worker_generation),
                        claim_token: Some(&claim_token),
                        claim_owner: Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH),
                        claim_created_at: None,
                        claim_lease_ms: Some(crate::commands::respawn::CLAIM_LEASE_MS),
                    },
                    agents_registry::JobStatePatch {
                        state: Some("working"),
                        tempo: None,
                        cwd: None,
                        detail: None,
                        worker_pid: Some(None),
                        worker_proc_start: Some(None),
                        phase: Some(Some(crate::commands::respawn::PHASE_QUEUED)),
                        worker_generation: Some(Some(&worker_generation)),
                        claim_token: Some(None),
                        claim_owner: Some(None),
                        claim_created_at: Some(None),
                        claim_lease_ms: Some(None),
                    },
                );
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
    runtime_dir: &Path,
    previous_dispatch: Option<Dispatch>,
    attach_sock: String,
    attach_auth: String,
) -> WorkerRecord {
    let session_id = job.session_id.clone().unwrap_or_default();
    let launch_spec = crate::background_launch::read_launch_spec(runtime_dir, short).ok();
    let previous_worktree = previous_dispatch
        .as_ref()
        .and_then(|dispatch| dispatch.worktree.clone());
    let cwd = launch_spec
        .as_ref()
        .map(|spec| spec.cwd.clone())
        .or_else(|| job.cwd.clone())
        .unwrap_or_default();
    let worktree_path = launch_spec
        .as_ref()
        .and_then(|spec| spec.worktree_path.clone())
        .or_else(|| {
            previous_worktree
                .as_ref()
                .map(|worktree| worktree.path.clone())
        });
    let canonical_launch = launch_spec
        .as_ref()
        .map(|spec| match spec.launch {
            crate::background_launch::BackgroundLaunchKind::Fresh => Launch::Prompt {
                args: vec!["--background".to_string()],
            },
            crate::background_launch::BackgroundLaunchKind::Resume
            | crate::background_launch::BackgroundLaunchKind::Fork => Launch::Resume {
                session_id: spec.session_id.clone(),
                transcript_path: Some(spec.transcript_path.clone()),
                fork: spec.launch == crate::background_launch::BackgroundLaunchKind::Fork,
                flag_args: Vec::new(),
            },
        })
        .or_else(|| {
            previous_dispatch
                .as_ref()
                .map(|dispatch| dispatch.launch.clone())
        })
        .unwrap_or_else(|| Launch::Prompt {
            args: vec!["--background".to_string()],
        });
    let canonical_launch_spec = launch_spec
        .as_ref()
        .map(|spec| spec.reference(runtime_dir))
        .or_else(|| {
            previous_dispatch
                .as_ref()
                .and_then(|dispatch| dispatch.launch_spec.clone())
        });
    let canonical_worktree = worktree_path.as_ref().map(|path| {
        previous_worktree
            .filter(|worktree| worktree.path == *path)
            .unwrap_or_else(|| crate::daemon_roster::Worktree {
                path: path.clone(),
                ownership_token: launch_spec
                    .as_ref()
                    .and_then(|spec| spec.worktree_ownership_token.clone())
                    .or_else(|| {
                        previous_dispatch.as_ref().and_then(|dispatch| {
                            dispatch
                                .worktree
                                .as_ref()
                                .filter(|worktree| worktree.path == *path)
                                .map(|worktree| worktree.ownership_token.clone())
                        })
                    })
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            })
    });
    let (cols, rows) = launch_spec
        .as_ref()
        .map(|spec| {
            (
                Some(u32::from(spec.terminal.cols)),
                Some(u32::from(spec.terminal.rows)),
            )
        })
        .unwrap_or_else(|| {
            previous_dispatch
                .as_ref()
                .map(|dispatch| (dispatch.cols, dispatch.rows))
                .unwrap_or((None, None))
        });
    let now = now_millis();
    // P1-12: the roster's `attachStallRespawns` is sourced from the DURABLE
    // counter, so it survives the respawn that increments it. Previously the
    // field was written `None` everywhere and read nowhere — dead.
    let stall_respawns = read_respawn_count(runtime_dir, short);
    let mut dispatch = previous_dispatch.unwrap_or_else(|| Dispatch {
        proto: PROTO,
        short: short.to_string(),
        nonce: None,
        session_id: session_id.clone(),
        created_at: now,
        source: DispatchSource::Shell,
        cwd: cwd.clone(),
        launch: canonical_launch.clone(),
        launch_spec: canonical_launch_spec.clone(),
        env: BTreeMap::new(),
        reattach_env: None,
        worktree: canonical_worktree.clone(),
        isolation: if canonical_worktree.is_some() {
            Isolation::Worktree
        } else {
            Isolation::None
        },
        respawn_flags: Vec::new(),
        attach_stall_respawns: (stall_respawns > 0).then_some(stall_respawns),
        agent: None,
        routine: None,
        seed: job.intent.clone().map(|intent| Seed { intent, name: None }),
        cols,
        rows,
    });
    // An ADOPTED dispatch predates this respawn, so its counter is stale;
    // the durable file is the source of truth either way.
    dispatch.attach_stall_respawns = (stall_respawns > 0).then_some(stall_respawns);
    // Canonicalize the fresh/resume/fork launch and worktree from the owner-only
    // launch spec. The foreground handoff record normally disappears when its
    // short-lived PID exits before daemon adoption, so relying on that record
    // would silently downgrade resume/fork jobs to a generic prompt launch.
    dispatch.proto = PROTO;
    dispatch.short = short.to_string();
    dispatch.session_id.clone_from(&session_id);
    dispatch.cwd.clone_from(&cwd);
    dispatch.launch = canonical_launch;
    dispatch.launch_spec = canonical_launch_spec;
    dispatch.worktree = canonical_worktree;
    dispatch.isolation = if dispatch.worktree.is_some() {
        Isolation::Worktree
    } else {
        Isolation::None
    };
    dispatch.cols = cols;
    dispatch.rows = rows;
    dispatch.env.clear();
    // Keep only the non-secret generation attestation required to prove that
    // roster ownership and state.json still name the same writer. Child-only
    // provider credentials remain exclusively in the owner-mode launch spec.
    if let Some(generation) = job.worker_generation.as_deref() {
        dispatch.env.insert(
            crate::commands::respawn::BG_WORKER_GENERATION_ENV.to_string(),
            generation.to_string(),
        );
    }

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
        worktree_path,
        dispatch,
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
    use std::sync::{Arc, Mutex};

    /// Serializes every test that drives `run_supervisor`.
    ///
    /// `tracing` caches per-callsite INTEREST process-globally, and
    /// `with_default` (unlike `set_global_default`) does not rebuild it. So a
    /// test running `run_supervisor` with NO subscriber caches
    /// `emit_worker_vanished` as "never", and the one test that DOES install a
    /// subscriber then captures nothing.
    ///
    /// A `rebuild_interest_cache()` inside the capturing test alone was not
    /// enough — the flake came straight back, because a concurrent
    /// `run_supervisor` on another thread re-poisons the cache after the
    /// rebuild. The guard must cover EVERY user of the shared callsite, not
    /// just the reader; the same invariant the shared web caches needed.
    ///
    /// Poison-tolerant: the payload is `()`, so a panicking test leaves nothing
    /// to corrupt and must not wedge the other fifteen.
    static SUPERVISOR_TRACING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn supervisor_lock() -> std::sync::MutexGuard<'static, ()> {
        SUPERVISOR_TRACING_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

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

    #[derive(Debug, Clone)]
    struct ProcTransition {
        after_sleeps: usize,
        pid: i32,
        alive: bool,
        start: Option<String>,
    }

    #[derive(Debug, Default)]
    struct ScriptedProcState {
        alive: HashMap<i32, bool>,
        start: HashMap<i32, String>,
        transitions: Vec<ProcTransition>,
        worker_signals: Vec<(i32, bool)>,
        pty_signals: Vec<(i32, bool)>,
        sleeps: Vec<u64>,
    }

    #[derive(Clone, Debug, Default)]
    struct ScriptedProc {
        state: Arc<Mutex<ScriptedProcState>>,
    }

    impl ScriptedProc {
        fn set_process(&self, pid: i32, start: &str) {
            let mut state = self.state.lock().unwrap();
            state.alive.insert(pid, true);
            state.start.insert(pid, start.to_string());
        }

        fn schedule_exit(&self, pid: i32, after_sleeps: usize) {
            self.schedule_state(pid, after_sleeps, false, None);
        }

        fn schedule_state(&self, pid: i32, after_sleeps: usize, alive: bool, start: Option<&str>) {
            self.state.lock().unwrap().transitions.push(ProcTransition {
                after_sleeps,
                pid,
                alive,
                start: start.map(str::to_string),
            });
        }

        fn worker_signals(&self) -> Vec<(i32, bool)> {
            self.state.lock().unwrap().worker_signals.clone()
        }

        fn pty_signals(&self) -> Vec<(i32, bool)> {
            self.state.lock().unwrap().pty_signals.clone()
        }

        fn sleep_count(&self) -> usize {
            self.state.lock().unwrap().sleeps.len()
        }

        fn apply_due_transitions(state: &mut ScriptedProcState) {
            let sleep_count = state.sleeps.len();
            let mut pending = Vec::new();
            for transition in state.transitions.drain(..) {
                if transition.after_sleeps <= sleep_count {
                    state.alive.insert(transition.pid, transition.alive);
                    match transition.start {
                        Some(start) => {
                            state.start.insert(transition.pid, start);
                        }
                        None => {
                            state.start.remove(&transition.pid);
                        }
                    }
                } else {
                    pending.push(transition);
                }
            }
            state.transitions = pending;
        }
    }

    impl ProcProbe for ScriptedProc {
        fn is_alive(&self, pid: i32) -> bool {
            self.state
                .lock()
                .unwrap()
                .alive
                .get(&pid)
                .copied()
                .unwrap_or(false)
        }

        fn start_time(&self, pid: i32) -> Option<String> {
            self.state.lock().unwrap().start.get(&pid).cloned()
        }
    }

    struct FakeStallTerminator {
        state: Arc<Mutex<ScriptedProcState>>,
    }

    impl FakeStallTerminator {
        fn new(proc: &ScriptedProc) -> Self {
            Self {
                state: Arc::clone(&proc.state),
            }
        }
    }

    impl StallTerminator for FakeStallTerminator {
        fn signal_worker(&mut self, pid: i32, hard: bool) {
            self.state.lock().unwrap().worker_signals.push((pid, hard));
        }

        fn signal_pty_tree<PP: ProcProbe>(
            &mut self,
            runtime: &crate::background_launch::BackgroundPtyRuntime,
            _probe: &PP,
            hard: bool,
        ) {
            if let Ok(child_pid) = i32::try_from(runtime.child_pid) {
                self.state
                    .lock()
                    .unwrap()
                    .pty_signals
                    .push((child_pid, hard));
            }
        }

        fn sleep(&mut self, ms: u64) {
            let mut state = self.state.lock().unwrap();
            state.sleeps.push(ms);
            ScriptedProc::apply_due_transitions(&mut state);
        }
    }

    fn no_sleep() -> impl FnMut(u64) {
        |_| {}
    }

    /// A `tracing` layer that records the `event = "..."` field of every emitted
    /// event, so a test can assert which `tengu_*` telemetry events actually
    /// fired (or did not).
    #[derive(Clone, Default)]
    struct EventCapture {
        events: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for EventCapture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct V<'a>(&'a mut Option<String>);
            impl tracing::field::Visit for V<'_> {
                fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                    if field.name() == "event" {
                        *self.0 = Some(value.to_string());
                    }
                }
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "event" && self.0.is_none() {
                        *self.0 = Some(format!("{value:?}"));
                    }
                }
            }
            let mut name = None;
            event.record(&mut V(&mut name));
            if let Some(n) = name {
                self.events.lock().unwrap().push(n);
            }
        }
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

    struct DeleteDuringSpawn {
        runtime_dir: PathBuf,
        called: bool,
    }

    impl WorkerSpawner for DeleteDuringSpawn {
        fn spawn_worker(
            &mut self,
            short: &str,
            _env: &BTreeMap<String, String>,
        ) -> std::io::Result<i32> {
            self.called = true;
            let current = agents_registry::read_job(&self.runtime_dir, short).unwrap();
            let updated = agents_registry::patch_job_state_if_matches(
                &self.runtime_dir,
                short,
                agents_registry::JobStateMatch {
                    state: &current.state,
                    phase: current.phase.as_deref(),
                    worker_pid: current.worker_pid,
                    worker_proc_start: current.worker_proc_start.as_deref(),
                    worker_generation: current.worker_generation.as_deref(),
                    claim_token: current.claim_token.as_deref(),
                    claim_owner: current.claim_owner.as_deref(),
                    claim_created_at: current.claim_created_at,
                    claim_lease_ms: current.claim_lease_ms,
                },
                agents_registry::JobStatePatch {
                    state: Some("working"),
                    tempo: None,
                    cwd: None,
                    detail: None,
                    worker_pid: Some(None),
                    worker_proc_start: Some(None),
                    phase: Some(Some(crate::commands::respawn::PHASE_DELETING)),
                    worker_generation: None,
                    claim_token: Some(Some("delete-during-spawn")),
                    claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_DELETE)),
                    claim_created_at: Some(Some(10)),
                    claim_lease_ms: Some(Some(crate::commands::respawn::CLAIM_LEASE_MS)),
                },
            )?;
            assert!(updated);
            Ok(99_001)
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
            worker_proc_start: None,
            phase: Some("queued"),
            worker_generation: None,
            claim_token: None,
            claim_owner: None,
            claim_created_at: None,
            claim_lease_ms: None,
        };
        agents_registry::write_job_state(home, short, &job).unwrap();
    }

    fn seed_resumable_launch_spec(home: &Path, short: &str) -> PathBuf {
        use crate::background_launch::{
            self, BackgroundLaunchKind, BackgroundLaunchOptions, BackgroundLaunchSpec,
            TerminalSize, LAUNCH_SPEC_VERSION,
        };

        let session_id = "11111111-1111-1111-1111-111111111111";
        let cwd = home.display().to_string();
        let transcript = session::jsonl::path::session_path(home, &cwd, session_id);
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
        )
        .unwrap();
        background_launch::write_launch_spec(
            home,
            short,
            &BackgroundLaunchSpec {
                schema_version: LAUNCH_SPEC_VERSION,
                short: short.to_string(),
                created_at: 1_700_000_000_000,
                preflight_approved: true,
                launch: BackgroundLaunchKind::Fresh,
                session_id: session_id.to_string(),
                transcript_path: transcript.display().to_string(),
                cwd: cwd.clone(),
                origin_cwd: cwd,
                worktree_path: None,
                worktree_ownership_token: None,
                initial_prompt: Some("do the thing".to_string()),
                shell_handoff: Vec::new(),
                handoff: None,
                options: BackgroundLaunchOptions::default(),
                env: BTreeMap::new(),
                terminal: TerminalSize::default(),
            },
        )
        .unwrap();
        transcript
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
                launch_spec: None,
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

    fn write_test_pty_runtime(
        runtime_dir: &Path,
        short: &str,
        worker_pid: i32,
        child_pid: u32,
        child_proc_start: &str,
    ) {
        crate::background_launch::write_pty_runtime(
            runtime_dir,
            short,
            &crate::background_launch::BackgroundPtyRuntime {
                schema_version: 1,
                short: short.to_string(),
                worker_pid,
                child_pid,
                child_proc_start: Some(child_proc_start.to_string()),
                process_group_id: Some(child_pid),
            },
        )
        .unwrap();
    }

    fn write_legacy_test_pty_runtime(
        runtime_dir: &Path,
        short: &str,
        worker_pid: i32,
        child_pid: u32,
    ) {
        let path = crate::background_launch::pty_runtime_path(runtime_dir, short);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::json!({
                "schemaVersion": 1,
                "short": short,
                "workerPid": worker_pid,
                "childPid": child_pid,
                "processGroupId": child_pid,
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn fresh_acquire_adopts_seeds_roster_and_bumps_updated_at() {
        let _supervisor_guard = supervisor_lock();
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
    fn roster_publication_requires_exact_read_back_identity() {
        let dir = tmpdir();
        let expected = worker(4_242);
        assert!(!roster_worker_publication_matches_disk(
            &dir, "feed0001", &expected
        ));

        let mut roster = empty_roster(1);
        roster
            .workers
            .insert("feed0001".to_string(), expected.clone());
        daemon_roster::write_roster(&dir, &roster).unwrap();
        assert!(roster_worker_publication_matches_disk(
            &dir, "feed0001", &expected
        ));

        let mut wrong_generation = expected;
        wrong_generation.pid = 4_243;
        assert!(!roster_worker_publication_matches_disk(
            &dir,
            "feed0001",
            &wrong_generation
        ));
    }

    #[test]
    fn heartbeat_loop_runs_once_then_stops() {
        let _supervisor_guard = supervisor_lock();
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
        let _supervisor_guard = supervisor_lock();
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
        let _supervisor_guard = supervisor_lock();
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
        let _supervisor_guard = supervisor_lock();
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
        // The worker publishes its own pid after it validates the launch claim.
        let job = agents_registry::read_job(&dir, "bc7c6b33").unwrap();
        assert_eq!(job.state, "working");
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_LAUNCHING)
        );
        assert_eq!(job.worker_pid, None);
        assert!(job.worker_generation.is_some());
        assert!(job.claim_token.is_some());
        assert!(!agents_registry::job_is_terminal(&job));
        // …and a live-worker roster record exists carrying that pid.
        let roster = read_roster(&dir, 0, false).into_roster();
        let rec = roster.workers.get("bc7c6b33").expect("worker record");
        assert_eq!(rec.pid, 90_000);
    }

    #[test]
    fn deleting_phase_job_is_not_respawned_by_heartbeat() {
        let _supervisor_guard = supervisor_lock();
        let dir = tmpdir();
        seed_working_job(&dir, "dead0001");
        agents_registry::patch_job_state_if_matches(
            &dir,
            "dead0001",
            agents_registry::JobStateMatch {
                state: "working",
                phase: Some(crate::commands::respawn::PHASE_QUEUED),
                worker_pid: None,
                worker_proc_start: None,
                worker_generation: None,
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                tempo: None,
                cwd: None,
                detail: None,
                worker_pid: Some(None),
                worker_proc_start: Some(None),
                phase: Some(Some(crate::commands::respawn::PHASE_DELETING)),
                worker_generation: Some(Some("gen-delete")),
                claim_token: Some(Some("claim-delete")),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_DELETE)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(crate::commands::respawn::CLAIM_LEASE_MS)),
            },
        )
        .unwrap();

        let proc = FakeProc {
            alive: HashMap::new(),
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
        assert!(spawner.spawned.is_empty());
        let deleting = agents_registry::read_job(&dir, "dead0001").unwrap();
        assert!(!stop_background_job(&dir, "dead0001", &deleting));
        let preserved = agents_registry::read_job(&dir, "dead0001").unwrap();
        assert_eq!(preserved.state, "working");
        assert_eq!(
            preserved.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_DELETING)
        );
        assert_eq!(preserved.claim_token.as_deref(), Some("claim-delete"));
    }

    #[test]
    fn delete_transition_during_spawn_revokes_roster_publication() {
        let dir = tmpdir();
        seed_working_job(&dir, "dead0002");
        let mut roster = empty_roster(1);
        let mut claimed = HashSet::new();
        let proc = FakeProc {
            alive: HashMap::new(),
            start: HashMap::new(),
        };
        let mut spawner = DeleteDuringSpawn {
            runtime_dir: dir.clone(),
            called: false,
        };

        spawn_pending_workers(
            &dir,
            &mut roster,
            "0.0.0",
            &proc,
            &mut spawner,
            &mut claimed,
        );

        assert!(spawner.called);
        assert!(!roster.workers.contains_key("dead0002"));
        let job = agents_registry::read_job(&dir, "dead0002").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_DELETING)
        );
        assert_eq!(job.claim_token.as_deref(), Some("delete-during-spawn"));
    }

    #[test]
    fn delete_claim_quiesces_published_launch_and_remains_absorbing() {
        let dir = tmpdir();
        seed_working_job(&dir, "dead0003");
        let queued = agents_registry::read_job(&dir, "dead0003").unwrap();
        assert!(agents_registry::patch_job_state_if_matches(
            &dir,
            "dead0003",
            agents_registry::JobStateMatch {
                state: &queued.state,
                phase: queued.phase.as_deref(),
                worker_pid: queued.worker_pid,
                worker_proc_start: queued.worker_proc_start.as_deref(),
                worker_generation: queued.worker_generation.as_deref(),
                claim_token: queued.claim_token.as_deref(),
                claim_owner: queued.claim_owner.as_deref(),
                claim_created_at: queued.claim_created_at,
                claim_lease_ms: queued.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                phase: Some(Some(crate::commands::respawn::PHASE_LAUNCHING)),
                worker_generation: Some(Some("generation-delete-race")),
                claim_token: Some(Some("launch-before-delete")),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(crate::commands::respawn::CLAIM_LEASE_MS)),
                ..Default::default()
            },
        )
        .unwrap());
        let launching = agents_registry::read_job(&dir, "dead0003").unwrap();
        let mut record = worker(44_503);
        record.proc_start = Some("START-44503".to_string());
        record.dispatch.short = "dead0003".to_string();
        record.dispatch.env.insert(
            crate::commands::respawn::BG_WORKER_GENERATION_ENV.to_string(),
            "generation-delete-race".to_string(),
        );
        let mut roster = empty_roster(1);
        roster.workers.insert("dead0003".to_string(), record);
        daemon_roster::write_roster(&dir, &roster).unwrap();

        let proc = ScriptedProc::default();
        proc.set_process(44_503, "START-44503");
        proc.schedule_exit(44_503, 1);
        let mut terminator = FakeStallTerminator::new(&proc);
        let claim = claim_and_quiesce_background_job_for_delete_with(
            &dir,
            "dead0003",
            &launching,
            &proc,
            &mut terminator,
            100,
        )
        .unwrap();

        assert_eq!(proc.worker_signals(), vec![(44_503, false)]);
        assert!(!read_roster(&dir, 0, false)
            .into_roster()
            .workers
            .contains_key("dead0003"));
        let job = agents_registry::read_job(&dir, "dead0003").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_DELETING)
        );
        assert_eq!(job.claim_token.as_deref(), Some(claim.token.as_str()));
        assert_eq!(
            job.worker_generation.as_deref(),
            Some("generation-delete-race")
        );
        assert_eq!(job.worker_pid, None);
    }

    #[test]
    fn delete_claim_quiesces_distinct_roster_and_job_identities() {
        let dir = tmpdir();
        seed_working_job(&dir, "dead0005");
        let queued = agents_registry::read_job(&dir, "dead0005").unwrap();
        assert!(agents_registry::patch_job_state_if_matches(
            &dir,
            "dead0005",
            agents_registry::JobStateMatch {
                state: &queued.state,
                phase: queued.phase.as_deref(),
                worker_pid: queued.worker_pid,
                worker_proc_start: queued.worker_proc_start.as_deref(),
                worker_generation: queued.worker_generation.as_deref(),
                claim_token: queued.claim_token.as_deref(),
                claim_owner: queued.claim_owner.as_deref(),
                claim_created_at: queued.claim_created_at,
                claim_lease_ms: queued.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                worker_pid: Some(Some(44_505)),
                worker_proc_start: Some(Some("JOB-START-44505")),
                phase: Some(Some(crate::commands::respawn::PHASE_RUNNING)),
                worker_generation: Some(Some("generation-split-brain")),
                ..Default::default()
            },
        )
        .unwrap());
        let running = agents_registry::read_job(&dir, "dead0005").unwrap();

        let mut record = worker(44_506);
        record.proc_start = Some("ROSTER-START-44506".to_string());
        record.dispatch.short = "dead0005".to_string();
        record.dispatch.env.insert(
            crate::commands::respawn::BG_WORKER_GENERATION_ENV.to_string(),
            "generation-split-brain".to_string(),
        );
        let mut roster = empty_roster(1);
        roster.workers.insert("dead0005".to_string(), record);
        daemon_roster::write_roster(&dir, &roster).unwrap();

        let proc = ScriptedProc::default();
        proc.set_process(44_506, "ROSTER-START-44506");
        proc.set_process(44_505, "JOB-START-44505");
        proc.schedule_exit(44_506, 1);
        proc.schedule_exit(44_505, 2);
        let mut terminator = FakeStallTerminator::new(&proc);
        let claim = claim_and_quiesce_background_job_for_delete_with(
            &dir,
            "dead0005",
            &running,
            &proc,
            &mut terminator,
            100,
        )
        .unwrap();

        assert_eq!(
            proc.worker_signals(),
            vec![(44_506, false), (44_505, false)]
        );
        assert!(!read_roster(&dir, 0, false)
            .into_roster()
            .workers
            .contains_key("dead0005"));
        assert_eq!(
            claim.job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_DELETING)
        );
        assert_eq!(claim.job.worker_pid, None);
    }

    #[test]
    fn terminal_job_is_also_claimed_before_delete() {
        let dir = tmpdir();
        seed_working_job(&dir, "dead0004");
        agents_registry::update_job_state(&dir, "dead0004", "completed", None).unwrap();
        let terminal = agents_registry::read_job(&dir, "dead0004").unwrap();
        let mut terminator = FakeStallTerminator::new(&ScriptedProc::default());
        let claim = claim_and_quiesce_background_job_for_delete_with(
            &dir,
            "dead0004",
            &terminal,
            &stall_probe(),
            &mut terminator,
            100,
        )
        .unwrap();

        assert_eq!(claim.job.state, "completed");
        assert_eq!(
            claim.job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_DELETING)
        );
        assert_eq!(claim.job.claim_token.as_deref(), Some(claim.token.as_str()));
    }

    #[test]
    fn stale_creating_job_with_launch_spec_becomes_queued() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0001");
        seed_resumable_launch_spec(&dir, "5a1e0001");
        agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0001",
            agents_registry::JobStateMatch {
                state: "working",
                phase: Some("queued"),
                worker_pid: None,
                worker_proc_start: None,
                worker_generation: None,
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                tempo: None,
                cwd: None,
                detail: None,
                worker_pid: Some(None),
                worker_proc_start: Some(None),
                phase: Some(Some(crate::commands::respawn::PHASE_CREATING)),
                worker_generation: Some(Some("gen-create")),
                claim_token: Some(None),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_DISPATCH)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(10)),
            },
        )
        .unwrap();

        let mut roster = empty_roster(1);
        let mut claimed: HashSet<String> = ["5a1e0001".to_string()].into_iter().collect();
        recover_stale_job_claims(&dir, &mut roster, &stall_probe(), &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0001").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_QUEUED)
        );
        assert_eq!(job.claim_owner, None);
        assert_eq!(job.claim_created_at, None);
        assert_eq!(job.claim_lease_ms, None);
        assert!(!claimed.contains("5a1e0001"));
    }

    #[test]
    fn legacy_creating_claim_with_live_pty_never_requeues() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0011");
        seed_resumable_launch_spec(&dir, "5a1e0011");
        let queued = agents_registry::read_job(&dir, "5a1e0011").unwrap();
        assert!(agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0011",
            agents_registry::JobStateMatch {
                state: &queued.state,
                phase: queued.phase.as_deref(),
                worker_pid: queued.worker_pid,
                worker_proc_start: queued.worker_proc_start.as_deref(),
                worker_generation: queued.worker_generation.as_deref(),
                claim_token: queued.claim_token.as_deref(),
                claim_owner: queued.claim_owner.as_deref(),
                claim_created_at: queued.claim_created_at,
                claim_lease_ms: queued.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                phase: Some(Some(crate::commands::respawn::PHASE_CREATING)),
                worker_generation: Some(Some("legacy-create-generation")),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_DISPATCH)),
                claim_created_at: Some(None),
                claim_lease_ms: Some(None),
                ..Default::default()
            },
        )
        .unwrap());
        write_test_pty_runtime(&dir, "5a1e0011", 44_511, 90_011, "PTY-START-90011");
        let proc = FakeProc {
            alive: HashMap::from([(90_011, true)]),
            start: HashMap::from([(90_011, "PTY-START-90011".to_string())]),
        };
        let mut roster = empty_roster(1);
        let mut claimed = HashSet::new();

        recover_stale_job_claims(&dir, &mut roster, &proc, &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0011").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_CREATING)
        );
        assert_eq!(job.claim_created_at, None);
        assert!(crate::background_launch::pty_runtime_path(&dir, "5a1e0011").exists());
    }

    #[test]
    fn stale_launching_job_without_live_record_requeues_and_bumps_budget() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0002");
        agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0002",
            agents_registry::JobStateMatch {
                state: "working",
                phase: Some("queued"),
                worker_pid: None,
                worker_proc_start: None,
                worker_generation: None,
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                tempo: None,
                cwd: None,
                detail: None,
                worker_pid: Some(None),
                worker_proc_start: Some(None),
                phase: Some(Some(crate::commands::respawn::PHASE_LAUNCHING)),
                worker_generation: Some(Some("gen-launch")),
                claim_token: Some(Some("claim-launch")),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(10)),
            },
        )
        .unwrap();

        let mut roster = empty_roster(1);
        let mut claimed: HashSet<String> = ["5a1e0002".to_string()].into_iter().collect();
        recover_stale_job_claims(&dir, &mut roster, &stall_probe(), &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0002").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_QUEUED)
        );
        assert_eq!(read_respawn_count(&dir, "5a1e0002"), 1);
        assert!(!claimed.contains("5a1e0002"));
    }

    #[test]
    fn stale_launching_live_unverifiable_pid_never_requeues() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0007");
        let queued = agents_registry::read_job(&dir, "5a1e0007").unwrap();
        assert!(agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0007",
            agents_registry::JobStateMatch {
                state: &queued.state,
                phase: queued.phase.as_deref(),
                worker_pid: queued.worker_pid,
                worker_proc_start: queued.worker_proc_start.as_deref(),
                worker_generation: queued.worker_generation.as_deref(),
                claim_token: queued.claim_token.as_deref(),
                claim_owner: queued.claim_owner.as_deref(),
                claim_created_at: queued.claim_created_at,
                claim_lease_ms: queued.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                phase: Some(Some(crate::commands::respawn::PHASE_LAUNCHING)),
                worker_generation: Some(Some("gen-unverified")),
                claim_token: Some(Some("claim-unverified")),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(10)),
                ..Default::default()
            },
        )
        .unwrap());
        let mut record = worker(44_507);
        record.proc_start = Some("EXPECTED-START".to_string());
        record.dispatch.env.insert(
            crate::commands::respawn::BG_WORKER_GENERATION_ENV.to_string(),
            "gen-unverified".to_string(),
        );
        let mut roster = empty_roster(1);
        roster.workers.insert("5a1e0007".to_string(), record);
        let proc = FakeProc {
            alive: HashMap::from([(44_507, true)]),
            start: HashMap::new(),
        };
        let mut claimed = HashSet::new();

        recover_stale_job_claims(&dir, &mut roster, &proc, &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0007").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_LAUNCHING)
        );
        assert_eq!(job.claim_token.as_deref(), Some("claim-unverified"));
        assert!(roster.workers.contains_key("5a1e0007"));
        assert_eq!(read_respawn_count(&dir, "5a1e0007"), 0);
    }

    #[test]
    fn stale_launching_live_pty_without_roster_never_requeues() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0009");
        let queued = agents_registry::read_job(&dir, "5a1e0009").unwrap();
        assert!(agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0009",
            agents_registry::JobStateMatch {
                state: &queued.state,
                phase: queued.phase.as_deref(),
                worker_pid: queued.worker_pid,
                worker_proc_start: queued.worker_proc_start.as_deref(),
                worker_generation: queued.worker_generation.as_deref(),
                claim_token: queued.claim_token.as_deref(),
                claim_owner: queued.claim_owner.as_deref(),
                claim_created_at: queued.claim_created_at,
                claim_lease_ms: queued.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                phase: Some(Some(crate::commands::respawn::PHASE_LAUNCHING)),
                worker_generation: Some(Some("gen-live-pty")),
                claim_token: Some(Some("claim-live-pty")),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(10)),
                ..Default::default()
            },
        )
        .unwrap());
        write_test_pty_runtime(&dir, "5a1e0009", 44_509, 90_009, "PTY-START-90009");
        let proc = FakeProc {
            alive: HashMap::from([(90_009, true)]),
            start: HashMap::from([(90_009, "PTY-START-90009".to_string())]),
        };
        let mut roster = empty_roster(1);
        let mut claimed = HashSet::new();

        recover_stale_job_claims(&dir, &mut roster, &proc, &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0009").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_LAUNCHING)
        );
        assert_eq!(job.claim_token.as_deref(), Some("claim-live-pty"));
        assert!(crate::background_launch::pty_runtime_path(&dir, "5a1e0009").exists());
        assert_eq!(read_respawn_count(&dir, "5a1e0009"), 0);
    }

    #[test]
    fn stale_launching_unverifiable_job_pid_without_roster_never_requeues() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0010");
        let queued = agents_registry::read_job(&dir, "5a1e0010").unwrap();
        assert!(agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0010",
            agents_registry::JobStateMatch {
                state: &queued.state,
                phase: queued.phase.as_deref(),
                worker_pid: queued.worker_pid,
                worker_proc_start: queued.worker_proc_start.as_deref(),
                worker_generation: queued.worker_generation.as_deref(),
                claim_token: queued.claim_token.as_deref(),
                claim_owner: queued.claim_owner.as_deref(),
                claim_created_at: queued.claim_created_at,
                claim_lease_ms: queued.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                worker_pid: Some(Some(44_510)),
                worker_proc_start: Some(Some("EXPECTED-START-44510")),
                phase: Some(Some(crate::commands::respawn::PHASE_LAUNCHING)),
                worker_generation: Some(Some("gen-job-unverified")),
                claim_token: Some(Some("claim-job-unverified")),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(10)),
                ..Default::default()
            },
        )
        .unwrap());
        let proc = FakeProc {
            alive: HashMap::from([(44_510, true)]),
            start: HashMap::new(),
        };
        let mut roster = empty_roster(1);
        let mut claimed = HashSet::new();

        recover_stale_job_claims(&dir, &mut roster, &proc, &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0010").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_LAUNCHING)
        );
        assert_eq!(job.claim_token.as_deref(), Some("claim-job-unverified"));
        assert_eq!(read_respawn_count(&dir, "5a1e0010"), 0);
    }

    #[test]
    fn legacy_launch_claim_without_timestamp_is_recovered() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0008");
        let queued = agents_registry::read_job(&dir, "5a1e0008").unwrap();
        assert!(agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0008",
            agents_registry::JobStateMatch {
                state: &queued.state,
                phase: queued.phase.as_deref(),
                worker_pid: queued.worker_pid,
                worker_proc_start: queued.worker_proc_start.as_deref(),
                worker_generation: queued.worker_generation.as_deref(),
                claim_token: queued.claim_token.as_deref(),
                claim_owner: queued.claim_owner.as_deref(),
                claim_created_at: queued.claim_created_at,
                claim_lease_ms: queued.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                phase: Some(Some(crate::commands::respawn::PHASE_LAUNCHING)),
                worker_generation: Some(Some("legacy-generation")),
                claim_token: Some(Some("legacy-claim")),
                claim_owner: Some(None),
                claim_created_at: Some(None),
                claim_lease_ms: Some(None),
                ..Default::default()
            },
        )
        .unwrap());
        let mut roster = empty_roster(1);
        let mut claimed: HashSet<String> = ["5a1e0008".to_string()].into_iter().collect();

        recover_stale_job_claims(&dir, &mut roster, &stall_probe(), &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0008").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_QUEUED)
        );
        assert_eq!(job.claim_token, None);
        assert!(!claimed.contains("5a1e0008"));
    }

    #[test]
    fn stale_restarting_job_with_resumable_transcript_requeues() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0003");
        seed_resumable_launch_spec(&dir, "5a1e0003");
        agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0003",
            agents_registry::JobStateMatch {
                state: "working",
                phase: Some("queued"),
                worker_pid: None,
                worker_proc_start: None,
                worker_generation: None,
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                tempo: None,
                cwd: None,
                detail: None,
                worker_pid: Some(None),
                worker_proc_start: Some(None),
                phase: Some(Some(crate::commands::respawn::PHASE_RESTARTING)),
                worker_generation: Some(Some("gen-restart")),
                claim_token: Some(Some("claim-restart")),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_RESTART)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(10)),
            },
        )
        .unwrap();

        let mut roster = empty_roster(1);
        let mut claimed: HashSet<String> = ["5a1e0003".to_string()].into_iter().collect();
        recover_stale_job_claims(&dir, &mut roster, &stall_probe(), &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0003").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_QUEUED)
        );
        assert_eq!(read_respawn_count(&dir, "5a1e0003"), 1);
        let launch = crate::background_launch::read_launch_spec(&dir, "5a1e0003").unwrap();
        assert_eq!(
            launch.launch,
            crate::background_launch::BackgroundLaunchKind::Resume
        );
        assert_eq!(launch.initial_prompt, None);
        assert!(!claimed.contains("5a1e0003"));
    }

    #[test]
    fn stale_creating_job_without_launch_spec_fails_closed() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0004");
        agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0004",
            agents_registry::JobStateMatch {
                state: "working",
                phase: Some("queued"),
                worker_pid: None,
                worker_proc_start: None,
                worker_generation: None,
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                tempo: None,
                cwd: None,
                detail: None,
                worker_pid: Some(None),
                worker_proc_start: Some(None),
                phase: Some(Some(crate::commands::respawn::PHASE_CREATING)),
                worker_generation: Some(Some("gen-create")),
                claim_token: Some(None),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_DISPATCH)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(10)),
            },
        )
        .unwrap();

        let mut roster = empty_roster(1);
        let mut claimed = HashSet::new();
        recover_stale_job_claims(&dir, &mut roster, &stall_probe(), &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0004").unwrap();
        assert_eq!(job.state, "failed");
        assert!(job
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("launch context")));
    }

    #[test]
    fn stale_launching_job_with_verified_live_record_is_preserved() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0005");
        agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0005",
            agents_registry::JobStateMatch {
                state: "working",
                phase: Some("queued"),
                worker_pid: None,
                worker_proc_start: None,
                worker_generation: None,
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                tempo: None,
                cwd: None,
                detail: None,
                worker_pid: Some(None),
                worker_proc_start: Some(None),
                phase: Some(Some(crate::commands::respawn::PHASE_LAUNCHING)),
                worker_generation: Some(Some("gen-launch")),
                claim_token: Some(Some("claim-launch")),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_LAUNCH)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(10)),
            },
        )
        .unwrap();
        let mut roster = empty_roster(1);
        let mut record = worker(4300);
        record.proc_start = Some("LIVE-START".to_string());
        roster.workers.insert("5a1e0005".to_string(), record);
        let mut alive = HashMap::new();
        alive.insert(4300, true);
        let mut start = HashMap::new();
        start.insert(4300, "LIVE-START".to_string());
        let proc = FakeProc { alive, start };
        let mut claimed = HashSet::new();

        recover_stale_job_claims(&dir, &mut roster, &proc, &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0005").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_LAUNCHING)
        );
        assert_eq!(read_respawn_count(&dir, "5a1e0005"), 0);
    }

    #[test]
    fn stale_restarting_job_over_budget_fails_closed() {
        let dir = tmpdir();
        seed_working_job(&dir, "5a1e0006");
        seed_resumable_launch_spec(&dir, "5a1e0006");
        agents_registry::patch_job_state_if_matches(
            &dir,
            "5a1e0006",
            agents_registry::JobStateMatch {
                state: "working",
                phase: Some("queued"),
                worker_pid: None,
                worker_proc_start: None,
                worker_generation: None,
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
            agents_registry::JobStatePatch {
                state: Some("working"),
                tempo: None,
                cwd: None,
                detail: None,
                worker_pid: Some(None),
                worker_proc_start: Some(None),
                phase: Some(Some(crate::commands::respawn::PHASE_RESTARTING)),
                worker_generation: Some(Some("gen-restart")),
                claim_token: Some(Some("claim-restart")),
                claim_owner: Some(Some(crate::commands::respawn::CLAIM_OWNER_RESTART)),
                claim_created_at: Some(Some(1)),
                claim_lease_ms: Some(Some(10)),
            },
        )
        .unwrap();
        write_respawn_count(
            &dir,
            "5a1e0006",
            crate::bg_attach_stall::STALL_RESPAWN_BUDGET,
        );

        let mut roster = empty_roster(1);
        let mut claimed = HashSet::new();
        recover_stale_job_claims(&dir, &mut roster, &stall_probe(), &mut claimed, 100);

        let job = agents_registry::read_job(&dir, "5a1e0006").unwrap();
        assert_eq!(job.state, "failed");
        assert_eq!(job.detail.as_deref(), Some(CLAIM_RECOVERY_EXHAUSTED_REASON));
    }

    #[test]
    fn spawned_worker_restores_resume_worktree_and_terminal_from_launch_spec() {
        let _supervisor_guard = supervisor_lock();
        use crate::background_launch::{
            self, BackgroundLaunchKind, BackgroundLaunchOptions, BackgroundLaunchSpec,
            TerminalSize, LAUNCH_SPEC_VERSION,
        };

        let dir = tmpdir();
        let short = "bc7c6b34";
        seed_working_job(&dir, short);
        let worktree = dir.join("worktrees").join(short);
        std::fs::create_dir_all(&worktree).unwrap();
        let transcript = dir.join("exact-resume.jsonl");
        let spec = BackgroundLaunchSpec {
            schema_version: LAUNCH_SPEC_VERSION,
            short: short.to_string(),
            created_at: 1_700_000_000_000,
            preflight_approved: true,
            launch: BackgroundLaunchKind::Fork,
            session_id: "11111111-1111-1111-1111-111111111111".to_string(),
            transcript_path: transcript.display().to_string(),
            cwd: worktree.display().to_string(),
            origin_cwd: dir.display().to_string(),
            worktree_path: Some(worktree.display().to_string()),
            worktree_ownership_token: Some("token-123".to_string()),
            initial_prompt: Some("continue in the worktree".to_string()),
            shell_handoff: Vec::new(),
            handoff: None,
            options: BackgroundLaunchOptions::default(),
            env: BTreeMap::new(),
            terminal: TerminalSize {
                cols: 151,
                rows: 47,
            },
        };
        background_launch::write_launch_spec(&dir, short, &spec).unwrap();

        let proc = FakeProc {
            alive: HashMap::new(),
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

        let roster = read_roster(&dir, 0, false).into_roster();
        let rec = roster.workers.get(short).expect("worker record");
        assert_eq!(rec.cwd, worktree.display().to_string());
        assert_eq!(rec.worktree_path.as_deref(), worktree.to_str());
        assert_eq!(rec.dispatch.cols, Some(151));
        assert_eq!(rec.dispatch.rows, Some(47));
        assert_eq!(rec.dispatch.isolation, Isolation::Worktree);
        assert_eq!(
            rec.dispatch
                .worktree
                .as_ref()
                .map(|value| value.path.as_str()),
            worktree.to_str()
        );
        let marker = crate::daemon_roster::read_worktree_ownership_marker(&worktree).unwrap();
        assert_eq!(marker.short, short);
        assert_eq!(marker.session_id, rec.session_id);
        assert_eq!(
            marker.ownership_token,
            rec.dispatch
                .worktree
                .as_ref()
                .expect("dispatch has worktree")
                .ownership_token
        );
        assert_eq!(
            marker.canonical_worktree_path,
            std::fs::canonicalize(&worktree)
                .unwrap()
                .display()
                .to_string()
        );
        assert_eq!(
            marker.schema_version,
            crate::daemon_roster::WORKTREE_OWNERSHIP_MARKER_SCHEMA_VERSION
        );
        assert_eq!(
            rec.dispatch.launch_spec,
            Some(spec.reference(&dir)),
            "daemon adoption must retain the exact owner-only launch file"
        );
        match &rec.dispatch.launch {
            Launch::Resume {
                session_id,
                transcript_path,
                fork,
                ..
            } => {
                assert_eq!(session_id, &spec.session_id);
                assert_eq!(
                    transcript_path.as_deref(),
                    Some(spec.transcript_path.as_str())
                );
                assert!(*fork);
            }
            other => panic!("expected canonical fork resume launch, got {other:?}"),
        }
    }

    #[test]
    fn terminal_job_is_not_spawned() {
        let _supervisor_guard = supervisor_lock();
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
        let _supervisor_guard = supervisor_lock();
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
        let _supervisor_guard = supervisor_lock();
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

    // ---- crashed-worker recovery -------------------------------------------

    #[test]
    fn crashed_worker_with_resumable_transcript_is_respawned_without_replaying_prompt() {
        let _supervisor_guard = supervisor_lock();
        let dir = tmpdir();
        seed_working_job(&dir, "cafe0001");
        seed_resumable_launch_spec(&dir, "cafe0001");
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

        assert_eq!(spawner.spawned, vec!["cafe0001".to_string()]);
        let job = agents_registry::read_job(&dir, "cafe0001").unwrap();
        assert_eq!(
            job.state, "working",
            "resumable vanished worker stays queued"
        );
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_LAUNCHING),
            "same-heartbeat respawn is claimed for launch"
        );
        assert_eq!(job.worker_pid, None, "worker publishes its own pid");
        assert!(job.worker_generation.is_some());
        assert!(job.claim_token.is_some());
        assert!(!agents_registry::job_is_terminal(&job));
        let launch = crate::background_launch::read_launch_spec(&dir, "cafe0001").unwrap();
        assert_eq!(
            launch.launch,
            crate::background_launch::BackgroundLaunchKind::Resume
        );
        assert_eq!(
            launch.initial_prompt, None,
            "original prompt must never replay"
        );
        let roster = read_roster(&dir, 0, false).into_roster();
        let record = roster.workers.get("cafe0001").expect("worker record");
        assert!(
            matches!(record.dispatch.launch, Launch::Resume { .. }),
            "respawned worker must reopen the recorded session, not relaunch the prompt"
        );
    }

    #[test]
    fn vanished_worker_without_resumable_transcript_fails_closed_without_respawn() {
        let _supervisor_guard = supervisor_lock();
        let dir = tmpdir();
        seed_working_job(&dir, "cafe0004");
        agents_registry::update_job_state(&dir, "cafe0004", "working", Some(4321)).unwrap();

        let proc = FakeProc {
            alive: HashMap::new(),
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
        assert!(
            spawner.spawned.is_empty(),
            "missing transcript must fail closed"
        );
        let job = agents_registry::read_job(&dir, "cafe0004").unwrap();
        assert_eq!(job.state, "failed");
        assert_eq!(job.worker_pid, None);
    }

    #[test]
    fn vanished_worker_does_not_emit_respawn_exhausted() {
        let _supervisor_guard = supervisor_lock();
        // Regression (RV9): when auto-resume is impossible, the daemon fails a
        // vanished worker CLOSED and never respawns it, so there is no respawn budget to exhaust. CC 2.1.208
        // emits `tengu_bg_respawn_exhausted` ONLY from scheduleRespawn once the
        // respawn budget (Jpp=20) is reached — never on the first crash. LingXi
        // used to emit it unconditionally on the very first vanish with a
        // constant `attempts:0`, a spurious event downstream analytics would
        // read as a real budget exhaustion. The vanish itself must still be
        // recorded via `tengu_bg_worker_vanished` (also a CC event).
        use tracing_subscriber::layer::SubscriberExt;
        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        // `tracing` caches per-callsite INTEREST process-globally. Every other
        // `run_supervisor` test in this binary reaches `emit_worker_vanished`
        // with no subscriber installed, which caches that callsite as
        // "never" — and `with_default` (unlike `set_global_default`) does not
        // rebuild the cache. Whether this test saw its own event then depended
        // on test ORDER: it captured nothing roughly one run in three, always
        // as an empty event list. Rebuilding the cache re-evaluates the
        // callsite against the subscriber we are about to install.
        tracing::callsite::rebuild_interest_cache();

        let dir = tmpdir();
        seed_working_job(&dir, "dead0009");
        agents_registry::update_job_state(&dir, "dead0009", "working", Some(4321)).unwrap();

        let proc = FakeProc {
            alive: HashMap::new(), // 4321 is NOT alive → vanished
            start: HashMap::new(),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };
        let mut spawner = FakeWorkerSpawner::default();

        tracing::subscriber::with_default(subscriber, || {
            run_supervisor(
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
        });

        let events = capture.events.lock().unwrap();
        // The vanish itself is still recorded (a real CC event) …
        assert!(
            events.iter().any(|e| e == "tengu_bg_worker_vanished"),
            "vanish must still be recorded, got {events:?}"
        );
        // … but NO spurious respawn-exhausted event on the very first crash.
        assert!(
            !events.iter().any(|e| e == "tengu_bg_respawn_exhausted"),
            "fail-closed vanish must not emit tengu_bg_respawn_exhausted, got {events:?}"
        );
    }

    #[test]
    fn vanished_worker_drains_and_surfaces_stranded_replies() {
        let _supervisor_guard = supervisor_lock();
        // Regression (RV2): the durable offline reply queue was write-only. A
        // follow-up reply the attach fallback persisted while the worker was
        // dying was only ever drained by `bg_worker::execute_job` at (re)spawn —
        // but a vanished worker is failed closed and never respawns, so that
        // drain never ran again and the reply sat on disk forever, undelivered.
        // The supervisor must drain + surface it when it reaps the vanished job.
        let dir = tmpdir();
        seed_working_job(&dir, "beef0002");
        agents_registry::update_job_state(&dir, "beef0002", "working", Some(4321)).unwrap();
        // A reply the attach fallback persisted just before the worker died.
        crate::bg_reply_queue::enqueue_reply(&dir, "beef0002", "please also add tests").unwrap();
        assert_eq!(crate::bg_reply_queue::pending_count(&dir, "beef0002"), 1);

        let proc = FakeProc {
            alive: HashMap::new(), // 4321 is NOT alive → vanished
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
        assert!(
            spawner.spawned.is_empty(),
            "vanished worker must not respawn"
        );

        // The queue is drained (not left write-only) …
        assert_eq!(
            crate::bg_reply_queue::pending_count(&dir, "beef0002"),
            0,
            "the stranded reply must be claimed, not left undelivered forever"
        );
        // … and its text is surfaced on the failed job so the input is not lost.
        let job = agents_registry::read_job(&dir, "beef0002").unwrap();
        assert_eq!(job.state, "failed");
        let detail = job.detail.unwrap_or_default();
        assert!(
            detail.contains("please also add tests"),
            "undelivered reply text must be surfaced in the job detail, got {detail:?}"
        );
    }

    // ---- spawn_cwd_gone (fail-closed, never spawn into a dead cwd) ----------

    #[test]
    fn pending_job_with_missing_cwd_is_failed_closed_not_spawned() {
        let _supervisor_guard = supervisor_lock();
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
        let _supervisor_guard = supervisor_lock();
        // The daemon must launch `__bg-run` with the LINGXI_ background-session
        // env (rebrand of CC's CLAUDE_CODE_SESSION_KIND/CLAUDE_BG_*/CLAUDE_JOB_DIR
        // worker-spawn keys) so the worker's turn gets the `# Background Session`
        // prompt section and `/stop` can locate the job.
        let dir = tmpdir();
        seed_working_job(&dir, "bead0001");
        seed_resumable_launch_spec(&dir, "bead0001");
        let mut launch = crate::background_launch::read_launch_spec(&dir, "bead0001").unwrap();
        launch.env.insert(
            "TEST_PROVIDER_TOKEN".to_string(),
            "provider-secret".to_string(),
        );
        crate::background_launch::write_launch_spec(&dir, "bead0001", &launch).unwrap();
        let proc = FakeProc {
            alive: HashMap::new(),
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
            env.get("TEST_PROVIDER_TOKEN").map(String::as_str),
            Some("provider-secret")
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
        assert!(
            !record.dispatch.env.contains_key("TEST_PROVIDER_TOKEN"),
            "provider credentials must not be copied into the roster"
        );
        let job = agents_registry::read_job(&dir, "bead0001").unwrap();
        assert_eq!(
            record
                .dispatch
                .env
                .get(crate::commands::respawn::BG_WORKER_GENERATION_ENV),
            job.worker_generation.as_ref(),
            "the non-secret generation attestation must survive roster sanitization"
        );
    }

    #[test]
    fn job_with_alive_worker_is_left_untouched_not_failed() {
        let _supervisor_guard = supervisor_lock();
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

    #[test]
    fn restart_adopts_live_roster_worker_when_job_pid_write_was_lost() {
        let _supervisor_guard = supervisor_lock();
        let dir = tmpdir();
        seed_working_job(&dir, "cafe0003");

        let mut roster = empty_roster(999);
        let mut live_record = worker(7778);
        live_record.dispatch.short = "cafe0003".to_string();
        live_record.rendezvous_sock = "/tmp/cafe0003.sock".to_string();
        live_record.pty_sock = Some(live_record.rendezvous_sock.clone());
        live_record.rv_auth = Some("attach-token".to_string());
        live_record.pty_auth = live_record.rv_auth.clone();
        live_record.proc_start = Some("START-7778".to_string());
        roster.workers.insert("cafe0003".to_string(), live_record);
        daemon_roster::write_roster(&dir, &roster).unwrap();

        let mut alive = HashMap::new();
        alive.insert(7778, true);
        let proc = FakeProc {
            alive,
            start: HashMap::from([(7778, "START-7778".to_string())]),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };

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
        let job = agents_registry::read_job(&dir, "cafe0003").unwrap();
        assert_eq!(job.worker_pid, Some(7778));
        assert_eq!(job.state, "working");
    }

    #[test]
    fn live_legacy_roster_without_generation_blocks_duplicate_spawn() {
        let _supervisor_guard = supervisor_lock();
        let dir = tmpdir();
        seed_working_job(&dir, "cafe0005");

        let mut roster = empty_roster(999);
        let mut live_record = worker(7779);
        live_record.dispatch.short = "cafe0005".to_string();
        roster.workers.insert("cafe0005".to_string(), live_record);
        daemon_roster::write_roster(&dir, &roster).unwrap();

        let proc = FakeProc {
            alive: HashMap::from([(7779, true)]),
            start: HashMap::from([(7779, "UNVERIFIED-START".to_string())]),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };

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
        let job = agents_registry::read_job(&dir, "cafe0005").unwrap();
        assert_eq!(job.state, "working");
        assert_eq!(job.worker_pid, None);
        assert_eq!(job.worker_proc_start, None);
    }

    #[test]
    fn verified_live_roster_generation_repairs_state_and_preserves_newer_pty() {
        let _supervisor_guard = supervisor_lock();
        let dir = tmpdir();
        seed_working_job(&dir, "cafe0006");
        agents_registry::update_job_state_with_generation(
            &dir,
            "cafe0006",
            "working",
            Some(7777),
            Some("OLD-START"),
        )
        .unwrap();

        let mut roster = empty_roster(999);
        let mut live_record = worker(7778);
        live_record.dispatch.short = "cafe0006".to_string();
        live_record.proc_start = Some("START-7778".to_string());
        roster.workers.insert("cafe0006".to_string(), live_record);
        daemon_roster::write_roster(&dir, &roster).unwrap();
        write_test_pty_runtime(&dir, "cafe0006", 7778, 9006, "CHILD-START");

        let proc = FakeProc {
            alive: HashMap::from([(7778, true), (9006, true)]),
            start: HashMap::from([
                (7778, "START-7778".to_string()),
                (9006, "CHILD-START".to_string()),
            ]),
        };
        let lockp = FakeLockProbe {
            alive_daemon: HashMap::new(),
        };

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
        let job = agents_registry::read_job(&dir, "cafe0006").unwrap();
        assert_eq!(job.state, "working");
        assert_eq!(job.worker_pid, Some(7778));
        assert_eq!(job.worker_proc_start.as_deref(), Some("START-7778"));
        assert!(crate::background_launch::pty_runtime_path(&dir, "cafe0006").exists());
    }

    #[test]
    fn recycled_live_job_pid_is_resumed_instead_of_hanging_forever() {
        let _supervisor_guard = supervisor_lock();
        let dir = tmpdir();
        seed_working_job(&dir, "cafe0007");
        seed_resumable_launch_spec(&dir, "cafe0007");
        agents_registry::update_job_state_with_generation(
            &dir,
            "cafe0007",
            "working",
            Some(7777),
            Some("OLD-START"),
        )
        .unwrap();

        let proc = FakeProc {
            alive: HashMap::from([(7777, true)]),
            start: HashMap::from([
                (7777, "RECYCLED-START".to_string()),
                (90_000, "START-90000".to_string()),
            ]),
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
        assert_eq!(spawner.spawned, vec!["cafe0007".to_string()]);
        let job = agents_registry::read_job(&dir, "cafe0007").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_LAUNCHING)
        );
        assert_eq!(job.worker_pid, None);
        assert_eq!(job.worker_proc_start, None);
        assert!(job.worker_generation.is_some());
        assert!(job.claim_token.is_some());
    }

    #[test]
    fn vanished_worker_auto_resume_stops_after_budget() {
        let _supervisor_guard = supervisor_lock();
        let dir = tmpdir();
        seed_working_job(&dir, "cafe0008");
        seed_resumable_launch_spec(&dir, "cafe0008");
        agents_registry::update_job_state_with_generation(
            &dir,
            "cafe0008",
            "working",
            Some(4321),
            Some("OLD-START"),
        )
        .unwrap();
        write_respawn_count(
            &dir,
            "cafe0008",
            crate::bg_attach_stall::STALL_RESPAWN_BUDGET,
        );

        let proc = FakeProc {
            alive: HashMap::new(),
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
        assert!(spawner.spawned.is_empty());
        let job = agents_registry::read_job(&dir, "cafe0008").unwrap();
        assert_eq!(job.state, "failed");
        assert_eq!(job.worker_pid, None);
    }

    // ── P1-12: attach-stall respawn requests ─────────────────────────────────

    fn stall_probe() -> FakeProc {
        FakeProc {
            alive: HashMap::new(),
            start: HashMap::new(),
        }
    }

    #[test]
    fn a_stall_request_respawns_the_worker_and_bumps_the_durable_counter() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_working_job(root, "cafe0001");
        seed_resumable_launch_spec(root, "cafe0001");
        let mut roster = empty_roster(1);
        let mut record = worker(4242);
        record.proc_start = Some("WORKER-START".to_string());
        roster.workers.insert("cafe0001".to_string(), record);
        let jobs = agents_registry::jobs_dir(root);
        crate::bg_attach_stall::request_stall_respawn(&jobs, "cafe0001");

        let proc = ScriptedProc::default();
        proc.set_process(4242, "WORKER-START");
        proc.schedule_exit(4242, 1);
        let mut terminator = FakeStallTerminator::new(&proc);
        let mut claimed: HashSet<String> = ["cafe0001".to_string()].into_iter().collect();
        service_stall_requests_with_terminator(
            root,
            &mut roster,
            &proc,
            &mut claimed,
            &mut terminator,
        );

        // The request is consumed, so the next heartbeat does not restart again.
        assert!(!crate::bg_attach_stall::take_stall_request(
            &jobs, "cafe0001"
        ));
        // The record and claim are dropped so `spawn_pending_workers` treats the
        // job as pending IN THE SAME heartbeat.
        assert!(!roster.workers.contains_key("cafe0001"));
        assert!(!claimed.contains("cafe0001"));
        // The durable counter advanced — this is what the budget is applied to.
        assert_eq!(read_respawn_count(root, "cafe0001"), 1);
        assert_eq!(proc.worker_signals(), vec![(4242, false)]);
    }

    #[test]
    fn a_stall_request_waits_for_old_writer_exit_before_respawning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_working_job(root, "cafe0101");
        seed_resumable_launch_spec(root, "cafe0101");
        let mut roster = empty_roster(1);
        let mut record = worker(4242);
        record.proc_start = Some("WORKER-START".to_string());
        roster.workers.insert("cafe0101".to_string(), record);
        write_test_pty_runtime(root, "cafe0101", 4242, 9001, "CHILD-START");
        let jobs = agents_registry::jobs_dir(root);
        crate::bg_attach_stall::request_stall_respawn(&jobs, "cafe0101");

        let proc = ScriptedProc::default();
        proc.set_process(4242, "WORKER-START");
        proc.set_process(9001, "CHILD-START");
        proc.schedule_exit(4242, 3);
        proc.schedule_exit(9001, 3);
        let mut terminator = FakeStallTerminator::new(&proc);
        let mut claimed: HashSet<String> = ["cafe0101".to_string()].into_iter().collect();

        service_stall_requests_with_terminator(
            root,
            &mut roster,
            &proc,
            &mut claimed,
            &mut terminator,
        );

        assert!(!roster.workers.contains_key("cafe0101"));
        assert!(!claimed.contains("cafe0101"));
        assert_eq!(read_respawn_count(root, "cafe0101"), 1);
        assert_eq!(proc.worker_signals(), vec![(4242, false)]);
        assert_eq!(proc.pty_signals(), vec![(9001, false)]);
        assert_eq!(proc.sleep_count(), 3);
        assert!(!crate::background_launch::pty_runtime_path(root, "cafe0101").exists());
        let launch = crate::background_launch::read_launch_spec(root, "cafe0101").unwrap();
        assert_eq!(
            launch.launch,
            crate::background_launch::BackgroundLaunchKind::Resume
        );
        assert_eq!(launch.initial_prompt, None);
    }

    #[test]
    fn a_stall_request_preserves_owner_if_restart_barrier_cannot_clear_the_old_writer() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_working_job(root, "cafe0102");
        seed_resumable_launch_spec(root, "cafe0102");
        let mut roster = empty_roster(1);
        let mut record = worker(4243);
        record.proc_start = Some("WORKER-START".to_string());
        roster.workers.insert("cafe0102".to_string(), record);
        write_test_pty_runtime(root, "cafe0102", 4243, 9002, "CHILD-START");
        let jobs = agents_registry::jobs_dir(root);
        crate::bg_attach_stall::request_stall_respawn(&jobs, "cafe0102");

        let proc = ScriptedProc::default();
        proc.set_process(4243, "WORKER-START");
        proc.set_process(9002, "CHILD-START");
        let mut terminator = FakeStallTerminator::new(&proc);
        let mut claimed: HashSet<String> = ["cafe0102".to_string()].into_iter().collect();

        service_stall_requests_with_terminator(
            root,
            &mut roster,
            &proc,
            &mut claimed,
            &mut terminator,
        );

        assert!(roster.workers.contains_key("cafe0102"));
        assert!(claimed.contains("cafe0102"));
        assert_eq!(read_respawn_count(root, "cafe0102"), 0);
        assert_eq!(proc.worker_signals(), vec![(4243, false), (4243, true)]);
        assert_eq!(proc.pty_signals(), vec![(9002, false), (9002, true)]);
        assert!(
            crate::background_launch::pty_runtime_path(root, "cafe0102").exists(),
            "the runtime identity stays in place while the old writer is unconfirmed"
        );
        let job = agents_registry::read_job(root, "cafe0102").unwrap();
        assert_eq!(job.state, "working");
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_QUEUED)
        );
        assert_eq!(job.detail, None);
        let launch = crate::background_launch::read_launch_spec(root, "cafe0102").unwrap();
        assert_eq!(
            launch.launch,
            crate::background_launch::BackgroundLaunchKind::Fresh
        );
        assert!(launch.initial_prompt.is_some());
    }

    #[test]
    fn a_stall_request_escalates_to_hard_termination_before_respawning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_working_job(root, "cafe0103");
        seed_resumable_launch_spec(root, "cafe0103");
        let mut roster = empty_roster(1);
        let mut record = worker(4244);
        record.proc_start = Some("WORKER-START".to_string());
        roster.workers.insert("cafe0103".to_string(), record);
        write_test_pty_runtime(root, "cafe0103", 4244, 9003, "CHILD-START");
        let jobs = agents_registry::jobs_dir(root);
        crate::bg_attach_stall::request_stall_respawn(&jobs, "cafe0103");

        let proc = ScriptedProc::default();
        proc.set_process(4244, "WORKER-START");
        proc.set_process(9003, "CHILD-START");
        proc.schedule_exit(4244, STALL_TERMINATION_POLL_ATTEMPTS + 1);
        proc.schedule_exit(9003, STALL_TERMINATION_POLL_ATTEMPTS + 1);
        let mut terminator = FakeStallTerminator::new(&proc);
        let mut claimed: HashSet<String> = ["cafe0103".to_string()].into_iter().collect();

        service_stall_requests_with_terminator(
            root,
            &mut roster,
            &proc,
            &mut claimed,
            &mut terminator,
        );

        assert!(!roster.workers.contains_key("cafe0103"));
        assert!(!claimed.contains("cafe0103"));
        assert_eq!(read_respawn_count(root, "cafe0103"), 1);
        assert_eq!(proc.worker_signals(), vec![(4244, false), (4244, true)]);
        assert_eq!(proc.pty_signals(), vec![(9003, false), (9003, true)]);
        assert_eq!(proc.sleep_count(), STALL_TERMINATION_POLL_ATTEMPTS + 1);
        assert!(!crate::background_launch::pty_runtime_path(root, "cafe0103").exists());
    }

    #[test]
    fn a_stall_request_does_not_signal_recycled_worker_or_pty_identities() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_working_job(root, "cafe0104");
        seed_resumable_launch_spec(root, "cafe0104");
        let mut roster = empty_roster(1);
        let mut record = worker(4245);
        record.proc_start = Some("WORKER-START".to_string());
        roster.workers.insert("cafe0104".to_string(), record);
        write_test_pty_runtime(root, "cafe0104", 4245, 9004, "CHILD-START");
        let jobs = agents_registry::jobs_dir(root);
        crate::bg_attach_stall::request_stall_respawn(&jobs, "cafe0104");

        let proc = ScriptedProc::default();
        proc.set_process(4245, "RECYCLED-WORKER");
        proc.set_process(9004, "RECYCLED-CHILD");
        let mut terminator = FakeStallTerminator::new(&proc);
        let mut claimed: HashSet<String> = ["cafe0104".to_string()].into_iter().collect();

        service_stall_requests_with_terminator(
            root,
            &mut roster,
            &proc,
            &mut claimed,
            &mut terminator,
        );

        assert!(roster.workers.contains_key("cafe0104"));
        assert!(claimed.contains("cafe0104"));
        assert_eq!(proc.worker_signals(), Vec::<(i32, bool)>::new());
        assert_eq!(proc.pty_signals(), Vec::<(i32, bool)>::new());
        assert_eq!(proc.sleep_count(), 0);
        assert_eq!(read_respawn_count(root, "cafe0104"), 0);
        assert!(crate::background_launch::pty_runtime_path(root, "cafe0104").exists());
        let job = agents_registry::read_job(root, "cafe0104").unwrap();
        assert_eq!(job.state, "working");
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_QUEUED)
        );
    }

    #[test]
    fn a_stall_request_fails_closed_for_live_legacy_pty_identity_without_signaling_or_respawning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_working_job(root, "cafe0105");
        seed_resumable_launch_spec(root, "cafe0105");
        let mut roster = empty_roster(1);
        let mut record = worker(4246);
        record.proc_start = Some("WORKER-START".to_string());
        roster.workers.insert("cafe0105".to_string(), record);
        write_legacy_test_pty_runtime(root, "cafe0105", 4246, 9005);
        let jobs = agents_registry::jobs_dir(root);
        crate::bg_attach_stall::request_stall_respawn(&jobs, "cafe0105");

        let proc = ScriptedProc::default();
        proc.set_process(4246, "WORKER-START");
        proc.set_process(9005, "CHILD-START");
        let mut terminator = FakeStallTerminator::new(&proc);
        let mut claimed: HashSet<String> = ["cafe0105".to_string()].into_iter().collect();

        service_stall_requests_with_terminator(
            root,
            &mut roster,
            &proc,
            &mut claimed,
            &mut terminator,
        );

        assert!(roster.workers.contains_key("cafe0105"));
        assert!(claimed.contains("cafe0105"));
        assert_eq!(proc.worker_signals(), Vec::<(i32, bool)>::new());
        assert_eq!(proc.pty_signals(), Vec::<(i32, bool)>::new());
        assert_eq!(proc.sleep_count(), 0);
        assert_eq!(read_respawn_count(root, "cafe0105"), 0);
        assert!(crate::background_launch::pty_runtime_path(root, "cafe0105").exists());
        let job = agents_registry::read_job(root, "cafe0105").unwrap();
        assert_eq!(job.state, "working");
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_QUEUED)
        );
        assert_eq!(job.detail, None);
        let launch = crate::background_launch::read_launch_spec(root, "cafe0105").unwrap();
        assert_eq!(
            launch.launch,
            crate::background_launch::BackgroundLaunchKind::Fresh
        );
    }

    #[test]
    fn cleanup_orphaned_pty_preserves_live_unverified_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_legacy_test_pty_runtime(root, "cafe0106", 4247, 9006);

        let mut alive = HashMap::new();
        alive.insert(9006, true);
        let mut start = HashMap::new();
        start.insert(9006, "CHILD-START".to_string());
        let proc = FakeProc { alive, start };

        cleanup_orphaned_pty(root, "cafe0106", Some(4247), &proc);

        assert!(crate::background_launch::pty_runtime_path(root, "cafe0106").exists());
    }

    #[test]
    fn a_stall_request_does_not_signal_live_legacy_worker_without_start_identity() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_working_job(root, "cafe0107");
        seed_resumable_launch_spec(root, "cafe0107");
        let mut roster = empty_roster(1);
        roster.workers.insert("cafe0107".to_string(), worker(4248));
        let jobs = agents_registry::jobs_dir(root);
        crate::bg_attach_stall::request_stall_respawn(&jobs, "cafe0107");

        let proc = ScriptedProc::default();
        proc.set_process(4248, "UNVERIFIED-START");
        let mut terminator = FakeStallTerminator::new(&proc);
        let mut claimed: HashSet<String> = ["cafe0107".to_string()].into_iter().collect();

        service_stall_requests_with_terminator(
            root,
            &mut roster,
            &proc,
            &mut claimed,
            &mut terminator,
        );

        assert!(roster.workers.contains_key("cafe0107"));
        assert!(claimed.contains("cafe0107"));
        assert_eq!(proc.worker_signals(), Vec::<(i32, bool)>::new());
        assert_eq!(proc.sleep_count(), 0);
        assert_eq!(read_respawn_count(root, "cafe0107"), 0);
        let job = agents_registry::read_job(root, "cafe0107").unwrap();
        assert_eq!(job.state, "working");
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_QUEUED)
        );
        assert_eq!(job.detail, None);
        let launch = crate::background_launch::read_launch_spec(root, "cafe0107").unwrap();
        assert_eq!(
            launch.launch,
            crate::background_launch::BackgroundLaunchKind::Fresh
        );
    }

    #[test]
    fn a_stall_request_never_signals_a_stale_worker_generation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_working_job(root, "cafe0108");
        seed_resumable_launch_spec(root, "cafe0108");
        let queued = agents_registry::read_job(root, "cafe0108").unwrap();
        assert!(agents_registry::patch_job_state_if_matches(
            root,
            "cafe0108",
            agents_registry::JobStateMatch {
                state: &queued.state,
                phase: queued.phase.as_deref(),
                worker_pid: queued.worker_pid,
                worker_proc_start: queued.worker_proc_start.as_deref(),
                worker_generation: queued.worker_generation.as_deref(),
                claim_token: queued.claim_token.as_deref(),
                claim_owner: queued.claim_owner.as_deref(),
                claim_created_at: queued.claim_created_at,
                claim_lease_ms: queued.claim_lease_ms,
            },
            agents_registry::JobStatePatch {
                worker_pid: Some(Some(4_249)),
                worker_proc_start: Some(Some("WORKER-START")),
                phase: Some(Some(crate::commands::respawn::PHASE_RUNNING)),
                worker_generation: Some(Some("job-generation")),
                ..Default::default()
            },
        )
        .unwrap());
        let mut record = worker(4_249);
        record.proc_start = Some("WORKER-START".to_string());
        record.dispatch.env.insert(
            crate::commands::respawn::BG_WORKER_GENERATION_ENV.to_string(),
            "stale-roster-generation".to_string(),
        );
        let mut roster = empty_roster(1);
        roster.workers.insert("cafe0108".to_string(), record);
        crate::bg_attach_stall::request_stall_respawn(&agents_registry::jobs_dir(root), "cafe0108");
        let proc = ScriptedProc::default();
        proc.set_process(4_249, "WORKER-START");
        let mut terminator = FakeStallTerminator::new(&proc);
        let mut claimed: HashSet<String> = ["cafe0108".to_string()].into_iter().collect();

        service_stall_requests_with_terminator(
            root,
            &mut roster,
            &proc,
            &mut claimed,
            &mut terminator,
        );

        assert!(roster.workers.contains_key("cafe0108"));
        assert!(claimed.contains("cafe0108"));
        assert_eq!(proc.worker_signals(), Vec::<(i32, bool)>::new());
        assert_eq!(proc.pty_signals(), Vec::<(i32, bool)>::new());
        assert_eq!(proc.sleep_count(), 0);
        assert_eq!(read_respawn_count(root, "cafe0108"), 0);
        let job = agents_registry::read_job(root, "cafe0108").unwrap();
        assert_eq!(
            job.phase.as_deref(),
            Some(crate::commands::respawn::PHASE_RUNNING)
        );
        assert_eq!(job.worker_generation.as_deref(), Some("job-generation"));
        let launch = crate::background_launch::read_launch_spec(root, "cafe0108").unwrap();
        assert_eq!(
            launch.launch,
            crate::background_launch::BackgroundLaunchKind::Fresh
        );
    }

    #[test]
    fn the_budget_gives_up_instead_of_respawning_forever() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        seed_working_job(root, "cafe0002");
        write_respawn_count(
            root,
            "cafe0002",
            crate::bg_attach_stall::STALL_RESPAWN_BUDGET,
        );
        let mut roster = empty_roster(1);
        let mut record = worker(4243);
        record.proc_start = Some("WORKER-START".to_string());
        roster.workers.insert("cafe0002".to_string(), record);
        let jobs = agents_registry::jobs_dir(root);
        crate::bg_attach_stall::request_stall_respawn(&jobs, "cafe0002");

        let proc = ScriptedProc::default();
        proc.set_process(4243, "WORKER-START");
        proc.schedule_exit(4243, 1);
        let mut terminator = FakeStallTerminator::new(&proc);
        let mut claimed: HashSet<String> = ["cafe0002".to_string()].into_iter().collect();
        service_stall_requests_with_terminator(
            root,
            &mut roster,
            &proc,
            &mut claimed,
            &mut terminator,
        );

        // Budget spent: the counter must NOT keep climbing, and the job is
        // failed closed rather than restarted a third time.
        assert_eq!(
            read_respawn_count(root, "cafe0002"),
            crate::bg_attach_stall::STALL_RESPAWN_BUDGET
        );
        assert!(!roster.workers.contains_key("cafe0002"));
        assert!(!claimed.contains("cafe0002"));
        assert_eq!(proc.worker_signals(), vec![(4243, true)]);
        let job = agents_registry::read_job(root, "cafe0002").unwrap();
        assert_eq!(job.state, "failed");
        assert_eq!(
            job.detail.as_deref(),
            Some(crate::bg_attach_stall::KEEPS_STALLING_KILL_REASON)
        );
    }

    #[test]
    fn a_worker_with_no_request_is_left_completely_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut roster = empty_roster(1);
        roster.workers.insert("cafe0003".to_string(), worker(4244));
        let mut claimed: HashSet<String> = ["cafe0003".to_string()].into_iter().collect();

        service_stall_requests(root, &mut roster, &stall_probe(), &mut claimed);

        // The whole point: a healthy session must never be touched by this.
        assert!(roster.workers.contains_key("cafe0003"));
        assert!(claimed.contains("cafe0003"));
        assert_eq!(read_respawn_count(root, "cafe0003"), 0);
    }
}
