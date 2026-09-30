//! Background-agent supervisor **roster** — schema, corruption-quarantine
//! read/write, and a PID-reuse-safe adoption check.
//!
//! This is the first self-contained slice of the real 2.1.201 background-agent
//! daemon (the supervisor that owns `daemon.lock`, an auth'd `control.sock`,
//! and a fleet of PTY workers). The daemon persists the live worker fleet to
//! `roster.json` so a freshly-launched CLI — or a supervisor that took over on
//! upgrade — can **re-adopt** still-running workers instead of orphaning them.
//!
//! Ported 1:1 from the 2.1.201 binary (module `lXe`/`G1`, @220279533):
//!
//! * **Envelope** (`BVl` / `$tn`) — `{proto, supervisorPid, updatedAt,
//!   workers}` where `workers` is a `Record<shortId, WorkerRecord>` keyed by
//!   an 8-hex `short` id. [`Roster`].
//! * **Worker record** (`UVl`, a zod *looseObject* → unknown keys preserved)
//!   — modeled field-for-field in [`WorkerRecord`] / [`Dispatch`].
//! * **Read** (`Dq`) — `lstat` guards (`is not a regular file — removing` /
//!   `too large (N bytes) — quarantining`), `JSON.parse` then schema-validate,
//!   with every failure path quarantining the file and emitting
//!   `tengu_bg_roster_parse_failed`. [`read_roster`].
//! * **Quarantine** (`jur`) — rename to `roster.json.corrupt.<millis>`.
//!   [`quarantine`].
//! * **Write** (`Tef`) — `mkdir` `0o700`, atomic pretty-JSON write `0o600`,
//!   swallowing transient FS errnos. [`write_roster`].
//! * **Adoption** (`AO` + `aE` + `IHd`) — a stored `procStart` (the process's
//!   `ps -o lstart=` string) is compared against the **live** start-time of
//!   the same pid; a matching-pid-but-different-start-time record is a
//!   **recycled PID** and must be dropped rather than adopted. [`adopt`].
//!
//! NOTE: this module is deliberately *not* wired into the live `agents`
//! command — it is the data/logic primitive the rest of the daemon (lock
//! handover, `control.sock` + `rvAuth`/`ptyAuth`, respawn/stall watchdog,
//! upgrade takeover, orphan reap) will build on. See the module's residual
//! plan in the porting notes.

use crate::background_launch::LaunchSpecRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const ROSTER_LOCK_FILE: &str = ".locks/roster.lock";

/// Wire protocol version the daemon speaks (binary `Ip`).
pub const PROTO: u32 = 2;
/// Minimum acceptable protocol version (binary `Zen`).
pub const PROTO_MIN: u32 = 1;
/// Maximum roster file size before it is treated as corrupt (binary `yef` =
/// 8 MiB). Anything larger is quarantined, not parsed.
pub const MAX_ROSTER_BYTES: u64 = 8_388_608;

/// Marker file written inside each managed worktree to pin ownership metadata.
pub const WORKTREE_OWNERSHIP_MARKER: &str = ".lingxi-worktree-owner.json";
/// Owned marker schema version.
pub const WORKTREE_OWNERSHIP_MARKER_SCHEMA_VERSION: u32 = 2;

/// Managed-worktree ownership metadata persisted in the worktree root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeOwnershipMarker {
    pub schema_version: u32,
    pub short: String,
    pub session_id: String,
    pub ownership_token: String,
    pub canonical_worktree_path: String,
    pub created_at_millis: i64,
}

impl WorktreeOwnershipMarker {
    fn new(
        short: &str,
        session_id: &str,
        token: &str,
        worktree_path: &Path,
    ) -> std::io::Result<Self> {
        Ok(Self {
            schema_version: WORKTREE_OWNERSHIP_MARKER_SCHEMA_VERSION,
            short: short.to_string(),
            session_id: session_id.to_string(),
            ownership_token: token.to_string(),
            canonical_worktree_path: canonicalize_managed_worktree_path(worktree_path)?
                .display()
                .to_string(),
            created_at_millis: now_millis(),
        })
    }
}

/// Canonicalize a managed-worktree path without accepting a leaf symlink.
pub fn canonicalize_managed_worktree_path(worktree_path: &Path) -> std::io::Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(worktree_path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "managed worktree path is not a real directory",
        ));
    }
    std::fs::canonicalize(worktree_path)
}

/// Canonical path of a managed-worktree ownership marker file.
#[must_use]
pub fn worktree_ownership_marker_path(worktree_path: &Path) -> PathBuf {
    worktree_path.join(WORKTREE_OWNERSHIP_MARKER)
}

/// Persist an owner-only marker next to a managed worktree.
pub fn write_worktree_ownership_marker(
    worktree_path: &Path,
    short: &str,
    session_id: &str,
    token: &str,
) -> std::io::Result<()> {
    use std::io::Write as _;

    let path = worktree_ownership_marker_path(worktree_path);
    let marker = WorktreeOwnershipMarker::new(short, session_id, token, worktree_path)?;
    let raw =
        serde_json::to_vec_pretty(&marker).map_err(|err| std::io::Error::other(err.to_string()))?;
    let tmp = worktree_path.join(format!(
        "{WORKTREE_OWNERSHIP_MARKER}.tmp.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut file = platform_pty::create_current_user_private_file(&tmp)?;
        file.write_all(&raw)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&path, perms)?;
    }
    Ok(())
}

/// Read a managed-worktree ownership marker.
pub fn read_worktree_ownership_marker(
    worktree_path: &Path,
) -> std::io::Result<WorktreeOwnershipMarker> {
    let path = worktree_ownership_marker_path(worktree_path);
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str(&raw).map_err(|err| std::io::Error::other(err.to_string()))
}

/// `roster.json` under the daemon runtime dir (binary `cae()` = `lae()`/
/// `roster.json`).
#[must_use]
pub fn roster_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("roster.json")
}

pub fn lock_roster(
    runtime_dir: &Path,
) -> std::io::Result<lingxi_core::host::rooted_fs::RootedFileLock> {
    lingxi_core::host::rooted_fs::lock_exclusive(
        runtime_dir,
        Path::new(ROSTER_LOCK_FILE),
        0o700,
        0o600,
    )
    .map_err(|error| std::io::Error::other(error.to_string()))
}

pub fn with_roster_lock<T>(
    runtime_dir: &Path,
    f: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let _lock = lock_roster(runtime_dir)?;
    f()
}

// ---------------------------------------------------------------------------
// Schema — modeled 1:1 with the 2.1.201 zod schemas (`BVl`/`UVl`/`Tcr`).
// ---------------------------------------------------------------------------

/// Roster envelope (`BVl`). Field order matches the schema so the pretty-JSON
/// write is byte-faithful: `proto, supervisorPid, updatedAt, workers`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Roster {
    /// Protocol version (`proto`, int in `[PROTO_MIN, PROTO]`).
    pub proto: u32,
    /// PID of the supervisor that last wrote this roster.
    #[serde(rename = "supervisorPid")]
    pub supervisor_pid: i32,
    /// Epoch-millis of the last write.
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    /// Live workers keyed by 8-hex `short` id.
    pub workers: BTreeMap<String, WorkerRecord>,

    /// Runtime-only flag set when the on-disk roster was unreadable/corrupt and
    /// a fresh empty envelope is being returned in its place. Mirrors the
    /// binary's transient `parseFailed` property, which is `delete`d before any
    /// write — so it is **never** serialized.
    #[serde(skip)]
    pub parse_failed: bool,
}

/// One background worker (`UVl`). A zod *looseObject*: any keys the daemon did
/// not model are preserved verbatim through [`extra`](WorkerRecord::extra).
/// Field order matches the schema declaration order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerRecord {
    /// OS process id of the worker.
    pub pid: i32,
    /// `ps -o lstart=` string captured at spawn — the PID-reuse guard. Absent
    /// on records written before start-time capture was available.
    #[serde(rename = "procStart", skip_serializing_if = "Option::is_none")]
    pub proc_start: Option<String>,
    /// Session UUID the worker is driving.
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// Rendezvous socket path.
    #[serde(rename = "rendezvousSock")]
    pub rendezvous_sock: String,
    /// PTY socket path (absent until the worker has a PTY).
    #[serde(rename = "ptySock", skip_serializing_if = "Option::is_none")]
    pub pty_sock: Option<String>,
    /// Messaging socket path.
    #[serde(rename = "messagingSock", skip_serializing_if = "Option::is_none")]
    pub messaging_sock: Option<String>,
    /// CLI version that spawned the worker (used to detect version skew).
    #[serde(rename = "cliVersion", skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    /// Epoch-millis the worker started.
    #[serde(rename = "startedAt")]
    pub started_at: i64,
    /// Respawn attempt counter.
    pub attempt: i64,
    /// Worker working directory.
    pub cwd: String,
    /// Worktree path for `isolation: "worktree"` workers.
    #[serde(rename = "worktreePath", skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// The dispatch request that launched this worker.
    pub dispatch: Dispatch,
    /// `"upgrade"` when a respawn is pending because of a version handover.
    #[serde(rename = "pendingRespawn", skip_serializing_if = "Option::is_none")]
    pub pending_respawn: Option<PendingRespawn>,
    /// DEC private-mode set to restore on reattach.
    #[serde(rename = "decModes", skip_serializing_if = "Option::is_none")]
    pub dec_modes: Option<Vec<i64>>,
    /// Rendezvous-socket auth token (`rvAuth`).
    #[serde(rename = "rvAuth", skip_serializing_if = "Option::is_none")]
    pub rv_auth: Option<String>,
    /// PTY-socket auth token (`ptyAuth`).
    #[serde(rename = "ptyAuth", skip_serializing_if = "Option::is_none")]
    pub pty_auth: Option<String>,

    /// Unmodeled keys, preserved (zod `looseObject`).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `pendingRespawn` is a single-variant literal today (`"upgrade"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PendingRespawn {
    /// Respawn queued because the supervisor took over on a version upgrade.
    Upgrade,
}

/// Dispatch request (`Tcr`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Dispatch {
    /// Protocol version.
    pub proto: u32,
    /// 8-hex short id.
    pub short: String,
    /// 8-hex nonce (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    /// Session UUID.
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// Epoch-millis the dispatch was created.
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    /// What triggered the dispatch. Invalid/missing values fall back to
    /// [`DispatchSource::Fleet`] (zod `.catch("fleet")`).
    #[serde(default, deserialize_with = "de_source_catch")]
    pub source: DispatchSource,
    /// Working directory.
    pub cwd: String,
    /// How the worker is launched (discriminated on `mode`).
    pub launch: Launch,
    /// Owner-only durable launch context. Protocol-v1 records omit this and
    /// are migrated on first worker load.
    #[serde(
        rename = "launchSpec",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub launch_spec: Option<LaunchSpecRef>,
    /// Extra environment (default `{}`).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Environment to re-apply on reattach.
    #[serde(rename = "reattachEnv", skip_serializing_if = "Option::is_none")]
    pub reattach_env: Option<BTreeMap<String, String>>,
    /// Worktree binding (path + ownership token).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<Worktree>,
    /// Isolation mode (default `"none"`).
    #[serde(default)]
    pub isolation: Isolation,
    /// Flags to pass on respawn (default `[]`).
    #[serde(rename = "respawnFlags", default)]
    pub respawn_flags: Vec<String>,
    /// How many attach-stall respawns have happened.
    #[serde(
        rename = "attachStallRespawns",
        skip_serializing_if = "Option::is_none"
    )]
    pub attach_stall_respawns: Option<i64>,
    /// Agent type driving the worker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Routine that scheduled the worker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routine: Option<String>,
    /// Seed intent/name shown before the worker produces output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<Seed>,
    /// Terminal columns hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cols: Option<u32>,
    /// Terminal rows hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u32>,
}

/// Dispatch trigger (`source` enum, `.catch("fleet")`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DispatchSource {
    /// Launched from an interactive shell session.
    Shell,
    /// Launched from a slash command.
    Slash,
    /// Launched from the fleet view (the default fallback).
    #[default]
    Fleet,
    /// Claimed from the spare pool.
    Spare,
    /// A respawn of an earlier worker.
    Respawn,
}

/// Isolation mode (`isolation` enum, default `"none"`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    /// Runs in the dispatch cwd directly.
    #[default]
    None,
    /// Runs in a dedicated git worktree.
    Worktree,
}

/// Launch mode (`launch`, discriminated union on `mode`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Launch {
    /// Fresh prompt run.
    Prompt {
        /// CLI args (the prompt words + flags).
        args: Vec<String>,
    },
    /// Resume/fork of an existing session.
    Resume {
        /// Session to resume.
        #[serde(rename = "sessionId")]
        session_id: String,
        /// Transcript to seed from.
        #[serde(rename = "transcriptPath", skip_serializing_if = "Option::is_none")]
        transcript_path: Option<String>,
        /// Whether this forks a copy rather than continuing in place.
        fork: bool,
        /// Extra flag args.
        #[serde(rename = "flagArgs")]
        flag_args: Vec<String>,
    },
    /// One-shot exec (`claude exec`-style).
    Exec {
        /// The command binary.
        cmd: String,
        /// Command args.
        args: Vec<String>,
    },
}

/// Worktree binding for isolated dispatches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Worktree {
    /// Worktree path.
    pub path: String,
    /// Ownership token that gates who may reap the worktree.
    #[serde(rename = "ownershipToken")]
    pub ownership_token: String,
}

/// Seed shown before a worker produces its first output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Seed {
    /// One-line intent.
    pub intent: String,
    /// Optional display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// zod `.catch("fleet")` semantics: any string that is not one of the known
/// variants — and a missing field — resolves to [`DispatchSource::Fleet`].
fn de_source_catch<'de, D>(d: D) -> Result<DispatchSource, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Deserialize as an arbitrary Value so a WRONG-TYPED source (e.g. `42`) is
    // coerced to fleet like zod `.catch("fleet")`, rather than erroring the whole
    // record (which would quarantine the roster + orphan every worker).
    let raw = serde_json::Value::deserialize(d)?;
    Ok(match raw.as_str() {
        Some("shell") => DispatchSource::Shell,
        Some("slash") => DispatchSource::Slash,
        Some("spare") => DispatchSource::Spare,
        Some("respawn") => DispatchSource::Respawn,
        // "fleet", any unknown string, null, or a non-string value → fleet.
        _ => DispatchSource::Fleet,
    })
}

// ---------------------------------------------------------------------------
// Empty envelope (`$tn`).
// ---------------------------------------------------------------------------

#[must_use]
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A fresh empty roster owned by `supervisor_pid` (binary `$tn`).
#[must_use]
pub fn empty_roster(supervisor_pid: i32) -> Roster {
    Roster {
        proto: PROTO,
        supervisor_pid,
        updated_at: now_millis(),
        workers: BTreeMap::new(),
        parse_failed: false,
    }
}

// ---------------------------------------------------------------------------
// Read + corruption quarantine (`Dq` / `jur` / `WJl`).
// ---------------------------------------------------------------------------

/// Why a roster read produced an empty envelope instead of on-disk data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseFailure {
    /// The path existed but was not a regular file — it was removed (`EFTYPE`).
    NotRegularFile,
    /// The file was larger than [`MAX_ROSTER_BYTES`] — quarantined (`E2BIG`).
    TooLarge {
        /// Observed size in bytes.
        bytes: u64,
    },
    /// Read or `JSON.parse` failed — quarantined.
    ReadOrParse {
        /// Errno name (`ENOENT`-excluded) or `"parse"` for a JSON syntax error.
        code: String,
    },
    /// The JSON was well-formed but failed schema validation — quarantined.
    Schema {
        /// Number of `workers` entries that were orphaned by the drop.
        orphaned: usize,
        /// Dotted path of the first schema issue (redacted like the binary).
        issue_path: String,
    },
}

/// Outcome of [`read_roster`].
#[derive(Debug, Clone, PartialEq)]
pub enum ReadOutcome {
    /// A parsed (or freshly-empty-because-absent) roster.
    Ok(Roster),
    /// The file was corrupt; a fresh empty roster is returned in its place and
    /// the bad file has been quarantined/removed. `parse_failed` is set on the
    /// returned roster.
    Corrupt {
        /// The replacement empty roster (with `parse_failed = true`).
        roster: Roster,
        /// What went wrong.
        failure: ParseFailure,
    },
}

impl ReadOutcome {
    /// The roster in either case (the caller usually just wants the data).
    #[must_use]
    pub fn into_roster(self) -> Roster {
        match self {
            ReadOutcome::Ok(r) | ReadOutcome::Corrupt { roster: r, .. } => r,
        }
    }
}

/// Count `workers` entries in a raw JSON value without schema-validating it
/// (binary `WJl`): used to report how many workers a corrupt roster orphaned.
#[must_use]
fn count_workers(raw: &serde_json::Value) -> usize {
    raw.get("workers")
        .and_then(serde_json::Value::as_object)
        .map_or(0, serde_json::Map::len)
}

/// Keys that survive verbatim in a `tengu_bg_roster_parse_failed` `issuePath`;
/// any other path segment is redacted to `*` (binary `_ef`).
fn issue_path_redacted(path: &[String]) -> String {
    const KNOWN: &[&str] = &[
        "proto",
        "supervisorPid",
        "updatedAt",
        "workers",
        "pid",
        "procStart",
        "sessionId",
        "rendezvousSock",
        "ptySock",
        "messagingSock",
        "rvAuth",
        "ptyAuth",
        "cliVersion",
        "startedAt",
        "attempt",
        "cwd",
        "worktreePath",
        "dispatch",
        "pendingRespawn",
        "decModes",
        "short",
        "nonce",
        "createdAt",
        "cols",
        "rows",
        "source",
        "launch",
        "mode",
        "args",
        "fork",
        "flagArgs",
        "cmd",
        "env",
        "reattachEnv",
        "worktree",
        "path",
        "ownershipToken",
        "isolation",
        "respawnFlags",
        "seed",
        "intent",
        "name",
        "agent",
        "routine",
        "attachStallRespawns",
    ];
    path.iter()
        .map(|seg| {
            if KNOWN.contains(&seg.as_str()) {
                seg.clone()
            } else {
                "*".to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Rename a corrupt roster aside to `roster.json.corrupt.<millis>` (binary
/// `jur`). Best-effort — a rename failure is swallowed exactly like the binary
/// (`.catch(He)`).
pub fn quarantine(path: &Path) {
    let target = format!("{}.corrupt.{}", path.to_string_lossy(), now_millis().max(0));
    let _ = std::fs::rename(path, &target);
}

/// Read + validate `roster.json`, quarantining/removing corrupt files
/// (binary `Dq`). `emit_events` mirrors the binary's `!silent` gate: when
/// `false`, no `tengu_bg_roster_parse_failed` telemetry is emitted (the
/// binary passes `{silent:true}` from status readers).
///
/// Returns [`ReadOutcome::Ok`] for a valid — or simply absent — roster, and
/// [`ReadOutcome::Corrupt`] (with a fresh empty roster) for anything corrupt.
#[must_use]
pub fn read_roster(runtime_dir: &Path, supervisor_pid: i32, emit_events: bool) -> ReadOutcome {
    let path = roster_path(runtime_dir);

    // Stage 0: lstat guards (`is not a regular file` / `too large`).
    match std::fs::symlink_metadata(&path) {
        Ok(meta) => {
            if !meta.is_file() {
                if emit_events {
                    emit_parse_failed(-1, 1, "EFTYPE", None, None);
                }
                // Not a regular file → remove, don't quarantine.
                let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
                return corrupt(supervisor_pid, ParseFailure::NotRegularFile);
            }
            if meta.len() > MAX_ROSTER_BYTES {
                if emit_events {
                    emit_parse_failed(-1, 1, "E2BIG", None, None);
                }
                quarantine(&path);
                return corrupt(supervisor_pid, ParseFailure::TooLarge { bytes: meta.len() });
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Fresh install — no roster yet is NOT a parse failure.
            return ReadOutcome::Ok(empty_roster(supervisor_pid));
        }
        Err(e) => {
            // lstat failed for some other reason — treat as read failure.
            let code = errno_name(&e);
            if emit_events {
                emit_parse_failed(-1, 1, &code, None, None);
            }
            quarantine(&path);
            return corrupt(supervisor_pid, ParseFailure::ReadOrParse { code });
        }
    }

    // Stage 1: read bytes.
    let bytes = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Raced with removal between lstat and read — fresh empty.
            return ReadOutcome::Ok(empty_roster(supervisor_pid));
        }
        Err(e) => {
            let code = errno_name(&e);
            if emit_events {
                emit_parse_failed(-1, 1, &code, None, None);
            }
            quarantine(&path);
            return corrupt(supervisor_pid, ParseFailure::ReadOrParse { code });
        }
    };

    // Stage 2: `JSON.parse`.
    let raw: serde_json::Value = match serde_json::from_str(&bytes) {
        Ok(v) => v,
        Err(_) => {
            if emit_events {
                emit_parse_failed(-1, 1, "parse", None, None);
            }
            quarantine(&path);
            return corrupt(
                supervisor_pid,
                ParseFailure::ReadOrParse {
                    code: "parse".to_string(),
                },
            );
        }
    };

    // Stage 3: schema validation.
    match serde_json::from_value::<Roster>(raw.clone()) {
        Ok(mut roster) => {
            // Envelope `proto` must be in [PROTO_MIN, PROTO] (zod
            // `.int().min(PROTO_MIN).max(PROTO)`). An out-of-range proto (e.g. a
            // future 2.2.x supervisor's proto:2, or a corrupt proto:0) is a schema
            // failure → quarantine + fresh roster, NOT adopt the incompatible fleet.
            if roster.proto < PROTO_MIN || roster.proto > PROTO {
                let orphaned = count_workers(&raw);
                if emit_events {
                    emit_parse_failed(orphaned as i64, 1, "schema", Some("proto"), None);
                }
                quarantine(&path);
                return corrupt(
                    supervisor_pid,
                    ParseFailure::Schema {
                        orphaned,
                        issue_path: "proto".to_string(),
                    },
                );
            }
            roster.parse_failed = false;
            ReadOutcome::Ok(roster)
        }
        Err(e) => {
            let orphaned = count_workers(&raw);
            let issue_path = schema_issue_path(&e);
            if emit_events {
                emit_parse_failed(orphaned as i64, 1, "schema", Some(&issue_path), None);
            }
            quarantine(&path);
            corrupt(
                supervisor_pid,
                ParseFailure::Schema {
                    orphaned,
                    issue_path,
                },
            )
        }
    }
}

fn corrupt(supervisor_pid: i32, failure: ParseFailure) -> ReadOutcome {
    let mut roster = empty_roster(supervisor_pid);
    roster.parse_failed = true;
    ReadOutcome::Corrupt { roster, failure }
}

/// Best-effort errno name for the `errCode` telemetry field.
fn errno_name(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(libc_errno) => format!("errno:{libc_errno}"),
        None => format!("{:?}", e.kind()),
    }
}

/// Recover a redacted dotted issue-path from a serde error message. serde does
/// not expose the structured path zod does, so we surface the leading key it
/// reports (best-effort), redacted through [`issue_path_redacted`].
fn schema_issue_path(e: &serde_json::Error) -> String {
    // serde messages look like `missing field `pid` at line .. column ..`.
    let msg = e.to_string();
    let key = msg
        .split('`')
        .nth(1)
        .map(str::to_string)
        .unwrap_or_default();
    if key.is_empty() {
        String::new()
    } else {
        issue_path_redacted(&[key])
    }
}

/// Emit `tengu_bg_roster_parse_failed` (binary `G("tengu_bg_roster_parse_failed", …)`).
/// The wire field names are camelCase (`orphaned`, `quarantined`, `errCode`,
/// `issuePath`, `issueCode`); tracing renders them snake_case per this crate's
/// convention.
fn emit_parse_failed(
    orphaned: i64,
    quarantined: i64,
    err_code: &str,
    issue_path: Option<&str>,
    issue_code: Option<&str>,
) {
    tracing::info!(
        event = "tengu_bg_roster_parse_failed",
        orphaned,
        quarantined,
        err_code,
        issue_path = issue_path.unwrap_or_default(),
        issue_code = issue_code.unwrap_or_default(),
    );
}

// ---------------------------------------------------------------------------
// Write (`Tef`).
// ---------------------------------------------------------------------------

/// errnos the binary treats as transient on roster write (`AG` set): the write
/// is logged and swallowed rather than propagated. Modeled on `ErrorKind` plus
/// the common storage-exhaustion raw errnos (`ENOSPC` 28, `EROFS` 30, `EDQUOT`
/// 69/122) that don't map to a stable `ErrorKind` on this toolchain.
fn is_transient_write_errno(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{AlreadyExists, Interrupted, NotFound, PermissionDenied, WouldBlock};
    if matches!(
        e.kind(),
        NotFound | PermissionDenied | AlreadyExists | Interrupted | WouldBlock
    ) {
        return true;
    }
    // ENOSPC / EROFS / EDQUOT across Linux + macOS.
    matches!(e.raw_os_error(), Some(28) | Some(30) | Some(69) | Some(122))
}

/// Persist a roster atomically (binary `Tef`): `mkdir` the parent `0o700`,
/// write `roster.json` as 2-space pretty JSON with mode `0o600` via a
/// temp-file rename, dropping the transient `parseFailed` flag. Transient FS
/// errnos are swallowed (logged); anything else is returned.
pub fn write_roster(runtime_dir: &Path, roster: &Roster) -> std::io::Result<()> {
    with_roster_lock(runtime_dir, || write_roster_unlocked(runtime_dir, roster))
}

pub fn write_roster_with_lock_held(runtime_dir: &Path, roster: &Roster) -> std::io::Result<()> {
    write_roster_unlocked(runtime_dir, roster)
}

fn write_roster_unlocked(runtime_dir: &Path, roster: &Roster) -> std::io::Result<()> {
    let path = roster_path(runtime_dir);
    // mkdir -p 0o700.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        set_mode(parent, 0o700);
    }

    // `De(n,null,2)` — 2-space pretty. `parse_failed` is `#[serde(skip)]` so it
    // never appears, matching the binary's `delete n.parseFailed`.
    let body = match serde_json::to_string_pretty(roster) {
        Ok(b) => b,
        Err(e) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
    };

    match atomic_write(&path, body.as_bytes(), 0o600) {
        Ok(()) => Ok(()),
        Err(e) if is_transient_write_errno(&e) => {
            tracing::error!("[daemon] roster write failed: {}", errno_name(&e));
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Atomic write via a sibling temp file + rename, chmod'd to `mode`.
fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write as _;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("roster.json");
    let tmp = path.with_file_name(format!(
        ".{file_name}.tmp.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        // Creation-time owner-only protection matters on Windows as well as
        // Unix: roster.json contains the bearer token for a live PTY.  The
        // platform helper supplies a protected current-user DACL on Windows
        // and CREATE_NEW+0600 on Unix, avoiding an inherited-ACL exposure
        // window before the atomic rename.
        let mut file = platform_pty::create_current_user_private_file(&tmp)?;
        set_mode(&tmp, mode);
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        // A failed write or fsync is just as terminal as a failed rename. Do
        // not leave a sibling temp file containing the private roster behind.
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

// ---------------------------------------------------------------------------
// PID-reuse-safe adoption (`AO` + `aE` + `IHd`).
// ---------------------------------------------------------------------------

/// Probes a pid's liveness and start-time. Abstracted so adoption is testable
/// without spawning real processes; [`SystemProbe`] is the production impl.
pub trait ProcProbe {
    /// Whether `pid` is a live process (binary `aE`: `pid > 1` and
    /// `kill(pid, 0)` succeeds — `EPERM` still means alive).
    fn is_alive(&self, pid: i32) -> bool;
    /// The process's `ps -o lstart=` start-time string (binary `IHd`), or
    /// `None` if it could not be read.
    fn start_time(&self, pid: i32) -> Option<String>;
}

/// Production probe: `kill(pid, 0)` for liveness, `ps -o lstart=` for
/// start-time (`LC_ALL=C`, `TZ=UTC`, trimmed — matching the binary exactly).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProbe;

impl ProcProbe for SystemProbe {
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

    #[cfg(windows)]
    fn is_alive(&self, pid: i32) -> bool {
        windows_process_alive(pid)
    }

    #[cfg(not(any(unix, windows)))]
    fn is_alive(&self, pid: i32) -> bool {
        pid > 1
    }

    fn start_time(&self, pid: i32) -> Option<String> {
        read_proc_start(pid)
    }
}

/// Read `ps -o lstart= -p <pid>` with `LC_ALL=C` and `TZ=UTC` (binary `IHd`).
/// Returns the trimmed start-time string, or `None` if `ps` failed / produced
/// no output.
#[must_use]
#[cfg(unix)]
pub fn read_proc_start(pid: i32) -> Option<String> {
    if pid <= 1 {
        return None;
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Query a Windows process creation timestamp for PID-reuse-safe adoption.
#[cfg(windows)]
#[must_use]
pub fn read_proc_start(pid: i32) -> Option<String> {
    if pid <= 1 {
        return None;
    }
    let script = format!(
        "(Get-Process -Id {pid} -ErrorAction Stop).StartTime.ToUniversalTime().ToString('o')"
    );
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(not(any(unix, windows)))]
#[must_use]
pub fn read_proc_start(_pid: i32) -> Option<String> {
    None
}

#[cfg(windows)]
fn windows_process_alive(pid: i32) -> bool {
    if pid <= 1 {
        return false;
    }
    let filter = format!("PID eq {pid}");
    let Ok(output) = std::process::Command::new("tasklist.exe")
        .args(["/FI", &filter, "/FO", "CSV", "/NH"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().any(|line| {
        let mut fields = line.split(',');
        let _image = fields.next();
        fields
            .next()
            .and_then(|field| field.trim().trim_matches('"').parse::<i32>().ok())
            == Some(pid)
    })
}

/// Why a worker was not adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropReason {
    /// The pid is no longer a live process.
    DeadPid,
    /// The pid is alive but its live start-time differs from the stored
    /// `procStart` — the PID was recycled onto an unrelated process.
    RecycledPid,
}

/// Decision for a single worker record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptDecision {
    /// Keep the worker — it is (or is assumed to still be) the same process.
    Adopt,
    /// Drop the worker for the given reason.
    Drop(DropReason),
}

/// The stored-vs-live start-time comparison (binary `AO`): a missing
/// `procStart`, or an unreadable live start-time, both resolve to *ok* (we
/// cannot prove the PID was recycled, so we do not drop on suspicion). Only a
/// definite mismatch rejects.
#[must_use]
pub fn start_time_ok<P: ProcProbe>(probe: &P, pid: i32, proc_start: Option<&str>) -> bool {
    let Some(stored) = proc_start else {
        return true;
    };
    match probe.start_time(pid) {
        None => true,
        Some(live) => live == stored,
    }
}

/// Decide whether a worker record should be adopted (`aE` + `AO` combined):
/// the pid must be alive **and** pass the start-time comparison. A live pid
/// whose start-time no longer matches the stored `procStart` is a recycled PID
/// and is dropped.
#[must_use]
pub fn adopt<P: ProcProbe>(probe: &P, record: &WorkerRecord) -> AdoptDecision {
    if !probe.is_alive(record.pid) {
        return AdoptDecision::Drop(DropReason::DeadPid);
    }
    if !start_time_ok(probe, record.pid, record.proc_start.as_deref()) {
        return AdoptDecision::Drop(DropReason::RecycledPid);
    }
    AdoptDecision::Adopt
}

/// Drop every non-adoptable worker from `roster`, returning the `(short,
/// reason)` pairs that were removed. Callers emit `tengu_bg_orphan_reap` /
/// `tengu_bg_roster_orphan_adopted` off the result.
pub fn retain_adoptable<P: ProcProbe>(roster: &mut Roster, probe: &P) -> Vec<(String, DropReason)> {
    let mut dropped = Vec::new();
    let shorts: Vec<String> = roster.workers.keys().cloned().collect();
    for short in shorts {
        // Guarded by the key list above.
        if let Some(rec) = roster.workers.get(&short) {
            if let AdoptDecision::Drop(reason) = adopt(probe, rec) {
                dropped.push((short.clone(), reason));
                roster.workers.remove(&short);
            }
        }
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tmpdir() -> PathBuf {
        // A process-wide monotonic counter — `now_millis()` alone collides when
        // parallel test threads land in the same millisecond (shared dir → race).
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("lingxi-roster-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn sample_worker(pid: i32, proc_start: Option<&str>) -> WorkerRecord {
        WorkerRecord {
            pid,
            proc_start: proc_start.map(str::to_string),
            session_id: "11111111-1111-1111-1111-111111111111".to_string(),
            rendezvous_sock: "/tmp/rv.sock".to_string(),
            pty_sock: None,
            messaging_sock: None,
            cli_version: Some("2.1.201".to_string()),
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
                source: DispatchSource::Fleet,
                cwd: "/work".to_string(),
                launch: Launch::Prompt {
                    args: vec!["hello".to_string()],
                },
                launch_spec: None,
                env: BTreeMap::new(),
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

    /// Test probe backed by explicit alive/start-time maps.
    struct FakeProbe {
        alive: HashMap<i32, bool>,
        start: HashMap<i32, String>,
    }
    impl ProcProbe for FakeProbe {
        fn is_alive(&self, pid: i32) -> bool {
            *self.alive.get(&pid).unwrap_or(&false)
        }
        fn start_time(&self, pid: i32) -> Option<String> {
            self.start.get(&pid).cloned()
        }
    }

    #[test]
    fn valid_roster_round_trips() {
        let dir = tmpdir();
        let mut roster = empty_roster(4242);
        roster.workers.insert(
            "abcd1234".to_string(),
            sample_worker(9000, Some("Mon Jan  1 00:00:00 2024")),
        );
        write_roster(&dir, &roster).unwrap();

        // pretty-JSON, 2-space indent, no `parseFailed` key.
        let body = std::fs::read_to_string(roster_path(&dir)).unwrap();
        assert!(
            body.contains(&format!("  \"proto\": {PROTO}")),
            "2-space pretty: {body}"
        );
        assert!(
            !body.contains("parseFailed"),
            "parseFailed stripped: {body}"
        );
        assert!(body.contains("\"procStart\""));

        let out = read_roster(&dir, 1, true);
        let got = out.into_roster();
        assert!(!got.parse_failed);
        assert_eq!(got.supervisor_pid, 4242);
        assert_eq!(got.workers.len(), 1);
        assert_eq!(got.workers["abcd1234"].pid, 9000);
        assert_eq!(got.workers, roster.workers);
    }

    #[test]
    fn absent_roster_is_fresh_not_parse_failed() {
        let dir = tmpdir();
        let out = read_roster(&dir, 77, true);
        match out {
            ReadOutcome::Ok(r) => {
                assert!(!r.parse_failed);
                assert_eq!(r.supervisor_pid, 77);
                assert!(r.workers.is_empty());
                assert_eq!(r.proto, PROTO);
            }
            other => panic!("expected fresh Ok, got {other:?}"),
        }
    }

    #[test]
    fn not_regular_file_is_removed() {
        let dir = tmpdir();
        // Make roster.json a *directory* → not a regular file.
        std::fs::create_dir_all(roster_path(&dir)).unwrap();
        let out = read_roster(&dir, 5, true);
        match out {
            ReadOutcome::Corrupt { roster, failure } => {
                assert!(roster.parse_failed);
                assert_eq!(failure, ParseFailure::NotRegularFile);
            }
            other => panic!("expected Corrupt/NotRegularFile, got {other:?}"),
        }
        // Removed, not quarantined.
        assert!(!roster_path(&dir).exists());
        assert!(!has_corrupt_sibling(&dir));
    }

    #[test]
    fn too_large_file_is_quarantined() {
        let dir = tmpdir();
        // Write a > MAX_ROSTER_BYTES file cheaply via set_len.
        let f = std::fs::File::create(roster_path(&dir)).unwrap();
        f.set_len(MAX_ROSTER_BYTES + 1).unwrap();
        drop(f);
        let out = read_roster(&dir, 5, true);
        match out {
            ReadOutcome::Corrupt { roster, failure } => {
                assert!(roster.parse_failed);
                assert_eq!(
                    failure,
                    ParseFailure::TooLarge {
                        bytes: MAX_ROSTER_BYTES + 1
                    }
                );
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        assert!(has_corrupt_sibling(&dir), "should have quarantined aside");
    }

    #[test]
    fn invalid_json_is_quarantined() {
        let dir = tmpdir();
        std::fs::write(roster_path(&dir), b"{ this is not json ]").unwrap();
        let out = read_roster(&dir, 5, true);
        match out {
            ReadOutcome::Corrupt { roster, failure } => {
                assert!(roster.parse_failed);
                assert_eq!(
                    failure,
                    ParseFailure::ReadOrParse {
                        code: "parse".to_string()
                    }
                );
            }
            other => panic!("expected ReadOrParse, got {other:?}"),
        }
        assert!(has_corrupt_sibling(&dir));
    }

    #[test]
    fn schema_invalid_json_reports_orphan_count() {
        let dir = tmpdir();
        // Well-formed JSON, two workers, but each worker is missing required
        // fields → schema validation fails; orphaned == 2.
        let raw = r#"{
            "proto": 1,
            "supervisorPid": 10,
            "updatedAt": 1,
            "workers": { "aaaaaaaa": {"pid": 1}, "bbbbbbbb": {"pid": 2} }
        }"#;
        std::fs::write(roster_path(&dir), raw).unwrap();
        let out = read_roster(&dir, 5, true);
        match out {
            ReadOutcome::Corrupt {
                roster,
                failure: ParseFailure::Schema { orphaned, .. },
            } => {
                assert!(roster.parse_failed);
                assert_eq!(orphaned, 2);
            }
            other => panic!("expected Schema orphaned=2, got {other:?}"),
        }
        assert!(has_corrupt_sibling(&dir));
    }

    #[test]
    fn count_workers_handles_non_objects() {
        assert_eq!(
            count_workers(&serde_json::json!({"workers": {"a": 1, "b": 2}})),
            2
        );
        assert_eq!(count_workers(&serde_json::json!({"workers": []})), 0);
        assert_eq!(count_workers(&serde_json::json!({"workers": 3})), 0);
        assert_eq!(count_workers(&serde_json::json!(42)), 0);
    }

    #[test]
    fn source_catch_falls_back_to_fleet() {
        // Unknown source string → fleet (zod `.catch`).
        let d: Dispatch = serde_json::from_value(serde_json::json!({
            "proto": 1, "short": "abcd1234",
            "sessionId": "s", "createdAt": 1,
            "source": "totally-bogus",
            "cwd": "/x",
            "launch": {"mode": "prompt", "args": []}
        }))
        .unwrap();
        assert_eq!(d.source, DispatchSource::Fleet);
        assert_eq!(d.isolation, Isolation::None); // default
        assert!(d.env.is_empty()); // default {}
    }

    #[test]
    fn source_catch_wrong_type_falls_back_to_fleet() {
        // A WRONG-TYPED source (integer) coerces to fleet like zod `.catch`,
        // instead of erroring the record (which would quarantine the roster).
        let d: Dispatch = serde_json::from_value(serde_json::json!({
            "proto": 1, "short": "abcd1234",
            "sessionId": "s", "createdAt": 1,
            "source": 42,
            "cwd": "/x",
            "launch": {"mode": "prompt", "args": []}
        }))
        .unwrap();
        assert_eq!(d.source, DispatchSource::Fleet);
    }

    #[test]
    fn out_of_range_proto_is_quarantined() {
        let dir = tmpdir();
        std::fs::write(
            roster_path(&dir),
            r#"{"proto":3,"supervisorPid":1,"updatedAt":1,"workers":{}}"#,
        )
        .unwrap();
        match read_roster(&dir, 5, false) {
            ReadOutcome::Corrupt {
                failure: ParseFailure::Schema { issue_path, .. },
                ..
            } => assert_eq!(issue_path, "proto"),
            other => panic!("expected Schema/proto corrupt, got {other:?}"),
        }
        // The out-of-range roster was quarantined, not left in place.
        assert!(!roster_path(&dir).exists());
    }

    #[test]
    fn adopt_drops_dead_pid() {
        let probe = FakeProbe {
            alive: HashMap::new(),
            start: HashMap::new(),
        };
        let rec = sample_worker(9000, Some("start-A"));
        assert_eq!(
            adopt(&probe, &rec),
            AdoptDecision::Drop(DropReason::DeadPid)
        );
    }

    #[test]
    fn adopt_keeps_matching_start_time() {
        let mut alive = HashMap::new();
        alive.insert(9000, true);
        let mut start = HashMap::new();
        start.insert(9000, "start-A".to_string());
        let probe = FakeProbe { alive, start };
        let rec = sample_worker(9000, Some("start-A"));
        assert_eq!(adopt(&probe, &rec), AdoptDecision::Adopt);
    }

    #[test]
    fn adopt_rejects_recycled_pid() {
        // pid alive but start-time differs → recycled PID, must drop.
        let mut alive = HashMap::new();
        alive.insert(9000, true);
        let mut start = HashMap::new();
        start.insert(9000, "start-DIFFERENT".to_string());
        let probe = FakeProbe { alive, start };
        let rec = sample_worker(9000, Some("start-A"));
        assert_eq!(
            adopt(&probe, &rec),
            AdoptDecision::Drop(DropReason::RecycledPid)
        );
    }

    #[test]
    fn adopt_keeps_when_procstart_missing() {
        // No stored procStart → cannot verify, do not drop on suspicion.
        let mut alive = HashMap::new();
        alive.insert(9000, true);
        let probe = FakeProbe {
            alive,
            start: HashMap::new(),
        };
        let rec = sample_worker(9000, None);
        assert_eq!(adopt(&probe, &rec), AdoptDecision::Adopt);
    }

    #[test]
    fn adopt_keeps_when_live_start_unreadable() {
        // pid alive, stored procStart present, but live start-time unknown →
        // ok (binary `AO` returns true on `undefined` live).
        let mut alive = HashMap::new();
        alive.insert(9000, true);
        let probe = FakeProbe {
            alive,
            start: HashMap::new(),
        };
        let rec = sample_worker(9000, Some("start-A"));
        assert_eq!(adopt(&probe, &rec), AdoptDecision::Adopt);
    }

    #[test]
    fn retain_adoptable_drops_dead_and_recycled_keeps_live() {
        let mut alive = HashMap::new();
        alive.insert(100, true); // live, matches
        alive.insert(200, true); // live, recycled
                                 // 300 dead (absent → false)
        let mut start = HashMap::new();
        start.insert(100, "S-100".to_string());
        start.insert(200, "S-OTHER".to_string());
        let probe = FakeProbe { alive, start };

        let mut roster = empty_roster(1);
        roster
            .workers
            .insert("aaaaaaaa".to_string(), sample_worker(100, Some("S-100")));
        roster
            .workers
            .insert("bbbbbbbb".to_string(), sample_worker(200, Some("S-200")));
        roster
            .workers
            .insert("cccccccc".to_string(), sample_worker(300, Some("S-300")));

        let dropped = retain_adoptable(&mut roster, &probe);
        assert_eq!(roster.workers.len(), 1);
        assert!(roster.workers.contains_key("aaaaaaaa"));
        // both non-live entries removed
        assert_eq!(dropped.len(), 2);
        let by_key: HashMap<_, _> = dropped.into_iter().collect();
        assert_eq!(by_key["bbbbbbbb"], DropReason::RecycledPid);
        assert_eq!(by_key["cccccccc"], DropReason::DeadPid);
    }

    #[test]
    fn empty_roster_fields() {
        let r = empty_roster(555);
        assert_eq!(r.proto, PROTO);
        assert_eq!(r.supervisor_pid, 555);
        assert!(r.workers.is_empty());
        assert!(!r.parse_failed);
    }

    fn has_corrupt_sibling(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten().any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .contains("roster.json.corrupt.")
                })
            })
            .unwrap_or(false)
    }
}
