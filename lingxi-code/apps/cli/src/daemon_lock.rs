//! Background-agent supervisor **`daemon.lock`** primitive — acquisition,
//! PID-reuse-safe stale-lock takeover, the bounded start-time retry loop, and
//! the yield/handover decision.
//!
//! This is the second self-contained slice of the real 2.1.201 background-agent
//! daemon. Where [`crate::daemon_roster`] owns the persisted worker fleet
//! (`roster.json`), this module owns the single-supervisor mutual-exclusion
//! primitive that decides **who** is allowed to own that fleet: the
//! `daemon.lock` file under the daemon runtime dir.
//!
//! Ported 1:1 from the 2.1.201 binary (module around `CJl="daemon.lock"`,
//! constants `Lur=2`, `def=250`):
//!
//! * **Lock path** (`lz`) — `<runtime>/daemon.lock`. [`lock_path`].
//! * **Temp path** (`uef`) — `<runtime>/daemon.lock.tmp.<pid>.<startedAt>`.
//!   [`lock_tmp_path`].
//! * **Acquire** (`RJl`) — an *exclusive-create* (`{flag:"wx"}` → `O_CREAT |
//!   O_EXCL`) write of the lock JSON. Success ⇒ we hold it; `EEXIST` ⇒ someone
//!   else already does (contended); any other errno propagates. [`acquire`].
//! * **Read + validate** (`a8`) — `lstat` guard (a non-regular-file or a file
//!   over 64 KiB is `rm -rf`'d and treated as absent), tolerant `JSON.parse`,
//!   then the manual shape check `typeof pid==="number" && typeof
//!   version==="string"`. [`read_lock`].
//! * **Takeover write** (`Our`) — atomic temp-write + rename, with an
//!   `EEXIST`/`EPERM` fallback that `unlink`s the stale target and retries the
//!   rename, finally verifying we won via a read-back of `pid`+`startedAt`.
//!   [`write_lock_takeover`].
//! * **Release** (`wJl`) — `unlink`, swallowing `ENOENT`. [`release`].
//! * **Daemon-process check** (`Htn`) — reads `/proc/<pid>/cmdline`; an
//!   unreadable cmdline is assumed to be ours, else the title must be the
//!   daemon title or one of argv[1..4] must be the `daemon` subcommand.
//!   [`classify_cmdline`].
//! * **Start-time retry loop** (`ZWo`) — a bounded ([`RETRY_ATTEMPTS`] = `Lur`)
//!   probe of the holder's live start-time, backing off [`RETRY_BACKOFF_MS`] =
//!   `def` between tries; an undefined expected start-time short-circuits to
//!   *ok*. [`start_time_matches`].
//! * **Live holder** (`BH`) — read the lock, then require the holder pid to be
//!   alive **and** a daemon process **and** to still have the recorded
//!   start-time; any failure ⇒ the lock is stale and returns `None`.
//!   [`live_holder`] / the richer [`evaluate_holder`].
//! * **Upgrade check** (`xJl`) — a live holder whose `version` differs from
//!   ours signals a binary handover is due. [`holder_needs_upgrade`].
//!
//! The composed [`acquire_or_yield`] state machine ties these together into the
//! decision a freshly-launched supervisor makes: **acquire** a free lock,
//! **take over** a stale one (dead / not-a-daemon / recycled-PID holder), or
//! **yield** to a live peer.
//!
//! NOTE: like [`crate::daemon_roster`], this module is deliberately *not* wired
//! into the live `agents` command yet — it is the mutual-exclusion primitive
//! the rest of the daemon (auth'd `control.sock` + `rvAuth`/`ptyAuth`,
//! respawn/stall watchdog, upgrade takeover, orphan reap) will build on. See
//! the residual plan in the porting notes.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Lock file name under the daemon runtime dir (binary `CJl`).
pub const LOCK_FILE: &str = "daemon.lock";

/// How many times [`start_time_matches`] probes the holder's live start-time
/// before giving up (binary `Lur`). Passed as `BH(Lur)` on the handover path.
pub const RETRY_ATTEMPTS: usize = 2;

/// Milliseconds to back off between start-time probes (binary `def`).
pub const RETRY_BACKOFF_MS: u64 = 250;

/// Largest acceptable lock file (binary `a8`: `n.size>65536`). Anything bigger
/// is assumed corrupt and removed rather than parsed.
pub const MAX_LOCK_BYTES: u64 = 65536;

/// Process title the daemon runs under, matched against `argv[0]` of a holder's
/// cmdline (binary `Htn`: `n[0]==="claude daemon"`). Brand token swapped
/// `claude`→`lingxi`; the `daemon` **subcommand** token is kept verbatim.
pub const DAEMON_PROC_TITLE: &str = "lingxi daemon";

/// The `daemon` subcommand token looked for in `argv[1..4]` (binary `Htn`:
/// `n.slice(1,4).includes("daemon")`). A protocol/CLI token — kept verbatim.
pub const DAEMON_SUBCOMMAND: &str = "daemon";

/// Supervisor log line emitted when a background sweep is skipped because the
/// lock is mid yield/handover (binary: `bg: skipped post-adopt sweeps + roster
/// rewrite — daemon.lock is …`). The em-dash matches the binary's `—`.
pub const YIELD_MESSAGE: &str =
    "yielding to a foreground/service daemon — bg workers will be re-adopted";

/// `daemon.lock` path (binary `lz`).
#[must_use]
pub fn lock_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(LOCK_FILE)
}

/// `daemon.lock.tmp.<pid>.<startedAt>` path (binary `uef`) — the sibling temp
/// file a takeover writes before renaming it into place.
#[must_use]
pub fn lock_tmp_path(runtime_dir: &Path, pid: i32, started_at: i64) -> PathBuf {
    runtime_dir.join(format!("{LOCK_FILE}.tmp.{pid}.{started_at}"))
}

// ---------------------------------------------------------------------------
// Lock record.
// ---------------------------------------------------------------------------

/// The `daemon.lock` payload. Field order matches the binary's object literal so
/// the pretty-JSON write is byte-faithful: `pid, version, jsonPath, logPath,
/// startedAt, origin, spawnedBy, procStart, launchTarget, bgDisabled`.
///
/// The only fields the reader (`a8`) *requires* are `pid` (a number) and
/// `version` (a string); everything else is optional so a lock written by a
/// slightly different code path still validates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonLock {
    /// OS process id of the supervisor holding the lock.
    pub pid: i32,
    /// CLI version string of the holder (the plain `VERSION`, e.g. `"2.1.201"`).
    pub version: String,
    /// Path to the holder's `daemon.json`.
    #[serde(rename = "jsonPath", skip_serializing_if = "Option::is_none")]
    pub json_path: Option<String>,
    /// Path to the holder's `daemon.log`.
    #[serde(rename = "logPath", skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
    /// Epoch-millis the holder started (part of the PID-reuse takeover check).
    #[serde(rename = "startedAt", default)]
    pub started_at: i64,
    /// How the daemon was launched (`"foreground"`, `"service"`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// What spawned the daemon.
    #[serde(rename = "spawnedBy", skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<String>,
    /// The holder's `ps -o lstart=` start-time string — the recycled-PID guard.
    #[serde(rename = "procStart", skip_serializing_if = "Option::is_none")]
    pub proc_start: Option<String>,
    /// Launch target hint.
    #[serde(rename = "launchTarget", skip_serializing_if = "Option::is_none")]
    pub launch_target: Option<String>,
    /// Set (by `vJl`) when the holder has disabled background dispatch while
    /// still owning the lock. Absent unless `true`.
    #[serde(rename = "bgDisabled", default, skip_serializing_if = "is_false")]
    pub bg_disabled: bool,

    /// Any unmodeled keys, preserved verbatim.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

#[must_use]
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl DaemonLock {
    /// A minimal lock owned by `pid` running `version`, started "now". Optional
    /// metadata (paths, origin, procStart) is filled by the caller.
    #[must_use]
    pub fn new(pid: i32, version: impl Into<String>) -> Self {
        DaemonLock {
            pid,
            version: version.into(),
            json_path: None,
            log_path: None,
            started_at: now_millis(),
            origin: None,
            spawned_by: None,
            proc_start: None,
            launch_target: None,
            bg_disabled: false,
            extra: serde_json::Map::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Liveness / start-time / cmdline probe (abstracted for testing).
// ---------------------------------------------------------------------------

/// Probes a candidate holder pid. Abstracted so the yield/takeover logic is
/// testable without spawning real processes; [`SystemLockProbe`] is the
/// production implementation.
pub trait LockProbe {
    /// Whether `pid` is a live process (binary `process.kill(pid,0)` — a raised
    /// error means dead; `EPERM` still means alive).
    fn is_alive(&self, pid: i32) -> bool;

    /// Whether `pid` looks like one of *our* daemon processes (binary `Htn`,
    /// reading `/proc/<pid>/cmdline`). An unreadable cmdline is assumed to be
    /// ours (returns `true`), matching the binary's `catch { return true }`.
    fn is_daemon_process(&self, pid: i32) -> bool;

    /// The process's live `ps -o lstart=` start-time (binary `IA`). `skip_cache`
    /// mirrors the binary's `{skipCache:r>0}`; the production probe reads `ps`
    /// fresh every call so it is advisory here.
    fn proc_start(&self, pid: i32, skip_cache: bool) -> Option<String>;
}

/// Production probe: `kill(pid,0)` liveness, `/proc/<pid>/cmdline` daemon check,
/// `ps -o lstart=` start-time (reusing [`crate::daemon_roster::read_proc_start`]).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemLockProbe;

impl LockProbe for SystemLockProbe {
    #[cfg(unix)]
    fn is_alive(&self, pid: i32) -> bool {
        if pid <= 1 {
            return false;
        }
        matches!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Ok(()) | Err(nix::errno::Errno::EPERM)
        )
    }

    #[cfg(not(unix))]
    fn is_alive(&self, pid: i32) -> bool {
        pid > 1
    }

    fn is_daemon_process(&self, pid: i32) -> bool {
        classify_cmdline(read_cmdline(pid).as_deref())
    }

    fn proc_start(&self, pid: i32, _skip_cache: bool) -> Option<String> {
        crate::daemon_roster::read_proc_start(pid)
    }
}

/// Read `/proc/<pid>/cmdline` and split on NUL into argv (binary `Htn`'s
/// `t.split("\x00")`). `None` when the file cannot be read (no `/proc`, or the
/// process is gone) — the caller treats that as "assume it's ours".
#[must_use]
fn read_cmdline(pid: i32) -> Option<Vec<String>> {
    if pid <= 1 {
        return None;
    }
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        raw.split(|b| *b == 0)
            .map(|seg| String::from_utf8_lossy(seg).into_owned())
            .collect(),
    )
}

/// Classify a holder's cmdline argv (binary `Htn`): `None` (unreadable) is
/// assumed to be our daemon; otherwise `argv[0]` must equal the daemon title,
/// or one of `argv[1..4]` must be the `daemon` subcommand token.
#[must_use]
pub fn classify_cmdline(argv: Option<&[String]>) -> bool {
    let Some(argv) = argv else {
        // Unreadable cmdline → binary `catch { return true }`.
        return true;
    };
    if argv.first().map(String::as_str) == Some(DAEMON_PROC_TITLE) {
        return true;
    }
    // `n.slice(1,4)` → indices 1,2,3.
    argv.iter()
        .skip(1)
        .take(3)
        .any(|a| a == DAEMON_SUBCOMMAND)
}

// ---------------------------------------------------------------------------
// Acquire / read / takeover / release (`RJl` / `a8` / `Our` / `wJl`).
// ---------------------------------------------------------------------------

/// Raw outcome of an exclusive-create acquire (`RJl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireRaw {
    /// The lock did not exist and we created it — we hold it.
    Acquired,
    /// The lock already existed (`EEXIST`) — someone else may hold it. The
    /// caller must [`evaluate_holder`] to decide take-over vs yield.
    Contended,
}

/// Attempt to acquire the lock by exclusively creating `daemon.lock` with our
/// payload (binary `RJl`: `writeFile(lz(), …, {flag:"wx"})`). Creates the
/// runtime dir (`0o700`) first. `EEXIST` ⇒ [`AcquireRaw::Contended`]; any other
/// errno propagates.
pub fn acquire(runtime_dir: &Path, lock: &DaemonLock) -> std::io::Result<AcquireRaw> {
    std::fs::create_dir_all(runtime_dir)?;
    set_mode(runtime_dir, 0o700);

    let body = serde_json::to_string_pretty(lock)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    match write_exclusive(&lock_path(runtime_dir), body.as_bytes(), 0o600) {
        Ok(()) => Ok(AcquireRaw::Acquired),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(AcquireRaw::Contended),
        Err(e) => Err(e),
    }
}

/// Read + validate `daemon.lock` (binary `a8`). Returns `None` for an absent,
/// non-regular, over-sized, unparseable, or shape-invalid lock — a non-regular
/// or over-sized file is additionally `rm -rf`'d, exactly like the binary.
///
/// The shape check requires `pid` (a JSON number) and `version` (a JSON string);
/// everything else is optional.
#[must_use]
pub fn read_lock(runtime_dir: &Path) -> Option<DaemonLock> {
    let path = lock_path(runtime_dir);

    match std::fs::symlink_metadata(&path) {
        Ok(meta) => {
            if !meta.is_file() || meta.len() > MAX_LOCK_BYTES {
                // `rm(lz(),{recursive:true,force:true})`.
                let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
                return None;
            }
        }
        // ENOENT (and, defensively, any other stat error) → treat as absent.
        Err(_) => return None,
    }

    let bytes = std::fs::read_to_string(&path).ok()?;
    let raw: serde_json::Value = serde_json::from_str(&bytes).ok()?;

    // Manual shape guard (`typeof pid==="number" && typeof version==="string"`)
    // before deserializing, so a partially-written or foreign lock is treated as
    // "not a valid lock" (→ stale) rather than parsed into garbage.
    let pid_ok = raw.get("pid").and_then(serde_json::Value::as_i64).is_some();
    let ver_ok = raw
        .get("version")
        .and_then(serde_json::Value::as_str)
        .is_some();
    if !pid_ok || !ver_ok {
        return None;
    }

    serde_json::from_value::<DaemonLock>(raw).ok()
}

/// Take over the lock by atomically writing our payload over whatever is there
/// (binary `Our`). Writes a sibling temp file exclusively, renames it into
/// place, and on an `EEXIST`/`EPERM` rename failure `unlink`s the stale target
/// and retries once. Returns `Ok(true)` only if the post-write read-back shows
/// **our** `pid`+`startedAt` won the race.
pub fn write_lock_takeover(runtime_dir: &Path, lock: &DaemonLock) -> std::io::Result<bool> {
    std::fs::create_dir_all(runtime_dir)?;
    set_mode(runtime_dir, 0o700);

    let target = lock_path(runtime_dir);
    let tmp = lock_tmp_path(runtime_dir, lock.pid, lock.started_at);
    let body = serde_json::to_string_pretty(lock)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    // `writeFile(t, …, {flag:"wx"})`.
    write_exclusive(&tmp, body.as_bytes(), 0o600)?;

    if let Err(first) = std::fs::rename(&tmp, &target) {
        if is_eexist_or_eperm(&first) {
            // `unlink(lz()).catch(()=>{})` then retry the rename.
            let _ = std::fs::remove_file(&target);
            if let Err(second) = std::fs::rename(&tmp, &target) {
                let _ = std::fs::remove_file(&tmp);
                if is_eexist_or_eperm(&second) {
                    return Ok(false);
                }
                return Err(second);
            }
        } else {
            let _ = std::fs::remove_file(&tmp);
            return Err(first);
        }
    }

    // Read-back: did *we* win? (`n?.pid===e.pid && n?.startedAt===e.startedAt`).
    Ok(read_lock(runtime_dir)
        .map(|l| l.pid == lock.pid && l.started_at == lock.started_at)
        .unwrap_or(false))
}

/// Release the lock (binary `wJl`): `unlink`, swallowing `ENOENT`.
pub fn release(runtime_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(lock_path(runtime_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn is_eexist_or_eperm(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
    ) || matches!(e.raw_os_error(), Some(1) | Some(17)) // EPERM=1, EEXIST=17
}

/// `O_CREAT | O_EXCL | O_WRONLY` write, chmod'd to `mode`. `AlreadyExists` on a
/// pre-existing file — the `{flag:"wx"}` semantics.
fn write_exclusive(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(bytes)?;
    f.flush()?;
    drop(f);
    set_mode(path, mode);
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

// ---------------------------------------------------------------------------
// Start-time retry loop + holder evaluation (`ZWo` / `BH`).
// ---------------------------------------------------------------------------

/// Bounded start-time match (binary `ZWo`). An `expected` of `None` (the
/// binary's `t===void 0`) short-circuits to `true` — we can't disprove the
/// holder, so we don't reject it. Otherwise probe the live start-time up to
/// `attempts` times, backing off `RETRY_BACKOFF_MS` between tries (via
/// `sleep`); the **first** readable live value decides (`==` ⇒ `true`), and an
/// exhausted loop that never read a value returns `false`.
///
/// `sleep` is injected so the loop is testable without real time.
pub fn start_time_matches<P: LockProbe>(
    probe: &P,
    pid: i32,
    expected: Option<&str>,
    attempts: usize,
    sleep: &mut dyn FnMut(u64),
) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    for r in 0..attempts {
        if r > 0 {
            sleep(RETRY_BACKOFF_MS);
        }
        if let Some(live) = probe.proc_start(pid, r > 0) {
            return live == expected;
        }
    }
    false
}

/// Why a lock is considered stale (takeable) rather than held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    /// The holder pid is no longer a live process.
    Dead,
    /// The holder pid is alive but is not one of our daemon processes — the PID
    /// was reused by something unrelated.
    NotDaemon,
    /// The holder pid is a live daemon but its start-time no longer matches the
    /// recorded `procStart` — a recycled PID.
    RecycledPid,
}

/// Full evaluation of the current lock holder — the richer form of `BH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HolderEval {
    /// No (valid) lock file present.
    Absent,
    /// A live daemon holds it, and it is *us*.
    LiveSelf { pid: i32 },
    /// A live *other* daemon holds it — we must yield.
    LiveOther { pid: i32, version: String },
    /// The lock file is present but its holder is stale — safe to take over.
    Stale(StaleReason),
}

/// Evaluate who currently holds the lock (binary `BH` + the caller's
/// self/other/stale interpretation). `my_pid` is the caller's own pid so a
/// re-entrant acquire recognises its own lock. `attempts` is the start-time
/// retry budget (`Lur` on the handover path).
pub fn evaluate_holder<P: LockProbe>(
    runtime_dir: &Path,
    my_pid: i32,
    probe: &P,
    attempts: usize,
    sleep: &mut dyn FnMut(u64),
) -> HolderEval {
    let Some(lock) = read_lock(runtime_dir) else {
        return HolderEval::Absent;
    };
    if !probe.is_alive(lock.pid) {
        return HolderEval::Stale(StaleReason::Dead);
    }
    if !probe.is_daemon_process(lock.pid) {
        return HolderEval::Stale(StaleReason::NotDaemon);
    }
    if !start_time_matches(probe, lock.pid, lock.proc_start.as_deref(), attempts, sleep) {
        return HolderEval::Stale(StaleReason::RecycledPid);
    }
    if lock.pid == my_pid {
        HolderEval::LiveSelf { pid: lock.pid }
    } else {
        HolderEval::LiveOther {
            pid: lock.pid,
            version: lock.version,
        }
    }
}

/// The live lock holder, or `None` if the lock is absent or stale (binary `BH`).
/// A thin projection of [`evaluate_holder`] for callers that only need "is
/// there a live holder, and who".
pub fn live_holder<P: LockProbe>(
    runtime_dir: &Path,
    probe: &P,
    attempts: usize,
    sleep: &mut dyn FnMut(u64),
) -> Option<DaemonLock> {
    // We pass a pid that can never match a real holder so LiveSelf collapses into
    // LiveOther — `live_holder` doesn't care about self vs other, only liveness.
    match evaluate_holder(runtime_dir, -1, probe, attempts, sleep) {
        HolderEval::LiveSelf { .. } | HolderEval::LiveOther { .. } => read_lock(runtime_dir),
        HolderEval::Absent | HolderEval::Stale(_) => None,
    }
}

/// Whether a live holder is running a *different* version than `current`,
/// signalling a binary handover is due (binary `xJl`).
pub fn holder_needs_upgrade<P: LockProbe>(
    runtime_dir: &Path,
    current_version: &str,
    probe: &P,
    attempts: usize,
    sleep: &mut dyn FnMut(u64),
) -> bool {
    matches!(
        evaluate_holder(runtime_dir, -1, probe, attempts, sleep),
        HolderEval::LiveOther { version, .. } if version != current_version
    )
}

// ---------------------------------------------------------------------------
// Composed acquire-or-yield state machine.
// ---------------------------------------------------------------------------

/// The decision a launching supervisor reaches for the lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockOutcome {
    /// We now hold the lock. `took_over` records the [`StaleReason`] if we stole
    /// it from a stale holder rather than creating it fresh.
    Acquired { took_over: Option<StaleReason> },
    /// The lock was already ours (re-entrant acquire).
    AlreadyOurs,
    /// A live peer holds the lock — we yield to it.
    Yield { holder_pid: i32, version: String },
}

/// Acquire the lock, take over a stale one, or yield to a live peer — the
/// composed handover decision.
///
/// 1. Try an exclusive-create [`acquire`]. Success ⇒ [`LockOutcome::Acquired`]
///    with `took_over: None`.
/// 2. On contention, [`evaluate_holder`] (with the [`RETRY_ATTEMPTS`] = `Lur`
///    start-time budget):
///    * a live *other* daemon ⇒ [`LockOutcome::Yield`];
///    * our own pid ⇒ [`LockOutcome::AlreadyOurs`];
///    * a stale holder ⇒ [`write_lock_takeover`]; if the read-back confirms we
///      won ⇒ `Acquired { took_over: Some(reason) }`, otherwise we lost the race
///      and re-evaluate to report the peer we lost to (`Yield`), or, if the
///      winner already vanished, report the takeover anyway.
///    * `Absent` (the file disappeared between the failed `acquire` and the
///      read) ⇒ retry the takeover write.
pub fn acquire_or_yield<P: LockProbe>(
    runtime_dir: &Path,
    lock: &DaemonLock,
    probe: &P,
    sleep: &mut dyn FnMut(u64),
) -> std::io::Result<LockOutcome> {
    match acquire(runtime_dir, lock)? {
        AcquireRaw::Acquired => Ok(LockOutcome::Acquired { took_over: None }),
        AcquireRaw::Contended => {
            match evaluate_holder(runtime_dir, lock.pid, probe, RETRY_ATTEMPTS, sleep) {
                HolderEval::LiveSelf { .. } => Ok(LockOutcome::AlreadyOurs),
                HolderEval::LiveOther { pid, version } => Ok(LockOutcome::Yield {
                    holder_pid: pid,
                    version,
                }),
                HolderEval::Absent | HolderEval::Stale(_) => {
                    let reason = match evaluate_holder(
                        runtime_dir,
                        lock.pid,
                        probe,
                        RETRY_ATTEMPTS,
                        sleep,
                    ) {
                        HolderEval::Stale(r) => Some(r),
                        _ => None,
                    };
                    if write_lock_takeover(runtime_dir, lock)? {
                        Ok(LockOutcome::Acquired { took_over: reason })
                    } else {
                        // Lost the takeover race — report the live peer that won,
                        // or fall back to reporting the takeover if it vanished.
                        match evaluate_holder(
                            runtime_dir,
                            lock.pid,
                            probe,
                            RETRY_ATTEMPTS,
                            sleep,
                        ) {
                            HolderEval::LiveOther { pid, version } => Ok(LockOutcome::Yield {
                                holder_pid: pid,
                                version,
                            }),
                            HolderEval::LiveSelf { .. } => Ok(LockOutcome::AlreadyOurs),
                            _ => Ok(LockOutcome::Acquired { took_over: reason }),
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Yield/handover log + telemetry.
// ---------------------------------------------------------------------------

/// The supervisor log line for a skipped post-adopt sweep (binary: `bg: skipped
/// post-adopt sweeps + roster rewrite — daemon.lock is …`). `holder_pid` is
/// `Some` when a live peer holds the lock, `None` when it is absent.
#[must_use]
pub fn yield_skip_log(holder_pid: Option<i32>) -> String {
    let held = match holder_pid {
        Some(pid) => format!("held by pid {pid}"),
        None => "absent".to_string(),
    };
    format!(
        "bg: skipped post-adopt sweeps + roster rewrite — daemon.lock is {held} (yield/handover in flight)"
    )
}

/// Emit `tengu_daemon_yield` (binary `G("tengu_daemon_yield",{})`) — the
/// supervisor is standing down for a foreground/service daemon.
pub fn emit_yield() {
    tracing::info!(event = "tengu_daemon_yield");
}

/// Emit `tengu_daemon_yield_takeover` — a takeover that was requested but the
/// yielding holder still owns the lock after the handover deadline (binary:
/// "yield acked but lock still held after 5s … refusing").
pub fn emit_yield_takeover(acked: bool, held_after_ms: u64) {
    tracing::info!(
        event = "tengu_daemon_yield_takeover",
        acked,
        held_after_ms,
    );
}

/// Emit `tengu_daemon_lease` — a lease lifecycle transition (`"open"` /
/// `"close"`) on the held lock.
pub fn emit_lease(phase: &str) {
    tracing::info!(event = "tengu_daemon_lease", phase);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn tmpdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("lingxi-daemonlock-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// A no-op sleeper for tests that don't assert on backoff.
    fn no_sleep() -> impl FnMut(u64) {
        |_| {}
    }

    /// Test probe backed by explicit maps + a per-pid start-time *sequence* so
    /// the retry loop can be exercised. `proc_start` pops the next value from the
    /// pid's queue (repeating the last one once exhausted); each call is counted.
    struct FakeProbe {
        alive: HashMap<i32, bool>,
        daemon: HashMap<i32, bool>,
        start_seq: RefCell<HashMap<i32, Vec<Option<String>>>>,
        calls: RefCell<usize>,
    }
    impl FakeProbe {
        fn new() -> Self {
            FakeProbe {
                alive: HashMap::new(),
                daemon: HashMap::new(),
                start_seq: RefCell::new(HashMap::new()),
                calls: RefCell::new(0),
            }
        }
        fn alive(mut self, pid: i32) -> Self {
            self.alive.insert(pid, true);
            self.daemon.entry(pid).or_insert(true);
            self
        }
        fn not_daemon(mut self, pid: i32) -> Self {
            self.daemon.insert(pid, false);
            self
        }
        fn start(mut self, pid: i32, seq: Vec<Option<&str>>) -> Self {
            self.start_seq
                .borrow_mut()
                .insert(pid, seq.into_iter().map(|s| s.map(str::to_string)).collect());
            self
        }
    }
    impl LockProbe for FakeProbe {
        fn is_alive(&self, pid: i32) -> bool {
            *self.alive.get(&pid).unwrap_or(&false)
        }
        fn is_daemon_process(&self, pid: i32) -> bool {
            *self.daemon.get(&pid).unwrap_or(&true)
        }
        fn proc_start(&self, pid: i32, _skip_cache: bool) -> Option<String> {
            *self.calls.borrow_mut() += 1;
            let mut seqs = self.start_seq.borrow_mut();
            match seqs.get_mut(&pid) {
                Some(q) if q.len() > 1 => q.remove(0),
                Some(q) => q.first().cloned().flatten(),
                None => None,
            }
        }
    }

    fn sample_lock(pid: i32, proc_start: Option<&str>) -> DaemonLock {
        let mut l = DaemonLock::new(pid, "2.1.201");
        l.started_at = 1_700_000_000_000;
        l.proc_start = proc_start.map(str::to_string);
        l.json_path = Some("/run/daemon.json".to_string());
        l.log_path = Some("/run/daemon.log".to_string());
        l
    }

    // ---- paths ----------------------------------------------------------

    #[test]
    fn lock_and_tmp_paths() {
        let dir = Path::new("/run/lx");
        assert_eq!(lock_path(dir), Path::new("/run/lx/daemon.lock"));
        assert_eq!(
            lock_tmp_path(dir, 4242, 1700),
            Path::new("/run/lx/daemon.lock.tmp.4242.1700")
        );
    }

    // ---- acquire / read / release --------------------------------------

    #[test]
    fn acquire_is_exclusive_create() {
        let dir = tmpdir();
        let lock = sample_lock(9000, Some("Mon Jan 1 2024"));
        assert_eq!(acquire(&dir, &lock).unwrap(), AcquireRaw::Acquired);
        // Second acquire (even a different pid) sees the existing lock.
        let other = sample_lock(9001, Some("Tue Jan 2 2024"));
        assert_eq!(acquire(&dir, &other).unwrap(), AcquireRaw::Contended);

        // The file holds the *first* writer's payload, 2-space pretty, no
        // bgDisabled key (default false).
        let body = std::fs::read_to_string(lock_path(&dir)).unwrap();
        assert!(body.contains("  \"pid\": 9000"), "2-space pretty: {body}");
        assert!(!body.contains("bgDisabled"), "bgDisabled stripped: {body}");
        assert!(body.contains("\"procStart\""));

        let got = read_lock(&dir).unwrap();
        assert_eq!(got.pid, 9000);
        assert_eq!(got.version, "2.1.201");
        assert_eq!(got, lock);
    }

    #[test]
    fn read_lock_absent_is_none() {
        let dir = tmpdir();
        assert!(read_lock(&dir).is_none());
    }

    #[test]
    fn read_lock_rejects_wrong_shape() {
        let dir = tmpdir();
        // valid JSON but pid is a string, version missing → not a lock.
        std::fs::write(lock_path(&dir), br#"{"pid":"nope"}"#).unwrap();
        assert!(read_lock(&dir).is_none());
        // pid ok but version missing.
        std::fs::write(lock_path(&dir), br#"{"pid":5}"#).unwrap();
        assert!(read_lock(&dir).is_none());
        // both present → parses.
        std::fs::write(lock_path(&dir), br#"{"pid":5,"version":"2.1.201"}"#).unwrap();
        let l = read_lock(&dir).unwrap();
        assert_eq!(l.pid, 5);
        assert_eq!(l.started_at, 0); // defaulted
    }

    #[test]
    fn read_lock_rejects_and_removes_directory() {
        let dir = tmpdir();
        std::fs::create_dir_all(lock_path(&dir)).unwrap();
        assert!(read_lock(&dir).is_none());
        assert!(!lock_path(&dir).exists(), "non-regular lock is rm -rf'd");
    }

    #[test]
    fn read_lock_rejects_and_removes_oversize() {
        let dir = tmpdir();
        let f = std::fs::File::create(lock_path(&dir)).unwrap();
        f.set_len(MAX_LOCK_BYTES + 1).unwrap();
        drop(f);
        assert!(read_lock(&dir).is_none());
        assert!(!lock_path(&dir).exists(), "oversize lock is removed");
    }

    #[test]
    fn read_lock_rejects_invalid_json() {
        let dir = tmpdir();
        std::fs::write(lock_path(&dir), b"{ not json ]").unwrap();
        assert!(read_lock(&dir).is_none());
    }

    #[test]
    fn release_is_idempotent() {
        let dir = tmpdir();
        let lock = sample_lock(9000, None);
        acquire(&dir, &lock).unwrap();
        assert!(lock_path(&dir).exists());
        release(&dir).unwrap();
        assert!(!lock_path(&dir).exists());
        // second release on an absent lock is a no-op (ENOENT swallowed).
        release(&dir).unwrap();
    }

    #[test]
    fn bg_disabled_serializes_only_when_true() {
        let dir = tmpdir();
        let mut lock = sample_lock(9000, None);
        lock.bg_disabled = true;
        acquire(&dir, &lock).unwrap();
        let body = std::fs::read_to_string(lock_path(&dir)).unwrap();
        assert!(body.contains("\"bgDisabled\": true"), "{body}");
        let got = read_lock(&dir).unwrap();
        assert!(got.bg_disabled);
    }

    // ---- takeover -------------------------------------------------------

    #[test]
    fn takeover_replaces_existing_lock() {
        let dir = tmpdir();
        let stale = sample_lock(1111, Some("old"));
        acquire(&dir, &stale).unwrap();

        let mut mine = sample_lock(2222, Some("new"));
        mine.started_at = 1_800_000_000_000;
        assert!(write_lock_takeover(&dir, &mine).unwrap(), "we won the readback");

        let got = read_lock(&dir).unwrap();
        assert_eq!(got.pid, 2222);
        assert_eq!(got.started_at, 1_800_000_000_000);
        // no temp file left behind.
        assert!(!lock_tmp_path(&dir, 2222, 1_800_000_000_000).exists());
    }

    // ---- cmdline classification ----------------------------------------

    #[test]
    fn cmdline_unreadable_assumed_ours() {
        assert!(classify_cmdline(None));
    }

    #[test]
    fn cmdline_matches_title() {
        let argv = vec![DAEMON_PROC_TITLE.to_string(), "x".to_string()];
        assert!(classify_cmdline(Some(&argv)));
    }

    #[test]
    fn cmdline_matches_daemon_subcommand_in_argv_1_to_3() {
        // node /path/to/lingxi daemon --flag  →  daemon at index 2 (within 1..4)
        let argv = vec![
            "/usr/bin/node".to_string(),
            "/opt/lingxi".to_string(),
            "daemon".to_string(),
            "--flag".to_string(),
        ];
        assert!(classify_cmdline(Some(&argv)));
    }

    #[test]
    fn cmdline_rejects_unrelated_process() {
        let argv = vec![
            "/usr/bin/vim".to_string(),
            "notes.txt".to_string(),
        ];
        assert!(!classify_cmdline(Some(&argv)));
    }

    #[test]
    fn cmdline_daemon_outside_window_is_rejected() {
        // "daemon" only appears at index 4 (outside slice(1,4)) → not matched.
        let argv = vec![
            "node".to_string(),
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "daemon".to_string(),
        ];
        assert!(!classify_cmdline(Some(&argv)));
    }

    // ---- start-time retry loop -----------------------------------------

    #[test]
    fn start_time_missing_expected_is_ok() {
        let probe = FakeProbe::new().alive(9000);
        let mut sleeper = no_sleep();
        // expected None → true, no probe calls.
        assert!(start_time_matches(&probe, 9000, None, RETRY_ATTEMPTS, &mut sleeper));
        assert_eq!(*probe.calls.borrow(), 0);
    }

    #[test]
    fn start_time_matches_first_try() {
        let probe = FakeProbe::new().alive(9000).start(9000, vec![Some("S")]);
        let mut sleeps = Vec::new();
        let mut sleeper = |ms: u64| sleeps.push(ms);
        assert!(start_time_matches(
            &probe,
            9000,
            Some("S"),
            RETRY_ATTEMPTS,
            &mut sleeper
        ));
        assert_eq!(*probe.calls.borrow(), 1, "returns on first readable value");
        assert!(sleeps.is_empty(), "no backoff before the first probe");
    }

    #[test]
    fn start_time_retries_then_reads_mismatch() {
        // First probe returns None (not readable yet) → retry after backoff, then
        // reads a value that does NOT match → false.
        let probe = FakeProbe::new()
            .alive(9000)
            .start(9000, vec![None, Some("DIFFERENT")]);
        let mut sleeps = Vec::new();
        let mut sleeper = |ms: u64| sleeps.push(ms);
        assert!(!start_time_matches(
            &probe,
            9000,
            Some("S"),
            RETRY_ATTEMPTS,
            &mut sleeper
        ));
        assert_eq!(*probe.calls.borrow(), 2, "probed twice");
        assert_eq!(sleeps, vec![RETRY_BACKOFF_MS], "one backoff between the two tries");
    }

    #[test]
    fn start_time_exhausts_all_none() {
        // Never readable across all attempts → false, backoff on each retry.
        let probe = FakeProbe::new()
            .alive(9000)
            .start(9000, vec![None, None]);
        let mut sleeps = Vec::new();
        let mut sleeper = |ms: u64| sleeps.push(ms);
        assert!(!start_time_matches(
            &probe,
            9000,
            Some("S"),
            RETRY_ATTEMPTS,
            &mut sleeper
        ));
        assert_eq!(*probe.calls.borrow(), RETRY_ATTEMPTS);
        assert_eq!(sleeps.len(), RETRY_ATTEMPTS - 1);
    }

    // ---- holder evaluation ---------------------------------------------

    #[test]
    fn holder_absent() {
        let dir = tmpdir();
        let probe = FakeProbe::new();
        let mut s = no_sleep();
        assert_eq!(
            evaluate_holder(&dir, 1, &probe, RETRY_ATTEMPTS, &mut s),
            HolderEval::Absent
        );
    }

    #[test]
    fn holder_live_other_yields() {
        let dir = tmpdir();
        acquire(&dir, &sample_lock(5555, Some("S"))).unwrap();
        let probe = FakeProbe::new().alive(5555).start(5555, vec![Some("S")]);
        let mut s = no_sleep();
        match evaluate_holder(&dir, 1, &probe, RETRY_ATTEMPTS, &mut s) {
            HolderEval::LiveOther { pid, version } => {
                assert_eq!(pid, 5555);
                assert_eq!(version, "2.1.201");
            }
            other => panic!("expected LiveOther, got {other:?}"),
        }
    }

    #[test]
    fn holder_live_self() {
        let dir = tmpdir();
        acquire(&dir, &sample_lock(5555, Some("S"))).unwrap();
        let probe = FakeProbe::new().alive(5555).start(5555, vec![Some("S")]);
        let mut s = no_sleep();
        assert_eq!(
            evaluate_holder(&dir, 5555, &probe, RETRY_ATTEMPTS, &mut s),
            HolderEval::LiveSelf { pid: 5555 }
        );
    }

    #[test]
    fn holder_stale_dead_pid() {
        let dir = tmpdir();
        acquire(&dir, &sample_lock(5555, Some("S"))).unwrap();
        let probe = FakeProbe::new(); // 5555 not alive
        let mut s = no_sleep();
        assert_eq!(
            evaluate_holder(&dir, 1, &probe, RETRY_ATTEMPTS, &mut s),
            HolderEval::Stale(StaleReason::Dead)
        );
    }

    #[test]
    fn holder_stale_not_daemon() {
        let dir = tmpdir();
        acquire(&dir, &sample_lock(5555, Some("S"))).unwrap();
        let probe = FakeProbe::new().alive(5555).not_daemon(5555);
        let mut s = no_sleep();
        assert_eq!(
            evaluate_holder(&dir, 1, &probe, RETRY_ATTEMPTS, &mut s),
            HolderEval::Stale(StaleReason::NotDaemon)
        );
    }

    #[test]
    fn holder_stale_recycled_pid() {
        let dir = tmpdir();
        acquire(&dir, &sample_lock(5555, Some("S-old"))).unwrap();
        // alive + daemon, but live start-time differs from the recorded one.
        let probe = FakeProbe::new()
            .alive(5555)
            .start(5555, vec![Some("S-NEW")]);
        let mut s = no_sleep();
        assert_eq!(
            evaluate_holder(&dir, 1, &probe, RETRY_ATTEMPTS, &mut s),
            HolderEval::Stale(StaleReason::RecycledPid)
        );
    }

    // ---- acquire_or_yield state machine --------------------------------

    #[test]
    fn acquire_or_yield_fresh() {
        let dir = tmpdir();
        let probe = FakeProbe::new();
        let mut s = no_sleep();
        let out = acquire_or_yield(&dir, &sample_lock(100, Some("S")), &probe, &mut s).unwrap();
        assert_eq!(out, LockOutcome::Acquired { took_over: None });
        assert_eq!(read_lock(&dir).unwrap().pid, 100);
    }

    #[test]
    fn acquire_or_yield_yields_to_live_peer() {
        let dir = tmpdir();
        acquire(&dir, &sample_lock(5555, Some("S"))).unwrap();
        let probe = FakeProbe::new().alive(5555).start(5555, vec![Some("S")]);
        let mut s = no_sleep();
        let out = acquire_or_yield(&dir, &sample_lock(100, Some("mine")), &probe, &mut s).unwrap();
        assert_eq!(
            out,
            LockOutcome::Yield {
                holder_pid: 5555,
                version: "2.1.201".to_string()
            }
        );
        // We did NOT clobber the peer's lock.
        assert_eq!(read_lock(&dir).unwrap().pid, 5555);
    }

    #[test]
    fn acquire_or_yield_takes_over_stale_dead() {
        let dir = tmpdir();
        acquire(&dir, &sample_lock(5555, Some("S"))).unwrap();
        let probe = FakeProbe::new(); // 5555 dead
        let mut mine = sample_lock(100, Some("mine"));
        mine.started_at = 1_900_000_000_000;
        let mut s = no_sleep();
        let out = acquire_or_yield(&dir, &mine, &probe, &mut s).unwrap();
        assert_eq!(
            out,
            LockOutcome::Acquired {
                took_over: Some(StaleReason::Dead)
            }
        );
        let got = read_lock(&dir).unwrap();
        assert_eq!(got.pid, 100);
        assert_eq!(got.started_at, 1_900_000_000_000);
    }

    #[test]
    fn acquire_or_yield_takes_over_recycled_pid() {
        let dir = tmpdir();
        acquire(&dir, &sample_lock(5555, Some("S-old"))).unwrap();
        let probe = FakeProbe::new()
            .alive(5555)
            .start(5555, vec![Some("S-new")]);
        let mut s = no_sleep();
        let out = acquire_or_yield(&dir, &sample_lock(100, Some("mine")), &probe, &mut s).unwrap();
        assert_eq!(
            out,
            LockOutcome::Acquired {
                took_over: Some(StaleReason::RecycledPid)
            }
        );
        assert_eq!(read_lock(&dir).unwrap().pid, 100);
    }

    #[test]
    fn acquire_or_yield_already_ours() {
        let dir = tmpdir();
        acquire(&dir, &sample_lock(100, Some("S"))).unwrap();
        let probe = FakeProbe::new().alive(100).start(100, vec![Some("S")]);
        let mut s = no_sleep();
        // Same pid re-acquires → recognises its own lock.
        let out = acquire_or_yield(&dir, &sample_lock(100, Some("S")), &probe, &mut s).unwrap();
        assert_eq!(out, LockOutcome::AlreadyOurs);
    }

    // ---- upgrade check --------------------------------------------------

    #[test]
    fn holder_needs_upgrade_detects_version_skew() {
        let dir = tmpdir();
        let mut old = sample_lock(5555, Some("S"));
        old.version = "2.1.200".to_string();
        acquire(&dir, &old).unwrap();
        let probe = FakeProbe::new().alive(5555).start(5555, vec![Some("S")]);
        let mut s = no_sleep();
        assert!(holder_needs_upgrade(&dir, "2.1.201", &probe, RETRY_ATTEMPTS, &mut s));
        // same version → no upgrade needed.
        assert!(!holder_needs_upgrade(&dir, "2.1.200", &probe, RETRY_ATTEMPTS, &mut s));
    }

    #[test]
    fn holder_needs_upgrade_false_when_absent() {
        let dir = tmpdir();
        let probe = FakeProbe::new();
        let mut s = no_sleep();
        assert!(!holder_needs_upgrade(&dir, "2.1.201", &probe, RETRY_ATTEMPTS, &mut s));
    }

    // ---- log line -------------------------------------------------------

    #[test]
    fn yield_skip_log_variants() {
        assert_eq!(
            yield_skip_log(Some(4242)),
            "bg: skipped post-adopt sweeps + roster rewrite — daemon.lock is held by pid 4242 (yield/handover in flight)"
        );
        assert_eq!(
            yield_skip_log(None),
            "bg: skipped post-adopt sweeps + roster rewrite — daemon.lock is absent (yield/handover in flight)"
        );
    }
}
